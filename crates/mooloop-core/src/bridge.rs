//! Lock-free message types exchanged between the GUI and the realtime audio
//! thread via SPSC ring buffers (`rtrb`).
//!
//! The audio thread must never block or allocate. Both enums are kept small
//! and POD-like so they can be pushed/popped from a ring buffer without
//! allocation. Large or variable-size payloads (e.g. sample buffers) are
//! transferred out of band via `ArcSwap` slots owned by the engine.
//!
//! Channel/pattern indices are bounded by `MAX_CHANNELS`/`MAX_PATTERNS`; the
//! engine pre-allocates pools at startup so these commands only mutate.

use crate::{
    AutomationPoint, BufferEvent, CompiledBusGraph, DeviceKind, DrumSynthParams, EffectTarget,
    ModRoute, ModSourceId, ModulatorParams, MonoSynthParams, MlM1Params, MlP8Params, NoteEvent,
    NoteId,
    ParamAddr, PlaybackMode, PointId, PolySynthParams, SamplerParams,
};

/// GUI -> audio. Drained at the top of each process callback.
// Every entry in the preallocated ring is sized for the widest variant, so
// width here is a fixed startup cost paid 1024 times over -- never a
// per-command allocation, and never a `Box` this enum would have to drop on
// the realtime callback (a deallocation the executor contract forbids,
// `docs/AUDIO_ARCHITECTURE.md`). That is why nothing here is boxed, and why
// what each variant *names* matters: this enum once carried a whole
// `ModRack`, so turning one LFO knob shipped every module and every route in
// the channel, and the ring grew with modulator capacity. The modulation
// variants below name one fact each instead.
//
// Nothing replaces a whole rack live. Project load, undo and the factory
// bank all rebuild the renderer through `EngineHandle::install_project`, so
// the wide command had no caller left once the gestures were narrowed. A
// future channel-preset verb belongs on the structural ring rather than
// here: that ring may carry a `Box` because it has a reclaim path back off
// the audio thread. See
// `docs/plans/modulator-capacity/03-per-slot-commands.md`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EngineCommand {
    /// Begin or resume playback from the current position.
    Play,
    /// Stop and reset transport position to the start.
    Stop,
    /// Pause playback (hold position).
    Pause,
    /// Set tempo in beats per minute.
    SetTempo(f64),
    /// Set global sixteenth-note swing. 50% is straight; 75% is maximum.
    SetSwing(u8),
    /// Select which pattern the transport loops (live-switchable).
    SetCurrentPattern(u8),
    /// Make the next preallocated pattern available to the project.
    AddPattern,
    /// Switch transport scheduling between the selected pattern and playlist.
    SetPlaybackMode(PlaybackMode),
    /// Set one pattern's logical length. Storage is pre-allocated, so this is
    /// a bounded mutation on the realtime thread.
    SetPatternLength { pattern: u8, length_steps: u16 },
    /// Add or remove one pattern placement on the absolute song timeline.
    SetPlaylistPlacement {
        pattern: u8,
        start_tick: u32,
        on: bool,
    },
    /// Shrink the channel pool's active region by one (removes the last
    /// channel). Kept last-index-only so existing indices stay valid.
    RemoveChannel,
    /// Mute/unmute a channel.
    SetChannelMuted { channel: u8, muted: bool },
    /// Set a channel's linear output volume in [0, 1].
    SetChannelVolume { channel: u8, volume: f32 },
    /// Set a channel's stereo pan in [-1, 1].
    SetChannelPan { channel: u8, pan: f32 },
    /// Assign a channel to a mixer bus. Out-of-range buses land on the master.
    SetChannelBus { channel: u8, bus: u8 },
    /// Mute/unmute a mixer bus, silencing everything feeding it.
    SetBusMuted { bus: u8, muted: bool },
    /// Set a bus's linear output volume in [0, 1].
    SetBusVolume { bus: u8, volume: f32 },
    /// Set a bus's stereo pan in [-1, 1].
    SetBusPan { bus: u8, pan: f32 },
    /// Atomically replace bus destinations and the render order compiled for
    /// them. The audio thread installs this fixed-size value and never reasons
    /// about editable graph topology.
    InstallBusGraph { graph: CompiledBusGraph },
    /// Toggle or set a step. Addresses the pattern bank so edits to
    /// non-playing patterns take effect when selected.
    SetStep {
        pattern: u8,
        channel: u8,
        step: u8,
        on: bool,
        note: u8,
        velocity: u8,
    },
    /// Insert or replace a tick-addressed note by stable ID.
    UpsertNote {
        pattern: u8,
        channel: u8,
        note: NoteEvent,
    },
    /// Remove a tick-addressed note by stable ID.
    RemoveNote {
        pattern: u8,
        channel: u8,
        id: NoteId,
    },
    /// Open an automation lane on `target`, or do nothing if it is already
    /// open. Lane storage is preallocated, so this only ever fails by being
    /// ignored when a channel's pattern already holds the maximum.
    OpenAutomationLane {
        pattern: u8,
        channel: u8,
        target: ParamAddr,
    },
    /// Drop a lane and its points. The destination returns to its knob value
    /// on the next block.
    RemoveAutomationLane {
        pattern: u8,
        channel: u8,
        target: ParamAddr,
    },
    /// Empty a lane without closing it. An empty lane has no opinion, so the
    /// destination also returns to its knob value.
    ClearAutomationLane {
        pattern: u8,
        channel: u8,
        target: ParamAddr,
    },
    /// Insert or replace one breakpoint by stable ID. `point.value` is
    /// normalized against the destination's descriptor, never natural units.
    UpsertAutomationPoint {
        pattern: u8,
        channel: u8,
        target: ParamAddr,
        point: AutomationPoint,
    },
    /// Remove one breakpoint by stable ID.
    RemoveAutomationPoint {
        pattern: u8,
        channel: u8,
        target: ParamAddr,
        id: PointId,
    },
    /// Sound one note on a channel's generator, now.
    ///
    /// The audition primitive: there was no UI-to-engine path to play a note
    /// on a channel at all before slicing needed one. The browser's preview
    /// voice goes straight to the master with no channel strip, so it cannot
    /// audition a slice through the envelopes, filter and drive the slice
    /// will actually play through. This is also what an on-screen keyboard
    /// needs.
    TriggerChannelNote { channel: u8, note: u8, velocity: u8 },
    /// Release a note started by [`EngineCommand::TriggerChannelNote`].
    ReleaseChannelNote { channel: u8, note: u8 },
    /// Replace a channel's sampler parameter set.
    SetChannelSamplerParams { channel: u8, params: SamplerParams },
    /// Replace a channel's sound source while retaining its mixer strip.
    SetChannelSource { channel: u8, source: DeviceKind },
    /// Replace a channel's drum synth parameter set.
    SetChannelDrumSynthParams {
        channel: u8,
        params: DrumSynthParams,
    },
    /// Replace a channel's mono synth parameter set.
    SetChannelMonoSynthParams {
        channel: u8,
        params: MonoSynthParams,
    },
    /// Replace a channel's ML-M1 parameter set.
    SetChannelMlM1Params {
        channel: u8,
        params: MlM1Params,
    },
    /// Replace a channel's ML-P8 parameter set.
    SetChannelMlP8Params {
        channel: u8,
        params: MlP8Params,
    },
    /// Replace a channel's poly synth parameter set.
    SetChannelPolySynthParams {
        channel: u8,
        params: PolySynthParams,
    },
    /// Swap two slots in a channel's effect chain (drag reorder). Swapping
    /// array entries moves pointers only, so this is safe on the realtime
    /// thread — installing/removing nodes is not, and goes through the
    /// engine's structural command ring instead.
    SwapEffectSlots {
        target: EffectTarget,
        slot_a: u8,
        slot_b: u8,
    },
    /// Bypass or re-enable one effect slot. While bypassed the slot's
    /// parameter events keep accumulating and flush on re-enable.
    SetEffectBypassed {
        target: EffectTarget,
        slot: u8,
        bypassed: bool,
    },
    /// Generic device-host wet/dry blend, applied around every insert.
    SetEffectWetDry {
        target: EffectTarget,
        slot: u8,
        wet_dry: f32,
    },
    /// Generic device-host input trim, applied before the effect DSP.
    SetEffectInputTrim {
        target: EffectTarget,
        slot: u8,
        input_trim: f32,
    },
    /// Generic device-host output trim, applied after the wet/dry blend.
    SetEffectOutputTrim {
        target: EffectTarget,
        slot: u8,
        output_trim: f32,
    },
    /// Queue one sample-timed parameter change for one effect slot. Delivered
    /// to the node as `Event::ParamValue` at the next block, so effect kinds
    /// need no bespoke command per parameter.
    SetEffectParam {
        target: EffectTarget,
        slot: u8,
        id: u32,
        value: f32,
    },
    /// Set one modulator parameter by descriptor id. This is the ordinary
    /// modulation edit — a knob drag, a selector click — and it names the
    /// fact that changed rather than shipping the rack it lives in.
    SetModulatorParam {
        channel: u8,
        slot: u8,
        id: u32,
        value: f32,
    },
    /// Put a module in one slot, under the identity the authoring rack
    /// minted. Retuning a slot that already holds the same kind preserves its
    /// running state (an LFO keeps its phase, a sequencer its cursor); a kind
    /// change rebuilds it.
    InstallModulator {
        channel: u8,
        slot: u8,
        source: ModSourceId,
        params: ModulatorParams,
    },
    /// Empty one slot and drop every route it drove, restoring each orphaned
    /// destination to its base at the next block.
    ClearModulator { channel: u8, slot: u8 },
    /// Move a module to another grid position, compacting the rack. Both
    /// racks run the same permutation, so routes and a math module's input
    /// slot stay pointed at the same modules on either side.
    MoveModulator { channel: u8, from: u8, to: u8 },
    /// Add or retune one route. The route names its source by durable id, so
    /// one that arrives before the module it names is inert rather than
    /// misaimed at whatever occupies that slot.
    SetModRoute { channel: u8, route: ModRoute },
    /// Drop one route by identity and restore its destination's base at the
    /// next block.
    RemoveModRoute {
        channel: u8,
        source: ModSourceId,
        destination: ParamAddr,
    },
    /// Fire one complete retained-audio edit at the start of the next block.
    /// The tuple is never split into parameter updates, so the read head sees
    /// one sample-accurate change.
    TriggerBuffer {
        target: EffectTarget,
        slot: u8,
        event: BufferEvent,
    },
    /// Release a gated retained-audio edit. Only an event whose duration is
    /// `Gate` responds, so a held control can send this on release without
    /// having to know what has happened to the head since.
    ReleaseBuffer { target: EffectTarget, slot: u8 },
}

/// audio -> GUI. Pushed sparingly (a few times per block at most) and drained
/// by a timer on the UI thread.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EngineEvent {
    /// Current transport position, pushed roughly once per block.
    Position {
        tick: u64,
        beat_in_bar: u8,
        playing: bool,
    },
    /// Output peak meters for the UI's level display.
    Metering { peak_l: f32, peak_r: f32 },
    /// An xrun (buffer overrun/underrun) was reported by JACK.
    Xrun,
    /// Internal acknowledgement used to reclaim a replaced project snapshot
    /// on the non-realtime thread.
    ProjectInstalled { generation: u64 },
}
