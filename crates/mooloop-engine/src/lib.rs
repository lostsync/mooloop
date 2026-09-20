//! The non-realtime handle to the audio engine.
//!
//! Workflow:
//! ```no_run
//! use mooloop_engine::CommandSink;
//! let (engine, mut handle) = mooloop_engine::Engine::new(Default::default()).unwrap();
//! let _ = handle.send(mooloop_core::EngineCommand::Play);
//! while let Some(ev) = handle.poll() { /* update UI */ }
//! # let _ = engine;
//! ```
//!
//! `Engine` keeps the audio driver alive and must outlive the handle's use.
//! Dropping it, with the handle, shuts the driver down.

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use mooloop_core::{
    BufferParams, EffectKind, EffectParams, EffectTarget, EngineCommand, EngineEvent, MAX_CHANNELS,
    modulation::CONTROL_SOURCE_SLOTS,
    CompiledBusGraph, DeviceKind,
};
use mooloop_dsp::{
    buffer_allocation_key, build_effect_at_tempo, AudioNode, ChannelAudioSnapshot, IntegerDelay,
    SampleData, SpectrumAnalyzer, StereoBus, StretchPool, SPECTRUM_BINS,
};
use rtrb::{Consumer, Producer};

/// A counting allocator, installed only for this crate's own tests.
///
/// Same instrument and same reason as `mooloop-session`'s: `block_cost` asks
/// what a prepared project *holds*, and resident set size cannot answer it,
/// because the allocator does not hand freed pages back to the OS.
///
/// It answers two different questions and they need different counters.
/// `live()` is a **process-wide** byte total, which is what "what does this
/// hold" wants and is why `block_cost` insists on `--test-threads=1`.
/// `allocations()` is a **per-thread** count of calls that never decreases,
/// which is what "did this allocate at all" wants: a net-zero figure cannot
/// see an allocation paired with a free in the same block, and that pair is
/// exactly what a `Vec` reallocating on the audio callback looks like.
///
/// `frees()` is the same shape for `dealloc`, and exists because a callback
/// can be clean of allocations and still free: dropping the last `Arc` of a
/// sample buffer on the audio thread allocates nothing and releases
/// megabytes. (`realloc` is not counted there; `allocations()` already sees
/// it.)
///
/// Per thread because the callback is one thread and the assertion is about
/// what *it* did. A process-wide count would be measuring the test harness.
#[cfg(test)]
pub(crate) struct CountingAllocator {
    live: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
thread_local! {
    /// Allocation calls made on this thread. `const`-initialised so that
    /// touching it from inside `alloc` cannot itself allocate, and read
    /// through `try_with` so a late allocation during TLS teardown returns
    /// rather than panicking.
    static ALLOCATION_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Deallocation calls made on this thread. Same construction, same
    /// reasons.
    static FREE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
impl CountingAllocator {
    pub(crate) fn live(&self) -> usize {
        self.live.load(Ordering::Relaxed)
    }

    /// How many times this thread has called the allocator.
    pub(crate) fn allocations(&self) -> usize {
        ALLOCATION_CALLS.try_with(|calls| calls.get()).unwrap_or(0)
    }

    /// How many times this thread has handed memory back to the allocator.
    pub(crate) fn frees(&self) -> usize {
        FREE_CALLS.try_with(|calls| calls.get()).unwrap_or(0)
    }
}

#[cfg(test)]
unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        self.live.fetch_add(layout.size(), Ordering::Relaxed);
        let _ = ALLOCATION_CALLS.try_with(|calls| calls.set(calls.get() + 1));
        unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        self.live.fetch_sub(layout.size(), Ordering::Relaxed);
        let _ = FREE_CALLS.try_with(|calls| calls.set(calls.get() + 1));
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(
        &self,
        ptr: *mut u8,
        layout: std::alloc::Layout,
        new_size: usize,
    ) -> *mut u8 {
        // Counted explicitly. The default `realloc` is alloc-copy-dealloc and
        // would be seen, but `System` overrides it with `mremap`, which is
        // the very thing a growing `Vec` does and would otherwise be
        // invisible to this counter.
        self.live.fetch_add(new_size, Ordering::Relaxed);
        self.live.fetch_sub(layout.size(), Ordering::Relaxed);
        let _ = ALLOCATION_CALLS.try_with(|calls| calls.set(calls.get() + 1));
        unsafe { std::alloc::System.realloc(ptr, layout, new_size) }
    }
}

#[cfg(test)]
#[global_allocator]
pub(crate) static COUNTING: CountingAllocator = CountingAllocator {
    live: std::sync::atomic::AtomicUsize::new(0),
};

/// What a driver calls one MIDI input that merges every hardware source.
///
/// **JACK gives mooloop one merged MIDI port**, with every keyboard
/// auto-connected to it, and a message arriving on it carries no record of
/// which one sent it. The picker therefore has one entry, named for what it
/// actually is rather than for a device, and two keyboards are told apart by
/// MIDI channel (`docs/CONTROL_SURFACES.md`).
///
/// Defined here rather than in `jack_driver`, and unconditionally, because
/// the interface says the same thing on the MIDI preferences page and a
/// second spelling of the name is exactly the copy that drifts. Core MIDI
/// connects per source, so nothing there ever carries it.
pub const MERGED_MIDI_IN_LABEL: &str = "All Hardware Inputs";

/// What the AUDIO menu calls the hardware input under JACK, where one stereo
/// pair is wired in the graph -- the audio twin of [`MERGED_MIDI_IN_LABEL`].
pub const AUDIO_IN_LABEL: &str = "Audio In";

#[cfg(target_os = "macos")]
mod coreaudio_driver;
mod driver;
mod executor;
#[cfg(not(target_os = "macos"))]
mod jack_driver;
pub mod load;
mod meters;
mod offline;
mod render;
mod sequencer;
mod take;
mod transport;

#[cfg(test)]
mod render_test_support;

#[cfg(test)]
mod audio_edge_tests;
#[cfg(test)]
mod block_cost;
#[cfg(test)]
mod buffer_workflow_tests;
#[cfg(test)]
mod container_tests;
#[cfg(test)]
mod console_tests;
#[cfg(test)]
mod ds01_tests;
#[cfg(test)]
mod gain_structure_tests;
#[cfg(test)]
mod idle_skip_tests;
#[cfg(test)]
mod strip_tests;
#[cfg(test)]
mod take_tests;

use executor::{Executor, ExecutorIo};
#[cfg(target_os = "macos")]
use coreaudio_driver::{CoreAudioDriver as Driver, Opening};
#[cfg(not(target_os = "macos"))]
use jack_driver::{JackDriver as Driver, Opening};
use render::{ReclaimedEffect, RenderState};
pub use render::{AudioTapBank, ChannelStorage, ContainerScratch, EffectSlot, SendBank, SendSpec};

pub use driver::{AudioConfig, DriverStatus, OutputTarget};
pub use take::{Take, TakeFrame, TakePhase, TakeStatus};
pub use meters::{
    BufferMarks, BusMeters, DeviceMeters, DeviceTelemetry, ModulatorMeters, PlayheadMeters,
};
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
        align: Option<Box<IntegerDelay>>,
        analyzer: Box<SpectrumAnalyzer>,
        /// The slot's host and control state. Allocated here with the node
        /// rather than reserved for all 256 addressable slots up front, which
        /// is what an empty slot used to cost
        /// (`docs/plans/archive/modulator-capacity/`).
        state: Box<EffectSlot>,
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
        align: Option<Box<IntegerDelay>>,
    },
    /// Tell a container how far it reaches, and hand it the ring that delays
    /// its dry copy by that run's declared latency.
    ///
    /// A span is *structure*, not a control: a curve drawn on it would
    /// rewrite the shape of the chain from the audio thread, which is why
    /// `children` has no descriptor id and travels here instead. Structural
    /// for the same reason [`Self::SetCompensation`] is -- the ring is
    /// allocated on the control thread and the displaced one is reclaimed
    /// there, because the audio thread may do neither.
    ///
    /// `scratch` is the chain's per-depth dry buffers, sent once with the
    /// first container a chain ever holds and `None` thereafter; a chain that
    /// already has them hands the arrival straight back.
    SetContainerSpan {
        target: EffectTarget,
        slot: u8,
        children: u8,
        align: Option<Box<IntegerDelay>>,
        scratch: Option<Box<ContainerScratch>>,
    },
    /// Remove whatever is at `slot`, if anything. Also reclaimed, not dropped.
    RemoveEffect { target: EffectTarget, slot: u8 },
    /// Append one channel's storage, built on this thread. The graph only
    /// grows: a removed channel's storage stays for the next one rather than
    /// being freed on the audio thread, so this arrives with storage the
    /// graph may already have and hands it straight back if so.
    AddChannel {
        storage: Box<ChannelStorage>,
        source: DeviceKind,
    },
    /// Arm a take on `channel`: it waits for the next bar line, then records
    /// the channel's audio input into the take's ring (`audio-recording/03`).
    /// Structural because the ring is allocated here; a take it displaces
    /// comes back through the reclaim ring, so its producer is dropped off
    /// the audio thread and its drain sees the ring abandoned.
    StartTake { channel: u8, take: Box<Take> },
    /// Replace how each channel takes MIDI input. Structural, not a shared
    /// cell, so the table the audio thread is reading can never be freed
    /// under it: the displaced one comes back through the reclaim ring
    /// (`reports/fable-2026-09-19.md`, finding 2).
    SetMidiRouting(Box<render::MidiRouting>),
    /// Replace which seat each channel records from. As `SetMidiRouting`.
    SetAudioInputRouting(Box<render::AudioInputRouting>),
    /// Replace, or clear, the MIDI mapping that drives a buffer insert.
    SetBufferMidi(Option<Box<mooloop_core::midi::BufferMidiMap>>),
    /// Install (or clear) a producer's latency compensation delay.
    ///
    /// `target` is the channel or bus whose *output* waits; `None` means it
    /// is the longest path into its destination and needs no delay, which is
    /// the common case and costs nothing.
    ///
    /// Structural for the same reason as [`Self::SetSamplerStretch`]: the ring
    /// is allocated on the control thread and the displaced one is reclaimed
    /// there, because the audio thread may do neither. The length comes from
    /// `mooloop_core::compile_latency`, which is recomputed whenever anything
    /// that could change a chain's latency moves.
    SetCompensation {
        target: EffectTarget,
        delay: Option<Box<IntegerDelay>>,
    },
    /// Give a bus its second input accumulator -- the sum of everything
    /// feeding it that opted into console summing -- or take it away.
    ///
    /// Structural for the same reason [`Self::SetCompensation`] is: the 64 KB
    /// buffer is allocated on the control thread and the displaced one is
    /// reclaimed there, because the audio thread may do neither. `None` means
    /// nothing encoded reaches this bus, which is every bus in a project that
    /// has not switched console on -- so the feature costs nothing while it
    /// is out, which is the rule everything in `docs/plans/archive/console/`
    /// is held
    /// to.
    ///
    /// The switch itself is a POD `EngineCommand::SetStripConsole`, and the
    /// two may arrive in either order: a strip encoding into a bus with no
    /// accumulator sums linearly until one turns up.
    SetConsoleSum {
        bus: u8,
        buffer: Option<Box<StereoBus>>,
    },
    /// Give a channel's sampler its time-stretch state, or take it away.
    ///
    /// Structural rather than a parameter because the pool is ~1.6 MB and has
    /// to be allocated on this thread; `Sampler::set_params` runs on the
    /// realtime command drain and must not allocate. `SamplerParams::
    /// stretch_enabled` records the intent this command realizes, and a
    /// sampler whose intent is on but whose pool has not arrived plays
    /// unstretched rather than falling silent -- so the two can be applied in
    /// either order.
    SetSamplerStretch {
        channel: u8,
        pool: Option<Box<StretchPool>>,
    },
    /// Install the channels' audio edges, their render order, and the buffers
    /// they carry, as one value.
    ///
    /// Structural for the reason [`Self::SetCompensation`] is: the buffers are
    /// allocated on the control thread and the displaced ones are reclaimed
    /// there. Whole rather than incremental because an edge without its
    /// schedule, or a schedule against another generation's buffers, is not a
    /// state the executor may ever observe -- the same rule
    /// `CompiledBusGraph` and its render order already travel under.
    ///
    /// The plan is derived from the model on every pump tick and sent only
    /// when it differs, exactly as `Session::sync_compensation` does, because
    /// the answer is a property of every channel at once and a per-edit call
    /// site is a list that grows silently.
    SetAudioGraph { bank: Box<AudioTapBank> },
    /// Install the track graph and the sends that ride on it, as one value.
    ///
    /// This replaces `EngineCommand::InstallBusGraph`, which was POD because
    /// a track's routing was a `[u8; MAX_BUSES]` permutation and nothing
    /// else. A send is a producer's *second* outgoing edge: it carries a
    /// compensation ring, which is a heap object the audio thread may neither
    /// allocate nor free, so the whole plan becomes structural.
    ///
    /// Whole rather than incremental, and one command rather than two, for
    /// the reason [`Self::SetAudioGraph`] gives: a send whose target the
    /// render order has not been told about would arrive a block late, and
    /// that is not a state the executor may observe even briefly.
    SetTrackGraph {
        graph: CompiledBusGraph,
        sends: Box<SendBank>,
    },
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
    /// A complete executor displaced by a project install, and the
    /// [`CarryPlan`] that install was made under. Keeping the renderer boxed
    /// lets the realtime thread swap ownership without allocating; both are
    /// destroyed when `EngineHandle::poll` drains this variant.
    ///
    /// **The plan travels with the renderer rather than in a variant of its
    /// own.** The executor reserves exactly one reclaim slot before it
    /// commits to an install, so a second push would mean a second slot, a
    /// `slots() < 2` check, and a new way for the swap to be turned away.
    /// Freeing the plan's four `Vec`s where it was last used -- the callback
    /// -- is the thing this variant exists to prevent.
    RenderState {
        retired: Box<RenderState>,
        carry: CarryPlan,
    },
    /// A sample whose browser preview finished or was replaced. Same
    /// ownership round trip as an effect node: the sample's last reference
    /// must not be dropped on the realtime thread.
    PreviewSample { sample: Arc<SampleData> },
    /// A sample, or the snapshot that carried it, that a sampler voice let go
    /// of on a note-on. Either can be the last reference to the buffer.
    SamplerAudio(mooloop_dsp::RetiredAudio),
    /// Stretch state displaced by an install, or surrendered when a sampler
    /// stopped stretching. Same reason as the rest: megabytes of `Box` must
    /// not be freed on the audio thread.
    SamplerStretch(Box<StretchPool>),
    /// A compensation delay displaced by a new one, or surrendered when a
    /// producer became the longest path and stopped needing one.
    Compensation(Box<IntegerDelay>),
    /// A bus's encoded-sum accumulator, surrendered when nothing console
    /// encoded feeds it any more. Same rule as the rest: 64 KB must not be
    /// freed on the audio thread.
    ConsoleSum(Box<StereoBus>),
    /// The audio-edge plan a newer one replaced. Its buffers are 64 KB each
    /// and must not be freed on the audio thread.
    AudioGraph(Box<AudioTapBank>),
    /// The previous generation's sends, with their compensation rings.
    TrackGraph(Box<SendBank>),
    /// A take displaced by a new one on the same channel, or one with no
    /// channel to record on.
    Take(Box<Take>),
    /// Routing tables displaced by their `Set*` commands.
    MidiRouting(Box<render::MidiRouting>),
    AudioInputRouting(Box<render::AudioInputRouting>),
    BufferMidi(Box<mooloop_core::midi::BufferMidiMap>),
    /// A container's dry-path ring displaced by a resize, or the per-depth
    /// scratch handed to a chain that already had it. Same rule as every
    /// other box that reaches the audio thread: it comes back to be dropped.
    Container {
        align: Option<Box<IntegerDelay>>,
        scratch: Option<Box<ContainerScratch>>,
    },
}

