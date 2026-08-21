use mooloop_core::{DrumMode, DrumSynthParams};
use mooloop_dsp::DrumSynth;
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

    let snapshot = ui.window().take_snapshot().unwrap();
    assert_eq!((snapshot.width(), snapshot.height()), (960, 760));
    assert!(snapshot.as_bytes().iter().any(|byte| *byte != 0));
    write_snapshot(&snapshot, "MOOLOOP_SAMPLER_SOURCE_SNAPSHOT");

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
