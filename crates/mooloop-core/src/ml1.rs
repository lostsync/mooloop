//! Parameters for the ML-1.
//!
//! A separate instrument from [`crate::MonoSynthParams`], not an extension of
//! it. The v1 mono synth is Poly with the voice count set to one; this one is
//! a filter and performance instrument, per
//! `docs/plans/mono-synth-v2/01-what-mono-is.md`. It keeps the three-
//! oscillator front end because that is genuinely shared, and diverges
//! everywhere else:
//!
//! - two envelopes, so filter motion is independent of amplitude motion,
//! - filter keytracking, so a patch voiced at C2 is not a thud at C5,
//! - no device-local LFO. General modulation is channel state and reaches
//!   these parameters through the `ModRack` and the descriptor event path.
//!
//! Because this struct has no pre-v2 form on disk, it carries
//! `#[serde(default)]` from the start rather than acquiring it after the
//! first field addition breaks every saved project.

use crate::OscParams;

/// Which held note the single voice plays when several are down.
///
/// `Last` is what the v1 synth does and is the conservative default; `Low` and
/// `High` are the other two standard answers, and having all three is part of
/// what separates a monosynth from a synth with one voice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NotePriority {
    #[default]
    Last,
    Low,
    High,
}

/// Whether an overlapping note restarts the envelopes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EnvTrigger {
    /// Every note starts both envelopes. Matches the v1 synth exactly.
    #[default]
    Retrig,
    /// An overlapping note changes pitch and leaves the envelopes running.
    Legato,
}

/// When glide applies.
///
/// Both modes glide between overlapping notes and neither glides from
/// silence. They differ only over a still-sounding release tail: `Always`
/// slides into it, `Legato` jumps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GlideMode {
    Always,
    #[default]
    Legato,
}

impl NotePriority {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Low,
            2 => Self::High,
            _ => Self::Last,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Last => 0,
            Self::Low => 1,
            Self::High => 2,
        }
    }
}

impl EnvTrigger {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Legato,
            _ => Self::Retrig,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Retrig => 0,
            Self::Legato => 1,
        }
    }
}

impl GlideMode {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Legato,
            _ => Self::Always,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Always => 0,
            Self::Legato => 1,
        }
    }
}

/// Which filter the voice runs. A character choice, not a response-shape
/// menu: all three are low-pass, and they differ in slope, in where the
/// nonlinearity sits, and in what resonance does to the low end.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FilterModel {
    /// Four-pole transistor-ladder-flavoured: heavy, symmetric saturation,
    /// bass held up under resonance.
    #[default]
    Ladder,
    /// Three-pole diode-ladder-flavoured: forward and nasal, asymmetric
    /// saturation, low end squeezed out as resonance rises.
    Acid,
    /// The shared linear state-variable filter: two poles, no saturation, no
    /// character of its own. The one to reach for when the filter should get
    /// out of the way.
    Clean,
}

impl FilterModel {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Acid,
            2 => Self::Clean,
            _ => Self::Ladder,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Ladder => 0,
            Self::Acid => 1,
            Self::Clean => 2,
        }
    }
}

/// All ML-1 parameters, in the units the DSP and UI share.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Ml1Params {
    pub osc: [OscParams; 3],
    /// Portamento time (seconds). `0` is instant pitch changes.
    pub glide: f32,
    /// Amplitude attack time (seconds).
    pub attack: f32,
    /// Amplitude decay time (seconds).
    pub decay: f32,
    /// Amplitude sustain level in `[0, 1]`.
    pub sustain: f32,
    /// Amplitude release time (seconds).
    pub release: f32,
    /// Low-pass cutoff on a perceptual `[0, 1]` scale. `1` bypasses it.
    pub filter_cutoff: f32,
    /// Low-pass resonance in `[0, 1]`.
    pub filter_resonance: f32,
    /// Bipolar filter envelope depth in `[-1, 1]` (up to six octaves).
    pub filter_env_amount: f32,
    /// Soft saturation drive in `[0, 1]`. `0` bypasses it.
    pub drive: f32,
    /// Filter attack time (seconds).
    pub filter_attack: f32,
    /// Filter decay time (seconds). The one that makes plucks and acid work.
    pub filter_decay: f32,
    /// Filter sustain level in `[0, 1]`, independent of [`Self::sustain`].
    pub filter_sustain: f32,
    /// Filter release time (seconds), independent of [`Self::release`].
    pub filter_release: f32,
    /// Keyboard tracking of cutoff in `[0, 1]`. `1` moves the cutoff about one
    /// octave per played octave, referenced to middle C.
    pub filter_keytrack: f32,
    /// When glide applies. Independent of [`Self::env_trigger`] — these are
    /// two switches, not one four-position one.
    pub glide_mode: GlideMode,
    /// Whether an overlapping note restarts the envelopes.
    pub env_trigger: EnvTrigger,
    /// Which held note wins when several are down.
    pub priority: NotePriority,
    /// Which filter the voice runs.
    pub filter_model: FilterModel,
}

impl Default for Ml1Params {
    fn default() -> Self {
        Self {
            osc: [
                OscParams {
                    // Wide open: the default patch IS the reference patch,
                    // calibrated against `gain::REFERENCE_PEAK_DBFS` with the
                    // oscillator's knob at its 0 dB top.
                    level: 1.0,
                    ..OscParams::default()
                },
                OscParams {
                    semitones: 12.0,
                    cents: 4.0,
                    level: 0.0,
                    ..OscParams::default()
                },
                OscParams {
                    semitones: -12.0,
                    cents: -4.0,
                    level: 0.0,
                    ..OscParams::default()
                },
            ],
            glide: 0.0,
            attack: 0.005,
            decay: 0.2,
            sustain: 0.7,
            release: 0.15,
            // The filter starts open and out of the way. Voicing the default
            // patch is the factory-bank step's job, and moving it here would
            // change the peak the gain reference is calibrated against.
            filter_cutoff: 1.0,
            filter_resonance: 0.0,
            filter_env_amount: 0.0,
            drive: 0.0,
            filter_attack: 0.005,
            filter_decay: 0.2,
            filter_sustain: 0.7,
            filter_release: 0.15,
            filter_keytrack: 0.0,
            // `Retrig` reproduces the v1 synth exactly. `Legato` glide does
            // not, but glide defaults to 0 s, so nothing can hear the
            // difference until it is dialled in — and by then `Legato` is the
            // mode a player wants.
            glide_mode: GlideMode::Legato,
            env_trigger: EnvTrigger::Retrig,
            priority: NotePriority::Last,
            filter_model: FilterModel::Ladder,
        }
    }
}
