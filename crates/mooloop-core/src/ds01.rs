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

/// The amplitude envelope's decay. Step 03 fills in 40, 41 and 43-46 around
/// it; this id is the one the instrument cannot be heard without.
pub const PARAM_AMP_DECAY: u32 = 42;

pub const PARAM_PITCH_DECAY: u32 = 51;
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

const ENVELOPE_DESCRIPTORS: [ParamDescriptor; 3] = [
    time_s(PARAM_AMP_DECAY, "Amp decay", 0.002, 4.0, 0.24),
    time_s(PARAM_PITCH_DECAY, "Pitch decay", 0.001, 2.0, 0.045),
    // Bipolar and in semitones, which is the one place DS-01 refuses to copy
    // v1. v1 spells the kick sweep as a start frequency and an end frequency,
    // which is why its ranges could not be shared and why the sweep could not
    // track the note. A depth around the tone pitch tracks correctly,
    // modulates meaningfully, and spells an upward blip as a negative number.
    // +21 semitones over 45 ms from 160 Hz is approximately v1's default kick.
    semitones(PARAM_PITCH_DEPTH, "Pitch depth", -60.0, 60.0, 21.0),
];

/// The complete DS-01 table for this step.
pub static DESCRIPTORS: [ParamDescriptor; 22] = concat(
    GLOBAL_DESCRIPTORS,
    TONE_DESCRIPTORS,
    NOISE_DESCRIPTORS,
    ENVELOPE_DESCRIPTORS,
);

const fn concat(
    global: [ParamDescriptor; 6],
    tone: [ParamDescriptor; 7],
    noise: [ParamDescriptor; 6],
    envelopes: [ParamDescriptor; 3],
) -> [ParamDescriptor; 22] {
    let mut out = [global[0]; 22];
    let mut i = 0;
    while i < 6 {
        out[i] = global[i];
        i += 1;
    }
    let mut j = 0;
    while j < 7 {
        out[6 + j] = tone[j];
        j += 1;
    }
    let mut k = 0;
    while k < 6 {
        out[13 + k] = noise[k];
        k += 1;
    }
    let mut l = 0;
    while l < 3 {
        out[19 + l] = envelopes[l];
        l += 1;
    }
    out
}

/// This device's descriptor for `id`, if it has one.
pub fn descriptor(id: u32) -> Option<&'static ParamDescriptor> {
    DESCRIPTORS.iter().find(|descriptor| descriptor.id == id)
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
    /// Amplitude decay (seconds). Step 03 gives it attack, hold, curve and a
    /// gate; this is the segment that makes the device audible.
    pub amp_decay: f32,
    /// Pitch envelope decay (seconds).
    pub pitch_decay: f32,
    /// Bipolar pitch excursion in semitones, around the tone pitch.
    pub pitch_depth: f32,
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
            amp_decay: 0.24,
            pitch_decay: 0.045,
            pitch_depth: 21.0,
        }
    }
}

/// Read one parameter in natural units by wire id.
pub fn get(p: &Ds01Params, id: u32) -> Option<f32> {
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
        PARAM_AMP_DECAY => p.amp_decay,
        PARAM_PITCH_DECAY => p.pitch_decay,
        PARAM_PITCH_DEPTH => p.pitch_depth,
        _ => return None,
    })
}

/// Write one parameter in natural units by wire id. The caller has already
/// clamped `value` through the descriptor.
pub fn set(p: &mut Ds01Params, id: u32, value: f32) -> bool {
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
        PARAM_AMP_DECAY => p.amp_decay = value,
        PARAM_PITCH_DECAY => p.pitch_decay = value,
        PARAM_PITCH_DEPTH => p.pitch_depth = value,
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

    /// This step owns 0-9, 10-19, 20-29, and exactly three ids inside the two
    /// envelope bands. Anything else would be spending a later step's
    /// reservation.
    #[test]
    fn every_id_lands_in_a_band_this_step_owns() {
        for d in &DESCRIPTORS {
            let owned = d.id < 30
                || d.id == PARAM_AMP_DECAY
                || d.id == PARAM_PITCH_DECAY
                || d.id == PARAM_PITCH_DEPTH;
            assert!(owned, "{} ({}) is outside step 02's bands", d.id, d.name);
        }
        assert_eq!(DESCRIPTORS.len(), 22);
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
            ]
        );
    }

    #[test]
    fn an_id_this_step_has_not_assigned_is_neither_read_nor_written() {
        let mut params = Ds01Params::default();
        // 6 is inside the global band but unassigned; 30 belongs to the body
        // resonator in step 04; 40 to the amplitude envelope in step 03.
        for id in [6, 30, 40, 100] {
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
