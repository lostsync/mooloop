//! Channel model. A channel is a mixer strip with an instrument device chain
//! (Phase 2: a single sampler per channel; effect chaining lands in Phase 3).

/// Upper bound on simultaneous channels. The engine pre-allocates its pools
/// to this size so channel add/remove never allocates on the RT thread.
pub const MAX_CHANNELS: usize = 16;

/// Upper bound on stored patterns. Pattern IDs cross the realtime bridge as
/// `u8`, so 256 is the complete addressable bank rather than a UI limit.
pub const MAX_PATTERNS: usize = u8::MAX as usize + 1;

/// Instrument kind for a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Sampler,
    DrumSynth,
    MonoSynth,
}

/// One mixer channel.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Channel {
    pub name: String,
    pub kind: DeviceKind,
    pub muted: bool,
    /// Linear output volume in [0, 1].
    pub volume: f32,
    /// Stereo pan in [-1, 1].
    pub pan: f32,
}

impl Channel {
    pub fn new(name: impl Into<String>, kind: DeviceKind) -> Self {
        Self {
            name: name.into(),
            kind,
            muted: false,
            volume: 0.8,
            pan: 0.0,
        }
    }
}
