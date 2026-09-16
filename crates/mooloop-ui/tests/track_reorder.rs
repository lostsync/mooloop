//! Reordering mixer tracks by dragging a strip's name plate, driven by real
//! pointer events.
//!
//! `channel_reorder.rs` one list over, with one deliberate difference: that
//! file rebuilds the rack row inside its own `slint!` block, so what it
//! checks is a copy of the markup `main.slint` runs, and nothing reads that
//! copy. This one imports the real `MixerPane` from `mixer.slint`, so the
//! strips, slots and plates it drags are the ones the application draws.
//!
//! Coordinates are computed from `MixerMetrics` rather than searched, for
//! the reason `rack_reorder.rs` records.

use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

slint::slint! {
    import { MixerPane, MixerMetrics, MixerStripRow } from "../ui/mixer.slint";

    export component TrackReorderHarness inherits Window {
        width: 900px;
        height: 560px;
        in property <[MixerStripRow]> strips;
        in property <bool> pending;
        out property <length> strip-width: MixerMetrics.strip-width;
        out property <length> strip-gap: MixerMetrics.strip-gap;
        out property <length> row-padding: MixerMetrics.row-padding;
        out property <length> strip-padding: MixerMetrics.strip-padding;
        callback reordered(int, int);
        callback selected(int);

        MixerPane {
            strips: root.strips;
            project-edit-pending: root.pending;
            track-reorder-requested(from, to) => { root.reordered(from, to); }
            bus-selected(i) => { root.selected(i); }
        }
    }
}

/// Half a name plate's height. The plate is the first thing in a strip, so
/// its middle is this far below the strip's own padding.
const HALF_PLATE: f32 = 10.0;

/// Strip geometry read off `MixerMetrics`, so this test is not a second copy
/// of the row's measurements.
struct Geometry {
    width: f32,
    gap: f32,
    padding: f32,
    /// The height of a name plate's middle.
    plate_y: f32,
}

impl Geometry {
    fn of(ui: &TrackReorderHarness) -> Self {
        Self {
            width: ui.get_strip_width(),
            gap: ui.get_strip_gap(),
            padding: ui.get_row_padding(),
            plate_y: ui.get_row_padding() + ui.get_strip_padding() + HALF_PLATE,
        }
    }

    /// The horizontal middle of strip `index`.
    fn middle_of(&self, index: usize) -> f32 {
        self.padding + index as f32 * (self.width + self.gap) + self.width / 2.0
    }
}

const NAMES: [&str; 5] = ["Master", "Drums", "Bass", "Keys", "Verb"];

fn harness() -> TrackReorderHarness {
    i_slint_backend_testing::init_no_event_loop();
    let ui = TrackReorderHarness::new().unwrap();
    ui.window().set_size(LogicalSize::new(900.0, 560.0));
    let strips = NAMES
        .iter()
        .enumerate()
        .map(|(index, name)| MixerStripRow {
            name: SharedString::from(*name),
            is_master: index == 0,
            ..Default::default()
        })
        .collect::<Vec<_>>();
    ui.set_strips(ModelRc::from(Rc::new(VecModel::from(strips))));
    ui
}

/// Press on a name plate at `from`, travel to `to` the way a real pointer
/// would, and release there.
fn drag(window: &slint::Window, g: &Geometry, from: f32, to: f32) {
    drag_in_steps(window, g, from, to, 24);
}

/// The same with a chosen number of moves. One move is a pointer that jumps,
/// which is what a fast flick delivers and what the slots with no outer
/// edge are for.
fn drag_in_steps(window: &slint::Window, g: &Geometry, from: f32, to: f32, increments: usize) {
    let at = |x: f32| LogicalPosition::new(x, g.plate_y);
    window.dispatch_event(WindowEvent::PointerMoved { position: at(from) });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: at(from),
        button: PointerEventButton::Left,
    });
    for i in 1..=increments {
        let x = from + (to - from) * i as f32 / increments as f32;
        window.dispatch_event(WindowEvent::PointerMoved { position: at(x) });
    }
    window.dispatch_event(WindowEvent::PointerReleased {
        position: at(to),
        button: PointerEventButton::Left,
    });
}

