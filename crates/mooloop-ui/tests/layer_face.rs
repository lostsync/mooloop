//! The layer's branch list, driven by real pointer events
//! (`docs/plans/containers/09`).
//!
//! What matters about the list is that a press on a row reaches the callback
//! carrying the **branch head's rack index** -- `branch.slot` -- and not the
//! row's place in the list, which is the `lvl`/`level` trap the plan warns
//! about. A face test that invoked the callbacks directly would pass while
//! every press named the wrong row, so these click.

use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize};
use std::cell::RefCell;
use std::rc::Rc;

slint::slint! {
    import { LayerBranchListRow, LayerBranchRow, LayerDeviceFace } from "../ui/layer-device.slint";

    export component LayerHarness inherits Window {
        width: 260px;
        height: 280px;
        background: #101010;
        // Branch heads at rack indices 3 and 7: nothing like 0 and 1, so a
        // callback that sent the list position would be seen to.
        in property <[LayerBranchRow]> branches: [
            { slot: 3, name: "Clean", controls: true },
            { slot: 7, name: "Crushed", controls: true },
        ];
        callback selected(int);
        callback muted(int);
        callback soloed(int);
        callback added();

        LayerDeviceFace {
            x: 0px; y: 0px;
            width: 220px; height: 268px;
            name: "Layer";
            branch-count: root.branches.length;
            add-branch-requested => { root.added(); }
            for branch[i] in root.branches : LayerBranchListRow {
                name: branch.name;
                controls: branch.controls;
                select-requested => { root.selected(branch.slot); }
                mute-toggled => { root.muted(branch.slot); }
                solo-toggled => { root.soloed(branch.slot); }
            }
        }
    }
}

/// The face's list starts below the 28px header, inset 6px by the face and
/// 3px by the list; rows are 22px on a 23px pitch. These are the middles of
/// the two rows' names, and of the second row's S and M buttons, which sit
/// at the row's right end before the 5px meter.
const FIRST_ROW: (f32, f32) = (40.0, 48.0);
const SECOND_ROW: (f32, f32) = (40.0, 71.0);
const SECOND_SOLO: (f32, f32) = (111.0, 71.0);
const SECOND_MUTE: (f32, f32) = (131.0, 71.0);
/// Under the two rows: 3px of padding above an 18px button.
const PLUS: (f32, f32) = (79.0, 95.0);

fn harness() -> LayerHarness {
    i_slint_backend_testing::init_no_event_loop();
    let ui = LayerHarness::new().unwrap();
    ui.window().set_size(LogicalSize::new(260.0, 280.0));
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

#[test]
fn a_press_on_a_row_names_the_branch_head() {
    let ui = harness();
    let seen: Rc<RefCell<Vec<i32>>> = Rc::default();
    let log = seen.clone();
    ui.on_selected(move |slot| log.borrow_mut().push(slot));
    click(ui.window(), SECOND_ROW);
    click(ui.window(), FIRST_ROW);
    assert_eq!(*seen.borrow(), [7, 3], "a row reported its list position, not its branch");
}

#[test]
fn s_and_m_name_the_branch_head_and_do_not_select_it() {
    let ui = harness();
    let selected: Rc<RefCell<Vec<i32>>> = Rc::default();
    let muted: Rc<RefCell<Vec<i32>>> = Rc::default();
    let soloed: Rc<RefCell<Vec<i32>>> = Rc::default();
    let (s, m, o) = (selected.clone(), muted.clone(), soloed.clone());
    ui.on_selected(move |slot| s.borrow_mut().push(slot));
    ui.on_muted(move |slot| m.borrow_mut().push(slot));
    ui.on_soloed(move |slot| o.borrow_mut().push(slot));
    click(ui.window(), SECOND_SOLO);
    click(ui.window(), SECOND_MUTE);
    assert_eq!(*soloed.borrow(), [7]);
    assert_eq!(*muted.borrow(), [7]);
    assert!(
        selected.borrow().is_empty(),
        "pressing S or M also selected the row: {:?}",
        selected.borrow()
    );
}

#[test]
fn the_plus_under_the_list_asks_for_a_branch() {
    let ui = harness();
    let asked = Rc::new(RefCell::new(0));
    let count = asked.clone();
    ui.on_added(move || *count.borrow_mut() += 1);
    click(ui.window(), PLUS);
    assert_eq!(*asked.borrow(), 1);
}
