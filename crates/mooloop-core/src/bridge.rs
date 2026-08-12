//! Lock-free message types exchanged between the GUI and the realtime audio
//! thread via SPSC ring buffers (`rtrb`).
//!
//! The audio thread must never block. Both enums are kept small and POD-like so
//! they can be pushed/popped from a ring buffer without allocation. Large or
//! variable-size payloads (e.g. sample buffers) are transferred out of band
//! via an `ArcSwap` slot owned by the engine, not through this queue.

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
    /// Toggle or set a step in the current pattern.
    SetStep {
        channel: u8,
        step: u8,
        on: bool,
        velocity: u8,
    },
    /// Replace the sampler's parameter set.
    SetSamplerParams(SamplerParams),
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
