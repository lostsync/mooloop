//! The channel strip's faces, against the table the engine reads.
//!
//! `slint_face_agreement.rs` holds a device's face to its descriptors by
//! *parsing* the markup for the ranges it mirrors. The strip's faces mirror
//! nothing: every range, default, curve and parameter id reaches them through
//! the `StripSpec` global, which `install_strip_spec` fills from
//! `mooloop_core::strip`. So the agreement test for this feature is a
//! different shape, and a stronger one -- it reads the table back out of the
//! window and holds it to the descriptors, and then checks that no control in
//! the markup declares a bound of its own.
//!
//! Two numbers in `strip.slint` are not covered by that and should not be:
//! the response plot's axes, which `StripEqPage` inverts to turn a dragged
//! point back into hertz and decibels. Those are `EqResponseDisplay`'s
//! convention, spelled the same way in `eq-device.slint`. The last two tests
//! here are for the couplings that leaves -- the gain axis against the range
//! it coincides with, and the floor the compressor's curve is sampled over
//! against the floor the plot indexes it by.

use mooloop_core::gain::MIN_DB as METER_FLOOR_DB;
use mooloop_core::strip::{
    strip_band_param, StripParams, STRIP_BAND_FREQ, STRIP_BAND_GAIN, STRIP_BAND_KIND, STRIP_BAND_Q,
    STRIP_BAND_STRIDE, STRIP_COMP_ATTACK_MS, STRIP_COMP_IN, STRIP_COMP_KNEE_DB, STRIP_COMP_MAKEUP_DB, STRIP_COMP_MIX, STRIP_COMP_RATIO,
    STRIP_COMP_RELEASE_MS, STRIP_COMP_THRESHOLD_DB, STRIP_DRIVE_DB, STRIP_EQ_BANDS, STRIP_EQ_IN,
    STRIP_FIRST, STRIP_PRE_IN, STRIP_VOICING,
};
use mooloop_core::{ParamCurve, PreampVoicing};
use mooloop_ui::{install_strip_spec, MainWindow, StripSpec};
use slint::{ComponentHandle, Model};

mod common;

const STRIP_SLINT: &str = include_str!("../ui/strip.slint");
const DISPLAYS_SLINT: &str = include_str!("../ui/device-displays.slint");

fn headless() -> MainWindow {
    common::install_testing_backend();
    MainWindow::new().unwrap()
}

/// **The agreement test.** Every id the markup addresses is the id the engine
/// reads, and every range behind a knob is the descriptor's.
///
/// This is what `install_strip_spec` is for, and the failure it prevents is
/// the one `ParamDescriptor` is documented against: a knob and an automation
/// lane are two views of one value, and a range written a second time is how
/// they come to disagree.
#[test]
fn the_faces_table_is_the_engines_table() {
    let ui = headless();
    install_strip_spec(&ui);
    let spec = ui.global::<StripSpec>();

    assert_eq!(spec.get_first(), STRIP_FIRST as i32);
    for (property, id) in [
        (spec.get_voicing(), STRIP_VOICING),
        (spec.get_pre_in(), STRIP_PRE_IN),
        (spec.get_drive_db(), STRIP_DRIVE_DB),
        (spec.get_eq_in(), STRIP_EQ_IN),
        (spec.get_comp_in(), STRIP_COMP_IN),
        (spec.get_threshold_db(), STRIP_COMP_THRESHOLD_DB),
        (spec.get_ratio(), STRIP_COMP_RATIO),
        (spec.get_attack_ms(), STRIP_COMP_ATTACK_MS),
        (spec.get_release_ms(), STRIP_COMP_RELEASE_MS),
        (spec.get_knee_db(), STRIP_COMP_KNEE_DB),
        (spec.get_mix(), STRIP_COMP_MIX),
        (spec.get_makeup_db(), STRIP_COMP_MAKEUP_DB),
    ] {
        assert_eq!(property, id as i32, "the face addresses a different id");
    }

    // The band arithmetic the markup does -- base plus band times stride
    // plus field -- has to land on the ids `strip_band_param` mints.
    for band in 0..STRIP_EQ_BANDS {
        for field in [
            STRIP_BAND_FREQ,
            STRIP_BAND_GAIN,
            STRIP_BAND_Q,
            STRIP_BAND_KIND,
        ] {
            let from_markup = spec.get_band_base()
                + band as i32 * spec.get_band_stride()
                + match field {
                    STRIP_BAND_FREQ => spec.get_band_frequency(),
                    STRIP_BAND_GAIN => spec.get_band_gain(),
                    STRIP_BAND_Q => spec.get_band_q(),
                    _ => spec.get_band_kind(),
                };
            assert_eq!(
                from_markup,
                strip_band_param(band, field) as i32,
                "band {band} field {field}"
            );
        }
    }
    assert_eq!(spec.get_band_stride(), STRIP_BAND_STRIDE as i32);

    let params = spec.get_params();
    assert_eq!(
        params.row_count(),
        StripParams::descriptors().len(),
        "the face was handed a different number of parameters than the strip has"
    );
    for (index, descriptor) in StripParams::descriptors().iter().enumerate() {
        let row = params.row_data(index).unwrap();
        assert_eq!(row.id, descriptor.id as i32, "{}", descriptor.name);
        assert_eq!(row.name.as_str(), descriptor.name);
        assert_eq!(row.unit.as_str(), descriptor.unit);
        assert_eq!(row.minimum, descriptor.min, "{}", descriptor.name);
        assert_eq!(row.maximum, descriptor.max, "{}", descriptor.name);
        assert_eq!(row.default_value, descriptor.default, "{}", descriptor.name);
        assert_eq!(
            row.logarithmic,
            matches!(descriptor.curve, ParamCurve::Exponential),
            "{}",
            descriptor.name
        );
    }

    // The voicing names are the engine's, not a second spelling of them.
    let voicings = spec.get_voicings();
    assert_eq!(voicings.row_count(), 4);
    for (index, voicing) in [
        PreampVoicing::Moo,
        PreampVoicing::Grip,
        PreampVoicing::Punch,
        PreampVoicing::Iron,
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(voicings.row_data(index).unwrap().as_str(), voicing.label());
    }
    assert_eq!(spec.get_band_names().row_count(), STRIP_EQ_BANDS);
}

