//! Regression tests for dragging handles whose own position is bound to the
//! value they control: the EQ band points and the reverb capture dot.
//!
//! Their `TouchArea` re-centres under the pointer every time the drag
//! callback updates the underlying value, so `mouse-x`/`mouse-y` are not a
//! stable, parent-relative coordinate system - see the fix on
//! eq-device.slint and reverb-device.slint (the same pitfall documented on
//! controls.slint's `MixerFader`). The old code mapped mouse position
//! directly and produced a recurrence that flips sign each event instead of
//! converging, which shows up here as a non-monotonic value sequence during
//! a drag that moves steadily in one direction - invoking the `*-changed`
//! callbacks directly would not catch this, since the bug is entirely in how
//! the `TouchArea`'s own coordinate system interacts with hit-testing.

use mooloop_ui::{EqDeviceDragHarness, ReverbDeviceDragHarness};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

/// Both faces are 3 rack units wide with 2 half-gaps between them, at the
/// standard face height - the same fixed size gallery.slint gives them.
const FACE_WIDTH: f32 = 220.0 * 3.0 + 4.0 * 2.0;
const FACE_HEIGHT: f32 = 268.0;
const HEADER_HEIGHT: f32 = 28.0;

/// Press at `from`, travel to `to` in small increments the way a real
/// pointer would, then release.
fn drag(window: &slint::Window, from: (f32, f32), to: (f32, f32), steps: usize) {
    let at = |p: (f32, f32)| LogicalPosition::new(p.0, p.1);
    window.dispatch_event(WindowEvent::PointerMoved { position: at(from) });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: at(from),
        button: PointerEventButton::Left,
    });
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let p = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        window.dispatch_event(WindowEvent::PointerMoved { position: at(p) });
    }
    window.dispatch_event(WindowEvent::PointerReleased {
        position: at(to),
        button: PointerEventButton::Left,
    });
}

/// A drag moving steadily in one direction must produce a value sequence
/// that moves the same direction throughout - a coordinate system that
/// isn't actually parent-relative shows up here as zig-zag instead.
fn assert_monotonic(values: &[f32], increasing: bool, what: &str) {
    assert!(values.len() >= 2, "{what}: drag produced no updates");
    for pair in values.windows(2) {
        if increasing {
            assert!(
                pair[1] >= pair[0] - 1e-4,
                "{what}: not monotonically increasing: {values:?}"
            );
        } else {
            assert!(
                pair[1] <= pair[0] + 1e-4,
                "{what}: not monotonically decreasing: {values:?}"
            );
        }
    }
}

#[test]
fn eq_point_drag_tracks_the_pointer() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = EqDeviceDragHarness::new().unwrap();
    ui.window()
        .set_size(LogicalSize::new(FACE_WIDTH, FACE_HEIGHT));

    // Band 0 active at the plot centre; the other six disabled so their
    // (irrelevant) hit areas can't shadow band 0's.
    let mut band_data = vec![0.0f32; 35];
    band_data[0] = 0.5; // frequency
    band_data[1] = 0.5; // gain
    band_data[2] = 0.707; // q
    band_data[3] = 1.0; // enabled
    band_data[4] = 0.0; // bell
    ui.set_band_data(ModelRc::from(Rc::new(VecModel::from(band_data))));

    let frequencies = Rc::new(RefCell::new(Vec::new()));
    let gains = Rc::new(RefCell::new(Vec::new()));
    ui.on_frequency_changed({
        let frequencies = frequencies.clone();
        move |v| frequencies.borrow_mut().push(v)
    });
    ui.on_gain_changed({
        let gains = gains.clone();
        move |v| gains.borrow_mut().push(v)
    });

    // Plot geometry from eq-device.slint's EqResponseDisplay: the face's
    // VerticalLayout has 6px padding (34px on top, for the header strip), a
    // 20px header row (FFT toggle), 4px spacing, then the 126px-tall display
    // filling the remaining width minus the 6px side padding.
    let plot_x = 6.0;
    let plot_y = HEADER_HEIGHT + 6.0 + 20.0 + 4.0;
    let plot_w = FACE_WIDTH - 12.0;
    let plot_h = 126.0;
    let start = (plot_x + 0.5 * plot_w, plot_y + 0.5 * plot_h);
    let dx = 100.0;
    let dy = 30.0;
    let end = (start.0 + dx, start.1 + dy);

    drag(ui.window(), start, end, 20);

    assert_monotonic(&frequencies.borrow(), true, "frequency");
    assert_monotonic(&gains.borrow(), false, "gain");
    let final_freq = *frequencies.borrow().last().unwrap();
    let final_gain = *gains.borrow().last().unwrap();
    assert!(
        (final_freq - (0.5 + dx / plot_w)).abs() < 0.02,
        "frequency should track the pointer 1:1: got {final_freq}"
    );
    assert!(
        (final_gain - (0.5 - dy / plot_h)).abs() < 0.02,
        "gain should track the pointer 1:1: got {final_gain}"
    );
}

