//! Smoke test for the sample browser's sidebar rendering.
//!
//! The tree-flattening logic itself is unit-tested in lib.rs; here the
//! flattened model is spliced in directly and the software renderer must
//! show it: rows change the pixels of an open sidebar, and the browser
//! starts empty and hidden.

use mooloop_ui::{BrowserRow, MainWindow};
use slint::{ComponentHandle, LogicalSize, Model, ModelRc, SharedString, VecModel};
use std::rc::Rc;

fn harness() -> MainWindow {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(SharedString::from("software")),
        },
    )))
    .ok();
    let ui = MainWindow::new().unwrap();
    ui.window().set_size(LogicalSize::new(960.0, 760.0));
    ui
}

fn snapshot(ui: &MainWindow) -> Vec<u8> {
    ui.window().take_snapshot().unwrap().as_bytes().to_vec()
}

#[test]
fn browser_rows_render_in_an_open_sidebar() {
    let ui = harness();
    assert!(!ui.get_sidebar_visible(), "the sidebar starts hidden");
    assert_eq!(ui.get_browser_rows().row_count(), 0, "no locations yet");

    ui.set_sidebar_visible(true);
    let empty = snapshot(&ui);

    ui.set_browser_rows(ModelRc::from(Rc::new(VecModel::from(vec![
        BrowserRow {
            depth: 0,
            kind: 0,
            name: "Drums".into(),
            path: "/sounds/Drums".into(),
            expanded: true,
        },
        BrowserRow {
            depth: 1,
            kind: 1,
            name: "909.wav".into(),
            path: "/sounds/Drums/909.wav".into(),
            expanded: false,
        },
    ]))));
    let populated = snapshot(&ui);

    assert_ne!(
        populated, empty,
        "browser rows must change what the open sidebar renders"
    );
}
