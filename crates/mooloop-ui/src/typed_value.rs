//! Reading a value a user typed into a control (MOO-143).
//!
//! A knob's readout is formatted wherever that parameter's formatting lives,
//! mostly in `.slint`, so a typed value is read against two things: the
//! parameter's descriptor when the control can name one, and the readout the
//! control was showing, whose unit a bare number is taken in. Typing `250`
//! at a knob reading `1.20 s` means 250 of whatever `1.20 s` is written in
//! only if you think in seconds; typing it beside `120 ms` means 250 ms. So
//! the readout's unit decides, and a unit typed explicitly overrides it.
//!
//! One parser, here, rather than one per face: `KnobField`'s reason, that a
//! unit-aware parser written twice comes to disagree with itself.

use mooloop_core::{ParamCurve, ParamDescriptor};

/// A number and the unit written after it, lower-cased. `"4.4 kHz"` is
/// `(4.4, "khz")`; `"-inf dB"` is `(-inf, "db")`.
fn read(text: &str) -> Option<(f32, String)> {
    // A readout that pairs a note with its frequency, `A4 · 440 Hz`, is
    // read by its number.
    let text = text.rsplit('·').next().unwrap_or(text);
    let text = text.trim().replace('\u{2212}', "-");
    let lower = text.to_lowercase();
    for (word, value) in [("-inf", f32::NEG_INFINITY), ("-∞", f32::NEG_INFINITY)] {
        if let Some(rest) = lower.strip_prefix(word) {
            return Some((value, rest.trim().to_owned()));
        }
    }
    let end = lower
        .char_indices()
        .find(|&(index, c)| {
            !(c.is_ascii_digit() || c == '.' || c == ',' || ((c == '-' || c == '+') && index == 0))
        })
        .map_or(lower.len(), |(index, _)| index);
    let (number, unit) = lower.split_at(end);
    // A comma is a decimal point only when there is no point already.
    let number = if number.contains('.') {
        number.replace(',', "")
    } else {
        number.replace(',', ".")
    };
    let value: f32 = number.parse().ok()?;
    if !value.is_finite() {
        return None;
    }
    Some((value, unit.trim().to_owned()))
}

/// How many of a unit's base a unit is, for the units that come in sizes.
fn scale(unit: &str) -> Option<(&'static str, f64)> {
    Some(match unit {
        "hz" => ("hz", 1.0),
        "khz" => ("hz", 1000.0),
        "ms" => ("s", 0.001),
        "s" | "sec" => ("s", 1.0),
        _ => return None,
    })
}

/// A number typed in `typed_unit`, in `into` instead, when the two are the
/// same unit or two sizes of one.
fn convert(value: f32, typed_unit: &str, into: &str) -> Option<f32> {
    if typed_unit == into {
        return Some(value);
    }
    let (from_base, from) = scale(typed_unit)?;
    let (to_base, to) = scale(into)?;
    // In f64: `1.5 s` into ms is 1499.9999 in f32, and a typed round number
    // has to land on the round number.
    (from_base == to_base).then_some((f64::from(value) * from / to) as f32)
}

/// `"4.4k"` is 4400 of whatever unit follows or is implied.
fn expand_kilo(value: f32, unit: String) -> (f32, String) {
    match unit.strip_prefix('k') {
        Some("") => (value * 1000.0, String::new()),
        _ => (value, unit),
    }
}

/// The unit a readout is written in, or `None` for one that is not a
/// number: `Center`, `Off`, a division name.
fn shown_unit(shown: &str) -> Option<String> {
    read(shown).map(|(_, unit)| unit)
}

/// A note name, `A4` or `C#3` or `Eb2`, as a frequency. C4 is MIDI 60,
/// which is `NoteFormat`'s convention.
fn note_hz(text: &str) -> Option<f32> {
    let text = text.trim();
    let mut chars = text.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    let class = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest = chars.as_str();
    let (accidental, octave) = if let Some(rest) = rest.strip_prefix('#') {
        (1, rest)
    } else if let Some(rest) = rest.strip_prefix('b') {
        (-1, rest)
    } else {
        (0, rest)
    };
    let octave: i32 = octave.trim().parse().ok()?;
    let midi = (octave + 1) * 12 + class + accidental;
    Some(440.0 * 2f32.powf((midi as f32 - 69.0) / 12.0))
}

