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

use crate::sampler::SamplerParams;

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
    /// Select which pattern the transport loops (live-switchable).
    SetCurrentPattern(u8),
    /// Set one pattern's logical length. Storage is pre-allocated, so this is
    /// a bounded mutation on the realtime thread.
    SetPatternLength { pattern: u8, length_steps: u16 },
    /// Grow the channel pool's active region by one (appends a channel).
    AddChannel,
    /// Shrink the channel pool's active region by one (removes the last
    /// channel). Kept last-index-only so existing indices stay valid.
    RemoveChannel,
    /// Mute/unmute a channel.
    SetChannelMuted { channel: u8, muted: bool },
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
    /// Replace a channel's sampler parameter set.
    SetChannelSamplerParams { channel: u8, params: SamplerParams },
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
}
