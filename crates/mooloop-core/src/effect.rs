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
    Eq,
    Filter,
    Drive,
    Bitcrush,
    Delay,
    Reverb,
    Gate,
    Compressor,
    Limiter,
}

impl EffectKind {
    /// Every kind, in the order the UI offers them when adding an effect.
    pub const ALL: [EffectKind; 9] = [
        EffectKind::Eq,
        EffectKind::Filter,
        EffectKind::Drive,
        EffectKind::Bitcrush,
        EffectKind::Delay,
        EffectKind::Reverb,
        EffectKind::Gate,
        EffectKind::Compressor,
        EffectKind::Limiter,
    ];

    /// Display name for device headers and the add-effect picker.
    pub fn label(self) -> &'static str {
        match self {
            Self::Eq => "EQ",
            Self::Filter => "Filter",
            Self::Drive => "Drive",
            Self::Bitcrush => "Bitcrush",
            Self::Delay => "Delay",
            Self::Reverb => "Reverb",
            Self::Gate => "Gate",
            Self::Compressor => "Comp",
            Self::Limiter => "Limiter",
        }
    }

    /// This kind's parameter table. Indexed by position, not by `id` — read
    /// [`ParamDescriptor::id`] for the value that goes on the wire.
    pub fn descriptors(self) -> &'static [ParamDescriptor] {
        match self {
            Self::Eq => &EQ_DESCRIPTORS,
            Self::Filter => &FILTER_DESCRIPTORS,
            Self::Drive => &DRIVE_DESCRIPTORS,
            Self::Bitcrush => &BITCRUSH_DESCRIPTORS,
            Self::Delay => &DELAY_DESCRIPTORS,
            Self::Reverb => &REVERB_DESCRIPTORS,
            Self::Gate => &GATE_DESCRIPTORS,
            Self::Compressor => &COMPRESSOR_DESCRIPTORS,
            Self::Limiter => &LIMITER_DESCRIPTORS,
        }
    }

    /// Look one parameter up by its wire id.
    pub fn descriptor(self, id: u32) -> Option<&'static ParamDescriptor> {
        self.descriptors().iter().find(|d| d.id == id)
    }

    /// Default parameter set for a freshly added effect of this kind.
    pub fn default_params(self) -> EffectParams {
        match self {
            Self::Eq => EffectParams::Eq(EqParams::default()),
            Self::Filter => EffectParams::Filter(FilterParams::default()),
            Self::Drive => EffectParams::Drive(DriveParams::default()),
            Self::Bitcrush => EffectParams::Bitcrush(BitcrushParams::default()),
            Self::Delay => EffectParams::Delay(DelayParams::default()),
            Self::Reverb => EffectParams::Reverb(ReverbParams::default()),
            Self::Gate => EffectParams::Gate(GateParams::default()),
            Self::Compressor => EffectParams::Compressor(CompressorParams::default()),
            Self::Limiter => EffectParams::Limiter(LimiterParams::default()),
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

// --- Equalizer -------------------------------------------------------------

/// The EQ has seven musical bands. High- and low-pass filters are separate
/// from this count, matching the way engineers usually think about a channel
/// EQ rather than spending a bell band on cleanup.
pub const EQ_MAX_BANDS: usize = 7;

/// `Event::ParamValue` ids for [`EqParams`]. These operate on the selected
/// EQ target, so the UI needs one stable control set rather than a dump of
/// seven duplicated controls.
pub const EQ_PARAM_TARGET: u32 = 0;
pub const EQ_PARAM_ENABLED: u32 = 1;
pub const EQ_PARAM_FREQUENCY_HZ: u32 = 2;
pub const EQ_PARAM_GAIN_DB: u32 = 3;
pub const EQ_PARAM_Q: u32 = 4;
/// Bell bands: 0 constant-Q, 1 proportional-Q. Pass filters: slope index.
pub const EQ_PARAM_CHARACTER: u32 = 5;

static EQ_DESCRIPTORS: [ParamDescriptor; 6] = [
    ParamDescriptor {
        id: EQ_PARAM_TARGET,
        name: "Band",
        unit: "",
        min: 0.0,
        max: 8.0,
        curve: ParamCurve::Stepped(9),
        default: 1.0,
    },
    ParamDescriptor {
        id: EQ_PARAM_ENABLED,
        name: "On",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Stepped(2),
        default: 1.0,
    },
    ParamDescriptor {
        id: EQ_PARAM_FREQUENCY_HZ,
        name: "Freq",
        unit: "Hz",
        min: 20.0,
        max: 20_000.0,
        curve: ParamCurve::Exponential,
        default: 1_000.0,
    },
    ParamDescriptor {
        id: EQ_PARAM_GAIN_DB,
        name: "Gain",
        unit: "dB",
        min: -18.0,
        max: 18.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    ParamDescriptor {
        id: EQ_PARAM_Q,
        name: "Q",
        unit: "",
        min: 0.15,
        max: 18.0,
        curve: ParamCurve::Exponential,
        default: 0.707,
    },
    ParamDescriptor {
        id: EQ_PARAM_CHARACTER,
        name: "Shape",
        unit: "",
        min: 0.0,
        max: 4.0,
        curve: ParamCurve::Stepped(5),
        default: 0.0,
    },
];

/// A band's response topology. The first and last default bands are shelves;
/// any active interior band is a peaking filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqBandKind {
    #[default]
    Bell,
    LowShelf,
    HighShelf,
}

impl EqBandKind {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::LowShelf,
            2 => Self::HighShelf,
            _ => Self::Bell,
        }
    }
    pub fn to_index(self) -> i32 {
        match self {
            Self::Bell => 0,
            Self::LowShelf => 1,
            Self::HighShelf => 2,
        }
    }
}