#[test]
fn reverb_capture_drag_tracks_the_pointer() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = ReverbDeviceDragHarness::new().unwrap();
    ui.window()
        .set_size(LogicalSize::new(FACE_WIDTH, FACE_HEIGHT));

    let width_value = 0.4f32;
    let depth_value = 0.5f32;
    ui.set_width_value(width_value);
    ui.set_depth_value(depth_value);
    ui.set_capture_x(0.5);
    ui.set_capture_y(0.5);

    let xs = Rc::new(RefCell::new(Vec::new()));
    let ys = Rc::new(RefCell::new(Vec::new()));
    ui.on_capture_x_changed({
        let xs = xs.clone();
        move |v| xs.borrow_mut().push(v)
    });
    ui.on_capture_y_changed({
        let ys = ys.clone();
        move |v| ys.borrow_mut().push(v)
    });

    // Room geometry from reverb-device.slint's RoomPlan, replicated from its
    // own formulas (rather than hand-picked) so a future tweak to the room
    // sizing constants can't quietly desync this test's coordinates.
    let width_m = 2.0 * 15f32.powf(width_value);
    let depth_m = 2.0 * 25f32.powf(depth_value);
    let aspect = (width_m / depth_m).clamp(0.42, 2.25);
    let plan_w = 272.0f32;
    let plan_h = 116.0f32;
    let room_w = (plan_w - 12.0).min((plan_h - 12.0) * aspect);
    let room_h = (plan_h - 12.0).min((plan_w - 12.0) / aspect);
    let room_x = (plan_w - room_w) * 0.5;
    let room_y = (plan_h - room_h) * 0.5;

    // RoomPlan itself sits after the VerticalLayout's padding/header row (a
    // 22px SegmentedControl height) plus spacing, at the 6px left padding.
    let plan_x = 6.0;
    let plan_y = HEADER_HEIGHT + 6.0 + 22.0 + 4.0;
    let start = (
        plan_x + room_x + 0.5 * room_w,
        plan_y + room_y + 0.5 * room_h,
    );
    let dx = 20.0;
    let dy = -15.0;
    let end = (start.0 + dx, start.1 + dy);

    drag(ui.window(), start, end, 20);

    assert_monotonic(&xs.borrow(), true, "capture-x");
    assert_monotonic(&ys.borrow(), false, "capture-y");
    let final_x = *xs.borrow().last().unwrap();
    let final_y = *ys.borrow().last().unwrap();
    assert!(
        (final_x - (0.5 + dx / room_w)).abs() < 0.02,
        "capture-x should track the pointer 1:1: got {final_x}"
    );
    assert!(
        (final_y - (0.5 + dy / room_h)).abs() < 0.02,
        "capture-y should track the pointer 1:1: got {final_y}"
    );
}
