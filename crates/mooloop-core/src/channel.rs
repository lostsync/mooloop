//! Channel model. A channel is a source, its own insert chain, and an output
//! stage that feeds one mixer bus. The buses themselves live in `mixer`.

/// Complete addressable channel bank.
///
/// Channel indices travel over the realtime bridge as `u8`, so this is a wire
/// format boundary, not a product-design limit. The engine preallocates this
/// bank so channel add/remove never allocates on the RT thread.
pub const MAX_CHANNELS: usize = u8::MAX as usize + 1;

/// Upper bound on stored patterns. Pattern IDs cross the realtime bridge as
/// `u8`, so 256 is the complete addressable bank rather than a UI limit.
pub const MAX_PATTERNS: usize = u8::MAX as usize + 1;

/// Complete addressable effect-chain bank. Chain slots cross the realtime
/// bridge as `u8`; this is therefore a protocol boundary, not an eight-device
/// product cap.
pub const MAX_EFFECTS_PER_CHANNEL: usize = u8::MAX as usize + 1;

/// Largest persisted linear gain for a channel/device output. This is the
/// +12 dB endpoint shared by the UI trim controls; defined in [`crate::gain`]
/// and re-exported here for its historical import path.
pub use crate::gain::MAX_LINEAR_GAIN;

/// Instrument kind for a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Sampler,
    DrumSynth,
    MonoSynth,
    PolySynth,
}

/// One mixer channel.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Channel {
    pub name: String,
    pub kind: DeviceKind,
    pub muted: bool,
    /// Linear output volume in [0, `MAX_LINEAR_GAIN`] (+12 dB).
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
