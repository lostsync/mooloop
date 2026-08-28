//! Headless render of the Preferences dialog's Appearance page, so a change to
//! the scheme list, the three color seeds, or the interface scalars can be
//! checked visually without the live app.

use mooloop_ui::AppearanceSchemeRow as MainAppearanceSchemeRow;
use mooloop_ui::MainWindow;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{
    Color, ComponentHandle, LogicalPosition, LogicalSize, ModelRc, SharedString, VecModel,
};
use std::rc::Rc;

slint::slint! {
    import { AppearancePage, AppearanceSchemeRow } from "../ui/appearance-dialog.slint";

    export component AppearancePageHarness inherits Window {
        width: 620px;
        height: 660px;
        background: #232328;
        in property <[AppearanceSchemeRow]> rows;
        AppearancePage {
            schemes: root.rows;
            scheme: "Mooloop";
            base: "#18181B";
            accent: "#84CC16";
            alert: "#EAB308";
            contrast: 1.0;
            roundness: 1.0;
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

/// The built-ins plus one user scheme, so the Remove affordance that only user
/// rows carry is in frame. `MainWindow` and the standalone page harness each
/// generate their own row struct, so the fixture is built per type.
const FIXTURE: [(&str, u32, u32, u32, bool); 7] = [
    ("Mooloop", 0x18181b, 0x84cc16, 0xeab308, false),
    ("Graphite", 0x151617, 0xf59e0b, 0x38bdf8, false),
    ("High Contrast", 0x000000, 0x22d3ee, 0xfacc15, false),
    ("Ember", 0x1a1413, 0xf97316, 0x38bdf8, false),
    ("Indigo", 0x14141f, 0xa78bfa, 0xf472b6, false),
    ("Daylight", 0xededf0, 0x3f7d00, 0xb45309, false),
    ("My Scheme", 0x101014, 0x22d3ee, 0xf97316, true),
];

#[test]
fn render_preferences_appearance_snapshot() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(SharedString::from("software")),
        },
    )))
    .expect("initialize headless renderer");

    let ui = MainWindow::new().unwrap();
    ui.window().set_size(LogicalSize::new(800.0, 600.0));
    ui.set_preferences_open(true);
    ui.set_preferences_appearance_schemes(ModelRc::from(Rc::new(VecModel::from(
        FIXTURE
            .iter()
            .map(
                |&(name, base, accent, alert, is_user)| MainAppearanceSchemeRow {
                    name: SharedString::from(name),
                    base: color(base),
                    accent: color(accent),
                    alert: color(alert),
                    is_user,
                },
            )
            .collect::<Vec<_>>(),
    ))));
    ui.set_preferences_appearance_scheme(SharedString::from("Mooloop"));
    ui.set_preferences_appearance_base(SharedString::from("#18181B"));
    ui.set_preferences_appearance_accent(SharedString::from("#84CC16"));
    ui.set_preferences_appearance_alert(SharedString::from("#EAB308"));
    ui.set_preferences_appearance_contrast(1.0);
    ui.set_preferences_appearance_roundness(1.0);

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
    harness.set_rows(ModelRc::from(Rc::new(VecModel::from(
        FIXTURE
            .iter()
            .map(
                |&(name, base, accent, alert, is_user)| AppearanceSchemeRow {
                    name: SharedString::from(name),
                    base: color(base),
                    accent: color(accent),
                    alert: color(alert),
                    is_user,
                },
            )
            .collect::<Vec<_>>(),
    ))));
    let page = harness.window().take_snapshot().expect("headless snapshot");
    write_snapshot(&page, "MOOLOOP_APPEARANCE_PAGE_SNAPSHOT");
}
