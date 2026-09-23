//! The in-app question (MOO-91), pressed through the real window.
//!
//! Unsaved changes, a preset name that is taken and a kit that drops
//! channels are asked in `QuestionDialog` rather than by a desktop dialog
//! program: a program that would not start read as Cancel, so with no zenity
//! a song with unsaved changes could not be quit at all. This presses each of
//! the three buttons with real pointer events, the way `menubar.rs` does,
//! because a test that invoked `question-answered` itself would pass with a
//! dialog whose buttons were dead.
//!
//! The card is a fixed 460 x 160 centred in the window (`question-dialog.slint`
//! says why), so in a 960 x 760 window it spans x 250..710 and y 300..460,
//! and its buttons sit on the row y 412..442 inside its 18px padding: Don't
//! Save at the left edge, Cancel and the primary button at the right.

use mooloop_ui::MainWindow;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize};
use std::cell::RefCell;
use std::rc::Rc;

const BUTTON_Y: f32 = 427.0;
const SECONDARY_X: f32 = 323.0;
const CANCEL_X: f32 = 532.0;
const PRIMARY_X: f32 = 637.0;

fn harness() -> (MainWindow, Rc<RefCell<Vec<i32>>>) {
    i_slint_backend_testing::init_no_event_loop();
    let ui = MainWindow::new().unwrap();
    ui.window().set_size(LogicalSize::new(960.0, 760.0));
    let answers = Rc::new(RefCell::new(Vec::new()));
    ui.on_question_answered({
        let answers = answers.clone();
        move |answer| answers.borrow_mut().push(answer)
    });
    (ui, answers)
}

fn ask(ui: &MainWindow) {
    ui.set_question_title("Save changes to \"beat\" before quitting?".into());
    ui.set_question_detail("If you don't save, the changes since the last save are lost.".into());
    ui.set_question_primary("Save".into());
    ui.set_question_secondary("Don't Save".into());
    ui.set_question_open(true);
}

fn click(window: &slint::Window, x: f32, y: f32) {
    let position = LogicalPosition::new(x, y);
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

/// **Each of the three answers reaches Rust, and closes the dialog first**,
/// so whatever the answer starts may ask the next question.
#[test]
fn each_button_answers_and_closes_the_question() {
    let (ui, answers) = harness();
    for (x, expected) in [(PRIMARY_X, 1), (SECONDARY_X, 2), (CANCEL_X, 0)] {
        ask(&ui);
        click(ui.window(), x, BUTTON_Y);
        assert_eq!(answers.borrow().last(), Some(&expected), "button at x={x}");
        assert!(!ui.get_question_open(), "answering {expected} leaves the question up");
    }
    assert_eq!(*answers.borrow(), [1, 2, 0]);
}

/// A two-answer question has no third button: where Don't Save would be,
/// a press lands on the scrim and answers nothing.
#[test]
fn a_question_without_a_secondary_answer_has_no_third_button() {
    let (ui, answers) = harness();
    ask(&ui);
    ui.set_question_secondary("".into());
    click(ui.window(), SECONDARY_X, BUTTON_Y);
    assert!(answers.borrow().is_empty(), "{:?}", answers.borrow());
    assert!(ui.get_question_open());
    click(ui.window(), PRIMARY_X, BUTTON_Y);
    assert_eq!(*answers.borrow(), [1]);
}

/// While the question is up, the window behind it cannot be pressed: the
/// scrim takes the click, so File > Save cannot start under it.
#[test]
fn the_window_behind_the_question_is_not_pressed() {
    let (ui, answers) = harness();
    let saved = Rc::new(RefCell::new(false));
    ui.on_save_song({
        let saved = saved.clone();
        move || *saved.borrow_mut() = true
    });
    ask(&ui);
    // The File title, then where its Save row would drop down.
    click(ui.window(), 30.0, 20.0);
    click(ui.window(), 60.0, 100.0);
    assert!(!*saved.borrow(), "a press went through the question");
    assert!(answers.borrow().is_empty());
}
