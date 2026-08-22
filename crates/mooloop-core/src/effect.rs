//! Effect types: the per-slot state persisted in a project, the parameter
//! sets the DSP nodes consume, and the descriptor tables that give every
//! parameter one authoritative range and normalization.
//!
//! Effects are chainable units that run after a channel's generator; see
//! `docs/EFFECTS_PLAN.md` for the plumbing and `docs/MODULATION_PLAN.md` for
//! why descriptors exist and what the parameter model is going to become.

/// Effect kind. The tag for [`EffectParams`], mirroring how `ChannelSource`
/// tags its per-source state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    Filter,
    Drive,
    Bitcrush,
}

impl EffectKind {
    /// Every kind, in the order the UI offers them when adding an effect.
    pub const ALL: [EffectKind; 3] = [EffectKind::Filter, EffectKind::Drive, EffectKind::Bitcrush];

    /// Display name for device headers and the add-effect picker.
    pub fn label(self) -> &'static str {
        match self {
            Self::Filter => "Filter",
            Self::Drive => "Drive",
            Self::Bitcrush => "Bitcrush",
        }
    }

    /// This kind's parameter table. Indexed by position, not by `id` — read
    /// [`ParamDescriptor::id`] for the value that goes on the wire.
    pub fn descriptors(self) -> &'static [ParamDescriptor] {
        match self {
            Self::Filter => &FILTER_DESCRIPTORS,
            Self::Drive => &DRIVE_DESCRIPTORS,
            Self::Bitcrush => &BITCRUSH_DESCRIPTORS,
        }
    }

    /// Look one parameter up by its wire id.
    pub fn descriptor(self, id: u32) -> Option<&'static ParamDescriptor> {
        self.descriptors().iter().find(|d| d.id == id)
    }

    /// Default parameter set for a freshly added effect of this kind.
    pub fn default_params(self) -> EffectParams {
        match self {
            Self::Filter => EffectParams::Filter(FilterParams::default()),
            Self::Drive => EffectParams::Drive(DriveParams::default()),
            Self::Bitcrush => EffectParams::Bitcrush(BitcrushParams::default()),
        }
    }
}

/// How a parameter's normalized 0..1 knob position maps onto its natural
/// range. Events on the wire always carry natural units; this is the mapping
/// the non-realtime side applies before sending them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamCurve {
    /// Even across the range. Mixes, depths, bipolar tilts.
    Linear,
    /// Even in ratio, so a knob feels right on frequencies and gains.
    /// Requires `min > 0`.
    Exponential,
    /// `n` discrete positions mapped across the range: mode selectors.
    Stepped(u8),
}

/// One parameter's identity, range, and mapping. The single source of truth:
/// a range written a second time anywhere else is a bug.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamDescriptor {
    /// Stable per-kind wire id, carried by `Event::ParamValue`. Automation
    /// will persist these, so never renumber a shipped id — append instead.
    pub id: u32,
    pub name: &'static str,
    /// Suffix for value readouts. Empty when the value is unitless.
    pub unit: &'static str,
    pub min: f32,
    pub max: f32,
    pub curve: ParamCurve,
    pub default: f32,
}

impl ParamDescriptor {
    /// Natural units -> 0..1 knob position.
    pub fn to_normalized(&self, natural: f32) -> f32 {
        let clamped = natural.clamp(self.min, self.max);
        let span = self.max - self.min;
        match self.curve {
            ParamCurve::Linear => {
                if span == 0.0 {
                    0.0
                } else {
                    (clamped - self.min) / span
                }
            }
            ParamCurve::Exponential => {
                let ratio = self.max / self.min;
                if ratio <= 1.0 {
                    0.0
                } else {
                    (clamped / self.min).ln() / ratio.ln()
                }
            }
            ParamCurve::Stepped(steps) => {
                if steps <= 1 || span == 0.0 {
                    0.0
                } else {
                    (clamped - self.min) / span
                }
            }
        }
    }

