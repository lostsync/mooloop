//! Descriptor addressing for channel generators.
//!
//! Effects have been `(EffectTarget, slot, param_id)` since parameters got
//! ranges and curves. Generators were not: they ship whole structs
//! (`SetChannelSamplerParams { channel, params }`), so nothing inside one
//! could be named, which meant no generator parameter could be automated or
//! modulated. This module closes that, mirroring `effect.rs` exactly — same
//! `ParamDescriptor`, same stable-id rule, same `get`/`set` pair — so a lane,
//! a matrix row, and a knob all keep talking about the same thing.
//!
//! Ids are per-kind and never renumbered, because automation persists them.
//! The three-oscillator synths reserve a block of ten ids per oscillator so
//! adding an oscillator parameter later does not disturb the others.

use crate::{
    DeviceKind, EnvTrigger, FilterModel, GlideMode, LfoParams, LfoWave, LoopMode, MlP8Params,
    MonoSynthParams, MlM1Params,
    NotePriority, OscParams, OscWave, ParamCurve, ParamDescriptor, PlayMode, PolySynthParams,
    RetriggerMode,
    SamplerParams, StretchMode, VoiceMode, DEFAULT_SLICE_BASE_NOTE, MAX_LINEAR_GAIN, MAX_POLY_VOICES,
    MAX_SAMPLER_VOICES, MAX_STRETCH_BARS, MAX_STRETCH_GRAIN, MAX_STRETCH_RATIO,
    MIN_STRETCH_BARS, MIN_STRETCH_GRAIN, MIN_STRETCH_RATIO,
};

// --- Sampler ---------------------------------------------------------------

pub const SAMPLER_PARAM_START: u32 = 0;
pub const SAMPLER_PARAM_END: u32 = 1;
pub const SAMPLER_PARAM_REVERSE: u32 = 2;
pub const SAMPLER_PARAM_TUNE_SEMITONES: u32 = 3;
pub const SAMPLER_PARAM_TUNE_CENTS: u32 = 4;
pub const SAMPLER_PARAM_LOOP_START: u32 = 5;
pub const SAMPLER_PARAM_LOOP_END: u32 = 6;
pub const SAMPLER_PARAM_LOOP_MODE: u32 = 7;
pub const SAMPLER_PARAM_ATTACK: u32 = 8;
pub const SAMPLER_PARAM_DECAY: u32 = 9;
pub const SAMPLER_PARAM_SUSTAIN: u32 = 10;
pub const SAMPLER_PARAM_RELEASE: u32 = 11;
pub const SAMPLER_PARAM_FILTER_CUTOFF: u32 = 12;
pub const SAMPLER_PARAM_FILTER_RESONANCE: u32 = 13;
pub const SAMPLER_PARAM_FILTER_ENV_AMOUNT: u32 = 14;
pub const SAMPLER_PARAM_DRIVE: u32 = 15;
pub const SAMPLER_PARAM_BIT_REDUCTION: u32 = 16;
pub const SAMPLER_PARAM_RATE_REDUCTION: u32 = 17;
pub const SAMPLER_PARAM_VOICE_MODE: u32 = 18;
pub const SAMPLER_PARAM_POLYPHONY: u32 = 19;
pub const SAMPLER_PARAM_RETRIGGER_MODE: u32 = 20;
pub const SAMPLER_PARAM_ROOT_NOTE: u32 = 21;
pub const SAMPLER_PARAM_OUTPUT_GAIN: u32 = 22;
pub const SAMPLER_PARAM_FILTER_ATTACK: u32 = 23;
pub const SAMPLER_PARAM_FILTER_DECAY: u32 = 24;
pub const SAMPLER_PARAM_FILTER_SUSTAIN: u32 = 25;
pub const SAMPLER_PARAM_FILTER_RELEASE: u32 = 26;
pub const SAMPLER_PARAM_STRETCH_ENABLED: u32 = 27;
pub const SAMPLER_PARAM_STRETCH_MODE: u32 = 28;
pub const SAMPLER_PARAM_STRETCH_RATIO: u32 = 29;
pub const SAMPLER_PARAM_STRETCH_GRAIN: u32 = 30;
pub const SAMPLER_PARAM_STRETCH_SYNC: u32 = 31;
pub const SAMPLER_PARAM_STRETCH_BARS: u32 = 32;
pub const SAMPLER_PARAM_RETUNE_LIVE: u32 = 33;
pub const SAMPLER_PARAM_PLAY_MODE: u32 = 34;
pub const SAMPLER_PARAM_SLICE_BASE_NOTE: u32 = 35;

/// Envelope stages share this range across every generator. Exponential, so
/// the fast end where percussion lives gets most of the travel.
///
/// The three constructors below are `pub(crate)` because [`crate::mlp8`]
/// builds its own table from them: ML-P8 owns its ids, but a second copy of
/// "an envelope stage runs 1 ms to 8 s" would be a range written twice.
const ENV_MIN_S: f32 = 0.001;
const ENV_MAX_S: f32 = 8.0;

pub(crate) const fn unit(id: u32, name: &'static str, default: f32) -> ParamDescriptor {
    ParamDescriptor {
        id,
        name,
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default,
    }
}

pub(crate) const fn seconds(id: u32, name: &'static str, default: f32) -> ParamDescriptor {
    ParamDescriptor {
        id,
        name,
        unit: "s",
        min: ENV_MIN_S,
        max: ENV_MAX_S,
        curve: ParamCurve::Exponential,
        default,
    }
}

pub(crate) const fn stepped(id: u32, name: &'static str, steps: u8, default: f32) -> ParamDescriptor {
    ParamDescriptor {
        id,
        name,
        unit: "",
        min: 0.0,
        max: (steps - 1) as f32,
        curve: ParamCurve::Stepped(steps),
        default,
    }
}

