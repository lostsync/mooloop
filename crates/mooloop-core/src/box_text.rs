//! What a box on the patch canvas says, and what typing one makes
//! (`docs/plans/song-patch/04-typing-a-box.md`).
//!
//! A box keeps its settings as parameters, never as text: [`spell`] writes
//! the text back from them, so `*   -.5` typed reads `* -0.5` once made, and
//! a knob turned on its face changes what the box says. [`parse`] is the
//! other way, and [`VOCABULARY`] is every name it knows, in the order the
//! completion list offers them.

use crate::modulation::{
    ModCounterParams, ModLfoParams, ModLfoWaveform, ModMathOp, ModMathParams, ModSelectParams,
    ModSlewParams, ModStepParams, ModTimeDivision, ModulatorKind, ModulatorParams,
    COUNTER_MAX_STEPS, MOD_STEP_MAX_STEPS, SELECT_MAX_INPUTS,
};

/// One name a box can be typed as, and the line the completion list shows
/// beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Word {
    pub name: &'static str,
    pub says: &'static str,
}

const fn word(name: &'static str, says: &'static str) -> Word {
    Word { name, says }
}

/// The control boxes, as the completion list offers them.
pub const VOCABULARY: [Word; 14] = [
    word("lfo", "Periodic movement. lfo tri 1/4"),
    word("env", "Attack, decay, sustain, release from a gate."),
    word("step", "A row of values, one per advance. step 4"),
    word("random", "A new value on each trigger."),
    word("counter", "Counts advances and wraps at n. counter 4"),
    word("select", "Passes one of n inputs, picked by index. select 4"),
    word("slew", "Smooths jumps over time. slew 0.2"),
    word("+", "Adds. + 0.5"),
    word("-", "Subtracts. - 0.5"),
    word("*", "Multiplies. * -0.5 inverts and halves"),
    word("/", "Divides. / 2"),
    word("min", "The lower of its input and n. min 0"),
    word("max", "The higher of its input and n. max 0"),
    word("clip", "Keeps its input between two bounds. clip -0.5 0.5"),
];

/// The vocabulary's words that start with `typed`'s first word, in order.
pub fn completions(typed: &str) -> impl Iterator<Item = Word> + '_ {
    let head = typed.split_whitespace().next().unwrap_or("");
    VOCABULARY.into_iter().filter(move |word| word.name.starts_with(head))
}

/// The arithmetic boxes' names and the operator each one is.
const ARITHMETIC: [(&str, ModMathOp); 7] = [
    ("+", ModMathOp::Add),
    ("-", ModMathOp::Subtract),
    ("*", ModMathOp::Multiply),
    ("/", ModMathOp::Divide),
    ("min", ModMathOp::Min),
    ("max", ModMathOp::Max),
    ("clip", ModMathOp::Clamp),
];

const SHAPES: [(&str, ModLfoWaveform); 5] = [
    ("sin", ModLfoWaveform::Sine),
    ("tri", ModLfoWaveform::Triangle),
    ("saw", ModLfoWaveform::Saw),
    ("sqr", ModLfoWaveform::Square),
    ("rnd", ModLfoWaveform::Random),
];

/// How a tempo division is typed and spelled: a fraction of a whole note,
/// `.` for dotted, `t` for a triplet.
const DIVISIONS: [(&str, ModTimeDivision); 21] = [
    ("4/1", ModTimeDivision::FourWhole),
    ("2/1", ModTimeDivision::DoubleWhole),
    ("1/1", ModTimeDivision::Whole),
    ("1/2.", ModTimeDivision::DottedHalf),
    ("1/2", ModTimeDivision::Half),
    ("1/2t", ModTimeDivision::HalfTriplet),
    ("1/4.", ModTimeDivision::DottedQuarter),
    ("1/4", ModTimeDivision::Quarter),
    ("1/4t", ModTimeDivision::QuarterTriplet),
    ("1/8.", ModTimeDivision::DottedEighth),
    ("1/8", ModTimeDivision::Eighth),
    ("1/8t", ModTimeDivision::EighthTriplet),
    ("1/16.", ModTimeDivision::DottedSixteenth),
    ("1/16", ModTimeDivision::Sixteenth),
    ("1/16t", ModTimeDivision::SixteenthTriplet),
    ("1/32.", ModTimeDivision::DottedThirtySecond),
    ("1/32", ModTimeDivision::ThirtySecond),
    ("1/32t", ModTimeDivision::ThirtySecondTriplet),
    ("1/64.", ModTimeDivision::DottedSixtyFourth),
    ("1/64", ModTimeDivision::SixtyFourth),
    ("1/64t", ModTimeDivision::SixtyFourthTriplet),
];

/// An LFO shape as a box spells it: `sin`, `tri`, `saw`, `sqr`, `rnd`.
pub fn shape_name(shape: ModLfoWaveform) -> &'static str {
    SHAPES
        .iter()
        .find(|(_, held)| *held == shape)
        .map_or("?", |(name, _)| name)
}

