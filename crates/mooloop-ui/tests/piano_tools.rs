//! The piano roll's pointer tools and marquee selection.
//!
//! Like `piano_drag.rs`, these dispatch real pointer-event sequences rather
//! than invoking the callbacks: the grid resolves the tool, the modifiers,
//! and the target note from the press position, and none of that is
//! exercised by calling the callback directly.

use mooloop_ui::{default_piano_gestures, note_hit_test, MainWindow, NoteCell};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

/// Grid geometry in logical pixels, matching `piano_drag.rs`. These move if
/// the editor's left gutter or the toolbar above it is resized.
const GRID_ORIGIN_X: f32 = 54.0;
const GRID_TOP_Y: f32 = 383.0;
const ROW_HEIGHT: f32 = 8.0;
const STEP_WIDTH: f32 = 32.0;
const TICKS_PER_STEP: i32 = 24;
/// The roll's default snap, 1/16, which at 96 PPQ is one step.
const SNAP_TICKS: i32 = 24;
const HIGH_NOTE: i32 = 84;

const TOOL_SELECT: i32 = 0;
const TOOL_DRAW: i32 = 1;
const TOOL_PAINT: i32 = 2;
const TOOL_SLICE: i32 = 3;
const TOOL_ERASE: i32 = 4;

fn note_centre_y(midi_note: i32) -> f32 {
    GRID_TOP_Y + (HIGH_NOTE - midi_note) as f32 * ROW_HEIGHT + ROW_HEIGHT / 2.0
}

fn tick_x(tick: i32) -> f32 {
    GRID_ORIGIN_X + tick as f32 * STEP_WIDTH / TICKS_PER_STEP as f32
}

fn harness(tool: i32, notes: Vec<NoteCell>) -> MainWindow {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(slint::SharedString::from("software")),
        },
    )))
    .ok();
    let ui = MainWindow::new().unwrap();
    ui.set_piano_gestures(default_piano_gestures());
    ui.window().set_size(LogicalSize::new(960.0, 760.0));
    ui.set_editor_page(1);
    ui.set_pattern_length(16);
    ui.set_piano_tool(tool);

    let model = Rc::new(VecModel::from(notes));
    ui.set_notes(ModelRc::from(model.clone()));
    ui.on_piano_note_hit_test(move |tick, midi_note| {
        let notes: Vec<NoteCell> = model.iter().collect();
        note_hit_test(&notes, tick, midi_note)
    });
    ui
}

/// A marquee's reported bounds: start tick, end tick, low note, high note.
type Band = Rc<RefCell<Vec<(i32, i32, i32, i32)>>>;

fn cell(id: i32, start_tick: i32, duration_ticks: i32, note: i32) -> NoteCell {
    NoteCell {
        id,
        start_tick,
        duration_ticks,
        note,
        velocity: 100,
        selected: false,
    }
}

/// Two selected notes spanning ticks 0..120, on different rows.
fn two_selected_notes() -> Vec<NoteCell> {
    let mut notes = vec![
        cell(7, 0, SNAP_TICKS, 60),
        cell(8, 4 * SNAP_TICKS, SNAP_TICKS, 62),
    ];
    for note in &mut notes {
        note.selected = true;
    }
    notes
}

/// What `refresh_selection_bounds` would publish for `two_selected_notes`.
fn set_selection_bounds(ui: &MainWindow) {
    ui.set_selection_count(2);
    ui.set_selection_start_tick(0);
    ui.set_selection_end_tick(5 * SNAP_TICKS);
    ui.set_selection_low_note(60);
    ui.set_selection_high_note(62);
}

fn press(window: &slint::Window, at: (f32, f32), button: PointerEventButton) {
    let pos = LogicalPosition::new(at.0, at.1);
    window.dispatch_event(WindowEvent::PointerMoved { position: pos });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: pos,
        button,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position: pos,
        button,
    });
}