static SAMPLER_DESCRIPTORS: [ParamDescriptor; 36] = [
    unit(SAMPLER_PARAM_START, "Start", 0.0),
    unit(SAMPLER_PARAM_END, "End", 1.0),
    stepped(SAMPLER_PARAM_REVERSE, "Reverse", 2, 0.0),
    ParamDescriptor {
        id: SAMPLER_PARAM_TUNE_SEMITONES,
        name: "Tune",
        unit: "st",
        min: -24.0,
        max: 24.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    ParamDescriptor {
        id: SAMPLER_PARAM_TUNE_CENTS,
        name: "Fine",
        unit: "ct",
        min: -100.0,
        max: 100.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    unit(SAMPLER_PARAM_LOOP_START, "Loop start", 0.0),
    unit(SAMPLER_PARAM_LOOP_END, "Loop end", 1.0),
    stepped(SAMPLER_PARAM_LOOP_MODE, "Loop", 3, 0.0),
    seconds(SAMPLER_PARAM_ATTACK, "Attack", 0.001),
    seconds(SAMPLER_PARAM_DECAY, "Decay", 0.25),
    unit(SAMPLER_PARAM_SUSTAIN, "Sustain", 1.0),
    seconds(SAMPLER_PARAM_RELEASE, "Release", 0.05),
    unit(SAMPLER_PARAM_FILTER_CUTOFF, "Cutoff", 1.0),
    unit(SAMPLER_PARAM_FILTER_RESONANCE, "Reso", 0.0),
    ParamDescriptor {
        id: SAMPLER_PARAM_FILTER_ENV_AMOUNT,
        name: "Env amt",
        unit: "",
        min: -1.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    unit(SAMPLER_PARAM_DRIVE, "Drive", 0.0),
    unit(SAMPLER_PARAM_BIT_REDUCTION, "Bits", 0.0),
    unit(SAMPLER_PARAM_RATE_REDUCTION, "Rate", 0.0),
    stepped(SAMPLER_PARAM_VOICE_MODE, "Voice mode", 2, 0.0),
    ParamDescriptor {
        id: SAMPLER_PARAM_POLYPHONY,
        name: "Voices",
        unit: "",
        min: 1.0,
        max: MAX_SAMPLER_VOICES as f32,
        curve: ParamCurve::Stepped(MAX_SAMPLER_VOICES),
        default: 1.0,
    },
    stepped(SAMPLER_PARAM_RETRIGGER_MODE, "Retrigger", 2, 0.0),
    ParamDescriptor {
        id: SAMPLER_PARAM_ROOT_NOTE,
        name: "Root",
        unit: "",
        min: 0.0,
        max: 127.0,
        curve: ParamCurve::Stepped(128),
        default: 60.0,
    },
    ParamDescriptor {
        id: SAMPLER_PARAM_OUTPUT_GAIN,
        name: "Output",
        // Linear rather than the knob's dB scale, for the reason
        // `STRIP_PARAM_VOLUME` gives: modulation depth is a fraction of the
        // normalized range, and the dB taper belongs to the control surface
        // rather than to the destination's numeric truth. The default is the
        // literal gain a fresh sampler starts at, so a lane written against
        // the descriptor and a knob left alone agree; the test below pins it
        // to `GENERATOR_OUTPUT_REFERENCE_DBFS`.
        unit: "x",
        min: 0.0,
        max: MAX_LINEAR_GAIN,
        curve: ParamCurve::Linear,
        default: 0.355_234_4,
    },
    // The filter envelope's own stages. Their defaults match the amplitude
    // envelope's, because a filter envelope that has never been given its own
    // shape follows the amplitude one -- reading either through the
    // descriptor has to agree with what the voice actually runs.
    seconds(SAMPLER_PARAM_FILTER_ATTACK, "Filter attack", 0.001),
    seconds(SAMPLER_PARAM_FILTER_DECAY, "Filter decay", 0.25),
    unit(SAMPLER_PARAM_FILTER_SUSTAIN, "Filter sustain", 1.0),
    seconds(SAMPLER_PARAM_FILTER_RELEASE, "Filter release", 0.05),
    // Stretch. `Stretch` is intent rather than state: the pool it needs is
    // allocated on the control thread, so the engine reconciles this rather
    // than the realtime drain acting on it. The other three are ordinary
    // parameters and are modulatable like any other.
    stepped(SAMPLER_PARAM_STRETCH_ENABLED, "Stretch", 2, 0.0),
    stepped(SAMPLER_PARAM_STRETCH_MODE, "Stretch mode", 3, 0.0),
    ParamDescriptor {
        id: SAMPLER_PARAM_STRETCH_RATIO,
        name: "Stretch",
        unit: "x",
        min: MIN_STRETCH_RATIO,
        max: MAX_STRETCH_RATIO,
        // Exponential, because the band that stays clean sits just around
        // unity while the ceiling is deliberately far past it. Linear travel
        // would spend almost all of the knob on extremes.
        curve: ParamCurve::Exponential,
        default: 1.0,
    },
    ParamDescriptor {
        id: SAMPLER_PARAM_STRETCH_GRAIN,
        name: "Grain",
        unit: "fr",
        min: MIN_STRETCH_GRAIN as f32,
        max: MAX_STRETCH_GRAIN as f32,
        // The window maps to a repetition frequency, so equal ratios of it
        // should feel like equal musical intervals.
        curve: ParamCurve::Exponential,
        default: 1024.0,
    },
    stepped(SAMPLER_PARAM_STRETCH_SYNC, "Fit to tempo", 2, 0.0),
    ParamDescriptor {
        id: SAMPLER_PARAM_STRETCH_BARS,
        name: "Bars",
        unit: "bar",
        min: MIN_STRETCH_BARS,
        max: MAX_STRETCH_BARS,
        // Exponential so the powers of two a loop actually lands on are
        // evenly spaced round the control, rather than everything below four
        // bars crowding into the first sixteenth of it.
        curve: ParamCurve::Exponential,
        default: 1.0,
    },
    // Defaults on: a tune knob that only takes effect on the next note is a
    // correctness gap, not a preference, so the ordinary case is fixed and
    // the historical one is opt-in.
    stepped(SAMPLER_PARAM_RETUNE_LIVE, "Live tune", 2, 1.0),
    // Slicing. Both are `Stepped`, so `mod_metadata` refuses to modulate
    // them: an LFO flapping the play mode or walking the keyboard map is
    // never what was meant.
    stepped(SAMPLER_PARAM_PLAY_MODE, "Play mode", 2, 0.0),
    ParamDescriptor {
        id: SAMPLER_PARAM_SLICE_BASE_NOTE,
        name: "Slice base",
        unit: "",
        min: 0.0,
        max: 127.0,
        curve: ParamCurve::Stepped(128),
        default: DEFAULT_SLICE_BASE_NOTE as f32,
    },
];

// --- Shared synth voice ----------------------------------------------------

/// First id of oscillator `n`'s block. Ten ids per oscillator, so a fourth
/// oscillator parameter can be appended without renumbering anything.
pub const fn synth_osc_param(oscillator: u32, offset: u32) -> u32 {
    100 + oscillator * 10 + offset
}

pub const OSC_OFFSET_WAVE: u32 = 0;
pub const OSC_OFFSET_SEMITONES: u32 = 1;
pub const OSC_OFFSET_CENTS: u32 = 2;
pub const OSC_OFFSET_LEVEL: u32 = 3;
pub const OSC_OFFSET_PULSE_WIDTH: u32 = 4;

pub const SYNTH_PARAM_GLIDE: u32 = 0;
pub const SYNTH_PARAM_ATTACK: u32 = 1;
pub const SYNTH_PARAM_DECAY: u32 = 2;
pub const SYNTH_PARAM_SUSTAIN: u32 = 3;
pub const SYNTH_PARAM_RELEASE: u32 = 4;
pub const SYNTH_PARAM_FILTER_CUTOFF: u32 = 5;
pub const SYNTH_PARAM_FILTER_RESONANCE: u32 = 6;
pub const SYNTH_PARAM_FILTER_ENV_AMOUNT: u32 = 7;
pub const SYNTH_PARAM_DRIVE: u32 = 8;
pub const SYNTH_PARAM_LFO_WAVE: u32 = 9;
pub const SYNTH_PARAM_LFO_RATE_HZ: u32 = 10;
pub const SYNTH_PARAM_LFO_TO_PITCH: u32 = 11;
pub const SYNTH_PARAM_LFO_TO_FILTER: u32 = 12;
pub const SYNTH_PARAM_LFO_TO_PULSE_WIDTH: u32 = 13;
pub const SYNTH_PARAM_LFO_TO_AMP: u32 = 14;
/// Poly only; the mono synth has no voice count or spread.
pub const SYNTH_PARAM_POLYPHONY: u32 = 15;
pub const SYNTH_PARAM_SPREAD: u32 = 16;

// 17-19 are deliberately unused, so the v1 synths keep room to grow without
// reaching into the ML-M1 block below.

/// The ML-M1's own ids, starting clear of everything above.
pub const SYNTH_PARAM_FILTER_ATTACK: u32 = 20;
pub const SYNTH_PARAM_FILTER_DECAY: u32 = 21;
pub const SYNTH_PARAM_FILTER_SUSTAIN: u32 = 22;
pub const SYNTH_PARAM_FILTER_RELEASE: u32 = 23;
pub const SYNTH_PARAM_FILTER_KEYTRACK: u32 = 24;
pub const SYNTH_PARAM_GLIDE_MODE: u32 = 25;
pub const SYNTH_PARAM_ENV_TRIGGER: u32 = 26;
pub const SYNTH_PARAM_NOTE_PRIORITY: u32 = 27;
pub const SYNTH_PARAM_FILTER_MODEL: u32 = 28;
pub const SYNTH_PARAM_ACCENT: u32 = 29;

const fn osc_descriptors(n: u32, name: &'static str) -> [ParamDescriptor; 5] {
    let (semitones, cents, level) = match n {
        0 => (0.0, 0.0, 1.0),
        1 => (12.0, 4.0, 0.0),
        _ => (-12.0, -4.0, 0.0),
    };
    [
        stepped(synth_osc_param(n, OSC_OFFSET_WAVE), name, 4, 2.0),
        ParamDescriptor {
            id: synth_osc_param(n, OSC_OFFSET_SEMITONES),
            name: "Semis",
            unit: "st",
            // Matches the knob's own travel. A depth is a fraction of this
            // range, so a narrower declaration would make a full-depth route
            // sweep less than the control visibly offers.
            min: -48.0,
            max: 48.0,
            curve: ParamCurve::Linear,
            default: semitones,
        },
        ParamDescriptor {
            id: synth_osc_param(n, OSC_OFFSET_CENTS),
            name: "Cents",
            unit: "ct",
            min: -100.0,
            max: 100.0,
            curve: ParamCurve::Linear,
            default: cents,
        },
        unit(synth_osc_param(n, OSC_OFFSET_LEVEL), "Level", level),
        ParamDescriptor {
            id: synth_osc_param(n, OSC_OFFSET_PULSE_WIDTH),
            name: "Width",
            unit: "",
            min: 0.05,
            max: 0.95,
            curve: ParamCurve::Linear,
            default: 0.5,
        },
    ]
}

/// The voice parameters every synth in the project shares: glide, one ADSR,
/// and the filter's cutoff, resonance, envelope depth, and drive. Split out
/// from the LFO block so the ML-M1, which has no device-local LFO, can
/// build its table from the same entries rather than a near-copy of them.
const SYNTH_CORE_DESCRIPTORS: [ParamDescriptor; 9] = [
    ParamDescriptor {
        id: SYNTH_PARAM_GLIDE,
        name: "Glide",
        unit: "s",
        min: 0.0,
        max: 2.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    seconds(SYNTH_PARAM_ATTACK, "Attack", 0.005),
    seconds(SYNTH_PARAM_DECAY, "Decay", 0.2),
    unit(SYNTH_PARAM_SUSTAIN, "Sustain", 0.7),
    seconds(SYNTH_PARAM_RELEASE, "Release", 0.15),
    unit(SYNTH_PARAM_FILTER_CUTOFF, "Cutoff", 1.0),
    unit(SYNTH_PARAM_FILTER_RESONANCE, "Reso", 0.0),
    ParamDescriptor {
        id: SYNTH_PARAM_FILTER_ENV_AMOUNT,
        name: "Env amt",
        unit: "",
        min: -1.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    unit(SYNTH_PARAM_DRIVE, "Drive", 0.0),
];

/// The v1 synths' device-local LFO. The ML-M1 does not have one; its
/// modulation comes from the channel's `ModRack` through these same
/// descriptor ids on the parameters themselves.
const LFO_DESCRIPTORS: [ParamDescriptor; 6] = [
    stepped(SYNTH_PARAM_LFO_WAVE, "LFO wave", 5, 0.0),
    ParamDescriptor {
        id: SYNTH_PARAM_LFO_RATE_HZ,
        name: "LFO rate",
        unit: "Hz",
        min: 0.01,
        max: 20.0,
        curve: ParamCurve::Exponential,
        default: 5.0,
    },
    ParamDescriptor {
        id: SYNTH_PARAM_LFO_TO_PITCH,
        name: "LFO pitch",
        unit: "st",
        min: -24.0,
        max: 24.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    ParamDescriptor {
        id: SYNTH_PARAM_LFO_TO_FILTER,
        name: "LFO filter",
        unit: "oct",
        min: -4.0,
        max: 4.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    ParamDescriptor {
        id: SYNTH_PARAM_LFO_TO_PULSE_WIDTH,
        name: "LFO width",
        unit: "",
        min: -0.45,
        max: 0.45,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    unit(SYNTH_PARAM_LFO_TO_AMP, "LFO amp", 0.0),
];

const SHARED_SYNTH_DESCRIPTORS: [ParamDescriptor; 15] =
    concat_core_lfo(SYNTH_CORE_DESCRIPTORS, LFO_DESCRIPTORS);

const fn concat_core_lfo(
    core: [ParamDescriptor; 9],
    lfo: [ParamDescriptor; 6],
) -> [ParamDescriptor; 15] {
    let mut out = [core[0]; 15];
    let mut i = 0;
    while i < 9 {
        out[i] = core[i];
        i += 1;
    }
    let mut j = 0;
    while j < 6 {
        out[9 + j] = lfo[j];
        j += 1;
    }
    out
}

/// The ML-M1's second envelope, its keytracking, and the three
/// switches that make its note behaviour a performance rather than a lookup.
const MLM1_VOICE_DESCRIPTORS: [ParamDescriptor; 10] = [
    seconds(SYNTH_PARAM_FILTER_ATTACK, "F attack", 0.005),
    seconds(SYNTH_PARAM_FILTER_DECAY, "F decay", 0.2),
    unit(SYNTH_PARAM_FILTER_SUSTAIN, "F sustain", 0.7),
    seconds(SYNTH_PARAM_FILTER_RELEASE, "F release", 0.15),
    unit(SYNTH_PARAM_FILTER_KEYTRACK, "Keytrack", 0.0),
    stepped(SYNTH_PARAM_GLIDE_MODE, "Glide mode", 2, 1.0),
    stepped(SYNTH_PARAM_ENV_TRIGGER, "Env trig", 2, 0.0),
    stepped(SYNTH_PARAM_NOTE_PRIORITY, "Priority", 3, 0.0),
    stepped(SYNTH_PARAM_FILTER_MODEL, "Model", 3, 0.0),
    unit(SYNTH_PARAM_ACCENT, "Accent", 0.0),
];

/// `SHARED_SYNTH_DESCRIPTORS` then three oscillator blocks. Written out rather
/// than concatenated at runtime so the table stays `static` and the engine
/// never allocates to enumerate it.
static MONO_DESCRIPTORS: [ParamDescriptor; 30] = concat_synth(
    SHARED_SYNTH_DESCRIPTORS,
    osc_descriptors(0, "Osc 1 wave"),
    osc_descriptors(1, "Osc 2 wave"),
    osc_descriptors(2, "Osc 3 wave"),
);

/// Built from the shared core rather than from `MONO_DESCRIPTORS`: the v2
/// mono synth is a different instrument, and a table that inherits from
/// another one quietly becomes a lie the moment the two diverge.
static MLM1_DESCRIPTORS: [ParamDescriptor; 34] = concat_mlm1(
    SYNTH_CORE_DESCRIPTORS,
    MLM1_VOICE_DESCRIPTORS,
    osc_descriptors(0, "Osc 1 wave"),
    osc_descriptors(1, "Osc 2 wave"),
    osc_descriptors(2, "Osc 3 wave"),
);

const fn concat_mlm1(
    core: [ParamDescriptor; 9],
    voice: [ParamDescriptor; 10],
    a: [ParamDescriptor; 5],
    b: [ParamDescriptor; 5],
    c: [ParamDescriptor; 5],
) -> [ParamDescriptor; 34] {
    let mut out = [core[0]; 34];
    let mut i = 0;
    while i < 9 {
        out[i] = core[i];
        i += 1;
    }
    let mut v = 0;
    while v < 10 {
        out[9 + v] = voice[v];
        v += 1;
    }
    let mut j = 0;
    while j < 5 {
        out[19 + j] = a[j];
        out[24 + j] = b[j];
        out[29 + j] = c[j];
        j += 1;
    }
    out
}

static POLY_DESCRIPTORS: [ParamDescriptor; 32] = {
    let mut out = [SHARED_SYNTH_DESCRIPTORS[0]; 32];
    let mono = MONO_DESCRIPTORS;
    let mut i = 0;
    while i < 30 {
        out[i] = mono[i];
        i += 1;
    }
    out[30] = ParamDescriptor {
        id: SYNTH_PARAM_POLYPHONY,
        name: "Voices",
        unit: "",
        min: 1.0,
        max: MAX_POLY_VOICES as f32,
        curve: ParamCurve::Stepped(MAX_POLY_VOICES),
        default: 8.0,
    };
    out[31] = unit(SYNTH_PARAM_SPREAD, "Spread", 0.0);
    out
};

const fn concat_synth(
    shared: [ParamDescriptor; 15],
    a: [ParamDescriptor; 5],
    b: [ParamDescriptor; 5],
    c: [ParamDescriptor; 5],
) -> [ParamDescriptor; 30] {
    let mut out = [shared[0]; 30];
    let mut i = 0;
    while i < 15 {
        out[i] = shared[i];
        i += 1;
    }
    let mut j = 0;
    while j < 5 {
        out[15 + j] = a[j];
        out[20 + j] = b[j];
        out[25 + j] = c[j];
        j += 1;
    }
    out
}

impl DeviceKind {
    /// This generator's parameter table, or empty for a kind that is not
    /// descriptor-addressed yet.
    ///
    /// The drum synth is the one still empty. Its twenty-five fields are three
    /// independent voices' worth of detail, and giving it a table is mechanical
    /// work rather than a design question — see
    /// `docs/plans/buffer-implementation/02-control-and-modulation.md`.
    pub fn descriptors(self) -> &'static [ParamDescriptor] {
        match self {
            Self::Sampler => &SAMPLER_DESCRIPTORS,
            Self::MonoSynth => &MONO_DESCRIPTORS,
            Self::PolySynth => &POLY_DESCRIPTORS,
            Self::MlM1 => &MLM1_DESCRIPTORS,
            // ML-P8's table lives beside its parameters in `mlp8.rs`. It is
            // the one generator whose ids are not the shared synth ids, so
            // keeping it here would put two unrelated numbering schemes in one
            // file and invite a collision that neither one can see.
            Self::MlP8 => &crate::mlp8::DESCRIPTORS,
            Self::DrumSynth => &[],
        }
    }

    pub fn descriptor(self, id: u32) -> Option<&'static ParamDescriptor> {
        self.descriptors().iter().find(|d| d.id == id)
    }

    /// The fields of one of this device's own internal modulation routes.
    ///
    /// Empty for every device that has none, which is all of them but the
    /// ML-P8. Separate from [`Self::descriptors`] because these are not
    /// controls of the device: they belong to a route, they are addressed
    /// through [`crate::ParamOwner::SourceRoute`] by the route's durable id,
    /// and a device with sixteen of them does not thereby have sixteen times
    /// as many parameters.
    pub fn route_descriptors(self) -> &'static [ParamDescriptor] {
        match self {
            Self::MlP8 => &crate::mlp8::ROUTE_DESCRIPTORS,
            _ => &[],
        }
    }

    pub fn route_descriptor(self, param: u32) -> Option<&'static ParamDescriptor> {
        self.route_descriptors().iter().find(|d| d.id == param)
    }

    /// Whether this device authors its own internal modulation routes at all.
    pub fn has_internal_routes(self) -> bool {
        !self.route_descriptors().is_empty()
    }
}

// --- Read/write ------------------------------------------------------------

fn osc_get(osc: &OscParams, offset: u32) -> Option<f32> {
    Some(match offset {
        OSC_OFFSET_WAVE => osc.wave.to_index() as f32,
        OSC_OFFSET_SEMITONES => osc.semitones,
        OSC_OFFSET_CENTS => osc.cents,
        OSC_OFFSET_LEVEL => osc.level,
        OSC_OFFSET_PULSE_WIDTH => osc.pulse_width,
        _ => return None,
    })
}

fn osc_set(osc: &mut OscParams, offset: u32, value: f32) -> bool {
    match offset {
        OSC_OFFSET_WAVE => osc.wave = OscWave::from_index(value.round() as i32),
        OSC_OFFSET_SEMITONES => osc.semitones = value,
        OSC_OFFSET_CENTS => osc.cents = value,
        OSC_OFFSET_LEVEL => osc.level = value,
        OSC_OFFSET_PULSE_WIDTH => osc.pulse_width = value,
        _ => return false,
    }
    true
}

/// Split an id into `(oscillator, offset)` when it lands in an oscillator
/// block. Returns `None` for the shared parameters below 100.
fn osc_slot(id: u32) -> Option<(usize, u32)> {
    let index = id.checked_sub(100)?;
    let oscillator = (index / 10) as usize;
    (oscillator < 3).then_some((oscillator, index % 10))
}

fn lfo_get(lfo: &LfoParams, id: u32) -> Option<f32> {
    Some(match id {
        SYNTH_PARAM_LFO_WAVE => lfo.wave.to_index() as f32,
        SYNTH_PARAM_LFO_RATE_HZ => lfo.rate_hz,
        SYNTH_PARAM_LFO_TO_PITCH => lfo.to_pitch,
        SYNTH_PARAM_LFO_TO_FILTER => lfo.to_filter,
        SYNTH_PARAM_LFO_TO_PULSE_WIDTH => lfo.to_pulse_width,
        SYNTH_PARAM_LFO_TO_AMP => lfo.to_amp,
        _ => return None,
    })
}

fn lfo_set(lfo: &mut LfoParams, id: u32, value: f32) -> bool {
    match id {
        SYNTH_PARAM_LFO_WAVE => lfo.wave = LfoWave::from_index(value.round() as i32),
        SYNTH_PARAM_LFO_RATE_HZ => lfo.rate_hz = value,
        SYNTH_PARAM_LFO_TO_PITCH => lfo.to_pitch = value,
        SYNTH_PARAM_LFO_TO_FILTER => lfo.to_filter = value,
        SYNTH_PARAM_LFO_TO_PULSE_WIDTH => lfo.to_pulse_width = value,
        SYNTH_PARAM_LFO_TO_AMP => lfo.to_amp = value,
        _ => return false,
    }
    true
}

/// One channel's generator parameters, tagged by kind. The mirror of
/// `EffectParams`, and it exists for the same reason: one type the engine can
/// hold as the authoritative base while the device holds only the resolved
/// value it was last sent.
// This type is `Copy` and is the value shuttle for parameter changes, so
// boxing the large variant is not available: it would both drop `Copy` and
// put an allocation on a path that exists to avoid one.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GeneratorParams {
    Sampler(SamplerParams),
    MonoSynth(MonoSynthParams),
    PolySynth(PolySynthParams),
    MlM1(MlM1Params),
    MlP8(MlP8Params),
    /// Not addressable yet; every `get`/`set` misses.
    DrumSynth,
}

impl GeneratorParams {
    /// This generator's own internal modulation routes, for the one kind that
    /// has them.
    ///
    /// Typed as the ML-P8's list rather than hidden behind a trait: there is
    /// exactly one device with internal routes, and a trait with one
    /// implementor would say less about the design than this does.
    pub fn internal_routes(&self) -> Option<&crate::mlp8::MlP8Routes> {
        match self {
            Self::MlP8(p) => Some(&p.routes),
            _ => None,
        }
    }

    pub fn internal_routes_mut(&mut self) -> Option<&mut crate::mlp8::MlP8Routes> {
        match self {
            Self::MlP8(p) => Some(&mut p.routes),
            _ => None,
        }
    }

    pub fn kind(&self) -> DeviceKind {
        match self {
            Self::Sampler(_) => DeviceKind::Sampler,
            Self::MonoSynth(_) => DeviceKind::MonoSynth,
            Self::PolySynth(_) => DeviceKind::PolySynth,
            Self::MlM1(_) => DeviceKind::MlM1,
            Self::MlP8(_) => DeviceKind::MlP8,
            Self::DrumSynth => DeviceKind::DrumSynth,
        }
    }

    /// Read one parameter in natural units by wire id.
    pub fn get(&self, id: u32) -> Option<f32> {
        match self {
            Self::Sampler(p) => Some(match id {
                SAMPLER_PARAM_START => p.start,
                SAMPLER_PARAM_END => p.end,
                SAMPLER_PARAM_REVERSE => f32::from(u8::from(p.reverse)),
                SAMPLER_PARAM_TUNE_SEMITONES => p.tune_semitones,
                SAMPLER_PARAM_TUNE_CENTS => p.tune_cents,
                SAMPLER_PARAM_LOOP_START => p.loop_start,
                SAMPLER_PARAM_LOOP_END => p.loop_end,
                SAMPLER_PARAM_LOOP_MODE => p.loop_mode.to_index() as f32,
                SAMPLER_PARAM_ATTACK => p.attack,
                SAMPLER_PARAM_DECAY => p.decay,
                SAMPLER_PARAM_SUSTAIN => p.sustain,
                SAMPLER_PARAM_RELEASE => p.release,
                SAMPLER_PARAM_FILTER_CUTOFF => p.filter_cutoff,
                SAMPLER_PARAM_FILTER_RESONANCE => p.filter_resonance,
                SAMPLER_PARAM_FILTER_ENV_AMOUNT => p.filter_env_amount,
                SAMPLER_PARAM_DRIVE => p.drive,
                SAMPLER_PARAM_BIT_REDUCTION => p.bit_reduction,
                SAMPLER_PARAM_RATE_REDUCTION => p.rate_reduction,
                SAMPLER_PARAM_VOICE_MODE => p.voice_mode.to_index() as f32,
                SAMPLER_PARAM_POLYPHONY => f32::from(p.polyphony),
                SAMPLER_PARAM_RETRIGGER_MODE => p.retrigger_mode.to_index() as f32,
                SAMPLER_PARAM_ROOT_NOTE => f32::from(p.root_note),
                SAMPLER_PARAM_OUTPUT_GAIN => p.output_gain,
                // Read through the resolution, so a lane pointed at a filter
                // stage reads what the voice runs rather than a placeholder
                // while the envelope is still following the amplitude one.
                SAMPLER_PARAM_FILTER_ATTACK => p.resolved_filter_env().attack,
                SAMPLER_PARAM_FILTER_DECAY => p.resolved_filter_env().decay,
                SAMPLER_PARAM_FILTER_SUSTAIN => p.resolved_filter_env().sustain,
                SAMPLER_PARAM_FILTER_RELEASE => p.resolved_filter_env().release,
                SAMPLER_PARAM_STRETCH_ENABLED => f32::from(u8::from(p.stretch_enabled)),
                SAMPLER_PARAM_STRETCH_MODE => match p.stretch_mode {
                    StretchMode::Music => 0.0,
                    StretchMode::Drums => 1.0,
                    StretchMode::Grain => 2.0,
                },
                SAMPLER_PARAM_STRETCH_RATIO => p.stretch_ratio,
                SAMPLER_PARAM_STRETCH_GRAIN => f32::from(p.stretch_grain),
                SAMPLER_PARAM_STRETCH_SYNC => f32::from(u8::from(p.stretch_sync)),
                SAMPLER_PARAM_STRETCH_BARS => p.stretch_bars,
                SAMPLER_PARAM_RETUNE_LIVE => f32::from(u8::from(p.retune_live)),
                SAMPLER_PARAM_PLAY_MODE => p.play_mode.to_index() as f32,
                SAMPLER_PARAM_SLICE_BASE_NOTE => f32::from(p.slice_base_note),
                _ => return None,
            }),
            Self::MonoSynth(p) => {
                if let Some((oscillator, offset)) = osc_slot(id) {
                    return osc_get(&p.osc[oscillator], offset);
                }
                if let Some(value) = lfo_get(&p.lfo, id) {
                    return Some(value);
                }
                Some(match id {
                    SYNTH_PARAM_GLIDE => p.glide,
                    SYNTH_PARAM_ATTACK => p.attack,
                    SYNTH_PARAM_DECAY => p.decay,
                    SYNTH_PARAM_SUSTAIN => p.sustain,
                    SYNTH_PARAM_RELEASE => p.release,
                    SYNTH_PARAM_FILTER_CUTOFF => p.filter_cutoff,
                    SYNTH_PARAM_FILTER_RESONANCE => p.filter_resonance,
                    SYNTH_PARAM_FILTER_ENV_AMOUNT => p.filter_env_amount,
                    SYNTH_PARAM_DRIVE => p.drive,
                    _ => return None,
                })
            }
            Self::PolySynth(p) => {
                if let Some((oscillator, offset)) = osc_slot(id) {
                    return osc_get(&p.osc[oscillator], offset);
                }
                if let Some(value) = lfo_get(&p.lfo, id) {
                    return Some(value);
                }
                Some(match id {
                    SYNTH_PARAM_GLIDE => p.glide,
                    SYNTH_PARAM_ATTACK => p.attack,
                    SYNTH_PARAM_DECAY => p.decay,
                    SYNTH_PARAM_SUSTAIN => p.sustain,
                    SYNTH_PARAM_RELEASE => p.release,
                    SYNTH_PARAM_FILTER_CUTOFF => p.filter_cutoff,
                    SYNTH_PARAM_FILTER_RESONANCE => p.filter_resonance,
                    SYNTH_PARAM_FILTER_ENV_AMOUNT => p.filter_env_amount,
                    SYNTH_PARAM_DRIVE => p.drive,
                    SYNTH_PARAM_POLYPHONY => f32::from(p.polyphony),
                    SYNTH_PARAM_SPREAD => p.spread,
                    _ => return None,
                })
            }
            Self::MlM1(p) => {
                if let Some((oscillator, offset)) = osc_slot(id) {
                    return osc_get(&p.osc[oscillator], offset);
                }
                Some(match id {
                    SYNTH_PARAM_GLIDE => p.glide,
                    SYNTH_PARAM_ATTACK => p.attack,
                    SYNTH_PARAM_DECAY => p.decay,
                    SYNTH_PARAM_SUSTAIN => p.sustain,
                    SYNTH_PARAM_RELEASE => p.release,
                    SYNTH_PARAM_FILTER_CUTOFF => p.filter_cutoff,
                    SYNTH_PARAM_FILTER_RESONANCE => p.filter_resonance,
                    SYNTH_PARAM_FILTER_ENV_AMOUNT => p.filter_env_amount,
                    SYNTH_PARAM_DRIVE => p.drive,
                    SYNTH_PARAM_FILTER_ATTACK => p.filter_attack,
                    SYNTH_PARAM_FILTER_DECAY => p.filter_decay,
                    SYNTH_PARAM_FILTER_SUSTAIN => p.filter_sustain,
                    SYNTH_PARAM_FILTER_RELEASE => p.filter_release,
                    SYNTH_PARAM_FILTER_KEYTRACK => p.filter_keytrack,
                    SYNTH_PARAM_GLIDE_MODE => p.glide_mode.to_index() as f32,
                    SYNTH_PARAM_ENV_TRIGGER => p.env_trigger.to_index() as f32,
                    SYNTH_PARAM_NOTE_PRIORITY => p.priority.to_index() as f32,
                    SYNTH_PARAM_FILTER_MODEL => p.filter_model.to_index() as f32,
                    SYNTH_PARAM_ACCENT => p.accent,
                    _ => return None,
                })
            }
            Self::MlP8(p) => crate::mlp8::get(p, id),
            Self::DrumSynth => None,
        }
    }

    /// Write one parameter in natural units by wire id, clamped through its
    /// descriptor. Returns the stored value, or `None` for an unknown id.
    pub fn set(&mut self, id: u32, value: f32) -> Option<f32> {
        let descriptor = self.kind().descriptor(id)?;
        let value = descriptor.clamp_natural(value);
        match self {
            Self::Sampler(p) => match id {
                SAMPLER_PARAM_START => p.start = value,
                SAMPLER_PARAM_END => p.end = value,
                SAMPLER_PARAM_REVERSE => p.reverse = value.round() > 0.0,
                SAMPLER_PARAM_TUNE_SEMITONES => p.tune_semitones = value,
                SAMPLER_PARAM_TUNE_CENTS => p.tune_cents = value,
                SAMPLER_PARAM_LOOP_START => p.loop_start = value,
                SAMPLER_PARAM_LOOP_END => p.loop_end = value,
                SAMPLER_PARAM_LOOP_MODE => p.loop_mode = LoopMode::from_index(value.round() as i32),
                SAMPLER_PARAM_ATTACK => p.attack = value,
                SAMPLER_PARAM_DECAY => p.decay = value,
                SAMPLER_PARAM_SUSTAIN => p.sustain = value,
                SAMPLER_PARAM_RELEASE => p.release = value,
                SAMPLER_PARAM_FILTER_CUTOFF => p.filter_cutoff = value,
                SAMPLER_PARAM_FILTER_RESONANCE => p.filter_resonance = value,
                SAMPLER_PARAM_FILTER_ENV_AMOUNT => p.filter_env_amount = value,
                SAMPLER_PARAM_DRIVE => p.drive = value,
                SAMPLER_PARAM_BIT_REDUCTION => p.bit_reduction = value,
                SAMPLER_PARAM_RATE_REDUCTION => p.rate_reduction = value,
                SAMPLER_PARAM_VOICE_MODE => {
                    p.voice_mode = VoiceMode::from_index(value.round() as i32)
                }
                SAMPLER_PARAM_POLYPHONY => p.polyphony = value.round() as u8,
                SAMPLER_PARAM_RETRIGGER_MODE => {
                    p.retrigger_mode = RetriggerMode::from_index(value.round() as i32)
                }
                SAMPLER_PARAM_ROOT_NOTE => p.root_note = value.round() as u8,
                SAMPLER_PARAM_OUTPUT_GAIN => p.output_gain = value,
                // Writing any stage gives the filter envelope its own shape,
                // seeded from wherever it was reading.
                SAMPLER_PARAM_FILTER_ATTACK => p.filter_env_mut().attack = value,
                SAMPLER_PARAM_FILTER_DECAY => p.filter_env_mut().decay = value,
                SAMPLER_PARAM_FILTER_SUSTAIN => p.filter_env_mut().sustain = value,
                SAMPLER_PARAM_FILTER_RELEASE => p.filter_env_mut().release = value,
                // Intent only: the engine provisions or reclaims the stretch
                // pool off the realtime thread in response. See
                // `SamplerParams::stretch_enabled`.
                SAMPLER_PARAM_STRETCH_ENABLED => p.stretch_enabled = value >= 0.5,
                SAMPLER_PARAM_STRETCH_MODE => {
                    p.stretch_mode = match value.round() as i32 {
                        1 => StretchMode::Drums,
                        2 => StretchMode::Grain,
                        _ => StretchMode::Music,
                    }
                }
                SAMPLER_PARAM_STRETCH_RATIO => {
                    p.stretch_ratio = value.clamp(MIN_STRETCH_RATIO, MAX_STRETCH_RATIO)
                }
                SAMPLER_PARAM_STRETCH_GRAIN => {
                    p.stretch_grain = (value.round() as i32)
                        .clamp(i32::from(MIN_STRETCH_GRAIN), i32::from(MAX_STRETCH_GRAIN))
                        as u16
                }
                SAMPLER_PARAM_STRETCH_SYNC => p.stretch_sync = value >= 0.5,
                SAMPLER_PARAM_STRETCH_BARS => {
                    p.stretch_bars = value.clamp(MIN_STRETCH_BARS, MAX_STRETCH_BARS)
                }
                SAMPLER_PARAM_RETUNE_LIVE => p.retune_live = value >= 0.5,
                SAMPLER_PARAM_PLAY_MODE => {
                    p.play_mode = PlayMode::from_index(value.round() as i32)
                }
                SAMPLER_PARAM_SLICE_BASE_NOTE => p.slice_base_note = value.round() as u8,
                _ => return None,
            },
            Self::MonoSynth(p) => {
                if let Some((oscillator, offset)) = osc_slot(id) {
                    if !osc_set(&mut p.osc[oscillator], offset, value) {
                        return None;
                    }
                } else if !lfo_set(&mut p.lfo, id, value) {
                    match id {
                        SYNTH_PARAM_GLIDE => p.glide = value,
                        SYNTH_PARAM_ATTACK => p.attack = value,
                        SYNTH_PARAM_DECAY => p.decay = value,
                        SYNTH_PARAM_SUSTAIN => p.sustain = value,
                        SYNTH_PARAM_RELEASE => p.release = value,
                        SYNTH_PARAM_FILTER_CUTOFF => p.filter_cutoff = value,
                        SYNTH_PARAM_FILTER_RESONANCE => p.filter_resonance = value,
                        SYNTH_PARAM_FILTER_ENV_AMOUNT => p.filter_env_amount = value,
                        SYNTH_PARAM_DRIVE => p.drive = value,
                        _ => return None,
                    }
                }
            }
            Self::PolySynth(p) => {
                if let Some((oscillator, offset)) = osc_slot(id) {
                    if !osc_set(&mut p.osc[oscillator], offset, value) {
                        return None;
                    }
                } else if !lfo_set(&mut p.lfo, id, value) {
                    match id {
                        SYNTH_PARAM_GLIDE => p.glide = value,
                        SYNTH_PARAM_ATTACK => p.attack = value,
                        SYNTH_PARAM_DECAY => p.decay = value,
                        SYNTH_PARAM_SUSTAIN => p.sustain = value,
                        SYNTH_PARAM_RELEASE => p.release = value,
                        SYNTH_PARAM_FILTER_CUTOFF => p.filter_cutoff = value,
                        SYNTH_PARAM_FILTER_RESONANCE => p.filter_resonance = value,
                        SYNTH_PARAM_FILTER_ENV_AMOUNT => p.filter_env_amount = value,
                        SYNTH_PARAM_DRIVE => p.drive = value,
                        SYNTH_PARAM_POLYPHONY => p.polyphony = value.round() as u8,
                        SYNTH_PARAM_SPREAD => p.spread = value,
                        _ => return None,
                    }
                }
            }
            Self::MlM1(p) => {
                if let Some((oscillator, offset)) = osc_slot(id) {
                    if !osc_set(&mut p.osc[oscillator], offset, value) {
                        return None;
                    }
                } else {
                    match id {
                        SYNTH_PARAM_GLIDE => p.glide = value,
                        SYNTH_PARAM_ATTACK => p.attack = value,
                        SYNTH_PARAM_DECAY => p.decay = value,
                        SYNTH_PARAM_SUSTAIN => p.sustain = value,
                        SYNTH_PARAM_RELEASE => p.release = value,
                        SYNTH_PARAM_FILTER_CUTOFF => p.filter_cutoff = value,
                        SYNTH_PARAM_FILTER_RESONANCE => p.filter_resonance = value,
                        SYNTH_PARAM_FILTER_ENV_AMOUNT => p.filter_env_amount = value,
                        SYNTH_PARAM_DRIVE => p.drive = value,
                        SYNTH_PARAM_FILTER_ATTACK => p.filter_attack = value,
                        SYNTH_PARAM_FILTER_DECAY => p.filter_decay = value,
                        SYNTH_PARAM_FILTER_SUSTAIN => p.filter_sustain = value,
                        SYNTH_PARAM_FILTER_RELEASE => p.filter_release = value,
                        SYNTH_PARAM_FILTER_KEYTRACK => p.filter_keytrack = value,
                        SYNTH_PARAM_GLIDE_MODE => {
                            p.glide_mode = GlideMode::from_index(value.round() as i32)
                        }
                        SYNTH_PARAM_ENV_TRIGGER => {
                            p.env_trigger = EnvTrigger::from_index(value.round() as i32)
                        }
                        SYNTH_PARAM_NOTE_PRIORITY => {
                            p.priority = NotePriority::from_index(value.round() as i32)
                        }
                        SYNTH_PARAM_FILTER_MODEL => {
                            p.filter_model = FilterModel::from_index(value.round() as i32)
                        }
                        SYNTH_PARAM_ACCENT => p.accent = value,
                        _ => return None,
                    }
                }
            }
            Self::MlP8(p) => {
                if !crate::mlp8::set(p, id, value) {
                    return None;
                }
            }
            Self::DrumSynth => return None,
        }
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn all() -> [GeneratorParams; 5] {
        [
            GeneratorParams::Sampler(SamplerParams::default()),
            GeneratorParams::MonoSynth(MonoSynthParams::default()),
            GeneratorParams::PolySynth(PolySynthParams::default()),
            GeneratorParams::MlM1(MlM1Params::default()),
            GeneratorParams::MlP8(crate::MlP8Params::default()),
        ]
    }

    #[test]
    fn every_described_parameter_reads_and_writes() {
        for mut params in all() {
            let kind = params.kind();
            for descriptor in kind.descriptors() {
                assert!(
                    params.get(descriptor.id).is_some(),
                    "{:?} describes {} but cannot read it",
                    kind,
                    descriptor.name
                );
                let midpoint = descriptor.from_normalized(0.5);
                let stored = params.set(descriptor.id, midpoint).unwrap_or_else(|| {
                    panic!("{:?} describes {} but cannot write it", kind, descriptor.name)
                });
                let read_back = params.get(descriptor.id).expect("just written");
                // Stepped parameters quantize, so compare against what `set`
                // reported rather than against the value handed in.
                assert!(
                    (read_back - stored).abs() <= stored.abs() * 0.02 + 0.51,
                    "{:?} {} round-tripped {stored} as {read_back}",
                    kind,
                    descriptor.name
                );
            }
        }
    }

    #[test]
    fn parameter_ids_are_unique_within_a_kind() {
        for params in all() {
            let kind = params.kind();
            let mut seen = HashSet::new();
            for descriptor in kind.descriptors() {
                assert!(
                    seen.insert(descriptor.id),
                    "{:?} reuses parameter id {}",
                    kind,
                    descriptor.id
                );
            }
        }
    }

    #[test]
    fn synth_descriptor_defaults_match_parameter_defaults() {
        for params in [
            GeneratorParams::MonoSynth(MonoSynthParams::default()),
            GeneratorParams::PolySynth(PolySynthParams::default()),
            GeneratorParams::MlM1(MlM1Params::default()),
            GeneratorParams::MlP8(crate::MlP8Params::default()),
        ] {
            let kind = params.kind();
            for descriptor in kind.descriptors() {
                let actual = params
                    .get(descriptor.id)
                    .expect("every synth descriptor reads from its parameter struct");
                assert!(
                    (actual - descriptor.default).abs() <= 1.0e-6,
                    "{:?} {} defaults to {actual}, descriptor says {}",
                    kind,
                    descriptor.name,
                    descriptor.default
                );
            }
        }
    }

    /// The descriptor's default is a literal, because `ParamDescriptor` is a
    /// const struct and `db_to_linear` is not a const fn. Pin it to the
    /// parameter default and to the generator output reference so the two
    /// cannot drift: an automation lane written against the descriptor and an
    /// untouched knob have to mean the same gain.
    #[test]
    fn the_sampler_output_descriptor_defaults_to_the_operating_level() {
        let params = GeneratorParams::Sampler(SamplerParams::default());
        let descriptor = DeviceKind::Sampler
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.id == SAMPLER_PARAM_OUTPUT_GAIN)
            .expect("the sampler describes its output gain");
        let actual = params.get(SAMPLER_PARAM_OUTPUT_GAIN).unwrap();
        assert!((actual - descriptor.default).abs() <= 1.0e-6, "{actual}");
        assert!(
            (crate::gain::linear_to_db(actual) - crate::gain::GENERATOR_OUTPUT_REFERENCE_DBFS).abs()
                <= 0.01,
            "a fresh sampler trims to {} dB, want the generator output reference",
            crate::gain::linear_to_db(actual)
        );
    }

    #[test]
    fn the_three_oscillator_blocks_are_independent() {
        let mut params = GeneratorParams::MonoSynth(MonoSynthParams::default());
        for oscillator in 0..3u32 {
            params.set(
                synth_osc_param(oscillator, OSC_OFFSET_LEVEL),
                oscillator as f32 / 4.0,
            );
        }
        for oscillator in 0..3u32 {
            let level = params
                .get(synth_osc_param(oscillator, OSC_OFFSET_LEVEL))
                .expect("oscillator level is addressable");
            assert!((level - oscillator as f32 / 4.0).abs() < 1e-6);
        }
    }

    #[test]
    fn an_unknown_id_misses_rather_than_writing_something_else() {
        let mut params = GeneratorParams::Sampler(SamplerParams::default());
        assert_eq!(params.get(9_999), None);
        assert_eq!(params.set(9_999, 1.0), None);
        // An oscillator id on a sampler is a miss, not a stray write.
        assert_eq!(params.set(synth_osc_param(0, OSC_OFFSET_LEVEL), 1.0), None);
    }

    #[test]
    fn the_drum_synth_is_honestly_empty_rather_than_partially_addressable() {
        let mut drum = GeneratorParams::DrumSynth;
        assert!(DeviceKind::DrumSynth.descriptors().is_empty());
        assert_eq!(drum.get(0), None);
        assert_eq!(drum.set(0, 1.0), None);
    }

    #[test]
    fn values_are_clamped_through_the_descriptor_on_the_way_in() {
        let mut params = GeneratorParams::Sampler(SamplerParams::default());
        assert_eq!(params.set(SAMPLER_PARAM_START, 5.0), Some(1.0));
        assert_eq!(params.set(SAMPLER_PARAM_START, -5.0), Some(0.0));
        let voices = params
            .set(SAMPLER_PARAM_POLYPHONY, 999.0)
            .expect("polyphony is addressable");
        assert_eq!(voices, MAX_SAMPLER_VOICES as f32);
    }
}
