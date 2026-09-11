//! The channel strip every track has: an input stage, four EQ bands and a
//! compressor, under one strip-wide voicing.
//!
//! `docs/plans/archive/console/03-the-channel-strip-device.md` is the work
//! order and `THE-STRIP.md` beside it is Adam's mockup. Two things about this file are
//! decisions rather than plumbing.
//!
//! **It is not a device.** There is no `EffectKind`, nothing to insert and
//! nothing to delete: a track has a strip the way it has a fader. What it
//! does have is a *position* in the track's chain, which is
//! [`crate::mixer::STRIP_PIN`] and is stated once.
//!
//! **A voicing selects laws, never values.** Nothing a voicing does moves a
//! number a knob on the face shows: it owns the harmonic profile, the tilt,
//! the slew limit, the Q law, the curve above the knee and the programme
//! dependence, and every one of those is either invisible or drawn.
//! `mooloop_dsp::strip` holds what each voicing is made of, exactly as
//! `mooloop_dsp::preamp` already holds the input stage's half -- this module
//! is only the choice and the values, which is what a project persists.
//!
//! # The id space, and the gap at the bottom of it
//!
//! Ids start at [`STRIP_FIRST`] rather than at 0, and the sixteen below it
//! are deliberately empty. `modulation::STRIP_PARAM_VOLUME` and
//! `STRIP_PARAM_PAN` are 0 and 1 of what is conceptually the *same* strip --
//! `ParamOwner::Strip`, the fader and the pan, already addressable by a
//! modulation route. These parameters are not modulation destinations yet
//! (see the step doc), and when they become ones the two tables should be one
//! table. Leaving the gap is what makes that a merge rather than a renumber
//! of ids automation has already persisted.

use crate::effect::{EqBandKind, EqQProfile, ParamCurve, ParamDescriptor, PreampVoicing};

/// Bands in a strip's EQ, read left to right and top to bottom: high shelf,
/// high mid, low mid, low shelf. Adam, 2026-09-09: *"this has been annoying
/// ppl since EQs were invented. there's no good way to lay it out. i chose
/// ltr ttb."*
pub const STRIP_EQ_BANDS: usize = 4;

/// First id in the strip's own parameter space. See the module header for
/// what the sixteen below it are reserved for.
pub const STRIP_FIRST: u32 = 16;

pub const STRIP_VOICING: u32 = STRIP_FIRST;
pub const STRIP_PRE_IN: u32 = STRIP_FIRST + 1;
pub const STRIP_DRIVE_DB: u32 = STRIP_FIRST + 2;
pub const STRIP_EQ_IN: u32 = STRIP_FIRST + 3;

/// Id of band 0's first field. A band's four fields are contiguous, so a
/// band's ids are `STRIP_BAND_BASE + band * STRIP_BAND_STRIDE + field`.
pub const STRIP_BAND_BASE: u32 = STRIP_FIRST + 4;
pub const STRIP_BAND_STRIDE: u32 = 4;
pub const STRIP_BAND_FREQ: u32 = 0;
pub const STRIP_BAND_GAIN: u32 = 1;
pub const STRIP_BAND_Q: u32 = 2;
pub const STRIP_BAND_KIND: u32 = 3;

pub const STRIP_COMP_IN: u32 = STRIP_BAND_BASE + STRIP_EQ_BANDS as u32 * STRIP_BAND_STRIDE;
pub const STRIP_COMP_THRESHOLD_DB: u32 = STRIP_COMP_IN + 1;
pub const STRIP_COMP_RATIO: u32 = STRIP_COMP_IN + 2;
pub const STRIP_COMP_IN_TRIM_DB: u32 = STRIP_COMP_IN + 3;
pub const STRIP_COMP_ATTACK_MS: u32 = STRIP_COMP_IN + 4;
pub const STRIP_COMP_RELEASE_MS: u32 = STRIP_COMP_IN + 5;
pub const STRIP_COMP_KNEE_DB: u32 = STRIP_COMP_IN + 6;
pub const STRIP_COMP_MIX: u32 = STRIP_COMP_IN + 7;
pub const STRIP_COMP_MAKEUP_DB: u32 = STRIP_COMP_IN + 8;

