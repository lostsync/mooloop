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
//!
//! The compressor's ids **were** renumbered once, on 2026-09-11, when Adam
//! took the input trim off the face -- *"no input gain"* -- and the five
//! below it moved down to close the hole. That was safe precisely because
//! nothing persists one of these: a project stores named fields, and a strip
//! parameter is not an automation destination yet. It stops being safe the
//! day it becomes one, and from then on the rule
//! [`crate::ParamDescriptor`] states applies -- append, never renumber. The
//! hole matters because `strip.slint` finds a descriptor by arithmetic on
//! its position, not by search.

use crate::effect::{EqBandKind, ParamCurve, ParamDescriptor, PreampVoicing};

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
pub const STRIP_COMP_ATTACK_MS: u32 = STRIP_COMP_IN + 3;
pub const STRIP_COMP_RELEASE_MS: u32 = STRIP_COMP_IN + 4;
pub const STRIP_COMP_KNEE_DB: u32 = STRIP_COMP_IN + 5;
pub const STRIP_COMP_MIX: u32 = STRIP_COMP_IN + 6;
pub const STRIP_COMP_MAKEUP_DB: u32 = STRIP_COMP_IN + 7;

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
    /// **Which of this band's positions is selected, not a frequency.**
    ///
    /// A band has [`STRIP_BAND_POSITIONS`] of them and the voicing decides
    /// what each one is worth in hertz -- `mooloop_dsp::strip` holds the
    /// tables. Adam, 2026-09-11, settling the thing this file had refused:
    /// *"i want to have a selectable range of freqs in the eq bands, not
    /// fully parametric. you didnt want to do that before bc it would make
    /// the labels lie... i say we dont label. we just have N
    /// positions/band and they're selectable."*
    ///
    /// Which is why it does not break "a voicing selects laws, never
    /// values": the face shows a *position*, and 3 of 7 is true under every
    /// voicing. The hertz arrives on hover, from the voicing that is
    /// running.
    ///
    /// Defaulted, because it replaced a `frequency_hz` that a song saved on
    /// 2026-09-11 may hold: serde ignores the old field and such a band opens
    /// at the middle of its own range. The default is *not* on this field --
    /// it cannot be, because the middle differs per band -- it is in
    /// [`StripParams`]'s `bands`, which fills it from the band's own index.
    /// See `docs/PROJECT_FORMAT.md`.
    pub position: u8,
    pub gain_db: f32,
    pub q: f32,
}

/// `bands`, with a band that states no `position` opening on the middle of
/// *its own* set rather than on one number chosen for all four.
///
/// This exists because `#[serde(default = "fn")]` gets no array index. The
/// field default it replaced returned a bare `2`, which is the middle of the
/// five-position outer bands and one step low for the seven-position mids --
/// so under `MOO_EQ` a band 1 that meant 3 kHz opened at 2 kHz, against a
/// face and a `double-click to default` that both said 3, and against
/// `PROJECT_FORMAT.md`'s own claim that such a band "loads with the band
/// centred". Four things stated what the middle was and the one serde reached
/// was the only one that was wrong.
///
/// The wire type is deliberately no more forgiving than the struct: `kind`,
/// `gain_db` and `q` have no default here either, so a band missing one of
/// them still fails to load. Only `position` is optional, and only because
/// one day's files may predate it. An unknown field -- `frequency_hz` -- is
/// ignored, which is serde's default and the whole migration.
fn bands_with_their_own_middles<'de, D>(
    deserializer: D,
) -> Result<[StripBand; STRIP_EQ_BANDS], D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    struct Wire {
        kind: EqBandKind,
        position: Option<u8>,
        gain_db: f32,
        q: f32,
    }

    use serde::Deserialize as _;
    let wire = <[Wire; STRIP_EQ_BANDS]>::deserialize(deserializer)?;
    Ok(std::array::from_fn(|index| StripBand {
        kind: wire[index].kind,
        position: wire[index].position.unwrap_or(DEFAULT_POSITIONS[index]),
        gain_db: wire[index].gain_db,
        q: wire[index].q,
    }))
}

