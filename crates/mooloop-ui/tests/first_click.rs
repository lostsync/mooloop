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
    import { ParameterFader, ParameterKnob, ToolButton } from "../ui/controls.slint";

    export component ClickHarness inherits Window {
        width: 200px;
        height: 200px;
        callback button-clicked;
        in-out property <float> knob-value: 0.5;
        in-out property <float> fader-value: 0.5;

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

        ParameterFader {
            x: 0px; y: 170px;
            label: "Vol";
            value <=> root.fader-value;
            changed(v) => { root.fader-value = v; }
        }
    }

    // The shape `main.slint` has: one root FocusScope *surrounding* the UI,
    // not sitting beside it. Slint delivers a key to the focused item and
    // then walks `parent_item` towards the window
    // (`i-slint-core/window.rs`, "Deliver key_event ... going up towards the
    // window"), so a scope that is merely a sibling of the content only ever
    // sees a key while it personally holds focus. That is what made shortcuts
    // depend on having clicked a neutral background first.
    export component KeyHarness inherits Window {
        width: 200px;
        height: 200px;
        callback button-clicked;
        callback root-key(string);

        forward-focus: keys;
        keys := FocusScope {
            focus-on-click: false;
            key-pressed(e) => { root.root-key(e.text); accept }

            ToolButton {
                x: 0px; y: 0px; width: 100px; height: 40px;
                text: "Mute";
                clicked => { root.button-clicked(); }
            }
        }
    }
}

const BUTTON_CENTER: (f32, f32) = (50.0, 20.0);
/// Inside the knob dial itself, which sits above the label and readout.
const KNOB_DIAL: (f32, f32) = (28.0, 82.0);
const KNOB_LABEL: (f32, f32) = (28.0, 112.0);
const FADER_TRACK: (f32, f32) = (150.0, 181.0);

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

#[test]
fn knob_label_drags_the_parameter() {
    let ui = harness();
    let before = ui.get_knob_value();
    let window = ui.window();

    let start = pos(KNOB_LABEL);
    window.dispatch_event(WindowEvent::PointerMoved { position: start });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: start,
        button: PointerEventButton::Left,
    });
    let end = LogicalPosition::new(start.x, start.y - 30.0);
    window.dispatch_event(WindowEvent::PointerMoved { position: end });
    window.dispatch_event(WindowEvent::PointerReleased {
        position: end,
        button: PointerEventButton::Left,
    });

    assert!(
        ui.get_knob_value() > before,
        "dragging a knob label must change its value (was {before}, now {})",
        ui.get_knob_value()
    );
}

#[test]
fn fader_responds_to_the_first_click() {
    let ui = harness();
    let before = ui.get_fader_value();

    click_at(ui.window(), FADER_TRACK);

    assert!(
        ui.get_fader_value() > before,
        "clicking a fader must change its value on the first press (was {before}, now {})",
        ui.get_fader_value()
    );
}

/// Press and release a key the way a real keyboard would.
fn press_key(window: &slint::Window, text: &str) {
    window.dispatch_event(WindowEvent::KeyPressed { text: text.into() });
    window.dispatch_event(WindowEvent::KeyReleased { text: text.into() });
}

fn key_harness() -> KeyHarness {
    i_slint_backend_testing::init_no_event_loop();
    KeyHarness::new().unwrap()
}

/// The bug: Space is play/stop, but `ToolButton` used to accept it, and
/// `ToolButton` is what every `ToggleButton`, `SegmentedControl`, pane tab and
/// mute button in the application is built from. Clicking one left a caret on
/// it, so the next Space re-fired that button instead of reaching the
/// transport -- which is why a shortcut so often needed a click on a neutral
/// background first.
#[test]
fn space_reaches_the_root_after_clicking_a_button() {
    let ui = key_harness();
    let clicks = Rc::new(Cell::new(0u32));
    let keys: Rc<std::cell::RefCell<Vec<String>>> = Rc::new(std::cell::RefCell::new(Vec::new()));
    ui.on_button_clicked({
        let clicks = clicks.clone();
        move || clicks.set(clicks.get() + 1)
    });
    ui.on_root_key({
        let keys = keys.clone();
        move |text| keys.borrow_mut().push(text.to_string())
    });

    click_at(ui.window(), BUTTON_CENTER);
    assert_eq!(clicks.get(), 1, "the click itself must still fire the button");

    press_key(ui.window(), " ");

    assert_eq!(
        clicks.get(),
        1,
        "Space must not re-fire the button that was just clicked"
    );
    assert_eq!(
        keys.borrow().as_slice(),
        [" "],
        "Space must bubble past the focused button to the root scope, which is \
         where the action dispatcher lives"
    );
}

/// The other half of the same contract: a focused button is still operable
/// from the keyboard, so rejecting Space did not cost keyboard activation.
#[test]
fn enter_still_activates_a_focused_button() {
    let ui = key_harness();
    let clicks = Rc::new(Cell::new(0u32));
    ui.on_button_clicked({
        let clicks = clicks.clone();
        move || clicks.set(clicks.get() + 1)
    });

    click_at(ui.window(), BUTTON_CENTER);
    press_key(ui.window(), "\n");

    assert_eq!(clicks.get(), 2, "Enter must still activate a focused button");
}

/// A key the button does not want must reach the root from wherever focus
/// happens to be -- including from a knob, which is the case that used to
/// work only because the knob was a child of nothing that listened.
#[test]
fn an_unclaimed_key_reaches_the_root_without_clicking_the_background() {
    let ui = key_harness();
    let keys: Rc<std::cell::RefCell<Vec<String>>> = Rc::new(std::cell::RefCell::new(Vec::new()));
    ui.on_root_key({
        let keys = keys.clone();
        move |text| keys.borrow_mut().push(text.to_string())
    });

    click_at(ui.window(), BUTTON_CENTER);
    press_key(ui.window(), "s");

    assert_eq!(
        keys.borrow().as_slice(),
        ["s"],
        "an unclaimed key must bubble to the root scope"
    );
}
