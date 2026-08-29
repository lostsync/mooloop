use mooloop_core::{DrumMode, DrumSynthParams};
use mooloop_dsp::DrumSynth;
use mooloop_ui::{
    ChannelRow, EffectSlotRow, MainWindow, ModulationRouteRow, ModulationSourceRow, StepCell,
};
use slint::platform::WindowEvent;
use slint::{ComponentHandle, LogicalPosition, LogicalSize, ModelRc, SharedString, VecModel};
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
    let rows = ["Kick", "Snare", "Closed Hat", "Open Hat"]
        .into_iter()
        .enumerate()
        .map(|(index, name)| ChannelRow {
            name: SharedString::from(name),
            muted: false,
            volume_db: -1.9382, // linear 0.8 in dB
            pan: 0.0,
            selected: index == 0,
            bus: 0,
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

fn set_drum_preview(ui: &MainWindow, params: DrumSynthParams) {
    let (minimums, maximums) = DrumSynth::preview_waveform(params, 144);
    ui.set_drum_preview_minimums(ModelRc::from(Rc::new(VecModel::from(minimums))));
    ui.set_drum_preview_maximums(ModelRc::from(Rc::new(VecModel::from(maximums))));
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
    set_drum_preview(&ui, DrumSynthParams::default());
    let drum = ui.window().take_snapshot().unwrap();
    assert_eq!((drum.width(), drum.height()), (960, 760));
    assert!(drum.as_bytes().iter().any(|byte| *byte != 0));
    write_snapshot(&drum, "MOOLOOP_DRUM_SOURCE_SNAPSHOT");

    ui.set_drum_mode(1);
    set_drum_preview(
        &ui,
        DrumSynthParams {
            mode: DrumMode::Snare,
            ..DrumSynthParams::default()
        },
    );
    let snare = ui.window().take_snapshot().unwrap();
    assert_ne!(drum.as_bytes(), snare.as_bytes());
    write_snapshot(&snare, "MOOLOOP_SNARE_SOURCE_SNAPSHOT");

    ui.set_drum_mode(2);
    set_drum_preview(
        &ui,
        DrumSynthParams {
            mode: DrumMode::Hat,
            ..DrumSynthParams::default()
        },
    );
    let hat = ui.window().take_snapshot().unwrap();
    assert_ne!(snare.as_bytes(), hat.as_bytes());
    write_snapshot(&hat, "MOOLOOP_HAT_SOURCE_SNAPSHOT");

    ui.set_source_kind(2);
    ui.set_selected_channel_name(SharedString::from("Mono"));
    let mono = ui.window().take_snapshot().unwrap();
    assert_eq!((mono.width(), mono.height()), (960, 760));
    assert_ne!(drum.as_bytes(), mono.as_bytes());
    write_snapshot(&mono, "MOOLOOP_MONO_SOURCE_SNAPSHOT");

    ui.set_mono_osc1_wave(3);
    ui.set_mono_osc1_level(1.0);
    ui.set_mono_osc1_pulse_width(0.2);
    let narrow_pulse = ui.window().take_snapshot().unwrap();
    ui.set_mono_osc1_pulse_width(0.8);
    let wide_pulse = ui.window().take_snapshot().unwrap();
    assert_ne!(narrow_pulse.as_bytes(), wide_pulse.as_bytes());
    write_snapshot(&wide_pulse, "MOOLOOP_MONO_PULSE_SOURCE_SNAPSHOT");

    ui.set_mono_device_page(1);
    ui.set_mono_filter_cutoff(0.35);
    ui.set_mono_filter_resonance(0.8);
    let mono_amp = ui.window().take_snapshot().unwrap();
    assert_ne!(mono.as_bytes(), mono_amp.as_bytes());
    write_snapshot(&mono_amp, "MOOLOOP_MONO_AMP_SOURCE_SNAPSHOT");

    ui.set_mono_device_page(2);
    let mono_mod = ui.window().take_snapshot().unwrap();
    assert_ne!(mono_amp.as_bytes(), mono_mod.as_bytes());
    write_snapshot(&mono_mod, "MOOLOOP_MONO_MOD_SOURCE_SNAPSHOT");

    ui.window().set_size(LogicalSize::new(720.0, 760.0));
    let narrow = ui.window().take_snapshot().unwrap();
    assert_eq!((narrow.width(), narrow.height()), (720, 760));
    assert!(narrow.as_bytes().iter().any(|byte| *byte != 0));
    write_snapshot(&narrow, "MOOLOOP_MONO_SOURCE_NARROW_SNAPSHOT");
}

#[test]
fn render_sampler_source_editor() {
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
    ui.set_source_kind(0);
    ui.set_sample_name(SharedString::from("kick_808.wav"));
    ui.set_sample_description(SharedString::from("48kHz / 16-bit / mono"));
    ui.set_sample_duration(1.2);
    ui.set_can_previous_sample(true);
    ui.set_can_next_sample(true);
    ui.set_waveform(ModelRc::from(Rc::new(VecModel::from(vec![
        0.2, 0.6, 0.9, 0.4, 0.3, 0.7, 0.5, 0.1,
    ]))));
    ui.set_sample_frames(57600);
    ui.set_tune_label(SharedString::from("C4 · 261.6 Hz"));
    ui.set_playhead_positions(ModelRc::from(Rc::new(VecModel::from(vec![0.42, 0.58]))));

    let snapshot = ui.window().take_snapshot().unwrap();
    assert_eq!((snapshot.width(), snapshot.height()), (960, 760));
    assert!(snapshot.as_bytes().iter().any(|byte| *byte != 0));
    write_snapshot(&snapshot, "MOOLOOP_SAMPLER_SOURCE_SNAPSHOT");

    // The shelf is channel-owned rather than a page inside the sampler. Its
    // armed cutoff markers and destination-first route row are visible
    // together, so a screenshot catches a lost model binding or a collapsed
    // layout before it reaches the application. Use the normal desktop size
    // for this expanded rack surface; the 960x760 shot above still covers the
    // compact-window composition and this one exposes the complete shelf.
    ui.window().set_size(LogicalSize::new(1440.0, 900.0));
    ui.set_sampler_device_page(2);
    ui.set_modulation_shelf_open(true);
    ui.set_modulation_selected_slot(0);
    ui.set_modulation_armed_slot(0);
    ui.set_modulation_sources(ModelRc::from(Rc::new(VecModel::from(vec![
        ModulationSourceRow {
            slot: 0,
            name: SharedString::from("LFO 1"),
            waveform: 0,
            rate: 2.0,
            depth: 1.0,
            phase: 0.0,
            retrigger: false,
            selected: true,
        },
    ]))));
    ui.set_modulation_routes(ModelRc::from(Rc::new(VecModel::from(vec![
        ModulationRouteRow {
            route_index: 0,
            source_slot: 0,
            owner: -1,
            param: 12,
            destination: SharedString::from("LFO 1 → Kick · Cutoff"),
            depth: 0.35,
            polarity: 0,
            allowed: true,
        },
    ]))));
    ui.set_modulation_max_sources(4);
    ui.set_modulation_selected_rate(2.0);
    ui.set_modulation_selected_depth(1.0);
    // Descriptor-id indexed, so the sampler's cutoff overlay sits at 12.
    let mut source_depths = vec![0.0f32; 22];
    source_depths[12] = 0.35;
    ui.set_source_modulation_depths(source_depths.as_slice().into());
    ui.set_source_modulation_allowed(vec![true; 22].as_slice().into());
    let modulation = ui.window().take_snapshot().unwrap();
    assert_eq!((modulation.width(), modulation.height()), (1440, 900));
    assert_ne!(snapshot.as_bytes(), modulation.as_bytes());
    write_snapshot(&modulation, "MOOLOOP_MODULATION_SHELF_SNAPSHOT");

    ui.window().set_size(LogicalSize::new(960.0, 760.0));
    ui.set_sampler_device_page(1);
    let voice = ui.window().take_snapshot().unwrap();
    assert_ne!(snapshot.as_bytes(), voice.as_bytes());
    write_snapshot(&voice, "MOOLOOP_SAMPLER_VOICE_SOURCE_SNAPSHOT");

    ui.set_sampler_device_page(2);
    let tone = ui.window().take_snapshot().unwrap();
    assert_ne!(voice.as_bytes(), tone.as_bytes());
    write_snapshot(&tone, "MOOLOOP_SAMPLER_TONE_SOURCE_SNAPSHOT");

    ui.set_bit_reduction(1.0);
    ui.set_rate_reduction(1.0);
    let crushed = ui.window().take_snapshot().unwrap();
    assert_ne!(tone.as_bytes(), crushed.as_bytes());
    write_snapshot(&crushed, "MOOLOOP_SAMPLER_CRUSHED_SOURCE_SNAPSHOT");
}

fn effect_slot(kind: i32, units: i32) -> EffectSlotRow {
    EffectSlotRow {
        kind,
        units,
        bypassed: false,
        p0: 0.5,
        p1: 0.5,
        p2: 0.0,
        p3: 0.5,
        p4: 0.5,
        p5: 0.5,
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
        input_left_db: -60.0,
        input_right_db: -60.0,
        output_left_db: -60.0,
        output_right_db: -60.0,
    }
}

#[test]
fn render_effect_header_comparison() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(SharedString::from("software")),
        },
    )))
    .expect("initialize headless renderer");

    let ui = MainWindow::new().unwrap();
    ui.window().set_size(LogicalSize::new(3200.0, 760.0));
    ui.set_channels(rack_rows());
    ui.set_pattern_length(16);
    ui.set_selected_channel_name(SharedString::from("Kick"));
    ui.set_editor_page(0);
    ui.set_source_kind(1);
    set_drum_preview(&ui, DrumSynthParams::default());

    let mut reverb = effect_slot(8, 3);
    reverb.p1 = 1.0;
    ui.set_effect_slots(ModelRc::from(Rc::new(VecModel::from(vec![
        effect_slot(0, 1),
        effect_slot(1, 1),
        effect_slot(2, 1),
        effect_slot(5, 2),
        reverb,
        // The buffer's own knobs, so its face is covered now that it has a
        // parameter surface and is not only a debug trigger panel.
        effect_slot(11, 1),
    ]))));

    let snapshot = ui.window().take_snapshot().unwrap();
    assert_eq!((snapshot.width(), snapshot.height()), (3200, 760));
    assert!(snapshot.as_bytes().iter().any(|byte| *byte != 0));
    write_snapshot(&snapshot, "MOOLOOP_EFFECT_HEADERS_SNAPSHOT");
}

