//! A shelf knob and the caption under it are one value, so they must read the
//! same.
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
