//! Reordering the channel rack by dragging a name plate, driven by real
//! pointer events.
//!
//! Modelled on `rack_reorder.rs`, and for the same reason: the landing must
//! be the row the pointer is actually over, reported by that row from its own
//! bounds, rather than anything derived from the pointer's *travel*. That
//! distinction is what `rack_reorder.rs` exists to defend for devices, and
//! the channel rack now shares the pattern, so it should share the test.
//!
//! What this file adds beyond the device case is the two-index callback. A
//! channel reorder reports `(from, to)` rather than just the landing, because
//! the same press that starts the drag also *selects* the row -- so a gesture
//! that asked "move the selected channel to here" would depend on a selection
//! it was itself setting.
//!
//! Coordinates are computed rather than searched, for the reason
//! `rack_reorder.rs` records: the `ElementHandle` search API needs a build
//! with debug info, and this geometry is fixed and known.

use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

slint::slint! {
    import { ChannelDrag } from "../ui/channel-rack.slint";

    // The channel rack row as `main.slint` lays it out, reduced to the parts
    // the drag touches: a wrapper that keeps its seat in the layout and
    // answers `hot` from its own bounds, an 88px name plate that owns the
    // grab, and contents that slide. Everything about the reorder lives in
    // those three, so the harness needs no knobs, no step grid and no
    // `MainWindow`.
    export component ChannelReorderHarness inherits Window {
        width: 400px;
        height: 400px;
        background: #101010;
        out property <length> row-height: 28px;
        out property <length> rack-padding: 8px;
        out property <length> row-spacing: 2px;
        in property <[string]> names;
        callback reordered(int, int);
        callback selected(int);

        VerticalLayout {
            spacing: root.row-spacing;
            padding: root.rack-padding;
            alignment: start;

            for name[ch] in root.names : channel-row := Rectangle {
                height: root.row-height;
                property <bool> dragging: ChannelDrag.source == ch;
                property <bool> hot: ChannelDrag.active
                    && ChannelDrag.pointer-y >= self.absolute-position.y
                    && ChannelDrag.pointer-y < self.absolute-position.y + self.height;
                changed hot => { if (self.hot) { ChannelDrag.target = ch; } }
                changed dragging => {
                    if (self.dragging) { ChannelDrag.source-height = self.height; }
                }
                property <length> shift:
                    !ChannelDrag.active || ChannelDrag.target < 0 || self.dragging ? 0px
                    : (ChannelDrag.source < ChannelDrag.target
                        && ch > ChannelDrag.source
                        && ch <= ChannelDrag.target) ? -ChannelDrag.source-height
                    : (ChannelDrag.target < ChannelDrag.source
                        && ch >= ChannelDrag.target
                        && ch < ChannelDrag.source) ? ChannelDrag.source-height
                    : 0px;

                HorizontalLayout {
                    y: channel-row.shift + (channel-row.dragging ? ChannelDrag.dy : 0px);
                    alignment: start;
                    Rectangle {
                        width: 88px;
                        height: channel-row.height;
                        background: #303030;
                        TouchArea {
                            pointer-event(e) => {
                                if (e.kind == PointerEventKind.down) {
                                    root.selected(ch);
                                    if (e.button != PointerEventButton.right) {
                                        ChannelDrag.source = ch;
                                        ChannelDrag.target = ch;
                                        ChannelDrag.dy = 0px;
                                        ChannelDrag.grab-y = self.absolute-position.y + self.mouse-y;
                                        ChannelDrag.pointer-y = ChannelDrag.grab-y;
                                        ChannelDrag.active = true;
                                    }
                                }
                                if (e.kind == PointerEventKind.up && ChannelDrag.active) {
                                    if (ChannelDrag.target != ch && ChannelDrag.target >= 0) {
                                        root.reordered(ch,
                                            clamp(ChannelDrag.target, 0, root.names.length - 1));
                                    }
                                    ChannelDrag.end();
                                }
                            }
                            moved => {
                                if (ChannelDrag.active) {
                                    ChannelDrag.pointer-y = self.absolute-position.y + self.mouse-y;
                                    ChannelDrag.dy = ChannelDrag.pointer-y - ChannelDrag.grab-y;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Row geometry read off the harness rather than restated here, for the
/// reason `rack_reorder.rs` gives: a test about which row a drop lands on
/// should not also be a second copy of the layout's measurements.
struct Geometry {
    height: f32,
    padding: f32,
    spacing: f32,
}

impl Geometry {
    fn of(ui: &ChannelReorderHarness) -> Self {
        Self {
            height: ui.get_row_height(),
            padding: ui.get_rack_padding(),
            spacing: ui.get_row_spacing(),
        }
    }

    /// The vertical middle of row `index`.
    fn middle_of(&self, index: usize) -> f32 {
        self.padding + index as f32 * (self.height + self.spacing) + self.height / 2.0
    }
}

fn harness() -> ChannelReorderHarness {
    i_slint_backend_testing::init_no_event_loop();
    let ui = ChannelReorderHarness::new().unwrap();
    ui.window().set_size(LogicalSize::new(400.0, 400.0));
    ui.set_names(ModelRc::from(Rc::new(VecModel::from(vec![
        slint::SharedString::from("Kick"),
        slint::SharedString::from("Snare"),
        slint::SharedString::from("Closed Hat"),
        slint::SharedString::from("Open Hat"),
        slint::SharedString::from("Bass"),
    ]))));
    ui
}

/// Press on a name plate at `from`, travel to `to` the way a real pointer
/// would, and release there.
fn drag(window: &slint::Window, from: f32, to: f32) {
    let at = |y: f32| LogicalPosition::new(40.0, y);
    window.dispatch_event(WindowEvent::PointerMoved { position: at(from) });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: at(from),
        button: PointerEventButton::Left,
    });
    const INCREMENTS: usize = 24;
    for i in 1..=INCREMENTS {
        let y = from + (to - from) * i as f32 / INCREMENTS as f32;
        window.dispatch_event(WindowEvent::PointerMoved { position: at(y) });
    }
    window.dispatch_event(WindowEvent::PointerReleased {
        position: at(to),
        button: PointerEventButton::Left,
    });
}

/// One test and one harness, for the reason `rack_reorder.rs` records at
/// length: `ChannelDrag` is a Slint global and the testing backend is
/// process-wide, so two of these as separate `#[test]`s would share state the
/// moment cargo ran them on different threads. Sequenced here there is
/// nothing to interleave.
#[test]
fn a_channel_lands_on_the_row_it_was_dropped_on() {
    let ui = harness();
    let g = Geometry::of(&ui);
    let moves: Rc<RefCell<Vec<(i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    let picks: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = moves.clone();
    let chosen = picks.clone();
    ui.on_reordered(move |from, to| seen.borrow_mut().push((from, to)));
    ui.on_selected(move |index| chosen.borrow_mut().push(index));

    // The acceptance case: three rows up.
    drag(ui.window(), g.middle_of(3), g.middle_of(0));
    assert_eq!(
        *moves.borrow(),
        vec![(3, 0)],
        "a channel dropped on row 0 did not land on row 0"
    );
    // The press selects, whatever the release then does. That is the
    // existing behaviour of the plate and the drag must not have taken it
    // away -- a channel you meant to select is one you often then drag.
    assert_eq!(*picks.borrow(), vec![3]);

    // Downwards, which is the direction whose renumbering differs.
    moves.borrow_mut().clear();
    picks.borrow_mut().clear();
    drag(ui.window(), g.middle_of(0), g.middle_of(4));
    assert_eq!(*moves.borrow(), vec![(0, 4)]);

    // A press that never leaves its own row is a selection and nothing else,
    // however far the pointer wandered inside it. No distance threshold is
    // involved: the row under the pointer *is* the answer.
    moves.borrow_mut().clear();
    picks.borrow_mut().clear();
    let inside = g.middle_of(2);
    drag(ui.window(), inside - g.height / 4.0, inside + g.height / 4.0);
    assert!(
        moves.borrow().is_empty(),
        "a drag inside one row reported a move: {:?}",
        moves.borrow()
    );
    assert_eq!(*picks.borrow(), vec![2]);

    // And the drag leaves no state behind for the next one to read.
    moves.borrow_mut().clear();
    drag(ui.window(), g.middle_of(1), g.middle_of(2));
    assert_eq!(*moves.borrow(), vec![(1, 2)]);
}
