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

    let ui = MainWindow::new().unwrap();
    ui.window().set_size(LogicalSize::new(960.0, 760.0));
    ui.set_save_error_title("Could not save song".into());
    ui.set_save_error_detail(
        "invalid document: channel 3 mono synth LFO rate is 400.0; expected 0..=20".into(),
    );
    ui.set_save_error_open(true);

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
