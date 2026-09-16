//! Headless render of the Preferences dialog's Appearance page, so a change to
//! the theme list, the variant control, the three colour seeds, the type
//! fields or the interface scalars can be checked visually without the live
//! app.

use mooloop_ui::AppearanceSchemeRow as MainAppearanceSchemeRow;
use mooloop_ui::MainWindow;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{
    Color, ComponentHandle, LogicalPosition, LogicalSize, ModelRc, SharedString, VecModel,
};
use std::rc::Rc;

mod common;

slint::slint! {
    import { AppearancePage, AppearanceSchemeRow } from "../ui/appearance-dialog.slint";

    export component AppearancePageHarness inherits Window {
        width: 620px;
        height: 1080px;
        background: #232328;
        in property <[AppearanceSchemeRow]> rows;
        in property <[color]> slots;
        AppearancePage {
            schemes: root.rows;
            ramp: root.slots;
            theme: "Nord";
            mode: 0;
            base: "#2E3440";
            accent: "#88C0D0";
            alert: "#EBCB8B";
            contrast: 1.0;
            roundness: 1.0;
            type-scale: 1.0;
            density: 1.0;
            font-family-mono: "monospace";
            font-weight: 400;
            hairline: 1.0;
            stroke-emphasis: 2.0;
            text-ratio: 7.45;
            accent-ratio: 5.03;
            smooth-curves: true;
        }
    }
}

fn write_snapshot(snapshot: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, variable: &str) {
    if let Ok(path) = std::env::var(variable) {
        let mut ppm = format!("P6\n{} {}\n255\n", snapshot.width(), snapshot.height()).into_bytes();
        for rgba in snapshot.as_bytes().as_chunks::<4>().0 {
            ppm.extend_from_slice(&rgba[..3]);
        }
        std::fs::write(path, ppm).unwrap();
    }
}

/// Center of the compact "Appearance" vertical tab at 800x600. The nav items
/// are a fixed 28 px tall in list order, so this is the Audio tab the sibling
/// audio snapshot measured (132) plus two rows.
const APPEARANCE_NAV_ITEM: (f32, f32) = (78.0, 188.0);

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

fn color(rgb: u32) -> Color {
    Color::from_rgb_u8(
        ((rgb >> 16) & 0xff) as u8,
        ((rgb >> 8) & 0xff) as u8,
        (rgb & 0xff) as u8,
    )
}

/// Built-ins, one theme whose variant on this side is derived rather than
/// authored, and one user theme -- so the "derived" note and the Remove
/// affordance are both in frame. `MainWindow` and the standalone page harness
/// each generate their own row struct, so the fixture is built per type.
const FIXTURE: [(&str, u32, u32, u32, bool, bool); 7] = [
    ("Mooloop", 0x18181b, 0x84cc16, 0xeab308, false, false),
    ("Dracula", 0x282a36, 0xbd93f9, 0xf1fa8c, false, false),
    ("Nord", 0x2e3440, 0x88c0d0, 0xebcb8b, false, false),
    ("Gruvbox", 0x282828, 0x83a598, 0xfabd2f, false, false),
    ("Monokai", 0x272822, 0xa6e22e, 0xf4bf75, false, true),
    ("Wallpaper", 0x1d1f21, 0x81a2be, 0xf0c674, false, false),
    ("My Theme", 0x101014, 0x22d3ee, 0xf97316, true, false),
];

/// One ramp for the read-only strip under the list. Nord's, which is what the
/// harness above says it is wearing.
const NORD: [u32; 16] = [
    0x2e3440, 0x3b4252, 0x434c5e, 0x4c566a, 0x9ba6ba, 0xd8dee9, 0xe5e9f0, 0xeceff4, 0xbf616a,
    0xd08770, 0xebcb8b, 0xa3be8c, 0x88c0d0, 0x81a1c1, 0xb48ead, 0x976a5f,
];

#[test]
fn render_preferences_appearance_snapshot() {
    common::install_testing_backend();

    let ui = MainWindow::new().unwrap();
    ui.window().set_size(LogicalSize::new(800.0, 600.0));
    ui.set_preferences_open(true);
    ui.set_preferences_appearance_schemes(ModelRc::from(Rc::new(VecModel::from(
        FIXTURE
            .iter()
            .map(
                |&(name, base, accent, alert, is_user, derived)| MainAppearanceSchemeRow {
                    name: SharedString::from(name),
                    description: SharedString::new(),
                    base: color(base),
                    accent: color(accent),
                    alert: color(alert),
                    is_user,
                    derived,
                },
            )
            .collect::<Vec<_>>(),
    ))));
    ui.set_preferences_appearance_ramp(ModelRc::from(Rc::new(VecModel::from(
        NORD.iter().map(|&slot| color(slot)).collect::<Vec<_>>(),
    ))));
    ui.set_preferences_appearance_theme(SharedString::from("Nord"));
    ui.set_preferences_appearance_mode(2);
    ui.set_preferences_appearance_base(SharedString::from("#2E3440"));
    ui.set_preferences_appearance_accent(SharedString::from("#88C0D0"));
    ui.set_preferences_appearance_alert(SharedString::from("#EBCB8B"));
    ui.set_preferences_appearance_contrast(1.0);
    ui.set_preferences_appearance_roundness(1.0);
    ui.set_preferences_appearance_text_ratio(7.45);
    ui.set_preferences_appearance_accent_ratio(5.03);

    // The Appearance page only becomes visible after clicking its nav item;
    // `page` is private to `PreferencesDialog` and not exposed to Rust.
    click_at(ui.window(), APPEARANCE_NAV_ITEM);

    let snapshot = ui.window().take_snapshot().expect("headless snapshot");
    write_snapshot(&snapshot, "MOOLOOP_PREFERENCES_APPEARANCE_SNAPSHOT");
    drop(ui);

    // The page alone, unclipped, so the sections below the dialog's fold --
    // the color fields, the interface scalars, and the preview strip -- are
    // checkable too.
    let harness = AppearancePageHarness::new().unwrap();
    harness.set_slots(ModelRc::from(Rc::new(VecModel::from(
        NORD.iter().map(|&slot| color(slot)).collect::<Vec<_>>(),
    ))));
    harness.set_rows(ModelRc::from(Rc::new(VecModel::from(
        FIXTURE
            .iter()
            .map(
                |&(name, base, accent, alert, is_user, derived)| AppearanceSchemeRow {
                    name: SharedString::from(name),
                    description: SharedString::new(),
                    base: color(base),
                    accent: color(accent),
                    alert: color(alert),
                    is_user,
                    derived,
                },
            )
            .collect::<Vec<_>>(),
    ))));
    let page = harness.window().take_snapshot().expect("headless snapshot");
    write_snapshot(&page, "MOOLOOP_APPEARANCE_PAGE_SNAPSHOT");
}
