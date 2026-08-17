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

/// Env-gated diagnostic logging (MOOLOOP_DEBUG=1).
fn dbg_log(msg: &str) {
    if std::env::var("MOOLOOP_DEBUG").is_ok() {
        eprintln!("mooloop: {msg}");
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
                dbg_log("UI: play clicked, queuing Play");
                let _ = tx.send(EngineCommand::Play);
            });
        }
        {
            let tx = cmd_tx.clone();
            window.on_stop_clicked(move || {
                dbg_log("UI: stop clicked, queuing Stop");
                let _ = tx.send(EngineCommand::Stop);
            });
        }
        {
            let tx = cmd_tx.clone();
            window.on_bpm_changed(move |bpm| {
                let _ = tx.send(EngineCommand::SetTempo(bpm as f64));
            });
        }
        {
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_toggle_play(move || {
                let playing = weak.upgrade().map(|w| w.get_playing()).unwrap_or(false);
                dbg_log(if playing { "UI: toggle-play -> Pause" } else { "UI: toggle-play -> Play" });
                let _ = tx.send(if playing {
                    EngineCommand::Pause
                } else {
                    EngineCommand::Play
                });
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
                dbg_log(&format!("UI: step {i} clicked, toggling"));
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
        // Diagnostics shared with the autodrive self-test (MOOLOOP_AUTODRIVE=1).
        let stats = Rc::new(std::cell::Cell::new((0.0f32, false, 0usize)));
        let stats_in = stats.clone();
        pump.start(
            TimerMode::Repeated,
            std::time::Duration::from_millis(PUMP_INTERVAL_MS),
            move || {
                let mut forwarded = 0usize;
                while let Ok(cmd) = cmd_rx.try_recv() {
                    handle.send(cmd);
                    forwarded += 1;
                }
                let Some(w) = weak.upgrade() else { return };
                let mut saw_nonzero = false;
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
                            let p = peak_l.clamp(0.0, 1.0);
                            if p > 0.0 {
                                saw_nonzero = true;
                            }
                            w.set_peak_l(p);
                            w.set_peak_r(peak_r.clamp(0.0, 1.0));
                        }
                        EngineEvent::Xrun => {}
                    }
                }
                let (mp, sp, cf) = stats_in.get();
                let new_mp = if saw_nonzero { mp.max(1.0) } else { mp };
                let new_sp = sp || w.get_playing();
                stats_in.set((new_mp, new_sp, cf + forwarded));
            },
        );

        // --- Optional autodrive self-test (MOOLOOP_AUTODRIVE=1) ---
        // Drives the actual Slint callbacks (as if the user clicked), then
        // exits with a report. Lets the full GUI build be tested headlessly.
        if std::env::var("MOOLOOP_AUTODRIVE").is_ok() {
            let weak = window.as_weak();
            slint::Timer::single_shot(std::time::Duration::from_millis(300), move || {
                let Some(w) = weak.upgrade() else { return };
                for step in [0, 4, 8, 12] {
                    w.invoke_step_clicked(step);
                }
                w.invoke_play_clicked();
            });
            let stats = stats.clone();
            slint::Timer::single_shot(std::time::Duration::from_millis(4500), move || {
                let (max_peak, saw_playing, forwarded) = stats.get();
                println!("--- ui autodrive report ---");
                println!("commands forwarded by pump : {forwarded}");
                println!("saw playing=true on window : {saw_playing}");
                println!("nonzero metering seen     : {max_peak:.4}");
                let ok = saw_playing && forwarded >= 5;
                println!(
                    "RESULT: {}",
                    if ok { "PASS — UI wiring delivers commands/events" } else { "FAIL" }
                );
                slint::quit_event_loop().ok();
            });
        }

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

/// Decode a WAV/RIFF file into stereo f32 frames. hound's `samples::<f32>()`
/// only works for IEEE-float files, so integer formats are read at their
/// native width and normalised to [-1, 1] here. Errors propagate loudly —
/// never silently drop samples (an empty buffer would silently mute the
/// sampler).
fn decode_wav(path: &str) -> Result<Arc<SampleData>, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let sample_rate = spec.sample_rate;
    let channels = spec.channels.max(1) as usize;

    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("sample decode failed: {e}"))?,
        (hound::SampleFormat::Int, 8) => reader
            .samples::<i8>()
            .map(|s| s.map(|v| f32::from(v) / 128.0))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("sample decode failed: {e}"))?,
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|s| s.map(|v| f32::from(v) / 32_768.0))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("sample decode failed: {e}"))?,
        (hound::SampleFormat::Int, 24) => reader
            .samples::<i32>()
            .map(|s| s.map(|v| v as f32 / 8_388_608.0))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("sample decode failed: {e}"))?,
        (hound::SampleFormat::Int, 32) => reader
            .samples::<i32>()
            .map(|s| s.map(|v| v as f32 / 2_147_483_648.0))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("sample decode failed: {e}"))?,
        (fmt, bits) => {
            return Err(format!("unsupported WAV format ({fmt:?}, {bits}-bit)"));
        }
    };

    if samples.is_empty() {
        return Err("file contained no samples".into());
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test: hound's `samples::<f32>()` errors on integer-PCM
    /// files; the decoder must decode them at native width instead (and
    /// never silently return an empty buffer).
    #[test]
    fn decodes_16bit_stereo_wav() {
        let path = std::env::temp_dir().join("mooloop_decode_test_16bit.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..1000i32 {
            let v = ((i % 100) * 300 - 15_000) as i16;
            writer.write_sample(v).unwrap();
            writer.write_sample(v).unwrap();
        }
        writer.finalize().unwrap();

        let data = decode_wav(path.to_str().unwrap()).unwrap();
        assert_eq!(data.sample_rate, 44_100);
        assert_eq!(data.len(), 1000);
        assert!(data.frames.iter().any(|f| f[0] != 0.0));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_garbage_file() {
        let path = std::env::temp_dir().join("mooloop_decode_test_garbage.wav");
        std::fs::write(&path, b"not a wav at all").unwrap();
        assert!(decode_wav(path.to_str().unwrap()).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
