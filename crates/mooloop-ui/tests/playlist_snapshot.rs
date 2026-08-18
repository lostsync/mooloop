use mooloop_ui::{ChannelRow, MainWindow, PlaylistClip, StepCell};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, ModelRc, SharedString, VecModel};
use std::cell::Cell;
use std::rc::Rc;

#[test]
fn render_playlist_snapshot() {
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
    ui.set_song_mode(true);
    ui.set_editor_page(2);
    ui.set_pattern_count(2);
    ui.set_pattern_length(2);
    ui.set_musical_snap_index(1);
    ui.set_current_pattern(0);
    ui.set_playlist_bars(64);
    ui.set_playlist_song_length_ticks(1152);
    ui.set_playlist_position_ticks(192);
    let step_model = Rc::new(VecModel::from(vec![
        StepCell {
            active: true,
            velocity: 42,
            substeps: 0b1111,
        },
        StepCell {
            active: true,
            velocity: 100,
            substeps: 0b0101,
        },
    ]));
    ui.set_channels(ModelRc::from(Rc::new(VecModel::from(vec![ChannelRow {
        name: SharedString::from("Sampler 1"),
        muted: false,
        volume: 0.8,
        pan: 0.0,
        selected: true,
        steps: ModelRc::from(step_model),
    }]))));
    ui.set_playlist_clips(ModelRc::from(std::rc::Rc::new(VecModel::from(vec![
        PlaylistClip {
            pattern: 0,
            start_tick: 0,
            length_steps: 32,
        },
        PlaylistClip {
            pattern: 1,
            start_tick: 192,
            length_steps: 16,
        },
    ]))));

    let snapshot = ui.window().take_snapshot().unwrap();
    assert_eq!(snapshot.width(), 960);
    assert_eq!(snapshot.height(), 760);
    assert!(snapshot.as_bytes().iter().any(|byte| *byte != 0));

    if let Ok(path) = std::env::var("MOOLOOP_PLAYLIST_SNAPSHOT") {
        let mut ppm = format!("P6\n{} {}\n255\n", snapshot.width(), snapshot.height()).into_bytes();
        for rgba in snapshot.as_bytes().chunks_exact(4) {
            ppm.extend_from_slice(&rgba[..3]);
        }
        std::fs::write(path, ppm).unwrap();
    }

    let pixel = |x: usize, y: usize| {
        let offset = (y * snapshot.width() as usize + x) * 4;
        &snapshot.as_bytes()[offset..offset + 3]
    };
    let clip_color = pixel(120, 450).to_vec();
    assert_eq!(pixel(159, 450), clip_color);
    assert_ne!(pixel(160, 450), clip_color);

    let low_velocity_fill = pixel(301, 73).to_vec();
    assert_eq!(pixel(323, 73), low_velocity_fill);
    assert_ne!(pixel(324, 73), low_velocity_fill);
    assert_ne!(pixel(301, 59), pixel(328, 59));

    let added = Rc::new(Cell::new(None));
    ui.on_playlist_placement_added({
        let added = added.clone();
        move |pattern, tick| added.set(Some((pattern, tick)))
    });
    let removed = Rc::new(Cell::new(None));
    ui.on_playlist_placement_removed({
        let removed = removed.clone();
        move |pattern, tick| removed.set(Some((pattern, tick)))
    });
    let position = LogicalPosition::new(124.0, 450.0);
    for button in [PointerEventButton::Left, PointerEventButton::Right] {
        ui.window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        ui.window()
            .dispatch_event(WindowEvent::PointerPressed { position, button });
        ui.window()
            .dispatch_event(WindowEvent::PointerReleased { position, button });
    }
    assert_eq!(added.get(), Some((0, 192)));
    assert_eq!(removed.get(), Some((0, 192)));
}
