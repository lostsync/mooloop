//! Parameters for the DS-01.
//!
//! A second drum instrument beside [`crate::DrumSynthParams`], not a
//! migration of it, per `docs/plans/drum-synth-v2/`. The v1 device keeps its
//! kind, its fields, and its saved projects.
//!
//! ## Why a new device rather than a table over the old one
//!
//! `DrumSynthParams` is a mode-union: `mode` selects Kick, Snare or Hat and
//! roughly two thirds of the struct is inert at any moment. A flat descriptor
//! table over that union produces parameter ids whose meaning depends on a
//! discrete selector, so a modulation route could address a live parameter
//! and then silently stop doing anything when the mode changed. DS-01 has one
//! universal voice instead: every control is live in every configuration, so
//! one id means one thing forever. See
//! `docs/plans/drum-synth-v2/00-status.md`.
//!
//! ## Its own parameter id space
//!
//! Ids start at zero here, in bands reserved for the whole plan, and are
//! never renumbered — automation and modulation routes persist them. The
//! bands:
//!
//! ```text
//!   0-9     Global              step 02
//!   10-19   Tone                step 02
//!   20-29   Noise               step 02
//!   30-39   Body                step 04
//!   40-49   Amp envelope        step 02, extended in 03
//!   50-59   Pitch envelope      step 02, extended in 03
//!   60-69   Noise envelope      step 03
//!   70-79   Mod envelope        step 03
//!   80-89   Burst               step 05
//!   90-99   Shape and output    step 06
//!   100-131 Matrix, 8 x 4       step 07
//! ```
//!
//! A step may only assign inside its own band. The gaps are reservations
//! under review, not free ids: spending one early is a renumbering later.

use crate::generator::{stepped, unit};
use crate::{ParamCurve, ParamDescriptor, MAX_CHOKE_GROUP};

// --- Parameter ids ---------------------------------------------------------

pub const PARAM_TUNE: u32 = 0;
pub const PARAM_LEVEL: u32 = 1;
pub const PARAM_CHOKE_GROUP: u32 = 2;
pub const PARAM_CHOKE_TIME: u32 = 3;
pub const PARAM_RETRIGGER: u32 = 4;
pub const PARAM_VELOCITY_AMOUNT: u32 = 5;

pub const PARAM_TONE_LEVEL: u32 = 10;
pub const PARAM_TONE_PITCH: u32 = 11;
pub const PARAM_TONE_WAVE: u32 = 12;
pub const PARAM_TONE_PARTIALS: u32 = 13;
pub const PARAM_TONE_SPREAD: u32 = 14;
pub const PARAM_TONE_FM_AMOUNT: u32 = 15;
pub const PARAM_TONE_FM_RATIO: u32 = 16;

pub const PARAM_NOISE_LEVEL: u32 = 20;
pub const PARAM_NOISE_COLOR: u32 = 21;
pub const PARAM_NOISE_RATE: u32 = 22;
pub const PARAM_FILTER_MORPH: u32 = 23;
pub const PARAM_FILTER_CUTOFF: u32 = 24;
pub const PARAM_FILTER_RES: u32 = 25;

// One envelope type used four times, so the offsets inside a block are named
// once. The pitch envelope is the exception and has its own four ids: it has
// no gate, so hold, sustain and release would be three controls that do
// nothing, which is exactly what `01-what-ds01-is.md` forbids.
pub const ENV_OFFSET_ATTACK: u32 = 0;
pub const ENV_OFFSET_HOLD: u32 = 1;
pub const ENV_OFFSET_DECAY: u32 = 2;
pub const ENV_OFFSET_CURVE: u32 = 3;
pub const ENV_OFFSET_SUSTAIN: u32 = 4;
pub const ENV_OFFSET_RELEASE: u32 = 5;
pub const ENV_OFFSET_GATE: u32 = 6;

/// Controls in one gated envelope block.
pub const ENV_BLOCK: u32 = 7;

pub const PARAM_AMP_ENV_BASE: u32 = 40;
pub const PARAM_NOISE_ENV_BASE: u32 = 60;
pub const PARAM_MOD_ENV_BASE: u32 = 70;

/// The amplitude envelope's decay. The id step 02 assigned before the
/// envelope had a shape, and it keeps it.
pub const PARAM_AMP_DECAY: u32 = PARAM_AMP_ENV_BASE + ENV_OFFSET_DECAY;

pub const PARAM_PITCH_ATTACK: u32 = 50;
pub const PARAM_PITCH_DECAY: u32 = 51;
pub const PARAM_PITCH_CURVE: u32 = 52;
pub const PARAM_PITCH_DEPTH: u32 = 53;

