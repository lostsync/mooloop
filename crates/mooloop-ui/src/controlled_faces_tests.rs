//! **Every control follows its parameter, after it has been touched** (MOO-220).
//!
//! Adam, 2026-09-24: *"when you pick a new point in the EQ, the knobs all stay
//! on the last point's values."* The Rust side was right -- picking a point
//! republishes the EQ's row with the new band's values -- and the knobs had
//! stopped listening. A control that wrote its own value on a drag wrote
//! through `<=>` into whatever fed it, and **Slint drops a binding at the
//! first assignment to the property it feeds**, so a face bound to a model
//! row (`frequency: slot.p2`) kept the value it was dragged to for the rest
//! of its life. Undo, a preset, a MIDI move and automation readback were
//! hidden the same way, on every face and every touched control.
//!
//! The fix is in the shared controls (`controls.slint`): a control is
//! *controlled* by default. It reports the change and never writes its own
//! value, so what it shows is always a function of what its owner publishes.
//! A control over a plain property that Rust sets directly -- the source
//! faces' window properties -- may still opt out, because writing a property
//! with no binding behind it drops nothing.
//!
//! These tests hold the *claim*, not the mechanism: whichever path a control
//! takes, **every** slider in the window shows its model's value again after
//! the model is changed from outside. They drive the real window with real
//! wheel events at each control's own position, then republish the way an
//! undo, a preset or a MIDI move does, and read each control's
//! `accessible-value` -- its readout -- back.
//!
//! The controls are found by walking the item tree for the `slider` role
//! rather than through `ElementHandle`'s search API, which needs element debug
//! info that only the `mcp` feature compiles in; CI builds without it, and a
//! test that silently found nothing there would be the defect this file is
//! about.

use super::*;
use crate::window_probe::{click, controls, install_backend, sliders, wheel, Control};
use i_slint_core::items::AccessibleRole;
use slint::LogicalSize;

const WIDTH: f32 = 2400.0;
const HEIGHT: f32 = 1400.0;

/// A window with the rack in the main pane and nothing docked below it, the
/// state `AppUi::new` would leave it in for the parts a face reads.
fn rack_window() -> (MainWindow, Rc<RefCell<UiState>>) {
    install_backend();
    let window = MainWindow::new().expect("the testing backend builds a window");
    window.window().set_size(LogicalSize::new(WIDTH, HEIGHT));
    install_strip_spec(&window);
    install_eq_spec(&window);
    let state = Rc::new(RefCell::new(UiState::new(None, 48_000, &window)));
    window.invoke_move_view(view::DEVICES, 0);
    window.invoke_show_view(view::DEVICES);
    window.set_bottom_pane_visible(false);
    (window, state)
}

/// The effect handler, as `AppUi::new` wires it: the session takes the
/// write and the row is republished. A controlled knob moves only through
/// this, so it is what makes a wheel step visible at all.
///
/// A stand-in with no history, because nothing here undoes. The receiver is
/// `ui` rather than `window` so `scripts/dupe-audit unrecorded-edit`, which
/// reads `src/` as the application, does not report a test's stand-in as an
/// edit path the application has.
fn wire_effect_params(ui: &MainWindow, state: &Rc<RefCell<UiState>>) {
    let st = state.clone();
    ui.on_effect_param_changed(move |slot, param_index, normalized| {
        let mut st = st.borrow_mut();
        if let EffectParamWrite::Applied { .. } =
            st.session.set_effect_param(slot, param_index, normalized)
        {
            st.refresh_effect_row(slot as usize);
        }
    });
}

/// How an outside change reaches the window.
#[derive(Clone, Copy)]
enum Republish {
    /// One effect row at a time, with `set_row_data` -- the path an EQ band
    /// change, a MIDI move, automation readback and a preset on one device
    /// take. A whole-model `set_vec` would rebuild the faces, and a rebuilt
    /// face has its bindings back, which would hide exactly what is tested.
    EffectRows,
    /// `refresh_editor`, which is how every source parameter reaches its
    /// window property on an undo, a preset load or a channel switch.
    Editor,
}

