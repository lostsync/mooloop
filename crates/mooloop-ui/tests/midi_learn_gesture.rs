//! What pressing a parameter control does while MIDI Learn is armed.
//!
//! The learn gesture rides on `modulation-edit-started`, the one callback
//! every device face carries that knows which parameter was pressed. That is
//! a deliberate overload and it has a failure mode worth a test: if the arm
//! did not also suppress the ordinary gesture, naming a knob would *move* it,
//! and the mapping would land on a parameter the press had just changed.
//! `CONTROL_SURFACES.md` says a learn gesture does not hand the knob's
//! position to the parameter; this is where that is checked.
//!
//! Driven through the real `TouchArea` rather than by invoking the callbacks,
//! because the whole of the change is in which branch a pointer event takes.

use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize};
use std::cell::RefCell;
use std::rc::Rc;

mod common;

slint::slint! {
    import { ControlAssign, MiniKnob, ParameterKnob } from "../ui/controls.slint";

    export { ControlAssign }

    export component LearnHarness inherits Window {
        in-out property <float> value: 0.5;
        in-out property <float> mini-value: 0.5;
        callback changed(float);
        callback assign-started();
        callback edit-started();

        width: 200px;
        height: 120px;

        HorizontalLayout {
            ParameterKnob {
                width: 100px;
                height: 120px;
                label: "Cutoff";
                value <=> root.value;
                changed(v) => { root.changed(v); }
                modulation-edit-started => { root.assign-started(); }
            }
            MiniKnob {
                width: 100px;
                height: 120px;
                label: "Trim";
                value <=> root.mini-value;
                changed(v) => { root.changed(v); }
                modulation-edit-started => { root.assign-started(); }
                edit-started => { root.edit-started(); }
            }
        }
    }
}

/// Press at `from`, travel to `to`, release -- a drag that would move an
/// unarmed knob a long way.
fn drag(window: &slint::Window, from: (f32, f32), to: (f32, f32)) {
    let at = |p: (f32, f32)| LogicalPosition::new(p.0, p.1);
    window.dispatch_event(WindowEvent::PointerMoved { position: at(from) });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: at(from),
        button: PointerEventButton::Left,
    });
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        let p = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        window.dispatch_event(WindowEvent::PointerMoved { position: at(p) });
    }
    window.dispatch_event(WindowEvent::PointerReleased {
        position: at(to),
        button: PointerEventButton::Left,
    });
}

struct Seen {
    changed: RefCell<Vec<f32>>,
    assign: Rc<RefCell<usize>>,
    edit: Rc<RefCell<usize>>,
}

fn harness() -> (LearnHarness, Rc<Seen>) {
    common::install_testing_backend();
    let ui = LearnHarness::new().unwrap();
    ui.window().set_size(LogicalSize::new(200.0, 120.0));
    let seen = Rc::new(Seen {
        changed: RefCell::new(Vec::new()),
        assign: Rc::new(RefCell::new(0)),
        edit: Rc::new(RefCell::new(0)),
    });
    {
        let seen = seen.clone();
        ui.on_changed(move |v| seen.changed.borrow_mut().push(v));
    }
    {
        let seen = seen.clone();
        ui.on_assign_started(move || *seen.assign.borrow_mut() += 1);
    }
    {
        let seen = seen.clone();
        ui.on_edit_started(move || *seen.edit.borrow_mut() += 1);
    }
    (ui, seen)
}

/// The control the pointer lands on, at the harness's size: the big knob's
/// face is the left half, the mini knob's the right.
const BIG_KNOB: (f32, f32) = (50.0, 60.0);
const MINI_KNOB: (f32, f32) = (150.0, 60.0);

/// With the arm off, a drag is an ordinary edit: the value moves and nothing
/// reports an assignment.
#[test]
fn an_unarmed_drag_still_moves_the_knob() {
    let (ui, seen) = harness();
    ui.global::<ControlAssign>().set_midi_learn(false);
    drag(ui.window(), BIG_KNOB, (50.0, 20.0));

    assert!(
        !seen.changed.borrow().is_empty(),
        "an unarmed drag reported no value change, so this test is not \
         reaching the knob and the armed case below proves nothing"
    );
    assert_eq!(*seen.assign.borrow(), 0, "nothing was being assigned");
    assert!(ui.get_value() > 0.5, "the knob moved up: {}", ui.get_value());
}

/// Armed, the same drag names the control and leaves it exactly where it was.
#[test]
fn an_armed_drag_names_the_control_without_moving_it() {
    let (ui, seen) = harness();
    ui.global::<ControlAssign>().set_midi_learn(true);
    drag(ui.window(), BIG_KNOB, (50.0, 20.0));

    assert_eq!(
        *seen.assign.borrow(),
        1,
        "the press should have named the parameter once"
    );
    assert!(
        seen.changed.borrow().is_empty(),
        "naming a control moved it: {:?}. A mapping would then land on a \
         parameter the naming press had just changed.",
        seen.changed.borrow()
    );
    assert_eq!(ui.get_value(), 0.5, "the value is untouched");
}

/// A `MiniKnob` bounds its own drags with `edit-started`/`edit-finished` so a
/// caller can coalesce one drag into one undo step. Naming it must not open
/// one of those: there is nothing to undo, and the entry would sit in the
/// history describing an edit that never happened.
#[test]
fn naming_a_mini_knob_opens_no_edit_gesture() {
    let (ui, seen) = harness();
    ui.global::<ControlAssign>().set_midi_learn(false);
    drag(ui.window(), MINI_KNOB, (150.0, 20.0));
    assert_eq!(
        *seen.edit.borrow(),
        1,
        "an unarmed mini-knob drag opens an edit gesture"
    );

    let (ui, seen) = harness();
    ui.global::<ControlAssign>().set_midi_learn(true);
    drag(ui.window(), MINI_KNOB, (150.0, 20.0));
    assert_eq!(*seen.assign.borrow(), 1, "the press named the parameter");
    assert_eq!(
        *seen.edit.borrow(),
        0,
        "naming a control opened an undo gesture"
    );
    assert!(seen.changed.borrow().is_empty(), "naming a control moved it");
    assert_eq!(ui.get_mini_value(), 0.5, "the value is untouched");
}
