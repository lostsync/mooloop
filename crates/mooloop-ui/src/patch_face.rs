//! What an open box's face shows (`docs/plans/song-patch/05-faces-bends-and-cable-activity.md`):
//! which of its settings get a knob, in what order, under what label, and
//! how each reads. Free of Slint, so the canvas lays faces out as plain data.
//!
//! A box shows the settings that change what it does now: an LFO in sync
//! shows its division and not its rate, and the other way round, so one knob
//! reads as the box's speed whichever way it is set. A box's name is not a
//! knob: the op of `* 0.5` is retyped, not turned.

use mooloop_core::box_text::{division_name, format_number, shape_name};
use mooloop_core::{
    ModulatorParams, COUNTER_PARAM_STEPS, ENV_PARAM_AMOUNT, ENV_PARAM_ATTACK_DIVISION,
    ENV_PARAM_ATTACK_S, ENV_PARAM_ATTACK_SYNC, ENV_PARAM_DECAY_DIVISION, ENV_PARAM_DECAY_S,
    ENV_PARAM_DECAY_SYNC, ENV_PARAM_RELEASE_DIVISION, ENV_PARAM_RELEASE_S, ENV_PARAM_RELEASE_SYNC,
    ENV_PARAM_SUSTAIN, LFO_PARAM_DEPTH, LFO_PARAM_FADE_IN_DIVISION, LFO_PARAM_FADE_IN_S,
    LFO_PARAM_FADE_IN_SYNC, LFO_PARAM_PHASE, LFO_PARAM_PULSE_WIDTH, LFO_PARAM_RATE_DIVISION,
    LFO_PARAM_RATE_HZ, LFO_PARAM_RETRIGGER, LFO_PARAM_SMOOTHING_S, LFO_PARAM_TEMPO_SYNC,
    LFO_PARAM_WAVEFORM, MATH_PARAM_CLAMP_HIGH, MATH_PARAM_CLAMP_LOW, MATH_PARAM_OPERAND,
    MOD_STEP_MAX_STEPS, ModMathOp, ParamCurve, ParamDescriptor, RANDOM_PARAM_BIPOLAR,
    RANDOM_PARAM_DRUNK, RANDOM_PARAM_PROBABILITY, RANDOM_PARAM_QUANTIZE, RANDOM_PARAM_RATE_DIVISION,
    RANDOM_PARAM_RATE_HZ, RANDOM_PARAM_TEMPO_SYNC, RANDOM_PARAM_TRIGGER, RANDOM_PARAM_WALK,
    SELECT_PARAM_INPUTS, SLEW_PARAM_TIME_S, STEP_PARAM_DIVISION, STEP_PARAM_GLIDE,
    STEP_PARAM_LENGTH, STEP_PARAM_TRIGGER, STEP_PARAM_VALUE_BASE,
};

/// The settings `params`' face has a knob for, in the order they sit.
pub(crate) fn face_params(params: &ModulatorParams) -> Vec<u32> {
    let pick = |sync: bool, synced: u32, free: u32| if sync { synced } else { free };
    match params {
        ModulatorParams::Lfo(lfo) => vec![
            LFO_PARAM_WAVEFORM,
            pick(lfo.tempo_sync, LFO_PARAM_RATE_DIVISION, LFO_PARAM_RATE_HZ),
            LFO_PARAM_TEMPO_SYNC,
            LFO_PARAM_DEPTH,
            LFO_PARAM_PHASE,
            LFO_PARAM_PULSE_WIDTH,
            LFO_PARAM_SMOOTHING_S,
            pick(lfo.fade_in_tempo_sync, LFO_PARAM_FADE_IN_DIVISION, LFO_PARAM_FADE_IN_S),
            LFO_PARAM_FADE_IN_SYNC,
            LFO_PARAM_RETRIGGER,
        ],
        ModulatorParams::Envelope(envelope) => vec![
            pick(envelope.attack_tempo_sync, ENV_PARAM_ATTACK_DIVISION, ENV_PARAM_ATTACK_S),
            pick(envelope.decay_tempo_sync, ENV_PARAM_DECAY_DIVISION, ENV_PARAM_DECAY_S),
            ENV_PARAM_SUSTAIN,
            pick(envelope.release_tempo_sync, ENV_PARAM_RELEASE_DIVISION, ENV_PARAM_RELEASE_S),
            ENV_PARAM_ATTACK_SYNC,
            ENV_PARAM_DECAY_SYNC,
            ENV_PARAM_RELEASE_SYNC,
            ENV_PARAM_AMOUNT,
        ],
        ModulatorParams::Step(_) => [
            STEP_PARAM_LENGTH,
            STEP_PARAM_DIVISION,
            STEP_PARAM_GLIDE,
            STEP_PARAM_TRIGGER,
        ]
        .into_iter()
        .chain((0..MOD_STEP_MAX_STEPS as u32).map(|step| STEP_PARAM_VALUE_BASE + step))
        .collect(),
        ModulatorParams::Random(random) => vec![
            pick(random.tempo_sync, RANDOM_PARAM_RATE_DIVISION, RANDOM_PARAM_RATE_HZ),
            RANDOM_PARAM_TEMPO_SYNC,
            RANDOM_PARAM_TRIGGER,
            RANDOM_PARAM_BIPOLAR,
            RANDOM_PARAM_PROBABILITY,
            RANDOM_PARAM_QUANTIZE,
            RANDOM_PARAM_DRUNK,
            RANDOM_PARAM_WALK,
        ],
        ModulatorParams::Math(math) if math.op == ModMathOp::Clamp => {
            vec![MATH_PARAM_CLAMP_LOW, MATH_PARAM_CLAMP_HIGH]
        }
        ModulatorParams::Math(_) => vec![MATH_PARAM_OPERAND],
        ModulatorParams::Counter(_) => vec![COUNTER_PARAM_STEPS],
        ModulatorParams::Select(_) => vec![SELECT_PARAM_INPUTS],
        ModulatorParams::Slew(_) => vec![SLEW_PARAM_TIME_S],
        ModulatorParams::Unknown => Vec::new(),
    }
}

