//! Public retained-audio buffer event contract.
//!
//! These types intentionally live in `mooloop-core`: UI/debug controls and
//! the realtime engine can exchange an edit without either depending on DSP.

/// How long a detached read head remains active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BufferDuration {
    Steps(u16),
    UntilNextEvent,
    Gate,
}

/// One atomic, beat-relative retained-audio edit.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BufferEvent {
    pub offset_beats: f32,
    pub rate: f32,
    pub window_beats: Option<f32>,
    pub repeat: Option<u32>,
    pub duration: BufferDuration,
    pub crossfade_ms: f32,
}

impl BufferEvent {
    pub const fn live() -> Self {
        Self {
            offset_beats: 0.0,
            rate: 1.0,
            window_beats: None,
            repeat: None,
            duration: BufferDuration::UntilNextEvent,
            crossfade_ms: 2.5,
        }
    }
}
