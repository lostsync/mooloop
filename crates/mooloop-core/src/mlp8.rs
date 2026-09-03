//! Parameters for the ML-P8.
//!
//! A new eight-voice instrument, not a rename or a migration of
//! [`crate::PolySynthParams`], per `docs/plans/poly-synth-v2/`. The v1 poly
//! synth keeps its kind, its ids, and its saved projects; this device starts
//! from an empty namespace.
//!
//! What makes it a different instrument rather than a wider one is that the
//! three oscillators are a *network*: every directed pair cross-modulates,
//! each oscillator phase-modulates itself, noise reaches the same phase
//! inputs, and any oscillator can hard-sync any other. An oscillator muted in
//! the source mix is still a modulator, so Level is a mixer control and not an
//! on switch.
//!
//! ## Its own parameter id space
//!
//! The v1 synths share `generator.rs`'s `SYNTH_PARAM_*` ids and the `100 + n *
//! 10` oscillator blocks, because they are the same voice with a different
//! count. ML-P8 does not: it is a separate kind whose table starts at zero and
//! only ever appends, so its ids and its descriptor table live here beside the
//! struct they describe rather than in the shared table. Ids are persisted by
//! automation and modulation routes, so nothing below is ever renumbered.
//!
//! Because this struct has no form on disk yet, it carries `#[serde(default)]`
//! from the start rather than acquiring it after the first field addition
//! breaks every saved project.

use crate::generator::{seconds, stepped, unit};
use crate::modulation::ModTimeDivision;
use crate::{OscParams, OscWave, ParamCurve, ParamDescriptor};

/// Physical voice slots. Not a knob: "eight voices" is the instrument's name
/// and its CPU ceiling, and Unison spends these slots rather than adding to
/// them. See `docs/plans/poly-synth-v2/01-what-poly-is.md`.
pub const MLP8_VOICES: usize = 8;

/// Which oscillator hard-syncs this one. `Off` is not "no oscillator" — it is
/// the absence of a sync edge, which is why this is a four-position selector
/// and not an optional index.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncSource {
    #[default]
    Off,
    Osc1,
    Osc2,
    Osc3,
}

impl SyncSource {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Osc1,
            2 => Self::Osc2,
            3 => Self::Osc3,
            _ => Self::Off,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Off => 0,
            Self::Osc1 => 1,
            Self::Osc2 => 2,
            Self::Osc3 => 3,
        }
    }

    /// The master's oscillator index, or `None` when this oscillator free-runs.
    pub fn master(self) -> Option<usize> {
        match self {
            Self::Off => None,
            Self::Osc1 => Some(0),
            Self::Osc2 => Some(1),
            Self::Osc3 => Some(2),
        }
    }
}

/// Which oscillator the sub divides. Sub is derived, not a fourth oscillator,
/// so it names a source instead of carrying its own tuning.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubSource {
    #[default]
    Osc1,
    Osc2,
    Osc3,
}

impl SubSource {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Osc2,
            2 => Self::Osc3,
            _ => Self::Osc1,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Osc1 => 0,
            Self::Osc2 => 1,
            Self::Osc3 => 2,
        }
    }

    pub fn index(self) -> usize {
        self.to_index() as usize
    }
}

/// How far below its source the sub sits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubOctave {
    #[default]
    Minus1,
    Minus2,
}

impl SubOctave {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Minus2,
            _ => Self::Minus1,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Minus1 => 0,
            Self::Minus2 => 1,
        }
    }

    /// Frequency divisor applied to the source oscillator's base pitch.
    pub fn divisor(self) -> f32 {
        match self {
            Self::Minus1 => 2.0,
            Self::Minus2 => 4.0,
        }
    }
}

/// Sub waveform. Two shapes, because a sub is a fundamental and not a third
/// place to program a timbre: sine disappears under the mix, square holds a
/// floor up against a mangled carrier.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubWave {
    #[default]
    Sine,
    Square,
}

impl SubWave {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Square,
            _ => Self::Sine,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Sine => 0,
            Self::Square => 1,
        }
    }
}

/// Flat index of the directed cross-modulation route `from -> to`.
///
/// Six routes, laid out `1>2, 1>3, 2>1, 2>3, 3>1, 3>2` so the parameter ids
/// below are contiguous and read in the order the panel does. `from == to` is
/// not a route: an oscillator modulating itself is the separate Feedback
/// control, which exists so a patch can say "this oscillator is unstable"
/// without spending one of the six pair amounts on it.
pub const fn xmod_index(from: usize, to: usize) -> usize {
    let offset = if to > from { to - 1 } else { to };
    from * 2 + offset
}

// --- Parameter ids ---------------------------------------------------------
//
// Append-only. The bands are the reservations reviewed in the plan:
//
//   0-14   three five-control oscillator blocks
//   15-19  amplitude ADSR and Glide
//   20-24  sub (24 reserved for expansion)
//   25-26  noise
//   27-32  six directed XMOD amounts
//   33-35  noise-to-oscillator amounts
//   36-38  oscillator self-feedback
//   39-41  sync-source selectors
//
// 42 onward belongs to later steps of the plan and is not used here.

/// First id of oscillator `n`'s five-control block.
pub const fn osc_param(oscillator: u32, offset: u32) -> u32 {
    oscillator * 5 + offset
}

pub const OSC_OFFSET_WAVE: u32 = 0;
pub const OSC_OFFSET_SEMITONES: u32 = 1;
pub const OSC_OFFSET_CENTS: u32 = 2;
pub const OSC_OFFSET_LEVEL: u32 = 3;
pub const OSC_OFFSET_PULSE_WIDTH: u32 = 4;

pub const PARAM_ATTACK: u32 = 15;
pub const PARAM_DECAY: u32 = 16;
pub const PARAM_SUSTAIN: u32 = 17;
pub const PARAM_RELEASE: u32 = 18;
pub const PARAM_GLIDE: u32 = 19;

pub const PARAM_SUB_LEVEL: u32 = 20;
pub const PARAM_SUB_OCTAVE: u32 = 21;
pub const PARAM_SUB_WAVE: u32 = 22;
pub const PARAM_SUB_SOURCE: u32 = 23;
// 24 is reserved for a fifth sub control.

pub const PARAM_NOISE_LEVEL: u32 = 25;
pub const PARAM_NOISE_COLOR: u32 = 26;

/// First of the six directed XMOD amounts; add [`xmod_index`].
pub const PARAM_XMOD_BASE: u32 = 27;
/// First of the three noise-to-oscillator amounts; add the oscillator index.
pub const PARAM_NOISE_TO_OSC_BASE: u32 = 33;
/// First of the three oscillator self-feedback amounts.
pub const PARAM_OSC_FEEDBACK_BASE: u32 = 36;
/// First of the three sync-source selectors.
pub const PARAM_SYNC_SOURCE_BASE: u32 = 39;

// --- Step 03: the voice's filter, its envelope, and its feedback loop ------

pub const PARAM_FILTER_MODE: u32 = 42;
pub const PARAM_FILTER_CUTOFF: u32 = 43;
pub const PARAM_FILTER_RESONANCE: u32 = 44;
pub const PARAM_FILTER_ENV_AMOUNT: u32 = 45;
pub const PARAM_DRIVE: u32 = 46;
pub const PARAM_KEYTRACK: u32 = 47;
pub const PARAM_FILTER_ATTACK: u32 = 48;
pub const PARAM_FILTER_DECAY: u32 = 49;
pub const PARAM_FILTER_SUSTAIN: u32 = 50;
pub const PARAM_FILTER_RELEASE: u32 = 51;
pub const PARAM_AMP_VELOCITY: u32 = 52;
pub const PARAM_FILTER_VELOCITY: u32 = 53;
pub const PARAM_VOICE_FEEDBACK: u32 = 54;

