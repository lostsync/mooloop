//! Reordering the rack by dragging a device header, driven by real pointer
//! events.
//!
//! What this exists to catch: the reorder used to work the landing out from
//! the pointer's *travel*, divided by a nominal 240px pitch -- the narrowest
//! face plus its join. Every device wider than that made it wrong, and
//! wrong in a way no test could see, because the callback fired with a
//! plausible number every time. Dragging a device from the far side of a
//! wide EQ landed it three rows short of where it was dropped.
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

    struct HarnessRow { face-width: length, depth: int, children: int, container: bool }

    // The rack row as `main.slint` lays it out: a cell per device holding the
    // box slices, the host frame and the face's shell, with the cell owning
    // the geometry the drag reads. Everything about the reorder lives in
    // these two pieces, so the harness needs no faces and no `MainWindow`.
    export component ReorderHarness inherits Window {
        width: 1700px;
        height: 320px;
        // The rack's own numbers, so the coordinates below are derived from
        // the layout rather than from a copy of it that can drift.
        out property <length> rail-width: DeviceRackMetrics.rail-width;
        out property <length> join-width: DeviceRackMetrics.join-width;
        out property <length> rack-padding: DeviceRackMetrics.rack-padding;
        out property <length> header-height: DeviceRackMetrics.header-height;
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
                    row.face-width + DeviceRackMetrics.rail-width * 2;
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

/// Rack geometry, read off the harness rather than restated here: this
/// file is about which row a drop lands on, and it should not also be a
/// second copy of the rack's measurements that can quietly disagree with the
/// first one. It did disagree once, when `rack-padding` moved.
struct Geometry {
    rail: f32,
    join: f32,
    header_y: f32,
}

impl Geometry {
    fn of(ui: &ReorderHarness) -> Self {
        Self {
            rail: ui.get_rail_width(),
            join: ui.get_join_width(),
            header_y: ui.get_rack_padding() + ui.get_header_height() / 2.0,
        }
    }

    /// A row's full pitch: face, both rails and the join.
    fn cell(&self, face_width: f32) -> f32 {
        face_width + self.rail * 2.0 + self.join
    }
}

/// A narrow face, and a face three times as wide plus the gaps between.
const NARROW: f32 = 220.0;
const WIDE: f32 = 668.0;

fn row(face_width: f32) -> HarnessRow {
    HarnessRow {
        face_width,
        depth: 0,
        children: 0,
        container: false,
    }
}

/// One narrow device, one wide one, then two more narrow ones: the layout that
/// makes travel-based and bounds-based landings disagree.
fn harness() -> ReorderHarness {
    i_slint_backend_testing::init_no_event_loop();
    let ui = ReorderHarness::new().unwrap();
    ui.window().set_size(LogicalSize::new(1700.0, 320.0));
    ui.set_rows(ModelRc::from(Rc::new(VecModel::from(vec![
        row(NARROW),
        row(WIDE),
        row(NARROW),
        row(NARROW),
    ]))));
    ui
}

/// Press on a device header at `from`, travel to `to` the way a real pointer
/// would, and release there.
fn drag(window: &slint::Window, from: f32, to: f32, header_y: f32) {
    let at = |x: f32| LogicalPosition::new(x, header_y);
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

/// One test rather than two, and one harness rather than two, on purpose.
///
/// `RackDrag` is a Slint global and the testing backend is process-wide, so
/// two of these running as separate `#[test]`s share state the moment cargo
/// runs them on different threads -- which it does by default. That is not
/// hypothetical: as a pair they reported one drag's grab against the other
/// drag's landing, and the reversed tuple `(1, 2)` for a drop that was
/// `(2, 1)`. Sequenced here, there is nothing to interleave.
#[test]
fn a_drop_lands_on_the_row_it_was_made_on() {
    let ui = harness();
    let g = Geometry::of(&ui);
    let moves: Rc<RefCell<Vec<(i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    let picks: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = moves.clone();
    let chosen = picks.clone();
    ui.on_reordered(move |from, to| seen.borrow_mut().push((from, to)));
    ui.on_selected(move |index| chosen.borrow_mut().push(index));

    // Row 2 sits past the wide row. Grab its header and drop it on row 1.
    let row_two_x = g.cell(NARROW) + g.cell(WIDE);
    let grab = row_two_x + g.rail + 30.0;
    let drop_on_row_one = g.cell(NARROW) + 300.0;
    drag(ui.window(), grab, drop_on_row_one, g.header_y);

    assert_eq!(
        *moves.borrow(),
        vec![(2, 1)],
        "a device dropped on row 1 did not land on row 1. Travel-based \
         arithmetic reports 0 here: the pointer moved {:.0}px, which is three \
         nominal narrow pitches, but only two rows -- the wide device is one \
         row and three pitches wide.",
        grab - drop_on_row_one
    );
    assert!(
        picks.borrow().is_empty(),
        "a drag that moved a device also selected something: {:?}",
        picks.borrow()
    );

    // A wander of 120px, well past the old half-pitch threshold, but entirely
    // inside the wide row's own bounds. A row is not a distance.
    moves.borrow_mut().clear();
    let inside_row_one = g.cell(NARROW) + g.rail + 20.0;
    drag(ui.window(), inside_row_one, inside_row_one + 120.0, g.header_y);

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
