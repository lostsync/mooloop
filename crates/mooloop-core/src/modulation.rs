//! Parameter addressing and the modulator rack.
//!
//! `docs/MODULATION_PLAN.md` is the approved design; this implements it.
//! Two ideas carry the whole thing:
//!
//! - A parameter is named by a [`ParamAddr`], not by a bespoke command per
//!   device kind. One address type is what makes an automation lane, a mod
//!   matrix row, and a knob all talk about the same thing.
//! - The engine owns a **base** value and the sum of **modulation offsets**,
//!   and emits the resolved sum. Devices store only resolved values, so no
//!   effect needs any change to support modulation.

use crate::effect::{ParamCurve, ParamDescriptor};
use crate::gain::MAX_LINEAR_GAIN;
use crate::mod_metadata::ModDestinationDescriptor;
use crate::EffectTarget;

/// Which device inside a channel or bus owns the parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamOwner {
    /// The channel's generator. Buses have none.
    Source,
    Effect {
        slot: u8,
    },
    Modulator {
        slot: u8,
    },
    /// Volume, pan, mute — the strip itself rather than a device on it.
    Strip,
}

/// A parameter, anywhere in the project.
///
/// `scope` carries the channel or bus from the day this type exists, so
/// enabling cross-channel modulation later is a routing change rather than a
/// retyping of every engine command (`MODULATION_PLAN.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ParamAddr {
    pub scope: EffectTarget,
    pub owner: ParamOwner,
    /// The owning kind's stable descriptor id. Never renumbered, because
    /// modulation and automation persist it.
    pub param: u32,
}

impl ParamAddr {
    pub const fn effect(scope: EffectTarget, slot: u8, param: u32) -> Self {
        Self {
            scope,
            owner: ParamOwner::Effect { slot },
            param,
        }
    }

    pub const fn strip(scope: EffectTarget, param: u32) -> Self {
        Self {
            scope,
            owner: ParamOwner::Strip,
            param,
        }
    }
}

/// The strip's own parameters. The strip is addressed like any device, so its
/// controls need stable descriptor ids too -- that is what lets a source
/// target a fader without the mixer growing a modulation special case
/// (`MODULATOR_SYSTEM_SPEC.md`, "Destinations and destination metadata").
pub const STRIP_PARAM_VOLUME: u32 = 0;
pub const STRIP_PARAM_PAN: u32 = 1;

pub static STRIP_DESCRIPTORS: [ParamDescriptor; 2] = [
    ParamDescriptor {
        id: STRIP_PARAM_VOLUME,
        name: "Volume",
        // Linear rather than the fader's display taper: modulation depth is a
        // fraction of the normalized range, and the taper belongs to the
        // control surface, not to the destination's numeric truth.
        unit: "x",
        min: 0.0,
        max: MAX_LINEAR_GAIN,
        curve: ParamCurve::Linear,
        default: 0.8,
    },
    ParamDescriptor {
        id: STRIP_PARAM_PAN,
        name: "Pan",
        unit: "",
        min: -1.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
];

pub fn strip_descriptor(id: u32) -> Option<&'static ParamDescriptor> {
    STRIP_DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.id == id)
}

/// Shape of a free-running LFO.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModLfoWaveform {
    #[default]
    Sine,
    Triangle,
    Saw,
    Square,
    /// Stepped random, held between transitions. Sample-and-hold as a wave
    /// rather than a separate modulator kind.
    Random,
}

impl ModLfoWaveform {
    pub const ALL: [Self; 5] = [
        Self::Sine,
        Self::Triangle,
        Self::Saw,
        Self::Square,
        Self::Random,
    ];

    pub fn from_index(index: i32) -> Self {
        Self::ALL
            .get(index.clamp(0, Self::ALL.len() as i32 - 1) as usize)
            .copied()
            .unwrap_or_default()
    }

    pub fn to_index(self) -> i32 {
        Self::ALL
            .iter()
            .position(|waveform| *waveform == self)
            .unwrap_or_default() as i32
    }
}

/// A transport-relative duration, ordered from the slowest useful LFO cycle
/// to a 64th-note triplet. The same vocabulary drives both synced rate and
/// synced fade-in, so their knobs never invent subtly different timing grids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModTimeDivision {
    FourWhole,
    DoubleWhole,
    Whole,
    DottedHalf,
    Half,
    HalfTriplet,
    DottedQuarter,
    #[default]
    Quarter,
    QuarterTriplet,
    DottedEighth,
    Eighth,
    EighthTriplet,
    DottedSixteenth,
    Sixteenth,
    SixteenthTriplet,
    DottedThirtySecond,
    ThirtySecond,
    ThirtySecondTriplet,
    DottedSixtyFourth,
    SixtyFourth,
    SixtyFourthTriplet,
}

impl ModTimeDivision {
    pub const ALL: [Self; 21] = [
        Self::FourWhole,
        Self::DoubleWhole,
        Self::Whole,
        Self::DottedHalf,
        Self::Half,
        Self::HalfTriplet,
        Self::DottedQuarter,
        Self::Quarter,
        Self::QuarterTriplet,
        Self::DottedEighth,
        Self::Eighth,
        Self::EighthTriplet,
        Self::DottedSixteenth,
        Self::Sixteenth,
        Self::SixteenthTriplet,
        Self::DottedThirtySecond,
        Self::ThirtySecond,
        Self::ThirtySecondTriplet,
        Self::DottedSixtyFourth,
        Self::SixtyFourth,
        Self::SixtyFourthTriplet,
    ];

    /// Duration in quarter-note beats.
    pub fn beats(self) -> f32 {
        match self {
            Self::FourWhole => 16.0,
            Self::DoubleWhole => 8.0,
            Self::Whole => 4.0,
            Self::DottedHalf => 3.0,
            Self::Half => 2.0,
            Self::HalfTriplet => 4.0 / 3.0,
            Self::DottedQuarter => 1.5,
            Self::Quarter => 1.0,
            Self::QuarterTriplet => 2.0 / 3.0,
            Self::DottedEighth => 0.75,
            Self::Eighth => 0.5,
            Self::EighthTriplet => 1.0 / 3.0,
            Self::DottedSixteenth => 0.375,
            Self::Sixteenth => 0.25,
            Self::SixteenthTriplet => 1.0 / 6.0,
            Self::DottedThirtySecond => 0.1875,
            Self::ThirtySecond => 0.125,
            Self::ThirtySecondTriplet => 1.0 / 12.0,
            Self::DottedSixtyFourth => 0.09375,
            Self::SixtyFourth => 0.0625,
            Self::SixtyFourthTriplet => 1.0 / 24.0,
        }
    }

    pub fn seconds(self, bpm: f64) -> f32 {
        self.beats() * (60.0 / bpm.max(1.0)) as f32
    }

    pub fn rate_hz(self, bpm: f64) -> f32 {
        self.seconds(bpm).recip()
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
            .position(|division| *division == self)
            .unwrap_or(7) as i32
    }
}

/// When a step sequencer advances. Free-running follows the transport clock;
/// note-advance steps once per note-on, which is what makes a pattern feel
/// played rather than merely running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModStepTrigger {
    #[default]
    Clock,
    NoteAdvance,
}

impl ModStepTrigger {
    pub const ALL: [Self; 2] = [Self::Clock, Self::NoteAdvance];

    pub fn from_index(index: i32) -> Self {
        Self::ALL
            .get(index.clamp(0, Self::ALL.len() as i32 - 1) as usize)
            .copied()
            .unwrap_or_default()
    }

    pub fn to_index(self) -> i32 {
        Self::ALL
            .iter()
            .position(|trigger| *trigger == self)
            .unwrap_or_default() as i32
    }
}

/// When a random source draws. The clock is the same musical grid every
/// other timed source uses; note-triggered draws once per note-on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModRandomTrigger {
    #[default]
    Clock,
    NoteTrigger,
}

