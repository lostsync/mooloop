//! The seven-band EQ's face, against the table the engine reads.
//!
//! `slint_face_agreement.rs` holds a device's face to its descriptors by
//! *parsing* the markup for the ranges it mirrors. The EQ's face mirrors
//! nothing as of 2026-09-14: every range, label, count and resting value
//! reaches it through the `EqSpec` global, which `install_eq_spec` fills from
//! `mooloop_core`'s EQ descriptors. So its agreement test is the shape
//! `strip_face.rs` is -- it reads the table back out of the window and holds
//! it to the descriptors, then checks that no bound has reappeared in the
//! markup.
//!
//! It also holds the *response plot's* two conventions, which are not
//! parameter ranges and coincide with two that are. A dragged handle reports
//! its position as 0..1 and that number is written straight to a parameter,
//! so the axis and the range have to be the same span.

use mooloop_core::{
    eq_band_param, eq_pass_param, eq_plot_position, EffectKind, EqBandKind, EqFaceControl,
    EqParams, EqSlope, ParamCurve, EQ_BAND_FREQ, EQ_BAND_GAIN, EQ_DEFAULT_BAND_HZ,
    EQ_FACE_CONTROLS, EQ_MAX_BANDS, EQ_PASS_SLOPE, EQ_SLOPE_COUNT,
};
use mooloop_ui::{
    eq_plot_band, eq_plot_pass, install_eq_spec, EqSpec, MainWindow, EQ_PLOT_BAND_STRIDE,
    EQ_PLOT_PASS_STRIDE,
};
use slint::{ComponentHandle, Model};

mod common;

const EQ_SLINT: &str = include_str!("../ui/eq-device.slint");
const DISPLAYS_SLINT: &str = include_str!("../ui/device-displays.slint");

fn headless() -> MainWindow {
    common::install_testing_backend();
    MainWindow::new().unwrap()
}

fn spec_window() -> MainWindow {
    let ui = headless();
    install_eq_spec(&ui);
    ui
}

/// **The agreement test.** Every range behind a knob is the descriptor's.
///
/// A control's row is read off whichever target has that control, which is
/// how `install_eq_spec` fills it; this asks the question the other way
/// round, from the face's own control indices.
#[test]
fn the_faces_ranges_are_the_engines_ranges() {
    let ui = spec_window();
    let spec = ui.global::<EqSpec>();
    let controls = spec.get_controls();
    assert_eq!(
        controls.row_count(),
        EQ_FACE_CONTROLS,
        "the face was handed a different number of controls than it has"
    );

    let opening = EqParams::default().selected_target();
    for index in 0..EQ_FACE_CONTROLS as u32 {
        let row = controls.row_data(index as usize).unwrap();
        let Some(control) = EqFaceControl::from_face_index(index) else {
            // Face index 0 is the target selector, which stopped being a
            // parameter in `eq-v2/01` and has no range to state.
            assert_eq!(row.minimum, 0.0);
            assert_eq!(row.maximum, 0.0);
            continue;
        };
        // Whichever target answers -- a pass filter has no gain, a band no
        // slope -- the range is that control's everywhere it exists.
        let id = (0..=EqParams::LOW_PASS_TARGET)
            .find_map(|target| EqParams::id_for_selected(target, control))
            .unwrap_or_else(|| panic!("no target has face control {index}"));
        let descriptor = EffectKind::Eq
            .descriptor(id)
            .expect("every id the face resolves is described");
        assert_eq!(row.unit.as_str(), descriptor.unit, "control {index}");
        assert_eq!(row.minimum, descriptor.min, "control {index}");
        assert_eq!(row.maximum, descriptor.max, "control {index}");
        assert_eq!(
            row.logarithmic,
            matches!(descriptor.curve, ParamCurve::Exponential),
            "control {index}"
        );

        // And the range does not depend on which target is showing: the
        // opening selection's descriptor for the same control has to agree,
        // or a knob would change range as the selector moved.
        if let Some(theirs) =
            EqParams::id_for_selected(opening, control).and_then(|id| EffectKind::Eq.descriptor(id))
        {
            assert_eq!((theirs.min, theirs.max), (descriptor.min, descriptor.max));
        }
    }
}