    /// 0..1 knob position -> natural units.
    pub fn from_normalized(&self, norm: f32) -> f32 {
        let t = norm.clamp(0.0, 1.0);
        match self.curve {
            ParamCurve::Linear => self.min + (self.max - self.min) * t,
            ParamCurve::Exponential => {
                let ratio = self.max / self.min;
                if ratio <= 0.0 {
                    self.min
                } else {
                    self.min * ratio.powf(t)
                }
            }
            ParamCurve::Stepped(steps) => {
                if steps <= 1 {
                    self.min
                } else {
                    let last = f32::from(steps - 1);
                    let index = (t * last).round();
                    self.min + (self.max - self.min) * (index / last)
                }
            }
        }
    }

    /// Clamp a natural value into range, snapping it when stepped.
    pub fn clamp_natural(&self, natural: f32) -> f32 {
        match self.curve {
            ParamCurve::Stepped(_) => self.from_normalized(self.to_normalized(natural)),
            _ => natural.clamp(self.min, self.max),
        }
    }
}

// --- Filter ----------------------------------------------------------------

/// `Event::ParamValue` ids for [`FilterParams`].
pub const FILTER_PARAM_CUTOFF_HZ: u32 = 0;
pub const FILTER_PARAM_RESONANCE: u32 = 1;
pub const FILTER_PARAM_MODE: u32 = 2;

static FILTER_DESCRIPTORS: [ParamDescriptor; 3] = [
    ParamDescriptor {
        id: FILTER_PARAM_CUTOFF_HZ,
        name: "Cutoff",
        unit: "Hz",
        min: 20.0,
        max: 20_000.0,
        curve: ParamCurve::Exponential,
        default: 8_000.0,
    },
    ParamDescriptor {
        id: FILTER_PARAM_RESONANCE,
        name: "Reso",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    ParamDescriptor {
        id: FILTER_PARAM_MODE,
        name: "Mode",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Stepped(2),
        default: 0.0,
    },
];

/// Filter mode: low-pass attenuates above the cutoff, high-pass below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterMode {
    #[default]
    LowPass,
    HighPass,
}

/// Parameters for the filter effect (`FilterEffect` in `mooloop-dsp`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FilterParams {
    /// Cutoff frequency in Hz, clamped by the DSP to [20, sample_rate * 0.45].
    pub cutoff_hz: f32,
    /// Resonance in `[0, 1]`, approaching self-oscillation at the top.
    pub resonance: f32,
    pub mode: FilterMode,
}

impl Default for FilterParams {
    fn default() -> Self {
        Self {
            cutoff_hz: 8_000.0,
            resonance: 0.0,
            mode: FilterMode::default(),
        }
    }
}

// --- Drive -----------------------------------------------------------------

/// `Event::ParamValue` ids for [`DriveParams`].
pub const DRIVE_PARAM_DRIVE: u32 = 0;
pub const DRIVE_PARAM_CURVE: u32 = 1;
pub const DRIVE_PARAM_TONE: u32 = 2;
pub const DRIVE_PARAM_MIX: u32 = 3;
pub const DRIVE_PARAM_OUTPUT: u32 = 4;

static DRIVE_DESCRIPTORS: [ParamDescriptor; 5] = [
    ParamDescriptor {
        id: DRIVE_PARAM_DRIVE,
        name: "Drive",
        unit: "x",
        min: 1.0,
        max: 64.0,
        curve: ParamCurve::Exponential,
        default: 2.0,
    },
    ParamDescriptor {
        id: DRIVE_PARAM_CURVE,
        name: "Curve",
        unit: "",
        min: 0.0,
        max: 3.0,
        curve: ParamCurve::Stepped(4),
        default: 0.0,
    },
    ParamDescriptor {
        id: DRIVE_PARAM_TONE,
        name: "Tone",
        unit: "",
        min: -1.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    ParamDescriptor {
        id: DRIVE_PARAM_MIX,
        name: "Mix",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 1.0,
    },
    ParamDescriptor {
        id: DRIVE_PARAM_OUTPUT,
        name: "Out",
        unit: "x",
        min: 0.0,
        max: 2.0,
        curve: ParamCurve::Linear,
        default: 1.0,
    },
];

/// Shaping curve. `Soft` and `Tape` compress gently, `Hard` clips, and `Fold`
/// wraps past full scale into the inharmonic territory this instrument is
/// actually for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriveCurve {
    #[default]
    Soft,
    Hard,
    Fold,
    Tape,
}

