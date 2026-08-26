//! Sampler device parameters. Pure data so the bridge can carry them.

pub const MAX_SAMPLER_VOICES: u8 = 16;
pub const MAX_CHOKE_GROUP: u8 = 16;

/// How the sampler treats the loop region once the play head reaches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopMode {
    /// No looping. Play from `start` to `loop_end` (or sample end), then stop.
    #[default]
    Off,
    /// Loop forward: wrap from `loop_end` back to `loop_start`.
    Forward,
    /// Loop ping-pong: reverse direction at both loop points.
    Pingpong,
}

impl LoopMode {
    pub fn all() -> [LoopMode; 3] {
        [LoopMode::Off, LoopMode::Forward, LoopMode::Pingpong]
    }

    pub fn label(self) -> &'static str {
        match self {
            LoopMode::Off => "Off",
            LoopMode::Forward => "Fwd",
            LoopMode::Pingpong => "Pong",
        }
    }
}

/// How note-off events affect sample playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceMode {
    /// Play the full region. For a looped voice, note-off exits the loop and
    /// lets the remaining sample tail play once.
    #[default]
    OneShot,
    /// Note-off enters the amplitude envelope's release stage.
    Gate,
}

/// How repeated notes of the same pitch use the voice pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetriggerMode {
    /// Replace the oldest active voice on the same pitch.
    #[default]
    Restart,
    /// Allow repeated pitches to overlap up to the polyphony limit.
    Layer,
}

/// All sampler parameters, in the units the DSP and UI share. All points are

impl LoopMode {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Forward,
            2 => Self::Pingpong,
            _ => Self::Off,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Off => 0,
            Self::Forward => 1,
            Self::Pingpong => 2,
        }
    }
}

impl VoiceMode {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Gate,
            _ => Self::OneShot,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::OneShot => 0,
            Self::Gate => 1,
        }
    }
}

impl RetriggerMode {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Layer,
            _ => Self::Restart,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Restart => 0,
            Self::Layer => 1,
        }
    }
}

/// fractions of the sample length in `[0, 1]`; times are seconds.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SamplerParams {
    pub voice_mode: VoiceMode,
    /// Active voice limit in `1..=16`.
    pub polyphony: u8,
    pub retrigger_mode: RetriggerMode,
    /// `0` disables choking; matching non-zero groups choke each other.
    pub choke_group: u8,
    /// Play start point as a fraction of the sample length.
    pub start: f32,
    /// Play end point as a fraction of the sample length.
    pub end: f32,
    /// Play the selected region backwards.
    pub reverse: bool,
    /// Root MIDI note used for keyboard tracking.
    pub root_note: u8,
    /// Coarse tuning offset in semitones.
    pub tune_semitones: f32,
    /// Fine tuning offset in cents.
    pub tune_cents: f32,
    /// Loop start point as a fraction.
    pub loop_start: f32,
    /// Loop end point as a fraction.
    pub loop_end: f32,
    pub loop_mode: LoopMode,
    /// Attack time (seconds).
    pub attack: f32,
    /// Decay time (seconds).
    pub decay: f32,
    /// Sustain level in `[0, 1]`.
    pub sustain: f32,
    /// Release time (seconds).
    pub release: f32,
    /// Low-pass cutoff on a perceptual `[0, 1]` scale. `1` bypasses it.
    pub filter_cutoff: f32,
    /// Low-pass resonance in `[0, 1]`.
    pub filter_resonance: f32,
    /// Bipolar filter envelope depth in `[-1, 1]` (up to six octaves).
    pub filter_env_amount: f32,
    /// Soft saturation drive in `[0, 1]`. `0` bypasses it.
    pub drive: f32,
    /// Bit-depth reduction amount in `[0, 1]`. `0` bypasses it.
    pub bit_reduction: f32,
    /// Sample-rate reduction amount in `[0, 1]`. `0` bypasses it.
    pub rate_reduction: f32,
}

impl Default for SamplerParams {
    fn default() -> Self {
        Self {
            voice_mode: VoiceMode::OneShot,
            polyphony: 1,
            retrigger_mode: RetriggerMode::Restart,
            choke_group: 0,
            start: 0.0,
            end: 1.0,
            reverse: false,
            root_note: 60,
            tune_semitones: 0.0,
            tune_cents: 0.0,
            loop_start: 0.0,
            loop_end: 1.0,
            loop_mode: LoopMode::Off,
            attack: 0.001,
            decay: 0.25,
            sustain: 1.0,
            release: 0.05,
            filter_cutoff: 1.0,
            filter_resonance: 0.0,
            filter_env_amount: 0.0,
            drive: 0.0,
            bit_reduction: 0.0,
            rate_reduction: 0.0,
        }
    }
}

/// Clamp helper used by both DSP (defensive) and UI (input validation).
pub fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}
