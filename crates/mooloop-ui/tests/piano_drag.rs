//! Tests for dragging notes in the piano roll.
//!
//! The grid deliberately uses ONE hit area for the whole note canvas and
//! resolves the grabbed note from the press position. Per-note `TouchArea`s
//! cannot support a drag: the area lives inside a rectangle whose `x`/`width`
//! are bound to that note's own tick and duration, so the moment the drag
//! updates the model the rectangle moves out from under the cursor, `mouse-x`
//! re-measures against the moved rectangle, and the delta collapses to zero.
//! The note then travels exactly one snap step and stalls.
//!
//! These tests dispatch a real sequence of pointer events, because that stall
//! only appears after the SECOND move event. Invoking the callbacks directly,
//! or dispatching a single move, passes even against the broken version.

use mooloop_ui::{default_piano_gestures, note_hit_test, MainWindow, NoteCell};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

/// Piano roll grid geometry in logical pixels, derived empirically from a
/// software render of the 960x760 window on the Notes page. These move if the
/// editor's left gutter or the toolbar above it is resized.
const GRID_ORIGIN_X: f32 = 54.0;
const GRID_TOP_Y: f32 = 383.0;
const ROW_HEIGHT: f32 = 8.0;
const STEP_WIDTH: f32 = 32.0;
const TICKS_PER_STEP: i32 = 24;
const HIGH_NOTE: i32 = 84;
const H_SCROLLBAR_Y: f32 = 649.0;
const V_SCROLLBAR_X: f32 = 946.0;

fn note_centre_y(midi_note: i32) -> f32 {
    GRID_TOP_Y + (HIGH_NOTE - midi_note) as f32 * ROW_HEIGHT + ROW_HEIGHT / 2.0
}

fn tick_x(tick: i32) -> f32 {
    GRID_ORIGIN_X + tick as f32 * STEP_WIDTH / TICKS_PER_STEP as f32
}

fn harness(notes: Vec<NoteCell>) -> MainWindow {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(slint::SharedString::from("software")),
        },
    )))
    .ok();
    let ui = MainWindow::new().unwrap();
    // `run` resolves these from the user's settings; without them every
    // gesture role is unbound and no modifier does anything.
    ui.set_piano_gestures(default_piano_gestures());
    ui.window().set_size(LogicalSize::new(960.0, 760.0));
    ui.set_editor_page(1);
    ui.set_pattern_length(16);

    let model = Rc::new(VecModel::from(notes));
    ui.set_notes(ModelRc::from(model.clone()));
    // Wire the same hit test the application wires in `run`, so these tests
    // exercise the real lookup rather than a stand-in.
    ui.on_piano_note_hit_test(move |tick, midi_note| {
        let notes: Vec<NoteCell> = model.iter().collect();
        note_hit_test(&notes, tick, midi_note)
    });
    ui
}

/// Press at `from`, travel to `to` in several steps the way a real pointer
/// would, then release. Multiple moves are the point: one move cannot
/// distinguish a working drag from one that stalls after the first step.
fn drag(window: &slint::Window, from: (f32, f32), to: (f32, f32)) {
    drag_with(window, from, to, None);
}

/// As `drag`, with a modifier held for the whole gesture.
fn drag_with(
    window: &slint::Window,
    from: (f32, f32),
    to: (f32, f32),
    modifier: Option<slint::platform::Key>,
) {
    if let Some(key) = modifier {
        window.dispatch_event(WindowEvent::KeyPressed { text: key.into() });
    }
    drag_inner(window, from, to);
    if let Some(key) = modifier {
        window.dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    }
}

fn drag_inner(window: &slint::Window, from: (f32, f32), to: (f32, f32)) {
    let pos = |(x, y): (f32, f32)| LogicalPosition::new(x, y);
    window.dispatch_event(WindowEvent::PointerMoved {
        position: pos(from),
    });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: pos(from),
        button: PointerEventButton::Left,
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
        button: PointerEventButton::Left,
    });
}

fn one_note() -> Vec<NoteCell> {
    vec![NoteCell {
        id: 7,
        start_tick: 0,
        duration_ticks: TICKS_PER_STEP,
        note: 60,
        velocity: 100,
        selected: false,
    }]
}

