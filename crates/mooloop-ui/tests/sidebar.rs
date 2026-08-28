//! Tests for the browser sidebar: the status bar's hide/show toggle and the
//! left-edge resize grip.
//!
//! As with the dock splitter, gestures are dispatched as real pointer
//! events against the software renderer and compared through whole-window
//! snapshots, so a round trip must be byte-identical and a clamped drag
//! must restore exactly. The sidebar's x geometry hangs off one measured
//! constant, CONTENT_RIGHT (the right edge of the window's layout area,
//! where the sidebar meets the window edge; the window keeps small side
//! insets, so this is inside the 960px surface).

use mooloop_ui::MainWindow;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, SharedString};

const CONTENT_RIGHT: f32 = 952.0;
const DEFAULT_WIDTH: f32 = 260.0;
const BUTTON_X: f32 = CONTENT_RIGHT - 6.0 - 32.0;
const BUTTON_Y: f32 = 740.0;
const NEUTRAL: (f32, f32) = (300.0, 400.0);

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

fn move_pointer(window: &slint::Window, at: (f32, f32)) {
    window.dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(at.0, at.1),
    });
}

fn click(window: &slint::Window, at: (f32, f32)) {
    let pos = LogicalPosition::new(at.0, at.1);
    window.dispatch_event(WindowEvent::PointerMoved { position: pos });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: pos,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position: pos,
        button: PointerEventButton::Left,
    });
}

/// Press at `from`, travel to `to` in several steps the way a real pointer
/// would, then release. Multiple moves exercise the grip's moving-origin
/// drag arithmetic the way a single jump cannot.
fn drag(window: &slint::Window, from: (f32, f32), to: (f32, f32)) {
    move_pointer(window, from);
    window.dispatch_event(WindowEvent::PointerPressed {
        position: LogicalPosition::new(from.0, from.1),
        button: PointerEventButton::Left,
    });
    const STEPS: usize = 8;
    for i in 1..=STEPS {
        let t = i as f32 / STEPS as f32;
        window.dispatch_event(WindowEvent::PointerMoved {
            position: LogicalPosition::new(
                from.0 + (to.0 - from.0) * t,
                from.1 + (to.1 - from.1) * t,
            ),
        });
    }
    window.dispatch_event(WindowEvent::PointerReleased {
        position: LogicalPosition::new(to.0, to.1),
        button: PointerEventButton::Left,
    });
}

