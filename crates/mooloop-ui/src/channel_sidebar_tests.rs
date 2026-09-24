//! The left sidebar, pressed the way a user presses it (MOO-8).
//!
//! What it owes, from the issue and Adam's answers of 2026-09-23: an action
//! and a shortcut that show and hide it, and NAME, COLOR, MIX, MIDI and AUDIO
//! rows that each reach the *same* verbs as the rest of the interface --
//! "both must drive the same verbs, so neither copy can drift". So every test
//! here presses a control in the real window and asserts which window
//! callback it reached, and for which channel or track, rather than calling
//! the callbacks itself.

use super::*;
use crate::window_probe::{click, controls, install_backend, type_text, wheel, Control};
use i_slint_core::items::AccessibleRole;
use slint::LogicalSize;

const WIDTH: f32 = 1800.0;
const HEIGHT: f32 = 1100.0;

/// A window with the sidebar showing and the *second* channel selected, so a
/// row that named channel 0 instead of the selection would be caught.
fn sidebar_window() -> (MainWindow, Rc<RefCell<UiState>>) {
    install_backend();
    let window = MainWindow::new().expect("the testing backend builds a window");
    window.window().set_size(LogicalSize::new(WIDTH, HEIGHT));
    install_strip_spec(&window);
    install_eq_spec(&window);
    let state = Rc::new(RefCell::new(UiState::new(None, 48_000, &window)));
    {
        let mut st = state.borrow_mut();
        st.session.channels.push(ChannelState::new(1));
        let project = st
            .session
            .project_snapshot(window.get_bpm(), window.get_swing_percent());
        let samples = vec![None; project.channels.len()];
        st.replace_project(&project, &samples, &window);
        st.session.selected = 1;
        st.sync_row_flags();
        st.refresh_editor(&window);
    }
    window.set_selected_channel(1);
    window.set_channel_sidebar_visible(true);
    (window, state)
}

/// Everything the sidebar can call, in the order it arrived.
fn listen(window: &MainWindow) -> Rc<RefCell<Vec<String>>> {
    let heard = Rc::new(RefCell::new(Vec::new()));
    macro_rules! record {
        ($on:ident, |$($arg:ident),*| $fmt:literal) => {{
            let heard = heard.clone();
            window.$on(move |$($arg),*| heard.borrow_mut().push(format!($fmt, $($arg),*)));
        }};
    }
    record!(on_channel_muted, |ch| "channel-muted {}");
    record!(on_channel_soloed, |ch| "channel-soloed {}");
    record!(on_channel_volume_changed, |ch, v| "channel-volume {} {}");
    record!(on_channel_pan_changed, |ch, v| "channel-pan {} {}");
    record!(on_channel_renamed, |ch, name| "channel-renamed {} {}");
    record!(on_channel_color_chosen, |hex| "channel-color {}");
    record!(on_bus_muted, |bus| "bus-muted {}");
    record!(on_bus_solo_toggled, |bus| "bus-solo {}");
    record!(on_bus_volume_changed, |bus, v| "bus-volume {} {}");
    record!(on_bus_pan_changed, |bus, v| "bus-pan {} {}");
    record!(on_midi_input_picked, |row| "midi-input {}");
    record!(on_midi_channel_picked, |row| "midi-channel {}");
    record!(on_audio_input_picked, |row| "audio-input {}");
    heard
}

/// The controls inside the sidebar, which is the leftmost panel: anything
/// left of its right edge. Popups are not in it, so they are asked for
/// separately.
fn in_sidebar(window: &MainWindow, role: AccessibleRole) -> Vec<Control> {
    let edge = window.get_channel_sidebar_width();
    controls(window, role)
        .into_iter()
        .filter(|control| control.centre.0 < edge)
        .collect()
}

fn one(window: &MainWindow, role: AccessibleRole, label: &str) -> Control {
    in_sidebar(window, role)
        .into_iter()
        .find(|control| control.label.ends_with(label) || control.label.starts_with(label))
        .unwrap_or_else(|| panic!("no {role:?} labelled {label:?} in the sidebar"))
}

fn heard_one(heard: &Rc<RefCell<Vec<String>>>, prefix: &str) {
    let calls = heard.borrow();
    assert!(
        calls.iter().any(|call| call.starts_with(prefix)),
        "expected a call starting {prefix:?}, heard {calls:?}"
    );
}

/// The action shows and hides the panel, and each flip is a layout change
/// the settings file is told about, as the status-bar chip's always was.
#[test]
fn the_action_toggles_the_sidebar_and_saves_the_layout() {
    let (window, _state) = sidebar_window();
    let saved = Rc::new(std::cell::Cell::new(0));
    {
        let saved = saved.clone();
        window.on_layout_changed(move || saved.set(saved.get() + 1));
    }
    window.invoke_toggle_channel_sidebar();
    assert!(!window.get_channel_sidebar_visible());
    window.invoke_toggle_channel_sidebar();
    assert!(window.get_channel_sidebar_visible());
    assert_eq!(saved.get(), 2, "each toggle is a layout change to save");

    let spec = actions::ACTIONS
        .iter()
        .find(|spec| spec.id == "view.channel-sidebar-toggle")
        .expect("the sidebar has an action id");
    let chord = spec.default_chord().expect("and a default shortcut");
    assert!(chord.ctrl && !chord.shift && !chord.alt && chord.key == "[", "{chord:?}");
}