/// Bell bandwidth profile. Proportional Q is the familiar API behavior: a
/// larger cut or boost produces a narrower, more assertive curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqQProfile {
    #[default]
    Constant,
    Proportional,
}

impl EqQProfile {
    pub fn from_index(index: i32) -> Self {
        if index >= 1 {
            Self::Proportional
        } else {
            Self::Constant
        }
    }
    pub fn to_index(self) -> i32 {
        match self {
            Self::Constant => 0,
            Self::Proportional => 1,
        }
    }
}

/// One band in the seven-band EQ bank.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EqBand {
    pub enabled: bool,
    pub kind: EqBandKind,
    pub frequency_hz: f32,
    pub gain_db: f32,
    pub q: f32,
    pub q_profile: EqQProfile,
}

impl EqBand {
    pub const fn bell(frequency_hz: f32) -> Self {
        Self {
            enabled: false,
            kind: EqBandKind::Bell,
            frequency_hz,
            gain_db: 0.0,
            q: 0.707,
            q_profile: EqQProfile::Constant,
        }
    }
}

/// Octave slope for dedicated high- and low-pass filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqSlope {
    #[default]
    Db6,
    Db12,
    Db18,
    Db24,
    Db36,
}

impl EqSlope {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Db12,
            2 => Self::Db18,
            3 => Self::Db24,
            4 => Self::Db36,
            _ => Self::Db6,
        }
    }
    pub fn to_index(self) -> i32 {
        match self {
            Self::Db6 => 0,
            Self::Db12 => 1,
            Self::Db18 => 2,
            Self::Db24 => 3,
            Self::Db36 => 4,
        }
    }
    pub fn stages(self) -> usize {
        match self {
            Self::Db6 => 1,
            Self::Db12 => 2,
            Self::Db18 => 3,
            Self::Db24 => 4,
            Self::Db36 => 6,
        }
    }
}

/// Independent low- or high-pass cleanup filter.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EqPassFilter {
    pub enabled: bool,
    pub frequency_hz: f32,
    pub q: f32,
    pub slope: EqSlope,
}

impl EqPassFilter {
    const fn high_pass() -> Self {
        Self {
            enabled: false,
            frequency_hz: 30.0,
            q: 0.707,
            slope: EqSlope::Db12,
        }
    }
    const fn low_pass() -> Self {
        Self {
            enabled: false,
            frequency_hz: 18_000.0,
            q: 0.707,
            slope: EqSlope::Db12,
        }
    }
}

/// Full persisted EQ state. `selected_target` is retained so reopening an EQ
/// returns to the band the user was shaping, without treating it as a seventh
/// automation destination.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EqParams {
    pub bands: [EqBand; EQ_MAX_BANDS],
    pub high_pass: EqPassFilter,
    pub low_pass: EqPassFilter,
    #[serde(default = "default_eq_selected_target")]
    pub selected_target: u8,
    #[serde(default)]
    pub analyzer_enabled: bool,
}

const fn default_eq_selected_target() -> u8 {
    1
}

impl Default for EqParams {
    fn default() -> Self {
        let mut bands = [EqBand::bell(1_000.0); EQ_MAX_BANDS];
        bands[0] = EqBand {
            enabled: true,
            kind: EqBandKind::LowShelf,
            frequency_hz: 120.0,
            gain_db: 0.0,
            q: 0.707,
            q_profile: EqQProfile::Constant,
        };
        bands[1] = EqBand {
            enabled: true,
            ..EqBand::bell(1_000.0)
        };
        bands[2] = EqBand {
            enabled: true,
            kind: EqBandKind::HighShelf,
            frequency_hz: 8_000.0,
            gain_db: 0.0,
            q: 0.707,
            q_profile: EqQProfile::Constant,
        };
        Self {
            bands,
            high_pass: EqPassFilter::high_pass(),
            low_pass: EqPassFilter::low_pass(),
            selected_target: default_eq_selected_target(),
            analyzer_enabled: false,
        }
    }
}