impl ModRandomTrigger {
    pub const ALL: [Self; 2] = [Self::Clock, Self::NoteTrigger];

    pub fn from_index(index: i32) -> Self {
        Self::ALL
            .get(index.clamp(0, Self::ALL.len() as i32 - 1) as usize)
            .copied()
            .unwrap_or_default()
    }

    pub fn to_index(self) -> i32 {
        Self::ALL
            .iter()
            .position(|trigger| *trigger == self)
            .unwrap_or_default() as i32
    }
}

/// What a math module does to its input. Arithmetic ops take the constant
/// operand; `Clamp` takes the low/high pair instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModMathOp {
    Add,
    Subtract,
    #[default]
    Multiply,
    Divide,
    Min,
    Max,
    Clamp,
}

impl ModMathOp {
    pub const ALL: [Self; 7] = [
        Self::Add,
        Self::Subtract,
        Self::Multiply,
        Self::Divide,
        Self::Min,
        Self::Max,
        Self::Clamp,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Subtract => "−",
            Self::Multiply => "×",
            Self::Divide => "÷",
            Self::Min => "MIN",
            Self::Max => "MAX",
            Self::Clamp => "CLAMP",
        }
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
            .position(|op| *op == self)
            .unwrap_or_default() as i32
    }
}

/// A modulator's own parameters. Modulators are addressable like any other
/// device: each kind publishes a `ParamDescriptor` table under these ids and
/// answers `ModulatorParams::get`/`set`, exactly as effects do. The first
/// four ids shipped before the table existed and must keep their numbers.
pub const LFO_PARAM_RATE_HZ: u32 = 0;
pub const LFO_PARAM_DEPTH: u32 = 1;
pub const LFO_PARAM_WAVEFORM: u32 = 2;
pub const LFO_PARAM_PHASE: u32 = 3;
pub const LFO_PARAM_TEMPO_SYNC: u32 = 4;
pub const LFO_PARAM_RATE_DIVISION: u32 = 5;
pub const LFO_PARAM_RETRIGGER: u32 = 6;
pub const LFO_PARAM_FADE_IN_S: u32 = 7;
pub const LFO_PARAM_FADE_IN_SYNC: u32 = 8;
pub const LFO_PARAM_FADE_IN_DIVISION: u32 = 9;
pub const LFO_PARAM_SMOOTHING_S: u32 = 10;
pub const LFO_PARAM_PULSE_WIDTH: u32 = 11;

/// The envelope's gate input channel is deliberately absent: it is an input
/// jack bound through a dynamic channel list, not a knob over a static
/// range, and it keeps its dedicated verb.
pub const ENV_PARAM_ATTACK_S: u32 = 0;
pub const ENV_PARAM_ATTACK_SYNC: u32 = 1;
pub const ENV_PARAM_ATTACK_DIVISION: u32 = 2;
pub const ENV_PARAM_DECAY_S: u32 = 3;
pub const ENV_PARAM_DECAY_SYNC: u32 = 4;
pub const ENV_PARAM_DECAY_DIVISION: u32 = 5;
pub const ENV_PARAM_SUSTAIN: u32 = 6;
pub const ENV_PARAM_RELEASE_S: u32 = 7;
pub const ENV_PARAM_RELEASE_SYNC: u32 = 8;
pub const ENV_PARAM_RELEASE_DIVISION: u32 = 9;
pub const ENV_PARAM_AMOUNT: u32 = 10;

/// Steps a pattern can hold. Sixteen is the Matrix gesture at modulator
/// scale, and it keeps `ModStepParams` small enough to stay `Copy` on the
/// command ring.
pub const MOD_STEP_MAX_STEPS: usize = 16;

/// The scalars keep the low ids and the sixteen per-step values follow in
/// one contiguous block, so the editor walks them as a bank rather than
/// naming each one.
pub const STEP_PARAM_LENGTH: u32 = 0;
pub const STEP_PARAM_DIVISION: u32 = 1;
pub const STEP_PARAM_GLIDE: u32 = 2;
pub const STEP_PARAM_TRIGGER: u32 = 3;
pub const STEP_PARAM_VALUE_BASE: u32 = 4;

/// The step-value block as an index into [`ModStepParams::steps`].
pub const fn step_value_index(id: u32) -> Option<usize> {
    match id.checked_sub(STEP_PARAM_VALUE_BASE) {
        Some(offset) if (offset as usize) < MOD_STEP_MAX_STEPS => Some(offset as usize),
        _ => None,
    }
}

/// The random source keeps the LFO's three-id tempo-syncable rate, because
/// it is the promotion of that LFO's hidden sample-and-hold and must not
/// lose its free rate on the way.
pub const RANDOM_PARAM_RATE_HZ: u32 = 0;
pub const RANDOM_PARAM_TEMPO_SYNC: u32 = 1;
pub const RANDOM_PARAM_RATE_DIVISION: u32 = 2;
pub const RANDOM_PARAM_TRIGGER: u32 = 3;
pub const RANDOM_PARAM_BIPOLAR: u32 = 4;
pub const RANDOM_PARAM_PROBABILITY: u32 = 5;
pub const RANDOM_PARAM_QUANTIZE: u32 = 6;
pub const RANDOM_PARAM_DRUNK: u32 = 7;
pub const RANDOM_PARAM_WALK: u32 = 8;

/// The math module's input is a slot reference rather than a channel list,
/// so unlike the envelope's gate it is an ordinary stepped descriptor.
pub const MATH_PARAM_INPUT_SLOT: u32 = 0;
pub const MATH_PARAM_OP: u32 = 1;
pub const MATH_PARAM_OPERAND: u32 = 2;
pub const MATH_PARAM_CLAMP_LOW: u32 = 3;
pub const MATH_PARAM_CLAMP_HIGH: u32 = 4;

/// A boolean as a descriptor: two positions, off by default.
const fn toggle(id: u32, name: &'static str) -> ParamDescriptor {
    ParamDescriptor {
        id,
        name,
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Stepped(2),
        default: 0.0,
    }
}

/// A `ModTimeDivision` as a descriptor: the shared 21-entry musical grid,
/// carried as its `ALL` index.
const fn division(id: u32, name: &'static str, default: f32) -> ParamDescriptor {
    ParamDescriptor {
        id,
        name,
        unit: "",
        min: 0.0,
        max: (ModTimeDivision::ALL.len() - 1) as f32,
        curve: ParamCurve::Stepped(ModTimeDivision::ALL.len() as u8),
        default,
    }
}