/// Voice slots. Eight, shared with the v1 drum synth's pool, because a drum
/// channel plays one drum and eight simultaneous hits of it is already more
/// than a pattern asks for.
pub const DS01_VOICES: usize = crate::MAX_DRUM_VOICES as usize;

/// Most partials the tone bank can run.
pub const DS01_MAX_PARTIALS: u8 = 6;

// --- Structural selectors --------------------------------------------------

/// What a new hit does to this channel's sounding hits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ds01Retrigger {
    /// Hits stack, which is v1's behaviour and stays the default so nothing
    /// about the feel of a fast pattern changes by accident.
    #[default]
    Poly,
    /// A new hit chokes the previous one at Choke Time. What a real 808 does,
    /// and what a fast hat pattern usually wants.
    Mono,
}

impl Ds01Retrigger {
    pub const ALL: [Self; 2] = [Self::Poly, Self::Mono];

    pub fn from_index(index: i32) -> Self {
        Self::ALL
            .get(index.clamp(0, Self::ALL.len() as i32 - 1) as usize)
            .copied()
            .unwrap_or_default()
    }

    pub fn to_index(self) -> i32 {
        Self::ALL
            .iter()
            .position(|mode| *mode == self)
            .unwrap_or_default() as i32
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Poly => "Poly",
            Self::Mono => "Mono",
        }
    }
}

/// Which noise generator the noise layer runs.
///
/// Structural because the generators differ rather than being one generator
/// under a tilt control: changing colour between hits is fine and changing it
/// mid-hit is not defined. [`Ds01Params::noise_rate`] is the control that
/// stays live in every colour, so the section is never inert.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ds01NoiseColor {
    #[default]
    White,
    /// Flat per octave. The body of a snare and the wash of a cymbal.
    Pink,
    /// Sparse random impulses. Crackle, vinyl, and brushes.
    Velvet,
    /// A ring-modulated square cluster. Cymbal grit, and the 808 hat's metal
    /// reached as a patch rather than as two hardcoded frequencies.
    Metal,
}

impl Ds01NoiseColor {
    pub const ALL: [Self; 4] = [Self::White, Self::Pink, Self::Velvet, Self::Metal];

    pub fn from_index(index: i32) -> Self {
        Self::ALL
            .get(index.clamp(0, Self::ALL.len() as i32 - 1) as usize)
            .copied()
            .unwrap_or_default()
    }

    pub fn to_index(self) -> i32 {
        Self::ALL
            .iter()
            .position(|color| *color == self)
            .unwrap_or_default() as i32
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::White => "White",
            Self::Pink => "Pink",
            Self::Velvet => "Velvet",
            Self::Metal => "Metal",
        }
    }
}

// --- Descriptor helpers ----------------------------------------------------

const fn hz(
    id: u32,
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
) -> ParamDescriptor {
    ParamDescriptor {
        id,
        name,
        unit: "Hz",
        min,
        max,
        curve: ParamCurve::Exponential,
        default,
    }
}

/// A time in seconds on its own range. Not `generator::seconds`, whose fixed
/// 1 ms - 8 s span is the shared synth envelope range: a drum's segments each
/// want their own ends, and a choke is not an envelope stage at all.
const fn time_s(id: u32, name: &'static str, min: f32, max: f32, default: f32) -> ParamDescriptor {
    ParamDescriptor {
        id,
        name,
        unit: "s",
        min,
        max,
        curve: ParamCurve::Exponential,
        default,
    }
}

/// A segment that has to be able to be *zero*, and is therefore linear.
///
/// The plan asks for a log taper on Attack and Hold, and `ParamCurve` cannot
/// give one that includes zero: [`ParamCurve::Exponential`] is a ratio sweep
/// and its bottom is `min`, which must be positive. Of the two properties,
/// zero is the one the plan calls non-negotiable — "a drum synth whose attack
/// cannot be zero is broken" — so it wins, and the taper is left to the
/// control surface, which is where a taper belongs anyway.
const fn segment_s(id: u32, name: &'static str, max: f32) -> ParamDescriptor {
    ParamDescriptor {
        id,
        name,
        unit: "s",
        min: 0.0,
        max,
        curve: ParamCurve::Linear,
        default: 0.0,
    }
}

const fn bipolar(id: u32, name: &'static str, default: f32) -> ParamDescriptor {
    ParamDescriptor {
        id,
        name,
        unit: "",
        min: -1.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default,
    }
}

const fn semitones(
    id: u32,
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
) -> ParamDescriptor {
    ParamDescriptor {
        id,
        name,
        unit: "st",
        min,
        max,
        curve: ParamCurve::Linear,
        default,
    }
}

