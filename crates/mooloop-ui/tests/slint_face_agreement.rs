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
//!
//! The third is every oscillator knob, added 2026-09-18. That group is here
//! because of *how* it was missing rather than what it said: the shared
//! oscillator face reads its defaults from the table at run time and spells
//! only its bounds, so a check looking for a copied `default-value:` saw
//! nothing to check and a face holding four literal numbers stayed outside
//! the list for as long as the list has existed.

use mooloop_core::{DeviceKind, EffectKind, EqFaceControl, EqParams, ParamCurve, ParamDescriptor};
use mooloop_core::{
    BITCRUSH_PARAM_DOWNSAMPLE, COMP_PARAM_ATTACK_MS, COMP_PARAM_RATIO, COMP_PARAM_RELEASE_MS,
    DELAY_PARAM_TIME_MS, DRIVE_PARAM_DRIVE, GATE_PARAM_ATTACK_MS, GATE_PARAM_RELEASE_MS,
    LIMITER_PARAM_RELEASE_MS, PLATE_PARAM_DECAY_S, PREAMP_PARAM_DRIVE_DB, PREAMP_PARAM_OUTPUT_DB,
    REVERB_PARAM_PREDELAY_MS,
};

const DRUM_SLINT: &str = include_str!("../ui/drum-device.slint");
const SAMPLER_SLINT: &str = include_str!("../ui/sampler-device.slint");
const MONO_SLINT: &str = include_str!("../ui/mono-device.slint");
const POLY_SLINT: &str = include_str!("../ui/poly-device.slint");
const MLM1_SLINT: &str = include_str!("../ui/mlm1-device.slint");
const MLP8_SLINT: &str = include_str!("../ui/mlp8-device.slint");
const OSCILLATOR_SLINT: &str = include_str!("../ui/device-oscillator.slint");

const BITCRUSH_SLINT: &str = include_str!("../ui/bitcrush-device.slint");
const PREAMP_SLINT: &str = include_str!("../ui/preamp-device.slint");
const DRIVE_SLINT: &str = include_str!("../ui/drive-device.slint");
const COMPRESSOR_SLINT: &str = include_str!("../ui/compressor-device.slint");
const DELAY_SLINT: &str = include_str!("../ui/delay-device.slint");
const GATE_SLINT: &str = include_str!("../ui/gate-device.slint");
const LIMITER_SLINT: &str = include_str!("../ui/limiter-device.slint");
const PLATE_SLINT: &str = include_str!("../ui/plate-device.slint");
const REVERB_SLINT: &str = include_str!("../ui/reverb-device.slint");
const EQ_SLINT: &str = include_str!("../ui/eq-device.slint");
const FILTER_SLINT: &str = include_str!("../ui/filter-device.slint");
const MODULATION_SLINT: &str = include_str!("../ui/modulation-device.slint");
const BUFFER_SLINT: &str = include_str!("../ui/buffer-device.slint");
const MAIN_SLINT: &str = include_str!("../ui/main.slint");
const CONTAINER_SLINT: &str = include_str!("../ui/container-device.slint");
const AUX_IN_SLINT: &str = include_str!("../ui/aux-in-device.slint");

/// A Buffer edit crosses two address spaces in `main.slint`: row fields and
/// modulation overlays use stable descriptor ids, while
/// `Session::set_effect_param` accepts a descriptor's position in the kind's
/// table. Most effect tables happen to make those numbers equal. Buffer's
/// retired ids make them differ, which is why passing ids here made JUMP
/// operate Reverse, REV and STUT do nothing, and QUANT operate Stutter.
///
/// Read the actual wiring rather than restating its numbers in a session
/// test: the DSP and session tests were all green while the face was broken.
#[test]
fn the_buffer_face_sends_descriptor_positions_for_edits() {
    let writes = [
        ("crossfade-changed(v)", mooloop_core::BUFFER_PARAM_CROSSFADE_MS),
        ("position-changed(v)", mooloop_core::BUFFER_PARAM_POSITION),
        ("quant-start-changed(v)", mooloop_core::BUFFER_PARAM_QUANT_START),
        ("jump-back-changed(v)", mooloop_core::BUFFER_PARAM_JUMP_BACK),
        (
            "stutter-length-changed(v)",
            mooloop_core::BUFFER_PARAM_STUTTER_LENGTH,
        ),
        (
            "position-span-changed(v)",
            mooloop_core::BUFFER_PARAM_POSITION_SPAN,
        ),
        ("jump-held(down)", mooloop_core::BUFFER_PARAM_JUMP),
        ("reverse-held(down)", mooloop_core::BUFFER_PARAM_REVERSE),
        ("stutter-held(down)", mooloop_core::BUFFER_PARAM_STUTTER),
    ];

    for (callback, id) in writes {
        let line = MAIN_SLINT
            .lines()
            .find(|line| line.contains(callback))
            .unwrap_or_else(|| panic!("main.slint no longer wires {callback}"));
        let after_slot = line
            .split_once("index,")
            .unwrap_or_else(|| panic!("{callback} no longer sends a parameter after its slot"))
            .1
            .trim_start();
        let actual: usize = after_slot
            .split(',')
            .next()
            .expect("parameter argument")
            .trim()
            .parse()
            .unwrap_or_else(|error| panic!("{callback} has no literal parameter position: {error}"));
        let expected = EffectKind::Buffer
            .descriptors()
            .iter()
            .position(|descriptor| descriptor.id == id)
            .unwrap_or_else(|| panic!("Buffer descriptor table has no id {id}"));

        assert_eq!(
            actual, expected,
            "{callback} sends descriptor id {actual} as though it were table position {expected}"
        );
    }
}

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


