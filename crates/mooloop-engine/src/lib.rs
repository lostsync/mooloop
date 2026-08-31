//! The non-realtime handle to the audio engine.
//!
//! Workflow:
//! ```no_run
//! let (engine, mut handle) = mooloop_engine::Engine::new(Default::default()).unwrap();
//! handle.send(mooloop_core::EngineCommand::Play);
//! while let Some(ev) = handle.poll() { /* update UI */ }
//! # let _ = engine;
//! ```
//!
//! `Engine` owns the JACK `AsyncClient` and must stay alive for as long as the
//! handle is used. Dropping `Engine` deactivates audio.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use arc_swap::{ArcSwap, ArcSwapOption};
use jack::{AudioOut, Client, ClientOptions, MidiIn};
use mooloop_core::{
    BufferParams, EffectKind, EffectParams, EffectTarget, EngineCommand, EngineEvent, MAX_CHANNELS,
    MAX_MODULATORS_PER_CHANNEL,
};
use mooloop_dsp::{
    buffer_allocation_key, build_effect_at_tempo, AudioNode, DryAlign, SampleData,
    SpectrumAnalyzer, SPECTRUM_BINS,
};
use rtrb::{Consumer, Producer};

mod driver;
mod graph;
mod meters;
mod offline;
mod render;
mod sequencer;
mod transport;

#[cfg(test)]
mod gain_structure_tests;

use graph::{AsyncClient, Graph};
use render::{ReclaimedEffect, RenderState};

pub use driver::{AudioConfig, DriverStatus, OutputTarget};
pub use meters::{BusMeters, DeviceMeters, DeviceTelemetry, ModulatorMeters, PlayheadMeters};
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
        kind: EffectKind,
        resource_key: Option<u64>,
        node: Box<dyn AudioNode + Send>,
        align: Option<Box<DryAlign>>,
        analyzer: Box<SpectrumAnalyzer>,
    },
    /// Replace an installed node only when the destination still contains the
    /// requested kind. Resource-backed devices use this after preparing new
    /// state on a worker, so a delayed result cannot overwrite a removed or
    /// reordered unrelated device.
    ReplaceEffect {
        target: EffectTarget,
        slot: u8,
        expected_kind: EffectKind,
        expected_resource_key: u64,
        resource_key: u64,
        node: Box<dyn AudioNode + Send>,
        align: Option<Box<DryAlign>>,
    },
    /// Remove whatever is at `slot`, if anything. Also reclaimed, not dropped.
    RemoveEffect { target: EffectTarget, slot: u8 },
}

/// GUI -> audio for the sample browser's audition voice. Owned here rather
/// than in `mooloop-core`'s `EngineCommand` because `SampleData` comes from
/// `mooloop-dsp`, which `mooloop-core` must not depend on.
pub enum PreviewCommand {
    /// Start (or restart) the preview voice with this decoded sample.
    Play { sample: Arc<SampleData> },
    /// Silence and release the preview voice, if one is playing.
    Stop,
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
    /// A sample whose browser preview finished or was replaced. Same
    /// ownership round trip as an effect node: the sample's last reference
    /// must not be dropped on the realtime thread.
    PreviewSample { sample: Arc<SampleData> },
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
// This ring buffer is preallocated. Boxing `EngineCommand` to shrink its
// elements would deallocate that box on the realtime callback.
#[allow(clippy::large_enum_variant)]
pub(crate) enum RealtimeCommand {
    Engine(EngineCommand),
    Structural(StructuralCommand),
    Preview(PreviewCommand),
    InstallProject(PreparedProject),
}

const QUEUE_CAPACITY: usize = 1024;
const CLIENT_NAME: &str = "mooloop";
const OUT_L_NAME: &str = "mooloop:out_l";
const OUT_R_NAME: &str = "mooloop:out_r";
const DEFAULT_OUTPUT_L: &str = "system:playback_1";
const DEFAULT_OUTPUT_R: &str = "system:playback_2";

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
    _client: Arc<AsyncClient>,
}