/// One test and one harness, because `TrackDrag` is a Slint global and the
/// testing backend is process-wide: see `channel_reorder.rs`.
#[test]
fn a_track_lands_on_the_strip_it_was_dropped_on() {
    let ui = harness();
    let g = Geometry::of(&ui);
    let moves: Rc<RefCell<Vec<(i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    let picks: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = moves.clone();
    let chosen = picks.clone();
    ui.on_reordered(move |from, to| seen.borrow_mut().push((from, to)));
    ui.on_selected(move |index| chosen.borrow_mut().push(index));
    let reset = || {
        moves.borrow_mut().clear();
        picks.borrow_mut().clear();
    };

    // Two strips towards the master.
    drag(ui.window(), &g, g.middle_of(3), g.middle_of(1));
    assert_eq!(*moves.borrow(), vec![(3, 1)]);
    // The press selects, whatever the release then does: it is the gesture
    // that points the rack at a track, and a track you meant to select is
    // one you often then drag.
    assert_eq!(*picks.borrow(), vec![3]);

    // Away from it, which is the direction whose renumbering differs. The
    // landing is the strip under the pointer, not one computed from travel:
    // the release is a quarter-strip into strip 3, not a whole pitch.
    reset();
    drag(ui.window(), &g, g.middle_of(1), g.middle_of(3) - g.width / 4.0);
    assert_eq!(*moves.borrow(), vec![(1, 3)]);

    // A press that never leaves its own strip selects and moves nothing,
    // however far the pointer wandered inside it.
    reset();
    let inside = g.middle_of(2);
    drag(ui.window(), &g, inside - g.width / 3.0, inside + g.width / 3.0);
    assert!(
        moves.borrow().is_empty(),
        "a drag inside one strip reported a move: {:?}",
        moves.borrow()
    );
    assert_eq!(*picks.borrow(), vec![2]);

    // The master refuses at the grab: it selects and never starts a drag.
    reset();
    drag(ui.window(), &g, g.middle_of(0), g.middle_of(3));
    assert!(moves.borrow().is_empty(), "the master was moved: {:?}", moves.borrow());
    assert_eq!(*picks.borrow(), vec![0]);

    // Released over the master, the drop is seat 1 -- never seat 0 -- and so
    // is a pointer that leaves the row past the master's left edge, even in
    // one jump.
    reset();
    drag(ui.window(), &g, g.middle_of(3), g.middle_of(0));
    assert_eq!(*moves.borrow(), vec![(3, 1)]);
    reset();
    drag_in_steps(ui.window(), &g, g.middle_of(4), 2.0, 1);
    assert_eq!(*moves.borrow(), vec![(4, 1)]);

    // Over the `+` button, or past it into the empty row, is the last seat.
    let last = NAMES.len() - 1;
    let plus = g.middle_of(last) + g.width / 2.0 + g.gap + 17.0;
    reset();
    drag(ui.window(), &g, g.middle_of(1), plus);
    assert_eq!(*moves.borrow(), vec![(1, last as i32)]);
    reset();
    drag_in_steps(ui.window(), &g, g.middle_of(2), plus + 200.0, 1);
    assert_eq!(*moves.borrow(), vec![(2, last as i32)]);

    // While a previous drop is still installing, the plate only selects.
    reset();
    ui.set_pending(true);
    drag(ui.window(), &g, g.middle_of(3), g.middle_of(1));
    assert!(moves.borrow().is_empty(), "a drag started mid-install: {:?}", moves.borrow());
    assert_eq!(*picks.borrow(), vec![3]);
    ui.set_pending(false);

    // And no drag leaves state behind for the next one to read.
    reset();
    drag(ui.window(), &g, g.middle_of(2), g.middle_of(1));
    assert_eq!(*moves.borrow(), vec![(2, 1)]);
}
