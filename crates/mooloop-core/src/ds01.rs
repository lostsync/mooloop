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

pub const PARAM_BODY_LEVEL: u32 = 30;
pub const PARAM_BODY_PITCH: u32 = 31;
pub const PARAM_BODY_RATIO: u32 = 32;
pub const PARAM_BODY_DECAY: u32 = 33;
pub const PARAM_BODY_DAMPING: u32 = 34;
pub const PARAM_BODY_EXCITE: u32 = 35;

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

pub const PARAM_BURST_REPEATS: u32 = 80;
pub const PARAM_BURST_SPACING: u32 = 81;
pub const PARAM_BURST_SPREAD: u32 = 82;
pub const PARAM_BURST_LEVEL_STEP: u32 = 83;
pub const PARAM_BURST_PITCH_STEP: u32 = 84;

pub const PARAM_DRIVE: u32 = 90;
pub const PARAM_CHARACTER: u32 = 91;
pub const PARAM_BIAS: u32 = 92;
pub const PARAM_BITS: u32 = 93;
pub const PARAM_OUTPUT_HP: u32 = 94;

/// The matrix's eight rows of four, 100-131.
pub const PARAM_MATRIX_BASE: u32 = 100;
pub const MATRIX_OFFSET_SOURCE: u32 = 0;
pub const MATRIX_OFFSET_DEST: u32 = 1;
pub const MATRIX_OFFSET_AMOUNT: u32 = 2;
pub const MATRIX_OFFSET_CURVE: u32 = 3;
pub const MATRIX_ROW_WIDTH: u32 = 4;

/// First id of `row`'s four controls.
pub const fn matrix_param(row: usize, offset: u32) -> u32 {
    PARAM_MATRIX_BASE + row as u32 * MATRIX_ROW_WIDTH + offset
}

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

/// Rows in DS-01's own modulation matrix.
pub const DS01_MATRIX_ROWS: usize = 8;

/// How many parameters a matrix row may address.
///
/// A literal because the Destination control is a `const` stepped descriptor
/// and [`destination_count`] is not a `const fn`;
/// `the_destination_list_is_every_continuous_parameter` pins the two
/// together.
pub const DS01_DESTINATIONS: u8 = 49;

/// Bit depth at which the reducer is exactly transparent.
///
/// Named rather than left as "the top of the range" because the identity is
/// load-bearing: the default patch has to reach the gain reference through a
/// shaper that is doing nothing at all, and `(x * 32768).round() / 32768` is
/// not `x`.
pub const DS01_BITS_TRANSPARENT: f32 = 16.0;

/// Most impulses one trigger can fire.
pub const DS01_MAX_REPEATS: u8 = 8;

/// Longest a burst's schedule may run, in seconds.
///
/// A bound on a voice's lifetime rather than a musical limit: at the top of
/// every control a decelerating eight-impulse burst would otherwise schedule
/// its last hit a minute after the first. Impulses past this are dropped, so
/// the schedule can shape a hit but cannot extend it indefinitely.
pub const DS01_BURST_MAX_S: f32 = 4.0;

/// Tuned resonators in the body layer.
pub const DS01_BODY_MODES: usize = 3;

/// Mode ratios at [`Ds01Params::body_ratio`] `0` and `1`.
///
/// At 0 they are harmonics, and the layer reads as a pitched drum — a tom, a
/// conga, a tuned kick body. At 1 they are the ideal circular membrane's mode
/// set, which is the reason a real drum head sounds like a drum head and not
/// like a sine: the layer stops having a pitch and starts having a material.
/// Sweeping between them is the whole design in one control.
pub const DS01_BODY_HARMONIC: [f32; DS01_BODY_MODES] = [1.0, 2.0, 3.0];
pub const DS01_BODY_INHARMONIC: [f32; DS01_BODY_MODES] = [1.0, 2.76, 5.40];

/// This mode's frequency ratio to the fundamental at a given Ratio setting.
pub fn body_mode_ratio(mode: usize, ratio: f32) -> f32 {
    let mode = mode.min(DS01_BODY_MODES - 1);
    let ratio = ratio.clamp(0.0, 1.0);
    let harmonic = DS01_BODY_HARMONIC[mode];
    harmonic + (DS01_BODY_INHARMONIC[mode] - harmonic) * ratio
}

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