impl Engine {
    /// Open the JACK client (works against pipewire-jack transparently) and
    /// start the realtime thread. All channel devices and the pattern bank are
    /// pre-allocated to pool size; every channel starts with an empty sample
    /// slot until the user loads an audio file or a project assigns one.
    pub fn new(config: AudioConfig) -> Result<(Engine, EngineHandle), Error> {
        let (client, _status) = Client::new(CLIENT_NAME, ClientOptions::NO_START_SERVER)
            .map_err(|e| Error::ClientOpen(e.to_string()))?;
        let sample_rate = client.sample_rate();

        if let Some(frames) = config.buffer_size {
            if let Err(e) = client.set_buffer_size(frames) {
                mooloop_core::log_warn!(
                    "audio",
                    "could not set JACK buffer size to {frames} frames ({e}); \
                     leaving the server's current buffer size in place"
                );
            }
        }

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
        // One input for now. Per-channel MIDI routing is a later concern;
        // what the control layer needs first is any way in at all.
        let midi_in = client
            .register_port("midi_in", MidiIn::default())
            .map_err(|e| Error::PortRegister(e.to_string()))?;

        // Every channel's slot starts empty; a channel is silent until the
        // user loads a sample or a project assigns one.
        let sample_slots: Arc<Vec<Arc<ArcSwapOption<SampleData>>>> = Arc::new(
            (0..MAX_CHANNELS)
                .map(|_| Arc::new(ArcSwapOption::from(None)))
                .collect(),
        );

        let xrun_count = Arc::new(AtomicU64::new(0));
        let bus_meters = BusMeters::new();
        let device_meters = DeviceMeters::new();
        let device_telemetry = DeviceTelemetry::new();
        let playhead_meters = PlayheadMeters::new();
        let modulator_meters = ModulatorMeters::new();
        let preview_gain = Arc::new(AtomicU32::new(mooloop_core::gain::db_to_linear(mooloop_core::gain::REFERENCE_PEAK_DBFS).to_bits()));
        let buffer_midi_map: Arc<ArcSwapOption<mooloop_core::midi::BufferMidiMap>> =
            Arc::new(ArcSwapOption::empty());
        let mut render = RenderState::new(sample_rate, sample_slots.clone());
        render.attach_meters(bus_meters.clone());
        render.attach_device_meters(device_meters.clone());
        render.attach_device_telemetry(device_telemetry.clone());
        render.attach_playhead_meters(playhead_meters.clone());
        render.attach_modulator_meters(modulator_meters.clone());
        render.attach_buffer_midi_map(buffer_midi_map.clone());
        render.attach_preview_gain(preview_gain.clone());
        let io = graph::GraphIo {
            out_l,
            out_r,
            midi_in,
            cmd_rx,
            evt_tx,
            reclaim_tx,
        };
        let graph = Graph::new(io, Box::new(render), xrun_count.clone());

        let target = config
            .output_target
            .unwrap_or_else(|| (DEFAULT_OUTPUT_L.to_owned(), DEFAULT_OUTPUT_R.to_owned()));
        let output_target = Arc::new(ArcSwap::from_pointee(target.clone()));
        let auto_reconnect = Arc::new(AtomicBool::new(config.auto_reconnect));

        let async_client = client
            .activate_async(
                graph::Notifications {
                    xrun_count,
                    auto_reconnect: auto_reconnect.clone(),
                    target: output_target.clone(),
                },
                graph,
            )
            .map_err(|e| Error::Activate(e.to_string()))?;
        let async_client = Arc::new(async_client);

        // Best-effort: wire our outputs to the configured target so the app is
        // audible out of the box. Auto-reconnect (if enabled) picks this back
        // up whenever the JACK graph changes and this connection is missing.
        let c = async_client.as_client();
        let sources = [OUT_L_NAME, OUT_R_NAME];
        let destinations = [target.0.as_str(), target.1.as_str()];
        for (src, dst) in sources.iter().zip(destinations.iter()) {
            match c.connect_ports_by_name(src, dst) {
                Ok(()) | Err(jack::Error::PortAlreadyConnected(_, _)) => {}
                Err(e) => mooloop_core::log_warn!(
                    "audio",
                    "could not auto-connect {src} -> {dst} ({e}); \
                     connect it manually in a patchbay (e.g. qpwgraph, qjackctl, Helvum)"
                ),
            }
        }

        Ok((
            Engine {
                _client: async_client.clone(),
            },
            EngineHandle {
                cmd_tx,
                evt_rx,
                reclaim_rx,
                bus_meters,
                device_meters,
                device_telemetry,
                buffer_midi_map,
                playhead_meters,
                modulator_meters,
                sample_slots,
                sample_rate,
                install_generation: 0,
                client: async_client,
                output_target,
                auto_reconnect,
                preview_gain,
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
    device_telemetry: Arc<DeviceTelemetry>,
    buffer_midi_map: Arc<ArcSwapOption<mooloop_core::midi::BufferMidiMap>>,
    playhead_meters: Arc<PlayheadMeters>,
    modulator_meters: Arc<ModulatorMeters>,
    sample_slots: Arc<Vec<Arc<ArcSwapOption<SampleData>>>>,
    sample_rate: u32,
    install_generation: u64,
    client: Arc<AsyncClient>,
    output_target: Arc<ArcSwap<(String, String)>>,
    auto_reconnect: Arc<AtomicBool>,
    preview_gain: Arc<AtomicU32>,
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

    /// Prepare and publish a replacement for a retained-audio buffer. The
    /// ring is allocated here, on the control thread; the audio callback only
    /// swaps the prepared box at a block boundary and returns the old one for
    /// deferred destruction. `expected` keeps a stale config rebuild from
    /// replacing a buffer whose bars setting has since changed.
    pub fn replace_buffer(
        &mut self,
        target: EffectTarget,
        slot: u8,
        expected: BufferParams,
        next: BufferParams,
        bpm: f64,
    ) -> bool {
        let node = build_effect_at_tempo(EffectParams::Buffer(next), self.sample_rate, bpm);
        let align = DryAlign::new(node.dry_path_latency_frames()).map(Box::new);
        self.cmd_tx
            .push(RealtimeCommand::Structural(
                StructuralCommand::ReplaceEffect {
                    target,
                    slot,
                    expected_kind: EffectKind::Buffer,
                    expected_resource_key: buffer_allocation_key(expected),
                    resource_key: buffer_allocation_key(next),
                    node,
                    align,
                },
            ))
            .is_ok()
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
                StructuralReclaim::PreviewSample { sample } => drop(sample),
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

    /// Queue a preview-voice command. Non-blocking; drops on overflow.
    pub fn preview(&mut self, command: PreviewCommand) {
        let _ = self.cmd_tx.push(RealtimeCommand::Preview(command));
    }

    /// Sets the preview voice's linear output gain. Live: the voice reads
    /// the shared cell every block, so turning the knob is heard at once.
    pub fn set_preview_gain(&self, gain: f32) {
        self.preview_gain.store(gain.to_bits(), Ordering::Relaxed);
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
        // A project swap replaces the complete renderer. Reconnect every meter
        // transport before it reaches the audio thread: otherwise the new
        // renderer publishes into its private, unread arrays while the UI
        // continues to read the startup arrays forever.
        render.attach_device_meters(self.device_meters.clone());
        render.attach_device_telemetry(self.device_telemetry.clone());
        render.attach_buffer_midi_map(self.buffer_midi_map.clone());
        render.attach_playhead_meters(self.playhead_meters.clone());
        render.attach_modulator_meters(self.modulator_meters.clone());
        render.attach_preview_gain(self.preview_gain.clone());
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
            self.device_telemetry.clear_spectra();
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

    /// Subscribe an effect stage's input to compact spectrum telemetry. This
    /// is observation-only: it never participates in audio or modulation
    /// signal flow, and disabled stages do not run spectral analysis.
    pub fn set_effect_spectrum_enabled(&self, target: EffectTarget, slot: u8, enabled: bool) {
        self.device_telemetry.set_spectrum_enabled(
            effect_target_index(target),
            usize::from(slot) + 1,
            enabled,
        );
    }

    /// The latest normalized log-frequency spectrum for one effect input.
    /// The data is a display vector, not PCM; callers may poll it at their
    /// own frame rate without affecting the audio callback.
    pub fn effect_spectrum(&self, target: EffectTarget, slot: u8) -> [f32; SPECTRUM_BINS] {
        self.device_telemetry
            .read_spectrum(effect_target_index(target), usize::from(slot) + 1)
    }

    /// Install the MIDI mapping that drives a buffer insert, or clear it.
    /// Built and dropped on this thread; the audio thread only loads it.
    pub fn set_buffer_midi_map(&self, map: Option<mooloop_core::midi::BufferMidiMap>) {
        self.buffer_midi_map.store(map.map(Arc::new));
    }

    /// How many times a retained-audio buffer insert has been overtaken by
    /// its writer and force-returned to live. Monotonic since the device was
    /// installed, so a UI compares it against the value it last displayed.
    pub fn effect_buffer_collisions(&self, target: EffectTarget, slot: u8) -> u32 {
        self.device_telemetry
            .read_buffer_collisions(effect_target_index(target), usize::from(slot) + 1)
    }

    /// Every currently-active sampler voice's normalized playback position
    /// on `channel`, for a UI playhead. Wait-free; see `meters` for why this
    /// is a plain array read rather than another event.
    pub fn playhead_positions(&self, channel: usize) -> Vec<f32> {
        self.playhead_meters.read(channel)
    }

    /// The channel's modulator outputs as of the last control tick the audio
    /// thread ran. The UI resolves these against the channel's routes to draw
    /// each destination's live offset, rather than the engine publishing a
    /// value per parameter: a channel has at most
    /// `MAX_MODULATORS_PER_CHANNEL` sources but many more destinations.
    pub fn modulator_outputs(&self, channel: usize) -> [f32; MAX_MODULATORS_PER_CHANNEL] {
        self.modulator_meters.read(channel)
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Clone the currently-published sample pointer for non-realtime display
    /// work such as waveform peak generation.
    pub fn sample_snapshot(&self, channel: usize) -> Option<Arc<SampleData>> {
        self.sample_slots.get(channel)?.load_full()
    }

    /// JACK input ports grouped by owning client, as candidate output
    /// destinations. A non-realtime JACK graph query; call it when the audio
    /// preferences page opens or on an explicit refresh, not every frame.
    pub fn available_output_targets(&self) -> Vec<OutputTarget> {
        let jack_client = self.client.as_client();
        let ports = jack_client.ports(None, None, jack::PortFlags::IS_INPUT);
        let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
        for port in ports {
            let Some((client_name, _)) = port.split_once(':') else {
                continue;
            };
            match grouped.iter_mut().find(|(name, _)| name == client_name) {
                Some((_, ports)) => ports.push(port),
                None => grouped.push((client_name.to_owned(), vec![port])),
            }
        }
        grouped
            .into_iter()
            .filter_map(|(client, ports)| {
                let mut ports = ports.into_iter();
                let port_l = ports.next()?;
                let port_r = ports.next()?;
                Some(OutputTarget {
                    client,
                    port_l,
                    port_r,
                })
            })
            .collect()
    }

    /// Disconnect the previously configured output target (if connected) and
    /// connect the new one, or the system default if `target` is `None`.
    /// Non-realtime JACK calls; control-thread only.
    pub fn set_output_target(&mut self, target: Option<(String, String)>) -> Result<(), String> {
        let jack_client = self.client.as_client();
        let previous = self.output_target.load_full();
        let next =
            target.unwrap_or_else(|| (DEFAULT_OUTPUT_L.to_owned(), DEFAULT_OUTPUT_R.to_owned()));
        if *previous != next {
            let _ = jack_client.disconnect_ports_by_name(OUT_L_NAME, &previous.0);
            let _ = jack_client.disconnect_ports_by_name(OUT_R_NAME, &previous.1);
        }
        for (src, dst) in [(OUT_L_NAME, &next.0), (OUT_R_NAME, &next.1)] {
            match jack_client.connect_ports_by_name(src, dst) {
                Ok(()) | Err(jack::Error::PortAlreadyConnected(_, _)) => {}
                Err(e) => return Err(format!("could not connect {src} to {dst}: {e}")),
            }
        }
        self.output_target.store(Arc::new(next));
        Ok(())
    }

    /// Request a new JACK buffer size. This is server-wide: it changes the
    /// buffer for every JACK client connected, not only mooloop.
    pub fn set_buffer_size(&mut self, frames: u32) -> Result<(), String> {
        self.client
            .as_client()
            .set_buffer_size(frames)
            .map_err(|e| format!("could not change the JACK buffer size: {e}"))
    }

    /// Enable or disable retrying the configured output target when the JACK
    /// port graph changes and the target is currently unconnected.
    pub fn set_auto_reconnect(&mut self, enabled: bool) {
        self.auto_reconnect.store(enabled, Ordering::Relaxed);
    }

    /// Live driver state for populating the preferences dialog.
    pub fn driver_status(&self) -> DriverStatus {
        DriverStatus {
            sample_rate: self.sample_rate,
            buffer_size: self.client.as_client().buffer_size(),
            current_target: (*self.output_target.load_full()).clone(),
        }
    }
}

fn effect_target_index(target: EffectTarget) -> usize {
    match target {
        EffectTarget::Channel(channel) => usize::from(channel),
        EffectTarget::Bus(bus) => MAX_CHANNELS + usize::from(bus),
    }
}
