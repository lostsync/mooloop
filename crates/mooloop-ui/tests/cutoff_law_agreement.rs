//! The cutoff knob's law is written twice, once in Rust
//! (`mooloop_dsp::scale::cutoff_hz_from_normalized`, which every filtered
//! instrument maps its Cutoff through) and once in markup (`CutoffLaw.hz` in
//! `controls.slint`, which every face's cutoff readout and the filter-response
//! display read through). Until MOO-119 the markup spelled
//! `20 * pow(1000, x)` in six places and the engine mapped to
//! `0.45 * sample_rate`, so the readout said 3.56 kHz where 96 kHz played
//! 6.3 kHz. This holds the two copies together and the faces to the one
//! markup copy.
//!
//! A markup scan: it reads the `.slint` files, because what the readout does
//! is what those files say.

use mooloop_dsp::scale::{cutoff_hz_from_normalized, CUTOFF_CEILING_HZ, CUTOFF_FLOOR_HZ};

const UI_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/ui");

fn markup(file: &str) -> String {
    std::fs::read_to_string(format!("{UI_DIR}/{file}")).expect("unreadable .slint file")
}

/// The body of `CutoffLaw.hz`, as `(floor, span)` from its
/// `return <floor> * pow(<span>, clamp(normalized, 0, 1));`.
fn markup_law() -> (f32, f32) {
    let controls = markup("controls.slint");
    let global = controls
        .find("export global CutoffLaw {")
        .expect("controls.slint has no CutoffLaw global");
    let body = &controls[global..];
    let start = body.find("return ").expect("CutoffLaw.hz has no return") + "return ".len();
    let end = start + body[start..].find(';').expect("unterminated return");
    let expression = body[start..end].trim();
    let (floor, rest) = expression
        .split_once(" * pow(")
        .unwrap_or_else(|| panic!("CutoffLaw.hz is not `floor * pow(span, ...)`: {expression}"));
    let (span, argument) = rest.split_once(", ").expect("pow takes two arguments");
    assert_eq!(
        argument, "clamp(normalized, 0, 1))",
        "CutoffLaw.hz must clamp its knob the way the Rust law does"
    );
    (floor.parse().expect("floor"), span.parse().expect("span"))
}

#[test]
fn the_markup_cutoff_law_is_the_rust_one() {
    let (floor, span) = markup_law();
    assert_eq!(floor, CUTOFF_FLOOR_HZ);
    assert_eq!(floor * span, CUTOFF_CEILING_HZ);
    for step in 0..=20 {
        let knob = step as f32 / 20.0;
        let markup_hz = floor * span.powf(knob);
        let rust_hz = cutoff_hz_from_normalized(knob);
        assert!(
            (markup_hz / rust_hz - 1.0).abs() < 1.0e-4,
            "knob {knob}: the face reads {markup_hz} Hz, the engine runs {rust_hz} Hz"
        );
    }
}

/// Every face that reads a cutoff out in Hz goes through `CutoffLaw.hz`, and
/// none spells the formula itself.
#[test]
fn every_cutoff_readout_reads_the_one_law() {
    for file in [
        "mono-device.slint",
        "poly-device.slint",
        "sampler-device.slint",
        "mlm1-device.slint",
        "filter-device.slint",
        "device-displays.slint",
    ] {
        let text = markup(file);
        assert!(
            text.contains("CutoffLaw.hz("),
            "{file} does not read its cutoff through CutoffLaw.hz"
        );
        assert!(
            !text.contains("pow(1000"),
            "{file} spells the cutoff law itself instead of calling CutoffLaw.hz"
        );
    }
}

/// The filter-response display warps at the engine's rate, not at a
/// hard-coded 48 kHz.
#[test]
fn the_filter_display_draws_at_the_running_sample_rate() {
    let displays = markup("device-displays.slint");
    let start = displays
        .find("export component FilterResponseDisplay")
        .expect("no FilterResponseDisplay");
    let body = &displays[start..];
    let body = &body[..body[1..].find("\nexport ").map_or(body.len(), |end| end + 1)];
    assert!(!body.contains("48000"), "FilterResponseDisplay still assumes 48 kHz");
    assert!(body.contains("AudioFormat.sample-rate"));
}

/// The filter display draws the SVF's resonance with the engine's taper
/// (`svf_damping`, exponential since MOO-123), not the linear one it used to
/// copy.
#[test]
fn the_filter_display_uses_the_engines_resonance_taper() {
    use mooloop_dsp::filter::{svf_damping, SVF_MAX_DAMPING, SVF_MIN_DAMPING};
    let displays = markup("device-displays.slint");
    let line = displays
        .lines()
        .find(|line| line.trim_start().starts_with("let damping = "))
        .expect("the display computes no damping");
    let expression = line.trim().trim_start_matches("let damping = ").trim_end_matches(';');
    let (max, rest) = expression.split_once(" * pow(").expect("damping is `max * pow(ratio, r)`");
    let (ratio, argument) = rest.split_once(", ").expect("pow takes two arguments");
    assert_eq!(argument, "clamp(root.resonance, 0, 1))");
    let max: f32 = max.parse().expect("max");
    let ratio: f32 = ratio.parse().expect("ratio");
    assert_eq!(max, SVF_MAX_DAMPING);
    assert!((ratio - SVF_MIN_DAMPING / SVF_MAX_DAMPING).abs() < 1.0e-6);
    for step in 0..=10 {
        let resonance = step as f32 / 10.0;
        let markup = max * ratio.powf(resonance);
        assert!((markup / svf_damping(resonance) - 1.0).abs() < 1.0e-4, "resonance {resonance}");
    }
}
