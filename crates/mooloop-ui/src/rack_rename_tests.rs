//! The step rack's inline rename, pressed the way a user presses it
//! (MOO-224).
//!
//! A double-click on a channel's plate turns it into a name field; Tab
//! commits and moves the field to the next channel, Shift+Tab to the
//! previous one, so a run of channels can be named from the keyboard the way
//! a column of cells is. It reaches the same `channel-renamed` verb every
//! other rename does, and each channel's edit is its own gesture, so its own
//! undo step. The plate's other presses -- select on the first click, drag to
//! reorder, the right-click menu -- stay what they were.

use super::*;
use crate::window_probe::{click, controls, install_backend, type_text, Control};
use i_slint_core::items::AccessibleRole;
use slint::platform::WindowEvent;
use slint::LogicalSize;

const WIDTH: f32 = 1800.0;
const HEIGHT: f32 = 1100.0;
const NAMES: [&str; 3] = ["Alpha", "Bravo", "Charlie"];

/// A window with three named channels and the first selected.
fn rack_window() -> (MainWindow, Rc<RefCell<UiState>>) {
    install_backend();
    let window = MainWindow::new().expect("the testing backend builds a window");
    window.window().set_size(LogicalSize::new(WIDTH, HEIGHT));
    install_strip_spec(&window);
    install_eq_spec(&window);
    let state = Rc::new(RefCell::new(UiState::new(None, 48_000, &window)));
    {
        let mut st = state.borrow_mut();
        st.session.channels.push(ChannelState::new(1));
        st.session.channels.push(ChannelState::new(2));
        for (channel, name) in st.session.channels.iter_mut().zip(NAMES) {
            channel.name = name.to_string();
        }
        let project = st
            .session
            .project_snapshot(window.get_bpm(), window.get_swing_percent());
        let samples = vec![None; project.channels.len()];
        st.replace_project(&project, &samples, &window);
        st.session.selected = 0;
        st.sync_row_flags();
        st.refresh_editor(&window);
    }
    window.set_selected_channel(0);
    (window, state)
}

/// What the plate reached, and the gestures the field opened and closed, in
/// the order they arrived.
fn listen(window: &MainWindow) -> Rc<RefCell<Vec<String>>> {
    let heard = Rc::new(RefCell::new(Vec::new()));
    macro_rules! record {
        ($on:ident, |$($arg:ident),*| $fmt:literal) => {{
            let heard = heard.clone();
            window.$on(move |$($arg),*| heard.borrow_mut().push(format!($fmt, $($arg),*)));
        }};
    }
    record!(on_channel_renamed, |ch, name| "channel-renamed {} {}");
    record!(on_channel_selected, |ch| "channel-selected {}");
    record!(on_channel_reorder_requested, |from, to| "channel-reorder {} {}");
    {
        let heard = heard.clone();
        window.global::<Gesture>().on_begin(move || heard.borrow_mut().push("begin".into()));
    }
    {
        let heard = heard.clone();
        window.global::<Gesture>().on_end(move || heard.borrow_mut().push("end".into()));
    }
    heard
}

/// The channel's plate in the step rack: the one button carrying its name.
fn plate(window: &MainWindow, name: &str) -> Control {
    let found: Vec<Control> = controls(window, AccessibleRole::Button)
        .into_iter()
        .filter(|control| control.label == name)
        .collect();
    assert_eq!(found.len(), 1, "expected one button named {name:?}, found {found:?}");
    found[0].clone()
}

/// The rack's rename fields that are open, which is none or one.
fn open_fields(window: &MainWindow) -> Vec<Control> {
    controls(window, AccessibleRole::TextInput)
        .into_iter()
        .filter(|control| control.label == "Rename channel")
        .collect()
}

fn key(window: &MainWindow, text: &str) {
    let text = slint::SharedString::from(text);
    window.window().dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    window.window().dispatch_event(WindowEvent::KeyReleased { text });
}

