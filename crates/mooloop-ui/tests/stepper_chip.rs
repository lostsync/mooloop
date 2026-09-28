//! `StepperChip`, the `◂ GRIP ▸` voicing chip (MOO-295), driven by real
//! pointer events.
//!
//! What matters about it is what its caller can rely on: an arrow or a wheel
//! notch reports the neighbouring index, nothing is reported past either end,
//! and the chip never moves itself -- it is controlled (MOO-220), so the Bus
//! Comp's undo can move it. And it is as wide as its longest option whichever
//! one it shows, so it does not jump as it steps.
//!
//! Coordinates are computed, for `picker_chip.rs`'s reason: the chip's
//! geometry is fixed, and the search API needs debug info. Text is measured
//! in the testing backend's fixed font, so the widths do not depend on what
//! fonts the machine running the test has.

use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

slint::slint! {
    import { StepperChip } from "../ui/controls.slint";

    export component StepperHarness inherits Window {
        width: 300px;
        height: 60px;
        background: #101010;
        // The testing backend's fixed font: every byte one pixel-size wide,
        // so a label's width is known without a font on the machine.
        default-font-family: "FixedTestFont";
        in property <[string]> options;
        in property <int> selected-index;
        in property <bool> enabled: true;
        out property <length> chip-width: chip.width;
        callback selected(int);

        chip := StepperChip {
            x: 0px;
            y: 0px;
            width: self.preferred-width;
            height: 22px;
            options: root.options;
            selected-index: root.selected-index;
            enabled: root.enabled;
            hint: "The voicing";
            selected(i) => { root.selected(i); }
        }
    }
}

const MIDDLE_Y: f32 = 11.0;
/// The arrows are 16px wide inside a 1px padding.
const PREVIOUS_X: f32 = 9.0;
fn next_x(ui: &StepperHarness) -> f32 {
    ui.get_chip_width() - 9.0
}

fn harness(options: &[&str]) -> (StepperHarness, Rc<RefCell<Vec<i32>>>) {
    i_slint_backend_testing::init_no_event_loop();
    another(options)
}

/// A second chip in a test that already has one: the platform is set once
/// per thread.
fn another(options: &[&str]) -> (StepperHarness, Rc<RefCell<Vec<i32>>>) {
    let ui = StepperHarness::new().unwrap();
    ui.window().set_size(LogicalSize::new(300.0, 60.0));
    ui.set_options(ModelRc::from(Rc::new(VecModel::from(
        options.iter().map(|o| SharedString::from(*o)).collect::<Vec<_>>(),
    ))));
    // A repeater over a model set from Rust is built on the next pass over
    // the item tree, not when the model is set, so until then the chip
    // measures no options and sits at its minimum width. A render or any
    // pointer event makes that pass; this is one, off the chip.
    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(299.0, 59.0),
    });
    let reports = Rc::new(RefCell::new(Vec::new()));
    let sink = reports.clone();
    ui.on_selected(move |i| sink.borrow_mut().push(i));
    (ui, reports)
}

fn click(window: &slint::Window, at: (f32, f32)) {
    let position = LogicalPosition::new(at.0, at.1);
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

fn scroll(window: &slint::Window, at: (f32, f32), delta_y: f32) {
    let position = LogicalPosition::new(at.0, at.1);
    window.dispatch_event(WindowEvent::PointerMoved { position });
    window.dispatch_event(WindowEvent::PointerScrolled { position, delta_x: 0.0, delta_y });
}

const VOICINGS: [&str; 3] = ["GRIP", "PUNCH", "TUBE"];

#[test]
fn the_arrows_report_the_neighbouring_index() {
    let (ui, reports) = harness(&VOICINGS);
    ui.set_selected_index(1);
    click(ui.window(), (next_x(&ui), MIDDLE_Y));
    click(ui.window(), (PREVIOUS_X, MIDDLE_Y));
    assert_eq!(*reports.borrow(), vec![2, 0]);
}

/// Controlled: a click reports and the chip stays where its owner put it, so
/// a second click with nothing republished asks for the same index again.
#[test]
fn a_step_does_not_move_the_chip_itself() {
    let (ui, reports) = harness(&VOICINGS);
    ui.set_selected_index(0);
    click(ui.window(), (next_x(&ui), MIDDLE_Y));
    click(ui.window(), (next_x(&ui), MIDDLE_Y));
    assert_eq!(*reports.borrow(), vec![1, 1]);
    // The owner republishes -- an edit, or an undo -- and the chip follows.
    ui.set_selected_index(2);
    click(ui.window(), (PREVIOUS_X, MIDDLE_Y));
    assert_eq!(*reports.borrow(), vec![1, 1, 1]);
}

/// It stops at the ends rather than wrapping.
#[test]
fn nothing_is_reported_past_either_end() {
    let (ui, reports) = harness(&VOICINGS);
    ui.set_selected_index(0);
    click(ui.window(), (PREVIOUS_X, MIDDLE_Y));
    scroll(ui.window(), (40.0, MIDDLE_Y), -1.0);
    ui.set_selected_index(2);
    click(ui.window(), (next_x(&ui), MIDDLE_Y));
    scroll(ui.window(), (40.0, MIDDLE_Y), 1.0);
    assert!(reports.borrow().is_empty(), "stepped past an end: {:?}", reports.borrow());
}

/// The wheel anywhere over the chip steps, up for the next option, as the
/// toolbar's `StepperField` counts up.
#[test]
fn the_wheel_steps() {
    let (ui, reports) = harness(&VOICINGS);
    ui.set_selected_index(1);
    let label = (ui.get_chip_width() / 2.0, MIDDLE_Y);
    scroll(ui.window(), label, 1.0);
    scroll(ui.window(), label, -1.0);
    assert_eq!(*reports.borrow(), vec![2, 0]);
}

#[test]
fn a_disabled_chip_reports_nothing() {
    let (ui, reports) = harness(&VOICINGS);
    ui.set_selected_index(1);
    ui.set_enabled(false);
    click(ui.window(), (next_x(&ui), MIDDLE_Y));
    click(ui.window(), (PREVIOUS_X, MIDDLE_Y));
    scroll(ui.window(), (40.0, MIDDLE_Y), 1.0);
    assert!(reports.borrow().is_empty());
}

/// As wide as its longest option whichever it shows, and wider than a chip
/// whose options are all short: the width is measured, not pinned.
#[test]
fn the_width_is_the_longest_options_and_does_not_jump() {
    let (ui, _) = harness(&VOICINGS);
    let widths: Vec<f32> = (0..3)
        .map(|i| {
            ui.set_selected_index(i);
            ui.get_chip_width()
        })
        .collect();
    assert!(widths.iter().all(|w| *w == widths[0]), "the chip jumps as it steps: {widths:?}");

    let (short, _) = another(&["A", "B", "C"]);
    assert!(
        short.get_chip_width() < widths[0],
        "a chip of one-letter options ({}) is not narrower than PUNCH's ({})",
        short.get_chip_width(),
        widths[0]
    );
}
