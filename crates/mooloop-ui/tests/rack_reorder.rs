//! Reordering the rack by dragging a device header, driven by real pointer
//! events.
//!
//! What this exists to catch: the reorder used to work the landing out from
//! the pointer's *travel*, divided by `unit-width + 20px` -- a nominal
//! one-unit pitch. Every device wider than one rack unit made that wrong, and
//! wrong in a way no test could see, because the callback fired with a
//! plausible number every time. Dragging a device from the far side of a
//! four-unit EQ landed it three rows short of where it was dropped.
//!
//! The landing is now the row the pointer is actually over, reported by that
//! row from its own bounds. So the case below is the whole point: the drag
//! crosses one three-unit device, and travel-based arithmetic and
//! bounds-based lookup disagree about where it ends up.
//!
//! Coordinates are computed rather than searched, for the reason
//! `effect_preset_menu.rs` and `first_click.rs` both record: the
//! `ElementHandle` search API needs a build with debug info, and the rack's
//! geometry is fixed and known.

use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

slint::slint! {
    import { ContainerEnclosure, DeviceFrame, DeviceRackMetrics, EffectDeviceShell, RackDrag }
        from "../ui/device-rack.slint";

    struct HarnessRow { units: int, depth: int, children: int, container: bool }

    // The rack row as `main.slint` lays it out: a cell per device holding the
    // box slices, the host frame and the face's shell, with the cell owning
    // the geometry the drag reads. Everything about the reorder lives in
    // these two pieces, so the harness needs no faces and no `MainWindow`.
    export component ReorderHarness inherits Window {
        width: 1700px;
        height: 320px;
        background: #101010;
        in property <[HarnessRow]> rows;
        callback reordered(int, int);
        callback selected(int);

        HorizontalLayout {
            padding-left: 0px;
            padding-right: 0px;
            padding-top: DeviceRackMetrics.rack-padding;
            padding-bottom: DeviceRackMetrics.rack-padding;
            spacing: 0px;
            alignment: start;

            for row[index] in root.rows : cell := Rectangle {
                property <length> frame-width:
                    DeviceRackMetrics.unit-width * row.units
                    + DeviceRackMetrics.half-gap * (row.units - 1)
                    + DeviceRackMetrics.rail-width * 2;
                property <int> next-depth: index + 1 < root.rows.length
                    ? root.rows[index + 1].depth : 0;
                property <bool> dragging: RackDrag.source == index;
                property <bool> hot: RackDrag.active
                    && RackDrag.pointer-x >= self.absolute-position.x
                    && RackDrag.pointer-x < self.absolute-position.x + self.width;
                changed hot => { if (self.hot) { RackDrag.target = index; } }
                property <length> shift:
                    !RackDrag.active || RackDrag.target < 0 || self.dragging ? 0px
                    : (RackDrag.source < RackDrag.target && index > RackDrag.source
                        && index <= RackDrag.target) ? -RackDrag.source-width
                    : (RackDrag.target < RackDrag.source && index >= RackDrag.target
                        && index < RackDrag.source) ? RackDrag.source-width
                    : 0px;

                width: self.frame-width + DeviceRackMetrics.join-width;
                height: DeviceRackMetrics.face-height;

                for level[lvl] in [1, 2, 3, 4] : ContainerEnclosure {
                    property <bool> inside: row.depth >= lvl;
                    property <bool> is-head: row.container && row.depth == lvl - 1;
                    visible: self.inside || self.is-head;
                    level: lvl;
                    head: self.is-head;
                    cap-right: (self.is-head && row.children == 0)
                        || (self.inside && cell.next-depth < lvl);
                    x: cell.shift;
                    width: cell.width;
                }

                DeviceFrame {
                    x: cell.shift + (cell.dragging ? RackDrag.dx : 0px);
                    y: cell.dragging ? -8px : 0px;
                    width: cell.frame-width;
                    height: DeviceRackMetrics.face-height;
                    EffectDeviceShell {
                        x: DeviceRackMetrics.rail-width;
                        y: 0px;
                        width: parent.content-width;
                        height: parent.height;
                        name: "Device";
                        slot-index: index;
                        slot-count: root.rows.length;
                        reorder-requested(target) => { root.reordered(index, target); }
                        select-requested => { root.selected(index); }
                    }
                }
            }
        }
    }
}