impl EqParams {
    pub const HIGH_PASS_TARGET: usize = EQ_MAX_BANDS;
    pub const LOW_PASS_TARGET: usize = EQ_MAX_BANDS + 1;

    pub fn selected_target(self) -> usize {
        usize::from(self.selected_target).min(Self::LOW_PASS_TARGET)
    }

    pub fn selected_band(self) -> Option<EqBand> {
        self.bands.get(self.selected_target()).copied()
    }

    fn set_selected_target(&mut self, value: f32) {
        self.selected_target = value.round().clamp(0.0, Self::LOW_PASS_TARGET as f32) as u8;
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

// --- Delay -----------------------------------------------------------------

/// `Event::ParamValue` ids for [`DelayParams`].
pub const DELAY_PARAM_TIME_MS: u32 = 0;
pub const DELAY_PARAM_FEEDBACK: u32 = 1;
pub const DELAY_PARAM_MODE: u32 = 2;
pub const DELAY_PARAM_CROSS: u32 = 3;
pub const DELAY_PARAM_TONE: u32 = 4;
pub const DELAY_PARAM_MIX: u32 = 5;

/// Longest delay time, and therefore the ring the effect allocates per slot:
/// two seconds of stereo `f32` is about 768 KiB at 48 kHz.
pub const DELAY_MAX_TIME_MS: f32 = 2_000.0;

static DELAY_DESCRIPTORS: [ParamDescriptor; 6] = [
    ParamDescriptor {
        id: DELAY_PARAM_TIME_MS,
        name: "Time",
        unit: "ms",
        min: 1.0,
        max: DELAY_MAX_TIME_MS,
        curve: ParamCurve::Exponential,
        default: 375.0,
    },
    ParamDescriptor {
        id: DELAY_PARAM_FEEDBACK,
        name: "Fdbk",
        unit: "",
        // Stops short of 1.0: unity feedback with any damping still runs away
        // once the wet path is summed back in.
        min: 0.0,
        max: 0.98,
        curve: ParamCurve::Linear,
        default: 0.35,
    },
    ParamDescriptor {
        id: DELAY_PARAM_MODE,
        name: "Mode",
        unit: "",
        min: 0.0,
        max: 2.0,
        curve: ParamCurve::Stepped(3),
        default: 0.0,
    },
    ParamDescriptor {
        id: DELAY_PARAM_CROSS,
        name: "Cross",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    ParamDescriptor {
        id: DELAY_PARAM_TONE,
        name: "Tone",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.6,
    },
    ParamDescriptor {
        id: DELAY_PARAM_MIX,
        name: "Mix",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.35,
    },
];

/// How the read head responds when the delay time moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelayMode {
    /// Crossfade to the new time. The repeats keep their pitch.
    #[default]
    Digital,
    /// Glide to the new time, so the buffered audio repitches on the way —
    /// the tape-delay behavior.
    Tape,
    /// Read the recent history backwards in windows the length of the delay
    /// time.
    Reverse,
}

impl DelayMode {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Tape,
            2 => Self::Reverse,
            _ => Self::Digital,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Digital => 0,
            Self::Tape => 1,
            Self::Reverse => 2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Digital => "DIGI",
            Self::Tape => "TAPE",
            Self::Reverse => "REV",
        }
    }
}

/// Parameters for the delay effect (`DelayEffect` in `mooloop-dsp`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DelayParams {
    pub time_ms: f32,
    /// Feedback gain in `[0, 0.98]`.
    pub feedback: f32,
    pub mode: DelayMode,
    /// How much of the feedback path crosses to the other channel. At 1.0 the
    /// repeats alternate sides (ping-pong).
    pub cross: f32,
    /// Damping of the feedback path in `[0, 1]`: 0 darkens each repeat
    /// heavily, 1 leaves it open.
    pub tone: f32,
    /// Dry/wet blend in `[0, 1]`.
    pub mix: f32,
}

impl Default for DelayParams {
    fn default() -> Self {
        Self {
            time_ms: 375.0,
            feedback: 0.35,
            mode: DelayMode::default(),
            cross: 0.0,
            tone: 0.6,
            mix: 0.35,
        }
    }
}

// --- Reverb ----------------------------------------------------------------

