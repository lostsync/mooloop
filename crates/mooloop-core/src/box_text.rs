//! What a box on the patch canvas says, and what typing one makes
//! (`docs/plans/archive/song-patch/04-typing-a-box.md`).
//!
//! A box keeps its settings as parameters, never as text: [`spell`] writes
//! the text back from them, so `*   -.5` typed reads `* -0.5` once made, and
//! a knob turned on its face changes what the box says. [`parse`] is the
//! other way, and [`VOCABULARY`] is every name it knows, in the order the
//! completion list offers them.
//!
//! The note boxes (`docs/plans/archive/song-patch/08-note-boxes.md`) read and spell
//! their chord types, roots and modes from [`crate::harmony`], the one place
//! those names live.

use crate::harmony::{ChordQuality, Mode, MAX_CHORD_NOTES, PITCH_NAMES};
use crate::modulation::{
    ModChanceParams, ModChordParams, ModModalParams, ModScaleParams, ModTransposeParams,
    TRANSPOSE_MAX_SEMITONES, ModCounterParams, ModLfoParams, ModLfoWaveform, ModMathOp, ModMathParams, ModSelectParams, ModSwitchParams,
    ModNumParams, ModSlewParams, ModStepParams, ModTimeDivision, ModulatorKind, ModulatorParams,
    ModTableParams, NumFormat, COUNTER_MAX_STEPS, TABLE_MAX_VALUES, MOD_STEP_MAX_STEPS, NUM_LIMIT, SELECT_MAX_INPUTS, SWITCH_MAX_OUTPUTS,
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

/// The boxes, the control boxes then the note boxes, as the completion
/// list offers them.
pub const VOCABULARY: [Word; 25] = [
    word("lfo", "Periodic movement. lfo tri 1/4"),
    word("env", "Attack, decay, sustain, release from a gate."),
    word("step", "A row of values, one per advance. step 8 uni"),
    word("random", "A new value on each trigger."),
    word("counter", "Counts advances and wraps at n. counter 4"),
    word("select", "Passes one of n inputs, picked by index. select 4"),
    word("switch", "Sends its input to one of n outlets, picked by which. switch 3"),
    word("gate", "Passes its input while open is high."),
    word("slew", "Smooths jumps over time. slew 0.2"),
    word("num", "Shows a value, or sends one. num int 0 16"),
    word("hold", "Keeps its input from the last trigger."),
    word("table", "Stores n values, read and written by index. table 8"),
    word("+", "Adds. + 0.5"),
    word("-", "Subtracts. - 0.5"),
    word("*", "Multiplies. * -0.5 inverts and halves"),
    word("/", "Divides. / 2"),
    word("min", "The lower of its input and n. min 0"),
    word("max", "The higher of its input and n. max 0"),
    word("clip", "Keeps its input between two bounds. clip -0.5 0.5"),
    word("chord", "Builds a chord on each note. chord min7 /1st"),
    word("modal", "Each note to its mode's chord. modal d dorian 7"),
    word("scale", "Snaps each note into a mode. scale c minor"),
    word("transpose", "Moves notes by semitones. transpose -12"),
    word("chance", "Lets each note through with a probability. chance 0.7"),
    word("held", "Notes to a gate, pitch and velocity."),
];

/// How an inversion is spelled after a chord's type: none for root
/// position, then `/1st` to `/3rd`.
const INVERSIONS: [&str; MAX_CHORD_NOTES] = ["", "/1st", "/2nd", "/3rd"];

/// An inversion as typed: `/1st` or `/1`, to `/3rd` or `/3`.
fn inversion(text: &str) -> Option<u8> {
    let number = text.strip_prefix('/')?;
    let number = ["st", "nd", "rd", "th"]
        .iter()
        .find_map(|suffix| number.strip_suffix(suffix))
        .unwrap_or(number);
    number.parse::<u8>().ok().filter(|&at| usize::from(at) < MAX_CHORD_NOTES)
}

/// A modal or scale box's root and mode, and a modal box's size, as typed
/// in any order: `d`, `dorian`, `7` (or `3` for a triad).
fn root_and_mode(args: &[&str], sized: bool) -> Option<(u8, Mode, bool)> {
    let (mut root, mut mode, mut seventh) = (0, Mode::default(), false);
    for arg in args {
        let arg = arg.to_ascii_lowercase();
        if let Some(class) = crate::harmony::pitch_class(&arg) {
            root = class;
        } else if let Some(named) = Mode::from_name(&arg) {
            mode = named;
        } else if sized && (arg == "7" || arg == "3") {
            seventh = arg == "7";
        } else {
            return None;
        }
    }
    Some((root, mode, seventh))
}

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
        ModulatorParams::Step(step) => {
            let mut spelled = String::from("step");
            if step.length != ModStepParams::default().length {
                spelled.push_str(&format!(" {}", step.length));
            }
            if step.unipolar {
                spelled.push_str(" uni");
            }
            spelled
        }
        ModulatorParams::Table(table) => format!("table {}", table.length()),
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
        ModulatorParams::Chord(chord) => {
            let inversion = INVERSIONS[usize::from(chord.inversion) % MAX_CHORD_NOTES];
            let mut spelled = format!("chord {}", chord.quality.name());
            if !inversion.is_empty() {
                spelled.push(' ');
                spelled.push_str(inversion);
            }
            spelled
        }
        ModulatorParams::Modal(modal) => format!(
            "modal {} {}{}",
            PITCH_NAMES[usize::from(modal.root % 12)],
            modal.mode.name(),
            if modal.seventh { " 7" } else { "" }
        ),
        ModulatorParams::Scale(scale) => format!(
            "scale {} {}",
            PITCH_NAMES[usize::from(scale.root % 12)],
            scale.mode.name()
        ),
        ModulatorParams::Transpose(transpose) => format!("transpose {}", transpose.semitones),
        ModulatorParams::Chance(chance) => format!("chance {}", format_number(chance.probability)),
        ModulatorParams::NoteGate => "held".into(),
        ModulatorParams::Switch(switch) => format!("switch {}", switch.outputs),
        ModulatorParams::Gate => "gate".into(),
        ModulatorParams::Num(num) => {
            let mut spelled = String::from("num");
            if let Some(format) = NUM_FORMATS.iter().find(|(_, held)| *held == num.format) {
                if num.format != NumFormat::Float {
                    spelled.push(' ');
                    spelled.push_str(format.0);
                }
            }
            if (num.low, num.high) != num_range(num.format) {
                spelled.push_str(&format!(" {} {}", format_number(num.low), format_number(num.high)));
            }
            spelled
        }
        ModulatorParams::Hold => "hold".into(),
        ModulatorParams::Unknown => text.trim().to_string(),
    }
}