/// Ranges mirror the shipped shelf knobs; the table is now where they live.
pub const LFO_DESCRIPTORS: [ParamDescriptor; 12] = [
    ParamDescriptor {
        id: LFO_PARAM_RATE_HZ,
        name: "Rate",
        unit: "Hz",
        min: 0.05,
        max: 20.0,
        curve: ParamCurve::Linear,
        default: 1.0,
    },
    ParamDescriptor {
        id: LFO_PARAM_DEPTH,
        name: "Amount",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 1.0,
    },
    ParamDescriptor {
        id: LFO_PARAM_WAVEFORM,
        name: "Waveform",
        unit: "",
        min: 0.0,
        max: (ModLfoWaveform::ALL.len() - 1) as f32,
        curve: ParamCurve::Stepped(ModLfoWaveform::ALL.len() as u8),
        default: 0.0,
    },
    ParamDescriptor {
        id: LFO_PARAM_PHASE,
        name: "Phase",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    toggle(LFO_PARAM_TEMPO_SYNC, "Rate sync"),
    division(LFO_PARAM_RATE_DIVISION, "Rate division", 7.0),
    toggle(LFO_PARAM_RETRIGGER, "Retrigger"),
    ParamDescriptor {
        id: LFO_PARAM_FADE_IN_S,
        name: "Fade in",
        unit: "s",
        min: 0.0,
        max: 16.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    toggle(LFO_PARAM_FADE_IN_SYNC, "Fade sync"),
    division(LFO_PARAM_FADE_IN_DIVISION, "Fade division", 7.0),
    ParamDescriptor {
        id: LFO_PARAM_SMOOTHING_S,
        name: "Smooth",
        unit: "s",
        min: 0.0,
        max: 2.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    ParamDescriptor {
        id: LFO_PARAM_PULSE_WIDTH,
        name: "Pulse width",
        unit: "",
        min: 0.01,
        max: 0.99,
        curve: ParamCurve::Linear,
        default: 0.5,
    },
];

pub const ENVELOPE_DESCRIPTORS: [ParamDescriptor; 11] = [
    ParamDescriptor {
        id: ENV_PARAM_ATTACK_S,
        name: "Attack",
        unit: "s",
        min: 0.0,
        max: 16.0,
        curve: ParamCurve::Linear,
        default: 0.01,
    },
    toggle(ENV_PARAM_ATTACK_SYNC, "Attack sync"),
    division(ENV_PARAM_ATTACK_DIVISION, "Attack division", 13.0),
    ParamDescriptor {
        id: ENV_PARAM_DECAY_S,
        name: "Decay",
        unit: "s",
        min: 0.0,
        max: 16.0,
        curve: ParamCurve::Linear,
        default: 0.2,
    },
    toggle(ENV_PARAM_DECAY_SYNC, "Decay sync"),
    division(ENV_PARAM_DECAY_DIVISION, "Decay division", 10.0),
    ParamDescriptor {
        id: ENV_PARAM_SUSTAIN,
        name: "Sustain",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.7,
    },
    ParamDescriptor {
        id: ENV_PARAM_RELEASE_S,
        name: "Release",
        unit: "s",
        min: 0.0,
        max: 16.0,
        curve: ParamCurve::Linear,
        default: 0.4,
    },
    toggle(ENV_PARAM_RELEASE_SYNC, "Release sync"),
    division(ENV_PARAM_RELEASE_DIVISION, "Release division", 7.0),
    ParamDescriptor {
        id: ENV_PARAM_AMOUNT,
        name: "Amount",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 1.0,
    },
];

/// An enum as a descriptor: `positions` discrete slots carried as the
/// enum's `ALL` index, the same projection `division` makes for time.
const fn selector(
    id: u32,
    name: &'static str,
    positions: u8,
    default: f32,
) -> ParamDescriptor {
    ParamDescriptor {
        id,
        name,
        unit: "",
        min: 0.0,
        max: (positions - 1) as f32,
        curve: ParamCurve::Stepped(positions),
        default,
    }
}

const STEP_VALUE_NAMES: [&str; MOD_STEP_MAX_STEPS] = [
    "Step 1", "Step 2", "Step 3", "Step 4", "Step 5", "Step 6", "Step 7", "Step 8", "Step 9",
    "Step 10", "Step 11", "Step 12", "Step 13", "Step 14", "Step 15", "Step 16",
];

/// A fresh pattern is flat: every step rests at zero, so adding a sequencer
/// changes nothing until it is drawn. Descriptor defaults and
/// `ModStepParams::default` are the same statement, pinned by a test.
pub const STEP_DESCRIPTORS: [ParamDescriptor; 4 + MOD_STEP_MAX_STEPS] = {
    let mut table = [ParamDescriptor {
        id: 0,
        name: "",
        unit: "",
        min: -1.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    }; 4 + MOD_STEP_MAX_STEPS];
    table[0] = ParamDescriptor {
        id: STEP_PARAM_LENGTH,
        name: "Length",
        unit: "",
        min: 1.0,
        max: MOD_STEP_MAX_STEPS as f32,
        curve: ParamCurve::Stepped(MOD_STEP_MAX_STEPS as u8),
        default: 8.0,
    };
    table[1] = division(STEP_PARAM_DIVISION, "Rate", 13.0);
    table[2] = ParamDescriptor {
        id: STEP_PARAM_GLIDE,
        name: "Glide",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    };
    table[3] = selector(STEP_PARAM_TRIGGER, "Trigger", ModStepTrigger::ALL.len() as u8, 0.0);
    let mut index = 0;
    while index < MOD_STEP_MAX_STEPS {
        table[4 + index] = ParamDescriptor {
            id: STEP_PARAM_VALUE_BASE + index as u32,
            name: STEP_VALUE_NAMES[index],
            unit: "",
            min: -1.0,
            max: 1.0,
            curve: ParamCurve::Linear,
            default: 0.0,
        };
        index += 1;
    }
    table
};

pub const RANDOM_DESCRIPTORS: [ParamDescriptor; 9] = [
    ParamDescriptor {
        id: RANDOM_PARAM_RATE_HZ,
        name: "Rate",
        unit: "Hz",
        min: 0.05,
        max: 20.0,
        curve: ParamCurve::Linear,
        default: 2.0,
    },
    toggle(RANDOM_PARAM_TEMPO_SYNC, "Rate sync"),
    division(RANDOM_PARAM_RATE_DIVISION, "Rate division", 13.0),
    selector(
        RANDOM_PARAM_TRIGGER,
        "Trigger",
        ModRandomTrigger::ALL.len() as u8,
        0.0,
    ),
    // Bipolar is the rack's resting convention, so this toggle starts on.
    selector(RANDOM_PARAM_BIPOLAR, "Bipolar", 2, 1.0),
    ParamDescriptor {
        id: RANDOM_PARAM_PROBABILITY,
        name: "Chance",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 1.0,
    },
    // Zero is off; anything higher is that many levels across the range.
    selector(RANDOM_PARAM_QUANTIZE, "Quantize", 17, 0.0),
    toggle(RANDOM_PARAM_DRUNK, "Drunk"),
    ParamDescriptor {
        id: RANDOM_PARAM_WALK,
        name: "Walk",
        unit: "",
        min: 0.01,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.25,
    },
];

/// The operand spans four times the rack's range so a multiply can actually
/// gain a quiet source up; the module clamps its own output back to `-1..1`
/// regardless, at the module edge, so a route never sees a stray value.
pub const MATH_DESCRIPTORS: [ParamDescriptor; 5] = [
    selector(
        MATH_PARAM_INPUT_SLOT,
        "Input",
        MAX_MODULATORS_PER_CHANNEL as u8,
        0.0,
    ),
    selector(MATH_PARAM_OP, "Operator", ModMathOp::ALL.len() as u8, 2.0),
    ParamDescriptor {
        id: MATH_PARAM_OPERAND,
        name: "Operand",
        unit: "",
        min: -4.0,
        max: 4.0,
        curve: ParamCurve::Linear,
        default: 1.0,
    },
    ParamDescriptor {
        id: MATH_PARAM_CLAMP_LOW,
        name: "Low",
        unit: "",
        min: -1.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: -1.0,
    },
    ParamDescriptor {
        id: MATH_PARAM_CLAMP_HIGH,
        name: "High",
        unit: "",
        min: -1.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 1.0,
    },
];

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ModLfoParams {
    pub rate_hz: f32,
    /// Follow a musical duration instead of `rate_hz`.
    pub tempo_sync: bool,
    pub rate_division: ModTimeDivision,
    /// Output scale, `0..1`. Per-destination depth is separate and lives in
    /// the matrix row; this is the modulator's own level.
    pub depth: f32,
    pub waveform: ModLfoWaveform,
    /// Starting phase in `0..1`, applied on reset.
    pub phase: f32,
    /// Restart the phase on note-on. What makes an LFO feel played rather
    /// than merely running.
    pub retrigger: bool,
    /// Free fade-in duration in seconds. A retrigger also restarts the fade.
    pub fade_in_seconds: f32,
    pub fade_in_tempo_sync: bool,
    pub fade_in_division: ModTimeDivision,
    /// One-pole output smoothing time in seconds.
    pub smoothing_seconds: f32,
    /// High portion of the square cycle, `0.01..0.99`.
    pub pulse_width: f32,
}

impl Default for ModLfoParams {
    fn default() -> Self {
        Self {
            rate_hz: 1.0,
            tempo_sync: false,
            rate_division: ModTimeDivision::Quarter,
            depth: 1.0,
            waveform: ModLfoWaveform::Sine,
            phase: 0.0,
            retrigger: false,
            fade_in_seconds: 0.0,
            fade_in_tempo_sync: false,
            fade_in_division: ModTimeDivision::Quarter,
            smoothing_seconds: 0.0,
            pulse_width: 0.5,
        }
    }
}

/// A gate-driven ADSR control source. The input is an explicit channel-note
/// adapter today; it is intentionally stored on the source so a future typed
/// generator `Gate` outlet can replace the adapter without changing routes or
/// destinations.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ModEnvelopeParams {
    /// Channel whose scheduled Note On/Off stream drives this gate.
    pub input_channel: u8,
    pub attack_seconds: f32,
    pub attack_tempo_sync: bool,
    pub attack_division: ModTimeDivision,
    pub decay_seconds: f32,
    pub decay_tempo_sync: bool,
    pub decay_division: ModTimeDivision,
    pub sustain: f32,
    pub release_seconds: f32,
    pub release_tempo_sync: bool,
    pub release_division: ModTimeDivision,
    /// Source-wide output scale. Destination route depth remains separate.
    pub amount: f32,
}

impl Default for ModEnvelopeParams {
    fn default() -> Self {
        Self {
            input_channel: 0,
            attack_seconds: 0.01,
            attack_tempo_sync: false,
            attack_division: ModTimeDivision::Sixteenth,
            decay_seconds: 0.2,
            decay_tempo_sync: false,
            decay_division: ModTimeDivision::Eighth,
            sustain: 0.7,
            release_seconds: 0.4,
            release_tempo_sync: false,
            release_division: ModTimeDivision::Quarter,
            amount: 1.0,
        }
    }
}


/// A clocked pattern of control values — the Reason Matrix gesture at
/// modulator scale, and the rack's first genuinely stepped source. The step
/// array is fixed at its maximum and `length` decides how much of it plays,
/// so editing the length never destroys the tail of a pattern.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ModStepParams {
    pub steps: [f32; MOD_STEP_MAX_STEPS],
    /// Steps actually played, `1..=16`.
    pub length: u8,
    /// Duration of one step on the shared musical grid.
    pub division: ModTimeDivision,
    /// Portion of a step spent sliding into its value, `0..1`. Zero is the
    /// hard staircase a stepped source is expected to make.
    pub glide: f32,
    pub trigger: ModStepTrigger,
}

impl Default for ModStepParams {
    fn default() -> Self {
        Self {
            steps: [0.0; MOD_STEP_MAX_STEPS],
            length: 8,
            division: ModTimeDivision::Sixteenth,
            glide: 0.0,
            trigger: ModStepTrigger::Clock,
        }
    }
}

/// Sample-and-hold promoted from a hidden LFO waveform to a source with room
/// to be musical: a draw can be skipped by chance, snapped to a grid, or
/// made to walk from the held value instead of jumping to a new one.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ModRandomParams {
    pub rate_hz: f32,
    pub tempo_sync: bool,
    pub rate_division: ModTimeDivision,
    pub trigger: ModRandomTrigger,
    /// `-1..1` when set, `0..1` when clear.
    pub bipolar: bool,
    /// Chance in `0..1` that a due draw actually replaces the held value.
    /// At zero the source freezes; at one it draws on every clock.
    pub probability: f32,
    /// Levels the output snaps to, or zero for continuous.
    pub quantize: u8,
    /// Walk from the held value instead of jumping to an independent one.
    pub drunk: bool,
    /// Largest distance a drunk step may travel, as a fraction of the range.
    pub walk: f32,
}