/// `Event::ParamValue` ids for [`ReverbParams`].
///
/// These describe a generated room rather than a conventional feedback
/// network. The control side turns the complete parameter set into a prepared
/// impulse response; the realtime node only convolves against that response.
pub const REVERB_PARAM_SHAPE: u32 = 0;
pub const REVERB_PARAM_MATERIAL: u32 = 1;
pub const REVERB_PARAM_WIDTH_M: u32 = 2;
pub const REVERB_PARAM_DEPTH_M: u32 = 3;
pub const REVERB_PARAM_HEIGHT_M: u32 = 4;
pub const REVERB_PARAM_DECAY_S: u32 = 5;
pub const REVERB_PARAM_CAPTURE_X: u32 = 6;
pub const REVERB_PARAM_CAPTURE_Y: u32 = 7;

static REVERB_DESCRIPTORS: [ParamDescriptor; 8] = [
    ParamDescriptor {
        id: REVERB_PARAM_SHAPE,
        name: "Shape",
        unit: "",
        min: 0.0,
        max: 2.0,
        curve: ParamCurve::Stepped(3),
        default: 0.0,
    },
    ParamDescriptor {
        id: REVERB_PARAM_MATERIAL,
        name: "Material",
        unit: "",
        min: 0.0,
        max: 3.0,
        curve: ParamCurve::Stepped(4),
        default: 0.0,
    },
    ParamDescriptor {
        id: REVERB_PARAM_WIDTH_M,
        name: "Width",
        unit: "m",
        min: 2.0,
        max: 30.0,
        curve: ParamCurve::Exponential,
        default: 6.0,
    },
    ParamDescriptor {
        id: REVERB_PARAM_DEPTH_M,
        name: "Depth",
        unit: "m",
        min: 2.0,
        max: 50.0,
        curve: ParamCurve::Exponential,
        default: 8.0,
    },
    ParamDescriptor {
        id: REVERB_PARAM_HEIGHT_M,
        name: "Height",
        unit: "m",
        min: 2.0,
        max: 20.0,
        curve: ParamCurve::Exponential,
        default: 3.2,
    },
    ParamDescriptor {
        id: REVERB_PARAM_DECAY_S,
        name: "Decay",
        unit: "s",
        min: 0.15,
        max: 8.0,
        curve: ParamCurve::Exponential,
        default: 1.2,
    },
    ParamDescriptor {
        id: REVERB_PARAM_CAPTURE_X,
        name: "Mic X",
        unit: "",
        min: 0.05,
        max: 0.95,
        curve: ParamCurve::Linear,
        default: 0.72,
    },
    ParamDescriptor {
        id: REVERB_PARAM_CAPTURE_Y,
        name: "Mic Y",
        unit: "",
        min: 0.05,
        max: 0.95,
        curve: ParamCurve::Linear,
        default: 0.66,
    },
];

/// Geometric family used by the room-IR generator. Dimensions remain explicit
/// in every family; shape changes the reflection density and tail diffusion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReverbShape {
    #[default]
    Studio,
    Chamber,
    Hall,
}

impl ReverbShape {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Chamber,
            2 => Self::Hall,
            _ => Self::Studio,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Studio => 0,
            Self::Chamber => 1,
            Self::Hall => 2,
        }
    }
}

/// Broad wall absorption profiles used by the generated room. They alter both
/// reflection loss and high-frequency decay before the IR reaches the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReverbMaterial {
    #[default]
    Plaster,
    Wood,
    Brick,
    Curtain,
}

impl ReverbMaterial {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Wood,
            2 => Self::Brick,
            3 => Self::Curtain,
            _ => Self::Plaster,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Plaster => 0,
            Self::Wood => 1,
            Self::Brick => 2,
            Self::Curtain => 3,
        }
    }
}

/// Parameters for the generated-room convolution reverb.
///
/// `capture_x` and `capture_y` are normalized room-plan coordinates. The
/// source remains in a musically useful asymmetric fixed position so moving
/// the capture point changes early timing and stereo perspective without
/// turning the compact face into a full acoustic CAD tool.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ReverbParams {
    pub shape: ReverbShape,
    pub material: ReverbMaterial,
    pub width_m: f32,
    pub depth_m: f32,
    pub height_m: f32,
    pub decay_s: f32,
    pub capture_x: f32,
    pub capture_y: f32,
}

impl Default for ReverbParams {
    fn default() -> Self {
        Self {
            shape: ReverbShape::default(),
            material: ReverbMaterial::default(),
            width_m: 6.0,
            depth_m: 8.0,
            height_m: 3.2,
            decay_s: 1.2,
            capture_x: 0.72,
            capture_y: 0.66,
        }
    }
}

