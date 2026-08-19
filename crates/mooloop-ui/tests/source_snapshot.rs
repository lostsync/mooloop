use mooloop_ui::{ChannelRow, MainWindow, StepCell};
use slint::{ComponentHandle, LogicalSize, ModelRc, SharedString, VecModel};
use std::rc::Rc;

fn write_snapshot(snapshot: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, variable: &str) {
    if let Ok(path) = std::env::var(variable) {
        let mut ppm = format!("P6\n{} {}\n255\n", snapshot.width(), snapshot.height()).into_bytes();
        for rgba in snapshot.as_bytes().chunks_exact(4) {
            ppm.extend_from_slice(&rgba[..3]);
        }
        std::fs::write(path, ppm).unwrap();
    }
}

fn rack_rows() -> ModelRc<ChannelRow> {
    let rows = ["Kick", "Snare", "Closed Hat", "Open Hat"]
        .into_iter()
        .enumerate()
        .map(|(index, name)| ChannelRow {
            name: SharedString::from(name),
            muted: false,
            volume: 0.8,
            pan: 0.0,
            selected: index == 0,
            steps: ModelRc::from(Rc::new(VecModel::from(vec![
                StepCell {
                    active: false,
                    velocity: 0,
                    substeps: 0,
                    onsets: 0,
                };
                16
            ]))),
        })
        .collect::<Vec<_>>();
    ModelRc::from(Rc::new(VecModel::from(rows)))
}

#[test]
fn render_drum_and_mono_source_editors() {
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
    ui.set_channels(rack_rows());
    ui.set_pattern_length(16);
    ui.set_selected_channel_name(SharedString::from("Kick"));
    ui.set_editor_page(0);
    ui.set_source_kind(1);
    ui.set_drum_mode(0);
    let drum = ui.window().take_snapshot().unwrap();
    assert_eq!((drum.width(), drum.height()), (960, 760));
    assert!(drum.as_bytes().iter().any(|byte| *byte != 0));
    write_snapshot(&drum, "MOOLOOP_DRUM_SOURCE_SNAPSHOT");

    ui.set_source_kind(2);
    ui.set_selected_channel_name(SharedString::from("Mono"));
    let mono = ui.window().take_snapshot().unwrap();
    assert_eq!((mono.width(), mono.height()), (960, 760));
    assert_ne!(drum.as_bytes(), mono.as_bytes());
    write_snapshot(&mono, "MOOLOOP_MONO_SOURCE_SNAPSHOT");

    ui.window().set_size(LogicalSize::new(720.0, 760.0));
    let narrow = ui.window().take_snapshot().unwrap();
    assert_eq!((narrow.width(), narrow.height()), (720, 760));
    assert!(narrow.as_bytes().iter().any(|byte| *byte != 0));
    write_snapshot(&narrow, "MOOLOOP_MONO_SOURCE_NARROW_SNAPSHOT");
}
