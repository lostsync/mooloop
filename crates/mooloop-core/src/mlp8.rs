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
pub static DESCRIPTORS: [ParamDescriptor; 54] = concat(
    osc_descriptors(0, "Osc 1 wave"),
    osc_descriptors(1, "Osc 2 wave"),
    osc_descriptors(2, "Osc 3 wave"),
    NETWORK_DESCRIPTORS,
    VOICE_DESCRIPTORS,
);

const fn concat(
    a: [ParamDescriptor; 5],
    b: [ParamDescriptor; 5],
    c: [ParamDescriptor; 5],
    network: [ParamDescriptor; 26],
    voice: [ParamDescriptor; 13],
) -> [ParamDescriptor; 54] {
    let mut out = [a[0]; 54];
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
            assert!(d.id <= 54, "{} ({}) is outside 0-54", d.id, d.name);
            assert_ne!(d.id, 24, "24 is reserved for a fifth sub control");
        }
        assert_eq!(DESCRIPTORS.len(), 54);
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
    fn an_unknown_id_is_neither_readable_nor_writable() {
        let mut params = MlP8Params::default();
        // 24 is a hole inside the band, 55 is past its end.
        assert_eq!(get(&params, 24), None);
        assert!(!set(&mut params, 24, 1.0));
        assert_eq!(get(&params, 55), None);
        assert!(!set(&mut params, 55, 1.0));
    }
}