/// The id of one field of one band.
pub const fn strip_band_param(band: usize, field: u32) -> u32 {
    STRIP_BAND_BASE + band as u32 * STRIP_BAND_STRIDE + field
}

/// Which band and which field an id names, or `None` if it is not a band id.
pub const fn strip_band_of(id: u32) -> Option<(usize, u32)> {
    if id < STRIP_BAND_BASE || id >= STRIP_COMP_IN {
        return None;
    }
    let offset = id - STRIP_BAND_BASE;
    Some((
        (offset / STRIP_BAND_STRIDE) as usize,
        offset % STRIP_BAND_STRIDE,
    ))
}

/// The shelf a band turns into when its `Type` is switched over: the top two
/// bands become high shelves and the bottom two low shelves.
///
/// A band offers its *own* shelf and nothing else, which is why `Type` is a
/// two-position switch rather than the three `EqBandKind` values. A high
/// shelf on the low band is a state nothing should be able to reach by
/// clicking, and the seven-band EQ -- where every band offers all three --
/// is the device for wanting it.
pub const fn strip_band_shelf(band: usize) -> EqBandKind {
    if band < STRIP_EQ_BANDS / 2 {
        EqBandKind::HighShelf
    } else {
        EqBandKind::LowShelf
    }
}

/// One band of the strip's EQ.
///
/// `q` is the bell's Q and the shelf's **slope**, which is the same knob
/// meaning the same kind of thing: how abruptly the band gives way. The two
/// shelf-capable bands therefore carry a narrower range than the two mid
/// bands, because a cookbook shelf's steepest useful slope is 2 -- past that
/// its radicand goes negative. A knob that saturated instead would be
/// showing a number the filter is not using.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StripBand {
    pub kind: EqBandKind,
    pub frequency_hz: f32,
    pub gain_db: f32,
    pub q: f32,
}

/// The four sections, as a track persists them.
///
/// Every switch is off and every value is the neutral one, so a strip that
/// nobody has touched is bit-identical to no strip at all -- which is what
/// entitles it to exist on all 256 tracks. `#[serde(default)]` on
/// `MixerBus::strip` is the migration: a song saved before this existed
/// opens with exactly this.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StripParams {
    /// Strip-wide, and the only control that governs all three sections.
    pub voicing: PreampVoicing,
    pub pre_in: bool,
    /// Gain into the voicing's curve. The profiles are authored at the
    /// -12 dBFS operating level, so 0 dB is where a voicing measures true.
    pub drive_db: f32,
    pub eq_in: bool,
    pub bands: [StripBand; STRIP_EQ_BANDS],
    pub comp_in: bool,
    pub threshold_db: f32,
    pub ratio: f32,
    /// Drive into the compressor: it moves the detector and the level
    /// together, which is what an input trim on a desk does. The dry side of
    /// `mix` does not see it, so `mix` at 0 is the signal as it arrived.
    pub in_trim_db: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub knee_db: f32,
    /// Parallel balance: 0 is the dry signal exactly, 1 the compressor
    /// alone. Adam: *"w/d is just a wet/dry balance control for parallel per
    /// strip"* -- not the device host's blend, which a strip does not have.
    pub mix: f32,
    pub makeup_db: f32,
}

/// Spelled once and read by both the defaults and the descriptor table,
/// which would otherwise be the same four numbers written twice.
const DEFAULT_FREQUENCIES: [f32; STRIP_EQ_BANDS] = [8_000.0, 3_000.0, 400.0, 100.0];
const DEFAULT_QS: [f32; STRIP_EQ_BANDS] = [0.707, 0.9, 0.9, 0.707];
/// `Type` positions: the two outer bands arrive as their shelf, the two mid
/// bands as bells.
const DEFAULT_TYPES: [f32; STRIP_EQ_BANDS] = [1.0, 0.0, 0.0, 1.0];

