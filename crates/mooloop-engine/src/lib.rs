//! The non-realtime handle to the audio engine.
//!
//! Workflow:
//! ```no_run
//! let (engine, mut handle) = mooloop_engine::Engine::new().unwrap();
//! handle.send(mooloop_core::EngineCommand::Play);
//! while let Some(ev) = handle.poll() { /* update UI */ }
//! # let _ = engine;
//! ```
//!
//! `Engine` owns the JACK `AsyncClient` and must stay alive for as long as the
//! handle is used. Dropping `Engine` deactivates audio.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use arc_swap::ArcSwapOption;
use jack::{AudioOut, Client, ClientOptions};
use mooloop_core::{EffectTarget, EngineCommand, EngineEvent, MAX_CHANNELS};
use mooloop_dsp::{AudioNode, SampleData};
use rtrb::{Consumer, Producer};

mod graph;
mod offline;
mod render;
mod sequencer;
mod transport;

use graph::{AsyncClient, Graph};

pub use offline::{
    ExportError, ExportFormat, ExportSpec, Mp3Bitrate, OfflineRenderer, RenderScope, RenderSummary,
    WavEncoding,
};

/// GUI -> audio. Carries ownership of a heap-allocated effect node into the
/// realtime thread. This lives here rather than in `mooloop-core`'s
/// `EngineCommand` for two reasons: `Box` is not `Copy` (the command ring is
/// POD-only), and `AudioNode` comes from `mooloop-dsp`, which `mooloop-core`
/// must not depend on.
pub enum StructuralCommand {
    /// Install `node` at `slot` on `target`, replacing whatever was there.
    /// The replaced node (if any) comes back via the reclaim ring — the
    /// realtime thread never drops a `Box` itself (that would be a
    /// deallocation on the audio thread).
    InstallEffect {
        target: EffectTarget,
        slot: u8,
        node: Box<dyn AudioNode + Send>,
    },
    /// Remove whatever is at `slot`, if anything. Also reclaimed, not dropped.
    RemoveEffect { target: EffectTarget, slot: u8 },
}

/// audio -> GUI. Hands back a displaced node so the GUI thread can drop it
/// (deallocate) safely, off the realtime thread. Drained as a side effect of
/// `EngineHandle::poll` — there is nothing to inspect.
pub enum StructuralReclaim {
    Node(Box<dyn AudioNode + Send>),
}

const QUEUE_CAPACITY: usize = 1024;
const CLIENT_NAME: &str = "mooloop";
const OUT_L_NAME: &str = "mooloop:out_l";
const OUT_R_NAME: &str = "mooloop:out_r";

#[derive(Debug)]
pub enum Error {
    ClientOpen(String),
    PortRegister(String),
    Activate(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::ClientOpen(s) => write!(f, "failed to open JACK client: {s}"),
            Error::PortRegister(s) => write!(f, "failed to register JACK port: {s}"),
            Error::Activate(s) => write!(f, "failed to activate JACK client: {s}"),
        }
    }
}

impl std::error::Error for Error {}

/// Keep-alive guard for the audio engine. Dropping this shuts down JACK.
pub struct Engine {
    _client: AsyncClient,
}

impl Engine {
    /// Open the JACK client (works against pipewire-jack transparently) and
    /// start the realtime thread. All channel devices and the pattern bank are
    /// pre-allocated to pool size; every channel starts with a synthesised
    /// default kick so the app is audible before the user loads a WAV.
    pub fn new() -> Result<(Engine, EngineHandle), Error> {
        let (client, _status) = Client::new(CLIENT_NAME, ClientOptions::NO_START_SERVER)
            .map_err(|e| Error::ClientOpen(e.to_string()))?;
        let sample_rate = client.sample_rate();

        let (cmd_tx, cmd_rx): (Producer<EngineCommand>, Consumer<EngineCommand>) =
            rtrb::RingBuffer::new(QUEUE_CAPACITY);
        let (evt_tx, evt_rx): (Producer<EngineEvent>, Consumer<EngineEvent>) =
            rtrb::RingBuffer::new(QUEUE_CAPACITY);
        let (structural_tx, structural_rx): (
            Producer<StructuralCommand>,
            Consumer<StructuralCommand>,
        ) = rtrb::RingBuffer::new(QUEUE_CAPACITY);
        let (reclaim_tx, reclaim_rx): (Producer<StructuralReclaim>, Consumer<StructuralReclaim>) =
            rtrb::RingBuffer::new(QUEUE_CAPACITY);

        let out_l = client
            .register_port("out_l", AudioOut::default())
            .map_err(|e| Error::PortRegister(e.to_string()))?;
        let out_r = client
            .register_port("out_r", AudioOut::default())
            .map_err(|e| Error::PortRegister(e.to_string()))?;

        // Every channel's slot starts with the same shared default kick.
        let kick = SampleData::default_kick(sample_rate);
        let sample_slots: Arc<Vec<Arc<ArcSwapOption<SampleData>>>> = Arc::new(
            (0..MAX_CHANNELS)
                .map(|_| Arc::new(ArcSwapOption::from(Some(kick.clone()))))
                .collect(),
        );

        let xrun_count = Arc::new(AtomicU64::new(0));
        let project_slot = Arc::new(ArcSwapOption::from(Some(Arc::new(
            mooloop_core::Project::default(),
        ))));
        let io = graph::GraphIo {
            out_l,
            out_r,
            cmd_rx,
            evt_tx,
            structural_rx,
            reclaim_tx,
        };
        let graph = Graph::new(
            sample_rate,
            io,
            sample_slots.clone(),
            project_slot.clone(),
            xrun_count.clone(),
        );

        let async_client = client
            .activate_async(graph::Notifications { xrun_count }, graph)
            .map_err(|e| Error::Activate(e.to_string()))?;

        // Best-effort: wire our outputs to system playback so the app is
        // audible out of the box. User-configurable routing comes later.
        let c = async_client.as_client();
        let sources = [OUT_L_NAME, OUT_R_NAME];
        let destinations = ["system:playback_1", "system:playback_2"];
        for (src, dst) in sources.iter().zip(destinations.iter()) {
            match c.connect_ports_by_name(src, dst) {
                Ok(()) | Err(jack::Error::PortAlreadyConnected(_, _)) => {}
                Err(e) => eprintln!(
                    "mooloop: could not auto-connect {src} -> {dst} ({e}); \
                     connect it manually in a patchbay (e.g. qpwgraph, qjackctl, Helvum)"
                ),
            }
        }

        Ok((
            Engine {
                _client: async_client,
            },
            EngineHandle {
                cmd_tx,
                evt_rx,
                structural_tx,
                reclaim_rx,
                sample_slots,
                project_slot,
                sample_rate,
                install_generation: 0,
                retired_projects: Vec::new(),
            },
        ))
    }
}