/// Press at `from`, travel to `to` in several steps, then release. Several
/// moves matter for the same reason they do in `piano_drag.rs`: a paint
/// stroke or an erase sweep only shows its per-cell behaviour after the
/// second frame.
fn drag_with(
    window: &slint::Window,
    from: (f32, f32),
    to: (f32, f32),
    button: PointerEventButton,
    modifier: Option<Key>,
) {
    if let Some(key) = modifier {
        window.dispatch_event(WindowEvent::KeyPressed { text: key.into() });
    }
    let pos = |(x, y): (f32, f32)| LogicalPosition::new(x, y);
    window.dispatch_event(WindowEvent::PointerMoved { position: pos(from) });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: pos(from),
        button,
    });
    const STEPS: usize = 8;
    for i in 1..=STEPS {
        let t = i as f32 / STEPS as f32;
        window.dispatch_event(WindowEvent::PointerMoved {
            position: pos((from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)),
        });
    }
    window.dispatch_event(WindowEvent::PointerReleased {
        position: pos(to),
        button,
    });
    if let Some(key) = modifier {
        window.dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    }
}

fn drag(window: &slint::Window, from: (f32, f32), to: (f32, f32)) {
    drag_with(window, from, to, PointerEventButton::Left, None);
}

// ---- marquee ------------------------------------------------------------

/// A band drawn across empty grid selects what it crosses. The band reports
/// its own tick/note bounds and Rust resolves which notes those catch, so
/// what this pins is the bounds the grid measures, not the hit rule.
#[test]
fn dragging_empty_grid_reports_the_band_it_swept() {
    let ui = harness(TOOL_SELECT, vec![cell(7, 0, TICKS_PER_STEP, 60)]);
    let bands: Band = Rc::new(RefCell::new(Vec::new()));
    let modes: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let bands = bands.clone();
        ui.on_piano_marquee_updated(move |a, b, lo, hi| bands.borrow_mut().push((a, b, lo, hi)));
    }
    {
        let modes = modes.clone();
        ui.on_piano_marquee_begin(move |mode| modes.borrow_mut().push(mode));
    }

    // From an empty cell above the note, down and right past it.
    drag(
        ui.window(),
        (tick_x(2 * TICKS_PER_STEP), note_centre_y(66)),
        (tick_x(6 * TICKS_PER_STEP), note_centre_y(58)),
    );

    assert_eq!(*modes.borrow(), vec![0], "a plain band replaces the selection");
    let bands = bands.borrow();
    let (start, end, low, high) = *bands.last().expect("the band should have reported bounds");
    assert_eq!(start, 2 * TICKS_PER_STEP);
    assert_eq!(end, 6 * TICKS_PER_STEP);
    assert_eq!(
        (low, high),
        (58, 66),
        "the band should report low..high whichever way it was drawn"
    );
}

/// Dragging upward and leftward has to report the same rectangle as dragging
/// down and right across the same two corners.
#[test]
fn a_band_drawn_backwards_reports_the_same_rectangle() {
    let ui = harness(TOOL_SELECT, vec![cell(7, 0, TICKS_PER_STEP, 60)]);
    let bands: Band = Rc::new(RefCell::new(Vec::new()));
    {
        let bands = bands.clone();
        ui.on_piano_marquee_updated(move |a, b, lo, hi| bands.borrow_mut().push((a, b, lo, hi)));
    }

    drag(
        ui.window(),
        (tick_x(6 * TICKS_PER_STEP), note_centre_y(58)),
        (tick_x(2 * TICKS_PER_STEP), note_centre_y(66)),
    );

    let bands = bands.borrow();
    assert_eq!(
        *bands.last().expect("bounds"),
        (2 * TICKS_PER_STEP, 6 * TICKS_PER_STEP, 58, 66)
    );
}

/// Ctrl is the add-to-selection role, so a Ctrl-band has to announce mode 1
/// before it starts reporting bounds -- Rust snapshots the base selection on
/// that call.
#[test]
fn a_ctrl_band_asks_to_add_rather_than_replace() {
    let ui = harness(TOOL_SELECT, vec![cell(7, 0, TICKS_PER_STEP, 60)]);
    let modes: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let modes = modes.clone();
        ui.on_piano_marquee_begin(move |mode| modes.borrow_mut().push(mode));
    }

    drag_with(
        ui.window(),
        (tick_x(2 * TICKS_PER_STEP), note_centre_y(66)),
        (tick_x(6 * TICKS_PER_STEP), note_centre_y(58)),
        PointerEventButton::Left,
        Some(Key::Control),
    );

    assert_eq!(*modes.borrow(), vec![1]);
}