/// A project that has already been instantiated and allocated off the audio
/// thread. The realtime callback only swaps the box and acknowledges its
/// generation.
pub(crate) struct PreparedProject {
    pub generation: u64,
    pub render: Box<RenderState>,
    /// Whether the incoming renderer should take over the outgoing one's
    /// transport instead of starting stopped at zero.
    ///
    /// Set for a **structural edit** -- a channel paste, delete or move --
    /// and clear for opening a document, which is meant to stop and rewind.
    /// `LOOSE_ENDS.md`, "Every structural edit stops the song": moving one
    /// channel halted the transport and rewound the whole arrangement,
    /// including for the channels the edit never touched.
    ///
    /// **It is a flag rather than a transport value**, and that is the point.
    /// The install is prepared on the control thread and swapped in on the
    /// audio thread, and the song keeps playing in between. A position
    /// captured at preparation time would be however many blocks stale by the
    /// time it landed, so the song would jump backwards by the length of its
    /// own install. The executor reads the outgoing transport at the instant
    /// it swaps.
    pub keep_transport: bool,
    /// The live strips the incoming renderer takes over rather than
    /// rebuilding; see [`CarryPlan`].
    pub carry: CarryPlan,
}

/// `(outgoing index, incoming index)` pairs of strips whose channel or track
/// did not change, so the live one can be moved across instead of rebuilt.
///
/// Decided on the control thread by [`EngineHandle::carry_plan`], because
/// that is the only place both projects exist. The audio thread swaps boxes
/// and does not compare anything; see [`RenderState::carry_strips_from`].
///
/// Bounded by `MAX_CHANNELS` and `MAX_BUSES`, and allocated here off-thread.
/// It is freed off-thread too: the executor hands the plan back through the
/// reclaim ring with the generation it retired, so the four `Vec`s are
/// destroyed by `EngineHandle::poll` on the control thread rather than by
/// drop glue at the end of the callback.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CarryPlan {
    pub channels: Vec<(u8, u8)>,
    pub tracks: Vec<(u8, u8)>,
    /// Where every channel that survives the edit went, whether or not its
    /// strip is carried. `channels` is the subset whose setup is unchanged.
    ///
    /// The sends need the whole map rather than the carried subset: a send's
    /// ring holds its producer's past audio, which is still right when the
    /// producer's chain was just edited, so an edge is matched by where its
    /// two ends went rather than by whether either was rebuilt.
    pub channel_seats: Vec<(u8, u8)>,
    /// `channel_seats` for the track list.
    pub track_seats: Vec<(u8, u8)>,
}

