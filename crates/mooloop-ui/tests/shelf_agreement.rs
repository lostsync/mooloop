//! The modulation shelf's numbers, held to the tables they come from.
//!
//! Three checks live here. The knob ids the editor sends edits under must be
//! `mooloop-core`'s; every range the shelf states must be the descriptor's;
//! and a knob and the caption under it must read the same.
//!
//! # A knob and its caption are one value
//!
//! The modulation shelf draws each parameter as a cell: a name above, a
//! `MiniKnob` in the middle, and a caption below. The knob's `value-text` is
//! what the tooltip and the accessibility layer report; the caption is what
//! the eye reads. Both are written out in full, six lines apart, from the
//! same `selected-values` entry -- fifteen times.
//!
//! That is a range written a second time wearing different clothes, and it
//! had already drifted once: the LFO's Smoothing knob reported `150 ms` while
//! its own caption said `150ms`, against twenty-six spaced millisecond
//! readouts elsewhere in the program and a Rust formatter that attaches every
//! unit but `%` with a space.
//!
//! Two cells differ on purpose, and only in one direction: the knob shows a
//! bare count and the caption names its unit, `16` against `16 STEPS`. That
//! is allowed exactly when the knob's own trailing literal is empty, which is
//! narrow enough that the spacing bug above still fails this test -- there
//! the knob's literal was ` ms`, not empty.

const SHELF: &str = include_str!("../ui/modulation-shelf.slint");

/// Collapse runs of whitespace, so an expression broken across two lines
/// compares against the same expression written on one.
fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A knob's `value-text` expression and the caption that follows it.
struct Readout {
    line: usize,
    knob: String,
    caption: String,
}

/// Read every cell out of the markup.
///
/// A `value-text` may wrap, so it is gathered until its terminating
/// semicolon; the caption is the next `Text` whose `text:` runs up to the
/// `color:` that every one of them carries.
fn readouts() -> Vec<Readout> {
    let lines: Vec<&str> = SHELF.lines().collect();
    let mut found = Vec::new();

    for (index, line) in lines.iter().enumerate() {
        let Some((_, after)) = line.split_once("value-text:") else {
            continue;
        };
        // Gather the expression across as many lines as it takes.
        let mut expression = String::from(after);
        let mut cursor = index;
        while !expression.contains(';') && cursor + 1 < lines.len() {
            cursor += 1;
            expression.push(' ');
            expression.push_str(lines[cursor]);
        }
        let Some((knob, _)) = expression.split_once(';') else {
            continue;
        };

        // The caption is the next `Text` in the cell. Ten lines is the whole
        // of the longest cell and stops this running into the next one.
        let caption = lines[cursor + 1..]
            .iter()
            .take(10)
            .find(|candidate| candidate.contains("Text {") && candidate.contains("text:"))
            .and_then(|candidate| candidate.split_once("text:"))
            .and_then(|(_, rest)| rest.split_once("; color:"))
            .map(|(caption, _)| caption);

        // A knob without a caption under it is a legitimate cell shape, not a
        // failure -- only pairs are this test's business.
        if let Some(caption) = caption {
            found.push(Readout {
                line: index + 1,
                knob: squash(knob),
                caption: squash(caption),
            });
        }
    }
    found
}

/// The caption may name a unit the knob leaves off, and may do nothing else.
fn caption_is_the_knob_plus_a_unit(knob: &str, caption: &str) -> bool {
    let Some(stem) = knob.strip_suffix("+ \"\"") else {
        return false;
    };
    let Some(tail) = caption.strip_prefix(stem) else {
        return false;
    };
    // `+ " STEPS"` and nothing cleverer: a word, introduced by a space.
    tail.starts_with("+ \" ") && tail.ends_with('"')
}

#[test]
fn every_shelf_knob_agrees_with_its_caption() {
    let found = readouts();
    assert!(
        found.len() >= 15,
        "only {} knob/caption pairs found; the cell shape changed and this \
         test is no longer reading the shelf",
        found.len()
    );

    for readout in &found {
        if readout.knob == readout.caption {
            continue;
        }
        assert!(
            caption_is_the_knob_plus_a_unit(&readout.knob, &readout.caption),
            "modulation-shelf.slint:{}: the knob and its caption report one \
             value two ways\n  knob:    {}\n  caption: {}",
            readout.line,
            readout.knob,
            readout.caption
        );
    }
}

// ---------------------------------------------------------------------------
// The ids, and then the ranges.
// ---------------------------------------------------------------------------