// --- Step 04: the device's own LFO ----------------------------------------

pub const PARAM_LFO_WAVE: u32 = 55;
pub const PARAM_LFO_SYNC: u32 = 56;
pub const PARAM_LFO_RATE_HZ: u32 = 57;
pub const PARAM_LFO_RATE_DIVISION: u32 = 58;
pub const PARAM_LFO_PHASE: u32 = 59;
pub const PARAM_LFO_WARP: u32 = 60;
pub const PARAM_LFO_SLEW: u32 = 61;
pub const PARAM_LFO_RETRIGGER: u32 = 62;
// 63 closes the band the plan reserved for the LFO and is deliberately
// unused, like 24. A reservation spent early is a renumbering later.

/// Which response the multimode filter runs.
///
/// All four come off the same shared state-variable stage; this is a response
/// menu rather than the ML-M1's character menu, where three low-passes differ
/// in slope and saturation. Two devices, two different questions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MlP8FilterMode {
    #[default]
    Lp12,
    Lp24,
    Bp12,
    Hp12,
}

impl MlP8FilterMode {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Lp24,
            2 => Self::Bp12,
            3 => Self::Hp12,
            _ => Self::Lp12,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Lp12 => 0,
            Self::Lp24 => 1,
            Self::Bp12 => 2,
            Self::Hp12 => 3,
        }
    }
}

/// The shape of ML-P8's own LFO.
///
/// Not [`crate::modulation::ModLfoWaveform`], which is the channel rack's
/// five-shape list. The overlap is real but the lists answer different
/// questions: this one names `Ramp` and `Pulse` because [`MlP8LfoParams::warp`]
/// is what makes them adjustable, and it carries `Chaos`, which has no
/// meaning without the per-sample evaluation an instrument LFO gets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MlP8LfoWave {
    #[default]
    Sine,
    Triangle,
    Ramp,
    Pulse,
    /// One held value per cycle.
    SampleHold,
    /// A bounded aperiodic wander. Deterministic, and not a renamed
    /// [`Self::SampleHold`]: it never holds still.
    Chaos,
}

impl MlP8LfoWave {
    pub const ALL: [Self; 6] = [
        Self::Sine,
        Self::Triangle,
        Self::Ramp,
        Self::Pulse,
        Self::SampleHold,
        Self::Chaos,
    ];

    /// Whether this shape has a phase for [`MlP8LfoParams::warp`] to skew.
    ///
    /// The two that do not are the two that are not periodic, so warp reads
    /// as a distribution bias there instead. One control, two honest
    /// meanings, decided by the wave rather than by a second knob.
    pub fn is_periodic(self) -> bool {
        !matches!(self, Self::SampleHold | Self::Chaos)
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
            .position(|wave| *wave == self)
            .unwrap_or_default() as i32
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Sine => "Sine",
            Self::Triangle => "Tri",
            Self::Ramp => "Ramp",
            Self::Pulse => "Pulse",
            Self::SampleHold => "S&H",
            Self::Chaos => "Chaos",
        }
    }
}

/// When the LFO restarts its cycle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MlP8LfoRetrigger {
    /// Never. The LFO runs with the transport and every voice reads the same
    /// place in the cycle.
    #[default]
    Free,
    /// On a note-on that arrives while nothing is held, so a chord starts the
    /// cycle once and notes added to it do not.
    Chord,
    /// On every note-on, including notes added to a held chord. This moves
    /// modulation on the notes already sounding; the status text says so.
    Note,
}

impl MlP8LfoRetrigger {
    pub const ALL: [Self; 3] = [Self::Free, Self::Chord, Self::Note];

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
            Self::Free => "Free",
            Self::Chord => "Chord",
            Self::Note => "Note",
        }
    }
}

/// ML-P8's own LFO: one global shape, read per sample.
///
/// Global rather than per voice because it is the instrument's clock, and the
/// route amounts in step 04 are what make it land differently on each voice.
/// A per-voice LFO would be a different feature and would need its own
/// retrigger vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct MlP8LfoParams {
    pub wave: MlP8LfoWave,
    /// Take the rate from [`Self::rate_division`] and the transport instead of
    /// from [`Self::rate_hz`].
    pub synced: bool,
    /// Free-running rate in Hz.
    pub rate_hz: f32,
    /// Synced rate, on the same musical grid every other synced control in
    /// the project uses.
    pub rate_division: ModTimeDivision,
    /// Read offset in cycles, `[0, 1]`. Applied where the shape is read, not
    /// to the accumulator, so turning it never changes the rate — the same
    /// choice the oscillators make for phase modulation.
    pub phase: f32,
    /// Bipolar shape skew in `[-1, 1]`. Phase asymmetry for the periodic
    /// waves, distribution bias for the other two.
    pub warp: f32,
    /// Rounding, in `[0, 1]`, as a fraction of the cycle rather than a fixed
    /// time, so a shape keeps its character when the rate changes.
    pub slew: f32,
    pub retrigger: MlP8LfoRetrigger,
}

impl Default for MlP8LfoParams {
    fn default() -> Self {
        Self {
            wave: MlP8LfoWave::Sine,
            synced: false,
            rate_hz: 2.0,
            rate_division: ModTimeDivision::Quarter,
            phase: 0.0,
            warp: 0.0,
            slew: 0.0,
            retrigger: MlP8LfoRetrigger::Free,
        }
    }
}

/// The LFO's free rate reaches audio frequencies on purpose: it is evaluated
/// per sample, and a route from it to a pitch or an XMOD amount at 30 Hz is a
/// different sound rather than a faster wobble. It is not band limited, so
/// the top is where that stops being musical rather than where it stops
/// working.
const LFO_RATE_MIN_HZ: f32 = 0.01;
const LFO_RATE_MAX_HZ: f32 = 100.0;

const LFO_DESCRIPTORS: [ParamDescriptor; 8] = [
    stepped(
        PARAM_LFO_WAVE,
        "LFO wave",
        MlP8LfoWave::ALL.len() as u8,
        0.0,
    ),
    stepped(PARAM_LFO_SYNC, "LFO sync", 2, 0.0),
    ParamDescriptor {
        id: PARAM_LFO_RATE_HZ,
        name: "LFO rate",
        unit: "Hz",
        min: LFO_RATE_MIN_HZ,
        max: LFO_RATE_MAX_HZ,
        curve: ParamCurve::Exponential,
        default: 2.0,
    },
    ParamDescriptor {
        id: PARAM_LFO_RATE_DIVISION,
        name: "LFO div",
        unit: "",
        min: 0.0,
        max: (ModTimeDivision::ALL.len() - 1) as f32,
        curve: ParamCurve::Stepped(ModTimeDivision::ALL.len() as u8),
        default: 7.0,
    },
    unit(PARAM_LFO_PHASE, "LFO phase", 0.0),
    bipolar(PARAM_LFO_WARP, "LFO warp"),
    unit(PARAM_LFO_SLEW, "LFO slew", 0.0),
    stepped(
        PARAM_LFO_RETRIGGER,
        "LFO trig",
        MlP8LfoRetrigger::ALL.len() as u8,
        0.0,
    ),
];

/// This device's descriptor for `id`, if it has one.
pub fn descriptor(id: u32) -> Option<&'static ParamDescriptor> {
    DESCRIPTORS.iter().find(|descriptor| descriptor.id == id)
}

