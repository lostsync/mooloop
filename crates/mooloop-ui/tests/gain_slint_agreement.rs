//! The fader taper must agree exactly across the Rust/Slint boundary: a
//! fader whose readout does not match its audio is the failure mode
//! docs/plans/archive/gain-structure/03-a-shared-gain-module.md exists to prevent.
//! `mooloop-core/src/gain.rs` owns the breakpoints; `ui/gain.slint` mirrors
//! them, and this test fails loudly when the two lists diverge.

use mooloop_core::gain::FADER_BREAKPOINTS;

const GAIN_SLINT: &str = include_str!("../ui/gain.slint");

/// Whether `line` is the *declaration* of property `name`, whatever
/// visibility qualifier it carries.
///
/// Anchored on the declaration rather than on the name alone, because a
/// comment or an expression that merely mentions a property is not where its
/// value lives. Both checks below share this, which is the point: they used
/// to parse the file two different ways. The taper check guarded on
/// `starts_with("property")` and the threshold check guarded on nothing, so
/// the latter took whichever line mentioned the name first and parsed after
/// its last colon -- a comment added above the property block would have
/// silently changed what it asserted, and it would still have passed.
///
/// The qualifier has to be stripped rather than ignored: the three meter
/// constants are `in property` and the two taper lists are bare `property`,
/// so guarding on `starts_with("property")` alone finds neither set of the
/// other's kind.
fn declares(line: &str, name: &str) -> bool {
    let trimmed = line.trim_start();
    let unqualified = trimmed
        .strip_prefix("in-out ")
        .or_else(|| trimmed.strip_prefix("in "))
        .or_else(|| trimmed.strip_prefix("out "))
        .unwrap_or(trimmed);
    unqualified.starts_with("property") && line.contains(&format!("{name}:"))
}

/// Extract one `property <[float]> name: [a, b, c];` list from the slint
/// source. Parsing text rather than evaluating Slint keeps the check
/// independent of any particular backend.
fn slint_float_list(name: &str) -> Vec<f32> {
    let marker = format!("{name}:");
    let line = GAIN_SLINT
        .lines()
        .find(|line| declares(line, name))
        .unwrap_or_else(|| panic!("gain.slint no longer declares {name}"));
    let marker_at = line.find(&marker).unwrap_or(0) + marker.len();
    let start = line[marker_at..]
        .find('[')
        .map(|offset| marker_at + offset)
        .unwrap_or_else(|| panic!("{name} is not a bracketed list"));
    let end = line[start..]
        .find(']')
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("{name} list is unterminated"));
    line[start + 1..end]
        .split(',')
        .map(|value| {
            value
                .trim()
                .parse::<f32>()
                .unwrap_or_else(|error| panic!("{name} holds a non-number {value:?}: {error}"))
        })
        .collect()
}

/// The guard above, tested directly, because what it prevents is this file
/// passing while checking nothing.
///
/// The threshold check used to match any line mentioning the name. Most
/// accidents that causes are loud -- a comment parses to garbage and panics
/// on "not a plain float literal", which sends you to the wrong file. The one
/// that matters is quiet: a comment recording the value parses cleanly, so
/// the check reads the comment, agrees with `gain.rs`, and goes green while
/// the real property drifts underneath it unchecked.
#[test]
fn a_comment_is_not_a_declaration() {
    assert!(
        declares("    in property <float> meter-warning-db: -10.0;", "meter-warning-db"),
        "a qualified declaration is still a declaration"
    );
    assert!(
        declares("    property <[float]> fader-db: [6.0, 0.0];", "fader-db"),
        "an unqualified declaration is still a declaration"
    );
    assert!(
        !declares("    // meter-warning-db: -10.0 (matches gain.rs)", "meter-warning-db"),
        "a comment that records the value must not be mistaken for it"
    );
    assert!(
        !declares("        warning-db: root.meter-warning-db;", "meter-warning-db"),
        "a binding that reads the property is not where it is declared"
    );
}

#[test]
fn slint_fader_taper_matches_the_rust_breakpoints() {
    let travel = slint_float_list("fader-travel");
    let db = slint_float_list("fader-db");
    assert_eq!(
        travel.len(),
        FADER_BREAKPOINTS.len(),
        "gain.slint and gain.rs disagree on breakpoint count"
    );
    assert_eq!(travel.len(), db.len(), "gain.slint taper lists diverged");
    for (index, (rust_travel, rust_db)) in FADER_BREAKPOINTS.iter().enumerate() {
        assert!(
            (travel[index] - rust_travel).abs() < 1e-6,
            "breakpoint {index} travel: slint {} vs rust {rust_travel}",
            travel[index]
        );
        // Slint spells -inf as -99999.0 (no infinity literal in Slint).
        let slint_db = if db[index] <= -99998.0 {
            f32::NEG_INFINITY
        } else {
            db[index]
        };
        let agrees = (slint_db.is_infinite() && slint_db == *rust_db)
            || (slint_db - rust_db).abs() < 1e-4;
        assert!(
            agrees,
            "breakpoint {index} dB: slint {} vs rust {rust_db}",
            db[index]
        );
    }
}

