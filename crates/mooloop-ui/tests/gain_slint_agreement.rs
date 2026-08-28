//! The fader taper must agree exactly across the Rust/Slint boundary: a
//! fader whose readout does not match its audio is the failure mode
//! docs/plans/gain-structure/03-a-shared-gain-module.md exists to prevent.
//! `mooloop-core/src/gain.rs` owns the breakpoints; `ui/gain.slint` mirrors
//! them, and this test fails loudly when the two lists diverge.

use mooloop_core::gain::FADER_BREAKPOINTS;

const GAIN_SLINT: &str = include_str!("../ui/gain.slint");

/// Extract one `property <[float]> name: [a, b, c];` list from the slint
/// source. Parsing text rather than evaluating Slint keeps the check
/// independent of any particular backend.
fn slint_float_list(name: &str) -> Vec<f32> {
    let marker = format!("{name}:");
    let line = GAIN_SLINT
        .lines()
        .find(|line| line.trim_start().starts_with("property") && line.contains(&marker))
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
    use mooloop_core::gain::{METER_HOT_DB, METER_WARNING_DB};

    for (name, expected) in [
        ("meter-warning-db", METER_WARNING_DB),
        ("meter-hot-db", METER_HOT_DB),
    ] {
        let marker = format!("{name}: ");
        let line = GAIN_SLINT
            .lines()
            .find(|line| line.contains(&marker))
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