impl CarryPlan {
    /// Where `seat` in the outgoing generation sits in the incoming one, or
    /// `None` when it did not survive. Audio thread: a scan of at most
    /// `MAX_BUSES` pairs, no allocation.
    pub(crate) fn seat(&self, seat: EffectTarget) -> Option<EffectTarget> {
        let find = |seats: &[(u8, u8)], from: u8| {
            seats.iter().find(|(was, _)| *was == from).map(|&(_, now)| now)
        };
        match seat {
            EffectTarget::Channel(from) => find(&self.channel_seats, from).map(EffectTarget::Channel),
            EffectTarget::Bus(from) => find(&self.track_seats, from).map(EffectTarget::Bus),
        }
    }
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

#[derive(Debug)]
pub enum Error {
    ClientOpen(String),
    PortRegister(String),
    Activate(String),
    /// Core Audio: no output device, or one that would not describe itself.
    Device(String),
    /// Core Audio: no device would run a stream.
    Stream(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::ClientOpen(s) => write!(f, "failed to open JACK client: {s}"),
            Error::PortRegister(s) => write!(f, "failed to register JACK port: {s}"),
            Error::Activate(s) => write!(f, "failed to activate JACK client: {s}"),
            Error::Device(s) => write!(f, "failed to open an audio device: {s}"),
            Error::Stream(s) => write!(f, "failed to start audio output: {s}"),
        }
    }
}

impl std::error::Error for Error {}

/// Keep-alive guard for the audio engine. Dropping it, together with the
/// handle, shuts the driver down.
pub struct Engine {
    _driver: Arc<Driver>,
}

impl Engine {
    /// Open the audio driver and start the realtime thread. All channel
    /// devices and the pattern bank are pre-allocated to pool size; every
    /// channel starts with an empty sample slot until the user loads an audio
    /// file or a project assigns one.
    pub fn new(config: AudioConfig) -> Result<(Engine, EngineHandle), Error> {
        // The driver opens first because the render state is built for the
        // sample rate it reports.
        let opening = Opening::connect()?;
        let sample_rate = opening.sample_rate();

        let (cmd_tx, cmd_rx): (Producer<RealtimeCommand>, Consumer<RealtimeCommand>) =
            rtrb::RingBuffer::new(QUEUE_CAPACITY);
        let (evt_tx, evt_rx): (Producer<EngineEvent>, Consumer<EngineEvent>) =
            rtrb::RingBuffer::new(QUEUE_CAPACITY);
        let (reclaim_tx, reclaim_rx): (Producer<StructuralReclaim>, Consumer<StructuralReclaim>) =
            rtrb::RingBuffer::new(QUEUE_CAPACITY);

        // Every channel's slot starts empty; a channel is silent until the
        // user loads a sample or a project assigns one. Sample and slice map
        // share one slot because they are one fact -- see
        // `ChannelAudioSnapshot` -- and they take this route rather than the
        // command ring because `EngineCommand` is `Copy` and unboxed by
        // design, so neither a buffer nor a `Vec` of markers can ride it.
        let audio_slots = render::empty_channel_audio_bank();

        let xrun_count = Arc::new(AtomicU64::new(0));
        let load = load::LoadMeters::new();
        let shared = SharedCells::new();
        let mut render = RenderState::new(sample_rate, audio_slots.clone());
        shared.attach(&mut render);
        let executor = Executor::new(
            ExecutorIo {
                cmd_rx,
                evt_tx,
                reclaim_tx,
            },
            Box::new(render),
            xrun_count.clone(),
            sample_rate,
            load.clone(),
        );
        let driver = Arc::new(opening.start(executor, xrun_count, config)?);

        Ok((
            Engine {
                _driver: driver.clone(),
            },
            EngineHandle {
                cmd_tx,
                evt_rx,
                reclaim_rx,
                shared,
                audio_slots,
                sample_rate,
                install_generation: 0,
            // Nothing has been installed, so nothing can be carried: the
            // startup generation is built rather than swapped in.
            last_installed: None,
                driver,
                load,
            },
        ))
    }
}

/// The cells the control thread and the renderer share rather than message
/// through: meters and telemetry the GUI reads, and the MIDI and preview
/// settings the GUI writes.
///
/// A [`RenderState`] is built with private copies of every one of them, so a
/// renderer that was not attached to these publishes into arrays nobody reads
/// and reads settings nobody writes. There are two places a renderer is built
/// for the audio thread -- at startup and at every project install -- and
/// [`Self::attach`] is the one list both of them use.
#[derive(Clone)]
struct SharedCells {
    bus_meters: Arc<BusMeters>,
    device_meters: Arc<DeviceMeters>,
    device_telemetry: Arc<DeviceTelemetry>,
    /// The buffer MIDI mapping last sent, kept on this side so a renderer
    /// built for an install starts with it. Not a cell: the live renderer
    /// holds its own copy, replaced by `StructuralCommand::SetBufferMidi`.
    buffer_midi: Option<mooloop_core::midi::BufferMidiMap>,
    keyboard_channel: Arc<AtomicU8>,
    playhead_meters: Arc<PlayheadMeters>,
    modulator_meters: Arc<ModulatorMeters>,
    preview_gain: Arc<AtomicU32>,
}

impl SharedCells {
    fn new() -> Self {
        Self {
            bus_meters: BusMeters::new(),
            device_meters: DeviceMeters::new(),
            device_telemetry: DeviceTelemetry::new(),
            buffer_midi: None,
            keyboard_channel: Arc::new(AtomicU8::new(render::NO_KEYBOARD_CHANNEL)),
            playhead_meters: PlayheadMeters::new(),
            modulator_meters: ModulatorMeters::new(),
            preview_gain: Arc::new(AtomicU32::new(
                mooloop_core::gain::db_to_linear(mooloop_core::gain::REFERENCE_PEAK_DBFS)
                    .to_bits(),
            )),
        }
    }

