//! Slint UI wrapper. Owns the `EngineHandle`, wires Slint callbacks to engine
//! commands, and runs a high-frequency timer that forwards commands and drains
//! audio events onto window properties.
//!
//! The UI owns the project state (channels, pattern bank, per-channel sampler
//! params) as the source of truth and mirrors every mutation to the engine
//! via commands. The engine keeps its own pre-allocated copy.

mod meter;
mod settings;

slint::include_modules!();

use meter::MeterBallistics;
use mooloop_core::{
    EngineCommand, EngineEvent, LoopMode, Ppq, SamplerParams, Step, DEFAULT_STEPS, MAX_CHANNELS,
    MAX_PATTERNS, MAX_PATTERN_STEPS,
};
use mooloop_dsp::SampleData;
use mooloop_engine::EngineHandle;
use settings::{AppearancePreset, AppearanceSettings, ThemePalette, UiSettings};
use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

const PUMP_INTERVAL_MS: u64 = 8;
const INITIAL_BPM: i32 = 120;
/// Fader positions for time-based params map onto [0, MAX_TIME_S] seconds.
const MAX_TIME_S: f32 = 2.0;
const WAVEFORM_BINS: usize = 256;

fn apply_theme(window: &MainWindow, palette: ThemePalette) {
    let theme = window.global::<Theme>();
    theme.set_background(palette.background.color());
    theme.set_panel(palette.panel.color());
    theme.set_surface(palette.surface.color());
    theme.set_surface_raised(palette.raised.color());
    theme.set_surface_active(palette.active.color());
    theme.set_border(palette.border.color());
    theme.set_text(palette.text.color());
    theme.set_text_muted(palette.muted.color());
    theme.set_text_faint(palette.faint.color());
    theme.set_accent(palette.accent.color());
    theme.set_accent_active(palette.accent_active.color());
    theme.set_focus(palette.focus.color());
    theme.set_warning(palette.warning.color());
    theme.set_destructive(palette.destructive.color());
    theme.set_destructive_active(palette.destructive_active.color());
    theme.set_meter_safe(palette.meter_safe.color());
    theme.set_meter_warning(palette.meter_warning.color());
    theme.set_meter_clip(palette.meter_clip.color());
}

fn sync_appearance_properties(window: &MainWindow, appearance: &AppearanceSettings) {
    window.set_appearance_preset(appearance.preset.index());
    window.set_appearance_accent(appearance.accent.as_str().into());
    window.set_appearance_error("".into());
}

/// UI-side state for one channel. `steps` is the pattern bank:
/// `[pattern][step]`.
struct ChannelState {
    name: String,
    muted: bool,
    params: SamplerParams,
    sample_name: String,
    sample_description: String,
    sample_duration: f32,
    sample_path: Option<PathBuf>,
    waveform: Vec<f32>,
    can_previous_sample: bool,
    can_next_sample: bool,
    steps: Vec<Vec<Step>>,
}

impl ChannelState {
    fn new(
        index: usize,
        default_waveform: Vec<f32>,
        default_description: String,
        default_duration: f32,
    ) -> Self {
        Self {
            name: format!("Sampler {}", index + 1),
            muted: false,
            params: SamplerParams::default(),
            sample_name: "default kick".into(),
            sample_description: default_description,
            sample_duration: default_duration,
            sample_path: None,
            waveform: default_waveform,
            can_previous_sample: false,
            can_next_sample: false,
            steps: vec![vec![Step::default(); MAX_PATTERN_STEPS as usize]; MAX_PATTERNS],
        }
    }
}

struct LoadedSample {
    path: PathBuf,
    sample: Arc<SampleData>,
    can_previous: bool,
    can_next: bool,
}