/// Rack-unit geometry, spelled out so the expectations below are readable
/// rather than magic. A cell is a face, both rails and the join.
const UNIT: f32 = 220.0;
const HALF_GAP: f32 = 4.0;
const RAIL: f32 = 30.0;
const JOIN: f32 = 20.0;
/// The drag handle covers the header, which starts at the top of the face --
/// itself `rack-padding` below the top of the rack.
const HEADER_Y: f32 = 20.0 + 14.0;

fn cell_width(units: f32) -> f32 {
    UNIT * units + HALF_GAP * (units - 1.0) + RAIL * 2.0 + JOIN
}

fn row(units: i32) -> HarnessRow {
    HarnessRow {
        units,
        depth: 0,
        children: 0,
        container: false,
    }
}

/// One 1U device, one 3U device, then two more 1U devices: the layout that
/// makes travel-based and bounds-based landings disagree.
fn harness() -> ReorderHarness {
    i_slint_backend_testing::init_no_event_loop();
    let ui = ReorderHarness::new().unwrap();
    ui.window().set_size(LogicalSize::new(1700.0, 320.0));
    ui.set_rows(ModelRc::from(Rc::new(VecModel::from(vec![
        row(1),
        row(3),
        row(1),
        row(1),
    ]))));
    ui
}

/// Press on a device header at `from`, travel to `to` the way a real pointer
/// would, and release there.
fn drag(window: &slint::Window, from: f32, to: f32) {
    let at = |x: f32| LogicalPosition::new(x, HEADER_Y);
    window.dispatch_event(WindowEvent::PointerMoved { position: at(from) });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: at(from),
        button: PointerEventButton::Left,
    });
    const INCREMENTS: usize = 24;
    for i in 1..=INCREMENTS {
        let x = from + (to - from) * i as f32 / INCREMENTS as f32;
        window.dispatch_event(WindowEvent::PointerMoved { position: at(x) });
    }
    window.dispatch_event(WindowEvent::PointerReleased {
        position: at(to),
        button: PointerEventButton::Left,
    });
}

#[test]
fn a_drag_lands_on_the_row_it_was_dropped_on_however_wide_the_ones_it_crossed() {
    let ui = harness();
    let moves: Rc<RefCell<Vec<(i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = moves.clone();
    ui.on_reordered(move |from, to| seen.borrow_mut().push((from, to)));

    // Row 2 sits past the 3U row. Grab its header and drop it on row 1.
    let row_two_x = cell_width(1.0) + cell_width(3.0);
    let grab = row_two_x + RAIL + 30.0;
    let drop_on_row_one = cell_width(1.0) + 300.0;
    drag(ui.window(), grab, drop_on_row_one);

    assert_eq!(
        *moves.borrow(),
        vec![(2, 1)],
        "a device dropped on row 1 did not land on row 1. Travel-based \
         arithmetic reports 0 here: the pointer moved {:.0}px, which is three \
         nominal one-unit pitches, but only two rows -- the 3U device is one \
         row and three pitches wide.",
        grab - drop_on_row_one
    );
}

#[test]
fn a_press_that_stays_on_its_own_row_is_a_selection() {
    let ui = harness();
    let moves: Rc<RefCell<Vec<(i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    let picks: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = moves.clone();
    let chosen = picks.clone();
    ui.on_reordered(move |from, to| seen.borrow_mut().push((from, to)));
    ui.on_selected(move |index| chosen.borrow_mut().push(index));

    // A wander of 120px, well past the old half-pitch threshold, but entirely
    // inside the 3U row's own bounds. A row is not a distance.
    let grab = cell_width(1.0) + RAIL + 20.0;
    drag(ui.window(), grab, grab + 120.0);

    assert!(
        moves.borrow().is_empty(),
        "a press that never left its own row reordered the rack: {:?}",
        moves.borrow()
    );
    assert_eq!(
        *picks.borrow(),
        vec![1],
        "a press that never left its own row did not select it"
    );
}
