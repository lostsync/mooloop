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

use mooloop_core::{DeviceKind, EffectKind, ParamCurve, ParamDescriptor};
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
const CONTAINER_SLINT: &str = include_str!("../ui/container-device.slint");
const AUX_IN_SLINT: &str = include_str!("../ui/aux-in-device.slint");

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
    let bytes = markup.as_bytes();
    let mut from = 0usize;
    while let Some(found) = markup[from..].find("ParameterKnob") {
        let start = from + found;
        let Some(open) = markup[start..].find('{').map(|at| start + at) else {
            break;
        };
        // `from` advances to the block's closing brace below, which is always
        // past `start`, so the walk cannot stall on one knob.

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
        let block = &markup[open..=end];
        from = end;
        let Some(default) = optional_number(block, "default-value:") else {
            continue;
        };
        out.push(FaceKnobBlock {
            line: markup[..start].matches('\n').count() + 1,
            property: bound_property(block),
            param: indexed_param(block),
            minimum: optional_number(block, "minimum:"),
            maximum: optional_number(block, "maximum:"),
            default,
        });
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
            let Some(descriptor) = kind.descriptor(param) else {
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
/// Its Level knob was the last face literal `scripts/dupe-audit
/// unchecked-face` reported that anything could be done about, and it is the
/// third spelling of one number. `aux_in.rs` says so itself, at length, above
/// the literal it holds: that default is `gain::reference_level_gain()`
/// written out because a const struct cannot call a function, and it is held
/// to the real thing by a chain of two tests. The markup's copy was held to
/// nothing at all.
///
/// The maximum is the same shape one level along. The face writes
/// `GainMath.db-to-linear(12.0)` where the table says `MAX_LINEAR_GAIN`, so
/// the two agree only as long as that `12.0` is `gain::MAX_DB` -- which is
/// what this checks, rather than evaluating the conversion twice.
#[test]
fn the_aux_in_face_agrees_with_its_table() {
    let descriptor = mooloop_core::aux_in::descriptor(mooloop_core::aux_in::PARAM_LEVEL)
        .expect("aux_in has no descriptor for PARAM_LEVEL");
    let knob = face_knobs(AUX_IN_SLINT)
        .into_iter()
        .find(|knob| knob.property.as_deref() == Some("level"))
        .expect("aux-in-device.slint no longer declares a knob bound to level");

    assert_eq!(
        knob.minimum,
        Some(descriptor.min),
        "aux-in-device.slint:{}: face min against the table's {}",
        knob.line,
        descriptor.min
    );
    assert!(
        (knob.default - descriptor.default).abs() < 1e-6,
        "aux-in-device.slint:{}: the face rests at {}, the table at {} -- and the \
         table's is `gain::reference_level_gain()`, so this is that number a third \
         time.",
        knob.line,
        knob.default,
        descriptor.default
    );

    // `maximum` is an expression, so `face_knobs` reads no number from it.
    let top = AUX_IN_SLINT
        .lines()
        .find(|line| line.contains("maximum: GainMath.db-to-linear("))
        .unwrap_or_else(|| {
            panic!("aux-in-device.slint's Level knob stopped topping out at a dB value")
        });
    let stated: f32 = top
        .split_once("db-to-linear(")
        .and_then(|(_, rest)| rest.split(')').next())
        .and_then(|number| number.trim().parse().ok())
        .unwrap_or_else(|| panic!("not a plain dB literal: {top}"));
    assert!(
        (stated - mooloop_core::gain::MAX_DB).abs() < 1e-4,
        "aux-in-device.slint tops the Level knob at {stated} dB where gain::MAX_DB \
         is {}, so the face and `MAX_LINEAR_GAIN` in the table no longer meet",
        mooloop_core::gain::MAX_DB
    );
}