/// Result of a background sample load, delivered to the pump.
struct LoadResult {
    channel: usize,
    /// `None` = dialog cancelled; `Some(Err)` = decode failed.
    result: Option<Result<LoadedSample, String>>,
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
    note_model: Rc<VecModel<NoteCell>>,
    waveform_model: Rc<VecModel<f32>>,
    default_waveform: Vec<f32>,
    default_sample_description: String,
    default_sample_duration: f32,
    pattern_lengths: Vec<usize>,
    current_pattern: usize,
    selected: usize,
    selected_step: usize,
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
        let length = self.pattern_lengths[pattern];
        for (i, ch) in self.channels.iter().enumerate() {
            let cells: Vec<StepCell> = ch.steps[pattern]
                .iter()
                .take(length)
                .map(|step| StepCell {
                    active: step.on,
                    velocity: step.velocity as i32,
                })
                .collect();
            self.step_models[i].set_vec(cells);
        }
    }

    fn refresh_note_editor(&self, window: &MainWindow) {
        let length = self.pattern_lengths[self.current_pattern];
        let Some(channel) = self.channels.get(self.selected) else {
            return;
        };
        let cells: Vec<NoteCell> = channel.steps[self.current_pattern]
            .iter()
            .take(length)
            .enumerate()
            .map(|(index, step)| NoteCell {
                active: step.on,
                note: step.note as i32,
                velocity: step.velocity as i32,
                selected: index == self.selected_step,
            })
            .collect();
        self.note_model.set_vec(cells);

        if let Some(step) = channel.steps[self.current_pattern].get(self.selected_step) {
            window.set_selected_note_step(self.selected_step as i32);
            window.set_selected_note(step.note as i32);
            window.set_selected_velocity(step.velocity as i32);
        }
    }

    fn refresh_note_cell(&self, index: usize) {
        let Some(channel) = self.channels.get(self.selected) else {
            return;
        };
        let Some(step) = channel.steps[self.current_pattern].get(index) else {
            return;
        };
        self.note_model.set_row_data(
            index,
            NoteCell {
                active: step.on,
                note: step.note as i32,
                velocity: step.velocity as i32,
                selected: index == self.selected_step,
            },
        );
    }

    fn refresh_selected_note_controls(&self, window: &MainWindow) {
        let Some(step) =
            self.channels[self.selected].steps[self.current_pattern].get(self.selected_step)
        else {
            return;
        };
        window.set_selected_note_step(self.selected_step as i32);
        window.set_selected_note(step.note as i32);
        window.set_selected_velocity(step.velocity as i32);
    }

    /// Refresh the bottom editor's properties from `selected`.
    fn refresh_editor(&self, window: &MainWindow) {
        let Some(ch) = self.channels.get(self.selected) else {
            return;
        };
        let p = &ch.params;
        window.set_selected_channel_name(ch.name.as_str().into());
        window.set_sample_name(ch.sample_name.as_str().into());
        window.set_sample_description(ch.sample_description.as_str().into());
        window.set_sample_duration(ch.sample_duration);
        self.waveform_model.set_vec(ch.waveform.clone());
        window.set_can_previous_sample(ch.can_previous_sample);
        window.set_can_next_sample(ch.can_next_sample);
        window.set_attack(time_to_norm(p.attack));
        window.set_decay(time_to_norm(p.decay));
        window.set_sustain(p.sustain);
        window.set_release(time_to_norm(p.release));
        window.set_start_pos(p.start);
        window.set_end_pos(p.end);
        window.set_loop_start(p.loop_start);
        window.set_loop_end(p.loop_end);
        window.set_reverse_playback(p.reverse);
        window.set_root_note(p.root_note as i32);
        window.set_tune_semitones(p.tune_semitones);
        window.set_tune_cents(p.tune_cents);
        window.set_loop_mode(match p.loop_mode {
            LoopMode::Off => 0,
            LoopMode::Forward => 1,
            LoopMode::Pingpong => 2,
        });
        window.set_filter_cutoff(p.filter_cutoff);
        window.set_filter_resonance(p.filter_resonance);
        window.set_filter_env((p.filter_env_amount + 1.0) * 0.5);
        window.set_sampler_drive(p.drive);
        window.set_bit_reduction(p.bit_reduction);
        window.set_rate_reduction(p.rate_reduction);
        self.refresh_note_editor(window);
    }
}