/// A knob's bounds as the face states them.
///
/// Separate from [`knob_in`], which also demands a numeric `default-value:` on
/// the line. The shared oscillator face has none to give: it reads every
/// default from the table at run time (`default-value: root.param-defaults[..]`),
/// which is a copy of nothing, while spelling its *bounds* as literals, which
/// are copies of something. A face can do one without the other, and that is
/// the case neither this test nor `scripts/dupe-audit unchecked-face` could
/// see until 2026-09-18.
fn face_bounds(markup: &str, file: &str, property: &str) -> (f32, f32) {
    let marker = format!("root.{property};");
    let line = markup
        .lines()
        .find(|line| line.contains(&marker) && line.contains("minimum:"))
        .unwrap_or_else(|| panic!("{file} no longer declares bounds for {property}"));
    (
        number(line, "minimum:").unwrap_or_else(|| panic!("{file}: {property} has no minimum")),
        number(line, "maximum:").unwrap_or_else(|| {
            panic!("{file}: {property} states a minimum and no maximum")
        }),
    )
}

fn assert_bounds_agree(
    markup: &str,
    file: &str,
    property: &str,
    kind: DeviceKind,
    id: u32,
) {
    let descriptor = kind
        .descriptor(id)
        .unwrap_or_else(|| panic!("{kind:?} has no descriptor for {property} (id {id})"));
    let (min, max) = face_bounds(markup, file, property);
    assert!(
        (descriptor.min - min).abs() < 1e-4,
        "{file}: {kind:?} {property} face min {min}, table min {}",
        descriptor.min
    );
    assert!(
        (descriptor.max - max).abs() < 1e-4,
        "{file}: {kind:?} {property} face max {max}, table max {}",
        descriptor.max
    );
}

/// The oscillator knobs whose travel the shared face spells as literals, by
/// the offset in an oscillator's five-control block that addresses them.
///
/// Level is absent deliberately: `device-oscillator.slint` draws it with a
/// `TrimKnob` in dB (`maximum: 0`, converted from the table's linear value),
/// so its bounds are not a second copy of the descriptor's `0 .. 1` and
/// comparing the two would be comparing two units.
const OSCILLATOR_BOUNDS: [(&str, u32); 3] = [
    ("semitones", mooloop_core::OSC_OFFSET_SEMITONES),
    ("cents", mooloop_core::OSC_OFFSET_CENTS),
    ("pulse-width", mooloop_core::OSC_OFFSET_PULSE_WIDTH),
];

/// The same four controls on the ML-P8's own face, which states a level range
/// in the table's units and so can be held to it.
const MLP8_OSCILLATOR_BOUNDS: [(&str, u32); 4] = [
    ("semitones", mooloop_core::mlp8::OSC_OFFSET_SEMITONES),
    ("cents", mooloop_core::mlp8::OSC_OFFSET_CENTS),
    ("level", mooloop_core::mlp8::OSC_OFFSET_LEVEL),
    ("pulse-width", mooloop_core::mlp8::OSC_OFFSET_PULSE_WIDTH),
];

