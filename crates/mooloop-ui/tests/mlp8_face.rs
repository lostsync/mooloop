//! The ML-P8 face's three controls that are not a plain two-way binding, and
//! the one requirement no property can express.
//!
//! A network cell, the LFO's RATE dial and a route row's depth field each read
//! something other than a property Rust sets: a per-id model, an expression
//! over two properties, and a row of a list. All three are shapes where a
//! knob's own write to its `value` drops the binding that feeds it, and none
//! of them is visible to a test that invokes the callbacks -- the defect is
//! entirely in what the widget does to its own property on the way past. So
//! the face is driven through `MlP8DeviceDragHarness` with real pointer events
//! at known coordinates, for the reason `first_click.rs` records: the
//! `ElementHandle` search API needs a build with `SLINT_EMIT_DEBUG_INFO=1`,
//! which this workspace only does under the `mcp` feature.
//!
//! Where a control reports rather than writes, the owner's half of the loop is
//! stood in for here -- `on_route_amount_changed` below is
//! `touch_mlp8_route_amount` with the engine left out -- so what these tests
//! hold is the widget's half. Every one of them was run against the face as it
//! stood before the fix, which is the only thing that makes them evidence: the
//! route-depth test was pointed at the value *field* first and passed there,
//! because the field is a binding the knob never touches.
//!
//! The fourth thing here is `MODULATION.md`'s rule that every legal control
//! becomes *visibly* assignable when a source is armed, and that an overlay
//! shows the resulting excursion. A knob gets both from its ring. A network
//! cell has no ring, drew neither, and so read as a region modulation could
//! not reach -- while the gesture underneath it had worked all along. That is
//! a claim about pixels and it is asserted about pixels.

use mooloop_core::mlp8;
use mooloop_ui::{MlP8DeviceDragHarness, MlP8RouteRow};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

mod common;

/// Four rack units of `DeviceRackMetrics` minus the two rails, by
/// `face-height` minus the device header. The same size the rack gives it.
const FACE_WIDTH: f32 = 884.0;
const FACE_HEIGHT: f32 = 240.0;

const NETWORK_PAGE: i32 = 1;
const MOD_PAGE: i32 = 4;

/// The `OSC 1 -> OSC 2` cell: row one, column two of the network grid, which
/// is `PARAM_XMOD_BASE`. Fixed rather than searched; if the grid moves, the
/// "the drag reached the cell" assertion fails rather than the test quietly
/// proving nothing.
const XMOD_CELL: (f32, f32) = (366.0, 75.0);
/// The cell's own rectangle, for the comparisons that are about what it drew.
const XMOD_CELL_BOX: (u32, u32, u32, u32) = (269, 63, 196, 26);

/// The centre of the ML-P8 MOD page's RATE dial, and the dial alone: the
/// value field below it is a separate binding the knob never writes, so it
/// would keep following the patch and hide the defect.
const RATE_DIAL: (f32, f32) = (394.0, 138.0);
const RATE_DIAL_BOX: (u32, u32, u32, u32) = (377, 121, 34, 34);

/// The first route row's depth knob, and its dial alone.
///
/// The dial, not the field beside it: the field is a separate binding that
/// the knob never writes, so it goes on following the row whatever the knob
/// does to itself and would hide the defect entirely. This test watched the
/// field first and passed against the unfixed face, which is the whole of
/// what `ds01_face.rs` says about picking a region.
const ROUTE_DEPTH_KNOB: (f32, f32) = (794.0, 67.0);
const ROUTE_DEPTH_DIAL: (u32, u32, u32, u32) = (785, 58, 18, 18);

/// A drag of this many pixels is this much of a parameter's travel:
/// `ParameterKnob` spends its whole range over 150px.
const KNOB_TRAVEL: f32 = 150.0;

fn init_software_backend() {
    common::install_testing_backend();
}

fn harness() -> MlP8DeviceDragHarness {
    init_software_backend();
    let harness = MlP8DeviceDragHarness::new().unwrap();
    harness
        .window()
        .set_size(LogicalSize::new(FACE_WIDTH, FACE_HEIGHT));
    harness
}