impl DriveCurve {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Hard,
            2 => Self::Fold,
            3 => Self::Tape,
            _ => Self::Soft,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Soft => 0,
            Self::Hard => 1,
            Self::Fold => 2,
            Self::Tape => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Soft => "SOFT",
            Self::Hard => "HARD",
            Self::Fold => "FOLD",
            Self::Tape => "TAPE",
        }
    }
}

/// Parameters for the drive/saturation effect (`DriveEffect` in `mooloop-dsp`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DriveParams {
    /// Linear input gain into the shaper.
    pub drive: f32,
    pub curve: DriveCurve,
    /// Post-shaper spectral tilt in `[-1, 1]`: negative darkens, positive
    /// brightens, zero is flat.
    pub tone: f32,
    /// Dry/wet blend in `[0, 1]`.
    pub mix: f32,
    /// Linear output trim, applied after the blend.
    pub output: f32,
}

impl Default for DriveParams {
    fn default() -> Self {
        Self {
            drive: 2.0,
            curve: DriveCurve::default(),
            tone: 0.0,
            mix: 1.0,
            output: 1.0,
        }
    }
}

// --- Bitcrush --------------------------------------------------------------

/// `Event::ParamValue` ids for [`BitcrushParams`].
pub const BITCRUSH_PARAM_BITS: u32 = 0;
pub const BITCRUSH_PARAM_DOWNSAMPLE: u32 = 1;
pub const BITCRUSH_PARAM_MIX: u32 = 2;

static BITCRUSH_DESCRIPTORS: [ParamDescriptor; 3] = [
    ParamDescriptor {
        id: BITCRUSH_PARAM_BITS,
        name: "Bits",
        unit: "bit",
        min: 1.0,
        max: 16.0,
        curve: ParamCurve::Linear,
        default: 16.0,
    },
    ParamDescriptor {
        id: BITCRUSH_PARAM_DOWNSAMPLE,
        name: "Rate",
        unit: "x",
        min: 1.0,
        max: 64.0,
        curve: ParamCurve::Exponential,
        default: 1.0,
    },
    ParamDescriptor {
        id: BITCRUSH_PARAM_MIX,
        name: "Mix",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 1.0,
    },
];

/// Parameters for the bitcrush effect (`BitcrushEffect` in `mooloop-dsp`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BitcrushParams {
    /// Quantization depth in bits. Fractional values are meaningful — the
    /// step size is continuous, so this can be swept without zippering.
    pub bits: f32,
    /// Sample-and-hold length in input samples. 1.0 holds nothing.
    pub downsample: f32,
    /// Dry/wet blend in `[0, 1]`.
    pub mix: f32,
}

impl Default for BitcrushParams {
    fn default() -> Self {
        Self {
            bits: 16.0,
            downsample: 1.0,
            mix: 1.0,
        }
    }
}

// --- Slot state ------------------------------------------------------------

/// Per-kind parameter set. Tagged with the same serde shape as
/// `ChannelSource` so new kinds join the v1 envelope additively.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "state", rename_all = "snake_case")]
pub enum EffectParams {
    Filter(FilterParams),
    Drive(DriveParams),
    Bitcrush(BitcrushParams),
}

impl EffectParams {
    pub fn kind(&self) -> EffectKind {
        match self {
            Self::Filter(_) => EffectKind::Filter,
            Self::Drive(_) => EffectKind::Drive,
            Self::Bitcrush(_) => EffectKind::Bitcrush,
        }
    }

    pub fn filter(&self) -> Option<&FilterParams> {
        match self {
            Self::Filter(p) => Some(p),
            _ => None,
        }
    }

    pub fn drive(&self) -> Option<&DriveParams> {
        match self {
            Self::Drive(p) => Some(p),
            _ => None,
        }
    }

    pub fn bitcrush(&self) -> Option<&BitcrushParams> {
        match self {
            Self::Bitcrush(p) => Some(p),
            _ => None,
        }
    }