impl Default for ModRandomParams {
    fn default() -> Self {
        Self {
            rate_hz: 2.0,
            tempo_sync: false,
            rate_division: ModTimeDivision::Sixteenth,
            trigger: ModRandomTrigger::Clock,
            bipolar: true,
            probability: 1.0,
            quantize: 0,
            drunk: false,
            walk: 0.25,
        }
    }
}

/// The first patch cord between modules: a source whose input is another
/// slot's output. Ordinary arithmetic against a constant, or a clamp into a
/// range; the result leaves as an ordinary source with no new routing
/// vocabulary behind it.
///
/// Modules evaluate in slot order within a control tick, so a module reading
/// a lower slot sees this tick's value and one reading itself or a higher
/// slot sees the previous tick's. That one rule is what makes feedback
/// bounded and identical between realtime and offline renders, with no cycle
/// machinery anywhere.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ModMathParams {
    /// Slot whose output is read. A slot reference today; when durable
    /// `ModSourceId` lands it resolves the same way a route's source does.
    pub input_slot: u8,
    pub op: ModMathOp,
    /// Constant right-hand side for the arithmetic and min/max operators.
    pub operand: f32,
    pub clamp_low: f32,
    pub clamp_high: f32,
}

impl Default for ModMathParams {
    fn default() -> Self {
        Self {
            input_slot: 0,
            op: ModMathOp::Multiply,
            operand: 1.0,
            clamp_low: -1.0,
            clamp_high: 1.0,
        }
    }
}

/// One modulator slot's configuration.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ModulatorParams {
    Lfo(ModLfoParams),
    Envelope(ModEnvelopeParams),
    Step(ModStepParams),
    Random(ModRandomParams),
    Math(ModMathParams),
}

impl ModulatorParams {
    pub fn kind(self) -> ModulatorKind {
        match self {
            Self::Lfo(_) => ModulatorKind::Lfo,
            Self::Envelope(_) => ModulatorKind::Envelope,
            Self::Step(_) => ModulatorKind::Step,
            Self::Random(_) => ModulatorKind::Random,
            Self::Math(_) => ModulatorKind::Math,
        }
    }