/// MIX drives the channel verbs the rack row drives, for the selected
/// channel.
#[test]
fn the_mix_row_drives_the_selected_channels_verbs() {
    let (window, _state) = sidebar_window();
    let heard = listen(&window);

    click(&window, one(&window, AccessibleRole::Button, "Mute").centre);
    click(&window, one(&window, AccessibleRole::Button, "Solo").centre);
    wheel(&window, &one(&window, AccessibleRole::Slider, "This channel's level"));
    wheel(&window, &one(&window, AccessibleRole::Slider, "This channel's pan"));

    heard_one(&heard, "channel-muted 1");
    heard_one(&heard, "channel-soloed 1");
    heard_one(&heard, "channel-volume 1 ");
    heard_one(&heard, "channel-pan 1 ");
}

/// The same row, with a track selected, drives the track's verbs.
#[test]
fn the_mix_row_drives_the_edited_tracks_verbs() {
    let (window, _state) = sidebar_window();
    window.set_editing_bus(true);
    window.set_editing_bus_index(1);
    window.set_editing_bus_is_master(false);
    let heard = listen(&window);

    click(&window, one(&window, AccessibleRole::Button, "Mute").centre);
    click(&window, one(&window, AccessibleRole::Button, "Solo").centre);
    wheel(&window, &one(&window, AccessibleRole::Slider, "This track's level"));
    wheel(&window, &one(&window, AccessibleRole::Slider, "This track's pan"));

    heard_one(&heard, "bus-muted 1");
    heard_one(&heard, "bus-solo 1");
    heard_one(&heard, "bus-volume 1 ");
    heard_one(&heard, "bus-pan 1 ");
    assert!(
        !heard.borrow().iter().any(|call| call.starts_with("channel-")),
        "a track's row reached a channel verb: {:?}",
        heard.borrow()
    );
}

/// The sidebar's mix controls follow the document, not their own last
/// touch: the rack row and the mixer move them too (MOO-220's rule).
#[test]
fn the_mix_row_follows_the_channel_it_shows() {
    let (window, state) = sidebar_window();
    let pan = |window: &MainWindow| one(window, AccessibleRole::Slider, "This channel's pan").value;
    let before = pan(&window);
    {
        let mut st = state.borrow_mut();
        st.session.channels[1].pan = -0.5;
        st.sync_row_flags();
    }
    assert_ne!(before, pan(&window), "the pan knob did not follow the row");
    assert_eq!(pan(&window), "L 50");
}

/// NAME renames the selected channel through the rename every field uses.
#[test]
fn the_name_row_renames_the_selected_channel() {
    let (window, _state) = sidebar_window();
    let heard = listen(&window);
    click(&window, one(&window, AccessibleRole::TextInput, "Rename this channel").centre);
    type_text(&window, "Z");
    heard_one(&heard, "channel-renamed 1 ");
}

/// COLOR opens its swatches, and a swatch colours the selected channel.
#[test]
fn the_color_row_colours_the_channel() {
    let (window, _state) = sidebar_window();
    let heard = listen(&window);
    click(&window, one(&window, AccessibleRole::Button, "Color this channel").centre);
    let none = controls(&window, AccessibleRole::Button)
        .into_iter()
        .find(|swatch| swatch.label == "No color")
        .expect("the open picker offers No color");
    click(&window, none.centre);
    heard_one(&heard, "channel-color ");
}

/// MIDI IN, CH and AUDIO IN each open their list and hand back the row
/// picked -- never a value, which is Rust's to map.
#[test]
fn the_midi_and_audio_rows_hand_back_the_row_picked() {
    let (window, _state) = sidebar_window();
    let strings = |items: &[&str]| {
        ModelRc::from(Rc::new(VecModel::from(
            items.iter().map(|item| SharedString::from(*item)).collect::<Vec<_>>(),
        )))
    };
    window.set_midi_input_options(strings(&["Follow Selection", "Keys"]));
    window.set_midi_channel_options(strings(&["Omni", "1"]));
    window.set_audio_input_options(strings(&["None", "Input 1"]));
    let heard = listen(&window);
    for (field, row, expected) in [
        ("Which MIDI input", "Keys", "midi-input 1"),
        ("Which MIDI channel", "1", "midi-channel 1"),
        ("Audio input", "Input 1", "audio-input 1"),
    ] {
        click(&window, one(&window, AccessibleRole::Combobox, field).centre);
        let item = controls(&window, AccessibleRole::ListItem)
            .into_iter()
            .find(|item| item.label == row)
            .unwrap_or_else(|| panic!("the {field} list has no {row:?} row"));
        click(&window, item.centre);
        heard_one(&heard, expected);
    }
}