/// A knob's caption: short, since a face knob has a column of its own.
pub(crate) fn label(params: &ModulatorParams, id: u32) -> String {
    let short = match (params, id) {
        (ModulatorParams::Lfo(_), LFO_PARAM_WAVEFORM) => "Shape",
        (ModulatorParams::Lfo(_), LFO_PARAM_RATE_HZ | LFO_PARAM_RATE_DIVISION) => "Rate",
        (ModulatorParams::Lfo(_), LFO_PARAM_PULSE_WIDTH) => "Width",
        (ModulatorParams::Lfo(_), LFO_PARAM_SMOOTHING_S) => "Smooth",
        (ModulatorParams::Lfo(_), LFO_PARAM_FADE_IN_S | LFO_PARAM_FADE_IN_DIVISION) => "Fade",
        (ModulatorParams::Lfo(_), LFO_PARAM_FADE_IN_SYNC) => "F.Sync",
        (ModulatorParams::Lfo(_), LFO_PARAM_RETRIGGER) => "Retrig",
        (ModulatorParams::Envelope(_), ENV_PARAM_ATTACK_S | ENV_PARAM_ATTACK_DIVISION) => "Attack",
        (ModulatorParams::Envelope(_), ENV_PARAM_DECAY_S | ENV_PARAM_DECAY_DIVISION) => "Decay",
        (ModulatorParams::Envelope(_), ENV_PARAM_RELEASE_S | ENV_PARAM_RELEASE_DIVISION) => "Release",
        (ModulatorParams::Envelope(_), ENV_PARAM_ATTACK_SYNC) => "A.Sync",
        (ModulatorParams::Envelope(_), ENV_PARAM_DECAY_SYNC) => "D.Sync",
        (ModulatorParams::Envelope(_), ENV_PARAM_RELEASE_SYNC) => "R.Sync",
        (ModulatorParams::Step(_), STEP_PARAM_TRIGGER) => "Advance",
        (ModulatorParams::Step(_), step) if step >= STEP_PARAM_VALUE_BASE => {
            return format!("{}", step - STEP_PARAM_VALUE_BASE + 1);
        }
        (ModulatorParams::Random(_), RANDOM_PARAM_RATE_HZ | RANDOM_PARAM_RATE_DIVISION) => "Rate",
        (ModulatorParams::Random(_), RANDOM_PARAM_TRIGGER) => "Draw",
        (ModulatorParams::Random(_), RANDOM_PARAM_PROBABILITY) => "Chance",
        _ => return descriptor(params, id).map_or_else(String::new, |d| d.name.to_string()),
    };
    short.to_string()
}

pub(crate) fn descriptor(params: &ModulatorParams, id: u32) -> Option<&'static ParamDescriptor> {
    params.kind().descriptor(id)
}

/// How many detents a stepped setting has, `None` for a continuous one.
pub(crate) fn steps(descriptor: &ParamDescriptor) -> Option<u16> {
    match descriptor.curve {
        ParamCurve::Stepped(steps) => Some(steps),
        _ => None,
    }
}

