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
use mooloop_dsp::{AudioNode, DryAlign, SampleData};
use rtrb::{Consumer, Producer};

mod graph;
mod meters;
mod offline;
mod render;
mod sequencer;
mod transport;

use graph::{AsyncClient, Graph};
use render::{ReclaimedEffect, RenderState};

pub use meters::{BusMeters, DeviceMeters};
pub use offline::{
    ExportError, ExportFormat, ExportSpec, Mp3Bitrate, OfflineRenderer, RenderScope, RenderSummary,
    WavEncoding,
};

/// GUI -> audio. Carries ownership of a heap-allocated effect node into the
/// realtime thread. This lives here rather than in `mooloop-core`'s
/// `EngineCommand` because `AudioNode` comes from `mooloop-dsp`, which
/// `mooloop-core` must not depend on. `RealtimeCommand` carries this enum and
/// POD commands in one ordered engine-private stream.
pub enum StructuralCommand {
    /// Install `node` at `slot` on `target`, replacing whatever was there.
    /// `align` is the container's dry-path delay matching the node's reported
    /// latency, allocated on this thread for the same reason as the node.
    /// The replaced occupants (if any) come back via the reclaim ring — the
    /// realtime thread never drops a `Box` itself (that would be a
    /// deallocation on the audio thread).
    InstallEffect {
        target: EffectTarget,
        slot: u8,
        node: Box<dyn AudioNode + Send>,
        align: Option<Box<DryAlign>>,
    },
    /// Remove whatever is at `slot`, if anything. Also reclaimed, not dropped.
    RemoveEffect { target: EffectTarget, slot: u8 },
}

/// audio -> GUI. Hands back displaced occupants so the GUI thread can drop
/// (deallocate) them safely, off the realtime thread. Drained as a side
/// effect of `EngineHandle::poll` — there is nothing to inspect.
pub(crate) enum StructuralReclaim {
    Effect(ReclaimedEffect),
    /// A complete executor displaced by a project install. Keeping it boxed
    /// lets the realtime thread swap ownership without allocating; the box is
    /// destroyed when `EngineHandle::poll` drains this variant.
    RenderState(Box<RenderState>),
}

/// A project that has already been instantiated and allocated off the audio
/// thread. The realtime callback only swaps the box and acknowledges its
/// generation.
pub(crate) struct PreparedProject {
    pub generation: u64,
    pub render: Box<RenderState>,
}

/// The ordered control stream consumed at block boundaries. Project swaps
/// share this queue with value commands so edits before and after a load can
/// never cross the generation boundary.
pub(crate) enum RealtimeCommand {
    Engine(EngineCommand),
    Structural(StructuralCommand),
    InstallProject(PreparedProject),
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

        let (cmd_tx, cmd_rx): (Producer<RealtimeCommand>, Consumer<RealtimeCommand>) =
            rtrb::RingBuffer::new(QUEUE_CAPACITY);
        let (evt_tx, evt_rx): (Producer<EngineEvent>, Consumer<EngineEvent>) =
            rtrb::RingBuffer::new(QUEUE_CAPACITY);
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
        let bus_meters = BusMeters::new();
        let device_meters = DeviceMeters::new();
        let mut render = RenderState::new(sample_rate, sample_slots.clone());
        render.attach_meters(bus_meters.clone());
        render.attach_device_meters(device_meters.clone());
        let io = graph::GraphIo {
            out_l,
            out_r,
            cmd_rx,
            evt_tx,
            reclaim_tx,
        };
        let graph = Graph::new(io, Box::new(render), xrun_count.clone());

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
                reclaim_rx,
                bus_meters,
                device_meters,
                sample_slots,
                sample_rate,
                install_generation: 0,
            },
        ))
    }
}

/// The control thread's handle into the engine. Realtime communication is
/// bounded and non-blocking; project installation also performs allocation
/// and node construction here before publishing a prepared executor.
pub struct EngineHandle {
    cmd_tx: Producer<RealtimeCommand>,
    evt_rx: Consumer<EngineEvent>,
    reclaim_rx: Consumer<StructuralReclaim>,
    bus_meters: Arc<BusMeters>,
    device_meters: Arc<DeviceMeters>,
    sample_slots: Arc<Vec<Arc<ArcSwapOption<SampleData>>>>,
    sample_rate: u32,
    install_generation: u64,
}

impl EngineHandle {
    /// Queue a command for the audio thread. Non-blocking; drops on overflow
    /// (which should not happen at sane UI event rates).
    pub fn send(&mut self, cmd: EngineCommand) {
        let _ = self.cmd_tx.push(RealtimeCommand::Engine(cmd));
    }

    /// Hand a heap-allocated structural change (effect install/remove) to the
    /// audio thread. Non-blocking; on overflow the command is dropped, which
    /// for `InstallEffect` drops the node back on this (GUI) thread.
    pub fn send_structural(&mut self, cmd: StructuralCommand) {
        let _ = self.cmd_tx.push(RealtimeCommand::Structural(cmd));
    }

    /// Pop one event if available.
    pub fn poll(&mut self) -> Option<EngineEvent> {
        // Reclaim displaced effect occupants first: dropping the boxes here
        // frees them off the realtime thread, which is the entire point of
        // the reclaim ring.
        while let Ok(reclaim) = self.reclaim_rx.pop() {
            match reclaim {
                StructuralReclaim::Effect(effect) => drop(effect),
                StructuralReclaim::RenderState(render) => drop(render),
            }
        }
        loop {
            match self.evt_rx.pop().ok()? {
                EngineEvent::ProjectInstalled { .. } => {}
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

    /// Prepare a complete executor from a validated project on this
    /// non-realtime thread, then queue an ownership swap for the next block.
    /// The displaced executor returns through the reclaim ring and is dropped
    /// by `poll`, never by the audio callback.
    #[must_use]
    pub fn install_project(&mut self, project: Arc<mooloop_core::Project>) -> bool {
        let generation = self
            .install_generation
            .checked_add(1)
            .expect("project install generation exhausted");
        let mut render = RenderState::new(self.sample_rate, self.sample_slots.clone());
        render.attach_meters(self.bus_meters.clone());
        // A project swap replaces the complete renderer. Reconnect both meter
        // transports before it reaches the audio thread: otherwise the new
        // renderer publishes device peaks into its private, unread array while
        // the UI continues to read the startup array forever.
        render.attach_device_meters(self.device_meters.clone());
        render.load_project(&project);
        let prepared = PreparedProject {
            generation,
            render: Box::new(render),
        };
        // A full queue leaves `prepared` on this thread, so dropping it is
        // realtime-safe. Project loads are rare and the queue has the same
        // generous capacity as other engine control paths.
        if self
            .cmd_tx
            .push(RealtimeCommand::InstallProject(prepared))
            .is_ok()
        {
            self.install_generation = generation;
            true
        } else {
            false
        }
    }

    /// Read and clear one bus's held peak. Wait-free; see `meters` for why
    /// this is an atomic array rather than another event.
    pub fn take_bus_peak(&self, bus: usize) -> (f32, f32) {
        self.bus_meters.take(bus)
    }

    /// Read and clear a device's held input/output peaks. `target` addresses
    /// channels and buses in one space: a channel is its own index, a bus is
    /// `MAX_CHANNELS + bus index`. Stage 0 is the source; effect slots follow.
    pub fn take_device_peak(&self, target: usize, stage: usize) -> ((f32, f32), (f32, f32)) {
        self.device_meters.take(target, stage)
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