/// And no control in the markup declares a bound of its own.
///
/// A face that mirrors a range can be checked for drift; a face with no range
/// cannot drift at all, and this is the test that keeps it that way. It fails
/// the moment somebody adds `minimum: -24` to a strip knob "to make it
/// clearer", which is exactly how the second copy of a range gets written.
///
/// Scanned by *statement* rather than by line. The first version matched a
/// trimmed line beginning with `minimum:` and ending in a semicolon, which is
/// one way of several to write the thing it is looking for: `StripKnob {
/// minimum: -24; }` on one line trims to something starting with `StripKnob`
/// and would have walked straight past. Splitting on the punctuation that
/// ends a Slint statement sees it wherever it sits.
#[test]
fn the_strips_markup_declares_no_range_of_its_own() {
    // Comments first, because the prose above `StripSpec` discusses minima
    // and maxima at length and a scanner should not be reading the argument
    // for the rule as a breach of it.
    let mut code: String = STRIP_SLINT
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    // `StripParamSpec`'s own field list names these three; naming the fields
    // is how the table is declared, not a second copy of a range.
    let declaration = code
        .find("struct StripParamSpec")
        .and_then(|start| code[start..].find('}').map(|end| start..start + end + 1))
        .expect("strip.slint still declares StripParamSpec");
    code.replace_range(declaration, "");

    let bindings: Vec<&str> = code
        .split(['{', '}', ';'])
        .map(str::trim)
        .filter(|statement| {
            ["minimum:", "maximum:", "default-value:"]
                .iter()
                .any(|name| statement.starts_with(name))
                // `StripKnob` is the one place bounds are bound, and it binds
                // them to the descriptor. Everything else must go through it.
                && !statement.contains("root.spec.")
        })
        .collect();
    // The pan knob on the paned back face is the mixer's own control rather
    // than a strip parameter, and it lives in `mixer.slint`; nothing in this
    // file may declare a bound.
    assert!(
        bindings.is_empty(),
        "strip.slint declares its own bounds, which the descriptor table already states: {bindings:?}"
    );
}