#[test]
fn trim_knob_reads_through_the_shared_formatter() {
    // One formatter, not two: TrimKnob's value-text must come from
    // GainMath.format-db rather than an inline copy.
    let controls = include_str!("../ui/controls.slint");
    let knob = controls
        .lines()
        .skip_while(|line| !line.contains("export component TrimKnob"))
        .take(8)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        knob.contains("GainMath.format-db"),
        "TrimKnob stopped reading its value-text from GainMath:\n{knob}"
    );
    assert!(
        !knob.contains("round("),
        "TrimKnob grew its own formatting again:\n{knob}"
    );
}

#[test]
fn slint_meter_thresholds_match_the_rust_constants() {
    use mooloop_core::gain::{METER_HOT_DB, METER_WARNING_DB, MIN_DB, REFERENCE_PEAK_DBFS};

    for (name, expected) in [
        ("meter-warning-db", METER_WARNING_DB),
        ("meter-hot-db", METER_HOT_DB),
        ("reference-peak-dbfs", REFERENCE_PEAK_DBFS),
        ("min-db", MIN_DB),
    ] {
        let line = GAIN_SLINT
            .lines()
            .find(|line| declares(line, name))
            .unwrap_or_else(|| panic!("gain.slint no longer declares {name}"));
        let value: f32 = line
            .rsplit(':')
            .next()
            .and_then(|rest| rest.trim().trim_end_matches(';').parse().ok())
            .unwrap_or_else(|| panic!("{name} is not a plain float literal"));
        assert!(
            (value - expected).abs() < 1e-4,
            "{name}: slint {value} vs rust {expected}"
        );
    }
}

/// The floor is checked above, which is only half of it: a property nothing
/// reads is a list nothing reads, and that is exactly the fault the taper
/// check was written to stop making.
///
/// `GainMath.min-db` was added on 2026-09-13 to replace fifty literal `-60`s
/// across ten files -- three inside `gain.slint`'s own converters and
/// forty-seven across nine others, as `minimum-db` scale bottoms and as the
/// resting value of every meter reading. This asserts the number did not come
/// back: a `<float>` property whose name ends `-db` and whose whole binding is
/// the literal `-60` is a copy of the floor that the check above cannot see.
///
/// **It sweeps every face, not the two files the floor was first found in.**
/// `LOOSE_ENDS.md` named `meters.slint` and `controls.slint` and counted
/// twenty-two; `device-rack.slint`, `bus-device.slint`, `main.slint`,
/// `gate-device.slint`, `limiter-device.slint`, `compressor-device.slint` and
/// `device-drag-harness.slint` held twenty-six more. The first version of this
/// check swept only the two files the note named, and would have passed while
/// most of the copies were still there.
#[test]
fn no_face_spells_the_floor_for_itself() {
    let converters = GAIN_SLINT
        .lines()
        .filter(|line| line.contains("return") && line.contains("min-db"))
        .count();
    assert_eq!(
        converters, 2,
        "db-to-linear and linear-to-db stopped reading GainMath.min-db"
    );

    // The two the sweep deliberately leaves alone, both for the same reason:
    // something already reads them, and reading them is what it does.
    //
    // `device-displays.slint`'s `threshold-min-db` and `floor-db` are held to
    // `METER_FLOOR_DB` by `strip_face.rs`, which finds them by parsing the
    // literal out of the declaration -- so replacing the literal with a
    // property reference would take that guard off rather than improve it.
    //
    // A binding that is an *expression* rather than a literal is not a copy of
    // the floor at all. `compressor-device.slint`'s
    // `threshold-db: -60 + root.threshold * 60` is the compressor's threshold
    // range being denormalized, which mirrors the descriptor table and is
    // checked as a range by `every_effect_linear_readout_agrees_with_its_table`
    // in `slint_face_agreement.rs`. Requiring the whole binding to be `-60;`
    // excludes it without naming it.
    const READ_BY_STRIP_FACE: &str = "device-displays.slint";

    let mut swept = 0usize;
    for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/ui"))
        .expect("mooloop-ui/ui is unreadable")
    {
        let path = entry.expect("unreadable directory entry").path();
        if path.extension().is_none_or(|kind| kind != "slint") {
            continue;
        }
        let file = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        if file == READ_BY_STRIP_FACE {
            continue;
        }
        swept += 1;
        let source = std::fs::read_to_string(&path).expect("unreadable .slint file");
        for (number, line) in source.lines().enumerate() {
            let Some((_, rest)) = line.split_once("property <float> ") else {
                continue;
            };
            let Some((name, binding)) = rest.split_once(':') else {
                continue;
            };
            assert!(
                !(name.trim().ends_with("-db") && binding.trim() == "-60;"),
                "{file}:{} spells the floor instead of reading GainMath.min-db: {line}",
                number + 1
            );
        }
    }

    // The sweep reads the directory, so a rename cannot quietly empty it.
    assert!(
        swept > 20,
        "only {swept} .slint files were swept; the walk has stopped finding them"
    );
}

