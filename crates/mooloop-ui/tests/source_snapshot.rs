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
fn render_mlm1_source_editor() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(SharedString::from("software")),
        },
    )))
    .ok();

    let ui = MainWindow::new().unwrap();
    ui.window().set_size(LogicalSize::new(960.0, 760.0));
    ui.set_channels(rack_rows());
    ui.set_pattern_length(16);
    ui.set_selected_channel_name(SharedString::from("ML-M1"));
    ui.set_editor_page(0);
    ui.set_source_kind(4);

    let osc = ui.window().take_snapshot().unwrap();
    assert_eq!((osc.width(), osc.height()), (960, 760));
    assert!(osc.as_bytes().iter().any(|byte| *byte != 0));
    write_snapshot(&osc, "MOOLOOP_MLM1_OSC_SOURCE_SNAPSHOT");

    // AMP/FILTER carries two envelope editors, which is the layout call the
    // whole face is built around; moving the filter envelope has to redraw.
    ui.set_mlm1_device_page(1);
    ui.set_mlm1_filter_cutoff(0.35);
    ui.set_mlm1_filter_resonance(0.8);
    ui.set_mlm1_filter_env(0.6);
    let amp_filter = ui.window().take_snapshot().unwrap();
    assert_ne!(osc.as_bytes(), amp_filter.as_bytes());
    write_snapshot(&amp_filter, "MOOLOOP_MLM1_AMP_SOURCE_SNAPSHOT");

    ui.set_mlm1_filter_decay(0.05);
    ui.set_mlm1_filter_sustain(0.0);
    let plucked = ui.window().take_snapshot().unwrap();
    assert_ne!(
        amp_filter.as_bytes(),
        plucked.as_bytes(),
        "the filter envelope editor should redraw independently of the amp one"
    );
    write_snapshot(&plucked, "MOOLOOP_MLM1_PLUCK_SOURCE_SNAPSHOT");

    // Each model draws its own slope, so switching has to redraw the curve.
    for model in [1, 2] {
        ui.set_mlm1_filter_model(model);
        let switched = ui.window().take_snapshot().unwrap();
        assert_ne!(
            plucked.as_bytes(),
            switched.as_bytes(),
            "filter model {model} drew the same response curve as the ladder"
        );
        if model == 1 {
            write_snapshot(&switched, "MOOLOOP_MLM1_ACID_SOURCE_SNAPSHOT");
        }
    }
    ui.set_mlm1_filter_model(0);

    ui.set_mlm1_device_page(2);
    let perf = ui.window().take_snapshot().unwrap();
    assert_ne!(plucked.as_bytes(), perf.as_bytes());
    write_snapshot(&perf, "MOOLOOP_MLM1_PERF_SOURCE_SNAPSHOT");

    ui.set_mlm1_priority(2);
    let high_priority = ui.window().take_snapshot().unwrap();
    assert_ne!(perf.as_bytes(), high_priority.as_bytes());

    // Accent shares the column with Glide, so this also proves the two knobs
    // are laid out as two knobs rather than drawn over each other.
    ui.set_mlm1_accent(1.0);
    let accented = ui.window().take_snapshot().unwrap();
    assert_ne!(high_priority.as_bytes(), accented.as_bytes());
    write_snapshot(&accented, "MOOLOOP_MLM1_ACCENT_SOURCE_SNAPSHOT");

    ui.window().set_size(LogicalSize::new(720.0, 760.0));
    let narrow = ui.window().take_snapshot().unwrap();
    assert_eq!((narrow.width(), narrow.height()), (720, 760));
    assert!(narrow.as_bytes().iter().any(|byte| *byte != 0));
    write_snapshot(&narrow, "MOOLOOP_MLM1_PERF_NARROW_SNAPSHOT");
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
    ui.set_modulation_selected_slot(1);
    ui.set_modulation_armed_slot(1);
    ui.set_modulation_sources(ModelRc::from(Rc::new(VecModel::from(vec![
        ModulationSourceRow {
            slot: 0,
            name: SharedString::from("LFO 1"),
            kind: 0,
            waveform: 3,
            rate: 2.0,
            depth: 1.0,
            phase: 0.0,
            pulse_width: 0.3,
            preview_fade_cycles: 0.5,
            preview_smoothing_cycles: 0.16,
            preview_attack: 0.0,
            preview_decay: 0.0,
            preview_sustain: 0.0,
            preview_release: 0.0,
            steps: Vec::<f32>::new().as_slice().into(),
            step_length: 16,
            math_op: 0,
            output: -0.42,
            retrigger: false,
            selected: false,
        },
        ModulationSourceRow {
            slot: 1,
            name: SharedString::from("ENV 2"),
            kind: 1,
            waveform: 0,
            rate: 0.0,
            depth: 1.0,
            phase: 0.0,
            pulse_width: 0.5,
            preview_fade_cycles: 0.0,
            preview_smoothing_cycles: 0.0,
            preview_attack: 0.015,
            preview_decay: 0.25,
            preview_sustain: 0.62,
            preview_release: 0.38,
            steps: Vec::<f32>::new().as_slice().into(),
            step_length: 16,
            math_op: 0,
            output: 0.68,
            retrigger: true,
            selected: true,
        },
        // A step tile draws its own pattern and a math tile its operator, so
        // both faces are in the shot alongside the curve-drawing kinds.
        ModulationSourceRow {
            slot: 2,
            name: SharedString::from("STEP 3"),
            kind: 2,
            steps: vec![
                0.0f32, 0.4, 0.8, 0.35, -0.2, -0.75, 0.15, 0.6, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
                0.0,
            ]
            .as_slice()
            .into(),
            step_length: 8,
            output: 0.8,
            depth: 1.0,
            pulse_width: 0.5,
            preview_sustain: 0.7,
            ..Default::default()
        },
        ModulationSourceRow {
            slot: 3,
            name: SharedString::from("MATH 4"),
            kind: 4,
            math_op: 2,
            steps: Vec::<f32>::new().as_slice().into(),
            step_length: 16,
            output: -0.25,
            depth: 1.0,
            pulse_width: 0.5,
            preview_sustain: 0.7,
            ..Default::default()
        },
    ]))));
    ui.set_modulation_routes(ModelRc::from(Rc::new(VecModel::from(vec![
        ModulationRouteRow {
            route_index: 0,
            source_slot: 1,
            owner: -1,
            param: 12,
            destination: SharedString::from("ENV 2 → Kick · Cutoff"),
            depth: 0.35,
            polarity: 1,
            allowed: true,
        },
    ]))));
    ui.set_modulation_max_sources(8);
    ui.set_modulation_slot_names(slot_names(8));
    ui.set_modulation_selected_kind(1);
    ui.set_modulation_input_channels(
        vec![SharedString::from("1 · Kick"), SharedString::from("2 · Snare")]
            .as_slice()
            .into(),
    );
    ui.set_modulation_selected_envelope_input_channel(0);
    // Descriptor-id indexed (ENV_PARAM_*): attack, attack sync, attack
    // division, decay, decay sync, decay division, sustain, release, release
    // sync, release division, amount.
    ui.set_modulation_selected_values(
        vec![
            0.015f32, 0.0, 13.0, 0.18, 1.0, 10.0, 0.62, 0.38, 0.0, 7.0, 1.0,
        ]
        .as_slice()
        .into(),
    );
    ui.set_modulation_selected_envelope_preview_attack(0.015);
    ui.set_modulation_selected_envelope_preview_decay(0.25);
    ui.set_modulation_selected_envelope_preview_release(0.38);
    ui.set_modulation_selected_preview_fade_cycles(0.5);
    ui.set_modulation_selected_preview_smoothing_cycles(0.16);
    // Descriptor-id indexed, so the sampler's cutoff overlay sits at 12.
    let mut source_depths = vec![0.0f32; 22];
    source_depths[12] = 0.35;
    ui.set_source_modulation_depths(source_depths.as_slice().into());
    ui.set_source_modulation_allowed(vec![true; 22].as_slice().into());
    // Cutoff carries two routes, resonance one, so the dot row is exercised
    // at more than a single dot.
    let mut source_route_counts = vec![0i32; 22];
    source_route_counts[12] = 2;
    source_route_counts[13] = 1;
    ui.set_source_modulation_route_counts(source_route_counts.as_slice().into());
    let modulation = ui.window().take_snapshot().unwrap();
    assert_eq!((modulation.width(), modulation.height()), (1440, 900));
    assert_ne!(snapshot.as_bytes(), modulation.as_bytes());
    write_snapshot(&modulation, "MOOLOOP_MODULATION_SHELF_SNAPSHOT");

    // The source faces are parameter readouts, not generic type icons. A
    // zero-time attack has a vertical leading edge, while a long attack
    // visibly slopes toward the peak.
    ui.set_modulation_selected_envelope_preview_attack(0.0);
    let instant_attack = ui.window().take_snapshot().unwrap();
    ui.set_modulation_selected_envelope_preview_attack(16.0);
    let long_attack = ui.window().take_snapshot().unwrap();
    assert_ne!(instant_attack.as_bytes(), long_attack.as_bytes());
    write_snapshot(&instant_attack, "MOOLOOP_ENVELOPE_INSTANT_ATTACK_SNAPSHOT");
    write_snapshot(&long_attack, "MOOLOOP_ENVELOPE_LONG_ATTACK_SNAPSHOT");

    // The LFO face likewise follows waveform, phase, amount, fade, smoothing,
    // and pulse width instead of retaining the last generic oscillator glyph.
    ui.set_modulation_selected_slot(0);
    ui.set_modulation_selected_kind(0);
    // Descriptor-id indexed (LFO_PARAM_*): rate, depth, waveform, phase,
    // tempo sync, rate division, retrigger, fade in, fade sync, fade
    // division, smoothing, pulse width.
    ui.set_modulation_selected_values(
        vec![
            2.0f32, 1.0, 2.0, 0.0, 1.0, 13.0, 0.0, 0.75, 0.0, 7.0, 0.08, 0.3,
        ]
        .as_slice()
        .into(),
    );
    ui.set_modulation_selected_preview_fade_cycles(0.0);
    ui.set_modulation_selected_preview_smoothing_cycles(0.0);
    let saw_face = ui.window().take_snapshot().unwrap();
    ui.set_modulation_selected_values(
        vec![
            2.0f32, 0.55, 3.0, 0.2, 1.0, 13.0, 0.0, 0.75, 0.0, 7.0, 0.08, 0.2,
        ]
        .as_slice()
        .into(),
    );
    ui.set_modulation_selected_preview_fade_cycles(0.75);
    ui.set_modulation_selected_preview_smoothing_cycles(0.2);
    let shaped_lfo = ui.window().take_snapshot().unwrap();
    assert_ne!(saw_face.as_bytes(), shaped_lfo.as_bytes());
    write_snapshot(&shaped_lfo, "MOOLOOP_LFO_SHAPED_FACE_SNAPSHOT");

    ui.set_modulation_selected_slot(1);
    ui.set_modulation_selected_kind(1);
    ui.set_modulation_selected_values(
        vec![
            0.015f32, 0.0, 13.0, 0.18, 1.0, 10.0, 0.62, 0.38, 0.0, 7.0, 1.0,
        ]
        .as_slice()
        .into(),
    );
    ui.set_modulation_selected_envelope_preview_attack(0.015);

    // The three module kinds added in step 02 each render their own editor
    // through the same descriptor-indexed surface: a step bank, a random
    // panel with its lamps, and a math module's operator and formula.
    ui.set_modulation_selected_slot(2);
    ui.set_modulation_selected_kind(2);
    // Descriptor-id indexed (STEP_PARAM_*): length, division, glide,
    // trigger, then the sixteen contiguous step values.
    ui.set_modulation_selected_values(
        vec![
            8.0f32, 13.0, 0.25, 0.0, 0.0, 0.4, 0.8, 0.35, -0.2, -0.75, 0.15, 0.6, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0,
        ]
        .as_slice()
        .into(),
    );
    let step_editor = ui.window().take_snapshot().unwrap();
    assert_ne!(modulation.as_bytes(), step_editor.as_bytes());
    write_snapshot(&step_editor, "MOOLOOP_STEP_MODULE_SNAPSHOT");

    ui.set_modulation_selected_kind(3);
    // Descriptor-id indexed (RANDOM_PARAM_*): rate, sync, division, trigger,
    // bipolar, chance, quantize, drunk, walk.
    ui.set_modulation_selected_values(
        vec![4.0f32, 0.0, 13.0, 0.0, 1.0, 0.65, 5.0, 1.0, 0.3]
            .as_slice()
            .into(),
    );
    let random_editor = ui.window().take_snapshot().unwrap();
    assert_ne!(step_editor.as_bytes(), random_editor.as_bytes());
    write_snapshot(&random_editor, "MOOLOOP_RANDOM_MODULE_SNAPSHOT");

    ui.set_modulation_selected_slot(3);
    ui.set_modulation_selected_kind(4);
    // Descriptor-id indexed (MATH_PARAM_*): input slot, operator, operand,
    // clamp low, clamp high. Reading slot 1 from slot 4 is a same-tick read.
    ui.set_modulation_selected_values(
        vec![0.0f32, 2.0, 1.75, -1.0, 1.0].as_slice().into(),
    );
    let math_editor = ui.window().take_snapshot().unwrap();
    assert_ne!(random_editor.as_bytes(), math_editor.as_bytes());
    write_snapshot(&math_editor, "MOOLOOP_MATH_MODULE_SNAPSHOT");

    // Back to the envelope, which is what the remaining shots are about.
    ui.set_modulation_selected_slot(1);
    ui.set_modulation_selected_kind(1);
    ui.set_modulation_selected_values(
        vec![
            0.015f32, 0.0, 13.0, 0.18, 1.0, 10.0, 0.62, 0.38, 0.0, 7.0, 1.0,
        ]
        .as_slice()
        .into(),
    );

    // Out of assign mode the same knobs must read differently: the value arc
    // returns, the dots appear, and a live offset displaces the arc's end.
    ui.set_modulation_armed_slot(-1);
    let mut source_offsets = vec![0.0f32; 22];
    source_offsets[12] = 0.18;
    source_offsets[13] = -0.12;
    ui.set_source_modulation_offsets(source_offsets.as_slice().into());
    let live = ui.window().take_snapshot().unwrap();
    assert_ne!(modulation.as_bytes(), live.as_bytes());
    write_snapshot(&live, "MOOLOOP_MODULATION_LIVE_SNAPSHOT");
    ui.set_modulation_armed_slot(1);

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

