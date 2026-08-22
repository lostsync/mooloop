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
    DeviceKind, DrumSynthParams, MonoSynthParams, NoteEvent, NoteId, PlaybackMode, SamplerParams,
};

/// GUI -> audio. Drained at the top of each process callback.
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
    /// Grow the channel pool's active region by one (appends a channel).
    AddChannel { source: DeviceKind },
    /// Shrink the channel pool's active region by one (removes the last
    /// channel). Kept last-index-only so existing indices stay valid.
    RemoveChannel,
    /// Mute/unmute a channel.
    SetChannelMuted { channel: u8, muted: bool },
    /// Set a channel's linear output volume in [0, 1].
    SetChannelVolume { channel: u8, volume: f32 },
    /// Set a channel's stereo pan in [-1, 1].
    SetChannelPan { channel: u8, pan: f32 },
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
    /// Swap two slots in a channel's effect chain (drag reorder). Swapping
    /// array entries moves pointers only, so this is safe on the realtime
    /// thread — installing/removing nodes is not, and goes through the
    /// engine's structural command ring instead.
    SwapEffectSlots { channel: u8, slot_a: u8, slot_b: u8 },
    /// Bypass or re-enable one effect slot. While bypassed the slot's
    /// parameter events keep accumulating and flush on re-enable.
    SetEffectBypassed {
        channel: u8,
        slot: u8,
        bypassed: bool,
    },
    /// Queue one sample-timed parameter change for one effect slot. Delivered
    /// to the node as `Event::ParamValue` at the next block, so effect kinds
    /// need no bespoke command per parameter.
    SetEffectParam {
        channel: u8,
        slot: u8,
        id: u32,
        value: f32,
    },
    /// Import the latest project snapshot published through the engine's
    /// out-of-band ArcSwap slot into preallocated realtime storage.
    InstallProject { generation: u64 },
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