    /// One parameter as its wire value: enums as their index, booleans as
    /// 0/1, everything else natural. Mirrors `EffectParams::get`.
    pub fn get(&self, id: u32) -> Option<f32> {
        match self {
            Self::Lfo(p) => Some(match id {
                LFO_PARAM_RATE_HZ => p.rate_hz,
                LFO_PARAM_DEPTH => p.depth,
                LFO_PARAM_WAVEFORM => p.waveform.to_index() as f32,
                LFO_PARAM_PHASE => p.phase,
                LFO_PARAM_TEMPO_SYNC => f32::from(p.tempo_sync),
                LFO_PARAM_RATE_DIVISION => p.rate_division.to_index() as f32,
                LFO_PARAM_RETRIGGER => f32::from(p.retrigger),
                LFO_PARAM_FADE_IN_S => p.fade_in_seconds,
                LFO_PARAM_FADE_IN_SYNC => f32::from(p.fade_in_tempo_sync),
                LFO_PARAM_FADE_IN_DIVISION => p.fade_in_division.to_index() as f32,
                LFO_PARAM_SMOOTHING_S => p.smoothing_seconds,
                LFO_PARAM_PULSE_WIDTH => p.pulse_width,
                _ => return None,
            }),
            Self::Envelope(p) => Some(match id {
                ENV_PARAM_ATTACK_S => p.attack_seconds,
                ENV_PARAM_ATTACK_SYNC => f32::from(p.attack_tempo_sync),
                ENV_PARAM_ATTACK_DIVISION => p.attack_division.to_index() as f32,
                ENV_PARAM_DECAY_S => p.decay_seconds,
                ENV_PARAM_DECAY_SYNC => f32::from(p.decay_tempo_sync),
                ENV_PARAM_DECAY_DIVISION => p.decay_division.to_index() as f32,
                ENV_PARAM_SUSTAIN => p.sustain,
                ENV_PARAM_RELEASE_S => p.release_seconds,
                ENV_PARAM_RELEASE_SYNC => f32::from(p.release_tempo_sync),
                ENV_PARAM_RELEASE_DIVISION => p.release_division.to_index() as f32,
                ENV_PARAM_AMOUNT => p.amount,
                _ => return None,
            }),
            Self::Step(p) => Some(match id {
                STEP_PARAM_LENGTH => f32::from(p.length),
                STEP_PARAM_DIVISION => p.division.to_index() as f32,
                STEP_PARAM_GLIDE => p.glide,
                STEP_PARAM_TRIGGER => p.trigger.to_index() as f32,
                _ => p.steps[step_value_index(id)?],
            }),
            Self::Random(p) => Some(match id {
                RANDOM_PARAM_RATE_HZ => p.rate_hz,
                RANDOM_PARAM_TEMPO_SYNC => f32::from(p.tempo_sync),
                RANDOM_PARAM_RATE_DIVISION => p.rate_division.to_index() as f32,
                RANDOM_PARAM_TRIGGER => p.trigger.to_index() as f32,
                RANDOM_PARAM_BIPOLAR => f32::from(p.bipolar),
                RANDOM_PARAM_PROBABILITY => p.probability,
                RANDOM_PARAM_QUANTIZE => f32::from(p.quantize),
                RANDOM_PARAM_DRUNK => f32::from(p.drunk),
                RANDOM_PARAM_WALK => p.walk,
                _ => return None,
            }),
            Self::Math(p) => Some(match id {
                MATH_PARAM_INPUT_SLOT => f32::from(p.input_slot),
                MATH_PARAM_OP => p.op.to_index() as f32,
                MATH_PARAM_OPERAND => p.operand,
                MATH_PARAM_CLAMP_LOW => p.clamp_low,
                MATH_PARAM_CLAMP_HIGH => p.clamp_high,
                _ => return None,
            }),
        }
    }

    /// Write one parameter by wire id, clamped through its descriptor.
    /// Unknown ids are ignored, matching the effects' tolerance for stale
    /// automation. The gate input channel is a jack, not an id here.
    pub fn set(&mut self, id: u32, value: f32) {
        let Some(descriptor) = self.kind().descriptor(id) else {
            return;
        };
        let value = descriptor.clamp_natural(value);
        let index = value.round() as i32;
        match self {
            Self::Lfo(p) => match id {
                LFO_PARAM_RATE_HZ => p.rate_hz = value,
                LFO_PARAM_DEPTH => p.depth = value,
                LFO_PARAM_WAVEFORM => p.waveform = ModLfoWaveform::from_index(index),
                LFO_PARAM_PHASE => p.phase = value,
                LFO_PARAM_TEMPO_SYNC => p.tempo_sync = index != 0,
                LFO_PARAM_RATE_DIVISION => p.rate_division = ModTimeDivision::from_index(index),
                LFO_PARAM_RETRIGGER => p.retrigger = index != 0,
                LFO_PARAM_FADE_IN_S => p.fade_in_seconds = value,
                LFO_PARAM_FADE_IN_SYNC => p.fade_in_tempo_sync = index != 0,
                LFO_PARAM_FADE_IN_DIVISION => {
                    p.fade_in_division = ModTimeDivision::from_index(index)
                }
                LFO_PARAM_SMOOTHING_S => p.smoothing_seconds = value,
                LFO_PARAM_PULSE_WIDTH => p.pulse_width = value,
                _ => {}
            },
            Self::Envelope(p) => match id {
                ENV_PARAM_ATTACK_S => p.attack_seconds = value,
                ENV_PARAM_ATTACK_SYNC => p.attack_tempo_sync = index != 0,
                ENV_PARAM_ATTACK_DIVISION => p.attack_division = ModTimeDivision::from_index(index),
                ENV_PARAM_DECAY_S => p.decay_seconds = value,
                ENV_PARAM_DECAY_SYNC => p.decay_tempo_sync = index != 0,
                ENV_PARAM_DECAY_DIVISION => p.decay_division = ModTimeDivision::from_index(index),
                ENV_PARAM_SUSTAIN => p.sustain = value,
                ENV_PARAM_RELEASE_S => p.release_seconds = value,
                ENV_PARAM_RELEASE_SYNC => p.release_tempo_sync = index != 0,
                ENV_PARAM_RELEASE_DIVISION => {
                    p.release_division = ModTimeDivision::from_index(index)
                }
                ENV_PARAM_AMOUNT => p.amount = value,
                _ => {}
            },
            Self::Step(p) => match id {
                STEP_PARAM_LENGTH => p.length = index.clamp(1, MOD_STEP_MAX_STEPS as i32) as u8,
                STEP_PARAM_DIVISION => p.division = ModTimeDivision::from_index(index),
                STEP_PARAM_GLIDE => p.glide = value,
                STEP_PARAM_TRIGGER => p.trigger = ModStepTrigger::from_index(index),
                _ => {
                    if let Some(step) = step_value_index(id) {
                        p.steps[step] = value;
                    }
                }
            },
            Self::Random(p) => match id {
                RANDOM_PARAM_RATE_HZ => p.rate_hz = value,
                RANDOM_PARAM_TEMPO_SYNC => p.tempo_sync = index != 0,
                RANDOM_PARAM_RATE_DIVISION => p.rate_division = ModTimeDivision::from_index(index),
                RANDOM_PARAM_TRIGGER => p.trigger = ModRandomTrigger::from_index(index),
                RANDOM_PARAM_BIPOLAR => p.bipolar = index != 0,
                RANDOM_PARAM_PROBABILITY => p.probability = value,
                RANDOM_PARAM_QUANTIZE => p.quantize = index.clamp(0, 16) as u8,
                RANDOM_PARAM_DRUNK => p.drunk = index != 0,
                RANDOM_PARAM_WALK => p.walk = value,
                _ => {}
            },
            Self::Math(p) => match id {
                MATH_PARAM_INPUT_SLOT => {
                    p.input_slot = index.clamp(0, MAX_MODULATORS_PER_CHANNEL as i32 - 1) as u8
                }
                MATH_PARAM_OP => p.op = ModMathOp::from_index(index),
                MATH_PARAM_OPERAND => p.operand = value,
                MATH_PARAM_CLAMP_LOW => p.clamp_low = value,
                MATH_PARAM_CLAMP_HIGH => p.clamp_high = value,
                _ => {}
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModulatorKind {
    Lfo,
    Envelope,
    Step,
    Random,
    Math,
}

impl ModulatorKind {
    pub const ALL: [ModulatorKind; 5] = [
        ModulatorKind::Lfo,
        ModulatorKind::Envelope,
        ModulatorKind::Step,
        ModulatorKind::Random,
        ModulatorKind::Math,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Lfo => "LFO",
            Self::Envelope => "Envelope",
            Self::Step => "Step",
            Self::Random => "Random",
            Self::Math => "Math",
        }
    }

    /// The badge a source chip and a route label wear. Short enough to sit
    /// in a 46px tile, and the one place these abbreviations are spelled.
    pub fn badge(self) -> &'static str {
        match self {
            Self::Lfo => "LFO",
            Self::Envelope => "ENV",
            Self::Step => "STEP",
            Self::Random => "RND",
            Self::Math => "MATH",
        }
    }

    /// `ALL` position, which is the wire index the shelf's kind token uses.
    pub fn to_index(self) -> i32 {
        Self::ALL
            .iter()
            .position(|kind| *kind == self)
            .unwrap_or_default() as i32
    }

    /// The inverse, for the add menu. Out-of-range indices fail closed
    /// rather than silently adding an LFO.
    pub fn from_index(index: i32) -> Option<Self> {
        usize::try_from(index).ok().and_then(|index| Self::ALL.get(index).copied())
    }

    pub fn default_params(self) -> ModulatorParams {
        match self {
            Self::Lfo => ModulatorParams::Lfo(ModLfoParams::default()),
            Self::Envelope => ModulatorParams::Envelope(ModEnvelopeParams::default()),
            Self::Step => ModulatorParams::Step(ModStepParams::default()),
            Self::Random => ModulatorParams::Random(ModRandomParams::default()),
            Self::Math => ModulatorParams::Math(ModMathParams::default()),
        }
    }

    /// The kind's full parameter table, mirroring `EffectKind::descriptors`.
    pub fn descriptors(self) -> &'static [ParamDescriptor] {
        match self {
            Self::Lfo => &LFO_DESCRIPTORS,
            Self::Envelope => &ENVELOPE_DESCRIPTORS,
            Self::Step => &STEP_DESCRIPTORS,
            Self::Random => &RANDOM_DESCRIPTORS,
            Self::Math => &MATH_DESCRIPTORS,
        }
    }

    /// Look one parameter up by its wire id.
    pub fn descriptor(self, id: u32) -> Option<&'static ParamDescriptor> {
        self.descriptors().iter().find(|d| d.id == id)
    }
}