/// One entry per slot, named by what occupies it, as the shelf builds them.
fn slot_names(count: usize) -> ModelRc<SharedString> {
    (0..count)
        .map(|slot| {
            SharedString::from(match slot {
                0 => "1 · LFO 1".to_string(),
                1 => "2 · ENV 2".to_string(),
                2 => "3 · STEP 3".to_string(),
                3 => "4 · MATH 4".to_string(),
                other => format!("{} · empty", other + 1),
            })
        })
        .collect::<Vec<_>>()
        .as_slice()
        .into()
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
        modulation_offsets: Vec::<f32>::new().as_slice().into(),
        modulation_route_counts: Vec::<i32>::new().as_slice().into(),
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
        detector_db: -60.0,
        gain_reduction_db: 0.0,
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

/// Markers at a zoomed scale, where snapping is actually judged.
///
/// A snapped marker only moves by frames, which is sub-pixel at full zoom —
/// the whole point of the waveform editor's zoom is that the resolved position
/// becomes visible. This pins that the four markers, their frame fields, and
/// the snap controls still compose once the view is windowed, which the
/// fully-zoomed-out shot above cannot show.
#[test]
fn render_sampler_zoomed_markers() {
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
    ui.set_sampler_device_page(0);
    ui.set_sample_name(SharedString::from("amen_break.wav"));
    ui.set_sample_description(SharedString::from("48kHz / 16-bit / stereo"));
    ui.set_sample_frames(480_000);
    ui.set_tune_label(SharedString::from("C4 · 261.6 Hz"));
    ui.set_waveform(ModelRc::from(Rc::new(VecModel::from(vec![
        0.1, 0.8, 0.3, 0.2, 0.9, 0.4, 0.15, 0.6, 0.35, 0.75, 0.2, 0.5,
    ]))));

    // A windowed view over the middle of the sample, with all four markers
    // inside it and a loop region narrower than the play region, so start/end
    // and loop start/end are separately visible rather than coincident.
    ui.set_waveform_view_offset(0.25);
    ui.set_waveform_view_visible_fraction(0.5);
    ui.set_start_pos(0.30);
    ui.set_end_pos(0.70);
    ui.set_loop_start(0.40);
    ui.set_loop_end(0.60);
    ui.set_loop_mode(1);
    ui.set_snap_to_zero(true);

    let snapshot = ui.window().take_snapshot().unwrap();
    assert_eq!((snapshot.width(), snapshot.height()), (960, 760));
    assert!(snapshot.as_bytes().iter().any(|byte| *byte != 0));
    write_snapshot(&snapshot, "MOOLOOP_SAMPLER_ZOOMED_MARKERS_SNAPSHOT");
}

/// Capacity is a constant, not a layout decision. The same shelf, told it
/// has sixteen slots instead of eight, must still show every module cell and
/// still pick an input by name — with no edit anywhere in the UI. This is the
/// test that fails if a literal row count or a per-slot segment creeps back
/// in (`docs/plans/modulator-capacity/01-capacity-is-a-constant.md`).
#[test]
fn the_module_grid_scales_with_capacity_alone() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(SharedString::from("software")),
        },
    )))
    .expect("initialize headless renderer");

    let ui = MainWindow::new().unwrap();
    ui.window().set_size(LogicalSize::new(1440.0, 900.0));
    ui.set_channels(rack_rows());
    ui.set_editor_page(0);
    ui.set_modulation_shelf_open(true);
    ui.set_modulation_selected_slot(3);
    ui.set_modulation_selected_kind(4);
    ui.set_modulation_selected_values(vec![0.0f32, 2.0, 1.75, -1.0, 1.0].as_slice().into());
    // Sixteen modules, so the grid needs four rows where two fit.
    let sources: Vec<ModulationSourceRow> = (0..16)
        .map(|slot| ModulationSourceRow {
            slot,
            name: SharedString::from(format!("LFO {}", slot + 1)),
            kind: 0,
            waveform: slot % 5,
            depth: 1.0,
            pulse_width: 0.5,
            preview_sustain: 0.7,
            output: (slot as f32 / 8.0) - 1.0,
            steps: Vec::<f32>::new().as_slice().into(),
            step_length: 16,
            selected: slot == 3,
            ..Default::default()
        })
        .collect();
    ui.set_modulation_sources(ModelRc::from(Rc::new(VecModel::from(sources))));

    for capacity in [8usize, 16] {
        ui.set_modulation_max_sources(capacity as i32);
        ui.set_modulation_slot_names(slot_names(capacity));
        let shot = ui.window().take_snapshot().unwrap();
        assert_eq!((shot.width(), shot.height()), (1440, 900));
        assert!(shot.as_bytes().iter().any(|byte| *byte != 0));
        write_snapshot(
            &shot,
            if capacity == 8 {
                "MOOLOOP_CAPACITY_EIGHT_SNAPSHOT"
            } else {
                "MOOLOOP_CAPACITY_SIXTEEN_SNAPSHOT"
            },
        );
    }
}