/// The band must open exactly once per drag, however many frames it spans,
/// or the base selection it snapshots would be replaced mid-gesture.
#[test]
fn a_band_opens_once_per_drag() {
    let ui = harness(TOOL_SELECT, vec![cell(7, 0, TICKS_PER_STEP, 60)]);
    let opens = Rc::new(RefCell::new(0usize));
    let updates = Rc::new(RefCell::new(0usize));
    {
        let opens = opens.clone();
        ui.on_piano_marquee_begin(move |_| *opens.borrow_mut() += 1);
    }
    {
        let updates = updates.clone();
        ui.on_piano_marquee_updated(move |_, _, _, _| *updates.borrow_mut() += 1);
    }

    drag(
        ui.window(),
        (tick_x(2 * TICKS_PER_STEP), note_centre_y(66)),
        (tick_x(6 * TICKS_PER_STEP), note_centre_y(58)),
    );

    assert!(*updates.borrow() > 1, "the band should track the pointer");
    assert_eq!(*opens.borrow(), 1);
}

/// A press that goes nowhere is the first half of a double-click, not a
/// band. Opening one would clear the selection on the way to creating a note.
#[test]
fn a_click_that_does_not_travel_is_not_a_band() {
    let ui = harness(TOOL_SELECT, Vec::new());
    let opens = Rc::new(RefCell::new(0usize));
    {
        let opens = opens.clone();
        ui.on_piano_marquee_begin(move |_| *opens.borrow_mut() += 1);
    }

    press(
        ui.window(),
        (tick_x(2 * TICKS_PER_STEP), note_centre_y(66)),
        PointerEventButton::Left,
    );

    assert_eq!(*opens.borrow(), 0);
}

// ---- tools --------------------------------------------------------------

/// Draw creates on the first click. Select needs two, so that a single press
/// on empty grid stays free to start a band.
#[test]
fn draw_creates_on_one_click_where_select_needs_two() {
    for (tool, expected) in [(TOOL_SELECT, 0usize), (TOOL_DRAW, 1)] {
        let ui = harness(tool, Vec::new());
        let created = Rc::new(RefCell::new(Vec::new()));
        {
            let created = created.clone();
            ui.on_piano_note_created(move |start, note, duration| {
                created.borrow_mut().push((start, note, duration));
                42
            });
        }

        press(
            ui.window(),
            (tick_x(2 * TICKS_PER_STEP) + 4.0, note_centre_y(60)),
            PointerEventButton::Left,
        );

        assert_eq!(
            created.borrow().len(),
            expected,
            "tool {tool} should have created {expected} note(s) from one click"
        );
        if expected == 1 {
            assert_eq!(
                created.borrow()[0],
                (2 * TICKS_PER_STEP, 60, SNAP_TICKS),
                "draw should snap the start and use one snap step of length"
            );
        }
    }
}

/// Paint lays one note per cell it crosses, and only one: a slow sweep dwells
/// on a cell for several frames.
#[test]
fn paint_lays_one_note_per_crossed_cell() {
    let ui = harness(TOOL_PAINT, Vec::new());
    let painted: Rc<RefCell<Vec<(i32, i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let painted = painted.clone();
        ui.on_piano_cell_painted(move |start, note, duration| {
            painted.borrow_mut().push((start, note, duration));
        });
    }

    // Four snap cells along a single row.
    drag(
        ui.window(),
        (tick_x(0) + 2.0, note_centre_y(60)),
        (tick_x(4 * SNAP_TICKS) + 2.0, note_centre_y(60)),
    );

    let painted = painted.borrow();
    assert!(
        painted.len() >= 4,
        "a stroke across four cells should lay a note in each, got {painted:?}"
    );
    let mut starts: Vec<i32> = painted.iter().map(|(start, _, _)| *start).collect();
    let unique = {
        let mut sorted = starts.clone();
        sorted.sort_unstable();
        sorted.dedup();
        sorted
    };
    starts.sort_unstable();
    assert_eq!(
        starts, unique,
        "dwelling on a cell must not stack duplicates on it"
    );
    assert!(
        painted.iter().all(|(_, note, _)| *note == 60),
        "a horizontal stroke should stay on its row"
    );
}

