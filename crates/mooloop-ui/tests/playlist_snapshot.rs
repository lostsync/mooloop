use mooloop_ui::{ChannelRow, MainWindow, PlaylistClip, StepCell};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, ModelRc, SharedString, VecModel};
use std::cell::Cell;
use std::rc::Rc;

fn write_snapshot(snapshot: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, variable: &str) {
    if let Ok(path) = std::env::var(variable) {
        let mut ppm = format!("P6\n{} {}\n255\n", snapshot.width(), snapshot.height()).into_bytes();
        for rgba in snapshot.as_bytes().as_chunks::<4>().0 {
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
        volume_db: -1.9382, // linear 0.8 in dB
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
    // The playlist canvas lives inside the fixed-height editor dock, which
    // the docked status bar lifts 24px: 450 in window coordinates before the
    // bar is 426 after.
    let clip_color = pixel(120, 426).to_vec();
    assert_eq!(pixel(159, 426), clip_color);
    assert_ne!(pixel(160, 426), clip_color);

    // The step grid starts after the rack row's name, mute, the volume/pan
    // knobs and the mixer-bus picker, so these x coordinates move whenever
    // that prefix is resized -- the picker's 30px plus its 6px of spacing is
    // why they sit 36px further right than they used to. The first cell spans
    // 228..=251 and the second 255..=278 at 24px per cell.
    const FIRST_CELL_X: usize = 229;
    const FIRST_CELL_LAST_X: usize = 251;
    const CELL_GAP_X: usize = 252;
    const SECOND_CELL_X: usize = 256;
    // Likewise these y values track the combined height of the menu bar, the
    // toolbar, and the work surface's Steps/Mixer header, since the rack sits
    // directly beneath them. FILL_Y crosses both cells' fills; VELOCITY_Y is
    // high enough that only the louder step reaches it.
    const FILL_Y: usize = 154;
    const VELOCITY_Y: usize = 140;
    // Cell one covers all four 64ths but is struck only on the first, so its
    // slots render at two different intensities.
    const ONSET_X: usize = FIRST_CELL_X;
    const HELD_X: usize = 241;

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

    // Where the playlist canvas sits vertically, measured rather than
    // assumed: it is found by walking down the column that crosses the clip
    // at tick 0 until the clip's colour appears. That clip is inset 2px
    // inside a row that begins 30px into the canvas, so the first such row is
    // canvas y 32.
    //
    // Measured because the alternative is a constant that silently means the
    // wrong thing the next time anything above the canvas changes height, and
    // every gesture below is aimed by it. The horizontal offset needs no such
    // care: the assertions above already pin the canvas's left edge 8px in.
    let canvas_top = {
        // Scanned from below the work surface, because the accent this is
        // looking for is also the colour of an active tab and a lit meter.
        let column = 120;
        let first = (300..snapshot.height() as usize)
            .find(|y| pixel(column, *y) == clip_color.as_slice())
            .expect("the clip at tick 0 should be somewhere in the column");
        assert!(
            first > 340,
            "the clip was found at y {first}, which is above the editor dock"
        );
        first as f32 - 32.0
    };
    // The two header strips, in window coordinates: the loop strip occupies
    // the canvas's first 10px and the bar numbers the 20px below it.
    let loop_strip_y = canvas_top + 5.0;
    let bar_ruler_y = canvas_top + 20.0;
    // Snap is 1/2 bar (192 ticks) in this fixture, and the timeline starts
    // 104px into the canvas at 24px per bar, so window x 160 is tick 768 and
    // window x 244 is tick 2112.
    const TICK_768_X: f32 = 160.0;
    const TICK_2112_X: f32 = 244.0;

    let drag = |from: LogicalPosition, to: LogicalPosition, button| {
        ui.window()
            .dispatch_event(WindowEvent::PointerMoved { position: from });
        ui.window()
            .dispatch_event(WindowEvent::PointerPressed {
                position: from,
                button,
            });
        ui.window()
            .dispatch_event(WindowEvent::PointerMoved { position: to });
        ui.window()
            .dispatch_event(WindowEvent::PointerReleased {
                position: to,
                button,
            });
    };

    // A loop drag runs from where it started to the end of the snap unit it
    // released on, which is what makes a click loop the unit clicked.
    let loop_set = Rc::new(Cell::new(None));
    ui.on_playlist_loop_set({
        let loop_set = loop_set.clone();
        move |from, to| loop_set.set(Some((from, to)))
    });
    drag(
        LogicalPosition::new(TICK_768_X, loop_strip_y),
        LogicalPosition::new(TICK_2112_X, loop_strip_y),
        PointerEventButton::Left,
    );
    assert_eq!(loop_set.get(), Some((768, 2304)));

    // Dragged the other way is the same section.
    loop_set.set(None);
    drag(
        LogicalPosition::new(TICK_2112_X, loop_strip_y),
        LogicalPosition::new(TICK_768_X, loop_strip_y),
        PointerEventButton::Left,
    );
    assert_eq!(loop_set.get(), Some((768, 2304)));

    let loop_cleared = Rc::new(Cell::new(false));
    ui.on_playlist_loop_cleared({
        let loop_cleared = loop_cleared.clone();
        move || loop_cleared.set(true)
    });
    drag(
        LogicalPosition::new(TICK_768_X, loop_strip_y),
        LogicalPosition::new(TICK_768_X, loop_strip_y),
        PointerEventButton::Right,
    );
    assert!(loop_cleared.get(), "right-click on the loop strip clears it");

    // The playhead is grabbed on the bar numbers, and follows the pointer
    // rather than jumping once per click.
    let seeks = Rc::new(std::cell::RefCell::new(Vec::new()));
    ui.on_playlist_seek({
        let seeks = seeks.clone();
        move |tick| seeks.borrow_mut().push(tick)
    });
    drag(
        LogicalPosition::new(TICK_768_X, bar_ruler_y),
        LogicalPosition::new(TICK_2112_X, bar_ruler_y),
        PointerEventButton::Left,
    );
    assert_eq!(
        seeks.borrow().first().copied(),
        Some(768),
        "the press should seek where it landed"
    );
    assert_eq!(
        seeks.borrow().last().copied(),
        Some(2112),
        "and the drag should carry the playhead with it"
    );

    // A live loop tints the lanes it covers; the same loop switched off does
    // not, because the points are kept and the tint is what says it is
    // running.
    let untinted = pixel(150, 426).to_vec();
    ui.set_playlist_loop_start_ticks(384);
    ui.set_playlist_loop_end_ticks(1152);
    ui.set_playlist_loop_enabled(true);
    let looped = ui.window().take_snapshot().unwrap();
    let looped_pixel = |x: usize, y: usize| {
        let offset = (y * looped.width() as usize + x) * 4;
        looped.as_bytes()[offset..offset + 3].to_vec()
    };
    assert_ne!(looped_pixel(150, 426), untinted, "a live loop tints its lanes");
    ui.set_playlist_loop_enabled(false);
    let unlooped = ui.window().take_snapshot().unwrap();
    let unlooped_pixel = |x: usize, y: usize| {
        let offset = (y * unlooped.width() as usize + x) * 4;
        unlooped.as_bytes()[offset..offset + 3].to_vec()
    };
    assert_eq!(
        unlooped_pixel(150, 426),
        untinted,
        "a loop switched off should stop tinting"
    );
    ui.set_playlist_loop_start_ticks(0);
    ui.set_playlist_loop_end_ticks(0);

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
    let position = LogicalPosition::new(124.0, 426.0);
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