/// How a num box's format is typed and spelled.
const NUM_FORMATS: [(&str, NumFormat); 3] = [
    ("float", NumFormat::Float),
    ("int", NumFormat::Int),
    ("note", NumFormat::Note),
];

/// The range a num box of `format` has when none is typed: a pitch's
/// `0..1`, otherwise the patch's `-1..1`.
fn num_range(format: NumFormat) -> (f32, f32) {
    match format {
        NumFormat::Note => (0.0, 1.0),
        NumFormat::Float | NumFormat::Int => (-1.0, 1.0),
    }
}

/// A num box as typed: a format word anywhere, then one number for its
/// value, two for its range, or three for its range and value.
fn num(args: &[&str]) -> Option<ModulatorParams> {
    let mut format = NumFormat::Float;
    let mut numbers = Vec::new();
    for arg in args {
        let arg = arg.to_ascii_lowercase();
        match NUM_FORMATS.iter().find(|(name, _)| *name == arg) {
            Some((_, named)) => format = *named,
            None => numbers.push(number(&arg)?.clamp(-NUM_LIMIT, NUM_LIMIT)),
        }
    }
    let (low, high) = match numbers.len() {
        0 | 1 => num_range(format),
        2 | 3 => (numbers[0], numbers[1]),
        _ => return None,
    };
    let mut params = ModNumParams {
        low,
        high,
        format,
        ..ModNumParams::default()
    };
    params.position = match numbers.len() {
        1 => params.position_of(numbers[0]),
        3 => params.position_of(numbers[2]),
        _ => params.position_of(0.0_f32.clamp(low.min(high), low.max(high))),
    };
    Some(ModulatorParams::Num(params))
}