/// What an internal route reads.
///
/// Deliberately short, and deliberately without oscillator or noise signals:
/// those already reach each other at audio rate through the XMOD network, and
/// offering them here as slow control values would be a misleading second
/// kind of FM. `Trigger` is absent for the opposite reason — it is a moment,
/// not a value, so it belongs on a reset inlet rather than in a list of
/// things sampled every sample.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MlP8ModSource {
    /// The device's own LFO. One global value, bipolar.
    #[default]
    Lfo,
    /// This voice's amplitude envelope, unipolar.
    AmpEnv,
    /// This voice's filter envelope, unipolar.
    FilterEnv,
    /// How hard this voice's note was played, unipolar.
    Velocity,
    /// This voice's pitch, bipolar about middle C.
    Key,
    /// High while this voice's note is held, low once it is released.
    Gate,
}

impl MlP8ModSource {
    pub const ALL: [Self; 6] = [
        Self::Lfo,
        Self::AmpEnv,
        Self::FilterEnv,
        Self::Velocity,
        Self::Key,
        Self::Gate,
    ];

    /// Whether this source swings both ways about zero. Unipolar sources rest
    /// at zero and only ever add in the direction the amount's sign chooses.
    pub fn is_bipolar(self) -> bool {
        matches!(self, Self::Lfo | Self::Key)
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
            Self::Lfo => "LFO",
            Self::AmpEnv => "Amp Env",
            Self::FilterEnv => "Filt Env",
            Self::Velocity => "Velocity",
            Self::Key => "Key",
            Self::Gate => "Gate",
        }
    }
}

/// What an internal route moves.
///
/// Almost every destination is an ordinary authored parameter, addressed by
/// the same descriptor id automation uses, so "base plus offset, then clamp
/// through the descriptor" needs no second table to stay honest. The two that
/// are not are the voice's own output stage, which has no knob because the
/// channel strip already owns the device's level and position — per *voice*
/// is a different thing from per device, and only a route can ask for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum MlP8ModDest {
    /// An authored parameter, by descriptor id.
    Param { id: u32 },
    /// This voice's output gain, offset down from unity.
    VcaLevel,
    /// This voice's position in the stereo field, offset from centre.
    Pan,
}

/// Width of a voice's per-sample offset table, which is one entry per
/// destination. Named so the audio path can size a fixed array against the
/// same number [`MlP8ModDest::slot`] indexes into.
pub const MLP8_MOD_DESTS: usize = 31;

impl MlP8ModDest {
    /// Every legal destination, in the order the panel lists them.
    pub const ALL: [Self; MLP8_MOD_DESTS] = [
        Self::Param { id: osc_param(0, OSC_OFFSET_SEMITONES) },
        Self::Param { id: osc_param(1, OSC_OFFSET_SEMITONES) },
        Self::Param { id: osc_param(2, OSC_OFFSET_SEMITONES) },
        Self::Param { id: osc_param(0, OSC_OFFSET_PULSE_WIDTH) },
        Self::Param { id: osc_param(1, OSC_OFFSET_PULSE_WIDTH) },
        Self::Param { id: osc_param(2, OSC_OFFSET_PULSE_WIDTH) },
        Self::Param { id: osc_param(0, OSC_OFFSET_LEVEL) },
        Self::Param { id: osc_param(1, OSC_OFFSET_LEVEL) },
        Self::Param { id: osc_param(2, OSC_OFFSET_LEVEL) },
        Self::Param { id: PARAM_SUB_LEVEL },
        Self::Param { id: PARAM_NOISE_LEVEL },
        Self::Param { id: PARAM_NOISE_COLOR },
        Self::Param { id: PARAM_XMOD_BASE },
        Self::Param { id: PARAM_XMOD_BASE + 1 },
        Self::Param { id: PARAM_XMOD_BASE + 2 },
        Self::Param { id: PARAM_XMOD_BASE + 3 },
        Self::Param { id: PARAM_XMOD_BASE + 4 },
        Self::Param { id: PARAM_XMOD_BASE + 5 },
        Self::Param { id: PARAM_NOISE_TO_OSC_BASE },
        Self::Param { id: PARAM_NOISE_TO_OSC_BASE + 1 },
        Self::Param { id: PARAM_NOISE_TO_OSC_BASE + 2 },
        Self::Param { id: PARAM_OSC_FEEDBACK_BASE },
        Self::Param { id: PARAM_OSC_FEEDBACK_BASE + 1 },
        Self::Param { id: PARAM_OSC_FEEDBACK_BASE + 2 },
        Self::Param { id: PARAM_VOICE_FEEDBACK },
        Self::Param { id: PARAM_FILTER_CUTOFF },
        Self::Param { id: PARAM_FILTER_RESONANCE },
        Self::Param { id: PARAM_FILTER_ENV_AMOUNT },
        Self::Param { id: PARAM_DRIVE },
        Self::VcaLevel,
        Self::Pan,
    ];

    /// Dense index into a voice's per-sample offset table.
    ///
    /// The audio path adds offsets into a flat array and reads them back by
    /// this index, so nothing in `process()` searches a descriptor table or
    /// matches on a parameter id.
    pub fn slot(self) -> Option<usize> {
        Self::ALL.iter().position(|dest| *dest == self)
    }

    /// `==`, but usable in a `const` context so the DSP can resolve its slot
    /// indices at compile time instead of searching for them per sample.
    pub const fn same(self, other: Self) -> bool {
        match (self, other) {
            (Self::Param { id: a }, Self::Param { id: b }) => a == b,
            (Self::VcaLevel, Self::VcaLevel) | (Self::Pan, Self::Pan) => true,
            _ => false,
        }
    }

    /// Whether this destination may be routed to at all.
    ///
    /// The rule is the descriptor's own curve rather than a hand-kept list:
    /// a stepped parameter is a structural choice — a waveform, a sync
    /// source, a filter mode — and flapping one at audio rate is a click, not
    /// a modulation. Keeping the rule here means a stepped control added
    /// later is excluded the day it is added.
    pub fn is_legal(self) -> bool {
        match self {
            Self::Param { id } => match descriptor(id) {
                Some(d) => !matches!(d.curve, ParamCurve::Stepped(_)),
                None => false,
            },
            Self::VcaLevel | Self::Pan => true,
        }
    }

    /// The span a route amount of 1.0 covers.
    pub fn full_range(self) -> f32 {
        match self {
            Self::Param { id } => descriptor(id).map(|d| d.max - d.min).unwrap_or(0.0),
            // Unity down to silence. A route can duck a voice but not push it
            // past the reference the gain contract puts at the top.
            Self::VcaLevel => 1.0,
            // Hard left to hard right.
            Self::Pan => 2.0,
        }
    }

    /// The span a resolved base-plus-offset is clamped into.
    ///
    /// The same descriptor mapping the authored knob obeys, so a route can
    /// only ever move a destination somewhere the knob could also have been
    /// put. Paired with [`Self::full_range`], which is its width.
    pub fn range(self) -> (f32, f32) {
        match self {
            Self::Param { id } => match descriptor(id) {
                Some(d) => (d.min, d.max),
                None => (0.0, 0.0),
            },
            Self::VcaLevel => (0.0, 1.0),
            Self::Pan => (-1.0, 1.0),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Param { id } => match descriptor(id) {
                Some(d) => d.name,
                None => "?",
            },
            Self::VcaLevel => "Voice level",
            Self::Pan => "Voice pan",
        }
    }
}

/// The most internal routes one ML-P8 patch may have active.
///
/// A measured ceiling on callback work, not a panel with sixteen empty slots
/// and not a promise. The UI shows the routes a patch actually has, plus an
/// add affordance that stops offering itself here.
pub const MLP8_MAX_ROUTES: usize = 16;