/// The four sections, as a track persists them.
///
/// Every switch is off and every value is the neutral one, so a strip that
/// nobody has touched is bit-identical to no strip at all -- which is what
/// entitles it to exist on every track a song has, and on however many
/// [`crate::MAX_BUSES`] later becomes. It is seventeen today;
/// `docs/CAPACITY_POLICY.md` prices the `u8` address space at 11.25 MB and
/// says what to fix first, and the strip's own share of that is the few
/// hundred bytes `a_strip_is_a_few_hundred_bytes` measures.
/// `#[serde(default)]` on `MixerBus::strip` is the migration: a song saved
/// before this existed opens with exactly this.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StripParams {
    /// Strip-wide, and the only control that governs all three sections.
    pub voicing: PreampVoicing,
    pub pre_in: bool,
    /// Gain into the voicing's curve. The profiles are authored at the
    /// -12 dBFS operating level, so 0 dB is where a voicing measures true.
    pub drive_db: f32,
    pub eq_in: bool,
    #[serde(deserialize_with = "bands_with_their_own_middles")]
    pub bands: [StripBand; STRIP_EQ_BANDS],
    pub comp_in: bool,
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub knee_db: f32,
    /// Parallel balance: 0 is the dry signal exactly, 1 the compressor
    /// alone. Adam: *"w/d is just a wet/dry balance control for parallel per
    /// strip"* -- not the device host's blend, which a strip does not have.
    pub mix: f32,
    pub makeup_db: f32,
}

/// How many frequency positions each band offers, outer bands first.
///
/// Five on the shelves and seven on the mids, Adam's 2026-09-11 ruling and
/// the idiomatic shape: a low shelf does not need seven choices between
/// 20 and 800 Hz, and a mid does. `mooloop_dsp::strip`'s tables are held to
/// these lengths for every voicing, so a position means the same *slot* in
/// all four and only its frequency changes.
pub const STRIP_BAND_POSITIONS: [u8; STRIP_EQ_BANDS] = [5, 7, 7, 5];

/// The position each band arrives on: the middle of its own set. Read by
/// both the defaults and the descriptor table, which would otherwise be the
/// same four numbers written twice.
const DEFAULT_POSITIONS: [u8; STRIP_EQ_BANDS] = [2, 3, 3, 2];
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
            position: DEFAULT_POSITIONS[index],
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
                STRIP_BAND_FREQ => band.position as f32,
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
                STRIP_BAND_FREQ => band.position = value.round().max(0.0) as u8,
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

/// Top of a band's Q knob. The outer two reach 2, which is where a cookbook
/// shelf stops being a shelf; the mid two reach 8, which is a bell narrow
/// enough to notch with. See [`StripBand`].
const BAND_Q_MAX: [f32; STRIP_EQ_BANDS] = [2.0, 8.0, 8.0, 2.0];

/// The table itself. The sixteen band rows are written out rather than
/// generated, because a `const fn` cannot `concat!` a name and a table whose
/// rows a reader cannot see is not a table anybody will check a face
/// against. Every *number* in them still comes from the four arrays above,
/// so a default lives in one place.
static DESCRIPTORS: [ParamDescriptor; 4 + STRIP_EQ_BANDS * 4 + 8] = [
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
        // A position, not a frequency: the unit is empty because there is
        // no unit to state. What the position is worth in hertz is the
        // voicing's, and `mooloop_dsp::strip::band_frequency` answers it.
        unit: "",
        min: 0.0,
        max: (STRIP_BAND_POSITIONS[0] - 1) as f32,
        curve: ParamCurve::Stepped(STRIP_BAND_POSITIONS[0] as u16),
        default: DEFAULT_POSITIONS[0] as f32,
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
        // A position, not a frequency: the unit is empty because there is
        // no unit to state. What the position is worth in hertz is the
        // voicing's, and `mooloop_dsp::strip::band_frequency` answers it.
        unit: "",
        min: 0.0,
        max: (STRIP_BAND_POSITIONS[1] - 1) as f32,
        curve: ParamCurve::Stepped(STRIP_BAND_POSITIONS[1] as u16),
        default: DEFAULT_POSITIONS[1] as f32,
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
        // A position, not a frequency: the unit is empty because there is
        // no unit to state. What the position is worth in hertz is the
        // voicing's, and `mooloop_dsp::strip::band_frequency` answers it.
        unit: "",
        min: 0.0,
        max: (STRIP_BAND_POSITIONS[2] - 1) as f32,
        curve: ParamCurve::Stepped(STRIP_BAND_POSITIONS[2] as u16),
        default: DEFAULT_POSITIONS[2] as f32,
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
        // A position, not a frequency: the unit is empty because there is
        // no unit to state. What the position is worth in hertz is the
        // voicing's, and `mooloop_dsp::strip::band_frequency` answers it.
        unit: "",
        min: 0.0,
        max: (STRIP_BAND_POSITIONS[3] - 1) as f32,
        curve: ParamCurve::Stepped(STRIP_BAND_POSITIONS[3] as u16),
        default: DEFAULT_POSITIONS[3] as f32,
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

// ---------------------------------------------------------------------------
// The master section
// ---------------------------------------------------------------------------

/// The master bus compressor's three voicings (`docs/plans/master-bus-compressor/`,
/// MOO-13).
///
/// **A voicing is a law, not a set of values** -- the channel strip's rule --
/// and here the three laws are measured units (`docs/SCOPE.md` §2.1): Grip is
/// the SSL G-bus's peak VCA, Punch the API-2500's RMS VCA, Tube the
/// Fairchild 670's vari-mu. Named the way the strip names its voicings, after
/// what they do rather than whose they are. `mooloop_dsp::strip::bus_comp`
/// holds what each one is made of.
///
/// Persisted by name, so renaming one is a serde alias rather than a
/// migration.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum BusCompVoicing {
    /// Peak detection, a soft knee, and the SSL's marked times.
    #[default]
    Grip,
    /// RMS detection, a near-hard knee, and the API-2500's attack floor.
    Punch,
    /// Peak detection on the 670's own curve, and its six coupled TIME
    /// positions in place of attack and release.
    Tube,
}

