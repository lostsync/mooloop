use mooloop_ui::{MainWindow, PlaylistClip};
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
    ui.set_current_pattern(0);
    ui.set_playlist_bars(64);
    ui.set_playlist_song_length_ticks(1152);
    ui.set_playlist_position_ticks(192);
    ui.set_playlist_clips(ModelRc::from(std::rc::Rc::new(VecModel::from(vec![
        PlaylistClip {
            pattern: 0,
            start_half_bar: 0,
            length_steps: 32,
        },
        PlaylistClip {
            pattern: 1,
            start_half_bar: 1,
            length_steps: 16,
        },
    ]))));

    let snapshot = ui.window().take_snapshot().unwrap();
    assert_eq!(snapshot.width(), 960);
    assert_eq!(snapshot.height(), 760);
    assert!(snapshot.as_bytes().iter().any(|byte| *byte != 0));

    let pixel = |x: usize, y: usize| {
        let offset = (y * snapshot.width() as usize + x) * 4;
        &snapshot.as_bytes()[offset..offset + 3]
    };
    let clip_color = pixel(120, 100).to_vec();
    assert_eq!(pixel(159, 100), clip_color);
    assert_ne!(pixel(160, 100), clip_color);

    let added = Rc::new(Cell::new(None));
    ui.on_playlist_placement_added({
        let added = added.clone();
        move |pattern, half_bar| added.set(Some((pattern, half_bar)))
    });
    let removed = Rc::new(Cell::new(None));
    ui.on_playlist_placement_removed({
        let removed = removed.clone();
        move |pattern, half_bar| removed.set(Some((pattern, half_bar)))
    });
    let position = LogicalPosition::new(142.0, 100.0);
    for button in [PointerEventButton::Left, PointerEventButton::Right] {
        ui.window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        ui.window()
            .dispatch_event(WindowEvent::PointerPressed { position, button });
        ui.window()
            .dispatch_event(WindowEvent::PointerReleased { position, button });
    }
    assert_eq!(added.get(), Some((0, 2)));
    assert_eq!(removed.get(), Some((0, 2)));

    if let Ok(path) = std::env::var("MOOLOOP_PLAYLIST_SNAPSHOT") {
        let mut ppm = format!("P6\n{} {}\n255\n", snapshot.width(), snapshot.height()).into_bytes();
        for rgba in snapshot.as_bytes().chunks_exact(4) {
            ppm.extend_from_slice(&rgba[..3]);
        }
        std::fs::write(path, ppm).unwrap();
    }
}
