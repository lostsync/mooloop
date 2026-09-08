//! Tests for dragging a pane tab, as a pointer gesture.
//!
//! `panes.rs` drives `move_view` directly and asks what a completed move does
//! to the arrangement. This asks the other half: whether a real drag reaches
//! that function at all, and whether the drop lands in the pane the pointer
//! was over. Between them they cover the gesture; neither does alone, and the
//! bug that reached Adam on 2026-09-08 lived in the half `panes.rs` now holds.
//!
//! Tab geometry in the 960x760 window, measured off a software render: both
//! strips are inset the same, the first tab spans x 15..80, and the rows are
//! the top pane's toolbar and the dock's. These move if a pane toolbar's
//! height or padding changes — `coordinates_still_land_on_the_tabs` is here so
//! that shows up as one obvious failure rather than as three silent ones.

use mooloop_ui::{view, MainWindow};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, SharedString};

const TAB_PITCH: f32 = 66.0;
const FIRST_TAB_CENTRE_X: f32 = 47.0;
const TOP_TAB_Y: f32 = 73.0;
const BOTTOM_TAB_Y: f32 = 334.0;
/// Well inside the dock, below the divider.
const IN_BOTTOM_PANE: (f32, f32) = (480.0, 430.0);
/// Inside the right-hand band of an unsplit top pane, past 72% of its width,
/// which is the zone that opens a split rather than moving within one.
const TOP_RIGHT_EDGE: (f32, f32) = (900.0, 150.0);

fn tab(index: usize, row: f32) -> (f32, f32) {
    (FIRST_TAB_CENTRE_X + TAB_PITCH * index as f32, row)
}

fn harness() -> MainWindow {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(SharedString::from("software")),
        },
    )))
    .ok();
    let ui = MainWindow::new().unwrap();
    ui.window().set_size(LogicalSize::new(960.0, 760.0));
    ui
}

fn click(window: &slint::Window, at: (f32, f32)) {
    let pos = LogicalPosition::new(at.0, at.1);
    window.dispatch_event(WindowEvent::PointerMoved { position: pos });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: pos,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position: pos,
        button: PointerEventButton::Left,
    });
}

/// Several moves, not one: the drag only arms once the pointer has travelled
/// past its threshold, so a single jump would test a different code path from
/// the one a hand produces.
fn drag(window: &slint::Window, from: (f32, f32), to: (f32, f32)) {
    let pos = |(x, y): (f32, f32)| LogicalPosition::new(x, y);
    window.dispatch_event(WindowEvent::PointerMoved { position: pos(from) });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: pos(from),
        button: PointerEventButton::Left,
    });
    const STEPS: usize = 8;
    for i in 1..=STEPS {
        let t = i as f32 / STEPS as f32;
        window.dispatch_event(WindowEvent::PointerMoved {
            position: pos((
                from.0 + (to.0 - from.0) * t,
                from.1 + (to.1 - from.1) * t,
            )),
        });
    }
    window.dispatch_event(WindowEvent::PointerReleased {
        position: pos(to),
        button: PointerEventButton::Left,
    });
}

/// If this fails, every other test in the file is testing empty space. It is
/// first on purpose.
#[test]
fn coordinates_still_land_on_the_tabs() {
    let ui = harness();
    click(ui.window(), tab(1, TOP_TAB_Y));
    assert!(
        ui.get_showing_mixer(),
        "the second top-pane tab must be the mixer -- if not, the measured \
         tab geometry has drifted and the rest of this file is meaningless"
    );
    click(ui.window(), tab(1, BOTTOM_TAB_Y));
    assert!(
        ui.get_showing_notes(),
        "the second dock tab must be the piano roll"
    );
}

#[test]
fn dragging_a_tab_into_the_dock_moves_it_there() {
    let ui = harness();
    drag(ui.window(), tab(1, TOP_TAB_Y), IN_BOTTOM_PANE);

    assert_eq!(
        ui.get_mixer_slot(),
        2,
        "a tab dropped over the dock belongs to the dock"
    );
    assert!(ui.get_showing_mixer(), "and is what the dock now shows");
    assert!(
        ui.get_showing_steps(),
        "the pane it left falls back to its remaining tab"
    );
}

#[test]
fn dropping_on_the_top_panes_right_edge_opens_the_split() {
    let ui = harness();
    assert_eq!(ui.get_split_active(), -1, "no split to begin with");

    drag(ui.window(), tab(1, TOP_TAB_Y), TOP_RIGHT_EDGE);

    assert_eq!(
        ui.get_mixer_slot(),
        1,
        "the right-hand band of an unsplit top pane opens the split"
    );
    assert_eq!(ui.get_split_active(), view::MIXER);
}

#[test]
fn a_press_that_does_not_travel_selects_rather_than_moving() {
    let ui = harness();
    let before = ui.get_mixer_slot();

    // Two pixels: inside the threshold that separates a wobbly click from a
    // drag, so this must read as a plain selection.
    let (x, y) = tab(1, TOP_TAB_Y);
    drag(ui.window(), (x, y), (x + 2.0, y));

    assert_eq!(
        ui.get_mixer_slot(),
        before,
        "a click that wobbled must not move the view"
    );
    assert!(ui.get_showing_mixer(), "it selects the tab instead");
}