/// The per-id arrays the face reads, the size `descriptor_slots` makes them.
fn slots() -> usize {
    mlp8::DESCRIPTORS
        .iter()
        .map(|descriptor| descriptor.id as usize + 1)
        .max()
        .unwrap_or(0)
}

/// Nothing routed anywhere, and every continuous destination open -- which is
/// what `ModDestinationDescriptor::for_param` says about this face's
/// percentages.
fn quiet_modulation(harness: &MlP8DeviceDragHarness) {
    let len = slots();
    harness.set_modulation_depths(ModelRc::from(Rc::new(VecModel::from(vec![0.0_f32; len]))));
    harness.set_modulation_allowed(ModelRc::from(Rc::new(VecModel::from(vec![true; len]))));
    harness.set_modulation_offsets(ModelRc::from(Rc::new(VecModel::from(vec![0.0_f32; len]))));
    harness.set_modulation_route_counts(ModelRc::from(Rc::new(VecModel::from(vec![0_i32; len]))));
}

fn set_slot<T: Clone + 'static>(model: &ModelRc<T>, id: u32, value: T) {
    model.set_row_data(id as usize, value);
}

fn drag(window: &slint::Window, from: (f32, f32), to: (f32, f32)) {
    let at = |p: (f32, f32)| LogicalPosition::new(p.0, p.1);
    window.dispatch_event(WindowEvent::PointerMoved { position: at(from) });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: at(from),
        button: PointerEventButton::Left,
    });
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        window.dispatch_event(WindowEvent::PointerMoved {
            position: at((from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)),
        });
    }
    window.dispatch_event(WindowEvent::PointerReleased {
        position: at(to),
        button: PointerEventButton::Left,
    });
}

/// The pixels of one control, so a comparison can be about that control.
///
/// A whole-frame comparison cannot answer these questions: a drag leaves the
/// control focused and the pointer hovering it, so two renders of the same
/// patch differ for reasons that have nothing to do with the patch.
fn region(harness: &MlP8DeviceDragHarness, (x, y, w, h): (u32, u32, u32, u32)) -> Vec<u8> {
    let snapshot = harness.window().take_snapshot().unwrap();
    let bytes = snapshot.as_bytes();
    let stride = snapshot.width() as usize * 4;
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for row in y..y + h {
        let from = row as usize * stride + x as usize * 4;
        out.extend_from_slice(&bytes[from..from + w as usize * 4]);
    }
    out
}

/// Whether a region holds a control rather than empty page.
///
/// Every comparison below concludes something from two renders being equal or
/// unequal, and a region that drifted off its control would answer both
/// questions with background. Asked of the *first* render of each, so a moved
/// layout fails saying so rather than passing on a coincidence.
fn drawn_on(pixels: &[u8]) -> bool {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .any(|pixel| pixel[..3] != pixels[..3])
}

/// A network cell is a horizontal slider, so it answers a horizontal drag.
///
/// Both halves matter. The cell is seven times wider than it is tall and its
/// bar runs along the long axis; a control drawn that way and dragged the
/// other way is a control nobody finds. And the vertical case is not merely
/// unasserted -- a cell that still moved on a vertical drag would be reading
/// an axis it does not draw.
#[test]
fn a_network_cell_is_dragged_along_the_axis_it_is_drawn_on() {
    let harness = harness();
    harness.set_page(NETWORK_PAGE);
    quiet_modulation(&harness);
    harness.set_xmod12(0.0);

    drag(
        harness.window(),
        (XMOD_CELL.0 - 30.0, XMOD_CELL.1),
        (XMOD_CELL.0 + 30.0, XMOD_CELL.1),
    );
    let sideways = harness.get_xmod12();
    assert!(
        sideways > 30.0,
        "a 60px drag across the cell should be most of a half turn, not {sideways}"
    );

    harness.set_xmod12(0.0);
    drag(
        harness.window(),
        XMOD_CELL,
        (XMOD_CELL.0, XMOD_CELL.1 - 60.0),
    );
    assert_eq!(
        harness.get_xmod12(),
        0.0,
        "the cell moved on a vertical drag, which is not the axis it is drawn on"
    );
}

