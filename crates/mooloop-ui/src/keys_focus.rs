//! The root shortcut scope keeps the keyboard (MOO-289).
//!
//! Every shortcut is decoded by one `FocusScope`, `keys` in `main.slint`,
//! which surrounds the whole window. It hears a key when it holds the focus
//! itself or when the focus is anywhere inside it: a focused knob or button
//! rejects the keys it has no use for and they bubble up. What it cannot hear
//! is a key sent while the window's focus is **nowhere**, and Slint 1.18.1
//! leaves it nowhere in more ways than one:
//!
//! - a text field that is done calls `clear-focus()` -- Enter or Escape in a
//!   rename field, a knob's typed value;
//! - a dialog's own scope (`QuestionDialog`, `TakesDialog`) takes the focus on
//!   opening, and hiding the dialog drops it, because an item that stops
//!   being visible loses the focus rather than handing it on;
//! - a focused knob or button whose pane is hidden, or whose face is rebuilt
//!   (another channel, another source), goes the same way, or is destroyed
//!   with the focus still pointing at it;
//! - a key that finds the focus on an item that is no longer visible is
//!   dropped, and the focus with it (`i-slint-core` `window.rs`,
//!   `process_key_input`).
//!
//! From there nothing reached `keys` until a click focused something inside
//! it, and only the piano roll's `focus-requested` gave it back directly --
//! Adam's "I have to click on the piano roll once before they work".
//!
//! One rule instead of a call at each of those places, which would be a list
//! that is wrong the day a new one is added: **when the focus is nowhere, it
//! goes back to `keys`.** The pump checks once a tick. A focus somewhere that
//! is not `keys` -- a text field being typed in, a knob taking the arrows --
//! is left alone, and so is an open popup, which holds the focus it was
//! given and returns it itself when it closes.

use crate::MainWindow;
use slint::ComponentHandle;
// The same re-export the code `slint-build` generates reaches the window
// through; there is no public API that says where the focus is. `slint` is
// held at 1.18.1 by the lockfile, and a change here is a compile error.
use slint::private_unstable_api::re_exports::WindowInner;

/// Whether no visible item holds the window's focus, so no key would reach
/// the root scope. False while a popup is open: the popup has it.
pub(crate) fn lost(window: &MainWindow) -> bool {
    let inner = WindowInner::from_pub(window.window());
    if !inner.active_popups().is_empty() {
        return false;
    }
    let focused = inner.focus_item.borrow().upgrade();
    focused.is_none_or(|item| !item.is_visible())
}

