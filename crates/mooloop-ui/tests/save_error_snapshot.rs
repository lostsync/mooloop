use mooloop_ui::MainWindow;
use slint::{ComponentHandle, LogicalSize, SharedString};

#[test]
fn save_error_dialog_renders_the_complete_reason() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(SharedString::from("software")),
        },
    )))
    .expect("initialize headless renderer");

    // The real shape of a blocked save: the plain-language summary the user
    // acts on, then the machine-readable report the copy button also picks up.
    // Rendered together so the dialog is checked against text that actually
    // needs the scroll area rather than a single short line.
    let project = {
        let mut project = mooloop_core::Project::default();
        project.channels[0].notes[0] = (0..mooloop_core::MAX_NOTES_PER_CHANNEL_PATTERN + 3)
            .map(|index| mooloop_core::NoteEvent::new(index as u32 + 1, 0, 24, 60, 100))
            .collect();
        project.bpm = 0;
        project
    };
    let error = mooloop_project::save_song(
        &std::env::temp_dir().join("mooloop-save-error-snapshot.mooloop"),
        &project,
        mooloop_project::AssetMode::Embedded,
    )
    .expect_err("a pattern over the note ceiling cannot be repaired");

    let ui = MainWindow::new().unwrap();
    ui.window().set_size(LogicalSize::new(960.0, 760.0));
    ui.set_save_error_title("Could not save this song".into());
    ui.set_save_error_detail(error.to_string().into());
    ui.set_save_error_report(error.report().unwrap_or_default().into());
    ui.set_save_error_open(true);

    // The reason has to survive the trip to the screen intact: where it is,
    // what is wrong, and the count needed to fix it.
    let detail = ui.get_save_error_detail().to_string();
    assert!(detail.contains("pattern 1"), "{detail}");
    assert!(detail.contains("delete 3 notes"), "{detail}");
    assert!(
        ui.get_save_error_report().contains("channel.notes.count"),
        "{}",
        ui.get_save_error_report()
    );

    let snapshot = ui.window().take_snapshot().unwrap();
    assert_eq!((snapshot.width(), snapshot.height()), (960, 760));
    assert!(snapshot.as_bytes().iter().any(|byte| *byte != 0));
    if let Ok(path) = std::env::var("MOOLOOP_SAVE_ERROR_SNAPSHOT") {
        let mut ppm = format!("P6\n{} {}\n255\n", snapshot.width(), snapshot.height()).into_bytes();
        for rgba in snapshot.as_bytes().as_chunks::<4>().0 {
            ppm.extend_from_slice(&rgba[..3]);
        }
        std::fs::write(path, ppm).unwrap();
    }
}