/// **The curve Rust sampled and the axis the plot indexes it against have to
/// share a floor.**
///
/// `strip_row` samples the compressor's static curve over
/// `METER_FLOOR_DB..0`; `DynamicsCurveDisplay::curve-out` turns an input
/// level back into an index with `(input-db - floor-db) / -floor-db`, where
/// `floor-db` is the display's own. The two are the same number written in
/// two languages, and the markup's copy is `private`, so it cannot even be
/// handed the Rust one. Move either and the curve is not wrong at the edges,
/// it is stretched: every point of it lands at the wrong input level, with
/// the threshold handle still drawn where the threshold really is.
///
/// The strip is the only caller that supplies `curve-db` at all -- every
/// other dynamics face lets the display compute its own curve, where the
/// floor is only an axis -- so this coupling exists for this feature and is
/// asserted with it. Parsed rather than read off the window, the way
/// `gain_slint_agreement.rs` parses the fader taper, and anchored on the
/// declaration so that a comment or an expression mentioning the name cannot
/// become what is being checked.
#[test]
fn the_sampled_curve_and_the_plot_share_a_floor() {
    let declaration = DISPLAYS_SLINT
        .lines()
        .map(str::trim)
        .find(|line| line.contains("property <float> floor-db:"))
        .expect("DynamicsCurveDisplay still declares a floor-db");
    let value: f32 = declaration
        .rsplit_once(':')
        .and_then(|(_, rest)| rest.trim().trim_end_matches(';').trim().parse().ok())
        .unwrap_or_else(|| panic!("could not read a number out of `{declaration}`"));
    assert_eq!(
        value, METER_FLOOR_DB,
        "`{declaration}` in device-displays.slint against METER_FLOOR_DB in          mooloop-core: `strip_row` samples the strip's compressor curve from          the second and the display indexes it with the first"
    );
}

/// **The one pair of numbers `strip.slint` does spell, and what holds them
/// to the table.**
///
/// `EqResponseDisplay` reports a dragged point normalized over *its own*
/// axes, so `StripEqPage`'s `point-dragged` inverts them -- `gain * 36 - 18`,
/// character for character what `eq-device.slint` writes for the seven-band
/// EQ, and the inverse of what `strip_row` does on the way out. That is the
/// display's convention rather than a parameter range, which is why the test
/// above does not and should not flag it.
///
/// But it *coincides* with a parameter range, and the coincidence is
/// load-bearing: widen a band's gain to +-24 in the descriptor and dragging a
/// point to the top of the plot still writes +18, while typing 24 into the
/// knob draws a curve off the top of it. Nothing about that is visible at
/// either end, so it is asserted here -- where the number is already spelled
/// twice, a third spelling that *fails* is the cheap one.
#[test]
fn the_response_plots_gain_axis_is_the_bands_gain_range() {
    for band in 0..STRIP_EQ_BANDS {
        let gain = StripParams::descriptor(strip_band_param(band, STRIP_BAND_GAIN))
            .expect("every band has a gain descriptor");
        assert_eq!(
            (gain.min, gain.max),
            (-18.0, 18.0),
            "band {band}'s gain range left the response plot's axis behind: \
             `strip.slint` and `eq-device.slint` both invert a dragged point \
             as `gain * 36 - 18`, and `strip_row` normalizes as \
             `(gain_db + 18) / 36`"
        );
    }
}

/// Every parameter the strip has is addressed by the markup, and the id space
/// the markup can reach is exactly the one the table describes.
///
/// The arithmetic is the point: thirteen scalar ids plus four fields on each
/// of four bands is twenty-nine, which is how many descriptors there are. A
/// parameter added to the table and not to a face would break this, which is
/// the failure a rendered test cannot see -- a control nobody drew looks like
/// nothing at all.
#[test]
fn every_strip_parameter_is_addressed_by_a_face() {
    let scalars = [
        "StripSpec.voicing",
        "StripSpec.pre-in",
        "StripSpec.drive-db",
        "StripSpec.eq-in",
        "StripSpec.comp-in",
        "StripSpec.threshold-db",
        "StripSpec.ratio",
        "StripSpec.attack-ms",
        "StripSpec.release-ms",
        "StripSpec.knee-db",
        "StripSpec.mix",
        "StripSpec.makeup-db",
    ];
    let bands = [
        "StripSpec.band-frequency",
        "StripSpec.band-gain",
        "StripSpec.band-q",
        "StripSpec.band-kind",
    ];
    for name in scalars.iter().chain(bands.iter()) {
        assert!(
            STRIP_SLINT.contains(name),
            "{name} is in the table and no face addresses it"
        );
    }
    assert_eq!(
        scalars.len() + bands.len() * STRIP_EQ_BANDS,
        StripParams::descriptors().len(),
        "the table and the faces disagree about how many parameters a strip has"
    );
}

/// The reading order the EQ is laid out in is Adam's ruling, and it is the
/// one thing about that layout a reader cannot infer: four stacked rows look
/// like they could be in any order. It is stated in the markup, and this is
/// what keeps the statement there.
#[test]
fn the_eq_says_which_way_it_reads() {
    assert!(
        STRIP_SLINT.contains("left to right and top to bottom"),
        "strip.slint no longer says which order its four bands are in"
    );
}