/// What a knob's readout says: a shape or a division by the name a box is
/// typed with, a switch as on or off, a trigger by what drives it, and a
/// number in its unit.
pub(crate) fn readout(params: &ModulatorParams, id: u32) -> String {
    let Some(value) = params.get(id) else {
        return String::new();
    };
    let on_off = |value: f32| if value >= 0.5 { "on" } else { "off" }.to_string();
    match (params, id) {
        (ModulatorParams::Lfo(lfo), LFO_PARAM_WAVEFORM) => shape_name(lfo.waveform).into(),
        (ModulatorParams::Lfo(lfo), LFO_PARAM_RATE_DIVISION) => division_name(lfo.rate_division).into(),
        (ModulatorParams::Lfo(lfo), LFO_PARAM_FADE_IN_DIVISION) => division_name(lfo.fade_in_division).into(),
        (ModulatorParams::Lfo(_), LFO_PARAM_TEMPO_SYNC | LFO_PARAM_FADE_IN_SYNC | LFO_PARAM_RETRIGGER) => {
            on_off(value)
        }
        (ModulatorParams::Envelope(envelope), ENV_PARAM_ATTACK_DIVISION) => division_name(envelope.attack_division).into(),
        (ModulatorParams::Envelope(envelope), ENV_PARAM_DECAY_DIVISION) => division_name(envelope.decay_division).into(),
        (ModulatorParams::Envelope(envelope), ENV_PARAM_RELEASE_DIVISION) => {
            division_name(envelope.release_division).into()
        }
        (ModulatorParams::Envelope(_), ENV_PARAM_ATTACK_SYNC | ENV_PARAM_DECAY_SYNC | ENV_PARAM_RELEASE_SYNC) => {
            on_off(value)
        }
        (ModulatorParams::Step(step), STEP_PARAM_DIVISION) => division_name(step.division).into(),
        (ModulatorParams::Step(_), STEP_PARAM_TRIGGER) | (ModulatorParams::Random(_), RANDOM_PARAM_TRIGGER) => {
            if value >= 0.5 { "note" } else { "clock" }.into()
        }
        (ModulatorParams::Random(random), RANDOM_PARAM_RATE_DIVISION) => division_name(random.rate_division).into(),
        (
            ModulatorParams::Random(_),
            RANDOM_PARAM_TEMPO_SYNC | RANDOM_PARAM_BIPOLAR | RANDOM_PARAM_QUANTIZE | RANDOM_PARAM_DRUNK,
        ) => on_off(value),
        _ => {
            let unit = descriptor(params, id).map_or("", |descriptor| descriptor.unit);
            let unit = match unit {
                "Hz" => "hz",
                other => other,
            };
            format!("{}{unit}", format_number(value))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{box_text::parse, ModulatorKind};

    /// Every kind's face knobs are settings it has, and say something.
    #[test]
    fn every_face_knob_is_a_described_setting_that_reads() {
        for kind in ModulatorKind::ALL {
            let params = kind.default_params();
            for id in face_params(&params) {
                assert!(descriptor(&params, id).is_some(), "{kind:?} {id}");
                assert!(!label(&params, id).is_empty(), "{kind:?} {id}");
                assert!(!readout(&params, id).is_empty(), "{kind:?} {id}");
            }
        }
    }

    /// A box shows the one rate it runs at, and its name is not a knob.
    #[test]
    fn a_face_shows_what_the_box_runs_on() {
        let free = parse("lfo 2hz").unwrap();
        let synced = parse("lfo 1/8t").unwrap();
        assert!(face_params(&free).contains(&LFO_PARAM_RATE_HZ));
        assert_eq!(readout(&free, LFO_PARAM_RATE_HZ), "2hz");
        assert!(face_params(&synced).contains(&LFO_PARAM_RATE_DIVISION));
        assert_eq!(readout(&synced, LFO_PARAM_RATE_DIVISION), "1/8t");
        assert_eq!(face_params(&parse("* 0.5").unwrap()), [MATH_PARAM_OPERAND]);
        assert_eq!(face_params(&parse("clip").unwrap()), [MATH_PARAM_CLAMP_LOW, MATH_PARAM_CLAMP_HIGH]);
        assert_eq!(face_params(&parse("counter 8").unwrap()), [COUNTER_PARAM_STEPS]);
        assert!(face_params(&ModulatorParams::Unknown).is_empty());
    }
}