/// The rack is wider than the window once a few devices are in the chain, so
/// it has to scroll horizontally to reach them.
///
/// This regressed because the viewport width was a constant sized for an
/// empty chain: every device past it was laid out but unreachable, and the
/// view could not scroll at all because the viewport never exceeded the
/// visible width.
#[test]
fn effect_rack_scrolls_horizontally_to_reach_a_long_chain() {
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
    ui.set_selected_channel_name(SharedString::from("Kick"));
    ui.set_editor_page(0);
    ui.set_source_kind(0);
    // Filter, drive, bitcrush, delay (two units wide), filter: comfortably
    // past the 960 px window even before the source device's own three units.
    ui.set_effect_slots(ModelRc::from(Rc::new(VecModel::from(vec![
        effect_slot(0, 1),
        effect_slot(1, 1),
        effect_slot(2, 1),
        effect_slot(3, 2),
        effect_slot(0, 1),
    ]))));

    let unscrolled = ui.window().take_snapshot().unwrap();

    // A point inside the device rack: below the page tabs and the device
    // chain header, above the bottom of the dock.
    let over_rack = LogicalPosition::new(480.0, 560.0);
    for _ in 0..12 {
        ui.window().dispatch_event(WindowEvent::PointerScrolled {
            position: over_rack,
            delta_x: -240.0,
            delta_y: -240.0,
        });
    }
    let scrolled = ui.window().take_snapshot().unwrap();

    assert_ne!(
        unscrolled.as_bytes(),
        scrolled.as_bytes(),
        "the device rack did not scroll, so devices past the window are unreachable"
    );
    write_snapshot(&scrolled, "MOOLOOP_EFFECT_RACK_SCROLLED_SNAPSHOT");
}

