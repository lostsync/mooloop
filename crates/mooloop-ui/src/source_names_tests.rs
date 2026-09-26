//! The source device's names as the window draws them (MOO-276).
//!
//! Adam, 2026-09-26: **both where there's room, the model number where it's
//! tight.** The rack's source header has the room, so it reads "Polyneight
//! ML-P8"; the toolbar's 96px source chip does not, so it reads "ML-P8", and
//! the menu the chip opens has the room again. `tests/source_kind_menu.rs`
//! holds the markup's lists to `DeviceKind`; this reads what the window
//! actually draws, by accessible role, which every build carries.

use super::*;
use crate::window_probe::{click, controls, install_backend, Control};
use i_slint_core::items::AccessibleRole;
use slint::LogicalSize;

/// The sources with a nickname, whose title and label differ. The rest read
/// the same string in both places, and would pass whatever was drawn.
const NICKNAMED: [DeviceKind; 4] = [
    DeviceKind::DrumSynth,
    DeviceKind::MlM1,
    DeviceKind::MlP8,
    DeviceKind::Ds01,
];

/// The device view, with its rack headed by `kind`.
fn devices_showing(kind: DeviceKind) -> MainWindow {
    install_backend();
    let window = MainWindow::new().expect("the testing backend builds a window");
    window.window().set_size(LogicalSize::new(2400.0, 1000.0));
    window.invoke_move_view(view::DEVICES, 0);
    window.invoke_show_view(view::DEVICES);
    window.set_bottom_pane_visible(false);
    window.set_source_kind(device_kind_to_int(kind));
    window
}

fn texts_reading(window: &MainWindow, text: &str) -> Vec<Control> {
    controls(window, AccessibleRole::Text)
        .into_iter()
        .filter(|control| control.label == text)
        .collect()
}

/// **The header reads the source's full name, and the chip its model
/// number.** Before MOO-276 both read the model number.
#[test]
fn the_source_header_reads_the_title_and_the_chip_the_model_number() {
    for kind in NICKNAMED {
        let window = devices_showing(kind);
        assert_eq!(
            texts_reading(&window, kind.title()).len(),
            1,
            "the source header should read {:?} for {kind:?}",
            kind.title()
        );
        assert_eq!(
            texts_reading(&window, kind.label()).len(),
            1,
            "the source chip should read {:?} for {kind:?}",
            kind.label()
        );
    }
}

/// **The chip's menu reads each offered source's full name,** and still
/// reports the row's own index, so the row reading "Munotone ML-M1" is the
/// one that switches the channel to the ML-M1.
#[test]
fn the_source_chips_menu_reads_the_titles() {
    let window = devices_showing(DeviceKind::MlP8);
    let picked = Rc::new(RefCell::new(Vec::new()));
    let seen = picked.clone();
    window.on_channel_source_changed(move |index| seen.borrow_mut().push(index));

    assert!(
        texts_reading(&window, DeviceKind::MlM1.title()).is_empty(),
        "nothing should read the ML-M1's title before the menu opens"
    );
    let chip = texts_reading(&window, DeviceKind::MlP8.label())
        .pop()
        .expect("the source chip reads ML-P8");
    click(&window, chip.centre);

    // The kinds whose title is their label ("Sampler") are left to
    // `tests/source_kind_menu.rs`: a plain word may be drawn elsewhere too.
    for kind in NICKNAMED {
        // The ML-P8's title is its header's too.
        let expected = if kind == DeviceKind::MlP8 { 2 } else { 1 };
        assert_eq!(
            texts_reading(&window, kind.title()).len(),
            expected,
            "the source menu should have a row reading {:?}",
            kind.title()
        );
    }
    for kind in RETIRED_SOURCE_KINDS {
        assert!(
            texts_reading(&window, kind.title()).is_empty(),
            "the menu should not offer the retired {kind:?}"
        );
    }

    let row = texts_reading(&window, DeviceKind::MlM1.title())
        .pop()
        .expect("the menu has an ML-M1 row");
    click(&window, row.centre);
    assert_eq!(
        *picked.borrow(),
        vec![device_kind_to_int(DeviceKind::MlM1)],
        "the row reading \"Munotone ML-M1\" should switch the channel to the ML-M1"
    );
}
