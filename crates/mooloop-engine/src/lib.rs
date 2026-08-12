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

use std::sync::Arc;

use arc_swap::ArcSwapOption;
use jack::{AudioOut, Client, ClientOptions};
use mooloop_core::{EngineCommand, EngineEvent, SamplerParams};
use mooloop_dsp::SampleData;
use rtrb::{Consumer, Producer};

mod graph;
mod sequencer;
mod transport;

use graph::{AsyncClient, Graph};

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
    /// start the realtime thread. The returned `EngineHandle` owns the queues
    /// and the sample-publishing slot.
    pub fn new() -> Result<(Engine, EngineHandle), Error> {
        Self::with_params(SamplerParams::default())
    }

    /// Open the engine with an initial sampler parameter set and a default
    /// synthesised sample so the app is audible before the user loads a WAV.
    pub fn with_params(initial_params: SamplerParams) -> Result<(Engine, EngineHandle), Error> {
        let (client, _status) = Client::new(CLIENT_NAME, ClientOptions::NO_START_SERVER)
            .map_err(|e| Error::ClientOpen(e.to_string()))?;
        let sample_rate = client.sample_rate();

        let (cmd_tx, cmd_rx): (Producer<EngineCommand>, Consumer<EngineCommand>) =
            rtrb::RingBuffer::new(QUEUE_CAPACITY);
        let (evt_tx, evt_rx): (Producer<EngineEvent>, Consumer<EngineEvent>) =
            rtrb::RingBuffer::new(QUEUE_CAPACITY);

        let out_l = client
            .register_port("out_l", AudioOut::default())
            .map_err(|e| Error::PortRegister(e.to_string()))?;
        let out_r = client
            .register_port("out_r", AudioOut::default())
            .map_err(|e| Error::PortRegister(e.to_string()))?;

        let sample_slot: Arc<ArcSwapOption<SampleData>> =
            Arc::new(ArcSwapOption::from(Some(SampleData::default_kick(sample_rate))));

        let graph = Graph::new(
            sample_rate,
            out_l,
            out_r,
            cmd_rx,
            evt_tx,
            sample_slot.clone(),
            initial_params,
        );

        let async_client = client
            .activate_async(graph::Notifications, graph)
            .map_err(|e| Error::Activate(e.to_string()))?;

        // Best-effort: wire our outputs to system playback so Phase 0/1 is
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
                sample_slot,
            },
        ))
    }
}

/// The GUI's handle into the engine. Safe to keep on the UI thread; all
/// operations are lock-free.
pub struct EngineHandle {
    cmd_tx: Producer<EngineCommand>,
    evt_rx: Consumer<EngineEvent>,
    sample_slot: Arc<ArcSwapOption<SampleData>>,
}

impl EngineHandle {
    /// Queue a command for the audio thread. Non-blocking; drops on overflow
    /// (which should not happen at sane UI event rates).
    pub fn send(&mut self, cmd: EngineCommand) {
        let _ = self.cmd_tx.push(cmd);
    }

    /// Pop one event if available.
    pub fn poll(&mut self) -> Option<EngineEvent> {
        self.evt_rx.pop().ok()
    }

    /// Drain all currently-queued events.
    pub fn drain(&mut self) -> impl Iterator<Item = EngineEvent> + '_ {
        std::iter::from_fn(|| self.evt_rx.pop().ok())
    }

    /// Publish a freshly-decoded sample. The realtime sampler picks it up on
    /// the next note-on. Wait-free; safe to call from the UI thread.
    pub fn load_sample(&self, sample: Arc<SampleData>) {
        self.sample_slot.store(Some(sample));
    }

    /// A clone of the shared sample slot. Lets the UI publish samples without
    /// routing through the command queue (which only carries `Copy` payloads).
    pub fn sample_slot(&self) -> Arc<ArcSwapOption<SampleData>> {
        self.sample_slot.clone()
    }
}