/// An armed source reaches the network, and the cell says so while it does.
///
/// The gesture is asserted first and the pixels second, because they failed
/// separately: the depth reached the rack all along, and the cell drew
/// nothing about it -- no armed state, no excursion, no base marker -- so the
/// whole grid read as a region modulation could not target. `MODULATION.md`
/// requires the opposite in as many words.
#[test]
fn an_armed_source_reaches_a_network_cell_and_the_cell_shows_it() {
    let harness = harness();
    harness.set_page(NETWORK_PAGE);
    quiet_modulation(&harness);
    harness.set_xmod12(0.0);

    let assignments: Rc<RefCell<Vec<(i32, f32)>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = assignments.clone();
    harness.on_modulation_depth_changed(move |param, depth| {
        seen.borrow_mut().push((param, depth));
    });

    let idle = region(&harness, XMOD_CELL_BOX);
    assert!(
        drawn_on(&idle),
        "nothing is drawn where the cell should be, so this test is about the \
         wrong pixels"
    );
    harness.set_modulation_armed(true);
    let armed = region(&harness, XMOD_CELL_BOX);
    assert_ne!(
        idle, armed,
        "arming a source left the cell looking exactly as it did, so nothing \
         on this page says it can be assigned to"
    );

    drag(
        harness.window(),
        (XMOD_CELL.0 - 30.0, XMOD_CELL.1),
        (XMOD_CELL.0 + 30.0, XMOD_CELL.1),
    );

    let assignments = assignments.borrow();
    let (param, depth) = *assignments
        .last()
        .expect("dragging an armed cell should author a route depth");
    assert_eq!(
        param as u32,
        mlp8::PARAM_XMOD_BASE,
        "the cell reported a depth for the wrong destination"
    );
    let expected = 60.0 / KNOB_TRAVEL;
    assert!(
        (depth - expected).abs() < 0.02,
        "a 60px drag should be {expected} of the destination's range, not {depth}"
    );
    assert_eq!(
        harness.get_xmod12(),
        0.0,
        "an armed drag moved the authored value as well as the route depth"
    );

    // The excursion overlay, which is the other half of the requirement: the
    // rack now holds the depth, and the cell has to draw what it will do.
    set_slot(
        &harness.get_modulation_depths(),
        mlp8::PARAM_XMOD_BASE,
        depth,
    );
    assert_ne!(
        armed,
        region(&harness, XMOD_CELL_BOX),
        "the cell drew the same thing with a route on it as with none, so the \
         depth a drag just authored is invisible"
    );
}

/// A cell that is not being assigned to shows what the running sources are
/// doing to it, the way a knob's arc moves.
#[test]
fn a_modulated_network_cell_draws_the_offset() {
    let harness = harness();
    harness.set_page(NETWORK_PAGE);
    quiet_modulation(&harness);
    harness.set_xmod12(0.0);

    let still = region(&harness, XMOD_CELL_BOX);
    assert!(
        drawn_on(&still),
        "nothing is drawn where the cell should be, so this test is about the \
         wrong pixels"
    );
    set_slot(
        &harness.get_modulation_offsets(),
        mlp8::PARAM_XMOD_BASE,
        0.25,
    );
    assert_ne!(
        still,
        region(&harness, XMOD_CELL_BOX),
        "a live modulation offset moved nothing on the cell"
    );
}

