//! Slint UI wrapper. Owns the `EngineHandle`, wires Slint callbacks to engine
//! commands, and runs a high-frequency timer that forwards commands and drains
//! audio events onto window properties.
//!
//! The UI owns the project state (channels, pattern bank, per-channel sampler
//! params) as the source of truth and mirrors every mutation to the engine
//! via commands. The engine keeps its own pre-allocated copy.

mod actions;
mod gestures;
mod meter;
#[cfg(feature = "mockup")]
mod mockup;
mod settings;

slint::include_modules!();

/// The mockup tool's Slint module, compiled from its own entry point so that
/// it stays out of the window's. Nested in a module of its own because two
/// generated modules define the same shared globals and structs, and only one
/// of them can be at the crate root.
#[cfg(feature = "mockup")]
mod mockup_ui {
    include!(concat!(env!("OUT_DIR"), "/mockup-tool.rs"));
}

use meter::MeterBallistics;
use mooloop_core::gain::{linear_to_db, MIN_DB as METER_FLOOR_DB};
use mooloop_core::log::Level;
use mooloop_core::{log_debug, log_error, log_info, log_warn};
use mooloop_core::{
    snap_bars_to_power_of_two,
    BufferDuration, BufferEvent, BusSetup,
    DeviceKind, DrumMode, DrumSynthParams, EffectKind,
    EffectSlotState, EffectTarget, EngineCommand, EngineEvent, EnvTrigger, FilterModel,
    GeneratorParams, GlideMode, HatCharacter,
    KickCharacter, Kit, LfoWave, LoopMode, ModDestinationDescriptor,
    ModPolarity, ModRack, ModRandomTrigger, ModStepTrigger,
    ModulatorKind, ModulatorParams,
    ds01, Ds01Params,
    NoteEvent,
    NoteId, NotePriority, OscWave, ParamAddr,
    ParamCurve, ParamDescriptor, ParamOwner, PointId,
    Project, ProjectChannel, RetriggerMode, SampleReference,
    PlayMode, SamplerParams, SnareCharacter, StretchMode,
    VoiceMode, MAX_SLICES,
    DEFAULT_STEPS, DEFAULT_SWING_PERCENT, MASTER_BUS, MAX_BUSES,
    MAX_CHANNELS, MAX_MODULATORS_PER_CHANNEL,
    MAX_MOD_ROUTES_PER_CHANNEL,
    MOD_STEP_MAX_STEPS,
    MAX_SAMPLER_VOICES, MAX_STRETCH_BARS, MAX_STRETCH_GRAIN, MAX_STRETCH_RATIO,
    MIN_STRETCH_BARS, MIN_STRETCH_GRAIN, MIN_STRETCH_RATIO,
    MAX_PATTERNS, MAX_PLAYLIST_BARS,
    MAX_POLY_VOICES, STRIP_DESCRIPTORS,
    TICKS_PER_64TH, TICKS_PER_BAR, TICKS_PER_STEP,
};
use mooloop_dsp::{
    buffer_allocation_key, build_effect_at_tempo, Ds01, DrumSynth, DryAlign, SampleData,
    SpectrumAnalyzer, StretchPool,
};
use mooloop_engine::{
    EffectSlot, EngineHandle, ExportSpec, OfflineRenderer, PreviewCommand, StructuralCommand,
};
use mooloop_project::{
    AssetMode, AssetWarning, Issue, LoadReport, LoadedDocument, PresetInfo, PresetKind,
    PresetSummary,
};
use mooloop_session::browser::{browser_display_name, has_playable_descendant, scan_browser_dir};
use mooloop_session::channel::{
    apply_sample_references, copied_channel_name, ChannelClipboard, ChannelState,
};
use mooloop_session::command::{cycle_pane, CommandState, Pane};
use mooloop_session::dialogs::{
    confirm_via_zenity, pick_bundle_via_zenity, pick_export_via_zenity, pick_sample_via_zenity,
    pick_save_via_zenity, pick_song_via_zenity,
};
use mooloop_session::document::{
    log_repairs, quarantine_song, repair_suffix, resolve_document, warning_suffix, DocumentProblem,
    DocumentResult, LoadTarget, ResolvedDocument,
};
use mooloop_session::engine::{
    publish_channel_audio_to, AudioAction, AudioActionSender, ChannelAudio, ChannelAudioSender,
    EngineCommandSender, PendingEngineMessage, PreviewSender, ProjectEditSender,
    StructuralCommandSender, TelemetryAction, TelemetryActionSender,
};
use mooloop_session::history::Entry as HistoryEntry;
use mooloop_session::roll::NoteEdit;
use mooloop_session::project::{
    fresh_starter_seed, normalize_project_pattern_banks, HistoryMove, ProjectEdit, ProjectSnapshot,
};
use mooloop_session::sampler::{
    commit_is_stale, slice_fractions, snap_marker, snap_status, SampleMarker, SliceEdit,
};
use mooloop_session::sample::{
    adjacent_sample, inspect_sample, load_sample_at_path, sample_description, sample_duration,
    tune_label, waveform_peaks, waveform_peaks_windowed,
    LoadResult, LoadedSample, SampleInspection,
};
use mooloop_session::session::{ArmedRoute, PresetSaveTarget, Session, WAVEFORM_BINS};
use mooloop_session::values::{
    descriptor_slots, format_bars, measured_loop_bars, parse_typed_value, stretch_bars_from_norm,
    stretch_bars_to_norm, stretch_grain_from_norm, stretch_grain_to_norm, stretch_ratio_from_norm,
    stretch_ratio_to_norm,
};
#[cfg(feature = "mockup")]
pub use mockup::{load_mockup_layout, wire_mockup};
#[cfg(feature = "mockup")]
pub use mockup_ui::MockupCanvas;
use settings::{AppearanceSettings, ThemePalette, ThemeScheme, UiSettings};
use slint::{
    CloseRequestResponse, ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode,
    VecModel,
};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

const PUMP_INTERVAL_MS: u64 = 8;
const INITIAL_BPM: i32 = 120;
/// Fader positions for time-based params map onto [0, MAX_TIME_S] seconds.
const MAX_TIME_S: f32 = 2.0;

/// Fixed JACK buffer size choices offered by the segmented control on the
/// Audio preferences page. Index-addressed to match `SegmentedControl`.
const JACK_BUFFER_SIZES: [u32; 6] = [64, 128, 256, 512, 1024, 2048];
const DRUM_PREVIEW_BINS: usize = 144;

/// Bins in DS-01's rendered hit. Wider than v1's because its scope is wider:
/// the device takes five rack units.
const DS01_PREVIEW_BINS: usize = 256;

/// How long a DS-01 edit sits still before the hit is re-rendered.
///
/// `08-the-face.md` is explicit that the preview must not run on the UI
/// thread per keystroke, and a knob drag is a keystroke every frame. Long
/// enough that a drag renders once when it stops; short enough that letting
/// go feels like the picture was already there.
const DS01_PREVIEW_DEBOUNCE_MS: u64 = 60;

fn sync_drum_preview(window: &MainWindow, params: DrumSynthParams) {
    let (minimums, maximums) = DrumSynth::preview_waveform(params, DRUM_PREVIEW_BINS);
    window.set_drum_preview_minimums(ModelRc::from(Rc::new(VecModel::from(minimums))));
    window.set_drum_preview_maximums(ModelRc::from(Rc::new(VecModel::from(maximums))));
}

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

/// Pushes one appearance state -- colors and the shared radius scale -- into
/// the running window. Live preview and a committed save both go through here,
/// so what the user sees while dragging is exactly what Apply persists.
fn apply_appearance(window: &MainWindow, appearance: &AppearanceSettings) {
    apply_theme(window, appearance.palette());
    window.global::<Theme>().set_roundness(appearance.roundness);
    window
        .global::<DisplayPrefs>()
        .set_smooth_curves(appearance.smooth_curves);
    // Motion deliberately not applied here: this also runs on every live
    // color preview, which would clobber the Appearance page's in-progress
    // motion edits. Motion reaches the window through
    // `sync_preferences_properties` (startup, cancel, and Apply) instead.
}

/// Reads back the Appearance page's live, uncommitted state. The dialog holds
/// the edit in its properties until Apply, so this is what preview, scheme
/// selection, and Save Scheme all have to work from.
fn window_appearance(window: &MainWindow, stored: &AppearanceSettings) -> AppearanceSettings {
    let motion = window.global::<Motion>();
    AppearanceSettings {
        scheme: window.get_preferences_appearance_scheme().into(),
        base: window.get_preferences_appearance_base().into(),
        accent: window.get_preferences_appearance_accent().into(),
        alert: window.get_preferences_appearance_alert().into(),
        contrast: window.get_preferences_appearance_contrast(),
        roundness: window.get_preferences_appearance_roundness(),
        smooth_curves: window.get_preferences_smooth_curves(),
        motion_speed: settings::motion_speed_name(motion.get_speed()).to_owned(),
        motion_easing: settings::motion_easing_name(motion.get_easing()).to_owned(),
        user_schemes: stored.user_schemes.clone(),
    }
}

fn scheme_rows(appearance: &AppearanceSettings) -> ModelRc<AppearanceSchemeRow> {
    let swatch = |hex: &str| settings::Rgb::parse_or_black(hex).color();
    let rows: Vec<AppearanceSchemeRow> = appearance
        .schemes()
        .into_iter()
        .map(|scheme| AppearanceSchemeRow {
            base: swatch(&scheme.base),
            accent: swatch(&scheme.accent),
            alert: swatch(&scheme.alert),
            is_user: !ThemeScheme::is_builtin(&scheme.name),
            name: scheme.name.into(),
        })
        .collect();
    ModelRc::from(Rc::new(VecModel::from(rows)))
}

fn sync_preferences_properties(window: &MainWindow, settings: &UiSettings) {
    let appearance = &settings.appearance;
    window.set_preferences_appearance_scheme(appearance.scheme.as_str().into());
    window.set_preferences_appearance_schemes(scheme_rows(appearance));
    window.set_preferences_appearance_base(appearance.base.as_str().into());
    window.set_preferences_appearance_accent(appearance.accent.as_str().into());
    window.set_preferences_appearance_alert(appearance.alert.as_str().into());
    window.set_preferences_appearance_contrast(appearance.contrast);
    window.set_preferences_appearance_roundness(appearance.roundness);
    window.set_preferences_developer_mode(settings.general.developer_mode);
    window.set_preferences_log_to_file(settings.general.log_to_file);
    window.set_preferences_log_path(settings::log_path().display().to_string().into());
    window.set_snap_to_zero(settings.general.snap_markers_to_zero);
    window.set_preferences_smooth_curves(appearance.smooth_curves);
    window
        .global::<DisplayPrefs>()
        .set_smooth_curves(appearance.smooth_curves);
    let motion = window.global::<Motion>();
    motion.set_speed(settings::motion_speed_index(&appearance.motion_speed));
    motion.set_easing(settings::motion_easing_index(&appearance.motion_easing));
    window.set_preferences_error("".into());
    let buffer_index = settings
        .audio
        .jack
        .buffer_size
        .and_then(|frames| JACK_BUFFER_SIZES.iter().position(|&f| f == frames))
        .map(|i| i as i32)
        .unwrap_or(-1);
    window.set_preferences_audio_buffer_size_index(buffer_index);
    window.set_preferences_audio_auto_reconnect(settings.audio.jack.auto_reconnect);
    window.set_preferences_audio_error("".into());
}

/// Builds the Preferences > Shortcuts page's row model from the action
/// registry (`actions.rs`) and the currently loaded bindings.
/// `is_first_in_category` is computed here rather than in `.slint`, since a
/// `for` loop there has no clean way to compare against the previous item.
fn shortcut_rows(table: &actions::ShortcutTable) -> Vec<ShortcutRow> {
    let mut previous_category = "";
    actions::ACTIONS
        .iter()
        .map(|spec| {
            let is_first_in_category = spec.category != previous_category;
            previous_category = spec.category;
            ShortcutRow {
                id: spec.id.into(),
                label: spec.label.into(),
                category: spec.category.into(),
                chord: table
                    .chord_for(spec.id)
                    .map(|chord| chord.to_string())
                    .unwrap_or_default()
                    .into(),
                is_default: table.is_default(spec.id),
                is_first_in_category,
            }
        })
        .collect()
}

/// Builds the Preferences > Shortcuts page's gesture rows from the gesture
/// registry (`gestures.rs`), and pushes the resolved table onto the piano
/// roll so its pointer handler stops hardcoding modifiers.
///
/// The gestures sit on the Shortcuts page but not in its recorder: that
/// captures a whole chord ending in a key, and these roles are modifiers
/// with no key at all.
/// The resolved modifier for every piano-roll drag role, in the shape the
/// grid tests against.
fn resolve_gestures(table: &gestures::GestureTable) -> PianoGestures {
    let resolve = |id: &str| {
        let modifier = table.modifier(id);
        GestureMod {
            ctrl: modifier.ctrl,
            shift: modifier.shift,
            alt: modifier.alt,
            meta: modifier.meta,
        }
    };
    PianoGestures {
        snap_override: resolve("gesture.snap-override"),
        add_to_selection: resolve("gesture.add-to-selection"),
        subtract_from_selection: resolve("gesture.subtract-from-selection"),
        copy_drag: resolve("gesture.copy-drag"),
        stretch_drag: resolve("gesture.stretch-drag"),
    }
}

/// The gesture table as it stands with no user overrides.
///
/// `run` resolves this from the user's settings and pushes it onto the
/// window. A harness that builds a bare `MainWindow` has to do the same, or
/// `piano-gestures` stays all-false and every gesture role is dead -- which
/// is correct for an unconfigured window and useless for a test.
pub fn default_piano_gestures() -> PianoGestures {
    resolve_gestures(&gestures::GestureTable::build(
        &std::collections::HashMap::new(),
    ))
}

fn sync_gesture_rows(window: &MainWindow, table: &gestures::GestureTable) {
    let rows: Vec<GestureRow> = gestures::GESTURES
        .iter()
        .map(|spec| {
            let modifier = table.modifier(spec.id);
            GestureRow {
                id: spec.id.into(),
                label: spec.label.into(),
                description: spec.description.into(),
                choice_index: gestures::choice_index(modifier),
                is_default: modifier == spec.default,
            }
        })
        .collect();
    window.set_preferences_gesture_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
    window.set_piano_gestures(resolve_gestures(table));
}

fn sync_shortcut_rows(window: &MainWindow, table: &actions::ShortcutTable) {
    window.set_preferences_shortcut_rows(ModelRc::from(Rc::new(VecModel::from(shortcut_rows(
        table,
    )))));
}

/// Re-read live JACK driver status and connectable output targets, and push
/// them onto the window. Called from the pump, which is the only place that
/// holds `EngineHandle`; a non-realtime JACK graph query, not something to
/// run every tick.
fn sync_audio_status(handle: &EngineHandle, window: &MainWindow) {
    let status = handle.driver_status();
    let rows: Vec<OutputTargetRow> = handle
        .available_output_targets()
        .into_iter()
        .map(|target| {
            let selected = target.port_l == status.current_target.0
                && target.port_r == status.current_target.1;
            OutputTargetRow {
                client: target.client.into(),
                port_l: target.port_l.into(),
                port_r: target.port_r.into(),
                selected,
            }
        })
        .collect();
    window.set_preferences_audio_output_targets(ModelRc::from(Rc::new(VecModel::from(rows))));
    let buffer_index = JACK_BUFFER_SIZES
        .iter()
        .position(|&f| f == status.buffer_size)
        .map(|i| i as i32)
        .unwrap_or(-1);
    window.set_preferences_audio_buffer_size_index(buffer_index);
    window.set_preferences_audio_sample_rate_text(
        format!("{} Hz — set by the JACK server", status.sample_rate).into(),
    );
}

/// Shows a pane and records which one it is.
///
/// Recorded rather than derived: the step grid and the dock tabs are
/// simultaneously visible, so there is no single window property that says
/// which pane is current, and Next/Prev has to cycle from where the user
/// actually is.
fn show_pane(commands: &Rc<RefCell<CommandState>>, window: &MainWindow, pane: Pane) {
    commands.borrow_mut().pane = pane;
    apply_pane(window, pane);
}

fn apply_pane(window: &MainWindow, pane: Pane) {
    match pane {
        Pane::Steps => window.set_mixer_visible(false),
        Pane::Mixer => window.set_mixer_visible(true),
        Pane::Source => {
            window.set_mixer_visible(false);
            window.set_editor_page(0);
        }
        Pane::Notes => {
            window.set_mixer_visible(false);
            window.set_editor_page(1);
        }
        Pane::Playlist => {
            window.set_mixer_visible(false);
            window.set_editor_page(2);
        }
    }
}

fn project_snapshot(state: &UiState, window: &MainWindow) -> ProjectSnapshot {
    let mut project = state.session.project_snapshot(window.get_bpm(), window.get_swing_percent());
    normalize_project_pattern_banks(&mut project);
    ProjectSnapshot {
        project,
        samples: state.session.sample_snapshots(),
    }
}

fn queue_project_edit(
    tx: &ProjectEditSender,
    before: ProjectSnapshot,
    after: ProjectSnapshot,
    status: &'static str,
) -> bool {
    let entry = HistoryEntry {
        before,
        after: after.clone(),
        label: status,
        gesture: None,
    };
    tx.send(ProjectEdit {
        project: after.project,
        samples: after.samples,
        status: status.into(),
        history: Some((HistoryMove::Record, entry)),
    })
}

fn queue_history_target(
    tx: &ProjectEditSender,
    entry: HistoryEntry<ProjectSnapshot>,
    movement: HistoryMove,
) -> bool {
    let snapshot = match movement {
        HistoryMove::Undo => entry.before.clone(),
        HistoryMove::Redo => entry.after.clone(),
        HistoryMove::Record => unreachable!("recording needs an edited snapshot"),
    };
    let status = match movement {
        HistoryMove::Undo => format!("Undid {}", entry.label),
        HistoryMove::Redo => format!("Redid {}", entry.label),
        HistoryMove::Record => unreachable!(),
    };
    tx.send(ProjectEdit {
        project: snapshot.project,
        samples: snapshot.samples,
        status,
        history: Some((movement, entry)),
    })
}

fn sync_command_availability(window: &MainWindow, commands: &CommandState) {
    window.set_can_undo(!commands.project_edit_pending && commands.history.can_undo());
    window.set_can_redo(!commands.project_edit_pending && commands.history.can_redo());
    window.set_channel_clipboard_available(commands.channel_clipboard.is_some());
    window.set_project_edit_pending(commands.project_edit_pending);
}

/// `SegmentedMeter` only changes pixels when its lit-segment count changes.
/// Keeping the raw dB value in the model is useful at that boundary, but
/// rewriting it for an in-between ballistics update just invalidates Slint.
fn meter_segments(db: f32, segments: u32) -> u32 {
    (((db - METER_FLOOR_DB) / -METER_FLOOR_DB).clamp(0.0, 1.0) * segments as f32).ceil() as u32
}

fn meter_display_changed(previous: f32, next: f32, segments: u32) -> bool {
    meter_segments(previous, segments) != meter_segments(next, segments)
}

/// A dynamics display is a continuous readout, not a segmented meter: its dot
/// slides along a transfer curve and its gain reduction fades a glow, so the
/// twelve-segment test the meter bars use would quantize both into visible
/// five-decibel jumps. Half a decibel is finer than either can resolve and
/// still keeps the model row from being rewritten on every idle frame.
const DYNAMICS_DISPLAY_STEP_DB: f32 = 0.5;

fn dynamics_display_changed(previous: f32, next: f32) -> bool {
    (previous - next).abs() >= DYNAMICS_DISPLAY_STEP_DB
}

/// Whether a keyboard edit should act on the piano roll's note selection
/// rather than on the channel.
///
/// Both conditions matter: the roll has to be the visible editor, and it has
/// to have something selected. Without the second, Ctrl+C on the Notes page
/// would silently stop copying the channel.
/// The musical snap/length divisions, in the order `musical-snap-options`
/// lists them. Mirrors `snap-ticks()` in `main.slint`; the two are asserted
/// equal in the tests below.
const MUSICAL_DIVISIONS: [(u32, &str); 11] = [
    (384, "1 Bar"),
    (192, "1/2"),
    (96, "1/4"),
    (64, "1/4T"),
    (48, "1/8"),
    (32, "1/8T"),
    (24, "1/16"),
    (16, "1/16T"),
    (12, "1/32"),
    (8, "1/32T"),
    (6, "1/64"),
];

/// The index in `MUSICAL_DIVISIONS` a length lands on exactly, if any.
fn division_index(ticks: u32) -> i32 {
    MUSICAL_DIVISIONS
        .iter()
        .position(|(division, _)| *division == ticks)
        .map_or(-1, |index| index as i32)
}

/// A note length written the way a musician would say it.
///
/// Exact divisions and their dotted forms get their own name. Anything else
/// -- which is what an unsnapped drag produces -- reads as the largest
/// division that fits plus the remainder in ticks, so the value is never
/// rounded away behind a tidy label.
fn length_text(ticks: u32) -> String {
    if ticks == 0 {
        return String::new();
    }
    for (division, label) in MUSICAL_DIVISIONS {
        if ticks == division {
            return label.to_string();
        }
        // Dotted forms are common enough to deserve a name rather than a
        // remainder; triplet divisions have no dotted convention.
        if !label.ends_with('T') && ticks == division + division / 2 {
            return format!("{label}.");
        }
    }
    match MUSICAL_DIVISIONS
        .iter()
        .find(|(division, _)| *division <= ticks)
    {
        Some((division, label)) => format!("{label} +{}", ticks - division),
        None => format!("{ticks}t"),
    }
}

fn notes_have_focus(window: &MainWindow) -> bool {
    window.get_editor_page() == 1 && window.get_has_note_selection()
}

fn record_project_history(
    commands: &Rc<RefCell<CommandState>>,
    before: ProjectSnapshot,
    state: &Rc<RefCell<UiState>>,
    window: &MainWindow,
    label: &'static str,
) {
    let after = project_snapshot(&state.borrow(), window);
    {
        let mut open = commands.borrow_mut();
        let gesture = open.gesture;
        open.history.record(HistoryEntry {
            before,
            after,
            label,
            gesture,
        });
    }
    sync_command_availability(window, &commands.borrow());
}

fn queue_channel_insert(
    tx: &ProjectEditSender,
    state: &Rc<RefCell<UiState>>,
    window: &MainWindow,
    after: usize,
    clipboard: ChannelClipboard,
    status: &'static str,
) -> bool {
    let before = {
        let state = state.borrow();
        project_snapshot(&state, window)
    };
    let mut project = before.project.clone();
    let mut samples = before.samples.clone();
    if project.channels.len() >= MAX_CHANNELS || after >= project.channels.len() {
        return false;
    }
    let mut channel = clipboard.channel;
    channel
        .notes
        .resize_with(project.pattern_lengths.len(), Vec::new);
    channel
        .automation
        .resize_with(project.pattern_lengths.len(), Vec::new);
    channel.setup.channel.name = copied_channel_name(&project, &channel.setup.channel.name);
    // The song renumbers every route and lane that named a later channel,
    // and points the newcomer's own at its new seat.
    let Some(index) = project.insert_channel(after + 1, channel) else {
        return false;
    };
    samples.insert(index, clipboard.sample);
    project.selected_channel = index as u8;
    queue_project_edit(tx, before, ProjectSnapshot { project, samples }, status)
}

fn queue_channel_delete(
    tx: &ProjectEditSender,
    state: &Rc<RefCell<UiState>>,
    window: &MainWindow,
    index: usize,
    status: &'static str,
) -> bool {
    let before = {
        let state = state.borrow();
        project_snapshot(&state, window)
    };
    let mut project = before.project.clone();
    let mut samples = before.samples.clone();
    if project.channels.len() <= 1 || index >= project.channels.len() {
        return false;
    }
    // The song drops what named this channel and renumbers what named the
    // ones after it; a lane left on the old index would otherwise automate
    // whichever channel slid into the seat.
    if project.remove_channel(index).is_none() {
        return false;
    }
    samples.remove(index);
    project.selected_channel = index.min(project.channels.len() - 1) as u8;
    queue_project_edit(tx, before, ProjectSnapshot { project, samples }, status)
}

/// Duplicates pattern `index`'s length and every channel's notes for it,
/// inserting the copy immediately after. Existing playlist placements (and
/// `current_pattern`) keep pointing at the same pattern *content*, which
/// means shifting any index greater than `index` up by one to follow the
/// insertion; the new clone becomes the selected pattern, mirroring
/// `queue_channel_insert` selecting the pasted/cloned channel.
fn queue_pattern_clone(
    tx: &ProjectEditSender,
    state: &Rc<RefCell<UiState>>,
    window: &MainWindow,
    index: usize,
    status: &'static str,
) -> bool {
    let before = {
        let state = state.borrow();
        project_snapshot(&state, window)
    };
    let mut project = before.project.clone();
    let samples = before.samples.clone();
    if project.pattern_lengths.len() >= MAX_PATTERNS || index >= project.pattern_lengths.len() {
        return false;
    }
    let length = project.pattern_lengths[index];
    project.pattern_lengths.insert(index + 1, length);
    for channel in &mut project.channels {
        let notes = channel.notes[index].clone();
        channel.notes.insert(index + 1, notes);
        let automation = channel.automation[index].clone();
        channel.automation.insert(index + 1, automation);
    }
    for placement in &mut project.playlist {
        if placement.pattern as usize > index {
            placement.pattern += 1;
        }
    }
    project.current_pattern = (index + 1) as u16;
    queue_project_edit(tx, before, ProjectSnapshot { project, samples }, status)
}

/// Removes pattern `index` and every channel's notes for it. Playlist
/// placements on the removed pattern are dropped; placements on later
/// patterns are reindexed down by one to keep pointing at the same
/// content, mirroring the clone side of this pair.
fn queue_pattern_remove(
    tx: &ProjectEditSender,
    state: &Rc<RefCell<UiState>>,
    window: &MainWindow,
    index: usize,
    status: &'static str,
) -> bool {
    let before = {
        let state = state.borrow();
        project_snapshot(&state, window)
    };
    let mut project = before.project.clone();
    let samples = before.samples.clone();
    if project.pattern_lengths.len() <= 1 || index >= project.pattern_lengths.len() {
        return false;
    }
    project.pattern_lengths.remove(index);
    for channel in &mut project.channels {
        channel.notes.remove(index);
        channel.automation.remove(index);
    }
    project
        .playlist
        .retain(|placement| placement.pattern as usize != index);
    for placement in &mut project.playlist {
        if placement.pattern as usize > index {
            placement.pattern -= 1;
        }
    }
    project.current_pattern = index.min(project.pattern_lengths.len() - 1) as u16;
    queue_project_edit(tx, before, ProjectSnapshot { project, samples }, status)
}

/// Empties pattern `index`'s notes and automation on every channel. The
/// pattern itself, its length, and any playlist placements referencing it are
/// untouched -- it still exists and still plays, just silently.
fn queue_pattern_clear(
    tx: &ProjectEditSender,
    state: &Rc<RefCell<UiState>>,
    window: &MainWindow,
    index: usize,
    status: &'static str,
) -> bool {
    let before = {
        let state = state.borrow();
        project_snapshot(&state, window)
    };
    let mut project = before.project.clone();
    let samples = before.samples.clone();
    if index >= project.pattern_lengths.len() {
        return false;
    }
    for channel in &mut project.channels {
        channel.notes[index].clear();
        channel.automation[index].clear();
    }
    queue_project_edit(tx, before, ProjectSnapshot { project, samples }, status)
}

pub struct AppUi {
    window: MainWindow,
    _pump: Timer,
}

/// Push a resolved marker back onto the face. The Slint side moves the marker
/// optimistically as the user drags; when a snap lands somewhere else, this is
/// what makes the control agree with the value that was actually stored.
fn set_marker_property(window: &MainWindow, marker: SampleMarker, value: f32) {
    match marker {
        SampleMarker::Start => window.set_start_pos(value),
        SampleMarker::End => window.set_end_pos(value),
        SampleMarker::LoopStart => window.set_loop_start(value),
        SampleMarker::LoopEnd => window.set_loop_end(value),
    }
}

fn norm_to_time(v: f32) -> f32 {
    v * MAX_TIME_S
}
fn time_to_norm(t: f32) -> f32 {
    t / MAX_TIME_S
}

/// Number of positional parameter fields `EffectSlotRow` carries. Raising it
/// means adding matching `pN` fields to the Slint struct too.
const EFFECT_ROW_PARAMS: usize = 8;

fn effect_kind_index(kind: EffectKind) -> i32 {
    match kind {
        EffectKind::Filter => 0,
        EffectKind::Drive => 1,
        EffectKind::Bitcrush => 2,
        EffectKind::Delay => 3,
        EffectKind::Gate => 4,
        EffectKind::Compressor => 5,
        EffectKind::Limiter => 6,
        EffectKind::Eq => 7,
        EffectKind::Reverb => 8,
        EffectKind::Modulation => 9,
        EffectKind::Plate => 10,
        EffectKind::Buffer => 11,
    }
}

/// Rack units a kind's device face occupies. Devices with more working
/// controls take more width rather than compressing them (docs/UI_DESIGN.md,
/// "Device Rack Layout").
fn effect_kind_units(kind: EffectKind) -> i32 {
    match kind {
        EffectKind::Filter
        | EffectKind::Drive
        | EffectKind::Bitcrush
        | EffectKind::Limiter => 1,
        EffectKind::Buffer => 1,
        EffectKind::Gate | EffectKind::Compressor | EffectKind::Plate => 2,
        EffectKind::Delay => 3,
        EffectKind::Reverb => 3,
        EffectKind::Modulation => 2,
        EffectKind::Eq => 2,
    }
}

fn effect_kind_from_index(index: i32) -> Option<EffectKind> {
    EffectKind::ALL
        .iter()
        .copied()
        .find(|kind| effect_kind_index(*kind) == index)
}

/// Project a slot into the flat, positional row the rack renders. Values are
/// normalized through the kind's descriptor table in descriptor order, so a
/// new effect kind needs a device face and no change here.
/// The effect presets saved for `kind`, in the order a row's menu lists them
/// and the order a menu index resolves back through.
fn effect_presets_of_kind(
    presets: &[PresetSummary],
    kind: EffectKind,
) -> impl Iterator<Item = &PresetSummary> {
    presets
        .iter()
        .filter(move |preset| preset.kind == PresetKind::Effect(kind))
}

fn effect_slot_row(slot: &EffectSlotState, presets: &[PresetSummary]) -> EffectSlotRow {
    let kind = slot.kind();
    let preset_options: Vec<slint::SharedString> = effect_presets_of_kind(presets, kind)
        .map(preset_menu_label)
        .collect();
    let mut p = [0.0f32; EFFECT_ROW_PARAMS];
    for (index, descriptor) in kind
        .descriptors()
        .iter()
        .take(EFFECT_ROW_PARAMS)
        .enumerate()
    {
        if let Some(natural) = slot.params.get(descriptor.id) {
            p[index] = descriptor.to_normalized(natural);
        }
    }
    debug_assert!(
        kind.descriptors().len() <= EFFECT_ROW_PARAMS,
        "{} has more parameters than EffectSlotRow can carry",
        kind.label()
    );
    if let Some(delay) = slot.params.delay() {
        p[6] = if delay.tempo_sync { 1.0 } else { 0.0 };
        p[7] = delay.time_division.to_index() as f32;
    }
    let mut eq_band_data = Vec::new();
    if let Some(eq) = slot.params.eq() {
        for band in eq.bands {
            eq_band_data.extend_from_slice(&[
                (band.frequency_hz / 20.0).ln() / 1000.0_f32.ln(),
                (band.gain_db + 18.0) / 36.0,
                band.q,
                if band.enabled { 1.0 } else { 0.0 },
                band.kind.to_index() as f32,
            ]);
        }
        eq_band_data.extend_from_slice(&[
            (eq.high_pass.frequency_hz / 20.0).ln() / 1000.0_f32.ln(),
            eq.high_pass.q,
            if eq.high_pass.enabled { 1.0 } else { 0.0 },
            eq.high_pass.slope.to_index() as f32,
            (eq.low_pass.frequency_hz / 20.0).ln() / 1000.0_f32.ln(),
            eq.low_pass.q,
            if eq.low_pass.enabled { 1.0 } else { 0.0 },
            eq.low_pass.slope.to_index() as f32,
        ]);
    }
    EffectSlotRow {
        kind: effect_kind_index(kind),
        units: effect_kind_units(kind),
        preset_options: ModelRc::from(Rc::new(VecModel::from(preset_options))),
        bypassed: slot.bypassed,
        p0: p[0],
        p1: p[1],
        p2: p[2],
        p3: p[3],
        p4: p[4],
        p5: p[5],
        p6: p[6],
        p7: p[7],
        modulation_depths: Vec::<f32>::new().as_slice().into(),
        modulation_allowed: Vec::<bool>::new().as_slice().into(),
        modulation_offsets: Vec::<f32>::new().as_slice().into(),
        modulation_route_counts: Vec::<i32>::new().as_slice().into(),
        eq_band_data: eq_band_data.as_slice().into(),
        eq_spectrum_data: Vec::<f32>::new().as_slice().into(),
        eq_analyzer_enabled: slot.params.eq().is_some_and(|eq| eq.analyzer_enabled),
        wet_dry: slot.wet_dry,
        input_trim_db: linear_to_db(slot.input_trim),
        output_trim_db: linear_to_db(slot.output_trim),
        input_left_db: METER_FLOOR_DB,
        input_right_db: METER_FLOOR_DB,
        output_left_db: METER_FLOOR_DB,
        output_right_db: METER_FLOOR_DB,
        buffer_collisions: 0,
        detector_db: METER_FLOOR_DB,
        gain_reduction_db: 0.0,
    }
}

/// The fixed debug events the buffer device face fires, in the order its
/// buttons appear. These stand in for the note layer and per-step parameter
/// locks that will eventually produce tuples, and deliberately go through the
/// same `TriggerBuffer` command those will: jump back a beat, and stutter the
/// last sixteenth eight times. Reverse is not here — it is held rather than
/// latched, so it is built at the press site with a `Gate` duration.
fn debug_buffer_event(index: i32) -> Option<BufferEvent> {
    let event = match index {
        0 => BufferEvent {
            offset_beats: -1.0,
            ..BufferEvent::live()
        },
        2 => BufferEvent {
            offset_beats: -0.0625,
            window_beats: Some(0.0625),
            repeat: Some(8),
            ..BufferEvent::live()
        },
        _ => return None,
    };
    Some(event)
}

/// The held reverse: runs backward from the moment of the press and loops
/// over the two bars behind it, so a long hold repeats recent material
/// instead of running until it exhausts the retained history. `Gate` is what
/// makes the release meaningful — a latching event would ignore it.
fn held_reverse_event() -> BufferEvent {
    BufferEvent {
        offset_beats: 0.0,
        rate: -1.0,
        // Four beats to the bar.
        window_beats: Some(8.0),
        repeat: None,
        duration: BufferDuration::Gate,
        crossfade_ms: BufferEvent::live().crossfade_ms,
    }
}

fn loop_mode_from_int(i: i32) -> LoopMode {
    match i {
        1 => LoopMode::Forward,
        2 => LoopMode::Pingpong,
        _ => LoopMode::Off,
    }
}

fn voice_mode_from_int(i: i32) -> VoiceMode {
    if i == 1 {
        VoiceMode::Gate
    } else {
        VoiceMode::OneShot
    }
}

fn retrigger_mode_from_int(i: i32) -> RetriggerMode {
    if i == 1 {
        RetriggerMode::Layer
    } else {
        RetriggerMode::Restart
    }
}

fn device_kind_from_int(value: i32) -> DeviceKind {
    match value {
        1 => DeviceKind::DrumSynth,
        2 => DeviceKind::MonoSynth,
        3 => DeviceKind::PolySynth,
        4 => DeviceKind::MlM1,
        5 => DeviceKind::MlP8,
        6 => DeviceKind::Ds01,
        _ => DeviceKind::Sampler,
    }
}

fn device_kind_to_int(kind: DeviceKind) -> i32 {
    match kind {
        DeviceKind::Sampler => 0,
        DeviceKind::DrumSynth => 1,
        DeviceKind::MonoSynth => 2,
        DeviceKind::PolySynth => 3,
        DeviceKind::MlM1 => 4,
        DeviceKind::MlP8 => 5,
        DeviceKind::Ds01 => 6,
    }
}

fn drum_mode_from_int(value: i32) -> DrumMode {
    match value {
        1 => DrumMode::Snare,
        2 => DrumMode::Hat,
        _ => DrumMode::Kick,
    }
}

fn drum_mode_to_int(mode: DrumMode) -> i32 {
    match mode {
        DrumMode::Kick => 0,
        DrumMode::Snare => 1,
        DrumMode::Hat => 2,
    }
}

fn kick_character_from_int(value: i32) -> KickCharacter {
    match value {
        0 => KickCharacter::Sub,
        1 => KickCharacter::Punch,
        2 => KickCharacter::Deep,
        4 => KickCharacter::Dnb,
        _ => KickCharacter::Kit,
    }
}

fn kick_character_to_int(character: KickCharacter) -> i32 {
    match character {
        KickCharacter::Sub => 0,
        KickCharacter::Punch => 1,
        KickCharacter::Deep => 2,
        KickCharacter::Kit => 3,
        KickCharacter::Dnb => 4,
    }
}

fn snare_character_from_int(value: i32) -> SnareCharacter {
    match value {
        1 => SnareCharacter::Snap,
        2 => SnareCharacter::Power,
        3 => SnareCharacter::Clap,
        4 => SnareCharacter::Rim,
        _ => SnareCharacter::Pop,
    }
}

fn snare_character_to_int(character: SnareCharacter) -> i32 {
    match character {
        SnareCharacter::Pop => 0,
        SnareCharacter::Snap => 1,
        SnareCharacter::Power => 2,
        SnareCharacter::Clap => 3,
        SnareCharacter::Rim => 4,
    }
}

fn hat_character_from_int(value: i32) -> HatCharacter {
    match value {
        0 => HatCharacter::Soft,
        2 => HatCharacter::Metal,
        3 => HatCharacter::Sizzle,
        4 => HatCharacter::Trash,
        _ => HatCharacter::Tight,
    }
}

fn hat_character_to_int(character: HatCharacter) -> i32 {
    match character {
        HatCharacter::Soft => 0,
        HatCharacter::Tight => 1,
        HatCharacter::Metal => 2,
        HatCharacter::Sizzle => 3,
        HatCharacter::Trash => 4,
    }
}

fn osc_wave_from_int(value: i32) -> OscWave {
    match value {
        0 => OscWave::Sine,
        1 => OscWave::Triangle,
        3 => OscWave::Pulse,
        _ => OscWave::Saw,
    }
}

/// Push one ML-P8's authored internal routes onto its face.
///
/// The row model carries the durable id rather than the position, so every
/// edit the face sends back names the route it was drawn for even if a
/// neighbour has since been removed. Destination names come from core's own
/// descriptor table, so the face holds no second copy of the parameter list.
fn refresh_mlp8_routes(window: &MainWindow, routes: &mooloop_core::MlP8Routes) {
    let rows: Vec<MlP8RouteRow> = routes
        .iter()
        .map(|route| MlP8RouteRow {
            id: i32::from(route.id),
            source: route.source.to_index(),
            dest: route.dest.slot().unwrap_or_default() as i32,
            amount: route.amount,
            dest_name: route.dest.label().into(),
            bipolar: route.source.is_bipolar(),
        })
        .collect();
    window.set_mlp8_routes(ModelRc::from(Rc::new(VecModel::from(rows))));
    let full = routes.len() >= mooloop_core::MLP8_MAX_ROUTES;
    window.set_mlp8_routes_full(full);
    window.set_mlp8_routes_status(
        format!("{} of {}", routes.len(), mooloop_core::MLP8_MAX_ROUTES).into(),
    );
}

/// The two picker vocabularies, set once: they are properties of the device,
/// not of the patch, so nothing that changes while editing can move them.
fn install_mlp8_route_vocabularies(window: &MainWindow) {
    let sources: Vec<SharedString> = mooloop_core::MlP8ModSource::ALL
        .iter()
        .map(|source| SharedString::from(source.label()))
        .collect();
    let dests: Vec<SharedString> = mooloop_core::MlP8ModDest::ALL
        .iter()
        .map(|dest| SharedString::from(dest.label()))
        .collect();
    window.set_mlp8_route_source_names(ModelRc::from(Rc::new(VecModel::from(sources))));
    window.set_mlp8_route_dest_names(ModelRc::from(Rc::new(VecModel::from(dests))));
}

fn osc_wave_to_int(wave: OscWave) -> i32 {
    match wave {
        OscWave::Sine => 0,
        OscWave::Triangle => 1,
        OscWave::Saw => 2,
        OscWave::Pulse => 3,
    }
}

fn lfo_wave_from_int(value: i32) -> LfoWave {
    match value {
        1 => LfoWave::Triangle,
        2 => LfoWave::Saw,
        3 => LfoWave::Square,
        4 => LfoWave::Random,
        _ => LfoWave::Sine,
    }
}

fn lfo_wave_to_int(wave: LfoWave) -> i32 {
    match wave {
        LfoWave::Sine => 0,
        LfoWave::Triangle => 1,
        LfoWave::Saw => 2,
        LfoWave::Square => 3,
        LfoWave::Random => 4,
    }
}

/// Length of an id-indexed parameter array. Ids are dense and small in
/// practice, but a gap costs one unused entry rather than a wrong lookup.
/// Which parameters accept modulation, indexed by descriptor id. The policy
/// lives in `ModDestinationDescriptor`, so a device opts a control in or out
/// through its own descriptor rather than through a UI special case. An id
/// with no descriptor stays `false`, which is the safe answer.
fn descriptor_policies(descriptors: &[ParamDescriptor]) -> ModelRc<bool> {
    let mut allowed = vec![false; descriptor_slots(descriptors)];
    for descriptor in descriptors {
        allowed[descriptor.id as usize] = ModDestinationDescriptor::for_param(descriptor).allowed;
    }
    allowed.as_slice().into()
}

/// How many routes land on each parameter, indexed by descriptor id. Drawn as
/// dots on the knob, so a control says how many sources reach it without the
/// shelf being open. Counted from every route, including ones whose
/// destination currently refuses modulation: the assignment is still authored
/// work the user made and can remove.
fn descriptor_route_counts(
    rack: &ModRack,
    descriptors: &[ParamDescriptor],
    address: impl Fn(u32) -> ParamAddr,
) -> ModelRc<i32> {
    let mut counts = vec![0i32; descriptor_slots(descriptors)];
    for descriptor in descriptors {
        counts[descriptor.id as usize] = rack
            .destinations()
            .filter(|destination| *destination == address(descriptor.id))
            .count() as i32;
    }
    counts.as_slice().into()
}

/// Starts diagnostic logging, honouring the saved preference for whether to
/// write a file.
///
/// Separate from [`AppUi::new`] and public so the binary can call it first:
/// bringing up the audio engine is one of the things worth having in the log,
/// and it happens before there is any UI to attach to.
pub fn start_logging() {
    init_logging(UiSettings::load_or_default().general.log_to_file);
}

/// Brings up diagnostic logging for the run and says what build is running.
///
/// The console threshold comes from `MOOLOOP_LOG` (`error`, `warn`, `info`, or
/// `debug`), falling back to the older `MOOLOOP_DEBUG=1`, which now means the
/// same as `MOOLOOP_LOG=debug`. With neither set it stays at `info`, so a run
/// started from a terminal still reports what it opened, saved, and repaired
/// without being asked.
///
/// `to_file` mirrors everything, `debug` included, into [`settings::log_path`].
/// A file that cannot be opened is reported and then dropped: no preference is
/// worth refusing to start over.
fn init_logging(to_file: bool) {
    let level = match std::env::var("MOOLOOP_LOG") {
        Ok(name) => Level::parse(&name).unwrap_or_else(|| {
            eprintln!("mooloop: MOOLOOP_LOG={name:?} is not a level, using info");
            Level::Info
        }),
        Err(_) if std::env::var_os("MOOLOOP_DEBUG").is_some() => Level::Debug,
        Err(_) => Level::Info,
    };
    mooloop_core::log::set_level(level);
    if to_file {
        let path = settings::log_path();
        if let Err(error) = mooloop_core::log::start_file(&path, &build_description()) {
            eprintln!("mooloop: could not write the log to {}: {error}", path.display());
        }
    }
    // A panic is the one failure with no dialog and no status bar to carry it,
    // which makes it the one that most needs to reach the file. Chained rather
    // than replaced, so the default hook still prints its message and
    // backtrace; this only adds a copy to wherever else records are going.
    let inherited = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log_error!("app", "panic: {info}");
        inherited(info);
    }));
    log_info!("app", "mooloop {} starting", build_description());
    log_info!(
        "app",
        "settings: {}, log level: {level:?}",
        settings::config_dir().display()
    );
}

/// Which build this is, for the top of a log someone sends back. The profile
/// matters as much as the version: a report about realtime behaviour means
/// something different from a debug build than from a release one.
fn build_description() -> String {
    format!(
        "{} ({} build, document format {})",
        env!("CARGO_PKG_VERSION"),
        if cfg!(debug_assertions) {
            "development"
        } else {
            "release"
        },
        mooloop_project::FORMAT_VERSION,
    )
}

/// The note under a grid position, if any.
///
/// Slint's binding-loop checker rejects a self-recursive `pure function`, so a
/// variable-length note list cannot be scanned from `.slint` at all. The piano
/// roll's single grid hit area calls this instead. Scans back to front so an
/// overlap resolves to the note drawn on top, which is the one the user sees.
pub fn note_hit_test(notes: &[NoteCell], tick: i32, midi_note: i32) -> NoteHit {
    notes
        .iter()
        .rev()
        .find(|cell| {
            cell.note == midi_note
                && tick >= cell.start_tick
                && tick < cell.start_tick + cell.duration_ticks
        })
        .map(|cell| NoteHit {
            found: true,
            id: cell.id,
            start_tick: cell.start_tick,
            duration_ticks: cell.duration_ticks,
            selected: cell.selected,
        })
        .unwrap_or_default()
}

fn rack_cell(notes: &[NoteEvent], step: usize) -> StepCell {
    let start = (step as u32).saturating_mul(TICKS_PER_STEP);
    let end = start.saturating_add(TICKS_PER_STEP);
    let mut substeps = 0;
    let mut onsets = 0;
    let mut velocity = 0;
    for note in notes {
        let note_end = note.end_tick();
        if note.start_tick >= end || note_end <= start {
            continue;
        }

        let overlap_start = note.start_tick.max(start);
        let overlap_end = note_end.min(end);
        let first = ((overlap_start - start) / TICKS_PER_64TH).min(3);
        let last = ((overlap_end - start - 1) / TICKS_PER_64TH).min(3);
        for substep in first..=last {
            substeps |= 1 << substep;
        }
        // Only a note that begins inside this sixteenth is struck here; one
        // that merely runs through it is being held.
        if note.start_tick >= start && note.start_tick < end {
            onsets |= 1 << ((note.start_tick - start) / TICKS_PER_64TH).min(3);
        }
        velocity = velocity.max(i32::from(note.velocity));
    }
    StepCell {
        active: substeps != 0,
        velocity,
        substeps,
        onsets,
    }
}

/// A normalized lane value rendered in the destination's own units. Value
/// only: what the number means is already on the lane header, and the status
/// bar carries the explanation.
fn format_param_value(descriptor: &ParamDescriptor, normalized: f32) -> String {
    let natural = descriptor.from_normalized(normalized);
    let magnitude = natural.abs();
    let text = if magnitude >= 10_000.0 {
        format!("{:.2}k", natural / 1_000.0)
    } else if magnitude >= 100.0 {
        format!("{natural:.0}")
    } else if magnitude >= 10.0 {
        format!("{natural:.1}")
    } else {
        format!("{natural:.2}")
    };
    if descriptor.unit.is_empty() {
        text
    } else {
        format!("{text} {}", descriptor.unit)
    }
}

/// The DS-01 face's per-id arrays.
///
/// The face is indexed by descriptor id rather than declaring a property per
/// parameter, because ninety-two properties would be a second copy of the
/// parameter table written out by hand — which is the thing the device exists
/// not to have. Rust owns the ranges, the curves and the formatting; the face
/// owns the layout.
struct Ds01FaceValues {
    /// Normalized `0..1`, which is the space a route and an automation lane
    /// both work in, and the one space where a knob's travel means the same
    /// thing on a linear parameter and an exponential one.
    values: Vec<f32>,
    defaults: Vec<f32>,
    texts: Vec<SharedString>,
    /// `0` for a continuous parameter, the position count for a stepped one.
    steps: Vec<i32>,
}

/// One past the highest DS-01 id, so an array indexed by id is long enough
/// for every one of them.
fn ds01_array_len() -> usize {
    ds01::DESCRIPTORS
        .iter()
        .map(|d| d.id as usize + 1)
        .max()
        .unwrap_or(0)
}

/// A stepped DS-01 control's label, or `None` where the number is the answer.
///
/// The enums carry their own labels in `mooloop_core`, so this is a mapping
/// from id to enum rather than a second list of names.
fn ds01_step_label(id: u32, params: &Ds01Params) -> Option<&'static str> {
    Some(match id {
        ds01::PARAM_RETRIGGER => params.retrigger.label(),
        ds01::PARAM_NOISE_COLOR => params.noise_color.label(),
        ds01::PARAM_CHARACTER => params.character.label(),
        id if id == ds01::PARAM_AMP_ENV_BASE + ds01::ENV_OFFSET_GATE => {
            if params.amp.gate { "GATE" } else { "ONE" }
        }
        id if id == ds01::PARAM_NOISE_ENV_BASE + ds01::ENV_OFFSET_GATE => {
            if params.noise_env.gate { "GATE" } else { "ONE" }
        }
        id if id == ds01::PARAM_MOD_ENV_BASE + ds01::ENV_OFFSET_GATE => {
            if params.mod_env.gate { "GATE" } else { "ONE" }
        }
        _ => return None,
    })
}

/// A DS-01 value as its control reads it.
///
/// A stepped control without an enum behind it is a count — a choke group, a
/// partial, a repeat, a bit depth — so it is shown as one. The shared
/// formatter's two decimal places are right for a continuous parameter and
/// wrong for "16 bits".
fn ds01_text(
    descriptor: &ParamDescriptor,
    params: &Ds01Params,
    normalized: f32,
) -> SharedString {
    if let Some(label) = ds01_step_label(descriptor.id, params) {
        return label.into();
    }
    if matches!(descriptor.curve, ParamCurve::Stepped(_)) {
        let natural = descriptor.from_normalized(normalized).round();
        return if descriptor.unit.is_empty() {
            format!("{natural:.0}").into()
        } else {
            format!("{natural:.0} {}", descriptor.unit).into()
        };
    }
    format_param_value(descriptor, normalized).into()
}

fn ds01_face_values(params: &Ds01Params) -> Ds01FaceValues {
    let len = ds01_array_len();
    let mut out = Ds01FaceValues {
        values: vec![0.0; len],
        defaults: vec![0.0; len],
        texts: vec![SharedString::new(); len],
        steps: vec![0; len],
    };
    for descriptor in ds01::DESCRIPTORS.iter() {
        let index = descriptor.id as usize;
        let natural = ds01::get(params, descriptor.id).unwrap_or(descriptor.default);
        let normalized = descriptor.to_normalized(natural);
        out.values[index] = normalized;
        out.defaults[index] = descriptor.to_normalized(descriptor.default);
        out.texts[index] = ds01_text(descriptor, params, normalized);
        if let ParamCurve::Stepped(count) = descriptor.curve {
            out.steps[index] = i32::from(count);
        }
    }
    out
}

/// How long the scopes are drawn over: the longest thing in the patch that
/// ends.
///
/// Auto-scaled rather than fixed, because a fixed window — v1's 300 ms —
/// draws a 5 ms hat as a single spike and clips a 4 s ride entirely, which
/// makes the display useless at both ends of the range this instrument is
/// supposed to reach.
fn ds01_span_seconds(p: &Ds01Params) -> f32 {
    let env = |e: &mooloop_core::Ds01EnvParams| e.attack + e.hold + e.decay;
    let longest = env(&p.amp)
        .max(env(&p.noise_env))
        .max(env(&p.mod_env))
        .max(p.pitch.attack + p.pitch.decay)
        .max(p.body_decay)
        .max(0.02);
    // Headroom past the longest contour, so the handle that ends it is not
    // pinned to the right edge. Without it the longest envelope in a patch
    // sits at fraction 1.0 and can only ever be dragged shorter — which is
    // the one envelope a musician is most likely to be lengthening.
    longest * SPAN_HEADROOM
}

/// How much of a scope sits past the longest contour in the patch.
const SPAN_HEADROOM: f32 = 1.25;

fn ds01_contour(
    attack: f32,
    hold: f32,
    decay: f32,
    curve: f32,
    sustain: f32,
    height: f32,
    span: f32,
) -> Ds01Contour {
    Ds01Contour {
        attack: attack / span,
        hold: hold / span,
        decay: (decay / span).max(0.001),
        curve,
        sustain,
        height,
        active: height > 0.001,
    }
}

/// The four contours the face draws, in the order it expects: amp, pitch,
/// noise, mod.
fn ds01_contours(p: &Ds01Params, span: f32) -> Vec<Ds01Contour> {
    vec![
        ds01_contour(p.amp.attack, p.amp.hold, p.amp.decay, p.amp.curve, p.amp.sustain, 1.0, span),
        // The pitch envelope is the one contour drawn at less than full
        // height: its depth *is* its height, which is what makes the depth
        // draggable on the same curve as its times.
        ds01_contour(
            p.pitch.attack,
            0.0,
            p.pitch.decay,
            p.pitch.curve,
            0.0,
            (p.pitch.depth.abs() / 60.0).clamp(0.0, 1.0),
            span,
        ),
        ds01_contour(
            p.noise_env.attack,
            p.noise_env.hold,
            p.noise_env.decay,
            p.noise_env.curve,
            p.noise_env.sustain,
            1.0,
            span,
        ),
        ds01_contour(
            p.mod_env.attack,
            p.mod_env.hold,
            p.mod_env.decay,
            p.mod_env.curve,
            p.mod_env.sustain,
            1.0,
            span,
        ),
    ]
}

/// A span, in the units a drum patch is read in.
fn ds01_format_span(seconds: f32) -> String {
    if seconds < 1.0 {
        format!("{:.0} ms", seconds * 1000.0)
    } else {
        format!("{seconds:.2} s")
    }
}

/// Push a DS-01 patch into the face.
///
/// Public because the face is a view of a patch and nothing else: handing it
/// one is the whole of showing it, which is what lets a snapshot test render
/// the device without standing up an engine.
///
/// Not cheap — it renders a hit through the production voice path — so the
/// caller decides when it is worth doing. The editor refresh only calls it
/// for a DS-01 channel, and a knob move goes through `touch_ds01_param` and
/// the debounce instead.
pub fn refresh_ds01(window: &MainWindow, params: &Ds01Params) {
    let face = ds01_face_values(params);
    window.set_ds01_values(face.values.as_slice().into());
    window.set_ds01_defaults(face.defaults.as_slice().into());
    window.set_ds01_value_texts(face.texts.as_slice().into());
    window.set_ds01_step_counts(face.steps.as_slice().into());
    refresh_ds01_contours(window, params);
    sync_ds01_preview(window, params);
    sync_ds01_burst_ticks(window, params);
}

/// Where a burst's impulses fall, as fractions of the burst's own length.
///
/// Its own axis rather than the scopes' span: a twelve-millisecond flam
/// inside a four-second ride would be four ticks in the first pixel, which
/// shows the spacing and the spread less well than not drawing them.
fn sync_ds01_burst_ticks(window: &MainWindow, params: &Ds01Params) {
    let offsets = Ds01::burst_offsets(*params, 48_000);
    let last = offsets.last().copied().unwrap_or(0.0);
    let ticks: Vec<f32> = if last <= 0.0 {
        vec![0.0]
    } else {
        offsets.iter().map(|at| at / last).collect()
    };
    window.set_ds01_burst_ticks(ticks.as_slice().into());
}

/// The rendered hit, over the same span the scopes are drawn on.
fn sync_ds01_preview(window: &MainWindow, params: &Ds01Params) {
    let (minimums, maximums) =
        Ds01::preview_waveform(*params, DS01_PREVIEW_BINS, ds01_span_seconds(params));
    window.set_ds01_preview_minimums(minimums.as_slice().into());
    window.set_ds01_preview_maximums(maximums.as_slice().into());
}

/// The scopes, and the span the header states them over.
fn refresh_ds01_contours(window: &MainWindow, params: &Ds01Params) {
    let span = ds01_span_seconds(params);
    window.set_ds01_contours(ds01_contours(params, span).as_slice().into());
    window.set_ds01_body_decay_fraction((params.body_decay / span).clamp(0.0, 1.0));
    window.set_ds01_span_text(
        format!("every scope  0 – {}", ds01_format_span(span)).into(),
    );
}

/// Where an envelope segment starts, in seconds, so a handle dropped at a
/// point on the scope becomes the length of its own segment rather than the
/// distance from the origin.
fn ds01_segment_start(p: &Ds01Params, id: u32) -> f32 {
    let env = |e: &mooloop_core::Ds01EnvParams, offset: u32| match offset {
        ds01::ENV_OFFSET_HOLD => e.attack,
        ds01::ENV_OFFSET_DECAY => e.attack + e.hold,
        _ => 0.0,
    };
    match id {
        id if (ds01::PARAM_AMP_ENV_BASE..ds01::PARAM_AMP_ENV_BASE + ds01::ENV_BLOCK)
            .contains(&id) =>
        {
            env(&p.amp, id - ds01::PARAM_AMP_ENV_BASE)
        }
        id if (ds01::PARAM_NOISE_ENV_BASE..ds01::PARAM_NOISE_ENV_BASE + ds01::ENV_BLOCK)
            .contains(&id) =>
        {
            env(&p.noise_env, id - ds01::PARAM_NOISE_ENV_BASE)
        }
        id if (ds01::PARAM_MOD_ENV_BASE..ds01::PARAM_MOD_ENV_BASE + ds01::ENV_BLOCK)
            .contains(&id) =>
        {
            env(&p.mod_env, id - ds01::PARAM_MOD_ENV_BASE)
        }
        ds01::PARAM_PITCH_DECAY => p.pitch.attack,
        _ => 0.0,
    }
}

/// Which column a parameter belongs to: 0 tone, 1 noise, 2 body, 3 amp.
///
/// The face dims the columns nobody is touching, and editing a control is the
/// touch that matters — so focus follows the parameter rather than the
/// pointer, and the id already says which column it is drawn in. The pitch
/// envelope answers "tone" because that is the scope it is drawn on; the
/// globals, the burst and the shaper answer nothing, because they are not
/// columns and moving one should not blank the emphasis.
fn ds01_column(id: u32) -> Option<i32> {
    Some(match id {
        10..=19 => 0,
        20..=29 => 1,
        30..=39 => 2,
        40..=49 => 3,
        50..=59 => 0,
        60..=69 => 1,
        70..=79 => 3,
        _ => return None,
    })
}

/// Whether moving this parameter changes what the scopes draw.
///
/// The envelope blocks and the body's ring are the whole of it, and they are
/// exactly the parameters `ds01::is_latched` names plus the body decay — the
/// contours are a drawing of the shape a hit latches.
fn ds01_redraws_contours(id: u32) -> bool {
    ds01::is_latched(id) || id == ds01::PARAM_BODY_DECAY
}

/// Update the one row a knob moved, rather than rebuilding a hundred and
/// thirty of them per drag sample.
fn touch_ds01_param(window: &MainWindow, params: &Ds01Params, id: u32) {
    let Some(descriptor) = ds01::descriptor(id) else {
        return;
    };
    let index = id as usize;
    let natural = ds01::get(params, id).unwrap_or(descriptor.default);
    let normalized = descriptor.to_normalized(natural);
    let values = window.get_ds01_values();
    if index < values.row_count() {
        values.set_row_data(index, normalized);
    }
    let texts = window.get_ds01_value_texts();
    if index < texts.row_count() {
        texts.set_row_data(index, ds01_text(descriptor, params, normalized));
    }
    if let Some(column) = ds01_column(id) {
        window.set_ds01_focused_column(column);
    }
    if ds01_redraws_contours(id) {
        refresh_ds01_contours(window, params);
    }
}

fn note_cell(note: NoteEvent, selected_ids: &HashSet<NoteId>) -> NoteCell {
    NoteCell {
        id: note.id as i32,
        start_tick: note.start_tick as i32,
        duration_ticks: note.duration_ticks as i32,
        note: note.note as i32,
        velocity: note.velocity as i32,
        selected: selected_ids.contains(&note.id),
    }
}

/// Shared UI state handed to the callback closures.
struct UiState {
    /// Everything the application would still be if the window went away.
    session: Session,
    rows: Rc<VecModel<ChannelRow>>,
    step_models: Vec<Rc<VecModel<StepCell>>>,
    note_model: Rc<VecModel<NoteCell>>,
    automation_point_model: Rc<VecModel<AutomationPointCell>>,
    automation_target_model: Rc<VecModel<AutomationTargetRow>>,
    playlist_model: Rc<VecModel<PlaylistClip>>,
    waveform_model: Rc<VecModel<f32>>,
    /// Slice boundaries of the selected channel, normalized against the
    /// published buffer so they ride the same `to-view` zoom the waveform and
    /// every other marker already go through.
    slice_model: Rc<VecModel<f32>>,
    /// Normalized position of every currently active sampler voice on the
    /// selected channel, refreshed each pump tick. Empty when idle, when a
    /// different device kind is selected, or while editing a bus.
    playhead_model: Rc<VecModel<f32>>,
    effect_slot_model: Rc<VecModel<EffectSlotRow>>,
    /// Existing modulation sources and routes for the selected channel. They
    /// are models rather than fixed slot properties because the shelf must
    /// show a collection, not four vacant bays.
    modulation_source_model: Rc<VecModel<ModulationSourceRow>>,
    modulation_route_model: Rc<VecModel<ModulationRouteRow>>,
    mixer_strip_model: Rc<VecModel<MixerStripRow>>,
    /// Flattened sample-browser tree, rebuilt whenever locations or folder
    /// expansion change.
    browser_rows: Rc<VecModel<BrowserRow>>,
}

impl UiState {
    fn replace_project(
        &mut self,
        project: &Project,
        samples: &[Option<Arc<SampleData>>],
        window: &MainWindow,
    ) {
        self.session.replace_project(project, samples);
        self.step_models = self
            .session.channels
            .iter()
            .map(|channel| {
                Rc::new(VecModel::from(
                    (0..self.session.pattern_lengths[self.session.current_pattern])
                        .map(|step| rack_cell(&channel.notes[self.session.current_pattern], step))
                        .collect::<Vec<_>>(),
                ))
            })
            .collect();
        let rows: Vec<ChannelRow> = self
            .session.channels
            .iter()
            .enumerate()
            .map(|(index, channel)| ChannelRow {
                name: channel.name.as_str().into(),
                muted: channel.muted,
                volume_db: linear_to_db(channel.volume),
                pan: channel.pan,
                selected: index == self.session.selected,
                bus: channel.bus as i32,
                steps: ModelRc::from(self.step_models[index].clone()),
            })
            .collect();
        self.rows.set_vec(rows);
        window.set_bpm(project.bpm.into());
        window.set_swing_percent(project.swing_percent.into());
        window.set_song_mode(self.session.song_mode);
        window.set_current_pattern(self.session.current_pattern as i32);
        window.set_pattern_count(self.session.pattern_lengths.len() as i32);
        window.set_pattern_length(self.session.pattern_lengths[self.session.current_pattern] as i32);
        window.set_selected_channel(self.session.selected as i32);
        self.sync_row_flags();
        self.sync_mixer(window);
        self.sync_playlist(window);
        self.refresh_editor(window);
    }

    fn update_document_title(&self, window: &MainWindow) {
        let name = self
            .session.bundle_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .and_then(|name| name.to_str())
            .unwrap_or("Untitled");
        window.set_document_title(
            if self.session.dirty {
                format!("{name} * - mooloop")
            } else {
                format!("{name} - mooloop")
            }
            .into(),
        );
    }
    /// Push the selected/muted flags of every row to the rack model.
    fn sync_row_flags(&self) {
        for (i, ch) in self.session.channels.iter().enumerate() {
            if let Some(mut row) = self.rows.row_data(i) {
                row.selected = i == self.session.selected;
                row.muted = ch.muted;
                row.volume_db = linear_to_db(ch.volume);
                row.pan = ch.pan;
                row.bus = ch.bus as i32;
                row.name = ch.name.as_str().into();
                self.rows.set_row_data(i, row);
            }
        }
    }

    /// Rebuild every channel's step model from `pattern`.
    fn show_pattern(&self, pattern: usize) {
        let length = self.session.pattern_lengths[pattern];
        for (i, ch) in self.session.channels.iter().enumerate() {
            let cells: Vec<StepCell> = (0..length)
                .map(|step| rack_cell(&ch.notes[pattern], step))
                .collect();
            self.step_models[i].set_vec(cells);
        }
    }

    fn refresh_rack_cell(&self, channel: usize, step: usize) {
        let notes = &self.session.channels[channel].notes[self.session.current_pattern];
        self.step_models[channel].set_row_data(step, rack_cell(notes, step));
    }

    /// Push the current pattern's name and the full pattern menu to the
    /// window. An empty name falls back to `Pattern N` in the menu.
    fn sync_pattern_menu(&self, window: &MainWindow) {
        let options: Vec<slint::SharedString> = self
            .session.pattern_names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let label = if name.is_empty() {
                    format!("Pattern {}", i + 1)
                } else {
                    name.clone()
                };
                format!("{:02}  {label}", i + 1).into()
            })
            .collect();
        window.set_pattern_menu_options(ModelRc::from(Rc::new(VecModel::from(options))));
        let current = self
            .session.pattern_names
            .get(self.session.current_pattern)
            .cloned()
            .unwrap_or_default();
        window.set_current_pattern_name(current.into());
    }

    fn sync_generator_preset_menu(&self, window: &MainWindow) {
        let options: Vec<slint::SharedString> = self
            .session.generator_presets
            .iter()
            .map(preset_menu_label)
            .collect();
        window.set_generator_preset_options(ModelRc::from(Rc::new(VecModel::from(options))));
    }

    fn sync_channel_preset_menu(&self, window: &MainWindow) {
        let options: Vec<slint::SharedString> =
            self.session.channel_presets.iter().map(preset_menu_label).collect();
        window.set_channel_preset_options(ModelRc::from(Rc::new(VecModel::from(options))));
    }

    fn sync_playlist(&self, window: &MainWindow) {
        let clips: Vec<PlaylistClip> = self
            .session.playlist
            .iter()
            .filter_map(|placement| {
                self.session.pattern_lengths
                    .get(placement.pattern as usize)
                    .map(|length| PlaylistClip {
                        pattern: placement.pattern as i32,
                        start_tick: placement.start_tick as i32,
                        length_steps: *length as i32,
                    })
            })
            .collect();
        self.playlist_model.set_vec(clips);
        let song_length = self.session.song_length_ticks();
        window.set_playlist_song_length_ticks(song_length as i32);
        window.set_playlist_bars(song_length.div_ceil(TICKS_PER_BAR).max(MAX_PLAYLIST_BARS) as i32);
    }

    fn refresh_note_editor(&self, window: &MainWindow) {
        let Some(channel) = self.session.channels.get(self.session.selected) else {
            return;
        };
        let length_ticks = self.session.pattern_lengths[self.session.current_pattern] as u32 * TICKS_PER_STEP;
        let cells: Vec<NoteCell> = channel.notes[self.session.current_pattern]
            .iter()
            .copied()
            .filter(|note| note.start_tick < length_ticks)
            .map(|note| note_cell(note, &self.session.selected_note_ids))
            .collect();
        self.note_model.set_vec(cells);
        self.refresh_selected_note_controls(window);
    }

    /// Rebuilds the lane picker, the drawn curve, and the header label.
    ///
    /// A destination whose device has since been removed leaves its lane in
    /// storage but drops it from the picker, and clears the visible lane. The
    /// alternative -- silently deleting the automation -- loses work when a
    /// device is removed and re-added.
    fn refresh_automation(&self, window: &MainWindow) {
        let destinations = self.session.automation_destinations();
        if self
            .session.automation_target
            .get()
            .is_some_and(|target| !destinations.iter().any(|(addr, _, _)| *addr == target))
        {
            self.session.automation_target.set(None);
        }
        let open: HashSet<ParamAddr> = self
            .session.automation_lanes()
            .map(|lanes| lanes.iter().map(|lane| lane.target).collect())
            .unwrap_or_default();

        let mut previous_device: Option<&str> = None;
        let rows: Vec<AutomationTargetRow> = destinations
            .iter()
            .map(|(address, device, descriptor)| {
                let starts_group = previous_device != Some(device.as_str());
                previous_device = Some(device.as_str());
                AutomationTargetRow {
                    param_name: descriptor.name.into(),
                    device: device.as_str().into(),
                    starts_group,
                    open: open.contains(address),
                    current: self.session.automation_target.get() == Some(*address),
                }
            })
            .collect();
        self.automation_target_model.set_vec(rows);

        let label = self
            .session.automation_target
            .get()
            .and_then(|target| {
                destinations
                    .iter()
                    .find(|(address, _, _)| *address == target)
                    .map(|(_, device, descriptor)| format!("{device} · {}", descriptor.name))
            })
            .unwrap_or_default();
        window.set_automation_lane_name(label.as_str().into());
        self.refresh_automation_points(window);
    }

    fn refresh_automation_points(&self, window: &MainWindow) {
        let length_ticks = self.session.pattern_lengths[self.session.current_pattern] as u32 * TICKS_PER_STEP;
        let selected = self.session.automation_selected_point.get();
        let cells: Vec<AutomationPointCell> = self
            .session.automation_lane()
            .map(|lane| {
                lane.points()
                    .iter()
                    .filter(|point| point.tick <= length_ticks)
                    .map(|point| AutomationPointCell {
                        id: point.id as i32,
                        tick: point.tick as i32,
                        value: point.value,
                        selected: selected == Some(point.id),
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.automation_point_model.set_vec(cells);

        let readout = self
            .session.automation_selected_point
            .get()
            .and_then(|id| {
                let lane = self.session.automation_lane()?;
                let point = lane.points().iter().find(|point| point.id == id)?;
                let descriptor = self.session.automation_descriptor()?;
                Some(format_param_value(descriptor, point.value))
            })
            .unwrap_or_default();
        window.set_automation_value_text(readout.as_str().into());
    }

    /// The selection's bounding box in ticks and MIDI notes, which is what
    /// the grid draws its selection frame and scale handles from.
    ///
    /// Published as plain properties rather than answered by a callback: the
    /// frame's geometry is a binding, and a callback there would be re-run
    /// on every layout pass rather than when the selection actually changes.
    fn refresh_selection_bounds(&self, window: &MainWindow) {
        let Some(channel) = self.session.channels.get(self.session.selected) else {
            return;
        };
        let mut count = 0;
        let (mut start, mut end) = (u32::MAX, 0u32);
        let (mut low, mut high) = (u8::MAX, 0u8);
        for note in channel.notes[self.session.current_pattern]
            .iter()
            .filter(|note| self.session.selected_note_ids.contains(&note.id))
        {
            count += 1;
            start = start.min(note.start_tick);
            end = end.max(note.end_tick());
            low = low.min(note.note);
            high = high.max(note.note);
        }
        window.set_selection_count(count);
        if count == 0 {
            window.set_selected_duration_index(-1);
            window.set_selected_duration_text("".into());
            return;
        }
        // One length across the whole selection reads as that length; a
        // mixed selection says so rather than picking a winner.
        let mut lengths = channel.notes[self.session.current_pattern]
            .iter()
            .filter(|note| self.session.selected_note_ids.contains(&note.id))
            .map(|note| note.duration_ticks);
        let first = lengths.next().unwrap_or(0);
        if lengths.all(|length| length == first) {
            window.set_selected_duration_index(division_index(first));
            window.set_selected_duration_text(length_text(first).into());
        } else {
            window.set_selected_duration_index(-1);
            window.set_selected_duration_text("mixed".into());
        }
        window.set_selection_start_tick(start as i32);
        window.set_selection_end_tick(end as i32);
        window.set_selection_low_note(low as i32);
        window.set_selection_high_note(high as i32);
    }

    fn refresh_selected_note_controls(&self, window: &MainWindow) {
        window.set_has_selected_note(false);
        window.set_has_note_selection(!self.session.selected_note_ids.is_empty());
        self.refresh_selection_bounds(window);
        // The precision editor shows one note's fields; once the selection
        // is a group (Shift-click, Select All) there is no single note left
        // to show them for.
        if self.session.selected_note_ids.len() > 1 {
            return;
        }
        let Some(id) = self.session.selected_note_id else {
            return;
        };
        let Some(note) = self.session.channels[self.session.selected].notes[self.session.current_pattern]
            .iter()
            .find(|note| note.id == id)
        else {
            return;
        };
        window.set_has_selected_note(true);
        window.set_selected_note_step(note.start_tick as i32);
        window.set_selected_note(note.note as i32);
        window.set_selected_velocity(note.velocity as i32);
    }

    /// Recomputes every step cell for `channel`'s current pattern. Used
    /// after an edit (like a multi-note delete) that can touch notes spread
    /// across many steps, where refreshing one step at a time would miss
    /// the rest.
    /// Hands the selected channel's audio and slice map to the engine.
    fn publish_selected_audio(&self, tx: &ChannelAudioSender) {
        if let Some(channel) = self.session.channels.get(self.session.selected) {
            publish_channel_audio_to(tx, self.session.selected, channel);
        }
    }

    /// Re-draws one device-rack row from the slot behind it.
    fn refresh_effect_row(&self, slot: usize) {
        if let Some(effect) = self.session.effect_chain().and_then(|chain| chain.get(slot)) {
            self.effect_slot_model.set_row_data(
                slot,
                effect_slot_row(effect, &self.session.effect_presets),
            );
        }
    }

    /// Draws a roll edit: the rack cells whose summary changed, then the
    /// roll and its lanes.
    fn apply_note_edit(&self, edit: &NoteEdit, window: &MainWindow) {
        match &edit.cells {
            Some(cells) => {
                for cell in cells {
                    self.refresh_rack_cell(self.session.selected, *cell);
                }
            }
            None => self.refresh_rack_row(self.session.selected),
        }
        self.refresh_note_editor(window);
    }

    fn refresh_rack_row(&self, channel: usize) {
        let notes = &self.session.channels[channel].notes[self.session.current_pattern];
        let cells: Vec<StepCell> = (0..self.session.pattern_lengths[self.session.current_pattern])
            .map(|step| rack_cell(notes, step))
            .collect();
        self.step_models[channel].set_vec(cells);
    }

    /// Resolve every tempo-synced delay to the new transport BPM. The engine
    /// remains millisecond-only: the resulting values take its normal
    /// sample-timed parameter path, so all delays move at the next block
    /// without allocating or rebuilding their rings.
    /// Rebuild the edited chain's rows. The model itself is installed on the
    /// window once; this refreshes its contents after structural changes
    /// (add/remove/reorder) and after the rack is pointed somewhere else.
    fn sync_effects(&self) {
        let armed = self.session.modulation_armed_slot.get();
        let rows: Vec<EffectSlotRow> = match self.session.effect_target {
            // Modulation state belongs to the selected channel, so an insert
            // rack pointed at a bus -- or at another channel -- renders its
            // rows without overlays rather than borrowing this channel's.
            EffectTarget::Channel(channel) if channel as usize == self.session.selected => self
                .session.channels
                .get(channel as usize)
                .map(|state| {
                    state
                        .effects
                        .iter()
                        .enumerate()
                        .map(|(slot, effect)| {
                            let mut row = effect_slot_row(effect, &self.session.effect_presets);
                            let descriptors = effect.kind().descriptors();
                            row.modulation_depths =
                                self.destination_depths(armed, descriptors, |param| {
                                    ParamAddr::effect(
                                        EffectTarget::Channel(channel),
                                        slot as u8,
                                        param,
                                    )
                                });
                            row.modulation_allowed = descriptor_policies(descriptors);
                            row.modulation_offsets = self.destination_offsets(descriptors, |param| {
                                ParamAddr::effect(EffectTarget::Channel(channel), slot as u8, param)
                            });
                            row.modulation_route_counts = descriptor_route_counts(
                                &state.modulation,
                                descriptors,
                                |param| {
                                    ParamAddr::effect(
                                        EffectTarget::Channel(channel),
                                        slot as u8,
                                        param,
                                    )
                                },
                            );
                            row
                        })
                        .collect()
                })
                .unwrap_or_default(),
            _ => self
                .session.effect_chain()
                .map(|effects| {
                    effects
                        .iter()
                        .map(|effect| effect_slot_row(effect, &self.session.effect_presets))
                        .collect()
                })
                .unwrap_or_default(),
        };
        self.effect_slot_model.set_vec(rows);
    }

    /// The armed source's depth for each described parameter, indexed by
    /// **descriptor id** so a knob reads its own overlay with the same stable
    /// number it already uses to address the parameter. Ids are contractually
    /// never renumbered, whereas a descriptor's position in the list is not a
    /// promise. Zero where no route exists, which is also what the overlay
    /// draws at the base value.
    fn destination_depths(
        &self,
        armed: Option<u8>,
        descriptors: &[ParamDescriptor],
        address: impl Fn(u32) -> ParamAddr,
    ) -> ModelRc<f32> {
        self.session
            .destination_depths(armed, descriptors, address)
            .as_slice()
            .into()
    }

    /// What the running modulators are adding to each described parameter
    /// right now, indexed by descriptor id. Resolved here rather than
    /// published per parameter by the engine: a channel has at most four
    /// sources but many destinations, so the audio thread ships the four
    /// outputs and the UI does the same sum `ModRack::offset_for` does on the
    /// realtime side, against the same declared policy.
    fn destination_offsets(
        &self,
        descriptors: &[ParamDescriptor],
        address: impl Fn(u32) -> ParamAddr,
    ) -> ModelRc<f32> {
        self.session
            .destination_offsets(descriptors, address)
            .as_slice()
            .into()
    }

    /// Push the current live modulation offsets onto the generator face and
    /// the effect rows. Called on the pump tick, so it touches only the
    /// offsets: rebuilding the rows here would fight the meter and spectrum
    /// updates landing on the same models.
    fn refresh_modulation_offsets(&self, window: &MainWindow) {
        let scope = EffectTarget::Channel(self.session.selected as u8);
        let Some(channel) = self.session.channels.get(self.session.selected) else {
            return;
        };
        // Each grid tile's meter, touched in place: rebuilding the source
        // rows on the pump tick would fight selection and the add menu for
        // the same reason the effect rows are updated field-wise here.
        let outputs = self.session.modulation_outputs.get();
        for index in 0..self.modulation_source_model.row_count() {
            let Some(mut row) = self.modulation_source_model.row_data(index) else {
                continue;
            };
            let next = usize::try_from(row.slot)
                .ok()
                .and_then(|slot| outputs.get(slot).copied())
                .unwrap_or(0.0);
            if row.output != next {
                row.output = next;
                self.modulation_source_model.set_row_data(index, row);
            }
        }
        window.set_source_modulation_offsets(self.destination_offsets(
            channel.generator_params().kind().descriptors(),
            |param| ParamAddr {
                scope,
                owner: ParamOwner::Source,
                param,
            },
        ));
        // The insert rack only carries this channel's overlays when it is
        // pointed at this channel, exactly as `sync_effects` decides.
        if self.session.effect_target != scope {
            return;
        }
        for (slot, effect) in channel.effects.iter().enumerate() {
            let Some(mut row) = self.effect_slot_model.row_data(slot) else {
                continue;
            };
            row.modulation_offsets =
                self.destination_offsets(effect.kind().descriptors(), |param| {
                    ParamAddr::effect(scope, slot as u8, param)
                });
            self.effect_slot_model.set_row_data(slot, row);
        }
    }

    /// Rebuild the channel-owned source collection and destination inspector.
    /// Selection and assignment are transient UI state: project reloads and
    /// channel changes never leave an invisible armed slot behind.
    fn refresh_modulation(&self, window: &MainWindow) {
        if self.session.modulation_ui_channel.get() != Some(self.session.selected) {
            self.session.modulation_ui_channel.set(Some(self.session.selected));
            self.session.modulation_selected_slot.set(None);
            self.session.modulation_armed_slot.set(None);
        }
        let Some(channel) = self.session.channels.get(self.session.selected) else {
            self.modulation_source_model.set_vec(Vec::new());
            self.modulation_route_model.set_vec(Vec::new());
            self.session.modulation_selected_slot.set(None);
            self.session.modulation_armed_slot.set(None);
            window.set_modulation_selected_slot(-1);
            window.set_modulation_armed_slot(-1);
            return;
        };

        let selected = self.session.modulation_selected_slot.get().filter(|slot| {
            channel
                .modulation
                .slots
                .get(*slot as usize)
                .is_some_and(Option::is_some)
        });
        let armed = self.session.modulation_armed_slot.get().filter(|slot| {
            channel
                .modulation
                .slots
                .get(*slot as usize)
                .is_some_and(Option::is_some)
        });
        self.session.modulation_selected_slot.set(selected);
        self.session.modulation_armed_slot.set(armed);
        let bpm = f64::from(window.get_bpm().max(1));
        let outputs = self.session.modulation_outputs.get();
        let sources: Vec<ModulationSourceRow> = channel
            .modulation
            .slots
            .iter()
            .enumerate()
            .filter_map(|(slot, entry)| {
                let params = (*entry)?.params;
                // One row shape for every kind: the tile's face is whichever
                // fields the kind actually fills, and the rest keep the
                // shape component's own resting values.
                let mut row = ModulationSourceRow {
                    slot: slot as i32,
                    name: format!("{} {}", params.kind().badge(), slot + 1).into(),
                    kind: params.kind().to_index(),
                    depth: 1.0,
                    pulse_width: 0.5,
                    preview_sustain: 0.7,
                    step_length: MOD_STEP_MAX_STEPS as i32,
                    output: outputs.get(slot).copied().unwrap_or(0.0),
                    selected: selected == Some(slot as u8),
                    ..Default::default()
                };
                match params {
                    ModulatorParams::Lfo(lfo) => {
                        let cycle_seconds = if lfo.tempo_sync {
                            lfo.rate_division.seconds(bpm)
                        } else {
                            lfo.rate_hz.max(0.001).recip()
                        };
                        let fade_seconds = if lfo.fade_in_tempo_sync {
                            lfo.fade_in_division.seconds(bpm)
                        } else {
                            lfo.fade_in_seconds
                        };
                        row.waveform = lfo.waveform.to_index();
                        row.rate = lfo.rate_hz;
                        row.depth = lfo.depth;
                        row.phase = lfo.phase;
                        row.pulse_width = lfo.pulse_width;
                        row.preview_fade_cycles = fade_seconds / cycle_seconds;
                        row.preview_smoothing_cycles = lfo.smoothing_seconds / cycle_seconds;
                        row.retrigger = lfo.retrigger;
                    }
                    ModulatorParams::Envelope(envelope) => {
                        row.depth = envelope.amount;
                        row.preview_attack = if envelope.attack_tempo_sync {
                            envelope.attack_division.seconds(bpm)
                        } else {
                            envelope.attack_seconds
                        };
                        row.preview_decay = if envelope.decay_tempo_sync {
                            envelope.decay_division.seconds(bpm)
                        } else {
                            envelope.decay_seconds
                        };
                        row.preview_sustain = envelope.sustain;
                        row.preview_release = if envelope.release_tempo_sync {
                            envelope.release_division.seconds(bpm)
                        } else {
                            envelope.release_seconds
                        };
                        row.retrigger = true;
                    }
                    ModulatorParams::Step(step) => {
                        row.steps = step.steps.as_slice().into();
                        row.step_length = i32::from(step.length);
                        row.retrigger = step.trigger == ModStepTrigger::NoteAdvance;
                    }
                    ModulatorParams::Random(random) => {
                        row.rate = random.rate_hz;
                        row.phase = slot as f32 * 0.25;
                        row.retrigger = random.trigger == ModRandomTrigger::NoteTrigger;
                    }
                    ModulatorParams::Math(math) => {
                        row.math_op = math.op.to_index();
                    }
                }
                Some(row)
            })
            .collect();
        let routes: Vec<ModulationRouteRow> = channel
            .modulation
            .routes
            .iter()
            .enumerate()
            .filter_map(|(index, route)| {
                let route = route.as_ref()?;
                let source_name = channel
                    .modulation
                    .params(route.source_slot as usize)
                    .map_or_else(
                        || "SOURCE ?".to_string(),
                        |params| {
                            format!("{} {}", params.kind().badge(), route.source_slot + 1)
                        },
                    );
                let (destination, allowed) = self
                    .session.channel_modulation_destination(route.destination)
                    .map(|(device, descriptor)| {
                        (
                            format!("{source_name} → {device} · {}", descriptor.name),
                            ModDestinationDescriptor::for_param(descriptor).allowed,
                        )
                    })
                    .unwrap_or_else(|| (format!("{source_name} → unavailable destination"), false));
                let owner = match route.destination.owner {
                    ParamOwner::Source => -1,
                    ParamOwner::Strip => -2,
                    ParamOwner::Effect { slot } => slot as i32,
                    ParamOwner::Modulator { slot } => -3 - slot as i32,
                    // Just past the modulator band, derived rather than
                    // written out, so growing the rack cannot collide with
                    // it. Unreachable today -- the shelf cannot address an
                    // instrument's internal routes -- but the encoding has to
                    // be total.
                    ParamOwner::SourceRoute { .. } => -3 - MAX_MODULATORS_PER_CHANNEL as i32,
                };
                Some(ModulationRouteRow {
                    route_index: index as i32,
                    source_slot: route.source_slot as i32,
                    owner,
                    param: route.destination.param as i32,
                    destination: destination.into(),
                    depth: route.depth,
                    polarity: match route.polarity {
                        ModPolarity::Bipolar => 0,
                        ModPolarity::Unipolar => 1,
                    },
                    allowed,
                })
            })
            .collect();
        self.modulation_source_model.set_vec(sources);
        self.modulation_route_model.set_vec(routes);
        window.set_modulation_shelf_open(self.session.modulation_shelf_open);
        window.set_modulation_selected_slot(selected.map_or(-1, i32::from));
        window.set_modulation_armed_slot(armed.map_or(-1, i32::from));
        window.set_modulation_max_sources(MAX_MODULATORS_PER_CHANNEL as i32);
        // One entry per slot, named by whatever occupies it. The math
        // module's input jack picks from this, so it reads "3 · STEP 3"
        // rather than "3"; the length comes from the protocol constant, so
        // raising capacity never needs a matching UI edit.
        let slot_names: Vec<slint::SharedString> = (0..MAX_MODULATORS_PER_CHANNEL)
            .map(|slot| match channel.modulation.params(slot) {
                Some(params) => {
                    format!("{} · {} {}", slot + 1, params.kind().badge(), slot + 1)
                }
                None => format!("{} · empty", slot + 1),
            })
            .map(slint::SharedString::from)
            .collect();
        window.set_modulation_slot_names(slot_names.as_slice().into());

        // The selected source's own controls. One editor is shown, so the shelf
        // reads scalars rather than searching the source rows for the
        // selected one.
        let selected_params = selected.and_then(|slot| channel.modulation.params(slot as usize));
        let selected_lfo = selected_params.and_then(|params| match params {
            ModulatorParams::Lfo(lfo) => Some(lfo),
            _ => None,
        });
        let selected_envelope = selected_params.and_then(|params| match params {
            ModulatorParams::Envelope(envelope) => Some(envelope),
            _ => None,
        });
        window.set_modulation_selected_kind(
            selected_params.map_or(-1, |params| params.kind().to_index()),
        );
        // The one visible editor reads its values by descriptor id, exactly
        // as the destination overlays already do; the kind decides which id
        // table the array answers for.
        let selected_values: Vec<f32> = selected_params.map_or_else(Vec::new, |params| {
            let descriptors = params.kind().descriptors();
            let mut values = vec![0.0; descriptor_slots(descriptors)];
            for descriptor in descriptors {
                if let Some(value) = params.get(descriptor.id) {
                    values[descriptor.id as usize] = value;
                }
            }
            values
        });
        window.set_modulation_selected_values(selected_values.as_slice().into());
        let selected_lfo_cycle_seconds = selected_lfo.map_or(1.0, |lfo| {
            if lfo.tempo_sync {
                lfo.rate_division.seconds(bpm)
            } else {
                lfo.rate_hz.max(0.001).recip()
            }
        });
        window.set_modulation_selected_preview_fade_cycles(selected_lfo.map_or(0.0, |lfo| {
            let seconds = if lfo.fade_in_tempo_sync {
                lfo.fade_in_division.seconds(bpm)
            } else {
                lfo.fade_in_seconds
            };
            seconds / selected_lfo_cycle_seconds
        }));
        window.set_modulation_selected_preview_smoothing_cycles(selected_lfo.map_or(0.0, |lfo| {
            lfo.smoothing_seconds / selected_lfo_cycle_seconds
        }));
        let input_channels: Vec<slint::SharedString> = self
            .session.channels
            .iter()
            .enumerate()
            .map(|(index, channel)| format!("{} · {}", index + 1, channel.name).into())
            .collect();
        window
            .set_modulation_input_channels(ModelRc::from(Rc::new(VecModel::from(input_channels))));
        window.set_modulation_selected_envelope_input_channel(
            selected_envelope.map_or(self.session.selected as i32, |env| i32::from(env.input_channel)),
        );
        window.set_modulation_selected_envelope_preview_attack(selected_envelope.map_or(
            0.0,
            |env| {
                if env.attack_tempo_sync {
                    env.attack_division.seconds(bpm)
                } else {
                    env.attack_seconds
                }
            },
        ));
        window.set_modulation_selected_envelope_preview_decay(selected_envelope.map_or(
            0.0,
            |env| {
                if env.decay_tempo_sync {
                    env.decay_division.seconds(bpm)
                } else {
                    env.decay_seconds
                }
            },
        ));
        window.set_modulation_selected_envelope_preview_release(selected_envelope.map_or(
            0.0,
            |env| {
                if env.release_tempo_sync {
                    env.release_division.seconds(bpm)
                } else {
                    env.release_seconds
                }
            },
        ));

        // Every described generator and strip parameter carries its own
        // overlay depth and legality, so which controls can be routed is
        // decided by descriptor metadata rather than by the UI naming them.
        let scope = EffectTarget::Channel(self.session.selected as u8);
        let generator = channel.generator_params().kind();
        window.set_source_modulation_depths(self.destination_depths(
            armed,
            generator.descriptors(),
            |param| ParamAddr {
                scope,
                owner: ParamOwner::Source,
                param,
            },
        ));
        window.set_source_modulation_allowed(descriptor_policies(generator.descriptors()));
        window.set_source_modulation_offsets(self.destination_offsets(
            generator.descriptors(),
            |param| ParamAddr {
                scope,
                owner: ParamOwner::Source,
                param,
            },
        ));
        window.set_source_modulation_route_counts(descriptor_route_counts(
            &channel.modulation,
            generator.descriptors(),
            |param| ParamAddr {
                scope,
                owner: ParamOwner::Source,
                param,
            },
        ));
        window.set_strip_modulation_depths(self.destination_depths(
            armed,
            &STRIP_DESCRIPTORS,
            |param| ParamAddr::strip(scope, param),
        ));
        window.set_strip_modulation_allowed(descriptor_policies(&STRIP_DESCRIPTORS));
        // The effect rows carry the focused source's overlay amounts, so a
        // chip selection repaints markers without touching any base value.
        self.sync_effects();
    }

    /// Sends a modulation edit on and re-draws what it changed.
    ///
    /// The session builds the command, since it is the thing that knows which
    /// channel it is on; what is left here is the projection. Every
    /// modulation gesture marks the document edited, and the shelf and the
    /// title both have to show it.
    fn send_modulation(
        &mut self,
        window: &MainWindow,
        tx: &EngineCommandSender,
        command: EngineCommand,
    ) {
        let _ = tx.send(command);
        self.session.mark_dirty();
        self.update_document_title(window);
        self.refresh_modulation(window);
    }

    fn begin_modulation_edit(&mut self, window: &MainWindow) {
        if self.session.modulation_edit_before.is_none() {
            let snapshot = project_snapshot(self, window);
            self.session.begin_modulation_edit(snapshot);
        }
    }

    /// Retune (or first create) the armed source's one explicit route. The
    /// base parameter is deliberately absent from this mutation: a normal
    /// knob drag in armed mode moves only the depth, and the renderer keeps
    /// resolving the same authored base underneath it.
    /// Points the armed modulation source at `destination`.
    ///
    /// The rack edit is the session's; refusing out loud when the matrix is
    /// full, and sending the route on, are this layer's.
    fn set_armed_modulation_depth(
        &mut self,
        window: &MainWindow,
        tx: &EngineCommandSender,
        destination: ParamAddr,
        depth: f32,
    ) -> bool {
        match self.session.arm_modulation_route(destination, depth) {
            ArmedRoute::Unchanged => false,
            ArmedRoute::Full => {
                // An assignment gesture that does nothing at all reads as a
                // broken knob, so say why.
                window.set_status_message(
                    format!(
                        "This channel already has its {MAX_MOD_ROUTES_PER_CHANNEL} modulation \
                         assignments; remove one to add another"
                    )
                    .as_str()
                    .into(),
                );
                false
            }
            ArmedRoute::Added(route) => {
                let channel = self.session.selected as u8;
                self.send_modulation(window, tx, EngineCommand::SetModRoute { channel, route });
                true
            }
        }
    }

    /// Rebuild every mixer strip and the shared name list. Called after a load
    /// or any change that moves channels between buses.
    fn sync_mixer(&self, window: &MainWindow) {
        let names: Vec<slint::SharedString> = self
            .session.buses
            .iter()
            .map(|setup| setup.bus.name.as_str().into())
            .collect();
        window.set_bus_names(ModelRc::from(Rc::new(VecModel::from(names))));
        let strips: Vec<MixerStripRow> = self
            .session.buses
            .iter()
            .enumerate()
            .map(|(index, setup)| self.mixer_strip_row(index, setup))
            .collect();
        self.mixer_strip_model.set_vec(strips);
        self.sync_bus_editor(window);
    }

    /// Destinations `bus` may be routed to, indexed by bus. The engine trusts
    /// the schedule it is sent, so refusing a loop is this side's job; the
    /// mask lets the picker show *why* rather than silently declining.
    fn allowed_destinations(&self, bus: usize) -> ModelRc<bool> {
        ModelRc::from(Rc::new(VecModel::from(
            self.session.allowed_destinations(bus),
        )))
    }

    fn mixer_strip_row(&self, index: usize, setup: &BusSetup) -> MixerStripRow {
        MixerStripRow {
            name: setup.bus.name.as_str().into(),
            muted: setup.bus.muted,
            volume: setup.bus.volume,
            pan: setup.bus.pan,
            output: setup.bus.output as i32,
            selected: self.session.effect_target == EffectTarget::Bus(index as u8),
            is_master: index == MASTER_BUS as usize,
            feed_count: self.session.bus_feed_count(index) as i32,
            allowed: self.allowed_destinations(index),
            // Levels are owned by the metering timer, which writes them in
            // place; rebuilding a row must not stamp them back to silence.
            left_db: self
                .mixer_strip_model
                .row_data(index)
                .map(|row| row.left_db)
                .unwrap_or(METER_FLOOR_DB),
            right_db: self
                .mixer_strip_model
                .row_data(index)
                .map(|row| row.right_db)
                .unwrap_or(METER_FLOOR_DB),
        }
    }

    /// Refresh one strip's controls without disturbing the rest.
    fn sync_mixer_strip(&self, index: usize) {
        let Some(setup) = self.session.buses.get(index) else {
            return;
        };
        self.mixer_strip_model
            .set_row_data(index, self.mixer_strip_row(index, setup));
    }

    /// Push the selection flag to every strip, so exactly one reads selected.
    fn sync_mixer_selection(&self) {
        for index in 0..self.mixer_strip_model.row_count() {
            if let Some(mut row) = self.mixer_strip_model.row_data(index) {
                row.selected = self.session.effect_target == EffectTarget::Bus(index as u8);
                self.mixer_strip_model.set_row_data(index, row);
            }
        }
    }

    /// Mirror the edited bus onto the device rack's head face. When a channel
    /// is being edited this only clears the flag; the source face takes over.
    fn sync_bus_editor(&self, window: &MainWindow) {
        let EffectTarget::Bus(index) = self.session.effect_target else {
            window.set_editing_bus(false);
            return;
        };
        let index = index as usize;
        let Some(setup) = self.session.buses.get(index) else {
            window.set_editing_bus(false);
            return;
        };
        window.set_editing_bus(true);
        window.set_editing_bus_index(index as i32);
        window.set_editing_bus_name(setup.bus.name.as_str().into());
        window.set_editing_bus_is_master(index == MASTER_BUS as usize);
        window.set_editing_bus_muted(setup.bus.muted);
        window.set_editing_bus_volume(setup.bus.volume);
        window.set_editing_bus_pan(setup.bus.pan);
        window.set_editing_bus_output(setup.bus.output as i32);
        window.set_editing_bus_feed_count(self.session.bus_feed_count(index) as i32);
        window.set_editing_bus_allowed(self.allowed_destinations(index));
    }

    /// Refresh the bottom editor's properties from `selected`.
    fn refresh_editor(&self, window: &MainWindow) {
        let Some(ch) = self.session.channels.get(self.session.selected) else {
            return;
        };
        let p = &ch.params;
        let drum = ch.drum_params;
        let mono = ch.mono_params;
        window.set_selected_channel_name(ch.name.as_str().into());
        window.set_selected_channel_volume_db(linear_to_db(ch.volume));
        window.set_source_kind(device_kind_to_int(ch.kind));
        self.sync_effects();
        self.refresh_modulation(window);
        // The lane's destination catalogue is built from the effect chains, so
        // it has to be rebuilt wherever the chains or the selection can have
        // moved -- which is exactly this function's job.
        self.refresh_automation(window);
        window.set_drum_mode(drum_mode_to_int(drum.mode));
        window.set_drum_kick_character(kick_character_to_int(drum.kick_character));
        window.set_drum_snare_character(snare_character_to_int(drum.snare_character));
        window.set_drum_hat_character(hat_character_to_int(drum.hat_character));
        window.set_drum_decay(drum.decay);
        window.set_drum_tune_semitones(drum.tune_semitones);
        window.set_drum_drive(drum.drive);
        window.set_drum_punch(drum.punch);
        window.set_drum_choke_group(drum.choke_group as i32);
        window.set_drum_kick_start_hz(drum.kick_start_hz);
        window.set_drum_kick_end_hz(drum.kick_end_hz);
        window.set_drum_kick_sweep(drum.kick_sweep);
        window.set_drum_kick_click(drum.kick_click);
        window.set_drum_snare_tone_hz(drum.snare_tone_hz);
        window.set_drum_snare_tone2_hz(drum.snare_tone2_hz);
        window.set_drum_snare_tone2_mix(drum.snare_tone2_mix);
        window.set_drum_snare_noise_mix(drum.snare_noise_mix);
        window.set_drum_snare_noise_decay(drum.snare_noise_decay);
        window.set_drum_snare_noise_color(drum.snare_noise_color);
        window.set_drum_hat_hp_hz(drum.hat_hp_hz);
        window.set_drum_hat_metallic(drum.hat_metallic);
        sync_drum_preview(window, drum);
        window.set_mono_osc1_wave(osc_wave_to_int(mono.osc[0].wave));
        window.set_mono_osc1_semitones(mono.osc[0].semitones);
        window.set_mono_osc1_cents(mono.osc[0].cents);
        window.set_mono_osc1_level(mono.osc[0].level);
        window.set_mono_osc1_pulse_width(mono.osc[0].pulse_width);
        window.set_mono_osc2_wave(osc_wave_to_int(mono.osc[1].wave));
        window.set_mono_osc2_semitones(mono.osc[1].semitones);
        window.set_mono_osc2_cents(mono.osc[1].cents);
        window.set_mono_osc2_level(mono.osc[1].level);
        window.set_mono_osc2_pulse_width(mono.osc[1].pulse_width);
        window.set_mono_osc3_wave(osc_wave_to_int(mono.osc[2].wave));
        window.set_mono_osc3_semitones(mono.osc[2].semitones);
        window.set_mono_osc3_cents(mono.osc[2].cents);
        window.set_mono_osc3_level(mono.osc[2].level);
        window.set_mono_osc3_pulse_width(mono.osc[2].pulse_width);
        window.set_mono_glide(mono.glide);
        window.set_mono_attack(mono.attack);
        window.set_mono_decay(mono.decay);
        window.set_mono_sustain(mono.sustain);
        window.set_mono_release(mono.release);
        window.set_mono_filter_cutoff(mono.filter_cutoff);
        window.set_mono_filter_resonance(mono.filter_resonance);
        window.set_mono_filter_env(mono.filter_env_amount);
        window.set_mono_drive(mono.drive);
        window.set_mono_lfo_wave(lfo_wave_to_int(mono.lfo.wave));
        window.set_mono_lfo_rate(mono.lfo.rate_hz);
        window.set_mono_lfo_retrigger(mono.lfo.retrigger);
        window.set_mono_lfo_pitch(mono.lfo.to_pitch);
        window.set_mono_lfo_filter(mono.lfo.to_filter);
        window.set_mono_lfo_pulse_width(mono.lfo.to_pulse_width);
        window.set_mono_lfo_amp(mono.lfo.to_amp);
        let mlp8 = ch.mlp8_params;
        window.set_mlp8_osc1_wave(osc_wave_to_int(mlp8.osc[0].wave));
        window.set_mlp8_osc1_semitones(mlp8.osc[0].semitones);
        window.set_mlp8_osc1_cents(mlp8.osc[0].cents);
        window.set_mlp8_osc1_level(mlp8.osc[0].level);
        window.set_mlp8_osc1_pulse_width(mlp8.osc[0].pulse_width);
        window.set_mlp8_osc2_wave(osc_wave_to_int(mlp8.osc[1].wave));
        window.set_mlp8_osc2_semitones(mlp8.osc[1].semitones);
        window.set_mlp8_osc2_cents(mlp8.osc[1].cents);
        window.set_mlp8_osc2_level(mlp8.osc[1].level);
        window.set_mlp8_osc2_pulse_width(mlp8.osc[1].pulse_width);
        window.set_mlp8_osc3_wave(osc_wave_to_int(mlp8.osc[2].wave));
        window.set_mlp8_osc3_semitones(mlp8.osc[2].semitones);
        window.set_mlp8_osc3_cents(mlp8.osc[2].cents);
        window.set_mlp8_osc3_level(mlp8.osc[2].level);
        window.set_mlp8_osc3_pulse_width(mlp8.osc[2].pulse_width);
        window.set_mlp8_attack(mlp8.attack);
        window.set_mlp8_decay(mlp8.decay);
        window.set_mlp8_sustain(mlp8.sustain);
        window.set_mlp8_release(mlp8.release);
        window.set_mlp8_glide(mlp8.glide);
        window.set_mlp8_sub_level(mlp8.sub_level);
        window.set_mlp8_noise_level(mlp8.noise_level);
        window.set_mlp8_noise_color(mlp8.noise_color);
        window.set_mlp8_sub_octave(mlp8.sub_octave.to_index());
        window.set_mlp8_sub_wave(mlp8.sub_wave.to_index());
        window.set_mlp8_sub_source(mlp8.sub_source.to_index());
        window.set_mlp8_xmod12(mlp8.xmod[mooloop_core::mlp8::xmod_index(0, 1)]);
        window.set_mlp8_xmod13(mlp8.xmod[mooloop_core::mlp8::xmod_index(0, 2)]);
        window.set_mlp8_xmod21(mlp8.xmod[mooloop_core::mlp8::xmod_index(1, 0)]);
        window.set_mlp8_xmod23(mlp8.xmod[mooloop_core::mlp8::xmod_index(1, 2)]);
        window.set_mlp8_xmod31(mlp8.xmod[mooloop_core::mlp8::xmod_index(2, 0)]);
        window.set_mlp8_xmod32(mlp8.xmod[mooloop_core::mlp8::xmod_index(2, 1)]);
        window.set_mlp8_noise_osc1(mlp8.noise_to_osc[0]);
        window.set_mlp8_noise_osc2(mlp8.noise_to_osc[1]);
        window.set_mlp8_noise_osc3(mlp8.noise_to_osc[2]);
        window.set_mlp8_feedback1(mlp8.osc_feedback[0]);
        window.set_mlp8_feedback2(mlp8.osc_feedback[1]);
        window.set_mlp8_feedback3(mlp8.osc_feedback[2]);
        window.set_mlp8_sync1(mlp8.sync_source[0].to_index());
        window.set_mlp8_sync2(mlp8.sync_source[1].to_index());
        window.set_mlp8_sync3(mlp8.sync_source[2].to_index());
        window.set_mlp8_filter_mode(mlp8.filter_mode.to_index());
        window.set_mlp8_filter_cutoff(mlp8.filter_cutoff);
        window.set_mlp8_filter_resonance(mlp8.filter_resonance);
        window.set_mlp8_filter_env(mlp8.filter_env_amount);
        window.set_mlp8_drive(mlp8.drive);
        window.set_mlp8_keytrack(mlp8.filter_keytrack);
        window.set_mlp8_amp_velocity(mlp8.amp_velocity);
        window.set_mlp8_filter_velocity(mlp8.filter_velocity);
        window.set_mlp8_voice_feedback(mlp8.voice_feedback);
        window.set_mlp8_filter_attack(mlp8.filter_attack);
        window.set_mlp8_filter_decay(mlp8.filter_decay);
        window.set_mlp8_filter_sustain(mlp8.filter_sustain);
        window.set_mlp8_filter_release(mlp8.filter_release);
        window.set_mlp8_lfo_wave(mlp8.lfo.wave.to_index());
        window.set_mlp8_lfo_synced(mlp8.lfo.synced);
        window.set_mlp8_lfo_rate_hz(mlp8.lfo.rate_hz);
        window.set_mlp8_lfo_division(mlp8.lfo.rate_division.to_index());
        window.set_mlp8_lfo_phase(mlp8.lfo.phase);
        window.set_mlp8_lfo_warp(mlp8.lfo.warp);
        window.set_mlp8_lfo_slew(mlp8.lfo.slew);
        window.set_mlp8_lfo_retrigger(mlp8.lfo.retrigger.to_index());
        refresh_mlp8_routes(window, &mlp8.routes);
        // Only when the face is actually showing. `refresh_ds01` renders a
        // hit through the production voice path and walks the burst schedule,
        // and this runs on every editor refresh — a pattern switch, an undo, a
        // channel select. Doing it for a sampler channel is the per-
        // interaction work the preview's own debounce exists to avoid.
        if ch.kind == DeviceKind::Ds01 {
            refresh_ds01(window, &ch.ds01_params);
        }
        let mlm1 = ch.mlm1_params;
        window.set_mlm1_osc1_wave(osc_wave_to_int(mlm1.osc[0].wave));
        window.set_mlm1_osc1_semitones(mlm1.osc[0].semitones);
        window.set_mlm1_osc1_cents(mlm1.osc[0].cents);
        window.set_mlm1_osc1_level(mlm1.osc[0].level);
        window.set_mlm1_osc1_pulse_width(mlm1.osc[0].pulse_width);
        window.set_mlm1_osc2_wave(osc_wave_to_int(mlm1.osc[1].wave));
        window.set_mlm1_osc2_semitones(mlm1.osc[1].semitones);
        window.set_mlm1_osc2_cents(mlm1.osc[1].cents);
        window.set_mlm1_osc2_level(mlm1.osc[1].level);
        window.set_mlm1_osc2_pulse_width(mlm1.osc[1].pulse_width);
        window.set_mlm1_osc3_wave(osc_wave_to_int(mlm1.osc[2].wave));
        window.set_mlm1_osc3_semitones(mlm1.osc[2].semitones);
        window.set_mlm1_osc3_cents(mlm1.osc[2].cents);
        window.set_mlm1_osc3_level(mlm1.osc[2].level);
        window.set_mlm1_osc3_pulse_width(mlm1.osc[2].pulse_width);
        window.set_mlm1_glide(mlm1.glide);
        window.set_mlm1_attack(mlm1.attack);
        window.set_mlm1_decay(mlm1.decay);
        window.set_mlm1_sustain(mlm1.sustain);
        window.set_mlm1_release(mlm1.release);
        window.set_mlm1_filter_cutoff(mlm1.filter_cutoff);
        window.set_mlm1_filter_resonance(mlm1.filter_resonance);
        window.set_mlm1_filter_env(mlm1.filter_env_amount);
        window.set_mlm1_drive(mlm1.drive);
        window.set_mlm1_filter_attack(mlm1.filter_attack);
        window.set_mlm1_filter_decay(mlm1.filter_decay);
        window.set_mlm1_filter_sustain(mlm1.filter_sustain);
        window.set_mlm1_filter_release(mlm1.filter_release);
        window.set_mlm1_filter_keytrack(mlm1.filter_keytrack);
        window.set_mlm1_accent(mlm1.accent);
        window.set_mlm1_glide_mode(mlm1.glide_mode.to_index());
        window.set_mlm1_env_trigger(mlm1.env_trigger.to_index());
        window.set_mlm1_priority(mlm1.priority.to_index());
        window.set_mlm1_filter_model(mlm1.filter_model.to_index());
        let poly = ch.poly_params;
        window.set_poly_osc1_wave(osc_wave_to_int(poly.osc[0].wave));
        window.set_poly_osc1_semitones(poly.osc[0].semitones);
        window.set_poly_osc1_cents(poly.osc[0].cents);
        window.set_poly_osc1_level(poly.osc[0].level);
        window.set_poly_osc1_pulse_width(poly.osc[0].pulse_width);
        window.set_poly_osc2_wave(osc_wave_to_int(poly.osc[1].wave));
        window.set_poly_osc2_semitones(poly.osc[1].semitones);
        window.set_poly_osc2_cents(poly.osc[1].cents);
        window.set_poly_osc2_level(poly.osc[1].level);
        window.set_poly_osc2_pulse_width(poly.osc[1].pulse_width);
        window.set_poly_osc3_wave(osc_wave_to_int(poly.osc[2].wave));
        window.set_poly_osc3_semitones(poly.osc[2].semitones);
        window.set_poly_osc3_cents(poly.osc[2].cents);
        window.set_poly_osc3_level(poly.osc[2].level);
        window.set_poly_osc3_pulse_width(poly.osc[2].pulse_width);
        window.set_poly_glide(poly.glide);
        window.set_poly_attack(poly.attack);
        window.set_poly_decay(poly.decay);
        window.set_poly_sustain(poly.sustain);
        window.set_poly_release(poly.release);
        window.set_poly_filter_cutoff(poly.filter_cutoff);
        window.set_poly_filter_resonance(poly.filter_resonance);
        window.set_poly_filter_env(poly.filter_env_amount);
        window.set_poly_drive(poly.drive);
        window.set_poly_lfo_wave(lfo_wave_to_int(poly.lfo.wave));
        window.set_poly_lfo_rate(poly.lfo.rate_hz);
        window.set_poly_lfo_retrigger(poly.lfo.retrigger);
        window.set_poly_lfo_pitch(poly.lfo.to_pitch);
        window.set_poly_lfo_filter(poly.lfo.to_filter);
        window.set_poly_lfo_pulse_width(poly.lfo.to_pulse_width);
        window.set_poly_lfo_amp(poly.lfo.to_amp);
        window.set_poly_polyphony(poly.polyphony.clamp(1, MAX_POLY_VOICES) as i32);
        window.set_poly_spread(poly.spread);
        window.set_sample_name(ch.sample_name.as_str().into());
        window.set_sample_description(ch.sample_description.as_str().into());
        window.set_sample_duration(ch.sample_duration);
        window.set_sample_frames(
            ch.published_sample()
                .map(|sample| sample.frames.len() as i32)
                .unwrap_or(0),
        );
        self.waveform_model.set_vec(ch.waveform.clone());
        self.slice_model.set_vec(slice_fractions(ch));
        window.set_play_mode(p.play_mode.to_index());
        window.set_slice_base_note(i32::from(p.slice_base_note));
        window.set_sample_committed(ch.commit.is_some());
        window.set_commit_label(
            ch.commit
                .as_ref()
                .map(|commit| format!("baked {:.2}x", commit.ratio))
                .unwrap_or_default()
                .into(),
        );
        // Stale is a bar-synced commit whose project has since changed tempo.
        // Reported, never acted on: re-baking a loop under someone without
        // being asked is worse than telling them it no longer fits.
        window.set_commit_stale(
            ch.commit
                .as_ref()
                .is_some_and(|commit| commit_is_stale(ch, commit, window.get_bpm() as f64)),
        );
        // A newly selected channel's waveform view starts fully zoomed out;
        // a stale zoom window from the previous channel would otherwise
        // misalign against this one's sample length.
        window.set_waveform_view_offset(0.0);
        window.set_waveform_view_visible_fraction(1.0);
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
        window.set_tune_label(tune_label(*p).into());
        window.set_retune_live(p.retune_live);
        window.set_loop_mode(match p.loop_mode {
            LoopMode::Off => 0,
            LoopMode::Forward => 1,
            LoopMode::Pingpong => 2,
        });
        window.set_voice_mode(match p.voice_mode {
            VoiceMode::OneShot => 0,
            VoiceMode::Gate => 1,
        });
        window.set_sampler_polyphony(p.polyphony as i32);
        window.set_retrigger_mode(match p.retrigger_mode {
            RetriggerMode::Restart => 0,
            RetriggerMode::Layer => 1,
        });
        window.set_choke_group(p.choke_group as i32);
        window.set_stretch_enabled(p.stretch_enabled);
        window.set_stretch_mode(match p.stretch_mode {
            StretchMode::Music => 0,
            StretchMode::Drums => 1,
            StretchMode::Grain => 2,
        });
        window.set_stretch_ratio(stretch_ratio_to_norm(p.stretch_ratio));
        window.set_stretch_ratio_label(format!("{:.2}x", p.stretch_ratio).into());
        window.set_stretch_grain(stretch_grain_to_norm(p.stretch_grain));
        // Frames are what the DSP works in, but the number a player is
        // chasing is the pitch of the rattle it produces.
        window.set_stretch_grain_label(
            format!(
                "{} fr / {:.0} Hz",
                p.stretch_grain,
                window.get_audio_sample_rate() as f32 / (p.stretch_grain.max(2) as f32 / 2.0)
            )
            .into(),
        );
        window.set_stretch_ratio_clean((0.5..=1.5).contains(&p.stretch_ratio));
        window.set_stretch_sync(p.stretch_sync);
        window.set_stretch_bars(stretch_bars_to_norm(p.stretch_bars));
        window.set_stretch_bars_label(format_bars(p.stretch_bars).into());
        window.set_filter_cutoff(p.filter_cutoff);
        window.set_filter_resonance(p.filter_resonance);
        window.set_filter_env((p.filter_env_amount + 1.0) * 0.5);
        window.set_sampler_drive(p.drive);
        window.set_bit_reduction(p.bit_reduction);
        window.set_rate_reduction(p.rate_reduction);
        window.set_sampler_output_gain(p.output_gain);
        // Published through the resolution, so a patch whose filter envelope
        // still follows the amplitude one shows the shape it actually runs
        // rather than an empty control group.
        let filter_env = p.resolved_filter_env();
        window.set_sampler_filter_attack(time_to_norm(filter_env.attack));
        window.set_sampler_filter_decay(time_to_norm(filter_env.decay));
        window.set_sampler_filter_sustain(filter_env.sustain);
        window.set_sampler_filter_release(time_to_norm(filter_env.release));
        self.refresh_note_editor(window);
    }
}

impl AppUi {
    pub fn new(mut handle: EngineHandle) -> Result<Self, slint::PlatformError> {
        let window = MainWindow::new()?;

        // The shipped patches become ordinary user presets the first time
        // the app runs, and are not touched again. A failure here is not
        // worth refusing to start over: the bank is content, not
        // configuration, and the browser simply shows one fewer category.
        if let Err(error) = mooloop_project::seed_mlm1_bank(&settings::channel_presets_dir()) {
            log_warn!("app", "could not write the ML-M1 factory bank: {error}");
        }
        // The effect banks, one directory a kind, on the same terms.
        for kind in EffectKind::ALL {
            if let Err(error) =
                mooloop_project::seed_effect_bank(&settings::effect_presets_dir(kind), kind)
            {
                log_warn!(
                    "app",
                    "could not write the {} factory bank: {error}",
                    kind.label()
                );
            }
        }

        // --- Transport initial state ---
        window.set_bpm(INITIAL_BPM);
        window.set_swing_percent(DEFAULT_SWING_PERCENT.into());
        window.set_playing(false);
        window.set_beat_in_bar(0);
        window.set_position_bar(1);
        window.set_position_beat(1);
        window.set_position_tick(0);
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
        // Two lists the device declares once; nothing about a patch moves
        // them, so they are installed here rather than on every refresh.
        install_mlp8_route_vocabularies(&window);
        handle.send(EngineCommand::SetTempo(INITIAL_BPM as f64));
        handle.send(EngineCommand::SetSwing(DEFAULT_SWING_PERCENT));

        // --- Channel rack state: start with one empty channel ---
        //
        // `default_sample`/`default_waveform`/etc. are kept only to resolve
        // legacy `SampleReference::Builtin` references when an old project
        // (saved before the sampler stopped auto-loading a kick) is opened —
        // see `apply_sample_references` and `install_project_in_ui`. They no
        // longer seed a freshly created channel, which starts genuinely
        // empty.
        let audio_sample_rate = handle.sample_rate();
        window.set_audio_sample_rate(audio_sample_rate as i32);
        let default_sample = Some(SampleData::default_kick(audio_sample_rate));
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
        let first = ChannelState::new(0);
        let first_steps: Vec<StepCell> = (0..DEFAULT_STEPS as usize)
            .map(|step| rack_cell(&first.notes[0], step))
            .collect();
        let step_model = Rc::new(VecModel::from(first_steps));
        let note_model = Rc::new(VecModel::from(Vec::<NoteCell>::new()));
        let automation_point_model = Rc::new(VecModel::from(Vec::<AutomationPointCell>::new()));
        let automation_target_model = Rc::new(VecModel::from(Vec::<AutomationTargetRow>::new()));
        let playlist_model = Rc::new(VecModel::from(Vec::<PlaylistClip>::new()));
        let row = ChannelRow {
            name: first.name.as_str().into(),
            muted: false,
            volume_db: linear_to_db(first.volume),
            pan: first.pan,
            selected: true,
            bus: first.bus as i32,
            steps: ModelRc::from(step_model.clone()),
        };
        let rows_model = Rc::new(VecModel::from(vec![row]));
        let waveform_model = Rc::new(VecModel::from(first.waveform.clone()));
        let slice_model = Rc::new(VecModel::from(Vec::<f32>::new()));
        let playhead_model = Rc::new(VecModel::from(Vec::<f32>::new()));
        let effect_slot_model = Rc::new(VecModel::from(Vec::<EffectSlotRow>::new()));
        let modulation_source_model = Rc::new(VecModel::from(Vec::<ModulationSourceRow>::new()));
        let modulation_route_model = Rc::new(VecModel::from(Vec::<ModulationRouteRow>::new()));
        let mixer_strip_model = Rc::new(VecModel::from(Vec::<MixerStripRow>::new()));
        let browser_row_model = Rc::new(VecModel::from(Vec::<BrowserRow>::new()));
        window.set_channels(ModelRc::from(rows_model.clone()));
        window.set_notes(ModelRc::from(note_model.clone()));
        window.set_automation_points(ModelRc::from(automation_point_model.clone()));
        window.set_automation_targets(ModelRc::from(automation_target_model.clone()));
        window.set_playlist_clips(ModelRc::from(playlist_model.clone()));
        window.set_waveform(ModelRc::from(waveform_model.clone()));
        window.set_slice_markers(ModelRc::from(slice_model.clone()));
        window.set_playhead_positions(ModelRc::from(playhead_model.clone()));
        window.set_effect_slots(ModelRc::from(effect_slot_model.clone()));
        window.set_modulation_sources(ModelRc::from(modulation_source_model.clone()));
        window.set_modulation_routes(ModelRc::from(modulation_route_model.clone()));
        window.set_mixer_strips(ModelRc::from(mixer_strip_model.clone()));
        window.set_browser_rows(ModelRc::from(browser_row_model.clone()));
        window.set_pattern_count(1);

        let state = Rc::new(RefCell::new(UiState {
            session: Session {
                channels: vec![first],
                default_waveform,
                default_sample_description,
                default_sample_duration,
                ..Session::default()
            },
            rows: rows_model,
            step_models: vec![step_model],
            note_model,
            playlist_model,
            waveform_model,
            slice_model,
            playhead_model,
            effect_slot_model,
            modulation_source_model,
            modulation_route_model,
            mixer_strip_model,
            browser_rows: browser_row_model,
            automation_point_model,
            automation_target_model,
        }));
        let starter = Project::starter_kit(fresh_starter_seed());
        let starter_samples = vec![None; starter.channels.len()];
        install_project_in_ui(
            &mut handle,
            default_sample.as_ref(),
            &state,
            &window,
            &starter,
            &starter_samples,
        );
        state.borrow().update_document_title(&window);
        state.borrow().sync_pattern_menu(&window);
        state.borrow().sync_mixer(&window);
        window.set_app_version(env!("CARGO_PKG_VERSION").into());

        let (document_tx, document_rx) = std::sync::mpsc::channel::<DocumentResult>();
        let command_state = Rc::new(RefCell::new(CommandState::default()));
        sync_command_availability(&window, &command_state.borrow());
        let export_sample_rate = handle.sample_rate();

        {
            let st = state.clone();
            window.on_quit_requested(move || {
                // Same guard as Open Song: unsaved work must be confirmed
                // away, and the zenity round-trip must not block the UI.
                let dirty = st.borrow().session.dirty;
                std::thread::spawn(move || {
                    if dirty && !confirm_via_zenity("Discard unsaved song changes and quit?") {
                        return;
                    }
                    let _ = slint::invoke_from_event_loop(|| {
                        slint::quit_event_loop().ok();
                    });
                });
            });
        }
        {
            let st = state.clone();
            let tx = document_tx.clone();
            let weak = window.as_weak();
            window.on_new_song(move || {
                let dirty = st.borrow().session.dirty;
                if let Some(window) = weak.upgrade() {
                    window.set_document_busy(true);
                    window.set_status_message("Creating new song...".into());
                }
                let tx = tx.clone();
                std::thread::spawn(move || {
                    if dirty && !confirm_via_zenity("Discard unsaved song changes?") {
                        let _ = tx.send(DocumentResult::Cancelled);
                        return;
                    }
                    let _ = tx.send(DocumentResult::NewSong(Project::starter_kit(
                        fresh_starter_seed(),
                    )));
                });
            });
        }

        {
            let st = state.clone();
            let tx = document_tx.clone();
            let weak = window.as_weak();
            window.on_open_song(move || {
                let dirty = st.borrow().session.dirty;
                if let Some(window) = weak.upgrade() {
                    window.set_document_busy(true);
                    window.set_status_message("Opening song...".into());
                }
                let tx = tx.clone();
                std::thread::spawn(move || {
                    if dirty && !confirm_via_zenity("Discard unsaved song changes?") {
                        let _ = tx.send(DocumentResult::Cancelled);
                        return;
                    }
                    let Some(path) = pick_song_via_zenity("Open mooloop song") else {
                        let _ = tx.send(DocumentResult::Cancelled);
                        return;
                    };
                    let result = resolve_document(&path)
                        .map(|document| DocumentResult::Loaded {
                            path,
                            target: LoadTarget::Song,
                            document,
                        })
                        .unwrap_or_else(|problem| DocumentResult::Failed {
                            action: "open this song",
                            problem,
                        });
                    let _ = tx.send(result);
                });
            });
        }
        for save_as in [false, true] {
            let st = state.clone();
            let tx = document_tx.clone();
            let weak = window.as_weak();
            let callback = move || {
                let Some(window) = weak.upgrade() else {
                    return;
                };
                // The free function, not `UiState::project_snapshot`: it is
                // the one that squares every channel's pattern-indexed banks
                // with the pattern list, and a save that skipped that step was
                // one of the ways a song reached disk in a shape it could not
                // be read back from.
                let project = project_snapshot(&st.borrow(), &window).project;
                let revision = st.borrow().session.revision;
                let mode = if window.get_embed_assets() {
                    AssetMode::Embedded
                } else {
                    AssetMode::Referenced
                };
                let current = (!save_as)
                    .then(|| st.borrow().session.bundle_path.clone())
                    .flatten();
                window.set_document_busy(true);
                window.set_status_message("Saving song...".into());
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let path = current
                        .or_else(|| pick_save_via_zenity("Save mooloop song", "Untitled.mooloop"));
                    let Some(path) = path else {
                        let _ = tx.send(DocumentResult::Cancelled);
                        return;
                    };
                    log_info!(
                        "project",
                        "saving song to {} ({mode:?} assets)",
                        path.display()
                    );
                    let target = path.clone();
                    let attempt = mooloop_project::save_song(&path, &project, mode).and_then(
                        |report| {
                            let loaded = mooloop_project::load_bundle(&path)?;
                            let LoadedDocument::Song(saved) = loaded.document else {
                                return Err(mooloop_project::Error::Invalid(
                                    "saved bundle did not contain a song".into(),
                                ));
                            };
                            Ok(DocumentResult::SavedSong {
                                path,
                                mode,
                                revision,
                                report,
                                sample_references: saved
                                    .channels
                                    .into_iter()
                                    .map(|channel| {
                                        channel
                                            .setup
                                            .source
                                            .sampler_state()
                                            .map(|sampler| sampler.sample.clone())
                                    })
                                    .collect(),
                            })
                        },
                    );
                    let result = attempt.unwrap_or_else(|error| {
                        let mut problem = DocumentProblem::from(error);
                        log_error!(
                            "project",
                            "save refused for {}: {}",
                            target.display(),
                            problem.one_line()
                        );
                        // The document the user was working on exists only in
                        // this process, and the save that would have committed
                        // it just failed. Park a copy before the failure is
                        // reported, so the answer to "can I look at it later"
                        // is yes even if they close the window in disgust.
                        if let Some(parked) = quarantine_song(
                            &settings::quarantine_dir(),
                            &build_description(),
                            &project,
                            &problem,
                        ) {
                            problem.message.push_str(&format!(
                                "\n\nNothing is lost: a copy of this song was set aside at {}.",
                                parked.display()
                            ));
                        }
                        DocumentResult::Failed {
                            action: "save this song",
                            problem,
                        }
                    });
                    let _ = tx.send(result);
                });
            };
            if save_as {
                window.on_save_song_as(callback);
            } else {
                window.on_save_song(callback);
            }
        }
        {
            let st = state.clone();
            let tx = document_tx.clone();
            let weak = window.as_weak();
            window.on_save_kit(move || {
                let Some(window) = weak.upgrade() else {
                    return;
                };
                let snapshot = st
                    .borrow()
                    .session.project_snapshot(window.get_bpm(), window.get_swing_percent());
                let kit = Kit {
                    channels: snapshot
                        .channels
                        .into_iter()
                        .map(|channel| channel.setup)
                        .collect(),
                };
                let mode = asset_mode_from_window(&window);
                window.set_document_busy(true);
                window.set_status_message("Saving kit...".into());
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let Some(path) =
                        pick_save_via_zenity("Save mooloop kit", "Untitled.mooloop-kit")
                    else {
                        let _ = tx.send(DocumentResult::Cancelled);
                        return;
                    };
                    let result = mooloop_project::save_kit(&path, &kit, mode)
                        .map(|report| DocumentResult::SavedOther {
                            label: "Kit saved",
                            report,
                        })
                        .unwrap_or_else(|error| DocumentResult::Failed {
                            action: "save this kit",
                            problem: error.into(),
                        });
                    let _ = tx.send(result);
                });
            });
        }
        {
            let st = state.clone();
            let tx = document_tx.clone();
            let weak = window.as_weak();
            window.on_save_channel(move || {
                let Some(window) = weak.upgrade() else {
                    return;
                };
                let snapshot = st
                    .borrow()
                    .session.project_snapshot(window.get_bpm(), window.get_swing_percent());
                let channel = snapshot.channels[snapshot.selected_channel as usize]
                    .setup
                    .clone();
                let mode = asset_mode_from_window(&window);
                window.set_document_busy(true);
                window.set_status_message("Saving channel...".into());
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let Some(path) =
                        pick_save_via_zenity("Save mooloop channel", "Untitled.mooloop-channel")
                    else {
                        let _ = tx.send(DocumentResult::Cancelled);
                        return;
                    };
                    let result = mooloop_project::save_channel(&path, &channel, mode)
                        .map(|report| DocumentResult::SavedOther {
                            label: "Channel saved",
                            report,
                        })
                        .unwrap_or_else(|error| DocumentResult::Failed {
                            action: "save this channel",
                            problem: error.into(),
                        });
                    let _ = tx.send(result);
                });
            });
        }
        for (kit, title) in [(true, "Load mooloop kit"), (false, "Load mooloop channel")] {
            let tx = document_tx.clone();
            let weak = window.as_weak();
            let callback = move || {
                if let Some(window) = weak.upgrade() {
                    window.set_document_busy(true);
                    window.set_status_message(if kit {
                        "Loading kit...".into()
                    } else {
                        "Loading channel...".into()
                    });
                }
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let Some(path) = pick_bundle_via_zenity(title) else {
                        let _ = tx.send(DocumentResult::Cancelled);
                        return;
                    };
                    let target = if kit {
                        LoadTarget::Kit
                    } else {
                        LoadTarget::Channel
                    };
                    let result = resolve_document(&path)
                        .map(|document| DocumentResult::Loaded {
                            path,
                            target,
                            document,
                        })
                        .unwrap_or_else(|problem| DocumentResult::Failed {
                            action: "open this file",
                            problem,
                        });
                    let _ = tx.send(result);
                });
            };
            if kit {
                window.on_load_kit(callback);
            } else {
                window.on_load_channel(callback);
            }
        }

        // --- Presets: browse-and-load from the well-known presets dirs ---
        for (generator, label) in [(true, "generator preset"), (false, "channel preset")] {
            let st = state.clone();
            let tx = document_tx.clone();
            let weak = window.as_weak();
            let callback = move |index: i32| {
                let Some(path) = ({
                    let st = st.borrow();
                    let presets = if generator {
                        &st.session.generator_presets
                    } else {
                        &st.session.channel_presets
                    };
                    presets
                        .get(index as usize)
                        .map(|preset| preset.path.clone())
                }) else {
                    return;
                };
                if let Some(window) = weak.upgrade() {
                    window.set_document_busy(true);
                    window.set_status_message(format!("Loading {label}...").into());
                }
                let tx = tx.clone();
                let target = if generator {
                    LoadTarget::Generator
                } else {
                    LoadTarget::Channel
                };
                std::thread::spawn(move || {
                    let result = resolve_document(&path)
                        .map(|document| DocumentResult::Loaded {
                            path,
                            target,
                            document,
                        })
                        .unwrap_or_else(|problem| DocumentResult::Failed {
                            action: "open this preset",
                            problem,
                        });
                    let _ = tx.send(result);
                });
            };
            if generator {
                window.on_generator_preset_selected(callback);
            } else {
                window.on_channel_preset_selected(callback);
            }
        }

        // --- Effect presets: one rack row, loaded into the row it was
        // picked from. The index names an entry of that row's kind, in the
        // order `effect_presets_of_kind` lists them. ---
        {
            let st = state.clone();
            let tx = document_tx.clone();
            let weak = window.as_weak();
            window.on_effect_preset_selected(move |slot, index| {
                let (Ok(slot), Ok(index)) = (u8::try_from(slot), usize::try_from(index)) else {
                    return;
                };
                let Some(path) = ({
                    let st = st.borrow();
                    st.session
                        .effect_chain()
                        .and_then(|chain| chain.get(slot as usize))
                        .map(EffectSlotState::kind)
                        .and_then(|kind| {
                            effect_presets_of_kind(&st.session.effect_presets, kind)
                                .nth(index)
                                .map(|preset| preset.path.clone())
                        })
                }) else {
                    return;
                };
                if let Some(window) = weak.upgrade() {
                    window.set_document_busy(true);
                    window.set_status_message("Loading effect preset...".into());
                }
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let result = resolve_document(&path)
                        .map(|document| DocumentResult::Loaded {
                            path,
                            target: LoadTarget::Effect { slot },
                            document,
                        })
                        .unwrap_or_else(|problem| DocumentResult::Failed {
                            action: "open this preset",
                            problem,
                        });
                    let _ = tx.send(result);
                });
            });
        }

        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_save_effect_preset_requested(move |slot| {
                let Ok(slot) = u8::try_from(slot) else {
                    return;
                };
                let mut st = st.borrow_mut();
                let target = st.session.effect_target;
                if st
                    .session
                    .effect_chain()
                    .is_none_or(|chain| chain.get(slot as usize).is_none())
                {
                    return;
                }
                st.session.pending_preset_save = Some(PresetSaveTarget::Effect { target, slot });
                if let Some(window) = weak.upgrade() {
                    window.set_save_preset_title("Save Effect Preset".into());
                    window.set_save_preset_name("".into());
                    window.set_save_preset_category("".into());
                    window.set_save_preset_open(true);
                }
            });
        }

        // --- Presets: open the save dialog, scoped to generator or channel ---
        for (generator, title) in [
            (true, "Save Generator Preset"),
            (false, "Save Channel Preset"),
        ] {
            let st = state.clone();
            let weak = window.as_weak();
            let callback = move || {
                st.borrow_mut().session.pending_preset_save = Some(if generator {
                    PresetSaveTarget::Generator
                } else {
                    PresetSaveTarget::Channel
                });
                if let Some(window) = weak.upgrade() {
                    window.set_save_preset_title(title.into());
                    window.set_save_preset_name("".into());
                    window.set_save_preset_category("".into());
                    window.set_save_preset_open(true);
                }
            };
            if generator {
                window.on_save_generator_preset_requested(callback);
            } else {
                window.on_save_channel_preset_requested(callback);
            }
        }
        {
            let st = state.clone();
            window.on_save_preset_cancelled(move || {
                st.borrow_mut().session.pending_preset_save = None;
            });
        }
        {
            let st = state.clone();
            let tx = document_tx.clone();
            let weak = window.as_weak();
            window.on_save_preset_confirmed(move |name, category| {
                let Some(window) = weak.upgrade() else {
                    return;
                };
                let name = name.trim().to_string();
                if name.is_empty() {
                    return;
                }
                let Some(source) = st
                    .borrow_mut()
                    .session
                    .take_preset_save(window.get_bpm(), window.get_swing_percent())
                else {
                    return;
                };
                let info = PresetInfo {
                    name: name.clone(),
                    category: category.trim().to_string(),
                    tags: Vec::new(),
                };
                let file_stem = mooloop_project::sanitize_preset_name(&name);
                let (dir, extension, label) = match source.target {
                    PresetSaveTarget::Generator => (
                        settings::generator_presets_dir(source.setup.kind()),
                        "mooloop-generator",
                        "Generator preset saved",
                    ),
                    PresetSaveTarget::Channel => (
                        settings::channel_presets_dir(),
                        "mooloop-channel",
                        "Channel preset saved",
                    ),
                    // The row's own kind picks the directory, so a delay
                    // preset can only ever be offered to a delay row.
                    PresetSaveTarget::Effect { .. } => match source.effect {
                        Some(effect) => (
                            settings::effect_presets_dir(effect.kind()),
                            "mooloop-effect",
                            "Effect preset saved",
                        ),
                        None => return,
                    },
                };
                let path = dir.join(format!("{file_stem}.{extension}"));
                window.set_document_busy(true);
                window.set_status_message("Saving preset...".into());
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let result = match source.target {
                        PresetSaveTarget::Generator => mooloop_project::save_generator_preset(
                            &path,
                            &source.setup.source,
                            info,
                            AssetMode::Embedded,
                        ),
                        PresetSaveTarget::Channel => mooloop_project::save_channel_preset(
                            &path,
                            &source.setup,
                            info,
                            AssetMode::Embedded,
                        ),
                        PresetSaveTarget::Effect { .. } => match source.effect {
                            Some(effect) => mooloop_project::save_effect_preset(
                                &path,
                                &effect,
                                info,
                                AssetMode::Embedded,
                            ),
                            None => return,
                        },
                    };
                    let result = result
                        .map(|report| DocumentResult::SavedPreset { label, report })
                        .unwrap_or_else(|error| DocumentResult::Failed {
                            action: "save this preset",
                            problem: error.into(),
                        });
                    let _ = tx.send(result);
                });
            });
        }

        {
            let weak = window.as_weak();
            window.on_export_audio(move || {
                if let Some(window) = weak.upgrade() {
                    window.set_export_open(true);
                }
            });
        }
        {
            let st = state.clone();
            let tx = document_tx.clone();
            let weak = window.as_weak();
            window.on_export_confirmed(move |format, bitrate, tail| {
                let Some(window) = weak.upgrade() else {
                    return;
                };
                let request = st.borrow().session.export_request(
                    window.get_bpm(),
                    window.get_swing_percent(),
                    format,
                    bitrate,
                );
                window.set_export_open(false);
                window.set_document_busy(true);
                window.set_status_message("Rendering audio...".into());
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let Some(path) = pick_export_via_zenity(request.extension()) else {
                        let _ = tx.send(DocumentResult::Cancelled);
                        return;
                    };
                    let spec = ExportSpec {
                        path: path.clone(),
                        scope: request.scope,
                        tail_seconds: tail as f32,
                        format: request.format,
                    };
                    let result = OfflineRenderer::render(
                        &request.project,
                        &request.samples,
                        export_sample_rate,
                        &spec,
                    )
                    .map(|_| DocumentResult::Exported { path })
                    .unwrap_or_else(|error| DocumentResult::Failed {
                        action: "export this song",
                        problem: error.to_string().into(),
                    });
                    let _ = tx.send(result);
                });
            });
        }

        {
            let st = state.clone();
            window.window().on_close_requested(move || {
                if st.borrow().session.dirty && !confirm_via_zenity("Quit without saving this song?") {
                    CloseRequestResponse::KeepWindowShown
                } else {
                    CloseRequestResponse::HideWindow
                }
            });
        }

        // --- Ordered command channel from UI closures to the pump ---
        let (pending_tx, pending_rx) = std::sync::mpsc::channel::<PendingEngineMessage>();
        let cmd_tx = EngineCommandSender(pending_tx.clone());
        let project_edit_tx = ProjectEditSender(pending_tx.clone());
        let audio_tx = AudioActionSender(pending_tx.clone());
        let telemetry_tx = TelemetryActionSender(pending_tx.clone());
        let preview_tx = PreviewSender(pending_tx.clone());
        let structural_tx = StructuralCommandSender(pending_tx);
        let sample_rate = handle.sample_rate();
        // Sample slots are published out-of-band, so source replacement asks
        // the pump (which owns the EngineHandle) to restore the built-in sample.
        let (sample_reset_tx, sample_reset_rx) = std::sync::mpsc::channel::<usize>();
        // Slice edits and stretch commits publish through the same route, for
        // the same reason: both change what sits in a channel's `ArcSwap`
        // slots rather than what its parameters say.
        let (channel_audio_tx, channel_audio_rx) = std::sync::mpsc::channel::<ChannelAudio>();
        let channel_audio_tx = ChannelAudioSender(channel_audio_tx);

        // --- Preferences: appearance applies live from here; audio reaches
        //     the engine through the pump below, the only place that owns
        //     `EngineHandle`. ---
        let ui_settings = Rc::new(RefCell::new(UiSettings::load_or_default()));
        // Kept alive here for as long as the app runs so the window survives
        // after this constructor returns; re-opening while it is already up
        // just refocuses it instead of spawning a second one.
        #[cfg(feature = "mockup")]
        let mockup_window: Rc<RefCell<Option<mockup_ui::MockupCanvas>>> =
            Rc::new(RefCell::new(None));
        // The Developer page hides its tools row entirely rather than offering
        // a button that would open nothing.
        window.set_preferences_mockup_tool_available(cfg!(feature = "mockup"));
        {
            let settings = ui_settings.borrow();
            apply_appearance(&window, &settings.appearance);
            sync_preferences_properties(&window, &settings);
            audio_tx.send(AudioAction::ApplyPersisted(settings.audio.engine_config()));
            // The browser opens with every top-level location expanded: the
            // point of the sidebar is seeing samples without extra clicks.
            let mut st = state.borrow_mut();
            st.session.browser_locations = settings.browser.locations.clone();
            st.session.browser_expanded = st.session.browser_locations.iter().cloned().collect();
            refresh_browser(&st);
        }

        // The action registry (`actions.rs`, `docs/ACTIONS.md`): resolves a
        // decoded key chord to a stable action id, then this dispatches to
        // whichever existing window callback already performs it. Keyboard
        // and menu therefore always agree, since a keyboard shortcut is
        // never anything more than an alternate way to invoke the same
        // callback the matching menu row calls.
        let shortcut_table = Rc::new(RefCell::new(actions::ShortcutTable::build(
            &ui_settings.borrow().shortcuts.overrides,
        )));
        {
            let table = shortcut_table.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_shortcut_key(move |key, ctrl, shift, alt, meta| {
                let Some(window) = weak.upgrade() else {
                    return false;
                };
                let chord = actions::KeyChord::new(ctrl, shift, alt, meta, key.as_str());
                let Some(action_id) = table.borrow().resolve(&chord) else {
                    return false;
                };
                let channel = window.get_selected_channel();
                match action_id {
                    "transport.play-pause" => window.invoke_toggle_play(),
                    "file.open" => window.invoke_open_song(),
                    "file.save" => window.invoke_save_song(),
                    "file.save-as" => window.invoke_save_song_as(),
                    "file.export" => window.invoke_export_audio(),
                    "file.quit" => window.invoke_quit_requested(),
                    "edit.undo" => window.invoke_edit_command_requested(0, channel),
                    "edit.redo" => window.invoke_edit_command_requested(1, channel),
                    // On the roll, with notes selected, the clipboard verbs
                    // mean the notes. Anywhere else they still mean the
                    // channel, which is what they have always meant.
                    "edit.cut-channel" => {
                        if notes_have_focus(&window) {
                            window.invoke_piano_notes_copied(true);
                        } else {
                            window.invoke_edit_command_requested(2, channel);
                        }
                    }
                    "edit.copy-channel" => {
                        if notes_have_focus(&window) {
                            window.invoke_piano_notes_copied(false);
                        } else {
                            window.invoke_edit_command_requested(3, channel);
                        }
                    }
                    "edit.paste-channel" => {
                        // Paste does not need a selection -- it needs
                        // something on the note clipboard.
                        if window.get_editor_page() == 1
                            && !commands.borrow().note_clipboard.is_empty()
                        {
                            window.invoke_piano_notes_pasted();
                        } else {
                            window.invoke_edit_command_requested(4, channel);
                        }
                    }
                    "notes.nudge-earlier" | "notes.nudge-later" => {
                        if !notes_have_focus(&window) {
                            return false;
                        }
                        let step = if window.get_piano_snap_enabled() {
                            window.get_piano_snap_ticks().max(1)
                        } else {
                            1
                        };
                        let sign = if action_id == "notes.nudge-earlier" { -1 } else { 1 };
                        window.invoke_piano_notes_nudged(sign * step, 0);
                    }
                    "notes.nudge-up" | "notes.nudge-down" => {
                        if !notes_have_focus(&window) {
                            return false;
                        }
                        let sign = if action_id == "notes.nudge-up" { 1 } else { -1 };
                        window.invoke_piano_notes_nudged(0, sign);
                    }
                    "channel.clone" => window.invoke_edit_command_requested(5, channel),
                    "channel.remove" => window.invoke_edit_command_requested(6, channel),
                    "channel.add" => window.invoke_add_channel_clicked(0),
                    "pattern.add" => window.invoke_add_pattern_clicked(),
                    "pattern.clone" => window.invoke_pattern_clone_requested(),
                    "pattern.remove" => window.invoke_pattern_remove_requested(),
                    "pattern.clear" => window.invoke_pattern_clear_requested(),
                    "edit.select-all" => window.invoke_select_all_requested(),
                    "edit.delete-note" => window.invoke_delete_selected_notes_requested(),
                    // Bare digits, and only while the roll is on screen: a
                    // number key means something else on every other page,
                    // and an unconditional binding would be a trap there.
                    "notes.tool-select"
                    | "notes.tool-draw"
                    | "notes.tool-paint"
                    | "notes.tool-slice"
                    | "notes.tool-erase" => {
                        if window.get_editor_page() != 1 {
                            return false;
                        }
                        window.set_piano_tool(match action_id {
                            "notes.tool-draw" => 1,
                            "notes.tool-paint" => 2,
                            "notes.tool-slice" => 3,
                            "notes.tool-erase" => 4,
                            _ => 0,
                        });
                    }
                    "notes.snap-toggle" => {
                        if window.get_editor_page() != 1 {
                            return false;
                        }
                        window.set_piano_snap_enabled(!window.get_piano_snap_enabled());
                    }
                    "view.zoom-in" => window.invoke_zoom_in_requested(),
                    "view.zoom-out" => window.invoke_zoom_out_requested(),
                    "view.pane-steps" => show_pane(&commands, &window, Pane::Steps),
                    "view.pane-mixer" => show_pane(&commands, &window, Pane::Mixer),
                    "view.pane-source" => show_pane(&commands, &window, Pane::Source),
                    "view.pane-notes" => show_pane(&commands, &window, Pane::Notes),
                    "view.pane-playlist" => show_pane(&commands, &window, Pane::Playlist),
                    "view.pane-next" => {
                        let pane = cycle_pane(commands.borrow().pane, true);
                        show_pane(&commands, &window, pane);
                    }
                    "view.pane-prev" => {
                        let pane = cycle_pane(commands.borrow().pane, false);
                        show_pane(&commands, &window, pane);
                    }
                    _ => return false,
                }
                true
            });
        }
        sync_shortcut_rows(&window, &shortcut_table.borrow());
        let gesture_table = Rc::new(RefCell::new(gestures::GestureTable::build(
            &ui_settings.borrow().gestures.overrides,
        )));
        window.set_preferences_gesture_choices(ModelRc::from(Rc::new(VecModel::from(
            gestures::choice_labels()
                .into_iter()
                .map(slint::SharedString::from)
                .collect::<Vec<_>>(),
        ))));
        sync_gesture_rows(&window, &gesture_table.borrow());
        {
            let settings = ui_settings.clone();
            let table = gesture_table.clone();
            let weak = window.as_weak();
            window.on_preferences_gesture_rebound(move |gesture_id, index| {
                let Some(window) = weak.upgrade() else { return };
                let Some(modifier) = gestures::CHOICES.get(index.max(0) as usize) else {
                    return;
                };
                // Unlike a key chord, two roles sharing a modifier is not a
                // collision to resolve: the roles apply at different moments
                // of a drag, and Ctrl meaning both "keep the selection" and
                // "duplicate it" is the default arrangement.
                let mut settings = settings.borrow_mut();
                settings
                    .gestures
                    .overrides
                    .insert(gesture_id.to_string(), modifier.to_string());
                let result = settings.save();
                *table.borrow_mut() = gestures::GestureTable::build(&settings.gestures.overrides);
                drop(settings);
                sync_gesture_rows(&window, &table.borrow());
                window.set_status_message(match result {
                    Ok(()) => "Gesture updated".into(),
                    Err(error) => format!("Could not save gesture: {error}").into(),
                });
            });
        }
        {
            let settings = ui_settings.clone();
            let table = shortcut_table.clone();
            let weak = window.as_weak();
            window.on_preferences_shortcut_rebind_key(
                move |action_id, key, ctrl, shift, alt, meta| {
                    let Some(window) = weak.upgrade() else { return };
                    let chord = actions::KeyChord::new(ctrl, shift, alt, meta, key.as_str());
                    // Assigning a chord already owned by another action clears
                    // that action rather than leaving two actions pointing at
                    // the same chord: `ShortcutTable::resolve` would only ever
                    // reach one of them, so a silent second owner is worse than
                    // a visible unbind.
                    let owners: Vec<&'static str> =
                        table.borrow().owners_of(&chord, action_id.as_str());
                    let mut settings = settings.borrow_mut();
                    for owner in &owners {
                        settings
                            .shortcuts
                            .overrides
                            .insert((*owner).to_string(), String::new());
                    }
                    settings
                        .shortcuts
                        .overrides
                        .insert(action_id.to_string(), chord.to_string());
                    let result = settings.save();
                    *table.borrow_mut() =
                        actions::ShortcutTable::build(&settings.shortcuts.overrides);
                    drop(settings);
                    sync_shortcut_rows(&window, &table.borrow());
                    match result {
                        Ok(()) => {
                            if let Some(owner) = owners.first() {
                                let label = actions::ACTIONS
                                    .iter()
                                    .find(|spec| spec.id == *owner)
                                    .map_or(*owner, |spec| spec.label);
                                window.set_status_message(format!("{label} is now unbound").into());
                            } else {
                                window.set_status_message("Shortcut updated".into());
                            }
                        }
                        Err(error) => {
                            window.set_status_message(
                                format!("Could not save shortcut: {error}").into(),
                            );
                        }
                    }
                },
            );
        }
        {
            let settings = ui_settings.clone();
            let table = shortcut_table.clone();
            let weak = window.as_weak();
            window.on_preferences_shortcut_reset(move |action_id| {
                let Some(window) = weak.upgrade() else { return };
                let mut settings = settings.borrow_mut();
                settings.shortcuts.overrides.remove(action_id.as_str());
                let result = settings.save();
                *table.borrow_mut() = actions::ShortcutTable::build(&settings.shortcuts.overrides);
                drop(settings);
                sync_shortcut_rows(&window, &table.borrow());
                if let Err(error) = result {
                    window.set_status_message(format!("Could not save shortcut: {error}").into());
                }
            });
        }
        {
            let settings = ui_settings.clone();
            let table = shortcut_table.clone();
            let weak = window.as_weak();
            window.on_preferences_shortcut_reset_all(move || {
                let Some(window) = weak.upgrade() else { return };
                let mut settings = settings.borrow_mut();
                settings.shortcuts.overrides.clear();
                let result = settings.save();
                *table.borrow_mut() = actions::ShortcutTable::build(&settings.shortcuts.overrides);
                drop(settings);
                sync_shortcut_rows(&window, &table.borrow());
                window.set_status_message(if result.is_ok() {
                    "Shortcuts reset to defaults".into()
                } else {
                    "Could not save shortcuts".into()
                });
            });
        }
        #[cfg(feature = "mockup")]
        {
            let mockup_window = mockup_window.clone();
            window.on_preferences_open_mockup_tool(move || {
                if let Some(existing) = mockup_window.borrow().as_ref() {
                    let _ = existing.show();
                    return;
                }
                match open_mockup_window() {
                    Ok(canvas) => *mockup_window.borrow_mut() = Some(canvas),
                    Err(error) => log_error!("ui", "could not open UI mockup tool: {error}"),
                }
            });
        }
        {
            let settings = ui_settings.clone();
            let tx = audio_tx.clone();
            let weak = window.as_weak();
            window.on_preferences_opened(move || {
                let Some(window) = weak.upgrade() else { return };
                let settings = settings.borrow();
                apply_appearance(&window, &settings.appearance);
                sync_preferences_properties(&window, &settings);
                tx.send(AudioAction::RefreshTargets);
            });
        }
        {
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_preferences_appearance_preview(
                move |base, accent, alert, contrast, roundness| {
                    let Some(window) = weak.upgrade() else { return };
                    let candidate = AppearanceSettings {
                        base: base.into(),
                        accent: accent.into(),
                        alert: alert.into(),
                        contrast,
                        roundness,
                        smooth_curves: window.get_preferences_smooth_curves(),
                        ..settings.borrow().appearance.clone()
                    };
                    // A hand-typed hex is invalid for the few keystrokes it takes
                    // to finish typing it, so a rejected preview reports the
                    // reason and leaves the last good theme on screen.
                    match candidate.validated() {
                        Ok(appearance) => {
                            window.set_preferences_appearance_scheme(
                                appearance.matching_scheme_name().into(),
                            );
                            apply_appearance(&window, &appearance);
                            window.set_preferences_error("".into());
                        }
                        Err(error) => window.set_preferences_error(error.to_string().into()),
                    }
                },
            );
        }
        {
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_preferences_appearance_select_scheme(move |name| {
                let Some(window) = weak.upgrade() else { return };
                let mut candidate = window_appearance(&window, &settings.borrow().appearance);
                let Some(scheme) = candidate.scheme(name.as_str()) else {
                    return;
                };
                candidate.apply_scheme(&scheme);
                // Selecting a scheme is a preview like any other: it only
                // reaches settings.toml through Apply or OK.
                match candidate.validated() {
                    Ok(appearance) => {
                        window.set_preferences_appearance_base(appearance.base.as_str().into());
                        window.set_preferences_appearance_accent(appearance.accent.as_str().into());
                        window.set_preferences_appearance_alert(appearance.alert.as_str().into());
                        window.set_preferences_appearance_scheme(appearance.scheme.as_str().into());
                        apply_appearance(&window, &appearance);
                        window.set_preferences_error("".into());
                    }
                    Err(error) => window.set_preferences_error(error.to_string().into()),
                }
            });
        }
        {
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_preferences_appearance_save_scheme(move |name| {
                let Some(window) = weak.upgrade() else { return };
                let mut settings = settings.borrow_mut();
                let mut candidate = window_appearance(&window, &settings.appearance);
                let mut appearance = match candidate.validated() {
                    Ok(appearance) => appearance,
                    Err(error) => {
                        window.set_preferences_error(error.to_string().into());
                        return;
                    }
                };
                if let Err(error) = appearance.save_user_scheme(name.as_str()) {
                    window.set_preferences_error(error.to_string().into());
                    return;
                }
                candidate = appearance;
                let previous = std::mem::replace(&mut settings.appearance, candidate);
                if let Err(error) = settings.save() {
                    settings.appearance = previous;
                    window
                        .set_preferences_error(format!("Could not save settings: {error}").into());
                    return;
                }
                window.set_preferences_appearance_scheme_name("".into());
                apply_appearance(&window, &settings.appearance);
                sync_preferences_properties(&window, &settings);
            });
        }
        {
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_preferences_appearance_remove_scheme(move |name| {
                let Some(window) = weak.upgrade() else { return };
                let mut settings = settings.borrow_mut();
                let mut appearance = settings.appearance.clone();
                appearance.remove_user_scheme(name.as_str());
                let previous = std::mem::replace(&mut settings.appearance, appearance);
                if let Err(error) = settings.save() {
                    settings.appearance = previous;
                    window
                        .set_preferences_error(format!("Could not save settings: {error}").into());
                    return;
                }
                // Removing a scheme drops the stored name but keeps the colors
                // on screen, so the list refreshes without the theme flickering.
                let scheme = window.get_preferences_appearance_scheme();
                window.set_preferences_appearance_schemes(scheme_rows(&settings.appearance));
                if scheme == name {
                    window.set_preferences_appearance_scheme("".into());
                }
            });
        }
        {
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_preferences_save(
                move |base, accent, alert, contrast, roundness, developer_mode, smooth_curves| {
                    let Some(window) = weak.upgrade() else {
                        return false;
                    };
                    let mut settings = settings.borrow_mut();
                    // Motion reads straight out of the global the Appearance
                    // page writes into, so its segment selections apply live
                    // and persist together with the palette.
                    let motion = window.global::<Motion>();
                    let candidate = AppearanceSettings {
                        base: base.into(),
                        accent: accent.into(),
                        alert: alert.into(),
                        contrast,
                        roundness,
                        smooth_curves,
                        motion_speed: settings::motion_speed_name(motion.get_speed())
                            .to_owned(),
                        motion_easing: settings::motion_easing_name(motion.get_easing())
                            .to_owned(),
                        ..settings.appearance.clone()
                    };
                    let mut appearance = match candidate.validated() {
                        Ok(appearance) => appearance,
                        Err(error) => {
                            window.set_preferences_error(error.to_string().into());
                            return false;
                        }
                    };
                    appearance.scheme = appearance.matching_scheme_name();
                    apply_appearance(&window, &appearance);
                    let previous = std::mem::replace(&mut settings.appearance, appearance);
                    let previous_developer_mode = settings.general.developer_mode;
                    settings.general.developer_mode = developer_mode;
                    if let Err(error) = settings.save() {
                        settings.appearance = previous;
                        settings.general.developer_mode = previous_developer_mode;
                        window.set_preferences_error(
                            format!("Could not save settings: {error}").into(),
                        );
                        return false;
                    }
                    sync_preferences_properties(&window, &settings);
                    true
                },
            );
        }
        {
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_preferences_cancelled(move || {
                let Some(window) = weak.upgrade() else { return };
                let settings = settings.borrow();
                apply_appearance(&window, &settings.appearance);
                sync_preferences_properties(&window, &settings);
            });
        }
        {
            let tx = audio_tx.clone();
            window.on_preferences_audio_refresh_targets(move || {
                tx.send(AudioAction::RefreshTargets);
            });
        }
        {
            let tx = audio_tx.clone();
            window.on_preferences_audio_select_output(move |_client, port_l, port_r| {
                tx.send(AudioAction::SelectOutput {
                    port_l: port_l.to_string(),
                    port_r: port_r.to_string(),
                });
            });
        }
        {
            let tx = audio_tx.clone();
            window.on_preferences_audio_select_buffer_size(move |index| {
                if let Some(&frames) = JACK_BUFFER_SIZES.get(index as usize) {
                    tx.send(AudioAction::SelectBufferSize(frames));
                }
            });
        }
        {
            let tx = audio_tx;
            window.on_preferences_audio_auto_reconnect_toggled(move |enabled| {
                tx.send(AudioAction::SetAutoReconnect(enabled));
            });
        }

        // Transport callbacks.
        {
            let tx = cmd_tx.clone();
            window.on_play_clicked(move || {
                log_debug!("ui", "play clicked, queuing Play");
                let _ = tx.send(EngineCommand::Play);
            });
        }
        {
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_stop_clicked(move || {
                log_debug!("ui", "stop clicked, queuing Stop");
                if let Some(window) = weak.upgrade() {
                    window.set_playing(false);
                    window.set_playlist_position_ticks(0);
                    window.set_position_bar(1);
                    window.set_position_beat(1);
                    window.set_position_tick(0);
                }
                let _ = tx.send(EngineCommand::Stop);
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_playback_mode_changed(move |song_mode| {
                let command = st.borrow_mut().session.set_playback_mode(song_mode);
                if let Some(window) = weak.upgrade() {
                    window.set_song_mode(song_mode);
                }
                let _ = tx.send(command);
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_bpm_changed(move |bpm| {
                let bpm = bpm as f64;
                // The session returns these already ordered: tempo first,
                // then every synced delay's resolved ms value, before any
                // beat-relative buffer replacement.
                let commands = {
                    let mut state = st.borrow_mut();
                    let commands = state.session.set_tempo(bpm);
                    state.sync_effects();
                    commands
                };
                for command in commands {
                    let _ = tx.send(command);
                }
                let _ = tx.resize_buffers(bpm);
                if let Some(window) = weak.upgrade() {
                    let st = st.borrow();
                    st.update_document_title(&window);
                    // A bar-synced bake was measured against the tempo, so
                    // its stale badge follows the tempo rather than waiting
                    // for the next full editor refresh.
                    if let Some(channel) = st.session.channels.get(st.session.selected) {
                        window.set_commit_stale(
                            channel
                                .commit
                                .as_ref()
                                .is_some_and(|commit| commit_is_stale(channel, commit, bpm)),
                        );
                    }
                }
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_swing_changed(move |percent| {
                let mut st = st.borrow_mut();
                let _ = tx.send(st.session.set_swing(percent));
                if let Some(window) = weak.upgrade() {
                    st.update_document_title(&window);
                }
            });
        }
        {
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_toggle_play(move || {
                let playing = weak.upgrade().map(|w| w.get_playing()).unwrap_or(false);
                log_debug!(
                    "ui",
                    "toggle-play -> {}",
                    if playing { "Pause" } else { "Play" }
                );
                if let Some(window) = weak.upgrade() {
                    window.set_playing(!playing);
                }
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
                let Some(p) = st.borrow_mut().session.select_pattern(p) else {
                    return;
                };
                log_debug!("ui", "pattern {p} selected");
                st.borrow().show_pattern(p);
                if let Some(w) = weak.upgrade() {
                    w.set_current_pattern(p as i32);
                    let st = st.borrow();
                    w.set_pattern_length(st.session.pattern_lengths[p] as i32);
                    st.refresh_editor(&w);
                    st.sync_pattern_menu(&w);
                }
                let _ = tx.send(EngineCommand::SetCurrentPattern(p as u8));
            });
        }

        // Patterns are created explicitly. The realtime engine owns a fully
        // preallocated pool, while the UI exposes only this active prefix.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_add_pattern_clicked(move || {
                if commands.borrow().project_edit_pending {
                    return;
                }
                let mut st = st.borrow_mut();
                let Some(pattern) = st.session.add_pattern() else {
                    return;
                };
                st.show_pattern(pattern);
                if let Some(window) = weak.upgrade() {
                    window.set_pattern_count(st.session.pattern_lengths.len() as i32);
                    window.set_current_pattern(pattern as i32);
                    window.set_pattern_length(DEFAULT_STEPS as i32);
                    st.refresh_editor(&window);
                    st.sync_playlist(&window);
                    st.sync_pattern_menu(&window);
                }
                let _ = tx.send(EngineCommand::AddPattern);
                let _ = tx.send(EngineCommand::SetCurrentPattern(pattern as u8));
            });
        }

        // Pattern renaming. An empty name is legal and falls back to
        // "Pattern N" in the menu.
        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_pattern_renamed(move |index, name| {
                let mut st = st.borrow_mut();
                if !st.session.rename_pattern(index as usize, &name) {
                    return;
                }
                if let Some(window) = weak.upgrade() {
                    st.sync_pattern_menu(&window);
                }
            });
        }

        // Per-pattern logical length. Channel storage stays at the maximum so
        // shortening and re-extending a pattern does not discard hidden steps.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_pattern_length_changed(move |length| {
                let mut st = st.borrow_mut();
                let Some(applied) = st.session.set_pattern_length(length) else {
                    return;
                };
                st.show_pattern(applied.pattern);
                if let Some(w) = weak.upgrade() {
                    w.set_pattern_length(applied.length as i32);
                    st.refresh_note_editor(&w);
                    st.sync_playlist(&w);
                }
                let _ = tx.send(EngineCommand::SetPatternLength {
                    pattern: applied.pattern as u8,
                    length_steps: applied.length as u16,
                });
            });
        }

        // Placement callbacks already carry musical-grid-snapped PPQ ticks.
        // Clip duration follows the referenced pattern's logical length.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_playlist_placement_added(move |pattern, start_tick| {
                let mut st = st.borrow_mut();
                let Some(placement) = st.session.add_playlist_placement(pattern, start_tick) else {
                    return;
                };
                if let Some(window) = weak.upgrade() {
                    st.sync_playlist(&window);
                }
                let _ = tx.send(EngineCommand::SetPlaylistPlacement {
                    pattern: placement.pattern,
                    start_tick: placement.start_tick,
                    on: true,
                });
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_playlist_placement_removed(move |pattern, tick| {
                let mut st = st.borrow_mut();
                let Some(placement) = st.session.remove_playlist_placement(pattern, tick) else {
                    return;
                };
                if let Some(window) = weak.upgrade() {
                    st.sync_playlist(&window);
                }
                let _ = tx.send(EngineCommand::SetPlaylistPlacement {
                    pattern: placement.pattern,
                    start_tick: placement.start_tick,
                    on: false,
                });
            });
        }

        // Rack cells summarize all notes starting in their sixteenth. A click
        // adds an anchor note to an empty cell or clears every substep in a
        // populated one; right-click always clears.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_step_clicked(move |channel, step| {
                let mut st = st.borrow_mut();
                let Some(edit) = st.session.toggle_step(channel, step) else {
                    return;
                };
                for cell in edit.redraw {
                    st.refresh_rack_cell(channel as usize, cell);
                }
                if channel as usize == st.session.selected {
                    if let Some(window) = weak.upgrade() {
                        st.refresh_note_editor(&window);
                    }
                }
                for command in edit.commands {
                    let _ = tx.send(command);
                }
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_step_removed(move |channel, step| {
                let mut st = st.borrow_mut();
                let Some(edit) = st.session.clear_step(channel, step) else {
                    return;
                };
                for cell in edit.redraw {
                    st.refresh_rack_cell(channel as usize, cell);
                }
                if channel as usize == st.session.selected {
                    if let Some(window) = weak.upgrade() {
                        st.refresh_note_editor(&window);
                    }
                }
                for command in edit.commands {
                    let _ = tx.send(command);
                }
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_step_velocity_edited(move |channel, step, value| {
                let mut st = st.borrow_mut();
                let Some(edit) = st.session.set_step_velocity(channel, step, value) else {
                    return;
                };
                for cell in edit.redraw {
                    st.refresh_rack_cell(channel as usize, cell);
                }
                if channel as usize == st.session.selected {
                    if let Some(window) = weak.upgrade() {
                        st.refresh_note_editor(&window);
                    }
                }
                for command in edit.commands {
                    let _ = tx.send(command);
                }
            });
        }

        // Paint-drag step editing: idempotent per call so a mouse drag can
        // call this repeatedly over the same cell without toggling it.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_step_painted(move |channel, step, on| {
                let mut st = st.borrow_mut();
                let Some(edit) = st.session.paint_step(channel, step, on) else {
                    return;
                };
                for cell in edit.redraw {
                    st.refresh_rack_cell(channel as usize, cell);
                }
                if channel as usize == st.session.selected {
                    if let Some(window) = weak.upgrade() {
                        st.refresh_note_editor(&window);
                    }
                }
                for command in edit.commands {
                    let _ = tx.send(command);
                }
            });
        }

        // Slice a sixteenth into `divisions` evenly spaced notes.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_step_sliced(move |channel, step, divisions| {
                let mut st = st.borrow_mut();
                let Some(edit) = st.session.slice_step(channel, step, divisions) else {
                    return;
                };
                for cell in edit.redraw {
                    st.refresh_rack_cell(channel as usize, cell);
                }
                if channel as usize == st.session.selected {
                    if let Some(window) = weak.upgrade() {
                        st.refresh_note_editor(&window);
                    }
                }
                for command in edit.commands {
                    let _ = tx.send(command);
                }
            });
        }

        // Drag-resize every note starting in a sixteenth. Called repeatedly
        // during a drag, so no-op durations are skipped and only the cells
        // the note used to (or now does) span are refreshed.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_step_length_dragged(move |channel, step, length_in_steps| {
                let mut st = st.borrow_mut();
                let Some(edit) = st.session.drag_step_length(channel, step, length_in_steps) else {
                    return;
                };
                for cell in edit.redraw {
                    st.refresh_rack_cell(channel as usize, cell);
                }
                if channel as usize == st.session.selected {
                    if let Some(window) = weak.upgrade() {
                        st.refresh_note_editor(&window);
                    }
                }
                for command in edit.commands {
                    let _ = tx.send(command);
                }
            });
        }

        // Tick-addressed piano-roll editing.
        {
            // Setting a length from the picker applies it to the whole
            // selection, which is the only reading that makes sense when
            // more than one note is selected.
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_selected_duration_picked(move |index| {
                let Some(window) = weak.upgrade() else { return };
                let Some((ticks, _)) = MUSICAL_DIVISIONS.get(index.max(0) as usize).copied() else {
                    return;
                };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let Some(edit) = st.session.set_selection_duration(ticks) else {
                    return;
                };
                st.apply_note_edit(&edit, &window);
                for command in edit.commands {
                    let _ = tx.send(command);
                }
                drop(st);
                record_project_history(&commands, before, &history_state, &window, "Note length");
            });
        }
        {
            // Arrow-key editing. The same group clamp the pointer drag uses:
            // a selection that hits the edge stops as one rather than
            // flattening onto it note by note.
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_piano_notes_nudged(move |tick_delta, note_delta| {
                let Some(window) = weak.upgrade() else { return; };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let Some(edit) = st.session.nudge_selection(tick_delta, note_delta) else {
                    return;
                };
                st.apply_note_edit(&edit, &window);
                for command in edit.commands {
                    let _ = tx.send(command);
                }
                drop(st);
                record_project_history(&commands, before, &history_state, &window, "Notes nudged");
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_piano_notes_copied(move |cut| {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let phrase = st.session.selection_phrase();
                if phrase.is_empty() {
                    return;
                }
                let copied = phrase.len();
                commands.borrow_mut().note_clipboard = phrase;
                if !cut {
                    window.set_status_message(format!("Copied {copied} note(s)").into());
                    return;
                }
                let Some(edit) = st.session.delete_selection() else {
                    return;
                };
                st.apply_note_edit(&edit, &window);
                for command in edit.commands {
                    let _ = tx.send(command);
                }
                window.set_status_message(format!("Cut {copied} note(s)").into());
                drop(st);
                record_project_history(&commands, before, &history_state, &window, "Notes cut");
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_piano_notes_pasted(move || {
                let Some(window) = weak.upgrade() else { return };
                let clipboard = commands.borrow().note_clipboard.clone();
                if clipboard.is_empty() {
                    return;
                }
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let Some(edit) = st.session.paste_phrase(&clipboard) else {
                    window.set_status_message("Nothing fits at the paste position".into());
                    return;
                };
                st.apply_note_edit(&edit, &window);
                for command in &edit.commands {
                    let _ = tx.send(*command);
                }
                window.set_status_message(format!("Pasted {} note(s)", edit.notes).into());
                drop(st);
                record_project_history(&commands, before, &history_state, &window, "Notes pasted");
            });
        }
        {
            // A drag's move frames each record an edit. Bracketing them with
            // one token collapses them into a single undo step; see
            // `History::record`.
            let commands = command_state.clone();
            window.on_piano_gesture_begin(move || {
                let mut commands = commands.borrow_mut();
                commands.next_gesture = commands.next_gesture.wrapping_add(1);
                commands.gesture = Some(commands.next_gesture);
            });
        }
        {
            let commands = command_state.clone();
            window.on_piano_gesture_end(move || {
                commands.borrow_mut().gesture = None;
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_piano_note_created(move |start_tick, midi_note, duration_ticks| {
                let Some(window) = weak.upgrade() else {
                    return 0;
                };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let (id, edit) = st
                    .session
                    .create_roll_note(start_tick, midi_note, duration_ticks);
                st.apply_note_edit(&edit, &window);
                for command in edit.commands {
                    let _ = tx.send(command);
                }
                drop(st);
                record_project_history(&commands, before, &history_state, &window, "Note created");
                id as i32
            });
        }
        {
            // Slint's binding-loop checker rejects a self-recursive `pure
            // function`, so a variable-length note list cannot be scanned from
            // .slint at all. The grid's single hit area asks Rust instead.
            // Scans back to front so an overlap resolves to the note drawn on
            // top, matching what the user sees.
            let st = state.clone();
            window.on_piano_note_hit_test(move |tick, midi_note| {
                let st = st.borrow();
                // ModelIterator is not DoubleEndedIterator, so collect first.
                // The list is bounded and small, and this runs on the UI thread
                // in response to pointer motion, not on the audio thread.
                let notes: Vec<NoteCell> = st.note_model.iter().collect();
                note_hit_test(&notes, tick, midi_note)
            });
        }
        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_piano_note_selected(move |id, mode| {
                let mut st = st.borrow_mut();
                if !st.session.select_roll_note(id as NoteId, mode) {
                    return;
                }
                if let Some(window) = weak.upgrade() {
                    st.refresh_note_editor(&window);
                }
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_piano_note_moved(move |id, start_tick, midi_note| {
                let Some(window) = weak.upgrade() else { return; };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let Some(edit) = st.session.move_selection(id as NoteId, start_tick, midi_note) else {
                    return;
                };
                st.apply_note_edit(&edit, &window);
                for command in edit.commands {
                    let _ = tx.send(command);
                }
                drop(st);
                record_project_history(&commands, before, &history_state, &window, "Note moved");
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_piano_selection_duplicated(move |anchor_id| {
                let Some(window) = weak.upgrade() else {
                    return -1;
                };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let Some((anchor_copy, edit)) = st
                    .session
                    .duplicate_selection(anchor_id.max(0) as NoteId)
                else {
                    return -1;
                };
                st.apply_note_edit(&edit, &window);
                for command in edit.commands {
                    let _ = tx.send(command);
                }
                drop(st);
                record_project_history(&commands, before, &history_state, &window, "Notes duplicated");
                // The grid reads -1 as "carry on holding what you had".
                anchor_copy.map_or(-1, |id| id as i32)
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_piano_note_resized(move |id, duration| {
                let Some(window) = weak.upgrade() else { return; };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let Some(edit) = st.session.resize_selection(id as NoteId, duration) else {
                    return;
                };
                st.apply_note_edit(&edit, &window);
                for command in edit.commands {
                    let _ = tx.send(command);
                }
                drop(st);
                record_project_history(&commands, before, &history_state, &window, "Note resized");
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_piano_note_start_resized(move |id, start_tick| {
                let Some(window) = weak.upgrade() else { return; };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let Some(edit) = st.session.resize_selection_start(id as NoteId, start_tick) else {
                    return;
                };
                st.apply_note_edit(&edit, &window);
                for command in edit.commands {
                    let _ = tx.send(command);
                }
                drop(st);
                record_project_history(&commands, before, &history_state, &window, "Note resized");
            });
        }
        {
            // The band updates live, so "add to the selection" has to mean
            // "add to what was selected when the drag started". Recomputing
            // from the live selection each frame would make the band's own
            // previous frame part of its base and the selection would only
            // ever grow.
            let st = state.clone();
            window.on_piano_marquee_begin(move |mode| {
                st.borrow_mut().session.begin_marquee(mode);
            });
        }
        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_piano_marquee_updated(move |start_tick, end_tick, low_note, high_note| {
                let Some(window) = weak.upgrade() else { return };
                let mut st = st.borrow_mut();
                if st
                    .session
                    .update_marquee(start_tick, end_tick, low_note, high_note)
                {
                    st.refresh_note_editor(&window);
                }
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_piano_note_sliced(move |id, tick| {
                let Some(window) = weak.upgrade() else { return; };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let Some(edit) = st.session.slice_note(id as NoteId, tick) else {
                    return;
                };
                st.apply_note_edit(&edit, &window);
                for command in edit.commands {
                    let _ = tx.send(command);
                }
                drop(st);
                record_project_history(&commands, before, &history_state, &window, "Note sliced");
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_piano_selection_joined(move || {
                let Some(window) = weak.upgrade() else { return; };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let Some(edit) = st.session.join_selection() else {
                    return;
                };
                st.apply_note_edit(&edit, &window);
                for command in edit.commands {
                    let _ = tx.send(command);
                }
                drop(st);
                record_project_history(&commands, before, &history_state, &window, "Notes joined");
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_piano_cell_painted(move |start_tick, midi_note, duration_ticks| {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let edit = st
                    .session
                    .paint_roll_note(start_tick, midi_note, duration_ticks);
                st.apply_note_edit(&edit, &window);
                for command in edit.commands {
                    let _ = tx.send(command);
                }
                drop(st);
                record_project_history(&commands, before, &history_state, &window, "Notes painted");
            });
        }
        {
            let st = state.clone();
            window.on_piano_scale_begin(move |from_left| {
                st.borrow_mut().session.begin_scale(from_left);
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_piano_selection_scaled(move |factor| {
                let Some(window) = weak.upgrade() else { return; };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let Some(edit) = st.session.scale_selection(factor) else {
                    return;
                };
                st.apply_note_edit(&edit, &window);
                for command in edit.commands {
                    let _ = tx.send(command);
                }
                drop(st);
                record_project_history(&commands, before, &history_state, &window, "Selection scaled");
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_piano_note_removed(move |id| {
                let Some(window) = weak.upgrade() else { return; };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let Some(edit) = st.session.remove_roll_note(id as NoteId) else {
                    return;
                };
                st.apply_note_edit(&edit, &window);
                for command in edit.commands {
                    let _ = tx.send(command);
                }
                drop(st);
                record_project_history(&commands, before, &history_state, &window, "Note removed");
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_velocity_edited(move |id, value| {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let Some(edit) = st.session.set_note_velocity(id as NoteId, value) else {
                    return;
                };
                st.apply_note_edit(&edit, &window);
                for command in edit.commands {
                    let _ = tx.send(command);
                }
                drop(st);
                record_project_history(
                    &commands,
                    before,
                    &history_state,
                    &window,
                    "Note velocity changed",
                );
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_automation_lane_selected(move |index| {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let Some(command) = st.session.open_automation_lane(index) else {
                    return;
                };
                let _ = tx.send(command);
                st.refresh_automation(&window);
                drop(st);
                // An open lane is saved state even before it has a point in it, so
                // opening one has to mark the document dirty.
                record_project_history(
                    &commands,
                    before,
                    &history_state,
                    &window,
                    "Automation lane opened",
                );
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_automation_lane_cleared(move || {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let Some(command) = st.session.clear_automation_lane() else {
                    return;
                };
                let _ = tx.send(command);
                st.refresh_automation(&window);
                drop(st);
                record_project_history(&commands, before, &history_state, &window, "Automation cleared");
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_automation_lane_closed(move || {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let Some(command) = st.session.close_automation_lane() else {
                    return;
                };
                let _ = tx.send(command);
                st.refresh_automation(&window);
                drop(st);
                record_project_history(
                    &commands,
                    before,
                    &history_state,
                    &window,
                    "Automation lane removed",
                );
            });
        }
        {
            let st = state.clone();
            window.on_automation_point_hit_test(move |tick, value, tolerance| {
                st.borrow()
                    .session
                    .automation_point_at(tick, value, tolerance)
                    .map_or(-1, |id| id as i32)
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_automation_point_created(move |tick, value| {
                let Some(window) = weak.upgrade() else {
                    return -1;
                };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let Some((id, command)) = st.session.create_automation_point(tick, value) else {
                    return -1;
                };
                let _ = tx.send(command);
                st.refresh_automation_points(&window);
                drop(st);
                record_project_history(
                    &commands,
                    before,
                    &history_state,
                    &window,
                    "Automation point added",
                );
                id as i32
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_automation_point_moved(move |id, tick, value| {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let Some(command) = st
                    .session
                    .move_automation_point(id.max(0) as PointId, tick, value)
                else {
                    return;
                };
                let _ = tx.send(command);
                st.refresh_automation_points(&window);
                drop(st);
                record_project_history(
                    &commands,
                    before,
                    &history_state,
                    &window,
                    "Automation point moved",
                );
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_automation_point_removed(move |id| {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let Some(command) = st.session.remove_automation_point(id.max(0) as PointId) else {
                    return;
                };
                let _ = tx.send(command);
                st.refresh_automation_points(&window);
                drop(st);
                record_project_history(
                    &commands,
                    before,
                    &history_state,
                    &window,
                    "Automation point removed",
                );
            });
        }

        macro_rules! wire_selected_note_edit {
            ($callback:ident, $field:ident, $value:expr) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                let weak = window.as_weak();
                window.$callback(move |value| {
                    let mut st = st.borrow_mut();
                    let pattern = st.session.current_pattern;
                    let channel = st.session.selected;
                    let Some(id) = st.session.selected_note_id else {
                        return;
                    };
                    let length_ticks = st.session.pattern_lengths[pattern] as u32 * TICKS_PER_STEP;
                    let Some(note) = st.session.channels[channel].notes[pattern]
                        .iter_mut()
                        .find(|note| note.id == id)
                    else {
                        return;
                    };
                    note.$field = $value(value, &mut *note, length_ticks);
                    let edited = *note;
                    st.refresh_rack_cell(channel, (edited.start_tick / TICKS_PER_STEP) as usize);
                    if let Some(window) = weak.upgrade() {
                        st.refresh_note_editor(&window);
                    }
                    let _ = tx.send(EngineCommand::UpsertNote {
                        pattern: pattern as u8,
                        channel: channel as u8,
                        note: edited,
                    });
                });
            }};
        }
        wire_selected_note_edit!(on_selected_note_changed, note, |value: i32, _, _| value
            .clamp(0, 127)
            as u8);
        wire_selected_note_edit!(
            on_selected_velocity_changed,
            velocity,
            |value: i32, _, _| value.clamp(1, 127) as u8
        );

        // Ctrl+A: select every note in the current channel's current
        // pattern. Shares the same selection set Shift/Ctrl-click builds.
        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_select_all_requested(move || {
                let Some(window) = weak.upgrade() else { return };
                let mut st = st.borrow_mut();
                let channel = st.session.selected;
                st.session.select_all_notes(channel);
                st.refresh_note_editor(&window);
            });
        }

        // Delete/Backspace, or the Edit menu row: removes every note in the
        // current selection, whether that's one note or a whole Select-All.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_delete_selected_notes_requested(move || {
                let Some(window) = weak.upgrade() else { return };
                if st.borrow().session.selected_note_ids.is_empty() {
                    return;
                }
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let Some(edit) = st.session.delete_selection() else {
                    return;
                };
                st.apply_note_edit(&edit, &window);
                for command in edit.commands {
                    let _ = tx.send(command);
                }
                drop(st);
                record_project_history(&commands, before, &history_state, &window, "Notes deleted");
            });
        }

        // Channel selection (for the bottom editor).
        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_channel_selected(move |ch| {
                let Some(ch) = st.borrow_mut().session.select_channel(ch) else {
                    return;
                };
                if let Some(w) = weak.upgrade() {
                    w.set_selected_channel(ch as i32);
                    let guard = st.borrow();
                    guard.sync_row_flags();
                    guard.sync_mixer_selection();
                    guard.sync_bus_editor(&w);
                    guard.refresh_editor(&w);
                    drop(guard);
                    refresh_preset_menus(&st, &w);
                }
            });
        }

        // Channel mute.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_channel_muted(move |ch| {
                let mut st = st.borrow_mut();
                let Some(command) = st.session.toggle_channel_mute(ch) else {
                    return;
                };
                st.sync_row_flags();
                let _ = tx.send(command);
            });
        }

        // Channel output level and pan.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_channel_volume_changed(move |ch, volume| {
                let mut st = st.borrow_mut();
                let Some(command) = st.session.set_channel_volume(ch, volume) else {
                    return;
                };
                st.sync_row_flags();
                // The source device's output-trim knob is the same parameter;
                // restate it or its readout freezes at whatever the channel
                // had when it was selected.
                if ch as usize == st.session.selected {
                    if let Some(w) = weak.upgrade() {
                        w.set_selected_channel_volume_db(linear_to_db(
                            st.session.channels[st.session.selected].volume,
                        ));
                    }
                }
                let _ = tx.send(command);
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_channel_pan_changed(move |ch, pan| {
                let mut st = st.borrow_mut();
                let Some(command) = st.session.set_channel_pan(ch, pan) else {
                    return;
                };
                st.sync_row_flags();
                let _ = tx.send(command);
            });
        }

        // Add, replace, or remove channel sources.
        {
            let tx = cmd_tx.clone();
            let reset_tx = sample_reset_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_channel_source_changed(move |value| {
                let source = device_kind_from_int(value);
                let channel = {
                    let mut guard = st.borrow_mut();
                    let Some(channel) = guard.session.change_selected_source(source) else {
                        return;
                    };
                    guard.sync_row_flags();
                    channel
                };
                if let Some(window) = weak.upgrade() {
                    st.borrow().refresh_editor(&window);
                    refresh_preset_menus(&st, &window);
                }
                let _ = reset_tx.send(channel);
                let _ = tx.send(EngineCommand::SetChannelSource {
                    channel: channel as u8,
                    source,
                });
            });
        }
        {
            let add_channel_tx = structural_tx.clone();
            let reset_tx = sample_reset_tx.clone();
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_add_channel_clicked(move |value| {
                if commands.borrow().project_edit_pending {
                    return;
                }
                let source = device_kind_from_int(value);
                let mut st = st.borrow_mut();
                let Some(index) = st.session.add_channel(source) else {
                    return;
                };
                log_debug!("ui", "add channel");
                let pattern = st.session.current_pattern;
                let ch = &st.session.channels[index];
                let cells: Vec<StepCell> = (0..st.session.pattern_lengths[pattern])
                    .map(|step| rack_cell(&ch.notes[pattern], step))
                    .collect();
                let model = Rc::new(VecModel::from(cells));
                let row = ChannelRow {
                    name: ch.name.as_str().into(),
                    muted: false,
                    volume_db: linear_to_db(ch.volume),
                    pan: ch.pan,
                    selected: true,
                    bus: ch.bus as i32,
                    steps: ModelRc::from(model.clone()),
                };
                st.rows.push(row);
                st.step_models.push(model);
                st.sync_row_flags();
                if let Some(window) = weak.upgrade() {
                    st.sync_mixer(&window);
                    window.set_selected_channel(index as i32);
                    st.refresh_editor(&window);
                }
                let _ = reset_tx.send(index);
                let _ = add_channel_tx.add_channel(index, source);
            });
        }
        {
            let st = state.clone();
            let commands = command_state.clone();
            let tx = project_edit_tx.clone();
            let weak = window.as_weak();
            window.on_remove_channel_clicked(move || {
                let Some(window) = weak.upgrade() else { return };
                if commands.borrow().project_edit_pending {
                    return;
                }
                let selected = st.borrow().session.selected;
                if queue_channel_delete(&tx, &st, &window, selected, "Channel deleted") {
                    commands.borrow_mut().project_edit_pending = true;
                    sync_command_availability(&window, &commands.borrow());
                }
            });
        }
        // All channel-edit surfaces arrive here.  The menu bar, Ctrl keys,
        // and per-row context menu deliberately know only command ids; they
        // cannot grow separate mutation paths.
        {
            let st = state.clone();
            let commands = command_state.clone();
            let tx = project_edit_tx.clone();
            let weak = window.as_weak();
            window.on_edit_command_requested(move |kind, index| {
                let Some(window) = weak.upgrade() else { return };
                let Ok(index) = usize::try_from(index) else {
                    return;
                };
                if kind != 3 && commands.borrow().project_edit_pending {
                    return;
                }
                match kind {
                    // Undo and redo use a target snapshot but only advance
                    // their cursor in the pump after installation succeeds.
                    0 => {
                        let entry = commands.borrow().history.undo_target().cloned();
                        if let Some(entry) = entry {
                            if queue_history_target(&tx, entry, HistoryMove::Undo) {
                                commands.borrow_mut().project_edit_pending = true;
                                sync_command_availability(&window, &commands.borrow());
                            }
                        }
                    }
                    1 => {
                        let entry = commands.borrow().history.redo_target().cloned();
                        if let Some(entry) = entry {
                            if queue_history_target(&tx, entry, HistoryMove::Redo) {
                                commands.borrow_mut().project_edit_pending = true;
                                sync_command_availability(&window, &commands.borrow());
                            }
                        }
                    }
                    2 => {
                        let Some(copy) = st.borrow().session.channel_clipboard(
                            index,
                            window.get_bpm(),
                            window.get_swing_percent(),
                        )
                        else {
                            return;
                        };
                        if st.borrow().session.channels.len() <= 1 {
                            return;
                        }
                        commands.borrow_mut().channel_clipboard = Some(copy);
                        sync_command_availability(&window, &commands.borrow());
                        if queue_channel_delete(&tx, &st, &window, index, "Channel cut") {
                            commands.borrow_mut().project_edit_pending = true;
                            sync_command_availability(&window, &commands.borrow());
                        }
                    }
                    3 => {
                        let Some(copy) = st.borrow().session.channel_clipboard(
                            index,
                            window.get_bpm(),
                            window.get_swing_percent(),
                        )
                        else {
                            return;
                        };
                        commands.borrow_mut().channel_clipboard = Some(copy);
                        sync_command_availability(&window, &commands.borrow());
                        window.set_status_message("Channel copied".into());
                    }
                    4 => {
                        let Some(copy) = commands.borrow().channel_clipboard.clone() else {
                            return;
                        };
                        if queue_channel_insert(&tx, &st, &window, index, copy, "Channel pasted") {
                            commands.borrow_mut().project_edit_pending = true;
                            sync_command_availability(&window, &commands.borrow());
                        }
                    }
                    5 => {
                        let Some(copy) = st.borrow().session.channel_clipboard(
                            index,
                            window.get_bpm(),
                            window.get_swing_percent(),
                        )
                        else {
                            return;
                        };
                        if queue_channel_insert(&tx, &st, &window, index, copy, "Channel cloned") {
                            commands.borrow_mut().project_edit_pending = true;
                            sync_command_availability(&window, &commands.borrow());
                        }
                    }
                    6 if queue_channel_delete(&tx, &st, &window, index, "Channel deleted") => {
                        commands.borrow_mut().project_edit_pending = true;
                        sync_command_availability(&window, &commands.borrow());
                    }
                    _ => {}
                }
            });
        }

        // Pattern clone/remove reuse the same whole-project undo pipeline
        // channel cut/copy/paste/clone/delete use above: mutate a `Project`
        // snapshot's pattern-indexed vectors and queue it as one undoable
        // edit, rather than a bespoke realtime engine command.
        {
            let st = state.clone();
            let commands = command_state.clone();
            let tx = project_edit_tx.clone();
            let weak = window.as_weak();
            window.on_pattern_clone_requested(move || {
                let Some(window) = weak.upgrade() else { return };
                if commands.borrow().project_edit_pending {
                    return;
                }
                let index = st.borrow().session.current_pattern;
                if queue_pattern_clone(&tx, &st, &window, index, "Pattern cloned") {
                    commands.borrow_mut().project_edit_pending = true;
                    sync_command_availability(&window, &commands.borrow());
                }
            });
        }
        {
            let st = state.clone();
            let commands = command_state.clone();
            let tx = project_edit_tx.clone();
            let weak = window.as_weak();
            window.on_pattern_remove_requested(move || {
                let Some(window) = weak.upgrade() else { return };
                if commands.borrow().project_edit_pending {
                    return;
                }
                let index = st.borrow().session.current_pattern;
                if queue_pattern_remove(&tx, &st, &window, index, "Pattern deleted") {
                    commands.borrow_mut().project_edit_pending = true;
                    sync_command_availability(&window, &commands.borrow());
                }
            });
        }
        {
            let st = state.clone();
            let commands = command_state.clone();
            let tx = project_edit_tx.clone();
            let weak = window.as_weak();
            window.on_pattern_clear_requested(move || {
                let Some(window) = weak.upgrade() else { return };
                if commands.borrow().project_edit_pending {
                    return;
                }
                let index = st.borrow().session.current_pattern;
                if queue_pattern_clear(&tx, &st, &window, index, "Pattern cleared") {
                    commands.borrow_mut().project_edit_pending = true;
                    sync_command_availability(&window, &commands.borrow());
                }
            });
        }

        // --- Mixer: bus selection, strip controls, and routing ---
        {
            let weak = window.as_weak();
            let st = state.clone();
            window.on_bus_selected(move |bus| {
                if st.borrow_mut().session.select_bus(bus).is_none() {
                    return;
                }
                let guard = st.borrow();
                guard.sync_mixer_selection();
                guard.sync_effects();
                if let Some(w) = weak.upgrade() {
                    guard.sync_bus_editor(&w);
                }
            });
        }

        {
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            let st = state.clone();
            window.on_bus_muted(move |bus| {
                let mut guard = st.borrow_mut();
                let Some(command) = guard.session.toggle_bus_mute(bus) else {
                    return;
                };
                guard.sync_mixer_strip(bus as usize);
                if let Some(w) = weak.upgrade() {
                    guard.sync_bus_editor(&w);
                }
                let _ = tx.send(command);
            });
        }

        {
            let telemetry = telemetry_tx.clone();
            let st = state.clone();
            window.on_eq_analyzer_changed(move |slot, enabled| {
                let mut st = st.borrow_mut();
                let Some((target, slot)) = st.session.set_eq_analyzer(slot, enabled) else {
                    return;
                };
                st.refresh_effect_row(slot as usize);
                let _ = telemetry.send(TelemetryAction::SetEffectSpectrumEnabled {
                    target,
                    slot,
                    enabled,
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            let st = state.clone();
            window.on_bus_volume_changed(move |bus, volume| {
                let mut guard = st.borrow_mut();
                let Some(command) = guard.session.set_bus_volume(bus, volume) else {
                    return;
                };
                guard.sync_mixer_strip(bus as usize);
                if let Some(w) = weak.upgrade() {
                    guard.sync_bus_editor(&w);
                }
                let _ = tx.send(command);
            });
        }

        {
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            let st = state.clone();
            window.on_bus_pan_changed(move |bus, pan| {
                let mut guard = st.borrow_mut();
                let Some(command) = guard.session.set_bus_pan(bus, pan) else {
                    return;
                };
                guard.sync_mixer_strip(bus as usize);
                if let Some(w) = weak.upgrade() {
                    guard.sync_bus_editor(&w);
                }
                let _ = tx.send(command);
            });
        }

        {
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            let st = state.clone();
            window.on_bus_output_changed(move |bus, output| {
                let mut guard = st.borrow_mut();
                match guard.session.set_bus_output(bus, output) {
                    Some(Ok(command)) => {
                        // Every strip's legal destinations move when an edge does.
                        if let Some(w) = weak.upgrade() {
                            guard.sync_mixer(&w);
                        }
                        let _ = tx.send(command);
                    }
                    Some(Err(refused)) => {
                        if let Some(w) = weak.upgrade() {
                            w.set_status_message(
                                format!(
                                    "{} already feeds this bus - routing would loop",
                                    refused.feeder
                                )
                                .into(),
                            );
                        }
                    }
                    None => {}
                }
            });
        }

        {
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            let st = state.clone();
            window.on_channel_bus_changed(move |channel, bus| {
                let mut guard = st.borrow_mut();
                let Some(command) = guard.session.set_channel_bus(channel, bus) else {
                    return;
                };
                guard.sync_row_flags();
                // Feed counts moved, so both the old and new bus restate them.
                if let Some(w) = weak.upgrade() {
                    guard.sync_mixer(&w);
                }
                let _ = tx.send(command);
            });
        }

        // --- Channel modulation shelf -------------------------------------
        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_modulation_shelf_toggled(move || {
                let Some(window) = weak.upgrade() else { return };
                let mut state = st.borrow_mut();
                state.session.toggle_modulation_shelf();
                state.refresh_modulation(&window);
            });
        }
        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_modulation_source_selected(move |slot| {
                let Some(window) = weak.upgrade() else { return };
                let mut state = st.borrow_mut();
                if !state.session.select_modulation_source(slot) {
                    return;
                }
                state.refresh_modulation(&window);
            });
        }
        // Reordering the grid. The rack compacts as it moves, so the target
        // is a position among the occupied modules; routes follow by
        // identity and a math module's input is remapped by the rack.
        {
            let st = state.clone();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_modulation_source_moved(move |slot, target| {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                {
                    let mut state = st.borrow_mut();
                    let Some(command) = state.session.move_modulation_source(slot, target) else {
                        return;
                    };
                    state.send_modulation(&window, &tx, command);
                }
                record_project_history(&commands, before, &st, &window, "Module moved");
            });
        }
        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_modulation_assignment_toggled(move || {
                let Some(window) = weak.upgrade() else { return };
                let mut state = st.borrow_mut();
                let armed = state.session.toggle_modulation_assignment();
                state.refresh_modulation(&window);
                window.set_status_message(match armed {
                    Some(source) => format!(
                        "Assigning {source} \u{2014} drag a highlighted control to set route depth"
                    )
                    .into(),
                    None => "Modulation assignment off \u{2014} controls edit their base values".into(),
                });
            });
        }
        // One add verb for every kind: the menu chooses a `ModulatorKind`
        // and the slot is filled from that kind's own defaults, so a new
        // module family costs a menu entry rather than a callback.
        {
            let st = state.clone();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_modulation_source_added(move |kind| {
                let (Some(window), Some(kind)) = (weak.upgrade(), ModulatorKind::from_index(kind)) else {
                    return;
                };
                let before = project_snapshot(&st.borrow(), &window);
                {
                    let mut state = st.borrow_mut();
                    let Some(command) = state.session.add_modulation_source(kind) else {
                        return;
                    };
                    state.send_modulation(&window, &tx, command);
                }
                // History labels are `&'static str`, so the per-kind wording is a
                // match rather than a format.
                let (history, status) = match kind {
                    ModulatorKind::Lfo => (
                        "LFO added",
                        "LFO added \u{2014} choose Assign when you are ready to route it",
                    ),
                    ModulatorKind::Envelope => (
                        "Envelope added",
                        "Envelope added \u{2014} choose its gate input, then Assign a destination",
                    ),
                    ModulatorKind::Step => (
                        "Step sequencer added",
                        "Step sequencer added \u{2014} drag the columns to draw a pattern",
                    ),
                    ModulatorKind::Random => (
                        "Random source added",
                        "Random source added \u{2014} choose Assign when you are ready to route it",
                    ),
                    ModulatorKind::Math => (
                        "Math module added",
                        "Math module added \u{2014} choose the slot it reads, then Assign it",
                    ),
                };
                record_project_history(&commands, before, &st, &window, history);
                window.set_status_message(status.into());
            });
        }
        // One descriptor-addressed edit path for every modulator parameter.
        // A knob drag arrives bracketed by edit-started/finished and lands as
        // one undo step; a discrete edit (selector click, LED toggle) arrives
        // bare and records immediately.
        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_modulation_param_edit_started(move || {
                let Some(window) = weak.upgrade() else { return };
                st.borrow_mut().begin_modulation_edit(&window);
            });
        }
        {
            let st = state.clone();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_modulation_param_changed(move |slot, id, value| {
                let Some(window) = weak.upgrade() else { return };
                // A knob gesture owns one undo entry, recorded on release; outside one,
                // every change is its own.
                let in_gesture = st.borrow().session.modulation_gesture_open();
                let before = (!in_gesture).then(|| project_snapshot(&st.borrow(), &window));
                {
                    let mut state = st.borrow_mut();
                    let Some(command) = state.session.set_modulator_param(slot, id, value) else {
                        return;
                    };
                    state.send_modulation(&window, &tx, command);
                }
                if let Some(before) = before {
                    record_project_history(&commands, before, &st, &window, "Modulator edited");
                }
            });
        }
        {
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_modulation_param_edit_finished(move || {
                let Some(window) = weak.upgrade() else { return };
                let before = st.borrow_mut().session.finish_modulation_edit();
                if let Some(before) = before {
                    record_project_history(&commands, before, &st, &window, "Modulator edited");
                }
            });
        }
        // Removing a source drops the slot and every route it feeds; the
        // engine restores those destinations' bases through the
        // `set_channel_modulation` diff.
        {
            let st = state.clone();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_modulation_source_removed(move |slot| {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                {
                    let mut state = st.borrow_mut();
                    let Some(command) = state.session.remove_modulation_source(slot) else {
                        return;
                    };
                    state.send_modulation(&window, &tx, command);
                }
                record_project_history(&commands, before, &st, &window, "Modulator removed");
            });
        }
        {
            let st = state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_modulation_envelope_input_channel_changed(move |slot, channel| {
                let Some(window) = weak.upgrade() else { return };
                let mut state = st.borrow_mut();
                // The gate is a jack rather than a descriptor id, so there is no
                // parameter to name: the module travels entire.
                let Some(command) = state.session.set_envelope_input_channel(slot, channel) else {
                    return;
                };
                state.send_modulation(&window, &tx, command);
            });
        }
        {
            let st = state.clone();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_modulation_route_polarity_changed(move |index, polarity| {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                {
                    let mut state = st.borrow_mut();
                    let Some(command) = state.session.set_route_polarity(index, polarity) else {
                        return;
                    };
                    state.send_modulation(&window, &tx, command);
                }
                record_project_history(
                    &commands,
                    before,
                    &st,
                    &window,
                    "Modulation polarity changed",
                );
            });
        }
        {
            let st = state.clone();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_modulation_route_removed(move |index| {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                {
                    let mut state = st.borrow_mut();
                    let Some(command) = state.session.remove_route(index) else {
                        return;
                    };
                    state.send_modulation(&window, &tx, command);
                }
                record_project_history(&commands, before, &st, &window, "Modulation route removed");
            });
        }

        // A direct parameter gesture begins, streams live route-depth
        // updates, then records one history entry on release. The same path
        // serves every source face that exposes an eligible cutoff.
        // Generator and strip destinations share one addressed path: the
        // control names its own parameter id, so adding a routable knob is a
        // binding on that knob rather than another callback triple here.
        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_source_modulation_edit_started(move |_| {
                let Some(window) = weak.upgrade() else { return };
                st.borrow_mut().begin_modulation_edit(&window);
            });
        }
        {
            let st = state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_source_modulation_depth_changed(move |param, depth| {
                let (Some(window), Ok(param)) = (weak.upgrade(), u32::try_from(param)) else {
                    return;
                };
                let mut state = st.borrow_mut();
                let destination = ParamAddr {
                    scope: EffectTarget::Channel(state.session.selected as u8),
                    owner: ParamOwner::Source,
                    param,
                };
                if !state.set_armed_modulation_depth(&window, &tx, destination, depth) {
                    // A full matrix or invalid target must snap the transient
                    // UI depth back to persisted truth rather than pretending
                    // a parked route was written.
                    state.refresh_modulation(&window);
                }
            });
        }
        {
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_source_modulation_edit_finished(move |_| {
                let Some(window) = weak.upgrade() else { return };
                let before = st.borrow_mut().session.finish_modulation_edit();
                if let Some(before) = before {
                    record_project_history(
                        &commands,
                        before,
                        &st,
                        &window,
                        "Modulation route changed",
                    );
                }
            });
        }
        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_strip_modulation_edit_started(move |_| {
                let Some(window) = weak.upgrade() else { return };
                st.borrow_mut().begin_modulation_edit(&window);
            });
        }
        {
            let st = state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_strip_modulation_depth_changed(move |param, depth| {
                let (Some(window), Ok(param)) = (weak.upgrade(), u32::try_from(param)) else {
                    return;
                };
                let mut state = st.borrow_mut();
                let destination =
                    ParamAddr::strip(EffectTarget::Channel(state.session.selected as u8), param);
                if !state.set_armed_modulation_depth(&window, &tx, destination, depth) {
                    state.refresh_modulation(&window);
                }
            });
        }
        {
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_strip_modulation_edit_finished(move |_| {
                let Some(window) = weak.upgrade() else { return };
                let before = st.borrow_mut().session.finish_modulation_edit();
                if let Some(before) = before {
                    record_project_history(
                        &commands,
                        before,
                        &st,
                        &window,
                        "Modulation route changed",
                    );
                }
            });
        }
        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_effect_modulation_edit_started(move |slot, param| {
                let (Some(window), Ok(slot), Ok(param)) = (
                    weak.upgrade(),
                    usize::try_from(slot),
                    u32::try_from(param),
                ) else {
                    return;
                };
                let mut state = st.borrow_mut();
                let valid = matches!(state.session.effect_target, EffectTarget::Channel(channel) if channel as usize == state.session.selected)
                    && state
                        .session.channels
                        .get(state.session.selected)
                        .and_then(|channel| channel.effects.get(slot))
                        .and_then(|effect| effect.kind().descriptor(param))
                        .is_some();
                if valid {
                    state.begin_modulation_edit(&window);
                }
            });
        }
        {
            let st = state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_effect_modulation_depth_changed(move |slot, param, depth| {
                let (Some(window), Ok(slot), Ok(param)) =
                    (weak.upgrade(), usize::try_from(slot), u32::try_from(param))
                else {
                    return;
                };
                let mut state = st.borrow_mut();
                let destination = match state.session.effect_target {
                    EffectTarget::Channel(channel) if channel as usize == state.session.selected => state
                        .session.channels
                        .get(state.session.selected)
                        .and_then(|channel| channel.effects.get(slot))
                        .and_then(|effect| effect.kind().descriptor(param))
                        .map(|descriptor| {
                            ParamAddr::effect(
                                EffectTarget::Channel(channel),
                                slot as u8,
                                descriptor.id,
                            )
                        }),
                    _ => None,
                };
                let Some(destination) = destination else {
                    return;
                };
                if !state.set_armed_modulation_depth(&window, &tx, destination, depth) {
                    state.refresh_modulation(&window);
                }
            });
        }
        {
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_effect_modulation_edit_finished(move |_, _| {
                let Some(window) = weak.upgrade() else { return };
                let before = st.borrow_mut().session.finish_modulation_edit();
                if let Some(before) = before {
                    record_project_history(
                        &commands,
                        before,
                        &st,
                        &window,
                        "Modulation route changed",
                    );
                }
            });
        }

        // --- Effect chain callbacks (edit whatever the rack is pointed at) ---
        //
        // Each structural edit is one permutation of the chain, computed by
        // `mooloop_core::structure` and applied here to the model, its routes
        // and its lanes, then mirrored on the engine with the two realtime
        // primitives it has: a structural install/remove at the vacant tail,
        // and a pointer-rotating move. The engine runs the same table over
        // its own routes and lanes for the same command.
        {
            let tx = cmd_tx.clone();
            let stx = structural_tx.clone();
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_add_effect_clicked(move |kind_index, insert_before| {
                let Some(kind) = effect_kind_from_index(kind_index) else {
                    return;
                };
                let Some(window) = weak.upgrade() else { return };
                let Ok(insert_before) = usize::try_from(insert_before) else {
                    return;
                };
                let before = project_snapshot(&st.borrow(), &window);
                {
                    let mut st = st.borrow_mut();
                    let Some(added) = st.session.insert_effect_at(kind, insert_before) else {
                        return;
                    };
                    st.sync_effects();
                    st.refresh_automation(&window);
                    st.refresh_modulation(&window);
                    // The node and its dry-align ring are built here because
                    // construction allocates: off the audio thread, riding the same
                    // structural command as the slot they belong to.
                    let bpm = window.get_bpm() as f64;
                    let node = build_effect_at_tempo(added.params, sample_rate, bpm);
                    let align = DryAlign::new(node.dry_path_latency_frames()).map(Box::new);
                    let _ = stx.send(StructuralCommand::InstallEffect {
                        target: added.target,
                        slot: added.tail as u8,
                        kind: added.kind,
                        resource_key: added.params.buffer().copied().map(buffer_allocation_key),
                        node,
                        align,
                        analyzer: Box::new(SpectrumAnalyzer::new()),
                        // Allocated here with the node: an empty addressable slot
                        // costs a pointer rather than its full host state.
                        state: Box::new(EffectSlot::new()),
                    });
                    if added.slot != added.tail {
                        let _ = tx.send(EngineCommand::MoveEffect {
                            target: added.target,
                            from: added.tail as u8,
                            to: added.slot as u8,
                        });
                    }
                }
                record_project_history(&commands, before, &st, &window, "Effect added");
            });
        }

        {
            let tx = cmd_tx.clone();
            let stx = structural_tx.clone();
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_remove_effect_clicked(move |slot| {
                let Some(window) = weak.upgrade() else { return };
                let Ok(slot) = usize::try_from(slot) else {
                    return;
                };
                let before = project_snapshot(&st.borrow(), &window);
                {
                    let mut st = st.borrow_mut();
                    let Some(removed) = st.session.remove_effect_at(slot) else {
                        return;
                    };
                    st.sync_effects();
                    st.refresh_automation(&window);
                    st.refresh_modulation(&window);
                    // Mirror on the engine: move the device to the vacated tail, then
                    // drop the tail. Its routes and lanes ride along and go with it.
                    if removed.slot != removed.tail {
                        let _ = tx.send(EngineCommand::MoveEffect {
                            target: removed.target,
                            from: removed.slot as u8,
                            to: removed.tail as u8,
                        });
                    }
                    let _ = stx.send(StructuralCommand::RemoveEffect {
                        target: removed.target,
                        slot: removed.tail as u8,
                    });
                }
                record_project_history(&commands, before, &st, &window, "Effect removed");
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_effect_bypass_toggled(move |slot| {
                let mut st = st.borrow_mut();
                let Some(command) = st.session.toggle_effect_bypass(slot) else {
                    return;
                };
                st.refresh_effect_row(slot as usize);
                let _ = tx.send(command);
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_effect_wet_dry_changed(move |slot, wet_dry| {
                let mut st = st.borrow_mut();
                let Some(command) = st.session.set_effect_wet_dry(slot, wet_dry) else {
                    return;
                };
                st.refresh_effect_row(slot as usize);
                let _ = tx.send(command);
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_effect_input_trim_changed(move |slot, input_trim_db| {
                let mut st = st.borrow_mut();
                let Some(command) = st.session.set_effect_input_trim(slot, input_trim_db) else {
                    return;
                };
                st.refresh_effect_row(slot as usize);
                let _ = tx.send(command);
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_effect_output_trim_changed(move |slot, output_trim_db| {
                let mut st = st.borrow_mut();
                let Some(command) = st.session.set_effect_output_trim(slot, output_trim_db) else {
                    return;
                };
                st.refresh_effect_row(slot as usize);
                let _ = tx.send(command);
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            // The buffer device's debug triggers. Unlike a parameter change
            // this never touches project state: a fired event is a transient
            // performance gesture, not something the project remembers.
            let rtx = cmd_tx.clone();
            let rst = state.clone();
            window.on_effect_buffer_reverse_pressed(move |slot| {
                let Ok(slot) = u8::try_from(slot) else {
                    return;
                };
                let _ = rtx.send(EngineCommand::TriggerBuffer {
                    target: rst.borrow().session.effect_target,
                    slot,
                    event: held_reverse_event(),
                });
            });
            let rtx = cmd_tx.clone();
            let rst = state.clone();
            window.on_effect_buffer_reverse_released(move |slot| {
                let Ok(slot) = u8::try_from(slot) else {
                    return;
                };
                // Inert unless the gated head is still the one running, so a
                // release can never cancel an event that superseded it.
                let _ = rtx.send(EngineCommand::ReleaseBuffer {
                    target: rst.borrow().session.effect_target,
                    slot,
                });
            });
            window.on_effect_buffer_triggered(move |slot, trigger| {
                let Some(event) = debug_buffer_event(trigger) else {
                    return;
                };
                let Ok(slot) = u8::try_from(slot) else {
                    return;
                };
                // The tuple travels whole, never split into parameter
                // updates, so the read head sees one sample-accurate change.
                let _ = tx.send(EngineCommand::TriggerBuffer {
                    target: st.borrow().session.effect_target,
                    slot,
                    event,
                });
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            // One callback for every parameter of every effect kind: the
            // rack sends a descriptor index and a normalized position, and
            // the descriptor table converts to the natural units the wire
            // and the DSP use.
            window.on_effect_param_changed(move |slot, param_index, normalized| {
                let mut st = st.borrow_mut();
                let Some(command) = st.session.set_effect_param(slot, param_index, normalized) else {
                    return;
                };
                st.refresh_effect_row(slot as usize);
                let _ = tx.send(command);
            });
        }

        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_delay_tempo_sync_changed(move |slot, enabled| {
                let mut state = st.borrow_mut();
                if !state.session.set_delay_tempo_sync(slot, enabled) {
                    return;
                }
                state.refresh_effect_row(slot as usize);
                if let Some(window) = weak.upgrade() {
                    state.update_document_title(&window);
                }
            });
        }

        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_delay_time_division_changed(move |slot, division| {
                let mut state = st.borrow_mut();
                if !state.session.set_delay_time_division(slot, division) {
                    return;
                }
                state.refresh_effect_row(slot as usize);
                if let Some(window) = weak.upgrade() {
                    state.update_document_title(&window);
                }
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_reorder_effect(move |from, to| {
                let Some(window) = weak.upgrade() else { return };
                let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else {
                    return;
                };
                let before = project_snapshot(&st.borrow(), &window);
                {
                    let mut st = st.borrow_mut();
                    let Some(target) = st.session.move_effect_to(from, to) else {
                        return;
                    };
                    st.sync_effects();
                    st.refresh_automation(&window);
                    st.refresh_modulation(&window);
                    let _ = tx.send(EngineCommand::MoveEffect {
                        target,
                        from: from as u8,
                        to: to as u8,
                    });
                }
                record_project_history(&commands, before, &st, &window, "Effect moved");
            });
        }

        // --- Sampler parameter callbacks (edit the selected channel) ---
        macro_rules! wire_time_param {
            ($on:ident, $field:ident) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                window.$on(move |v: f32| {
                    let mut st = st.borrow_mut();
                    let ch = st.session.selected;
                    let Some(channel) = st.session.channels.get_mut(ch) else {
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
                    let ch = st.session.selected;
                    let Some(channel) = st.session.channels.get_mut(ch) else {
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
        // The four markers do not use `wire_unit_param!`: each one may be
        // resolved onto a zero crossing on the way in, and the resolved value
        // has to travel back to the face so the control agrees with what was
        // stored.
        macro_rules! wire_marker_param {
            ($on:ident, $marker:expr) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                let window_weak = window.as_weak();
                window.$on(move |v: f32| {
                    let Some(window) = window_weak.upgrade() else {
                        return;
                    };
                    let marker = $marker;
                    let (value, status) = {
                        let mut st = st.borrow_mut();
                        let ch = st.session.selected;
                        let Some(channel) = st.session.channels.get_mut(ch) else {
                            return;
                        };
                        let mut value = v;
                        let mut status = None;
                        if window.get_snap_to_zero() {
                            if let Some(sample) = channel.published_sample().cloned() {
                                if let Some((resolved, result)) =
                                    snap_marker(&channel.params, &sample, marker, v)
                                {
                                    value = resolved;
                                    status = Some(snap_status(marker, result));
                                }
                            }
                        }
                        marker.set(&mut channel.params, value);
                        let p = channel.params;
                        let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                            channel: ch as u8,
                            params: p,
                        });
                        (value, status)
                    };
                    set_marker_property(&window, marker, value);
                    if let Some(status) = status {
                        window.set_status_message(status.into());
                    }
                });
            }};
        }

        wire_marker_param!(on_start_pos_changed, SampleMarker::Start);
        wire_marker_param!(on_end_pos_changed, SampleMarker::End);
        wire_marker_param!(on_loop_start_changed, SampleMarker::LoopStart);
        wire_marker_param!(on_loop_end_changed, SampleMarker::LoopEnd);

        {
            // Applied now, not on OK: someone turning this on is about to go
            // and reproduce something, and a log that only starts after they
            // confirm a dialog can miss the very run they wanted.
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_preferences_log_to_file_toggled(move |enabled| {
                let path = settings::log_path();
                let started = if enabled {
                    match mooloop_core::log::start_file(&path, &build_description()) {
                        Ok(()) => {
                            log_info!("app", "logging to {}", path.display());
                            true
                        }
                        Err(error) => {
                            log_error!(
                                "app",
                                "could not write the log to {}: {error}",
                                path.display()
                            );
                            if let Some(window) = weak.upgrade() {
                                window.set_preferences_error(
                                    format!("Could not write {}: {error}", path.display()).into(),
                                );
                            }
                            false
                        }
                    }
                } else {
                    log_info!("app", "logging to file switched off");
                    mooloop_core::log::stop_file();
                    false
                };
                // Persist what actually happened, not what was asked for: a
                // preference recorded as on when the file could not be opened
                // would fail again silently on every future run.
                let mut settings = settings.borrow_mut();
                settings.general.log_to_file = started;
                let _ = settings.save();
                if let Some(window) = weak.upgrade() {
                    window.set_preferences_log_to_file(started);
                }
            });
        }

        {
            // The toggle is a user preference, so it outlives the project. A
            // failed save leaves the session's choice in place rather than
            // fighting the user over a checkbox.
            let settings = ui_settings.clone();
            window.on_snap_to_zero_changed(move |enabled| {
                let mut settings = settings.borrow_mut();
                settings.general.snap_markers_to_zero = enabled;
                let _ = settings.save();
            });
        }

        {
            // The explicit action, which works whether or not the toggle is
            // on. Markers resolve in region order so each one is bounded by
            // its neighbours' already-resolved positions.
            let tx = cmd_tx.clone();
            let st = state.clone();
            let window_weak = window.as_weak();
            window.on_snap_markers_clicked(move || {
                let Some(window) = window_weak.upgrade() else {
                    return;
                };
                let Some(snapped) = st.borrow_mut().session.snap_all_markers() else {
                    window.set_status_message("No sample to snap".into());
                    return;
                };
                let _ = tx.send(snapped.command);
                for (marker, value) in snapped.resolved {
                    set_marker_property(&window, marker, value);
                }
                window.set_status_message(
                    format!(
                        "Snapped {} of {} markers to zero crossings",
                        snapped.moved, snapped.searched
                    )
                    .into(),
                );
            });
        }
        wire_unit_param!(on_filter_cutoff_changed, filter_cutoff);
        wire_unit_param!(on_filter_resonance_changed, filter_resonance);
        wire_unit_param!(on_sampler_drive_changed, drive);
        wire_unit_param!(on_bit_reduction_changed, bit_reduction);
        wire_unit_param!(on_rate_reduction_changed, rate_reduction);
        // The face converts dB to linear before this runs, so the trim is an
        // ordinary linear parameter by the time it reaches the engine.
        wire_unit_param!(on_sampler_output_gain_changed, output_gain);
        // The filter envelope's stages live behind `filter_env_mut`, which
        // materializes the whole shape from wherever it was reading, so
        // editing one stage cannot silently move the other three.
        macro_rules! wire_filter_env_param {
            ($on:ident, $field:ident, $map:expr) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                window.$on(move |v: f32| {
                    let mut st = st.borrow_mut();
                    let ch = st.session.selected;
                    let Some(channel) = st.session.channels.get_mut(ch) else {
                        return;
                    };
                    #[allow(clippy::redundant_closure_call)]
                    let value = ($map)(v);
                    channel.params.filter_env_mut().$field = value;
                    let p = channel.params;
                    let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                        channel: ch as u8,
                        params: p,
                    });
                });
            }};
        }
        wire_filter_env_param!(on_sampler_filter_attack_changed, attack, norm_to_time);
        wire_filter_env_param!(on_sampler_filter_decay_changed, decay, norm_to_time);
        wire_filter_env_param!(on_sampler_filter_sustain_changed, sustain, |v: f32| v);
        wire_filter_env_param!(on_sampler_filter_release_changed, release, norm_to_time);

        {
            // Pure view state: re-bin the waveform for whatever range is
            // now visible so zooming in reveals real detail rather than
            // just stretching the full-sample overview's fixed bins.
            let st = state.clone();
            window.on_waveform_view_changed(move |offset: f32, visible_fraction: f32| {
                let st = st.borrow();
                let Some(channel) = st.session.channels.get(st.session.selected) else {
                    return;
                };
                let Some(sample) = channel.published_sample() else {
                    return;
                };
                let total = sample.frames.len();
                if total == 0 {
                    return;
                }
                let start = (offset.clamp(0.0, 1.0) * total as f32).round() as usize;
                let span = (visible_fraction.max(0.0) * total as f32).round().max(1.0) as usize;
                let end = (start + span).min(total);
                st.waveform_model.set_vec(waveform_peaks_windowed(
                    sample,
                    WAVEFORM_BINS,
                    start,
                    end,
                ));
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_reverse_playback_changed(move |reverse| {
                let mut st = st.borrow_mut();
                let ch = st.session.selected;
                let Some(channel) = st.session.channels.get_mut(ch) else {
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
            let weak = window.as_weak();
            window.on_root_note_changed(move |note| {
                let mut st = st.borrow_mut();
                let ch = st.session.selected;
                let Some(channel) = st.session.channels.get_mut(ch) else {
                    return;
                };
                channel.params.root_note = note.clamp(0, 127) as u8;
                if let Some(window) = weak.upgrade() {
                    window.set_tune_label(tune_label(channel.params).into());
                }
                let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                    channel: ch as u8,
                    params: channel.params,
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_tune_semitones_changed(move |v: f32| {
                let mut st = st.borrow_mut();
                let ch = st.session.selected;
                let Some(channel) = st.session.channels.get_mut(ch) else {
                    return;
                };
                channel.params.tune_semitones = v;
                if let Some(window) = weak.upgrade() {
                    window.set_tune_label(tune_label(channel.params).into());
                }
                let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                    channel: ch as u8,
                    params: channel.params,
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_tune_cents_changed(move |v: f32| {
                let mut st = st.borrow_mut();
                let ch = st.session.selected;
                let Some(channel) = st.session.channels.get_mut(ch) else {
                    return;
                };
                channel.params.tune_cents = v;
                if let Some(window) = weak.upgrade() {
                    window.set_tune_label(tune_label(channel.params).into());
                }
                let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                    channel: ch as u8,
                    params: channel.params,
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_retune_live_changed(move |on| {
                let mut st = st.borrow_mut();
                let ch = st.session.selected;
                let Some(channel) = st.session.channels.get_mut(ch) else {
                    return;
                };
                channel.params.retune_live = on;
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
                let ch = st.session.selected;
                let Some(channel) = st.session.channels.get_mut(ch) else {
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
                let ch = st.session.selected;
                let Some(channel) = st.session.channels.get_mut(ch) else {
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

        // --- Slice mode -------------------------------------------------
        //
        // Marker edits are the first undoable sampler edits -- there were
        // none before this. They follow the modulator-param precedent:
        // snapshot, mutate, publish, record. Drags collapse through the
        // gesture token the way the piano roll's already do.
        {
            let commands = command_state.clone();
            window.on_slice_drag_started(move || {
                let mut commands = commands.borrow_mut();
                commands.next_gesture = commands.next_gesture.wrapping_add(1);
                commands.gesture = Some(commands.next_gesture);
            });
        }
        {
            let commands = command_state.clone();
            window.on_slice_drag_finished(move || {
                commands.borrow_mut().gesture = None;
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_play_mode_changed(move |value| {
                let mut st = st.borrow_mut();
                let ch = st.session.selected;
                let Some(channel) = st.session.channels.get_mut(ch) else {
                    return;
                };
                channel.params.play_mode = PlayMode::from_index(value);
                let p = channel.params;
                let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                    channel: ch as u8,
                    params: p,
                });
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_slice_base_note_changed(move |note| {
                let mut st = st.borrow_mut();
                let ch = st.session.selected;
                let Some(channel) = st.session.channels.get_mut(ch) else {
                    return;
                };
                channel.params.slice_base_note = note.clamp(0, 127) as u8;
                let p = channel.params;
                let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                    channel: ch as u8,
                    params: p,
                });
            });
        }
        {
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            let audio_out = channel_audio_tx.clone();
            window.on_slice_added(move |position| {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                {
                    let mut st = st.borrow_mut();
                    match st.session.add_slice(position, window.get_snap_to_zero()) {
                        SliceEdit::Ignored => return,
                        SliceEdit::Refused => {
                            window.set_status_message(
                                format!("No slice added: {MAX_SLICES} is the limit, or one is already there")
                                    .into(),
                            );
                            return;
                        }
                        SliceEdit::Changed(markers) => {
                            st.slice_model.set_vec(markers);
                        }
                    }
                    st.publish_selected_audio(&audio_out);
                }
                record_project_history(&commands, before, &history_state, &window, "Slice added");
            });
        }
        {
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            let audio_out = channel_audio_tx.clone();
            window.on_slice_moved(move |index, position| {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                {
                    let mut st = st.borrow_mut();
                    let Some(markers) = st.session.move_slice(index, position) else {
                        return;
                    };
                    st.slice_model.set_vec(markers);
                    st.publish_selected_audio(&audio_out);
                }
                record_project_history(&commands, before, &history_state, &window, "Slice moved");
            });
        }
        {
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            let audio_out = channel_audio_tx.clone();
            window.on_slice_removed(move |index| {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                {
                    let mut st = st.borrow_mut();
                    let Some(markers) = st.session.remove_slice(index) else {
                        return;
                    };
                    st.slice_model.set_vec(markers);
                    st.publish_selected_audio(&audio_out);
                }
                record_project_history(&commands, before, &history_state, &window, "Slice removed");
            });
        }
        {
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            let audio_out = channel_audio_tx.clone();
            window.on_slices_divided(move |count| {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                {
                    let mut st = st.borrow_mut();
                    let Some(markers) = st.session.divide_slices(count, window.get_snap_to_zero()) else {
                        window.set_status_message("No sample to slice".into());
                        return;
                    };
                    st.slice_model.set_vec(markers);
                    st.publish_selected_audio(&audio_out);
                    window.set_status_message(format!("Divided into {} slices", count.max(1)).into());
                }
                record_project_history(&commands, before, &history_state, &window, "Slices divided");
            });
        }
        {
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            let audio_out = channel_audio_tx.clone();
            window.on_slices_cleared(move || {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                {
                    let mut st = st.borrow_mut();
                    let Some(markers) = st.session.clear_slices() else {
                        return;
                    };
                    st.slice_model.set_vec(markers);
                    st.publish_selected_audio(&audio_out);
                }
                record_project_history(&commands, before, &history_state, &window, "Slices cleared");
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_slice_auditioned(move |index| {
                let mut st = st.borrow_mut();
                let ch = st.session.selected;
                let Some(channel) = st.session.channels.get(ch) else {
                    return;
                };
                // Through the channel's own device, so what is heard is the
                // slice as it will actually play -- envelopes, filter, drive
                // and all. The browser's preview voice bypasses the strip
                // entirely and could not do this.
                let note = i32::from(channel.params.slice_base_note) + index.max(0);
                if note > 127 {
                    return;
                }
                st.session.slice_audition = Some((ch as u8, note as u8));
                let _ = tx.send(EngineCommand::TriggerChannelNote {
                    channel: ch as u8,
                    note: note as u8,
                    velocity: 100,
                });
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_slice_audition_released(move |_index| {
                // The note that was struck, not the note under the handle's
                // current index: a drag that crossed a neighbour has already
                // renumbered the handles by the time the button comes up.
                let Some((channel, note)) = st.borrow_mut().session.slice_audition.take() else {
                    return;
                };
                let _ = tx.send(EngineCommand::ReleaseChannelNote { channel, note });
            });
        }

        {
            let tx = cmd_tx.clone();
            let stx = structural_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            let audio_out = channel_audio_tx.clone();
            window.on_commit_clicked(move || {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                {
                    let mut st = st.borrow_mut();
                    let ch = st.session.selected;
                    let committed = match st.session.commit_stretch(window.get_bpm() as f64) {
                        Ok(committed) => committed,
                        Err(no_sample) => {
                            window.set_status_message(if no_sample {
                                "No sample to commit".into()
                            } else {
                                "Nothing to commit".into()
                            });
                            return;
                        }
                    };
                    st.publish_selected_audio(&audio_out);
                    let _ = tx.send(committed.command);
                    // The stretch is in the audio now and the patch no longer asks for
                    // it, so the pool goes back the way it came rather than holding
                    // ~1.6 MB for a stretcher that will not run. Same reconciliation
                    // the ON toggle does.
                    let _ = stx.send(StructuralCommand::SetSamplerStretch {
                        channel: ch as u8,
                        pool: None,
                    });
                    window.set_status_message(
                        format!("Committed the stretch at {:.2}x", committed.ratio).into(),
                    );
                }
                st.borrow().refresh_editor(&window);
                record_project_history(&commands, before, &history_state, &window, "Stretch committed");
            });
        }
        {
            let tx = cmd_tx.clone();
            let stx = structural_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            let audio_out = channel_audio_tx.clone();
            window.on_revert_clicked(move || {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                {
                    let mut st = st.borrow_mut();
                    let ch = st.session.selected;
                    let Some((params, command)) = st.session.revert_stretch() else {
                        return;
                    };
                    st.publish_selected_audio(&audio_out);
                    let _ = tx.send(command);
                    // The patch is stretching live again, and the state to do it cannot
                    // be assumed: a project saved committed and reloaded never
                    // provisioned a pool, because its patch did not ask for one.
                    // Without this, revert after a reload put the switch on and played
                    // unstretched.
                    let _ = stx.send(StructuralCommand::SetSamplerStretch {
                        channel: ch as u8,
                        pool: Some(Box::new(StretchPool::new(
                            params.stretch_mode,
                            sample_rate,
                            MAX_SAMPLER_VOICES as usize,
                        ))),
                    });
                    window.set_status_message("Reverted to the source sample".into());
                }
                st.borrow().refresh_editor(&window);
                record_project_history(&commands, before, &history_state, &window, "Stretch reverted");
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_voice_mode_changed(move |value| {
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                let channel = &mut st.session.channels[channel_index];
                channel.params.voice_mode = voice_mode_from_int(value);
                let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                    channel: channel_index as u8,
                    params: channel.params,
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_sampler_polyphony_changed(move |value| {
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                let channel = &mut st.session.channels[channel_index];
                channel.params.polyphony = value.clamp(1, 16) as u8;
                let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                    channel: channel_index as u8,
                    params: channel.params,
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_retrigger_mode_changed(move |value| {
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                let channel = &mut st.session.channels[channel_index];
                channel.params.retrigger_mode = retrigger_mode_from_int(value);
                let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                    channel: channel_index as u8,
                    params: channel.params,
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_choke_group_changed(move |value| {
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                let channel = &mut st.session.channels[channel_index];
                channel.params.choke_group = value.clamp(0, 16) as u8;
                let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                    channel: channel_index as u8,
                    params: channel.params,
                });
            });
        }

        // Mode, ratio and grain are ordinary parameters. The enable is not:
        // the pool it needs is ~1.6 MB and must be built here rather than on
        // the audio thread, so it rides a structural command alongside the
        // parameter write. The two can arrive in either order -- a sampler
        // whose intent is on but whose pool has not landed plays unstretched.
        {
            let tx = cmd_tx.clone();
            let stx = structural_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_stretch_enabled_changed(move |on| {
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                let channel = &mut st.session.channels[channel_index];
                channel.params.stretch_enabled = on;
                // Guess the loop length on the way in. A loop is nearly
                // always some power of two of bars and nearly always
                // recorded a little off it, so seeding this is the
                // difference between one click and a knob turn every time.
                if on {
                    // Measured in the sample's own frames against its own
                    // rate: the frame count is the file's, so a 44.1 kHz
                    // break measured at the engine's 48 kHz read 8% short
                    // and could snap a two-bar loop to one.
                    let (frames, rate) = channel
                        .published_sample()
                        .map_or((0, sample_rate), |sample| {
                            (sample.frames.len(), sample.sample_rate)
                        });
                    let bpm = weak.upgrade().map_or(120.0, |w| w.get_bpm() as f64);
                    let measured = measured_loop_bars(channel.params, frames, rate, bpm);
                    channel.params.stretch_bars = snap_bars_to_power_of_two(measured);
                    channel.params.stretch_sync = true;
                }
                let params = channel.params;
                let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                    channel: channel_index as u8,
                    params,
                });
                let _ = stx.send(StructuralCommand::SetSamplerStretch {
                    channel: channel_index as u8,
                    pool: on.then(|| {
                        Box::new(StretchPool::new(
                            params.stretch_mode,
                            sample_rate,
                            MAX_SAMPLER_VOICES as usize,
                        ))
                    }),
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_stretch_sync_changed(move |on| {
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                let channel = &mut st.session.channels[channel_index];
                channel.params.stretch_sync = on;
                let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                    channel: channel_index as u8,
                    params: channel.params,
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_stretch_bars_changed(move |norm| {
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                let channel = &mut st.session.channels[channel_index];
                let bars = stretch_bars_from_norm(norm);
                channel.params.stretch_bars = bars;
                let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                    channel: channel_index as u8,
                    params: channel.params,
                });
                if let Some(window) = weak.upgrade() {
                    window.set_stretch_bars_label(format_bars(bars).into());
                }
            });
        }

        // Typed entry. Parsing lives here rather than in Slint because the
        // formatting does too, and a unit-aware parser written on both sides
        // is one that will eventually disagree with itself. Anything
        // unparseable is dropped and the field re-reads the authoritative
        // value, so a half-typed string never reaches the engine.
        macro_rules! wire_typed_stretch_field {
            ($callback:ident, $apply:expr) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                let weak = window.as_weak();
                window.$callback(move |text| {
                    let Some(typed) = parse_typed_value(text.as_str()) else {
                        if let Some(window) = weak.upgrade() {
                            st.borrow().refresh_editor(&window);
                        }
                        return;
                    };
                    {
                        let mut st = st.borrow_mut();
                        let channel_index = st.session.selected;
                        let channel = &mut st.session.channels[channel_index];
                        #[allow(clippy::redundant_closure_call)]
                        ($apply)(&mut channel.params, typed);
                        let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                            channel: channel_index as u8,
                            params: channel.params,
                        });
                    }
                    if let Some(window) = weak.upgrade() {
                        st.borrow().refresh_editor(&window);
                    }
                });
            }};
        }

        wire_typed_stretch_field!(on_stretch_ratio_typed, |p: &mut SamplerParams, v: f32| {
            p.stretch_ratio = v.clamp(MIN_STRETCH_RATIO, MAX_STRETCH_RATIO);
        });
        wire_typed_stretch_field!(on_stretch_bars_typed, |p: &mut SamplerParams, v: f32| {
            p.stretch_bars = v.clamp(MIN_STRETCH_BARS, MAX_STRETCH_BARS);
        });
        wire_typed_stretch_field!(on_stretch_grain_typed, |p: &mut SamplerParams, v: f32| {
            p.stretch_grain = (v.round() as i32)
                .clamp(i32::from(MIN_STRETCH_GRAIN), i32::from(MAX_STRETCH_GRAIN))
                as u16;
        });
        wire_typed_stretch_field!(on_tune_typed, |p: &mut SamplerParams, v: f32| {
            p.tune_semitones = v.clamp(-48.0, 48.0);
        });

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_stretch_mode_changed(move |value| {
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                let channel = &mut st.session.channels[channel_index];
                channel.params.stretch_mode = match value {
                    1 => StretchMode::Drums,
                    2 => StretchMode::Grain,
                    _ => StretchMode::Music,
                };
                let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                    channel: channel_index as u8,
                    params: channel.params,
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_stretch_ratio_changed(move |norm| {
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                let channel = &mut st.session.channels[channel_index];
                let ratio = stretch_ratio_from_norm(norm);
                channel.params.stretch_ratio = ratio;
                let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                    channel: channel_index as u8,
                    params: channel.params,
                });
                if let Some(window) = weak.upgrade() {
                    window.set_stretch_ratio_label(format!("{ratio:.2}x").into());
                    window.set_stretch_ratio_clean((0.5..=1.5).contains(&ratio));
                }
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_stretch_grain_changed(move |norm| {
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                let channel = &mut st.session.channels[channel_index];
                let frames = stretch_grain_from_norm(norm);
                channel.params.stretch_grain = frames;
                let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                    channel: channel_index as u8,
                    params: channel.params,
                });
                if let Some(window) = weak.upgrade() {
                    window.set_stretch_grain_label(
                        format!(
                            "{frames} fr / {:.0} Hz",
                            sample_rate as f32 / (frames.max(2) as f32 / 2.0)
                        )
                        .into(),
                    );
                }
            });
        }

        macro_rules! wire_drum_param {
            ($callback:ident, $field:ident) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                let window_weak = window.as_weak();
                window.$callback(move |value: f32| {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.drum_params.$field = value;
                    let params = channel.drum_params;
                    let _ = tx.send(EngineCommand::SetChannelDrumSynthParams {
                        channel: channel_index as u8,
                        params,
                    });
                    drop(st);
                    if let Some(window) = window_weak.upgrade() {
                        sync_drum_preview(&window, params);
                    }
                });
            }};
        }

        wire_drum_param!(on_drum_decay_changed, decay);
        wire_drum_param!(on_drum_tune_semitones_changed, tune_semitones);
        wire_drum_param!(on_drum_drive_changed, drive);
        wire_drum_param!(on_drum_punch_changed, punch);
        wire_drum_param!(on_drum_kick_start_hz_changed, kick_start_hz);
        wire_drum_param!(on_drum_kick_end_hz_changed, kick_end_hz);
        wire_drum_param!(on_drum_kick_sweep_changed, kick_sweep);
        wire_drum_param!(on_drum_kick_click_changed, kick_click);
        wire_drum_param!(on_drum_snare_tone_hz_changed, snare_tone_hz);
        wire_drum_param!(on_drum_snare_tone2_hz_changed, snare_tone2_hz);
        wire_drum_param!(on_drum_snare_tone2_mix_changed, snare_tone2_mix);
        wire_drum_param!(on_drum_snare_noise_mix_changed, snare_noise_mix);
        wire_drum_param!(on_drum_snare_noise_decay_changed, snare_noise_decay);
        wire_drum_param!(on_drum_snare_noise_color_changed, snare_noise_color);
        wire_drum_param!(on_drum_hat_hp_hz_changed, hat_hp_hz);
        wire_drum_param!(on_drum_hat_metallic_changed, hat_metallic);

        macro_rules! wire_drum_int_param {
            ($callback:ident, $field:ident, $map:ident) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                let window_weak = window.as_weak();
                window.$callback(move |value| {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.drum_params.$field = $map(value);
                    let params = channel.drum_params;
                    let _ = tx.send(EngineCommand::SetChannelDrumSynthParams {
                        channel: channel_index as u8,
                        params,
                    });
                    drop(st);
                    if let Some(window) = window_weak.upgrade() {
                        sync_drum_preview(&window, params);
                    }
                });
            }};
        }

        wire_drum_int_param!(
            on_drum_kick_character_changed,
            kick_character,
            kick_character_from_int
        );
        wire_drum_int_param!(
            on_drum_snare_character_changed,
            snare_character,
            snare_character_from_int
        );
        wire_drum_int_param!(
            on_drum_hat_character_changed,
            hat_character,
            hat_character_from_int
        );

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let window_weak = window.as_weak();
            window.on_drum_mode_changed(move |value| {
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                let channel = &mut st.session.channels[channel_index];
                channel.drum_params.mode = drum_mode_from_int(value);
                let params = channel.drum_params;
                let _ = tx.send(EngineCommand::SetChannelDrumSynthParams {
                    channel: channel_index as u8,
                    params,
                });
                drop(st);
                if let Some(window) = window_weak.upgrade() {
                    sync_drum_preview(&window, params);
                }
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_drum_choke_group_changed(move |value| {
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                let channel = &mut st.session.channels[channel_index];
                channel.drum_params.choke_group = value.clamp(0, 16) as u8;
                let _ = tx.send(EngineCommand::SetChannelDrumSynthParams {
                    channel: channel_index as u8,
                    params: channel.drum_params,
                });
            });
        }

        macro_rules! wire_mono_param {
            ($callback:ident, $($field:ident).+) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                window.$callback(move |value: f32| {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.mono_params.$($field).+ = value;
                    let _ = tx.send(EngineCommand::SetChannelMonoSynthParams {
                        channel: channel_index as u8,
                        params: channel.mono_params,
                    });
                });
            }};
        }

        wire_mono_param!(on_mono_glide_changed, glide);
        wire_mono_param!(on_mono_attack_changed, attack);
        wire_mono_param!(on_mono_decay_changed, decay);
        wire_mono_param!(on_mono_sustain_changed, sustain);
        wire_mono_param!(on_mono_release_changed, release);
        wire_mono_param!(on_mono_filter_cutoff_changed, filter_cutoff);
        wire_mono_param!(on_mono_filter_resonance_changed, filter_resonance);
        wire_mono_param!(on_mono_filter_env_changed, filter_env_amount);
        wire_mono_param!(on_mono_drive_changed, drive);
        wire_mono_param!(on_mono_lfo_rate_changed, lfo.rate_hz);
        wire_mono_param!(on_mono_lfo_pitch_changed, lfo.to_pitch);
        wire_mono_param!(on_mono_lfo_filter_changed, lfo.to_filter);
        wire_mono_param!(on_mono_lfo_pulse_width_changed, lfo.to_pulse_width);
        wire_mono_param!(on_mono_lfo_amp_changed, lfo.to_amp);

        // The LFO's two non-float controls take the same shape by hand.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_mono_lfo_wave_changed(move |value| {
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                let channel = &mut st.session.channels[channel_index];
                channel.mono_params.lfo.wave = lfo_wave_from_int(value);
                let _ = tx.send(EngineCommand::SetChannelMonoSynthParams {
                    channel: channel_index as u8,
                    params: channel.mono_params,
                });
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_mono_lfo_retrigger_changed(move |value| {
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                let channel = &mut st.session.channels[channel_index];
                channel.mono_params.lfo.retrigger = value;
                let _ = tx.send(EngineCommand::SetChannelMonoSynthParams {
                    channel: channel_index as u8,
                    params: channel.mono_params,
                });
            });
        }

        macro_rules! wire_mlm1_param {
            ($callback:ident, $($field:ident).+) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                window.$callback(move |value: f32| {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.mlm1_params.$($field).+ = value;
                    let _ = tx.send(EngineCommand::SetChannelMlM1Params {
                        channel: channel_index as u8,
                        params: channel.mlm1_params,
                    });
                });
            }};
        }

        wire_mlm1_param!(on_mlm1_glide_changed, glide);
        wire_mlm1_param!(on_mlm1_attack_changed, attack);
        wire_mlm1_param!(on_mlm1_decay_changed, decay);
        wire_mlm1_param!(on_mlm1_sustain_changed, sustain);
        wire_mlm1_param!(on_mlm1_release_changed, release);
        wire_mlm1_param!(on_mlm1_filter_cutoff_changed, filter_cutoff);
        wire_mlm1_param!(on_mlm1_filter_resonance_changed, filter_resonance);
        wire_mlm1_param!(on_mlm1_filter_env_changed, filter_env_amount);
        wire_mlm1_param!(on_mlm1_drive_changed, drive);
        wire_mlm1_param!(on_mlm1_filter_attack_changed, filter_attack);
        wire_mlm1_param!(on_mlm1_filter_decay_changed, filter_decay);
        wire_mlm1_param!(on_mlm1_filter_sustain_changed, filter_sustain);
        wire_mlm1_param!(on_mlm1_filter_release_changed, filter_release);
        wire_mlm1_param!(on_mlm1_filter_keytrack_changed, filter_keytrack);
        wire_mlm1_param!(on_mlm1_accent_changed, accent);

        /// The three performance switches arrive as selector indices rather
        /// than floats, so they take the same shape with a conversion.
        macro_rules! wire_mlm1_enum {
            ($callback:ident, $field:ident, $from_index:path) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                window.$callback(move |value: i32| {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.mlm1_params.$field = $from_index(value);
                    let _ = tx.send(EngineCommand::SetChannelMlM1Params {
                        channel: channel_index as u8,
                        params: channel.mlm1_params,
                    });
                });
            }};
        }

        wire_mlm1_enum!(on_mlm1_glide_mode_changed, glide_mode, GlideMode::from_index);
        wire_mlm1_enum!(
            on_mlm1_env_trigger_changed,
            env_trigger,
            EnvTrigger::from_index
        );
        wire_mlm1_enum!(
            on_mlm1_priority_changed,
            priority,
            NotePriority::from_index
        );
        wire_mlm1_enum!(
            on_mlm1_filter_model_changed,
            filter_model,
            FilterModel::from_index
        );

        macro_rules! wire_mlm1_osc_float {
            ($callback:ident, $index:expr, $field:ident) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                window.$callback(move |value: f32| {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.mlm1_params.osc[$index].$field = value;
                    let _ = tx.send(EngineCommand::SetChannelMlM1Params {
                        channel: channel_index as u8,
                        params: channel.mlm1_params,
                    });
                });
            }};
        }
        macro_rules! wire_mlm1_osc_wave {
            ($callback:ident, $index:expr) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                window.$callback(move |value| {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.mlm1_params.osc[$index].wave = osc_wave_from_int(value);
                    let _ = tx.send(EngineCommand::SetChannelMlM1Params {
                        channel: channel_index as u8,
                        params: channel.mlm1_params,
                    });
                });
            }};
        }

        wire_mlm1_osc_wave!(on_mlm1_osc1_wave_changed, 0);
        wire_mlm1_osc_float!(on_mlm1_osc1_semitones_changed, 0, semitones);
        wire_mlm1_osc_float!(on_mlm1_osc1_cents_changed, 0, cents);
        wire_mlm1_osc_float!(on_mlm1_osc1_level_changed, 0, level);
        wire_mlm1_osc_float!(on_mlm1_osc1_pulse_width_changed, 0, pulse_width);
        wire_mlm1_osc_wave!(on_mlm1_osc2_wave_changed, 1);
        wire_mlm1_osc_float!(on_mlm1_osc2_semitones_changed, 1, semitones);
        wire_mlm1_osc_float!(on_mlm1_osc2_cents_changed, 1, cents);
        wire_mlm1_osc_float!(on_mlm1_osc2_level_changed, 1, level);
        wire_mlm1_osc_float!(on_mlm1_osc2_pulse_width_changed, 1, pulse_width);
        wire_mlm1_osc_wave!(on_mlm1_osc3_wave_changed, 2);
        wire_mlm1_osc_float!(on_mlm1_osc3_semitones_changed, 2, semitones);
        wire_mlm1_osc_float!(on_mlm1_osc3_cents_changed, 2, cents);
        wire_mlm1_osc_float!(on_mlm1_osc3_level_changed, 2, level);
        wire_mlm1_osc_float!(on_mlm1_osc3_pulse_width_changed, 2, pulse_width);


        // ML-P8. Every control is addressed by its descriptor id rather than
        // by a field path, so the knob, the typed value and an automation
        // lane all reach the parameter the same way and land under the same
        // clamp. The device has sixty-two parameters; a closure a control
        // writing its own field is where a wrong field goes unnoticed.
        macro_rules! wire_mlp8 {
            ($callback:ident, $id:expr, $ty:ty) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                window.$callback(move |value: $ty| {
                    let id: u32 = $id;
                    let value = value as f32;
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    let mut params = GeneratorParams::MlP8(channel.mlp8_params);
                    let Some(value) = params.set(id, value) else {
                        return;
                    };
                    if let GeneratorParams::MlP8(updated) = params {
                        channel.mlp8_params = updated;
                    }
                    let _ = tx.send(EngineCommand::SetChannelGeneratorParam {
                        channel: channel_index as u8,
                        id,
                        value,
                    });
                });
            }};
        }

        // The hit is re-rendered once an edit stops, not once per frame of a
        // drag. One timer for the whole device: a second edit restarts it,
        // which is the debounce.
        let ds01_preview = Rc::new(Timer::default());
        let schedule_ds01_preview = {
            let st = state.clone();
            let weak = window.as_weak();
            let timer = ds01_preview.clone();
            move || {
                let st = st.clone();
                let weak = weak.clone();
                timer.start(
                    TimerMode::SingleShot,
                    std::time::Duration::from_millis(DS01_PREVIEW_DEBOUNCE_MS),
                    move || {
                        let Some(window) = weak.upgrade() else {
                            return;
                        };
                        let params = {
                            let st = st.borrow();
                            st.session.channels[st.session.selected].ds01_params
                        };
                        sync_ds01_preview(&window, &params);
                        sync_ds01_burst_ticks(&window, &params);
                    },
                );
            }
        };

        // DS-01's face reports `(id, normalized)` for everything, so one
        // handler covers ninety-two controls rather than ninety-two closures
        // covering one each. That is the same reason the face takes arrays: a
        // device whose whole premise is that every parameter is addressable by
        // id should not need its parameter table written out a second time to
        // be edited.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            let redraw = schedule_ds01_preview.clone();
            window.on_ds01_value_changed(move |id, normalized| {
                let id = id as u32;
                let Some(descriptor) = ds01::descriptor(id) else {
                    return;
                };
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                let channel = &mut st.session.channels[channel_index];
                let mut params = GeneratorParams::Ds01(channel.ds01_params);
                let Some(value) = params.set(id, descriptor.from_normalized(normalized)) else {
                    return;
                };
                if let GeneratorParams::Ds01(updated) = params {
                    channel.ds01_params = updated;
                }
                let _ = tx.send(EngineCommand::SetChannelGeneratorParam {
                    channel: channel_index as u8,
                    id,
                    value,
                });
                if let Some(window) = weak.upgrade() {
                    touch_ds01_param(&window, &channel.ds01_params, id);
                }
                redraw();
            });
        }

        // A handle drop, as a fraction of the scope. The face cannot turn that
        // back into a parameter value because the conversion needs the span
        // and the descriptor, and the face has neither.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            let redraw = schedule_ds01_preview.clone();
            window.on_ds01_handle_dragged(move |id, fraction| {
                let id = id as u32;
                let Some(descriptor) = ds01::descriptor(id) else {
                    return;
                };
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                let channel = &mut st.session.channels[channel_index];
                let base = channel.ds01_params;
                let natural = if id == ds01::PARAM_PITCH_DEPTH {
                    // The only handle that drags vertically: the pitch
                    // envelope's depth is drawn as the contour's height, so
                    // its height is what the drag reports. The sign stays
                    // where the patch had it — a drag on a curve cannot say
                    // "the other way", and flipping it silently would turn a
                    // downward sweep into an upward one mid-gesture.
                    let magnitude = fraction * descriptor.max;
                    if base.pitch.depth < 0.0 {
                        -magnitude
                    } else {
                        magnitude
                    }
                } else {
                    // Every other handle is a time, and the scope is drawn
                    // over the span, so the fraction is seconds directly.
                    // Handles past the first in a chain report where they
                    // were dropped, so the segment is the gap to the one
                    // before it rather than the drop position.
                    let seconds = fraction * ds01_span_seconds(&base);
                    let before = ds01_segment_start(&base, id);
                    (seconds - before).max(0.0)
                };
                let mut params = GeneratorParams::Ds01(base);
                let Some(value) = params.set(id, natural) else {
                    return;
                };
                if let GeneratorParams::Ds01(updated) = params {
                    channel.ds01_params = updated;
                }
                let _ = tx.send(EngineCommand::SetChannelGeneratorParam {
                    channel: channel_index as u8,
                    id,
                    value,
                });
                if let Some(window) = weak.upgrade() {
                    touch_ds01_param(&window, &channel.ds01_params, id);
                }
                redraw();
            });
        }

        // The face also reports base-value gesture boundaries, so a drag can
        // one day coalesce into one undo step. Nothing subscribes yet, here or
        // on any other device face; the callbacks exist because the widget
        // emits them and the face should not swallow them.

        use mooloop_core::mlp8 as p8;
        macro_rules! wire_mlp8_osc {
            ($callback:ident, $osc:expr, $offset:expr, $ty:ty) => {
                wire_mlp8!($callback, p8::osc_param($osc, $offset), $ty)
            };
        }

        wire_mlp8_osc!(on_mlp8_osc1_wave_changed, 0, p8::OSC_OFFSET_WAVE, i32);
        wire_mlp8_osc!(on_mlp8_osc1_semitones_changed, 0, p8::OSC_OFFSET_SEMITONES, f32);
        wire_mlp8_osc!(on_mlp8_osc1_cents_changed, 0, p8::OSC_OFFSET_CENTS, f32);
        wire_mlp8_osc!(on_mlp8_osc1_level_changed, 0, p8::OSC_OFFSET_LEVEL, f32);
        wire_mlp8_osc!(on_mlp8_osc1_pulse_width_changed, 0, p8::OSC_OFFSET_PULSE_WIDTH, f32);
        wire_mlp8_osc!(on_mlp8_osc2_wave_changed, 1, p8::OSC_OFFSET_WAVE, i32);
        wire_mlp8_osc!(on_mlp8_osc2_semitones_changed, 1, p8::OSC_OFFSET_SEMITONES, f32);
        wire_mlp8_osc!(on_mlp8_osc2_cents_changed, 1, p8::OSC_OFFSET_CENTS, f32);
        wire_mlp8_osc!(on_mlp8_osc2_level_changed, 1, p8::OSC_OFFSET_LEVEL, f32);
        wire_mlp8_osc!(on_mlp8_osc2_pulse_width_changed, 1, p8::OSC_OFFSET_PULSE_WIDTH, f32);
        wire_mlp8_osc!(on_mlp8_osc3_wave_changed, 2, p8::OSC_OFFSET_WAVE, i32);
        wire_mlp8_osc!(on_mlp8_osc3_semitones_changed, 2, p8::OSC_OFFSET_SEMITONES, f32);
        wire_mlp8_osc!(on_mlp8_osc3_cents_changed, 2, p8::OSC_OFFSET_CENTS, f32);
        wire_mlp8_osc!(on_mlp8_osc3_level_changed, 2, p8::OSC_OFFSET_LEVEL, f32);
        wire_mlp8_osc!(on_mlp8_osc3_pulse_width_changed, 2, p8::OSC_OFFSET_PULSE_WIDTH, f32);
        wire_mlp8!(on_mlp8_attack_changed, p8::PARAM_ATTACK, f32);
        wire_mlp8!(on_mlp8_decay_changed, p8::PARAM_DECAY, f32);
        wire_mlp8!(on_mlp8_sustain_changed, p8::PARAM_SUSTAIN, f32);
        wire_mlp8!(on_mlp8_release_changed, p8::PARAM_RELEASE, f32);
        wire_mlp8!(on_mlp8_glide_changed, p8::PARAM_GLIDE, f32);
        wire_mlp8!(on_mlp8_sub_level_changed, p8::PARAM_SUB_LEVEL, f32);
        wire_mlp8!(on_mlp8_noise_level_changed, p8::PARAM_NOISE_LEVEL, f32);
        wire_mlp8!(on_mlp8_noise_color_changed, p8::PARAM_NOISE_COLOR, f32);
        wire_mlp8!(on_mlp8_sub_octave_changed, p8::PARAM_SUB_OCTAVE, i32);
        wire_mlp8!(on_mlp8_sub_wave_changed, p8::PARAM_SUB_WAVE, i32);
        wire_mlp8!(on_mlp8_sub_source_changed, p8::PARAM_SUB_SOURCE, i32);
        wire_mlp8!(on_mlp8_xmod12_changed, p8::PARAM_XMOD_BASE + p8::xmod_index(0, 1) as u32, f32);
        wire_mlp8!(on_mlp8_xmod13_changed, p8::PARAM_XMOD_BASE + p8::xmod_index(0, 2) as u32, f32);
        wire_mlp8!(on_mlp8_xmod21_changed, p8::PARAM_XMOD_BASE + p8::xmod_index(1, 0) as u32, f32);
        wire_mlp8!(on_mlp8_xmod23_changed, p8::PARAM_XMOD_BASE + p8::xmod_index(1, 2) as u32, f32);
        wire_mlp8!(on_mlp8_xmod31_changed, p8::PARAM_XMOD_BASE + p8::xmod_index(2, 0) as u32, f32);
        wire_mlp8!(on_mlp8_xmod32_changed, p8::PARAM_XMOD_BASE + p8::xmod_index(2, 1) as u32, f32);
        wire_mlp8!(on_mlp8_noise_osc1_changed, p8::PARAM_NOISE_TO_OSC_BASE, f32);
        wire_mlp8!(on_mlp8_noise_osc2_changed, p8::PARAM_NOISE_TO_OSC_BASE + 1, f32);
        wire_mlp8!(on_mlp8_noise_osc3_changed, p8::PARAM_NOISE_TO_OSC_BASE + 2, f32);
        wire_mlp8!(on_mlp8_feedback1_changed, p8::PARAM_OSC_FEEDBACK_BASE, f32);
        wire_mlp8!(on_mlp8_feedback2_changed, p8::PARAM_OSC_FEEDBACK_BASE + 1, f32);
        wire_mlp8!(on_mlp8_feedback3_changed, p8::PARAM_OSC_FEEDBACK_BASE + 2, f32);
        wire_mlp8!(on_mlp8_sync1_changed, p8::PARAM_SYNC_SOURCE_BASE, i32);
        wire_mlp8!(on_mlp8_sync2_changed, p8::PARAM_SYNC_SOURCE_BASE + 1, i32);
        wire_mlp8!(on_mlp8_sync3_changed, p8::PARAM_SYNC_SOURCE_BASE + 2, i32);
        wire_mlp8!(on_mlp8_filter_mode_changed, p8::PARAM_FILTER_MODE, i32);
        wire_mlp8!(on_mlp8_filter_cutoff_changed, p8::PARAM_FILTER_CUTOFF, f32);
        wire_mlp8!(on_mlp8_filter_resonance_changed, p8::PARAM_FILTER_RESONANCE, f32);
        wire_mlp8!(on_mlp8_filter_env_changed, p8::PARAM_FILTER_ENV_AMOUNT, f32);
        wire_mlp8!(on_mlp8_drive_changed, p8::PARAM_DRIVE, f32);
        wire_mlp8!(on_mlp8_keytrack_changed, p8::PARAM_KEYTRACK, f32);
        wire_mlp8!(on_mlp8_amp_velocity_changed, p8::PARAM_AMP_VELOCITY, f32);
        wire_mlp8!(on_mlp8_filter_velocity_changed, p8::PARAM_FILTER_VELOCITY, f32);
        wire_mlp8!(on_mlp8_voice_feedback_changed, p8::PARAM_VOICE_FEEDBACK, f32);
        wire_mlp8!(on_mlp8_filter_attack_changed, p8::PARAM_FILTER_ATTACK, f32);
        wire_mlp8!(on_mlp8_filter_decay_changed, p8::PARAM_FILTER_DECAY, f32);
        wire_mlp8!(on_mlp8_filter_sustain_changed, p8::PARAM_FILTER_SUSTAIN, f32);
        wire_mlp8!(on_mlp8_filter_release_changed, p8::PARAM_FILTER_RELEASE, f32);
        // The device's own LFO is eight more descriptor ids, not a second
        // kind of control, so it takes the same path everything else does.
        wire_mlp8!(on_mlp8_lfo_wave_changed, p8::PARAM_LFO_WAVE, i32);
        wire_mlp8!(on_mlp8_lfo_rate_changed, p8::PARAM_LFO_RATE_HZ, f32);
        wire_mlp8!(on_mlp8_lfo_division_changed, p8::PARAM_LFO_RATE_DIVISION, i32);
        wire_mlp8!(on_mlp8_lfo_phase_changed, p8::PARAM_LFO_PHASE, f32);
        wire_mlp8!(on_mlp8_lfo_warp_changed, p8::PARAM_LFO_WARP, f32);
        wire_mlp8!(on_mlp8_lfo_slew_changed, p8::PARAM_LFO_SLEW, f32);
        wire_mlp8!(on_mlp8_lfo_retrigger_changed, p8::PARAM_LFO_RETRIGGER, i32);
        {
            // Sync is a lamp rather than a selector, so it arrives as a bool
            // and reaches the same stepped descriptor as everything else.
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_mlp8_lfo_sync_changed(move |on| {
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                let channel = &mut st.session.channels[channel_index];
                let mut params = GeneratorParams::MlP8(channel.mlp8_params);
                let Some(value) = params.set(p8::PARAM_LFO_SYNC, f32::from(u8::from(on))) else {
                    return;
                };
                if let GeneratorParams::MlP8(updated) = params {
                    channel.mlp8_params = updated;
                }
                let _ = tx.send(EngineCommand::SetChannelGeneratorParam {
                    channel: channel_index as u8,
                    id: p8::PARAM_LFO_SYNC,
                    value,
                });
            });
        }

        // --- The ML-P8's internal routes ------------------------------------
        //
        // Three of these are structural -- add, remove, repoint -- and go to
        // the engine as whole routes so the audio thread recompiles its flat
        // table from what a save would write. The fourth, the depth, is an
        // ordinary continuous value and deliberately takes a narrower command
        // that does not rebuild anything.
        //
        // Every one of them names the route's durable id, never its row: the
        // face redraws its list from this state, and a row that moves because
        // a neighbour was removed must still edit the route it was drawn for.
        macro_rules! mlp8_route_edit {
            ($callback:ident, |$routes:ident, $channel:ident, $($arg:ident),*| $body:block) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                let weak = window.as_weak();
                window.$callback(move |$($arg),*| {
                    let Some(window) = weak.upgrade() else {
                        return;
                    };
                    let mut st = st.borrow_mut();
                    let index = st.session.selected;
                    if st.session.channels[index].kind != DeviceKind::MlP8 {
                        return;
                    }
                    // A closure so an edit can bail with `?` on an id that
                    // names no route -- which is what a stale click during a
                    // list rebuild looks like.
                    let edit = |$routes: &mut mooloop_core::MlP8Routes,
                                $channel: u8|
                     -> Option<EngineCommand> { $body };
                    let Some(command) = edit(
                        &mut st.session.channels[index].mlp8_params.routes,
                        index as u8,
                    ) else {
                        return;
                    };
                    let _ = tx.send(command);
                    let routes = st.session.channels[index].mlp8_params.routes;
                    refresh_mlp8_routes(&window, &routes);
                    st.session.dirty = true;
                    st.session.revision = st.session.revision.wrapping_add(1);
                    st.update_document_title(&window);
                });
            }};
        }

        mlp8_route_edit!(on_mlp8_route_added, |routes, channel,| {
            // Something audible by default would be a surprise; something
            // that reads nothing would be a dead row. A new route reads the
            // LFO into the filter, at zero depth.
            let dest = mooloop_core::MlP8ModDest::Param {
                id: p8::PARAM_FILTER_CUTOFF,
            };
            routes
                .add(mooloop_core::MlP8ModSource::Lfo, dest)
                .and_then(|id| routes.get(id).copied())
                .map(|route| EngineCommand::SetSourceRoute {
                    channel,
                    route,
                })
        });

        mlp8_route_edit!(on_mlp8_route_removed, |routes, channel, id| {
            let id = u16::try_from(id).ok()?;
            routes
                .remove(id)
                .then_some(EngineCommand::RemoveSourceRoute {
                    channel,
                    route: id,
                })
        });

        mlp8_route_edit!(on_mlp8_route_source_changed, |routes, channel, id, index| {
            let id = u16::try_from(id).ok()?;
            let existing = *routes.get(id)?;
            let source = mooloop_core::MlP8ModSource::from_index(index);
            routes
                .set_endpoints(id, source, existing.dest)
                .then_some(EngineCommand::SetSourceRoute {
                    channel,
                    route: mooloop_core::MlP8Route { source, ..existing },
                })
        });

        mlp8_route_edit!(on_mlp8_route_dest_changed, |routes, channel, id, index| {
            let id = u16::try_from(id).ok()?;
            let existing = *routes.get(id)?;
            let dest = *mooloop_core::MlP8ModDest::ALL.get(index.max(0) as usize)?;
            routes
                .set_endpoints(id, existing.source, dest)
                .then_some(EngineCommand::SetSourceRoute {
                    channel,
                    route: mooloop_core::MlP8Route { dest, ..existing },
                })
        });

        {
            // The depth is a drag, so it neither redraws the list nor takes
            // the structural path: it is the one part of a route that is an
            // ordinary automatable value.
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_mlp8_route_amount_changed(move |id, amount| {
                let Ok(id) = u16::try_from(id) else {
                    return;
                };
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                if st.session.channels[channel_index].kind != DeviceKind::MlP8 {
                    return;
                }
                if !st.session.channels[channel_index]
                    .mlp8_params
                    .routes
                    .set_amount(id, amount)
                {
                    return;
                }
                let _ = tx.send(EngineCommand::SetSourceRouteAmount {
                    channel: channel_index as u8,
                    route: id,
                    amount,
                });
                st.session.dirty = true;
            });
        }

        // Every ML-P8 value field commits through one handler, because the
        // descriptor id travels with the text. `GeneratorParams::set` does
        // the clamping, so a typed number lands under exactly the same rules
        // as a dragged one -- which is the thing forty-one hand-written
        // handlers would eventually stop doing.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_mlp8_text_committed(move |id, text| {
                let Some(typed) = parse_typed_value(text.as_str()) else {
                    if let Some(window) = weak.upgrade() {
                        st.borrow().refresh_editor(&window);
                    }
                    return;
                };
                let id = id.max(0) as u32;
                // The five mix levels read in dB and store linear; core owns
                // which those are so the face and this handler cannot drift.
                let value = if mooloop_core::mlp8::is_gain_param(id) {
                    mooloop_core::gain::db_to_linear(typed)
                } else {
                    typed
                };
                {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    let mut params = GeneratorParams::MlP8(channel.mlp8_params);
                    if let Some(clamped) = params.set(id, value) {
                        if let GeneratorParams::MlP8(updated) = params {
                            channel.mlp8_params = updated;
                            let _ = tx.send(EngineCommand::SetChannelGeneratorParam {
                                channel: channel_index as u8,
                                id,
                                value: clamped,
                            });
                        }
                    }
                }
                if let Some(window) = weak.upgrade() {
                    st.borrow().refresh_editor(&window);
                }
            });
        }

        macro_rules! wire_mono_osc_float {
            ($callback:ident, $index:expr, $field:ident) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                window.$callback(move |value: f32| {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.mono_params.osc[$index].$field = value;
                    let _ = tx.send(EngineCommand::SetChannelMonoSynthParams {
                        channel: channel_index as u8,
                        params: channel.mono_params,
                    });
                });
            }};
        }
        macro_rules! wire_mono_osc_wave {
            ($callback:ident, $index:expr) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                window.$callback(move |value| {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.mono_params.osc[$index].wave = osc_wave_from_int(value);
                    let _ = tx.send(EngineCommand::SetChannelMonoSynthParams {
                        channel: channel_index as u8,
                        params: channel.mono_params,
                    });
                });
            }};
        }

        wire_mono_osc_wave!(on_mono_osc1_wave_changed, 0);
        wire_mono_osc_float!(on_mono_osc1_semitones_changed, 0, semitones);
        wire_mono_osc_float!(on_mono_osc1_cents_changed, 0, cents);
        wire_mono_osc_float!(on_mono_osc1_level_changed, 0, level);
        wire_mono_osc_float!(on_mono_osc1_pulse_width_changed, 0, pulse_width);
        wire_mono_osc_wave!(on_mono_osc2_wave_changed, 1);
        wire_mono_osc_float!(on_mono_osc2_semitones_changed, 1, semitones);
        wire_mono_osc_float!(on_mono_osc2_cents_changed, 1, cents);
        wire_mono_osc_float!(on_mono_osc2_level_changed, 1, level);
        wire_mono_osc_float!(on_mono_osc2_pulse_width_changed, 1, pulse_width);
        wire_mono_osc_wave!(on_mono_osc3_wave_changed, 2);
        wire_mono_osc_float!(on_mono_osc3_semitones_changed, 2, semitones);
        wire_mono_osc_float!(on_mono_osc3_cents_changed, 2, cents);
        wire_mono_osc_float!(on_mono_osc3_level_changed, 2, level);
        wire_mono_osc_float!(on_mono_osc3_pulse_width_changed, 2, pulse_width);

        macro_rules! wire_poly_param {
            ($callback:ident, $($field:ident).+) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                window.$callback(move |value: f32| {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.poly_params.$($field).+ = value;
                    let _ = tx.send(EngineCommand::SetChannelPolySynthParams {
                        channel: channel_index as u8,
                        params: channel.poly_params,
                    });
                });
            }};
        }

        wire_poly_param!(on_poly_glide_changed, glide);
        wire_poly_param!(on_poly_attack_changed, attack);
        wire_poly_param!(on_poly_decay_changed, decay);
        wire_poly_param!(on_poly_sustain_changed, sustain);
        wire_poly_param!(on_poly_release_changed, release);
        wire_poly_param!(on_poly_filter_cutoff_changed, filter_cutoff);
        wire_poly_param!(on_poly_filter_resonance_changed, filter_resonance);
        wire_poly_param!(on_poly_filter_env_changed, filter_env_amount);
        wire_poly_param!(on_poly_drive_changed, drive);
        wire_poly_param!(on_poly_lfo_rate_changed, lfo.rate_hz);
        wire_poly_param!(on_poly_lfo_pitch_changed, lfo.to_pitch);
        wire_poly_param!(on_poly_lfo_filter_changed, lfo.to_filter);
        wire_poly_param!(on_poly_lfo_pulse_width_changed, lfo.to_pulse_width);
        wire_poly_param!(on_poly_lfo_amp_changed, lfo.to_amp);
        wire_poly_param!(on_poly_spread_changed, spread);

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_poly_lfo_wave_changed(move |value| {
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                let channel = &mut st.session.channels[channel_index];
                channel.poly_params.lfo.wave = lfo_wave_from_int(value);
                let _ = tx.send(EngineCommand::SetChannelPolySynthParams {
                    channel: channel_index as u8,
                    params: channel.poly_params,
                });
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_poly_lfo_retrigger_changed(move |value| {
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                let channel = &mut st.session.channels[channel_index];
                channel.poly_params.lfo.retrigger = value;
                let _ = tx.send(EngineCommand::SetChannelPolySynthParams {
                    channel: channel_index as u8,
                    params: channel.poly_params,
                });
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_poly_polyphony_changed(move |value| {
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                let channel = &mut st.session.channels[channel_index];
                channel.poly_params.polyphony = value.clamp(1, MAX_POLY_VOICES as i32) as u8;
                let _ = tx.send(EngineCommand::SetChannelPolySynthParams {
                    channel: channel_index as u8,
                    params: channel.poly_params,
                });
            });
        }

        macro_rules! wire_poly_osc_float {
            ($callback:ident, $index:expr, $field:ident) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                window.$callback(move |value: f32| {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.poly_params.osc[$index].$field = value;
                    let _ = tx.send(EngineCommand::SetChannelPolySynthParams {
                        channel: channel_index as u8,
                        params: channel.poly_params,
                    });
                });
            }};
        }
        macro_rules! wire_poly_osc_wave {
            ($callback:ident, $index:expr) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                window.$callback(move |value| {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.poly_params.osc[$index].wave = osc_wave_from_int(value);
                    let _ = tx.send(EngineCommand::SetChannelPolySynthParams {
                        channel: channel_index as u8,
                        params: channel.poly_params,
                    });
                });
            }};
        }

        wire_poly_osc_wave!(on_poly_osc1_wave_changed, 0);
        wire_poly_osc_float!(on_poly_osc1_semitones_changed, 0, semitones);
        wire_poly_osc_float!(on_poly_osc1_cents_changed, 0, cents);
        wire_poly_osc_float!(on_poly_osc1_level_changed, 0, level);
        wire_poly_osc_float!(on_poly_osc1_pulse_width_changed, 0, pulse_width);
        wire_poly_osc_wave!(on_poly_osc2_wave_changed, 1);
        wire_poly_osc_float!(on_poly_osc2_semitones_changed, 1, semitones);
        wire_poly_osc_float!(on_poly_osc2_cents_changed, 1, cents);
        wire_poly_osc_float!(on_poly_osc2_level_changed, 1, level);
        wire_poly_osc_float!(on_poly_osc2_pulse_width_changed, 1, pulse_width);
        wire_poly_osc_wave!(on_poly_osc3_wave_changed, 2);
        wire_poly_osc_float!(on_poly_osc3_semitones_changed, 2, semitones);
        wire_poly_osc_float!(on_poly_osc3_cents_changed, 2, cents);
        wire_poly_osc_float!(on_poly_osc3_level_changed, 2, level);
        wire_poly_osc_float!(on_poly_osc3_pulse_width_changed, 2, pulse_width);

        // --- Sample browser: locations persist in settings.toml and the
        //     tree re-flattens on every change. The folder picker runs on a
        //     worker thread like every other zenity call, handing the picked
        //     path to the pump, which applies it on the UI thread. ---
        let (browser_pick_tx, browser_pick_rx) = std::sync::mpsc::channel::<PathBuf>();
        let (browser_info_tx, browser_info_rx) =
            std::sync::mpsc::channel::<Result<SampleInspection, (String, String)>>();
        {
            let browser_info_tx = browser_info_tx.clone();
            window.on_browser_row_previewed(move |path| {
                let path = PathBuf::from(path.to_string());
                let tx = browser_info_tx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(
                        inspect_sample(&path).map_err(|error| (path.display().to_string(), error)),
                    );
                });
            });
        }
        {
            let preview_tx = preview_tx.clone();
            window.on_browser_preview_gain_changed(move |gain| {
                preview_tx.send_gain(gain);
            });
        }
        {
            let browser_pick_tx = browser_pick_tx.clone();
            window.on_browser_add_location(move || {
                let tx = browser_pick_tx.clone();
                std::thread::spawn(move || {
                    if let Some(path) = pick_bundle_via_zenity("Add sample folder") {
                        let _ = tx.send(path);
                    }
                });
            });
        }
        {
            let st = state.clone();
            window.on_browser_row_toggled(move |path| {
                let mut st = st.borrow_mut();
                st.session
                    .toggle_browser_folder(PathBuf::from(path.to_string()));
                refresh_browser(&st);
            });
        }
        {
            let st = state.clone();
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_browser_location_removed(move |path| {
                let path = PathBuf::from(path.to_string());
                let Some(window) = weak.upgrade() else { return };
                settings
                    .borrow_mut()
                    .browser
                    .locations
                    .retain(|p| p != &path);
                let saved = settings.borrow().save();
                {
                    let mut st = st.borrow_mut();
                    st.session.remove_browser_location(&path);
                    refresh_browser(&st);
                }
                window.set_status_message(match saved {
                    Ok(()) => format!("Removed sample folder {}", path.display()).into(),
                    Err(error) => format!("Could not save settings: {error}").into(),
                });
            });
        }

        // --- Sample loading via zenity + Symphonia (selected channel) ---
        // The dialog + decode run on a worker thread so the UI stays
        // responsive (a blocking dialog makes the OS mark the app frozen and
        // offer to kill it). Results come back through `load_rx` and are
        // applied by the pump on the UI thread.
        let (load_tx, load_rx) = std::sync::mpsc::channel::<LoadResult>();
        {
            let st = state.clone();
            let load_tx = load_tx.clone();
            window.on_browser_sample_loaded(move |path| {
                let (channel, source_revision) = {
                    let st = st.borrow();
                    (st.session.selected, st.session.source_revision)
                };
                spawn_browser_sample_load(&path, channel, source_revision, false, &load_tx);
            });
        }
        {
            let st = state.clone();
            let load_tx = load_tx.clone();
            window.on_browser_sample_loaded_new_channel(move |path| {
                let (channel, source_revision) = {
                    let st = st.borrow();
                    (st.session.channels.len(), st.session.source_revision)
                };
                spawn_browser_sample_load(&path, channel, source_revision, true, &load_tx);
            });
        }
        {
            let st = state.clone();
            let load_tx = load_tx.clone();
            window.on_load_sample_clicked(move || {
                let (channel, source_revision) = {
                    let st = st.borrow();
                    (st.session.selected, st.session.source_revision)
                };
                let tx = load_tx.clone();
                log_debug!("ui", "loading sample for channel {channel}");
                std::thread::spawn(move || {
                    let result = pick_sample_via_zenity().map(|path| load_sample_at_path(&path));
                    let _ = tx.send(LoadResult {
                        channel,
                        source_revision,
                    new_channel: false,
                        result,
                    });
                });
            });
        }
        {
            let st = state.clone();
            let load_tx = load_tx.clone();
            window.on_previous_sample_clicked(move || {
                let Some(target) = st.borrow().session.selected_sample_target() else {
                    return;
                };
                let tx = load_tx.clone();
                std::thread::spawn(move || {
                    let result = match adjacent_sample(&target.path, -1) {
                        Ok(Some(path)) => Some(load_sample_at_path(&path)),
                        Ok(None) => None,
                        Err(error) => Some(Err(error)),
                    };
                    let _ = tx.send(LoadResult {
                        channel: target.channel,
                        source_revision: target.source_revision,
                        new_channel: false,
                        result,
                    });
                });
            });
        }
        {
            let st = state.clone();
            let load_tx = load_tx.clone();
            window.on_next_sample_clicked(move || {
                let Some(target) = st.borrow().session.selected_sample_target() else {
                    return;
                };
                let tx = load_tx.clone();
                std::thread::spawn(move || {
                    let result = match adjacent_sample(&target.path, 1) {
                        Ok(Some(path)) => Some(load_sample_at_path(&path)),
                        Ok(None) => None,
                        Err(error) => Some(Err(error)),
                    };
                    let _ = tx.send(LoadResult {
                        channel: target.channel,
                        source_revision: target.source_revision,
                        new_channel: false,
                        result,
                    });
                });
            });
        }

        // --- Pump: forward queued commands, apply finished sample loads,
        //     drain audio events onto window ---
        let weak = window.as_weak();
        let st = state.clone();
        let commands = command_state.clone();
        let default_sample_for_pump = default_sample.clone();
        let ui_settings_for_pump = ui_settings.clone();
        let pump = Timer::default();
        // Diagnostics shared with the autodrive self-test (MOOLOOP_AUTODRIVE=1).
        let stats = Rc::new(std::cell::Cell::new((0.0f32, false, 0usize)));
        let stats_in = stats.clone();
        let mut left_meter = MeterBallistics::default();
        let mut right_meter = MeterBallistics::default();
        // One pair per bus, so a strip's decay is its own rather than shared.
        let mut bus_meters: Vec<(MeterBallistics, MeterBallistics)> =
            (0..MAX_BUSES).map(|_| Default::default()).collect();
        let mut last_meter_update = std::time::Instant::now();
        let autodrive_verbose = std::env::var_os("MOOLOOP_AUTODRIVE_VERBOSE").is_some();
        let mut playhead_was_nonempty = false;
        pump.start(
            TimerMode::Repeated,
            std::time::Duration::from_millis(PUMP_INTERVAL_MS),
            move || {
                // Applied here rather than in the callback because the
                // settings and state live in non-Send Rc/RefCells, while the
                // picked path crosses the thread boundary as plain data.
                while let Ok(path) = browser_pick_rx.try_recv() {
                    let Some(window) = weak.upgrade() else {
                        continue;
                    };
                    if st.borrow().session.browser_locations.contains(&path) {
                        window.set_status_message(
                            format!("Already browsing {}", path.display()).into(),
                        );
                        continue;
                    }
                    ui_settings_for_pump
                        .borrow_mut()
                        .browser
                        .locations
                        .push(path.clone());
                    let saved = ui_settings_for_pump.borrow().save();
                    {
                        let mut state = st.borrow_mut();
                        state.session.browser_locations.push(path.clone());
                        state.session.browser_expanded.insert(path.clone());
                        refresh_browser(&state);
                    }
                    match saved {
                        Ok(()) => window.set_status_message(
                            format!("Added sample folder {}", path.display()).into(),
                        ),
                        Err(error) => window.set_status_message(
                            format!("Could not save settings: {error}").into(),
                        ),
                    };
                }
                // Finished inspections fill the info pane and, when autoplay
                // is armed, hand the decoded sample to the preview voice.
                while let Ok(inspection) = browser_info_rx.try_recv() {
                    let Some(window) = weak.upgrade() else {
                        continue;
                    };
                    match inspection {
                        Ok(inspection) => {
                            window.set_browser_info_name(inspection.name.into());
                            window.set_browser_info_stats(inspection.stats.into());
                            window.set_browser_info_waveform(ModelRc::from(Rc::new(
                                VecModel::from(inspection.peaks),
                            )));
                            if window.get_browser_autoplay() {
                                handle.preview(PreviewCommand::Play {
                                    sample: inspection.sample,
                                });
                            }
                        }
                        Err((path, error)) => {
                            window.set_status_message(
                                format!("Could not preview {path}: {error}").into(),
                            );
                        }
                    }
                }
                while let Ok(result) = document_rx.try_recv() {
                    let Some(window) = weak.upgrade() else {
                        return;
                    };
                    window.set_document_busy(false);
                    match result {
                        DocumentResult::Cancelled => {
                            window.set_status_message("".into());
                        }
                        DocumentResult::NewSong(project) => {
                            let samples = vec![None; project.channels.len()];
                            install_project_in_ui(
                                &mut handle,
                                default_sample_for_pump.as_ref(),
                                &st,
                                &window,
                                &project,
                                &samples,
                            );
                            let mut state = st.borrow_mut();
                            state.session.bundle_path = None;
                            state.session.dirty = false;
                            state.session.revision = state.session.revision.wrapping_add(1);
                            state.update_document_title(&window);
                            window.set_status_message("New randomized kit".into());
                        }
                        DocumentResult::SavedSong {
                            path,
                            mode,
                            revision,
                            report,
                            sample_references,
                        } => {
                            let mut state = st.borrow_mut();
                            state.session.bundle_path = Some(path.clone());
                            if state.session.revision == revision {
                                state.session.dirty = false;
                                apply_sample_references(&mut state.session.channels, sample_references);
                            }
                            state.update_document_title(&window);
                            window.set_embed_assets(mode == AssetMode::Embedded);
                            log_info!(
                                "project",
                                "song saved: {} ({} warnings, {} repairs)",
                                path.display(),
                                report.warnings.len(),
                                report.repairs.len()
                            );
                            log_repairs("saving the song", &report.repairs);
                            window.set_status_message(
                                operation_status(
                                    "Song saved",
                                    &path,
                                    &report.warnings,
                                    &report.repairs,
                                )
                                .into(),
                            );
                        }
                        DocumentResult::SavedOther { label, report } => {
                            log_info!("project", "{label}");
                            log_repairs(label, &report.repairs);
                            window.set_status_message(
                                format!(
                                    "{label}{}{}",
                                    warning_suffix(report.warnings.len()),
                                    repair_suffix(report.repairs.len())
                                )
                                .into(),
                            );
                        }
                        DocumentResult::SavedPreset { label, report } => {
                            log_info!("project", "{label}");
                            log_repairs(label, &report.repairs);
                            window.set_status_message(
                                format!(
                                    "{label}{}{}",
                                    warning_suffix(report.warnings.len()),
                                    repair_suffix(report.repairs.len())
                                )
                                .into(),
                            );
                            window.set_save_preset_open(false);
                            refresh_preset_menus(&st, &window);
                        }
                        DocumentResult::Exported { path } => {
                            log_info!("project", "exported {}", path.display());
                            window
                                .set_status_message(format!("Exported {}", path.display()).into());
                        }
                        // Every failure gets the dialog, not just saves: a
                        // song that will not open leaves the user with as
                        // little to go on as one that will not save, and the
                        // status bar cannot hold a located, copyable answer.
                        DocumentResult::Failed { action, problem } => {
                            // Logged here rather than at each failing call
                            // site: this arm is the one place every document
                            // failure passes through, so nothing new can be
                            // added later that forgets to record itself.
                            log_error!("project", "could not {action}: {}", problem.one_line());
                            window.set_save_error_title(format!("Could not {action}").into());
                            window.set_save_error_detail(problem.message.into());
                            window.set_save_error_report(problem.report.into());
                            window.set_save_error_open(true);
                            window.set_status_message(format!("Could not {action}").into());
                        }
                        DocumentResult::Loaded {
                            path,
                            target,
                            document,
                        } => {
                            let ResolvedDocument {
                                report,
                                samples: loaded_samples,
                            } = document;
                            let LoadReport {
                                document,
                                asset_mode,
                                warnings,
                                repairs,
                            } = report;
                            log_info!(
                                "project",
                                "opened {} as {target:?} ({asset_mode:?} assets, {} warnings, {} repairs)",
                                path.display(),
                                warnings.len(),
                                repairs.len()
                            );
                            log_repairs("opening the file", &repairs);
                            let current = st
                                .borrow()
                                .session.project_snapshot(window.get_bpm(), window.get_swing_percent());
                            let current_samples = st.borrow().session.sample_snapshots();
                            // Set by the one load that is an edit of the
                            // open song rather than a replacement of it, and
                            // recorded once the engine has taken it.
                            let mut history_entry: Option<HistoryEntry<ProjectSnapshot>> = None;
                            let merged = match (target, document) {
                                (LoadTarget::Song, LoadedDocument::Song(project)) => {
                                    Some((project, loaded_samples, true))
                                }
                                (LoadTarget::Kit, LoadedDocument::Kit(kit)) => {
                                    let dropping_notes = kit.channels.len()
                                        < current.channels.len()
                                        && current.channels[kit.channels.len()..].iter().any(
                                            |channel| {
                                                channel.notes.iter().any(|lane| !lane.is_empty())
                                            },
                                        );
                                    if dropping_notes
                                        && !confirm_via_zenity(
                                            "This kit removes channels containing notes. Continue?",
                                        )
                                    {
                                        window.set_status_message("Kit load cancelled".into());
                                        None
                                    } else {
                                        let mut project = current.clone();
                                        project.channels = kit
                                            .channels
                                            .into_iter()
                                            .enumerate()
                                            .map(|(index, mut setup)| {
                                                // A kit entry's routes name
                                                // the channel they were saved
                                                // from; they mean this one.
                                                setup.rescope_modulation(index as u8);
                                                if let Some(mut channel) =
                                                    current.channels.get(index).cloned()
                                                {
                                                    channel.setup = setup;
                                                    channel
                                                } else {
                                                    ProjectChannel {
                                                        setup,
                                                        notes: vec![
                                                            Vec::new();
                                                            current.pattern_lengths.len()
                                                        ],
                                                        automation: vec![
                                                            Vec::new();
                                                            current.pattern_lengths.len()
                                                        ],
                                                        next_note_id: 1,
                                                    }
                                                }
                                            })
                                            .collect();
                                        project.selected_channel = project
                                            .selected_channel
                                            .min(project.channels.len().saturating_sub(1) as u8);
                                        let mut samples = current_samples;
                                        samples.resize(project.channels.len(), None);
                                        samples.truncate(project.channels.len());
                                        for (index, sample) in
                                            loaded_samples.into_iter().enumerate()
                                        {
                                            if let Some(slot) = samples.get_mut(index) {
                                                *slot = sample;
                                            }
                                        }
                                        Some((project, samples, false))
                                    }
                                }
                                (LoadTarget::Channel, LoadedDocument::Channel(setup)) => {
                                    let mut project = current;
                                    let selected = project.selected_channel as usize;
                                    project.channels[selected].setup = *setup;
                                    // A saved rack still names the channel it
                                    // was authored on. Point it at this one,
                                    // or a preset saved from channel 3 would
                                    // modulate channel 3 from wherever it
                                    // landed.
                                    mooloop_project::rescope_modulation(
                                        &mut project.channels[selected].setup,
                                        selected as u8,
                                    );
                                    let mut samples = current_samples;
                                    samples[selected] = loaded_samples.into_iter().next().flatten();
                                    Some((project, samples, false))
                                }
                                (LoadTarget::Generator, LoadedDocument::Generator(source)) => {
                                    let mut project = current;
                                    let selected = project.selected_channel as usize;
                                    project.channels[selected].setup.channel.kind = source.kind();
                                    project.channels[selected].setup.source = *source;
                                    let mut samples = current_samples;
                                    samples[selected] = loaded_samples.into_iter().next().flatten();
                                    Some((project, samples, false))
                                }
                                (LoadTarget::Effect { slot }, LoadedDocument::Effect(effect)) => {
                                    // One rack row replaced, as one undoable
                                    // edit like every other rack mutation. The
                                    // session refuses a preset of another
                                    // kind and leaves the rack alone.
                                    let before = project_snapshot(&st.borrow(), &window);
                                    let loaded = st
                                        .borrow_mut()
                                        .session
                                        .load_effect_preset(slot as usize, &effect);
                                    if loaded.is_none() {
                                        window.set_status_message(
                                            "That preset is for a different kind of device"
                                                .into(),
                                        );
                                        None
                                    } else {
                                        st.borrow().sync_effects();
                                        let after = project_snapshot(&st.borrow(), &window);
                                        let project = after.project.clone();
                                        history_entry = Some(HistoryEntry {
                                            before,
                                            after,
                                            label: "Effect preset loaded",
                                            gesture: None,
                                        });
                                        Some((project, current_samples, false))
                                    }
                                }
                                _ => {
                                    window.set_status_message(
                                        "Selected bundle has the wrong document type".into(),
                                    );
                                    None
                                }
                            };
                            if let Some((project, samples, is_song)) = merged {
                                // UI edits are mirrored into `project` before
                                // they enter this relay. Discard any that have
                                // not reached the engine yet: the prepared
                                // project already contains them (or, for a
                                // song load, deliberately supersedes them).
                                while pending_rx.try_recv().is_ok() {}
                                while sample_reset_rx.try_recv().is_ok() {}
                                if !install_project_in_ui(
                                    &mut handle,
                                    default_sample_for_pump.as_ref(),
                                    &st,
                                    &window,
                                    &project,
                                    &samples,
                                ) {
                                    window.set_status_message(
                                        "Audio engine is busy; project was not installed".into(),
                                    );
                                    continue;
                                }
                                if let Some(entry) = history_entry {
                                    let mut commands = commands.borrow_mut();
                                    commands.history.record(entry);
                                    sync_command_availability(&window, &commands);
                                }
                                let mut state = st.borrow_mut();
                                if is_song {
                                    state.session.bundle_path = Some(path.clone());
                                    state.session.dirty = false;
                                    window.set_embed_assets(asset_mode == AssetMode::Embedded);
                                } else {
                                    state.session.dirty = true;
                                    state.session.revision = state.session.revision.wrapping_add(1);
                                }
                                state.update_document_title(&window);
                                window.set_status_message(
                                    format!(
                                        "Loaded {}{}{}",
                                        path.display(),
                                        warning_suffix(warnings.len()),
                                        repair_suffix(repairs.len())
                                    )
                                    .into(),
                                );
                            }
                        }
                    }
                }
                // Resets are applied before loads, never after: a reset
                // carries only "put this channel back to the default sample",
                // while a load carries a sample the user actually asked for.
                // Drained the other way round, a reset queued in the same
                // window -- by adding a channel, or by switching a channel's
                // source to the sampler -- silently overwrites the load and
                // leaves the slot holding the default while the waveform,
                // name, and duration on screen all describe the new file.
                while let Ok(channel) = sample_reset_rx.try_recv() {
                    if let Some(sample) = default_sample_for_pump.as_ref() {
                        handle.load_sample(channel, sample.clone());
                    } else {
                        handle.clear_sample(channel);
                    }
                }
                // After the resets, and both halves together: a slice edit or
                // a commit is the most specific statement about what a
                // channel is playing, and its buffer and its map change at
                // the same instant.
                while let Ok(update) = channel_audio_rx.try_recv() {
                    match update.sample {
                        Some(sample) => handle.load_sample(update.channel, sample),
                        None => handle.clear_sample(update.channel),
                    }
                    match update.slices {
                        Some(slices) => handle.load_slices(update.channel, slices),
                        None => handle.clear_slices(update.channel),
                    }
                }
                let mut deferred_new_channel_load = None;
                while let Ok(load) = load_rx.try_recv() {
                    let still_current = {
                        let st = st.borrow();
                        load.source_revision == st.session.source_revision
                            && (load.new_channel && st.session.channels.len() < MAX_CHANNELS
                                || !load.new_channel
                                    && st
                                        .session.channels
                                        .get(load.channel)
                                        .is_some_and(|channel| channel.kind == DeviceKind::Sampler))
                    };
                    if !still_current {
                        continue;
                    }
                    let Some(loaded) = (match load.result {
                        Some(Ok(loaded)) => Some(loaded),
                        Some(Err(e)) => {
                            log_error!("ui", "failed to load sample: {e}");
                            None
                        }
                        None => None, // dialog cancelled
                    }) else {
                        continue;
                    };
                    if load.new_channel {
                        // The channel does not exist yet. Creating it is
                        // deferred to below, where its default-sample reset
                        // can be spent before this load lands rather than
                        // after.
                        deferred_new_channel_load = Some(loaded);
                        continue;
                    }
                    apply_loaded_sample(&handle, &st, &weak, load.channel, loaded);
                }
                if let Some(loaded) = deferred_new_channel_load {
                    if let Some(window) = weak.upgrade() {
                        window.invoke_add_channel_clicked(0);
                        // Creating the channel queues its own default-sample
                        // reset. Spend it here, so the sample this whole
                        // branch exists to deliver is the last write to the
                        // slot rather than the first.
                        while let Ok(channel) = sample_reset_rx.try_recv() {
                            if let Some(sample) = default_sample_for_pump.as_ref() {
                                handle.load_sample(channel, sample.clone());
                            } else {
                                handle.clear_sample(channel);
                            }
                        }
                        let channel = st.borrow().session.channels.len().saturating_sub(1);
                        apply_loaded_sample(&handle, &st, &weak, channel, loaded);
                    }
                }
                let mut forwarded = 0usize;
                let mut document_title_needs_refresh = false;
                while let Ok(message) = pending_rx.try_recv() {
                    if autodrive_verbose {
                        if let PendingEngineMessage::Command(cmd) = &message {
                            eprintln!("autodrive cmd: {cmd:?}");
                        }
                    }
                    match message {
                        PendingEngineMessage::ProjectEdit(edit) => {
                            let Some(window) = weak.upgrade() else { return };
                            if edit.history.is_some() {
                                commands.borrow_mut().project_edit_pending = false;
                            }
                            if install_project_in_ui(
                                &mut handle,
                                default_sample_for_pump.as_ref(),
                                &st,
                                &window,
                                &edit.project,
                                &edit.samples,
                            ) {
                                let mut state = st.borrow_mut();
                                state.session.dirty = true;
                                state.session.revision = state.session.revision.wrapping_add(1);
                                state.update_document_title(&window);
                                window.set_status_message(edit.status.into());
                                drop(state);
                                if let Some((movement, entry)) = edit.history {
                                    let mut commands = commands.borrow_mut();
                                    match movement {
                                        HistoryMove::Record => commands.history.record(entry),
                                        HistoryMove::Undo => commands.history.commit_undo(),
                                        HistoryMove::Redo => commands.history.commit_redo(),
                                    }
                                    sync_command_availability(&window, &commands);
                                }
                            } else {
                                window.set_status_message("Channel edit is waiting for audio".into());
                                sync_command_availability(&window, &commands.borrow());
                            }
                        }
                        PendingEngineMessage::Audio(action) => {
                            let Some(window) = weak.upgrade() else { return };
                            match action {
                                AudioAction::ApplyPersisted(config) => {
                                    if let Some(target) = config.output_target.clone() {
                                        if let Err(error) = handle.set_output_target(Some(target)) {
                                            log_warn!(
                                                "audio",
                                                "could not apply saved output target: {error}"
                                            );
                                        }
                                    }
                                    if let Some(frames) = config.buffer_size {
                                        if let Err(error) = handle.set_buffer_size(frames) {
                                            log_warn!(
                                                "audio",
                                                "could not apply saved buffer size: {error}"
                                            );
                                        }
                                    }
                                    handle.set_auto_reconnect(config.auto_reconnect);
                                    sync_audio_status(&handle, &window);
                                }
                                AudioAction::RefreshTargets => {
                                    sync_audio_status(&handle, &window);
                                }
                                AudioAction::SelectOutput { port_l, port_r } => {
                                    match handle
                                        .set_output_target(Some((port_l.clone(), port_r.clone())))
                                    {
                                        Ok(()) => {
                                            let mut settings = ui_settings_for_pump.borrow_mut();
                                            settings.audio.jack.output_port_l = Some(port_l);
                                            settings.audio.jack.output_port_r = Some(port_r);
                                            if let Err(error) = settings.save() {
                                                window.set_preferences_audio_error(
                                                    format!("Could not save settings: {error}")
                                                        .into(),
                                                );
                                            } else {
                                                window.set_preferences_audio_error("".into());
                                            }
                                            drop(settings);
                                            sync_audio_status(&handle, &window);
                                        }
                                        Err(error) => {
                                            window.set_preferences_audio_error(error.into())
                                        }
                                    }
                                }
                                AudioAction::SelectBufferSize(frames) => {
                                    match handle.set_buffer_size(frames) {
                                        Ok(()) => {
                                            let mut settings = ui_settings_for_pump.borrow_mut();
                                            settings.audio.jack.buffer_size = Some(frames);
                                            if let Err(error) = settings.save() {
                                                window.set_preferences_audio_error(
                                                    format!("Could not save settings: {error}")
                                                        .into(),
                                                );
                                            } else {
                                                window.set_preferences_audio_error("".into());
                                            }
                                            drop(settings);
                                            sync_audio_status(&handle, &window);
                                        }
                                        Err(error) => {
                                            window.set_preferences_audio_error(error.into())
                                        }
                                    }
                                }
                                AudioAction::SetAutoReconnect(enabled) => {
                                    handle.set_auto_reconnect(enabled);
                                    let mut settings = ui_settings_for_pump.borrow_mut();
                                    settings.audio.jack.auto_reconnect = enabled;
                                    if let Err(error) = settings.save() {
                                        window.set_preferences_audio_error(
                                            format!("Could not save settings: {error}").into(),
                                        );
                                    } else {
                                        window.set_preferences_audio_error("".into());
                                    }
                                    drop(settings);
                                    window.set_preferences_audio_auto_reconnect(enabled);
                                }
                            }
                        }
                        // Everything else needs only the handle.
                        message => {
                            // The self-test counts what reaches the realtime
                            // ring, which is the POD commands and the preview
                            // gain -- not the structural edits beside them.
                            forwarded += usize::from(matches!(
                                message,
                                PendingEngineMessage::Command(_)
                                    | PendingEngineMessage::PreviewGain(_)
                            ));
                            document_title_needs_refresh |= st
                                .borrow_mut()
                                .session
                                .apply_engine_message(&mut handle, message);
                        }
                    }
                }
                if document_title_needs_refresh {
                    let Some(window) = weak.upgrade() else { return };
                    st.borrow().update_document_title(&window);
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
                            let position = st.borrow().session.transport_position(tick);
                            w.set_current_step(position.step);
                            if let Some(ticks) = position.playlist_ticks {
                                w.set_playlist_position_ticks(ticks);
                            }
                            w.set_position_bar(position.bar);
                            w.set_position_beat(position.beat);
                            w.set_position_tick(position.tick);
                        }
                        EngineEvent::Metering { peak_l, peak_r } => {
                            block_peak_l = block_peak_l.max(peak_l.max(0.0));
                            block_peak_r = block_peak_r.max(peak_r.max(0.0));
                            if peak_l > 0.0 || peak_r > 0.0 {
                                saw_nonzero = true;
                            }
                        }
                        EngineEvent::Xrun => {
                            // Read off the event queue on the UI thread. The
                            // audio thread only ever pushes the marker; it
                            // does no formatting and takes no lock.
                            log_warn!("audio", "JACK reported an xrun (audio dropout)");
                        }
                        EngineEvent::ProjectInstalled { .. } => {
                            unreachable!("EngineHandle filters project acknowledgements")
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

                // Bus peaks come from the shared atomic array, not the event
                // ring. Always drain them, even while the mixer is hidden, so
                // a strip does not open showing a peak from minutes ago; only
                // write the models when something is actually displaying them.
                let showing_mixer = w.get_mixer_visible();
                let showing_device_rack = w.get_editor_page() == 0;
                let editing_bus = w.get_editing_bus();
                let edited_bus = w.get_editing_bus_index().max(0) as usize;
                let selected_channel = st.borrow().session.selected;
                for (bus, meters) in bus_meters.iter_mut().enumerate() {
                    let (peak_l, peak_r) = handle.take_bus_peak(bus);
                    let left = meters.0.update(peak_l, elapsed);
                    let right = meters.1.update(peak_r, elapsed);
                    if showing_mixer {
                        let strips = st.borrow();
                        if let Some(mut row) = strips.mixer_strip_model.row_data(bus) {
                            if meter_display_changed(row.left_db, left.level_db, 14)
                                || meter_display_changed(row.right_db, right.level_db, 14)
                            {
                                row.left_db = left.level_db;
                                row.right_db = right.level_db;
                                strips.mixer_strip_model.set_row_data(bus, row);
                            }
                        }
                    }
                    if editing_bus && bus == edited_bus {
                        w.set_editing_bus_left_db(left.level_db);
                        w.set_editing_bus_right_db(right.level_db);
                    }
                }
                // Device meters address channels and buses in one space: a
                // bus's chain publishes at MAX_CHANNELS + bus index (see
                // DeviceMeters). Poll whichever chain the rack is showing.
                let device_target = if editing_bus {
                    mooloop_core::MAX_CHANNELS + edited_bus
                } else {
                    selected_channel
                };
                let ((bus_or_source_in_l, bus_or_source_in_r), (source_out_l, source_out_r)) =
                    handle.take_device_peak(device_target, 0);
                if showing_device_rack && !editing_bus {
                    w.set_source_output_left_db(linear_to_db(source_out_l));
                    w.set_source_output_right_db(linear_to_db(source_out_r));
                } else if showing_device_rack {
                    // A bus has no generator; its head's input meter reads
                    // what the bus summed this block, before its chain.
                    w.set_editing_bus_input_left_db(linear_to_db(bus_or_source_in_l));
                    w.set_editing_bus_input_right_db(linear_to_db(bus_or_source_in_r));
                }
                {
                    let state = st.borrow();
                    for slot in 0..state.effect_slot_model.row_count() {
                        let ((in_l, in_r), (out_l, out_r)) =
                            handle.take_device_peak(device_target, slot + 1);
                        if showing_device_rack {
                            if let Some(mut row) = state.effect_slot_model.row_data(slot) {
                                let input_left_db = linear_to_db(in_l);
                                let input_right_db = linear_to_db(in_r);
                                let output_left_db = linear_to_db(out_l);
                                let output_right_db = linear_to_db(out_r);
                                let meter_changed = meter_display_changed(row.input_left_db, input_left_db, 12)
                                    || meter_display_changed(row.input_right_db, input_right_db, 12)
                                    || meter_display_changed(row.output_left_db, output_left_db, 12)
                                    || meter_display_changed(row.output_right_db, output_right_db, 12);
                                // Non-dynamics stages never publish here, so
                                // they read the resting pair and need no
                                // check for what kind of device they hold.
                                let (detector, reduction_db) =
                                    handle.take_device_dynamics(device_target, slot + 1);
                                let detector_db = linear_to_db(detector);
                                let dynamics_changed =
                                    dynamics_display_changed(row.detector_db, detector_db)
                                        || dynamics_display_changed(
                                            row.gain_reduction_db,
                                            reduction_db,
                                        );
                                if row.eq_analyzer_enabled {
                                    let spectrum = handle.effect_spectrum(state.session.effect_target, slot as u8);
                                    row.eq_spectrum_data = spectrum.as_slice().into();
                                }
                                // A forced return to live leaves no other
                                // trace, so the buffer face reads the count
                                // rather than waiting for an audible cue.
                                let collisions = if row.kind == effect_kind_index(EffectKind::Buffer) {
                                    handle.effect_buffer_collisions(state.session.effect_target, slot as u8)
                                        as i32
                                } else {
                                    row.buffer_collisions
                                };
                                let collisions_changed = collisions != row.buffer_collisions;
                                if meter_changed
                                    || dynamics_changed
                                    || collisions_changed
                                    || row.eq_analyzer_enabled
                                {
                                    row.input_left_db = input_left_db;
                                    row.input_right_db = input_right_db;
                                    row.output_left_db = output_left_db;
                                    row.output_right_db = output_right_db;
                                    row.buffer_collisions = collisions;
                                    row.detector_db = detector_db;
                                    row.gain_reduction_db = reduction_db;
                                    state.effect_slot_model.set_row_data(slot, row);
                                }
                            }
                        }
                    }
                }
                {
                    // A playhead only means anything for the selected
                    // channel's sampler; otherwise leave it empty so no
                    // stale line lingers over an unrelated device or a bus.
                    let state = st.borrow();
                    let is_sampler = state
                        .session.channels
                        .get(selected_channel)
                        .is_some_and(|channel| channel.kind == DeviceKind::Sampler);
                    if showing_device_rack && !editing_bus && is_sampler {
                        let positions = handle.playhead_positions(selected_channel);
                        let has_positions = !positions.is_empty();
                        if has_positions || playhead_was_nonempty {
                            state.playhead_model.set_vec(positions);
                        }
                        playhead_was_nonempty = has_positions;
                    } else if playhead_was_nonempty {
                        playhead_was_nonempty = false;
                        state.playhead_model.set_vec(Vec::new());
                    }
                }
                {
                    // Live modulation on the knobs. The engine publishes the
                    // channel's four modulator outputs; resolving those into a
                    // per-destination offset is the UI's job, so this is a
                    // read of four cells plus arithmetic over the visible
                    // descriptors -- not a per-parameter feed.
                    let state = st.borrow();
                    let outputs = handle.modulator_outputs(selected_channel);
                    let routed = state
                        .session.channels
                        .get(selected_channel)
                        .is_some_and(|channel| {
                            channel.modulation.routes.iter().flatten().next().is_some()
                        });
                    // An unrouted channel has nothing to animate, and once the
                    // outputs stop moving the arcs are already where they
                    // belong -- so neither case is worth a model write.
                    if routed && !editing_bus && outputs != state.session.modulation_outputs.get() {
                        state.session.modulation_outputs.set(outputs);
                        state.refresh_modulation_offsets(&w);
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
                w.invoke_add_pattern_clicked();
                w.invoke_add_channel_clicked(0);
                w.invoke_step_clicked(1, 2);
                w.invoke_pattern_selected(0);
                w.invoke_pattern_length_changed(32);
                w.invoke_step_velocity_edited(0, 0, 0.5);
                w.invoke_step_removed(0, 4);
                w.invoke_piano_note_created(36, 72, 24);
                w.invoke_piano_note_moved(5, 42, 74);
                w.invoke_piano_note_resized(5, 12);
                w.invoke_velocity_edited(5, 0.35);
                w.invoke_piano_note_removed(5);
                w.invoke_voice_mode_changed(1);
                w.invoke_sampler_polyphony_changed(4);
                w.invoke_retrigger_mode_changed(1);
                w.invoke_choke_group_changed(1);
                w.invoke_channel_volume_changed(0, 0.65);
                w.invoke_channel_pan_changed(0, -0.25);
                w.invoke_playlist_placement_added(0, 0);
                w.invoke_playlist_placement_added(1, 192);
                w.invoke_playlist_placement_added(1, 768);
                w.invoke_playlist_placement_removed(1, 768);
                // Effect chain: one slot of every kind, edited across their
                // descriptor tables, then reordered, bypassed, and removed.
                // Covers the full structural/param/swap command surface and
                // proves each kind is constructible from the UI path.
                w.invoke_add_effect_clicked(0, 0);
                w.invoke_add_effect_clicked(1, 1);
                w.invoke_add_effect_clicked(2, 2);
                w.invoke_effect_param_changed(0, 0, 0.4); // filter cutoff
                w.invoke_effect_param_changed(0, 1, 0.5); // filter resonance
                w.invoke_effect_param_changed(0, 2, 1.0); // filter -> high-pass
                w.invoke_effect_param_changed(1, 0, 0.75); // drive amount
                w.invoke_effect_param_changed(1, 1, 2.0 / 3.0); // drive -> fold
                w.invoke_effect_param_changed(1, 3, 0.8); // drive mix
                w.invoke_effect_param_changed(2, 0, 0.2); // bitcrush bits
                w.invoke_effect_param_changed(2, 1, 0.6); // bitcrush rate
                w.invoke_add_effect_clicked(3, 3);
                w.invoke_effect_param_changed(3, 0, 0.6); // delay time
                w.invoke_effect_param_changed(3, 1, 0.5); // delay feedback
                w.invoke_effect_param_changed(3, 2, 1.0); // delay -> reverse
                w.invoke_effect_param_changed(3, 3, 1.0); // delay ping-pong
                w.invoke_add_effect_clicked(4, 4);
                w.invoke_effect_param_changed(4, 0, 0.4); // gate threshold
                w.invoke_effect_param_changed(4, 4, 0.2); // gate range
                w.invoke_add_effect_clicked(5, 5);
                w.invoke_effect_param_changed(5, 0, 0.5); // comp threshold
                w.invoke_effect_param_changed(5, 1, 0.8); // comp ratio
                w.invoke_add_effect_clicked(6, 6);
                w.invoke_effect_param_changed(6, 0, 0.9); // limiter ceiling
                w.invoke_effect_param_changed(6, 2, 0.4); // limiter gain
                w.invoke_reorder_effect(0, 2);
                w.invoke_effect_bypass_toggled(0);
                w.invoke_effect_bypass_toggled(0);
                w.invoke_remove_effect_clicked(1);
                // Mixer: assign channels to buses, chain one bus into
                // another, and build an effect chain on a bus rather than a
                // channel. This is the surface the routing rule guards, so
                // include the uphill route it must refuse.
                w.set_mixer_visible(true);
                w.invoke_channel_bus_changed(0, 3);
                w.invoke_channel_bus_changed(1, 3);
                w.invoke_bus_output_changed(3, 1);
                w.invoke_bus_output_changed(1, 9); // uphill: must fall back
                w.invoke_bus_volume_changed(3, 0.7);
                w.invoke_bus_pan_changed(3, -0.4);
                w.invoke_bus_muted(3);
                w.invoke_bus_muted(3);
                w.invoke_bus_selected(3);
                w.invoke_add_effect_clicked(5, 0); // compressor on the bus
                w.invoke_effect_param_changed(0, 0, 0.45);
                w.invoke_effect_param_changed(0, 1, 0.6);
                w.invoke_bus_selected(0); // master
                w.invoke_add_effect_clicked(6, 0); // limiter on the master
                w.invoke_effect_param_changed(0, 0, 0.95);
                w.invoke_channel_selected(0); // back to a channel's chain
                w.set_mixer_visible(false);
                w.set_song_mode(true);
                w.invoke_playback_mode_changed(true);
                w.set_editor_page(2);
                w.invoke_play_clicked();
            });
            let stats = stats.clone();
            slint::Timer::single_shot(std::time::Duration::from_millis(4500), move || {
                let (max_peak, saw_playing, forwarded) = stats.get();
                println!("--- ui autodrive report ---");
                println!("commands forwarded by pump : {forwarded}");
                println!("saw playing=true on window : {saw_playing}");
                println!("nonzero metering seen     : {max_peak:.4}");
                let ok = saw_playing && forwarded >= 31;
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

/// Opens the drag-and-drop UI mockup tool as a standalone window, shown
/// alongside the main app rather than blocking it. Same component and same
/// wiring as `cargo run -p mooloop-ui --example mockup`; the developer
/// preferences page just saves leaving the running app to reach it.
#[cfg(feature = "mockup")]
fn open_mockup_window() -> Result<mockup_ui::MockupCanvas, slint::PlatformError> {
    let canvas = mockup_ui::MockupCanvas::new()?;
    mockup::wire_mockup(&canvas);
    canvas.show()?;
    Ok(canvas)
}

fn asset_mode_from_window(window: &MainWindow) -> AssetMode {
    if window.get_embed_assets() {
        AssetMode::Embedded
    } else {
        AssetMode::Referenced
    }
}

fn operation_status(
    label: &str,
    path: &Path,
    warnings: &[AssetWarning],
    repairs: &[Issue],
) -> String {
    format!(
        "{label}: {}{}{}",
        path.display(),
        warning_suffix(warnings.len()),
        repair_suffix(repairs.len())
    )
}

fn install_project_in_ui(
    handle: &mut EngineHandle,
    default_sample: Option<&Arc<SampleData>>,
    state: &Rc<RefCell<UiState>>,
    window: &MainWindow,
    project: &Project,
    samples: &[Option<Arc<SampleData>>],
) -> bool {
    let mut project = project.clone();
    normalize_project_pattern_banks(&mut project);
    // Queue the complete state first. If the bounded realtime queue is full,
    // leave both the sample slots and visible project untouched.
    if !handle.install_project(Arc::new(project.clone())) {
        return false;
    }
    for index in 0..MAX_CHANNELS {
        let sample = project
            .channels
            .get(index)
            // Asked through the accessor rather than by naming every
            // generator: this is a question about samples, and the four
            // synths were only listed here to say "not me".
            .and_then(|channel| match channel.setup.source.sampler_state() {
                Some(sampler) => samples.get(index).cloned().flatten().or_else(|| {
                    matches!(sampler.sample, SampleReference::Builtin { .. })
                        .then(|| default_sample.cloned())
                        .flatten()
                }),
                None => default_sample.cloned(),
            });
        if let Some(sample) = sample {
            handle.load_sample(index, sample);
        } else {
            handle.clear_sample(index);
        }
    }
    state.borrow_mut().replace_project(&project, samples, window);
    // Republish from the installed state rather than from `samples`: a
    // channel whose stretch was committed plays the re-rendered buffer, and
    // its slice map has to arrive with it.
    {
        let st = state.borrow();
        for (index, channel) in st.session.channels.iter().enumerate() {
            if channel.kind == DeviceKind::Sampler {
                publish_channel_audio(handle, index, channel);
            }
        }
    }
    sync_effect_spectrum_subscriptions(&state.borrow(), handle);
    window.set_playing(false);
    window.set_playlist_position_ticks(0);
    refresh_preset_menus(state, window);
    true
}

fn sync_effect_spectrum_subscriptions(state: &UiState, handle: &EngineHandle) {
    for (channel, setup) in state.session.channels.iter().enumerate() {
        for (slot, effect) in setup.effects.iter().enumerate() {
            if let Some(eq) = effect.params.eq() {
                handle.set_effect_spectrum_enabled(
                    EffectTarget::Channel(channel as u8),
                    slot as u8,
                    eq.analyzer_enabled,
                );
            }
        }
    }
    for (bus, setup) in state.session.buses.iter().enumerate() {
        for (slot, effect) in setup.effects.iter().enumerate() {
            if let Some(eq) = effect.params.eq() {
                handle.set_effect_spectrum_enabled(
                    EffectTarget::Bus(bus as u8),
                    slot as u8,
                    eq.analyzer_enabled,
                );
            }
        }
    }
}

fn preset_menu_label(preset: &PresetSummary) -> slint::SharedString {
    if preset.category.trim().is_empty() {
        preset.name.as_str().into()
    } else {
        format!("{} — {}", preset.category, preset.name).into()
    }
}

/// Re-scans the on-disk preset directories for the currently selected
/// channel's generator kind, plus the whole-channel presets, and pushes
/// the results into the `MenuField` popups. Cheap enough to call on every
/// channel/kind switch and project load: presets are a handful of small
/// TOML manifests, not a large library.
fn refresh_preset_menus(state: &Rc<RefCell<UiState>>, window: &MainWindow) {
    let kind = {
        let st = state.borrow();
        st.session.channels
            .get(st.session.selected)
            .map(|channel| channel.kind)
            .unwrap_or(DeviceKind::Sampler)
    };
    let generator_presets = mooloop_project::list_presets(&settings::generator_presets_dir(kind));
    let channel_presets = mooloop_project::list_presets(&settings::channel_presets_dir());
    // Every kind's directory in one scan, kept flat: each rack row filters
    // the list down to its own kind when its row is built.
    let effect_presets: Vec<PresetSummary> = EffectKind::ALL
        .iter()
        .flat_map(|kind| mooloop_project::list_presets(&settings::effect_presets_dir(*kind)))
        .collect();
    {
        let mut st = state.borrow_mut();
        st.session.generator_presets = generator_presets;
        st.session.channel_presets = channel_presets;
        st.session.effect_presets = effect_presets;
    }
    let st = state.borrow();
    st.sync_generator_preset_menu(window);
    st.sync_channel_preset_menu(window);
    st.sync_effects();
}


/// Hand one channel's audio and slice map to the engine.
///
/// The *published* buffer, not the source: after a commit the engine plays
/// the render. Both travel out of band through `ArcSwap` slots rather than on
/// the command ring, so this is wait-free and safe to call from the UI thread.
fn publish_channel_audio(handle: &EngineHandle, index: usize, channel: &ChannelState) {
    match channel.published_sample() {
        Some(sample) => handle.load_sample(index, sample.clone()),
        None => handle.clear_sample(index),
    }
    if channel.slices.is_empty() {
        handle.clear_slices(index);
    } else {
        handle.load_slices(index, Arc::new(channel.slices.clone()));
    }
}

/// Publish a finished background load to `channel`: hand the decoded sample
/// to the engine, record it on the channel state, and refresh the visible
/// editor when that channel is the one on screen.
fn apply_loaded_sample(
    handle: &EngineHandle,
    st: &Rc<RefCell<UiState>>,
    weak: &slint::Weak<MainWindow>,
    channel: usize,
    loaded: LoadedSample,
) {
    let name = loaded
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("loaded")
        .to_string();
    log_debug!("ui", "channel {channel} loaded {name}");
    let waveform = waveform_peaks(&loaded.sample, WAVEFORM_BINS);
    let description = sample_description(&loaded.sample);
    let duration = sample_duration(&loaded.sample);
    handle.load_sample(channel, loaded.sample.clone());
    // The markers went with the old file; the engine must not keep playing a
    // map that names frames in audio it no longer holds.
    handle.clear_slices(channel);
    let mut st = st.borrow_mut();
    if let Some(ch) = st.session.channels.get_mut(channel) {
        ch.sample_name = name;
        ch.sample_description = description;
        ch.sample_duration = duration;
        ch.sample_path = Some(loaded.path);
        ch.sample_embedded = false;
        ch.sample_data = Some(loaded.sample.clone());
        // A new file retires the old commit and the old markers outright:
        // both named frames in audio that is no longer loaded.
        ch.committed_sample = None;
        ch.commit = None;
        ch.slices.clear();
        ch.waveform = waveform;
        ch.can_previous_sample = loaded.can_previous;
        ch.can_next_sample = loaded.can_next;
    }
    st.session.dirty = true;
    st.session.revision = st.session.revision.wrapping_add(1);
    if channel == st.session.selected {
        if let Some(window) = weak.upgrade() {
            st.refresh_editor(&window);
            st.update_document_title(&window);
        }
    }
}

/// Decode a browser sample off the UI thread and deliver it to the pump as a
/// `LoadResult`. `new_channel` targets the sampler channel the pump will
/// create on arrival, so `channel` is the index it will take.
fn spawn_browser_sample_load(
    path: &str,
    channel: usize,
    source_revision: u64,
    new_channel: bool,
    load_tx: &std::sync::mpsc::Sender<LoadResult>,
) {
    let path = PathBuf::from(path.to_string());
    let tx = load_tx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(LoadResult {
            channel,
            source_revision,
            new_channel,
            result: Some(load_sample_at_path(&path)),
        });
    });
}

/// Flattens the browser's folder hierarchy into visible rows: each location
/// that can play something, then recursively the children of every expanded
/// folder. Folders whose whole subtree is unplayable are hidden.
fn build_browser_rows(locations: &[PathBuf], expanded: &HashSet<PathBuf>) -> Vec<BrowserRow> {
    let mut rows = Vec::new();
    for location in locations {
        if has_playable_descendant(location, 0) {
            push_browser_rows(&mut rows, location, 0, expanded);
        }
    }
    rows
}

fn push_browser_rows(
    rows: &mut Vec<BrowserRow>,
    path: &Path,
    depth: usize,
    expanded: &HashSet<PathBuf>,
) {
    let is_expanded = expanded.contains(path);
    rows.push(BrowserRow {
        depth: depth as i32,
        kind: 0,
        name: browser_display_name(path).into(),
        path: path.to_string_lossy().to_string().into(),
        expanded: is_expanded,
    });
    if !is_expanded {
        return;
    }
    for (is_dir, child) in scan_browser_dir(path) {
        if is_dir {
            // A folder with nothing playable below it is noise, however
            // legitimately it exists on disk.
            if has_playable_descendant(&child, depth + 1) {
                push_browser_rows(rows, &child, depth + 1, expanded);
            }
        } else {
            rows.push(BrowserRow {
                depth: depth as i32 + 1,
                kind: 1,
                name: browser_display_name(&child).into(),
                path: child.to_string_lossy().to_string().into(),
                expanded: false,
            });
        }
    }
}

/// Rebuilds the visible tree from the session's locations and expansion set.
fn refresh_browser(st: &UiState) {
    st.browser_rows.set_vec(build_browser_rows(
        &st.session.browser_locations,
        &st.session.browser_expanded,
    ));
}

#[cfg(test)]
mod tests {
    use mooloop_session::browser::is_playable_sample;

    #[test]
    fn modulation_uses_two_rack_units() {
        assert_eq!(effect_kind_units(EffectKind::Modulation), 2);
    }

    use super::*;

    #[test]
    fn musical_divisions_match_the_snap_table_in_main_slint() {
        // `snap-ticks()` in main.slint is the other half of this table, and
        // the length picker indexes into `musical-snap-options` by the same
        // position. A drift between them would set the wrong length silently.
        assert_eq!(
            super::MUSICAL_DIVISIONS.map(|(ticks, _)| ticks),
            [384, 192, 96, 64, 48, 32, 24, 16, 12, 8, 6]
        );
    }

    #[test]
    fn note_lengths_read_as_note_values() {
        assert_eq!(super::length_text(96), "1/4");
        assert_eq!(super::length_text(24), "1/16");
        assert_eq!(super::length_text(64), "1/4T");
        // Dotted forms get a name rather than a remainder, being common.
        assert_eq!(super::length_text(144), "1/4.");
        assert_eq!(super::length_text(36), "1/16.");
    }

    #[test]
    fn an_unsnapped_length_shows_its_remainder_rather_than_rounding() {
        // What a free drag produces. The point of the readout is that the
        // exact value is never hidden behind a tidy label.
        assert_eq!(super::length_text(26), "1/16 +2");
        assert_eq!(super::length_text(100), "1/4 +4");
        // Shorter than the smallest division there is.
        assert_eq!(super::length_text(3), "3t");
        assert_eq!(super::length_text(0), "");
    }

    #[test]
    fn only_an_exact_division_selects_a_picker_entry() {
        assert_eq!(super::division_index(24), 6);
        assert_eq!(super::division_index(384), 0);
        // A dotted or free length has no entry, so the picker shows none and
        // the readout carries the value instead.
        assert_eq!(super::division_index(36), -1);
        assert_eq!(super::division_index(26), -1);
    }

    #[test]
    fn browser_load_delivery_carries_its_target() {
        let (tx, rx) = std::sync::mpsc::channel();
        spawn_browser_sample_load("/nonexistent/missing.wav", 3, 7, true, &tx);
        let load = rx.recv().unwrap();
        assert_eq!(load.channel, 3);
        assert_eq!(load.source_revision, 7);
        assert!(load.new_channel);
        // The decode fails off-thread; the pump owns the user-visible handling.
        assert!(matches!(load.result, Some(Err(_))));
    }

    #[test]
    fn browser_tree_hides_folders_without_playable_samples() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // `Empty` has nothing below it, `Deep` only earns its place through
        // an audio file two levels down, and `silent` holds nothing but text.
        std::fs::create_dir_all(root.join("Empty")).unwrap();
        std::fs::create_dir_all(root.join("Deep/Nested")).unwrap();
        std::fs::create_dir_all(root.join("silent")).unwrap();
        std::fs::write(root.join("Deep/Nested/hit.wav"), b"x").unwrap();
        std::fs::write(root.join("silent/readme.txt"), b"x").unwrap();

        let rows = build_browser_rows(&[root.to_path_buf()], &HashSet::from([root.to_path_buf()]));
        let names: Vec<String> = rows.iter().map(|row| row.name.to_string()).collect();
        // The collapsed root hides its children, so expansion is required to
        // prove `Deep` survives (via audio two levels down) while `Empty` and
        // `silent` — nothing playable below either — are hidden.
        assert_eq!(names, vec![browser_display_name(root), "Deep".to_owned()]);

        // A location with nothing playable anywhere below it lists as
        // nothing at all.
        let bare = tempfile::tempdir().unwrap();
        std::fs::write(bare.path().join("notes.txt"), b"x").unwrap();
        assert!(build_browser_rows(&[bare.path().to_path_buf()], &HashSet::new()).is_empty());

        // The playable predicate is the single switch new formats flip.
        assert!(is_playable_sample(Path::new("a.WAV")));
        assert!(!is_playable_sample(Path::new("a.txt")));
    }

    #[test]
    fn browser_tree_lists_folders_first_and_only_supported_audio() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("Drums/Kicks")).unwrap();
        std::fs::create_dir_all(root.join("ambience")).unwrap();
        std::fs::write(root.join("Drums/Kicks/909.wav"), b"x").unwrap();
        std::fs::write(root.join("Drums/Kicks/notes.txt"), b"x").unwrap();
        std::fs::write(root.join("zebra.WAV"), b"x").unwrap();
        std::fs::write(root.join(".hidden.wav"), b"x").unwrap();
        std::fs::write(root.join("apple.wav"), b"x").unwrap();
        std::fs::write(root.join("middle.flac"), b"x").unwrap();

        // Root and Drums expanded; everything else starts collapsed.
        let expanded: HashSet<PathBuf> = [root.to_path_buf(), root.join("Drums")]
            .into_iter()
            .collect();
        let rows = build_browser_rows(&[root.to_path_buf()], &expanded);

        let summary: Vec<(usize, i32, bool, String)> = rows
            .iter()
            .map(|row| {
                (
                    row.depth as usize,
                    row.kind,
                    row.expanded,
                    row.name.to_string(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (0, 0, true, browser_display_name(root)),
                // `ambience` has nothing playable below it, so it is hidden;
                // Dirs before files, case-insensitively sorted.
                (1, 0, true, "Drums".into()),
                (2, 0, false, "Kicks".into()),
                (1, 1, false, "apple.wav".into()),
                (1, 1, false, "middle.flac".into()),
                (1, 1, false, "zebra.WAV".into()),
            ]
        );
        // The rows carry their paths so toggling and removal round-trip.
        assert_eq!(
            rows[1].path,
            root.join("Drums").to_string_lossy().to_string()
        );
        assert_eq!(
            rows[2].path,
            root.join("Drums/Kicks").to_string_lossy().to_string()
        );
        assert_eq!(
            rows[3].path,
            root.join("apple.wav").to_string_lossy().to_string()
        );

        // Collapsed locations list as a single row with no children.
        let rows = build_browser_rows(&[root.to_path_buf()], &HashSet::new());
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].expanded);
    }

    #[test]
    fn rack_cell_preserves_sixty_fourth_note_gaps() {
        let notes = [
            NoteEvent::new(1, 0, 6, 60, 70),
            NoteEvent::new(2, 12, 6, 62, 110),
        ];
        let cell = rack_cell(&notes, 0);
        assert!(cell.active);
        assert_eq!(cell.substeps, 0b0101);
        assert_eq!(cell.velocity, 110);
    }

    #[test]
    fn rack_cell_fills_note_duration_at_its_actual_velocity() {
        let cell = rack_cell(&[NoteEvent::new(1, 0, TICKS_PER_STEP, 60, 42)], 0);
        assert!(cell.active);
        assert_eq!(cell.substeps, 0b1111);
        assert_eq!(cell.velocity, 42);
    }

    #[test]
    fn rack_cell_shows_duration_crossing_a_step_boundary() {
        let notes = [NoteEvent::new(1, 18, 12, 60, 90)];
        assert_eq!(rack_cell(&notes, 0).substeps, 0b1000);
        assert_eq!(rack_cell(&notes, 1).substeps, 0b0001);
    }

    #[test]
    fn separates_struck_substeps_from_held_ones() {
        // One note filling the whole sixteenth covers every 64th but is only
        // struck on the first, so coverage alone cannot describe it.
        let sustained = vec![NoteEvent::new(1, 0, TICKS_PER_STEP, 60, 100)];
        let cell = rack_cell(&sustained, 0);
        assert_eq!(cell.substeps, 0b1111);
        assert_eq!(cell.onsets, 0b0001);

        // The same coverage, ratcheted into two hits, is struck twice. If the
        // rack drew coverage alone these two cells would be indistinguishable.
        let half = TICKS_PER_STEP / 2;
        let ratcheted = vec![
            NoteEvent::new(1, 0, half, 60, 100),
            NoteEvent::new(2, half, half, 60, 100),
        ];
        let cell = rack_cell(&ratcheted, 0);
        assert_eq!(cell.substeps, 0b1111);
        assert_eq!(cell.onsets, 0b0101);

        // A note running in from an earlier sixteenth is held, never struck.
        let carried = vec![NoteEvent::new(1, 0, TICKS_PER_STEP * 2, 60, 100)];
        let cell = rack_cell(&carried, 1);
        assert_eq!(cell.substeps, 0b1111);
        assert_eq!(cell.onsets, 0);
    }

    #[test]
    fn note_cell_reports_membership_in_a_multi_selection() {
        let note = NoteEvent::new(5, 0, TICKS_PER_STEP, 60, 100);
        assert!(!note_cell(note, &HashSet::new()).selected);

        let mut selected = HashSet::new();
        selected.insert(3);
        assert!(
            !note_cell(note, &selected).selected,
            "a note not in the set should not read as selected"
        );

        selected.insert(5);
        assert!(
            note_cell(note, &selected).selected,
            "every member of the set should read as selected, not just a lone primary"
        );
    }

}
