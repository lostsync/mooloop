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
    use mooloop_core::gain::{METER_HOT_DB, METER_WARNING_DB, REFERENCE_PEAK_DBFS};

    for (name, expected) in [
        ("meter-warning-db", METER_WARNING_DB),
        ("meter-hot-db", METER_HOT_DB),
        ("reference-peak-dbfs", REFERENCE_PEAK_DBFS),
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
