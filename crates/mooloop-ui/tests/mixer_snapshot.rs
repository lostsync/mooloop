use mooloop_core::strip::{StripBand, StripParams};
use mooloop_core::{EffectKind, EqBandKind, PreampVoicing, MAX_BUSES};
use mooloop_ui::{
    effect_kind_index, effect_kind_units, strip_row, view, ChannelRow, EffectSlotRow, MainWindow,
    MixerMetrics, MixerSendRow, MixerStripRow, StepCell, StripRow,
};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, ModelRc, SharedString, VecModel};
use std::cell::Cell;
use std::rc::Rc;

mod common;

fn write_snapshot(snapshot: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, variable: &str) {
    if let Ok(path) = std::env::var(variable) {
        let mut ppm = format!("P6\n{} {}\n255\n", snapshot.width(), snapshot.height()).into_bytes();
        for rgba in snapshot.as_bytes().as_chunks::<4>().0 {
            ppm.extend_from_slice(&rgba[..3]);
        }
        std::fs::write(path, ppm).unwrap();
    }
}

fn rack_rows() -> ModelRc<ChannelRow> {
    let rows = [("Kick", 3), ("Snare", 3), ("Bass", 5), ("Pad", 0)]
        .into_iter()
        .enumerate()
        .map(|(index, (name, bus))| ChannelRow {
            name: SharedString::from(name),
            color: Default::default(),
            has_color: false,
            track_color: Default::default(),
            has_track_color: false,
            muted: false,
            volume_db: -1.9382, // linear 0.8 in dB
            pan: 0.0,
            selected: index == 0,
            bus,
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

fn bus_names() -> ModelRc<SharedString> {
    let names = (0..MAX_BUSES)
        .map(|index| {
            SharedString::from(if index == 0 {
                "Master".to_string()
            } else {
                format!("Bus {index}")
            })
        })
        .collect::<Vec<_>>();
    ModelRc::from(Rc::new(VecModel::from(names)))
}

fn strips(selected: usize) -> Rc<VecModel<MixerStripRow>> {
    Rc::new(VecModel::from(
        (0..MAX_BUSES)
            .map(|index| MixerStripRow {
                name: SharedString::from(if index == 0 {
                    "Master".to_string()
                } else {
                    format!("Bus {index}")
                }),
                // Uncoloured: these harnesses are about the strip's controls,
                // and a track nobody has coloured is the ordinary case.
                color: Default::default(),
                has_color: false,
                muted: false,
                volume: 1.0,
                pan: 0.0,
                output: 0,
                selected: index == selected,
                is_master: index == 0,
                console: false,
                polarity: index == 2,
                // One track soloed, which is what dims the names of the
                // tracks a solo silences -- so the snapshot carries both
                // states rather than only the quiet one.
                solo: index == 1,
                solo_silenced: index > 1,
                // Every section out on most tracks, which is what a track
                // arrives with; one loaded strip so the turned-over face and
                // the rack's pinned row have something to draw.
                strip: if index == 1 {
                    demo_strip()
                } else {
                    StripRow::default()
                },
                sends: ModelRc::from(Rc::new(VecModel::from(Vec::new()))),
                send_allowed: ModelRc::from(Rc::new(VecModel::from(
                    (0..MAX_BUSES).map(|other| other != index).collect::<Vec<_>>(),
                ))),
                feed_count: match index {
                    0 => 1,
                    3 => 2,
                    5 => 1,
                    _ => 0,
                },
                // Nothing is routed bus-to-bus here, so every destination
                // except the strip itself is reachable without looping.
                allowed: ModelRc::from(Rc::new(VecModel::from(
                    (0..MAX_BUSES).map(|other| other != index).collect::<Vec<_>>(),
                ))),
                left_db: if index == 0 { -6.0 } else { -60.0 },
                right_db: if index == 0 { -8.0 } else { -60.0 },
                // A peak above the level, so the snapshot carries the held
                // marker the strips did not draw before.
                held_left_db: if index == 0 { -3.0 } else { -60.0 },
                held_right_db: if index == 0 { -4.0 } else { -60.0 },
                clipping: false,
            })
            .collect::<Vec<_>>(),
    ))
}

/// A strip with something to draw: two sections in, a voicing that colours,
/// and an EQ curve with a bell and a shelf in it. Built through the same
/// `strip_row` the application publishes, so the response plot's band array
/// and the compressor's sampled curve are the real ones.
fn demo_strip() -> StripRow {
    let mut params = StripParams {
        voicing: PreampVoicing::Iron,
        pre_in: true,
        drive_db: 4.0,
        eq_in: true,
        comp_in: true,
        threshold_db: -22.0,
        ratio: 4.0,
        makeup_db: 3.0,
        ..StripParams::default()
    };
    params.bands[0] = StripBand {
        kind: EqBandKind::HighShelf,
        position: 1,
        gain_db: 3.5,
        q: 0.7,
    };
    params.bands[1] = StripBand {
        kind: EqBandKind::Bell,
        position: 3,
        gain_db: -4.0,
        q: 1.8,
    };
    params.bands[3] = StripBand {
        kind: EqBandKind::LowShelf,
        position: 1,
        gain_db: 5.0,
        q: 0.8,
    };
    strip_row(&params, 48_000)
}

fn headless() -> MainWindow {
    common::install_testing_backend();

    let ui = MainWindow::new().unwrap();
    // The strip's faces read every range and every parameter id out of this,
    // so a window that has not been given it draws a strip with no labels
    // and no ranges. A test should reach the strip the way the application
    // does.
    mooloop_ui::install_strip_spec(&ui);
    // 760 deliberately, which is *shorter* than the mixer's own default
    // dock height: the work area gives the pane about 200px here, where the
    // strip's fader face wants about 180 plus the pane's own chrome, so
    // this window renders the squeezed case. Every coordinate below is
    // probed against it, and a taller window would move all of them.
    ui.window().set_size(LogicalSize::new(1100.0, 760.0));
    ui.set_channels(rack_rows());
    ui.set_pattern_length(16);
    ui.set_bus_names(bus_names());
    ui.set_mixer_strips(ModelRc::from(strips(3)));
    ui
}

fn click(ui: &MainWindow, x: f32, y: f32) {
    let position = LogicalPosition::new(x, y);
    ui.window().dispatch_event(WindowEvent::PointerMoved { position });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

/// The mixer replaces the step grid in the same pane, and the device rack
/// below keeps working while it is up — that pairing is the whole interaction,
/// so render both halves together rather than the pane on its own.
#[test]
fn render_mixer_pane_with_a_bus_chain() {
    let ui = headless();
    ui.invoke_show_view(view::MIXER);
    ui.invoke_show_view(view::DEVICES);
    ui.set_editing_bus(true);
    ui.set_editing_bus_index(3);
    ui.set_editing_bus_name(SharedString::from("Bus 3"));
    ui.set_editing_bus_feed_count(2);
    ui.set_editing_bus_volume(1.0);
    ui.set_editing_bus_strip(demo_strip());
    ui.set_editing_bus_left_db(-9.0);
    ui.set_editing_bus_right_db(-11.0);
    ui.set_effect_slots(ModelRc::from(Rc::new(VecModel::from(vec![
        EffectSlotRow {
            kind: effect_kind_index(EffectKind::Compressor),
            units: effect_kind_units(EffectKind::Compressor),
            preset_options: Vec::<SharedString>::new().as_slice().into(),
            preset_name: Default::default(),
            bypassed: false,
            p0: 0.5,
            p1: 0.4,
            p2: 0.2,
            p3: 0.3,
            p4: 0.1,
            p5: 0.0,
            p6: 0.0,
            p7: 0.0,
            p8: 0.0,
            p9: 0.0,
            p10: 0.0,
            p11: 0.0,
            p12: 0.0,
            p13: 0.0,
            p14: 0.0,
            p15: 0.0,
            p16: 0.0,
            p17: 0.0,
            modulation_depths: Vec::<f32>::new().as_slice().into(),
            modulation_allowed: Vec::<bool>::new().as_slice().into(),
            modulation_offsets: Vec::<f32>::new().as_slice().into(),
            modulation_route_counts: Vec::<i32>::new().as_slice().into(),
            eq_band_data: Vec::<f32>::new().as_slice().into(),
            eq_band_kinds: Vec::<i32>::new().as_slice().into(),
            eq_pass_data: Vec::<f32>::new().as_slice().into(),
            eq_curve_db: Vec::<f32>::new().as_slice().into(),
            eq_spectrum_data: Vec::<f32>::new().as_slice().into(),
            eq_analyzer_enabled: false,
            preamp_deviation: Vec::<f32>::new().as_slice().into(),
            preamp_display_enabled: false,
            buffer_collisions: 0,
            buffer_peaks: Vec::<f32>::new().as_slice().into(),
            buffer_head: -1.0,
            buffer_write: 0.0,
            buffer_window_start: -1.0,
            buffer_window_end: -1.0,
            buffer_frozen: false,
            buffer_armed_freeze: 0,
            buffer_armed_gesture: false,
            buffer_history_bars: 0,
            buffer_position_bar: 1,
            buffer_position_beat: 1,
            buffer_position_tick: 0,
            wet_dry: 1.0,
            input_trim_db: 0.0,
            output_trim_db: 0.0,
            input_left_db: -9.0,
            input_right_db: -11.0,
            output_left_db: -12.0,
            output_right_db: -14.0,
            detector_db: -60.0,
            gain_reduction_db: 0.0,
            children: 0,
            depth: 0,
            // A fixture row: these tests are about what the rack draws, not
            // about the depth cap, so wrapping is offered.
            wrap_enabled: true,
            closing: Vec::<i32>::new().as_slice().into(),
            selected: false,
        },
        EffectSlotRow {
            kind: effect_kind_index(EffectKind::Limiter),
            units: effect_kind_units(EffectKind::Limiter),
            preset_options: Vec::<SharedString>::new().as_slice().into(),
            preset_name: Default::default(),
            bypassed: false,
            p0: 0.95,
            p1: 0.5,
            p2: 0.1,
            p3: 0.0,
            p4: 0.0,
            p5: 0.0,
            p6: 0.0,
            p7: 0.0,
            p8: 0.0,
            p9: 0.0,
            p10: 0.0,
            p11: 0.0,
            p12: 0.0,
            p13: 0.0,
            p14: 0.0,
            p15: 0.0,
            p16: 0.0,
            p17: 0.0,
            modulation_depths: Vec::<f32>::new().as_slice().into(),
            modulation_allowed: Vec::<bool>::new().as_slice().into(),
            modulation_offsets: Vec::<f32>::new().as_slice().into(),
            modulation_route_counts: Vec::<i32>::new().as_slice().into(),
            eq_band_data: Vec::<f32>::new().as_slice().into(),
            eq_band_kinds: Vec::<i32>::new().as_slice().into(),
            eq_pass_data: Vec::<f32>::new().as_slice().into(),
            eq_curve_db: Vec::<f32>::new().as_slice().into(),
            eq_spectrum_data: Vec::<f32>::new().as_slice().into(),
            eq_analyzer_enabled: false,
            preamp_deviation: Vec::<f32>::new().as_slice().into(),
            preamp_display_enabled: false,
            buffer_collisions: 0,
            buffer_peaks: Vec::<f32>::new().as_slice().into(),
            buffer_head: -1.0,
            buffer_write: 0.0,
            buffer_window_start: -1.0,
            buffer_window_end: -1.0,
            buffer_frozen: false,
            buffer_armed_freeze: 0,
            buffer_armed_gesture: false,
            buffer_history_bars: 0,
            buffer_position_bar: 1,
            buffer_position_beat: 1,
            buffer_position_tick: 0,
            wet_dry: 1.0,
            input_trim_db: 0.0,
            output_trim_db: 0.0,
            input_left_db: -12.0,
            input_right_db: -14.0,
            output_left_db: -10.0,
            output_right_db: -10.5,
            detector_db: -12.0,
            gain_reduction_db: -6.0,
            children: 0,
            depth: 0,
            // A fixture row: these tests are about what the rack draws, not
            // about the depth cap, so wrapping is offered.
            wrap_enabled: true,
            closing: Vec::<i32>::new().as_slice().into(),
            selected: false,
        },
    ]))));

    let snapshot = ui.window().take_snapshot().unwrap();
    assert_eq!((snapshot.width(), snapshot.height()), (1100, 760));
    assert!(snapshot.as_bytes().iter().any(|byte| *byte != 0));
    write_snapshot(&snapshot, "MOOLOOP_MIXER_SNAPSHOT");
}

/// **The turn**, which is the one gesture in the strip that nothing else
/// tests: a click on the arrow in a strip's bottom-right corner replaces its
/// lower half with the strip's own three sections, and a control on the face
/// that arrives has to report the parameter id the *engine* reads.
///
/// The corner is probed off `MOOLOOP_MIXER_SNAPSHOT` -- the small chevron at
/// the bottom right of strip 1. The control is then found by **sweeping the
/// strip** rather than by a second probe, so moving something inside the back
/// face cannot leave this test passing while testing nothing.
///
/// Swept upward on purpose. The tab row sits above the page, so a downward
/// sweep would click all four tabs before reaching the page area and would
/// then be looking at SENDS, which has no `in` switch to find.
#[test]
fn turning_a_strip_over_reaches_its_own_parameters() {
    let ui = headless();
    ui.invoke_show_view(view::MIXER);

    let moved = Rc::new(Cell::new((-1, -1)));
    let sink = moved.clone();
    ui.on_bus_strip_param(move |bus, param, _| sink.set((bus, param)));

    // Nothing on the front face reports a strip parameter.
    let mut ids = sweep_strip(&ui, &moved);
    assert!(
        ids.is_empty(),
        "the front face reported a strip parameter: {ids:?}"
    );

    click(&ui, TURN_OVER_X, TURN_OVER_Y);
    write_snapshot(
        &ui.window().take_snapshot().unwrap(),
        "MOOLOOP_TURNED_SNAPSHOT",
    );
    ids = sweep_strip(&ui, &moved);
    assert!(
        !ids.is_empty(),
        "nothing on the turned-over strip reported a parameter"
    );
    let known: Vec<i32> = StripParams::descriptors()
        .iter()
        .map(|descriptor| descriptor.id as i32)
        .collect();
    for id in &ids {
        assert!(
            known.contains(id),
            "the face reported {id}, which is not a parameter this strip has"
        );
    }
    assert!(
        ids.contains(&(mooloop_core::strip::STRIP_EQ_IN as i32)),
        "the EQ's own `in` switch was not reachable on the back face: {ids:?}"
    );

    // And it turns back.
    click(&ui, TURN_OVER_X, TURN_OVER_Y);
    assert!(
        sweep_strip(&ui, &moved).is_empty(),
        "the strip did not turn back to its fader"
    );
}

/// **Responsive rather than a mode.** Adam, 2026-09-12: *"i'd like this to
/// basically just be responsive design -- if the mixer is big enough, it
/// shows everything: pre, eq, comp, sends, fader."* So there is no zoom to
/// press and nothing to turn: a pane given `MixerMetrics.full-height` draws
/// the whole strip on the face you are already looking at.
///
/// The height comes out of the global rather than being written here, and
/// the chrome above the pane carries a margin, because what this asserts is
/// the *reachability* -- the same claim `THE-STRIP.md` makes about the three
/// paged faces, applied to the fourth arrangement of the same controls.
#[test]
fn a_tall_enough_mixer_draws_the_whole_strip_without_turning_it() {
    let ui = headless();
    ui.invoke_show_view(view::MIXER);
    let full = ui.global::<MixerMetrics>().get_full_height();
    ui.window()
        .set_size(LogicalSize::new(1100.0, PANE_CHROME + full));

    let moved = Rc::new(Cell::new((-1, -1)));
    let sink = moved.clone();
    ui.on_bus_strip_param(move |bus, param, _| sink.set((bus, param)));
    write_snapshot(
        &ui.window().take_snapshot().unwrap(),
        "MOOLOOP_FULL_SNAPSHOT",
    );

    // Nothing was turned: the sections are simply there, above the sends and
    // the fader, in the order the mockup stacks them.
    let ids = sweep_column(&ui, &moved, SECTIONS_TOP, SECTIONS_BOTTOM);
    assert!(
        ids.contains(&(mooloop_core::strip::STRIP_EQ_IN as i32)),
        "the EQ was not on the strip the pane had room to draw whole: {ids:?}"
    );
    assert!(
        ids.contains(&(mooloop_core::strip::STRIP_COMP_IN as i32)),
        "the compressor was not on it either, so the sections are cut off \
         rather than laid out: {ids:?}"
    );

    // And the fader is still the strip's, under the sends where the mockup
    // puts it -- it is what the room a tall pane leaves is *for*.
    let volume = Rc::new(Cell::new(-1.0_f32));
    let level = volume.clone();
    ui.on_bus_volume_changed(move |bus, value| {
        if bus == 1 {
            level.set(value);
        }
    });
    // Dragged, not clicked. A mixer fader is deliberately **relative** --
    // grabbing it never jumps the level -- so a press and a release in the
    // same place is exactly the gesture it is built to ignore.
    drag(&ui, FULL_FADER_X, FULL_FADER_Y, FULL_FADER_Y - 40.0);
    assert!(
        volume.get() >= 0.0,
        "the fader is not under the sends on the full strip"
    );
}

/// A press, a move and a release, for the controls that only answer to a
/// drag.
fn drag(ui: &MainWindow, x: f32, from: f32, to: f32) {
    let start = LogicalPosition::new(x, from);
    let end = LogicalPosition::new(x, to);
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
}

/// The chrome above the mixer pane: the menu bar, both toolbar rows and the
/// pane's own tab row, plus the dock below it. Measured against
/// `headless()`'s 760, where the pane comes out about 200 tall, and rounded
/// **up** so the pane clears the breakpoint rather than landing on it.
const PANE_CHROME: f32 = 600.0;
/// The band strip 1's three sections occupy on the full face: from just
/// under the name plate to `MixerMetrics.strip-page-height` below it. Probed
/// off `MOOLOOP_FULL_SNAPSHOT`.
const SECTIONS_TOP: f32 = 125.0;
const SECTIONS_BOTTOM: f32 = 460.0;
/// Strip 1's fader on the full face, probed off the same snapshot: it runs
/// from about 548 to 660, under the sends and above the destination. The x
/// is the same on every face -- the fader does not move sideways -- and only
/// the y is particular to this arrangement.
const FULL_FADER_X: f32 = 156.0;
const FULL_FADER_Y: f32 = 620.0;

/// Every parameter id reported by clicking over strip 1's own column, from
/// just above its turn-over button up to just below its fader. Deliberately
/// clear of the corner, so the sweep cannot turn the strip over half way
/// through and start testing the other face.
fn sweep_strip(ui: &MainWindow, moved: &Rc<Cell<(i32, i32)>>) -> Vec<i32> {
    sweep_column(ui, moved, 190.0, 285.0)
}

/// The same sweep between two heights, for the faces that put the strip's
/// sections somewhere else.
fn sweep_column(
    ui: &MainWindow,
    moved: &Rc<Cell<(i32, i32)>>,
    top: f32,
    bottom: f32,
) -> Vec<i32> {
    let mut ids: Vec<i32> = Vec::new();
    let mut y = bottom;
    while y > top {
        // Clear of the page's own scroll bar at the strip's right edge,
        // which is a control and would otherwise be most of what this
        // sweep clicks.
        let mut x = 118.0;
        while x < 186.0 {
            moved.set((-1, -1));
            click(ui, x, y);
            let (bus, param) = moved.get();
            if bus == 1 && !ids.contains(&param) {
                ids.push(param);
            }
            x += 3.0;
        }
        y -= 3.0;
    }
    ids
}

/// The arrow in strip 1's bottom-right corner, probed off
/// `MOOLOOP_MIXER_SNAPSHOT`. It is the face's last row rather than an
/// overlay, so it sits just inside the strip's own bottom padding.
const TURN_OVER_X: f32 = 194.0;
const TURN_OVER_Y: f32 = 291.0;

/// Clicking a strip's name plate is the gesture that points the device rack at
/// that bus. If it stops reporting, the mixer becomes a display.
///
/// The coordinates are measured, not derived: render the pane with
/// `MOOLOOP_MIXER_SNAPSHOT` and probe it if this assertion ever moves.
#[test]
fn clicking_a_strip_name_selects_that_bus() {
    let ui = headless();
    ui.invoke_show_view(view::MIXER);

    let picked = Rc::new(Cell::new(-1));
    let sink = picked.clone();
    ui.on_bus_selected(move |bus| sink.set(bus));

    // Name-plate row of the first three strips: master, then two inserts one
    // strip pitch apart.
    let pitch = strip_pitch(&ui);
    // Inside the first strip's name plate. Any point in it will do: what the
    // clicks below are about is the pitch between strips, not the plate's
    // own extent.
    let first = 54.0;
    click(&ui, first, NAME_PLATE_Y);
    assert_eq!(picked.get(), 0, "the leftmost strip is the master");
    click(&ui, first + pitch, NAME_PLATE_Y);
    assert_eq!(picked.get(), 1);
    click(&ui, first + pitch * 2.0, NAME_PLATE_Y);
    assert_eq!(picked.get(), 2);

    // The gap between two strips belongs to neither. Found by walking
    // rather than computed: the pane sits at an offset inside the work area,
    // so the only honest way to name an absolute gutter coordinate is to
    // sweep for it -- and the property worth asserting is that two strips
    // are not adjacent, whatever the offset is.
    let mut runs: Vec<i32> = Vec::new();
    for step in 0..(pitch as i32 * 2) {
        picked.set(-2);
        click(&ui, first + step as f32, NAME_PLATE_Y);
        let hit = picked.get();
        if runs.last() != Some(&hit) {
            runs.push(hit);
        }
    }
    assert_eq!(
        runs,
        vec![0, -2, 1, -2, 2],
        "a sweep across two strip pitches should read strip, gap, strip, gap, strip"
    );
}

/// A channel owns its mixer destination even while the mixer pane is hidden.
/// Exercise the real popup rather than invoking the Rust callback directly:
/// this is the path that previously made every channel appear stuck on Master.
#[test]
fn channel_bus_picker_reports_the_selected_destination() {
    let ui = headless();
    ui.invoke_show_view(view::STEPS);

    // How the three constants below were measured, kept rather than deleted:
    // they are rack-row geometry, and the rack row moved on 2026-09-09 when
    // the reorder wrapped it, again on 2026-09-10 when that wrapper stopped
    // stretching, and twice before that. Render with
    // `MOOLOOP_RACK_ROW_SNAPSHOT` and read the picker's centre off the image.
    write_snapshot(
        &ui.window().take_snapshot().unwrap(),
        "MOOLOOP_RACK_ROW_SNAPSHOT",
    );

    let picked = Rc::new(Cell::new(-1));
    let sink = picked.clone();
    ui.on_channel_bus_changed(move |channel, bus| {
        assert_eq!(channel, 0);
        sink.set(bus);
    });

    // The first rack row's destination picker, then the fourth item in its
    // popup (Master, Bus 1, Bus 2, Bus 3).
    click(&ui, CHANNEL_BUS_PICKER_X, CHANNEL_ROW_Y);
    click(&ui, CHANNEL_BUS_PICKER_X, CHANNEL_MENU_BUS_3_Y);
    assert_eq!(picked.get(), 3);
}

/// Vertical centre of a strip's name plate, below the menu bar and both
/// toolbar rows.
///
/// Twenty-seven pixels higher than it was on 2026-09-08: the work surface
/// used to carry a 26px strip and its 1px rule above the pane, holding only
/// the Steps/Mixer switcher, which now leads the toolbar row above.
const NAME_PLATE_Y: f32 = 109.0;
/// Strip width plus the layout gap between two strips, both read from
/// `MixerMetrics` rather than kept here.
///
/// It was 66 until 2026-09-11, when the strip went to 92px so that three
/// knobs fit a row and an EQ band could be one -- and a test holding its own
/// copy of a width is how a passing suite comes to be clicking the gutter.
fn strip_pitch(ui: &MainWindow) -> f32 {
    let metrics = ui.global::<MixerMetrics>();
    metrics.get_strip_width() + metrics.get_strip_gap()
}

/// Centre of the first channel row's bus picker in the normal work surface.
const CHANNEL_BUS_PICKER_X: f32 = 202.0;
const CHANNEL_ROW_Y: f32 = 116.0;
/// Centre of Bus 3 in the picker popup. The menu opens directly below its
/// 22px owner and each option is 21px tall after 4px top padding.
const CHANNEL_MENU_BUS_3_Y: f32 = 216.0;

/// The channel sidebar with a channel selected, which is where a channel's
/// mixer destination can be set without the rack row in front of you.
///
/// `headless` already hands the window a rack whose first channel is
/// selected and feeds Bus 3, so this only has to open the panel and say that
/// the selection is a channel rather than a track -- the one flag that
/// decides which half of the panel is drawn.
fn face_with_channel_output() -> MainWindow {
    let ui = headless();
    ui.invoke_show_view(view::STEPS);
    ui.set_channel_sidebar_visible(true);
    ui.set_channel_sidebar_width(260.0);
    ui.set_editing_bus(false);
    ui.set_selected_channel(0);
    ui
}

/// The OUTPUT chip in the channel sidebar, and Master in the popup it opens.
///
/// Measured from `MOOLOOP_CHANNEL_OUTPUT_SNAPSHOT` rather than derived, for
/// the reason the send rows' constants above record at length: the panel
/// stacks a label and a field above this one and the heights of both are the
/// renderer's, not the markup's. The chip is 96px wide and spans x 16..111
/// in a panel whose own left edge is the window's 8px inset plus 8px of
/// padding, so this is its centre; the popup hangs 2px under it.
const CHANNEL_OUTPUT_PICKER_X: f32 = 64.0;
const CHANNEL_OUTPUT_PICKER_Y: f32 = 174.0;
const CHANNEL_OUTPUT_MENU_MASTER_Y: f32 = 202.0;

/// The panel **with its menu open**, which is what the three constants above
/// are read off. A render of the closed chip would show where to click once
/// and say nothing about where the rows land.
#[test]
fn render_a_channels_output_picker() {
    let ui = face_with_channel_output();
    click(&ui, CHANNEL_OUTPUT_PICKER_X, CHANNEL_OUTPUT_PICKER_Y);
    let snapshot = ui.window().take_snapshot().unwrap();
    write_snapshot(&snapshot, "MOOLOOP_CHANNEL_OUTPUT_SNAPSHOT");
    assert!(snapshot.width() > 0 && snapshot.height() > 0);
}

/// Which row of the open OUTPUT menu is lit, read off a render.
///
/// The active row is drawn in the accent, which is green, and every other
/// row's background is a neutral grey -- so "more green than red" separates
/// them without this file keeping a copy of the theme's colour. Sampled 40px
/// left of the chip's centre, which is inside the row and clear of its
/// centred text.
fn lit_menu_row(snapshot: &slint::SharedPixelBuffer<slint::Rgba8Pixel>) -> Option<usize> {
    let pixels = snapshot.as_bytes().as_chunks::<4>().0;
    let x = CHANNEL_OUTPUT_PICKER_X as u32 - 40;
    (0..MAX_BUSES).find(|row| {
        let y = CHANNEL_OUTPUT_MENU_MASTER_Y as u32 + 21 * *row as u32;
        let pixel = pixels[(y * snapshot.width() + x) as usize];
        i32::from(pixel[1]) - i32::from(pixel[0]) > 20
    })
}

/// The panel names **this channel's** destination, not a default and not its
/// neighbour's.
///
/// `channel-bus` is the half of this the test below cannot see: a picker
/// wired to the wrong row would still report the track that was clicked
/// while describing the wrong one, and a description is what a routing
/// decision gets made from. The lit row in the open menu *is* the value the
/// chip was handed, so this reads it off the render rather than trusting a
/// property it set itself.
#[test]
fn the_channel_sidebars_output_names_the_track_that_channel_feeds() {
    // `rack_rows`: Kick and Snare on Bus 3, Bass on Bus 5, Pad on the
    // master. Three channels rather than one, because a binding stuck on a
    // constant agrees with any single case.
    for (channel, bus) in [(0, 3), (2, 5), (3, 0)] {
        let ui = face_with_channel_output();
        ui.set_selected_channel(channel);
        click(&ui, CHANNEL_OUTPUT_PICKER_X, CHANNEL_OUTPUT_PICKER_Y);
        let snapshot = ui.window().take_snapshot().unwrap();
        assert_eq!(
            lit_menu_row(&snapshot),
            Some(bus),
            "the sidebar drew channel {channel} feeding something other than bus {bus}"
        );
    }
}

/// The sidebar's OUTPUT picker moves **the selected channel**, and reports
/// the track that was clicked.
///
/// The picker in the rack row beside the steps has the same job and its own
/// test above; this is a second instance of one control, which in this
/// codebase is the arrangement that drifts. What it would look like is a
/// panel that sets somebody else's destination, or sets nothing at all --
/// `channel_bus_picker_reports_the_selected_destination` records the popup
/// closing over its own click and every channel appearing stuck on Master,
/// which is the failure this shape produces.
#[test]
fn the_channel_sidebar_picks_a_mixer_track() {
    let ui = face_with_channel_output();

    let picked = Rc::new(Cell::new((-1, -1)));
    let sink = picked.clone();
    ui.on_channel_bus_changed(move |channel, bus| sink.set((channel, bus)));

    ui.set_selected_channel(2);
    click(&ui, CHANNEL_OUTPUT_PICKER_X, CHANNEL_OUTPUT_PICKER_Y);
    click(&ui, CHANNEL_OUTPUT_PICKER_X, CHANNEL_OUTPUT_MENU_MASTER_Y);
    assert_eq!(
        picked.get(),
        (2, 0),
        "the sidebar's OUTPUT picker did not move the selected channel to the master"
    );
}

/// The three sends a track is given, drawn in the real window and the real
/// layout.
///
/// **They moved on 2026-09-13.** They were an area of the track's device face
/// and are now the channel sidebar's, so this opens the sidebar rather than
/// relying on the bottom pane: the panel draws exactly the sends that exist,
/// there is no fixed number of bars and no empty bays, and this is what says
/// the model reaches it. Render with `MOOLOOP_SENDS_SNAPSHOT` to probe the
/// coordinates the tests below use.
fn face_with_sends() -> MainWindow {
    let ui = headless();
    ui.invoke_show_view(view::MIXER);
    ui.invoke_show_view(view::DEVICES);
    ui.set_channel_sidebar_visible(true);
    ui.set_channel_sidebar_width(260.0);
    ui.set_editing_bus(true);
    ui.set_editing_bus_index(1);
    ui.set_editing_bus_name(SharedString::from("Drums"));
    ui.set_editing_bus_volume(1.0);
    ui.set_editing_bus_feed_count(2);
    ui.set_editing_bus_send_allowed(ModelRc::from(vec![true, false, true, true].as_slice()));
    ui.set_editing_bus_sends(ModelRc::from(Rc::new(VecModel::from(vec![
        MixerSendRow {
            target: 3,
            target_name: SharedString::from("Reverb"),
            level: 0.5,
            pre_fader: false,
            enabled: true,
        },
        MixerSendRow {
            target: 2,
            target_name: SharedString::from("Duck"),
            level: 1.0,
            pre_fader: true,
            enabled: false,
        },
        MixerSendRow {
            target: 3,
            target_name: SharedString::from("Plate"),
            level: 0.25,
            pre_fader: false,
            enabled: true,
        },
    ]))));
    ui
}

#[test]
fn render_a_tracks_sends() {
    let ui = face_with_sends();
    let snapshot = ui.window().take_snapshot().unwrap();
    write_snapshot(&snapshot, "MOOLOOP_SENDS_SNAPSHOT");
    assert!(snapshot.width() > 0 && snapshot.height() > 0);
}

/// Where the send rows' switches land, measured from
/// `MOOLOOP_SENDS_SNAPSHOT` rather than derived. Probe it again if these move.
///
/// They moved on 2026-09-13, when sends left the track's device face for the
/// channel sidebar: the row pitch is 45px and the first row's controls sat at
/// y 248 in a 260px-wide panel. The lamps were found by scanning the render
/// for accent-coloured pixels rather than by counting paddings, which is the
/// same reason the old numbers were measured rather than derived.
///
/// **They moved again on 2026-09-15**, up 74px, when the colour picker became
/// a chip beside the name field and its swatch grid left the panel. Measured
/// the same way, and the arithmetic is why: the block that went looks like
/// 94px of label, grid and hex field, but the grid draws one row rather than
/// two when the palette model is empty, as it is in a headless fixture. A
/// derived number would have been 20px wrong and the clicks would have landed
/// on nothing.
///
/// Only rows 0 and 2 are lit in the fixture -- row 1's send is disabled -- so
/// the scan finds lamps 90px apart and the pitch is half that.
const SEND_ENABLE_X: f32 = 216.0;
const SEND_ROW_0_Y: f32 = 174.0;
const SEND_ROW_1_Y: f32 = 219.0;

/// A send row reports **its own** index.
///
/// The failure this exists for is the one Slint makes easy and this codebase
/// has already been bitten by three times in popups: a `for` body that closes
/// over the wrong thing draws perfectly and acts on somebody else's row. A
/// send that removed its neighbour, or turned down the one above it, would
/// look like a mix that changed on its own.
#[test]
fn a_send_row_reports_its_own_index() {
    let ui = face_with_sends();

    let toggled = Rc::new(Cell::new((-1, -1)));
    let sink = toggled.clone();
    ui.on_send_enable_toggled(move |bus, send| sink.set((bus, send)));

    click(&ui, SEND_ENABLE_X, SEND_ROW_0_Y);
    assert_eq!(
        toggled.get(),
        (1, 0),
        "the first row switched something other than itself"
    );

    click(&ui, SEND_ENABLE_X, SEND_ROW_1_Y);
    assert_eq!(
        toggled.get(),
        (1, 1),
        "the second row switched something other than itself"
    );
}

/// The remove button, one gap to the right of the switch above. It spans
/// x 230..=247 in the sidebar, so this is its centre.
const SEND_REMOVE_X: f32 = 238.0;

/// The remove button is **reachable**, which is the half of the row the test
/// above does not cover.
///
/// This is not a duplicate of it. The scroll bar is drawn over the right edge
/// of the viewport rather than beside it, so the rightmost control in a send
/// row can sit underneath it and stop receiving clicks entirely while every
/// control to its left keeps working -- which is exactly what happened at the
/// row's first padding, and why `SendRow` reserves 12px on the right and why
/// this dispatches a real pointer event instead of invoking the callback.
///
/// A remove button that cannot be clicked is a send that cannot be undone,
/// and nothing else in the interface would look wrong.
#[test]
fn a_send_row_can_be_removed_from_under_the_scroll_bar() {
    let ui = face_with_sends();

    let removed = Rc::new(Cell::new((-1, -1)));
    let sink = removed.clone();
    ui.on_send_removed(move |bus, send| sink.set((bus, send)));

    click(&ui, SEND_REMOVE_X, SEND_ROW_0_Y);
    assert_eq!(
        removed.get(),
        (1, 0),
        "the remove button did not report -- it is most likely under the scroll bar again"
    );

    click(&ui, SEND_REMOVE_X, SEND_ROW_1_Y);
    assert_eq!(
        removed.get(),
        (1, 1),
        "the second row removed something other than itself"
    );
}