/// Erase deletes what the pointer crosses, so a sweep clears a run.
#[test]
fn erase_sweeps_away_every_note_it_crosses() {
    let notes: Vec<NoteCell> = (0..4)
        .map(|i| cell(10 + i, i * TICKS_PER_STEP, TICKS_PER_STEP, 60))
        .collect();
    let ui = harness(TOOL_ERASE, notes);
    let removed: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let removed = removed.clone();
        ui.on_piano_note_removed(move |id| removed.borrow_mut().push(id));
    }

    drag(
        ui.window(),
        (tick_x(0) + 4.0, note_centre_y(60)),
        (tick_x(3 * TICKS_PER_STEP) + 4.0, note_centre_y(60)),
    );

    let mut seen = removed.borrow().clone();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(
        seen,
        vec![10, 11, 12, 13],
        "the sweep should have reached every note on the row"
    );
}

/// A right-drag erases in every tool, not only the erase one.
#[test]
fn a_right_drag_erases_from_the_select_tool() {
    let notes: Vec<NoteCell> = (0..3)
        .map(|i| cell(10 + i, i * TICKS_PER_STEP, TICKS_PER_STEP, 60))
        .collect();
    let ui = harness(TOOL_SELECT, notes);
    let removed: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let removed = removed.clone();
        ui.on_piano_note_removed(move |id| removed.borrow_mut().push(id));
    }

    drag_with(
        ui.window(),
        (tick_x(0) + 4.0, note_centre_y(60)),
        (tick_x(2 * TICKS_PER_STEP) + 4.0, note_centre_y(60)),
        PointerEventButton::Right,
        None,
    );

    let mut seen = removed.borrow().clone();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen, vec![10, 11, 12]);
}

/// Slice cuts at the pointer, snapped. The cut is reported as an absolute
/// tick so Rust does not have to re-derive where the pointer was.
#[test]
fn slice_cuts_the_note_at_the_pointer() {
    let ui = harness(TOOL_SLICE, vec![cell(7, 0, 4 * TICKS_PER_STEP, 60)]);
    let cuts: Rc<RefCell<Vec<(i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let cuts = cuts.clone();
        ui.on_piano_note_sliced(move |id, tick| cuts.borrow_mut().push((id, tick)));
    }

    press(
        ui.window(),
        (tick_x(2 * TICKS_PER_STEP) + 1.0, note_centre_y(60)),
        PointerEventButton::Left,
    );

    assert_eq!(*cuts.borrow(), vec![(7, 2 * TICKS_PER_STEP)]);
}

/// Cutting at a note's own edge would leave a zero-length half, which is a
/// delete the user did not ask for. The grid refuses rather than reporting it.
#[test]
fn slice_refuses_a_cut_at_the_notes_edge() {
    let ui = harness(TOOL_SLICE, vec![cell(7, 0, 4 * TICKS_PER_STEP, 60)]);
    let cuts = Rc::new(RefCell::new(0usize));
    {
        let cuts = cuts.clone();
        ui.on_piano_note_sliced(move |_, _| *cuts.borrow_mut() += 1);
    }

    press(
        ui.window(),
        (tick_x(0) + 1.0, note_centre_y(60)),
        PointerEventButton::Left,
    );

    assert_eq!(*cuts.borrow(), 0);
}

/// Slice plus the add-to-selection modifier is the inverse operation.
#[test]
fn slice_with_the_add_modifier_joins_instead() {
    let ui = harness(TOOL_SLICE, vec![cell(7, 0, 4 * TICKS_PER_STEP, 60)]);
    let joins = Rc::new(RefCell::new(0usize));
    let cuts = Rc::new(RefCell::new(0usize));
    {
        let joins = joins.clone();
        ui.on_piano_selection_joined(move || *joins.borrow_mut() += 1);
    }
    {
        let cuts = cuts.clone();
        ui.on_piano_note_sliced(move |_, _| *cuts.borrow_mut() += 1);
    }

    let at = (tick_x(2 * TICKS_PER_STEP) + 1.0, note_centre_y(60));
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    press(ui.window(), at, PointerEventButton::Left);
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });

    assert_eq!(*joins.borrow(), 1);
    assert_eq!(*cuts.borrow(), 0, "joining must not also cut");
}

