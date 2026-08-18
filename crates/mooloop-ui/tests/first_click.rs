//! Regression test for the "every control needs two clicks" bug.
//!
//! A Slint `FocusScope` that does not yet have focus consumes the pointer
//! press that would focus it (`InputEventResult::EventAccepted`) and only
//! ignores presses once it is focused. Our controls stack a keyboard
//! `FocusScope` on top of the `TouchArea` that does the real work, so with
//! the default `focus-on-click: true` the first click on any control was
//! swallowed -- it drew the focus ring and nothing else -- and only the
//! second click reached the `TouchArea`.
//!
//! These tests drive real pointer events through the window so that
//! hit-testing and event routing are actually exercised. Note that
//! `invoke_accessible_default_action` would call the callback directly and
//! so would pass even with the bug present.
//!
//! The controls are given explicit geometry so the tests can click known
//! coordinates without needing the `ElementHandle` search API (which would
//! require building with `SLINT_EMIT_DEBUG_INFO=1`).

use slint::platform::{PointerEventButton, WindowEvent};
use slint::LogicalPosition;
use std::cell::Cell;
use std::rc::Rc;

slint::slint! {
    import { ToolButton, ParameterKnob } from "../ui/controls.slint";

    export component ClickHarness inherits Window {
        width: 200px;
        height: 200px;
        callback button-clicked;
        in-out property <float> knob-value: 0.5;

        ToolButton {
            x: 0px; y: 0px; width: 100px; height: 40px;
            text: "Load";
            clicked => { root.button-clicked(); }
        }

        ParameterKnob {
            x: 0px; y: 60px;
            label: "Cutoff";
            value <=> root.knob-value;
            changed(v) => { root.knob-value = v; }
        }
    }
}

const BUTTON_CENTER: (f32, f32) = (50.0, 20.0);
/// Inside the knob dial itself, which sits above the label and readout.
const KNOB_DIAL: (f32, f32) = (28.0, 82.0);

fn pos(p: (f32, f32)) -> LogicalPosition {
    LogicalPosition::new(p.0, p.1)
}

/// Press and release the left button at `p`, the way a real mouse would.
fn click_at(window: &slint::Window, p: (f32, f32)) {
    let position = pos(p);
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

fn harness() -> ClickHarness {
    i_slint_backend_testing::init_no_event_loop();
    ClickHarness::new().unwrap()
}

#[test]
fn tool_button_fires_on_the_first_click() {
    let ui = harness();
    let clicks = Rc::new(Cell::new(0u32));
    ui.on_button_clicked({
        let clicks = clicks.clone();
        move || clicks.set(clicks.get() + 1)
    });

    click_at(ui.window(), BUTTON_CENTER);
    assert_eq!(
        clicks.get(),
        1,
        "first click on a ToolButton must fire `clicked`"
    );

    click_at(ui.window(), BUTTON_CENTER);
    assert_eq!(clicks.get(), 2, "subsequent clicks must keep working");
}

#[test]
fn knob_responds_to_the_first_drag() {
    let ui = harness();
    let before = ui.get_knob_value();
    let window = ui.window();

    // Press and drag upwards with no prior "focusing" click.
    let start = pos(KNOB_DIAL);
    window.dispatch_event(WindowEvent::PointerMoved { position: start });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: start,
        button: PointerEventButton::Left,
    });
    let end = LogicalPosition::new(start.x, start.y - 40.0);
    window.dispatch_event(WindowEvent::PointerMoved { position: end });
    window.dispatch_event(WindowEvent::PointerReleased {
        position: end,
        button: PointerEventButton::Left,
    });

    assert!(
        ui.get_knob_value() > before,
        "dragging a knob must change its value on the first press (was {before}, now {})",
        ui.get_knob_value()
    );
}
