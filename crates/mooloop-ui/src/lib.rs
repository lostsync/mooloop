//! Slint UI wrapper. Owns the `EngineHandle`, wires Slint callbacks to engine
//! commands, and runs a high-frequency timer that forwards commands and drains
//! audio events onto window properties.

slint::include_modules!();

use mooloop_core::{EngineCommand, EngineEvent, LoopMode, SamplerParams, DEFAULT_STEPS};
use mooloop_dsp::SampleData;
use mooloop_engine::EngineHandle;
use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

const PUMP_INTERVAL_MS: u64 = 8;
const INITIAL_BPM: i32 = 120;
/// Fader positions for time-based params map onto [0, MAX_TIME_S] seconds.
const MAX_TIME_S: f32 = 2.0;
const NUM_STEPS: usize = DEFAULT_STEPS as usize;

pub struct AppUi {
    window: MainWindow,
    _pump: Timer,
}

fn norm_to_time(v: f32) -> f32 {
    v * MAX_TIME_S
}
fn time_to_norm(t: f32) -> f32 {
    t / MAX_TIME_S
}

fn loop_mode_from_int(i: i32) -> LoopMode {
    match i {
        1 => LoopMode::Forward,
        2 => LoopMode::Pingpong,
        _ => LoopMode::Off,
    }
}

impl AppUi {
    pub fn new(mut handle: EngineHandle) -> Result<Self, slint::PlatformError> {
        let window = MainWindow::new()?;

        // --- Transport initial state ---
        window.set_bpm(INITIAL_BPM);
        window.set_playing(false);
        window.set_beat_in_bar(0);
        window.set_peak_l(0.0);
        window.set_peak_r(0.0);
        handle.send(EngineCommand::SetTempo(INITIAL_BPM as f64));

        // --- Step grid model (UI source of truth) ---
        let steps_model: Rc<VecModel<StepCell>> =
            Rc::new(VecModel::from(vec![StepCell { active: false }; NUM_STEPS]));
        window.set_steps(ModelRc::from(steps_model.clone()));

        // --- Sampler params (UI source of truth, mirrored to engine) ---
        let params = Rc::new(RefCell::new(SamplerParams::default()));
        {
            let p = params.borrow();
            window.set_attack(time_to_norm(p.attack));
            window.set_decay(time_to_norm(p.decay));
            window.set_sustain(p.sustain);
            window.set_release(time_to_norm(p.release));
            window.set_start_pos(p.start);
            window.set_loop_start(p.loop_start);
            window.set_loop_end(p.loop_end);
            window.set_loop_mode(0);
            window.set_sample_name("default kick".into());
        }
        handle.send(EngineCommand::SetSamplerParams(*params.borrow()));

        // --- Command channel from UI closures to the pump ---
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<EngineCommand>();

        // Transport callbacks.
        {
            let tx = cmd_tx.clone();
            window.on_play_clicked(move || {
                let _ = tx.send(EngineCommand::Play);
            });
        }
        {
            let tx = cmd_tx.clone();
            window.on_stop_clicked(move || {
                let _ = tx.send(EngineCommand::Stop);
            });
        }
        {
            let tx = cmd_tx.clone();
            window.on_bpm_changed(move |bpm| {
                let _ = tx.send(EngineCommand::SetTempo(bpm as f64));
            });
        }

        // Step toggle.
        {
            let tx = cmd_tx.clone();
            let model = steps_model.clone();
            window.on_step_clicked(move |i| {
                let i = i as usize;
                if i >= NUM_STEPS {
                    return;
                }
                let cur = model.row_data(i).map(|c| c.active).unwrap_or(false);
                let new_active = !cur;
                model.set_row_data(i, StepCell { active: new_active });
                let _ = tx.send(EngineCommand::SetStep {
                    channel: 0,
                    step: i as u8,
                    on: new_active,
                    velocity: 100,
                });
            });
        }

        // --- Sampler parameter callbacks ---
        macro_rules! wire_time_param {
            ($on:ident, $field:ident) => {{
                let tx = cmd_tx.clone();
                let params = params.clone();
                window.$on(move |v: f32| {
                    params.borrow_mut().$field = norm_to_time(v);
                    let p = *params.borrow();
                    let _ = tx.send(EngineCommand::SetSamplerParams(p));
                });
            }};
        }
        macro_rules! wire_unit_param {
            ($on:ident, $field:ident) => {{
                let tx = cmd_tx.clone();
                let params = params.clone();
                window.$on(move |v: f32| {
                    params.borrow_mut().$field = v;
                    let p = *params.borrow();
                    let _ = tx.send(EngineCommand::SetSamplerParams(p));
                });
            }};
        }

        wire_time_param!(on_attack_changed, attack);
        wire_time_param!(on_decay_changed, decay);
        wire_unit_param!(on_sustain_changed, sustain);
        wire_time_param!(on_release_changed, release);
        wire_unit_param!(on_start_pos_changed, start);
        wire_unit_param!(on_loop_start_changed, loop_start);
        wire_unit_param!(on_loop_end_changed, loop_end);

        {
            let tx = cmd_tx.clone();
            let params = params.clone();
            window.on_loop_mode_changed(move |i| {
                params.borrow_mut().loop_mode = loop_mode_from_int(i);
                let p = *params.borrow();
                let _ = tx.send(EngineCommand::SetSamplerParams(p));
            });
        }

        // --- Sample loading via zenity + hound ---
        let sample_slot = handle.sample_slot();
        {
            let weak = window.as_weak();
            window.on_load_sample_clicked(move || {
                let Some(path) = pick_wav_via_zenity() else { return };
                match decode_wav(&path) {
                    Ok(sample) => {
                        sample_slot.store(Some(sample));
                        if let Some(w) = weak.upgrade() {
                            let name = std::path::Path::new(&path)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("loaded")
                                .to_string();
                            w.set_sample_name(name.into());
                        }
                    }
                    Err(e) => {
                        eprintln!("mooloop: failed to load sample {path}: {e}");
                    }
                }
            });
        }

        // --- Pump: forward queued commands, drain audio events onto window ---
        let weak = window.as_weak();
        let pump = Timer::default();
        pump.start(
            TimerMode::Repeated,
            std::time::Duration::from_millis(PUMP_INTERVAL_MS),
            move || {
                while let Ok(cmd) = cmd_rx.try_recv() {
                    handle.send(cmd);
                }
                let Some(w) = weak.upgrade() else { return };
                for ev in handle.drain() {
                    match ev {
                        EngineEvent::Position {
                            beat_in_bar,
                            playing,
                            ..
                        } => {
                            w.set_beat_in_bar(beat_in_bar as i32);
                            w.set_playing(playing);
                        }
                        EngineEvent::Metering { peak_l, peak_r } => {
                            w.set_peak_l(peak_l.clamp(0.0, 1.0));
                            w.set_peak_r(peak_r.clamp(0.0, 1.0));
                        }
                        EngineEvent::Xrun => {}
                    }
                }
            },
        );

        Ok(AppUi {
            window,
            _pump: pump,
        })
    }

