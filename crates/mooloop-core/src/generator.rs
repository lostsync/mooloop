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
    DeviceKind, LfoParams, LfoWave, LoopMode, MonoSynthParams, OscParams, OscWave, ParamCurve,
    ParamDescriptor, PolySynthParams, RetriggerMode, SamplerParams, VoiceMode, MAX_POLY_VOICES,
    MAX_SAMPLER_VOICES,
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

/// Envelope stages share this range across every generator. Exponential, so
/// the fast end where percussion lives gets most of the travel.
const ENV_MIN_S: f32 = 0.001;
const ENV_MAX_S: f32 = 8.0;

const fn unit(id: u32, name: &'static str, default: f32) -> ParamDescriptor {
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

const fn seconds(id: u32, name: &'static str, default: f32) -> ParamDescriptor {
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

const fn stepped(id: u32, name: &'static str, steps: u8, default: f32) -> ParamDescriptor {
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

static SAMPLER_DESCRIPTORS: [ParamDescriptor; 22] = [
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
        curve: ParamCurve::Stepped(MAX_SAMPLER_VOICES as u8),
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

const fn osc_descriptors(n: u32, name: &'static str) -> [ParamDescriptor; 5] {
    [
        stepped(synth_osc_param(n, OSC_OFFSET_WAVE), name, 4, 2.0),
        ParamDescriptor {
            id: synth_osc_param(n, OSC_OFFSET_SEMITONES),
            name: "Semis",
            unit: "st",
            min: -24.0,
            max: 24.0,
            curve: ParamCurve::Linear,
            default: 0.0,
        },
        ParamDescriptor {
            id: synth_osc_param(n, OSC_OFFSET_CENTS),
            name: "Cents",
            unit: "ct",
            min: -100.0,
            max: 100.0,
            curve: ParamCurve::Linear,
            default: 0.0,
        },
        unit(synth_osc_param(n, OSC_OFFSET_LEVEL), "Level", 0.0),
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

const SHARED_SYNTH_DESCRIPTORS: [ParamDescriptor; 15] = [
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
    unit(SYNTH_PARAM_FILTER_CUTOFF, "Cutoff", 0.7),
    unit(SYNTH_PARAM_FILTER_RESONANCE, "Reso", 0.1),
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

/// `SHARED_SYNTH_DESCRIPTORS` then three oscillator blocks. Written out rather
/// than concatenated at runtime so the table stays `static` and the engine
/// never allocates to enumerate it.
static MONO_DESCRIPTORS: [ParamDescriptor; 30] = concat_synth(
    SHARED_SYNTH_DESCRIPTORS,
    osc_descriptors(0, "Osc 1 wave"),
    osc_descriptors(1, "Osc 2 wave"),
    osc_descriptors(2, "Osc 3 wave"),
);

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
        curve: ParamCurve::Stepped(MAX_POLY_VOICES as u8),
        default: 8.0,
    };
    out[31] = unit(SYNTH_PARAM_SPREAD, "Spread", 0.3);
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
            Self::DrumSynth => &[],
        }
    }

    pub fn descriptor(self, id: u32) -> Option<&'static ParamDescriptor> {
        self.descriptors().iter().find(|d| d.id == id)
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
    (oscillator < 3).then(|| (oscillator, index % 10))
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
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GeneratorParams {
    Sampler(SamplerParams),
    MonoSynth(MonoSynthParams),
    PolySynth(PolySynthParams),
    /// Not addressable yet; every `get`/`set` misses.
    DrumSynth,
}

impl GeneratorParams {
    pub fn kind(&self) -> DeviceKind {
        match self {
            Self::Sampler(_) => DeviceKind::Sampler,
            Self::MonoSynth(_) => DeviceKind::MonoSynth,
            Self::PolySynth(_) => DeviceKind::PolySynth,
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
            Self::DrumSynth => return None,
        }
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn all() -> [GeneratorParams; 3] {
        [
            GeneratorParams::Sampler(SamplerParams::default()),
            GeneratorParams::MonoSynth(MonoSynthParams::default()),
            GeneratorParams::PolySynth(PolySynthParams::default()),
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