use mooloop_core::{
    ParamCurve, ParamDescriptor, ENVELOPE_DESCRIPTORS, LFO_DESCRIPTORS, MATH_DESCRIPTORS,
    RANDOM_DESCRIPTORS, STEP_DESCRIPTORS,
};

/// Every property of every `<Module>Param` global in the shelf, against the
/// `mooloop-core` constant it mirrors.
///
/// This is a hand-written pairing and it has a hand-written pairing's failure
/// mode, so both directions are checked below: every entry here must exist in
/// the markup, and every property in the markup must appear here. The pairing
/// cannot be derived, because the two sides do not spell the same names --
/// `rate` is `RATE_HZ`, `fade-in` is `FADE_IN_S`, `smoothing` is
/// `SMOOTHING_S`. A derivation that papered over that would be deriving one
/// side from the other, which is the thing being guarded against.
#[rustfmt::skip]
const SHELF_IDS: [(&str, &str, u32); 42] = [
    ("Lfo", "rate", mooloop_core::LFO_PARAM_RATE_HZ),
    ("Lfo", "depth", mooloop_core::LFO_PARAM_DEPTH),
    ("Lfo", "waveform", mooloop_core::LFO_PARAM_WAVEFORM),
    ("Lfo", "phase", mooloop_core::LFO_PARAM_PHASE),
    ("Lfo", "tempo-sync", mooloop_core::LFO_PARAM_TEMPO_SYNC),
    ("Lfo", "rate-division", mooloop_core::LFO_PARAM_RATE_DIVISION),
    ("Lfo", "retrigger", mooloop_core::LFO_PARAM_RETRIGGER),
    ("Lfo", "fade-in", mooloop_core::LFO_PARAM_FADE_IN_S),
    ("Lfo", "fade-sync", mooloop_core::LFO_PARAM_FADE_IN_SYNC),
    ("Lfo", "fade-division", mooloop_core::LFO_PARAM_FADE_IN_DIVISION),
    ("Lfo", "smoothing", mooloop_core::LFO_PARAM_SMOOTHING_S),
    ("Lfo", "pulse-width", mooloop_core::LFO_PARAM_PULSE_WIDTH),
    ("Env", "attack", mooloop_core::ENV_PARAM_ATTACK_S),
    ("Env", "attack-sync", mooloop_core::ENV_PARAM_ATTACK_SYNC),
    ("Env", "attack-division", mooloop_core::ENV_PARAM_ATTACK_DIVISION),
    ("Env", "decay", mooloop_core::ENV_PARAM_DECAY_S),
    ("Env", "decay-sync", mooloop_core::ENV_PARAM_DECAY_SYNC),
    ("Env", "decay-division", mooloop_core::ENV_PARAM_DECAY_DIVISION),
    ("Env", "sustain", mooloop_core::ENV_PARAM_SUSTAIN),
    ("Env", "release", mooloop_core::ENV_PARAM_RELEASE_S),
    ("Env", "release-sync", mooloop_core::ENV_PARAM_RELEASE_SYNC),
    ("Env", "release-division", mooloop_core::ENV_PARAM_RELEASE_DIVISION),
    ("Env", "amount", mooloop_core::ENV_PARAM_AMOUNT),
    ("Step", "length", mooloop_core::STEP_PARAM_LENGTH),
    ("Step", "division", mooloop_core::STEP_PARAM_DIVISION),
    ("Step", "glide", mooloop_core::STEP_PARAM_GLIDE),
    ("Step", "trigger", mooloop_core::STEP_PARAM_TRIGGER),
    ("Step", "value-base", mooloop_core::STEP_PARAM_VALUE_BASE),
    ("Random", "rate", mooloop_core::RANDOM_PARAM_RATE_HZ),
    ("Random", "tempo-sync", mooloop_core::RANDOM_PARAM_TEMPO_SYNC),
    ("Random", "rate-division", mooloop_core::RANDOM_PARAM_RATE_DIVISION),
    ("Random", "trigger", mooloop_core::RANDOM_PARAM_TRIGGER),
    ("Random", "bipolar", mooloop_core::RANDOM_PARAM_BIPOLAR),
    ("Random", "probability", mooloop_core::RANDOM_PARAM_PROBABILITY),
    ("Random", "quantize", mooloop_core::RANDOM_PARAM_QUANTIZE),
    ("Random", "drunk", mooloop_core::RANDOM_PARAM_DRUNK),
    ("Random", "walk", mooloop_core::RANDOM_PARAM_WALK),
    ("Math", "input-slot", mooloop_core::MATH_PARAM_INPUT_SLOT),
    ("Math", "op", mooloop_core::MATH_PARAM_OP),
    ("Math", "operand", mooloop_core::MATH_PARAM_OPERAND),
    ("Math", "clamp-low", mooloop_core::MATH_PARAM_CLAMP_LOW),
    ("Math", "clamp-high", mooloop_core::MATH_PARAM_CLAMP_HIGH),
];