/// Every oscillator knob's travel, against the table that addresses it.
///
/// `-48 .. 48` semitones and `-100 .. 100` cents were spelled in four places
/// -- `generator.rs`, `mlp8.rs`, and both faces -- with no test on any pair.
/// The two Rust copies became one constant on 2026-09-18
/// (`mooloop_core::OSC_SEMITONE_RANGE`); this is what holds the markup to it.
///
/// The descriptor's own comment says why a disagreement would matter and why
/// it would be quiet: a modulation depth is a fraction of the *declared*
/// range, so a table narrower than the knob makes a full-depth route sweep
/// less than the control visibly offers. Nothing panics and nothing looks
/// wrong; the synth is simply less modulated than it says.
#[test]
fn every_oscillator_knob_agrees_with_its_table() {
    // One face, instantiated by all three of the v1-era synths with a
    // `param-base`. Every oscillator in a device has the same travel, so
    // oscillator 0 carries the whole claim -- and all three devices share
    // `generator::osc_descriptors`, so a disagreement would be in the markup.
    for kind in [
        DeviceKind::MonoSynth,
        DeviceKind::PolySynth,
        DeviceKind::MlM1,
    ] {
        for (property, offset) in OSCILLATOR_BOUNDS {
            assert_bounds_agree(
                OSCILLATOR_SLINT,
                "device-oscillator.slint",
                property,
                kind,
                mooloop_core::synth_osc_param(0, offset),
            );
        }
    }

    // The ML-P8 draws its own oscillator strip with `P8Knob` rather than
    // instantiating the shared face, which is the reason it needs saying
    // twice here: it is a fourth copy of the same two ranges.
    for (property, offset) in MLP8_OSCILLATOR_BOUNDS {
        assert_bounds_agree(
            MLP8_SLINT,
            "mlp8-device.slint",
            property,
            DeviceKind::MlP8,
            mooloop_core::mlp8::osc_param(0, offset),
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


// --- The effect faces ------------------------------------------------------
//
// The generators declare their ranges on the knob, where `knob_in` can read
// them. An effect does not: its knob carries a plain normalized 0..1 and the
// face converts for the readout, spelling the range as a multiplier and a
// ratio. `0.05 * pow(4000, root.attack)` is `min * (max / min) ^ t`, which is
// `ParamDescriptor::from_normalized` on an exponential curve written out by
// hand -- so those two numbers are the descriptor's two ends, stated a second
// time, which is the thing this file exists to catch.
//
// They needed catching separately because a `pow()` constant does not look
// like a range. `4000` is a ratio, not a maximum; nothing about editing
// `max: 200.0` in the table draws the eye to it.

/// The range an effect face's ratio mapping implies.
struct FaceRatio {
    min: f32,
    max: f32,
}

fn effect_markup(file: &str) -> &'static str {
    match file {
        "bitcrush-device.slint" => BITCRUSH_SLINT,
        "preamp-device.slint" => PREAMP_SLINT,
        "drive-device.slint" => DRIVE_SLINT,
        "compressor-device.slint" => COMPRESSOR_SLINT,
        "delay-device.slint" => DELAY_SLINT,
        "gate-device.slint" => GATE_SLINT,
        "limiter-device.slint" => LIMITER_SLINT,
        "plate-device.slint" => PLATE_SLINT,
        "reverb-device.slint" => REVERB_SLINT,
        other => panic!("no effect markup registered for {other}"),
    }
}

/// Read `<float> name: [min *] pow(ratio, root.x);` back into a range.
///
/// An absent multiplier is 1, not a parse failure: `pow(2000, x)` is the
/// ordinary spelling of a range that starts at one.
fn face_ratio(markup: &str, file: &str, property: &str) -> FaceRatio {
    let marker = format!("<float> {property}:");
    let line = markup
        .lines()
        .find(|line| line.contains(&marker))
        .unwrap_or_else(|| panic!("{file} no longer derives {property}"));
    let body = line
        .split_once(&marker)
        .unwrap_or_else(|| panic!("{file} {property}: marker matched but would not split"))
        .1;
    let (before, after) = body
        .split_once("pow(")
        .unwrap_or_else(|| panic!("{file} {property}: no pow(ratio, ...) to read"));
    let ratio: f32 = after
        .split_once(',')
        .and_then(|(ratio, _)| ratio.trim().parse().ok())
        .unwrap_or_else(|| panic!("{file} {property}: pow's ratio is not a literal"));
    let min: f32 = before
        .trim()
        .trim_end_matches('*')
        .trim()
        .parse()
        .unwrap_or(1.0);
    FaceRatio {
        min,
        max: min * ratio,
    }
}

/// Every effect readout that spells its range out, against the table that
/// owns it. The generators have had this since the drum synth; the effects
/// are the other half, and nothing was checking them.
#[test]
fn every_effect_ratio_readout_agrees_with_its_table() {
    let cases: &[(&str, &str, EffectKind, u32)] = &[
        (
            "bitcrush-device.slint",
            "rate-natural",
            EffectKind::Bitcrush,
            BITCRUSH_PARAM_DOWNSAMPLE,
        ),
        ("drive-device.slint", "drive-natural", EffectKind::Drive, DRIVE_PARAM_DRIVE),
        ("compressor-device.slint", "ratio-value", EffectKind::Compressor, COMP_PARAM_RATIO),
        ("compressor-device.slint", "attack-ms", EffectKind::Compressor, COMP_PARAM_ATTACK_MS),
        ("compressor-device.slint", "release-ms", EffectKind::Compressor, COMP_PARAM_RELEASE_MS),
        ("delay-device.slint", "time-natural", EffectKind::Delay, DELAY_PARAM_TIME_MS),
        ("gate-device.slint", "attack-ms", EffectKind::Gate, GATE_PARAM_ATTACK_MS),
        ("gate-device.slint", "release-ms", EffectKind::Gate, GATE_PARAM_RELEASE_MS),
        ("limiter-device.slint", "release-ms", EffectKind::Limiter, LIMITER_PARAM_RELEASE_MS),
        ("plate-device.slint", "decay-natural", EffectKind::Plate, PLATE_PARAM_DECAY_S),
        ("reverb-device.slint", "predelay-ms", EffectKind::Reverb, REVERB_PARAM_PREDELAY_MS),
    ];

    for (file, property, kind, id) in cases {
        let descriptor = kind
            .descriptor(*id)
            .unwrap_or_else(|| panic!("{kind:?} has no descriptor for id {id}"));
        let face = face_ratio(effect_markup(file), file, property);
        assert!(
            (descriptor.min - face.min).abs() < 1e-6,
            "{file} {property}: face min {}, table min {}",
            face.min,
            descriptor.min
        );
        assert!(
            (descriptor.max - face.max).abs() < 1e-2,
            "{file} {property}: face max {}, table max {}",
            face.max,
            descriptor.max
        );
        assert_eq!(
            descriptor.curve,
            ParamCurve::Exponential,
            "{file} {property}: the face maps it in ratio, so the table must too"
        );
    }
}

/// Read `<float> name: min + root.x * span;` back into a range.
///
/// The linear counterpart of [`face_ratio`], and it exists because the preamp
/// was the first face to spell a linear range out in markup. A dB readout is
/// the natural place for one: nobody writes `pow` for decibels.
fn face_linear(markup: &str, file: &str, property: &str) -> FaceRatio {
    let marker = format!("<float> {property}:");
    let line = markup
        .lines()
        .find(|line| line.contains(&marker))
        .unwrap_or_else(|| panic!("{file} no longer derives {property}"));
    let body = line
        .split_once(&marker)
        .unwrap_or_else(|| panic!("{file} {property}: marker matched but would not split"))
        .1
        .trim()
        .trim_end_matches(';');
    let (min_text, rest) = body
        .split_once('+')
        .unwrap_or_else(|| panic!("{file} {property}: no `min + ...` to read"));
    let span: f32 = rest
        .rsplit_once('*')
        .and_then(|(_, span)| span.trim().parse().ok())
        .unwrap_or_else(|| panic!("{file} {property}: the span is not a literal"));
    let min: f32 = min_text
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("{file} {property}: the minimum is not a literal"));
    FaceRatio {
        min,
        max: min + span,
    }
}

/// The same check as `every_effect_ratio_readout_agrees_with_its_table`, for
/// the faces that map linearly.
///
/// `AGENTS.md` names a range spelled in both a Rust table and the Slint
/// markup as this codebase's characteristic fault. The ratio half has been
/// guarded since the effects pass; this is the half that was not, and the
/// preamp is the first face in it.
#[test]
fn every_effect_linear_readout_agrees_with_its_table() {
    let cases: &[(&str, &str, EffectKind, u32)] = &[
        (
            "preamp-device.slint",
            "drive-db",
            EffectKind::Preamp,
            PREAMP_PARAM_DRIVE_DB,
        ),
        (
            "preamp-device.slint",
            "output-db",
            EffectKind::Preamp,
            PREAMP_PARAM_OUTPUT_DB,
        ),
    ];

    for (file, property, kind, id) in cases {
        let descriptor = kind
            .descriptor(*id)
            .unwrap_or_else(|| panic!("{kind:?} has no descriptor for id {id}"));
        let face = face_linear(effect_markup(file), file, property);
        assert!(
            (descriptor.min - face.min).abs() < 1e-6,
            "{file} {property}: face min {}, table min {}",
            face.min,
            descriptor.min
        );
        assert!(
            (descriptor.max - face.max).abs() < 1e-6,
            "{file} {property}: face max {}, table max {}",
            face.max,
            descriptor.max
        );
        assert_eq!(
            descriptor.curve,
            ParamCurve::Linear,
            "{file} {property}: the face maps it linearly, so the table must too"
        );
    }
}

// --- Every knob on every effect face, paired by the markup's own index ------

/// Every effect face, with the kind whose table its knobs address.
///
/// The lists above this one name each knob by hand, and say why: a derivation
/// from either side would move with the side it was derived from. That is right
/// about the *numbers* and it turned out to be unnecessary for the *pairing*,
/// which is the part that made the lists expensive to extend and left four
/// faces out of them entirely.
///
/// A knob already declares which parameter it edits. It has to: the modulation
/// overlay reads `modulation-allowed[4]`, `modulation-depths[4]` and so on, and
/// the index is the descriptor id, because an effect's ids are its table's
/// positions. So the pairing is the face's own claim, and the numbers still
/// come from two independent places. A face that starts routing a knob's
/// modulation to a different parameter is making a real change and this should
/// follow it there.
const EFFECT_FACES: [(&str, &str, EffectKind); 13] = [
    ("bitcrush-device.slint", BITCRUSH_SLINT, EffectKind::Bitcrush),
    ("buffer-device.slint", BUFFER_SLINT, EffectKind::Buffer),
    ("compressor-device.slint", COMPRESSOR_SLINT, EffectKind::Compressor),
    ("delay-device.slint", DELAY_SLINT, EffectKind::Delay),
    ("drive-device.slint", DRIVE_SLINT, EffectKind::Drive),
    ("eq-device.slint", EQ_SLINT, EffectKind::Eq),
    ("filter-device.slint", FILTER_SLINT, EffectKind::Filter),
    ("gate-device.slint", GATE_SLINT, EffectKind::Gate),
    ("limiter-device.slint", LIMITER_SLINT, EffectKind::Limiter),
    ("modulation-device.slint", MODULATION_SLINT, EffectKind::Modulation),
    ("plate-device.slint", PLATE_SLINT, EffectKind::Plate),
    ("preamp-device.slint", PREAMP_SLINT, EffectKind::Preamp),
    ("reverb-device.slint", REVERB_SLINT, EffectKind::Reverb),
];

/// One `ParameterKnob { .. }` as the markup declares it.
struct FaceKnobBlock {
    line: usize,
    /// The face property the knob is bound to, for the knobs that carry no
    /// modulation index to be found by.
    property: Option<String>,
    /// The descriptor id the knob's modulation overlay addresses, when it has
    /// one. A knob that refuses modulation carries no index.
    param: Option<u32>,
    /// Present only when the knob works in the parameter's own units. A knob
    /// with neither bound works in 0..1 and its `default-value` is a position.
    minimum: Option<f32>,
    maximum: Option<f32>,
    default: f32,
}

/// Every `ParameterKnob` block in `markup` that states a numeric resting value
/// and names the parameter it edits.
///
/// Knobs whose `default-value` is an expression rather than a number are
/// skipped: `modulation-device`'s Rate is `root.tempo-sync ? 2 : 0.445` and
/// `ds01-device`'s whole face reads `root.defaults[root.param]`, which is a
/// copy of nothing and the shape the rest could move to.
fn face_knobs(markup: &str) -> Vec<FaceKnobBlock> {
    let mut out = Vec::new();
    for (line, block) in blocks_after(markup, "ParameterKnob") {
        let Some(default) = optional_number(block, "default-value:") else {
            continue;
        };
        out.push(FaceKnobBlock {
            line,
            property: bound_property(block),
            param: indexed_param(block),
            minimum: optional_number(block, "minimum:"),
            maximum: optional_number(block, "maximum:"),
            default,
        });
    }
    out
}

/// Every `{ .. }` block that follows an occurrence of `marker`, with the
/// 1-based line `marker` sits on. Braces are matched, so a block holding
/// callbacks or nested elements is returned whole.
fn blocks_after<'a>(markup: &'a str, marker: &str) -> Vec<(usize, &'a str)> {
    let mut out = Vec::new();
    let bytes = markup.as_bytes();
    let mut from = 0usize;
    while let Some(found) = markup[from..].find(marker) {
        let start = from + found;
        let Some(open) = markup[start..].find('{').map(|at| start + at) else {
            break;
        };
        // `from` advances to the block's closing brace below, which is always
        // past `start`, so the walk cannot stall on one block.

        let mut depth = 0i32;
        let mut end = open;
        for (offset, byte) in bytes[open..].iter().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + offset;
                        break;
                    }
                }
                _ => {}
            }
        }
        out.push((markup[..start].matches('\n').count() + 1, &markup[open..=end]));
        from = end;
    }
    out
}