/// **A knob's double-click returns to the selected target's own default.**
///
/// The defect this closes had been in `LOOSE_ENDS.md` since the day per-band
/// ids landed: the face's resting values were three numbers written into the
/// markup, worked out for the target a fresh EQ opens on, so a double-click
/// on band 1's Freq knob returned it to 1 kHz when band 1 rests at 120 Hz. It
/// was equally true before per-band ids and uncheckable then, because one
/// Freq descriptor stood for all seven bands.
#[test]
fn every_target_rests_where_its_own_descriptors_do() {
    let ui = spec_window();
    let spec = ui.global::<EqSpec>();
    let defaults = spec.get_defaults();
    assert_eq!(
        defaults.row_count(),
        (EqParams::LOW_PASS_TARGET + 1) * EQ_FACE_CONTROLS,
        "one resting value per control per target"
    );

    for target in 0..=EqParams::LOW_PASS_TARGET {
        for index in 0..EQ_FACE_CONTROLS as u32 {
            let want = EqFaceControl::from_face_index(index)
                .and_then(|control| EqParams::id_for_selected(target, control))
                .and_then(|id| EffectKind::Eq.descriptor(id))
                .map(|descriptor| descriptor.to_normalized(descriptor.default))
                .unwrap_or(0.0);
            let got = defaults
                .row_data(target * EQ_FACE_CONTROLS + index as usize)
                .unwrap();
            assert!(
                (got - want).abs() < 1e-6,
                "target {target} control {index}: face rests at {got}, table at {want}"
            );
        }
    }

    // The case that was wrong, stated as itself rather than left to the loop:
    // no two bands rest at the same frequency, so one resting value could not
    // have served any two of them. It is the whole bank rather than the first
    // two because all seven spread out on 2026-09-15.
    for (band, rest_hz) in EQ_DEFAULT_BAND_HZ.iter().enumerate() {
        let descriptor = EffectKind::Eq
            .descriptor(eq_band_param(band, EQ_BAND_FREQ))
            .unwrap();
        assert_eq!(
            descriptor.default,
            *rest_hz,
            "band {} rests somewhere the shared table does not put it",
            band + 1
        );
    }
    let freq_control = EqFaceControl::Frequency.face_index() as usize;
    assert!(
        (defaults.row_data(freq_control).unwrap()
            - defaults.row_data(EQ_FACE_CONTROLS + freq_control).unwrap())
        .abs()
            > 0.1,
        "band 1 and band 2 rest at the same place, so the per-target array is not doing anything"
    );
}

/// The selector counts the way the parameters count.
///
/// Band 0's descriptors are `B1 Freq` and friends, and its button said LOW
/// until 2026-09-14 -- so the face called a band one number and the
/// automation menu called it another, which is a small lie that costs
/// somebody an evening exactly once.
#[test]
fn the_selectors_labels_are_the_parameters_numbering() {
    let ui = spec_window();
    let spec = ui.global::<EqSpec>();
    let names = spec.get_target_names();
    assert_eq!(spec.get_band_count(), EQ_MAX_BANDS as i32);
    assert_eq!(
        spec.get_target_count(),
        EqParams::LOW_PASS_TARGET as i32 + 1
    );
    assert_eq!(names.row_count(), EqParams::LOW_PASS_TARGET + 1);

    for band in 0..EQ_MAX_BANDS {
        let label = names.row_data(band).unwrap();
        let name = EffectKind::Eq
            .descriptor(eq_band_param(band, EQ_BAND_FREQ))
            .unwrap()
            .name;
        assert_eq!(
            name,
            format!("B{label} Freq"),
            "band {band}'s button says {label} and its parameter is called {name}"
        );
    }
    assert_eq!(names.row_data(EqParams::HIGH_PASS_TARGET).unwrap(), "HP");
    assert_eq!(names.row_data(EqParams::LOW_PASS_TARGET).unwrap(), "LP");
}

/// **The slope selector says what the bank rolls off at.**
///
/// It read 6/12/18/24/36 over a bank doing 12/24/36/48/72 from the day it
/// shipped, because a stage is `Biquad::pass` and that is a second-order
/// section. `EqSlope::db_per_octave` is the arithmetic; the variant names are
/// the persisted spelling and stayed wrong on purpose.
#[test]
fn the_slope_selector_says_what_the_bank_runs() {
    let ui = spec_window();
    let names = ui.global::<EqSpec>().get_slope_names();
    assert_eq!(names.row_count(), EQ_SLOPE_COUNT);
    for (index, slope) in EqSlope::all().iter().enumerate() {
        assert_eq!(slope.to_index() as usize, index, "slope order");
        assert_eq!(
            names.row_data(index).unwrap(),
            slope.db_per_octave().to_string(),
            "slope {index} runs {} stages",
            slope.stages()
        );
    }
    // And the selector has a position for every slope the parameter offers.
    let descriptor = EffectKind::Eq
        .descriptor(eq_pass_param(0, EQ_PASS_SLOPE))
        .unwrap();
    assert_eq!(descriptor.curve, ParamCurve::Stepped(EQ_SLOPE_COUNT as u16));
}

/// **The handle's axes are the parameter's range.**
///
/// A dragged point reports where it is as 0..1 and the face writes that
/// number straight to a parameter, so if the plot's frequency axis stopped
/// being the Freq descriptor's range, or its gain axis the Gain descriptor's,
/// a point dragged to the edge would write something other than the edge --
/// and nothing about that is visible at either end. This is the seven-band
/// EQ's half of the coupling `strip_face.rs` asserts for the strip.
#[test]
fn a_dragged_point_writes_the_parameter_it_looks_like() {
    for band in 0..EQ_MAX_BANDS {
        let freq = EffectKind::Eq
            .descriptor(eq_band_param(band, EQ_BAND_FREQ))
            .unwrap();
        for hz in [20.0, 120.0, 1_000.0, 8_000.0, 20_000.0] {
            let axis = eq_plot_position(hz);
            let parameter = freq.to_normalized(hz);
            assert!(
                (axis - parameter).abs() < 1e-5,
                "band {band}: {hz} Hz is {axis} along the plot and {parameter} along the knob"
            );
        }

        let gain = EffectKind::Eq
            .descriptor(eq_band_param(band, EQ_BAND_GAIN))
            .unwrap();
        for db in [-18.0, -6.0, 0.0, 6.0, 18.0] {
            let axis = eq_plot_band(1_000.0, db, true)[1];
            let parameter = gain.to_normalized(db);
            assert!(
                (axis - parameter).abs() < 1e-5,
                "band {band}: {db} dB is {axis} up the plot and {parameter} up the knob"
            );
        }
    }
}