/// What a matrix row reads.
///
/// Eight sources, all per voice, all evaluated inside the voice. That is the
/// whole reason DS-01 has a matrix at all: a channel source produces one
/// number per control tick for the whole channel, and a drum channel can have
/// eight hits ringing at once, each with its own velocity, its own position in
/// a burst, and its own envelopes. "This hit's velocity opens this hit's
/// filter" is not expressible as a channel-rate signal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ds01ModSource {
    /// The row is off. Rows are not deleted, they are switched off, so a
    /// patch's eight rows are eight rows for the life of the patch.
    #[default]
    None,
    /// How hard this hit was played. Latched.
    ///
    /// A first-class source rather than v1's multiply, so a patch can put
    /// velocity on pitch, decay, colour, cutoff, drive or burst spacing. This
    /// is the single control that most decides whether ghost notes read as
    /// part of the groove or as quiet copies of the same hit.
    Velocity,
    /// This hit's note. Latched.
    Note,
    AmpEnv,
    NoiseEnv,
    /// The contour with no other job.
    ModEnv,
    /// `0` at a burst's first impulse and `1` at its last, constant within an
    /// impulse, `0` throughout at Repeats 1. A shape across a flam or a roll.
    BurstIndex,
    /// `+1` and `-1` on successive hits of this channel. Latched.
    ///
    /// The 808 open/closed alternation and the every-other-hat ghost, and —
    /// with [`Self::BurstIndex`] — one of the two sources that are *consistent
    /// displacement* rather than noise, which is the distinction the taste
    /// brief draws.
    HitAlternator,
    /// One deterministic value per hit, bipolar. Latched.
    ///
    /// Not a humanize button and never presented as one: it is off until
    /// routed, it has an explicit destination and a signed depth like every
    /// other source, and it is derived from the hit counter and the node seed
    /// so an offline render and a live take of the same event stream produce
    /// identical samples. There is no global amount, no dice button, and no
    /// default route.
    Random,
}

impl Ds01ModSource {
    pub const ALL: [Self; 9] = [
        Self::None,
        Self::Velocity,
        Self::Note,
        Self::AmpEnv,
        Self::NoiseEnv,
        Self::ModEnv,
        Self::BurstIndex,
        Self::HitAlternator,
        Self::Random,
    ];

    /// Whether this source swings both ways about zero. A unipolar source
    /// rests at zero and only ever moves in the direction its amount's sign
    /// chooses.
    pub fn is_bipolar(self) -> bool {
        matches!(self, Self::HitAlternator | Self::Random)
    }

    /// Whether this source has one value for the life of a hit. The live ones
    /// are the three envelopes and the burst position.
    pub fn is_latched(self) -> bool {
        matches!(
            self,
            Self::None | Self::Velocity | Self::Note | Self::HitAlternator | Self::Random
        )
    }

    pub fn from_index(index: i32) -> Self {
        Self::ALL
            .get(index.clamp(0, Self::ALL.len() as i32 - 1) as usize)
            .copied()
            .unwrap_or_default()
    }

    pub fn to_index(self) -> i32 {
        Self::ALL
            .iter()
            .position(|source| *source == self)
            .unwrap_or_default() as i32
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "Off",
            Self::Velocity => "Velocity",
            Self::Note => "Note",
            Self::AmpEnv => "Amp Env",
            Self::NoiseEnv => "Noise Env",
            Self::ModEnv => "Mod Env",
            Self::BurstIndex => "Burst Idx",
            Self::HitAlternator => "Alternate",
            Self::Random => "Random",
        }
    }
}

/// Which nonlinearity the shape stage runs.
///
/// The one place DS-01 gets an opinion rather than a range, and four is all
/// it gets. Structural, and so ineligible for modulation by default:
/// switching between hits must not click, and switching mid-hit is undefined.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ds01Character {
    /// Symmetric soft clip: v1's `apply_drive`, curve for curve, so an
    /// old-sounding patch stays reachable.
    #[default]
    Soft,
    /// Sharp knee. Squares off a kick and puts the click back.
    Hard,
    /// Wavefolder. Turns level into timbre — a folded sine kick gets
    /// harmonically dense without getting louder — and because folding is a
    /// function of instantaneous amplitude, the shape of a hit changes across
    /// its own decay for free.
    Fold,
    /// Rectification plus the bit reducer's character. The damaged one.
    Crush,
}

impl Ds01Character {
    pub const ALL: [Self; 4] = [Self::Soft, Self::Hard, Self::Fold, Self::Crush];

    pub fn from_index(index: i32) -> Self {
        Self::ALL
            .get(index.clamp(0, Self::ALL.len() as i32 - 1) as usize)
            .copied()
            .unwrap_or_default()
    }

    pub fn to_index(self) -> i32 {
        Self::ALL
            .iter()
            .position(|character| *character == self)
            .unwrap_or_default() as i32
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Soft => "Soft",
            Self::Hard => "Hard",
            Self::Fold => "Fold",
            Self::Crush => "Crush",
        }
    }
}