/// The face property a knob is bound to, from either binding form.
fn bound_property(block: &str) -> Option<String> {
    for key in ["value <=> root.", "value: root."] {
        if let Some(at) = block.find(key) {
            let rest = &block[at + key.len()..];
            let end = rest.find(';')?;
            return Some(rest[..end].trim().to_string());
        }
    }
    None
}

/// `key`'s value when it is a plain number, and `None` when it is anything
/// else.
///
/// Deliberately not the `number` above, which panics on a non-number: several
/// knobs state an expression on purpose. `modulation-device`'s Rate is
/// `root.tempo-sync ? 2 : 0.445` because its range changes with the sync
/// switch, and its `maximum` is a ternary for the same reason. Those are not
/// copies of a table value and there is nothing for this test to compare them
/// with.
fn optional_number(text: &str, key: &str) -> Option<f32> {
    let at = text.find(key)? + key.len();
    let rest = text[at..].trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// The descriptor a face's overlay index names.
///
/// For every kind but one the index *is* the id: the ids are dense from zero
/// and `modulation-allowed[2]` is parameter 2. The EQ is the exception since
/// `eq-v2/01` -- its seven controls are a view over fifty descriptors, so an
/// index is an `EqFaceControl` and the id depends on which target the face is
/// showing.
///
/// Resolved here against **the target a fresh EQ opens on**, because a
/// modulation overlay index is a face control and a face control is only an
/// id once a selection is chosen. Any target answers the same for the
/// *pairing*, which is all this is used for.
///
/// The limit this comment used to record -- a knob's double-click returning
/// to band 2's default whatever band was selected -- was closed on
/// 2026-09-14 by `EqSpec.defaults`, a resting value per target. The EQ's
/// knobs therefore state no number here any more and `face_knobs` skips
/// them; `tests/eq_face.rs` is where that face is held to its table now, in
/// the stronger `strip_face.rs` shape.
/// The parameter a face's modulation index names.
///
/// **The index is the descriptor id.** `descriptor_policy_flags` and
/// `destination_depths` both write `slot[descriptor.id]` into a vector
/// `descriptor_slots` sizes as `max(id) + 1`, so a face reading
/// `modulation-allowed[n]` is asking about id `n` and nothing else. A
/// retired id leaves a hole in those vectors, and a face that indexed by
/// position instead would read its neighbour's hole.
///
/// **This was changed to a position lookup on 2026-09-16 and that was
/// wrong.** The Buffer had just retired `Offset`, its face still said `[0]`
/// for the knob that had become id 2, and this test reported it exactly as it
/// should have: "a knob routes modulation to parameter 0, which Buffer does
/// not describe". Rewriting the *test* to agree with the face made it pass
/// and left Position's modulation overlay reading a slot nothing writes --
/// an arc that would never have drawn. The fix belonged in the markup.
///
/// The lesson is `AGENTS.md`'s, in a new costume: when a guard fails, check
/// which side moved before deciding which side is wrong.
fn face_param_id(kind: EffectKind, index: u32) -> u32 {
    if kind != EffectKind::Eq {
        return index;
    }
    // The EQ's seven controls are a view over fifty descriptors, so its index
    // is not an id either.
    let opening = EqParams::default().selected_target();
    EqFaceControl::from_face_index(index)
        .and_then(|control| EqParams::id_for_selected(opening, control))
        .unwrap_or(index)
}

/// The descriptor id a knob's modulation overlay addresses, from any of the
/// four arrays it indexes.
fn indexed_param(block: &str) -> Option<u32> {
    for key in [
        "modulation-allowed[",
        "modulation-depths[",
        "modulation-offsets[",
        "modulation-route-counts[",
    ] {
        if let Some(at) = block.find(key) {
            let rest = &block[at + key.len()..];
            let end = rest.find(']')?;
            if let Ok(param) = rest[..end].trim().parse::<u32>() {
                return Some(param);
            }
        }
    }
    None
}

/// The whole point: every effect face's knobs, against the table, without a
/// hand-written list of which knob is which.
///
/// It found three when it was written, all of them the same mistake -- a
/// normalized resting position worked out by hand from an exponential range.
/// Two were rounded (the EQ's Q to two places, the Buffer's crossfade to three)
/// and the Filter's cutoff was simply a round number somebody liked: 0.9, which
/// is 10 kHz, against a table that opens a fresh filter at 8 kHz.
#[test]
fn every_effect_face_knob_agrees_with_its_table() {
    let mut checked = 0usize;
    for (file, markup, kind) in EFFECT_FACES {
        for knob in face_knobs(markup) {
            // A knob that refuses modulation carries no index, so it cannot be
            // paired from the markup. `UNROUTED_KNOBS` names those by hand.
            let Some(param) = knob.param else {
                continue;
            };
            let Some(descriptor) = kind.descriptor(face_param_id(kind, param)) else {
                panic!(
                    "{file}:{}: a knob routes modulation to parameter {param}, which \
                     {kind:?} does not describe",
                    knob.line
                );
            };
            checked += 1;
            match (knob.minimum, knob.maximum) {
                // A knob in the parameter's own units states the range twice.
                (None, None) => {
                    let want = descriptor.to_normalized(descriptor.default);
                    assert!(
                        (knob.default - want).abs() < 1e-3,
                        "{file}:{} {}: the face rests at {}, and the table's default \
                         of {} is {want} of the way along its range. A knob with no \
                         bounds works in 0..1, so these are the same number written \
                         twice.",
                        knob.line,
                        descriptor.name,
                        knob.default,
                        descriptor.default,
                    );
                }
                _ => {
                    if let Some(minimum) = knob.minimum {
                        assert!(
                            (minimum - descriptor.min).abs() < 1e-4,
                            "{file}:{} {}: face min {minimum}, table {}",
                            knob.line,
                            descriptor.name,
                            descriptor.min
                        );
                    }
                    if let Some(maximum) = knob.maximum {
                        assert!(
                            (maximum - descriptor.max).abs() < 1e-4,
                            "{file}:{} {}: face max {maximum}, table {}",
                            knob.line,
                            descriptor.name,
                            descriptor.max
                        );
                    }
                    assert!(
                        (knob.default - descriptor.default).abs() < 1e-4,
                        "{file}:{} {}: face default {}, table {}",
                        knob.line,
                        descriptor.name,
                        knob.default,
                        descriptor.default
                    );
                }
            }
        }
    }

    // A parser that stops matching is a test that stops testing, and this one
    // reads markup it does not compile. The count is the tripwire: it was 54
    // across the thirteen faces when this was written, so a change that halves
    // it has broken the parsing rather than the faces.
    assert!(
        checked >= 45,
        "only {checked} knobs were compared; `face_knobs` has stopped matching \
         the markup it is meant to read"
    );
}

/// The knobs the pairing above cannot reach, named by hand.
///
/// A knob is paired with its parameter by the index its modulation overlay
/// reads, so a control that refuses modulation has no index to be found by.
/// There is one: the container's Mix, which cannot be a modulation destination
/// because resolving a route onto it would mean rewriting the shape of the chain
/// from the audio thread (`effect.rs`, above `CHAIN_PARAM_MIX`).
///
/// A list of one is worth having rather than a note, because `EffectKind::ALL`
/// is fourteen and the automatic pass covers thirteen. Leaving the fourteenth to
/// a sentence is how the effect faces came to be uncovered while the generators
/// were checked.
const UNROUTED_KNOBS: [(&str, &str, EffectKind, &str, u32); 1] = [(
    "container-device.slint",
    CONTAINER_SLINT,
    EffectKind::Chain,
    "mix",
    mooloop_core::CHAIN_PARAM_MIX,
)];

#[test]
fn every_unrouted_face_knob_agrees_with_its_table() {
    for (file, markup, kind, property, id) in UNROUTED_KNOBS {
        let descriptor = kind
            .descriptor(id)
            .unwrap_or_else(|| panic!("{kind:?} has no descriptor for id {id}"));
        let knob = face_knobs(markup)
            .into_iter()
            .find(|knob| knob.property.as_deref() == Some(property))
            .unwrap_or_else(|| panic!("{file} no longer declares a knob bound to {property}"));
        let want = match (knob.minimum, knob.maximum) {
            (None, None) => descriptor.to_normalized(descriptor.default),
            _ => descriptor.default,
        };
        assert!(
            (knob.default - want).abs() < 1e-4,
            "{file}:{} {}: face rests at {}, table says {want}",
            knob.line,
            descriptor.name,
            knob.default
        );
    }
}

/// Aux In's face, which is outside both passes above because Aux In is not an
/// `EffectKind`: it has its own `aux_in::DESCRIPTORS` and its own
/// `aux_in::descriptor`, so there is nothing for `EFFECT_FACES` to name it
/// with.
///
/// Its Level knob is the pilot for the generator row struct finding 4 of
/// `reports/fable-2026-09-22.md` asks for
/// (`docs/plans/generator-face-rows/`): as of that pilot it works in
/// normalized space, like every effect knob with no stated range, rather
/// than the dB-derived natural range it used before. The check follows —
/// `every_effect_face_knob_agrees_with_its_table`'s `(None, None)` branch is
/// the model — because a bare `default-value:` is a normalized position,
/// held to `descriptor.to_normalized(descriptor.default)` rather than to
/// `descriptor.default` directly.
#[test]
fn the_aux_in_face_agrees_with_its_table() {
    let descriptor = mooloop_core::aux_in::descriptor(mooloop_core::aux_in::PARAM_LEVEL)
        .expect("aux_in has no descriptor for PARAM_LEVEL");
    let knob = face_knobs(AUX_IN_SLINT)
        .into_iter()
        .find(|knob| knob.property.as_deref() == Some("level"))
        .expect("aux-in-device.slint no longer declares a knob bound to level");

    assert_eq!(
        knob.minimum, None,
        "aux-in-device.slint:{}: Level states a minimum again, so its default \
         should be read against the table's natural units, not normalized",
        knob.line
    );
    assert_eq!(
        knob.maximum, None,
        "aux-in-device.slint:{}: Level states a maximum again, so its default \
         should be read against the table's natural units, not normalized",
        knob.line
    );

    let want = descriptor.to_normalized(descriptor.default);
    assert!(
        (knob.default - want).abs() < 1e-3,
        "aux-in-device.slint:{}: the face rests at {}, and the table's default \
         of {} -- `gain::reference_level_gain()`, held to the real thing by a \
         chain of two tests in `aux_in.rs` -- is {want} of the way along its \
         range. A knob with no bounds works in 0..1, so these are the same \
         number written twice.",
        knob.line,
        knob.default,
        descriptor.default,
    );
}

/// The read/write twin of `the_buffer_face_sends_descriptor_positions_for_edits`
/// and `every_face_reads_its_parameters_by_id`, scoped to the one generator
/// face migrated so far. `main.slint` feeds Aux In's knob from `source.pK`
/// and forwards its edits through `source-param-changed(id, v)`; both `K`
/// and `id` have to be `aux_in::PARAM_LEVEL`, not its position in a table
/// that -- for Aux In specifically -- currently agrees with it, which is
/// exactly the trap `AGENTS.md`'s "Parameter identity across the session
/// boundary" section describes.
#[test]
fn the_aux_in_face_reads_and_writes_level_by_id() {
    let level_id = mooloop_core::aux_in::PARAM_LEVEL;

    // Not the bare component name: `main.slint` also names it in its own
    // `import { AuxInDeviceFace } from "aux-in-device.slint";`, whose brace
    // `blocks_after` would otherwise match instead of the instantiation's.
    let (_, block) = blocks_after(MAIN_SLINT, ": AuxInDeviceFace {")
        .into_iter()
        .next()
        .expect("main.slint no longer instantiates AuxInDeviceFace");

    let read = block
        .lines()
        .find(|line| line.trim().starts_with("level:"))
        .unwrap_or_else(|| panic!("AuxInDeviceFace's instantiation no longer binds level"));
    let field: usize = read
        .split("source.p")
        .nth(1)
        .and_then(|rest| {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        })
        .unwrap_or_else(|| panic!("{read} does not read Level from source.pK"));
    assert_eq!(
        field, level_id as usize,
        "AuxInDeviceFace.level reads source.p{field}; the row carries Level in \
         p{level_id}"
    );

    let write = block
        .lines()
        .find(|line| line.contains("level-changed(v)"))
        .unwrap_or_else(|| panic!("AuxInDeviceFace's instantiation no longer forwards level-changed"));
    let sent: u32 = write
        .split_once("source-param-changed(")
        .and_then(|(_, rest)| rest.split(',').next())
        .and_then(|id| id.trim().parse().ok())
        .unwrap_or_else(|| panic!("{write} has no literal id argument"));
    assert_eq!(
        sent, level_id,
        "level-changed forwards id {sent} to source-param-changed; Aux In's \
         Level descriptor is id {level_id}"
    );
}

// --- The read side: which row field each face displays ---------------------

/// The face properties `main.slint` feeds from a row field that no knob in the
/// face's own markup can name, because the control is a selector, a switch, a
/// slider, a knob whose `value` is an expression, or a knob that refuses
/// modulation. Each is the descriptor id the property displays.
const UNKNOBBED_BINDINGS: [(EffectKind, &str, u32); 12] = [
    (EffectKind::Filter, "mode", mooloop_core::FILTER_PARAM_MODE),
    (EffectKind::Filter, "slope", mooloop_core::FILTER_PARAM_SLOPE),
    (EffectKind::Drive, "curve", mooloop_core::DRIVE_PARAM_CURVE),
    (EffectKind::Drive, "output", mooloop_core::DRIVE_PARAM_OUTPUT),
    (EffectKind::Preamp, "voicing", mooloop_core::PREAMP_PARAM_VOICING),
    (EffectKind::Bitcrush, "style", mooloop_core::BITCRUSH_PARAM_STYLE),
    (EffectKind::Delay, "time", mooloop_core::DELAY_PARAM_TIME_MS),
    (EffectKind::Delay, "mode", mooloop_core::DELAY_PARAM_MODE),
    (EffectKind::Modulation, "mode", mooloop_core::MODULATION_PARAM_MODE),
    (EffectKind::Modulation, "rate", mooloop_core::MODULATION_PARAM_RATE_HZ),
    (EffectKind::Buffer, "quantize", mooloop_core::BUFFER_PARAM_QUANTIZE),
    (EffectKind::Chain, "mix", mooloop_core::CHAIN_PARAM_MIX),
];

/// The face properties fed from the row's reserved fields: a tempo-sync flag
/// and a musical division, which a device keeps beside its parameters and
/// which have no descriptor id at all.
const RESERVED_BINDINGS: [(EffectKind, &str); 4] = [
    (EffectKind::Delay, "tempo-sync"),
    (EffectKind::Delay, "time-division"),
    (EffectKind::Modulation, "tempo-sync"),
    (EffectKind::Modulation, "rate-division"),
];

/// The markup a `main.slint` face element is declared in.
fn face_markup(face: &str) -> (&'static str, &'static str) {
    match face {
        "FilterDeviceFace" => ("filter-device.slint", FILTER_SLINT),
        "DriveDeviceFace" => ("drive-device.slint", DRIVE_SLINT),
        "PreampDeviceFace" => ("preamp-device.slint", PREAMP_SLINT),
        "BitcrushDeviceFace" => ("bitcrush-device.slint", BITCRUSH_SLINT),
        "DelayDeviceFace" => ("delay-device.slint", DELAY_SLINT),
        "GateDeviceFace" => ("gate-device.slint", GATE_SLINT),
        "CompressorDeviceFace" => ("compressor-device.slint", COMPRESSOR_SLINT),
        "LimiterDeviceFace" => ("limiter-device.slint", LIMITER_SLINT),
        "EqDeviceFace" => ("eq-device.slint", EQ_SLINT),
        "ReverbDeviceFace" => ("reverb-device.slint", REVERB_SLINT),
        "ModulationDeviceFace" => ("modulation-device.slint", MODULATION_SLINT),
        "PlateDeviceFace" => ("plate-device.slint", PLATE_SLINT),
        "BufferDeviceFace" => ("buffer-device.slint", BUFFER_SLINT),
        "ContainerDeviceFace" => ("container-device.slint", CONTAINER_SLINT),
        other => panic!("main.slint dispatches to {other}, which this test does not know"),
    }
}

/// `prop: <expression reading slot.pK>;` -> `(prop, K)`, for one line of a
/// face element's body in `main.slint`.
fn row_field_binding(line: &str) -> Option<(String, usize)> {
    let (property, expression) = line.trim().split_once(':')?;
    let property = property.trim();
    if property.is_empty()
        || !property
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return None;
    }
    let at = expression.find("slot.p")? + "slot.p".len();
    let digits: String = expression[at..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let field = digits.parse().ok()?;
    Some((property.to_string(), field))
}

/// The read-side twin of `the_buffer_face_sends_descriptor_positions_for_edits`.
///
/// `EffectSlotRow.pN` is filled by descriptor **id**, so a face must read the
/// field numbered by the id of the parameter it displays. Every kind but two
/// has `id == position`, which is why a face reading by position looked
/// right. The Reverb's ids are 8..15 -- 0..7 are the retired convolution-era
/// parameters -- and its face kept reading `p0..p7`, which nothing fills for
/// that kind: every knob opened at zero whatever the saved values, and Low
/// Cut (id 15) had nowhere to go but the field reserved for the tempo-sync
/// flag. Found by the 2026-09-17 review, a day after the Buffer's write-side
/// incident, and for the same reason: only the write side was tested.
///
/// Each `if slot.kind == N : XDeviceFace` element in `main.slint` is read for
/// its `prop: slot.pK` bindings. `prop` is resolved to an id through the
/// face's own knob for it -- the modulation index that knob reads, which
/// `every_effect_face_knob_agrees_with_its_table` already holds to the table
/// -- or through `UNKNOBBED_BINDINGS` where the face has no such knob. The
/// reserved tempo pair must sit past every id any kind has. The EQ is skipped:
/// its row is a documented view over one target, numbered by `EqFaceControl`.
#[test]
fn every_face_reads_its_parameters_by_id() {
    let highest_id = EffectKind::ALL
        .iter()
        .filter(|kind| **kind != EffectKind::Eq)
        .flat_map(|kind| kind.descriptors().iter().map(|descriptor| descriptor.id))
        .max()
        .expect("effect kinds describe parameters") as usize;

    let mut failures = Vec::new();
    let mut checked = 0usize;
    for (line, body) in blocks_after(MAIN_SLINT, "if slot.kind == ") {
        let header = MAIN_SLINT.lines().nth(line - 1).expect("header line");
        let Some((number, face)) = header
            .trim()
            .strip_prefix("if slot.kind == ")
            .and_then(|rest| rest.split_once(':'))
        else {
            continue;
        };
        let number: i32 = number.trim().parse().expect("a literal kind number");
        let face = face.trim().trim_end_matches('{').trim();
        let kind = EffectKind::ALL
            .into_iter()
            .find(|kind| mooloop_ui::effect_kind_index(*kind) == number)
            .unwrap_or_else(|| panic!("main.slint:{line}: no effect kind is numbered {number}"));
        if kind == EffectKind::Eq {
            continue;
        }
        let (file, markup) = face_markup(face);
        let knobs: Vec<(String, u32)> = blocks_after(markup, "ParameterKnob")
            .into_iter()
            .filter_map(|(_, block)| Some((bound_property(block)?, indexed_param(block)?)))
            // A knob whose `value` is an expression (the modulation effect's
            // Rate) is not bound to one property; `UNKNOBBED_BINDINGS` names it.
            .filter(|(property, _)| {
                property
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
            })
            .collect();

        let bindings: Vec<(String, usize)> =
            body.lines().filter_map(row_field_binding).collect();

        for (property, field) in &bindings {
            if RESERVED_BINDINGS
                .iter()
                .any(|(k, p)| *k == kind && *p == property.as_str())
            {
                if *field <= highest_id {
                    failures.push(format!(
                        "{face}.{property} reads p{field} as a reserved field, but \
                         ids run to {highest_id}, so p{field} is also where a \
                         parameter is carried"
                    ));
                }
                checked += 1;
                continue;
            }
            let id = knobs
                .iter()
                .find(|(bound, _)| bound == property)
                .map(|(_, index)| face_param_id(kind, *index))
                .or_else(|| {
                    UNKNOBBED_BINDINGS
                        .iter()
                        .find(|(k, p, _)| *k == kind && *p == property.as_str())
                        .map(|(_, _, id)| *id)
                });
            let Some(id) = id else {
                failures.push(format!(
                    "{face}.{property} reads p{field}, and neither a knob in {file} \
                     nor UNKNOBBED_BINDINGS says which {kind:?} parameter it shows"
                ));
                continue;
            };
            let name = kind
                .descriptor(id)
                .map(|descriptor| descriptor.name)
                .unwrap_or("<undescribed>");
            if *field != id as usize {
                failures.push(format!(
                    "{face}.{property} ({name}, id {id}) reads p{field}; the row \
                     carries it in p{id}"
                ));
            }
            checked += 1;
        }

        // And the other way: a knob the face draws for a parameter has to be
        // fed from the row at all.
        for (property, index) in &knobs {
            if !bindings.iter().any(|(bound, _)| bound == property) {
                failures.push(format!(
                    "{face}.{property} (id {}) is a knob in {file} that main.slint \
                     never feeds from the row",
                    face_param_id(kind, *index)
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} face binding(s) read the wrong row field:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
    // The tripwire, as above: 66 bindings across thirteen faces when this was
    // written.
    assert!(
        checked >= 60,
        "only {checked} bindings were compared; the main.slint parse has stopped \
         matching the markup it is meant to read"
    );
}