fn republish(window: &MainWindow, state: &Rc<RefCell<UiState>>, how: Republish) {
    let st = state.borrow();
    if let Republish::Editor = how {
        st.refresh_editor(window);
    }
    for channel in 0..st.session.channels.len() {
        st.refresh_rack_row(channel);
    }
    let slots = st.session.effect_chain().map_or(0, |chain| chain.len());
    for slot in 0..slots {
        st.refresh_effect_row(slot);
    }
}

/// Touch every control once, then put the document back underneath them
/// and require every readout to say what it said before the touch.
///
/// The outside change puts back the project as it was, the way an undo does,
/// and republishes it `how`. Returns how many controls the wheel visibly
/// moved, so a caller can tell the touches reached something.
fn every_control_follows(
    window: &MainWindow,
    state: &Rc<RefCell<UiState>>,
    how: Republish,
    skip: &[Control],
    what: &str,
) -> usize {
    let under_test = |window: &MainWindow| -> Vec<Control> {
        sliders(window)
            .into_iter()
            .filter(|control| {
                !skip
                    .iter()
                    .any(|other| other.label == control.label && other.centre == control.centre)
            })
            .collect()
    };
    let project = state
        .borrow()
        .session
        .project_snapshot(window.get_bpm(), window.get_swing_percent());
    let before = under_test(window);
    assert!(!before.is_empty(), "{what}: no sliders found, so this proves nothing");

    for control in &before {
        wheel(window, control);
    }
    let touched = under_test(window);
    let moved = before
        .iter()
        .zip(&touched)
        .filter(|(was, now)| was.value != now.value)
        .count();

    let samples = vec![None; project.channels.len()];
    state.borrow_mut().session.replace_project(&project, &samples);
    republish(window, state, how);

    let after = under_test(window);
    assert_eq!(
        before.len(),
        after.len(),
        "{what}: the republish changed which controls are drawn"
    );
    let stale: Vec<String> = before
        .iter()
        .zip(&after)
        .filter(|(was, now)| was.value != now.value)
        .map(|(was, now)| {
            format!(
                "{:?} at {:?}: showed {:?}, then {:?} after the document went back",
                was.label, was.centre, was.value, now.value
            )
        })
        .collect();
    assert!(
        stale.is_empty(),
        "{what}: {} control(s) stopped following their parameter once touched:\n  {}",
        stale.len(),
        stale.join("\n  ")
    );
    moved
}

/// Every insert device's face, and the rack chrome around it.
#[test]
fn every_effect_face_follows_its_parameters_after_a_touch() {
    let mut moved_anywhere = 0;
    for kind in EffectKind::ALL {
        let (window, state) = rack_window();
        wire_effect_params(&window, &state);
        // The source face and the chrome are the source test's; only what
        // the effect brought is touched here, because an effect-row
        // republish is not what reaches them.
        let chrome = sliders(&window);
        {
            let mut st = state.borrow_mut();
            st.session
                .insert_effect_at(kind, 0)
                .unwrap_or_else(|| panic!("{} inserts into an empty rack", kind.label()));
            st.sync_effects();
        }
        moved_anywhere += every_control_follows(
            &window,
            &state,
            Republish::EffectRows,
            &chrome,
            kind.label(),
        );
    }
    assert!(
        moved_anywhere > 0,
        "no wheel step moved any effect control, so the touches reached nothing"
    );
}

