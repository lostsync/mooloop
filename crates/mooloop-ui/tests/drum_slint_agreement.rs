//! The v1 drum synth's descriptor table and its face must agree exactly.
//!
//! A knob and an automation lane are two views of one value. Before the table
//! existed the face was the only place a range was written down; now there are
//! two, and two ranges for one parameter is precisely how a knob comes to
//! disagree with the lane drawn against it — the failure
//! `mooloop-core`'s `ParamDescriptor` ("a range written a second time anywhere
//! else is a bug") exists to prevent.
//!
//! `mooloop-core/src/generator.rs` owns the ranges; `ui/drum-device.slint`
//! mirrors them in its knob declarations, and this test fails loudly when the
//! two diverge. It parses the markup rather than evaluating it, so it does not
//! depend on a backend and runs anywhere.

use mooloop_core::{DeviceKind, ParamCurve};

const DRUM_SLINT: &str = include_str!("../ui/drum-device.slint");

/// The face's declaration for one knob: its bounds, its resting value, and
/// whether it is drawn in ratio.
///
/// A knob declaration puts the binding and everything about it on one line,
/// which is what makes this a line scan rather than a parser. Bounds are
/// optional in the markup -- a unit knob leaves them to the widget's own
/// `0..1` -- so an absent bound is that default rather than a parse failure.
struct FaceKnob {
    min: f32,
    max: f32,
    default: f32,
    logarithmic: bool,
}

fn face_knob(property: &str) -> FaceKnob {
    let marker = format!("root.{property};");
    let line = DRUM_SLINT
        .lines()
        .find(|line| line.contains(&marker))
        .unwrap_or_else(|| panic!("drum-device.slint no longer binds {property}"));
    FaceKnob {
        min: number(line, "minimum:").unwrap_or(0.0),
        max: number(line, "maximum:").unwrap_or(1.0),
        default: number(line, "default-value:")
            .unwrap_or_else(|| panic!("{property} has no default-value")),
        logarithmic: line.contains("ValueScale.logarithmic"),
    }
}

fn number(line: &str, key: &str) -> Option<f32> {
    let at = line.find(key)? + key.len();
    let rest = line[at..].trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .unwrap_or(rest.len());
    Some(
        rest[..end]
            .parse()
            .unwrap_or_else(|error| panic!("{key} in {line:?} is not a number: {error}")),
    )
}

/// Every continuous control the face draws, paired with the parameter id that
/// now addresses it. Spelled out rather than derived, because the point is to
/// catch a table and a face that stopped matching — a derivation from either
/// side would move with the side it was derived from.
const KNOBS: [(&str, u32); 16] = [
    ("decay", mooloop_core::DRUM_PARAM_DECAY),
    ("tune-semitones", mooloop_core::DRUM_PARAM_TUNE_SEMITONES),
    ("drive", mooloop_core::DRUM_PARAM_DRIVE),
    ("punch", mooloop_core::DRUM_PARAM_PUNCH),
    ("kick-start-hz", mooloop_core::DRUM_PARAM_KICK_START_HZ),
    ("kick-end-hz", mooloop_core::DRUM_PARAM_KICK_END_HZ),
    ("kick-sweep", mooloop_core::DRUM_PARAM_KICK_SWEEP),
    ("kick-click", mooloop_core::DRUM_PARAM_KICK_CLICK),
    ("snare-tone-hz", mooloop_core::DRUM_PARAM_SNARE_TONE_HZ),
    ("snare-tone2-hz", mooloop_core::DRUM_PARAM_SNARE_TONE2_HZ),
    ("snare-tone2-mix", mooloop_core::DRUM_PARAM_SNARE_TONE2_MIX),
    ("snare-noise-mix", mooloop_core::DRUM_PARAM_SNARE_NOISE_MIX),
    (
        "snare-noise-decay",
        mooloop_core::DRUM_PARAM_SNARE_NOISE_DECAY,
    ),
    (
        "snare-noise-color",
        mooloop_core::DRUM_PARAM_SNARE_NOISE_COLOR,
    ),
    ("hat-hp-hz", mooloop_core::DRUM_PARAM_HAT_HP_HZ),
    ("hat-metallic", mooloop_core::DRUM_PARAM_HAT_METALLIC),
];

#[test]
fn the_drum_table_agrees_with_its_face() {
    for (property, id) in KNOBS {
        let descriptor = DeviceKind::DrumSynth
            .descriptor(id)
            .unwrap_or_else(|| panic!("no descriptor for {property} (id {id})"));
        let face = face_knob(property);
        assert!(
            (descriptor.min - face.min).abs() < 1e-4,
            "{property}: face min {}, table min {}",
            face.min,
            descriptor.min
        );
        assert!(
            (descriptor.max - face.max).abs() < 1e-4,
            "{property}: face max {}, table max {}",
            face.max,
            descriptor.max
        );
        // The resting value too: an untouched knob and the base an automation
        // lane resolves from have to be the same number.
        assert!(
            (descriptor.default - face.default).abs() < 1e-4,
            "{property}: face default {}, table default {}",
            face.default,
            descriptor.default
        );
        // A knob the face draws in ratio and a lane that resolved it linearly
        // would agree at the ends and nowhere in between, which is the harder
        // half of this bug to see.
        assert_eq!(
            face.logarithmic,
            descriptor.curve == ParamCurve::Exponential,
            "{property}: face logarithmic={}, table curve={:?}",
            face.logarithmic,
            descriptor.curve
        );
    }
}