/// The layer v1 has no equivalent of, and where most of the new drum types
/// come from: a rim, a clave, a conga, a cowbell, a bell, a piece of struck
/// metal. Those sounds are a short excitation ringing through a resonant
/// object, and the object is the sound.
const BODY_DESCRIPTORS: [ParamDescriptor; 6] = [
    // Zero, like the noise layer: the default patch is still a clean tone
    // hit, and the body is something a patch reaches for rather than
    // something it has to turn off.
    unit(PARAM_BODY_LEVEL, "Body level", 0.0),
    hz(PARAM_BODY_PITCH, "Body pitch", 20.0, 8_000.0, 220.0),
    unit(PARAM_BODY_RATIO, "Ratio", 0.0),
    // A time in seconds at every pitch, which is what deriving the
    // resonator's pole radius from it rather than from a Q buys.
    time_s(PARAM_BODY_DECAY, "Body decay", 0.005, 8.0, 0.4),
    // The difference between a bell and a woodblock, and the most gestural
    // control in the device: from the mod envelope it is a strike that opens
    // or closes over its own tail.
    unit(PARAM_BODY_DAMPING, "Damping", 0.3),
    // Impulse at 0 — a hard strike, which makes clave, rim and woodblock —
    // and the noise layer at 1, a sustained excitation, which makes cymbal
    // shimmer and the ring under a snare. Between them is most of what
    // percussion actually is.
    unit(PARAM_BODY_EXCITE, "Excite", 0.0),
];

/// One trigger, several impulses: the clap, and the control most likely to
/// produce something nobody planned.
///
/// A clap is not a snare with a longer noise tail. It is three or four noise
/// bursts a few milliseconds apart followed by a longer one, and no amount of
/// envelope shaping reaches it from a single hit. Once the mechanism exists
/// it is also a flam, a drag, a buzz roll, a stutter and a machine-gun fill.
///
/// Repeats 1 is an ordinary hit and is the default, so the section looks
/// inert and is not: every other control has an effect the moment Repeats
/// moves, and Repeats itself is a destination worth having.
const BURST_DESCRIPTORS: [ParamDescriptor; 5] = [
    ParamDescriptor {
        id: PARAM_BURST_REPEATS,
        name: "Repeats",
        unit: "",
        min: 1.0,
        max: DS01_MAX_REPEATS as f32,
        curve: ParamCurve::Stepped(DS01_MAX_REPEATS),
        default: 1.0,
    },
    // Milliseconds, and they stay milliseconds. Tempo-syncing this would make
    // a clap change shape when the project tempo changed, which is wrong: a
    // burst is one event's internal structure, not a placement decision.
    time_s(PARAM_BURST_SPACING, "Spacing", 0.001, 0.5, 0.012),
    // Negative accelerates — each gap shorter than the last, which is the
    // clap and the buzz roll. Positive decelerates, which is a drag. Zero is
    // even, which is a machine-gun.
    bipolar(PARAM_BURST_SPREAD, "Spread", 0.0),
    // Per impulse and cumulative. Negative is the natural clap and flam
    // shape; positive is a build that arrives on the last impulse.
    bipolar(PARAM_BURST_LEVEL_STEP, "Level step", 0.0),
    // A fill that climbs, or a tom roll that falls.
    semitones(PARAM_BURST_PITCH_STEP, "Pitch step", -24.0, 24.0, 0.0),
];

/// Everything above sums into one shaper, and this is what that shaper is.
///
/// v1 applies `apply_drive` and multiplies by a single `OUTPUT_REFERENCE`
/// constant. That is a reasonable calibration and the wrong structure: the
/// constant does the job of a mix decision, a character control, and a safety
/// bound at once. Here the mix decision is [`PARAM_LEVEL`], the safety bound
/// is the device's own, and these five are the colour.
const SHAPE_DESCRIPTORS: [ParamDescriptor; 5] = [
    unit(PARAM_DRIVE, "Drive", 0.0),
    stepped(
        PARAM_CHARACTER,
        "Character",
        Ds01Character::ALL.len() as u8,
        0.0,
    ),
    // Asymmetry: it adds even harmonics, and at the top it gates and spits.
    // The DC it creates is what the output high-pass is for.
    unit(PARAM_BIAS, "Bias", 0.0),
    ParamDescriptor {
        id: PARAM_BITS,
        name: "Bits",
        unit: "",
        min: 1.0,
        max: DS01_BITS_TRANSPARENT,
        curve: ParamCurve::Stepped(DS01_BITS_TRANSPARENT as u8),
        default: DS01_BITS_TRANSPARENT,
    },
    // Removes the DC that Bias creates, and thins a hit deliberately.
    hz(PARAM_OUTPUT_HP, "Output HP", 5.0, 2_000.0, 20.0),
];