/// The LFO's rate dial has to change the rate, and go on following it.
///
/// Its `value` is an expression over the free rate and the division, so the
/// dial must not write it -- and something else then has to, or the knob
/// moves nothing at all. Both halves are one fix and this asserts both: the
/// drag changes the rate the face holds, and a rate set from outside
/// afterwards still redraws the dial.
#[test]
fn the_lfo_rate_dial_changes_the_rate_and_keeps_following_it() {
    let harness = harness();
    harness.set_page(MOD_PAGE);
    quiet_modulation(&harness);
    harness.set_lfo_synced(false);
    harness.set_lfo_rate_hz(2.0);

    drag(
        harness.window(),
        RATE_DIAL,
        (RATE_DIAL.0, RATE_DIAL.1 - 40.0),
    );
    let dragged = harness.get_lfo_rate_hz();
    assert!(
        dragged > 2.5,
        "dragging the rate dial upwards left the rate at {dragged} Hz"
    );

    harness.set_lfo_rate_hz(0.05);
    let slow = region(&harness, RATE_DIAL_BOX);
    assert!(
        drawn_on(&slow),
        "nothing is drawn where the rate dial should be, so this test is about \
         the wrong pixels"
    );
    harness.set_lfo_rate_hz(80.0);
    let fast = region(&harness, RATE_DIAL_BOX);
    assert_ne!(
        slow, fast,
        "the dial drew the same thing at 0.05 Hz and 80 Hz once it had been \
         dragged, so it stopped following the patch"
    );
}

/// A route row's depth is a field of a model row, so the knob reports and the
/// owner writes.
///
/// Both halves are needed and only one of them is asserted here. The widget's
/// half is: a knob that wrote its own value would drop the binding onto
/// `route.amount` and stop following the row -- that is what the dial
/// comparison at the end catches. The owner's half is `touch_mlp8_route_amount`,
/// stood in for below by the handler this test installs, exactly as
/// `ds01_face.rs` stands in for `touch_ds01_param`; without it the knob moves
/// nothing at all, which is why the drag is asserted to have reached the
/// route before anything else is concluded.
#[test]
fn a_route_depth_field_follows_the_route_it_was_dragged_on() {
    let harness = harness();
    harness.set_page(MOD_PAGE);
    quiet_modulation(&harness);
    harness.set_route_source_names(ModelRc::from(Rc::new(VecModel::from(vec![
        SharedString::from("LFO"),
        SharedString::from("Env"),
    ]))));
    harness.set_route_dest_names(ModelRc::from(Rc::new(VecModel::from(vec![
        SharedString::from("Cutoff"),
        SharedString::from("Reso"),
    ]))));
    let rows = Rc::new(VecModel::from(vec![MlP8RouteRow {
        id: 0,
        source: 0,
        dest: 0,
        amount: 0.0,
        dest_name: SharedString::from("Cutoff"),
        bipolar: true,
    }]));
    harness.set_routes(ModelRc::from(rows.clone()));

    // `touch_mlp8_route_amount` with the engine left out: the edit lands in
    // the row, and the row is what the field reads.
    let owned = rows.clone();
    harness.on_route_amount_changed(move |id, amount| {
        let Some(index) = (0..owned.row_count())
            .find(|index| owned.row_data(*index).is_some_and(|row| row.id == id))
        else {
            return;
        };
        let Some(mut row) = owned.row_data(index) else {
            return;
        };
        row.amount = amount;
        owned.set_row_data(index, row);
    });

    let parked = region(&harness, ROUTE_DEPTH_DIAL);
    assert!(
        drawn_on(&parked),
        "nothing is drawn where the depth dial should be, so this test is \
         about the wrong pixels"
    );
    drag(
        harness.window(),
        ROUTE_DEPTH_KNOB,
        (ROUTE_DEPTH_KNOB.0, ROUTE_DEPTH_KNOB.1 - 45.0),
    );
    let authored = rows.row_data(0).expect("the row is still there").amount;
    assert!(
        authored > 10.0,
        "dragging the depth knob upwards left the route at {authored}%"
    );
    let dragged = region(&harness, ROUTE_DEPTH_DIAL);
    assert_ne!(
        parked, dragged,
        "the drag did not reach the dial, so the rest of this test proves nothing"
    );

    // And the row is still the one place the depth lives: a depth set from
    // outside -- an undo, a preset, the other end of the same route -- has to
    // reach the dial the drag went through.
    let mut row = rows.row_data(0).unwrap();
    row.amount = -88.0;
    rows.set_row_data(0, row);
    assert_ne!(
        dragged,
        region(&harness, ROUTE_DEPTH_DIAL),
        "the dial drew the same thing at +{authored}% and -88%, so it stopped \
         following its row once it had been dragged"
    );
}
