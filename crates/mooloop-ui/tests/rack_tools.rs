//! Tests for the rack grid's cursor tools.
//!
//! The grid deliberately uses ONE hit area for a channel's whole run of steps
//! and derives the cell from mouse-x. Per-cell `TouchArea`s cannot support a
//! drag at all: the cell where the press lands keeps the pointer grab, so the
//! pointer never reaches its neighbours and paint/stretch are impossible.
//!
//! These tests dispatch real pointer events so that hit-testing and the
//! step-from-x arithmetic are actually exercised; invoking the callbacks
//! directly would pass even if the grid stopped routing events entirely.

use mooloop_ui::{ChannelRow, MainWindow, StepCell};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

/// Geometry of the first rack row, in logical pixels. The grid begins after the
/// row's name, mute button and volume/pan knobs, and each cell is 24px wide
/// with a 3px gap. These move if that prefix is resized or the bar above the
/// rack (menu bar + toolbar) changes height.
const GRID_ORIGIN_X: f32 = 192.0;
const CELL_PITCH: f32 = 27.0;
const ROW_CENTRE_Y: f32 = 121.0;
const STEPS: usize = 8;

/// Centre of the given step cell.
fn step_x(step: usize) -> f32 {
    GRID_ORIGIN_X + step as f32 * CELL_PITCH + 12.0
}

fn harness() -> MainWindow {
    i_slint_backend_testing::init_no_event_loop();
    let ui = MainWindow::new().unwrap();
    ui.window().set_size(LogicalSize::new(960.0, 760.0));
    ui.set_pattern_length(STEPS as i32);
    let steps: Vec<StepCell> = (0..STEPS)
        .map(|_| StepCell {
            active: false,
            velocity: 0,
            substeps: 0,
            onsets: 0,
        })
        .collect();
    ui.set_channels(ModelRc::from(Rc::new(VecModel::from(vec![ChannelRow {
        name: SharedString::from("Sampler 1"),
        muted: false,
        volume: 0.8,
        pan: 0.0,
        selected: true,
        bus: 0,
        steps: ModelRc::from(Rc::new(VecModel::from(steps))),
    }]))));
    ui
}

/// Press at `from`, travel to `to` in small increments the way a real pointer
/// would, then release.
fn drag(window: &slint::Window, from: f32, to: f32, button: PointerEventButton) {
    let at = |x: f32| LogicalPosition::new(x, ROW_CENTRE_Y);
    window.dispatch_event(WindowEvent::PointerMoved { position: at(from) });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: at(from),
        button,
    });
    const INCREMENTS: usize = 24;
    for i in 1..=INCREMENTS {
        let x = from + (to - from) * i as f32 / INCREMENTS as f32;
        window.dispatch_event(WindowEvent::PointerMoved { position: at(x) });
    }
    window.dispatch_event(WindowEvent::PointerReleased {
        position: at(to),
        button,
    });
}

fn click(window: &slint::Window, x: f32, button: PointerEventButton) {
    drag(window, x, x, button);
}

#[test]
fn paint_drag_fills_every_step_it_crosses() {
    let ui = harness();
    ui.set_tool_mode(1);
    let painted = Rc::new(RefCell::new(Vec::new()));
    ui.on_step_painted({
        let painted = painted.clone();
        move |channel, step, on| painted.borrow_mut().push((channel, step, on))
    });

    drag(ui.window(), step_x(0), step_x(4), PointerEventButton::Left);

    let painted = painted.borrow();
    assert_eq!(
        painted.iter().map(|e| e.1).collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4],
        "a paint drag must report every step it crosses, once each"
    );
    assert!(
        painted.iter().all(|e| e.2 && e.0 == 0),
        "a left drag paints the whole run on, on one channel: {painted:?}"
    );
}

