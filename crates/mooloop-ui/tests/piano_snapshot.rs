//! Renders the Notes page headlessly and dumps it when
//! `MOOLOOP_PIANO_SNAPSHOT` is set, so zoom-scrollbar placement and the
//! default pitch zoom can be verified without a compositor.

use mooloop_ui::{note_hit_test, MainWindow, NoteCell};
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
    ui.window().set_size(LogicalSize::new(960.0, 760.0));
    ui.set_editor_page(1);
    ui.set_pattern_length(16);

    assert_eq!(ui.get_piano_low_note(), 0);
    assert_eq!(ui.get_piano_high_note(), 127);
    assert_eq!(ui.get_piano_note_count(), 128);

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
        for rgba in snapshot.as_bytes().chunks_exact(4) {
            ppm.extend_from_slice(&rgba[..3]);
        }
        std::fs::write(&path, ppm).unwrap();
    }

    assert!(snapshot.as_bytes().iter().any(|byte| *byte != 0));
}