fn descriptors(module: &str) -> &'static [ParamDescriptor] {
    match module {
        "Lfo" => &LFO_DESCRIPTORS,
        "Env" => &ENVELOPE_DESCRIPTORS,
        "Step" => &STEP_DESCRIPTORS,
        "Random" => &RANDOM_DESCRIPTORS,
        "Math" => &MATH_DESCRIPTORS,
        other => panic!("no descriptor table for {other}Param"),
    }
}

/// The `{ .. }` body of the next item `opener` starts at or after `from`,
/// brace-matched, with the byte offset it starts at and the line it is on.
fn body<'a>(markup: &'a str, opener: &str, from: usize) -> Option<(usize, usize, usize, &'a str)> {
    let at = markup[from..].find(opener)? + from;
    let open = markup[at..].find('{')? + at;
    let mut depth = 0i32;
    for (offset, byte) in markup.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    let end = open + offset;
                    let line = markup[..at].matches('\n').count() + 1;
                    return Some((at, line, end, &markup[open..=end]));
                }
            }
            _ => {}
        }
    }
    None
}

/// The ids the shelf sends every edit under, against `mooloop-core`'s.
///
/// These are the wire contract -- `param-changed(slot, id, value)` goes
/// straight to `ModulatorParams::set` -- so a renumbering on one side alone
/// would not fail to compile, would not fail any other test, and would wire
/// every control on that module to the wrong parameter. The globals' own
/// comment says they "are never renumbered", which is a policy rather than a
/// guard.
#[test]
fn the_shelf_param_ids_match_the_rust_tables() {
    let mut seen: Vec<(String, String)> = Vec::new();
    for module in ["Lfo", "Env", "Step", "Random", "Math"] {
        let opener = format!("global {module}Param ");
        let (_, line, _, block) = body(SHELF, &opener, 0)
            .unwrap_or_else(|| panic!("modulation-shelf.slint no longer declares {opener}{{"));
        for declaration in block.lines() {
            let Some((_, rest)) = declaration.split_once("out property <int> ") else {
                continue;
            };
            let Some((name, value)) = rest.split_once(':') else {
                continue;
            };
            let name = name.trim();
            let stated: u32 = value
                .trim()
                .trim_end_matches(';')
                .parse()
                .unwrap_or_else(|_| panic!("{module}Param.{name} is not a plain integer"));
            let expected = SHELF_IDS
                .iter()
                .find(|(owner, property, _)| *owner == module && *property == name)
                .unwrap_or_else(|| {
                    panic!(
                        "modulation-shelf.slint:{line}: {module}Param.{name} is not in \
                         SHELF_IDS, so nothing holds it to a `mooloop-core` constant"
                    )
                })
                .2;
            assert_eq!(
                stated, expected,
                "{module}Param.{name}: the shelf sends {stated}, mooloop-core says {expected}"
            );
            seen.push((module.to_string(), name.to_string()));
        }
    }

    for (module, name, _) in SHELF_IDS {
        assert!(
            seen.iter().any(|(m, n)| m == module && n == name),
            "SHELF_IDS names {module}Param.{name}, which the markup no longer declares"
        );
    }
}

/// One knob as the shelf declares it.
struct ShelfKnob {
    line: usize,
    module: &'static str,
    property: String,
    /// Absent means the knob took `MiniKnob`'s own `0`/`1`, which is the
    /// shelf's way of saying the parameter's natural range is already 0..1.
    minimum: Option<f32>,
    maximum: Option<f32>,
    default: Option<f32>,
    /// Whether the knob states `ValueScale.logarithmic`. A knob that states
    /// nothing is not recorded as linear: the shelf's default and a stepped
    /// descriptor are not in conflict, so there would be nothing to compare.
    logarithmic: bool,
}