const TAB: &str = "\t";
const BACKTAB: &str = "\u{19}";
const ESCAPE: &str = "\u{1b}";

/// The last name `channel-renamed` gave channel `ch`.
fn last_rename(heard: &Rc<RefCell<Vec<String>>>, ch: usize) -> Option<String> {
    let prefix = format!("channel-renamed {ch} ");
    heard
        .borrow()
        .iter()
        .rev()
        .find_map(|call| call.strip_prefix(&prefix).map(str::to_string))
}

/// The issue's own case: double-click channel 1, type, Tab, type, and both
/// renames arrive, each for its own channel and each in its own gesture.
#[test]
fn double_click_renames_and_tab_moves_to_the_next_channel() {
    let (window, _state) = rack_window();
    let heard = listen(&window);

    let bravo = plate(&window, "Bravo").centre;
    click(&window, bravo);
    click(&window, bravo);
    assert_eq!(open_fields(&window).len(), 1, "the double-click opened no field");
    type_text(&window, "Kick");
    key(&window, TAB);
    assert_eq!(open_fields(&window).len(), 1, "Tab left no field open");
    type_text(&window, "Snare");
    key(&window, ESCAPE);

    // The field opens with the name selected, so typing replaces it.
    assert_eq!(last_rename(&heard, 1).as_deref(), Some("Kick"));
    assert_eq!(last_rename(&heard, 2).as_deref(), Some("Snare"));
    assert!(open_fields(&window).is_empty(), "Escape left the field open");

    let calls = heard.borrow();
    let first_2 = calls.iter().position(|call| call.starts_with("channel-renamed 2")).unwrap();
    let last_1 = calls.iter().rposition(|call| call.starts_with("channel-renamed 1")).unwrap();
    assert!(last_1 < first_2, "the renames interleaved: {calls:?}");
    // One gesture per channel: begin, channel 1's keystrokes, end, begin,
    // channel 2's, end. A second begin with nothing closed between would
    // fold both renames into one undo step.
    let gestures: Vec<&str> = calls
        .iter()
        .filter(|call| *call == "begin" || *call == "end")
        .map(String::as_str)
        .collect();
    assert_eq!(gestures, ["begin", "end", "begin", "end"], "{calls:?}");
    let end_1 = calls.iter().position(|call| call == "end").unwrap();
    assert!(last_1 < end_1 && end_1 < first_2, "channel 1's gesture closed late: {calls:?}");
    assert!(
        !calls.iter().any(|call| call.starts_with("channel-reorder")),
        "the double-click dragged: {calls:?}"
    );
}

/// Shift+Tab goes back a channel; Tab past the last channel closes the
/// field rather than wrapping.
#[test]
fn shift_tab_goes_back_and_tab_past_the_end_closes() {
    let (window, _state) = rack_window();
    let heard = listen(&window);

    let charlie = plate(&window, "Charlie").centre;
    click(&window, charlie);
    click(&window, charlie);
    key(&window, BACKTAB);
    type_text(&window, "Hat");
    assert_eq!(last_rename(&heard, 1).as_deref(), Some("Hat"));

    key(&window, TAB);
    assert_eq!(open_fields(&window).len(), 1);
    key(&window, TAB);
    assert!(open_fields(&window).is_empty(), "Tab past the last channel left a field open");
}

/// One click still selects and opens no field, and the right button still
/// selects without renaming.
#[test]
fn a_single_click_selects_and_does_not_rename() {
    let (window, _state) = rack_window();
    let heard = listen(&window);
    click(&window, plate(&window, "Bravo").centre);
    assert!(open_fields(&window).is_empty(), "one click opened the rename");
    assert!(heard.borrow().iter().any(|call| call == "channel-selected 1"), "{:?}", heard.borrow());
    assert!(!heard.borrow().iter().any(|call| call.starts_with("channel-renamed")));
}
