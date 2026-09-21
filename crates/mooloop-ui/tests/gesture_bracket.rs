//! Where a value gesture begins and ends, driven by real pointer events.
//!
//! `docs/plans/gesture-undo/` step 02. Undo installs a whole-project
//! snapshot, so an edit that never reached the history is not merely
//! un-undoable -- it is destroyed by the next Ctrl+Z. The fix is one entry
//! per gesture, and only the control knows where a gesture is: a fader emits
//! a value on every pointer frame, and nothing downstream can tell the last
//! frame of a drag from the first frame of the next one.
//!
//! It used to guess. `with_continuous_history` treated move frames arriving
//! within 400 ms as one drag, which worked and was a heuristic sitting where
//! the markup already knew the answer. This is the answer: `Gesture.begin()`
//! on the press, `Gesture.end()` on the release, however many frames came
//! between.
//!
//! **A markup assertion could not have caught the failure this guards.**
//! The fader's `changed` callback was correct the whole time the drag was
//! recorded as forty undo steps -- the right value, every frame, to the right
//! handler. Only a click can tell a bracket that fires from one that does
//! not, which is `add_source_menu.rs`'s lesson arriving at a different
//! control.
//!
//! Coordinates are computed rather than searched, for the reason
//! `first_click.rs` records: the `ElementHandle` search API needs a build
//! with debug info, and this harness has fixed geometry anyway.

use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize};
use std::cell::RefCell;
use std::rc::Rc;

slint::slint! {
    import { Gesture, MixerFader, MiniKnob } from "../ui/controls.slint";

    export { Gesture }

    export component GestureHarness inherits Window {
        width: 200px;
        height: 200px;
        background: #101010;
        callback fader-changed(float);
        callback knob-changed(float);

        MixerFader {
            x: 0px;
            y: 0px;
            height: 120px;
            changed(v) => { root.fader-changed(v); }
        }
        MiniKnob {
            x: 100px;
            y: 0px;
            changed(v) => { root.knob-changed(v); }
        }
    }
}

/// Inside the fader: it is 30px wide and 120px tall at the window's origin.
const FADER: (f32, f32) = (15.0, 60.0);
/// Inside the knob: 22px square at x = 100.
const KNOB: (f32, f32) = (111.0, 11.0);

#[derive(Default)]
struct Log {
    begins: usize,
    ends: usize,
    values: usize,
}

fn harness() -> (GestureHarness, Rc<RefCell<Log>>) {
    i_slint_backend_testing::init_no_event_loop();
    let ui = GestureHarness::new().expect("harness");
    ui.window().set_size(LogicalSize::new(200.0, 200.0));
    let log = Rc::new(RefCell::new(Log::default()));
    {
        let log = log.clone();
        ui.global::<Gesture>().on_begin(move || log.borrow_mut().begins += 1);
    }
    {
        let log = log.clone();
        ui.global::<Gesture>().on_end(move || log.borrow_mut().ends += 1);
    }
    {
        let log = log.clone();
        ui.on_fader_changed(move |_| log.borrow_mut().values += 1);
    }
    {
        let log = log.clone();
        ui.on_knob_changed(move |_| log.borrow_mut().values += 1);
    }
    (ui, log)
}

fn press(ui: &GestureHarness, at: (f32, f32)) {
    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(at.0, at.1),
    });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: LogicalPosition::new(at.0, at.1),
        button: PointerEventButton::Left,
    });
}

fn drag_to(ui: &GestureHarness, at: (f32, f32)) {
    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(at.0, at.1),
    });
}

fn release(ui: &GestureHarness, at: (f32, f32)) {
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: LogicalPosition::new(at.0, at.1),
        button: PointerEventButton::Left,
    });
}

/// **One drag, one bracket, however many frames it took.** The frames in
/// between are the whole point: each one is a value the old code would have
/// snapshotted the project for.
#[test]
fn a_fader_drag_is_one_begin_and_one_end() {
    let (ui, log) = harness();
    press(&ui, FADER);
    for step in 1..=20 {
        drag_to(&ui, (FADER.0, FADER.1 - step as f32 * 2.0));
    }
    release(&ui, (FADER.0, FADER.1 - 40.0));

    let log = log.borrow();
    assert_eq!(log.begins, 1, "the press did not open exactly one gesture");
    assert_eq!(log.ends, 1, "the release did not close exactly one gesture");
    assert!(
        log.values > 1,
        "the drag reported {} values; this test proves nothing unless it is many",
        log.values
    );
}

/// Two grabs are two entries. This is the case the 400 ms timer got wrong:
/// it measured the gap between move frames, so two grabs of the same fader
/// less than 400 ms apart collapsed into one undo step. The bracket gets it
/// right by being told rather than by timing anything, so the grabs here are
/// back to back.
#[test]
fn two_grabs_are_two_gestures() {
    let (ui, log) = harness();
    // Two *different* places on the fader, which is not incidental: a second
    // press at the same point is a double-click, and `MixerFader` brackets
    // its reset separately -- see the test below.
    for at in [FADER, (FADER.0, FADER.1 - 40.0)] {
        press(&ui, at);
        drag_to(&ui, (at.0, at.1 - 10.0));
        release(&ui, (at.0, at.1 - 10.0));
    }
    let log = log.borrow();
    assert_eq!(log.begins, 2);
    assert_eq!(log.ends, 2);
}

/// **A double-click reset is three brackets and one undo entry**, and the
/// difference between those two numbers is the recorder's rule rather than
/// the markup's.
///
/// A double-click is two presses and then `double-clicked`, so the widget
/// opens three gestures: two that move nothing and one that resets the
/// value. A gesture that changed nothing records nothing
/// (`Session::finish_gesture`), so what reaches the history is one entry.
/// Asserted here because the first version of `two_grabs_are_two_gestures`
/// counted three begins and read it as a bug in the widget.
#[test]
fn a_double_click_reset_brackets_itself() {
    let (ui, log) = harness();
    for _ in 0..2 {
        press(&ui, FADER);
        release(&ui, FADER);
    }
    let log = log.borrow();
    assert_eq!(
        log.begins, log.ends,
        "every gesture the double-click opened was closed"
    );
    assert!(
        log.begins >= 1,
        "the double-click reported no gesture at all"
    );
    assert_eq!(log.values, 1, "the reset is one value change, not three");
}

/// `MiniKnob` carries the pair already, ungated, and the mixer's pan knobs
/// needed wiring rather than a new callback. This is that wiring asserted
/// from the other side of it.
#[test]
fn a_knob_drag_is_one_begin_and_one_end() {
    let (ui, log) = harness();
    press(&ui, KNOB);
    for step in 1..=10 {
        drag_to(&ui, (KNOB.0, KNOB.1 - step as f32));
    }
    release(&ui, (KNOB.0, KNOB.1 - 10.0));

    let log = log.borrow();
    assert_eq!(log.begins, 1, "the press did not open exactly one gesture");
    assert_eq!(log.ends, 1, "the release did not close exactly one gesture");
}