#[test]
fn paint_drag_sets_one_state_instead_of_toggling() {
    let ui = harness();
    ui.set_tool_mode(1);
    let painted = Rc::new(RefCell::new(Vec::new()));
    ui.on_step_painted({
        let painted = painted.clone();
        move |_, step, on| painted.borrow_mut().push((step, on))
    });

    // A right drag clears. If the grid toggled per cell instead of setting a
    // target state, a drag across a mixed row would invert it rather than
    // clear it, which is the whole reason paint reports `on`.
    drag(ui.window(), step_x(5), step_x(2), PointerEventButton::Right);

    let painted = painted.borrow();
    assert!(!painted.is_empty(), "a right drag must report something");
    assert!(
        painted.iter().all(|e| !e.1),
        "every step in a right drag clears: {painted:?}"
    );
    assert_eq!(
        painted.iter().map(|e| e.0).collect::<Vec<_>>(),
        vec![5, 4, 3, 2],
        "dragging leftwards walks back down the row"
    );
}

#[test]
fn select_mode_toggles_a_single_step() {
    let ui = harness();
    ui.set_tool_mode(0);
    let clicked = Rc::new(RefCell::new(Vec::new()));
    let painted = Rc::new(RefCell::new(Vec::new()));
    ui.on_step_clicked({
        let clicked = clicked.clone();
        move |channel, step| clicked.borrow_mut().push((channel, step))
    });
    ui.on_step_painted({
        let painted = painted.clone();
        move |_, step, _| painted.borrow_mut().push(step)
    });

    click(ui.window(), step_x(3), PointerEventButton::Left);

    assert_eq!(*clicked.borrow(), vec![(0, 3)]);
    assert!(
        painted.borrow().is_empty(),
        "select mode must not paint: {:?}",
        painted.borrow()
    );
}

#[test]
fn slice_mode_ratchets_the_clicked_step() {
    let ui = harness();
    ui.set_tool_mode(2);
    ui.set_slice_divisions(3);
    let sliced = Rc::new(RefCell::new(Vec::new()));
    ui.on_step_sliced({
        let sliced = sliced.clone();
        move |channel, step, divisions| sliced.borrow_mut().push((channel, step, divisions))
    });

    click(ui.window(), step_x(2), PointerEventButton::Left);

    assert_eq!(
        *sliced.borrow(),
        vec![(0, 2, 3)],
        "slice reports the step and the division count from the toolbar"
    );
}

#[test]
fn stretch_drag_reports_length_from_its_origin() {
    let ui = harness();
    ui.set_tool_mode(3);
    let lengths = Rc::new(RefCell::new(Vec::new()));
    ui.on_step_length_dragged({
        let lengths = lengths.clone();
        move |channel, step, length| lengths.borrow_mut().push((channel, step, length))
    });

    drag(ui.window(), step_x(1), step_x(4), PointerEventButton::Left);

    let lengths = lengths.borrow();
    assert!(!lengths.is_empty(), "a stretch drag must report a length");
    assert!(
        lengths.iter().all(|e| e.0 == 0 && e.1 == 1),
        "every report belongs to the step the drag started on: {lengths:?}"
    );
    assert_eq!(
        lengths.last().unwrap().2,
        4,
        "dragging from step 1 to step 4 is a four-step note"
    );
    // The length must never run backwards past its own origin.
    assert!(lengths.iter().all(|e| e.2 >= 1), "{lengths:?}");
}

#[test]
fn a_drag_past_the_end_of_the_row_stays_in_range() {
    let ui = harness();
    ui.set_tool_mode(1);
    let painted = Rc::new(RefCell::new(Vec::new()));
    ui.on_step_painted({
        let painted = painted.clone();
        move |_, step, _| painted.borrow_mut().push(step)
    });

    // Well past the last cell: the grid must clamp rather than report a step
    // that does not exist in the model.
    drag(
        ui.window(),
        step_x(6),
        step_x(STEPS) + 400.0,
        PointerEventButton::Left,
    );

    let painted = painted.borrow();
    assert!(
        painted.iter().all(|&s| s >= 0 && (s as usize) < STEPS),
        "steps must stay inside the pattern: {painted:?}"
    );
    assert_eq!(
        painted.last().copied(),
        Some(STEPS as i32 - 1),
        "the drag clamps to the final step"
    );
}
