//! Renders the Notes page headlessly and dumps it when
//! `MOOLOOP_PIANO_SNAPSHOT` is set, so zoom-scrollbar placement and the
//! default pitch zoom can be verified without a compositor.

use mooloop_ui::{
    default_piano_gestures, note_hit_test, AutomationPointCell, AutomationTargetRow,
    MainWindow, NoteCell,
};
use slint::{ComponentHandle, LogicalSize, Model, ModelRc, SharedString, VecModel};
use std::rc::Rc;

#[test]
fn render_piano_snapshot() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(SharedString::from("software")),
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

    let model = Rc::new(VecModel::from(vec![NoteCell {
        id: 7,
        start_tick: 0,
        duration_ticks: 24,
        note: 60,
        velocity: 100,
        selected: true,
    }]));
    ui.set_notes(ModelRc::from(model.clone()));
    ui.on_piano_note_hit_test(move |tick, midi_note| {
        let notes: Vec<NoteCell> = model.iter().collect();
        note_hit_test(&notes, tick, midi_note)
    });
    let snapshot = ui.window().take_snapshot().unwrap();
    if let Ok(path) = std::env::var("MOOLOOP_PIANO_SNAPSHOT") {
        let mut ppm =
            format!("P6\n{} {}\n255\n", snapshot.width(), snapshot.height()).into_bytes();
        for rgba in snapshot.as_bytes().as_chunks::<4>().0 {
            ppm.extend_from_slice(&rgba[..3]);
        }
        std::fs::write(&path, ppm).unwrap();
    }

    assert!(snapshot.as_bytes().iter().any(|byte| *byte != 0));
}

/// The lanes below the roll are toggleable and independent, so the one layout
/// worth pinning is the fully expanded one: velocity stems and an automation
/// curve stacked under the grid, with the keyboard column's labels still
/// aligned to them.
#[test]
fn render_piano_lanes_snapshot() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(SharedString::from("software")),
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
    ui.set_velocity_lane_visible(true);
    ui.set_automation_lane_visible(true);

    let notes = Rc::new(VecModel::from(vec![
        NoteCell {
            id: 1,
            start_tick: 0,
            duration_ticks: 24,
            note: 60,
            velocity: 100,
            selected: true,
        },
        NoteCell {
            id: 2,
            start_tick: 48,
            duration_ticks: 24,
            note: 64,
            velocity: 40,
            selected: false,
        },
        NoteCell {
            id: 3,
            start_tick: 96,
            duration_ticks: 48,
            note: 67,
            velocity: 127,
            selected: false,
        },
    ]));
    ui.set_notes(ModelRc::from(notes.clone()));
    ui.on_piano_note_hit_test(move |tick, midi_note| {
        let notes: Vec<NoteCell> = notes.iter().collect();
        note_hit_test(&notes, tick, midi_note)
    });

    let points = Rc::new(VecModel::from(vec![
        AutomationPointCell {
            id: 1,
            tick: 0,
            value: 0.1,
            selected: false,
        },
        AutomationPointCell {
            id: 2,
            tick: 96,
            value: 0.9,
            selected: true,
        },
        AutomationPointCell {
            id: 3,
            tick: 192,
            value: 0.35,
            selected: false,
        },
        AutomationPointCell {
            id: 4,
            tick: 336,
            value: 0.75,
            selected: false,
        },
    ]));
    ui.set_automation_points(ModelRc::from(points.clone()));
    ui.set_automation_lane_name("Filter 1 · Cutoff".into());
    ui.set_automation_value_text("1.20k Hz".into());
    ui.set_automation_targets(ModelRc::from(Rc::new(VecModel::from(vec![
        AutomationTargetRow {
            param_name: "Cutoff".into(),
            device: "Filter 1".into(),
            starts_group: true,
            open: true,
            current: true,
        },
        AutomationTargetRow {
            param_name: "Resonance".into(),
            device: "Filter 1".into(),
            starts_group: false,
            open: false,
            current: false,
        },
    ]))));
    ui.on_automation_point_hit_test(move |tick, value, tolerance| {
        points
            .iter()
            .find(|point| {
                (point.tick - tick).abs() <= tolerance && (point.value - value).abs() <= 0.12
            })
            .map(|point| point.id)
            .unwrap_or(-1)
    });

    let snapshot = ui.window().take_snapshot().unwrap();
    if let Ok(path) = std::env::var("MOOLOOP_PIANO_LANES_SNAPSHOT") {
        let mut ppm =
            format!("P6\n{} {}\n255\n", snapshot.width(), snapshot.height()).into_bytes();
        for rgba in snapshot.as_bytes().as_chunks::<4>().0 {
            ppm.extend_from_slice(&rgba[..3]);
        }
        std::fs::write(&path, ppm).unwrap();
    }

    assert!(snapshot.as_bytes().iter().any(|byte| *byte != 0));
}
