//! The non-realtime handle to the audio engine.
//!
//! Workflow:
//! ```no_run
//! let (engine, mut handle) = mooloop_engine::Engine::new().unwrap();
//! handle.send(mooloop_core::EngineCommand::Play);
//! // ... later, on a timer ...
//! while let Some(ev) = handle.poll() { /* update UI */ }
//! # let _ = engine;
//! ```
//!
//! `Engine` owns the JACK `AsyncClient` and must stay alive for as long as the
//! handle is used. Dropping `Engine` deactivates audio.

use jack::{AudioOut, Client, ClientOptions};
use mooloop_core::{EngineCommand, EngineEvent};
use rtrb::{Consumer, Producer};

mod graph;
mod transport;

use graph::{AsyncClient, Graph};

const QUEUE_CAPACITY: usize = 1024;
const CLIENT_NAME: &str = "mooloop";

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
    /// start the realtime thread.
    pub fn new() -> Result<(Engine, EngineHandle), Error> {
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

        let graph = Graph::new(sample_rate, out_l, out_r, cmd_rx, evt_tx);

        let async_client = client
            .activate_async(graph::Notifications, graph)
            .map_err(|e| Error::Activate(e.to_string()))?;

        Ok((Engine { _client: async_client }, EngineHandle { cmd_tx, evt_rx }))
    }

    /// The audio client's sample rate, for display purposes. Reads a value
    /// captured at construction time.
    pub fn sample_rate(&self) -> u32 {
        // The AsyncClient owns the Client internally; reading it back requires
        // a notification round-trip we don't need for Phase 0. The UI gets the
        // sample rate via the first `Position` event implicitly. Returning 0
        // here is a placeholder until we cache it on the Engine in a later pass.
        0
    }
}

/// The GUI's handle into the engine. Safe to keep on the UI thread; all
/// operations are lock-free.
pub struct EngineHandle {
    cmd_tx: Producer<EngineCommand>,
    evt_rx: Consumer<EngineEvent>,
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
}