// --- Dynamics --------------------------------------------------------------
//
// Gate, compressor, and limiter share the detector and gain computers in
// `mooloop_dsp::dynamics`. They are separate kinds rather than one device
// with a mode, because their controls barely overlap and a mode switch that
// swaps every knob is not a device face.

/// `Event::ParamValue` ids for [`GateParams`].
pub const GATE_PARAM_THRESHOLD_DB: u32 = 0;
pub const GATE_PARAM_ATTACK_MS: u32 = 1;
pub const GATE_PARAM_HOLD_MS: u32 = 2;
pub const GATE_PARAM_RELEASE_MS: u32 = 3;
pub const GATE_PARAM_RANGE_DB: u32 = 4;

static GATE_DESCRIPTORS: [ParamDescriptor; 5] = [
    ParamDescriptor {
        id: GATE_PARAM_THRESHOLD_DB,
        name: "Thresh",
        unit: "dB",
        min: -80.0,
        max: 0.0,
        curve: ParamCurve::Linear,
        default: -40.0,
    },
    ParamDescriptor {
        id: GATE_PARAM_ATTACK_MS,
        name: "Attack",
        unit: "ms",
        min: 0.05,
        max: 100.0,
        curve: ParamCurve::Exponential,
        default: 1.0,
    },
    ParamDescriptor {
        id: GATE_PARAM_HOLD_MS,
        name: "Hold",
        unit: "ms",
        min: 0.0,
        max: 500.0,
        curve: ParamCurve::Linear,
        default: 10.0,
    },
    ParamDescriptor {
        id: GATE_PARAM_RELEASE_MS,
        name: "Release",
        unit: "ms",
        min: 1.0,
        max: 2_000.0,
        curve: ParamCurve::Exponential,
        default: 100.0,
    },
    ParamDescriptor {
        id: GATE_PARAM_RANGE_DB,
        name: "Range",
        unit: "dB",
        min: -80.0,
        max: 0.0,
        curve: ParamCurve::Linear,
        default: -80.0,
    },
];

/// Parameters for the gate effect (`GateEffect` in `mooloop-dsp`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GateParams {
    pub threshold_db: f32,
    pub attack_ms: f32,
    /// How long the gate stays open after the level falls back below the
    /// threshold. Stops it chattering on material that hovers at the line.
    pub hold_ms: f32,
    pub release_ms: f32,
    /// Attenuation applied while shut. 0 dB makes the gate inaudible.
    pub range_db: f32,
}

impl Default for GateParams {
    fn default() -> Self {
        Self {
            threshold_db: -40.0,
            attack_ms: 1.0,
            hold_ms: 10.0,
            release_ms: 100.0,
            range_db: -80.0,
        }
    }
}

/// `Event::ParamValue` ids for [`CompressorParams`].
pub const COMP_PARAM_THRESHOLD_DB: u32 = 0;
pub const COMP_PARAM_RATIO: u32 = 1;
pub const COMP_PARAM_ATTACK_MS: u32 = 2;
pub const COMP_PARAM_RELEASE_MS: u32 = 3;
pub const COMP_PARAM_KNEE_DB: u32 = 4;
pub const COMP_PARAM_MAKEUP_DB: u32 = 5;

static COMPRESSOR_DESCRIPTORS: [ParamDescriptor; 6] = [
    ParamDescriptor {
        id: COMP_PARAM_THRESHOLD_DB,
        name: "Thresh",
        unit: "dB",
        min: -60.0,
        max: 0.0,
        curve: ParamCurve::Linear,
        default: -18.0,
    },
    ParamDescriptor {
        id: COMP_PARAM_RATIO,
        name: "Ratio",
        unit: ":1",
        min: 1.0,
        max: 20.0,
        curve: ParamCurve::Exponential,
        default: 4.0,
    },
    ParamDescriptor {
        id: COMP_PARAM_ATTACK_MS,
        name: "Attack",
        unit: "ms",
        min: 0.05,
        max: 200.0,
        curve: ParamCurve::Exponential,
        default: 10.0,
    },
    ParamDescriptor {
        id: COMP_PARAM_RELEASE_MS,
        name: "Release",
        unit: "ms",
        min: 5.0,
        max: 2_000.0,
        curve: ParamCurve::Exponential,
        default: 120.0,
    },
    ParamDescriptor {
        id: COMP_PARAM_KNEE_DB,
        name: "Knee",
        unit: "dB",
        min: 0.0,
        max: 24.0,
        curve: ParamCurve::Linear,
        default: 6.0,
    },
    ParamDescriptor {
        id: COMP_PARAM_MAKEUP_DB,
        name: "Makeup",
        unit: "dB",
        min: 0.0,
        max: 24.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
];

/// Parameters for the compressor effect (`CompressorEffect` in `mooloop-dsp`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CompressorParams {
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    /// Width of the soft knee around the threshold. 0 is a hard corner.
    pub knee_db: f32,
    pub makeup_db: f32,
}