/// How a source's `-1..1` output is applied to a destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModPolarity {
    /// The full signed swing, centred on the base value.
    #[default]
    Bipolar,
    /// Only the positive half, so the base value is the floor.
    Unipolar,
}

/// One matrix row: a modulator slot driving one parameter.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModRoute {
    pub source_slot: u8,
    pub destination: ParamAddr,
    /// Signed, `-1..1`, as a fraction of the destination's full range. The
    /// drag depth of the assignment gesture.
    pub depth: f32,
    pub polarity: ModPolarity,
}

/// Fixed rack size. Four slots per channel, matching the rack UI and keeping
/// a channel a self-contained instrument (`MODULATION_PLAN.md`).
pub const MAX_MODULATORS_PER_CHANNEL: usize = 4;
/// Ceiling on matrix rows per channel. Bounded so evaluation is a fixed cost
/// and the whole rack stays `Copy`.
pub const MAX_MOD_ROUTES_PER_CHANNEL: usize = 16;

/// One channel's complete modulation state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModRack {
    pub slots: [Option<ModulatorParams>; MAX_MODULATORS_PER_CHANNEL],
    pub routes: [Option<ModRoute>; MAX_MOD_ROUTES_PER_CHANNEL],
}

/// The persisted form stays sparse: TOML has no `null`, so serializing the
/// fixed realtime arrays directly would either fail or write sixteen empty
/// rows. Slot numbers make an absent entry unambiguous and leave room for the
/// rack capacity to grow without changing a saved route's meaning.
#[derive(serde::Serialize, serde::Deserialize)]
struct SavedModRack {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    slots: Vec<SavedModulatorSlot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    routes: Vec<ModRoute>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedModulatorSlot {
    slot: u8,
    params: ModulatorParams,
}

impl serde::Serialize for ModRack {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let slots = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(slot, params)| {
                params.map(|params| SavedModulatorSlot {
                    slot: slot as u8,
                    params,
                })
            })
            .collect();
        let routes = self.routes.iter().flatten().copied().collect();
        SavedModRack { slots, routes }.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for ModRack {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let saved = SavedModRack::deserialize(deserializer)?;
        let mut rack = Self::default();
        for saved_slot in saved.slots {
            if let Some(slot) = rack.slots.get_mut(saved_slot.slot as usize) {
                *slot = Some(saved_slot.params);
            }
        }
        for route in saved.routes {
            let _ = rack.add_route(route);
        }
        Ok(rack)
    }
}

impl Default for ModRack {
    fn default() -> Self {
        Self {
            slots: [None; MAX_MODULATORS_PER_CHANNEL],
            routes: [None; MAX_MOD_ROUTES_PER_CHANNEL],
        }
    }
}