// --- The table -------------------------------------------------------------

const GLOBAL_DESCRIPTORS: [ParamDescriptor; 6] = [
    // Stepped, and therefore not a modulation destination. That is the right
    // answer rather than an accident of the curve: Tune is which note this
    // drum is, latched with the note itself, and the continuous pitch
    // controls a route wants are Tone Pitch and Pitch Depth.
    ParamDescriptor {
        id: PARAM_TUNE,
        name: "Tune",
        unit: "st",
        min: -48.0,
        max: 48.0,
        curve: ParamCurve::Stepped(97),
        default: 0.0,
    },
    unit(PARAM_LEVEL, "Level", 0.8),
    ParamDescriptor {
        id: PARAM_CHOKE_GROUP,
        name: "Choke grp",
        unit: "",
        min: 0.0,
        max: MAX_CHOKE_GROUP as f32,
        curve: ParamCurve::Stepped(MAX_CHOKE_GROUP + 1),
        default: 0.0,
    },
    time_s(PARAM_CHOKE_TIME, "Choke time", 0.001, 0.5, 0.005),
    stepped(
        PARAM_RETRIGGER,
        "Retrigger",
        Ds01Retrigger::ALL.len() as u8,
        0.0,
    ),
    unit(PARAM_VELOCITY_AMOUNT, "Vel amt", 1.0),
];

const TONE_DESCRIPTORS: [ParamDescriptor; 7] = [
    unit(PARAM_TONE_LEVEL, "Tone level", 1.0),
    hz(PARAM_TONE_PITCH, "Tone pitch", 20.0, 8_000.0, 160.0),
    // A morph and not a four-way selector, for a reason that is not
    // cosmetic: a selector would be a structural discrete and therefore
    // modulation-ineligible, and sweeping timbre across a hit is a
    // percussion gesture rather than a setup choice.
    unit(PARAM_TONE_WAVE, "Tone wave", 0.0),
    ParamDescriptor {
        id: PARAM_TONE_PARTIALS,
        name: "Partials",
        unit: "",
        min: 1.0,
        max: DS01_MAX_PARTIALS as f32,
        curve: ParamCurve::Stepped(DS01_MAX_PARTIALS),
        default: 1.0,
    },
    unit(PARAM_TONE_SPREAD, "Spread", 0.5),
    unit(PARAM_TONE_FM_AMOUNT, "FM amt", 0.0),
    ParamDescriptor {
        id: PARAM_TONE_FM_RATIO,
        name: "FM ratio",
        unit: "",
        min: 0.25,
        max: 16.0,
        curve: ParamCurve::Exponential,
        default: 2.0,
    },
];

const NOISE_DESCRIPTORS: [ParamDescriptor; 6] = [
    // Zero, so the default patch is a clean tone hit and the first thing
    // anyone does to it is add something.
    unit(PARAM_NOISE_LEVEL, "Noise level", 0.0),
    stepped(
        PARAM_NOISE_COLOR,
        "Color",
        Ds01NoiseColor::ALL.len() as u8,
        0.0,
    ),
    // The rate reducer applies to every colour, so it is the one noise
    // control that is never inert. Its top is the sample rate the device is
    // calibrated at; above the running rate it is simply transparent.
    hz(PARAM_NOISE_RATE, "Rate", 500.0, 48_000.0, 48_000.0),
    unit(PARAM_FILTER_MORPH, "Morph", 1.0),
    hz(PARAM_FILTER_CUTOFF, "Cutoff", 20.0, 18_000.0, 7_500.0),
    unit(PARAM_FILTER_RES, "Reso", 0.1),
];

/// Longest attack or hold. Half a second is already past a drum and into a
/// swell, which is the point: the top of the range is where the envelope
/// stops being percussive.
const SEGMENT_MAX_S: f32 = 0.5;