/// The kind of every source, written as a match so a new kind cannot be left
/// out of the test below without failing to compile here.
fn every_source_kind() -> Vec<DeviceKind> {
    let listed = [
        DeviceKind::Sampler,
        DeviceKind::DrumSynth,
        DeviceKind::MonoSynth,
        DeviceKind::PolySynth,
        DeviceKind::MlM1,
        DeviceKind::MlP8,
        DeviceKind::Ds01,
        DeviceKind::AuxIn,
    ];
    for kind in listed {
        match kind {
            DeviceKind::Sampler
            | DeviceKind::DrumSynth
            | DeviceKind::MonoSynth
            | DeviceKind::PolySynth
            | DeviceKind::MlM1
            | DeviceKind::MlP8
            | DeviceKind::Ds01
            | DeviceKind::AuxIn => {}
            // No face of its own yet: a plugin's face is step 08's
            // (MOO-83), which adds it here when it draws one (MOO-84).
            DeviceKind::Plugin => {}
        }
    }
    listed.to_vec()
}

/// Every source device's face, on every page it has.
///
/// No source handler is wired here: those live in `AppUi::new`. So a
/// controlled control does not move, and an opted-out one writes its window
/// property and nothing else. Either way the republish has to bring every
/// readout back, which is the claim.
#[test]
fn every_source_face_follows_its_parameters_after_a_touch() {
    for kind in every_source_kind() {
        let (window, state) = rack_window();
        {
            let mut st = state.borrow_mut();
            st.session.reset_channel_source(0, kind);
            st.refresh_editor(&window);
        }
        // Each page is a different set of controls; the pages are numbered
        // from 0 and a face ignores a page it does not have.
        for page in 0..6 {
            window.set_sampler_device_page(page);
            window.set_mono_device_page(page);
            window.set_poly_device_page(page);
            window.set_mlp8_device_page(page);
            window.set_ds01_device_page(page);
            window.set_mlm1_device_page(page);
            every_control_follows(
                &window,
                &state,
                Republish::Editor,
                &[],
                &format!("{} page {page}", kind.label()),
            );
        }
    }
}

/// Adam's case, as he met it: turn a band's Freq, pick another band, and the
/// three knobs have to show the band that is now selected.
///
/// Held against a second, untouched window showing the same band, so the
/// expectation is the face's own formatting of the right values rather than
/// a copy of it written here.
#[test]
fn picking_another_eq_band_shows_that_bands_values_after_a_knob_was_turned() {
    let open_eq = || {
        let (window, state) = rack_window();
        wire_effect_params(&window, &state);
        {
            let mut st = state.borrow_mut();
            st.session.insert_effect_at(EffectKind::Eq, 0).expect("an EQ inserts");
            st.sync_effects();
        }
        (window, state)
    };
    let band_button = |window: &MainWindow, position: usize| {
        let buttons: Vec<Control> = controls(window, AccessibleRole::Button)
            .into_iter()
            .filter(|button| button.label.starts_with("Select EQ band"))
            .collect();
        assert!(buttons.len() > position, "the EQ draws its row of targets");
        buttons[position].centre
    };
    let knobs = |window: &MainWindow| -> Vec<Control> {
        sliders(window)
            .into_iter()
            .filter(|knob| knob.label.starts_with("Selected band"))
            .collect()
    };

    // Position 0 is the high-pass; 1 and 3 are bands 1 and 3.
    let (window, _state) = open_eq();
    click(&window, band_button(&window, 1));
    let band_one = knobs(&window);
    assert_eq!(band_one.len(), 3, "Freq, Gain and Q");
    wheel(&window, &band_one[0]);
    assert_ne!(
        band_one[0].value,
        knobs(&window)[0].value,
        "the wheel did not move band 1's Freq, so the rest proves nothing"
    );
    click(&window, band_button(&window, 3));
    let shown = knobs(&window);

    let (fresh, _fresh_state) = open_eq();
    click(&fresh, band_button(&fresh, 3));
    let expected = knobs(&fresh);

    let values = |knobs: &[Control]| knobs.iter().map(|k| k.value.clone()).collect::<Vec<_>>();
    assert_eq!(
        values(&shown),
        values(&expected),
        "after turning band 1's Freq and picking band 3, the knobs must show band 3"
    );
}
