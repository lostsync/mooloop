//! The focused pane and its outline (MOO-287), pressed the way a user
//! presses it.
//!
//! Adam asked for the pane the keyboard is aimed at to be highlighted "like
//! the border drawn on a selected control", so it is clear where shortcuts
//! are picked up. The rule the outline is held to is that **it shows what the
//! contextual chords act on**: `focused-surface`, which the dispatcher reads
//! through `focused_surface` before any `Scope::Focused` arm runs, is derived
//! from the outlined pane and from nothing else. So every test here asserts
//! both halves -- which pane is outlined, and which surface a chord pressed
//! now would resolve against -- after a real press.

use super::*;
use crate::window_probe::{click, install_backend};
use slint::platform::WindowEvent;
use slint::{LogicalPosition, LogicalSize};

const WIDTH: f32 = 1280.0;
const HEIGHT: f32 = 800.0;

/// The default arrangement: the steps in the main slot, the devices in the
/// dock below it.
fn window() -> MainWindow {
    install_backend();
    let window = MainWindow::new().unwrap();
    window.window().set_size(LogicalSize::new(WIDTH, HEIGHT));
    window.invoke_show_view(view::STEPS);
    window.invoke_show_view(view::DEVICES);
    // Revealing a view focuses it; start from nothing, so the first press is
    // what moves the outline.
    window.invoke_focus_pane(-1);
    window
}

fn hover(window: &MainWindow, point: (f32, f32)) {
    window.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(point.0, point.1),
    });
}

/// A point inside `view`: the lowest pixel row the pointer finds the pane
/// under, a little inside its bottom edge.
fn background_of(window: &MainWindow, view: i32) -> (f32, f32) {
    pane_rows(window, view).0
}

/// The bottom and the top of `view` down the middle of the window, each a
/// little inside the pane.
fn pane_rows(window: &MainWindow, view: i32) -> ((f32, f32), (f32, f32)) {
    let x = WIDTH / 2.0;
    let mut y = HEIGHT - 1.0;
    let mut bottom = None;
    while y > 0.0 {
        hover(window, (x, y));
        let inside = window.get_hovered_pane() == view;
        match (bottom, inside) {
            (None, true) => bottom = Some(y),
            (Some(b), false) => return ((x, b - 4.0), (x, y + 6.0)),
            _ => {}
        }
        y -= 2.0;
    }
    match bottom {
        Some(b) => ((x, b - 4.0), (x, 6.0)),
        None => panic!("the pointer never found view {view} down the middle of the window"),
    }
}

/// Presses `view` somewhere nothing inside it claims -- its background --
/// and returns where. Tried along the pane's bottom edge and its toolbar row,
/// because what covers a pane depends on the view: with no channels the steps
/// are empty, while the rack's face takes presses almost everywhere. A press
/// that something claimed leaves the outline where it was, and the next point
/// is tried from there.
///
/// **Middle outwards, not right to left.** A toolbar's controls sit at its
/// two ends and its stretch is in the middle. Right to left worked only while
/// the window's 8px padding happened to leave 0.97 of the width in a gap past
/// the dock toolbar's last control. When MOO-284 took the padding away, the
/// toolbar's right-hand controls moved 8px right and that press landed on
/// one. Every point after it then stopped hovering the pane, most likely
/// under the menu it had opened.
fn press_background(window: &MainWindow, view: i32) -> (f32, f32) {
    let (bottom, top) = pane_rows(window, view);
    let before = window.get_active_pane();
    for y in [bottom.1, top.1] {
        for fraction in [0.5, 0.7, 0.3, 0.85, 0.1, 0.97] {
            let point = (WIDTH * fraction, y);
            hover(window, point);
            if window.get_hovered_pane() != view {
                continue;
            }
            click(window, point);
            if window.get_active_pane() == view {
                return point;
            }
            window.invoke_focus_pane(before);
        }
    }
    panic!("no press anywhere along view {view}'s bottom edge or toolbar focused it");
}

fn outlined(window: &MainWindow) -> i32 {
    window.get_active_pane()
}

#[test]
fn pressing_two_panes_moves_the_outline_and_the_chords_with_it() {
    let window = window();
    assert_eq!(outlined(&window), -1);

    press_background(&window, view::STEPS);
    assert_eq!(outlined(&window), view::STEPS, "a press in the steps outlines them");
    assert_eq!(
        focused_surface(&window),
        actions::Surface::Channels,
        "the steps are the channel list: Ctrl+C there copies the channel"
    );

    press_background(&window, view::DEVICES);
    assert_eq!(outlined(&window), view::DEVICES, "a press in the rack moves the outline to it");
    assert_eq!(
        focused_surface(&window),
        actions::Surface::Rack,
        "and Ctrl+C now resolves against the rack, which is what the outline is around"
    );
    assert_eq!(window.get_active_slot(), 2, "the outlined pane's slot is the one Zoom means");
}

/// Ctrl+1..5 reveal a view through `show-view`, and the outline goes with it.
#[test]
fn revealing_a_view_moves_the_outline_to_it() {
    let window = window();
    window.invoke_show_view(view::NOTES);
    assert_eq!(outlined(&window), view::NOTES);
    assert_eq!(focused_surface(&window), actions::Surface::Notes);
    window.invoke_show_view(view::STEPS);
    assert_eq!(outlined(&window), view::STEPS);
    assert_eq!(focused_surface(&window), actions::Surface::Channels);
}

/// A selection that reaches Rust -- a device picked from the keyboard, the
/// browser taken with Ctrl+B -- names a surface, and the outline goes to
/// the pane that shows it.
#[test]
fn a_selection_outlines_the_pane_that_shows_it() {
    let window = window();
    set_focused_surface(&window, actions::Surface::Rack);
    assert_eq!(outlined(&window), view::DEVICES);
    set_focused_surface(&window, actions::Surface::Browser);
    assert_eq!(outlined(&window), 5, "the browser sidebar is pane 5");
    assert_eq!(focused_surface(&window), actions::Surface::Browser);
}

