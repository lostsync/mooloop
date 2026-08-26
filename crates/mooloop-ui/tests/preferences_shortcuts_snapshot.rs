//! Headless render of the Preferences dialog's Shortcuts page, so changes
//! to the action-registry prefpane (`docs/ACTIONS.md`) can be checked
//! visually without the live app.

use mooloop_ui::{MainWindow, ShortcutRow as MainShortcutRow};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, ModelRc, SharedString, VecModel};
use std::rc::Rc;

fn write_snapshot(snapshot: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, variable: &str) {
    if let Ok(path) = std::env::var(variable) {
        let mut ppm = format!("P6\n{} {}\n255\n", snapshot.width(), snapshot.height()).into_bytes();
        for rgba in snapshot.as_bytes().as_chunks::<4>().0 {
            ppm.extend_from_slice(&rgba[..3]);
        }
        std::fs::write(path, ppm).unwrap();
    }
}

/// Center of the "Shortcuts" row in the Preferences nav column, with the
/// window at 800x600, measured from a rendered snapshot the same way
/// `preferences_audio_snapshot.rs`'s `AUDIO_NAV_ITEM` was (nominal-geometry
/// derivation drifted from the real layout once and isn't trustworthy).
/// `PreferenceNavItem` rows are a uniform 28px pitch; General is at
/// `AUDIO_NAV_ITEM.1 - 28`, and Shortcuts is the fifth row (General, Audio,
/// MIDI, Appearance, Shortcuts) at `AUDIO_NAV_ITEM.1 + 28 * 3`.
const SHORTCUTS_NAV_ITEM: (f32, f32) = (78.0, 132.0 + 28.0 * 3.0);

fn click_at(window: &slint::Window, p: (f32, f32)) {
    let position = LogicalPosition::new(p.0, p.1);
    window.dispatch_event(WindowEvent::PointerMoved { position });
    window.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

fn sample_rows() -> Vec<MainShortcutRow> {
    vec![
        MainShortcutRow {
            id: SharedString::from("transport.play-pause"),
            label: SharedString::from("Play/Pause"),
            category: SharedString::from("Transport"),
            chord: SharedString::from("Space"),
            is_default: true,
            is_first_in_category: true,
        },
        MainShortcutRow {
            id: SharedString::from("view.pane-next"),
            label: SharedString::from("Next Pane"),
            category: SharedString::from("View"),
            chord: SharedString::from("Ctrl+Right"),
            is_default: true,
            is_first_in_category: true,
        },
        MainShortcutRow {
            id: SharedString::from("channel.clone"),
            label: SharedString::from("Clone Channel"),
            category: SharedString::from("Channel"),
            chord: SharedString::from("Ctrl+Alt+D"),
            is_default: false,
            is_first_in_category: true,
        },
        MainShortcutRow {
            id: SharedString::from("pattern.remove"),
            label: SharedString::from("Remove Pattern"),
            category: SharedString::from("Pattern"),
            chord: SharedString::from(""),
            is_default: false,
            is_first_in_category: true,
        },
    ]
}

slint::slint! {
    import { ShortcutRowView, ShortcutRow } from "../ui/appearance-dialog.slint";

    export component ShortcutRowHarness inherits Window {
        width: 420px;
        height: 40px;
        in property <ShortcutRow> entry;
        in property <bool> capturing;
        ShortcutRowView { entry: root.entry; capturing: root.capturing; }
    }
}

#[test]
fn render_preferences_shortcuts_snapshots() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(SharedString::from("software")),
        },
    )))
    .expect("initialize headless renderer");

    // The dialog page, at the size it actually renders at.
    let ui = MainWindow::new().unwrap();
    ui.window().set_size(LogicalSize::new(800.0, 600.0));
    ui.set_preferences_open(true);
    ui.set_preferences_shortcut_rows(ModelRc::from(Rc::new(VecModel::from(sample_rows()))));

    // The Shortcuts page only becomes visible after clicking its nav item;
    // `page` is private to `PreferencesDialog` and not exposed to Rust.
    click_at(ui.window(), SHORTCUTS_NAV_ITEM);

    let snapshot = ui.window().take_snapshot().expect("headless snapshot");
    write_snapshot(&snapshot, "MOOLOOP_PREFERENCES_SHORTCUTS_SNAPSHOT");
    drop(ui);

    // One row in isolation, in its "Not set" and "capturing" states, to
    // check those two harder-to-reach visuals directly.
    let unset = ShortcutRowHarness::new().unwrap();
    unset.set_entry(ShortcutRow {
        id: SharedString::from("pattern.remove"),
        label: SharedString::from("Remove Pattern"),
        category: SharedString::from("Pattern"),
        chord: SharedString::from(""),
        is_default: false,
        is_first_in_category: false,
    });
    let unset_snapshot = unset.window().take_snapshot().expect("headless snapshot");
    write_snapshot(&unset_snapshot, "MOOLOOP_SHORTCUT_ROW_UNSET_SNAPSHOT");
    drop(unset);

    let capturing = ShortcutRowHarness::new().unwrap();
    capturing.set_entry(ShortcutRow {
        id: SharedString::from("channel.clone"),
        label: SharedString::from("Clone Channel"),
        category: SharedString::from("Channel"),
        chord: SharedString::from("Ctrl+D"),
        is_default: true,
        is_first_in_category: false,
    });
    capturing.set_capturing(true);
    let capturing_snapshot = capturing.window().take_snapshot().expect("headless snapshot");
    write_snapshot(&capturing_snapshot, "MOOLOOP_SHORTCUT_ROW_CAPTURING_SNAPSHOT");
}
