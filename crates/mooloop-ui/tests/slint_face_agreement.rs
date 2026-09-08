//! A device's descriptor table and its face must agree exactly.
//!
//! A knob and an automation lane are two views of one value. Before the table
//! existed the face was the only place a range was written down; now there are
//! two, and two ranges for one parameter is precisely how a knob comes to
//! disagree with the lane drawn against it — the failure
//! `mooloop-core`'s `ParamDescriptor` ("a range written a second time anywhere
//! else is a bug") exists to prevent.
//!
//! `mooloop-core` owns the ranges; the `.slint` faces mirror them in their
//! knob declarations, and this test fails loudly when the two diverge. It
//! parses the markup rather than evaluating it, so it does not depend on a
//! backend and runs anywhere.
//!
//! It started as the v1 drum synth's alone. The second group it covers is
//! every envelope stage in the program, which is what the pass that widened
//! it found: five faces declaring `0 .. 2` linear, or `0 .. 1` and a
//! five-second display, against one table saying 1 ms to 8 s in ratio.

use mooloop_core::{DeviceKind, ParamCurve, ParamDescriptor};

const DRUM_SLINT: &str = include_str!("../ui/drum-device.slint");
const SAMPLER_SLINT: &str = include_str!("../ui/sampler-device.slint");
const MONO_SLINT: &str = include_str!("../ui/mono-device.slint");
const POLY_SLINT: &str = include_str!("../ui/poly-device.slint");
const MLM1_SLINT: &str = include_str!("../ui/mlm1-device.slint");
const MLP8_SLINT: &str = include_str!("../ui/mlp8-device.slint");

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
    knob_in(DRUM_SLINT, "drum-device.slint", property)
}

/// The knob declaration for `property`, which is the line that both binds it
/// and states a resting value. A graphical editor binds the same property on
/// a line of its own and declares no bounds, so matching on the binding alone
/// finds the wrong line.
fn knob_in(markup: &str, file: &str, property: &str) -> FaceKnob {
    let marker = format!("root.{property};");
    let line = markup
        .lines()
        .find(|line| line.contains(&marker) && line.contains("default-value:"))
        .unwrap_or_else(|| panic!("{file} no longer declares a knob for {property}"));
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


/// Every envelope stage on every face that draws one, with the parameter it
/// addresses. Attack, decay and release only: sustain is a level and glide is
/// a linear range of its own, both of which their tables already agree with.
const ENVELOPES: [(&str, &str, &str, u32); 21] = [
    ("sampler-device.slint", "attack", "sampler", mooloop_core::SAMPLER_PARAM_ATTACK),
    ("sampler-device.slint", "decay", "sampler", mooloop_core::SAMPLER_PARAM_DECAY),
    ("sampler-device.slint", "release", "sampler", mooloop_core::SAMPLER_PARAM_RELEASE),
    ("sampler-device.slint", "filter-attack", "sampler", mooloop_core::SAMPLER_PARAM_FILTER_ATTACK),
    ("sampler-device.slint", "filter-decay", "sampler", mooloop_core::SAMPLER_PARAM_FILTER_DECAY),
    ("sampler-device.slint", "filter-release", "sampler", mooloop_core::SAMPLER_PARAM_FILTER_RELEASE),
    ("mono-device.slint", "attack", "mono", mooloop_core::SYNTH_PARAM_ATTACK),
    ("mono-device.slint", "decay", "mono", mooloop_core::SYNTH_PARAM_DECAY),
    ("mono-device.slint", "release", "mono", mooloop_core::SYNTH_PARAM_RELEASE),
    ("poly-device.slint", "attack", "poly", mooloop_core::SYNTH_PARAM_ATTACK),
    ("poly-device.slint", "decay", "poly", mooloop_core::SYNTH_PARAM_DECAY),
    ("poly-device.slint", "release", "poly", mooloop_core::SYNTH_PARAM_RELEASE),
    ("mlm1-device.slint", "attack", "mlm1", mooloop_core::SYNTH_PARAM_ATTACK),
    ("mlm1-device.slint", "decay", "mlm1", mooloop_core::SYNTH_PARAM_DECAY),
    ("mlm1-device.slint", "release", "mlm1", mooloop_core::SYNTH_PARAM_RELEASE),
    ("mlp8-device.slint", "attack", "mlp8", mooloop_core::mlp8::PARAM_ATTACK),
    ("mlp8-device.slint", "decay", "mlp8", mooloop_core::mlp8::PARAM_DECAY),
    ("mlp8-device.slint", "release", "mlp8", mooloop_core::mlp8::PARAM_RELEASE),
    ("mlp8-device.slint", "filter-attack", "mlp8", mooloop_core::mlp8::PARAM_FILTER_ATTACK),
    ("mlp8-device.slint", "filter-decay", "mlp8", mooloop_core::mlp8::PARAM_FILTER_DECAY),
    ("mlp8-device.slint", "filter-release", "mlp8", mooloop_core::mlp8::PARAM_FILTER_RELEASE),
];

fn markup(file: &str) -> &'static str {
    match file {
        "sampler-device.slint" => SAMPLER_SLINT,
        "mono-device.slint" => MONO_SLINT,
        "poly-device.slint" => POLY_SLINT,
        "mlm1-device.slint" => MLM1_SLINT,
        "mlp8-device.slint" => MLP8_SLINT,
        other => panic!("no markup registered for {other}"),
    }
}

fn envelope_descriptor(device: &str, id: u32) -> &'static ParamDescriptor {
    let kind = match device {
        "sampler" => DeviceKind::Sampler,
        "mono" => DeviceKind::MonoSynth,
        "poly" => DeviceKind::PolySynth,
        "mlm1" => DeviceKind::MlM1,
        "mlp8" => DeviceKind::MlP8,
        other => panic!("no device kind for {other}"),
    };
    kind.descriptor(id)
        .unwrap_or_else(|| panic!("{device} has no descriptor for id {id}"))
}

/// The whole point of the widening: a stage's bounds, curve and resting value
/// are one fact, and five faces used to hold five different versions of it.
#[test]
fn every_envelope_stage_agrees_with_its_table() {
    for (file, property, device, id) in ENVELOPES {
        let descriptor = envelope_descriptor(device, id);
        let face = knob_in(markup(file), file, property);
        assert!(
            (descriptor.min - face.min).abs() < 1e-6,
            "{file} {property}: face min {}, table min {}",
            face.min,
            descriptor.min
        );
        assert!(
            (descriptor.max - face.max).abs() < 1e-4,
            "{file} {property}: face max {}, table max {}",
            face.max,
            descriptor.max
        );
        assert!(
            (descriptor.default - face.default).abs() < 1e-6,
            "{file} {property}: face default {}, table default {}",
            face.default,
            descriptor.default
        );
        assert_eq!(
            face.logarithmic,
            descriptor.curve == ParamCurve::Exponential,
            "{file} {property}: face logarithmic={}, table curve={:?}",
            face.logarithmic,
            descriptor.curve
        );
    }
}

/// The shared range is 1 ms to 8 s, and both ends are public so a face can
/// state the same numbers instead of remembering them.
#[test]
fn the_shared_envelope_range_is_what_the_faces_declare() {
    let descriptor = envelope_descriptor("mono", mooloop_core::SYNTH_PARAM_ATTACK);
    assert_eq!(descriptor.min, mooloop_core::ENV_MIN_SECONDS);
    assert_eq!(descriptor.max, mooloop_core::ENV_MAX_SECONDS);
}