#[test]
fn render_mlp8_source_editor() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(SharedString::from("software")),
        },
    )))
    .ok();

    let ui = MainWindow::new().unwrap();
    ui.window().set_size(LogicalSize::new(960.0, 760.0));
    ui.set_channels(rack_rows());
    ui.set_pattern_length(16);
    ui.set_selected_channel_name(SharedString::from("ML-P8"));
    ui.set_editor_page(0);
    ui.set_source_kind(5);

    let osc = ui.window().take_snapshot().unwrap();
    assert_eq!((osc.width(), osc.height()), (960, 760));
    assert!(osc.as_bytes().iter().any(|byte| *byte != 0));
    write_snapshot(&osc, "MOOLOOP_MLP8_OSC_SOURCE_SNAPSHOT");

    // The sources row is under the three oscillator strips, so it is the part
    // most likely to be laid out off the bottom of the face.
    ui.set_mlp8_sub_level(0.7);
    ui.set_mlp8_noise_level(0.4);
    ui.set_mlp8_noise_color(-70.0);
    let sources = ui.window().take_snapshot().unwrap();
    assert_ne!(osc.as_bytes(), sources.as_bytes());
    write_snapshot(&sources, "MOOLOOP_MLP8_SOURCES_SNAPSHOT");

    // ROUTE is the page the device exists for: twelve bipolar amounts, three
    // sync selectors, and none of them drawn over each other.
    ui.set_mlp8_device_page(1);
    let route = ui.window().take_snapshot().unwrap();
    assert_ne!(sources.as_bytes(), route.as_bytes());
    write_snapshot(&route, "MOOLOOP_MLP8_ROUTE_SOURCE_SNAPSHOT");

    ui.set_mlp8_xmod12(80.0);
    ui.set_mlp8_xmod21(-45.0);
    ui.set_mlp8_feedback1(60.0);
    ui.set_mlp8_noise_osc3(-30.0);
    let wired = ui.window().take_snapshot().unwrap();
    assert_ne!(route.as_bytes(), wired.as_bytes());
    write_snapshot(&wired, "MOOLOOP_MLP8_WIRED_SOURCE_SNAPSHOT");

    // Oscillator 1's list is OFF/2/3, so index 2 is oscillator 3 -- the
    // selector has to skip the oscillator it belongs to.
    ui.set_mlp8_sync1(3);
    let synced = ui.window().take_snapshot().unwrap();
    assert_ne!(wired.as_bytes(), synced.as_bytes());
    write_snapshot(&synced, "MOOLOOP_MLP8_SYNC_SOURCE_SNAPSHOT");

    ui.set_mlp8_device_page(2);
    let amp = ui.window().take_snapshot().unwrap();
    assert_ne!(synced.as_bytes(), amp.as_bytes());
    write_snapshot(&amp, "MOOLOOP_MLP8_AMP_SOURCE_SNAPSHOT");

    ui.window().set_size(LogicalSize::new(720.0, 760.0));
    let narrow = ui.window().take_snapshot().unwrap();
    assert_eq!((narrow.width(), narrow.height()), (720, 760));
    assert!(narrow.as_bytes().iter().any(|byte| *byte != 0));
    write_snapshot(&narrow, "MOOLOOP_MLP8_AMP_NARROW_SNAPSHOT");
}