/// One internal modulation route.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MlP8Route {
    /// Durable identity, handed out once and never reused within a patch.
    /// Automation addresses [`Self::amount`] through this, so reordering or
    /// removing a neighbour must not move it.
    pub id: u16,
    pub source: MlP8ModSource,
    pub dest: MlP8ModDest,
    /// Signed depth in percent, `[-100, 100]`, of the destination's full
    /// range. Percent rather than a `[-1, 1]` fraction because every other
    /// depth on this device is authored in percent, and because that is the
    /// number its automation lane and its readout both have to agree on.
    pub amount: f32,
}

/// A patch's internal routes.
///
/// Fixed capacity and `Copy`, so installing a patch never allocates and the
/// audio thread never grows a container. Empty slots are `None` rather than
/// placeholder rows, so a saved patch carries the routes it has and no more.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(into = "MlP8RoutesRepr", from = "MlP8RoutesRepr")]
pub struct MlP8Routes {
    routes: [Option<MlP8Route>; MLP8_MAX_ROUTES],
    /// Next durable id. Monotonic within a patch so a removed route's id is
    /// never handed to a different route later, which would silently
    /// re-point an automation lane.
    next_id: u16,
}

/// What a patch writes: the routes it actually has, not sixteen slots most of
/// which are empty.
///
/// The in-memory form is a fixed array because the audio thread may not
/// allocate, but that is a runtime concern and not a file format. Serializing
/// it directly also does not merely look wasteful -- TOML has no
/// representation for a `None` element, so the derived form could not save an
/// ML-P8 at all.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct MlP8RoutesRepr {
    routes: Vec<MlP8Route>,
    next_id: u16,
}

impl Default for MlP8RoutesRepr {
    fn default() -> Self {
        Self {
            routes: Vec::new(),
            next_id: 1,
        }
    }
}

impl From<MlP8Routes> for MlP8RoutesRepr {
    fn from(value: MlP8Routes) -> Self {
        Self {
            routes: value.iter().copied().collect(),
            next_id: value.next_id,
        }
    }
}

impl From<MlP8RoutesRepr> for MlP8Routes {
    fn from(value: MlP8RoutesRepr) -> Self {
        let mut routes = [None; MLP8_MAX_ROUTES];
        for (slot, route) in routes
            .iter_mut()
            .zip(value.routes.into_iter().take(MLP8_MAX_ROUTES))
        {
            *slot = Some(route);
        }
        // A file whose `next_id` does not clear the ids it also carries would
        // hand a live route's id to the next one added, which is exactly the
        // silent re-pointing the durable id exists to prevent. Trust the
        // routes over the counter.
        let highest = routes.iter().flatten().map(|route| route.id).max();
        let next_id = match highest {
            Some(highest) => value.next_id.max(highest.saturating_add(1)),
            None => value.next_id.max(1),
        };
        Self { routes, next_id }
    }
}

impl Default for MlP8Routes {
    fn default() -> Self {
        Self {
            routes: [None; MLP8_MAX_ROUTES],
            next_id: 1,
        }
    }
}

impl MlP8Routes {
    pub fn iter(&self) -> impl Iterator<Item = &MlP8Route> {
        self.routes.iter().flatten()
    }

    pub fn len(&self) -> usize {
        self.iter().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, id: u16) -> Option<&MlP8Route> {
        self.iter().find(|route| route.id == id)
    }

    /// Whether two route lists differ only in their amounts.
    ///
    /// The audio path compiles a flat table from the topology and moves the
    /// depths inside it, so this is the question that decides whether an
    /// arriving parameter block is a rebuild or a retune. Asked here rather
    /// than in the DSP because it is a property of the authored list, and
    /// because answering it by id, source and destination costs sixteen
    /// comparisons and no descriptor lookups.
    pub fn same_topology(&self, other: &Self) -> bool {
        self.routes
            .iter()
            .zip(other.routes.iter())
            .all(|pair| match pair {
                (Some(a), Some(b)) => a.id == b.id && a.source == b.source && a.dest == b.dest,
                (None, None) => true,
                _ => false,
            })
    }

    /// Insert or repoint a route under an id the caller already minted.
    ///
    /// The authoring side owns the identity — the same rule the modulator
    /// rack follows — so an edit that arrives twice, or out of order, lands
    /// on the route it names rather than minting a second one.
    pub fn upsert(&mut self, route: MlP8Route) -> bool {
        if !route.dest.is_legal() {
            return false;
        }
        for slot in self.routes.iter_mut() {
            if slot.is_some_and(|existing| existing.id == route.id) {
                *slot = Some(route);
                self.next_id = self.next_id.max(route.id.saturating_add(1));
                return true;
            }
        }
        let Some(slot) = self.routes.iter_mut().find(|slot| slot.is_none()) else {
            return false;
        };
        *slot = Some(route);
        self.next_id = self.next_id.max(route.id.saturating_add(1));
        true
    }

    /// The id the next authored route will take, without taking it.
    ///
    /// The UI mints ids so the engine never has to answer back; this is what
    /// it mints from.
    pub fn next_id(&self) -> u16 {
        self.next_id
    }

    /// Add a route, returning its durable id.
    ///
    /// `None` when the patch is full or the destination is structural. A
    /// rejected route is not silently dropped in place of a different one —
    /// the caller still holds what it authored and can say why it did not
    /// land.
    pub fn add(&mut self, source: MlP8ModSource, dest: MlP8ModDest) -> Option<u16> {
        if !dest.is_legal() {
            return None;
        }
        let slot = self.routes.iter().position(Option::is_none)?;
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1)?;
        self.routes[slot] = Some(MlP8Route {
            id,
            source,
            dest,
            amount: 0.0,
        });
        Some(id)
    }

    /// Every authored route, mutably. For the load-time repair pass, which
    /// clamps depths in place rather than rebuilding the list.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut MlP8Route> {
        self.routes.iter_mut().flatten()
    }

    /// Drop every route the predicate rejects, returning how many went.
    ///
    /// The repair pass is what needs this: a file can carry a route onto a
    /// destination the device refuses, or two routes claiming one id, and
    /// neither can be corrected into something meaningful.
    pub fn retain(&mut self, mut keep: impl FnMut(&MlP8Route) -> bool) -> usize {
        let mut dropped = 0;
        for slot in self.routes.iter_mut() {
            if slot.is_some_and(|route| !keep(&route)) {
                *slot = None;
                dropped += 1;
            }
        }
        dropped
    }

    pub fn remove(&mut self, id: u16) -> bool {
        for slot in &mut self.routes {
            if slot.is_some_and(|route| route.id == id) {
                *slot = None;
                return true;
            }
        }
        false
    }

    /// Set a route's signed amount. This is the one part of a route that is
    /// an ordinary automatable value rather than a structural edit.
    pub fn set_amount(&mut self, id: u16, amount: f32) -> bool {
        for route in self.routes.iter_mut().flatten() {
            if route.id == id {
                route.amount = amount.clamp(MOD_PERCENT_MIN, MOD_PERCENT_MAX);
                return true;
            }
        }
        false
    }

    /// Re-point an existing route. Structural: prepared off the audio thread.
    pub fn set_endpoints(&mut self, id: u16, source: MlP8ModSource, dest: MlP8ModDest) -> bool {
        if !dest.is_legal() {
            return false;
        }
        for route in self.routes.iter_mut().flatten() {
            if route.id == id {
                route.source = source;
                route.dest = dest;
                return true;
            }
        }
        false
    }
}