    /// Point `render` at these cells in place of its private ones, and seed it
    /// with the buffer mapping. Control thread only: it allocates.
    fn attach(&self, render: &mut RenderState) {
        render.attach_meters(self.bus_meters.clone());
        render.attach_device_meters(self.device_meters.clone());
        render.attach_device_telemetry(self.device_telemetry.clone());
        drop(render.set_buffer_midi(self.buffer_midi.map(Box::new)));
        render.attach_keyboard_channel(self.keyboard_channel.clone());
        render.attach_playhead_meters(self.playhead_meters.clone());
        render.attach_modulator_meters(self.modulator_meters.clone());
        render.attach_preview_gain(self.preview_gain.clone());
    }
}

/// Performance state a project does not contain but a renderer holds, handed
/// over with every install so the incoming renderer starts with it.
///
/// A project install replaces the complete renderer, and a fresh one starts
/// from defaults. Anything the control thread set by command rather than by
/// document has to travel here, or the swap quietly resets it: record arm did
/// exactly that until 2026-09-17, leaving the interface showing armed over an
/// engine that was not. Carried rather than re-sent after the install so there
/// is no block in which the new renderer holds the default.
#[derive(Debug, Clone, Default)]
pub struct InputState {
    /// Whether recording is armed, as the session says.
    pub record_armed: bool,
    /// How each channel of the *incoming* project takes MIDI input, indexed
    /// by its channel order, as [`EngineHandle::set_midi_routing`] takes it.
    ///
    /// A channel removal, move, paste or undo renumbers channels, so the
    /// outgoing project's routing names the wrong ones. The incoming renderer
    /// gets a routing cell of its own holding this, and the handle's later
    /// writes go to that cell once the install is queued -- so neither
    /// renderer ever reads the other's channel order.
    pub midi_routing: Vec<mooloop_core::MidiInputRoute>,
    /// Which of the *incoming* project's seats each of its channels records
    /// from -- [`Self::midi_routing`]'s twin, for the same reason.
    pub audio_input: Vec<Option<mooloop_core::AudioTap>>,
    /// Which of the *incoming* project's channels monitor the hardware input,
    /// by seat. Performance state, so it travels here rather than in the
    /// document; `Session::monitor_seats` builds it from identities.
    pub monitor: Vec<bool>,
}

/// Which strips `incoming` can take over from the generation built for `live`.
///
/// A channel qualifies when it is **the same channel** -- matched by
/// `ChannelId`, which is what `channel-identity` steps 01 to 03 built -- **and
/// its setup is unchanged**.
///
/// Comparing the whole `ChannelSetup` rather than the shape of its chain is
/// deliberate, and simpler than `05-strips-by-id.md` asked for. The plan
/// proposed matching structure and then replaying the incoming parameter
/// values onto the carried strip as events, which is a second mechanism to
/// get wrong in exchange for nothing: if anything about a channel differs it
/// is rebuilt, and a channel that differs is by definition the one the user
/// just edited. Every case that makes a song audibly stutter still carries --
/// a move, a paste, a delete, a track added, an undo -- because none of them
/// change the setup of the channels they are not about.
///
/// A free function rather than a method so the tests can ask the same
/// question the handle asks, of two projects they built themselves.
pub(crate) fn carry_plan(
    live: &mooloop_core::Project,
    incoming: &mooloop_core::Project,
) -> CarryPlan {
    CarryPlan {
        channels: carry_channels(live, incoming),
        tracks: carry_tracks(live, incoming),
        channel_seats: seats(
            live.channels.iter().take(MAX_CHANNELS).map(|channel| channel.id),
            incoming.channels.iter().take(MAX_CHANNELS).map(|channel| channel.id),
            mooloop_core::ChannelId::is_assigned,
        ),
        track_seats: seats(
            live.buses.iter().take(mooloop_core::MAX_BUSES).map(|track| track.id),
            incoming.buses.iter().take(mooloop_core::MAX_BUSES).map(|track| track.id),
            mooloop_core::TrackId::is_assigned,
        ),
    }
}

/// `(outgoing seat, incoming seat)` for every assigned id present in both.
fn seats<Id: PartialEq + Copy>(
    live: impl Iterator<Item = Id> + Clone,
    incoming: impl Iterator<Item = Id>,
    assigned: fn(Id) -> bool,
) -> Vec<(u8, u8)> {
    incoming
        .enumerate()
        .filter(|&(_, id)| assigned(id))
        .filter_map(|(to, id)| {
            live.clone()
                .position(|held| held == id)
                .map(|from| (from as u8, to as u8))
        })
        .collect()
}

fn carry_channels(
    live: &mooloop_core::Project,
    incoming: &mooloop_core::Project,
) -> Vec<(u8, u8)> {
    let mut plan = Vec::new();
    for (to, channel) in incoming.channels.iter().take(MAX_CHANNELS).enumerate() {
        if !channel.id.is_assigned() {
            continue;
        }
        let Some(from) = live
            .channels
            .iter()
            .take(MAX_CHANNELS)
            .position(|held| held.id == channel.id && same_strip(&held.setup, &channel.setup))
        else {
            continue;
        };
        plan.push((from as u8, to as u8));
    }
    plan
}

/// The tracks `incoming` can take over from the generation built for `live`:
/// [`carry_channels`] one list over, matched by `TrackId`
/// (`incremental-structure/01`) and compared by [`same_track`].
fn carry_tracks(
    live: &mooloop_core::Project,
    incoming: &mooloop_core::Project,
) -> Vec<(u8, u8)> {
    let mut plan = Vec::new();
    for (to, track) in incoming.buses.iter().take(mooloop_core::MAX_BUSES).enumerate() {
        if !track.id.is_assigned() {
            continue;
        }
        let Some(from) = live
            .buses
            .iter()
            .take(mooloop_core::MAX_BUSES)
            .position(|held| held.id == track.id && same_track(held, track))
        else {
            continue;
        };
        plan.push((from as u8, to as u8));
    }
    plan
}

/// Whether a track strip built for `held` can stand in for one built for
/// `incoming`: [`same_strip`]'s question for a track.
///
/// Where a track sends its audio is left out -- its `output` and its sends --
/// and so is its solo. None of them is strip content: the bus graph, the send
/// bank and the solo verdict are compiled from the incoming project whole,
/// and `carry_strips_from` leaves what they derive with the fresh strip. A
/// track move renumbers the first two on every track that routes to a moved
/// one, which is the case this exists for.
///
/// Destructured for [`same_strip`]'s reason.
fn same_track(held: &mooloop_core::BusSetup, incoming: &mooloop_core::BusSetup) -> bool {
    let mooloop_core::BusSetup {
        id: _,
        bus,
        effects,
        next_device_id,
        sends: _,
    } = held;
    let mooloop_core::MixerBus {
        name,
        muted,
        volume,
        pan,
        output: _,
        console,
        polarity,
        solo: _,
        strip,
        color,
    } = bus;
    let other = &incoming.bus;
    *name == other.name
        && *muted == other.muted
        && *volume == other.volume
        && *pan == other.pan
        && *console == other.console
        && *polarity == other.polarity
        && *strip == other.strip
        && *color == other.color
        && *effects == incoming.effects
        && *next_device_id == incoming.next_device_id
}