/// `typed` as a number in `shown`'s units, with whether those units are a
/// percentage. The fallback a control uses when it cannot name its
/// parameter: right whenever its readout shows its own value.
pub fn parse_in_shown_units(typed: &str, shown: &str) -> Option<(f32, bool)> {
    let unit = shown_unit(shown).unwrap_or_default();
    let percent = unit == "%";
    let (value, typed_unit) = read(typed)
        .map(|(value, unit)| expand_kilo(value, unit))
        .or_else(|| {
            (scale(&unit)?.0 == "hz")
                .then(|| note_hz(typed))
                .flatten()
                .map(|hz| (hz, "hz".to_owned()))
        })?;
    let value = if value == f32::NEG_INFINITY {
        -1.0e9
    } else {
        value
    };
    if typed_unit.is_empty() {
        return Some((value, percent));
    }
    Some((convert(value, &typed_unit, &unit)?, percent))
}

/// `typed` as a natural value of `descriptor`, clamped and snapped to it.
///
/// `shown` is the readout the control showed, whose unit a bare number is
/// read in. `None` when the text is not a number, names a unit that cannot
/// be turned into the descriptor's, or is read against a readout whose
/// units have no known relation to the descriptor's -- a face that draws a
/// unitless descriptor through a law of its own. The control then falls
/// back on its own readout.
pub fn parse_for(descriptor: &ParamDescriptor, typed: &str, shown: &str) -> Option<f32> {
    let natural_unit = descriptor.unit.to_lowercase();
    let shown = shown_unit(shown);
    let (value, typed_unit) = match read(typed) {
        Some((value, unit)) => expand_kilo(value, unit),
        None if natural_unit == "hz" => (note_hz(typed)?, "hz".to_owned()),
        None => return None,
    };
    // A bare number is in the readout's unit, or the descriptor's when the
    // readout is not a number at all.
    let unit = if typed_unit.is_empty() {
        shown.unwrap_or_else(|| natural_unit.clone())
    } else {
        typed_unit
    };

    // The fader's natural value is a gain, and every readout of it is in dB.
    if descriptor.curve == ParamCurve::Fader {
        if unit != "db" && !unit.is_empty() {
            return None;
        }
        let gain = if value == f32::NEG_INFINITY {
            0.0
        } else {
            10f32.powf(value / 20.0)
        };
        return Some(descriptor.clamp_natural(gain));
    }
    if value == f32::NEG_INFINITY {
        return (natural_unit == "db").then_some(descriptor.min);
    }

    let natural = if unit == natural_unit {
        value
    } else if unit == "%" && natural_unit.is_empty() {
        // A unitless 0..1 drawn as a percentage: Resonance `50%`.
        value / 100.0
    } else if unit.is_empty() {
        // Typed bare beside a readout in the descriptor's own terms.
        value
    } else {
        convert(value, &unit, &natural_unit)?
    };
    Some(descriptor.clamp_natural(natural))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(unit: &'static str, min: f32, max: f32, curve: ParamCurve) -> ParamDescriptor {
        ParamDescriptor {
            id: 0,
            name: "Test",
            unit,
            min,
            max,
            curve,
            default: min,
        }
    }

    #[test]
    fn a_frequency_takes_hertz_kilohertz_and_note_names() {
        let cutoff = descriptor("Hz", 20.0, 20_000.0, ParamCurve::Exponential);
        assert_eq!(parse_for(&cutoff, "440", "8000 Hz"), Some(440.0));
        assert_eq!(parse_for(&cutoff, "4.4k", "8000 Hz"), Some(4400.0));
        assert_eq!(parse_for(&cutoff, "4.4 kHz", "8000 Hz"), Some(4400.0));
        let a4 = parse_for(&cutoff, "A4", "8000 Hz").unwrap();
        assert!((a4 - 440.0).abs() < 0.01, "{a4}");
        let c4 = parse_for(&cutoff, "C4", "8000 Hz").unwrap();
        assert!((c4 - 261.63).abs() < 0.01, "{c4}");
        assert_eq!(
            parse_for(&cutoff, "90000", "8000 Hz"),
            Some(20_000.0),
            "clamped"
        );
    }

    #[test]
    fn a_bare_number_is_read_in_the_readouts_unit() {
        let time = descriptor("s", 0.001, 10.0, ParamCurve::Exponential);
        assert_eq!(parse_for(&time, "250", "120 ms"), Some(0.25));
        assert_eq!(parse_for(&time, "2", "1.20 s"), Some(2.0));
        assert_eq!(parse_for(&time, "250 ms", "1.20 s"), Some(0.25));
    }

    #[test]
    fn a_unitless_amount_drawn_as_a_percentage_takes_a_percentage() {
        let amount = descriptor("", 0.0, 1.0, ParamCurve::Linear);
        assert_eq!(parse_for(&amount, "50", "12%"), Some(0.5));
        assert_eq!(parse_for(&amount, "25%", "12%"), Some(0.25));
        assert_eq!(parse_for(&amount, "0.3", "0.12"), Some(0.3));
    }

    /// A face that draws a unitless descriptor through a law of its own
    /// is not guessed at: the control falls back on its readout.
    #[test]
    fn an_unrelated_unit_is_refused_rather_than_guessed() {
        let amount = descriptor("", 0.0, 1.0, ParamCurve::Linear);
        assert_eq!(parse_for(&amount, "300 ms", "120 ms"), None);
        let cutoff = descriptor("Hz", 20.0, 20_000.0, ParamCurve::Exponential);
        assert_eq!(parse_for(&cutoff, "3 s", "8000 Hz"), None);
        assert_eq!(parse_for(&cutoff, "loud", "8000 Hz"), None);
    }

    #[test]
    fn decibels_take_minus_infinity_and_the_fader_reads_them_as_gain() {
        let gain = descriptor("dB", -60.0, 12.0, ParamCurve::Linear);
        assert_eq!(parse_for(&gain, "-6", "0.0 dB"), Some(-6.0));
        assert_eq!(parse_for(&gain, "-inf", "0.0 dB"), Some(-60.0));
        assert_eq!(parse_for(&gain, "\u{2212}3 dB", "0.0 dB"), Some(-3.0));
        let fader = descriptor(
            "",
            0.0,
            mooloop_core::gain::FADER_MAX_GAIN,
            ParamCurve::Fader,
        );
        let unity = parse_for(&fader, "0", "-6.0 dB").unwrap();
        assert!((unity - 1.0).abs() < 1e-6, "{unity}");
        assert_eq!(parse_for(&fader, "-inf", "-6.0 dB"), Some(0.0));
    }

    #[test]
    fn a_stepped_parameter_snaps() {
        let steps = descriptor("st", -24.0, 24.0, ParamCurve::Stepped(49));
        assert_eq!(parse_for(&steps, "7.4", "0 st"), Some(7.0));
    }

    #[test]
    fn the_fallback_reads_in_the_readouts_units() {
        assert_eq!(parse_in_shown_units("250", "120 ms"), Some((250.0, false)));
        assert_eq!(
            parse_in_shown_units("1.5 s", "120 ms"),
            Some((1500.0, false))
        );
        assert_eq!(parse_in_shown_units("40", "12%"), Some((40.0, true)));
        assert_eq!(parse_in_shown_units("2k", "300 Hz"), Some((2000.0, false)));
        assert_eq!(parse_in_shown_units("A4", "300 Hz"), Some((440.0, false)));
        assert_eq!(parse_in_shown_units("1,5", "0.20"), Some((1.5, false)));
        assert_eq!(parse_in_shown_units("x", "0.20"), None);
        assert_eq!(parse_in_shown_units("3 s", "300 Hz"), None);
    }
}