/// A tempo division as a box spells it: `1/4`, `1/8t`, `1/16.`.
pub fn division_name(division: ModTimeDivision) -> &'static str {
    DIVISIONS
        .iter()
        .find(|(_, held)| *held == division)
        .map_or("?", |(name, _)| name)
}

/// A number as a box spells it: at most two decimals, no trailing zeros.
pub fn format_number(value: f32) -> String {
    let text = format!("{value:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

/// A rate in hertz as an LFO box spells it.
fn format_hz(hz: f32) -> String {
    format!("{}hz", format_number(hz))
}

/// What a box says: its name, then whatever of its settings its name
/// carries. An unknown box says what was typed into it.
pub fn spell(params: &ModulatorParams, text: &str) -> String {
    match params {
        ModulatorParams::Lfo(lfo) => {
            let mut spelled = String::from("lfo");
            let defaults = ModLfoParams::default();
            if lfo.waveform != defaults.waveform {
                if let Some((name, _)) = SHAPES.iter().find(|(_, shape)| *shape == lfo.waveform) {
                    spelled.push(' ');
                    spelled.push_str(name);
                }
            }
            if lfo.tempo_sync {
                if let Some((name, _)) = DIVISIONS.iter().find(|(_, d)| *d == lfo.rate_division) {
                    spelled.push(' ');
                    spelled.push_str(name);
                }
            } else if lfo.rate_hz != defaults.rate_hz {
                spelled.push(' ');
                spelled.push_str(&format_hz(lfo.rate_hz));
            }
            spelled
        }
        ModulatorParams::Envelope(_) => "env".into(),
        ModulatorParams::Step(step) if step.length != ModStepParams::default().length => {
            format!("step {}", step.length)
        }
        ModulatorParams::Step(_) => "step".into(),
        ModulatorParams::Random(_) => "random".into(),
        ModulatorParams::Math(math) => {
            let name = ARITHMETIC
                .iter()
                .find(|(_, op)| *op == math.op)
                .map_or("*", |(name, _)| name);
            match math.op {
                ModMathOp::Clamp => format!(
                    "clip {} {}",
                    format_number(math.clamp_low),
                    format_number(math.clamp_high)
                ),
                _ => format!("{name} {}", format_number(math.operand)),
            }
        }
        ModulatorParams::Counter(counter) => format!("counter {}", counter.steps),
        ModulatorParams::Select(select) => format!("select {}", select.inputs),
        ModulatorParams::Slew(slew) => format!("slew {}", format_number(slew.time_seconds)),
        ModulatorParams::Unknown => text.trim().to_string(),
    }
}

/// A number as typed: `-.5`, `0.25`, `2`.
fn number(text: &str) -> Option<f32> {
    text.parse::<f32>().ok().filter(|value| value.is_finite())
}

/// A count as typed, held to `low..=high`.
fn count(text: Option<&str>, default: u8, low: u8, high: u8) -> Option<u8> {
    match text {
        None => Some(default),
        Some(text) => {
            let value = number(text)?.round();
            Some(value.clamp(f32::from(low), f32::from(high)) as u8)
        }
    }
}

/// A duration as typed: seconds, or with `ms` or `s` after it.
fn seconds(text: &str) -> Option<f32> {
    if let Some(ms) = text.strip_suffix("ms") {
        return number(ms).map(|ms| ms / 1_000.0);
    }
    number(text.strip_suffix('s').unwrap_or(text))
}

/// What typing `text` makes: the box its first word names, with what
/// follows as its settings. `None` for a name not in [`VOCABULARY`] or
/// settings it cannot read, which the canvas makes as an unknown box that
/// keeps the text. An empty `text` makes nothing at all, which is the
/// caller's to see.
pub fn parse(text: &str) -> Option<ModulatorParams> {
    let mut words = text.split_whitespace();
    let name = words.next()?;
    let args: Vec<&str> = words.collect();
    let at_most = |n: usize| (args.len() <= n).then_some(());
    match name {
        "lfo" => {
            let mut lfo = ModLfoParams::default();
            for arg in &args {
                let arg = arg.to_ascii_lowercase();
                if let Some((_, shape)) = SHAPES.iter().find(|(name, _)| *name == arg) {
                    lfo.waveform = *shape;
                } else if let Some((_, division)) = DIVISIONS.iter().find(|(name, _)| *name == arg)
                {
                    lfo.tempo_sync = true;
                    lfo.rate_division = *division;
                } else {
                    let hz = number(arg.strip_suffix("hz").unwrap_or(&arg))?;
                    lfo.tempo_sync = false;
                    lfo.rate_hz = clamp_by(ModulatorKind::Lfo, crate::LFO_PARAM_RATE_HZ, hz);
                }
            }
            Some(ModulatorParams::Lfo(lfo))
        }
        "env" => at_most(0).map(|()| ModulatorKind::Envelope.default_params()),
        "random" => at_most(0).map(|()| ModulatorKind::Random.default_params()),
        "step" => {
            at_most(1)?;
            let mut step = ModStepParams::default();
            step.length = count(args.first().copied(), step.length, 1, MOD_STEP_MAX_STEPS as u8)?;
            Some(ModulatorParams::Step(step))
        }
        "counter" => {
            at_most(1)?;
            let steps = count(args.first().copied(), 4, 2, COUNTER_MAX_STEPS)?;
            Some(ModulatorParams::Counter(ModCounterParams { steps }))
        }
        "select" => {
            at_most(1)?;
            let inputs = count(args.first().copied(), 4, 2, SELECT_MAX_INPUTS)?;
            Some(ModulatorParams::Select(ModSelectParams { inputs }))
        }
        "slew" => {
            at_most(1)?;
            let time = match args.first() {
                Some(arg) => seconds(arg)?,
                None => ModSlewParams::default().time_seconds,
            };
            Some(ModulatorParams::Slew(ModSlewParams {
                time_seconds: clamp_by(ModulatorKind::Slew, crate::SLEW_PARAM_TIME_S, time),
            }))
        }
        "clip" => {
            at_most(2)?;
            let low = args.first().map_or(Some(-1.0), |arg| number(arg))?;
            let high = args.get(1).map_or(Some(1.0), |arg| number(arg))?;
            Some(ModulatorParams::Math(ModMathParams {
                op: ModMathOp::Clamp,
                clamp_low: low.clamp(-1.0, 1.0),
                clamp_high: high.clamp(-1.0, 1.0),
                ..ModMathParams::default()
            }))
        }
        _ => {
            let (_, op) = ARITHMETIC.iter().find(|(word, _)| *word == name)?;
            at_most(1)?;
            let identity = match op {
                ModMathOp::Multiply | ModMathOp::Divide => 1.0,
                _ => 0.0,
            };
            let operand = args.first().map_or(Some(identity), |arg| number(arg))?;
            Some(ModulatorParams::Math(ModMathParams {
                op: *op,
                operand: clamp_by(ModulatorKind::Math, crate::MATH_PARAM_OPERAND, operand),
                ..ModMathParams::default()
            }))
        }
    }
}

/// `value` held inside the range `kind`'s descriptor `id` gives it.
fn clamp_by(kind: ModulatorKind, id: u32, value: f32) -> f32 {
    kind.descriptor(id)
        .map_or(value, |descriptor| descriptor.clamp_natural(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(typed: &str) -> String {
        spell(&parse(typed).expect(typed), "")
    }

    #[test]
    fn a_box_is_spelled_from_its_settings_not_its_typing() {
        assert_eq!(round_trip("*   -.5"), "* -0.5");
        assert_eq!(round_trip("+"), "+ 0");
        assert_eq!(round_trip("/"), "/ 1");
        assert_eq!(round_trip("clip -.5 .5"), "clip -0.5 0.5");
        assert_eq!(round_trip("min 0.25"), "min 0.25");
        assert_eq!(round_trip("counter"), "counter 4");
        assert_eq!(round_trip("counter 100"), "counter 64", "held to its range");
        assert_eq!(round_trip("select 6"), "select 6");
        assert_eq!(round_trip("select 12"), "select 8");
        assert_eq!(round_trip("slew 200ms"), "slew 0.2");
        assert_eq!(round_trip("step"), "step");
        assert_eq!(round_trip("step 4"), "step 4");
        assert_eq!(round_trip("lfo"), "lfo");
        assert_eq!(round_trip("lfo tri 1/4"), "lfo tri 1/4");
        assert_eq!(round_trip("lfo 1/8t sqr"), "lfo sqr 1/8t");
        assert_eq!(round_trip("lfo 2hz"), "lfo 2hz");
        assert_eq!(round_trip("env"), "env");
        assert_eq!(round_trip("random"), "random");
    }

    #[test]
    fn what_parse_cannot_read_is_left_to_an_unknown_box() {
        for typed in ["", "chord min7", "lfo wobbly", "* two", "counter 4 4", "env 3"] {
            assert_eq!(parse(typed), None, "{typed:?}");
        }
        assert_eq!(spell(&ModulatorParams::Unknown, " chord min7 "), "chord min7");
    }

    #[test]
    fn every_word_in_the_vocabulary_parses_and_completes() {
        for word in VOCABULARY {
            assert!(parse(word.name).is_some(), "{}", word.name);
        }
        let offered: Vec<_> = completions("s").map(|word| word.name).collect();
        assert_eq!(offered, ["step", "select", "slew"]);
        let offered: Vec<_> = completions("m").map(|word| word.name).collect();
        assert_eq!(offered, ["min", "max"]);
        assert_eq!(completions("").count(), VOCABULARY.len());
    }

    #[test]
    fn a_tempo_division_spells_as_it_is_typed() {
        for (name, division) in DIVISIONS {
            let Some(ModulatorParams::Lfo(lfo)) = parse(&format!("lfo {name}")) else {
                panic!("{name}");
            };
            assert!(lfo.tempo_sync);
            assert_eq!(lfo.rate_division, division);
        }
        assert_eq!(DIVISIONS.len(), ModTimeDivision::ALL.len());
    }
}