impl Default for StripParams {
    fn default() -> Self {
        let band = |index: usize| StripBand {
            kind: if DEFAULT_TYPES[index] >= 0.5 {
                strip_band_shelf(index)
            } else {
                EqBandKind::Bell
            },
            frequency_hz: DEFAULT_FREQUENCIES[index],
            gain_db: 0.0,
            q: DEFAULT_QS[index],
        };
        Self {
            voicing: PreampVoicing::Moo,
            pre_in: false,
            drive_db: 0.0,
            eq_in: false,
            bands: [band(0), band(1), band(2), band(3)],
            comp_in: false,
            threshold_db: -18.0,
            ratio: 3.0,
            in_trim_db: 0.0,
            attack_ms: 10.0,
            release_ms: 120.0,
            knee_db: 6.0,
            mix: 1.0,
            makeup_db: 0.0,
        }
    }
}

impl StripParams {
    /// Every parameter's range, curve and default, in id order.
    ///
    /// The one table a face is held to. `slint_face_agreement.rs` reads it,
    /// which is what stops a range being spelled once in Rust and once in
    /// the markup -- the fault `AGENTS.md` opens on.
    pub fn descriptors() -> &'static [ParamDescriptor] {
        &DESCRIPTORS
    }

    pub fn descriptor(id: u32) -> Option<&'static ParamDescriptor> {
        DESCRIPTORS.iter().find(|descriptor| descriptor.id == id)
    }

    /// Whether the Q law is proportional for this strip's voicing -- the one
    /// thing a voicing does that a knob's reading depends on, and it is
    /// drawn by the response display rather than merely applied.
    pub fn proportional_q(&self) -> bool {
        matches!(self.voicing, PreampVoicing::Grip | PreampVoicing::Punch)
    }

    /// The Q a band is actually running, after the voicing's law.
    ///
    /// The response display plots this, not `band.q`, which is the condition
    /// that makes a law-selecting voicing honest.
    pub fn effective_q(&self, band: usize) -> f32 {
        let Some(band) = self.bands.get(band) else {
            return 0.707;
        };
        crate::effect::eq_effective_q(
            band.q,
            band.gain_db,
            if self.proportional_q() {
                EqQProfile::Proportional
            } else {
                EqQProfile::Constant
            },
        )
    }

    /// Whether anything at all is switched in. A strip that answers `true`
    /// is skipped whole, which is the "free while it is out" rule and the
    /// reason the sections default to off.
    pub fn is_out(&self) -> bool {
        !self.pre_in && !self.eq_in && !self.comp_in
    }

    pub fn get(&self, id: u32) -> Option<f32> {
        if let Some((index, field)) = strip_band_of(id) {
            let band = self.bands.get(index)?;
            return Some(match field {
                STRIP_BAND_FREQ => band.frequency_hz,
                STRIP_BAND_GAIN => band.gain_db,
                STRIP_BAND_Q => band.q,
                STRIP_BAND_KIND => switch_to_f32(band.kind != EqBandKind::Bell),
                _ => return None,
            });
        }
        Some(match id {
            STRIP_VOICING => self.voicing.to_index() as f32,
            STRIP_PRE_IN => switch_to_f32(self.pre_in),
            STRIP_DRIVE_DB => self.drive_db,
            STRIP_EQ_IN => switch_to_f32(self.eq_in),
            STRIP_COMP_IN => switch_to_f32(self.comp_in),
            STRIP_COMP_THRESHOLD_DB => self.threshold_db,
            STRIP_COMP_RATIO => self.ratio,
            STRIP_COMP_IN_TRIM_DB => self.in_trim_db,
            STRIP_COMP_ATTACK_MS => self.attack_ms,
            STRIP_COMP_RELEASE_MS => self.release_ms,
            STRIP_COMP_KNEE_DB => self.knee_db,
            STRIP_COMP_MIX => self.mix,
            STRIP_COMP_MAKEUP_DB => self.makeup_db,
            _ => return None,
        })
    }

    /// Store `value` under `id`, clamped to the descriptor's range, and
    /// report whether the id was one this strip has.
    ///
    /// Clamped *here* rather than in the DSP, so the model and the audio
    /// cannot hold different numbers: the engine publishes these back to the
    /// face, which would otherwise show a value nothing is using.
    pub fn set(&mut self, id: u32, value: f32) -> bool {
        let Some(descriptor) = Self::descriptor(id) else {
            return false;
        };
        let value = if value.is_finite() {
            value.clamp(descriptor.min, descriptor.max)
        } else {
            descriptor.default
        };
        if let Some((index, field)) = strip_band_of(id) {
            let shelf = strip_band_shelf(index);
            let Some(band) = self.bands.get_mut(index) else {
                return false;
            };
            match field {
                STRIP_BAND_FREQ => band.frequency_hz = value,
                STRIP_BAND_GAIN => band.gain_db = value,
                STRIP_BAND_Q => band.q = value,
                STRIP_BAND_KIND => {
                    band.kind = if switch_from_f32(value) {
                        shelf
                    } else {
                        EqBandKind::Bell
                    }
                }
                _ => return false,
            }
            return true;
        }
        match id {
            STRIP_VOICING => self.voicing = PreampVoicing::from_index(value.round() as i32),
            STRIP_PRE_IN => self.pre_in = switch_from_f32(value),
            STRIP_DRIVE_DB => self.drive_db = value,
            STRIP_EQ_IN => self.eq_in = switch_from_f32(value),
            STRIP_COMP_IN => self.comp_in = switch_from_f32(value),
            STRIP_COMP_THRESHOLD_DB => self.threshold_db = value,
            STRIP_COMP_RATIO => self.ratio = value,
            STRIP_COMP_IN_TRIM_DB => self.in_trim_db = value,
            STRIP_COMP_ATTACK_MS => self.attack_ms = value,
            STRIP_COMP_RELEASE_MS => self.release_ms = value,
            STRIP_COMP_KNEE_DB => self.knee_db = value,
            STRIP_COMP_MIX => self.mix = value,
            STRIP_COMP_MAKEUP_DB => self.makeup_db = value,
            _ => return false,
        }
        true
    }
}

