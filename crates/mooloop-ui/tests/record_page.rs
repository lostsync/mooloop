//! The RECORD page's phase, held together across the Rust/Slint boundary.
//!
//! MOO-54. `RecordFace` (`mooloop-core/src/input.rs`) is the truth: it names
//! the numbers `publish_take` writes into the window's `sampler-record-state`
//! and the label the RECORD button carries in each one. Slint cannot import a
//! Rust enum, so `sampler-device.slint` decodes those numbers with a ternary
//! ladder -- a rule spelled twice, which is this codebase's characteristic
//! fault (`AGENTS.md`, "Duplication"). This test is the thing that notices
//! when the copies drift.
//!
//! **Not an addition to `slint_face_agreement.rs`.** That file's stated
//! contract is a *device's descriptor table* against its *face's knob
//! declarations*, parsed by its `knob_in`/`face_knobs` helpers over
//! `ParameterKnob` blocks. The record page has no descriptor and no knob, so a
//! check of it would sit outside every helper there -- and that file's list of
//! covered faces is the input to `scripts/dupe-audit unchecked-face`, which
//! would then be describing a file it no longer describes. The *technique* is
//! reused (`include_str!` plus line scanning), not the file.
//!
//! It reads the markup and the enum only, so it needs no Slint backend and
//! runs anywhere.
//!
//! # Why a pairwise assertion
//!
//! A set-equality check -- `{0,1,2}` against the declared discriminants,
//! `{"REC","WAIT","STOP"}` against the labels -- passes unchanged when two
//! labels are swapped, which is the exact drift the page is exposed to. So
//! [`the_record_button_labels_match_the_rust_table`] parses the ladder into
//! ordered `(number, label)` pairs and asks `RecordFace` about each one.
//!
//! # Mutations run against the tree, before the fix was called done
//!
//! `AGENTS.md`'s `bar-arithmetic` lesson is that a check written after its fix
//! is shaped to report what its author already knows about. The collector half
//! of this file (the `.slint` parsing) was written and run first, against
//! `44fa4d5` before any Rust changed; it printed ten `record-state`
//! comparisons on nine lines, and found two spellings the source report does
//! not name -- the ladder runs to `:1144`, not `:1139`, and `main.slint` holds
//! a second `64` default beside `sampler-device.slint`'s. Then each assertion
//! was proved to bite, by mutating the tree and reverting:
//!
//! | Mutation | Must fail |
//! | --- | --- |
//! | Swap `"STOP"` and `"REC"` in the `:1086` ladder | `the_record_button_labels_match_the_rust_table` |
//! | `record-state == 1` becomes `== 3` at `:1083` | `every_record_state_compared_in_the_markup_is_a_declared_face` |
//! | Delete the `== 1` arms from the ladder and the page | `every_record_face_is_decoded_somewhere_in_the_markup` |
//! | Restore `record-max-bars: 64` at `:293` | `the_record_length_maximum_has_no_second_spelling_in_the_markup` |
//! | Restore `sampler-record-max-bars: 64` in `main.slint` | the same test, on its own |
//!
//! The last two were run separately, because dropping only one of the two
//! `64`s leaves the live spelling in place -- `main.slint` binds
//! `record-max-bars: root.sampler-record-max-bars`, so the face's own default
//! never reaches the real window, and Rust overwrites the window's on every
//! editor refresh (`lib.rs`, `set_sampler_record_max_bars`). Both literals
//! were dead; their only possible effect was to be wrong later.

use mooloop_core::RecordFace;

const SAMPLER_FACE: &str = include_str!("../ui/sampler-device.slint");
const MAIN_WINDOW: &str = include_str!("../ui/main.slint");

/// Every `record-state == N` in `src`, as `(line number, N)`.
fn record_state_comparisons(src: &str) -> Vec<(usize, i32)> {
    let mut found = Vec::new();
    for (index, line) in src.lines().enumerate() {
        let mut rest = line;
        while let Some(at) = rest.find("record-state ==") {
            rest = &rest[at + "record-state ==".len()..];
            let digits: String = rest
                .trim_start()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if let Ok(value) = digits.parse::<i32>() {
                found.push((index + 1, value));
            }
        }
    }
    found
}

/// The RECORD button's label ladder, as the markup spells it.
struct LabelLadder {
    /// Where it is, for a failure that has to be found and edited.
    line: usize,
    /// The `(number, label)` arms, in the order the ternary tests them.
    arms: Vec<(i32, String)>,
    /// The label drawn when no arm matched.
    otherwise: String,
}

/// The RECORD button's label ladder: the one single-line `text:` binding on a
/// `record-state` ternary whose every branch is a string literal.
///
/// The page holds three other `record-state` ternaries and none of them is
/// this one: two are spread over several lines, and all three have at least
/// one branch that is a property rather than a literal.
fn button_label_ladder(src: &str) -> Option<LabelLadder> {
    for (index, line) in src.lines().enumerate() {
        let trimmed = line.trim();
        let Some(body) = trimmed.strip_prefix("text:") else {
            continue;
        };
        if !body.contains("record-state ==") {
            continue;
        }
        // A ladder split over several lines is one of the others.
        let Some(body) = body.trim().strip_suffix(';') else {
            continue;
        };

        let mut arms = Vec::new();
        let mut rest = body.trim();
        let parsed = loop {
            let Some((condition, tail)) = rest.split_once('?') else {
                break false;
            };
            let Some(number) = condition.trim().strip_prefix("root.record-state ==") else {
                break false;
            };
            let Ok(number) = number.trim().parse::<i32>() else {
                break false;
            };
            let Some((value, tail)) = tail.split_once(':') else {
                break false;
            };
            let Some(label) = string_literal(value) else {
                break false;
            };
            arms.push((number, label));
            rest = tail.trim();
            if !rest.contains('?') {
                break true;
            }
        };
        if !parsed {
            continue;
        }
        let Some(otherwise) = string_literal(rest) else {
            continue;
        };
        return Some(LabelLadder {
            line: index + 1,
            arms,
            otherwise,
        });
    }
    None
}