/// What a num box shows for `value`: two decimals, a whole number, or a
/// pitch (`0..1` across notes 0 to 127, middle C being `C4`).
pub fn num_readout(params: &ModNumParams, value: f32) -> String {
    match params.format {
        NumFormat::Float => format_number(value),
        NumFormat::Int => format!("{}", value.round() as i32),
        NumFormat::Note => {
            let note = (value.clamp(0.0, 1.0) * 127.0).round() as i32;
            let name = crate::harmony::PITCH_NAMES[(note % 12) as usize].to_ascii_uppercase();
            format!("{name}{}", note / 12 - 1)
        }
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
            at_most(2)?;
            let mut step = ModStepParams::default();
            let mut length = None;
            for arg in &args {
                match arg.to_ascii_lowercase().as_str() {
                    "uni" => step.unipolar = true,
                    "bi" => step.unipolar = false,
                    _ if length.is_none() => length = Some(*arg),
                    _ => return None,
                }
            }
            step.length = count(length, step.length, 1, MOD_STEP_MAX_STEPS as u8)?;
            Some(ModulatorParams::Step(step))
        }
        "table" => {
            at_most(1)?;
            let length = count(args.first().copied(), 8, 2, TABLE_MAX_VALUES as u8)?;
            Some(ModulatorParams::Table(ModTableParams {
                length,
                ..ModTableParams::default()
            }))
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
        "chord" => {
            at_most(2)?;
            let mut chord = ModChordParams::default();
            for arg in &args {
                let arg = arg.to_ascii_lowercase();
                match ChordQuality::from_name(&arg) {
                    Some(quality) => chord.quality = quality,
                    None => chord.inversion = inversion(&arg)?,
                }
            }
            Some(ModulatorParams::Chord(chord))
        }
        "modal" => {
            at_most(3)?;
            let (root, mode, seventh) = root_and_mode(&args, true)?;
            Some(ModulatorParams::Modal(ModModalParams { root, mode, seventh }))
        }
        "scale" => {
            at_most(2)?;
            let (root, mode, _) = root_and_mode(&args, false)?;
            Some(ModulatorParams::Scale(ModScaleParams { root, mode }))
        }
        "transpose" => {
            at_most(1)?;
            let most = f32::from(TRANSPOSE_MAX_SEMITONES);
            let semitones = args.first().map_or(Some(0.0), |arg| number(arg))?;
            Some(ModulatorParams::Transpose(ModTransposeParams {
                semitones: semitones.round().clamp(-most, most) as i8,
            }))
        }
        "chance" => {
            at_most(1)?;
            let probability = args
                .first()
                .map_or(Some(ModChanceParams::default().probability), |arg| number(arg))?;
            Some(ModulatorParams::Chance(ModChanceParams {
                probability: clamp_by(ModulatorKind::Chance, crate::CHANCE_PARAM_PROBABILITY, probability),
            }))
        }
        "held" => at_most(0).map(|()| ModulatorParams::NoteGate),
        "switch" => {
            at_most(1)?;
            let outputs = count(args.first().copied(), 2, 2, SWITCH_MAX_OUTPUTS)?;
            Some(ModulatorParams::Switch(ModSwitchParams { outputs }))
        }
        "gate" => at_most(0).map(|()| ModulatorParams::Gate),
        "num" => num(&args),
        "hold" => at_most(0).map(|()| ModulatorParams::Hold),
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
        assert_eq!(round_trip("hold"), "hold");
        assert_eq!(round_trip("step 8 uni"), "step uni");
        assert_eq!(round_trip("step bi 4"), "step 4");
        assert_eq!(round_trip("step uni 12"), "step 12 uni");
        assert_eq!(round_trip("table"), "table 8");
        assert_eq!(round_trip("table 40"), "table 16", "held to its range");
        for typed in ["step 4 4", "step uni uni uni", "table a"] {
            assert_eq!(parse(typed), None, "{typed:?}");
        }
    }

    #[test]
    fn a_num_box_spells_its_format_and_range_and_keeps_its_value() {
        assert_eq!(round_trip("num"), "num");
        assert_eq!(round_trip("num 0.25"), "num", "a value is set, not spelled");
        assert_eq!(round_trip("num int 0 16"), "num int 0 16");
        assert_eq!(round_trip("num 0 1 float"), "num 0 1");
        assert_eq!(round_trip("num note"), "num note");
        assert_eq!(round_trip("num note -1 1"), "num note -1 1");
        let value = |typed: &str| match parse(typed) {
            Some(ModulatorParams::Num(num)) => num.value(),
            other => panic!("{typed}: {other:?}"),
        };
        assert_eq!(value("num"), 0.0);
        assert_eq!(value("num .25"), 0.25);
        assert_eq!(value("num 2"), 1.0, "held to its range");
        assert_eq!(value("num int 0 16 5"), 5.0);
        assert_eq!(value("num 2 4"), 2.0, "zero is outside, so the nearest end");
        for typed in ["num a", "num 1 2 3 4", "hold 1"] {
            assert_eq!(parse(typed), None, "{typed:?}");
        }
    }

    #[test]
    fn a_num_box_reads_as_its_format() {
        let Some(ModulatorParams::Num(note)) = parse("num note") else { panic!() };
        assert_eq!(num_readout(&note, 60.0 / 127.0), "C4");
        assert_eq!(num_readout(&note, 0.0), "C-1");
        let Some(ModulatorParams::Num(int)) = parse("num int 0 16") else { panic!() };
        assert_eq!(num_readout(&int, 3.4), "3");
        let Some(ModulatorParams::Num(float)) = parse("num") else { panic!() };
        assert_eq!(num_readout(&float, -0.126), "-0.13");
    }

    #[test]
    fn note_boxes_spell_their_chords_roots_and_modes() {
        assert_eq!(round_trip("chord"), "chord maj");
        assert_eq!(round_trip("chord min7 /1st"), "chord min7 /1st");
        assert_eq!(round_trip("chord /2 dim"), "chord dim /2nd");
        assert_eq!(round_trip("chord 7 /0"), "chord 7");
        assert_eq!(round_trip("modal d dorian 7"), "modal d dorian 7");
        assert_eq!(round_trip("modal Bb aeolian"), "modal a# minor");
        assert_eq!(round_trip("modal"), "modal c major");
        assert_eq!(round_trip("scale c minor"), "scale c minor");
        assert_eq!(round_trip("scale harmonic e"), "scale e harmonic");
        assert_eq!(round_trip("transpose -12"), "transpose -12");
        assert_eq!(round_trip("transpose 100"), "transpose 48", "held to four octaves");
        assert_eq!(round_trip("chance .7"), "chance 0.7");
        assert_eq!(round_trip("chance 2"), "chance 1");
        assert_eq!(round_trip("held"), "held");
        for typed in ["chord min9", "chord /4th", "modal h", "scale c minor 7", "held 1"] {
            assert_eq!(parse(typed), None, "{typed:?}");
        }
    }

    #[test]
    fn switch_and_gate_spell_as_typed() {
        assert_eq!(round_trip("switch"), "switch 2");
        assert_eq!(round_trip("switch 3"), "switch 3");
        assert_eq!(round_trip("switch 20"), "switch 8");
        assert_eq!(round_trip("gate"), "gate");
        assert_eq!(parse("gate"), Some(ModulatorParams::Gate), "gate is the control gate now");
        for typed in ["gate 1", "switch 3 3"] {
            assert_eq!(parse(typed), None, "{typed:?}");
        }
    }

    #[test]
    fn what_parse_cannot_read_is_left_to_an_unknown_box() {
        for typed in ["", "arp up", "lfo wobbly", "* two", "counter 4 4", "env 3"] {
            assert_eq!(parse(typed), None, "{typed:?}");
        }
        assert_eq!(spell(&ModulatorParams::Unknown, " arp up "), "arp up");
    }

    #[test]
    fn every_word_in_the_vocabulary_parses_and_completes() {
        for word in VOCABULARY {
            assert!(parse(word.name).is_some(), "{}", word.name);
        }
        let offered: Vec<_> = completions("s").map(|word| word.name).collect();
        assert_eq!(offered, ["step", "select", "switch", "slew", "scale"]);
        let offered: Vec<_> = completions("m").map(|word| word.name).collect();
        assert_eq!(offered, ["min", "max", "modal"]);
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