/// A switch crosses the parameter wire as a number, like every other
/// parameter, so that one command carries the whole strip.
fn switch_to_f32(on: bool) -> f32 {
    if on {
        1.0
    } else {
        0.0
    }
}

fn switch_from_f32(value: f32) -> bool {
    value >= 0.5
}

/// Frequency reach per band. Wide and overlapping on purpose: a high shelf
/// at 1 kHz and a low shelf at 500 Hz are both ordinary things to ask a desk
/// for, and a band that cannot leave its own octave is a band you have to
/// plan around.
const BAND_RANGES: [(f32, f32); STRIP_EQ_BANDS] = [
    (1_000.0, 20_000.0),
    (400.0, 16_000.0),
    (40.0, 2_000.0),
    (20.0, 800.0),
];

/// Top of a band's Q knob. The outer two reach 2, which is where a cookbook
/// shelf stops being a shelf; the mid two reach 8, which is a bell narrow
/// enough to notch with. See [`StripBand`].
const BAND_Q_MAX: [f32; STRIP_EQ_BANDS] = [2.0, 8.0, 8.0, 2.0];

/// The table itself. The sixteen band rows are written out rather than
/// generated, because a `const fn` cannot `concat!` a name and a table whose
/// rows a reader cannot see is not a table anybody will check a face
/// against. Every *number* in them still comes from the four arrays above,
/// so a default lives in one place.
static DESCRIPTORS: [ParamDescriptor; 4 + STRIP_EQ_BANDS * 4 + 9] = [
    ParamDescriptor {
        id: STRIP_VOICING,
        name: "Voicing",
        unit: "",
        min: 0.0,
        max: 3.0,
        curve: ParamCurve::Stepped(4),
        default: 0.0,
    },
    ParamDescriptor {
        id: STRIP_PRE_IN,
        name: "Pre In",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Stepped(2),
        default: 0.0,
    },
    ParamDescriptor {
        id: STRIP_DRIVE_DB,
        name: "Drive",
        unit: "dB",
        min: -24.0,
        max: 24.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    ParamDescriptor {
        id: STRIP_EQ_IN,
        name: "EQ In",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Stepped(2),
        default: 0.0,
    },
    ParamDescriptor {
        id: strip_band_param(0, STRIP_BAND_FREQ),
        name: "HS Freq",
        unit: "Hz",
        min: BAND_RANGES[0].0,
        max: BAND_RANGES[0].1,
        curve: ParamCurve::Exponential,
        default: DEFAULT_FREQUENCIES[0],
    },
    ParamDescriptor {
        id: strip_band_param(0, STRIP_BAND_GAIN),
        name: "HS Gain",
        unit: "dB",
        min: -18.0,
        max: 18.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    ParamDescriptor {
        id: strip_band_param(0, STRIP_BAND_Q),
        name: "HS Q",
        unit: "",
        min: 0.2,
        max: BAND_Q_MAX[0],
        curve: ParamCurve::Exponential,
        default: DEFAULT_QS[0],
    },
    ParamDescriptor {
        id: strip_band_param(0, STRIP_BAND_KIND),
        name: "HS Type",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Stepped(2),
        default: DEFAULT_TYPES[0],
    },
    ParamDescriptor {
        id: strip_band_param(1, STRIP_BAND_FREQ),
        name: "HM Freq",
        unit: "Hz",
        min: BAND_RANGES[1].0,
        max: BAND_RANGES[1].1,
        curve: ParamCurve::Exponential,
        default: DEFAULT_FREQUENCIES[1],
    },
    ParamDescriptor {
        id: strip_band_param(1, STRIP_BAND_GAIN),
        name: "HM Gain",
        unit: "dB",
        min: -18.0,
        max: 18.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    ParamDescriptor {
        id: strip_band_param(1, STRIP_BAND_Q),
        name: "HM Q",
        unit: "",
        min: 0.2,
        max: BAND_Q_MAX[1],
        curve: ParamCurve::Exponential,
        default: DEFAULT_QS[1],
    },
    ParamDescriptor {
        id: strip_band_param(1, STRIP_BAND_KIND),
        name: "HM Type",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Stepped(2),
        default: DEFAULT_TYPES[1],
    },
    ParamDescriptor {
        id: strip_band_param(2, STRIP_BAND_FREQ),
        name: "LM Freq",
        unit: "Hz",
        min: BAND_RANGES[2].0,
        max: BAND_RANGES[2].1,
        curve: ParamCurve::Exponential,
        default: DEFAULT_FREQUENCIES[2],
    },
    ParamDescriptor {
        id: strip_band_param(2, STRIP_BAND_GAIN),
        name: "LM Gain",
        unit: "dB",
        min: -18.0,
        max: 18.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    ParamDescriptor {
        id: strip_band_param(2, STRIP_BAND_Q),
        name: "LM Q",
        unit: "",
        min: 0.2,
        max: BAND_Q_MAX[2],
        curve: ParamCurve::Exponential,
        default: DEFAULT_QS[2],
    },
    ParamDescriptor {
        id: strip_band_param(2, STRIP_BAND_KIND),
        name: "LM Type",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Stepped(2),
        default: DEFAULT_TYPES[2],
    },
    ParamDescriptor {
        id: strip_band_param(3, STRIP_BAND_FREQ),
        name: "LS Freq",
        unit: "Hz",
        min: BAND_RANGES[3].0,
        max: BAND_RANGES[3].1,
        curve: ParamCurve::Exponential,
        default: DEFAULT_FREQUENCIES[3],
    },
    ParamDescriptor {
        id: strip_band_param(3, STRIP_BAND_GAIN),
        name: "LS Gain",
        unit: "dB",
        min: -18.0,
        max: 18.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    ParamDescriptor {
        id: strip_band_param(3, STRIP_BAND_Q),
        name: "LS Q",
        unit: "",
        min: 0.2,
        max: BAND_Q_MAX[3],
        curve: ParamCurve::Exponential,
        default: DEFAULT_QS[3],
    },
    ParamDescriptor {
        id: strip_band_param(3, STRIP_BAND_KIND),
        name: "LS Type",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Stepped(2),
        default: DEFAULT_TYPES[3],
    },
    ParamDescriptor {
        id: STRIP_COMP_IN,
        name: "Comp In",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Stepped(2),
        default: 0.0,
    },
    ParamDescriptor {
        id: STRIP_COMP_THRESHOLD_DB,
        name: "Thresh",
        unit: "dB",
        min: -60.0,
        max: 0.0,
        curve: ParamCurve::Linear,
        default: -18.0,
    },
    ParamDescriptor {
        id: STRIP_COMP_RATIO,
        name: "Ratio",
        unit: ":1",
        min: 1.0,
        max: 20.0,
        curve: ParamCurve::Exponential,
        default: 3.0,
    },
    ParamDescriptor {
        id: STRIP_COMP_IN_TRIM_DB,
        name: "In Trim",
        unit: "dB",
        min: -24.0,
        max: 24.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    ParamDescriptor {
        id: STRIP_COMP_ATTACK_MS,
        name: "Attack",
        unit: "ms",
        min: 0.05,
        max: 200.0,
        curve: ParamCurve::Exponential,
        default: 10.0,
    },
    ParamDescriptor {
        id: STRIP_COMP_RELEASE_MS,
        name: "Release",
        unit: "ms",
        min: 5.0,
        max: 2_000.0,
        curve: ParamCurve::Exponential,
        default: 120.0,
    },
    ParamDescriptor {
        id: STRIP_COMP_KNEE_DB,
        name: "Knee",
        unit: "dB",
        min: 0.0,
        max: 24.0,
        curve: ParamCurve::Linear,
        default: 6.0,
    },
    ParamDescriptor {
        id: STRIP_COMP_MIX,
        name: "W/D Mix",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 1.0,
    },
    ParamDescriptor {
        id: STRIP_COMP_MAKEUP_DB,
        name: "Makeup",
        unit: "dB",
        min: 0.0,
        max: 24.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The defaults and the descriptor table are two statements of the same
    /// neutral strip, and the one that drifts is the one that decides what a
    /// reset knob does. Every descriptor's default has to be the value
    /// `StripParams::default()` actually holds.
    #[test]
    fn every_descriptor_default_is_the_default_strip() {
        let params = StripParams::default();
        for descriptor in StripParams::descriptors() {
            let held = params.get(descriptor.id).unwrap_or_else(|| {
                panic!("no value behind {} ({})", descriptor.name, descriptor.id)
            });
            assert!(
                (held - descriptor.default).abs() < 1e-6,
                "{} ({}) defaults to {} in the table and {} in the struct",
                descriptor.name,
                descriptor.id,
                descriptor.default,
                held
            );
            assert!(
                held >= descriptor.min && held <= descriptor.max,
                "{}'s default {} is outside its own range",
                descriptor.name,
                held
            );
        }
    }

    /// Every id the table carries round-trips, and nothing outside it is
    /// accepted -- so a stale id from an older build cannot quietly land on
    /// a neighbouring parameter.
    #[test]
    fn every_parameter_round_trips_and_strangers_are_refused() {
        let mut params = StripParams::default();
        for descriptor in StripParams::descriptors() {
            let target = descriptor.max;
            assert!(params.set(descriptor.id, target), "{}", descriptor.name);
            let back = params.get(descriptor.id).unwrap();
            assert!(
                (back - target).abs() <= 1e-3 * (1.0 + descriptor.max.abs()),
                "{} ({}) stored {target} and read back {back}",
                descriptor.name,
                descriptor.id
            );
        }
        assert!(!params.set(0, 1.0), "the fader's id is not a strip section");
        assert!(!params.set(1, 1.0), "the pan's id is not a strip section");
        assert!(!params.set(STRIP_COMP_MAKEUP_DB + 1, 1.0));
        assert!(params.get(STRIP_COMP_MAKEUP_DB + 1).is_none());
    }

    /// A value outside a parameter's range is clamped rather than stored, and
    /// a value that is not a number at all falls back to the default. The
    /// engine reads these back to publish them, so a strip holding `NaN`
    /// would be a face showing `NaN`.
    #[test]
    fn a_wild_value_is_clamped_and_a_nan_is_refused() {
        let mut params = StripParams::default();
        params.set(STRIP_COMP_RATIO, 1e9);
        assert_eq!(params.ratio, 20.0);
        params.set(STRIP_COMP_THRESHOLD_DB, -400.0);
        assert_eq!(params.threshold_db, -60.0);
        params.set(STRIP_DRIVE_DB, f32::NAN);
        assert_eq!(params.drive_db, 0.0);
    }

    /// The id space is contiguous from `STRIP_FIRST` and the gap below it is
    /// the one the module header describes. Both halves matter: a hole in
    /// the middle would be a parameter nothing can reach, and a table that
    /// started at 0 would collide with the fader and the pan.
    #[test]
    fn the_id_space_starts_after_the_faders_and_has_no_holes() {
        assert_eq!(STRIP_FIRST, 16);
        assert!(StripParams::descriptor(crate::modulation::STRIP_PARAM_VOLUME).is_none());
        assert!(StripParams::descriptor(crate::modulation::STRIP_PARAM_PAN).is_none());
        let mut ids: Vec<u32> = StripParams::descriptors().iter().map(|d| d.id).collect();
        ids.sort_unstable();
        for (offset, id) in ids.iter().enumerate() {
            assert_eq!(
                *id,
                STRIP_FIRST + offset as u32,
                "a hole at offset {offset}"
            );
        }
    }

    #[test]
    fn a_band_id_says_which_band_and_which_field() {
        for band in 0..STRIP_EQ_BANDS {
            for field in [
                STRIP_BAND_FREQ,
                STRIP_BAND_GAIN,
                STRIP_BAND_Q,
                STRIP_BAND_KIND,
            ] {
                let id = strip_band_param(band, field);
                assert_eq!(strip_band_of(id), Some((band, field)));
            }
        }
        assert_eq!(strip_band_of(STRIP_DRIVE_DB), None);
        assert_eq!(strip_band_of(STRIP_COMP_IN), None);
        assert_eq!(strip_band_of(STRIP_COMP_RATIO), None);
    }

    /// A band offers its own shelf and nothing else, so switching `Type`
    /// twice comes back to where it started rather than walking through a
    /// high shelf on the bottom band.
    #[test]
    fn a_bands_type_switch_offers_its_own_shelf_only() {
        let mut params = StripParams::default();
        for band in 0..STRIP_EQ_BANDS {
            let id = strip_band_param(band, STRIP_BAND_KIND);
            params.set(id, 1.0);
            assert_eq!(params.bands[band].kind, strip_band_shelf(band));
            params.set(id, 0.0);
            assert_eq!(params.bands[band].kind, EqBandKind::Bell);
        }
        assert_eq!(strip_band_shelf(0), EqBandKind::HighShelf);
        assert_eq!(strip_band_shelf(3), EqBandKind::LowShelf);
    }

    /// The whole cost argument: an untouched strip is out, and out is not a
    /// setting that happens to be neutral -- it is three switches nothing
    /// has turned on.
    #[test]
    fn a_default_strip_is_out() {
        assert!(StripParams::default().is_out());
        let with_eq = StripParams {
            eq_in: true,
            ..StripParams::default()
        };
        assert!(!with_eq.is_out());
    }

    /// The one place a voicing touches what a knob means, and the two it
    /// does not.
    #[test]
    fn only_two_voicings_narrow_a_boosted_band() {
        let mut params = StripParams::default();
        // Not a field assignment on a fresh `default()`, which clippy denies
        // (`field_reassign_with_default`): this is the band inside an array.
        params.bands[1] = StripBand {
            gain_db: 12.0,
            ..params.bands[1]
        };
        for (voicing, proportional) in [
            (PreampVoicing::Moo, false),
            (PreampVoicing::Grip, true),
            (PreampVoicing::Punch, true),
            (PreampVoicing::Iron, false),
        ] {
            params.voicing = voicing;
            assert_eq!(params.proportional_q(), proportional, "{voicing:?}");
            let effective = params.effective_q(1);
            if proportional {
                assert!(
                    effective > params.bands[1].q * 1.5,
                    "{voicing:?} {effective}"
                );
            } else {
                assert!((effective - params.bands[1].q).abs() < 1e-6, "{voicing:?}");
            }
        }
    }
}
