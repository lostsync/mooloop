//! Tests for moving a view between panes.
//!
//! These drive `move_view` directly rather than dispatching a pointer drag:
//! the drag's *geometry* is a separate question from what a completed move
//! does to the arrangement, and it is the arrangement that can strand a pane
//! with nothing in it.
//!
//! Adam, 2026-09-08, on the bug the first two pin down:
//!
//! > if im looking at the mixer in the top left pane and drag it to the top
//! > right, the top left pane is then empty and there's no button to make it
//! > show something. it just needs to snap to whatever next-available tab is
//! > in the pane.

use mooloop_ui::{view, MainWindow};
use slint::SharedString;

const MAIN: i32 = 0;
const SPLIT: i32 = 1;
const BOTTOM: i32 = 2;

fn harness() -> MainWindow {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(SharedString::from("software")),
        },
    )))
    .ok();
    MainWindow::new().unwrap()
}

/// Every pane that holds a view must be showing one of them. This is the
/// invariant the bug broke, and it is worth asserting as an invariant rather
/// than as three separate expectations.
fn every_occupied_pane_shows_something(ui: &MainWindow) {
    let slots = [
        ui.get_steps_slot(),
        ui.get_mixer_slot(),
        ui.get_devices_slot(),
        ui.get_notes_slot(),
        ui.get_playlist_slot(),
    ];
    let active = [
        ui.get_main_active(),
        ui.get_split_active(),
        ui.get_bottom_active(),
    ];
    for (slot, shown) in active.iter().enumerate() {
        let holds_any = slots.contains(&(slot as i32));
        if holds_any {
            assert!(
                *shown >= 0 && slots[*shown as usize] == slot as i32,
                "pane {slot} holds views but shows {shown}"
            );
        } else {
            assert_eq!(*shown, -1, "pane {slot} holds nothing but shows {shown}");
        }
    }
}

#[test]
fn a_pane_whose_shown_view_leaves_falls_back_to_the_next_one() {
    let ui = harness();
    // Looking at the mixer in the top-left, exactly as reported.
    ui.invoke_show_view(view::MIXER);
    assert!(ui.get_showing_mixer());

    ui.invoke_move_view(view::MIXER, SPLIT);

    assert!(
        ui.get_showing_steps(),
        "the pane the mixer left must fall back to its remaining tab"
    );
    assert!(ui.get_showing_mixer(), "and the mixer must show where it went");
    every_occupied_pane_shows_something(&ui);
}

#[test]
fn moving_a_view_that_is_not_showing_leaves_the_shown_one_alone() {
    let ui = harness();
    // Three tabs in the dock; look at the playlist, move the devices away.
    ui.invoke_show_view(view::PLAYLIST);
    ui.invoke_move_view(view::DEVICES, MAIN);

    assert!(
        ui.get_showing_playlist(),
        "moving another tab must not change what the pane was showing"
    );
    every_occupied_pane_shows_something(&ui);
}

#[test]
fn the_main_panes_last_view_refuses_to_leave() {
    let ui = harness();
    // Empty the main pane down to one view, then try to take that one too.
    ui.invoke_move_view(view::MIXER, BOTTOM);
    ui.invoke_move_view(view::STEPS, BOTTOM);

    assert_eq!(
        ui.get_steps_slot(),
        MAIN,
        "the main pane's last view must stay: there would be nothing to drop onto"
    );
    every_occupied_pane_shows_something(&ui);
}

#[test]
fn emptying_the_split_closes_it() {
    let ui = harness();
    ui.invoke_move_view(view::MIXER, SPLIT);
    assert_eq!(ui.get_split_active(), view::MIXER);

    ui.invoke_move_view(view::MIXER, BOTTOM);

    assert_eq!(
        ui.get_split_active(),
        -1,
        "a split with nothing left in it must close rather than draw empty"
    );
    every_occupied_pane_shows_something(&ui);
}