/// Modulation amounts are authored as signed percent. The musical mapping from
/// percent to phase deviation is one documented curve in the DSP, so the
/// persisted value stays a number a musician recognises and an automation lane
/// can pass through zero to invert the modulation phase.
const MOD_PERCENT_MIN: f32 = -100.0;
const MOD_PERCENT_MAX: f32 = 100.0;

const fn percent(id: u32, name: &'static str) -> ParamDescriptor {
    ParamDescriptor {
        id,
        name,
        unit: "%",
        min: MOD_PERCENT_MIN,
        max: MOD_PERCENT_MAX,
        curve: ParamCurve::Linear,
        default: 0.0,
    }
}

/// A route's signed amount, addressed inside the route rather than in the
/// device's parameter table.
///
/// The route's durable id is the address; this is the field within it. Kept as
/// a named constant rather than a bare `0` so a second per-route value later
/// is an addition here instead of a re-interpretation of every saved lane.
pub const MLP8_ROUTE_PARAM_AMOUNT: u32 = 0;

/// What an internal route exposes to automation and to the UI.
///
/// A route deliberately does *not* consume ids from [`DESCRIPTORS`]. Sixteen
/// routes times their fields would be a permanent block of the device's own id
/// space spent on a capacity number the plan calls provisional, and every
/// route would then have to keep the slot it was authored in forever. The
/// route's identity carries the address instead.
pub static ROUTE_DESCRIPTORS: [ParamDescriptor; 1] =
    [percent(MLP8_ROUTE_PARAM_AMOUNT, "Amount")];

pub fn route_descriptor(param: u32) -> Option<&'static ParamDescriptor> {
    ROUTE_DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.id == param)
}

const fn osc_descriptors(n: u32, wave_name: &'static str) -> [ParamDescriptor; 5] {
    // Osc 1 is the reference source: open, at pitch, and the one the gain
    // contract is calibrated against. The other two are tuned an octave apart
    // and silent, so the default patch is one saw and nothing else.
    let (semitones, cents, level) = match n {
        0 => (0.0, 0.0, 1.0),
        1 => (12.0, 4.0, 0.0),
        _ => (-12.0, -4.0, 0.0),
    };
    [
        stepped(osc_param(n, OSC_OFFSET_WAVE), wave_name, 4, 2.0),
        ParamDescriptor {
            id: osc_param(n, OSC_OFFSET_SEMITONES),
            name: "Semis",
            unit: "st",
            min: -48.0,
            max: 48.0,
            curve: ParamCurve::Linear,
            default: semitones,
        },
        ParamDescriptor {
            id: osc_param(n, OSC_OFFSET_CENTS),
            name: "Cents",
            unit: "ct",
            min: -100.0,
            max: 100.0,
            curve: ParamCurve::Linear,
            default: cents,
        },
        unit(osc_param(n, OSC_OFFSET_LEVEL), "Level", level),
        ParamDescriptor {
            id: osc_param(n, OSC_OFFSET_PULSE_WIDTH),
            name: "Width",
            unit: "",
            min: 0.05,
            max: 0.95,
            curve: ParamCurve::Linear,
            default: 0.5,
        },
    ]
}

