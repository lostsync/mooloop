//! The channel rack's `+` menu, driven by real pointer events.
//!
//! MOO-53: the menu opened, drew all eight sources, and added no channel.
//! The row closed the popup before reporting, and closing a popup tears down
//! the repeater item whose handler is still running, so the call after it
//! never landed. That is the fourth time this repository has found that
//! sequence -- `BusPicker`, `DeviceFrame`'s preset menu, `PickerChip`'s
//! `MenuField`, and here -- and the first three each got a pointer-event
//! test; this is the menu's.
//!
//! The point is that *no* markup assertion could have caught it.
//! `source_kind_menu.rs` holds the menu's list against `DeviceKind::label()`
//! and it was right the whole time the menu was dead: the rows were built
//! from the correct list, in the correct order, and sent the correct index
//! to a callback that was never reached. Only a click can tell the
//! difference between a row that reports and a row that looks like it does.
//!
//! Coordinates are computed rather than searched, for the reason
//! `first_click.rs` records: the `ElementHandle` search API needs a build
//! with debug info, and this control has fixed geometry anyway. The one test
//! that reads what a row *says* walks the open popup's item tree by
//! accessible role instead, as `src/window_probe.rs` does, which every build
//! carries.

use i_slint_core::accessibility::AccessibleStringProperty;
use i_slint_core::item_tree::ItemRc;
use i_slint_core::items::AccessibleRole;
use i_slint_core::window::{PopupWindowLocation, WindowInner};
use mooloop_ui::{device_kind_to_int, RETIRED_SOURCE_KINDS, SOURCE_KINDS_IN_PICKER_ORDER};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize};
use std::cell::RefCell;
use std::ops::ControlFlow;
use std::rc::Rc;

slint::slint! {
    import { AddSourceButton } from "../ui/channel-rack.slint";

    export component AddSourceHarness inherits Window {
        width: 260px;
        height: 320px;
        background: #101010;
        callback picked(int);

        AddSourceButton {
            x: 0px;
            y: 0px;
            width: 28px;
            height: 24px;
            picked(i) => { root.picked(i); }
        }
    }
}

/// The middle of the `+` button: 28 by 24 at the window's origin.
const BUTTON: (f32, f32) = (14.0, 12.0);

/// The menu opens 2px under the button, insets its rows by 4px, and stacks
/// them 24px tall with nothing between. So the first row's middle is at
/// 24 + 2 + 4 + 12.
const FIRST_ROW_Y: f32 = 42.0;
const ROW_PITCH: f32 = 24.0;
/// Inside the 156px popup and past `MenuRow`'s 26px label gutter.
const ROW_X: f32 = 60.0;

fn row_y(row: usize) -> f32 {
    FIRST_ROW_Y + row as f32 * ROW_PITCH
}