impl ModRack {
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }

    /// Add a route, returning its index. Returns `None` when the matrix is
    /// full rather than silently dropping the assignment.
    pub fn add_route(&mut self, route: ModRoute) -> Option<usize> {
        // An existing row for the same pair is retuned rather than doubled:
        // dragging depth on an already-assigned knob must not stack a second
        // route on top of the first.
        if let Some(index) = self.routes.iter().position(|existing| {
            existing.is_some_and(|existing| {
                existing.source_slot == route.source_slot
                    && existing.destination == route.destination
            })
        }) {
            self.routes[index] = Some(route);
            return Some(index);
        }
        let index = self.routes.iter().position(Option::is_none)?;
        self.routes[index] = Some(route);
        Some(index)
    }

    pub fn remove_route(&mut self, source_slot: u8, destination: ParamAddr) {
        for route in self.routes.iter_mut() {
            if route.is_some_and(|route| {
                route.source_slot == source_slot && route.destination == destination
            }) {
                *route = None;
            }
        }
    }

    /// Total signed offset applied to `destination`, as a fraction of its
    /// range, given each slot's current output and the destination's declared
    /// policy.
    ///
    /// The policy is the gate, not a suggestion: a destination that refuses
    /// modulation contributes nothing however many routes name it, and each
    /// route's depth is clamped into the declared limit before it sums. An
    /// illegal route is therefore inert rather than deleted -- the spec keeps
    /// it as inspectable authored work.
    pub fn offset_for(
        &self,
        destination: ParamAddr,
        outputs: &[f32; MAX_MODULATORS_PER_CHANNEL],
        policy: &ModDestinationDescriptor,
    ) -> f32 {
        if !policy.allowed {
            return 0.0;
        }
        let mut total = 0.0;
        for route in self.routes.iter().flatten() {
            if route.destination != destination {
                continue;
            }
            let Some(output) = outputs.get(route.source_slot as usize) else {
                continue;
            };
            let shaped = match route.polarity {
                ModPolarity::Bipolar => *output,
                // Half the swing, lifted, so the base value is the floor
                // rather than the midpoint.
                ModPolarity::Unipolar => (*output + 1.0) * 0.5,
            };
            total += shaped * policy.clamp_depth(route.depth);
        }
        total
    }

    /// Whether a control signal will actually resolve `destination` this
    /// block. This must agree with [`ModRack::offset_for`]: the engine uses it
    /// to decide whether to suppress a knob's base write, and a route parked
    /// on a destination that refuses modulation must not hold that knob
    /// hostage.
    pub fn modulates(&self, destination: ParamAddr, policy: &ModDestinationDescriptor) -> bool {
        policy.allowed && self.destinations().any(|address| address == destination)
    }

    /// Every destination this rack drives, for the UI's "which knobs are
    /// modulated" pass. Includes routes whose destination currently refuses
    /// modulation, because the inspector still has to show them.
    pub fn destinations(&self) -> impl Iterator<Item = ParamAddr> + '_ {
        self.routes.iter().flatten().map(|route| route.destination)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(param: u32) -> ParamAddr {
        ParamAddr::effect(EffectTarget::Channel(0), 0, param)
    }

    fn open(param: u32) -> ModDestinationDescriptor {
        ModDestinationDescriptor::unrestricted(param)
    }

    #[test]
    fn modulation_divisions_span_four_whole_notes_to_a_64th_triplet() {
        assert_eq!(ModTimeDivision::ALL.len(), 21);
        assert_eq!(ModTimeDivision::FourWhole.beats(), 16.0);
        assert_eq!(ModTimeDivision::Quarter.beats(), 1.0);
        assert_eq!(ModTimeDivision::SixtyFourthTriplet.beats(), 1.0 / 24.0);
        assert_eq!(ModTimeDivision::Quarter.rate_hz(120.0), 2.0);
        assert_eq!(ModTimeDivision::FourWhole.seconds(120.0), 8.0);
        for (index, division) in ModTimeDivision::ALL.iter().copied().enumerate() {
            assert_eq!(ModTimeDivision::from_index(index as i32), division);
            assert_eq!(division.to_index(), index as i32);
        }
    }

    #[test]
    fn legacy_lfo_params_receive_new_control_defaults() {
        let legacy = r#"
rate_hz = 3.5
depth = 0.75
waveform = "triangle"
phase = 0.25
retrigger = true
"#;
        let decoded: ModLfoParams = toml::from_str(legacy).unwrap();
        assert_eq!(decoded.rate_hz, 3.5);
        assert!(!decoded.tempo_sync);
        assert_eq!(decoded.rate_division, ModTimeDivision::Quarter);
        assert_eq!(decoded.fade_in_seconds, 0.0);
        assert_eq!(decoded.smoothing_seconds, 0.0);
        assert_eq!(decoded.pulse_width, 0.5);
    }

    /// Re-assigning the same source to the same destination retunes the
    /// existing row. Otherwise dragging depth on an assigned knob would
    /// stack routes until the matrix filled up.
    #[test]
    fn reassigning_a_pair_retunes_rather_than_stacking() {
        let mut rack = ModRack::default();
        let route = ModRoute {
            source_slot: 0,
            destination: addr(7),
            depth: 0.25,
            polarity: ModPolarity::Bipolar,
        };
        assert_eq!(rack.add_route(route), Some(0));
        assert_eq!(
            rack.add_route(ModRoute {
                depth: 0.75,
                ..route
            }),
            Some(0)
        );
        assert_eq!(rack.routes.iter().flatten().count(), 1);
        assert_eq!(rack.routes[0].unwrap().depth, 0.75);

        // A different source to the same destination is a separate row.
        assert_eq!(
            rack.add_route(ModRoute {
                source_slot: 1,
                ..route
            }),
            Some(1)
        );
        assert_eq!(rack.routes.iter().flatten().count(), 2);
    }

    #[test]
    fn a_full_matrix_refuses_rather_than_dropping_silently() {
        let mut rack = ModRack::default();
        for param in 0..MAX_MOD_ROUTES_PER_CHANNEL as u32 {
            assert!(rack
                .add_route(ModRoute {
                    source_slot: 0,
                    destination: addr(param),
                    depth: 1.0,
                    polarity: ModPolarity::Bipolar,
                })
                .is_some());
        }
        assert_eq!(
            rack.add_route(ModRoute {
                source_slot: 0,
                destination: addr(999),
                depth: 1.0,
                polarity: ModPolarity::Bipolar,
            }),
            None
        );
    }

    /// Offsets from several sources sum, and polarity decides whether the
    /// base value sits at the centre of the swing or at its floor.
    #[test]
    fn offsets_sum_and_polarity_shapes_the_swing() {
        let mut rack = ModRack::default();
        rack.add_route(ModRoute {
            source_slot: 0,
            destination: addr(1),
            depth: 0.5,
            polarity: ModPolarity::Bipolar,
        });
        rack.add_route(ModRoute {
            source_slot: 1,
            destination: addr(1),
            depth: 1.0,
            polarity: ModPolarity::Unipolar,
        });
        let mut outputs = [0.0; MAX_MODULATORS_PER_CHANNEL];

        // Both sources at full negative: bipolar swings down, unipolar rests
        // on the base value.
        outputs[0] = -1.0;
        outputs[1] = -1.0;
        assert_eq!(rack.offset_for(addr(1), &outputs, &open(1)), -0.5);

        // Both at full positive.
        outputs[0] = 1.0;
        outputs[1] = 1.0;
        assert_eq!(rack.offset_for(addr(1), &outputs, &open(1)), 1.5);

        // An unrelated destination is untouched.
        assert_eq!(rack.offset_for(addr(2), &outputs, &open(2)), 0.0);
    }

    /// The destination's declaration is the gate. A route parked on a
    /// parameter that refuses modulation resolves to nothing and does not
    /// count as modulating it -- otherwise it would hold that knob hostage,
    /// suppressing the base write while contributing no movement. A narrowed
    /// depth limit clamps the route rather than trusting the stored depth.
    #[test]
    fn the_destination_policy_gates_and_clamps_its_routes() {
        let mut rack = ModRack::default();
        rack.add_route(ModRoute {
            source_slot: 0,
            destination: addr(1),
            depth: 1.0,
            polarity: ModPolarity::Bipolar,
        });
        let mut outputs = [0.0; MAX_MODULATORS_PER_CHANNEL];
        outputs[0] = 1.0;

        let refused = ModDestinationDescriptor {
            allowed: false,
            ..open(1)
        };
        assert_eq!(rack.offset_for(addr(1), &outputs, &refused), 0.0);
        assert!(!rack.modulates(addr(1), &refused));
        // The route is still authored work: the inspector must be able to
        // show it, so it stays in the rack's destination list.
        assert!(rack.destinations().any(|address| address == addr(1)));

        let narrowed = ModDestinationDescriptor {
            depth_limit: (-0.2, 0.2),
            ..open(1)
        };
        assert_eq!(rack.offset_for(addr(1), &outputs, &narrowed), 0.2);
        assert!(rack.modulates(addr(1), &narrowed));
    }

    /// The strip's own controls are described destinations like any device's,
    /// which is what lets a source reach a fader without the mixer growing a
    /// modulation special case.
    #[test]
    fn strip_parameters_are_ordinary_modulation_destinations() {
        let volume = strip_descriptor(STRIP_PARAM_VOLUME).unwrap();
        let pan = strip_descriptor(STRIP_PARAM_PAN).unwrap();
        assert!(ModDestinationDescriptor::for_param(volume).allowed);
        assert!(ModDestinationDescriptor::for_param(pan).allowed);
        assert_eq!(volume.from_normalized(0.0), 0.0);
        assert_eq!(volume.from_normalized(1.0), MAX_LINEAR_GAIN);
        // Centre pan sits at the middle of the normalized range, so a bipolar
        // route swings evenly to both sides of it.
        assert_eq!(pan.to_normalized(0.0), 0.5);
        assert_eq!(
            ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_PAN).owner,
            ParamOwner::Strip
        );
    }

    #[test]
    fn removing_a_route_leaves_its_neighbours() {
        let mut rack = ModRack::default();
        for slot in 0..2u8 {
            rack.add_route(ModRoute {
                source_slot: slot,
                destination: addr(1),
                depth: 1.0,
                polarity: ModPolarity::Bipolar,
            });
        }
        rack.remove_route(0, addr(1));
        let remaining: Vec<_> = rack.routes.iter().flatten().collect();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].source_slot, 1);
    }

    #[test]
    fn sparse_rack_round_trips_through_toml() {
        let mut rack = ModRack::default();
        rack.slots[2] = Some(ModulatorParams::Lfo(ModLfoParams {
            rate_hz: 3.5,
            ..ModLfoParams::default()
        }));
        rack.slots[1] = Some(ModulatorParams::Envelope(ModEnvelopeParams {
            input_channel: 3,
            attack_tempo_sync: true,
            sustain: 0.42,
            ..ModEnvelopeParams::default()
        }));
        rack.add_route(ModRoute {
            source_slot: 2,
            destination: addr(4),
            depth: -0.75,
            polarity: ModPolarity::Unipolar,
        });

        let text = toml::to_string(&rack).unwrap();
        assert!(text.contains("slot = 2"));
        assert!(!text.contains("null"));
        assert_eq!(toml::from_str::<ModRack>(&text).unwrap(), rack);
    }

    /// Every modulator kind's table has unique ids, and a default params
    /// struct reads back exactly the descriptor defaults — the two sources
    /// of "default" can never drift apart unnoticed.
    #[test]
    fn modulator_descriptor_defaults_match_default_params() {
        for kind in ModulatorKind::ALL {
            let params = kind.default_params();
            let descriptors = kind.descriptors();
            for (index, descriptor) in descriptors.iter().enumerate() {
                assert!(
                    descriptors[..index].iter().all(|d| d.id != descriptor.id),
                    "{kind:?} repeats id {}",
                    descriptor.id
                );
                assert_eq!(
                    params.get(descriptor.id),
                    Some(descriptor.default),
                    "{kind:?} id {} default",
                    descriptor.id
                );
            }
        }
    }

    /// `set(id, get(id))` is identity across every table, and a written
    /// value survives the wire round trip. This is the contract the whole
    /// generic editor stands on.
    #[test]
    fn modulator_params_round_trip_by_id() {
        for kind in ModulatorKind::ALL {
            let mut params = kind.default_params();
            for descriptor in kind.descriptors() {
                let before = params;
                params.set(descriptor.id, params.get(descriptor.id).unwrap());
                assert_eq!(params, before, "{kind:?} id {} identity", descriptor.id);

                // A mid-range write reads back as written (snapped when
                // stepped, which get/set both express in index space).
                let target = descriptor.clamp_natural(descriptor.min * 0.25 + descriptor.max * 0.75);
                params.set(descriptor.id, target);
                assert_eq!(
                    params.get(descriptor.id),
                    Some(target),
                    "{kind:?} id {} write",
                    descriptor.id
                );
            }
        }
    }

    /// Wire values decode into the typed fields: enum indices land on the
    /// enum, booleans on the flag, and out-of-range writes clamp through
    /// the descriptor instead of poisoning the struct.
    #[test]
    fn modulator_set_decodes_enums_and_clamps() {
        let mut params = ModulatorParams::Lfo(ModLfoParams::default());
        params.set(LFO_PARAM_WAVEFORM, 3.0);
        params.set(LFO_PARAM_TEMPO_SYNC, 1.0);
        params.set(LFO_PARAM_RATE_DIVISION, ModTimeDivision::Eighth.to_index() as f32);
        params.set(LFO_PARAM_RATE_HZ, 999.0);
        params.set(LFO_PARAM_PULSE_WIDTH, -4.0);
        let ModulatorParams::Lfo(lfo) = params else {
            unreachable!()
        };
        assert_eq!(lfo.waveform, ModLfoWaveform::Square);
        assert!(lfo.tempo_sync);
        assert_eq!(lfo.rate_division, ModTimeDivision::Eighth);
        assert_eq!(lfo.rate_hz, 20.0);
        assert_eq!(lfo.pulse_width, 0.01);

        // Unknown ids are ignored, and one kind's ids do not bleed into
        // another kind's fields.
        let mut envelope = ModulatorParams::Envelope(ModEnvelopeParams::default());
        let before = envelope;
        envelope.set(9_999, 1.0);
        assert_eq!(envelope, before);
        assert_eq!(envelope.get(9_999), None);
    }

    /// The sixteen step values are one contiguous id block and nothing
    /// outside it, so the editor can walk the bank by offset and a stale
    /// automation id past the end is ignored rather than aliasing step 1.
    #[test]
    fn the_step_value_block_is_contiguous_and_bounded() {
        assert_eq!(step_value_index(STEP_PARAM_VALUE_BASE), Some(0));
        assert_eq!(
            step_value_index(STEP_PARAM_VALUE_BASE + MOD_STEP_MAX_STEPS as u32 - 1),
            Some(MOD_STEP_MAX_STEPS - 1)
        );
        assert_eq!(
            step_value_index(STEP_PARAM_VALUE_BASE + MOD_STEP_MAX_STEPS as u32),
            None
        );
        for scalar in [
            STEP_PARAM_LENGTH,
            STEP_PARAM_DIVISION,
            STEP_PARAM_GLIDE,
            STEP_PARAM_TRIGGER,
        ] {
            assert_eq!(step_value_index(scalar), None, "scalar {scalar} aliased");
        }

        let mut params = ModulatorParams::Step(ModStepParams::default());
        params.set(STEP_PARAM_VALUE_BASE + 5, -0.75);
        params.set(STEP_PARAM_VALUE_BASE + MOD_STEP_MAX_STEPS as u32, 1.0);
        let ModulatorParams::Step(step) = params else {
            unreachable!()
        };
        assert_eq!(step.steps[5], -0.75);
        assert!(step.steps.iter().filter(|value| **value != 0.0).count() == 1);
    }

    /// Every new kind decodes its enums and clamps out-of-range writes, and
    /// none of them accepts a neighbouring kind's structural id by accident.
    #[test]
    fn the_new_kinds_decode_their_enums_and_clamp() {
        let mut random = ModulatorParams::Random(ModRandomParams::default());
        random.set(RANDOM_PARAM_TRIGGER, 1.0);
        random.set(RANDOM_PARAM_BIPOLAR, 0.0);
        random.set(RANDOM_PARAM_PROBABILITY, 4.0);
        random.set(RANDOM_PARAM_QUANTIZE, 99.0);
        random.set(RANDOM_PARAM_WALK, -3.0);
        let ModulatorParams::Random(p) = random else {
            unreachable!()
        };
        assert_eq!(p.trigger, ModRandomTrigger::NoteTrigger);
        assert!(!p.bipolar);
        assert_eq!(p.probability, 1.0);
        assert_eq!(p.quantize, 16);
        assert_eq!(p.walk, 0.01);

        let mut math = ModulatorParams::Math(ModMathParams::default());
        math.set(MATH_PARAM_OP, 6.0);
        math.set(MATH_PARAM_INPUT_SLOT, 99.0);
        math.set(MATH_PARAM_OPERAND, 40.0);
        let ModulatorParams::Math(p) = math else {
            unreachable!()
        };
        assert_eq!(p.op, ModMathOp::Clamp);
        assert_eq!(p.input_slot, MAX_MODULATORS_PER_CHANNEL as u8 - 1);
        assert_eq!(p.operand, 4.0);
    }

    /// A rack of new kinds persists sparse and reloads identically, and the
    /// tagged format leaves an LFO-only project byte-identical to what a
    /// build without these kinds wrote.
    #[test]
    fn new_kinds_round_trip_through_toml() {
        let mut rack = ModRack::default();
        let mut steps = [0.0; MOD_STEP_MAX_STEPS];
        steps[0] = 1.0;
        steps[3] = -0.5;
        rack.slots[0] = Some(ModulatorParams::Step(ModStepParams {
            steps,
            length: 4,
            division: ModTimeDivision::Eighth,
            glide: 0.3,
            trigger: ModStepTrigger::NoteAdvance,
        }));
        rack.slots[1] = Some(ModulatorParams::Random(ModRandomParams {
            drunk: true,
            quantize: 5,
            ..ModRandomParams::default()
        }));
        rack.slots[3] = Some(ModulatorParams::Math(ModMathParams {
            input_slot: 1,
            op: ModMathOp::Clamp,
            ..ModMathParams::default()
        }));

        let text = toml::to_string(&rack).unwrap();
        assert!(text.contains("slot = 3"));
        assert_eq!(toml::from_str::<ModRack>(&text).unwrap(), rack);

        // A project written before these kinds existed still decodes: the
        // variant tag is the only thing that grew.
        let legacy = r#"
[[slots]]
slot = 0

[slots.params]
kind = "lfo"
rate_hz = 2.0
"#;
        let decoded = toml::from_str::<ModRack>(legacy).unwrap();
        assert_eq!(decoded.slots[0].unwrap().kind(), ModulatorKind::Lfo);
        assert_eq!(decoded.slots[0].unwrap().get(LFO_PARAM_RATE_HZ), Some(2.0));
    }
}