/// `text` as the string literal it is, if it is one and nothing else.
fn string_literal(text: &str) -> Option<String> {
    let text = text.trim();
    let inner = text.strip_prefix('"')?.strip_suffix('"')?;
    (!inner.contains('"')).then(|| inner.to_string())
}

/// Declarations of a property named `name` that carry a literal initialiser,
/// as `(line number, the literal)`.
fn literal_defaults(src: &str, name: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for (index, line) in src.lines().enumerate() {
        let trimmed = line.trim();
        if !trimmed.starts_with("in property") && !trimmed.starts_with("in-out property") {
            continue;
        }
        let Some((_, declaration)) = trimmed.split_once('>') else {
            continue;
        };
        let Some((declared, initialiser)) = declaration.trim().split_once(':') else {
            continue;
        };
        if declared.trim() != name {
            continue;
        }
        found.push((
            index + 1,
            initialiser.trim().trim_end_matches(';').trim().to_string(),
        ));
    }
    found
}

/// The ladder's numbers and labels agree with `RecordFace`, pair by pair.
///
/// Ordered and pairwise on purpose: swapping two labels leaves every set in
/// this file unchanged.
#[test]
fn the_record_button_labels_match_the_rust_table() {
    let LabelLadder {
        line,
        arms,
        otherwise,
    } = button_label_ladder(SAMPLER_FACE)
        .expect("sampler-device.slint has a single-line RECORD button label ladder");

    assert!(
        !arms.is_empty(),
        "sampler-device.slint:{line}: the label ladder tests no record-state value"
    );

    for (number, label) in &arms {
        let face = RecordFace::from_i32(*number).unwrap_or_else(|| {
            panic!(
                "sampler-device.slint:{line}: the label ladder decodes record-state == {number}, \
                 which is not a RecordFace discriminant"
            )
        });
        assert_eq!(
            face.label(),
            label,
            "sampler-device.slint:{line}: record-state == {number} is {face:?}, whose label is \
             {:?}, but the markup draws {label:?}",
            face.label()
        );
    }

    assert_eq!(
        RecordFace::Idle.label(),
        otherwise,
        "sampler-device.slint:{line}: the ladder's else branch draws {otherwise:?}, but the \
         state no arm claims is {:?}, whose label is {:?}",
        RecordFace::Idle,
        RecordFace::Idle.label()
    );
}

/// Nothing in the markup decodes a number `RecordFace` does not declare.
///
/// Adding a `== 3` arm to the page fails here until the enum grows one.
#[test]
fn every_record_state_compared_in_the_markup_is_a_declared_face() {
    let comparisons = record_state_comparisons(SAMPLER_FACE);
    assert!(
        !comparisons.is_empty(),
        "sampler-device.slint no longer decodes record-state at all -- if the page moved, this \
         test has to move with it"
    );

    for (line, value) in comparisons {
        assert!(
            RecordFace::from_i32(value).is_some(),
            "sampler-device.slint:{line}: record-state == {value}, which is not a RecordFace \
             discriminant. Add the variant to mooloop-core, or fix the markup."
        );
    }
}

/// Every state the enum declares is decoded by the markup.
///
/// A variant the page cannot draw is a state the user can reach and not see;
/// deleting an arm fails here.
#[test]
fn every_record_face_is_decoded_somewhere_in_the_markup() {
    let compared: Vec<i32> = record_state_comparisons(SAMPLER_FACE)
        .into_iter()
        .map(|(_, value)| value)
        .collect();
    let ladder_else = button_label_ladder(SAMPLER_FACE).map(|ladder| ladder.otherwise);

    for face in RecordFace::ALL {
        let compared_directly = compared.contains(&face.as_i32());
        let drawn_by_else = ladder_else.as_deref() == Some(face.label());
        assert!(
            compared_directly || drawn_by_else,
            "sampler-device.slint decodes no {face:?}: nothing compares record-state == {} and \
             the label ladder's else branch is not {:?}",
            face.as_i32(),
            face.label()
        );
    }
}

/// The longest clip is `MAX_RECORD_BARS` and the markup holds no second copy.
///
/// Both spellings were dead -- Rust writes the window's value on every editor
/// refresh and the window's binding overwrites the face's -- so each was a
/// literal whose only possible effect was to disagree with core later.
#[test]
fn the_record_length_maximum_has_no_second_spelling_in_the_markup() {
    for (file, src, property) in [
        (
            "sampler-device.slint",
            SAMPLER_FACE,
            "record-max-bars",
        ),
        ("main.slint", MAIN_WINDOW, "sampler-record-max-bars"),
    ] {
        for (line, literal) in literal_defaults(src, property) {
            assert_eq!(
                literal, "0",
                "{file}:{line}: `{property}` declares a default of {literal}, a second spelling \
                 of mooloop_core::MAX_RECORD_BARS ({}). Rust supplies this value; leave the \
                 declaration bare.",
                mooloop_core::MAX_RECORD_BARS
            );
        }
    }
}