impl Default for CompressorParams {
    fn default() -> Self {
        Self {
            threshold_db: -18.0,
            ratio: 4.0,
            attack_ms: 10.0,
            release_ms: 120.0,
            knee_db: 6.0,
            makeup_db: 0.0,
        }
    }
}

/// `Event::ParamValue` ids for [`LimiterParams`].
pub const LIMITER_PARAM_CEILING_DB: u32 = 0;
pub const LIMITER_PARAM_RELEASE_MS: u32 = 1;
pub const LIMITER_PARAM_GAIN_DB: u32 = 2;

static LIMITER_DESCRIPTORS: [ParamDescriptor; 3] = [
    ParamDescriptor {
        id: LIMITER_PARAM_CEILING_DB,
        name: "Ceiling",
        unit: "dB",
        min: -24.0,
        max: 0.0,
        curve: ParamCurve::Linear,
        default: -0.3,
    },
    ParamDescriptor {
        id: LIMITER_PARAM_RELEASE_MS,
        name: "Release",
        unit: "ms",
        min: 1.0,
        max: 500.0,
        curve: ParamCurve::Exponential,
        default: 50.0,
    },
    ParamDescriptor {
        id: LIMITER_PARAM_GAIN_DB,
        name: "Gain",
        unit: "dB",
        min: 0.0,
        max: 24.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
];

/// Parameters for the limiter effect (`LimiterEffect` in `mooloop-dsp`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LimiterParams {
    pub ceiling_db: f32,
    pub release_ms: f32,
    /// Input gain driven into the ceiling: this is the loudness control.
    pub gain_db: f32,
}

impl Default for LimiterParams {
    fn default() -> Self {
        Self {
            ceiling_db: -0.3,
            release_ms: 50.0,
            gain_db: 0.0,
        }
    }
}

// --- Slot state ------------------------------------------------------------

/// Per-kind parameter set. Tagged with the same serde shape as
/// `ChannelSource` so new kinds join the v1 envelope additively.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "state", rename_all = "snake_case")]
pub enum EffectParams {
    Eq(EqParams),
    Filter(FilterParams),
    Drive(DriveParams),
    Bitcrush(BitcrushParams),
    Delay(DelayParams),
    Reverb(ReverbParams),
    Gate(GateParams),
    Compressor(CompressorParams),
    Limiter(LimiterParams),
}

impl EffectParams {
    pub fn kind(&self) -> EffectKind {
        match self {
            Self::Eq(_) => EffectKind::Eq,
            Self::Filter(_) => EffectKind::Filter,
            Self::Drive(_) => EffectKind::Drive,
            Self::Bitcrush(_) => EffectKind::Bitcrush,
            Self::Delay(_) => EffectKind::Delay,
            Self::Reverb(_) => EffectKind::Reverb,
            Self::Gate(_) => EffectKind::Gate,
            Self::Compressor(_) => EffectKind::Compressor,
            Self::Limiter(_) => EffectKind::Limiter,
        }
    }

    pub fn filter(&self) -> Option<&FilterParams> {
        match self {
            Self::Filter(p) => Some(p),
            _ => None,
        }
    }