/// Every range the shelf states, against the descriptor that owns it.
///
/// Twenty-one ranges were written out by hand here and nothing read any of
/// them back. All twenty-one agreed when this was written, so what it prevents
/// is drift rather than a present fault -- but it is the same shape as the
/// device faces, and those did drift: three of them rested at a normalized
/// position somebody had worked out by hand from an exponential range, and one
/// was simply a round number that looked right.
///
/// It is not reachable by extending `slint_face_agreement.rs`, which walks a
/// list of *device* faces; the shelf is not one.
#[test]
fn every_shelf_knob_range_agrees_with_its_table() {
    let mut knobs: Vec<ShelfKnob> = Vec::new();
    for (widget, min_key, max_key, default_key) in [
        ("SyncMiniKnob {", "free-minimum:", "free-maximum:", "free-default:"),
        ("MiniKnob {", "minimum:", "maximum:", "default-value:"),
    ] {
        let mut from = 0usize;
        while let Some((at, line, end, block)) = body(SHELF, widget, from) {
            from = end;
            // `MiniKnob {` is a suffix of `SyncMiniKnob {`, so the second pass
            // would otherwise re-read every knob the first one already has --
            // with the wrong key set, and silently, because the keys it looks
            // for are then simply absent and every range reads as the default
            // 0..1. Anchor on the character before the name.
            if at > 0 && SHELF.as_bytes()[at - 1].is_ascii_alphanumeric() {
                continue;
            }
            let Some(reference) = block.split_once("value: root.selected-values[") else {
                continue;
            };
            let Some((address, _)) = reference.1.split_once(']') else {
                continue;
            };
            let Some((owner, property)) = address.split_once("Param.") else {
                continue;
            };
            let Some(module) = ["Lfo", "Env", "Step", "Random", "Math"]
                .into_iter()
                .find(|known| owner.ends_with(known))
            else {
                continue;
            };
            fn number(block: &str, key: &str) -> Option<f32> {
                let at = block.find(key)? + key.len();
                let rest = block[at..].trim_start();
                let end = rest
                    .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
                    .unwrap_or(rest.len());
                rest[..end].parse().ok()
            }
            knobs.push(ShelfKnob {
                line,
                module,
                property: property.trim().to_string(),
                minimum: number(block, min_key),
                maximum: number(block, max_key),
                default: number(block, default_key),
                logarithmic: block.contains("ValueScale.logarithmic"),
            });
        }
    }

    for knob in &knobs {
        let id = SHELF_IDS
            .iter()
            .find(|(owner, property, _)| *owner == knob.module && *property == knob.property)
            .unwrap_or_else(|| {
                panic!(
                    "modulation-shelf.slint:{}: a knob edits {}Param.{}, which SHELF_IDS \
                     does not name",
                    knob.line, knob.module, knob.property
                )
            })
            .2;
        let descriptor = descriptors(knob.module)
            .iter()
            .find(|descriptor| descriptor.id == id)
            .unwrap_or_else(|| {
                panic!(
                    "modulation-shelf.slint:{}: {}Param.{} is id {id}, which its table does \
                     not describe",
                    knob.line, knob.module, knob.property
                )
            });
        let minimum = knob.minimum.unwrap_or(0.0);
        let maximum = knob.maximum.unwrap_or(1.0);
        assert!(
            (minimum - descriptor.min).abs() < 1e-4,
            "modulation-shelf.slint:{} {}: face min {minimum}, table {}",
            knob.line,
            descriptor.name,
            descriptor.min
        );
        assert!(
            (maximum - descriptor.max).abs() < 1e-4,
            "modulation-shelf.slint:{} {}: face max {maximum}, table {}",
            knob.line,
            descriptor.name,
            descriptor.max
        );
        if let Some(default) = knob.default {
            assert!(
                (default - descriptor.default).abs() < 1e-4,
                "modulation-shelf.slint:{} {}: face rests at {default}, table says {}",
                knob.line,
                descriptor.name,
                descriptor.default
            );
        }
        // The two names for one idea. Slint calls it `ValueScale.logarithmic`
        // and `mooloop-core` calls it `ParamCurve::Exponential` -- "even in
        // ratio", which is the same law read from the two ends. A knob drawn
        // that way over a table that says `Linear` is a disagreement about
        // what the parameter *is*, and it went unseen on the LFO's and the
        // Random module's Rate until 2026-09-14 because nothing outside
        // `modulation.rs` read these tables.
        //
        // Only a knob that states the scale is checked. One that states
        // nothing took the shelf's linear default, which does not contradict a
        // `Stepped` descriptor, and most of these knobs sit on one.
        if knob.logarithmic {
            assert!(
                matches!(descriptor.curve, ParamCurve::Exponential),
                "modulation-shelf.slint:{} {}: the knob is drawn \
                 `ValueScale.logarithmic`, and the table says {:?}",
                knob.line,
                descriptor.name,
                descriptor.curve
            );
        }
    }

    // The tripwire. Twenty-one is what the shelf stated when this was written,
    // and a parser that stops matching reports zero without complaining.
    assert_eq!(
        knobs.len(),
        21,
        "found {} shelf knobs, not the twenty-one the shelf states; the parser has \
         stopped matching the markup it is meant to read",
        knobs.len()
    );
}