impl BusCompVoicing {
    pub const ALL: [Self; 3] = [Self::Grip, Self::Punch, Self::Tube];

    /// The position a stepped control stores. Out-of-range input clamps to
    /// the nearest end, the `ALL`-table convention.
    pub fn from_index(index: i32) -> Self {
        Self::ALL[index.clamp(0, Self::ALL.len() as i32 - 1) as usize]
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Grip => 0,
            Self::Punch => 1,
            Self::Tube => 2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Grip => "GRIP",
            Self::Punch => "PUNCH",
            Self::Tube => "TUBE",
        }
    }
}

/// Positions on each voicing's own switches. Grip's are the SSL's panel
/// (ratio 2/4/10, six attacks, four releases and Auto); Punch's the
/// API-2500's (six ratios, seven attacks, six releases); Tube's the 670's
/// six TIME positions. `mooloop_dsp::strip::bus_comp` says what each
/// position is worth and is held to these lengths.
pub const GRIP_RATIO_POSITIONS: u8 = 3;
pub const GRIP_ATTACK_POSITIONS: u8 = 6;
pub const GRIP_RELEASE_POSITIONS: u8 = 5;
pub const PUNCH_RATIO_POSITIONS: u8 = 6;
pub const PUNCH_ATTACK_POSITIONS: u8 = 7;
pub const PUNCH_RELEASE_POSITIONS: u8 = 6;
pub const TUBE_TIME_POSITIONS: u8 = 6;

/// The longest lookahead the master's safety limiter offers, in
/// milliseconds. Adam, 2026-09-23 (MOO-169): *"make it a knob, defaults to
/// 0.0"*; a few milliseconds is the usual span.
pub const MAX_LOOKAHEAD_MS: f32 = 5.0;

/// First id of the master section: the next one after the strip's own
/// table, so the two tables are one contiguous id space and
/// `StripSpec`'s arithmetic on position still finds each row.
pub const MASTER_FIRST: u32 = STRIP_COMP_MAKEUP_DB + 1;
pub const MASTER_COMP_IN: u32 = MASTER_FIRST;
pub const MASTER_COMP_VOICING: u32 = MASTER_FIRST + 1;
pub const MASTER_COMP_THRESHOLD_DB: u32 = MASTER_FIRST + 2;
pub const MASTER_COMP_MAKEUP_DB: u32 = MASTER_FIRST + 3;
pub const MASTER_COMP_MIX: u32 = MASTER_FIRST + 4;
pub const MASTER_GRIP_RATIO: u32 = MASTER_FIRST + 5;
pub const MASTER_GRIP_ATTACK: u32 = MASTER_FIRST + 6;
pub const MASTER_GRIP_RELEASE: u32 = MASTER_FIRST + 7;
pub const MASTER_PUNCH_RATIO: u32 = MASTER_FIRST + 8;
pub const MASTER_PUNCH_ATTACK: u32 = MASTER_FIRST + 9;
pub const MASTER_PUNCH_RELEASE: u32 = MASTER_FIRST + 10;
pub const MASTER_TUBE_TIME: u32 = MASTER_FIRST + 11;
pub const MASTER_LOOKAHEAD_MS: u32 = MASTER_FIRST + 12;