/// One gated envelope's seven controls.
///
/// The same type four times over — three times here and once, without the
/// gate half, as the pitch envelope. v1 had a single `ExpDecay`: no attack,
/// no hold, no curve, one rate law. That is the largest single reason its
/// snare and its hat differ mostly in noise content, and it is what this
/// block replaces.
const fn env_block(
    base: u32,
    names: [&'static str; ENV_BLOCK as usize],
    decay_default: f32,
) -> [ParamDescriptor; ENV_BLOCK as usize] {
    [
        segment_s(base + ENV_OFFSET_ATTACK, names[0], SEGMENT_MAX_S),
        segment_s(base + ENV_OFFSET_HOLD, names[1], SEGMENT_MAX_S),
        time_s(base + ENV_OFFSET_DECAY, names[2], 0.002, 8.0, decay_default),
        // -1 logarithmic, 0 exponential (v1's law), +1 linear. One control
        // shaping the normalized output rather than three integrators, so it
        // is continuous across zero and costs one latched value a hit.
        bipolar(base + ENV_OFFSET_CURVE, names[3], 0.0),
        unit(base + ENV_OFFSET_SUSTAIN, names[4], 0.0),
        time_s(base + ENV_OFFSET_RELEASE, names[5], 0.002, 4.0, 0.1),
        stepped(base + ENV_OFFSET_GATE, names[6], 2, 0.0),
    ]
}

/// The pitch envelope: attack, decay, curve, and a bipolar depth.
///
/// No gate, and therefore no hold, sustain or release. A pitch envelope that
/// held its excursion for the length of a note would be a transposition, not
/// a sweep, and the three controls it would take to say so would all be inert
/// with the gate off.
const PITCH_DESCRIPTORS: [ParamDescriptor; 4] = [
    // Attack lets a pitch *rise* into the hit, which is the reverse-swell and
    // the reversed tom.
    segment_s(PARAM_PITCH_ATTACK, "Pitch attack", SEGMENT_MAX_S),
    time_s(PARAM_PITCH_DECAY, "Pitch decay", 0.001, 2.0, 0.045),
    bipolar(PARAM_PITCH_CURVE, "Pitch curve", 0.0),
    // Bipolar and in semitones, which is the one place DS-01 refuses to copy
    // v1. v1 spells the kick sweep as a start frequency and an end frequency,
    // which is why its ranges could not be shared and why the sweep could not
    // track the note. A depth around the tone pitch tracks correctly,
    // modulates meaningfully, and spells an upward blip as a negative number.
    // +21 semitones over 45 ms from 160 Hz is approximately v1's default kick.
    semitones(PARAM_PITCH_DEPTH, "Pitch depth", -60.0, 60.0, 21.0),
];

const AMP_DESCRIPTORS: [ParamDescriptor; ENV_BLOCK as usize] = env_block(
    PARAM_AMP_ENV_BASE,
    [
        "Amp attack",
        "Amp hold",
        "Amp decay",
        "Amp curve",
        "Amp sustain",
        "Amp release",
        "Amp gate",
    ],
    0.24,
);

const NOISE_ENV_DESCRIPTORS: [ParamDescriptor; ENV_BLOCK as usize] = env_block(
    PARAM_NOISE_ENV_BASE,
    [
        "Noise attack",
        "Noise hold",
        "Noise decay",
        "Noise curve",
        "Noise sustain",
        "Noise release",
        "Noise gate",
    ],
    0.12,
);

/// The one with no other job. Step 07 makes it a matrix source; until then it
/// runs and reaches nothing, which is the honest state of a contour whose
/// point is that it has no fixed destination.
const MOD_ENV_DESCRIPTORS: [ParamDescriptor; ENV_BLOCK as usize] = env_block(
    PARAM_MOD_ENV_BASE,
    [
        "Mod attack",
        "Mod hold",
        "Mod decay",
        "Mod curve",
        "Mod sustain",
        "Mod release",
        "Mod gate",
    ],
    0.3,
);

/// The complete DS-01 table for this step.
pub static DESCRIPTORS: [ParamDescriptor; 44] = concat();

const fn concat() -> [ParamDescriptor; 44] {
    let mut out = [GLOBAL_DESCRIPTORS[0]; 44];
    let mut at = 0;
    let mut i = 0;
    while i < GLOBAL_DESCRIPTORS.len() {
        out[at] = GLOBAL_DESCRIPTORS[i];
        at += 1;
        i += 1;
    }
    i = 0;
    while i < TONE_DESCRIPTORS.len() {
        out[at] = TONE_DESCRIPTORS[i];
        at += 1;
        i += 1;
    }
    i = 0;
    while i < NOISE_DESCRIPTORS.len() {
        out[at] = NOISE_DESCRIPTORS[i];
        at += 1;
        i += 1;
    }
    i = 0;
    while i < AMP_DESCRIPTORS.len() {
        out[at] = AMP_DESCRIPTORS[i];
        at += 1;
        i += 1;
    }
    i = 0;
    while i < PITCH_DESCRIPTORS.len() {
        out[at] = PITCH_DESCRIPTORS[i];
        at += 1;
        i += 1;
    }
    i = 0;
    while i < NOISE_ENV_DESCRIPTORS.len() {
        out[at] = NOISE_ENV_DESCRIPTORS[i];
        at += 1;
        i += 1;
    }
    i = 0;
    while i < MOD_ENV_DESCRIPTORS.len() {
        out[at] = MOD_ENV_DESCRIPTORS[i];
        at += 1;
        i += 1;
    }
    out
}

/// This device's descriptor for `id`, if it has one.
pub fn descriptor(id: u32) -> Option<&'static ParamDescriptor> {
    DESCRIPTORS.iter().find(|descriptor| descriptor.id == id)
}