fn two_notes() -> Vec<NoteCell> {
    vec![
        NoteCell {
            id: 7,
            start_tick: 0,
            duration_ticks: TICKS_PER_STEP,
            note: 60,
            velocity: 100,
            selected: false,
        },
        NoteCell {
            id: 8,
            start_tick: 4 * TICKS_PER_STEP,
            duration_ticks: TICKS_PER_STEP,
            note: 62,
            velocity: 100,
            selected: false,
        },
    ]
}

/// Click at `at`, optionally holding Shift, the way a Shift-click to extend a
/// selection would arrive from the OS: modifier key down, then press/release,
/// then modifier key up.
fn click(window: &slint::Window, at: (f32, f32), shift: bool) {
    let pos = LogicalPosition::new(at.0, at.1);
    if shift {
        window.dispatch_event(WindowEvent::KeyPressed {
            text: slint::platform::Key::Shift.into(),
        });
    }
    window.dispatch_event(WindowEvent::PointerMoved { position: pos });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: pos,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position: pos,
        button: PointerEventButton::Left,
    });
    if shift {
        window.dispatch_event(WindowEvent::KeyReleased {
            text: slint::platform::Key::Shift.into(),
        });
    }
}

#[test]
fn dragging_a_note_body_tracks_the_pointer_the_whole_way() {
    let ui = harness(one_note());
    let moves: Rc<RefCell<Vec<(i32, i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let moves = moves.clone();
        ui.on_piano_note_moved(move |id, tick, note| {
            moves.borrow_mut().push((id, tick, note));
        });
    }

    // Grab the middle of the note and travel four steps right, two semitones up.
    let start = (tick_x(0) + STEP_WIDTH / 2.0, note_centre_y(60));
    let end = (start.0 + 4.0 * STEP_WIDTH, note_centre_y(62));
    drag(ui.window(), start, end);

    let moves = moves.borrow();
    let (id, tick, note) = *moves.last().expect("the drag should have moved the note");
    assert_eq!(id, 7, "the grabbed note's id should be reported");
    assert_eq!(
        tick,
        4 * TICKS_PER_STEP,
        "the note should land where the pointer did, not one snap step in"
    );
    assert_eq!(note, 62, "the note should follow the pointer's pitch");
}

#[test]
fn piano_scrollbar_grips_zoom_both_axes() {
    let ui = harness(one_note());

    // The horizontal bar belongs to the bottom edge of the grid. Pulling its
    // left end inward shrinks the visible time fraction and therefore zooms in.
    let before_time_zoom = ui.window().take_snapshot().unwrap();
    drag(ui.window(), (GRID_ORIGIN_X + 2.0, H_SCROLLBAR_Y), (120.0, H_SCROLLBAR_Y));
    assert!(
        ui.window().take_snapshot().unwrap().as_bytes() != before_time_zoom.as_bytes(),
        "dragging a time-scrollbar grip should zoom time"
    );

    // The pitch bar is on the right edge. Its top grip works the same way.
    let before_pitch_zoom = ui.window().take_snapshot().unwrap();
    drag(ui.window(), (V_SCROLLBAR_X, GRID_TOP_Y + 2.0), (V_SCROLLBAR_X, 450.0));
    assert!(
        ui.window().take_snapshot().unwrap().as_bytes() != before_pitch_zoom.as_bytes(),
        "dragging a pitch-scrollbar grip should zoom pitch"
    );
}

#[test]
fn dragging_the_right_edge_lengthens_the_note() {
    let ui = harness(one_note());
    let sizes: Rc<RefCell<Vec<(i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let sizes = sizes.clone();
        ui.on_piano_note_resized(move |id, duration| {
            sizes.borrow_mut().push((id, duration));
        });
    }

    // Grab just inside the note's right edge and pull three steps right.
    let start = (tick_x(TICKS_PER_STEP) - 2.0, note_centre_y(60));
    let end = (start.0 + 3.0 * STEP_WIDTH, note_centre_y(60));
    drag(ui.window(), start, end);

    let sizes = sizes.borrow();
    let (id, duration) = *sizes.last().expect("the drag should have resized the note");
    assert_eq!(id, 7);
    assert_eq!(
        duration,
        4 * TICKS_PER_STEP,
        "the note should end under the pointer, not one snap step longer"
    );
}

#[test]
fn dragging_a_note_does_not_create_a_new_one() {
    let ui = harness(one_note());
    let created = Rc::new(RefCell::new(0usize));
    {
        let created = created.clone();
        ui.on_piano_note_created(move |_, _, _| {
            *created.borrow_mut() += 1;
            99
        });
    }

    let start = (tick_x(0) + STEP_WIDTH / 2.0, note_centre_y(60));
    drag(ui.window(), start, (start.0 + 2.0 * STEP_WIDTH, start.1));

    assert_eq!(
        *created.borrow(),
        0,
        "pressing on an existing note should grab it, not add another"
    );
}