fn harness() -> AddSourceHarness {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AddSourceHarness::new().unwrap();
    ui.window().set_size(LogicalSize::new(260.0, 320.0));
    ui
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

/// **Choosing a source reaches the callback carrying that source's number.**
///
/// This is MOO-53 asked directly. It fails on the unfixed markup with an
/// empty `reported`, which is what "adds no channel" looks like from here.
#[test]
fn choosing_a_row_reports_it() {
    let ui = harness();
    let picked: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = picked.clone();
    ui.on_picked(move |index| seen.borrow_mut().push(index));

    click(ui.window(), BUTTON);
    click(ui.window(), (ROW_X, row_y(0)));
    assert_eq!(
        *picked.borrow(),
        vec![0],
        "choosing the first source did not reach the callback"
    );

    click(ui.window(), BUTTON);
    click(ui.window(), (ROW_X, row_y(1)));
    assert_eq!(
        *picked.borrow(),
        vec![0, 1],
        "the second row reported the wrong index"
    );
}

/// **Every source device can be started from the menu, and reports its own
/// kind's number.**
///
/// Clicking every row rather than counting them, for `effect_preset_menu.rs`'s
/// reason: a menu one row short still has a last row, and a click at a
/// missing row's coordinates lands on nothing and reports nothing. What comes
/// back has to be every offered kind's number exactly once, which is the
/// reachability question asked without assuming the menu's order --
/// `device_kind_to_int` decides that, and `source_kind_menu.rs` holds the
/// labels to it.
///
/// Offered, not every: the retired sources have no row, and the rows after
/// them move up while still reporting their own kind. One click past the
/// last row checks nothing is drawn there.
#[test]
fn every_offered_source_is_reachable_and_carries_its_own_number() {
    let ui = harness();
    let picked: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = picked.clone();
    ui.on_picked(move |index| seen.borrow_mut().push(index));

    let offered: Vec<_> = SOURCE_KINDS_IN_PICKER_ORDER
        .into_iter()
        .filter(|kind| !RETIRED_SOURCE_KINDS.contains(kind))
        .collect();
    let rows = offered.len();
    // A row past the bottom of the harness window cannot be clicked, and an
    // unclicked row reads exactly like a missing one.
    let window_height = ui.window().size().height as f32;
    assert!(
        row_y(rows - 1) + ROW_PITCH / 2.0 < window_height,
        "the harness window is {window_height}px tall and the menu now needs \
         {rows} rows: make it taller, or this test starts passing by failing \
         to click"
    );

    // One past the end too: a retired source that still had a row would
    // push the last offered one there.
    for row in 0..=rows {
        click(ui.window(), BUTTON);
        click(ui.window(), (ROW_X, row_y(row)));
    }

    let mut reported = picked.borrow().clone();
    reported.sort();
    let mut expected: Vec<i32> = offered.into_iter().map(device_kind_to_int).collect();
    expected.sort();
    assert_eq!(
        reported, expected,
        "the menu did not offer every source but the retired ones, each once"
    );
}

/// **The menu closes when a row is chosen.**
///
/// The fix is to delete the row's `close()` and leave the dismissal to
/// `close-policy: close-on-click`. That is only a fix if the policy really
/// does dismiss it, so this checks the other half: after choosing a row, a
/// click where the second row was must land on nothing rather than on a menu
/// still standing open.
#[test]
fn choosing_a_row_closes_the_menu() {
    let ui = harness();
    let picked: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = picked.clone();
    ui.on_picked(move |index| seen.borrow_mut().push(index));

    click(ui.window(), BUTTON);
    click(ui.window(), (ROW_X, row_y(0)));
    click(ui.window(), (ROW_X, row_y(1)));
    assert_eq!(
        *picked.borrow(),
        vec![0],
        "the menu was still open after a row was chosen"
    );
}

/// Every row of the open menu, top to bottom, as its text and its middle in
/// window coordinates. A `MenuRow` is `accessible-role: button` with its text
/// as its label, so this reads what a user reads.
fn open_menu_rows(window: &slint::Window) -> Vec<(String, (f32, f32))> {
    let inner = WindowInner::from_pub(window);
    let popups = inner.active_popups();
    assert_eq!(popups.len(), 1, "the + should have opened exactly one menu");
    let popup = &popups[0];
    let offset = match popup.location {
        PopupWindowLocation::ChildWindow(at) => (at.x, at.y),
        _ => (0.0, 0.0),
    };
    let mut rows = Vec::new();
    ItemRc::new_root(popup.component.clone()).visit_descendants(|item| {
        if item.accessible_role() == AccessibleRole::Button && item.is_visible() {
            let geometry = item.geometry();
            let origin = item.map_to_window(geometry.origin);
            let label = item
                .accessible_string_property(AccessibleStringProperty::Label)
                .map(|text| text.to_string())
                .unwrap_or_default();
            rows.push((
                label,
                (
                    offset.0 + origin.x + geometry.size.width / 2.0,
                    offset.1 + origin.y + geometry.size.height / 2.0,
                ),
            ));
        }
        ControlFlow::<()>::Continue(())
    });
    rows
}

/// **Each row names its source by `DeviceKind::title()`, and adds the kind it
/// names.**
///
/// MOO-275: the menu has room, so it reads "Add Munotone ML-M1" where the
/// picker chip reads "ML-M1" (Adam, 2026-09-26: *both where there's room, the
/// model number where it's tight*). A row's text comes from
/// `SourceKinds.titles` and the number it reports from its position in
/// `SourceKinds.labels`: two lists, and this holds them together from the
/// outside. Each row is clicked where it is actually drawn and has to report
/// the kind whose title it shows. A `titles` list out of step with `labels`
/// would put one kind's name on another kind's row, and every other test in
/// this file would still pass.
#[test]
fn every_row_reads_its_sources_title_and_adds_that_source() {
    let ui = harness();
    let picked: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = picked.clone();
    ui.on_picked(move |index| seen.borrow_mut().push(index));

    click(ui.window(), BUTTON);
    let rows = open_menu_rows(ui.window());

    let offered: Vec<_> = SOURCE_KINDS_IN_PICKER_ORDER
        .into_iter()
        .filter(|kind| !RETIRED_SOURCE_KINDS.contains(kind))
        .collect();
    let mut expected: Vec<String> = offered
        .iter()
        .map(|kind| format!("Add {}", kind.title()))
        .collect();
    expected.push("Add Plugin…".to_owned());
    let texts: Vec<&str> = rows.iter().map(|(text, _)| text.as_str()).collect();
    assert_eq!(
        texts, expected,
        "the + menu's rows should read each offered source's title, in picker order"
    );

    // Dismiss the menu that was opened only to be read, then choose each
    // source row where it was drawn.
    click(ui.window(), (250.0, 310.0));
    for ((text, centre), kind) in rows.iter().zip(&offered) {
        click(ui.window(), BUTTON);
        click(ui.window(), *centre);
        assert_eq!(
            picked.borrow().last().copied(),
            Some(device_kind_to_int(*kind)),
            "the row reading `{text}` added some other source"
        );
    }
    assert_eq!(
        picked.borrow().len(),
        offered.len(),
        "a click meant only to dismiss the menu chose a source"
    );
}