/// The GUI's handle into the engine. Safe to keep on the UI thread; all
/// operations are lock-free.
pub struct EngineHandle {
    cmd_tx: Producer<EngineCommand>,
    evt_rx: Consumer<EngineEvent>,
    structural_tx: Producer<StructuralCommand>,
    reclaim_rx: Consumer<StructuralReclaim>,
    sample_slots: Arc<Vec<Arc<ArcSwapOption<SampleData>>>>,
    project_slot: Arc<ArcSwapOption<mooloop_core::Project>>,
    sample_rate: u32,
    install_generation: u64,
    retired_projects: Vec<(u64, Arc<mooloop_core::Project>)>,
}

impl EngineHandle {
    /// Queue a command for the audio thread. Non-blocking; drops on overflow
    /// (which should not happen at sane UI event rates).
    pub fn send(&mut self, cmd: EngineCommand) {
        let _ = self.cmd_tx.push(cmd);
    }

    /// Hand a heap-allocated structural change (effect install/remove) to the
    /// audio thread. Non-blocking; on overflow the command is dropped, which
    /// for `InstallEffect` drops the node back on this (GUI) thread.
    pub fn send_structural(&mut self, cmd: StructuralCommand) {
        let _ = self.structural_tx.push(cmd);
    }

    /// Pop one event if available.
    pub fn poll(&mut self) -> Option<EngineEvent> {
        // Reclaim displaced effect nodes first: dropping the boxes here frees
        // them off the realtime thread, which is the entire point of the
        // reclaim ring.
        while let Ok(StructuralReclaim::Node(node)) = self.reclaim_rx.pop() {
            drop(node);
        }
        loop {
            match self.evt_rx.pop().ok()? {
                EngineEvent::ProjectInstalled { generation } => {
                    self.retired_projects
                        .retain(|(retire_after, _)| *retire_after > generation);
                }
                event => return Some(event),
            }
        }
    }

    /// Drain all currently-queued events.
    pub fn drain(&mut self) -> impl Iterator<Item = EngineEvent> + '_ {
        std::iter::from_fn(|| self.poll())
    }

    /// Publish a freshly-decoded sample for `channel`. The realtime sampler
    /// picks it up on the next note-on. Wait-free; UI-thread safe.
    pub fn load_sample(&self, channel: usize, sample: Arc<SampleData>) {
        if let Some(slot) = self.sample_slots.get(channel) {
            slot.store(Some(sample));
        }
    }

    pub fn clear_sample(&self, channel: usize) {
        if let Some(slot) = self.sample_slots.get(channel) {
            slot.store(None);
        }
    }

    /// Publish a validated project snapshot and import it on the next block.
    /// Replaced snapshots remain owned here until the audio callback
    /// acknowledges the generation, so their allocations are reclaimed on
    /// this non-realtime thread.
    pub fn install_project(&mut self, project: Arc<mooloop_core::Project>) {
        self.install_generation = self
            .install_generation
            .checked_add(1)
            .expect("project install generation exhausted");
        if let Some(retired) = self.project_slot.swap(Some(project)) {
            self.retired_projects
                .push((self.install_generation, retired));
        }
        self.send(EngineCommand::InstallProject {
            generation: self.install_generation,
        });
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Clone the currently-published sample pointer for non-realtime display
    /// work such as waveform peak generation.
    pub fn sample_snapshot(&self, channel: usize) -> Option<Arc<SampleData>> {
        self.sample_slots.get(channel)?.load_full()
    }
}
