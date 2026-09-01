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
    /// The ML-M1: the mono synth built around its filter and its note
    /// behaviour. `MonoSynth` is the older device it replaces, kept loadable
    /// until its channels have somewhere to migrate to.
    ///
    /// Serialized as `ml1`, not as the `rename_all` default `ml_m1`. The
    /// device shipped under the wrong name, and projects and channel presets
    /// saved before it was corrected carry the old spelling. A serialized
    /// variant name is an on-disk identifier like a parameter id, so it is
    /// frozen rather than corrected; the rename is a source and UI change
    /// only.
    #[serde(rename = "ml1")]
    MlM1,
    /// The ML-P8: eight voices around a three-oscillator network. A new kind
    /// rather than a reinterpretation of [`Self::PolySynth`], which stays as
    /// its own simpler device with its own saved projects.
    ///
    /// Serialized as `mlp8`, chosen rather than inherited: the `rename_all`
    /// default would spell it `ml_p8`, and the ML-M1's history is the reason
    /// to pick a serialized name on purpose the first time. It matches the
    /// preset directory slug, and it is frozen from here.
    #[serde(rename = "mlp8")]
    MlP8,
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
            // Genuinely at unity: the operating-level headroom comes from
            // source calibration (`gain::REFERENCE_PEAK_DBFS`), not from a
            // quiet default fader.
            volume: 1.0,
            pan: 0.0,
            bus: crate::MASTER_BUS,
        }
    }
}
