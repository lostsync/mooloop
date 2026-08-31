//! Smoke test for the sample browser's sidebar rendering.
//!
//! The tree-flattening logic itself is unit-tested in lib.rs; here the
//! flattened model is spliced in directly and the software renderer must
//! show it: rows change the pixels of an open sidebar, and the browser
//! starts empty and hidden.

use mooloop_ui::{BrowserRow, MainWindow};
use slint::{
    platform::{PointerEventButton, WindowEvent},
    ComponentHandle, LogicalPosition, LogicalSize, Model, ModelRc, SharedString, VecModel,
};
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
    assert!(ui.get_browser_autoplay(), "autoplay arms by default");
    assert_eq!(
        ui.get_browser_preview_gain_db(),
        mooloop_core::gain::REFERENCE_PEAK_DBFS,
        "the preview monitor starts at the operating level, not unity"
    );

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

#[test]
fn info_pane_renders_once_a_sample_is_inspected() {
    let ui = harness();
    ui.set_sidebar_visible(true);
    ui.set_browser_rows(ModelRc::from(Rc::new(VecModel::from(vec![BrowserRow {
        depth: 0,
        kind: 0,
        name: "Drums".into(),
        path: "/sounds/Drums".into(),
        expanded: false,
    }]))));
    let before = snapshot(&ui);

    ui.set_browser_info_name("909.wav".into());
    ui.set_browser_info_stats("44100 Hz · 16-bit int · mono\n4410 frames · 0.10 s".into());
    ui.set_browser_info_waveform(ModelRc::from(Rc::new(VecModel::from(vec![
        0.1, 0.5, 0.9, 0.4, 0.2,
    ]))));
    let after = snapshot(&ui);

    assert_ne!(
        first_diff(&before, &after),
        None,
        "the info pane must appear when a sample is inspected"
    );
}

/// The sidebar is right-docked at the window's right edge; sample rows fill
/// its width. Column x is any point inside a row clear of the scrollbar.
const ROW_X: f32 = 960.0 - 260.0 + 40.0;

/// Right-button press + release at `at`, the way a context click arrives.
fn right_click(window: &slint::Window, at: LogicalPosition) {
    window.dispatch_event(WindowEvent::PointerMoved { position: at });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: at,
        button: PointerEventButton::Right,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position: at,
        button: PointerEventButton::Right,
    });
}

/// Find a browser row by its hover highlight: move the pointer down the
/// row column and report the first y whose render differs from the resting
/// snapshot. Scanning keeps the test honest about layout drift.
fn find_row_y(ui: &MainWindow, rest: &[u8]) -> f32 {
    let window = ui.window();
    for y in (0..300).step_by(2) {
        let at = LogicalPosition::new(ROW_X, y as f32 + 0.5);
        window.dispatch_event(WindowEvent::PointerMoved { position: at });
        if snapshot(ui) != rest {
            return y as f32 + 0.5;
        }
    }
    panic!("no browser row highlighted under the scan column");
}

#[test]
fn right_clicking_a_sample_row_opens_the_load_menu() {
    let ui = harness();
    ui.set_sidebar_visible(true);
    ui.set_browser_rows(ModelRc::from(Rc::new(VecModel::from(vec![BrowserRow {
        depth: 1,
        kind: 1,
        name: "909.wav".into(),
        path: "/sounds/Drums/909.wav".into(),
        expanded: false,
    }]))));
    let removed: Rc<std::cell::RefCell<Vec<String>>> = Rc::new(std::cell::RefCell::new(Vec::new()));
    {
        let removed = removed.clone();
        ui.on_browser_location_removed(move |path| removed.borrow_mut().push(path.to_string()));
    }
    let rest = snapshot(&ui);
    let row_y = find_row_y(&ui, &rest);

    right_click(ui.window(), LogicalPosition::new(ROW_X, row_y));
    let with_menu = snapshot(&ui);
    assert_ne!(
        first_diff(&rest, &with_menu),
        None,
        "right-clicking a sample row should open the load menu"
    );
    assert!(
        removed.borrow().is_empty(),
        "right-clicking a sample row must not remove a location"
    );
}

#[test]
fn right_clicking_a_folder_row_still_removes_the_location() {
    let ui = harness();
    ui.set_sidebar_visible(true);
    ui.set_browser_rows(ModelRc::from(Rc::new(VecModel::from(vec![BrowserRow {
        depth: 0,
        kind: 0,
        name: "Drums".into(),
        path: "/sounds/Drums".into(),
        expanded: false,
    }]))));
    let removed: Rc<std::cell::RefCell<Vec<String>>> = Rc::new(std::cell::RefCell::new(Vec::new()));
    {
        let removed = removed.clone();
        ui.on_browser_location_removed(move |path| removed.borrow_mut().push(path.to_string()));
    }
    let rest = snapshot(&ui);
    let row_y = find_row_y(&ui, &rest);

    right_click(ui.window(), LogicalPosition::new(ROW_X, row_y));
    assert_eq!(
        *removed.borrow(),
        vec!["/sounds/Drums".to_string()],
        "right-clicking a location row should still remove it"
    );
}

/// First differing pixel of two RGBA snapshots, as (x, y).
fn first_diff(a: &[u8], b: &[u8]) -> Option<(usize, usize)> {
    a.iter()
        .zip(b)
        .position(|(p, q)| p != q)
        .map(|index| ((index / 4) % 960, (index / 4) / 960))
}