/// Everything from id 15 up: the amplitude envelope, glide, the two extra
/// sources, and the network amounts.
const NETWORK_DESCRIPTORS: [ParamDescriptor; 26] = [
    seconds(PARAM_ATTACK, "Attack", 0.005),
    seconds(PARAM_DECAY, "Decay", 0.2),
    unit(PARAM_SUSTAIN, "Sustain", 0.7),
    seconds(PARAM_RELEASE, "Release", 0.15),
    ParamDescriptor {
        id: PARAM_GLIDE,
        name: "Glide",
        unit: "s",
        min: 0.0,
        max: 2.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    unit(PARAM_SUB_LEVEL, "Sub level", 0.0),
    stepped(PARAM_SUB_OCTAVE, "Sub oct", 2, 0.0),
    stepped(PARAM_SUB_WAVE, "Sub wave", 2, 0.0),
    stepped(PARAM_SUB_SOURCE, "Sub src", 3, 0.0),
    unit(PARAM_NOISE_LEVEL, "Noise level", 0.0),
    // Bipolar rather than 0..1: white is the centre a musician tunes away
    // from in two directions, so it belongs at zero rather than at a half.
    percent(PARAM_NOISE_COLOR, "Noise color"),
    percent(PARAM_XMOD_BASE, "XM 1>2"),
    percent(PARAM_XMOD_BASE + 1, "XM 1>3"),
    percent(PARAM_XMOD_BASE + 2, "XM 2>1"),
    percent(PARAM_XMOD_BASE + 3, "XM 2>3"),
    percent(PARAM_XMOD_BASE + 4, "XM 3>1"),
    percent(PARAM_XMOD_BASE + 5, "XM 3>2"),
    percent(PARAM_NOISE_TO_OSC_BASE, "N>Osc 1"),
    percent(PARAM_NOISE_TO_OSC_BASE + 1, "N>Osc 2"),
    percent(PARAM_NOISE_TO_OSC_BASE + 2, "N>Osc 3"),
    percent(PARAM_OSC_FEEDBACK_BASE, "FB Osc 1"),
    percent(PARAM_OSC_FEEDBACK_BASE + 1, "FB Osc 2"),
    percent(PARAM_OSC_FEEDBACK_BASE + 2, "FB Osc 3"),
    stepped(PARAM_SYNC_SOURCE_BASE, "Sync 1", 4, 0.0),
    stepped(PARAM_SYNC_SOURCE_BASE + 1, "Sync 2", 4, 0.0),
    stepped(PARAM_SYNC_SOURCE_BASE + 2, "Sync 3", 4, 0.0),
];

const fn bipolar(id: u32, name: &'static str) -> ParamDescriptor {
    ParamDescriptor {
        id,
        name,
        unit: "",
        min: -1.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    }
}

/// The filter, its envelope, the two velocity depths, and the loop around the
/// whole voice.
const VOICE_DESCRIPTORS: [ParamDescriptor; 13] = [
    stepped(PARAM_FILTER_MODE, "Mode", 4, 0.0),
    unit(PARAM_FILTER_CUTOFF, "Cutoff", 1.0),
    unit(PARAM_FILTER_RESONANCE, "Reso", 0.0),
    bipolar(PARAM_FILTER_ENV_AMOUNT, "Env amt"),
    unit(PARAM_DRIVE, "Drive", 0.0),
    // 0-200%: 100% is one octave of cutoff per played octave, and the top
    // half is the exaggeration that makes a patch open up as it climbs.
    ParamDescriptor {
        id: PARAM_KEYTRACK,
        name: "Keytrack",
        unit: "",
        min: 0.0,
        max: 2.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    seconds(PARAM_FILTER_ATTACK, "F attack", 0.005),
    seconds(PARAM_FILTER_DECAY, "F decay", 0.2),
    unit(PARAM_FILTER_SUSTAIN, "F sustain", 0.7),
    seconds(PARAM_FILTER_RELEASE, "F release", 0.15),
    // Velocity's amplitude role is a crossfade from "every note the same" to
    // "every note as played", which is why it is unipolar and defaults open.
    unit(PARAM_AMP_VELOCITY, "Amp vel", 1.0),
    bipolar(PARAM_FILTER_VELOCITY, "Filt vel"),
    bipolar(PARAM_VOICE_FEEDBACK, "Feedback"),
];

/// The complete ML-P8 table: three oscillator blocks then everything else.
///
/// Written as one `static` so the engine can enumerate it without allocating,
/// and assembled by a `const fn` rather than by hand so the ids stay derived
/// from the constants above.
pub static DESCRIPTORS: [ParamDescriptor; 62] = concat(
    osc_descriptors(0, "Osc 1 wave"),
    osc_descriptors(1, "Osc 2 wave"),
    osc_descriptors(2, "Osc 3 wave"),
    NETWORK_DESCRIPTORS,
    VOICE_DESCRIPTORS,
    LFO_DESCRIPTORS,
);

const fn concat(
    a: [ParamDescriptor; 5],
    b: [ParamDescriptor; 5],
    c: [ParamDescriptor; 5],
    network: [ParamDescriptor; 26],
    voice: [ParamDescriptor; 13],
    lfo: [ParamDescriptor; 8],
) -> [ParamDescriptor; 62] {
    let mut out = [a[0]; 62];
    let mut i = 0;
    while i < 5 {
        out[i] = a[i];
        out[5 + i] = b[i];
        out[10 + i] = c[i];
        i += 1;
    }
    let mut j = 0;
    while j < 26 {
        out[15 + j] = network[j];
        j += 1;
    }
    let mut k = 0;
    while k < 13 {
        out[41 + k] = voice[k];
        k += 1;
    }
    let mut l = 0;
    while l < 8 {
        out[54 + l] = lfo[l];
        l += 1;
    }
    out
}

/// All ML-P8 parameters, in the units the DSP and UI share.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct MlP8Params {
    pub osc: [OscParams; 3],
    /// Amplitude attack time (seconds).
    pub attack: f32,
    /// Amplitude decay time (seconds).
    pub decay: f32,
    /// Amplitude sustain level in `[0, 1]`.
    pub sustain: f32,
    /// Amplitude release time (seconds).
    pub release: f32,
    /// Portamento time (seconds) applied when a slot is reused. `0` is instant.
    pub glide: f32,
    /// Sub mix level in `[0, 1]`.
    pub sub_level: f32,
    pub sub_octave: SubOctave,
    pub sub_wave: SubWave,
    pub sub_source: SubSource,
    /// Noise mix level in `[0, 1]`. Independent of [`Self::noise_to_osc`]:
    /// noise can modulate every oscillator while contributing nothing audible.
    pub noise_level: f32,
    /// Noise tilt in percent. Negative is dark, `0` is white, positive bright.
    pub noise_color: f32,
    /// Directed cross-modulation amounts in percent, indexed by
    /// [`xmod_index`].
    pub xmod: [f32; 6],
    /// Noise into each oscillator's phase input, in percent.
    pub noise_to_osc: [f32; 3],
    /// Each oscillator's self-feedback amount, in percent.
    pub osc_feedback: [f32; 3],
    /// Which oscillator, if any, hard-syncs each oscillator.
    pub sync_source: [SyncSource; 3],

    // --- The voice around the network -----------------------------------
    pub filter_mode: MlP8FilterMode,
    /// Cutoff on a perceptual `[0, 1]` scale. `1` is wide open.
    pub filter_cutoff: f32,
    /// Resonance in `[0, 1]`.
    pub filter_resonance: f32,
    /// Bipolar filter-envelope depth in `[-1, 1]`.
    pub filter_env_amount: f32,
    /// Pre-filter drive in `[0, 1]`, inside the feedback loop. `0` bypasses.
    pub drive: f32,
    /// Cutoff tracking in `[0, 2]`. `1` is one octave per played octave.
    pub filter_keytrack: f32,
    pub filter_attack: f32,
    pub filter_decay: f32,
    pub filter_sustain: f32,
    pub filter_release: f32,
    /// How much velocity scales the VCA, in `[0, 1]`. At `0` every note has
    /// the same amplitude; at `1` it follows the note as played.
    pub amp_velocity: f32,
    /// Bipolar velocity depth added to the filter envelope amount.
    pub filter_velocity: f32,
    /// Bipolar output-to-input feedback around the voice's filter.
    pub voice_feedback: f32,

    // --- The instrument's own modulation --------------------------------
    pub lfo: MlP8LfoParams,
    /// Internal routes. Not addressed by descriptor id: a route's amount is
    /// automated through its durable route identity instead, so the id space
    /// above stays a description of the instrument rather than a fixed number
    /// of route slots.
    pub routes: MlP8Routes,
}

impl Default for MlP8Params {
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
            attack: 0.005,
            decay: 0.2,
            sustain: 0.7,
            release: 0.15,
            glide: 0.0,
            sub_level: 0.0,
            sub_octave: SubOctave::Minus1,
            sub_wave: SubWave::Sine,
            sub_source: SubSource::Osc1,
            noise_level: 0.0,
            noise_color: 0.0,
            xmod: [0.0; 6],
            noise_to_osc: [0.0; 3],
            osc_feedback: [0.0; 3],
            sync_source: [SyncSource::Off; 3],
            filter_mode: MlP8FilterMode::Lp12,
            // The filter starts open and out of the way, so the default patch
            // is still one saw at the reference level and the gain contract
            // is calibrated against a path with nothing in it.
            filter_cutoff: 1.0,
            filter_resonance: 0.0,
            filter_env_amount: 0.0,
            drive: 0.0,
            filter_keytrack: 0.0,
            filter_attack: 0.005,
            filter_decay: 0.2,
            filter_sustain: 0.7,
            filter_release: 0.15,
            // Open, because a synth that ignores how hard you played it is
            // the surprising default, not the safe one.
            amp_velocity: 1.0,
            filter_velocity: 0.0,
            voice_feedback: 0.0,
            lfo: MlP8LfoParams::default(),
            routes: MlP8Routes::default(),
        }
    }
}

/// Whether this parameter's control surface reads in decibels.
///
/// The five mix levels are stored linear in `[0, 1]` and shown as gain, so a
/// value typed into one of their fields is a dB figure and has to be
/// converted before it is written. That pairing is named here rather than
/// assumed separately by the face and by the handler that parses the text,
/// which is how the two would come to disagree.
pub fn is_gain_param(id: u32) -> bool {
    matches!(id, PARAM_SUB_LEVEL | PARAM_NOISE_LEVEL)
        || (id < 15 && id % 5 == OSC_OFFSET_LEVEL)
}

/// Split an id into `(oscillator, offset)` when it lands in an oscillator
/// block. Unlike the shared synths' `100 + n * 10`, ML-P8's blocks start at
/// zero and are exactly five wide, because this table was never grown from an
/// older one that needed the gaps.
fn osc_slot(id: u32) -> Option<(usize, u32)> {
    let oscillator = (id / 5) as usize;
    (oscillator < 3).then_some((oscillator, id % 5))
}

fn indexed(id: u32, base: u32, count: u32) -> Option<usize> {
    let offset = id.checked_sub(base)?;
    (offset < count).then_some(offset as usize)
}

