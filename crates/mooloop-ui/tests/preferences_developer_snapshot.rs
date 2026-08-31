//! Headless render of the Preferences dialog's Developer page, which is where
//! the diagnostic log is switched on. The page only exists while developer
//! mode is enabled, so this is also the check that it appears at all.

use mooloop_ui::MainWindow;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, SharedString};

/// Center of the compact "Developer" vertical tab at 800x600. The nav list is
/// a zero-spacing column of 28px items starting at y=90, and Developer is the
/// sixth -- the one that only renders when `developer-mode` is on.
const DEVELOPER_NAV_ITEM: (f32, f32) = (78.0, 244.0);

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

#[test]
fn render_preferences_developer_snapshot() {
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
    ui.set_preferences_developer_mode(true);
    ui.set_preferences_log_to_file(true);
    ui.set_preferences_log_path(SharedString::from("/home/adam/.config/mooloop/mooloop.log"));

    // `page` is private to `PreferencesDialog`, so the page is reached the way
    // the user reaches it.
    click_at(ui.window(), DEVELOPER_NAV_ITEM);

    let snapshot = ui.window().take_snapshot().expect("headless snapshot");
    assert_eq!((snapshot.width(), snapshot.height()), (800, 600));
    assert!(snapshot.as_bytes().iter().any(|byte| *byte != 0));

    if let Ok(path) = std::env::var("MOOLOOP_PREFERENCES_DEVELOPER_SNAPSHOT") {
        let mut ppm = format!("P6\n{} {}\n255\n", snapshot.width(), snapshot.height()).into_bytes();
        for rgba in snapshot.as_bytes().as_chunks::<4>().0 {
            ppm.extend_from_slice(&rgba[..3]);
        }
        std::fs::write(path, ppm).unwrap();
    }
}