/// The publisher and the reader agree about the flat arrays between them.
///
/// `02-the-curve-tells-the-truth.md` opened on this: the pass filters were
/// appended to the band array at a different stride, so one model had two
/// layouts, written in `lib.rs` and read in `device-displays.slint`, with
/// nothing asserting either. They are two properties now, and this is the
/// test that the split stayed split.
#[test]
fn the_plot_reads_the_strides_the_publisher_writes() {
    assert_eq!(eq_plot_band(1_000.0, 0.0, true).len(), EQ_PLOT_BAND_STRIDE);
    assert_eq!(eq_plot_pass(1_000.0, true).len(), EQ_PLOT_PASS_STRIDE);
    for (property, stride) in [
        ("band-stride", EQ_PLOT_BAND_STRIDE),
        ("pass-stride", EQ_PLOT_PASS_STRIDE),
    ] {
        let declared = format!("property <int> {property}: {stride};");
        assert!(
            DISPLAYS_SLINT.contains(&declared),
            "device-displays.slint does not say `{declared}`, so the plot and the \
             publisher have stopped agreeing about the layout of a flat array"
        );
    }
    assert!(
        DISPLAYS_SLINT.contains(&format!("property <int> band-count: {EQ_MAX_BANDS};")),
        "the plot draws a different number of bands than the bank has"
    );
    // The handle indices the plot reports are the EQ's own target numbering,
    // which is what lets the face turn one into a selection with no table.
    assert!(
        DISPLAYS_SLINT.contains("root.point-grabbed(root.band-count + pass)"),
        "a pass filter's handle no longer reports itself as a target past the bands"
    );
}

/// **No range, resting value, count or label is spelled in the markup.**
///
/// `install_eq_spec` is what makes this true by construction; this is what
/// notices when one comes back. The numbers are the ones that were in the
/// file before 2026-09-14, plus the two the face used to invert a dragged
/// point with.
#[test]
fn the_eq_face_declares_no_bounds_of_its_own() {
    let banned = [
        ("20 * pow(1000", "the frequency range"),
        ("* 36 - 18", "the gain range"),
        ("0.15 * pow(120", "the Q range"),
        ("0.566", "Freq's resting position"),
        ("0.323843", "Q's resting position"),
        ("round(root.target * 8)", "the target count"),
        ("index / 8", "the target count"),
        ("\"LOW\"", "a band label"),
        ("\"36\"", "a slope label"),
    ];
    for (needle, what) in banned {
        assert!(
            !EQ_SLINT.contains(needle),
            "eq-device.slint spells {what} (`{needle}`). It comes from `EqSpec`, \
             which Rust fills from the descriptor table -- see `install_eq_spec`."
        );
    }
    // And it does reach the table, rather than having simply lost the
    // controls that needed it.
    for name in [
        "EqSpec.rest(",
        "EqSpec.format(",
        "EqSpec.target-value(",
        "EqSpec.slope-value(",
        "EqSpec.target-names",
        "EqSpec.slope-names",
    ] {
        assert!(
            EQ_SLINT.contains(name),
            "eq-device.slint no longer reads {name}"
        );
    }
}

/// **The selector's glyphs are the bands' own kinds.**
///
/// A target button draws a shelf where its band is a shelf and its number
/// where it is a bell, from the `band-kinds` array Rust publishes -- which
/// carries `EqBandKind::to_index`. Those two integers are the only thing
/// about the engine's numbering `eq-device.slint` knows, so they are the only
/// thing that can drift, and the drift would be silent in the way this
/// codebase keeps finding: a reordered enum would leave every high shelf
/// drawn as a low one and nothing about the sound would change.
#[test]
fn the_selectors_glyphs_follow_the_bands_own_kind() {
    for (kind, icon) in [
        (EqBandKind::LowShelf, "low-shelf-icon"),
        (EqBandKind::HighShelf, "high-shelf-icon"),
    ] {
        let clause = format!("kind == {} ? root.{icon}", kind.to_index());
        assert!(
            EQ_SLINT.contains(&clause),
            "eq-device.slint does not say `{clause}`, so the glyph a band draws \
             and the kind it is have stopped agreeing"
        );
    }
    // The fallback is the band's number, which is what a bell draws.
    assert_eq!(EqBandKind::Bell.to_index(), 0);
    assert!(
        EQ_SLINT.contains("root.band-kinds[target]"),
        "the face no longer reads the live kinds, so its glyphs are decoration"
    );
}
