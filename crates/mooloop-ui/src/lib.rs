//! Slint UI wrapper. Owns the `EngineHandle`, wires Slint callbacks to engine
//! commands, and runs a high-frequency timer that forwards commands and drains
//! audio events onto window properties.
//!
//! The UI owns the project state (channels, pattern bank, per-channel sampler
//! params) as the source of truth and mirrors every mutation to the engine
//! via commands. The engine keeps its own pre-allocated copy.

slint::include_modules!();

use mooloop_core::{EngineCommand, EngineEvent, LoopMode, SamplerParams, MAX_CHANNELS, MAX_PATTERNS};
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
const NUM_STEPS: usize = mooloop_core::DEFAULT_STEPS as usize;

/// UI-side state for one channel. `steps` is the pattern bank:
/// `[pattern][step]`.
struct ChannelState {
    name: String,
    muted: bool,
    params: SamplerParams,
    sample_name: String,
    steps: Vec<Vec<bool>>,
}

impl ChannelState {
    fn new(index: usize) -> Self {
        Self {
            name: format!("Sampler {}", index + 1),
            muted: false,
            params: SamplerParams::default(),
            sample_name: "default kick".into(),
            steps: vec![vec![false; NUM_STEPS]; MAX_PATTERNS],
        }
    }
}

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

/// Shared UI state handed to the callback closures.
struct UiState {
    channels: Vec<ChannelState>,
    rows: Rc<VecModel<ChannelRow>>,
    step_models: Vec<Rc<VecModel<StepCell>>>,
    current_pattern: usize,
    selected: usize,
}

impl UiState {
    /// Push the selected/muted flags of every row to the rack model.
    fn sync_row_flags(&self) {
        for (i, ch) in self.channels.iter().enumerate() {
            if let Some(mut row) = self.rows.row_data(i) {
                row.selected = i == self.selected;
                row.muted = ch.muted;
                row.name = ch.name.as_str().into();
                self.rows.set_row_data(i, row);
            }
        }
    }

    /// Rebuild every channel's step model from `pattern`.
    fn show_pattern(&self, pattern: usize) {
        for (i, ch) in self.channels.iter().enumerate() {
            let cells: Vec<StepCell> = ch.steps[pattern]
                .iter()
                .map(|&active| StepCell { active })
                .collect();
            self.step_models[i].set_vec(cells);
        }
    }

