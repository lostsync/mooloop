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

/// One modulator slot's configuration.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ModulatorParams {
    Lfo(ModLfoParams),
    Envelope(ModEnvelopeParams),
}

impl ModulatorParams {
    pub fn kind(self) -> ModulatorKind {
        match self {
            Self::Lfo(_) => ModulatorKind::Lfo,
            Self::Envelope(_) => ModulatorKind::Envelope,
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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModulatorKind {
    Lfo,
    Envelope,
}

impl ModulatorKind {
    pub const ALL: [ModulatorKind; 2] = [ModulatorKind::Lfo, ModulatorKind::Envelope];

    pub fn label(self) -> &'static str {
        match self {
            Self::Lfo => "LFO",
            Self::Envelope => "Envelope",
        }
    }

    pub fn default_params(self) -> ModulatorParams {
        match self {
            Self::Lfo => ModulatorParams::Lfo(ModLfoParams::default()),
            Self::Envelope => ModulatorParams::Envelope(ModEnvelopeParams::default()),
        }
    }

    /// The kind's full parameter table, mirroring `EffectKind::descriptors`.
    pub fn descriptors(self) -> &'static [ParamDescriptor] {
        match self {
            Self::Lfo => &LFO_DESCRIPTORS,
            Self::Envelope => &ENVELOPE_DESCRIPTORS,
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
}