/// Gives the focus back to the root scope when nothing holds it. Returns
/// whether it had to.
pub(crate) fn keep(window: &MainWindow) -> bool {
    if !lost(window) {
        return false;
    }
    window.invoke_focus_root();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window_probe::{click, controls, install_backend, sliders, type_text};
    use i_slint_core::items::AccessibleRole;
    use slint::platform::WindowEvent;
    use slint::LogicalSize;
    use std::cell::RefCell;
    use std::rc::Rc;

    const WIDTH: f32 = 1280.0;
    const HEIGHT: f32 = 800.0;

    struct Harness {
        window: MainWindow,
        heard: Rc<RefCell<Vec<String>>>,
    }

    fn harness() -> Harness {
        install_backend();
        let window = MainWindow::new().unwrap();
        window.window().set_size(LogicalSize::new(WIDTH, HEIGHT));
        window.invoke_show_view(crate::view::DEVICES);
        let heard = Rc::new(RefCell::new(Vec::new()));
        let seen = heard.clone();
        window.on_shortcut_key(move |name, _, _, _, _| {
            seen.borrow_mut().push(name.to_string());
            true
        });
        Harness { window, heard }
    }

    impl Harness {
        fn key(&self, text: &str) {
            let text = slint::SharedString::from(text);
            self.window
                .window()
                .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
            self.window
                .window()
                .dispatch_event(WindowEvent::KeyReleased { text });
        }

        /// Whether Space, the play key, reaches the dispatcher now.
        fn space_reaches_the_root(&self) -> bool {
            let before = self.heard.borrow().len();
            self.key(" ");
            self.heard.borrow()[before..].iter().any(|name| name == "space")
        }

        /// A knob on the device face in the bottom pane, pressed: a control
        /// that takes the focus on press, the way every knob does.
        fn press_a_knob(&self) {
            let bottom = HEIGHT / 2.0;
            let knob = sliders(&self.window)
                .into_iter()
                .find(|knob| knob.centre.1 > bottom)
                .expect("the default device face has a knob in the bottom pane");
            click(&self.window, knob.centre);
            assert!(!lost(&self.window), "pressing a knob focuses it");
        }

        /// The loss, then the pump's tick: the key is dead before it and
        /// alive after it.
        fn assert_lost_then_kept(&self, how: &str) {
            assert!(
                lost(&self.window),
                "{how} should leave the focus nowhere; the test no longer shows the loss"
            );
            assert!(keep(&self.window), "the tick did not see the loss");
            assert!(
                self.space_reaches_the_root(),
                "Space still does nothing after the tick gave the focus back, {how}"
            );
            assert!(!keep(&self.window), "a second tick has nothing to give back");
        }
    }

    /// The control: an untouched window hands its keys to the root scope,
    /// and the tick leaves a focus that is where it should be alone.
    #[test]
    fn a_fresh_window_hears_space_and_the_tick_does_nothing() {
        let ui = harness();
        assert!(!lost(&ui.window));
        assert!(ui.space_reaches_the_root());
        assert!(!keep(&ui.window));
    }

    /// A pressed knob keeps the focus, and Space still bubbles past it: the
    /// tick must not take the focus from a control that holds it.
    #[test]
    fn a_focused_knob_is_left_alone() {
        let ui = harness();
        ui.press_a_knob();
        assert!(!keep(&ui.window), "the tick stole the focus from a knob");
        assert!(ui.space_reaches_the_root());
    }

    /// Answering the in-app question from the keyboard hides the dialog with
    /// its own scope still focused.
    #[test]
    fn answering_a_question_gives_the_keys_back() {
        let ui = harness();
        ui.window.set_question_title("Save changes?".into());
        ui.window.set_question_primary("Save".into());
        ui.window.set_question_open(true);
        ui.key("\n");
        assert!(!ui.window.get_question_open(), "Return answers the question");
        ui.assert_lost_then_kept("after the question was answered with Return");
    }

    /// Escape in the takes dialog closes it the same way.
    #[test]
    fn closing_the_takes_dialog_gives_the_keys_back() {
        let ui = harness();
        ui.window.set_takes_open(true);
        ui.key("\u{1b}");
        assert!(!ui.window.get_takes_open(), "Escape closes the takes dialog");
        ui.assert_lost_then_kept("after the takes dialog was closed with Escape");
    }

    /// A rename field is left with `clear-focus()`, which focuses nothing.
    #[test]
    fn leaving_a_rename_field_gives_the_keys_back() {
        for (exit, how) in [("\u{1b}", "Escape"), ("\n", "Enter")] {
            let ui = harness();
            let field = controls(&ui.window, AccessibleRole::TextInput)
                .into_iter()
                .next()
                .expect("the window shows a rename field");
            click(&ui.window, field.centre);
            assert!(!lost(&ui.window), "clicking the field puts the caret in it");
            type_text(&ui.window, "x");
            ui.key(exit);
            ui.assert_lost_then_kept(&format!("after a rename field was left with {how}"));
        }
    }

    /// The knob that had the focus is destroyed when the face is rebuilt for
    /// another source.
    #[test]
    fn rebuilding_the_face_under_a_focused_knob_gives_the_keys_back() {
        let ui = harness();
        ui.press_a_knob();
        let other = (ui.window.get_source_kind() + 1) % 8;
        ui.window.set_source_kind(other);
        ui.assert_lost_then_kept("after the focused knob's face was rebuilt");
    }

    /// The knob that had the focus is hidden when its pane shows another view.
    #[test]
    fn hiding_the_pane_under_a_focused_knob_gives_the_keys_back() {
        let ui = harness();
        ui.press_a_knob();
        ui.window.invoke_show_view(crate::view::NOTES);
        ui.assert_lost_then_kept("after the focused knob's pane was hidden");
    }
}