    pub fn show(&self) -> Result<(), slint::PlatformError> {
        self.window.show()
    }

    pub fn run(&self) -> Result<(), slint::PlatformError> {
        self.window.run()
    }
}

/// Spawn zenity to pick a WAV file. Returns `None` if cancelled or unavailable.
fn pick_wav_via_zenity() -> Option<String> {
    let out = std::process::Command::new("zenity")
        .arg("--file-selection")
        .arg("--file-filter=*.wav")
        .arg("--file-filter=*.WAV")
        .arg("--title=Load sample")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

/// Decode a WAV/RIFF file into stereo f32 frames. hound handles common PCM
/// bit depths and IEEE float, normalising integers to [-1, 1].
fn decode_wav(path: &str) -> Result<Arc<SampleData>, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let sample_rate = spec.sample_rate;
    let channels = spec.channels.max(1) as usize;

    let samples: Vec<f32> = reader
        .samples::<f32>()
        .filter_map(Result::ok)
        .collect();

    let frames: Vec<[f32; 2]> = samples
        .chunks(channels)
        .map(|ch| {
            let l = ch.first().copied().unwrap_or(0.0);
            let r = ch.get(1).copied().unwrap_or(l);
            [l, r]
        })
        .collect();

    Ok(Arc::new(SampleData {
        frames,
        sample_rate,
        root_note: 60,
    }))
}