/// Whether a strip built for `held` can stand in for one built for
/// `incoming`: every field of the setup equal **except the track it feeds**.
///
/// A track is still a seat, so a track move renumbers `channel.bus` on every
/// channel feeding a track that moved. Comparing it made each of those
/// channels look edited, and they were rebuilt -- which was half of the
/// glitch Adam heard on a track move on 2026-09-18. The destination is not
/// strip content: it is re-read from the incoming project by
/// [`RenderState::carry_strips_from`], the way the compensation delay is.
///
/// Destructured rather than compared through a clone with the bus zeroed, so
/// that a field added to either struct fails to compile here until somebody
/// decides whether it belongs to the strip.
fn same_strip(held: &mooloop_core::ChannelSetup, incoming: &mooloop_core::ChannelSetup) -> bool {
    let mooloop_core::ChannelSetup {
        channel,
        source,
        effects,
        modulation,
        next_device_id,
    } = held;
    let mooloop_core::Channel {
        name,
        kind,
        muted,
        volume,
        pan,
        bus: _,
        color,
        midi_input,
        // Where the channel records audio from is routing, held in the
        // engine's own audio input cell, not strip content.
        audio_input: _,
    } = channel;
    let other = &incoming.channel;
    *name == other.name
        && *kind == other.kind
        && *muted == other.muted
        && *volume == other.volume
        && *pan == other.pan
        && *color == other.color
        && *midi_input == other.midi_input
        && *source == incoming.source
        && *effects == incoming.effects
        && *modulation == incoming.modulation
        && *next_device_id == incoming.next_device_id
}

/// The renderer a project install hands the audio thread, built and attached
/// on the calling thread. Separate from [`EngineHandle::install_project`]
/// because a handle cannot be built without opening an audio driver, and what
/// this returns is the part of an install a test can hold.
fn prepare_render_state(
    shared: &SharedCells,
    sample_rate: u32,
    bank: render::ChannelAudioBank,
    project: &mooloop_core::Project,
    input: &InputState,
) -> (RenderState, SharedCells) {
    let mut render = RenderState::new(sample_rate, bank);
    let cells = shared.clone();
    // A project swap replaces the complete renderer. Reconnect every shared
    // cell before it reaches the audio thread: otherwise the new renderer
    // publishes into its private, unread arrays while the UI continues to
    // read the startup arrays forever.
    cells.attach(&mut render);
    // The two routing tables are the incoming project's own, because a
    // channel edit renumbers the seats they are indexed by. Set here rather
    // than sent after the install, so there is no block in which the new
    // renderer reads the default; later changes follow the install down the
    // same ordered stream, so they cannot reach the wrong generation.
    drop(render.set_midi_routing(Box::new(render::MidiRouting {
        routes: input.midi_routing.clone(),
    })));
    drop(render.set_audio_input_routing(Box::new(render::AudioInputRouting {
        taps: input.audio_input.clone(),
    })));
    render.load_project(project);
    render.set_record_armed(input.record_armed);
    render.set_input_monitors(&input.monitor);
    (render, cells)
}

/// Somewhere to put a command for the audio thread, and an honest answer
/// about whether it got there.
///
/// Every method returns whether the command reached the bounded ring, and
/// every method is `#[must_use]`, because the failure this exists to prevent
/// is not "a command was lost" -- it is "a command was lost and the sender
/// stopped believing anything was wrong". The session's reconcilers are
/// diff-based: one that advances its `_sent` mirror after a dropped send will
/// never retry it, because the difference that would have found it has
/// already been satisfied. `docs/AUDIO_ARCHITECTURE.md` says queue overflow
/// must be observable to the sender; this is the shape of that.
///
/// [`EngineHandle`] is the only implementor that ships. The trait exists
/// because an `EngineHandle` cannot be built without opening an audio driver,
/// so a test that wants to see a reconciler *refused* has no other way to
/// reach one. See `mooloop-session`'s `delivery` tests.
pub trait CommandSink {
    /// Queue a POD command. Non-blocking.
    #[must_use]
    fn send(&mut self, cmd: EngineCommand) -> bool;

    /// Hand a heap-allocated structural change (effect install/remove) over.
    /// Non-blocking; on a refusal the command -- and any `Box` it carries --
    /// is dropped here, on the calling (GUI) thread, which is the whole point
    /// of refusing rather than blocking. A caller that sees `false` has
    /// nothing left to reclaim.
    #[must_use]
    fn send_structural(&mut self, cmd: StructuralCommand) -> bool;

    /// The rate the prepared nodes a caller builds must be sized for.
    fn sample_rate(&self) -> u32;
}

/// The control thread's handle into the engine. Realtime communication is
/// bounded and non-blocking; project installation also performs allocation
/// and node construction here before publishing a prepared executor.
pub struct EngineHandle {
    cmd_tx: Producer<RealtimeCommand>,
    evt_rx: Consumer<EngineEvent>,
    reclaim_rx: Consumer<StructuralReclaim>,
    shared: SharedCells,
    /// The bank the **most recently prepared** generation reads.
    ///
    /// Not "the live generation's": a per-channel publication between queueing
    /// an install and the audio thread consuming it belongs to the incoming
    /// project, and lands in its bank so it is heard the instant that
    /// generation goes live. A refused install leaves this pointing at the
    /// outgoing generation's bank, which is what makes the caller's early
    /// return honest.
    audio_slots: render::ChannelAudioBank,
    sample_rate: u32,
    install_generation: u64,
    /// The project the live generation was built from, for deciding what an
    /// install can carry across.
    ///
    /// Only the control thread reads it and only a *successful* install
    /// replaces it, so it always describes the generation the audio thread is
    /// either running or about to run.
    ///
    /// **It can be stale against the live graph, and only ever in the safe
    /// direction.** Incremental structural commands -- a device dragged in the
    /// rack, a channel added -- change the engine without an install, and are
    /// mirrored into the session, so the *next* project to arrive here carries
    /// them and compares as different. That costs a rebuild that was not
    /// strictly needed; it cannot carry a strip whose chain has moved on.
    last_installed: Option<Arc<mooloop_core::Project>>,
    driver: Arc<Driver>,
    load: Arc<load::LoadMeters>,
}

impl EngineHandle {
    /// Read the audio callback's timing since this was last called, and start
    /// a new window.
    ///
    /// Meant to be polled about once a second. Every field is a count or a
    /// ratio over the window, so calling it faster narrows the window rather
    /// than repeating a reading.
    pub fn take_load(&self) -> load::LoadSnapshot {
        self.load.take()
    }