/// The master's own section: the bus compressor and the safety limiter's
/// lookahead.
///
/// **Each voicing keeps its own controls.** Grip's ratio, attack and release
/// are separate fields from Punch's, and Tube has one TIME instead of either
/// pair, so switching voicing swaps a unit in the rack rather than
/// reinterpreting three knobs -- and no control ever shows a range its
/// voicing does not have (Adam, 2026-09-23: *"the face is per voicing"*).
/// Threshold, makeup and mix mean the same thing under all three and are
/// shared.
///
/// A stepped field holds a **position**, as a band's frequency does: what it
/// is worth belongs to the law table.
///
/// Every track's strip will carry one, and only the master's runs; the
/// default is out, with the lookahead at 0, which is the master exactly as
/// it was before this existed.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct MasterSectionParams {
    pub comp_in: bool,
    pub voicing: BusCompVoicing,
    pub threshold_db: f32,
    pub makeup_db: f32,
    /// Parallel balance, as the strip's: 0 is the dry mix exactly.
    pub mix: f32,
    pub grip_ratio: u8,
    pub grip_attack: u8,
    pub grip_release: u8,
    pub punch_ratio: u8,
    pub punch_attack: u8,
    pub punch_release: u8,
    pub tube_time: u8,
    /// The safety limiter's lookahead, `0..=MAX_LOOKAHEAD_MS`. Not the
    /// compressor's: it is kept here because it is the other control the
    /// master's own face carries, and because the master's strip is the one
    /// saved thing that belongs to the master alone.
    pub lookahead_ms: f32,
}

impl Default for MasterSectionParams {
    fn default() -> Self {
        let position = |id: u32| {
            MasterSectionParams::descriptor(id).map_or(0, |descriptor| descriptor.default as u8)
        };
        Self {
            comp_in: false,
            voicing: BusCompVoicing::Grip,
            threshold_db: MASTER_DESCRIPTORS[2].default,
            makeup_db: MASTER_DESCRIPTORS[3].default,
            mix: MASTER_DESCRIPTORS[4].default,
            grip_ratio: position(MASTER_GRIP_RATIO),
            grip_attack: position(MASTER_GRIP_ATTACK),
            grip_release: position(MASTER_GRIP_RELEASE),
            punch_ratio: position(MASTER_PUNCH_RATIO),
            punch_attack: position(MASTER_PUNCH_ATTACK),
            punch_release: position(MASTER_PUNCH_RELEASE),
            tube_time: position(MASTER_TUBE_TIME),
            lookahead_ms: 0.0,
        }
    }
}