/// The channel is in more than one pane, so a channel picked with the
/// pointer outlines the pane the pointer is in, and one picked from the
/// keyboard, with the pointer over nothing that shows channels, outlines
/// the steps.
#[test]
fn a_channel_picked_outlines_the_pane_it_was_picked_in() {
    let window = window();
    window.invoke_show_view(view::MIXER);
    window.invoke_focus_pane(-1);
    let in_mixer = background_of(&window, view::MIXER);
    hover(&window, in_mixer);
    set_focused_surface(&window, actions::Surface::Channels);
    assert_eq!(outlined(&window), view::MIXER, "picked in the mixer");

    let in_rack = background_of(&window, view::DEVICES);
    hover(&window, in_rack);
    set_focused_surface(&window, actions::Surface::Rack);
    assert_eq!(outlined(&window), view::DEVICES);
    set_focused_surface(&window, actions::Surface::Channels);
    assert_eq!(
        outlined(&window),
        -1,
        "the steps are not on screen, so no pane shows the channel: no outline, \
         and the chords fall back to the selected channel"
    );
    assert_eq!(focused_surface(&window), actions::Surface::Channels);

    window.invoke_show_view(view::STEPS);
    hover(&window, in_rack);
    set_focused_surface(&window, actions::Surface::Rack);
    set_focused_surface(&window, actions::Surface::Channels);
    assert_eq!(outlined(&window), view::STEPS, "with the steps showing, they are the list");
}

/// The roll's own press still aims at the notes, as it did before there was
/// an outline, and so the notes' clipboard is the one Ctrl+C uses.
#[test]
fn a_press_in_the_roll_outlines_the_notes() {
    let window = window();
    window.invoke_show_view(view::NOTES);
    window.invoke_focus_pane(view::DEVICES);
    // The roll's grid claims the press and asks for the keys back through
    // `focus-requested`; anywhere in the pane's lower half is grid.
    let point = background_of(&window, view::NOTES);
    click(&window, (point.0, point.1 - 60.0));
    assert_eq!(outlined(&window), view::NOTES);
    assert_eq!(focused_surface(&window), actions::Surface::Notes);
}

/// Every `Scope::Focused` chord, after a real press in each pane, lands on
/// what the outline is around. `focused_target` is what the dispatcher calls
/// with the surface it reads, so this is the routing end to end short of the
/// engine: press, pane, surface, target.
#[test]
fn every_focused_chord_lands_on_the_outlined_pane() {
    use actions::{focused_target, Aim, Target};
    let focused: Vec<&str> = actions::ACTIONS
        .iter()
        .filter(|spec| spec.scope == actions::Scope::Focused)
        .map(|spec| spec.id)
        .collect();
    assert_eq!(focused.len(), 7, "a Focused action was added or removed; route it here");

    // Everything a target could want, so the surface alone decides.
    let aim = Aim { device_selected: true, device_clipboard: true, note_clipboard: true };
    let lands = |window: &MainWindow, id: &str| {
        focused_target(id, focused_surface(window), aim)
            .unwrap_or_else(|| panic!("{id} is Focused but has no route"))
    };
    let window = window();

    // The steps: the channel list.
    press_background(&window, view::STEPS);
    assert_eq!(outlined(&window), view::STEPS);
    for id in &focused {
        let expected = match *id {
            "notes.nudge-earlier" | "notes.nudge-later" => Target::Nothing,
            _ => Target::Channel,
        };
        assert_eq!(lands(&window, id), expected, "{id} with the steps outlined");
    }

    // The rack: the device where it has a verb, the channel for the two
    // arrows that pick one, nothing for the two that do not.
    press_background(&window, view::DEVICES);
    assert_eq!(outlined(&window), view::DEVICES);
    for id in &focused {
        let expected = match *id {
            "notes.nudge-earlier" | "notes.nudge-later" => Target::Nothing,
            "notes.nudge-up" | "notes.nudge-down" => Target::Channel,
            _ => Target::Device,
        };
        assert_eq!(lands(&window, id), expected, "{id} with the rack outlined");
    }

    // The roll, pressed in its grid: its notes, every one.
    window.invoke_show_view(view::NOTES);
    window.invoke_focus_pane(view::STEPS);
    let roll = background_of(&window, view::NOTES);
    click(&window, (roll.0, roll.1 - 60.0));
    assert_eq!(outlined(&window), view::NOTES);
    for id in &focused {
        assert_eq!(lands(&window, id), Target::Notes, "{id} with the roll outlined");
    }

    // The browser, taken with Ctrl+B: the arrows walk it, and the clipboard
    // chords mean the channel, as they do in any pane without a clipboard.
    set_focused_surface(&window, actions::Surface::Browser);
    assert_eq!(outlined(&window), 5);
    for id in &focused {
        let expected =
            if id.starts_with("notes.nudge") { Target::Browser } else { Target::Channel };
        assert_eq!(lands(&window, id), expected, "{id} with the browser outlined");
    }

    // What changed (MOO-287): notes selected in a roll that is on screen but
    // not outlined no longer take Ctrl+C, and the note clipboard is no longer
    // pasted into another pane.
    press_background(&window, view::STEPS);
    window.set_has_note_selection(true);
    assert!(window.get_showing_notes(), "the roll is still on screen");
    assert_eq!(lands(&window, "edit.copy-channel"), Target::Channel);
    assert_eq!(lands(&window, "edit.paste-channel"), Target::Channel);
}