/// One gated attack-hold-decay envelope.
///
/// ```text
///           +---- hold ----+
///          /|              |\
///         / |              | \___ decay (curve)
///   attack                       \_____
///
///   gate on:  ... decay falls to sustain, then release at note-off
/// ```
///
/// Every field here is latched at the hit per `01-what-ds01-is.md`: changing
/// a running envelope's rate steps its output. Because each hit re-latches,
/// an LFO on a decay time produces a pattern whose hits differ from one
/// another, which is the musically useful reading.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Ds01EnvParams {
    /// Seconds to full level. `0` means the first sample is the peak, with no
    /// ramp and no smoothing — a drum synth whose attack cannot be zero is
    /// broken.
    pub attack: f32,
    /// Seconds flat at the peak. The 909 clap tail and the gated snare.
    pub hold: f32,
    pub decay: f32,
    /// `-1` logarithmic, `0` exponential (v1's law), `+1` linear.
    pub curve: f32,
    /// Level the decay falls to while a note is held. Only meaningful with
    /// [`Self::gate`] on.
    pub sustain: f32,
    /// Seconds from the held level to silence at note-off. Only meaningful
    /// with [`Self::gate`] on.
    pub release: f32,
    /// Off is one-shot, matching v1: note-offs end nothing and a hit runs to
    /// silence. On is what makes a ride that rings for as long as it is
    /// written, a held shaker, and a sustained noise wash — sounds v1 cannot
    /// make at all.
    pub gate: bool,
}

impl Ds01EnvParams {
    pub const fn one_shot(decay: f32) -> Self {
        Self {
            attack: 0.0,
            hold: 0.0,
            decay,
            curve: 0.0,
            sustain: 0.0,
            release: 0.1,
            gate: false,
        }
    }
}

impl Default for Ds01EnvParams {
    fn default() -> Self {
        Self::one_shot(0.24)
    }
}

/// The pitch envelope: the same shape, without the gate half.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Ds01PitchEnvParams {
    pub attack: f32,
    pub decay: f32,
    pub curve: f32,
    /// Bipolar excursion in semitones, around the tone pitch.
    pub depth: f32,
}

impl Default for Ds01PitchEnvParams {
    fn default() -> Self {
        Self {
            attack: 0.0,
            decay: 0.045,
            curve: 0.0,
            depth: 21.0,
        }
    }
}

/// Read one control out of a gated envelope block by its offset.
fn env_get(env: &Ds01EnvParams, offset: u32) -> Option<f32> {
    Some(match offset {
        ENV_OFFSET_ATTACK => env.attack,
        ENV_OFFSET_HOLD => env.hold,
        ENV_OFFSET_DECAY => env.decay,
        ENV_OFFSET_CURVE => env.curve,
        ENV_OFFSET_SUSTAIN => env.sustain,
        ENV_OFFSET_RELEASE => env.release,
        ENV_OFFSET_GATE => f32::from(u8::from(env.gate)),
        _ => return None,
    })
}

fn env_set(env: &mut Ds01EnvParams, offset: u32, value: f32) -> bool {
    match offset {
        ENV_OFFSET_ATTACK => env.attack = value,
        ENV_OFFSET_HOLD => env.hold = value,
        ENV_OFFSET_DECAY => env.decay = value,
        ENV_OFFSET_CURVE => env.curve = value,
        ENV_OFFSET_SUSTAIN => env.sustain = value,
        ENV_OFFSET_RELEASE => env.release = value,
        ENV_OFFSET_GATE => env.gate = value.round() > 0.0,
        _ => return false,
    }
    true
}

/// Split an id into its offset inside the envelope block starting at `base`.
fn env_slot(id: u32, base: u32) -> Option<u32> {
    let offset = id.checked_sub(base)?;
    (offset < ENV_BLOCK).then_some(offset)
}

// --- The parameter set -----------------------------------------------------