impl MasterSectionParams {
    /// Every parameter's range, curve and default, in id order.
    pub fn descriptors() -> &'static [ParamDescriptor] {
        &MASTER_DESCRIPTORS
    }

    pub fn descriptor(id: u32) -> Option<&'static ParamDescriptor> {
        MASTER_DESCRIPTORS.iter().find(|descriptor| descriptor.id == id)
    }

    /// Whether this is the section nobody has touched, which is what a song
    /// does not need to write down.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn get(&self, id: u32) -> Option<f32> {
        Some(match id {
            MASTER_COMP_IN => switch_to_f32(self.comp_in),
            MASTER_COMP_VOICING => self.voicing.to_index() as f32,
            MASTER_COMP_THRESHOLD_DB => self.threshold_db,
            MASTER_COMP_MAKEUP_DB => self.makeup_db,
            MASTER_COMP_MIX => self.mix,
            MASTER_GRIP_RATIO => f32::from(self.grip_ratio),
            MASTER_GRIP_ATTACK => f32::from(self.grip_attack),
            MASTER_GRIP_RELEASE => f32::from(self.grip_release),
            MASTER_PUNCH_RATIO => f32::from(self.punch_ratio),
            MASTER_PUNCH_ATTACK => f32::from(self.punch_attack),
            MASTER_PUNCH_RELEASE => f32::from(self.punch_release),
            MASTER_TUBE_TIME => f32::from(self.tube_time),
            MASTER_LOOKAHEAD_MS => self.lookahead_ms,
            _ => return None,
        })
    }

    /// Store `value` under `id`, clamped to its descriptor, and report
    /// whether the id is one of this section's. The same contract as
    /// [`StripParams::set`]: a non-finite value is the default.
    pub fn set(&mut self, id: u32, value: f32) -> bool {
        let Some(descriptor) = Self::descriptor(id) else {
            return false;
        };
        let value = if value.is_finite() {
            value.clamp(descriptor.min, descriptor.max)
        } else {
            descriptor.default
        };
        let position = value.round().max(0.0) as u8;
        match id {
            MASTER_COMP_IN => self.comp_in = switch_from_f32(value),
            MASTER_COMP_VOICING => self.voicing = BusCompVoicing::from_index(value.round() as i32),
            MASTER_COMP_THRESHOLD_DB => self.threshold_db = value,
            MASTER_COMP_MAKEUP_DB => self.makeup_db = value,
            MASTER_COMP_MIX => self.mix = value,
            MASTER_GRIP_RATIO => self.grip_ratio = position,
            MASTER_GRIP_ATTACK => self.grip_attack = position,
            MASTER_GRIP_RELEASE => self.grip_release = position,
            MASTER_PUNCH_RATIO => self.punch_ratio = position,
            MASTER_PUNCH_ATTACK => self.punch_attack = position,
            MASTER_PUNCH_RELEASE => self.punch_release = position,
            MASTER_TUBE_TIME => self.tube_time = position,
            MASTER_LOOKAHEAD_MS => self.lookahead_ms = value,
            _ => return false,
        }
        true
    }
}

/// A stepped descriptor over `positions` switch positions.
const fn stepped(id: u32, name: &'static str, positions: u8, default: u8) -> ParamDescriptor {
    ParamDescriptor {
        id,
        name,
        unit: "",
        min: 0.0,
        max: (positions - 1) as f32,
        curve: ParamCurve::Stepped(positions as u16),
        default: default as f32,
    }
}

