//! Tests for dragging the piano roll's dock splitter.
//!
//! The splitter is a 1px line at the dock's top edge with a 6px grab zone
//! reaching up into the work surface. These tests dispatch real pointer
//! gestures against the software renderer and compare whole-window
//! snapshots: the dock must follow the pointer exactly (a round trip is
//! byte-identical) and must clamp at its bound without drifting, so the
//! restore after a clamped drag is exact too.
//!
//! Snapshots are taken while hovering the note canvas (a neutral spot that
//! only drives the status bar's hover hint), so grip hover states never
//! leak into the comparison.

use mooloop_ui::MainWindow;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, SharedString};

// Dock geometry in the 960x760 window, measured with the grip probe: the
// splitter's 1px line sits at y=317 with the dock's top edge directly below,
// and the grip spans the 6px just above that line.
const DOCK_TOP_Y: f32 = 318.0;
const GRIP_X: f32 = 480.0;
const GRIP_Y: f32 = DOCK_TOP_Y - 3.0;
const NEUTRAL_Y: f32 = 400.0;

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
    ui.set_editor_page(1);
    ui
}

/// Press at `from`, travel to `to` in several steps the way a real pointer
/// would, then release. Multiple moves exercise the grip's moving-origin
/// drag arithmetic the way a single jump cannot.
fn drag(window: &slint::Window, from: (f32, f32), to: (f32, f32)) {
    let pos = |(x, y): (f32, f32)| LogicalPosition::new(x, y);
    window.dispatch_event(WindowEvent::PointerMoved { position: pos(from) });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: pos(from),
        button: PointerEventButton::Left,
    });
    const STEPS: usize = 8;
    for i in 1..=STEPS {
        let t = i as f32 / STEPS as f32;
        window.dispatch_event(WindowEvent::PointerMoved {
            position: pos((
                from.0 + (to.0 - from.0) * t,
                from.1 + (to.1 - from.1) * t,
            )),
        });
    }
    window.dispatch_event(WindowEvent::PointerReleased {
        position: pos(to),
        button: PointerEventButton::Left,
    });
}

fn hover_neutral(ui: &MainWindow) {
    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(GRIP_X, NEUTRAL_Y),
    });
}

fn snapshot(ui: &MainWindow) -> Vec<u8> {
    ui.window().take_snapshot().unwrap().as_bytes().to_vec()
}

/// First differing pixel of two RGBA snapshots, as (x, y), so a mismatch
/// report points at the region that moved instead of dumping megabytes.
fn first_diff(a: &[u8], b: &[u8]) -> Option<(usize, usize)> {
    a.iter()
        .zip(b)
        .position(|(p, q)| p != q)
        .map(|i| ((i / 4) % 960, (i / 4) / 960))
}

#[test]
fn splitter_drag_resizes_and_restores_exactly() {
    let ui = harness();
    hover_neutral(&ui);
    let before = snapshot(&ui);

    drag(ui.window(), (GRIP_X, GRIP_Y), (GRIP_X, GRIP_Y - 24.0));
    hover_neutral(&ui);
    assert_ne!(
        snapshot(&ui),
        before,
        "dragging the splitter up must enlarge the dock"
    );

    drag(ui.window(), (GRIP_X, GRIP_Y - 24.0), (GRIP_X, GRIP_Y));
    hover_neutral(&ui);
    assert_eq!(
        first_diff(&before, &snapshot(&ui)),
        None,
        "dragging back must restore the exact layout"
    );
}

#[test]
fn splitter_drag_clamps_at_minimum_and_restores_exactly() {
    let ui = harness();
    hover_neutral(&ui);
    let before = snapshot(&ui);

    // Downward travel of 315px. The dock floors at 140px after 270px of
    // travel, so the drag must clamp with 45px left over. (The grid above
    // only grows, so the splitter's position stays exactly pointer-driven.)
    drag(ui.window(), (GRIP_X, GRIP_Y), (GRIP_X, GRIP_Y + 315.0));
    hover_neutral(&ui);
    assert_ne!(
        snapshot(&ui),
        before,
        "dragging past the bound must still resize the dock"
    );
    assert_eq!(
        ui.get_piano_dock_height(),
        140.0,
        "the dock must clamp at its 140px floor"
    );

    // The floored dock's top edge sits 410 - 140 = 270px lower than it
    // started. Exactly the clamped overshoot of upward travel must return
    // to 410px; any drift from the clamp re-anchoring breaks byte equality.
    drag(
        ui.window(),
        (GRIP_X, DOCK_TOP_Y + 270.0 - 3.0),
        (GRIP_X, GRIP_Y),
    );
    hover_neutral(&ui);
    let restored = ui.get_piano_dock_height();
    assert_eq!(
        restored, 410.0,
        "the dock property must return to the default after the clamped drag"
    );
    assert_eq!(
        first_diff(&before, &snapshot(&ui)),
        None,
        "restore after a clamped drag must be exact"
    );
}