/// **No meter in the application states a segment count.**
///
/// This replaces `slint_meter_segment_counts_match_the_throttle`, which held
/// `mixer.slint`'s and `device-rack.slint`'s counts against two Rust
/// constants the repaint throttle quantized by. Meters became continuous bars
/// on 2026-09-15, the constants went, and the old test would have passed
/// forever by finding nothing on either side -- `left: [], right: []` -- which
/// is the shape of a guard that has stopped guarding.
///
/// So this checks what is now true instead. A `segments:` literal in an
/// application face means that meter went back to LEDs, and the throttle --
/// which now steps by a quarter of a decibel, finer than a pixel -- would be
/// repainting a meter far more often than it can change. `mockup-catalog.slint`
/// is the one file allowed to state a count, because the LED form is a widget
/// it exists to display.
#[test]
fn no_application_meter_states_a_segment_count() {
    let mut swept = 0;
    let mut stating = Vec::new();
    for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/ui")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("slint") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name == "mockup-catalog.slint" {
            continue;
        }
        swept += 1;
        for (number, line) in std::fs::read_to_string(&path).unwrap().lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let Some((_, rest)) = line.split_once("segments:") else {
                continue;
            };
            let stated = rest.trim().trim_end_matches(';').trim();
            // Only a literal is a count. `segments: root.segments;` is a
            // wrapper handing its own property down, which is the markup
            // reading the number from somewhere rather than stating it --
            // the same distinction the test this replaced drew, and for the
            // same reason. `segments: 0` is a component declaring the
            // continuous default.
            if stated.starts_with(|c: char| c.is_ascii_digit()) && stated != "0" {
                stating.push(format!("{name}:{}: {}", number + 1, line.trim()));
            }
        }
    }
    assert!(
        stating.is_empty(),
        "these faces ask for LED segments, which the repaint throttle no \
         longer matches: {stating:#?}"
    );
    // The sweep reads the directory, so a rename cannot quietly empty it --
    // the failure the test this replaced went out on.
    assert!(
        swept > 20,
        "only {swept} .slint files were swept; the walk has stopped finding them"
    );
}

/// **No readout rounds a dB for itself.** `GainMath` owns the two dB formats
/// -- `format-db` for a gain, `format-plain-db` for a setting that merely
/// reads in dB -- and a face that spells `round(x * 10) / 10` is neither.
///
/// The failure is width, not accuracy. Slint's number-to-string drops a
/// trailing zero, so `round(x * 10) / 10` renders 6.0 as "6": a limiter
/// ceiling read "-0.3 dB" at one knob position and "-1 dB" at the next, and
/// the field changed width under the pointer. That is the same jitter
/// `format-db` was written to take out of the faders, left behind on ten
/// readouts because the note that recorded it counted six and looked only at
/// the dynamics trio.
///
/// The two it missed are the interesting ones. `preamp-device.slint`'s Drive
/// and Output are -24..24 dB *gains* that had simply never been given
/// `format-db`, and ML-P8's two level fields hand-rolled their own `-inf`
/// floor beside it -- so "every readout that is a gain uses the shared
/// formatter" was false while nothing could say so.
///
/// A literal-text search, like `no_face_spells_the_floor_for_itself` above:
/// the rendered string cannot be read back without `ElementHandle`, and the
/// defect and its fix are both one line of markup.
#[test]
fn no_db_readout_rounds_for_itself() {
    let mut swept = 0usize;
    let mut readouts = 0usize;
    for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/ui"))
        .expect("mooloop-ui/ui is unreadable")
    {
        let path = entry.expect("unreadable directory entry").path();
        if path.extension().is_none_or(|kind| kind != "slint") {
            continue;
        }
        swept += 1;
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        let source = std::fs::read_to_string(&path).expect("unreadable .slint file");
        for (number, line) in source.lines().enumerate() {
            // A dB readout is a line that puts the unit into a string. The
            // comment above `format-plain-db` says `" dB"` too, so the line
            // also has to be building a value rather than describing one.
            if !line.contains("\" dB\"") || line.trim_start().starts_with("//") {
                continue;
            }
            readouts += 1;
            assert!(
                !line.contains("round("),
                "{file}:{} rounds a dB for itself instead of calling \
                 GainMath.format-db (for a gain) or GainMath.format-plain-db \
                 (for a setting that reads in dB): {}",
                number + 1,
                line.trim()
            );
        }
    }
    assert!(
        swept > 20,
        "only {swept} .slint files were swept; the walk has stopped finding them"
    );
    assert!(
        readouts >= 6,
        "only {readouts} dB readouts were found; the scan used to see the \
         dynamics trio, the preamp, ML-P8 and the gain-reduction badge, so \
         either the unit moved out of the markup or this test stopped looking"
    );
}
