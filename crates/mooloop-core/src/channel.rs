//! Channel model. A channel is a source, its own insert chain, and an output
//! stage that feeds one mixer bus. The buses themselves live in `mixer`.

/// Upper bound on simultaneous channels. The engine pre-allocates its pools
/// to this size so channel add/remove never allocates on the RT thread.
pub const MAX_CHANNELS: usize = 16;

/// Upper bound on stored patterns. Pattern IDs cross the realtime bridge as
/// `u8`, so 256 is the complete addressable bank rather than a UI limit.
pub const MAX_PATTERNS: usize = u8::MAX as usize + 1;

/// Upper bound on effects in one channel's chain. Chain slots cross the
/// realtime bridge as `u8`, so this stays well under 256.
pub const MAX_EFFECTS_PER_CHANNEL: usize = 8;

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
    /// Mixer bus this channel feeds. Defaulted on load so songs written before
    /// the mixer existed land on the master.
    #[serde(default)]
    pub bus: u8,
}

impl Channel {
    pub fn new(name: impl Into<String>, kind: DeviceKind) -> Self {
        Self {
            name: name.into(),
            kind,
            muted: false,
            volume: 0.8,
            pan: 0.0,
            bus: crate::MASTER_BUS,
        }
    }
}