/// Pressing an existing note moves it whatever the tool is, so draw and paint
/// can adjust what they just laid down without a trip to the tool selector.
#[test]
fn pressing_a_note_still_moves_it_in_the_draw_tool() {
    let ui = harness(TOOL_DRAW, vec![cell(7, 0, TICKS_PER_STEP, 60)]);
    let moves: Rc<RefCell<Vec<(i32, i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    let created = Rc::new(RefCell::new(0usize));
    {
        let moves = moves.clone();
        ui.on_piano_note_moved(move |id, tick, note| moves.borrow_mut().push((id, tick, note)));
    }
    {
        let created = created.clone();
        ui.on_piano_note_created(move |_, _, _| {
            *created.borrow_mut() += 1;
            42
        });
    }

    let start = (tick_x(0) + STEP_WIDTH / 2.0, note_centre_y(60));
    drag(ui.window(), start, (start.0 + 2.0 * STEP_WIDTH, note_centre_y(60)));

    assert_eq!(*created.borrow(), 0, "the press landed on a note, not empty grid");
    let (id, tick, _) = *moves.borrow().last().expect("the note should have moved");
    assert_eq!(id, 7);
    assert_eq!(tick, 2 * TICKS_PER_STEP);
}

// ---- snap toggle --------------------------------------------------------

/// With snap off the drag reports raw ticks, and the override modifier turns
/// snapping back on for that one drag rather than only ever defeating it.
#[test]
fn the_snap_override_inverts_the_toggle_in_both_directions() {
    for (snap_enabled, modifier) in [(true, Some(Key::Shift)), (false, None)] {
        let ui = harness(TOOL_SELECT, vec![cell(7, 0, TICKS_PER_STEP, 60)]);
        ui.set_piano_snap_enabled(snap_enabled);
        let moves: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
        {
            let moves = moves.clone();
            ui.on_piano_note_moved(move |_, tick, _| moves.borrow_mut().push(tick));
        }

        // Two thirds of a step: a snapped drag lands on 0, a free one does not.
        let start = (tick_x(0) + STEP_WIDTH / 2.0, note_centre_y(60));
        drag_with(
            ui.window(),
            start,
            (start.0 + STEP_WIDTH * 2.0 / 3.0, note_centre_y(60)),
            PointerEventButton::Left,
            modifier,
        );

        let moves = moves.borrow();
        assert!(
            moves.iter().any(|tick| *tick % SNAP_TICKS != 0),
            "snap {snap_enabled} with modifier {modifier:?} should have left the grid, got {moves:?}"
        );
    }
}

/// The complement: snapping on with no modifier stays on the grid.
#[test]
fn snapping_on_keeps_the_drag_on_the_grid() {
    let ui = harness(TOOL_SELECT, vec![cell(7, 0, TICKS_PER_STEP, 60)]);
    ui.set_piano_snap_enabled(true);
    let moves: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let moves = moves.clone();
        ui.on_piano_note_moved(move |_, tick, _| moves.borrow_mut().push(tick));
    }

    let start = (tick_x(0) + STEP_WIDTH / 2.0, note_centre_y(60));
    drag(ui.window(), start, (start.0 + STEP_WIDTH * 2.0 / 3.0, note_centre_y(60)));

    let moves = moves.borrow();
    assert!(
        moves.iter().all(|tick| *tick % SNAP_TICKS == 0),
        "every landing should be on a 1/16 boundary, got {moves:?}"
    );
}

// ---- resize -------------------------------------------------------------