/// Read one parameter in natural units by wire id.
pub fn get(p: &MlP8Params, id: u32) -> Option<f32> {
    if let Some((oscillator, offset)) = osc_slot(id) {
        let osc = &p.osc[oscillator];
        return Some(match offset {
            OSC_OFFSET_WAVE => osc.wave.to_index() as f32,
            OSC_OFFSET_SEMITONES => osc.semitones,
            OSC_OFFSET_CENTS => osc.cents,
            OSC_OFFSET_LEVEL => osc.level,
            OSC_OFFSET_PULSE_WIDTH => osc.pulse_width,
            _ => return None,
        });
    }
    if let Some(route) = indexed(id, PARAM_XMOD_BASE, 6) {
        return Some(p.xmod[route]);
    }
    if let Some(n) = indexed(id, PARAM_NOISE_TO_OSC_BASE, 3) {
        return Some(p.noise_to_osc[n]);
    }
    if let Some(n) = indexed(id, PARAM_OSC_FEEDBACK_BASE, 3) {
        return Some(p.osc_feedback[n]);
    }
    if let Some(n) = indexed(id, PARAM_SYNC_SOURCE_BASE, 3) {
        return Some(p.sync_source[n].to_index() as f32);
    }
    Some(match id {
        PARAM_FILTER_MODE => p.filter_mode.to_index() as f32,
        PARAM_FILTER_CUTOFF => p.filter_cutoff,
        PARAM_FILTER_RESONANCE => p.filter_resonance,
        PARAM_FILTER_ENV_AMOUNT => p.filter_env_amount,
        PARAM_DRIVE => p.drive,
        PARAM_KEYTRACK => p.filter_keytrack,
        PARAM_FILTER_ATTACK => p.filter_attack,
        PARAM_FILTER_DECAY => p.filter_decay,
        PARAM_FILTER_SUSTAIN => p.filter_sustain,
        PARAM_FILTER_RELEASE => p.filter_release,
        PARAM_AMP_VELOCITY => p.amp_velocity,
        PARAM_FILTER_VELOCITY => p.filter_velocity,
        PARAM_VOICE_FEEDBACK => p.voice_feedback,
        PARAM_LFO_WAVE => p.lfo.wave.to_index() as f32,
        PARAM_LFO_SYNC => f32::from(u8::from(p.lfo.synced)),
        PARAM_LFO_RATE_HZ => p.lfo.rate_hz,
        PARAM_LFO_RATE_DIVISION => p.lfo.rate_division.to_index() as f32,
        PARAM_LFO_PHASE => p.lfo.phase,
        PARAM_LFO_WARP => p.lfo.warp,
        PARAM_LFO_SLEW => p.lfo.slew,
        PARAM_LFO_RETRIGGER => p.lfo.retrigger.to_index() as f32,
        PARAM_ATTACK => p.attack,
        PARAM_DECAY => p.decay,
        PARAM_SUSTAIN => p.sustain,
        PARAM_RELEASE => p.release,
        PARAM_GLIDE => p.glide,
        PARAM_SUB_LEVEL => p.sub_level,
        PARAM_SUB_OCTAVE => p.sub_octave.to_index() as f32,
        PARAM_SUB_WAVE => p.sub_wave.to_index() as f32,
        PARAM_SUB_SOURCE => p.sub_source.to_index() as f32,
        PARAM_NOISE_LEVEL => p.noise_level,
        PARAM_NOISE_COLOR => p.noise_color,
        _ => return None,
    })
}