/// The grip's horizontal centre for a given sidebar width: it spans the 6px
/// ending 1px inside the sidebar's left edge.
fn grip_center_x(width: f32) -> f32 {
    CONTENT_RIGHT - width - 2.0
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
fn sidebar_toggles_from_the_status_bar_button() {
    let ui = harness();
    let window = ui.window();
    assert!(!ui.get_sidebar_visible(), "the sidebar starts hidden");

    move_pointer(window, NEUTRAL);
    let before = snapshot(&ui);

    click(window, (BUTTON_X, BUTTON_Y));
    assert!(ui.get_sidebar_visible(), "the button must show the sidebar");
    move_pointer(window, NEUTRAL);
    assert_ne!(
        first_diff(&before, &snapshot(&ui)),
        None,
        "showing the sidebar must change the layout"
    );

    click(window, (BUTTON_X, BUTTON_Y));
    assert!(
        !ui.get_sidebar_visible(),
        "the button must hide the sidebar again"
    );
    move_pointer(window, NEUTRAL);
    assert_eq!(
        first_diff(&before, &snapshot(&ui)),
        None,
        "hiding the sidebar must restore the exact layout"
    );
}

#[test]
fn sidebar_grip_resizes_and_restores_exactly() {
    let ui = harness();
    ui.set_sidebar_visible(true);
    let window = ui.window();
    move_pointer(window, NEUTRAL);
    let before = snapshot(&ui);
    assert_eq!(ui.get_sidebar_width(), DEFAULT_WIDTH);

    // Dragging the grip left widens the sidebar.
    let grip = (grip_center_x(DEFAULT_WIDTH), 400.0);
    drag(window, grip, (grip.0 - 24.0, 400.0));
    move_pointer(window, NEUTRAL);
    assert_eq!(
        ui.get_sidebar_width(),
        DEFAULT_WIDTH + 24.0,
        "dragging left must widen the sidebar"
    );
    assert_ne!(
        first_diff(&before, &snapshot(&ui)),
        None,
        "widening the sidebar must change the layout"
    );

    drag(window, (grip.0 - 24.0, 400.0), grip);
    move_pointer(window, NEUTRAL);
    assert_eq!(ui.get_sidebar_width(), DEFAULT_WIDTH);
    assert_eq!(
        first_diff(&before, &snapshot(&ui)),
        None,
        "the round trip must restore the exact layout"
    );
}

#[test]
fn sidebar_grip_clamps_at_floor_and_restores_exactly() {
    let ui = harness();
    ui.set_sidebar_visible(true);
    let window = ui.window();

    // Below the floor: 200 -> 30px of rightward drag clamps at 180 with a
    // 10px overshoot that the grip must swallow by re-anchoring. The
    // restore drag starts on the grip's post-clamp position, not where the
    // first drag's pointer happened to release.
    ui.set_sidebar_width(200.0);
    move_pointer(window, NEUTRAL);
    let narrow = snapshot(&ui);
    drag(
        window,
        (grip_center_x(200.0), 400.0),
        (grip_center_x(200.0) + 30.0, 400.0),
    );
    move_pointer(window, NEUTRAL);
    assert_eq!(
        ui.get_sidebar_width(),
        180.0,
        "the sidebar must clamp at its 180px floor"
    );
    drag(
        window,
        (grip_center_x(180.0), 400.0),
        (grip_center_x(200.0), 400.0),
    );
    move_pointer(window, NEUTRAL);
    assert_eq!(ui.get_sidebar_width(), 200.0);
    assert_eq!(
        first_diff(&narrow, &snapshot(&ui)),
        None,
        "restore from the floor must be exact"
    );
}

#[test]
fn sidebar_grip_clamps_at_ceiling_and_restores_exactly() {
    let ui = harness();
    // The 400px ceiling only fits when the window is wide enough: the work
    // surface's own content minimum (measured at ~608px on this machine's
    // 960px window) would otherwise pin the sidebar before the bound does.
    ui.window().set_size(LogicalSize::new(1100.0, 760.0));
    ui.set_sidebar_visible(true);
    let window = ui.window();
    const WIDE_RIGHT: f32 = 1092.0;
    let wide_grip_x = |width: f32| WIDE_RIGHT - width - 2.0;

    // 390 -> 30px of leftward drag clamps at 400 with a 20px overshoot.
    ui.set_sidebar_width(390.0);
    move_pointer(window, NEUTRAL);
    let wide = snapshot(&ui);
    drag(
        window,
        (wide_grip_x(390.0), 400.0),
        (wide_grip_x(390.0) - 30.0, 400.0),
    );
    move_pointer(window, NEUTRAL);
    assert_eq!(
        ui.get_sidebar_width(),
        400.0,
        "the sidebar must clamp at its 400px ceiling"
    );
    drag(
        window,
        (wide_grip_x(400.0), 400.0),
        (wide_grip_x(390.0), 400.0),
    );
    move_pointer(window, NEUTRAL);
    assert_eq!(ui.get_sidebar_width(), 390.0);
    assert_eq!(
        first_diff(&wide, &snapshot(&ui)),
        None,
        "restore from the ceiling must be exact"
    );
}

#[test]
fn hidden_sidebar_grip_is_inert() {
    let ui = harness();
    assert!(!ui.get_sidebar_visible());
    let window = ui.window();

    // Where the grip would be if the sidebar were visible at its default
    // width: a press-and-drag there must do nothing at all.
    let grip = (grip_center_x(DEFAULT_WIDTH), 400.0);
    drag(window, grip, (grip.0 - 24.0, 400.0));
    assert_eq!(ui.get_sidebar_width(), DEFAULT_WIDTH);
    assert!(!ui.get_sidebar_visible());
}

/// Diagnostic: reports where the status bar button and the grip actually
/// respond, so the measured constants above stay honest.
#[test]
#[ignore = "diagnostic probe; run with --ignored --nocapture"]
fn probe_locates_button_and_grip() {
    let ui = harness();
    let window = ui.window();

    for x in (840..956).step_by(8) {
        click(window, (x as f32, BUTTON_Y));
        if ui.get_sidebar_visible() {
            println!("button responds at x={x}");
            click(window, (x as f32, BUTTON_Y));
        }
    }

    ui.set_sidebar_visible(true);
    for x in (640..760).step_by(2) {
        ui.set_sidebar_width(DEFAULT_WIDTH);
        drag(window, (x as f32, 400.0), (x as f32 - 10.0, 400.0));
        if ui.get_sidebar_width() != DEFAULT_WIDTH {
            println!("grip responds at x={x}");
        }
    }

    ui.set_sidebar_width(390.0);
    move_pointer(window, NEUTRAL);
    let _ = snapshot(&ui);
    // Locate the grip at width 390 by its hover highlight: the splitter
    // line lights up Theme.focus (a bright yellow-green no other surface
    // uses) while the pointer rests on it.
    for x in (540..=900).step_by(2) {
        move_pointer(window, (x as f32, 400.0));
        let shot = snapshot(&ui);
        let lit = shot.as_chunks::<4>().0.iter().any(|px| {
            px[0] > 170 && px[1] > 230 && px[2] < 160
        });
        if lit {
            println!("grip highlight at width 390 responds near x={x}");
        }
    }
    move_pointer(window, NEUTRAL);
}