/// The matrix's eight rows, four controls each.
///
/// Source and Destination are structural and therefore modulation-ineligible;
/// **Amount is not**, which is how a channel LFO gets to scale a per-hit
/// relationship without knowing anything about voices.
const MATRIX_DESCRIPTORS: [[ParamDescriptor; MATRIX_ROW_WIDTH as usize]; DS01_MATRIX_ROWS] = [
    [
        stepped(
            matrix_param(0, MATRIX_OFFSET_SOURCE),
            "Row 1 src",
            Ds01ModSource::ALL.len() as u8,
            0.0,
        ),
        stepped(
            matrix_param(0, MATRIX_OFFSET_DEST),
            "Row 1 dest",
            DS01_DESTINATIONS,
            0.0,
        ),
        bipolar(matrix_param(0, MATRIX_OFFSET_AMOUNT), "Row 1 amt", 0.0),
        bipolar(matrix_param(0, MATRIX_OFFSET_CURVE), "Row 1 curve", 0.0),
    ],
    [
        stepped(
            matrix_param(1, MATRIX_OFFSET_SOURCE),
            "Row 2 src",
            Ds01ModSource::ALL.len() as u8,
            0.0,
        ),
        stepped(
            matrix_param(1, MATRIX_OFFSET_DEST),
            "Row 2 dest",
            DS01_DESTINATIONS,
            0.0,
        ),
        bipolar(matrix_param(1, MATRIX_OFFSET_AMOUNT), "Row 2 amt", 0.0),
        bipolar(matrix_param(1, MATRIX_OFFSET_CURVE), "Row 2 curve", 0.0),
    ],
    [
        stepped(
            matrix_param(2, MATRIX_OFFSET_SOURCE),
            "Row 3 src",
            Ds01ModSource::ALL.len() as u8,
            0.0,
        ),
        stepped(
            matrix_param(2, MATRIX_OFFSET_DEST),
            "Row 3 dest",
            DS01_DESTINATIONS,
            0.0,
        ),
        bipolar(matrix_param(2, MATRIX_OFFSET_AMOUNT), "Row 3 amt", 0.0),
        bipolar(matrix_param(2, MATRIX_OFFSET_CURVE), "Row 3 curve", 0.0),
    ],
    [
        stepped(
            matrix_param(3, MATRIX_OFFSET_SOURCE),
            "Row 4 src",
            Ds01ModSource::ALL.len() as u8,
            0.0,
        ),
        stepped(
            matrix_param(3, MATRIX_OFFSET_DEST),
            "Row 4 dest",
            DS01_DESTINATIONS,
            0.0,
        ),
        bipolar(matrix_param(3, MATRIX_OFFSET_AMOUNT), "Row 4 amt", 0.0),
        bipolar(matrix_param(3, MATRIX_OFFSET_CURVE), "Row 4 curve", 0.0),
    ],
    [
        stepped(
            matrix_param(4, MATRIX_OFFSET_SOURCE),
            "Row 5 src",
            Ds01ModSource::ALL.len() as u8,
            0.0,
        ),
        stepped(
            matrix_param(4, MATRIX_OFFSET_DEST),
            "Row 5 dest",
            DS01_DESTINATIONS,
            0.0,
        ),
        bipolar(matrix_param(4, MATRIX_OFFSET_AMOUNT), "Row 5 amt", 0.0),
        bipolar(matrix_param(4, MATRIX_OFFSET_CURVE), "Row 5 curve", 0.0),
    ],
    [
        stepped(
            matrix_param(5, MATRIX_OFFSET_SOURCE),
            "Row 6 src",
            Ds01ModSource::ALL.len() as u8,
            0.0,
        ),
        stepped(
            matrix_param(5, MATRIX_OFFSET_DEST),
            "Row 6 dest",
            DS01_DESTINATIONS,
            0.0,
        ),
        bipolar(matrix_param(5, MATRIX_OFFSET_AMOUNT), "Row 6 amt", 0.0),
        bipolar(matrix_param(5, MATRIX_OFFSET_CURVE), "Row 6 curve", 0.0),
    ],
    [
        stepped(
            matrix_param(6, MATRIX_OFFSET_SOURCE),
            "Row 7 src",
            Ds01ModSource::ALL.len() as u8,
            0.0,
        ),
        stepped(
            matrix_param(6, MATRIX_OFFSET_DEST),
            "Row 7 dest",
            DS01_DESTINATIONS,
            0.0,
        ),
        bipolar(matrix_param(6, MATRIX_OFFSET_AMOUNT), "Row 7 amt", 0.0),
        bipolar(matrix_param(6, MATRIX_OFFSET_CURVE), "Row 7 curve", 0.0),
    ],
    [
        stepped(
            matrix_param(7, MATRIX_OFFSET_SOURCE),
            "Row 8 src",
            Ds01ModSource::ALL.len() as u8,
            0.0,
        ),
        stepped(
            matrix_param(7, MATRIX_OFFSET_DEST),
            "Row 8 dest",
            DS01_DESTINATIONS,
            0.0,
        ),
        bipolar(matrix_param(7, MATRIX_OFFSET_AMOUNT), "Row 8 amt", 0.0),
        bipolar(matrix_param(7, MATRIX_OFFSET_CURVE), "Row 8 curve", 0.0),
    ],
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
pub static DESCRIPTORS: [ParamDescriptor; 92] = concat();

const fn concat() -> [ParamDescriptor; 92] {
    let mut out = [GLOBAL_DESCRIPTORS[0]; 92];
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
    while i < BODY_DESCRIPTORS.len() {
        out[at] = BODY_DESCRIPTORS[i];
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
    i = 0;
    while i < BURST_DESCRIPTORS.len() {
        out[at] = BURST_DESCRIPTORS[i];
        at += 1;
        i += 1;
    }
    i = 0;
    while i < SHAPE_DESCRIPTORS.len() {
        out[at] = SHAPE_DESCRIPTORS[i];
        at += 1;
        i += 1;
    }
    let mut row = 0;
    while row < DS01_MATRIX_ROWS {
        i = 0;
        while i < MATRIX_ROW_WIDTH as usize {
            out[at] = MATRIX_DESCRIPTORS[row][i];
            at += 1;
            i += 1;
        }
        row += 1;
    }
    out
}

/// Whether this parameter is resolved once at the hit and not revisited.
///
/// `01-what-ds01-is.md`'s two tables, as a function. It is what decides when a
/// matrix route is evaluated — a route to a latched destination at the
/// trigger, one to a continuous destination every control tick — and that
/// falls straight out of the tables rather than needing a rule of its own.
///
/// Three of the classifications are not in either table, and are recorded in
/// the plan's status file rather than decided at a call site: the whole of an
/// envelope block is latched, because an envelope is one shape and half of it
/// cannot be; Velocity Amount is latched with the velocity it scales; and
/// Tone Partials is latched because a hit does not grow an oscillator halfway
/// through.
pub fn is_latched(id: u32) -> bool {
    if env_slot(id, PARAM_AMP_ENV_BASE).is_some()
        || env_slot(id, PARAM_NOISE_ENV_BASE).is_some()
        || env_slot(id, PARAM_MOD_ENV_BASE).is_some()
    {
        return true;
    }
    matches!(
        id,
        PARAM_TUNE
            | PARAM_VELOCITY_AMOUNT
            | PARAM_TONE_PARTIALS
            | PARAM_PITCH_ATTACK
            | PARAM_PITCH_DECAY
            | PARAM_PITCH_CURVE
            | PARAM_PITCH_DEPTH
            | PARAM_BURST_REPEATS
            | PARAM_BURST_SPACING
            | PARAM_BURST_SPREAD
            | PARAM_BURST_LEVEL_STEP
            | PARAM_BURST_PITCH_STEP
    )
}

/// Every parameter a matrix row may address, in table order.
///
/// Eligibility is the descriptor's own curve, exactly as it is for a channel
/// route: a stepped parameter is a structural choice — a waveform, a colour,
/// a drive character — and flapping one at a control rate is a click rather
/// than a modulation. Keeping the rule here means a stepped control added
/// later is excluded the day it is added.
///
/// The matrix's own band is excluded on top of that. Source and Destination
/// are stepped and would be refused anyway; Amount and Curve are not, and a
/// row modulating another row's amount would make the result depend on the
/// order the rows happen to be evaluated in. A *channel* route still reaches
/// Amount, which is how an LFO scales a per-hit relationship without knowing
/// anything about voices — it is resolved before the block rather than inside
/// it, so there is no order to depend on.
pub fn destinations() -> impl Iterator<Item = &'static ParamDescriptor> {
    DESCRIPTORS
        .iter()
        .filter(|d| d.id < PARAM_MATRIX_BASE && !matches!(d.curve, ParamCurve::Stepped(_)))
}

/// How many destinations there are. The Destination control is a stepped
/// choice over this list.
pub fn destination_count() -> usize {
    destinations().count()
}

/// The destination at `index`, for the stepped Destination control.
pub fn destination_at(index: usize) -> Option<&'static ParamDescriptor> {
    destinations().nth(index)
}