/// All DS-01 parameters, in the units the DSP and UI share.
///
/// `#[serde(default)]` from the first commit rather than after the first
/// field addition breaks every saved project.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Ds01Params {
    /// Transpose in semitones, added to the played note.
    pub tune: f32,
    /// Device output level in `[0, 1]`. A mix control; the shaper in step 06
    /// is a separate stage.
    pub level: f32,
    /// `0` disables choking; matching non-zero groups choke each other.
    pub choke_group: u8,
    /// How long a choke takes (seconds). Applied as an amplitude-envelope
    /// release rather than as a coefficient stamped over the envelope, so
    /// step 03's shapes do not have to special-case it.
    pub choke_time: f32,
    pub retrigger: Ds01Retrigger,
    /// How much velocity scales the voice, in `[0, 1]`. At `0` every hit has
    /// the same amplitude.
    pub velocity_amount: f32,

    // --- Tone -----------------------------------------------------------
    pub tone_level: f32,
    /// Fundamental in Hz, before the note and [`Self::tune`] track it.
    pub tone_pitch: f32,
    /// Continuous morph, sine > triangle > saw > pulse.
    pub tone_wave: f32,
    /// Partial count in `1..=DS01_MAX_PARTIALS`. The one structural control
    /// inside a source section, and the reason [`Self::tone_spread`] is
    /// allowed to be inert at 1.
    pub tone_partials: u8,
    /// How far the inharmonic partial ratios spread, in `[0, 1]`. Inert at
    /// one partial, which `01-what-ds01-is.md` names as an exception.
    pub tone_spread: f32,
    /// Depth of the sine modulator into the tone oscillator, in `[0, 1]`.
    pub tone_fm_amount: f32,
    /// The modulator's frequency as a ratio of the tone pitch.
    pub tone_fm_ratio: f32,

    // --- Noise ----------------------------------------------------------
    pub noise_level: f32,
    pub noise_color: Ds01NoiseColor,
    /// Sample-rate reduction applied to the noise, in Hz. At or above the
    /// running rate it is transparent.
    pub noise_rate: f32,
    /// Filter response morph, low-pass at 0 through band-pass to high-pass
    /// at 1.
    pub filter_morph: f32,
    pub filter_cutoff: f32,
    pub filter_res: f32,

    // --- Envelopes ------------------------------------------------------
    /// The VCA, always.
    pub amp: Ds01EnvParams,
    /// Tone pitch, by a bipolar depth in semitones.
    pub pitch: Ds01PitchEnvParams,
    /// The noise layer's own level.
    pub noise_env: Ds01EnvParams,
    /// Nothing, by default. A matrix source in step 07, and the difference
    /// between a hit with one shape and a hit with layers that move against
    /// each other.
    pub mod_env: Ds01EnvParams,
}

impl Default for Ds01Params {
    fn default() -> Self {
        Self {
            tune: 0.0,
            level: 0.8,
            choke_group: 0,
            choke_time: 0.005,
            retrigger: Ds01Retrigger::Poly,
            velocity_amount: 1.0,
            tone_level: 1.0,
            tone_pitch: 160.0,
            tone_wave: 0.0,
            tone_partials: 1,
            tone_spread: 0.5,
            tone_fm_amount: 0.0,
            tone_fm_ratio: 2.0,
            noise_level: 0.0,
            noise_color: Ds01NoiseColor::White,
            noise_rate: 48_000.0,
            filter_morph: 1.0,
            filter_cutoff: 7_500.0,
            filter_res: 0.1,
            amp: Ds01EnvParams::one_shot(0.24),
            pitch: Ds01PitchEnvParams::default(),
            noise_env: Ds01EnvParams::one_shot(0.12),
            mod_env: Ds01EnvParams::one_shot(0.3),
        }
    }
}

/// Read one parameter in natural units by wire id.
pub fn get(p: &Ds01Params, id: u32) -> Option<f32> {
    if let Some(offset) = env_slot(id, PARAM_AMP_ENV_BASE) {
        return env_get(&p.amp, offset);
    }
    if let Some(offset) = env_slot(id, PARAM_NOISE_ENV_BASE) {
        return env_get(&p.noise_env, offset);
    }
    if let Some(offset) = env_slot(id, PARAM_MOD_ENV_BASE) {
        return env_get(&p.mod_env, offset);
    }
    Some(match id {
        PARAM_TUNE => p.tune,
        PARAM_LEVEL => p.level,
        PARAM_CHOKE_GROUP => f32::from(p.choke_group),
        PARAM_CHOKE_TIME => p.choke_time,
        PARAM_RETRIGGER => p.retrigger.to_index() as f32,
        PARAM_VELOCITY_AMOUNT => p.velocity_amount,
        PARAM_TONE_LEVEL => p.tone_level,
        PARAM_TONE_PITCH => p.tone_pitch,
        PARAM_TONE_WAVE => p.tone_wave,
        PARAM_TONE_PARTIALS => f32::from(p.tone_partials),
        PARAM_TONE_SPREAD => p.tone_spread,
        PARAM_TONE_FM_AMOUNT => p.tone_fm_amount,
        PARAM_TONE_FM_RATIO => p.tone_fm_ratio,
        PARAM_NOISE_LEVEL => p.noise_level,
        PARAM_NOISE_COLOR => p.noise_color.to_index() as f32,
        PARAM_NOISE_RATE => p.noise_rate,
        PARAM_FILTER_MORPH => p.filter_morph,
        PARAM_FILTER_CUTOFF => p.filter_cutoff,
        PARAM_FILTER_RES => p.filter_res,
        PARAM_PITCH_ATTACK => p.pitch.attack,
        PARAM_PITCH_DECAY => p.pitch.decay,
        PARAM_PITCH_CURVE => p.pitch.curve,
        PARAM_PITCH_DEPTH => p.pitch.depth,
        _ => return None,
    })
}