    /// Read one parameter in natural units by wire id. Returns `None` for an
    /// id this kind does not have.
    pub fn get(&self, id: u32) -> Option<f32> {
        match self {
            Self::Filter(p) => match id {
                FILTER_PARAM_CUTOFF_HZ => Some(p.cutoff_hz),
                FILTER_PARAM_RESONANCE => Some(p.resonance),
                FILTER_PARAM_MODE => Some(match p.mode {
                    FilterMode::LowPass => 0.0,
                    FilterMode::HighPass => 1.0,
                }),
                _ => None,
            },
            Self::Drive(p) => match id {
                DRIVE_PARAM_DRIVE => Some(p.drive),
                DRIVE_PARAM_CURVE => Some(p.curve.to_index() as f32),
                DRIVE_PARAM_TONE => Some(p.tone),
                DRIVE_PARAM_MIX => Some(p.mix),
                DRIVE_PARAM_OUTPUT => Some(p.output),
                _ => None,
            },
            Self::Bitcrush(p) => match id {
                BITCRUSH_PARAM_BITS => Some(p.bits),
                BITCRUSH_PARAM_DOWNSAMPLE => Some(p.downsample),
                BITCRUSH_PARAM_MIX => Some(p.mix),
                _ => None,
            },
        }
    }

    /// Write one parameter in natural units by wire id, clamped through its
    /// descriptor. Returns the stored value, or `None` for an unknown id.
    pub fn set(&mut self, id: u32, value: f32) -> Option<f32> {
        let descriptor = self.kind().descriptor(id)?;
        let value = descriptor.clamp_natural(value);
        match self {
            Self::Filter(p) => match id {
                FILTER_PARAM_CUTOFF_HZ => p.cutoff_hz = value,
                FILTER_PARAM_RESONANCE => p.resonance = value,
                FILTER_PARAM_MODE => {
                    p.mode = if value >= 0.5 {
                        FilterMode::HighPass
                    } else {
                        FilterMode::LowPass
                    }
                }
                _ => return None,
            },
            Self::Drive(p) => match id {
                DRIVE_PARAM_DRIVE => p.drive = value,
                DRIVE_PARAM_CURVE => p.curve = DriveCurve::from_index(value.round() as i32),
                DRIVE_PARAM_TONE => p.tone = value,
                DRIVE_PARAM_MIX => p.mix = value,
                DRIVE_PARAM_OUTPUT => p.output = value,
                _ => return None,
            },
            Self::Bitcrush(p) => match id {
                BITCRUSH_PARAM_BITS => p.bits = value,
                BITCRUSH_PARAM_DOWNSAMPLE => p.downsample = value,
                BITCRUSH_PARAM_MIX => p.mix = value,
                _ => return None,
            },
        }
        Some(value)
    }
}

/// Songs written before `EffectParams` was tagged stored a bare `FilterParams`
/// table, because `Filter` was the only kind. Accept both shapes on load.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum EffectParamsCompat {
    Tagged(EffectParams),
    LegacyFilter(FilterParams),
}

fn deserialize_effect_params<'de, D>(deserializer: D) -> Result<EffectParams, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    Ok(match EffectParamsCompat::deserialize(deserializer)? {
        EffectParamsCompat::Tagged(params) => params,
        EffectParamsCompat::LegacyFilter(params) => EffectParams::Filter(params),
    })
}

/// Persisted state of one slot in a channel's effect chain.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EffectSlotState {
    #[serde(deserialize_with = "deserialize_effect_params")]
    pub params: EffectParams,
    pub bypassed: bool,
}

impl EffectSlotState {
    pub fn new(params: EffectParams) -> Self {
        Self {
            params,
            bypassed: false,
        }
    }

    /// A slot holding this kind's defaults.
    pub fn of_kind(kind: EffectKind) -> Self {
        Self::new(kind.default_params())
    }

    pub fn filter(params: FilterParams) -> Self {
        Self::new(EffectParams::Filter(params))
    }

    pub fn drive(params: DriveParams) -> Self {
        Self::new(EffectParams::Drive(params))
    }

    pub fn bitcrush(params: BitcrushParams) -> Self {
        Self::new(EffectParams::Bitcrush(params))
    }