#[test]
fn render_poly_source_editor() {
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
    ui.set_selected_channel_name(SharedString::from("Poly"));
    ui.set_editor_page(0);
    ui.set_source_kind(3);

    let poly = ui.window().take_snapshot().unwrap();
    assert_eq!((poly.width(), poly.height()), (960, 760));
    assert!(poly.as_bytes().iter().any(|byte| *byte != 0));
    write_snapshot(&poly, "MOOLOOP_POLY_SOURCE_SNAPSHOT");

    ui.set_poly_osc2_level(0.6);
    ui.set_poly_osc2_semitones(7.0);
    let layered = ui.window().take_snapshot().unwrap();
    assert_ne!(poly.as_bytes(), layered.as_bytes());
    write_snapshot(&layered, "MOOLOOP_POLY_LAYERED_SOURCE_SNAPSHOT");

    ui.set_poly_device_page(1);
    let poly_amp = ui.window().take_snapshot().unwrap();
    assert_ne!(poly.as_bytes(), poly_amp.as_bytes());
    write_snapshot(&poly_amp, "MOOLOOP_POLY_AMP_SOURCE_SNAPSHOT");

    ui.set_poly_device_page(2);
    let poly_mod = ui.window().take_snapshot().unwrap();
    assert_ne!(poly_amp.as_bytes(), poly_mod.as_bytes());
    write_snapshot(&poly_mod, "MOOLOOP_POLY_MOD_SOURCE_SNAPSHOT");

    ui.set_poly_device_page(3);
    let poly_voice = ui.window().take_snapshot().unwrap();
    assert_ne!(poly_mod.as_bytes(), poly_voice.as_bytes());
    write_snapshot(&poly_voice, "MOOLOOP_POLY_VOICE_SOURCE_SNAPSHOT");
}