/// Write one parameter in natural units by wire id. The caller has already
/// clamped `value` through the descriptor.
pub fn set(p: &mut Ds01Params, id: u32, value: f32) -> bool {
    if let Some(offset) = env_slot(id, PARAM_AMP_ENV_BASE) {
        return env_set(&mut p.amp, offset, value);
    }
    if let Some(offset) = env_slot(id, PARAM_NOISE_ENV_BASE) {
        return env_set(&mut p.noise_env, offset, value);
    }
    if let Some(offset) = env_slot(id, PARAM_MOD_ENV_BASE) {
        return env_set(&mut p.mod_env, offset, value);
    }
    match id {
        PARAM_TUNE => p.tune = value,
        PARAM_LEVEL => p.level = value,
        PARAM_CHOKE_GROUP => p.choke_group = (value.round().clamp(0.0, 255.0) as u8).min(MAX_CHOKE_GROUP),
        PARAM_CHOKE_TIME => p.choke_time = value,
        PARAM_RETRIGGER => p.retrigger = Ds01Retrigger::from_index(value.round() as i32),
        PARAM_VELOCITY_AMOUNT => p.velocity_amount = value,
        PARAM_TONE_LEVEL => p.tone_level = value,
        PARAM_TONE_PITCH => p.tone_pitch = value,
        PARAM_TONE_WAVE => p.tone_wave = value,
        PARAM_TONE_PARTIALS => {
            p.tone_partials = (value.round().clamp(1.0, DS01_MAX_PARTIALS as f32)) as u8
        }
        PARAM_TONE_SPREAD => p.tone_spread = value,
        PARAM_TONE_FM_AMOUNT => p.tone_fm_amount = value,
        PARAM_TONE_FM_RATIO => p.tone_fm_ratio = value,
        PARAM_NOISE_LEVEL => p.noise_level = value,
        PARAM_NOISE_COLOR => p.noise_color = Ds01NoiseColor::from_index(value.round() as i32),
        PARAM_NOISE_RATE => p.noise_rate = value,
        PARAM_FILTER_MORPH => p.filter_morph = value,
        PARAM_FILTER_CUTOFF => p.filter_cutoff = value,
        PARAM_FILTER_RES => p.filter_res = value,
        PARAM_PITCH_ATTACK => p.pitch.attack = value,
        PARAM_PITCH_DECAY => p.pitch.decay = value,
        PARAM_PITCH_CURVE => p.pitch.curve = value,
        PARAM_PITCH_DEPTH => p.pitch.depth = value,
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_ids_are_unique() {
        for (i, a) in DESCRIPTORS.iter().enumerate() {
            for b in &DESCRIPTORS[i + 1..] {
                assert_ne!(a.id, b.id, "duplicate DS-01 id {} ({})", a.id, a.name);
            }
        }
    }

    /// Steps 02 and 03 own 0-29 and the four envelope bands. 30-39 is the
    /// body resonator, 80 onward is the burst, the shaper and the matrix;
    /// reaching into either would be spending a later step's reservation.
    #[test]
    fn every_id_lands_in_a_band_these_steps_own() {
        for d in &DESCRIPTORS {
            let owned = d.id < 30 || (40..80).contains(&d.id);
            assert!(owned, "{} ({}) is outside steps 02-03's bands", d.id, d.name);
            assert!(
                !(54..60).contains(&d.id),
                "{} ({}) sits in the pitch band's unused tail",
                d.id,
                d.name
            );
        }
        assert_eq!(DESCRIPTORS.len(), 44);
    }

    /// The four envelopes are one type used four times, so their blocks have
    /// to line up: same offsets, same ranges, same curves. A block that
    /// drifted would be a second envelope design nobody decided on.
    #[test]
    fn the_three_gated_envelopes_are_the_same_block() {
        let offsets_of = |base: u32| {
            (0..ENV_BLOCK)
                .map(|offset| {
                    let d = descriptor(base + offset).expect("every offset has a descriptor");
                    (d.unit, d.min, d.max, d.curve)
                })
                .collect::<Vec<_>>()
        };
        let amp = offsets_of(PARAM_AMP_ENV_BASE);
        assert_eq!(amp, offsets_of(PARAM_NOISE_ENV_BASE));
        assert_eq!(amp, offsets_of(PARAM_MOD_ENV_BASE));
    }

    /// The pitch envelope deliberately has no gate, and therefore no hold,
    /// sustain or release: with the gate off they would be three controls
    /// that do nothing, which is what this instrument exists not to have.
    #[test]
    fn the_pitch_envelope_has_no_gate_half() {
        for id in [
            PARAM_PITCH_ATTACK + 1,
            PARAM_PITCH_DEPTH + 1,
            PARAM_PITCH_DEPTH + 2,
        ] {
            if id == PARAM_PITCH_DECAY || id == PARAM_PITCH_CURVE {
                continue;
            }
            assert!(descriptor(id).is_none(), "id {id} exists");
        }
        assert_eq!(
            DESCRIPTORS
                .iter()
                .filter(|d| (50..60).contains(&d.id))
                .count(),
            4
        );
    }

    #[test]
    fn every_descriptor_round_trips_through_get_and_set() {
        for d in &DESCRIPTORS {
            let mut params = Ds01Params::default();
            let target = d.clamp_natural(d.min + (d.max - d.min) * 0.75);
            assert!(set(&mut params, d.id, target), "{} is not settable", d.name);
            let read = get(&params, d.id).unwrap_or_else(|| panic!("{} is not readable", d.name));
            assert!(
                (read - target).abs() < 1.0e-3,
                "{} wrote {target} and read {read}",
                d.name
            );
        }
    }

    #[test]
    fn defaults_agree_with_the_descriptor_table() {
        let params = Ds01Params::default();
        for d in &DESCRIPTORS {
            let value = get(&params, d.id).unwrap();
            assert!(
                (value - d.default).abs() < 1.0e-6,
                "{} defaults to {value} but declares {}",
                d.name,
                d.default
            );
        }
    }

    /// The `*` set in `02-the-voice-and-the-descriptor-table.md`, restated
    /// through the descriptor curve so a stepped control added later is
    /// excluded the day it is added rather than the day someone remembers.
    #[test]
    fn exactly_the_structural_controls_are_stepped() {
        let stepped: Vec<u32> = DESCRIPTORS
            .iter()
            .filter(|d| matches!(d.curve, ParamCurve::Stepped(_)))
            .map(|d| d.id)
            .collect();
        assert_eq!(
            stepped,
            vec![
                PARAM_TUNE,
                PARAM_CHOKE_GROUP,
                PARAM_RETRIGGER,
                PARAM_TONE_PARTIALS,
                PARAM_NOISE_COLOR,
                PARAM_AMP_ENV_BASE + ENV_OFFSET_GATE,
                PARAM_NOISE_ENV_BASE + ENV_OFFSET_GATE,
                PARAM_MOD_ENV_BASE + ENV_OFFSET_GATE,
            ]
        );
    }

    #[test]
    fn an_id_this_step_has_not_assigned_is_neither_read_nor_written() {
        let mut params = Ds01Params::default();
        // 6 is inside the global band but unassigned; 30 belongs to the body
        // resonator in step 04, 47 to the tail of the amplitude block, 54 to
        // the tail of the pitch one, and 80 to the burst in step 05.
        for id in [6, 30, 47, 54, 80] {
            assert_eq!(get(&params, id), None, "id {id} reads");
            assert!(!set(&mut params, id, 1.0), "id {id} writes");
        }
    }

    #[test]
    fn a_choke_group_is_clamped_to_the_bank() {
        let mut params = Ds01Params::default();
        assert!(set(&mut params, PARAM_CHOKE_GROUP, 900.0));
        assert_eq!(params.choke_group, MAX_CHOKE_GROUP);
    }

    #[test]
    fn tune_snaps_to_whole_semitones() {
        let d = descriptor(PARAM_TUNE).unwrap();
        assert_eq!(d.clamp_natural(11.4), 11.0);
        assert_eq!(d.clamp_natural(-11.6), -12.0);
    }
}