    pub fn kind(&self) -> EffectKind {
        self.params.kind()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_round_trips_across_every_descriptor() {
        for kind in EffectKind::ALL {
            for descriptor in kind.descriptors() {
                for step in 0..=10 {
                    let norm = step as f32 / 10.0;
                    let natural = descriptor.from_normalized(norm);
                    assert!(
                        natural >= descriptor.min - 1e-3 && natural <= descriptor.max + 1e-3,
                        "{}/{} produced {natural} outside [{}, {}]",
                        kind.label(),
                        descriptor.name,
                        descriptor.min,
                        descriptor.max
                    );
                    let back = descriptor.to_normalized(natural);
                    let tolerance = match descriptor.curve {
                        // Stepped params snap, so only the snapped positions
                        // round-trip exactly.
                        ParamCurve::Stepped(_) => 0.5,
                        _ => 1e-3,
                    };
                    assert!(
                        (back - norm).abs() <= tolerance,
                        "{}/{} round-tripped {norm} to {back}",
                        kind.label(),
                        descriptor.name
                    );
                }
            }
        }
    }

    #[test]
    fn every_descriptor_default_is_in_range() {
        for kind in EffectKind::ALL {
            for descriptor in kind.descriptors() {
                assert!(
                    descriptor.default >= descriptor.min && descriptor.default <= descriptor.max,
                    "{}/{} default {} outside range",
                    kind.label(),
                    descriptor.name,
                    descriptor.default
                );
            }
        }
    }

    #[test]
    fn descriptor_defaults_match_the_params_defaults() {
        for kind in EffectKind::ALL {
            let params = kind.default_params();
            for descriptor in kind.descriptors() {
                let actual = params
                    .get(descriptor.id)
                    .unwrap_or_else(|| panic!("{}/{} has no getter", kind.label(), descriptor.name));
                assert!(
                    (actual - descriptor.default).abs() <= 1e-4,
                    "{}/{}: descriptor says {}, params say {actual}",
                    kind.label(),
                    descriptor.name,
                    descriptor.default
                );
            }
        }
    }

    #[test]
    fn exponential_cutoff_matches_the_uis_perceptual_mapping() {
        // The filter face renders `20 * 1000^x`; the descriptor must agree or
        // the knob and the audio disagree.
        let cutoff = EffectKind::Filter
            .descriptor(FILTER_PARAM_CUTOFF_HZ)
            .unwrap();
        for step in 0..=10 {
            let norm = step as f32 / 10.0;
            let expected = 20.0 * 1000f32.powf(norm);
            let actual = cutoff.from_normalized(norm);
            assert!(
                (actual / expected - 1.0).abs() < 1e-3,
                "at {norm}: descriptor {actual} vs face {expected}"
            );
        }
    }

    #[test]
    fn set_clamps_through_the_descriptor() {
        let mut params = EffectParams::Filter(FilterParams::default());
        assert_eq!(params.set(FILTER_PARAM_CUTOFF_HZ, 1_000_000.0), Some(20_000.0));
        assert_eq!(params.set(FILTER_PARAM_RESONANCE, -5.0), Some(0.0));
        assert_eq!(params.set(99, 1.0), None);

        let mut drive = EffectParams::Drive(DriveParams::default());
        drive.set(DRIVE_PARAM_CURVE, 2.0);
        assert_eq!(drive.drive().unwrap().curve, DriveCurve::Fold);
    }

    #[test]
    fn legacy_untagged_filter_params_still_deserialize() {
        // The shape songs used while `Filter` was the only effect kind.
        let legacy = "\
kind = \"filter\"
bypassed = false

[params]
cutoff_hz = 1250.0
resonance = 0.6
mode = \"high_pass\"
";
        let slot: EffectSlotState = toml::from_str(legacy).expect("legacy slot should load");
        assert_eq!(slot.kind(), EffectKind::Filter);
        let filter = slot.params.filter().unwrap();
        assert_eq!(filter.cutoff_hz, 1_250.0);
        assert_eq!(filter.mode, FilterMode::HighPass);
        assert!(!slot.bypassed);
    }

    #[test]
    fn tagged_params_round_trip_for_every_kind() {
        for kind in EffectKind::ALL {
            let slot = EffectSlotState::of_kind(kind);
            let text = toml::to_string(&slot).unwrap();
            let back: EffectSlotState = toml::from_str(&text).unwrap();
            assert_eq!(slot, back, "{} did not round-trip:\n{text}", kind.label());
        }
    }
}