/// The left edge moves the start and holds the end, which is the whole point
/// of having it: the right edge already covers "make this longer at the end".
#[test]
fn dragging_the_left_edge_moves_the_start() {
    let ui = harness(TOOL_SELECT, vec![cell(7, 4 * SNAP_TICKS, 4 * SNAP_TICKS, 60)]);
    let starts: Rc<RefCell<Vec<(i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    let lengths = Rc::new(RefCell::new(0usize));
    {
        let starts = starts.clone();
        ui.on_piano_note_start_resized(move |id, start| starts.borrow_mut().push((id, start)));
    }
    {
        let lengths = lengths.clone();
        ui.on_piano_note_resized(move |_, _| *lengths.borrow_mut() += 1);
    }

    // Grab the note's left edge and pull it two steps earlier.
    let left = (tick_x(4 * SNAP_TICKS) + 1.0, note_centre_y(60));
    drag(ui.window(), left, (left.0 - 2.0 * STEP_WIDTH, note_centre_y(60)));

    assert_eq!(
        *lengths.borrow(),
        0,
        "the left edge must not report a right-edge resize"
    );
    let (id, start) = *starts.borrow().last().expect("the start should have moved");
    assert_eq!(id, 7);
    assert_eq!(start, 2 * SNAP_TICKS);
}

/// The start can travel until it would reach the note's own end; past that
/// there would be no note left.
#[test]
fn the_left_edge_stops_before_it_reaches_the_end() {
    let ui = harness(TOOL_SELECT, vec![cell(7, 0, 2 * SNAP_TICKS, 60)]);
    let starts: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let starts = starts.clone();
        ui.on_piano_note_start_resized(move |_, start| starts.borrow_mut().push(start));
    }

    let left = (tick_x(0) + 1.0, note_centre_y(60));
    drag(ui.window(), left, (left.0 + 6.0 * STEP_WIDTH, note_centre_y(60)));

    let starts = starts.borrow();
    assert!(
        starts.iter().all(|start| *start <= SNAP_TICKS),
        "the start should stop one snap step short of the end at 48, got {starts:?}"
    );
}

// ---- scale --------------------------------------------------------------

/// Stretch needs a real selection and an edge to pull. On a single note the
/// modifier does nothing and the edge keeps its ordinary resize, or there
/// would be no way to lengthen one note.
#[test]
fn a_single_selection_does_not_stretch() {
    let mut notes = vec![cell(7, 0, 4 * SNAP_TICKS, 60)];
    notes[0].selected = true;
    let ui = harness(TOOL_SELECT, notes);
    ui.set_selection_count(1);
    ui.set_selection_start_tick(0);
    ui.set_selection_end_tick(4 * SNAP_TICKS);
    let scales = Rc::new(RefCell::new(0usize));
    let lengths = Rc::new(RefCell::new(0usize));
    {
        let scales = scales.clone();
        ui.on_piano_scale_begin(move |_| *scales.borrow_mut() += 1);
    }
    {
        let lengths = lengths.clone();
        ui.on_piano_note_resized(move |_, _| *lengths.borrow_mut() += 1);
    }

    let edge = (tick_x(4 * SNAP_TICKS) - 2.0, note_centre_y(60));
    drag_with(
        ui.window(),
        edge,
        (edge.0 + 2.0 * STEP_WIDTH, note_centre_y(60)),
        PointerEventButton::Left,
        Some(Key::Alt),
    );

    assert_eq!(*scales.borrow(), 0, "one note is not a selection to stretch");
    assert!(
        *lengths.borrow() > 0,
        "the edge should still resize the note itself"
    );
}

/// Without the modifier, an edge drag on a multi-selection resizes rather
/// than stretching: stretch is the deliberate gesture, resize the ordinary
/// one.
#[test]
fn an_unmodified_edge_drag_resizes_rather_than_stretching() {
    let ui = harness(TOOL_SELECT, two_selected_notes());
    set_selection_bounds(&ui);
    let scales = Rc::new(RefCell::new(0usize));
    let lengths = Rc::new(RefCell::new(0usize));
    {
        let scales = scales.clone();
        ui.on_piano_scale_begin(move |_| *scales.borrow_mut() += 1);
    }
    {
        let lengths = lengths.clone();
        ui.on_piano_note_resized(move |_, _| *lengths.borrow_mut() += 1);
    }

    let edge = (tick_x(5 * SNAP_TICKS) - 2.0, note_centre_y(62));
    drag(ui.window(), edge, (edge.0 + 2.0 * STEP_WIDTH, note_centre_y(62)));

    assert_eq!(*scales.borrow(), 0);
    assert!(*lengths.borrow() > 0);
}

