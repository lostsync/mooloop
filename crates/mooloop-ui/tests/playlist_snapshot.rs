use mooloop_ui::{ChannelRow, MainWindow, PlaylistClip, StepCell};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, ModelRc, SharedString, VecModel};
use std::cell::Cell;
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
    ui.set_playlist_snap_index(1);
    ui.set_current_pattern(0);
    ui.set_playlist_bars(64);
    ui.set_playlist_song_length_ticks(1152);
    ui.set_playlist_position_ticks(192);
    let step_model = Rc::new(VecModel::from(vec![
        StepCell {
            active: true,
            velocity: 42,
            substeps: 0b1111,
            onsets: 0b0001,
        },
        StepCell {
            active: true,
            velocity: 100,
            substeps: 0b0101,
            onsets: 0b0101,
        },
    ]));
    ui.set_channels(ModelRc::from(Rc::new(VecModel::from(vec![ChannelRow {
        name: SharedString::from("Sampler 1"),
        muted: false,
        volume: 0.8,
        pan: 0.0,
        selected: true,
        bus: 0,
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

    write_snapshot(&snapshot, "MOOLOOP_PLAYLIST_SNAPSHOT");

    let pixel = |x: usize, y: usize| {
        let offset = (y * snapshot.width() as usize + x) * 4;
        &snapshot.as_bytes()[offset..offset + 3]
    };
    let clip_color = pixel(120, 450).to_vec();
    assert_eq!(pixel(159, 450), clip_color);
    assert_ne!(pixel(160, 450), clip_color);

    // The step grid starts after the rack row's name, mute and the volume/pan
    // knobs, so these x coordinates move whenever that prefix is resized. The
    // first cell spans 192..=215 and the second 219..=242 at 24px per cell.
    const FIRST_CELL_X: usize = 193;
    const FIRST_CELL_LAST_X: usize = 215;
    const CELL_GAP_X: usize = 216;
    const SECOND_CELL_X: usize = 220;
    // Likewise these y values track the combined height of the menu bar and
    // toolbar, since the rack sits directly beneath them. FILL_Y crosses both
    // cells' fills; VELOCITY_Y is high enough that only the louder step
    // reaches it.
    const FILL_Y: usize = 127;
    const VELOCITY_Y: usize = 113;
    // Cell one covers all four 64ths but is struck only on the first, so its
    // slots render at two different intensities.
    const ONSET_X: usize = FIRST_CELL_X;
    const HELD_X: usize = 205;

    // A struck 64th is solid, one that is only being held is dim, and the gap
    // between cells is background. That ordering is the whole reason a
    // ratcheted step looks different from a single sustained note.
    let onset = pixel(ONSET_X, FILL_Y).to_vec();
    let held = pixel(HELD_X, FILL_Y).to_vec();
    let gap = pixel(CELL_GAP_X, FILL_Y).to_vec();
    assert_ne!(onset, held);
    assert_ne!(held, gap);
    assert!(
        onset[1] > held[1] && held[1] > gap[1],
        "expected struck > held > empty, got {onset:?} {held:?} {gap:?}"
    );
    // The fill still stops at the cell boundary.
    assert_ne!(pixel(FIRST_CELL_LAST_X, FILL_Y), gap);
    // Velocity 42 and velocity 100 reach different heights, so the two cells
    // differ on a row that only the taller one fills.
    assert_ne!(
        pixel(FIRST_CELL_X, VELOCITY_Y),
        pixel(SECOND_CELL_X, VELOCITY_Y)
    );

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

    // The File title in the menu bar. Its x tracks the bar's leading padding
    // and the width of the title text; its y is the bar's 26px height.
    let file_position = LogicalPosition::new(22.0, 13.0);
    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: file_position,
    });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: file_position,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: file_position,
        button: PointerEventButton::Left,
    });
    let menu_snapshot = ui.window().take_snapshot().unwrap();
    write_snapshot(&menu_snapshot, "MOOLOOP_FILE_MENU_SNAPSHOT");

    let outside_menu = LogicalPosition::new(250.0, 300.0);
    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: outside_menu,
    });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: outside_menu,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: outside_menu,
        button: PointerEventButton::Left,
    });
    ui.set_export_open(true);
    let export_snapshot = ui.window().take_snapshot().unwrap();
    assert_eq!(export_snapshot.width(), 960);
    assert_eq!(export_snapshot.height(), 760);
    assert_ne!(
        export_snapshot.as_bytes(),
        snapshot.as_bytes(),
        "the export dialog must alter the rendered surface"
    );
    write_snapshot(&export_snapshot, "MOOLOOP_EXPORT_SNAPSHOT");
}