/// The master section's table, contiguous with the strip's.
///
/// **Ids are frozen from the day they land**: append, never renumber
/// (`crate::ParamDescriptor`'s rule). The defaults are the settings a mix
/// engineer reaches for first -- Grip at 4:1, 10 ms, 0.3 s; Punch at 4:1,
/// 3 ms, 0.2 s; Tube at position 2 -- so switching the section in does
/// something musical before anything is turned.
static MASTER_DESCRIPTORS: [ParamDescriptor; 13] = [
    stepped(MASTER_COMP_IN, "Comp In", 2, 0),
    stepped(MASTER_COMP_VOICING, "Voicing", 3, 0),
    ParamDescriptor {
        id: MASTER_COMP_THRESHOLD_DB,
        name: "Thresh",
        unit: "dB",
        min: -40.0,
        max: 0.0,
        curve: ParamCurve::Linear,
        default: -16.0,
    },
    ParamDescriptor {
        id: MASTER_COMP_MAKEUP_DB,
        name: "Makeup",
        unit: "dB",
        min: 0.0,
        max: 18.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    ParamDescriptor {
        id: MASTER_COMP_MIX,
        name: "Mix",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 1.0,
    },
    stepped(MASTER_GRIP_RATIO, "Ratio", GRIP_RATIO_POSITIONS, 1),
    stepped(MASTER_GRIP_ATTACK, "Attack", GRIP_ATTACK_POSITIONS, 4),
    stepped(MASTER_GRIP_RELEASE, "Release", GRIP_RELEASE_POSITIONS, 1),
    stepped(MASTER_PUNCH_RATIO, "Ratio", PUNCH_RATIO_POSITIONS, 3),
    stepped(MASTER_PUNCH_ATTACK, "Attack", PUNCH_ATTACK_POSITIONS, 4),
    stepped(MASTER_PUNCH_RELEASE, "Release", PUNCH_RELEASE_POSITIONS, 2),
    stepped(MASTER_TUBE_TIME, "Time", TUBE_TIME_POSITIONS, 1),
    ParamDescriptor {
        id: MASTER_LOOKAHEAD_MS,
        name: "Lookahead",
        unit: "ms",
        min: 0.0,
        max: MAX_LOOKAHEAD_MS,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
];

#[cfg(test)]
mod master_tests {
    use super::*;

    #[test]
    fn every_master_descriptor_default_is_the_default_section() {
        let params = MasterSectionParams::default();
        for descriptor in MasterSectionParams::descriptors() {
            let held = params
                .get(descriptor.id)
                .unwrap_or_else(|| panic!("no value behind {}", descriptor.name));
            assert_eq!(held, descriptor.default, "{} ({})", descriptor.name, descriptor.id);
        }
        assert!(!params.comp_in, "the section arrives out");
        assert_eq!(params.lookahead_ms, 0.0, "the limiter arrives with no lookahead");
        assert!(params.is_default());
    }

    /// The two tables are one id space: the master's continues where the
    /// strip's stops, in table order, which is what `StripSpec` finds a row
    /// by.
    #[test]
    fn the_master_ids_continue_the_strips_with_no_holes() {
        let strip_end = STRIP_FIRST + StripParams::descriptors().len() as u32;
        assert_eq!(MASTER_FIRST, strip_end);
        for (offset, descriptor) in MasterSectionParams::descriptors().iter().enumerate() {
            assert_eq!(descriptor.id, MASTER_FIRST + offset as u32, "{}", descriptor.name);
            assert!(StripParams::descriptor(descriptor.id).is_none());
        }
    }

    /// The ids are frozen once they land: a renumber would move every saved
    /// setting a face or a later lane addresses.
    #[test]
    fn the_master_ids_are_frozen() {
        assert_eq!(MASTER_COMP_IN, 44);
        assert_eq!(MASTER_TUBE_TIME, 55);
        assert_eq!(MASTER_LOOKAHEAD_MS, 56);
    }

    #[test]
    fn every_master_parameter_round_trips_and_strangers_are_refused() {
        let mut params = MasterSectionParams::default();
        for descriptor in MasterSectionParams::descriptors() {
            assert!(params.set(descriptor.id, descriptor.max), "{}", descriptor.name);
            assert_eq!(params.get(descriptor.id), Some(descriptor.max), "{}", descriptor.name);
        }
        assert!(!params.set(STRIP_COMP_MAKEUP_DB, 1.0));
        assert!(!params.set(MASTER_LOOKAHEAD_MS + 1, 1.0));
        assert!(params.get(MASTER_LOOKAHEAD_MS + 1).is_none());
    }

    /// A stepped control holds exactly its switch's positions: one past the
    /// end clamps onto the last, and a NaN is the default.
    #[test]
    fn a_position_past_the_switch_clamps_and_a_nan_is_the_default() {
        let mut params = MasterSectionParams::default();
        params.set(MASTER_PUNCH_ATTACK, 40.0);
        assert_eq!(params.punch_attack, PUNCH_ATTACK_POSITIONS - 1);
        params.set(MASTER_GRIP_RELEASE, -3.0);
        assert_eq!(params.grip_release, 0);
        params.set(MASTER_TUBE_TIME, f32::NAN);
        assert_eq!(params.tube_time, 1);
        params.set(MASTER_COMP_VOICING, 9.0);
        assert_eq!(params.voicing, BusCompVoicing::Tube);
        params.set(MASTER_LOOKAHEAD_MS, 60.0);
        assert_eq!(params.lookahead_ms, MAX_LOOKAHEAD_MS);
    }

    #[test]
    fn a_voicing_index_round_trips() {
        for voicing in BusCompVoicing::ALL {
            assert_eq!(BusCompVoicing::from_index(voicing.to_index()), voicing);
        }
    }

    /// Serde-defaulted field by field, so a section written by an older
    /// build that lacked a field opens with that field at its default.
    #[test]
    fn a_section_missing_fields_loads_with_their_defaults() {
        let loaded: MasterSectionParams =
            toml::from_str("comp_in = true\nvoicing = \"Tube\"\n").expect("partial section");
        assert!(loaded.comp_in);
        assert_eq!(loaded.voicing, BusCompVoicing::Tube);
        assert_eq!(loaded.tube_time, MasterSectionParams::default().tube_time);
        let round: MasterSectionParams =
            toml::from_str(&toml::to_string(&loaded).unwrap()).unwrap();
        assert_eq!(round, loaded);
    }
}

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

    /// The id space is contiguous from `STRIP_FIRST`, **in table order**,
    /// and the gap below it is the one the module header describes.
    ///
    /// Three halves matter. A hole in the middle would be a parameter
    /// nothing can reach. A table that started at 0 would collide with the
    /// fader and the pan. And the *order of the rows* is load-bearing in a
    /// way nothing else in this file makes visible: `strip.slint`'s
    /// `StripSpec.spec(id)` reads `params[id - first]`, so the descriptor
    /// behind a knob is found by arithmetic on its position, not by search.
    /// Swap two rows to read better and every knob from there down gets its
    /// neighbour's range, name and unit, with no Rust change to blame.
    ///
    /// This deliberately does not sort: an earlier version did, and a sorted
    /// copy of the ids cannot see the one property the markup depends on.
    #[test]
    fn the_id_space_starts_after_the_faders_and_has_no_holes() {
        assert_eq!(STRIP_FIRST, 16);
        assert!(StripParams::descriptor(crate::modulation::STRIP_PARAM_VOLUME).is_none());
        assert!(StripParams::descriptor(crate::modulation::STRIP_PARAM_PAN).is_none());
        for (offset, descriptor) in StripParams::descriptors().iter().enumerate() {
            assert_eq!(
                descriptor.id,
                STRIP_FIRST + offset as u32,
                "{} sits at offset {offset}, where the face will look for id {}",
                descriptor.name,
                STRIP_FIRST + offset as u32
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

    /// Every field of a strip as TOML, which is how a project stores one.
    ///
    /// Built by serializing the default rather than written out by hand: a
    /// hand-written manifest is a second copy of the struct's field list, and
    /// the first version of the test below was exactly that and failed on a
    /// field it had not heard of.
    fn default_manifest() -> String {
        toml::to_string(&StripParams::default()).expect("a default strip does not serialize")
    }

    /// A strip written on 2026-09-11, the one day `StripBand` carried
    /// `frequency_hz` instead of `position`, opens with every band on the
    /// middle of its *own* set.
    ///
    /// This is the case the wire type exists for, and it is the case that was
    /// wrong: `#[serde(default = "fn")]` gets no array index, so the field
    /// default had to name one number for four bands. It named `2`, which is
    /// right for the two five-position outer bands and one step low for the
    /// two seven-position mids -- so a band 1 that meant 3 kHz under `MOO_EQ`
    /// opened at 2 kHz, while the face and the descriptor's
    /// double-click-to-default both said 3.
    ///
    /// `frequency_hz` is put back rather than merely dropped, because
    /// ignoring it is the other half of what makes such a file open at all.
    #[test]
    fn a_band_saved_before_positions_existed_opens_on_its_own_middle() {
        let manifest = default_manifest().replace("position = ", "frequency_hz = ");
        assert!(
            !manifest.contains("position ="),
            "the manifest still states a position, so this proves nothing"
        );

        let loaded: StripParams =
            toml::from_str(&manifest).expect("a 2026-09-11 strip no longer loads at all");
        let positions: Vec<u8> = loaded.bands.iter().map(|band| band.position).collect();
        assert_eq!(
            positions,
            DEFAULT_POSITIONS.to_vec(),
            "each band should open on the middle of its own set, not on one number for all four"
        );
        assert_eq!(
            loaded.bands,
            StripParams::default().bands,
            "a band stating only what 2026-09-11 could state should be the default band"
        );
    }

    /// The wire type is not a general loosening. `position` is optional
    /// because one day's files predate it; its three siblings are not, and a
    /// band missing one of them still fails to load rather than inventing it.
    #[test]
    fn a_band_missing_anything_but_its_position_still_refuses_to_load() {
        for missing in ["kind = ", "gain_db = ", "q = "] {
            let manifest = default_manifest();
            let at = manifest
                .find("[[bands]]")
                .expect("a default strip no longer writes its bands as a table array");
            let (head, bands) = manifest.split_at(at);
            let cut: String = bands
                .lines()
                .filter(|line| !line.trim_start().starts_with(missing))
                .collect::<Vec<_>>()
                .join("\n");
            assert_ne!(cut, bands, "`{missing}` was not in the manifest to remove");
            assert!(
                toml::from_str::<StripParams>(&format!("{head}{cut}")).is_err(),
                "a band with no `{missing}` loaded anyway"
            );
        }
    }
}