/// Alt on the selection's trailing edge, dragged out to twice its span,
/// reports a factor of about two -- which is what turns an eighth into a
/// quarter.
#[test]
fn alt_dragging_the_trailing_edge_scales_by_the_span_it_travelled() {
    let ui = harness(TOOL_SELECT, two_selected_notes());
    set_selection_bounds(&ui);
    let from_left: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(Vec::new()));
    let factors: Rc<RefCell<Vec<f32>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let from_left = from_left.clone();
        ui.on_piano_scale_begin(move |left| from_left.borrow_mut().push(left));
    }
    {
        let factors = factors.clone();
        ui.on_piano_selection_scaled(move |factor| factors.borrow_mut().push(factor));
    }

    // The selection spans ticks 0..120; grab the trailing note's right edge.
    let edge = (tick_x(5 * SNAP_TICKS) - 2.0, note_centre_y(62));
    drag_with(
        ui.window(),
        edge,
        (edge.0 + 5.0 * STEP_WIDTH, note_centre_y(62)),
        PointerEventButton::Left,
        Some(Key::Alt),
    );

    assert_eq!(
        *from_left.borrow(),
        vec![false],
        "a trailing-edge stretch scales about the leading edge"
    );
    let factor = *factors.borrow().last().expect("a factor should be reported");
    assert!(
        (factor - 2.0).abs() < 0.1,
        "doubling the span should report ~2.0, got {factor}"
    );
}

/// Alt on the leading edge scales about the trailing one instead.
#[test]
fn alt_dragging_the_leading_edge_scales_about_the_other_end() {
    let ui = harness(TOOL_SELECT, two_selected_notes());
    set_selection_bounds(&ui);
    let from_left: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(Vec::new()));
    let factors: Rc<RefCell<Vec<f32>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let from_left = from_left.clone();
        ui.on_piano_scale_begin(move |left| from_left.borrow_mut().push(left));
    }
    {
        let factors = factors.clone();
        ui.on_piano_selection_scaled(move |factor| factors.borrow_mut().push(factor));
    }

    // The leading note's left edge, pulled inward by half the span.
    let edge = (tick_x(0) + 1.0, note_centre_y(60));
    drag_with(
        ui.window(),
        edge,
        (edge.0 + 2.5 * STEP_WIDTH, note_centre_y(60)),
        PointerEventButton::Left,
        Some(Key::Alt),
    );

    assert_eq!(*from_left.borrow(), vec![true]);
    let factor = *factors.borrow().last().expect("a factor should be reported");
    assert!(
        (factor - 0.5).abs() < 0.1,
        "halving the span should report ~0.5, got {factor}"
    );
}

// ---- selection-preserving press -----------------------------------------