#[test]
fn pressing_empty_grid_does_not_create_a_note() {
    let ui = harness(one_note());
    let created: Rc<RefCell<Vec<(i32, i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let created = created.clone();
        ui.on_piano_note_created(move |tick, note, dur| {
            created.borrow_mut().push((tick, note, dur));
            21
        });
    }

    // A row well away from the only note.
    let at = LogicalPosition::new(tick_x(4 * TICKS_PER_STEP), note_centre_y(55));
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position: at });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: at,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: at,
        button: PointerEventButton::Left,
    });

    let created = created.borrow();
    assert!(created.is_empty(), "single-clicking empty grid should only focus/arm");
}

#[test]
fn double_clicking_empty_grid_creates_and_drags_note_length() {
    let ui = harness(one_note());
    let created: Rc<RefCell<Vec<(i32, i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    let sizes: Rc<RefCell<Vec<(i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let created = created.clone();
        ui.on_piano_note_created(move |tick, note, dur| {
            created.borrow_mut().push((tick, note, dur));
            21
        });
    }
    {
        let sizes = sizes.clone();
        ui.on_piano_note_resized(move |id, duration| {
            sizes.borrow_mut().push((id, duration));
        });
    }

    let start = LogicalPosition::new(tick_x(4 * TICKS_PER_STEP), note_centre_y(55));
    let end = LogicalPosition::new(start.x + 3.0 * STEP_WIDTH, start.y);
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position: start });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: start,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: start,
        button: PointerEventButton::Left,
    });
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position: start });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: start,
        button: PointerEventButton::Left,
    });
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position: end });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: end,
        button: PointerEventButton::Left,
    });

    let created = created.borrow();
    assert_eq!(created.len(), 1, "double-clicking empty grid should add one note");
    assert_eq!(created[0], (4 * TICKS_PER_STEP, 55, TICKS_PER_STEP));
    let sizes = sizes.borrow();
    let (id, duration) = *sizes.last().expect("dragging after creation should resize the note");
    assert_eq!(id, 21);
    assert_eq!(duration, 4 * TICKS_PER_STEP);
}

#[test]
fn plain_click_replaces_the_selection_but_shift_click_adds_to_it() {
    let ui = harness(two_notes());
    let selections: Rc<RefCell<Vec<(i32, i32)>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let selections = selections.clone();
        ui.on_piano_note_selected(move |id, mode| {
            selections.borrow_mut().push((id, mode));
        });
    }

    click(ui.window(), (tick_x(0) + STEP_WIDTH / 2.0, note_centre_y(60)), false);
    click(
        ui.window(),
        (tick_x(4 * TICKS_PER_STEP) + STEP_WIDTH / 2.0, note_centre_y(62)),
        true,
    );

    let selections = selections.borrow();
    assert_eq!(
        selections.as_slice(),
        &[(7, 0), (8, 1)],
        "a plain click should ask to replace the selection (0); a Shift-click should ask \
         to add to it (1), Shift being the default add-to-selection gesture"
    );
}

/// The gesture Adam reported broken: two notes selected, only one movable.
/// The roll reports the grabbed note's landing position and Rust applies that
/// delta to the rest, so what this pins is that the drag keeps reporting the
/// grabbed note's own position for the whole travel.
#[test]
fn dragging_one_note_of_a_selection_reports_its_own_landing_position() {
    let mut notes = two_notes();
    for note in &mut notes {
        note.selected = true;
    }
    let ui = harness(notes);
    let moves = Rc::new(std::cell::RefCell::new(Vec::<(i32, i32)>::new()));
    let sink = moves.clone();
    ui.on_piano_note_moved(move |id, tick, _| sink.borrow_mut().push((id, tick)));

    drag(
        ui.window(),
        (tick_x(0) + 6.0, note_centre_y(60)),
        (tick_x(4 * TICKS_PER_STEP) + 6.0, note_centre_y(60)),
    );

    let moves = moves.borrow();
    assert!(!moves.is_empty(), "the drag reported nothing");
    assert!(
        moves.iter().all(|(id, _)| *id == 7),
        "the drag switched notes mid-gesture: {moves:?}"
    );
    assert_eq!(
        moves.last().unwrap().1,
        4 * TICKS_PER_STEP,
        "the grabbed note did not land under the pointer: {moves:?}"
    );
}