/// Where `id` sits in the destination list, if it is one.
pub fn destination_index(id: u32) -> Option<usize> {
    destinations().position(|d| d.id == id)
}

/// One matrix row: a source, a destination, a signed depth, and a curve.
///
/// Persisted with the destination's *parameter id* rather than its position
/// in the list, so a patch keeps meaning what it meant even if the list ever
/// grows. Only the stepped control the UI turns is an index.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Ds01Route {
    pub source: Ds01ModSource,
    /// Descriptor id of the destination.
    pub dest: u32,
    /// Signed depth, `[-1, 1]`, as a fraction of the destination's full
    /// range. Routes add an offset in normalized destination space around the
    /// base value, identical to a channel route — never an absolute write.
    pub amount: f32,
    /// Shapes the source before it is scaled, `[-1, 1]`.
    pub curve: f32,
}

impl Default for Ds01Route {
    fn default() -> Self {
        Self {
            source: Ds01ModSource::None,
            dest: PARAM_LEVEL,
            amount: 0.0,
            curve: 0.0,
        }
    }
}

impl Ds01Route {
    /// Whether this row reaches anything.
    pub fn is_active(&self) -> bool {
        self.source != Ds01ModSource::None && self.amount != 0.0
    }
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

/// Split an id into `(row, offset)` when it lands in the matrix.
fn matrix_slot(id: u32) -> Option<(usize, u32)> {
    let offset = id.checked_sub(PARAM_MATRIX_BASE)?;
    let row = (offset / MATRIX_ROW_WIDTH) as usize;
    (row < DS01_MATRIX_ROWS).then_some((row, offset % MATRIX_ROW_WIDTH))
}

fn matrix_get(route: &Ds01Route, offset: u32) -> Option<f32> {
    Some(match offset {
        MATRIX_OFFSET_SOURCE => route.source.to_index() as f32,
        // The stepped control is a position in the destination list; the
        // route itself keeps the parameter id, which is what makes a saved
        // patch stable.
        MATRIX_OFFSET_DEST => destination_index(route.dest).unwrap_or(0) as f32,
        MATRIX_OFFSET_AMOUNT => route.amount,
        MATRIX_OFFSET_CURVE => route.curve,
        _ => return None,
    })
}

fn matrix_set(route: &mut Ds01Route, offset: u32, value: f32) -> bool {
    match offset {
        MATRIX_OFFSET_SOURCE => route.source = Ds01ModSource::from_index(value.round() as i32),
        MATRIX_OFFSET_DEST => {
            let index = value.round().max(0.0) as usize;
            // A destination out of range keeps the one it had: a route is
            // never silently re-pointed at whatever happened to be first.
            if let Some(descriptor) = destination_at(index) {
                route.dest = descriptor.id;
            }
        }
        MATRIX_OFFSET_AMOUNT => route.amount = value.clamp(-1.0, 1.0),
        MATRIX_OFFSET_CURVE => route.curve = value.clamp(-1.0, 1.0),
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

    // --- Body -----------------------------------------------------------
    pub body_level: f32,
    /// Fundamental in Hz, before the note and [`Self::tune`] track it, the
    /// same way the tone layer's pitch does.
    pub body_pitch: f32,
    /// Harmonic at `0`, the circular membrane's modes at `1`.
    pub body_ratio: f32,
    /// Ring time in seconds, the same at every pitch.
    pub body_decay: f32,
    /// High-frequency loss, in `[0, 1]`. Damps the upper modes faster than
    /// the fundamental.
    pub body_damping: f32,
    /// What the resonators are struck with: the impulse at `0`, the noise
    /// layer's post-filter signal at `1`.
    pub body_excite: f32,

    // --- Burst ----------------------------------------------------------
    /// Impulses one trigger fires, `1..=DS01_MAX_REPEATS`. `1` is an
    /// ordinary hit.
    pub burst_repeats: u8,
    /// Gap to the second impulse, in seconds. Later gaps follow
    /// [`Self::burst_spread`].
    pub burst_spacing: f32,
    /// Bipolar. Negative accelerates, positive decelerates, zero is even.
    pub burst_spread: f32,
    /// Bipolar, applied per impulse and cumulatively.
    pub burst_level_step: f32,
    /// Bipolar semitones, applied per impulse and cumulatively.
    pub burst_pitch_step: f32,

    // --- Shape and output -----------------------------------------------
    /// Amount into the nonlinearity, `[0, 1]`. `0` is exactly transparent.
    pub drive: f32,
    pub character: Ds01Character,
    /// Asymmetry, `[0, 1]`.
    pub bias: f32,
    /// Bit depth, `1..=DS01_BITS_TRANSPARENT`. The top is exactly
    /// transparent rather than very nearly so.
    pub bits: f32,
    /// Output high-pass in Hz.
    pub output_hp: f32,

    // --- The instrument's own modulation --------------------------------
    /// Eight rows. Empty rows are switched off rather than absent, so a
    /// patch's matrix is the same eight rows for its whole life and a row's
    /// identity is its position.
    pub matrix: [Ds01Route; DS01_MATRIX_ROWS],

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
            body_level: 0.0,
            body_pitch: 220.0,
            body_ratio: 0.0,
            body_decay: 0.4,
            body_damping: 0.3,
            body_excite: 0.0,
            // No default route. The step asks for Velocity to Amp at full
            // amount so the device feels normal unprogrammed — but
            // `velocity_amount` at id 5 already does exactly that, and the
            // same paragraph says it stays as the plain control for the
            // common case. Shipping both would apply velocity twice. See the
            // plan's status file.
            matrix: [Ds01Route {
                source: Ds01ModSource::None,
                // Wherever the destination list starts. A switched-off row
                // does not have an opinion about where it would point, and
                // the descriptor's default has to agree with this one.
                dest: PARAM_LEVEL,
                amount: 0.0,
                curve: 0.0,
            }; DS01_MATRIX_ROWS],
            drive: 0.0,
            character: Ds01Character::Soft,
            bias: 0.0,
            bits: DS01_BITS_TRANSPARENT,
            output_hp: 20.0,
            burst_repeats: 1,
            burst_spacing: 0.012,
            burst_spread: 0.0,
            burst_level_step: 0.0,
            burst_pitch_step: 0.0,
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
    if let Some((row, offset)) = matrix_slot(id) {
        return matrix_get(&p.matrix[row], offset);
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
        PARAM_BODY_LEVEL => p.body_level,
        PARAM_BODY_PITCH => p.body_pitch,
        PARAM_BODY_RATIO => p.body_ratio,
        PARAM_BODY_DECAY => p.body_decay,
        PARAM_BODY_DAMPING => p.body_damping,
        PARAM_BODY_EXCITE => p.body_excite,
        PARAM_DRIVE => p.drive,
        PARAM_CHARACTER => p.character.to_index() as f32,
        PARAM_BIAS => p.bias,
        PARAM_BITS => p.bits,
        PARAM_OUTPUT_HP => p.output_hp,
        PARAM_BURST_REPEATS => f32::from(p.burst_repeats),
        PARAM_BURST_SPACING => p.burst_spacing,
        PARAM_BURST_SPREAD => p.burst_spread,
        PARAM_BURST_LEVEL_STEP => p.burst_level_step,
        PARAM_BURST_PITCH_STEP => p.burst_pitch_step,
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
    if let Some((row, offset)) = matrix_slot(id) {
        return matrix_set(&mut p.matrix[row], offset, value);
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
        PARAM_BODY_LEVEL => p.body_level = value,
        PARAM_BODY_PITCH => p.body_pitch = value,
        PARAM_BODY_RATIO => p.body_ratio = value,
        PARAM_BODY_DECAY => p.body_decay = value,
        PARAM_BODY_DAMPING => p.body_damping = value,
        PARAM_BODY_EXCITE => p.body_excite = value,
        PARAM_DRIVE => p.drive = value,
        PARAM_CHARACTER => p.character = Ds01Character::from_index(value.round() as i32),
        PARAM_BIAS => p.bias = value,
        PARAM_BITS => p.bits = value.round().clamp(1.0, DS01_BITS_TRANSPARENT),
        PARAM_OUTPUT_HP => p.output_hp = value,
        PARAM_BURST_REPEATS => {
            p.burst_repeats = value.round().clamp(1.0, DS01_MAX_REPEATS as f32) as u8
        }
        PARAM_BURST_SPACING => p.burst_spacing = value,
        PARAM_BURST_SPREAD => p.burst_spread = value,
        PARAM_BURST_LEVEL_STEP => p.burst_level_step = value,
        PARAM_BURST_PITCH_STEP => p.burst_pitch_step = value,
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

    /// The plan reserves 0-131 and this is all of it. Steps 02 through 07 have
    /// now assigned inside every band they named, so a parameter added after
    /// this appends past 131 rather than filling a gap someone left.
    #[test]
    fn every_id_lands_in_a_band_these_steps_own() {
        for d in &DESCRIPTORS {
            let owned = d.id < PARAM_MATRIX_BASE
                + DS01_MATRIX_ROWS as u32 * MATRIX_ROW_WIDTH;
            assert!(owned, "{} ({}) is outside the plan's bands", d.id, d.name);
            assert!(
                !(54..60).contains(&d.id),
                "{} ({}) sits in the pitch band's unused tail",
                d.id,
                d.name
            );
        }
        assert_eq!(DESCRIPTORS.len(), 92);
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
        let mut want = vec![
            PARAM_TUNE,
            PARAM_CHOKE_GROUP,
            PARAM_RETRIGGER,
            PARAM_TONE_PARTIALS,
            PARAM_NOISE_COLOR,
            PARAM_AMP_ENV_BASE + ENV_OFFSET_GATE,
            PARAM_NOISE_ENV_BASE + ENV_OFFSET_GATE,
            PARAM_MOD_ENV_BASE + ENV_OFFSET_GATE,
            PARAM_BURST_REPEATS,
            PARAM_CHARACTER,
            PARAM_BITS,
        ];
        // Every row's Source and Destination, and neither of its Amount or
        // Curve: the two that pick what a route *is* are structural, and the
        // two that say how much it does are not.
        for row in 0..DS01_MATRIX_ROWS {
            want.push(matrix_param(row, MATRIX_OFFSET_SOURCE));
            want.push(matrix_param(row, MATRIX_OFFSET_DEST));
        }
        assert_eq!(stepped, want);
    }

    #[test]
    fn an_id_this_step_has_not_assigned_is_neither_read_nor_written() {
        let mut params = Ds01Params::default();
        // 6 is inside the global band but unassigned; 36 is the tail of the
        // body one, 47 the tail of the amplitude block, 54 the tail of the
        // pitch one, 85 the tail of the burst, 95 the tail of the shaper, and
        // 132 the first id past the matrix's last row.
        for id in [6, 36, 47, 54, 85, 95, 132] {
            assert_eq!(get(&params, id), None, "id {id} reads");
            assert!(!set(&mut params, id, 1.0), "id {id} writes");
        }
    }

    /// Ratio 0 is harmonic and Ratio 1 is the circular membrane. The two
    /// tables are the design, so they are pinned rather than described.
    #[test]
    fn the_body_ratio_sweeps_from_harmonic_to_a_membrane() {
        for mode in 0..DS01_BODY_MODES {
            assert_eq!(body_mode_ratio(mode, 0.0), DS01_BODY_HARMONIC[mode]);
            assert_eq!(body_mode_ratio(mode, 1.0), DS01_BODY_INHARMONIC[mode]);
        }
        // The fundamental is the fundamental at every setting: Ratio detunes
        // the modes above it, it does not transpose the layer.
        for ratio in [0.0, 0.25, 0.5, 1.0] {
            assert_eq!(body_mode_ratio(0, ratio), 1.0);
        }
        // And the modes stay ordered, so "mode 2" never crosses "mode 1".
        for ratio in [0.0, 0.5, 1.0] {
            assert!(body_mode_ratio(1, ratio) < body_mode_ratio(2, ratio));
        }
    }

    /// The Destination control is a stepped choice over every continuous
    /// parameter, and `DS01_DESTINATIONS` is a literal because that control's
    /// descriptor is `const`. Pinned to the list it is a count of.
    #[test]
    fn the_destination_list_is_every_continuous_parameter() {
        assert_eq!(destination_count(), DS01_DESTINATIONS as usize);
        for descriptor in destinations() {
            assert!(
                !matches!(descriptor.curve, ParamCurve::Stepped(_)),
                "{} is stepped and cannot be a destination",
                descriptor.name
            );
            assert!(
                descriptor.id < PARAM_MATRIX_BASE,
                "{} is a matrix control and cannot be a destination",
                descriptor.name
            );
            assert_eq!(
                destination_at(destination_index(descriptor.id).unwrap()).map(|d| d.id),
                Some(descriptor.id)
            );
        }
        // Choke Group and another row's Source are the two the step names.
        assert_eq!(destination_index(PARAM_CHOKE_GROUP), None);
        assert_eq!(destination_index(matrix_param(0, MATRIX_OFFSET_SOURCE)), None);
        assert_eq!(destination_index(matrix_param(1, MATRIX_OFFSET_AMOUNT)), None);
    }

    /// `01-what-ds01-is.md`'s two tables, restated as one function and
    /// checked for completeness: every parameter is on exactly one side, and
    /// the sides are the ones the plan named.
    #[test]
    fn every_parameter_is_latched_or_continuous() {
        for d in DESCRIPTORS.iter().filter(|d| d.id < PARAM_MATRIX_BASE) {
            let latched = is_latched(d.id);
            let expected = matches!(
                d.id,
                PARAM_TUNE | PARAM_VELOCITY_AMOUNT | PARAM_TONE_PARTIALS
            ) || (40..80).contains(&d.id)
                || (80..90).contains(&d.id);
            assert_eq!(latched, expected, "{} ({})", d.id, d.name);
        }
        // The four that make a hit's shape, and the four that sweep within
        // one, as the plan's own examples.
        for id in [PARAM_AMP_DECAY, PARAM_PITCH_DEPTH, PARAM_BURST_SPACING, PARAM_TUNE] {
            assert!(is_latched(id), "{id} should be latched");
        }
        for id in [
            PARAM_LEVEL,
            PARAM_FILTER_CUTOFF,
            PARAM_TONE_WAVE,
            PARAM_BODY_DAMPING,
            PARAM_DRIVE,
        ] {
            assert!(!is_latched(id), "{id} should be continuous");
        }
    }

    /// A row's destination is persisted as a parameter id, so a patch keeps
    /// meaning what it meant; only the control the UI turns is a position in
    /// the list.
    #[test]
    fn a_route_keeps_its_destination_as_an_id() {
        let mut params = Ds01Params::default();
        let cutoff = destination_index(PARAM_FILTER_CUTOFF).unwrap();
        assert!(set(&mut params, matrix_param(2, MATRIX_OFFSET_DEST), cutoff as f32));
        assert_eq!(params.matrix[2].dest, PARAM_FILTER_CUTOFF);
        assert_eq!(
            get(&params, matrix_param(2, MATRIX_OFFSET_DEST)),
            Some(cutoff as f32)
        );

        // Out of range keeps what it had rather than silently re-pointing.
        assert!(set(
            &mut params,
            matrix_param(2, MATRIX_OFFSET_DEST),
            DS01_DESTINATIONS as f32 + 10.0
        ));
        assert_eq!(params.matrix[2].dest, PARAM_FILTER_CUTOFF);
    }

    /// The default patch ships no routes. The step asks for Velocity to Amp
    /// at full amount so the device feels normal unprogrammed, and
    /// `velocity_amount` at id 5 already does exactly that — the same
    /// paragraph says it stays as the plain control for the common case, so
    /// shipping both would apply velocity twice.
    #[test]
    fn the_default_patch_ships_no_routes() {
        let params = Ds01Params::default();
        assert!(params.matrix.iter().all(|route| !route.is_active()));
        assert_eq!(params.velocity_amount, 1.0);
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