    pub fn eq(&self) -> Option<&EqParams> {
        match self {
            Self::Eq(p) => Some(p),
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

    pub fn delay(&self) -> Option<&DelayParams> {
        match self {
            Self::Delay(p) => Some(p),
            _ => None,
        }
    }

    pub fn reverb(&self) -> Option<&ReverbParams> {
        match self {
            Self::Reverb(p) => Some(p),
            _ => None,
        }
    }

    pub fn gate(&self) -> Option<&GateParams> {
        match self {
            Self::Gate(p) => Some(p),
            _ => None,
        }
    }

    pub fn compressor(&self) -> Option<&CompressorParams> {
        match self {
            Self::Compressor(p) => Some(p),
            _ => None,
        }
    }

    pub fn limiter(&self) -> Option<&LimiterParams> {
        match self {
            Self::Limiter(p) => Some(p),
            _ => None,
        }
    }

    /// Read one parameter in natural units by wire id. Returns `None` for an
    /// id this kind does not have.
    pub fn get(&self, id: u32) -> Option<f32> {
        match self {
            Self::Eq(p) => match id {
                EQ_PARAM_TARGET => Some(f32::from(p.selected_target)),
                EQ_PARAM_ENABLED => Some(
                    if match p.selected_target() {
                        0..EQ_MAX_BANDS => p.bands[p.selected_target()].enabled,
                        EqParams::HIGH_PASS_TARGET => p.high_pass.enabled,
                        _ => p.low_pass.enabled,
                    } {
                        1.0
                    } else {
                        0.0
                    },
                ),
                EQ_PARAM_FREQUENCY_HZ => Some(match p.selected_target() {
                    0..EQ_MAX_BANDS => p.bands[p.selected_target()].frequency_hz,
                    EqParams::HIGH_PASS_TARGET => p.high_pass.frequency_hz,
                    _ => p.low_pass.frequency_hz,
                }),
                EQ_PARAM_GAIN_DB => Some(if p.selected_target() < EQ_MAX_BANDS {
                    p.bands[p.selected_target()].gain_db
                } else {
                    0.0
                }),
                EQ_PARAM_Q => Some(match p.selected_target() {
                    0..EQ_MAX_BANDS => p.bands[p.selected_target()].q,
                    EqParams::HIGH_PASS_TARGET => p.high_pass.q,
                    _ => p.low_pass.q,
                }),
                EQ_PARAM_CHARACTER => Some(match p.selected_target() {
                    0..EQ_MAX_BANDS => p.bands[p.selected_target()].q_profile.to_index() as f32,
                    EqParams::HIGH_PASS_TARGET => p.high_pass.slope.to_index() as f32,
                    _ => p.low_pass.slope.to_index() as f32,
                }),
                _ => None,
            },
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
            Self::Delay(p) => match id {
                DELAY_PARAM_TIME_MS => Some(p.time_ms),
                DELAY_PARAM_FEEDBACK => Some(p.feedback),
                DELAY_PARAM_MODE => Some(p.mode.to_index() as f32),
                DELAY_PARAM_CROSS => Some(p.cross),
                DELAY_PARAM_TONE => Some(p.tone),
                DELAY_PARAM_MIX => Some(p.mix),
                _ => None,
            },
            Self::Reverb(p) => match id {
                REVERB_PARAM_SHAPE => Some(p.shape.to_index() as f32),
                REVERB_PARAM_MATERIAL => Some(p.material.to_index() as f32),
                REVERB_PARAM_WIDTH_M => Some(p.width_m),
                REVERB_PARAM_DEPTH_M => Some(p.depth_m),
                REVERB_PARAM_HEIGHT_M => Some(p.height_m),
                REVERB_PARAM_DECAY_S => Some(p.decay_s),
                REVERB_PARAM_CAPTURE_X => Some(p.capture_x),
                REVERB_PARAM_CAPTURE_Y => Some(p.capture_y),
                _ => None,
            },
            Self::Gate(p) => match id {
                GATE_PARAM_THRESHOLD_DB => Some(p.threshold_db),
                GATE_PARAM_ATTACK_MS => Some(p.attack_ms),
                GATE_PARAM_HOLD_MS => Some(p.hold_ms),
                GATE_PARAM_RELEASE_MS => Some(p.release_ms),
                GATE_PARAM_RANGE_DB => Some(p.range_db),
                _ => None,
            },
            Self::Compressor(p) => match id {
                COMP_PARAM_THRESHOLD_DB => Some(p.threshold_db),
                COMP_PARAM_RATIO => Some(p.ratio),
                COMP_PARAM_ATTACK_MS => Some(p.attack_ms),
                COMP_PARAM_RELEASE_MS => Some(p.release_ms),
                COMP_PARAM_KNEE_DB => Some(p.knee_db),
                COMP_PARAM_MAKEUP_DB => Some(p.makeup_db),
                _ => None,
            },
            Self::Limiter(p) => match id {
                LIMITER_PARAM_CEILING_DB => Some(p.ceiling_db),
                LIMITER_PARAM_RELEASE_MS => Some(p.release_ms),
                LIMITER_PARAM_GAIN_DB => Some(p.gain_db),
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
            Self::Eq(p) => match id {
                EQ_PARAM_TARGET => p.set_selected_target(value),
                EQ_PARAM_ENABLED => match p.selected_target() {
                    0..EQ_MAX_BANDS => p.bands[p.selected_target()].enabled = value >= 0.5,
                    EqParams::HIGH_PASS_TARGET => p.high_pass.enabled = value >= 0.5,
                    _ => p.low_pass.enabled = value >= 0.5,
                },
                EQ_PARAM_FREQUENCY_HZ => match p.selected_target() {
                    0..EQ_MAX_BANDS => p.bands[p.selected_target()].frequency_hz = value,
                    EqParams::HIGH_PASS_TARGET => p.high_pass.frequency_hz = value,
                    _ => p.low_pass.frequency_hz = value,
                },
                EQ_PARAM_GAIN_DB => {
                    if p.selected_target() < EQ_MAX_BANDS {
                        p.bands[p.selected_target()].gain_db = value;
                    }
                }
                EQ_PARAM_Q => match p.selected_target() {
                    0..EQ_MAX_BANDS => p.bands[p.selected_target()].q = value,
                    EqParams::HIGH_PASS_TARGET => p.high_pass.q = value,
                    _ => p.low_pass.q = value,
                },
                EQ_PARAM_CHARACTER => match p.selected_target() {
                    0..EQ_MAX_BANDS => {
                        p.bands[p.selected_target()].q_profile =
                            EqQProfile::from_index(value.round() as i32)
                    }
                    EqParams::HIGH_PASS_TARGET => {
                        p.high_pass.slope = EqSlope::from_index(value.round() as i32)
                    }
                    _ => p.low_pass.slope = EqSlope::from_index(value.round() as i32),
                },
                _ => return None,
            },
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
            Self::Delay(p) => match id {
                DELAY_PARAM_TIME_MS => p.time_ms = value,
                DELAY_PARAM_FEEDBACK => p.feedback = value,
                DELAY_PARAM_MODE => p.mode = DelayMode::from_index(value.round() as i32),
                DELAY_PARAM_CROSS => p.cross = value,
                DELAY_PARAM_TONE => p.tone = value,
                DELAY_PARAM_MIX => p.mix = value,
                _ => return None,
            },
            Self::Reverb(p) => match id {
                REVERB_PARAM_SHAPE => p.shape = ReverbShape::from_index(value.round() as i32),
                REVERB_PARAM_MATERIAL => {
                    p.material = ReverbMaterial::from_index(value.round() as i32)
                }
                REVERB_PARAM_WIDTH_M => p.width_m = value,
                REVERB_PARAM_DEPTH_M => p.depth_m = value,
                REVERB_PARAM_HEIGHT_M => p.height_m = value,
                REVERB_PARAM_DECAY_S => p.decay_s = value,
                REVERB_PARAM_CAPTURE_X => p.capture_x = value,
                REVERB_PARAM_CAPTURE_Y => p.capture_y = value,
                _ => return None,
            },
            Self::Gate(p) => match id {
                GATE_PARAM_THRESHOLD_DB => p.threshold_db = value,
                GATE_PARAM_ATTACK_MS => p.attack_ms = value,
                GATE_PARAM_HOLD_MS => p.hold_ms = value,
                GATE_PARAM_RELEASE_MS => p.release_ms = value,
                GATE_PARAM_RANGE_DB => p.range_db = value,
                _ => return None,
            },
            Self::Compressor(p) => match id {
                COMP_PARAM_THRESHOLD_DB => p.threshold_db = value,
                COMP_PARAM_RATIO => p.ratio = value,
                COMP_PARAM_ATTACK_MS => p.attack_ms = value,
                COMP_PARAM_RELEASE_MS => p.release_ms = value,
                COMP_PARAM_KNEE_DB => p.knee_db = value,
                COMP_PARAM_MAKEUP_DB => p.makeup_db = value,
                _ => return None,
            },
            Self::Limiter(p) => match id {
                LIMITER_PARAM_CEILING_DB => p.ceiling_db = value,
                LIMITER_PARAM_RELEASE_MS => p.release_ms = value,
                LIMITER_PARAM_GAIN_DB => p.gain_db = value,
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
    #[serde(default = "default_wet_dry")]
    pub wet_dry: f32,
    #[serde(default = "default_input_trim")]
    pub input_trim: f32,
    #[serde(default = "default_output_trim")]
    pub output_trim: f32,
}

fn default_wet_dry() -> f32 {
    1.0
}
fn default_input_trim() -> f32 {
    1.0
}
fn default_output_trim() -> f32 {
    1.0
}

impl EffectSlotState {
    pub fn new(params: EffectParams) -> Self {
        Self {
            params,
            bypassed: false,
            wet_dry: 1.0,
            input_trim: 1.0,
            output_trim: 1.0,
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

    pub fn delay(params: DelayParams) -> Self {
        Self::new(EffectParams::Delay(params))
    }

    pub fn gate(params: GateParams) -> Self {
        Self::new(EffectParams::Gate(params))
    }

    pub fn compressor(params: CompressorParams) -> Self {
        Self::new(EffectParams::Compressor(params))
    }

    pub fn limiter(params: LimiterParams) -> Self {
        Self::new(EffectParams::Limiter(params))
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
                let actual = params.get(descriptor.id).unwrap_or_else(|| {
                    panic!("{}/{} has no getter", kind.label(), descriptor.name)
                });
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
        assert_eq!(
            params.set(FILTER_PARAM_CUTOFF_HZ, 1_000_000.0),
            Some(20_000.0)
        );
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