impl AppUi {
    pub fn new(mut handle: EngineHandle) -> Result<Self, slint::PlatformError> {
        let window = MainWindow::new()?;

        // --- Transport initial state ---
        window.set_bpm(INITIAL_BPM);
        window.set_playing(false);
        window.set_beat_in_bar(0);
        window.set_meter_l_db(-60.0);
        window.set_meter_r_db(-60.0);
        window.set_meter_l_held_db(-60.0);
        window.set_meter_r_held_db(-60.0);
        window.set_meter_l_clipping(false);
        window.set_meter_r_clipping(false);
        window.set_current_pattern(0);
        window.set_pattern_length(DEFAULT_STEPS as i32);
        window.set_current_step(0);
        window.set_editor_page(0);
        handle.send(EngineCommand::SetTempo(INITIAL_BPM as f64));

        // --- Appearance settings and live preview ---
        let ui_settings = Rc::new(RefCell::new(UiSettings::load_or_default()));
        {
            let settings = ui_settings.borrow();
            apply_theme(&window, settings.appearance.palette());
            sync_appearance_properties(&window, &settings.appearance);
        }
        {
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_appearance_opened(move || {
                let Some(window) = weak.upgrade() else { return };
                let settings = settings.borrow();
                apply_theme(&window, settings.appearance.palette());
                sync_appearance_properties(&window, &settings.appearance);
            });
        }
        {
            let weak = window.as_weak();
            window.on_appearance_preview(move |preset, accent| {
                let Some(window) = weak.upgrade() else { return };
                match AppearanceSettings::validated(
                    AppearancePreset::from_index(preset),
                    accent.as_str(),
                ) {
                    Ok(appearance) => {
                        apply_theme(&window, appearance.palette());
                        window.set_appearance_error("".into());
                    }
                    Err(error) => window.set_appearance_error(error.to_string().into()),
                }
            });
        }
        {
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_appearance_save(move |preset, accent| {
                let Some(window) = weak.upgrade() else {
                    return false;
                };
                let appearance = match AppearanceSettings::validated(
                    AppearancePreset::from_index(preset),
                    accent.as_str(),
                ) {
                    Ok(appearance) => appearance,
                    Err(error) => {
                        window.set_appearance_error(error.to_string().into());
                        return false;
                    }
                };
                apply_theme(&window, appearance.palette());
                let mut settings = settings.borrow_mut();
                let previous = settings.appearance.clone();
                settings.appearance = appearance;
                if let Err(error) = settings.save() {
                    settings.appearance = previous;
                    window.set_appearance_error(format!("Could not save settings: {error}").into());
                    return false;
                }
                sync_appearance_properties(&window, &settings.appearance);
                true
            });
        }
        {
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_appearance_cancelled(move || {
                let Some(window) = weak.upgrade() else { return };
                let settings = settings.borrow();
                apply_theme(&window, settings.appearance.palette());
                sync_appearance_properties(&window, &settings.appearance);
            });
        }

        // --- Channel rack state: start with one channel ---
        let default_sample = handle.sample_snapshot(0);
        let audio_sample_rate = default_sample
            .as_ref()
            .map(|sample| sample.sample_rate)
            .unwrap_or(48_000);
        window.set_audio_sample_rate(audio_sample_rate as i32);
        let default_waveform = default_sample
            .as_ref()
            .map(|sample| waveform_peaks(sample, WAVEFORM_BINS))
            .unwrap_or_default();
        let default_sample_description = default_sample
            .as_ref()
            .map(|sample| sample_description(sample))
            .unwrap_or_default();
        let default_sample_duration = default_sample
            .as_ref()
            .map(|sample| sample_duration(sample))
            .unwrap_or_default();
        let first = ChannelState::new(
            0,
            default_waveform.clone(),
            default_sample_description.clone(),
            default_sample_duration,
        );
        let first_steps: Vec<StepCell> = first.steps[0]
            .iter()
            .take(DEFAULT_STEPS as usize)
            .map(|step| StepCell {
                active: step.on,
                velocity: step.velocity as i32,
            })
            .collect();
        let step_model = Rc::new(VecModel::from(first_steps));
        let note_model = Rc::new(VecModel::from(
            first.steps[0]
                .iter()
                .take(DEFAULT_STEPS as usize)
                .enumerate()
                .map(|(index, step)| NoteCell {
                    active: step.on,
                    note: step.note as i32,
                    velocity: step.velocity as i32,
                    selected: index == 0,
                })
                .collect::<Vec<_>>(),
        ));
        let row = ChannelRow {
            name: first.name.as_str().into(),
            muted: false,
            selected: true,
            steps: ModelRc::from(step_model.clone()),
        };
        let rows_model = Rc::new(VecModel::from(vec![row]));
        let waveform_model = Rc::new(VecModel::from(first.waveform.clone()));
        window.set_channels(ModelRc::from(rows_model.clone()));
        window.set_notes(ModelRc::from(note_model.clone()));
        window.set_waveform(ModelRc::from(waveform_model.clone()));

        let state = Rc::new(RefCell::new(UiState {
            channels: vec![first],
            rows: rows_model,
            step_models: vec![step_model],
            note_model,
            waveform_model,
            default_waveform,
            default_sample_description,
            default_sample_duration,
            pattern_lengths: vec![DEFAULT_STEPS as usize; MAX_PATTERNS],
            current_pattern: 0,
            selected: 0,
            selected_step: 0,
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
                    st.selected_step = st
                        .selected_step
                        .min(st.pattern_lengths[p].saturating_sub(1));
                    st.show_pattern(p);
                }
                if let Some(w) = weak.upgrade() {
                    w.set_current_pattern(p as i32);
                    let st = st.borrow();
                    w.set_pattern_length(st.pattern_lengths[p] as i32);
                    st.refresh_editor(&w);
                }
                let _ = tx.send(EngineCommand::SetCurrentPattern(p as u8));
            });
        }