    /// How the audio callback thread is scheduled, without draining the
    /// timing window.
    pub fn realtime_status(&self) -> load::RealtimeStatus {
        self.load.realtime()
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
        let align = IntegerDelay::new(node.dry_path_latency_frames()).map(Box::new);
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
        // Driver upkeep rides the poll every caller already makes: under Core
        // Audio this is where a stream whose device went away is reopened.
        self.driver.service();
        // Reclaim displaced effect occupants first: dropping the boxes here
        // frees them off the realtime thread, which is the entire point of
        // the reclaim ring.
        while let Ok(reclaim) = self.reclaim_rx.pop() {
            match reclaim {
                StructuralReclaim::Effect(effect) => drop(effect),
                StructuralReclaim::RenderState { retired, carry } => {
                    drop(retired);
                    drop(carry);
                }
                StructuralReclaim::PreviewSample { sample } => drop(sample),
                StructuralReclaim::SamplerAudio(audio) => drop(audio),
                StructuralReclaim::SamplerStretch(pool) => drop(pool),
                StructuralReclaim::Compensation(delay) => drop(delay),
                StructuralReclaim::ConsoleSum(buffer) => drop(buffer),
                StructuralReclaim::AudioGraph(bank) => drop(bank),
                StructuralReclaim::TrackGraph(bank) => drop(bank),
                StructuralReclaim::Take(take) => drop(take),
                StructuralReclaim::MidiRouting(routing) => drop(routing),
                StructuralReclaim::AudioInputRouting(routing) => drop(routing),
                StructuralReclaim::BufferMidi(map) => drop(map),
                StructuralReclaim::Container { align, scratch } => {
                    drop(align);
                    drop(scratch);
                }
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

    /// Publish what `channel` plays: its buffer and the map that indexes it,
    /// as one store. The realtime sampler picks the pair up on the next
    /// note-on. Wait-free; UI-thread safe.
    ///
    /// **There is deliberately no way to set one without the other.** The
    /// four methods this replaced -- `load_sample`, `clear_sample`,
    /// `load_slices`, `clear_slices` -- let a note-on land between two stores
    /// and play a new buffer against old markers, which after a stretch
    /// commit means a marker indexing a buffer of a different length. Keeping
    /// any of them as a convenience, reading the current snapshot and storing
    /// a modified copy, would reintroduce exactly that.
    pub fn set_channel_audio(&self, channel: usize, audio: ChannelAudioSnapshot) {
        if let Some(slot) = self.audio_slots.get(channel) {
            // An empty snapshot is stored as nothing, so a silent channel has
            // one representation rather than two.
            slot.store((!audio.is_empty()).then(|| Arc::new(audio)));
        }
    }

    /// Queue a preview-voice command. Non-blocking; returns whether it
    /// reached the ring. A refused preview is silence where the user expected
    /// to hear a file, so the caller is the only place that can say so.
    #[must_use]
    pub fn preview(&mut self, command: PreviewCommand) -> bool {
        self.cmd_tx.push(RealtimeCommand::Preview(command)).is_ok()
    }

    /// Add a channel. Its strip, event list and control-output buffer are
    /// built here rather than reserved at startup, and travel to the graph
    /// through the structural ring like any other allocation.
    ///
    /// `false` means the channel did not reach the graph -- either its index
    /// is outside the addressable range, or the ring refused the command and
    /// dropped the prepared storage here.
    #[must_use]
    pub fn add_channel(&mut self, channel: usize, source: DeviceKind) -> bool {
        let Some(slot) = self.audio_slots.get(channel).cloned() else {
            return false;
        };
        let storage = RenderState::build_channel(slot, self.sample_rate);
        self.send_structural(StructuralCommand::AddChannel { storage, source })
    }

    /// Sets the preview voice's linear output gain. Live: the voice reads
    /// the shared cell every block, so turning the knob is heard at once.
    pub fn set_preview_gain(&self, gain: f32) {
        self.shared.preview_gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    /// Prepare a complete executor from a validated project on this
    /// non-realtime thread, then queue an ownership swap for the next block.
    /// The displaced executor returns through the reclaim ring and is dropped
    /// by `poll`, never by the audio callback.
    #[must_use]
    /// Which strips the incoming project can take over from the live one.
    ///
    /// A channel qualifies when it is **the same channel** -- matched by
    /// `ChannelId`, which is what `channel-identity` steps 01 to 03 built --
    /// **and its setup is unchanged**. Equality of the whole `ChannelSetup`
    /// rather than of the chain's shape is deliberate, and simpler than the
    /// plan asked for: the plan proposed matching structure and then replaying
    /// the incoming parameter values onto the carried strip as events, which
    /// is a second mechanism to get wrong for no gain. If anything about a
    /// channel differs, that channel is rebuilt -- and the channel that
    /// differs is, by definition, the one the user just edited.
    ///
    /// Every case that makes a song audibly stutter still carries: a move, a
    /// paste, a delete, a track added, an undo -- none of them change the
    /// setup of the channels they are not about.
    fn carry_plan(&self, incoming: &mooloop_core::Project) -> CarryPlan {
        match self.last_installed.as_ref() {
            Some(live) => carry_plan(live, incoming),
            None => CarryPlan::default(),
        }
    }

    /// `keep_transport` carries the song across the swap rather than stopping
    /// and rewinding it; see [`PreparedProject::keep_transport`]. A structural
    /// edit sets it, opening a document does not.
    pub fn install_project(
        &mut self,
        project: Arc<mooloop_core::Project>,
        audio: Vec<ChannelAudioSnapshot>,
        input: InputState,
        keep_transport: bool,
    ) -> bool {
        let generation = self
            .install_generation
            .checked_add(1)
            .expect("project install generation exhausted");
        // A **fresh** bank, built from values the caller owns. This used to
        // be `self.audio_slots.clone()` -- one bank shared by every
        // generation that had ever existed -- so the caller published the new
        // project's samples into it out of band, and the outgoing project's
        // graph read them for the block or two before the audio thread
        // consumed the install. The bank is now part of what the install
        // carries, and `audio` is taken by value so the live generation's
        // slots cannot be handed in by mistake.
        let bank = render::channel_audio_bank(audio);
        let (render, cells) = prepare_render_state(
            &self.shared,
            self.sample_rate,
            bank.clone(),
            &project,
            &input,
        );
        let carry = self.carry_plan(&project);
        let prepared = PreparedProject {
            generation,
            render: Box::new(render),
            keep_transport,
            carry,
        };
        // A full queue leaves `prepared` on this thread, so dropping it is
        // realtime-safe. Project loads are rare and the queue has the same
        // generous capacity as other engine control paths.
        if self
            .cmd_tx
            .push(RealtimeCommand::InstallProject(prepared))
            .is_ok()
        {
            // From here on, handle writes address the incoming generation's
            // cells; the outgoing renderer keeps the routing it was built for.
            self.shared = cells;
            self.shared.device_telemetry.clear_spectra();
            self.install_generation = generation;
            // Per-channel publication now addresses the bank of the
            // generation that is *about to* be live, which is the right
            // answer on both sides of the switch: before it, a sample the
            // user loads belongs to the incoming project and is heard the
            // instant it arrives; after it, this is simply the live bank. The
            // outgoing generation keeps its own bank until its `RenderState`
            // comes back through the reclaim ring and `poll` drops both here,
            // on this thread.
            self.audio_slots = bank;
            self.last_installed = Some(project);
            true
        } else {
            // `bank` is dropped here with `prepared`, and `self.audio_slots`
            // still names the live generation's. That is what makes the
            // caller's "leave everything untouched on a refusal" true of the
            // samples as well as the project.
            false
        }
    }

    /// Read and clear one bus's held peak. Wait-free; see `meters` for why
    /// this is an atomic array rather than another event.
    pub fn take_bus_peak(&self, bus: usize) -> (f32, f32) {
        self.shared.bus_meters.take(bus)
    }

    /// Read and clear the hardware input's held peak.
    pub fn take_input_peak(&self) -> (f32, f32) {
        self.shared.bus_meters.take_input()
    }

    /// Read and clear how much gain reduction a track's channel strip took,
    /// in dB as a positive amount. Zero while its compressor is out.
    pub fn take_strip_reduction(&self, bus: usize) -> f32 {
        self.shared.bus_meters.take_reduction(bus)
    }

    /// Read and clear a device's held input/output peaks. `target` addresses
    /// channels and buses in one space: a channel is its own index, a bus is
    /// `MAX_CHANNELS + bus index`. Stage 0 is the source; effect slots follow.
    pub fn take_device_peak(&self, target: usize, stage: usize) -> ((f32, f32), (f32, f32)) {
        self.shared.device_meters.take(target, stage)
    }

    /// Read and clear a dynamics device's held display state: the loudest
    /// level its detector reached and the deepest gain reduction it applied
    /// over the blocks since the last read, as `(detector level, dB)`.
    /// Stages that do not reduce gain read `(0.0, 0.0)`.
    /// Empty every held cell of one chain's device meters.
    ///
    /// For the pump to call on the target it is *leaving*: a device meter is
    /// a `fetch_max` hold and only a read empties one, so a chain nobody is
    /// looking at keeps its loudest block forever and shows it for one tick
    /// the moment the rack is turned back to it.
    pub fn clear_device_meters(&self, target: usize) {
        self.shared.device_meters.clear_target(target);
    }

    pub fn take_device_dynamics(&self, target: usize, stage: usize) -> (f32, f32) {
        self.shared.device_meters.take_dynamics(target, stage)
    }

    /// Subscribe an effect stage's input to compact spectrum telemetry,
    /// returning whether it is subscribed afterwards. This is
    /// observation-only: it never participates in audio or modulation signal
    /// flow, and disabled stages do not run spectral analysis.
    ///
    /// `false` from an `enabled: true` call means the spectrum pool was full.
    /// Worth propagating rather than swallowing: the display is then reading
    /// zeros, and a caller that wanted to say so has the answer.
    pub fn set_effect_spectrum_enabled(
        &self,
        target: EffectTarget,
        slot: u8,
        enabled: bool,
    ) -> bool {
        self.shared.device_telemetry.set_spectrum_enabled(
            effect_target_index(target),
            usize::from(slot) + 1,
            enabled,
        )
    }

    /// The latest normalized log-frequency spectrum for one effect input.
    /// The data is a display vector, not PCM; callers may poll it at their
    /// own frame rate without affecting the audio callback.
    pub fn effect_spectrum(&self, target: EffectTarget, slot: u8) -> [f32; SPECTRUM_BINS] {
        self.shared.device_telemetry
            .read_spectrum(effect_target_index(target), usize::from(slot) + 1)
    }

    /// Point MIDI keyboard input at a channel, or at none. Notes a buffer
    /// mapping claims still go to the buffer. Cheap enough to call every
    /// frame, which is how the UI keeps it on the selected channel.
    pub fn set_keyboard_channel(&self, channel: Option<u8>) {
        let channel = channel
            .filter(|&channel| channel != render::NO_KEYBOARD_CHANNEL)
            .unwrap_or(render::NO_KEYBOARD_CHANNEL);
        self.shared.keyboard_channel.store(channel, Ordering::Relaxed);
    }

    /// Install the MIDI mapping that drives a buffer insert, or clear it.
    /// Built here and sent down the ordered stream; the one it displaces
    /// comes back through the reclaim ring. Returns whether it was queued.
    #[must_use]
    pub fn set_buffer_midi_map(&mut self, map: Option<mooloop_core::midi::BufferMidiMap>) -> bool {
        self.shared.buffer_midi = map;
        self.send_structural(StructuralCommand::SetBufferMidi(map.map(Box::new)))
    }

    /// Install how each channel takes MIDI input, indexed by channel.
    ///
    /// Built from the project's stored settings resolved against
    /// [`Self::midi_ports`], so the audio thread compares port ids rather
    /// than names. Call it when a channel's setting changes and when the port
    /// list does -- a keyboard plugged in mid-session is a channel whose
    /// stored port name resolves for the first time.
    #[must_use]
    pub fn set_midi_routing(&mut self, routes: Vec<mooloop_core::MidiInputRoute>) -> bool {
        self.send_structural(StructuralCommand::SetMidiRouting(Box::new(
            render::MidiRouting { routes },
        )))
    }

    /// Install which seat each channel records from, resolved by
    /// `Session::audio_input_taps`. Call it when a channel's AUDIO row
    /// changes; an install carries its own in [`InputState::audio_input`].
    #[must_use]
    pub fn set_audio_input_routing(&mut self, taps: Vec<Option<mooloop_core::AudioTap>>) -> bool {
        self.send_structural(StructuralCommand::SetAudioInputRouting(Box::new(
            render::AudioInputRouting { taps },
        )))
    }

    /// The MIDI inputs available to pick from right now.
    ///
    /// Under JACK this is one merged port; under Core MIDI it is one entry per
    /// source. The difference is the driver's, and the picker shows whichever
    /// it is rather than pretending they are the same.
    pub fn midi_ports(&self) -> Vec<mooloop_core::MidiPortInfo> {
        self.driver.midi_ports()
    }

    /// The hardware input's name for the AUDIO menu, or `None` when the driver
    /// has none.
    pub fn audio_input_label(&self) -> Option<String> {
        self.driver.audio_input_label()
    }

    /// The round trip a take from the hardware input is delayed by; see
    /// `Take::new`.
    pub fn input_latency_frames(&self) -> u32 {
        self.driver.input_latency_frames()
    }

    /// Arm or disarm recording.
    ///
    /// Returns what [`CommandSink::send`] returned: `false` means the ring
    /// refused it and the engine's arm state is *not* what the caller asked
    /// for. Passed on rather than swallowed, because a caller that mirrors
    /// the arm state would otherwise show armed over an engine that is not.
    #[must_use]
    pub fn set_record_armed(&mut self, armed: bool) -> bool {
        self.send(EngineCommand::SetRecordArmed(armed))
    }

    /// How many times a retained-audio buffer insert has been overtaken by
    /// its writer and force-returned to live. Monotonic since the device was
    /// installed, so a UI compares it against the value it last displayed.
    pub fn effect_buffer_collisions(&self, target: EffectTarget, slot: u8) -> u32 {
        self.shared.device_telemetry
            .read_buffer_collisions(effect_target_index(target), usize::from(slot) + 1)
    }

    /// Start or stop drawing a Buffer insert's waveform.
    ///
    /// A pooled subscription, exactly like a spectrum's: the peaks only cross
    /// to the GUI while a face is looking at them, and the pool is small
    /// because the number of Buffer faces a person can watch is smaller than
    /// the number of stages that could offer a spectrum. `false` means the
    /// pool was full, which draws as an empty waveform.
    pub fn set_buffer_waveform_enabled(
        &self,
        target: EffectTarget,
        slot: u8,
        enabled: bool,
    ) -> bool {
        self.shared.device_telemetry.set_waveform_enabled(
            effect_target_index(target),
            usize::from(slot) + 1,
            enabled,
        )
    }

    /// Latest peaks over a Buffer insert's retained history, oldest ring
    /// index first. All zero when nothing is subscribed.
    pub fn effect_buffer_waveform(&self, target: EffectTarget, slot: u8) -> Vec<f32> {
        self.shared.device_telemetry
            .read_waveform(effect_target_index(target), usize::from(slot) + 1)
    }

    /// Where a Buffer insert's head and window are, and whether it is frozen
    /// or waiting for a boundary. Published every block, subscription or not.
    pub fn effect_buffer_marks(&self, target: EffectTarget, slot: u8) -> BufferMarks {
        self.shared.device_telemetry
            .read_buffer_marks(effect_target_index(target), usize::from(slot) + 1)
    }

    /// Every currently-active sampler voice's normalized playback position
    /// on `channel`, for a UI playhead. Wait-free; see `meters` for why this
    /// is a plain array read rather than another event.
    pub fn playhead_positions(&self, channel: usize) -> Vec<f32> {
        self.shared.playhead_meters.read(channel)
    }

    /// The channel's modulator outputs as of the last control tick the audio
    /// thread ran. The UI resolves these against the channel's routes to draw
    /// each destination's live offset, rather than the engine publishing a
    /// value per parameter: a channel has at most
    /// `MAX_MODULATORS_PER_CHANNEL` sources but many more destinations.
    pub fn modulator_outputs(&self, channel: usize) -> [f32; CONTROL_SOURCE_SLOTS] {
        self.shared.modulator_meters.read(channel)
    }

    /// Candidate output destinations as the driver discovers them -- under
    /// JACK, input ports grouped by owning client. A non-realtime query; call
    /// it when the audio preferences page opens or on an explicit refresh, not
    /// every frame.
    pub fn available_output_targets(&self) -> Vec<OutputTarget> {
        self.driver.available_output_targets()
    }

    /// Route the output to `target`, or to the system default if `target` is
    /// `None`. Non-realtime driver calls; control-thread only.
    pub fn set_output_target(&mut self, target: Option<(String, String)>) -> Result<(), String> {
        self.driver.set_output_target(target)
    }

    /// Request a new buffer size. Under JACK this is server-wide: it changes
    /// the buffer for every JACK client connected, not only mooloop.
    pub fn set_buffer_size(&mut self, frames: u32) -> Result<(), String> {
        self.driver.set_buffer_size(frames)
    }

    /// Enable or disable restoring the configured output target when it goes
    /// away and comes back.
    pub fn set_auto_reconnect(&mut self, enabled: bool) {
        self.driver.set_auto_reconnect(enabled);
    }

    /// Live driver state for populating the preferences dialog.
    pub fn driver_status(&self) -> DriverStatus {
        DriverStatus {
            sample_rate: self.sample_rate,
            buffer_size: self.driver.buffer_size(),
            current_target: self.driver.current_target(),
        }
    }
}

impl CommandSink for EngineHandle {
    fn send(&mut self, cmd: EngineCommand) -> bool {
        self.cmd_tx.push(RealtimeCommand::Engine(cmd)).is_ok()
    }

    fn send_structural(&mut self, cmd: StructuralCommand) -> bool {
        self.cmd_tx.push(RealtimeCommand::Structural(cmd)).is_ok()
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

fn effect_target_index(target: EffectTarget) -> usize {
    match target {
        EffectTarget::Channel(channel) => usize::from(channel),
        EffectTarget::Bus(bus) => MAX_CHANNELS + usize::from(bus),
    }
}

#[cfg(test)]
mod install_tests {
    use super::*;
    use mooloop_core::{
        MidiChannelFilter, MidiInputRoute, MidiKind, MidiMessage, MidiPortId, MidiRouteSource,
        Project, ProjectChannel,
    };

    /// A channel's MIDI input setting is heard by the renderer a project
    /// install hands the audio thread, not only by the one built at startup.
    ///
    /// The app installs a project at startup, so the startup renderer is
    /// replaced before anybody plays a note. Until 2026-09-17 the install
    /// path attached every shared cell but the routing one, and every channel
    /// behaved as Follow Selection however it was set. Driven through
    /// `prepare_render_state` because an `EngineHandle` needs an audio driver;
    /// the routing arrives as the structural command `set_midi_routing` sends.
    #[test]
    fn an_installed_renderer_reads_the_routing_the_handle_writes() {
        let shared = SharedCells::new();
        let mut project = Project::default();
        project.channels.push(ProjectChannel::sampler(1, 1));
        let bank = render::channel_audio_bank(Vec::new());
        let (mut render, shared) =
            prepare_render_state(&shared, 48_000, bank, &project, &InputState::default());

        // Channel 1 listens on MIDI channel 10; the selection is channel 0.
        let displaced = render.apply_structural(StructuralCommand::SetMidiRouting(Box::new(
            render::MidiRouting {
                routes: vec![
            MidiInputRoute {
                source: MidiRouteSource::AllPorts,
                channel: MidiChannelFilter::One(0),
            },
            MidiInputRoute {
                source: MidiRouteSource::AllPorts,
                channel: MidiChannelFilter::One(9),
            },
                ],
            },
        )));
        assert!(displaced.is_some(), "the default table leaves through the reclaim ring");
        shared.keyboard_channel.store(0, Ordering::Relaxed);

        render.apply_midi(&[MidiMessage {
            offset: 0,
            port: MidiPortId::FIRST,
            channel: 9,
            kind: MidiKind::NoteOn {
                note: 60,
                velocity: 100,
            },
        }]);
        assert_eq!(
            render.audition_channels(),
            vec![1],
            "a note on MIDI channel 10 belongs to the channel listening there, \
             not to the selection"
        );
    }

    /// A renderer an install hands over is armed when the session is.
    ///
    /// Every install -- a channel added or removed, an undo, a load -- builds
    /// a fresh renderer, and a fresh renderer starts disarmed. Nothing re-sent
    /// the arm, so until 2026-09-17 the first structural edit silently stopped
    /// recording while the record button still read armed.
    #[test]
    fn an_installed_renderer_keeps_the_record_arm() {
        use mooloop_core::EngineEvent;

        let shared = SharedCells::new();
        let mut project = Project::default();
        project.channels.push(ProjectChannel::sampler(1, 1));
        let bank = render::channel_audio_bank(Vec::new());
        let input = InputState {
            record_armed: true,
            ..InputState::default()
        };
        let (mut render, shared) = prepare_render_state(&shared, 48_000, bank, &project, &input);
        shared.keyboard_channel.store(0, Ordering::Relaxed);

        let key = |kind| MidiMessage {
            offset: 0,
            port: MidiPortId::FIRST,
            channel: 0,
            kind,
        };
        render.play();
        render.apply_midi(&[key(MidiKind::NoteOn {
            note: 60,
            velocity: 100,
        })]);
        render.process_block(256);
        render.apply_midi(&[key(MidiKind::NoteOff { note: 60 })]);
        render.process_block(256);
        let recorded: Vec<_> = std::iter::from_fn(|| render.pop_outgoing()).collect();
        assert!(
            matches!(recorded[..], [EngineEvent::RecordedNote { note: 60, .. }]),
            "an armed session records through the installed renderer, got {recorded:?}"
        );
    }

    /// An install carries the incoming project's routing as its own: the
    /// incoming renderer reads it from its first block, the outgoing one keeps
    /// the routing its own channel order was built for, and a write after the
    /// install reaches only the incoming one.
    ///
    /// Until 2026-09-17 nothing republished the routing after an install, so
    /// removing, moving or pasting a channel left later channels reading
    /// another channel's input setting until the port list next changed.
    #[test]
    fn an_install_carries_its_own_routing() {
        let listen = |channel| MidiInputRoute {
            source: MidiRouteSource::AllPorts,
            channel: MidiChannelFilter::One(channel),
        };
        let note_on_ten = |note| {
            [MidiMessage {
                offset: 0,
                port: MidiPortId::FIRST,
                channel: 9,
                kind: MidiKind::NoteOn {
                    note,
                    velocity: 100,
                },
            }]
        };
        let mut project = Project::default();
        project.channels.push(ProjectChannel::sampler(1, 1));
        let startup = SharedCells::new();
        startup.keyboard_channel.store(0, Ordering::Relaxed);

        // The outgoing generation: channel 0 listens on MIDI channel 10.
        let (mut outgoing, live) = prepare_render_state(
            &startup,
            48_000,
            render::channel_audio_bank(Vec::new()),
            &project,
            &InputState {
                midi_routing: vec![listen(9), listen(0)],
                ..InputState::default()
            },
        );
        // The incoming one, after the two channels swapped places.
        let (mut incoming, live) = prepare_render_state(
            &live,
            48_000,
            render::channel_audio_bank(Vec::new()),
            &project,
            &InputState {
                midi_routing: vec![listen(0), listen(9)],
                ..InputState::default()
            },
        );

        incoming.apply_midi(&note_on_ten(60));
        assert_eq!(incoming.audition_channels(), vec![1], "the incoming order");
        outgoing.apply_midi(&note_on_ten(60));
        assert_eq!(outgoing.audition_channels(), vec![0], "the outgoing order");

        // A later write -- a picker change, a port appearing -- follows the
        // install down the ordered stream, so it reaches only the renderer that
        // is live by then. Another pitch, so the press does not also release
        // the first one.
        drop(live);
        drop(incoming.apply_structural(StructuralCommand::SetMidiRouting(Box::new(
            render::MidiRouting {
                routes: vec![listen(3), listen(3)],
            },
        ))));
        incoming.process_block(64);
        outgoing.process_block(64);
        incoming.apply_midi(&note_on_ten(62));
        outgoing.apply_midi(&note_on_ten(62));
        assert_eq!(incoming.audition_channels(), Vec::<u8>::new(), "nothing listens on 10 now");
        assert_eq!(outgoing.audition_channels(), vec![0]);
    }

    /// **The audio input routing is attached by the install**, and it is the
    /// incoming generation's own. `audio-recording/02` asks for this test
    /// before anything reads the cell, because leaving the MIDI routing cell
    /// out of the install is the bug 2026-09-17 found, and this cell has the
    /// same shape.
    #[test]
    fn an_install_carries_its_own_audio_input_routing() {
        use mooloop_core::AudioTap;
        let project = Project::default();
        let startup = SharedCells::new();
        let before = vec![Some(AudioTap::Master)];
        let after = vec![Some(AudioTap::Channel(0))];

        let (outgoing, live) = prepare_render_state(
            &startup,
            48_000,
            render::channel_audio_bank(Vec::new()),
            &project,
            &InputState {
                audio_input: before.clone(),
                ..InputState::default()
            },
        );
        assert_eq!(outgoing.audio_input_taps(), before);

        let (incoming, live) = prepare_render_state(
            &live,
            48_000,
            render::channel_audio_bank(Vec::new()),
            &project,
            &InputState::default(),
        );
        assert!(incoming.audio_input_taps().is_empty(), "the incoming project's own");
        assert_eq!(outgoing.audio_input_taps(), before, "the outgoing keeps its own");

        // A later write -- a picker change -- follows the install down the
        // ordered stream and reaches only the renderer live by then.
        drop(live);
        let mut incoming = incoming;
        drop(incoming.apply_structural(StructuralCommand::SetAudioInputRouting(Box::new(
            render::AudioInputRouting { taps: after.clone() },
        ))));
        assert_eq!(incoming.audio_input_taps(), after);
        assert_eq!(outgoing.audio_input_taps(), before);
    }
}