    /// Refresh the bottom editor's properties from `selected`.
    fn refresh_editor(&self, window: &MainWindow) {
        let Some(ch) = self.channels.get(self.selected) else {
            return;
        };
        let p = &ch.params;
        window.set_selected_channel_name(ch.name.as_str().into());
        window.set_sample_name(ch.sample_name.as_str().into());
        window.set_attack(time_to_norm(p.attack));
        window.set_decay(time_to_norm(p.decay));
        window.set_sustain(p.sustain);
        window.set_release(time_to_norm(p.release));
        window.set_start_pos(p.start);
        window.set_loop_start(p.loop_start);
        window.set_loop_end(p.loop_end);
        window.set_loop_mode(match p.loop_mode {
            LoopMode::Off => 0,
            LoopMode::Forward => 1,
            LoopMode::Pingpong => 2,
        });
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
        window.set_current_pattern(0);
        handle.send(EngineCommand::SetTempo(INITIAL_BPM as f64));

        // --- Channel rack state: start with one channel ---
        let first = ChannelState::new(0);
        let first_steps: Vec<StepCell> = first.steps[0]
            .iter()
            .map(|&active| StepCell { active })
            .collect();
        let step_model = Rc::new(VecModel::from(first_steps));
        let row = ChannelRow {
            name: first.name.as_str().into(),
            muted: false,
            selected: true,
            steps: ModelRc::from(step_model.clone()),
        };
        let rows_model = Rc::new(VecModel::from(vec![row]));
        window.set_channels(ModelRc::from(rows_model.clone()));

        let state = Rc::new(RefCell::new(UiState {
            channels: vec![first],
            rows: rows_model,
            step_models: vec![step_model],
            current_pattern: 0,
            selected: 0,
        }));
        state.borrow().refresh_editor(&window);

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
                dbg_log(if playing {
                    "UI: toggle-play -> Pause"
                } else {
                    "UI: toggle-play -> Play"
                });
                let _ = tx.send(if playing {
                    EngineCommand::Pause
                } else {
                    EngineCommand::Play
                });
            });
        }

        // Pattern selection.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_pattern_selected(move |p| {
                let p = p.clamp(0, MAX_PATTERNS as i32 - 1) as usize;
                dbg_log(&format!("UI: pattern {p} selected"));
                {
                    let mut st = st.borrow_mut();
                    st.current_pattern = p;
                    st.show_pattern(p);
                }
                if let Some(w) = weak.upgrade() {
                    w.set_current_pattern(p as i32);
                }
                let _ = tx.send(EngineCommand::SetCurrentPattern(p as u8));
            });
        }

        // Step toggle (channel, step).
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_step_clicked(move |ch, step| {
                let (ch, step) = (ch as usize, step as usize);
                let mut st = st.borrow_mut();
                if ch >= st.channels.len() || step >= NUM_STEPS {
                    return;
                }
                dbg_log(&format!("UI: ch {ch} step {step} toggled"));
                let p = st.current_pattern;
                let new_active = !st.channels[ch].steps[p][step];
                st.channels[ch].steps[p][step] = new_active;
                st.step_models[ch].set_row_data(
                    step,
                    StepCell {
                        active: new_active,
                    },
                );
                let _ = tx.send(EngineCommand::SetStep {
                    pattern: p as u8,
                    channel: ch as u8,
                    step: step as u8,
                    on: new_active,
                    velocity: 100,
                });
            });
        }

        // Channel selection (for the bottom editor).
        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_channel_selected(move |ch| {
                let ch = ch as usize;
                let mut st = st.borrow_mut();
                if ch >= st.channels.len() || ch == st.selected {
                    return;
                }
                st.selected = ch;
                if let Some(w) = weak.upgrade() {
                    w.set_selected_channel(ch as i32);
                    st.sync_row_flags();
                    st.refresh_editor(&w);
                }
            });
        }

        // Channel mute.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_channel_muted(move |ch| {
                let ch = ch as usize;
                let mut st = st.borrow_mut();
                if ch >= st.channels.len() {
                    return;
                }
                st.channels[ch].muted = !st.channels[ch].muted;
                let muted = st.channels[ch].muted;
                st.sync_row_flags();
                let _ = tx.send(EngineCommand::SetChannelMuted {
                    channel: ch as u8,
                    muted,
                });
            });
        }

        // Add / remove channels.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_add_channel_clicked(move || {
                let mut st = st.borrow_mut();
                if st.channels.len() >= MAX_CHANNELS {
                    return;
                }
                dbg_log("UI: add channel");
                let index = st.channels.len();
                let ch = ChannelState::new(index);
                let cells: Vec<StepCell> = ch.steps[st.current_pattern]
                    .iter()
                    .map(|&active| StepCell { active })
                    .collect();
                let model = Rc::new(VecModel::from(cells));
                let row = ChannelRow {
                    name: ch.name.as_str().into(),
                    muted: false,
                    selected: false,
                    steps: ModelRc::from(model.clone()),
                };
                st.rows.push(row);
                st.step_models.push(model);
                st.channels.push(ch);
                let _ = tx.send(EngineCommand::AddChannel);
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_remove_channel_clicked(move || {
                let mut st = st.borrow_mut();
                if st.channels.len() <= 1 {
                    return;
                }
                dbg_log("UI: remove channel");
                st.channels.pop();
                st.step_models.pop();
                st.rows.remove(st.rows.row_count() - 1);
                if st.selected >= st.channels.len() {
                    st.selected = st.channels.len() - 1;
                    if let Some(w) = weak.upgrade() {
                        w.set_selected_channel(st.selected as i32);
                    }
                }
                st.sync_row_flags();
                if let Some(w) = weak.upgrade() {
                    st.refresh_editor(&w);
                }
                let _ = tx.send(EngineCommand::RemoveChannel);
            });
        }

        // --- Sampler parameter callbacks (edit the selected channel) ---
        macro_rules! wire_time_param {
            ($on:ident, $field:ident) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                window.$on(move |v: f32| {
                    let mut st = st.borrow_mut();
                    let ch = st.selected;
                    let Some(channel) = st.channels.get_mut(ch) else {
                        return;
                    };
                    channel.params.$field = norm_to_time(v);
                    let p = channel.params;
                    let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                        channel: ch as u8,
                        params: p,
                    });
                });
            }};
        }
        macro_rules! wire_unit_param {
            ($on:ident, $field:ident) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                window.$on(move |v: f32| {
                    let mut st = st.borrow_mut();
                    let ch = st.selected;
                    let Some(channel) = st.channels.get_mut(ch) else {
                        return;
                    };
                    channel.params.$field = v;
                    let p = channel.params;
                    let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                        channel: ch as u8,
                        params: p,
                    });
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
            let st = state.clone();
            window.on_loop_mode_changed(move |i| {
                let mut st = st.borrow_mut();
                let ch = st.selected;
                let Some(channel) = st.channels.get_mut(ch) else {
                    return;
                };
                channel.params.loop_mode = loop_mode_from_int(i);
                let p = channel.params;
                let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                    channel: ch as u8,
                    params: p,
                });
            });
        }

        // --- Sample loading via zenity + hound (selected channel) ---
        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_load_sample_clicked(move || {
                let selected = st.borrow().selected;
                let Some(path) = pick_wav_via_zenity() else { return };
                match decode_wav(&path) {
                    Ok(sample) => {
                        let name = std::path::Path::new(&path)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("loaded")
                            .to_string();
                        if let Some(ch) = st.borrow_mut().channels.get_mut(selected) {
                            ch.sample_name = name;
                        }
                        if let Some(w) = weak.upgrade() {
                            if selected == st.borrow().selected {
                                st.borrow().refresh_editor(&w);
                            }
                        }
                        LOAD_TARGET.with(|t| *t.borrow_mut() = Some((selected, sample)));
                    }
                    Err(e) => {
                        eprintln!("mooloop: failed to load sample {path}: {e}");
                    }
                }
            });
        }
        // The load callback can't capture the handle (it lives in the pump),
        // so park the decoded sample here and let the pump deliver it.
        thread_local! {
            static LOAD_TARGET: RefCell<Option<(usize, Arc<SampleData>)>> =
                const { RefCell::new(None) };
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
                LOAD_TARGET.with(|t| {
                    if let Some((ch, sample)) = t.borrow_mut().take() {
                        handle.load_sample(ch, sample);
                    }
                });
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
                // Channel 0, pattern 0: four on the floor.
                for step in [0, 4, 8, 12] {
                    w.invoke_step_clicked(0, step);
                }
                // Pattern 1: off-beat ghost notes; channel 1 on pattern 0.
                w.invoke_pattern_selected(1);
                w.invoke_add_channel_clicked();
                w.invoke_step_clicked(1, 2);
                w.invoke_pattern_selected(0);
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
                    if ok {
                        "PASS — UI wiring delivers commands/events"
                    } else {
                        "FAIL"
                    }
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
