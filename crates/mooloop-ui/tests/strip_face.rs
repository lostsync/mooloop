//! The channel strip's faces, against the table the engine reads.
//!
//! `slint_face_agreement.rs` holds a device's face to its descriptors by
//! *parsing* the markup for the ranges it mirrors. The strip's faces mirror
//! nothing: every range, default, curve and parameter id reaches them through
//! the `StripSpec` global, which `install_strip_spec` fills from
//! `mooloop_core::strip`. So the agreement test for this feature is a
//! different shape, and a stronger one -- it reads the table back out of the
//! window and holds it to the descriptors, and then checks that the markup
//! really has no numbers of its own to drift.

use mooloop_core::strip::{
    strip_band_param, StripParams, STRIP_BAND_FREQ, STRIP_BAND_GAIN, STRIP_BAND_KIND, STRIP_BAND_Q,
    STRIP_BAND_STRIDE, STRIP_COMP_ATTACK_MS, STRIP_COMP_IN, STRIP_COMP_IN_TRIM_DB,
    STRIP_COMP_KNEE_DB, STRIP_COMP_MAKEUP_DB, STRIP_COMP_MIX, STRIP_COMP_RATIO,
    STRIP_COMP_RELEASE_MS, STRIP_COMP_THRESHOLD_DB, STRIP_DRIVE_DB, STRIP_EQ_BANDS, STRIP_EQ_IN,
    STRIP_FIRST, STRIP_PRE_IN, STRIP_VOICING,
};
use mooloop_core::{ParamCurve, PreampVoicing};
use mooloop_ui::{install_strip_spec, MainWindow, StripSpec};
use slint::{ComponentHandle, Model, SharedString};

const STRIP_SLINT: &str = include_str!("../ui/strip.slint");

fn headless() -> MainWindow {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(SharedString::from("software")),
        },
    )))
    .ok();
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
        (spec.get_in_trim_db(), STRIP_COMP_IN_TRIM_DB),
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

/// And the markup really has nothing of its own to drift.
///
/// A face that mirrors a range can be checked for drift; a face with no range
/// cannot drift at all, and this is the test that keeps it that way. It fails
/// the moment somebody adds `minimum: -24` to a strip knob "to make it
/// clearer", which is exactly how the second copy of a range gets written.
#[test]
fn the_strips_markup_declares_no_range_of_its_own() {
    // `StripKnob` is the one place bounds are bound, and it binds them to
    // the descriptor. Everything else must go through it.
    let bindings: Vec<&str> = STRIP_SLINT
        .lines()
        .map(str::trim)
        .filter(|line| {
            // A binding ends in a semicolon; `StripParamSpec`'s own field
            // list ends each line in a comma, and naming the fields is how
            // the table is declared rather than a second copy of a range.
            line.ends_with(';')
                && (line.starts_with("minimum:")
                    || line.starts_with("maximum:")
                    || line.starts_with("default-value:"))
                && !line.contains("root.spec.")
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
        "StripSpec.in-trim-db",
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