/// Write one parameter in natural units by wire id. The caller has already
/// clamped `value` through the descriptor.
pub fn set(p: &mut MlP8Params, id: u32, value: f32) -> bool {
    if let Some((oscillator, offset)) = osc_slot(id) {
        let osc = &mut p.osc[oscillator];
        match offset {
            OSC_OFFSET_WAVE => osc.wave = OscWave::from_index(value.round() as i32),
            OSC_OFFSET_SEMITONES => osc.semitones = value,
            OSC_OFFSET_CENTS => osc.cents = value,
            OSC_OFFSET_LEVEL => osc.level = value,
            OSC_OFFSET_PULSE_WIDTH => osc.pulse_width = value,
            _ => return false,
        }
        return true;
    }
    if let Some(route) = indexed(id, PARAM_XMOD_BASE, 6) {
        p.xmod[route] = value;
        return true;
    }
    if let Some(n) = indexed(id, PARAM_NOISE_TO_OSC_BASE, 3) {
        p.noise_to_osc[n] = value;
        return true;
    }
    if let Some(n) = indexed(id, PARAM_OSC_FEEDBACK_BASE, 3) {
        p.osc_feedback[n] = value;
        return true;
    }
    if let Some(n) = indexed(id, PARAM_SYNC_SOURCE_BASE, 3) {
        p.sync_source[n] = SyncSource::from_index(value.round() as i32);
        return true;
    }
    match id {
        PARAM_FILTER_MODE => p.filter_mode = MlP8FilterMode::from_index(value.round() as i32),
        PARAM_FILTER_CUTOFF => p.filter_cutoff = value,
        PARAM_FILTER_RESONANCE => p.filter_resonance = value,
        PARAM_FILTER_ENV_AMOUNT => p.filter_env_amount = value,
        PARAM_DRIVE => p.drive = value,
        PARAM_KEYTRACK => p.filter_keytrack = value,
        PARAM_FILTER_ATTACK => p.filter_attack = value,
        PARAM_FILTER_DECAY => p.filter_decay = value,
        PARAM_FILTER_SUSTAIN => p.filter_sustain = value,
        PARAM_FILTER_RELEASE => p.filter_release = value,
        PARAM_AMP_VELOCITY => p.amp_velocity = value,
        PARAM_FILTER_VELOCITY => p.filter_velocity = value,
        PARAM_VOICE_FEEDBACK => p.voice_feedback = value,
        PARAM_LFO_WAVE => p.lfo.wave = MlP8LfoWave::from_index(value.round() as i32),
        PARAM_LFO_SYNC => p.lfo.synced = value.round() > 0.0,
        PARAM_LFO_RATE_HZ => p.lfo.rate_hz = value,
        PARAM_LFO_RATE_DIVISION => {
            p.lfo.rate_division = ModTimeDivision::from_index(value.round() as i32)
        }
        PARAM_LFO_PHASE => p.lfo.phase = value,
        PARAM_LFO_WARP => p.lfo.warp = value,
        PARAM_LFO_SLEW => p.lfo.slew = value,
        PARAM_LFO_RETRIGGER => {
            p.lfo.retrigger = MlP8LfoRetrigger::from_index(value.round() as i32)
        }
        PARAM_ATTACK => p.attack = value,
        PARAM_DECAY => p.decay = value,
        PARAM_SUSTAIN => p.sustain = value,
        PARAM_RELEASE => p.release = value,
        PARAM_GLIDE => p.glide = value,
        PARAM_SUB_LEVEL => p.sub_level = value,
        PARAM_SUB_OCTAVE => p.sub_octave = SubOctave::from_index(value.round() as i32),
        PARAM_SUB_WAVE => p.sub_wave = SubWave::from_index(value.round() as i32),
        PARAM_SUB_SOURCE => p.sub_source = SubSource::from_index(value.round() as i32),
        PARAM_NOISE_LEVEL => p.noise_level = value,
        PARAM_NOISE_COLOR => p.noise_color = value,
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
                assert_ne!(a.id, b.id, "duplicate ML-P8 id {} ({})", a.id, a.name);
            }
        }
    }

    /// The plan reserves 0-41 with 24 held back. A descriptor outside that
    /// band would be reaching into a later step's reservation.
    #[test]
    fn descriptor_ids_stay_inside_the_reserved_band() {
        for d in &DESCRIPTORS {
            assert!(d.id <= 63, "{} ({}) is outside 0-63", d.id, d.name);
            assert_ne!(d.id, 24, "24 is reserved for a fifth sub control");
            assert_ne!(d.id, 63, "63 closes the LFO band and stays reserved");
        }
        assert_eq!(DESCRIPTORS.len(), 62);
    }

    #[test]
    fn every_descriptor_round_trips_through_get_and_set() {
        for d in &DESCRIPTORS {
            let mut params = MlP8Params::default();
            let target = d.clamp_natural(d.min + (d.max - d.min) * 0.75);
            assert!(set(&mut params, d.id, target), "{} is not settable", d.name);
            let read = get(&params, d.id).unwrap_or_else(|| panic!("{} is not readable", d.name));
            assert!(
                (read - target).abs() < 1.0e-4,
                "{} wrote {target} and read {read}",
                d.name
            );
        }
    }

    #[test]
    fn defaults_agree_with_the_descriptor_table() {
        let params = MlP8Params::default();
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

    #[test]
    fn xmod_indices_cover_every_directed_pair() {
        let mut seen = [false; 6];
        for from in 0..3 {
            for to in 0..3 {
                if from == to {
                    continue;
                }
                let index = xmod_index(from, to);
                assert!(!seen[index], "{from}->{to} collides at {index}");
                seen[index] = true;
            }
        }
        assert!(seen.iter().all(|s| *s));
    }

    /// The panel order the ids were reserved in: `1>2, 1>3, 2>1, 2>3, 3>1,
    /// 3>2`. Pinned because the descriptor names are written out by hand.
    #[test]
    fn xmod_ids_follow_the_panel_order() {
        assert_eq!(xmod_index(0, 1), 0);
        assert_eq!(xmod_index(0, 2), 1);
        assert_eq!(xmod_index(1, 0), 2);
        assert_eq!(xmod_index(1, 2), 3);
        assert_eq!(xmod_index(2, 0), 4);
        assert_eq!(xmod_index(2, 1), 5);
    }

    #[test]
    fn the_gain_parameters_are_exactly_the_five_mix_levels() {
        let gains: Vec<u32> = DESCRIPTORS
            .iter()
            .map(|d| d.id)
            .filter(|id| is_gain_param(*id))
            .collect();
        assert_eq!(
            gains,
            vec![
                osc_param(0, OSC_OFFSET_LEVEL),
                osc_param(1, OSC_OFFSET_LEVEL),
                osc_param(2, OSC_OFFSET_LEVEL),
                PARAM_SUB_LEVEL,
                PARAM_NOISE_LEVEL,
            ]
        );
        // Every one of them is a linear unit range, which is what makes the
        // dB reading a display convention rather than the stored value.
        for id in gains {
            let d = DESCRIPTORS.iter().find(|d| d.id == id).unwrap();
            assert_eq!((d.min, d.max), (0.0, 1.0), "{} is not a unit range", d.name);
        }
    }

    #[test]
    fn every_route_destination_is_continuous_and_addressable() {
        for dest in MlP8ModDest::ALL {
            assert!(dest.is_legal(), "{:?} is listed but not legal", dest);
            assert!(dest.slot().is_some(), "{:?} has no offset slot", dest);
            assert!(
                dest.full_range() > 0.0,
                "{:?} spans nothing, so an amount would mean nothing",
                dest
            );
        }
        let mut slots: Vec<usize> = MlP8ModDest::ALL
            .iter()
            .map(|dest| dest.slot().unwrap())
            .collect();
        slots.sort_unstable();
        slots.dedup();
        assert_eq!(slots.len(), MLP8_MOD_DESTS, "two destinations share a slot");
    }

    #[test]
    fn structural_selectors_refuse_to_be_routed() {
        // The rule is the descriptor's curve, so this list is a restatement
        // of the plan rather than a second source of truth: waveform, sync
        // source, filter mode, sub source and sub octave are all stepped.
        for id in [
            osc_param(0, OSC_OFFSET_WAVE),
            PARAM_SUB_OCTAVE,
            PARAM_SUB_WAVE,
            PARAM_SUB_SOURCE,
            PARAM_SYNC_SOURCE_BASE,
            PARAM_FILTER_MODE,
            PARAM_LFO_WAVE,
            PARAM_LFO_RETRIGGER,
        ] {
            let dest = MlP8ModDest::Param { id };
            assert!(!dest.is_legal(), "id {id} is stepped but was accepted");
            let mut routes = MlP8Routes::default();
            assert_eq!(routes.add(MlP8ModSource::Lfo, dest), None);
        }
    }

    #[test]
    fn a_removed_route_never_gives_its_id_away() {
        let mut routes = MlP8Routes::default();
        let cutoff = MlP8ModDest::Param { id: PARAM_FILTER_CUTOFF };
        let drive = MlP8ModDest::Param { id: PARAM_DRIVE };

        let first = routes.add(MlP8ModSource::Lfo, cutoff).unwrap();
        assert!(routes.set_amount(first, 0.5));
        assert!(routes.remove(first));

        // The freed slot is reused, but the identity is not: an automation
        // lane still pointed at `first` must not start driving this instead.
        let second = routes.add(MlP8ModSource::Velocity, drive).unwrap();
        assert_ne!(first, second);
        assert!(routes.get(first).is_none());
        assert!(!routes.set_amount(first, 1.0));
    }

    #[test]
    fn the_route_list_stops_at_its_measured_ceiling() {
        let mut routes = MlP8Routes::default();
        let dest = MlP8ModDest::Param { id: PARAM_FILTER_CUTOFF };
        for _ in 0..MLP8_MAX_ROUTES {
            assert!(routes.add(MlP8ModSource::Lfo, dest).is_some());
        }
        assert_eq!(routes.len(), MLP8_MAX_ROUTES);
        // Refused rather than silently replacing one that was authored.
        assert_eq!(routes.add(MlP8ModSource::Lfo, dest), None);
        assert_eq!(routes.len(), MLP8_MAX_ROUTES);
    }

    #[test]
    fn a_route_amount_is_signed_and_clamped() {
        let mut routes = MlP8Routes::default();
        let id = routes
            .add(MlP8ModSource::FilterEnv, MlP8ModDest::Param { id: PARAM_XMOD_BASE })
            .unwrap();
        assert!(routes.set_amount(id, -300.0));
        assert_eq!(routes.get(id).unwrap().amount, -100.0);
        assert!(routes.set_amount(id, 25.0));
        assert_eq!(routes.get(id).unwrap().amount, 25.0);
    }

    #[test]
    fn a_route_amount_is_addressed_by_its_own_descriptor() {
        // The amount is automated through the route's durable id, so it needs
        // a descriptor of its own -- and exactly one, because a route has
        // exactly one continuous value.
        assert!(route_descriptor(MLP8_ROUTE_PARAM_AMOUNT).is_some());
        assert_eq!(ROUTE_DESCRIPTORS.len(), 1);
        let descriptor = route_descriptor(MLP8_ROUTE_PARAM_AMOUNT).unwrap();
        assert_eq!((descriptor.min, descriptor.max), (-100.0, 100.0));
        // Nothing else answers: an address whose param is not a route field
        // must miss rather than land on the amount by default.
        assert!(route_descriptor(MLP8_ROUTE_PARAM_AMOUNT + 1).is_none());
    }

    #[test]
    fn every_destination_reports_the_span_it_clamps_to() {
        for dest in MlP8ModDest::ALL {
            let (min, max) = dest.range();
            assert!(min < max, "{dest:?} has an empty span");
            assert_eq!(
                max - min,
                dest.full_range(),
                "{dest:?} clamps to a different span than an amount of 100% covers"
            );
        }
    }

    #[test]
    fn an_unknown_id_is_neither_readable_nor_writable() {
        let mut params = MlP8Params::default();
        // 24 and 63 are holes inside the band, 64 is past its end.
        assert_eq!(get(&params, 24), None);
        assert!(!set(&mut params, 24, 1.0));
        assert_eq!(get(&params, 63), None);
        assert!(!set(&mut params, 63, 1.0));
        assert_eq!(get(&params, 64), None);
        assert!(!set(&mut params, 64, 1.0));
    }
}
