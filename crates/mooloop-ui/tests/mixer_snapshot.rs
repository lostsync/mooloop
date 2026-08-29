use mooloop_core::MAX_BUSES;
use mooloop_ui::{ChannelRow, EffectSlotRow, MainWindow, MixerStripRow, StepCell};
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

fn rack_rows() -> ModelRc<ChannelRow> {
    let rows = [("Kick", 3), ("Snare", 3), ("Bass", 5), ("Pad", 0)]
        .into_iter()
        .enumerate()
        .map(|(index, (name, bus))| ChannelRow {
            name: SharedString::from(name),
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
                muted: false,
                volume: 1.0,
                pan: 0.0,
                output: 0,
                selected: index == selected,
                is_master: index == 0,
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
            })
            .collect::<Vec<_>>(),
    ))
}

fn headless() -> MainWindow {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(SharedString::from("software")),
        },
    )))
    .expect("initialize headless renderer");

    let ui = MainWindow::new().unwrap();
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
    ui.set_mixer_visible(true);
    ui.set_editor_page(0);
    ui.set_editing_bus(true);
    ui.set_editing_bus_index(3);
    ui.set_editing_bus_name(SharedString::from("Bus 3"));
    ui.set_editing_bus_feed_count(2);
    ui.set_editing_bus_volume(1.0);
    ui.set_editing_bus_left_db(-9.0);
    ui.set_editing_bus_right_db(-11.0);
    ui.set_effect_slots(ModelRc::from(Rc::new(VecModel::from(vec![
        EffectSlotRow {
            kind: 5,
            units: 2,
            bypassed: false,
            p0: 0.5,
            p1: 0.4,
            p2: 0.2,
            p3: 0.3,
            p4: 0.1,
            p5: 0.0,
            p6: 0.0,
            p7: 0.0,
            modulation_depths: Vec::<f32>::new().as_slice().into(),
            modulation_allowed: Vec::<bool>::new().as_slice().into(),
            eq_band_data: Vec::<f32>::new().as_slice().into(),
            eq_spectrum_data: Vec::<f32>::new().as_slice().into(),
            eq_analyzer_enabled: false,
            buffer_collisions: 0,
            wet_dry: 1.0,
            input_trim_db: 0.0,
            output_trim_db: 0.0,
            input_left_db: -9.0,
            input_right_db: -11.0,
            output_left_db: -12.0,
            output_right_db: -14.0,
        },
        EffectSlotRow {
            kind: 6,
            units: 1,
            bypassed: false,
            p0: 0.95,
            p1: 0.5,
            p2: 0.1,
            p3: 0.0,
            p4: 0.0,
            p5: 0.0,
            p6: 0.0,
            p7: 0.0,
            modulation_depths: Vec::<f32>::new().as_slice().into(),
            modulation_allowed: Vec::<bool>::new().as_slice().into(),
            eq_band_data: Vec::<f32>::new().as_slice().into(),
            eq_spectrum_data: Vec::<f32>::new().as_slice().into(),
            eq_analyzer_enabled: false,
            buffer_collisions: 0,
            wet_dry: 1.0,
            input_trim_db: 0.0,
            output_trim_db: 0.0,
            input_left_db: -12.0,
            input_right_db: -14.0,
            output_left_db: -10.0,
            output_right_db: -10.5,
        },
    ]))));

    let snapshot = ui.window().take_snapshot().unwrap();
    assert_eq!((snapshot.width(), snapshot.height()), (1100, 760));
    assert!(snapshot.as_bytes().iter().any(|byte| *byte != 0));
    write_snapshot(&snapshot, "MOOLOOP_MIXER_SNAPSHOT");
}

/// Clicking a strip's name plate is the gesture that points the device rack at
/// that bus. If it stops reporting, the mixer becomes a display.
///
/// The coordinates are measured, not derived: render the pane with
/// `MOOLOOP_MIXER_SNAPSHOT` and probe it if this assertion ever moves.
#[test]
fn clicking_a_strip_name_selects_that_bus() {
    let ui = headless();
    ui.set_mixer_visible(true);

    let picked = Rc::new(Cell::new(-1));
    let sink = picked.clone();
    ui.on_bus_selected(move |bus| sink.set(bus));

    // Name-plate row of the first three strips: master, then two inserts one
    // strip pitch apart.
    click(&ui, 44.0, NAME_PLATE_Y);
    assert_eq!(picked.get(), 0, "the leftmost strip is the master");
    click(&ui, 44.0 + STRIP_PITCH, NAME_PLATE_Y);
    assert_eq!(picked.get(), 1);
    click(&ui, 44.0 + STRIP_PITCH * 2.0, NAME_PLATE_Y);
    assert_eq!(picked.get(), 2);

    // The gap between two strips belongs to neither.
    picked.set(-1);
    click(&ui, 80.0, NAME_PLATE_Y);
    assert_eq!(picked.get(), -1, "the gutter must not select a bus");
}

/// A channel owns its mixer destination even while the mixer pane is hidden.
/// Exercise the real popup rather than invoking the Rust callback directly:
/// this is the path that previously made every channel appear stuck on Master.
#[test]
fn channel_bus_picker_reports_the_selected_destination() {
    let ui = headless();
    ui.set_mixer_visible(false);

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

/// Vertical centre of a strip's name plate, below the menu bar, both toolbar
/// rows, and the work surface's Steps/Mixer header.
const NAME_PLATE_Y: f32 = 136.0;
/// Strip width plus the layout gap between two strips.
const STRIP_PITCH: f32 = 66.0;

/// Centre of the first channel row's bus picker in the normal work surface.
const CHANNEL_BUS_PICKER_X: f32 = 202.0;
const CHANNEL_ROW_Y: f32 = 143.0;
/// Centre of Bus 3 in the picker popup. The menu opens directly below its
/// 22px owner and each option is 21px tall after 4px top padding.
const CHANNEL_MENU_BUS_3_Y: f32 = 243.0;