        // Per-pattern logical length. Channel storage stays at the maximum so
        // shortening and re-extending a pattern does not discard hidden steps.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_pattern_length_changed(move |length| {
                let length = length.clamp(1, MAX_PATTERN_STEPS as i32) as usize;
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                if st.pattern_lengths[pattern] == length {
                    return;
                }
                st.pattern_lengths[pattern] = length;
                st.selected_step = st.selected_step.min(length - 1);
                st.show_pattern(pattern);
                if let Some(w) = weak.upgrade() {
                    w.set_pattern_length(length as i32);
                    st.refresh_note_editor(&w);
                }
                let _ = tx.send(EngineCommand::SetPatternLength {
                    pattern: pattern as u8,
                    length_steps: length as u16,
                });
            });
        }

        // Step toggle (channel, step).
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_step_clicked(move |ch, step| {
                let (ch, step) = (ch as usize, step as usize);
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                if ch >= st.channels.len() || step >= st.pattern_lengths[pattern] {
                    return;
                }
                dbg_log(&format!("UI: ch {ch} step {step} toggled"));
                let p = pattern;
                st.channels[ch].steps[p][step].on = !st.channels[ch].steps[p][step].on;
                let edited = st.channels[ch].steps[p][step];
                let new_active = edited.on;
                st.step_models[ch].set_row_data(
                    step,
                    StepCell {
                        active: new_active,
                        velocity: edited.velocity as i32,
                    },
                );
                if ch == st.selected {
                    st.selected_step = step;
                    if let Some(w) = weak.upgrade() {
                        st.refresh_note_editor(&w);
                    }
                }
                let _ = tx.send(EngineCommand::SetStep {
                    pattern: p as u8,
                    channel: ch as u8,
                    step: step as u8,
                    on: new_active,
                    note: edited.note,
                    velocity: edited.velocity,
                });
            });
        }

        // Right-click clearing is idempotent and preserves pitch/velocity.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_step_removed(move |ch, step| {
                let (ch, step) = (ch as usize, step as usize);
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                if ch >= st.channels.len() || step >= st.pattern_lengths[pattern] {
                    return;
                }
                let edited = {
                    let edited = &mut st.channels[ch].steps[pattern][step];
                    edited.on = false;
                    *edited
                };
                st.step_models[ch].set_row_data(
                    step,
                    StepCell {
                        active: false,
                        velocity: edited.velocity as i32,
                    },
                );
                if ch == st.selected {
                    st.selected_step = step;
                    if let Some(w) = weak.upgrade() {
                        st.refresh_note_editor(&w);
                    }
                }
                let _ = tx.send(EngineCommand::SetStep {
                    pattern: pattern as u8,
                    channel: ch as u8,
                    step: step as u8,
                    on: false,
                    note: edited.note,
                    velocity: edited.velocity,
                });
            });
        }

        // Ctrl-dragging a rack step sets velocity and activates the step.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_step_velocity_edited(move |ch, step, value| {
                let (ch, step) = (ch as usize, step as usize);
                let velocity = (1.0 + value.clamp(0.0, 1.0) * 126.0).round() as u8;
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                if ch >= st.channels.len() || step >= st.pattern_lengths[pattern] {
                    return;
                }
                let edited = {
                    let edited = &mut st.channels[ch].steps[pattern][step];
                    edited.on = true;
                    edited.velocity = velocity;
                    *edited
                };
                st.step_models[ch].set_row_data(
                    step,
                    StepCell {
                        active: true,
                        velocity: edited.velocity as i32,
                    },
                );
                if ch == st.selected {
                    st.selected_step = step;
                    if let Some(w) = weak.upgrade() {
                        st.refresh_note_editor(&w);
                    }
                }
                let _ = tx.send(EngineCommand::SetStep {
                    pattern: pattern as u8,
                    channel: ch as u8,
                    step: step as u8,
                    on: true,
                    note: edited.note,
                    velocity: edited.velocity,
                });
            });
        }

        // Piano roll pitch editing for the selected channel and pattern.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_piano_note_edited(move |step, note| {
                let step = step as usize;
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                let channel = st.selected;
                if step >= st.pattern_lengths[pattern] {
                    return;
                }
                let previous = st.selected_step;
                st.selected_step = step;
                let edited = {
                    let edited = &mut st.channels[channel].steps[pattern][step];
                    edited.on = true;
                    edited.note = note.clamp(36, 84) as u8;
                    *edited
                };
                st.step_models[channel].set_row_data(
                    step,
                    StepCell {
                        active: true,
                        velocity: edited.velocity as i32,
                    },
                );
                if let Some(w) = weak.upgrade() {
                    st.refresh_note_cell(previous);
                    st.refresh_note_cell(step);
                    st.refresh_selected_note_controls(&w);
                }
                let _ = tx.send(EngineCommand::SetStep {
                    pattern: pattern as u8,
                    channel: channel as u8,
                    step: step as u8,
                    on: edited.on,
                    note: edited.note,
                    velocity: edited.velocity,
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_piano_note_removed(move |step| {
                let step = step as usize;
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                let channel = st.selected;
                if step >= st.pattern_lengths[pattern] {
                    return;
                }
                let previous = st.selected_step;
                st.selected_step = step;
                let edited = {
                    let edited = &mut st.channels[channel].steps[pattern][step];
                    edited.on = false;
                    *edited
                };
                st.step_models[channel].set_row_data(
                    step,
                    StepCell {
                        active: false,
                        velocity: edited.velocity as i32,
                    },
                );
                if let Some(w) = weak.upgrade() {
                    st.refresh_note_cell(previous);
                    st.refresh_note_cell(step);
                    st.refresh_selected_note_controls(&w);
                }
                let _ = tx.send(EngineCommand::SetStep {
                    pattern: pattern as u8,
                    channel: channel as u8,
                    step: step as u8,
                    on: false,
                    note: edited.note,
                    velocity: edited.velocity,
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_piano_note_moved(move |_source, destination, note| {
                let destination = destination as usize;
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                let channel = st.selected;
                let source = st.selected_step;
                if source >= st.pattern_lengths[pattern]
                    || destination >= st.pattern_lengths[pattern]
                {
                    return;
                }

                let mut moved = st.channels[channel].steps[pattern][source];
                moved.on = true;
                moved.note = note.clamp(36, 84) as u8;

                if source != destination {
                    let source_after = {
                        let source_step = &mut st.channels[channel].steps[pattern][source];
                        source_step.on = false;
                        *source_step
                    };
                    st.channels[channel].steps[pattern][destination] = moved;
                    st.step_models[channel].set_row_data(
                        source,
                        StepCell {
                            active: false,
                            velocity: source_after.velocity as i32,
                        },
                    );
                    let _ = tx.send(EngineCommand::SetStep {
                        pattern: pattern as u8,
                        channel: channel as u8,
                        step: source as u8,
                        on: false,
                        note: source_after.note,
                        velocity: source_after.velocity,
                    });
                } else {
                    st.channels[channel].steps[pattern][source] = moved;
                }

                st.selected_step = destination;
                st.step_models[channel].set_row_data(
                    destination,
                    StepCell {
                        active: true,
                        velocity: moved.velocity as i32,
                    },
                );
                if let Some(w) = weak.upgrade() {
                    st.refresh_note_cell(source);
                    st.refresh_note_cell(destination);
                    st.refresh_selected_note_controls(&w);
                }
                let _ = tx.send(EngineCommand::SetStep {
                    pattern: pattern as u8,
                    channel: channel as u8,
                    step: destination as u8,
                    on: true,
                    note: moved.note,
                    velocity: moved.velocity,
                });
            });
        }

        // Direct drawing in the parameter lane. Velocity zero is avoided so
        // an active note never becomes an implicit MIDI note-off.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_velocity_edited(move |step, value| {
                let step = step as usize;
                let velocity = (1.0 + value.clamp(0.0, 1.0) * 126.0).round() as u8;
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                let channel = st.selected;
                if step >= st.pattern_lengths[pattern] {
                    return;
                }
                st.selected_step = step;
                let edited = {
                    let edited = &mut st.channels[channel].steps[pattern][step];
                    edited.velocity = velocity;
                    *edited
                };
                st.step_models[channel].set_row_data(
                    step,
                    StepCell {
                        active: edited.on,
                        velocity: edited.velocity as i32,
                    },
                );
                if let Some(w) = weak.upgrade() {
                    st.refresh_note_editor(&w);
                }
                let _ = tx.send(EngineCommand::SetStep {
                    pattern: pattern as u8,
                    channel: channel as u8,
                    step: step as u8,
                    on: edited.on,
                    note: edited.note,
                    velocity: edited.velocity,
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_selected_note_changed(move |note| {
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                let channel = st.selected;
                let step = st.selected_step;
                let edited = {
                    let edited = &mut st.channels[channel].steps[pattern][step];
                    edited.on = true;
                    edited.note = note.clamp(36, 84) as u8;
                    *edited
                };
                st.step_models[channel].set_row_data(
                    step,
                    StepCell {
                        active: true,
                        velocity: edited.velocity as i32,
                    },
                );
                if let Some(w) = weak.upgrade() {
                    st.refresh_note_editor(&w);
                }
                let _ = tx.send(EngineCommand::SetStep {
                    pattern: pattern as u8,
                    channel: channel as u8,
                    step: step as u8,
                    on: edited.on,
                    note: edited.note,
                    velocity: edited.velocity,
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_selected_velocity_changed(move |velocity| {
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                let channel = st.selected;
                let step = st.selected_step;
                let edited = {
                    let edited = &mut st.channels[channel].steps[pattern][step];
                    edited.velocity = velocity.clamp(1, 127) as u8;
                    *edited
                };
                st.step_models[channel].set_row_data(
                    step,
                    StepCell {
                        active: edited.on,
                        velocity: edited.velocity as i32,
                    },
                );
                if let Some(w) = weak.upgrade() {
                    st.refresh_note_editor(&w);
                }
                let _ = tx.send(EngineCommand::SetStep {
                    pattern: pattern as u8,
                    channel: channel as u8,
                    step: step as u8,
                    on: edited.on,
                    note: edited.note,
                    velocity: edited.velocity,
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
                let ch = ChannelState::new(
                    index,
                    st.default_waveform.clone(),
                    st.default_sample_description.clone(),
                    st.default_sample_duration,
                );
                let cells: Vec<StepCell> = ch.steps[st.current_pattern]
                    .iter()
                    .take(st.pattern_lengths[st.current_pattern])
                    .map(|step| StepCell {
                        active: step.on,
                        velocity: step.velocity as i32,
                    })
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
        wire_unit_param!(on_end_pos_changed, end);
        wire_unit_param!(on_loop_start_changed, loop_start);
        wire_unit_param!(on_loop_end_changed, loop_end);
        wire_unit_param!(on_tune_semitones_changed, tune_semitones);
        wire_unit_param!(on_tune_cents_changed, tune_cents);
        wire_unit_param!(on_filter_cutoff_changed, filter_cutoff);
        wire_unit_param!(on_filter_resonance_changed, filter_resonance);
        wire_unit_param!(on_sampler_drive_changed, drive);
        wire_unit_param!(on_bit_reduction_changed, bit_reduction);
        wire_unit_param!(on_rate_reduction_changed, rate_reduction);

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_reverse_playback_changed(move |reverse| {
                let mut st = st.borrow_mut();
                let ch = st.selected;
                let Some(channel) = st.channels.get_mut(ch) else {
                    return;
                };
                channel.params.reverse = reverse;
                let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                    channel: ch as u8,
                    params: channel.params,
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_root_note_changed(move |note| {
                let mut st = st.borrow_mut();
                let ch = st.selected;
                let Some(channel) = st.channels.get_mut(ch) else {
                    return;
                };
                channel.params.root_note = note.clamp(0, 127) as u8;
                let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                    channel: ch as u8,
                    params: channel.params,
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_filter_env_changed(move |v| {
                let mut st = st.borrow_mut();
                let ch = st.selected;
                let Some(channel) = st.channels.get_mut(ch) else {
                    return;
                };
                channel.params.filter_env_amount = v.clamp(0.0, 1.0) * 2.0 - 1.0;
                let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                    channel: ch as u8,
                    params: channel.params,
                });
            });
        }

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
        // The dialog + decode run on a worker thread so the UI stays
        // responsive (a blocking dialog makes the OS mark the app frozen and
        // offer to kill it). Results come back through `load_rx` and are
        // applied by the pump on the UI thread.
        let (load_tx, load_rx) = std::sync::mpsc::channel::<LoadResult>();
        {
            let st = state.clone();
            let load_tx = load_tx.clone();
            window.on_load_sample_clicked(move || {
                let channel = st.borrow().selected;
                let tx = load_tx.clone();
                dbg_log(&format!("UI: loading sample for channel {channel}"));
                std::thread::spawn(move || {
                    let result = pick_wav_via_zenity().map(|path| load_sample_at_path(&path));
                    let _ = tx.send(LoadResult { channel, result });
                });
            });
        }
        {
            let st = state.clone();
            let load_tx = load_tx.clone();
            window.on_previous_sample_clicked(move || {
                let (channel, path) = {
                    let st = st.borrow();
                    (st.selected, st.channels[st.selected].sample_path.clone())
                };
                let Some(path) = path else { return };
                let tx = load_tx.clone();
                std::thread::spawn(move || {
                    let result = match adjacent_wav(&path, -1) {
                        Ok(Some(path)) => Some(load_sample_at_path(&path)),
                        Ok(None) => None,
                        Err(error) => Some(Err(error)),
                    };
                    let _ = tx.send(LoadResult { channel, result });
                });
            });
        }
        {
            let st = state.clone();
            let load_tx = load_tx.clone();
            window.on_next_sample_clicked(move || {
                let (channel, path) = {
                    let st = st.borrow();
                    (st.selected, st.channels[st.selected].sample_path.clone())
                };
                let Some(path) = path else { return };
                let tx = load_tx.clone();
                std::thread::spawn(move || {
                    let result = match adjacent_wav(&path, 1) {
                        Ok(Some(path)) => Some(load_sample_at_path(&path)),
                        Ok(None) => None,
                        Err(error) => Some(Err(error)),
                    };
                    let _ = tx.send(LoadResult { channel, result });
                });
            });
        }

        // --- Pump: forward queued commands, apply finished sample loads,
        //     drain audio events onto window ---
        let weak = window.as_weak();
        let st = state.clone();
        let pump = Timer::default();
        // Diagnostics shared with the autodrive self-test (MOOLOOP_AUTODRIVE=1).
        let stats = Rc::new(std::cell::Cell::new((0.0f32, false, 0usize)));
        let stats_in = stats.clone();
        let mut left_meter = MeterBallistics::default();
        let mut right_meter = MeterBallistics::default();
        let mut last_meter_update = std::time::Instant::now();
        pump.start(
            TimerMode::Repeated,
            std::time::Duration::from_millis(PUMP_INTERVAL_MS),
            move || {
                while let Ok(load) = load_rx.try_recv() {
                    let Some(loaded) = (match load.result {
                        Some(Ok(loaded)) => Some(loaded),
                        Some(Err(e)) => {
                            eprintln!("mooloop: failed to load sample: {e}");
                            None
                        }
                        None => None, // dialog cancelled
                    }) else {
                        continue;
                    };
                    let name = loaded
                        .path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("loaded")
                        .to_string();
                    dbg_log(&format!("UI: channel {} loaded {name}", load.channel));
                    let waveform = waveform_peaks(&loaded.sample, WAVEFORM_BINS);
                    let description = sample_description(&loaded.sample);
                    let duration = sample_duration(&loaded.sample);
                    handle.load_sample(load.channel, loaded.sample);
                    let mut st = st.borrow_mut();
                    if let Some(ch) = st.channels.get_mut(load.channel) {
                        ch.sample_name = name;
                        ch.sample_description = description;
                        ch.sample_duration = duration;
                        ch.sample_path = Some(loaded.path);
                        ch.waveform = waveform;
                        ch.can_previous_sample = loaded.can_previous;
                        ch.can_next_sample = loaded.can_next;
                    }
                    if load.channel == st.selected {
                        if let Some(w) = weak.upgrade() {
                            st.refresh_editor(&w);
                        }
                    }
                }
                let mut forwarded = 0usize;
                while let Ok(cmd) = cmd_rx.try_recv() {
                    handle.send(cmd);
                    forwarded += 1;
                }
                let Some(w) = weak.upgrade() else { return };
                let mut saw_nonzero = false;
                let mut block_peak_l = 0.0f32;
                let mut block_peak_r = 0.0f32;
                for ev in handle.drain() {
                    match ev {
                        EngineEvent::Position {
                            tick,
                            beat_in_bar,
                            playing,
                        } => {
                            w.set_beat_in_bar(beat_in_bar as i32);
                            w.set_playing(playing);
                            let st = st.borrow();
                            let length = st.pattern_lengths[st.current_pattern] as u64;
                            let ticks_per_step = (Ppq::DEFAULT.ticks_per_beat() / 4) as u64;
                            w.set_current_step(((tick / ticks_per_step) % length) as i32);
                        }
                        EngineEvent::Metering { peak_l, peak_r } => {
                            block_peak_l = block_peak_l.max(peak_l.max(0.0));
                            block_peak_r = block_peak_r.max(peak_r.max(0.0));
                            if peak_l > 0.0 || peak_r > 0.0 {
                                saw_nonzero = true;
                            }
                        }
                        EngineEvent::Xrun => {
                            eprintln!("mooloop: JACK reported an xrun (audio dropout)");
                        }
                    }
                }
                let now = std::time::Instant::now();
                let elapsed = now.duration_since(last_meter_update).as_secs_f32();
                last_meter_update = now;
                let left = left_meter.update(block_peak_l, elapsed);
                let right = right_meter.update(block_peak_r, elapsed);
                w.set_meter_l_db(left.level_db);
                w.set_meter_r_db(right.level_db);
                w.set_meter_l_held_db(left.held_db);
                w.set_meter_r_held_db(right.held_db);
                w.set_meter_l_clipping(left.clipping);
                w.set_meter_r_clipping(right.clipping);
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
                w.invoke_pattern_length_changed(32);
                w.invoke_step_velocity_edited(0, 0, 0.5);
                w.invoke_step_removed(0, 4);
                w.invoke_piano_note_edited(6, 72);
                w.invoke_piano_note_moved(6, 7, 74);
                w.invoke_velocity_edited(7, 0.35);
                w.invoke_piano_note_removed(7);
                w.set_editor_page(1);
                w.invoke_play_clicked();
            });
            let stats = stats.clone();
            slint::Timer::single_shot(std::time::Duration::from_millis(4500), move || {
                let (max_peak, saw_playing, forwarded) = stats.get();
                println!("--- ui autodrive report ---");
                println!("commands forwarded by pump : {forwarded}");
                println!("saw playing=true on window : {saw_playing}");
                println!("nonzero metering seen     : {max_peak:.4}");
                let ok = saw_playing && forwarded >= 17;
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
fn pick_wav_via_zenity() -> Option<PathBuf> {
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
        Some(PathBuf::from(path))
    }
}

fn wav_files_in_directory(path: &Path) -> Result<Vec<PathBuf>, String> {
    let directory = path
        .parent()
        .ok_or_else(|| "sample path has no parent directory".to_string())?;
    let mut files = std::fs::read_dir(directory)
        .map_err(|e| format!("could not read sample directory: {e}"))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|entry| {
            entry.is_file()
                && entry
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
        })
        .collect::<Vec<_>>();
    files.sort_by_cached_key(|entry| {
        entry
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    Ok(files)
}

fn sample_index(path: &Path, files: &[PathBuf]) -> Option<usize> {
    files
        .iter()
        .position(|candidate| candidate == path)
        .or_else(|| {
            let name = path.file_name()?;
            files
                .iter()
                .position(|candidate| candidate.file_name() == Some(name))
        })
}

fn adjacent_wav(path: &Path, direction: isize) -> Result<Option<PathBuf>, String> {
    let files = wav_files_in_directory(path)?;
    let Some(index) = sample_index(path, &files) else {
        return Ok(None);
    };
    let next = index as isize + direction;
    Ok((next >= 0)
        .then(|| files.get(next as usize).cloned())
        .flatten())
}

fn load_sample_at_path(path: &Path) -> Result<LoadedSample, String> {
    let files = wav_files_in_directory(path)?;
    let index = sample_index(path, &files);
    let sample = decode_wav(path)?;
    Ok(LoadedSample {
        path: path.to_path_buf(),
        sample,
        can_previous: index.is_some_and(|index| index > 0),
        can_next: index.is_some_and(|index| index + 1 < files.len()),
    })
}

fn waveform_peaks(sample: &SampleData, max_bins: usize) -> Vec<f32> {
    if sample.frames.is_empty() || max_bins == 0 {
        return Vec::new();
    }
    let bins = max_bins.min(sample.frames.len());
    let mut peaks = (0..bins)
        .map(|bin| {
            let start = bin * sample.frames.len() / bins;
            let end = ((bin + 1) * sample.frames.len() / bins).max(start + 1);
            sample.frames[start..end]
                .iter()
                .map(|frame| frame[0].abs().max(frame[1].abs()))
                .fold(0.0f32, f32::max)
        })
        .collect::<Vec<_>>();
    let peak = peaks.iter().copied().fold(0.0f32, f32::max);
    if peak > 0.0 {
        for value in &mut peaks {
            *value /= peak;
        }
    }
    peaks
}

fn sample_description(sample: &SampleData) -> String {
    let seconds = f64::from(sample_duration(sample));
    format!("{seconds:.3} s  |  {} Hz  |  stereo", sample.sample_rate)
}

fn sample_duration(sample: &SampleData) -> f32 {
    sample.len() as f32 / sample.sample_rate.max(1) as f32
}

/// Decode a WAV/RIFF file into stereo f32 frames. hound's `samples::<f32>()`
/// only works for IEEE-float files, so integer formats are read at their
/// native width and normalised to [-1, 1] here. Errors propagate loudly —
/// never silently drop samples (an empty buffer would silently mute the
/// sampler).
fn decode_wav(path: &Path) -> Result<Arc<SampleData>, String> {
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

        let data = decode_wav(&path).unwrap();
        assert_eq!(data.sample_rate, 44_100);
        assert_eq!(data.len(), 1000);
        assert!(data.frames.iter().any(|f| f[0] != 0.0));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_garbage_file() {
        let path = std::env::temp_dir().join("mooloop_decode_test_garbage.wav");
        std::fs::write(&path, b"not a wav at all").unwrap();
        assert!(decode_wav(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn waveform_peaks_are_normalized_and_bounded() {
        let sample = SampleData {
            frames: vec![[0.0, 0.0], [0.25, -0.5], [1.0, -0.75], [0.1, 0.2]],
            sample_rate: 48_000,
            root_note: 60,
        };

        let peaks = waveform_peaks(&sample, 2);

        assert_eq!(peaks, vec![0.5, 1.0]);
    }

    #[test]
    fn adjacent_wav_walks_sorted_directory_without_wrapping() {
        let directory = std::env::temp_dir().join(format!(
            "mooloop_sample_browser_test_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let a = directory.join("a-kick.wav");
        let b = directory.join("B-snare.WAV");
        let c = directory.join("c-hat.wav");
        for path in [&a, &b, &c] {
            std::fs::write(path, []).unwrap();
        }
        std::fs::write(directory.join("ignore.txt"), []).unwrap();

        assert_eq!(adjacent_wav(&a, -1).unwrap(), None);
        assert_eq!(adjacent_wav(&a, 1).unwrap(), Some(b.clone()));
        assert_eq!(adjacent_wav(&b, 1).unwrap(), Some(c.clone()));
        assert_eq!(adjacent_wav(&c, 1).unwrap(), None);

        std::fs::remove_dir_all(directory).unwrap();
    }
}