/// Adam's report: "clicking one to drag them deselects the others."
///
/// A plain press on an already-selected note must not collapse the selection,
/// or the drag that press begins can only ever move one note. This is also
/// what made group length adjust look missing -- the resize was group-aware,
/// but the press had already thrown the group away.
#[test]
fn pressing_a_selected_note_does_not_collapse_the_selection() {
    let ui = harness(TOOL_SELECT, two_selected_notes());
    set_selection_bounds(&ui);
    let selections: Rc<RefCell<Vec<(i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    let moves: Rc<RefCell<Vec<(i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let selections = selections.clone();
        ui.on_piano_note_selected(move |id, mode| selections.borrow_mut().push((id, mode)));
    }
    {
        let moves = moves.clone();
        ui.on_piano_note_moved(move |id, tick, _| moves.borrow_mut().push((id, tick)));
    }

    let body = (tick_x(0) + STEP_WIDTH / 2.0, note_centre_y(60));
    drag(ui.window(), body, (body.0 + 2.0 * STEP_WIDTH, note_centre_y(60)));

    assert!(
        selections.borrow().is_empty(),
        "a drag off an already-selected note should not touch the selection, got {:?}",
        selections.borrow()
    );
    assert!(!moves.borrow().is_empty(), "and it should still move");
}

/// The collapse is deferred, not abandoned: a press that turns out to be a
/// plain click still selects just that note, so clicking one note of a
/// selection to isolate it keeps working.
#[test]
fn clicking_without_dragging_still_collapses_to_that_note() {
    let ui = harness(TOOL_SELECT, two_selected_notes());
    set_selection_bounds(&ui);
    let selections: Rc<RefCell<Vec<(i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let selections = selections.clone();
        ui.on_piano_note_selected(move |id, mode| selections.borrow_mut().push((id, mode)));
    }

    press(
        ui.window(),
        (tick_x(0) + STEP_WIDTH / 2.0, note_centre_y(60)),
        PointerEventButton::Left,
    );

    assert_eq!(*selections.borrow(), vec![(7, 0)]);
}

/// Pressing a note that is *not* selected still replaces the selection
/// immediately -- deferring that would leave the wrong notes selected for
/// the drag about to start.
#[test]
fn pressing_an_unselected_note_replaces_the_selection_at_once() {
    let mut notes = two_selected_notes();
    notes[1].selected = false;
    let ui = harness(TOOL_SELECT, notes);
    let selections: Rc<RefCell<Vec<(i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let selections = selections.clone();
        ui.on_piano_note_selected(move |id, mode| selections.borrow_mut().push((id, mode)));
    }

    let body = (tick_x(4 * SNAP_TICKS) + STEP_WIDTH / 2.0, note_centre_y(62));
    drag(ui.window(), body, (body.0 + 2.0 * STEP_WIDTH, note_centre_y(62)));

    assert_eq!(*selections.borrow(), vec![(8, 0)]);
}

/// Shift is snap override alone now. Holding it while dragging a selected
/// note used to remove that note from the selection and carry it off on its
/// own, which read as an accidental clone.
#[test]
fn shift_dragging_a_selected_note_does_not_change_the_selection() {
    let ui = harness(TOOL_SELECT, two_selected_notes());
    set_selection_bounds(&ui);
    let selections: Rc<RefCell<Vec<(i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    let duplicated = Rc::new(RefCell::new(0usize));
    {
        let selections = selections.clone();
        ui.on_piano_note_selected(move |id, mode| selections.borrow_mut().push((id, mode)));
    }
    {
        let duplicated = duplicated.clone();
        ui.on_piano_selection_duplicated(move |_| {
            *duplicated.borrow_mut() += 1;
            99
        });
    }

    let body = (tick_x(0) + STEP_WIDTH / 2.0, note_centre_y(60));
    drag_with(
        ui.window(),
        body,
        (body.0 + 2.0 * STEP_WIDTH, note_centre_y(60)),
        PointerEventButton::Left,
        Some(Key::Shift),
    );

    assert!(selections.borrow().is_empty(), "Shift is not a selection role");
    assert_eq!(*duplicated.borrow(), 0, "and it is not a copy either");
}

// ---- note width ---------------------------------------------------------

/// Adam's report: "changing snap/grid will change all of the notes in the
/// clip's length to that snap size."
///
/// The drawn width used to be floored at one snap step, so every note shorter
/// than the grid rendered as a full grid step and changing the interval
/// looked like it had rewritten the clip. The floor also inflated the edge
/// grab zones, which is what this can actually observe: on a note far shorter
/// than the snap, the middle must still be a body grab rather than an edge.
#[test]
fn a_short_note_is_not_inflated_to_the_snap_interval() {
    // 6 ticks (1/64) against a 24-tick (1/16) snap: an eighth of a step wide.
    let ui = harness(TOOL_SELECT, vec![cell(7, 0, 6, 60)]);
    let moves = Rc::new(RefCell::new(0usize));
    let resizes = Rc::new(RefCell::new(0usize));
    {
        let moves = moves.clone();
        ui.on_piano_note_moved(move |_, _, _| *moves.borrow_mut() += 1);
    }
    {
        let resizes = resizes.clone();
        ui.on_piano_note_resized(move |_, _| *resizes.borrow_mut() += 1);
    }

    // Two thirds of a step in: inside the note only if the note was inflated
    // to a full snap step. It should be empty grid.
    let beyond = (tick_x(0) + STEP_WIDTH * 2.0 / 3.0, note_centre_y(60));
    drag(ui.window(), beyond, (beyond.0 + STEP_WIDTH, note_centre_y(60)));

    assert_eq!(
        (*moves.borrow(), *resizes.borrow()),
        (0, 0),
        "a 1/64 note must not answer to a press two thirds of a 1/16 step away"
    );
}