#[test]
fn ctrl_drag_duplicates_once_and_continues_on_the_copy() {
    let ui = harness(one_note());
    let duplications = Rc::new(std::cell::RefCell::new(Vec::<i32>::new()));
    let sink = duplications.clone();
    // Stand in for Rust's real duplication, which returns the copy's ID.
    ui.on_piano_selection_duplicated(move |anchor| {
        sink.borrow_mut().push(anchor);
        99
    });
    let moves = Rc::new(std::cell::RefCell::new(Vec::<i32>::new()));
    let move_sink = moves.clone();
    ui.on_piano_note_moved(move |id, _, _| move_sink.borrow_mut().push(id));

    drag_with(
        ui.window(),
        (tick_x(0) + 6.0, note_centre_y(60)),
        (tick_x(4 * TICKS_PER_STEP) + 6.0, note_centre_y(60)),
        Some(slint::platform::Key::Control),
    );

    assert_eq!(
        *duplications.borrow(),
        vec![7],
        "Ctrl-drag must duplicate exactly once, naming the grabbed note"
    );
    let moves = moves.borrow();
    assert!(
        moves.iter().all(|id| *id == 99),
        "the drag did not continue on the copy: {moves:?}"
    );
}

#[test]
fn shift_drag_leaves_the_grid() {
    let ui = harness(one_note());
    let moves = Rc::new(std::cell::RefCell::new(Vec::<i32>::new()));
    let sink = moves.clone();
    ui.on_piano_note_moved(move |_, tick, _| sink.borrow_mut().push(tick));

    // Half a step across. Snapped, every report would be 0 or a whole step;
    // free, the ticks in between are reachable.
    let half_step = tick_x(TICKS_PER_STEP / 2) - tick_x(0);
    drag_with(
        ui.window(),
        (tick_x(0) + 6.0, note_centre_y(60)),
        (tick_x(0) + 6.0 + half_step, note_centre_y(60)),
        Some(slint::platform::Key::Shift),
    );

    let moves = moves.borrow();
    assert!(
        moves.iter().any(|tick| *tick % TICKS_PER_STEP != 0),
        "Shift-drag still snapped to the grid: {moves:?}"
    );
}

/// A drag reports an edit on every move frame, and each of those used to
/// become its own undo entry, so moving a note across the grid cost twenty
/// undos to take back. The history collapses frames that share a gesture
/// token (`History::record`); this asserts the grid actually brackets a drag
/// with exactly one token, which is what makes that collapsing apply.
#[test]
fn a_drag_opens_and_closes_exactly_one_gesture() {
    let ui = harness(one_note());
    let marks: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let marks = marks.clone();
        ui.on_piano_gesture_begin(move || marks.borrow_mut().push("begin"));
    }
    {
        let marks = marks.clone();
        ui.on_piano_gesture_end(move || marks.borrow_mut().push("end"));
    }
    let moves = Rc::new(RefCell::new(0usize));
    {
        let moves = moves.clone();
        ui.on_piano_note_moved(move |_, _, _| *moves.borrow_mut() += 1);
    }

    let start = (tick_x(0) + STEP_WIDTH / 2.0, note_centre_y(60));
    drag(ui.window(), start, (start.0 + 4.0 * STEP_WIDTH, note_centre_y(62)));

    assert!(
        *moves.borrow() > 1,
        "the drag needs several move frames for coalescing to mean anything"
    );
    assert_eq!(
        *marks.borrow(),
        vec!["begin", "end"],
        "one press-to-release drag is one gesture, however many frames it took"
    );
}

/// Two separate drags have to stay two undo steps, so the grid must close its
/// gesture on release rather than leaving one open across the gap.
#[test]
fn two_drags_are_two_gestures() {
    let ui = harness(one_note());
    let begins = Rc::new(RefCell::new(0usize));
    {
        let begins = begins.clone();
        ui.on_piano_gesture_begin(move || *begins.borrow_mut() += 1);
    }

    let start = (tick_x(0) + STEP_WIDTH / 2.0, note_centre_y(60));
    drag(ui.window(), start, (start.0 + 2.0 * STEP_WIDTH, note_centre_y(60)));
    drag(ui.window(), start, (start.0 + 2.0 * STEP_WIDTH, note_centre_y(60)));

    assert_eq!(*begins.borrow(), 2);
}
