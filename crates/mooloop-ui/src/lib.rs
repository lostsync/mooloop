//! Slint UI wrapper. Owns the `EngineHandle`, wires Slint callbacks to engine
//! commands, and runs a high-frequency timer that forwards commands and drains
//! audio events onto window properties.
//!
//! The UI owns the project state (channels, pattern bank, per-channel sampler
//! params) as the source of truth and mirrors every mutation to the engine
//! via commands. The engine keeps its own pre-allocated copy.

mod actions;
mod history;
mod meter;
mod settings;

slint::include_modules!();

use history::{Entry as HistoryEntry, History};
use meter::{db_to_linear, linear_to_db, MeterBallistics, MIN_DB as METER_FLOOR_DB};
use mooloop_core::{
    compile_bus_graph, default_buses, sanitize_route, would_create_cycle, AutomationLane,
    AutomationPoint, BufferDuration,
    BufferEvent, BusSetup, Channel, ChannelSetup, ChannelSource, DeviceKind, DrumMode,
    DrumSynthParams, DrumSynthState, EffectKind, EffectParams, EffectSlotState, EffectTarget,
    EngineCommand, EngineEvent, HatCharacter, KickCharacter, Kit, LfoWave, LoopMode, ModRack,
    MonoSynthParams, MonoSynthState, NoteEvent, NoteId, OscWave, ParamAddr, ParamDescriptor,
    ParamOwner, PatternPlacement, PlaybackMode, PointId,
    PolySynthParams, PolySynthState, Ppq, Project, ProjectChannel, RetriggerMode, ReverbParams,
    SampleReference, SamplerParams, SamplerState, SnareCharacter, VoiceMode,
    DEFAULT_NOTE_DURATION_TICKS, DEFAULT_STEPS, DEFAULT_SWING_PERCENT, MASTER_BUS,
    MAX_AUTOMATION_LANES_PER_CHANNEL, MAX_BUSES,
    MAX_CHANNELS, MAX_EFFECTS_PER_CHANNEL, MAX_LINEAR_GAIN, MAX_PATTERNS, MAX_PATTERN_STEPS,
    MAX_PLAYLIST_BARS, MAX_PLAYLIST_PLACEMENTS, MAX_PLAYLIST_TICKS, MAX_POLY_VOICES,
    MAX_SWING_PERCENT, MIN_SWING_PERCENT, TICKS_PER_64TH, TICKS_PER_BAR, TICKS_PER_STEP,
};
use mooloop_dsp::{
    buffer_allocation_key, build_effect, build_effect_at_tempo, DrumSynth, DryAlign, SampleData,
    SpectrumAnalyzer,
};
use mooloop_engine::{
    EngineHandle, ExportFormat, ExportSpec, Mp3Bitrate, OfflineRenderer, RenderScope,
    StructuralCommand, WavEncoding,
};
use mooloop_project::{
    AssetMode, AssetWarning, LoadReport, LoadedDocument, PresetInfo, PresetSummary, SaveReport,
};
use settings::{AppearancePreset, AppearanceSettings, ThemePalette, UiSettings};
use slint::{CloseRequestResponse, ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const PUMP_INTERVAL_MS: u64 = 8;
const INITIAL_BPM: i32 = 120;
/// Fader positions for time-based params map onto [0, MAX_TIME_S] seconds.
const MAX_TIME_S: f32 = 2.0;

/// UI callbacks all run on one thread, but boxed structural edits and POD
/// commands used to enter separate relay queues and lose their relative
/// order. These typed senders share one queue while preserving the convenient
/// `.send(...)` call shape used by the callback wiring below.
enum PendingEngineMessage {
    Command(EngineCommand),
    ResizeBuffers { bpm: f64 },
    Structural(StructuralCommand),
    ProjectEdit(ProjectEdit),
    Audio(AudioAction),
    Telemetry(TelemetryAction),
}

/// A prepared-room request stays off the UI and audio threads. A short
/// coalescing interval means pointer drags regenerate the final room state,
/// not every intermediate pixel.
#[derive(Clone, Copy)]
struct ReverbBuildRequest {
    target: EffectTarget,
    slot: u8,
    expected_resource_key: u64,
    params: ReverbParams,
}

/// Display subscriptions are handled by the pump, which exclusively owns the
/// engine handle. They observe a device's signal; they are not audio-thread
/// commands and never become modulation routes.
enum TelemetryAction {
    SetEffectSpectrumEnabled {
        target: EffectTarget,
        slot: u8,
        enabled: bool,
    },
}

/// One requested change from the Audio preferences page. These reach
/// `EngineHandle` directly rather than through `EngineCommand`: they are
/// non-realtime JACK API calls (port connect/disconnect, buffer resize), not
/// realtime-thread state, but `handle` still only lives inside the pump.
enum AudioAction {
    /// Apply settings loaded from disk at startup, before the user has
    /// touched the Audio page.
    ApplyPersisted(mooloop_engine::AudioConfig),
    /// Re-read the live JACK graph and driver status.
    RefreshTargets,
    SelectOutput {
        port_l: String,
        port_r: String,
    },
    SelectBufferSize(u32),
    SetAutoReconnect(bool),
}

#[derive(Clone)]
struct EngineCommandSender(std::sync::mpsc::Sender<PendingEngineMessage>);

impl EngineCommandSender {
    fn send(&self, command: EngineCommand) -> bool {
        self.0.send(PendingEngineMessage::Command(command)).is_ok()
    }

    fn resize_buffers(&self, bpm: f64) -> bool {
        self.0
            .send(PendingEngineMessage::ResizeBuffers { bpm })
            .is_ok()
    }
}

#[derive(Clone)]
struct StructuralCommandSender(std::sync::mpsc::Sender<PendingEngineMessage>);

impl StructuralCommandSender {
    fn send(&self, command: StructuralCommand) -> bool {
        self.0
            .send(PendingEngineMessage::Structural(command))
            .is_ok()
    }
}

#[derive(Clone)]
struct ProjectEditSender(std::sync::mpsc::Sender<PendingEngineMessage>);

impl ProjectEditSender {
    fn send(&self, edit: ProjectEdit) -> bool {
        self.0.send(PendingEngineMessage::ProjectEdit(edit)).is_ok()
    }
}

#[derive(Clone)]
struct AudioActionSender(std::sync::mpsc::Sender<PendingEngineMessage>);

impl AudioActionSender {
    fn send(&self, action: AudioAction) -> bool {
        self.0.send(PendingEngineMessage::Audio(action)).is_ok()
    }
}

#[derive(Clone)]
struct TelemetryActionSender(std::sync::mpsc::Sender<PendingEngineMessage>);

impl TelemetryActionSender {
    fn send(&self, action: TelemetryAction) -> bool {
        self.0.send(PendingEngineMessage::Telemetry(action)).is_ok()
    }
}

/// Fixed JACK buffer size choices offered by the segmented control on the
/// Audio preferences page. Index-addressed to match `SegmentedControl`.
const JACK_BUFFER_SIZES: [u32; 6] = [64, 128, 256, 512, 1024, 2048];
const WAVEFORM_BINS: usize = 256;
const DRUM_PREVIEW_BINS: usize = 144;

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

fn sync_preferences_properties(window: &MainWindow, settings: &UiSettings) {
    window.set_preferences_appearance_preset(settings.appearance.preset.index());
    window.set_preferences_appearance_accent(settings.appearance.accent.as_str().into());
    window.set_preferences_developer_mode(settings.general.developer_mode);
    window.set_preferences_smooth_curves(settings.appearance.smooth_curves);
    window
        .global::<DisplayPrefs>()
        .set_smooth_curves(settings.appearance.smooth_curves);
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

/// UI-side state for one channel. `notes` is the pattern bank.
/// Coerce a loaded bus bank to the fixed size the engine preallocates,
/// padding a short one and repairing any routing an older or hand-edited file
/// left illegal. Everything downstream can then index the bank directly.
///
/// Per-edge nonsense is fixed first, then the graph as a whole: a file whose
/// routing contains a loop is flattened to everything-to-master rather than
/// rejected, matching what the engine does with the same file.
fn normalized_buses(buses: &[BusSetup]) -> Vec<BusSetup> {
    let mut normalized: Vec<BusSetup> = (0..MAX_BUSES)
        .map(|index| match buses.get(index) {
            Some(setup) => {
                let mut setup = setup.clone();
                setup.bus.output = sanitize_route(index as u8, setup.bus.output);
                setup
            }
            None => BusSetup::new(index),
        })
        .collect();
    if compile_bus_graph(&normalized).is_none() {
        for setup in &mut normalized {
            setup.bus.output = MASTER_BUS;
        }
    }
    normalized
}

struct ChannelState {
    name: String,
    kind: DeviceKind,
    muted: bool,
    volume: f32,
    pan: f32,
    params: SamplerParams,
    drum_params: DrumSynthParams,
    mono_params: MonoSynthParams,
    poly_params: PolySynthParams,
    sample_name: String,
    sample_description: String,
    sample_duration: f32,
    sample_path: Option<PathBuf>,
    sample_embedded: bool,
    sample_data: Option<Arc<SampleData>>,
    waveform: Vec<f32>,
    can_previous_sample: bool,
    can_next_sample: bool,
    notes: Vec<Vec<NoteEvent>>,
    /// Pattern-indexed automation lanes, parallel to `notes`. A lane is kept
    /// even when the editor is not showing it, so switching the visible lane
    /// never destroys what is behind it.
    automation: Vec<Vec<AutomationLane>>,
    next_note_id: NoteId,
    effects: Vec<EffectSlotState>,
    modulation: ModRack,
    /// Mixer bus this channel feeds; 0 is the master.
    bus: u8,
}

impl ChannelState {
    /// A brand new sampler channel is silent and empty until a sample is
    /// loaded or a project assigns one.
    fn new(index: usize) -> Self {
        Self {
            name: format!("Sampler {}", index + 1),
            kind: DeviceKind::Sampler,
            muted: false,
            volume: 0.8,
            pan: 0.0,
            params: SamplerParams::default(),
            drum_params: DrumSynthParams::default(),
            mono_params: MonoSynthParams::default(),
            poly_params: PolySynthParams::default(),
            sample_name: String::new(),
            sample_description: String::new(),
            sample_duration: 0.0,
            sample_path: None,
            sample_embedded: false,
            sample_data: None,
            waveform: Vec::new(),
            can_previous_sample: false,
            can_next_sample: false,
            notes: vec![Vec::new()],
            automation: vec![Vec::new()],
            next_note_id: 1,
            effects: Vec::new(),
            modulation: ModRack::default(),
            bus: MASTER_BUS,
        }
    }

    fn create_note(
        &mut self,
        pattern: usize,
        start_tick: u32,
        duration_ticks: u32,
        note: u8,
    ) -> NoteEvent {
        let event = NoteEvent::new(self.next_note_id, start_tick, duration_ticks, note, 100);
        self.next_note_id = self.next_note_id.wrapping_add(1).max(1);
        self.notes[pattern].push(event);
        self.notes[pattern].sort_by_key(|note| (note.start_tick, note.id));
        event
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
    source_revision: u64,
    /// `None` = dialog cancelled; `Some(Err)` = decode failed.
    result: Option<Result<LoadedSample, String>>,
}

struct ResolvedDocument {
    report: LoadReport,
    samples: Vec<Option<Arc<SampleData>>>,
}

/// In-memory channel clipboard. It intentionally keeps decoded sample data
/// alongside the serializable channel so pasting never needs to re-read a WAV
/// on the UI thread.
#[derive(Clone)]
struct ChannelClipboard {
    channel: ProjectChannel,
    sample: Option<Arc<SampleData>>,
}

/// A complete, UI-owned project snapshot. Samples stay beside the serializable
/// project because restoring an edit must never decode a WAV on the UI thread.
#[derive(Clone)]
struct ProjectSnapshot {
    project: Project,
    samples: Vec<Option<Arc<SampleData>>>,
}

/// The command layer's state. Clipboard data and history live here rather
/// than in a particular widget, so menu, keyboard, and context-menu surfaces
/// all dispatch the same command.
#[derive(Default)]
struct CommandState {
    channel_clipboard: Option<ChannelClipboard>,
    history: History<ProjectSnapshot>,
    project_edit_pending: bool,
    pane: Pane,
}

/// The work-surface/lower-dock combination a `view.pane-*` shortcut
/// targets. `mixer-visible` and `editor-page` are independent Slint
/// properties (the step grid or mixer sits above an always-visible
/// Source/Notes/Playlist dock), so there is no single UI property that
/// says "which pane is current" -- this is tracked here instead of derived,
/// so Next/Prev cycles predictably even though Steps and the dock tabs are
/// simultaneously visible.
#[derive(Clone, Copy, Default, PartialEq)]
enum Pane {
    Steps,
    Mixer,
    #[default]
    Source,
    Notes,
    Playlist,
}

const PANE_CYCLE: [Pane; 5] = [
    Pane::Steps,
    Pane::Mixer,
    Pane::Source,
    Pane::Notes,
    Pane::Playlist,
];

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

fn cycle_pane(current: Pane, forward: bool) -> Pane {
    let position = PANE_CYCLE
        .iter()
        .position(|pane| *pane == current)
        .unwrap_or(0);
    let len = PANE_CYCLE.len();
    let next = if forward {
        (position + 1) % len
    } else {
        (position + len - 1) % len
    };
    PANE_CYCLE[next]
}

#[derive(Clone, Copy)]
enum HistoryMove {
    Record,
    Undo,
    Redo,
}

/// Structural channel edits are prepared by UI callbacks and installed by the
/// pump, which exclusively owns the engine handle. A complete project swap
/// keeps insertion/removal/reordering atomically visible to the audio thread.
struct ProjectEdit {
    project: Project,
    samples: Vec<Option<Arc<SampleData>>>,
    status: String,
    history: Option<(HistoryMove, HistoryEntry<ProjectSnapshot>)>,
}

enum DocumentResult {
    Cancelled,
    NewSong(Project),
    SavedSong {
        path: PathBuf,
        mode: AssetMode,
        revision: u64,
        report: SaveReport,
        sample_references: Vec<Option<SampleReference>>,
    },
    SavedOther {
        label: &'static str,
        report: SaveReport,
    },
    SavedPreset {
        label: &'static str,
        report: SaveReport,
    },
    SaveFailed {
        action: &'static str,
        error: String,
    },
    Loaded {
        path: PathBuf,
        target: LoadTarget,
        document: ResolvedDocument,
    },
    Exported {
        path: PathBuf,
    },
    Failed(String),
}

fn fresh_starter_seed() -> u64 {
    static SEQUENCE: AtomicU64 = AtomicU64::new(1);
    let clock = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    clock
        ^ SEQUENCE
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

fn copied_channel_name(project: &Project, source_name: &str) -> String {
    let base = if source_name.trim().is_empty() {
        "Channel".to_string()
    } else {
        format!("{source_name} copy")
    };
    if !project
        .channels
        .iter()
        .any(|channel| channel.setup.channel.name == base)
    {
        return base;
    }
    for suffix in 2..=MAX_CHANNELS {
        let candidate = format!("{base} {suffix}");
        if !project
            .channels
            .iter()
            .any(|channel| channel.setup.channel.name == candidate)
        {
            return candidate;
        }
    }
    base
}

fn snapshot_channel_clipboard(
    state: &UiState,
    window: &MainWindow,
    index: usize,
) -> Option<ChannelClipboard> {
    let project = state.project_snapshot(window.get_bpm(), window.get_swing_percent());
    Some(ChannelClipboard {
        channel: project.channels.get(index)?.clone(),
        sample: state.sample_snapshots().get(index)?.clone(),
    })
}

fn project_snapshot(state: &UiState, window: &MainWindow) -> ProjectSnapshot {
    ProjectSnapshot {
        project: state.project_snapshot(window.get_bpm(), window.get_swing_percent()),
        samples: state.sample_snapshots(),
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

fn record_project_history(
    commands: &Rc<RefCell<CommandState>>,
    before: ProjectSnapshot,
    state: &Rc<RefCell<UiState>>,
    window: &MainWindow,
    label: &'static str,
) {
    let after = project_snapshot(&state.borrow(), window);
    commands.borrow_mut().history.record(HistoryEntry {
        before,
        after,
        label,
    });
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
    channel.setup.channel.name = copied_channel_name(&project, &channel.setup.channel.name);
    let index = after + 1;
    project.channels.insert(index, channel);
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
    project.channels.remove(index);
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

/// Empties pattern `index`'s notes on every channel. The pattern itself,
/// its length, and any playlist placements referencing it are untouched --
/// it still exists and still plays, just silently.
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
    }
    queue_project_edit(tx, before, ProjectSnapshot { project, samples }, status)
}

#[derive(Clone, Copy)]
enum LoadTarget {
    Song,
    Kit,
    Channel,
    Generator,
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
        | EffectKind::Limiter
        | EffectKind::Plate => 1,
        EffectKind::Buffer => 1,
        EffectKind::Delay | EffectKind::Gate | EffectKind::Compressor => 2,
        EffectKind::Eq | EffectKind::Reverb | EffectKind::Modulation => 3,
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
fn effect_slot_row(slot: &EffectSlotState) -> EffectSlotRow {
    let kind = slot.kind();
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
        bypassed: slot.bypassed,
        p0: p[0],
        p1: p[1],
        p2: p[2],
        p3: p[3],
        p4: p[4],
        p5: p[5],
        p6: p[6],
        p7: p[7],
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
        _ => DeviceKind::Sampler,
    }
}

fn device_kind_to_int(kind: DeviceKind) -> i32 {
    match kind {
        DeviceKind::Sampler => 0,
        DeviceKind::DrumSynth => 1,
        DeviceKind::MonoSynth => 2,
        DeviceKind::PolySynth => 3,
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

/// Env-gated diagnostic logging (MOOLOOP_DEBUG=1).
fn dbg_log(msg: &str) {
    if std::env::var("MOOLOOP_DEBUG").is_ok() {
        eprintln!("mooloop: {msg}");
    }
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
    channels: Vec<ChannelState>,
    rows: Rc<VecModel<ChannelRow>>,
    step_models: Vec<Rc<VecModel<StepCell>>>,
    note_model: Rc<VecModel<NoteCell>>,
    automation_point_model: Rc<VecModel<AutomationPointCell>>,
    automation_target_model: Rc<VecModel<AutomationTargetRow>>,
    /// Destination shown in the piano roll's variable lane. `None` means the
    /// lane is open but empty-handed, which is the state a fresh project is
    /// in; it is not the same as the lane being hidden.
    /// A `Cell` so `refresh_automation` can run from the shared `&self`
    /// editor refresh: reconciling a destination whose device was removed is
    /// part of drawing the lane, not a separate edit.
    automation_target: Cell<Option<ParamAddr>>,
    /// Point last created or dragged. Drives the highlight and the header
    /// readout; a drag re-reads it by id, so reordering the model underneath
    /// an in-flight drag is harmless.
    automation_selected_point: Cell<Option<PointId>>,
    playlist_model: Rc<VecModel<PlaylistClip>>,
    waveform_model: Rc<VecModel<f32>>,
    /// Normalized position of every currently active sampler voice on the
    /// selected channel, refreshed each pump tick. Empty when idle, when a
    /// different device kind is selected, or while editing a bus.
    playhead_model: Rc<VecModel<f32>>,
    effect_slot_model: Rc<VecModel<EffectSlotRow>>,
    mixer_strip_model: Rc<VecModel<MixerStripRow>>,
    default_waveform: Vec<f32>,
    default_sample_description: String,
    default_sample_duration: f32,
    /// Mirror of the project's bus bank, master first. Always `MAX_BUSES`
    /// long, matching the engine's preallocated bank.
    buses: Vec<BusSetup>,
    pattern_lengths: Vec<usize>,
    pattern_names: Vec<String>,
    playlist: Vec<PatternPlacement>,
    song_mode: bool,
    current_pattern: usize,
    selected: usize,
    /// Which effect chain the device rack edits. Selecting a channel in the
    /// step grid points it at that channel; selecting a strip in the mixer
    /// points it at a bus. `selected` stays put either way, because the piano
    /// roll, the step grid, and the sampler all still mean a channel.
    effect_target: EffectTarget,
    selected_note_id: Option<NoteId>,
    /// The full multi-selection, driving highlight and bulk delete. Always a
    /// superset of `selected_note_id` when non-empty; the precision editor
    /// (`refresh_selected_note_controls`) only shows fields when this has
    /// settled on exactly one member, since it edits one note, not a group.
    selected_note_ids: HashSet<NoteId>,
    bundle_path: Option<PathBuf>,
    dirty: bool,
    revision: u64,
    source_revision: u64,
    generator_presets: Vec<PresetSummary>,
    channel_presets: Vec<PresetSummary>,
    pending_preset_save: Option<PresetSaveTarget>,
}

#[derive(Clone, Copy)]
enum PresetSaveTarget {
    Generator,
    Channel,
}

impl UiState {
    fn reset_channel_source(&mut self, index: usize, kind: DeviceKind) {
        let Some(channel) = self.channels.get_mut(index) else {
            return;
        };
        self.source_revision = self.source_revision.wrapping_add(1);
        channel.kind = kind;
        channel.name = match kind {
            DeviceKind::Sampler => format!("Sampler {}", index + 1),
            DeviceKind::DrumSynth => format!("Drum {}", index + 1),
            DeviceKind::MonoSynth => format!("Mono {}", index + 1),
            DeviceKind::PolySynth => format!("Poly {}", index + 1),
        };
        match kind {
            DeviceKind::Sampler => {
                channel.params = SamplerParams::default();
                channel.sample_name.clear();
                channel.sample_description.clear();
                channel.sample_duration = 0.0;
                channel.sample_path = None;
                channel.sample_embedded = false;
                channel.sample_data = None;
                channel.waveform.clear();
                channel.can_previous_sample = false;
                channel.can_next_sample = false;
            }
            DeviceKind::DrumSynth => {
                channel.drum_params = DrumSynthParams::default();
                channel.sample_name.clear();
                channel.sample_description.clear();
                channel.sample_duration = 0.0;
                channel.sample_path = None;
                channel.sample_embedded = false;
                channel.sample_data = None;
                channel.waveform.clear();
                channel.can_previous_sample = false;
                channel.can_next_sample = false;
            }
            DeviceKind::MonoSynth => {
                channel.mono_params = MonoSynthParams::default();
                channel.sample_name.clear();
                channel.sample_description.clear();
                channel.sample_duration = 0.0;
                channel.sample_path = None;
                channel.sample_embedded = false;
                channel.sample_data = None;
                channel.waveform.clear();
                channel.can_previous_sample = false;
                channel.can_next_sample = false;
            }
            DeviceKind::PolySynth => {
                channel.poly_params = PolySynthParams::default();
                channel.sample_name.clear();
                channel.sample_description.clear();
                channel.sample_duration = 0.0;
                channel.sample_path = None;
                channel.sample_embedded = false;
                channel.sample_data = None;
                channel.waveform.clear();
                channel.can_previous_sample = false;
                channel.can_next_sample = false;
            }
        }
    }

    fn project_snapshot(&self, bpm: i32, swing_percent: i32) -> Project {
        let channels = self
            .channels
            .iter()
            .map(|channel| {
                let source = match channel.kind {
                    DeviceKind::Sampler => {
                        let sample = channel
                            .sample_path
                            .as_ref()
                            .map(|path| SampleReference::File {
                                path: path.clone(),
                                embedded: channel.sample_embedded,
                            })
                            .unwrap_or_default();
                        ChannelSource::Sampler(SamplerState {
                            params: channel.params,
                            sample,
                        })
                    }
                    DeviceKind::DrumSynth => ChannelSource::DrumSynth(DrumSynthState {
                        params: channel.drum_params,
                    }),
                    DeviceKind::MonoSynth => ChannelSource::MonoSynth(MonoSynthState {
                        params: channel.mono_params,
                    }),
                    DeviceKind::PolySynth => ChannelSource::PolySynth(PolySynthState {
                        params: channel.poly_params,
                    }),
                };
                ProjectChannel {
                    setup: ChannelSetup {
                        channel: Channel {
                            name: channel.name.clone(),
                            kind: channel.kind,
                            muted: channel.muted,
                            volume: channel.volume,
                            pan: channel.pan,
                            bus: channel.bus,
                        },
                        source,
                        effects: channel.effects.clone(),
                        modulation: channel.modulation,
                    },
                    notes: channel.notes.clone(),
                    automation: channel.automation.clone(),
                    next_note_id: channel.next_note_id,
                }
            })
            .collect();
        Project {
            bpm: bpm.clamp(1, 999) as u16,
            swing_percent: swing_percent.clamp(MIN_SWING_PERCENT.into(), MAX_SWING_PERCENT.into())
                as u8,
            ppq: 96,
            beats_per_bar: 4,
            playback_mode: if self.song_mode {
                PlaybackMode::Song
            } else {
                PlaybackMode::Pattern
            },
            current_pattern: self.current_pattern as u16,
            selected_channel: self.selected as u8,
            channels,
            buses: self.buses.clone(),
            pattern_lengths: self
                .pattern_lengths
                .iter()
                .map(|length| *length as u16)
                .collect(),
            playlist: self.playlist.clone(),
        }
    }

    fn sample_snapshots(&self) -> Vec<Option<Arc<SampleData>>> {
        self.channels
            .iter()
            .map(|channel| {
                (channel.kind == DeviceKind::Sampler)
                    .then(|| channel.sample_data.clone())
                    .flatten()
            })
            .collect()
    }

    fn replace_project(
        &mut self,
        project: &Project,
        samples: &[Option<Arc<SampleData>>],
        window: &MainWindow,
    ) {
        self.source_revision = self.source_revision.wrapping_add(1);
        let channels = project
            .channels
            .iter()
            .enumerate()
            .map(|(index, project_channel)| {
                let setup = &project_channel.setup;
                let (sampler, drum_params, mono_params, poly_params) = match &setup.source {
                    ChannelSource::Sampler(sampler) => (
                        Some(sampler),
                        DrumSynthParams::default(),
                        MonoSynthParams::default(),
                        PolySynthParams::default(),
                    ),
                    ChannelSource::DrumSynth(drum) => (
                        None,
                        drum.params,
                        MonoSynthParams::default(),
                        PolySynthParams::default(),
                    ),
                    ChannelSource::MonoSynth(mono) => (
                        None,
                        DrumSynthParams::default(),
                        mono.params,
                        PolySynthParams::default(),
                    ),
                    ChannelSource::PolySynth(poly) => (
                        None,
                        DrumSynthParams::default(),
                        MonoSynthParams::default(),
                        poly.params,
                    ),
                };
                let sample = sampler
                    .is_some()
                    .then(|| samples.get(index).cloned().flatten())
                    .flatten();
                let (sample_path, embedded) = match sampler.map(|state| &state.sample) {
                    Some(SampleReference::File { path, embedded }) => {
                        (Some(path.clone()), *embedded)
                    }
                    Some(SampleReference::Builtin { .. } | SampleReference::Empty) | None => {
                        (None, false)
                    }
                };
                // Only a legacy `Builtin` reference (a project saved before
                // the sampler stopped auto-loading a kick) substitutes the
                // cached default sample; `Empty` means genuinely no sample.
                let is_builtin = matches!(
                    sampler.map(|state| &state.sample),
                    Some(SampleReference::Builtin { .. })
                );
                let missing = sample_path.is_some() && sample.is_none();
                let sample_name = if sampler.is_some() {
                    sample_path
                        .as_ref()
                        .and_then(|path| path.file_name())
                        .and_then(|name| name.to_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| {
                            if is_builtin {
                                "default kick".to_string()
                            } else {
                                String::new()
                            }
                        })
                } else {
                    String::new()
                };
                let waveform = sample
                    .as_ref()
                    .map(|sample| waveform_peaks(sample, WAVEFORM_BINS))
                    .unwrap_or_else(|| {
                        if is_builtin {
                            self.default_waveform.clone()
                        } else {
                            Vec::new()
                        }
                    });
                let description = sample
                    .as_ref()
                    .map(|sample| sample_description(sample))
                    .unwrap_or_else(|| {
                        if missing {
                            "Missing sample - load a WAV to relink".into()
                        } else if is_builtin {
                            self.default_sample_description.clone()
                        } else {
                            String::new()
                        }
                    });
                let duration = sample
                    .as_ref()
                    .map(|sample| sample_duration(sample))
                    .unwrap_or_else(|| {
                        if missing {
                            0.0
                        } else if is_builtin {
                            self.default_sample_duration
                        } else {
                            0.0
                        }
                    });
                let (can_previous, can_next) = sample_path
                    .as_ref()
                    .and_then(|path| wav_files_in_directory(path).ok().map(|files| (path, files)))
                    .map(|(path, files)| {
                        let index = sample_index(path, &files);
                        (
                            index.is_some_and(|index| index > 0),
                            index.is_some_and(|index| index + 1 < files.len()),
                        )
                    })
                    .unwrap_or((false, false));
                ChannelState {
                    name: setup.channel.name.clone(),
                    kind: setup.channel.kind,
                    muted: setup.channel.muted,
                    volume: setup.channel.volume,
                    pan: setup.channel.pan,
                    params: sampler.map(|state| state.params).unwrap_or_default(),
                    drum_params,
                    mono_params,
                    poly_params,
                    sample_name,
                    sample_description: description,
                    sample_duration: duration,
                    sample_path,
                    sample_embedded: embedded,
                    sample_data: sample,
                    waveform,
                    can_previous_sample: can_previous,
                    can_next_sample: can_next,
                    notes: project_channel.notes.clone(),
                    automation: project_channel.automation.clone(),
                    next_note_id: project_channel.next_note_id,
                    effects: setup.effects.clone(),
                    modulation: setup.modulation,
                    bus: setup.channel.bus,
                }
            })
            .collect::<Vec<_>>();

        self.buses = normalized_buses(&project.buses);
        self.pattern_lengths = project
            .pattern_lengths
            .iter()
            .map(|length| *length as usize)
            .collect();
        self.pattern_names = vec![String::new(); self.pattern_lengths.len()];
        self.playlist = project.playlist.clone();
        self.song_mode = project.playback_mode == PlaybackMode::Song;
        self.current_pattern = project.current_pattern as usize;
        self.selected = project.selected_channel as usize;
        // A load points the device rack back at a channel; the bus the
        // previous document had open means nothing in this one.
        self.effect_target = EffectTarget::Channel(project.selected_channel);
        self.selected_note_id = None;
        self.selected_note_ids.clear();
        self.channels = channels;
        self.step_models = self
            .channels
            .iter()
            .map(|channel| {
                Rc::new(VecModel::from(
                    (0..self.pattern_lengths[self.current_pattern])
                        .map(|step| rack_cell(&channel.notes[self.current_pattern], step))
                        .collect::<Vec<_>>(),
                ))
            })
            .collect();
        let rows: Vec<ChannelRow> = self
            .channels
            .iter()
            .enumerate()
            .map(|(index, channel)| ChannelRow {
                name: channel.name.as_str().into(),
                muted: channel.muted,
                volume_db: linear_to_db(channel.volume),
                pan: channel.pan,
                selected: index == self.selected,
                bus: channel.bus as i32,
                steps: ModelRc::from(self.step_models[index].clone()),
            })
            .collect();
        self.rows.set_vec(rows);
        window.set_bpm(project.bpm.into());
        window.set_swing_percent(project.swing_percent.into());
        window.set_song_mode(self.song_mode);
        window.set_current_pattern(self.current_pattern as i32);
        window.set_pattern_count(self.pattern_lengths.len() as i32);
        window.set_pattern_length(self.pattern_lengths[self.current_pattern] as i32);
        window.set_selected_channel(self.selected as i32);
        self.sync_row_flags();
        self.sync_mixer(window);
        self.sync_playlist(window);
        self.refresh_editor(window);
    }

    fn update_document_title(&self, window: &MainWindow) {
        let name = self
            .bundle_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .and_then(|name| name.to_str())
            .unwrap_or("Untitled");
        window.set_document_title(
            if self.dirty {
                format!("{name} * - mooloop")
            } else {
                format!("{name} - mooloop")
            }
            .into(),
        );
    }
    /// Push the selected/muted flags of every row to the rack model.
    fn sync_row_flags(&self) {
        for (i, ch) in self.channels.iter().enumerate() {
            if let Some(mut row) = self.rows.row_data(i) {
                row.selected = i == self.selected;
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
        let length = self.pattern_lengths[pattern];
        for (i, ch) in self.channels.iter().enumerate() {
            let cells: Vec<StepCell> = (0..length)
                .map(|step| rack_cell(&ch.notes[pattern], step))
                .collect();
            self.step_models[i].set_vec(cells);
        }
    }

    fn refresh_rack_cell(&self, channel: usize, step: usize) {
        let notes = &self.channels[channel].notes[self.current_pattern];
        self.step_models[channel].set_row_data(step, rack_cell(notes, step));
    }

    /// Push the current pattern's name and the full pattern menu to the
    /// window. An empty name falls back to `Pattern N` in the menu.
    fn sync_pattern_menu(&self, window: &MainWindow) {
        let options: Vec<slint::SharedString> = self
            .pattern_names
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
            .pattern_names
            .get(self.current_pattern)
            .cloned()
            .unwrap_or_default();
        window.set_current_pattern_name(current.into());
    }

    fn sync_generator_preset_menu(&self, window: &MainWindow) {
        let options: Vec<slint::SharedString> = self
            .generator_presets
            .iter()
            .map(preset_menu_label)
            .collect();
        window.set_generator_preset_options(ModelRc::from(Rc::new(VecModel::from(options))));
    }

    fn sync_channel_preset_menu(&self, window: &MainWindow) {
        let options: Vec<slint::SharedString> =
            self.channel_presets.iter().map(preset_menu_label).collect();
        window.set_channel_preset_options(ModelRc::from(Rc::new(VecModel::from(options))));
    }

    fn song_length_ticks(&self) -> u32 {
        let content_end = self
            .playlist
            .iter()
            .filter_map(|placement| {
                self.pattern_lengths
                    .get(placement.pattern as usize)
                    .map(|steps| {
                        placement
                            .start_tick
                            .saturating_add(*steps as u32 * TICKS_PER_STEP)
                    })
            })
            .max()
            .unwrap_or(TICKS_PER_BAR)
            .max(TICKS_PER_BAR);
        content_end.div_ceil(TICKS_PER_BAR) * TICKS_PER_BAR
    }

    fn sync_playlist(&self, window: &MainWindow) {
        let clips: Vec<PlaylistClip> = self
            .playlist
            .iter()
            .filter_map(|placement| {
                self.pattern_lengths
                    .get(placement.pattern as usize)
                    .map(|length| PlaylistClip {
                        pattern: placement.pattern as i32,
                        start_tick: placement.start_tick as i32,
                        length_steps: *length as i32,
                    })
            })
            .collect();
        self.playlist_model.set_vec(clips);
        let song_length = self.song_length_ticks();
        window.set_playlist_song_length_ticks(song_length as i32);
        window.set_playlist_bars(song_length.div_ceil(TICKS_PER_BAR).max(MAX_PLAYLIST_BARS) as i32);
    }

    fn placement_covering(&self, pattern: usize, tick: u32) -> Option<PatternPlacement> {
        self.playlist.iter().copied().find(|placement| {
            if placement.pattern as usize != pattern {
                return false;
            }
            let length = self.pattern_lengths[pattern] as u32 * TICKS_PER_STEP;
            tick >= placement.start_tick && tick < placement.start_tick.saturating_add(length)
        })
    }

    fn refresh_note_editor(&self, window: &MainWindow) {
        let Some(channel) = self.channels.get(self.selected) else {
            return;
        };
        let length_ticks = self.pattern_lengths[self.current_pattern] as u32 * TICKS_PER_STEP;
        let cells: Vec<NoteCell> = channel.notes[self.current_pattern]
            .iter()
            .copied()
            .filter(|note| note.start_tick < length_ticks)
            .map(|note| note_cell(note, &self.selected_note_ids))
            .collect();
        self.note_model.set_vec(cells);
        self.refresh_selected_note_controls(window);
    }

    /// Every destination the selected clip can address: the channel's own
    /// effect chain plus every bus's, because a clip's automation is allowed
    /// to reach the buses its channel feeds into.
    ///
    /// Generators are deliberately absent. They ship whole parameter structs
    /// rather than descriptor-addressed params, so there is nothing to name
    /// yet (`docs/plans/buffer-implementation/02-control-and-modulation.md`,
    /// build order step 2).
    fn automation_destinations(&self) -> Vec<(ParamAddr, String, &'static ParamDescriptor)> {
        let mut rows = Vec::new();
        let channel = EffectTarget::Channel(self.selected as u8);
        if let Some(state) = self.channels.get(self.selected) {
            for (slot, effect) in state.effects.iter().enumerate() {
                let kind = effect.kind();
                let device = format!("{} {}", kind.label(), slot + 1);
                for descriptor in kind.descriptors() {
                    rows.push((
                        ParamAddr::effect(channel, slot as u8, descriptor.id),
                        device.clone(),
                        descriptor,
                    ));
                }
            }
        }
        for (index, bus) in self.buses.iter().enumerate() {
            for (slot, effect) in bus.effects.iter().enumerate() {
                let kind = effect.kind();
                let device = format!("{} · {} {}", bus.bus.name, kind.label(), slot + 1);
                for descriptor in kind.descriptors() {
                    rows.push((
                        ParamAddr::effect(EffectTarget::Bus(index as u8), slot as u8, descriptor.id),
                        device.clone(),
                        descriptor,
                    ));
                }
            }
        }
        rows
    }

    fn automation_lanes(&self) -> Option<&Vec<AutomationLane>> {
        self.channels
            .get(self.selected)?
            .automation
            .get(self.current_pattern)
    }

    fn automation_lane(&self) -> Option<&AutomationLane> {
        let target = self.automation_target.get()?;
        self.automation_lanes()?
            .iter()
            .find(|lane| lane.target == target)
    }

    fn automation_lane_mut(&mut self) -> Option<&mut AutomationLane> {
        let target = self.automation_target.get()?;
        let pattern = self.current_pattern;
        self.channels
            .get_mut(self.selected)?
            .automation
            .get_mut(pattern)?
            .iter_mut()
            .find(|lane| lane.target == target)
    }

    /// Descriptor for the currently shown lane, used to turn normalized
    /// breakpoints back into the natural units the readout displays.
    fn automation_descriptor(&self) -> Option<&'static ParamDescriptor> {
        let target = self.automation_target.get()?;
        let ParamOwner::Effect { slot } = target.owner else {
            return None;
        };
        let effects = match target.scope {
            EffectTarget::Channel(channel) => &self.channels.get(channel as usize)?.effects,
            EffectTarget::Bus(bus) => &self.buses.get(bus as usize)?.effects,
        };
        effects
            .get(slot as usize)?
            .kind()
            .descriptor(target.param)
    }

    /// Rebuilds the lane picker, the drawn curve, and the header label.
    ///
    /// A destination whose device has since been removed leaves its lane in
    /// storage but drops it from the picker, and clears the visible lane. The
    /// alternative -- silently deleting the automation -- loses work when a
    /// device is removed and re-added.
    fn refresh_automation(&self, window: &MainWindow) {
        let destinations = self.automation_destinations();
        if self
            .automation_target
            .get()
            .is_some_and(|target| !destinations.iter().any(|(addr, _, _)| *addr == target))
        {
            self.automation_target.set(None);
        }
        let open: HashSet<ParamAddr> = self
            .automation_lanes()
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
                    current: self.automation_target.get() == Some(*address),
                }
            })
            .collect();
        self.automation_target_model.set_vec(rows);

        let label = self
            .automation_target
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
        let length_ticks = self.pattern_lengths[self.current_pattern] as u32 * TICKS_PER_STEP;
        let selected = self.automation_selected_point.get();
        let cells: Vec<AutomationPointCell> = self
            .automation_lane()
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
            .automation_selected_point
            .get()
            .and_then(|id| {
                let lane = self.automation_lane()?;
                let point = lane.points().iter().find(|point| point.id == id)?;
                let descriptor = self.automation_descriptor()?;
                Some(format_param_value(descriptor, point.value))
            })
            .unwrap_or_default();
        window.set_automation_value_text(readout.as_str().into());
    }

    fn refresh_selected_note_controls(&self, window: &MainWindow) {
        window.set_has_selected_note(false);
        // The precision editor shows one note's fields; once the selection
        // is a group (Shift-click, Select All) there is no single note left
        // to show them for.
        if self.selected_note_ids.len() > 1 {
            return;
        }
        let Some(id) = self.selected_note_id else {
            return;
        };
        let Some(note) = self.channels[self.selected].notes[self.current_pattern]
            .iter()
            .find(|note| note.id == id)
        else {
            return;
        };
        window.set_has_selected_note(true);
        window.set_selected_note_step(note.start_tick as i32);
        window.set_selected_note(note.note as i32);
        window.set_selected_velocity(note.velocity as i32);
        window.set_selected_duration_ticks(note.duration_ticks as i32);
    }

    /// Replaces the whole note selection with exactly one note (or clears it
    /// when `id` is `None`). Every single-note interaction -- rack step
    /// edits, a plain piano-roll click, create/move/resize/velocity -- goes
    /// through this, so a Shift-click or Select All selection never lingers
    /// once the user touches a single note through any other gesture.
    fn select_note(&mut self, id: Option<NoteId>) {
        self.selected_note_id = id;
        self.selected_note_ids.clear();
        self.selected_note_ids.extend(id);
    }

    /// Adds or removes one note from the selection (Shift/Ctrl-click).
    fn toggle_note_selection(&mut self, id: NoteId) {
        if !self.selected_note_ids.remove(&id) {
            self.selected_note_ids.insert(id);
        }
        self.selected_note_id = (self.selected_note_ids.len() == 1)
            .then(|| *self.selected_note_ids.iter().next().unwrap());
    }

    /// Selects every note in `channel`'s current pattern (Ctrl+A).
    fn select_all_notes(&mut self, channel: usize) {
        let pattern = self.current_pattern;
        let length_ticks = self.pattern_lengths[pattern] as u32 * TICKS_PER_STEP;
        self.selected_note_ids = self.channels[channel].notes[pattern]
            .iter()
            .filter(|note| note.start_tick < length_ticks)
            .map(|note| note.id)
            .collect();
        self.selected_note_id = (self.selected_note_ids.len() == 1)
            .then(|| *self.selected_note_ids.iter().next().unwrap());
    }

    /// Drops ids that no longer exist from the selection, e.g. after a batch
    /// removal elsewhere in the rack or piano roll.
    fn prune_note_selection(&mut self, removed: &[NoteId]) {
        self.selected_note_ids.retain(|id| !removed.contains(id));
        if self
            .selected_note_id
            .is_some_and(|id| removed.contains(&id))
        {
            self.selected_note_id = None;
        }
    }

    /// Recomputes every step cell for `channel`'s current pattern. Used
    /// after an edit (like a multi-note delete) that can touch notes spread
    /// across many steps, where refreshing one step at a time would miss
    /// the rest.
    fn refresh_rack_row(&self, channel: usize) {
        let notes = &self.channels[channel].notes[self.current_pattern];
        let cells: Vec<StepCell> = (0..self.pattern_lengths[self.current_pattern])
            .map(|step| rack_cell(notes, step))
            .collect();
        self.step_models[channel].set_vec(cells);
    }

    /// The chain the device rack is currently editing, channel or bus.
    fn effect_chain(&self) -> Option<&Vec<EffectSlotState>> {
        match self.effect_target {
            EffectTarget::Channel(index) => self.channels.get(index as usize).map(|c| &c.effects),
            EffectTarget::Bus(index) => self.buses.get(index as usize).map(|b| &b.effects),
        }
    }

    fn effect_chain_mut(&mut self) -> Option<&mut Vec<EffectSlotState>> {
        match self.effect_target {
            EffectTarget::Channel(index) => self
                .channels
                .get_mut(index as usize)
                .map(|c| &mut c.effects),
            EffectTarget::Bus(index) => self.buses.get_mut(index as usize).map(|b| &mut b.effects),
        }
    }

    /// Rebuild the edited chain's rows. The model itself is installed on the
    /// window once; this refreshes its contents after structural changes
    /// (add/remove/reorder) and after the rack is pointed somewhere else.
    fn sync_effects(&self) {
        let rows: Vec<EffectSlotRow> = self
            .effect_chain()
            .map(|effects| effects.iter().map(effect_slot_row).collect())
            .unwrap_or_default();
        self.effect_slot_model.set_vec(rows);
    }

    /// Sequencer channels feeding `bus` directly. Buses routed into it are not
    /// counted: the number answers "what lands here", not "what reaches here".
    fn bus_feed_count(&self, bus: usize) -> usize {
        self.channels
            .iter()
            .filter(|channel| channel.bus as usize == bus)
            .count()
    }

    /// Rebuild every mixer strip and the shared name list. Called after a load
    /// or any change that moves channels between buses.
    fn sync_mixer(&self, window: &MainWindow) {
        let names: Vec<slint::SharedString> = self
            .buses
            .iter()
            .map(|setup| setup.bus.name.as_str().into())
            .collect();
        window.set_bus_names(ModelRc::from(Rc::new(VecModel::from(names))));
        let strips: Vec<MixerStripRow> = self
            .buses
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
        let flags: Vec<bool> = (0..self.buses.len())
            .map(|candidate| {
                candidate != bus && !would_create_cycle(&self.buses, bus as u8, candidate as u8)
            })
            .collect();
        ModelRc::from(Rc::new(VecModel::from(flags)))
    }

    fn mixer_strip_row(&self, index: usize, setup: &BusSetup) -> MixerStripRow {
        MixerStripRow {
            name: setup.bus.name.as_str().into(),
            muted: setup.bus.muted,
            volume: setup.bus.volume,
            pan: setup.bus.pan,
            output: setup.bus.output as i32,
            selected: self.effect_target == EffectTarget::Bus(index as u8),
            is_master: index == MASTER_BUS as usize,
            feed_count: self.bus_feed_count(index) as i32,
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
        let Some(setup) = self.buses.get(index) else {
            return;
        };
        self.mixer_strip_model
            .set_row_data(index, self.mixer_strip_row(index, setup));
    }

    /// Push the selection flag to every strip, so exactly one reads selected.
    fn sync_mixer_selection(&self) {
        for index in 0..self.mixer_strip_model.row_count() {
            if let Some(mut row) = self.mixer_strip_model.row_data(index) {
                row.selected = self.effect_target == EffectTarget::Bus(index as u8);
                self.mixer_strip_model.set_row_data(index, row);
            }
        }
    }

    /// Mirror the edited bus onto the device rack's head face. When a channel
    /// is being edited this only clears the flag; the source face takes over.
    fn sync_bus_editor(&self, window: &MainWindow) {
        let EffectTarget::Bus(index) = self.effect_target else {
            window.set_editing_bus(false);
            return;
        };
        let index = index as usize;
        let Some(setup) = self.buses.get(index) else {
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
        window.set_editing_bus_feed_count(self.bus_feed_count(index) as i32);
        window.set_editing_bus_allowed(self.allowed_destinations(index));
    }

    /// Refresh the bottom editor's properties from `selected`.
    fn refresh_editor(&self, window: &MainWindow) {
        let Some(ch) = self.channels.get(self.selected) else {
            return;
        };
        let p = &ch.params;
        let drum = ch.drum_params;
        let mono = ch.mono_params;
        window.set_selected_channel_name(ch.name.as_str().into());
        window.set_selected_channel_volume_db(linear_to_db(ch.volume));
        window.set_source_kind(device_kind_to_int(ch.kind));
        self.sync_effects();
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
            ch.sample_data
                .as_ref()
                .map(|sample| sample.frames.len() as i32)
                .unwrap_or(0),
        );
        self.waveform_model.set_vec(ch.waveform.clone());
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
        let playhead_model = Rc::new(VecModel::from(Vec::<f32>::new()));
        let effect_slot_model = Rc::new(VecModel::from(Vec::<EffectSlotRow>::new()));
        let mixer_strip_model = Rc::new(VecModel::from(Vec::<MixerStripRow>::new()));
        window.set_channels(ModelRc::from(rows_model.clone()));
        window.set_notes(ModelRc::from(note_model.clone()));
        window.set_automation_points(ModelRc::from(automation_point_model.clone()));
        window.set_automation_targets(ModelRc::from(automation_target_model.clone()));
        window.set_playlist_clips(ModelRc::from(playlist_model.clone()));
        window.set_waveform(ModelRc::from(waveform_model.clone()));
        window.set_playhead_positions(ModelRc::from(playhead_model.clone()));
        window.set_effect_slots(ModelRc::from(effect_slot_model.clone()));
        window.set_mixer_strips(ModelRc::from(mixer_strip_model.clone()));
        window.set_pattern_count(1);

        let state = Rc::new(RefCell::new(UiState {
            channels: vec![first],
            rows: rows_model,
            step_models: vec![step_model],
            note_model,
            playlist_model,
            waveform_model,
            playhead_model,
            effect_slot_model,
            mixer_strip_model,
            default_waveform,
            default_sample_description,
            default_sample_duration,
            buses: default_buses(),
            pattern_lengths: vec![DEFAULT_STEPS as usize],
            pattern_names: vec![String::new()],
            playlist: Vec::with_capacity(MAX_PLAYLIST_PLACEMENTS),
            song_mode: false,
            current_pattern: 0,
            selected: 0,
            effect_target: EffectTarget::Channel(0),
            selected_note_id: None,
            selected_note_ids: HashSet::new(),
            automation_point_model,
            automation_target_model,
            automation_target: Cell::new(None),
            automation_selected_point: Cell::new(None),
            bundle_path: None,
            dirty: false,
            revision: 0,
            source_revision: 0,
            generator_presets: Vec::new(),
            channel_presets: Vec::new(),
            pending_preset_save: None,
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
                let dirty = st.borrow().dirty;
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
                let dirty = st.borrow().dirty;
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
                let dirty = st.borrow().dirty;
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
                        .unwrap_or_else(DocumentResult::Failed);
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
                let project = st
                    .borrow()
                    .project_snapshot(window.get_bpm(), window.get_swing_percent());
                let revision = st.borrow().revision;
                let mode = if window.get_embed_assets() {
                    AssetMode::Embedded
                } else {
                    AssetMode::Referenced
                };
                let current = (!save_as)
                    .then(|| st.borrow().bundle_path.clone())
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
                    let result = mooloop_project::save_song(&path, &project, mode)
                        .and_then(|report| {
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
                                    .map(|channel| match channel.setup.source {
                                        ChannelSource::Sampler(sampler) => Some(sampler.sample),
                                        ChannelSource::DrumSynth(_)
                                        | ChannelSource::MonoSynth(_)
                                        | ChannelSource::PolySynth(_) => None,
                                    })
                                    .collect(),
                            })
                        })
                        .unwrap_or_else(|error| DocumentResult::SaveFailed {
                            action: "song",
                            error: error.to_string(),
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
                    .project_snapshot(window.get_bpm(), window.get_swing_percent());
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
                        .unwrap_or_else(|error| DocumentResult::SaveFailed {
                            action: "kit",
                            error: error.to_string(),
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
                    .project_snapshot(window.get_bpm(), window.get_swing_percent());
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
                        .unwrap_or_else(|error| DocumentResult::SaveFailed {
                            action: "channel",
                            error: error.to_string(),
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
                        .unwrap_or_else(DocumentResult::Failed);
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
                        &st.generator_presets
                    } else {
                        &st.channel_presets
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
                        .unwrap_or_else(DocumentResult::Failed);
                    let _ = tx.send(result);
                });
            };
            if generator {
                window.on_generator_preset_selected(callback);
            } else {
                window.on_channel_preset_selected(callback);
            }
        }

        // --- Presets: open the save dialog, scoped to generator or channel ---
        for (generator, title) in [
            (true, "Save Generator Preset"),
            (false, "Save Channel Preset"),
        ] {
            let st = state.clone();
            let weak = window.as_weak();
            let callback = move || {
                st.borrow_mut().pending_preset_save = Some(if generator {
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
                st.borrow_mut().pending_preset_save = None;
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
                let Some(target) = st.borrow_mut().pending_preset_save.take() else {
                    return;
                };
                let snapshot = st
                    .borrow()
                    .project_snapshot(window.get_bpm(), window.get_swing_percent());
                let selected = snapshot.channels[snapshot.selected_channel as usize]
                    .setup
                    .clone();
                let info = PresetInfo {
                    name: name.clone(),
                    category: category.trim().to_string(),
                    tags: Vec::new(),
                };
                let file_stem = mooloop_project::sanitize_preset_name(&name);
                let (dir, extension, label) = match target {
                    PresetSaveTarget::Generator => (
                        settings::generator_presets_dir(selected.kind()),
                        "mooloop-generator",
                        "Generator preset saved",
                    ),
                    PresetSaveTarget::Channel => (
                        settings::channel_presets_dir(),
                        "mooloop-channel",
                        "Channel preset saved",
                    ),
                };
                let path = dir.join(format!("{file_stem}.{extension}"));
                window.set_document_busy(true);
                window.set_status_message("Saving preset...".into());
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let result = match target {
                        PresetSaveTarget::Generator => mooloop_project::save_generator_preset(
                            &path,
                            &selected.source,
                            info,
                            AssetMode::Embedded,
                        ),
                        PresetSaveTarget::Channel => mooloop_project::save_channel_preset(
                            &path,
                            &selected,
                            info,
                            AssetMode::Embedded,
                        ),
                    };
                    let result = result
                        .map(|report| DocumentResult::SavedPreset { label, report })
                        .unwrap_or_else(|error| DocumentResult::SaveFailed {
                            action: "preset",
                            error: error.to_string(),
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
                let project = st
                    .borrow()
                    .project_snapshot(window.get_bpm(), window.get_swing_percent());
                let samples = st.borrow().sample_snapshots();
                let scope = if st.borrow().song_mode {
                    RenderScope::Song
                } else {
                    RenderScope::Pattern {
                        index: st.borrow().current_pattern,
                    }
                };
                let format = match format {
                    1 => ExportFormat::Wav(WavEncoding::Float32),
                    2 => ExportFormat::Mp3(match bitrate {
                        0 => Mp3Bitrate::Kbps192,
                        1 => Mp3Bitrate::Kbps256,
                        _ => Mp3Bitrate::Kbps320,
                    }),
                    _ => ExportFormat::Wav(WavEncoding::Pcm24),
                };
                window.set_export_open(false);
                window.set_document_busy(true);
                window.set_status_message("Rendering audio...".into());
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let extension = if matches!(format, ExportFormat::Mp3(_)) {
                        "mp3"
                    } else {
                        "wav"
                    };
                    let Some(path) = pick_export_via_zenity(extension) else {
                        let _ = tx.send(DocumentResult::Cancelled);
                        return;
                    };
                    let spec = ExportSpec {
                        path: path.clone(),
                        scope,
                        tail_seconds: tail as f32,
                        format,
                    };
                    let result =
                        OfflineRenderer::render(&project, &samples, export_sample_rate, &spec)
                            .map(|_| DocumentResult::Exported { path })
                            .unwrap_or_else(|error| DocumentResult::Failed(error.to_string()));
                    let _ = tx.send(result);
                });
            });
        }

        {
            let st = state.clone();
            window.window().on_close_requested(move || {
                if st.borrow().dirty && !confirm_via_zenity("Quit without saving this song?") {
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
        let structural_tx = StructuralCommandSender(pending_tx);
        let sample_rate = handle.sample_rate();
        let (reverb_build_tx, reverb_build_rx) = std::sync::mpsc::channel::<ReverbBuildRequest>();
        let reverb_structural_tx = structural_tx.clone();
        std::thread::spawn(move || {
            const REGEN_SETTLE: std::time::Duration = std::time::Duration::from_millis(80);
            while let Ok(first) = reverb_build_rx.recv() {
                let mut pending = vec![first];
                loop {
                    match reverb_build_rx.recv_timeout(REGEN_SETTLE) {
                        Ok(request) => {
                            if let Some(existing) = pending.iter_mut().find(|existing| {
                                existing.target == request.target && existing.slot == request.slot
                            }) {
                                // Preserve the first key: it is the IR that
                                // actually owns the live slot until this
                                // coalesced replacement lands.
                                existing.params = request.params;
                            } else {
                                pending.push(request);
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
                for request in pending {
                    let params = EffectParams::Reverb(request.params);
                    let node = build_effect(params, sample_rate);
                    let align = DryAlign::new(node.dry_path_latency_frames()).map(Box::new);
                    if !reverb_structural_tx.send(StructuralCommand::ReplaceEffect {
                        target: request.target,
                        slot: request.slot,
                        expected_kind: EffectKind::Reverb,
                        expected_resource_key: request.expected_resource_key,
                        resource_key: request.params.fingerprint(),
                        node,
                        align,
                    }) {
                        return;
                    }
                }
            }
        });
        // Sample slots are published out-of-band, so source replacement asks
        // the pump (which owns the EngineHandle) to restore the built-in sample.
        let (sample_reset_tx, sample_reset_rx) = std::sync::mpsc::channel::<usize>();

        // --- Preferences: appearance applies live from here; audio reaches
        //     the engine through the pump below, the only place that owns
        //     `EngineHandle`. ---
        let ui_settings = Rc::new(RefCell::new(UiSettings::load_or_default()));
        // Kept alive here for as long as the app runs so the window survives
        // after this constructor returns; re-opening while it is already up
        // just refocuses it instead of spawning a second one.
        let mockup_window: Rc<RefCell<Option<MockupCanvas>>> = Rc::new(RefCell::new(None));
        {
            let settings = ui_settings.borrow();
            apply_theme(&window, settings.appearance.palette());
            sync_preferences_properties(&window, &settings);
            audio_tx.send(AudioAction::ApplyPersisted(settings.audio.engine_config()));
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
                    "edit.cut-channel" => window.invoke_edit_command_requested(2, channel),
                    "edit.copy-channel" => window.invoke_edit_command_requested(3, channel),
                    "edit.paste-channel" => window.invoke_edit_command_requested(4, channel),
                    "channel.clone" => window.invoke_edit_command_requested(5, channel),
                    "channel.remove" => window.invoke_edit_command_requested(6, channel),
                    "channel.add" => window.invoke_add_channel_clicked(0),
                    "pattern.add" => window.invoke_add_pattern_clicked(),
                    "pattern.clone" => window.invoke_pattern_clone_requested(),
                    "pattern.remove" => window.invoke_pattern_remove_requested(),
                    "pattern.clear" => window.invoke_pattern_clear_requested(),
                    "edit.select-all" => window.invoke_select_all_requested(),
                    "edit.delete-note" => window.invoke_delete_selected_notes_requested(),
                    "view.zoom-in" => window.invoke_zoom_in_requested(),
                    "view.zoom-out" => window.invoke_zoom_out_requested(),
                    "view.pane-steps" => {
                        commands.borrow_mut().pane = Pane::Steps;
                        apply_pane(&window, Pane::Steps);
                    }
                    "view.pane-mixer" => {
                        commands.borrow_mut().pane = Pane::Mixer;
                        apply_pane(&window, Pane::Mixer);
                    }
                    "view.pane-source" => {
                        commands.borrow_mut().pane = Pane::Source;
                        apply_pane(&window, Pane::Source);
                    }
                    "view.pane-notes" => {
                        commands.borrow_mut().pane = Pane::Notes;
                        apply_pane(&window, Pane::Notes);
                    }
                    "view.pane-playlist" => {
                        commands.borrow_mut().pane = Pane::Playlist;
                        apply_pane(&window, Pane::Playlist);
                    }
                    "view.pane-next" => {
                        let pane = cycle_pane(commands.borrow().pane, true);
                        commands.borrow_mut().pane = pane;
                        apply_pane(&window, pane);
                    }
                    "view.pane-prev" => {
                        let pane = cycle_pane(commands.borrow().pane, false);
                        commands.borrow_mut().pane = pane;
                        apply_pane(&window, pane);
                    }
                    _ => return false,
                }
                true
            });
        }
        sync_shortcut_rows(&window, &shortcut_table.borrow());
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
        {
            let mockup_window = mockup_window.clone();
            window.on_preferences_open_mockup_tool(move || {
                if let Some(existing) = mockup_window.borrow().as_ref() {
                    let _ = existing.show();
                    return;
                }
                match open_mockup_window() {
                    Ok(canvas) => *mockup_window.borrow_mut() = Some(canvas),
                    Err(error) => eprintln!("Could not open UI mockup tool: {error}"),
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
                apply_theme(&window, settings.appearance.palette());
                sync_preferences_properties(&window, &settings);
                tx.send(AudioAction::RefreshTargets);
            });
        }
        {
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_preferences_appearance_preview(move |preset, accent| {
                let Some(window) = weak.upgrade() else { return };
                let smooth_curves = settings.borrow().appearance.smooth_curves;
                match AppearanceSettings::validated(
                    AppearancePreset::from_index(preset),
                    accent.as_str(),
                    smooth_curves,
                ) {
                    Ok(appearance) => {
                        apply_theme(&window, appearance.palette());
                        window.set_preferences_error("".into());
                    }
                    Err(error) => window.set_preferences_error(error.to_string().into()),
                }
            });
        }
        {
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_preferences_save(move |preset, accent, developer_mode, smooth_curves| {
                let Some(window) = weak.upgrade() else {
                    return false;
                };
                let appearance = match AppearanceSettings::validated(
                    AppearancePreset::from_index(preset),
                    accent.as_str(),
                    smooth_curves,
                ) {
                    Ok(appearance) => appearance,
                    Err(error) => {
                        window.set_preferences_error(error.to_string().into());
                        return false;
                    }
                };
                apply_theme(&window, appearance.palette());
                window
                    .global::<DisplayPrefs>()
                    .set_smooth_curves(smooth_curves);
                let mut settings = settings.borrow_mut();
                let previous = settings.appearance.clone();
                settings.appearance = appearance;
                let previous_developer_mode = settings.general.developer_mode;
                settings.general.developer_mode = developer_mode;
                if let Err(error) = settings.save() {
                    settings.appearance = previous;
                    settings.general.developer_mode = previous_developer_mode;
                    window
                        .set_preferences_error(format!("Could not save settings: {error}").into());
                    return false;
                }
                sync_preferences_properties(&window, &settings);
                true
            });
        }
        {
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_preferences_cancelled(move || {
                let Some(window) = weak.upgrade() else { return };
                let settings = settings.borrow();
                apply_theme(&window, settings.appearance.palette());
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
                dbg_log("UI: play clicked, queuing Play");
                let _ = tx.send(EngineCommand::Play);
            });
        }
        {
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_stop_clicked(move || {
                dbg_log("UI: stop clicked, queuing Stop");
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
                st.borrow_mut().song_mode = song_mode;
                if let Some(window) = weak.upgrade() {
                    window.set_song_mode(song_mode);
                }
                let mode = if song_mode {
                    PlaybackMode::Song
                } else {
                    PlaybackMode::Pattern
                };
                let _ = tx.send(EngineCommand::SetPlaybackMode(mode));
            });
        }
        {
            let tx = cmd_tx.clone();
            window.on_bpm_changed(move |bpm| {
                let bpm = bpm as f64;
                // Preserve stream order: the transport adopts the tempo
                // first, then the pump allocates and publishes replacements.
                let _ = tx.send(EngineCommand::SetTempo(bpm));
                let _ = tx.resize_buffers(bpm);
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_swing_changed(move |percent| {
                let percent =
                    percent.clamp(MIN_SWING_PERCENT.into(), MAX_SWING_PERCENT.into()) as u8;
                let _ = tx.send(EngineCommand::SetSwing(percent));
                let mut st = st.borrow_mut();
                st.dirty = true;
                st.revision = st.revision.wrapping_add(1);
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
                dbg_log(if playing {
                    "UI: toggle-play -> Pause"
                } else {
                    "UI: toggle-play -> Play"
                });
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
                let count = st.borrow().pattern_lengths.len();
                if p < 0 || p as usize >= count {
                    return;
                }
                let p = p as usize;
                dbg_log(&format!("UI: pattern {p} selected"));
                {
                    let mut st = st.borrow_mut();
                    st.current_pattern = p;
                    st.select_note(None);
                    st.show_pattern(p);
                }
                if let Some(w) = weak.upgrade() {
                    w.set_current_pattern(p as i32);
                    let st = st.borrow();
                    w.set_pattern_length(st.pattern_lengths[p] as i32);
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
            let weak = window.as_weak();
            window.on_add_pattern_clicked(move || {
                let mut st = st.borrow_mut();
                if st.pattern_lengths.len() >= MAX_PATTERNS {
                    return;
                }
                let pattern = st.pattern_lengths.len();
                st.pattern_lengths.push(DEFAULT_STEPS as usize);
                st.pattern_names.push(String::new());
                for channel in &mut st.channels {
                    channel.notes.push(Vec::new());
                    channel.automation.push(Vec::new());
                }
                st.current_pattern = pattern;
                st.select_note(None);
                st.show_pattern(pattern);
                if let Some(window) = weak.upgrade() {
                    window.set_pattern_count(st.pattern_lengths.len() as i32);
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
                let index = index as usize;
                let mut st = st.borrow_mut();
                if index >= st.pattern_names.len() {
                    return;
                }
                st.pattern_names[index] = name.trim().to_string();
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
                let length = length.clamp(1, MAX_PATTERN_STEPS as i32) as usize;
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                if st.pattern_lengths[pattern] == length {
                    return;
                }
                st.pattern_lengths[pattern] = length;
                let length_ticks = length as u32 * TICKS_PER_STEP;
                let notes = &st.channels[st.selected].notes[pattern];
                let out_of_range: Vec<NoteId> = st
                    .selected_note_ids
                    .iter()
                    .copied()
                    .filter(|id| {
                        notes
                            .iter()
                            .find(|note| note.id == *id)
                            .is_none_or(|note| note.start_tick >= length_ticks)
                    })
                    .collect();
                st.prune_note_selection(&out_of_range);
                st.show_pattern(pattern);
                if let Some(w) = weak.upgrade() {
                    w.set_pattern_length(length as i32);
                    st.refresh_note_editor(&w);
                    st.sync_playlist(&w);
                }
                let _ = tx.send(EngineCommand::SetPatternLength {
                    pattern: pattern as u8,
                    length_steps: length as u16,
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
                if pattern < 0 || pattern as usize >= st.pattern_lengths.len() {
                    return;
                }
                let pattern = pattern as usize;
                let start_tick = start_tick.max(0) as u32;
                if start_tick >= MAX_PLAYLIST_TICKS || st.playlist.len() >= MAX_PLAYLIST_PLACEMENTS
                {
                    return;
                }
                let end_tick =
                    start_tick.saturating_add(st.pattern_lengths[pattern] as u32 * TICKS_PER_STEP);
                let overlaps = st.playlist.iter().any(|placement| {
                    if placement.pattern as usize != pattern {
                        return false;
                    }
                    let existing_end = placement
                        .start_tick
                        .saturating_add(st.pattern_lengths[pattern] as u32 * TICKS_PER_STEP);
                    start_tick < existing_end && placement.start_tick < end_tick
                });
                if overlaps {
                    return;
                }
                let placement = PatternPlacement::new(pattern as u8, start_tick);
                st.playlist.push(placement);
                st.playlist.sort_unstable();
                if let Some(window) = weak.upgrade() {
                    st.sync_playlist(&window);
                }
                let _ = tx.send(EngineCommand::SetPlaylistPlacement {
                    pattern: pattern as u8,
                    start_tick,
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
                if pattern < 0 || pattern as usize >= st.pattern_lengths.len() {
                    return;
                }
                let pattern = pattern as usize;
                let tick = tick.max(0) as u32;
                let Some(placement) = st.placement_covering(pattern, tick) else {
                    return;
                };
                let Some(index) = st.playlist.iter().position(|item| *item == placement) else {
                    return;
                };
                st.playlist.remove(index);
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
                let (channel, step) = (channel as usize, step as usize);
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                if channel >= st.channels.len() || step >= st.pattern_lengths[pattern] {
                    return;
                }
                let start = step as u32 * TICKS_PER_STEP;
                let end = start + TICKS_PER_STEP;
                let ids: Vec<NoteId> = st.channels[channel].notes[pattern]
                    .iter()
                    .filter(|note| note.start_tick >= start && note.start_tick < end)
                    .map(|note| note.id)
                    .collect();
                if ids.is_empty() {
                    let note = st.channels[channel].create_note(
                        pattern,
                        start,
                        DEFAULT_NOTE_DURATION_TICKS,
                        60,
                    );
                    if channel == st.selected {
                        st.select_note(Some(note.id));
                    }
                    let _ = tx.send(EngineCommand::UpsertNote {
                        pattern: pattern as u8,
                        channel: channel as u8,
                        note,
                    });
                } else {
                    st.channels[channel].notes[pattern].retain(|note| !ids.contains(&note.id));
                    st.prune_note_selection(&ids);
                    for id in ids {
                        let _ = tx.send(EngineCommand::RemoveNote {
                            pattern: pattern as u8,
                            channel: channel as u8,
                            id,
                        });
                    }
                }
                st.refresh_rack_cell(channel, step);
                if channel == st.selected {
                    if let Some(window) = weak.upgrade() {
                        st.refresh_note_editor(&window);
                    }
                }
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_step_removed(move |channel, step| {
                let (channel, step) = (channel as usize, step as usize);
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                if channel >= st.channels.len() || step >= st.pattern_lengths[pattern] {
                    return;
                }
                let start = step as u32 * TICKS_PER_STEP;
                let end = start + TICKS_PER_STEP;
                let ids: Vec<NoteId> = st.channels[channel].notes[pattern]
                    .iter()
                    .filter(|note| note.start_tick >= start && note.start_tick < end)
                    .map(|note| note.id)
                    .collect();
                st.channels[channel].notes[pattern].retain(|note| !ids.contains(&note.id));
                st.prune_note_selection(&ids);
                for id in ids {
                    let _ = tx.send(EngineCommand::RemoveNote {
                        pattern: pattern as u8,
                        channel: channel as u8,
                        id,
                    });
                }
                st.refresh_rack_cell(channel, step);
                if channel == st.selected {
                    if let Some(window) = weak.upgrade() {
                        st.refresh_note_editor(&window);
                    }
                }
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_step_velocity_edited(move |channel, step, value| {
                let (channel, step) = (channel as usize, step as usize);
                let velocity = (1.0 + value.clamp(0.0, 1.0) * 126.0).round() as u8;
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                if channel >= st.channels.len() || step >= st.pattern_lengths[pattern] {
                    return;
                }
                let start = step as u32 * TICKS_PER_STEP;
                let end = start + TICKS_PER_STEP;
                let mut edited: Vec<NoteEvent> = st.channels[channel].notes[pattern]
                    .iter_mut()
                    .filter(|note| note.start_tick >= start && note.start_tick < end)
                    .map(|note| {
                        note.velocity = velocity;
                        *note
                    })
                    .collect();
                if edited.is_empty() {
                    let mut note = st.channels[channel].create_note(
                        pattern,
                        start,
                        DEFAULT_NOTE_DURATION_TICKS,
                        60,
                    );
                    note.velocity = velocity;
                    *st.channels[channel].notes[pattern]
                        .iter_mut()
                        .find(|stored| stored.id == note.id)
                        .unwrap() = note;
                    edited.push(note);
                }
                let primary = (channel == st.selected).then_some(edited[0].id);
                st.select_note(primary);
                st.refresh_rack_cell(channel, step);
                if channel == st.selected {
                    if let Some(window) = weak.upgrade() {
                        st.refresh_note_editor(&window);
                    }
                }
                for note in edited {
                    let _ = tx.send(EngineCommand::UpsertNote {
                        pattern: pattern as u8,
                        channel: channel as u8,
                        note,
                    });
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
                let (channel, step) = (channel as usize, step as usize);
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                if channel >= st.channels.len() || step >= st.pattern_lengths[pattern] {
                    return;
                }
                let start = step as u32 * TICKS_PER_STEP;
                let end = start + TICKS_PER_STEP;
                let ids: Vec<NoteId> = st.channels[channel].notes[pattern]
                    .iter()
                    .filter(|note| note.start_tick >= start && note.start_tick < end)
                    .map(|note| note.id)
                    .collect();
                if on {
                    if !ids.is_empty() {
                        return;
                    }
                    let note = st.channels[channel].create_note(
                        pattern,
                        start,
                        DEFAULT_NOTE_DURATION_TICKS,
                        60,
                    );
                    if channel == st.selected {
                        st.select_note(Some(note.id));
                    }
                    let _ = tx.send(EngineCommand::UpsertNote {
                        pattern: pattern as u8,
                        channel: channel as u8,
                        note,
                    });
                } else {
                    if ids.is_empty() {
                        return;
                    }
                    st.channels[channel].notes[pattern].retain(|note| !ids.contains(&note.id));
                    st.prune_note_selection(&ids);
                    for id in ids {
                        let _ = tx.send(EngineCommand::RemoveNote {
                            pattern: pattern as u8,
                            channel: channel as u8,
                            id,
                        });
                    }
                }
                st.refresh_rack_cell(channel, step);
                if channel == st.selected {
                    if let Some(window) = weak.upgrade() {
                        st.refresh_note_editor(&window);
                    }
                }
            });
        }

        // Slice a sixteenth into `divisions` evenly spaced notes.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_step_sliced(move |channel, step, divisions| {
                let (channel, step) = (channel as usize, step as usize);
                let divisions = divisions.clamp(2, 4) as u32;
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                if channel >= st.channels.len() || step >= st.pattern_lengths[pattern] {
                    return;
                }
                let start = step as u32 * TICKS_PER_STEP;
                let end = start + TICKS_PER_STEP;
                let ids: Vec<NoteId> = st.channels[channel].notes[pattern]
                    .iter()
                    .filter(|note| note.start_tick >= start && note.start_tick < end)
                    .map(|note| note.id)
                    .collect();
                st.channels[channel].notes[pattern].retain(|note| !ids.contains(&note.id));
                st.prune_note_selection(&ids);
                for id in ids {
                    let _ = tx.send(EngineCommand::RemoveNote {
                        pattern: pattern as u8,
                        channel: channel as u8,
                        id,
                    });
                }
                let slice_ticks = TICKS_PER_STEP / divisions;
                for k in 0..divisions {
                    let note = st.channels[channel].create_note(
                        pattern,
                        start + k * slice_ticks,
                        slice_ticks,
                        60,
                    );
                    let _ = tx.send(EngineCommand::UpsertNote {
                        pattern: pattern as u8,
                        channel: channel as u8,
                        note,
                    });
                }
                st.refresh_rack_cell(channel, step);
                if channel == st.selected {
                    if let Some(window) = weak.upgrade() {
                        st.refresh_note_editor(&window);
                    }
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
                let (channel, step) = (channel as usize, step as usize);
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                if channel >= st.channels.len() || step >= st.pattern_lengths[pattern] {
                    return;
                }
                let pattern_length = st.pattern_lengths[pattern];
                let max_length = (pattern_length - step) as i32;
                let length_in_steps = length_in_steps.clamp(1, max_length) as u32;
                let duration_ticks = length_in_steps * TICKS_PER_STEP;
                let start = step as u32 * TICKS_PER_STEP;
                let end = start + TICKS_PER_STEP;
                let mut edited = Vec::new();
                let mut max_end_step = step;
                for note in st.channels[channel].notes[pattern].iter_mut() {
                    if note.start_tick < start || note.start_tick >= end {
                        continue;
                    }
                    let old_end_step = ((note.start_tick + note.duration_ticks.max(1) - 1)
                        / TICKS_PER_STEP) as usize;
                    max_end_step = max_end_step.max(old_end_step);
                    if note.duration_ticks == duration_ticks {
                        continue;
                    }
                    note.duration_ticks = duration_ticks;
                    let new_end_step =
                        ((note.start_tick + duration_ticks - 1) / TICKS_PER_STEP) as usize;
                    max_end_step = max_end_step.max(new_end_step);
                    edited.push(*note);
                }
                if edited.is_empty() {
                    return;
                }
                for s in step..=max_end_step.min(pattern_length - 1) {
                    st.refresh_rack_cell(channel, s);
                }
                if channel == st.selected {
                    if let Some(window) = weak.upgrade() {
                        st.refresh_note_editor(&window);
                    }
                }
                for note in edited {
                    let _ = tx.send(EngineCommand::UpsertNote {
                        pattern: pattern as u8,
                        channel: channel as u8,
                        note,
                    });
                }
            });
        }

        // Tick-addressed piano-roll editing.
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
                let pattern = st.current_pattern;
                let channel = st.selected;
                let length_ticks = st.pattern_lengths[pattern] as u32 * TICKS_PER_STEP;
                let start_tick = (start_tick.max(0) as u32).min(length_ticks.saturating_sub(1));
                let mut note = st.channels[channel].create_note(
                    pattern,
                    start_tick,
                    duration_ticks.max(1) as u32,
                    midi_note.clamp(36, 84) as u8,
                );
                note.duration_ticks = note
                    .duration_ticks
                    .min(length_ticks.saturating_sub(start_tick).max(1));
                if let Some(stored) = st.channels[channel].notes[pattern]
                    .iter_mut()
                    .find(|stored| stored.id == note.id)
                {
                    *stored = note;
                }
                st.select_note(Some(note.id));
                st.refresh_rack_cell(channel, (start_tick / TICKS_PER_STEP) as usize);
                st.refresh_note_editor(&window);
                let _ = tx.send(EngineCommand::UpsertNote {
                    pattern: pattern as u8,
                    channel: channel as u8,
                    note,
                });
                drop(st);
                record_project_history(&commands, before, &history_state, &window, "Note created");
                note.id as i32
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
            window.on_piano_note_selected(move |id, shift, ctrl| {
                let mut st = st.borrow_mut();
                let id = id as NoteId;
                let pattern = st.current_pattern;
                let channel = st.selected;
                if st.channels[channel].notes[pattern]
                    .iter()
                    .any(|note| note.id == id)
                {
                    // Shift/Ctrl-click builds a multi-selection; a plain
                    // click always collapses to just this note.
                    if shift || ctrl {
                        st.toggle_note_selection(id);
                    } else {
                        st.select_note(Some(id));
                    }
                    if let Some(window) = weak.upgrade() {
                        st.refresh_note_editor(&window);
                    }
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
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                let channel = st.selected;
                let length_ticks = st.pattern_lengths[pattern] as u32 * TICKS_PER_STEP;
                let Some(anchor) = st.channels[channel].notes[pattern]
                    .iter()
                    .copied()
                    .find(|note| note.id == id as NoteId)
                else {
                    return;
                };
                // The gesture reports where the grabbed note should land; every
                // other selected note moves by the same delta, so a chord keeps
                // its shape. A single selection is just the one-note case of
                // this, which is why there is no separate single-note path.
                let mut moving: Vec<NoteId> = st
                    .selected_note_ids
                    .iter()
                    .copied()
                    .filter(|id| {
                        st.channels[channel].notes[pattern]
                            .iter()
                            .any(|note| note.id == *id)
                    })
                    .collect();
                if !moving.contains(&anchor.id) {
                    moving.push(anchor.id);
                }
                let wanted_tick = (start_tick.max(0) as u32).min(length_ticks.saturating_sub(1));
                let wanted_note = midi_note.clamp(36, 84) as u8;
                let tick_delta = wanted_tick as i64 - anchor.start_tick as i64;
                let note_delta = wanted_note as i32 - anchor.note as i32;
                // Clamp the delta by the group, not per note: letting notes
                // clip individually would silently collapse a chord onto one
                // pitch at the edge of the range.
                //
                // Bound by note *starts*, not by their tails. A note is
                // allowed to overhang the pattern's logical end -- that is
                // how a shortened pattern keeps its notes -- so measuring the
                // tail would refuse to move a selection right the moment any
                // member overhung, which is not a rule the single-note drag
                // ever had.
                let (mut min_tick, mut max_tick) = (i64::MAX, i64::MIN);
                let (mut min_note, mut max_note) = (i32::MAX, i32::MIN);
                for note in st.channels[channel].notes[pattern]
                    .iter()
                    .filter(|note| moving.contains(&note.id))
                {
                    min_tick = min_tick.min(note.start_tick as i64);
                    max_tick = max_tick.max(note.start_tick as i64);
                    min_note = min_note.min(note.note as i32);
                    max_note = max_note.max(note.note as i32);
                }
                if min_tick == i64::MAX {
                    return;
                }
                let last_start = length_ticks.saturating_sub(1) as i64;
                let tick_delta = tick_delta.clamp(-min_tick, (last_start - max_tick).max(-min_tick));
                let note_delta = note_delta.clamp(36 - min_note, (84 - max_note).max(36 - min_note));

                let mut edited = Vec::with_capacity(moving.len());
                let mut touched_steps = Vec::with_capacity(moving.len() * 2);
                for note in st.channels[channel].notes[pattern]
                    .iter_mut()
                    .filter(|note| moving.contains(&note.id))
                {
                    touched_steps.push(note.start_tick / TICKS_PER_STEP);
                    note.start_tick = (note.start_tick as i64 + tick_delta).max(0) as u32;
                    note.start_tick = note.start_tick.min(length_ticks.saturating_sub(1));
                    note.duration_ticks = note
                        .duration_ticks
                        .min(length_ticks.saturating_sub(note.start_tick).max(1));
                    note.note = (note.note as i32 + note_delta).clamp(0, 127) as u8;
                    touched_steps.push(note.start_tick / TICKS_PER_STEP);
                    edited.push(*note);
                }
                st.channels[channel].notes[pattern].sort_by_key(|note| (note.start_tick, note.id));
                if edited.len() == 1 {
                    st.select_note(Some(edited[0].id));
                }
                for step in touched_steps {
                    st.refresh_rack_cell(channel, step as usize);
                }
                st.refresh_note_editor(&window);
                for note in edited {
                    let _ = tx.send(EngineCommand::UpsertNote {
                        pattern: pattern as u8,
                        channel: channel as u8,
                        note,
                    });
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
                let Some(window) = weak.upgrade() else { return -1 };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                let channel = st.selected;
                let anchor_id = anchor_id.max(0) as NoteId;
                let originals: Vec<NoteEvent> = st.channels[channel].notes[pattern]
                    .iter()
                    .copied()
                    .filter(|note| {
                        note.id == anchor_id || st.selected_note_ids.contains(&note.id)
                    })
                    .collect();
                if originals.is_empty() {
                    return -1;
                }
                // The copies land exactly on the originals and the selection
                // moves to them, so the drag that triggered this continues on
                // the duplicate with no visible jump.
                let mut copies = Vec::with_capacity(originals.len());
                let mut anchor_copy = -1;
                for original in originals {
                    let id = st.channels[channel].next_note_id;
                    st.channels[channel].next_note_id = id.wrapping_add(1).max(1);
                    let copy = NoteEvent { id, ..original };
                    if original.id == anchor_id {
                        anchor_copy = id as i32;
                    }
                    st.channels[channel].notes[pattern].push(copy);
                    copies.push(copy);
                }
                st.channels[channel].notes[pattern].sort_by_key(|note| (note.start_tick, note.id));
                st.selected_note_ids = copies.iter().map(|note| note.id).collect();
                st.selected_note_id = (copies.len() == 1).then(|| copies[0].id);
                st.refresh_rack_row(channel);
                st.refresh_note_editor(&window);
                for note in copies {
                    let _ = tx.send(EngineCommand::UpsertNote {
                        pattern: pattern as u8,
                        channel: channel as u8,
                        note,
                    });
                }
                drop(st);
                record_project_history(
                    &commands,
                    before,
                    &history_state,
                    &window,
                    "Notes duplicated",
                );
                anchor_copy
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_piano_note_resized(move |id, duration| {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                let channel = st.selected;
                let length_ticks = st.pattern_lengths[pattern] as u32 * TICKS_PER_STEP;
                let Some(note) = st.channels[channel].notes[pattern]
                    .iter_mut()
                    .find(|note| note.id == id as NoteId)
                else {
                    return;
                };
                note.duration_ticks = (duration.max(1) as u32)
                    .min(length_ticks.saturating_sub(note.start_tick).max(1));
                let edited = *note;
                st.select_note(Some(edited.id));
                st.refresh_note_editor(&window);
                let _ = tx.send(EngineCommand::UpsertNote {
                    pattern: pattern as u8,
                    channel: channel as u8,
                    note: edited,
                });
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
            window.on_piano_note_removed(move |id| {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                let channel = st.selected;
                let Some(index) = st.channels[channel].notes[pattern]
                    .iter()
                    .position(|note| note.id == id as NoteId)
                else {
                    return;
                };
                let removed = st.channels[channel].notes[pattern].remove(index);
                st.prune_note_selection(&[removed.id]);
                st.refresh_rack_cell(channel, (removed.start_tick / TICKS_PER_STEP) as usize);
                st.refresh_note_editor(&window);
                let _ = tx.send(EngineCommand::RemoveNote {
                    pattern: pattern as u8,
                    channel: channel as u8,
                    id: removed.id,
                });
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
                let velocity = (1.0 + value.clamp(0.0, 1.0) * 126.0).round() as u8;
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                let channel = st.selected;
                let Some(note) = st.channels[channel].notes[pattern]
                    .iter_mut()
                    .find(|note| note.id == id as NoteId)
                else {
                    return;
                };
                note.velocity = velocity;
                let edited = *note;
                st.select_note(Some(edited.id));
                st.refresh_rack_cell(channel, (edited.start_tick / TICKS_PER_STEP) as usize);
                st.refresh_note_editor(&window);
                let _ = tx.send(EngineCommand::UpsertNote {
                    pattern: pattern as u8,
                    channel: channel as u8,
                    note: edited,
                });
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
                let destinations = st.automation_destinations();
                let Some(target) = destinations
                    .get(index.max(0) as usize)
                    .map(|(target, _, _)| *target)
                else {
                    return;
                };
                st.automation_target.set(Some(target));
                st.automation_selected_point.set(None);
                // Opening the lane in the project and in the engine keeps the
                // picker's "already open" marks meaningful even before the
                // first point is drawn.
                let pattern = st.current_pattern;
                let channel = st.selected;
                if let Some(lanes) = st
                    .channels
                    .get_mut(channel)
                    .and_then(|state| state.automation.get_mut(pattern))
                {
                    if !lanes.iter().any(|lane| lane.target == target)
                        && lanes.len() < MAX_AUTOMATION_LANES_PER_CHANNEL
                    {
                        lanes.push(AutomationLane::new(target));
                    }
                }
                let _ = tx.send(EngineCommand::OpenAutomationLane {
                    pattern: pattern as u8,
                    channel: channel as u8,
                    target,
                });
                st.refresh_automation(&window);
                drop(st);
                // An open lane is saved state even before it has a point in
                // it, so opening one has to mark the document dirty.
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
                let (pattern, channel) = (st.current_pattern, st.selected);
                let Some(target) = st.automation_target.get() else {
                    return;
                };
                let Some(lane) = st.automation_lane_mut() else {
                    return;
                };
                lane.clear();
                st.automation_selected_point.set(None);
                let _ = tx.send(EngineCommand::ClearAutomationLane {
                    pattern: pattern as u8,
                    channel: channel as u8,
                    target,
                });
                st.refresh_automation(&window);
                drop(st);
                record_project_history(
                    &commands,
                    before,
                    &history_state,
                    &window,
                    "Automation cleared",
                );
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
                let (pattern, channel) = (st.current_pattern, st.selected);
                let Some(target) = st.automation_target.get() else {
                    return;
                };
                if let Some(lanes) = st
                    .channels
                    .get_mut(channel)
                    .and_then(|state| state.automation.get_mut(pattern))
                {
                    lanes.retain(|lane| lane.target != target);
                }
                st.automation_target.set(None);
                st.automation_selected_point.set(None);
                let _ = tx.send(EngineCommand::RemoveAutomationLane {
                    pattern: pattern as u8,
                    channel: channel as u8,
                    target,
                });
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
                let st = st.borrow();
                let Some(lane) = st.automation_lane() else {
                    return -1;
                };
                let tolerance = tolerance.max(1);
                lane.points()
                    .iter()
                    .filter(|point| (point.tick as i32 - tick).abs() <= tolerance)
                    .filter(|point| (point.value - value).abs() <= 0.12)
                    .min_by(|a, b| {
                        let key = |point: &AutomationPoint| {
                            (point.tick as i32 - tick).abs() as f32 / tolerance as f32
                                + (point.value - value).abs() / 0.12
                        };
                        key(a).total_cmp(&key(b))
                    })
                    .map(|point| point.id as i32)
                    .unwrap_or(-1)
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_automation_point_created(move |tick, value| {
                let Some(window) = weak.upgrade() else { return -1 };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let (pattern, channel) = (st.current_pattern, st.selected);
                let length_ticks = st.pattern_lengths[pattern] as u32 * TICKS_PER_STEP;
                let Some(target) = st.automation_target.get() else {
                    return -1;
                };
                let Some(lane) = st.automation_lane_mut() else {
                    return -1;
                };
                let id = lane.allocate_id();
                let point = AutomationPoint::new(
                    id,
                    (tick.max(0) as u32).min(length_ticks),
                    value.clamp(0.0, 1.0),
                );
                if !lane.upsert(point) {
                    return -1;
                }
                st.automation_selected_point.set(Some(id));
                let _ = tx.send(EngineCommand::UpsertAutomationPoint {
                    pattern: pattern as u8,
                    channel: channel as u8,
                    target,
                    point,
                });
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
                let (pattern, channel) = (st.current_pattern, st.selected);
                let length_ticks = st.pattern_lengths[pattern] as u32 * TICKS_PER_STEP;
                let Some(target) = st.automation_target.get() else {
                    return;
                };
                let Some(lane) = st.automation_lane_mut() else {
                    return;
                };
                let id = id.max(0) as PointId;
                if !lane.points().iter().any(|point| point.id == id) {
                    return;
                }
                let point = AutomationPoint::new(
                    id,
                    (tick.max(0) as u32).min(length_ticks),
                    value.clamp(0.0, 1.0),
                );
                lane.upsert(point);
                st.automation_selected_point.set(Some(id));
                let _ = tx.send(EngineCommand::UpsertAutomationPoint {
                    pattern: pattern as u8,
                    channel: channel as u8,
                    target,
                    point,
                });
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
                let (pattern, channel) = (st.current_pattern, st.selected);
                let Some(target) = st.automation_target.get() else {
                    return;
                };
                let id = id.max(0) as PointId;
                let Some(lane) = st.automation_lane_mut() else {
                    return;
                };
                if lane.remove(id).is_none() {
                    return;
                }
                if st.automation_selected_point.get() == Some(id) {
                    st.automation_selected_point.set(None);
                }
                let _ = tx.send(EngineCommand::RemoveAutomationPoint {
                    pattern: pattern as u8,
                    channel: channel as u8,
                    target,
                    id,
                });
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
                    let pattern = st.current_pattern;
                    let channel = st.selected;
                    let Some(id) = st.selected_note_id else {
                        return;
                    };
                    let length_ticks = st.pattern_lengths[pattern] as u32 * TICKS_PER_STEP;
                    let Some(note) = st.channels[channel].notes[pattern]
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
            .clamp(36, 84)
            as u8);
        wire_selected_note_edit!(
            on_selected_velocity_changed,
            velocity,
            |value: i32, _, _| value.clamp(1, 127) as u8
        );
        wire_selected_note_edit!(
            on_selected_duration_changed,
            duration_ticks,
            |value: i32, note: &mut NoteEvent, length_ticks: u32| (value.max(1) as u32)
                .min(length_ticks.saturating_sub(note.start_tick).max(1))
        );

        // Ctrl+A: select every note in the current channel's current
        // pattern. Shares the same selection set Shift/Ctrl-click builds.
        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_select_all_requested(move || {
                let Some(window) = weak.upgrade() else { return };
                let mut st = st.borrow_mut();
                let channel = st.selected;
                st.select_all_notes(channel);
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
                let ids: Vec<NoteId> = st.borrow().selected_note_ids.iter().copied().collect();
                if ids.is_empty() {
                    return;
                }
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                let channel = st.selected;
                st.channels[channel].notes[pattern].retain(|note| !ids.contains(&note.id));
                st.prune_note_selection(&ids);
                st.refresh_rack_row(channel);
                st.refresh_note_editor(&window);
                for id in &ids {
                    let _ = tx.send(EngineCommand::RemoveNote {
                        pattern: pattern as u8,
                        channel: channel as u8,
                        id: *id,
                    });
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
                let ch = ch as usize;
                {
                    let mut guard = st.borrow_mut();
                    // Re-clicking the selected channel is still meaningful
                    // when the rack is showing a bus: it points it back.
                    let already_here = ch == guard.selected
                        && guard.effect_target == EffectTarget::Channel(ch as u8);
                    if ch >= guard.channels.len() || already_here {
                        return;
                    }
                    guard.selected = ch;
                    guard.effect_target = EffectTarget::Channel(ch as u8);
                    guard.select_note(None);
                }
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

        // Channel output level and pan.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_channel_volume_changed(move |ch, volume| {
                let ch = ch as usize;
                let mut st = st.borrow_mut();
                let Some(channel) = st.channels.get_mut(ch) else {
                    return;
                };
                // Gain stages share the container's +12 dB headroom.
                channel.volume = volume.clamp(0.0, MAX_LINEAR_GAIN);
                let volume = channel.volume;
                st.sync_row_flags();
                // The source device's output-trim knob is the same parameter;
                // restate it or its readout freezes at whatever the channel
                // had when it was selected.
                if ch == st.selected {
                    if let Some(w) = weak.upgrade() {
                        w.set_selected_channel_volume_db(linear_to_db(volume));
                    }
                }
                let _ = tx.send(EngineCommand::SetChannelVolume {
                    channel: ch as u8,
                    volume,
                });
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_channel_pan_changed(move |ch, pan| {
                let ch = ch as usize;
                let mut st = st.borrow_mut();
                let Some(channel) = st.channels.get_mut(ch) else {
                    return;
                };
                channel.pan = pan.clamp(-1.0, 1.0);
                let pan = channel.pan;
                st.sync_row_flags();
                let _ = tx.send(EngineCommand::SetChannelPan {
                    channel: ch as u8,
                    pan,
                });
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
                    let channel = guard.selected;
                    if guard.channels[channel].kind == source {
                        return;
                    }
                    guard.reset_channel_source(channel, source);
                    guard.select_note(None);
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
            let tx = cmd_tx.clone();
            let reset_tx = sample_reset_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_add_channel_clicked(move |value| {
                let source = device_kind_from_int(value);
                let mut st = st.borrow_mut();
                if st.channels.len() >= MAX_CHANNELS {
                    return;
                }
                dbg_log("UI: add channel");
                let index = st.channels.len();
                let mut ch = ChannelState::new(index);
                ch.notes.resize_with(st.pattern_lengths.len(), Vec::new);
                ch.automation.resize_with(st.pattern_lengths.len(), Vec::new);
                let cells: Vec<StepCell> = (0..st.pattern_lengths[st.current_pattern])
                    .map(|step| rack_cell(&ch.notes[st.current_pattern], step))
                    .collect();
                let model = Rc::new(VecModel::from(cells));
                st.channels.push(ch);
                st.reset_channel_source(index, source);
                st.selected = index;
                st.effect_target = EffectTarget::Channel(index as u8);
                st.select_note(None);
                let ch = &st.channels[index];
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
                let _ = tx.send(EngineCommand::AddChannel { source });
            });
        }
        {
            let st = state.clone();
            let tx = project_edit_tx.clone();
            let weak = window.as_weak();
            window.on_remove_channel_clicked(move || {
                let Some(window) = weak.upgrade() else { return };
                let selected = st.borrow().selected;
                queue_channel_delete(&tx, &st, &window, selected, "Channel deleted");
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
                        let Some(copy) = snapshot_channel_clipboard(&st.borrow(), &window, index)
                        else {
                            return;
                        };
                        if st.borrow().channels.len() <= 1 {
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
                        let Some(copy) = snapshot_channel_clipboard(&st.borrow(), &window, index)
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
                        let Some(copy) = snapshot_channel_clipboard(&st.borrow(), &window, index)
                        else {
                            return;
                        };
                        if queue_channel_insert(&tx, &st, &window, index, copy, "Channel cloned") {
                            commands.borrow_mut().project_edit_pending = true;
                            sync_command_availability(&window, &commands.borrow());
                        }
                    }
                    6 => {
                        if queue_channel_delete(&tx, &st, &window, index, "Channel deleted") {
                            commands.borrow_mut().project_edit_pending = true;
                            sync_command_availability(&window, &commands.borrow());
                        }
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
                let index = st.borrow().current_pattern;
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
                let index = st.borrow().current_pattern;
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
                let index = st.borrow().current_pattern;
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
                let Ok(bus) = u8::try_from(bus) else { return };
                {
                    let mut guard = st.borrow_mut();
                    if bus as usize >= guard.buses.len() {
                        return;
                    }
                    guard.effect_target = EffectTarget::Bus(bus);
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
                let Ok(index) = usize::try_from(bus) else {
                    return;
                };
                let mut guard = st.borrow_mut();
                let Some(setup) = guard.buses.get_mut(index) else {
                    return;
                };
                setup.bus.muted = !setup.bus.muted;
                let muted = setup.bus.muted;
                guard.sync_mixer_strip(index);
                if let Some(w) = weak.upgrade() {
                    guard.sync_bus_editor(&w);
                }
                let _ = tx.send(EngineCommand::SetBusMuted {
                    bus: index as u8,
                    muted,
                });
            });
        }

        {
            let telemetry = telemetry_tx.clone();
            let st = state.clone();
            window.on_eq_analyzer_changed(move |slot, enabled| {
                let mut st = st.borrow_mut();
                let target = st.effect_target;
                let slot = slot as usize;
                let Some(effect) = st.effect_chain_mut().and_then(|chain| chain.get_mut(slot))
                else {
                    return;
                };
                let mooloop_core::EffectParams::Eq(params) = &mut effect.params else {
                    return;
                };
                params.analyzer_enabled = enabled;
                let row = effect_slot_row(effect);
                st.effect_slot_model.set_row_data(slot, row);
                let _ = telemetry.send(TelemetryAction::SetEffectSpectrumEnabled {
                    target,
                    slot: slot as u8,
                    enabled,
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            let st = state.clone();
            window.on_bus_volume_changed(move |bus, volume| {
                let Ok(index) = usize::try_from(bus) else {
                    return;
                };
                let volume = volume.clamp(0.0, 1.0);
                let mut guard = st.borrow_mut();
                let Some(setup) = guard.buses.get_mut(index) else {
                    return;
                };
                setup.bus.volume = volume;
                guard.sync_mixer_strip(index);
                if let Some(w) = weak.upgrade() {
                    guard.sync_bus_editor(&w);
                }
                let _ = tx.send(EngineCommand::SetBusVolume {
                    bus: index as u8,
                    volume,
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            let st = state.clone();
            window.on_bus_pan_changed(move |bus, pan| {
                let Ok(index) = usize::try_from(bus) else {
                    return;
                };
                let pan = pan.clamp(-1.0, 1.0);
                let mut guard = st.borrow_mut();
                let Some(setup) = guard.buses.get_mut(index) else {
                    return;
                };
                setup.bus.pan = pan;
                guard.sync_mixer_strip(index);
                if let Some(w) = weak.upgrade() {
                    guard.sync_bus_editor(&w);
                }
                let _ = tx.send(EngineCommand::SetBusPan {
                    bus: index as u8,
                    pan,
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            let st = state.clone();
            window.on_bus_output_changed(move |bus, output| {
                let (Ok(index), Ok(output)) = (usize::try_from(bus), u8::try_from(output)) else {
                    return;
                };
                let output = sanitize_route(index as u8, output);
                let mut guard = st.borrow_mut();
                if guard.buses.get(index).is_none() {
                    return;
                }
                // The picker greys out looping destinations, but this is the
                // boundary the engine's schedule rests on, so refuse here too
                // rather than shipping a graph that cannot be sorted.
                if would_create_cycle(&guard.buses, index as u8, output) {
                    if let Some(w) = weak.upgrade() {
                        let name = guard.buses[output as usize].bus.name.clone();
                        w.set_status_message(
                            format!("{name} already feeds this bus - routing would loop").into(),
                        );
                    }
                    return;
                }
                let previous = std::mem::replace(&mut guard.buses[index].bus.output, output);
                let Some(graph) = compile_bus_graph(&guard.buses) else {
                    // Unreachable given the check above. Restore the visible
                    // graph rather than letting UI and audio generations
                    // diverge.
                    guard.buses[index].bus.output = previous;
                    return;
                };
                // Every strip's legal destinations move when an edge does.
                if let Some(w) = weak.upgrade() {
                    guard.sync_mixer(&w);
                }
                let _ = tx.send(EngineCommand::InstallBusGraph { graph });
            });
        }

        {
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            let st = state.clone();
            window.on_channel_bus_changed(move |channel, bus| {
                let (Ok(channel), Ok(bus)) = (usize::try_from(channel), u8::try_from(bus)) else {
                    return;
                };
                if bus as usize >= MAX_BUSES {
                    return;
                }
                let mut guard = st.borrow_mut();
                let Some(state) = guard.channels.get_mut(channel) else {
                    return;
                };
                state.bus = bus;
                guard.sync_row_flags();
                // Feed counts moved, so both the old and new bus restate them.
                if let Some(w) = weak.upgrade() {
                    guard.sync_mixer(&w);
                }
                let _ = tx.send(EngineCommand::SetChannelBus {
                    channel: channel as u8,
                    bus,
                });
            });
        }

        // --- Effect chain callbacks (edit whatever the rack is pointed at) ---
        {
            let tx = cmd_tx.clone();
            let stx = structural_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_add_effect_clicked(move |kind_index, insert_before| {
                let Some(kind) = effect_kind_from_index(kind_index) else {
                    return;
                };
                let mut st = st.borrow_mut();
                let target = st.effect_target;
                let (slot, tail_slot, params) = {
                    let Some(effects) = st.effect_chain_mut() else {
                        return;
                    };
                    if effects.len() >= MAX_EFFECTS_PER_CHANNEL {
                        return;
                    }
                    let effect = EffectSlotState::of_kind(kind);
                    let slot = (insert_before as usize).min(effects.len());
                    let tail_slot = effects.len();
                    effects.insert(slot, effect);
                    (slot, tail_slot, effect.params)
                };
                st.sync_effects();
                // Install into the vacant tail then move it left. Keeping this
                // on the ordered stream means the realtime chain sees the same
                // order as the UI/model without allocating in its callback.
                // The dry-align ring is built here for the same reason as the
                // node: construction allocates, so it happens off the audio
                // thread and rides the same structural command.
                let bpm = weak
                    .upgrade()
                    .map_or(INITIAL_BPM as f64, |window| window.get_bpm() as f64);
                let node = build_effect_at_tempo(params, sample_rate, bpm);
                let align = DryAlign::new(node.dry_path_latency_frames()).map(Box::new);
                let _ = stx.send(StructuralCommand::InstallEffect {
                    target,
                    slot: tail_slot as u8,
                    kind,
                    resource_key: params
                        .reverb()
                        .map(|params| params.fingerprint())
                        .or_else(|| params.buffer().copied().map(buffer_allocation_key)),
                    node,
                    align,
                    analyzer: Box::new(SpectrumAnalyzer::new()),
                });
                for position in (slot + 1..=tail_slot).rev() {
                    let _ = tx.send(EngineCommand::SwapEffectSlots {
                        target,
                        slot_a: position as u8,
                        slot_b: position as u8 - 1,
                    });
                }
            });
        }

        {
            let tx = cmd_tx.clone();
            let stx = structural_tx.clone();
            let st = state.clone();
            window.on_remove_effect_clicked(move |slot| {
                let mut st = st.borrow_mut();
                let target = st.effect_target;
                let slot = slot as usize;
                let removed_tail = {
                    let Some(effects) = st.effect_chain_mut() else {
                        return;
                    };
                    if slot >= effects.len() {
                        return;
                    }
                    effects.remove(slot);
                    effects.len()
                };
                st.sync_effects();
                // Mirror on the engine with its two primitives: shift later
                // slots down by adjacent swaps, then drop the vacated tail.
                for j in (slot + 1)..=removed_tail {
                    let _ = tx.send(EngineCommand::SwapEffectSlots {
                        target,
                        slot_a: j as u8,
                        slot_b: j as u8 - 1,
                    });
                }
                let _ = stx.send(StructuralCommand::RemoveEffect {
                    target,
                    slot: removed_tail as u8,
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_effect_bypass_toggled(move |slot| {
                let mut st = st.borrow_mut();
                let target = st.effect_target;
                let slot = slot as usize;
                let Some(effects) = st.effect_chain_mut() else {
                    return;
                };
                let Some(effect) = effects.get_mut(slot) else {
                    return;
                };
                effect.bypassed = !effect.bypassed;
                let bypassed = effect.bypassed;
                let row = effect_slot_row(effect);
                st.effect_slot_model.set_row_data(slot, row);
                let _ = tx.send(EngineCommand::SetEffectBypassed {
                    target,
                    slot: slot as u8,
                    bypassed,
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_effect_wet_dry_changed(move |slot, wet_dry| {
                let mut st = st.borrow_mut();
                let target = st.effect_target;
                let Some(effect) = st
                    .effect_chain_mut()
                    .and_then(|chain| chain.get_mut(slot as usize))
                else {
                    return;
                };
                effect.wet_dry = wet_dry.clamp(0.0, 1.0);
                let value = effect.wet_dry;
                let row = effect_slot_row(effect);
                st.effect_slot_model.set_row_data(slot as usize, row);
                let _ = tx.send(EngineCommand::SetEffectWetDry {
                    target,
                    slot: slot as u8,
                    wet_dry: value,
                });
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_effect_input_trim_changed(move |slot, input_trim_db| {
                let mut st = st.borrow_mut();
                let target = st.effect_target;
                let Some(effect) = st
                    .effect_chain_mut()
                    .and_then(|chain| chain.get_mut(slot as usize))
                else {
                    return;
                };
                // The knob works in dB from unity; the project and the wire
                // carry linear gain.
                effect.input_trim = db_to_linear(input_trim_db.clamp(METER_FLOOR_DB, 12.0));
                let value = effect.input_trim;
                let row = effect_slot_row(effect);
                st.effect_slot_model.set_row_data(slot as usize, row);
                let _ = tx.send(EngineCommand::SetEffectInputTrim {
                    target,
                    slot: slot as u8,
                    input_trim: value,
                });
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_effect_output_trim_changed(move |slot, output_trim_db| {
                let mut st = st.borrow_mut();
                let target = st.effect_target;
                let Some(effect) = st
                    .effect_chain_mut()
                    .and_then(|chain| chain.get_mut(slot as usize))
                else {
                    return;
                };
                effect.output_trim = db_to_linear(output_trim_db.clamp(METER_FLOOR_DB, 12.0));
                let value = effect.output_trim;
                let row = effect_slot_row(effect);
                st.effect_slot_model.set_row_data(slot as usize, row);
                let _ = tx.send(EngineCommand::SetEffectOutputTrim {
                    target,
                    slot: slot as u8,
                    output_trim: value,
                });
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
                    target: rst.borrow().effect_target,
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
                    target: rst.borrow().effect_target,
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
                    target: st.borrow().effect_target,
                    slot,
                    event,
                });
            });
        }
        {
            let tx = cmd_tx.clone();
            let reverb_tx = reverb_build_tx.clone();
            let st = state.clone();
            // One callback for every parameter of every effect kind: the
            // rack sends a descriptor index and a normalized position, and
            // the descriptor table converts to the natural units the wire
            // and the DSP use.
            window.on_effect_param_changed(move |slot, param_index, normalized| {
                let mut st = st.borrow_mut();
                let target = st.effect_target;
                let slot = slot as usize;
                let Some(effects) = st.effect_chain_mut() else {
                    return;
                };
                let Some(effect) = effects.get_mut(slot) else {
                    return;
                };
                let Ok(param_index) = usize::try_from(param_index) else {
                    return;
                };
                let Some(descriptor) = effect.kind().descriptors().get(param_index) else {
                    return;
                };
                let id = descriptor.id;
                let reverb_expected_key = effect.params.reverb().map(|params| params.fingerprint());
                let Some(value) = effect
                    .params
                    .set(id, descriptor.from_normalized(normalized))
                else {
                    return;
                };
                let reverb_request = reverb_expected_key.zip(effect.params.reverb().copied());
                let row = effect_slot_row(effect);
                st.effect_slot_model.set_row_data(slot, row);
                if let Some((expected_resource_key, params)) = reverb_request {
                    let _ = reverb_tx.send(ReverbBuildRequest {
                        target,
                        slot: slot as u8,
                        expected_resource_key,
                        params,
                    });
                } else {
                    let _ = tx.send(EngineCommand::SetEffectParam {
                        target,
                        slot: slot as u8,
                        id,
                        value,
                    });
                }
            });
        }

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_reorder_effect(move |from, to| {
                let mut st = st.borrow_mut();
                let target = st.effect_target;
                let (from, to) = (from as usize, to as usize);
                {
                    let Some(effects) = st.effect_chain_mut() else {
                        return;
                    };
                    let len = effects.len();
                    if from >= len || to >= len || from == to {
                        return;
                    }
                    let effect = effects.remove(from);
                    effects.insert(to, effect);
                }
                st.sync_effects();
                // The engine's only reorder primitive is an adjacent-slot
                // swap (pointer swap, realtime-safe); a move is a run of them.
                if from < to {
                    for i in from..to {
                        let _ = tx.send(EngineCommand::SwapEffectSlots {
                            target,
                            slot_a: i as u8,
                            slot_b: i as u8 + 1,
                        });
                    }
                } else {
                    for i in (to + 1..=from).rev() {
                        let _ = tx.send(EngineCommand::SwapEffectSlots {
                            target,
                            slot_a: i as u8,
                            slot_b: i as u8 - 1,
                        });
                    }
                }
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
        wire_unit_param!(on_filter_cutoff_changed, filter_cutoff);
        wire_unit_param!(on_filter_resonance_changed, filter_resonance);
        wire_unit_param!(on_sampler_drive_changed, drive);
        wire_unit_param!(on_bit_reduction_changed, bit_reduction);
        wire_unit_param!(on_rate_reduction_changed, rate_reduction);

        {
            // Pure view state: re-bin the waveform for whatever range is
            // now visible so zooming in reveals real detail rather than
            // just stretching the full-sample overview's fixed bins.
            let st = state.clone();
            window.on_waveform_view_changed(move |offset: f32, visible_fraction: f32| {
                let st = st.borrow();
                let Some(channel) = st.channels.get(st.selected) else {
                    return;
                };
                let Some(sample) = channel.sample_data.as_ref() else {
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
            let weak = window.as_weak();
            window.on_root_note_changed(move |note| {
                let mut st = st.borrow_mut();
                let ch = st.selected;
                let Some(channel) = st.channels.get_mut(ch) else {
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
                let ch = st.selected;
                let Some(channel) = st.channels.get_mut(ch) else {
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
                let ch = st.selected;
                let Some(channel) = st.channels.get_mut(ch) else {
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

        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_voice_mode_changed(move |value| {
                let mut st = st.borrow_mut();
                let channel_index = st.selected;
                let channel = &mut st.channels[channel_index];
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
                let channel_index = st.selected;
                let channel = &mut st.channels[channel_index];
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
                let channel_index = st.selected;
                let channel = &mut st.channels[channel_index];
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
                let channel_index = st.selected;
                let channel = &mut st.channels[channel_index];
                channel.params.choke_group = value.clamp(0, 16) as u8;
                let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                    channel: channel_index as u8,
                    params: channel.params,
                });
            });
        }

        macro_rules! wire_drum_param {
            ($callback:ident, $field:ident) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                let window_weak = window.as_weak();
                window.$callback(move |value: f32| {
                    let mut st = st.borrow_mut();
                    let channel_index = st.selected;
                    let channel = &mut st.channels[channel_index];
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
                    let channel_index = st.selected;
                    let channel = &mut st.channels[channel_index];
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
                let channel_index = st.selected;
                let channel = &mut st.channels[channel_index];
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
                let channel_index = st.selected;
                let channel = &mut st.channels[channel_index];
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
                    let channel_index = st.selected;
                    let channel = &mut st.channels[channel_index];
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
                let channel_index = st.selected;
                let channel = &mut st.channels[channel_index];
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
                let channel_index = st.selected;
                let channel = &mut st.channels[channel_index];
                channel.mono_params.lfo.retrigger = value;
                let _ = tx.send(EngineCommand::SetChannelMonoSynthParams {
                    channel: channel_index as u8,
                    params: channel.mono_params,
                });
            });
        }

        macro_rules! wire_mono_osc_float {
            ($callback:ident, $index:expr, $field:ident) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                window.$callback(move |value: f32| {
                    let mut st = st.borrow_mut();
                    let channel_index = st.selected;
                    let channel = &mut st.channels[channel_index];
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
                    let channel_index = st.selected;
                    let channel = &mut st.channels[channel_index];
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
                    let channel_index = st.selected;
                    let channel = &mut st.channels[channel_index];
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
                let channel_index = st.selected;
                let channel = &mut st.channels[channel_index];
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
                let channel_index = st.selected;
                let channel = &mut st.channels[channel_index];
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
                let channel_index = st.selected;
                let channel = &mut st.channels[channel_index];
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
                    let channel_index = st.selected;
                    let channel = &mut st.channels[channel_index];
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
                    let channel_index = st.selected;
                    let channel = &mut st.channels[channel_index];
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
                let (channel, source_revision) = {
                    let st = st.borrow();
                    (st.selected, st.source_revision)
                };
                let tx = load_tx.clone();
                dbg_log(&format!("UI: loading sample for channel {channel}"));
                std::thread::spawn(move || {
                    let result = pick_wav_via_zenity().map(|path| load_sample_at_path(&path));
                    let _ = tx.send(LoadResult {
                        channel,
                        source_revision,
                        result,
                    });
                });
            });
        }
        {
            let st = state.clone();
            let load_tx = load_tx.clone();
            window.on_previous_sample_clicked(move || {
                let (channel, source_revision, path) = {
                    let st = st.borrow();
                    (
                        st.selected,
                        st.source_revision,
                        st.channels[st.selected].sample_path.clone(),
                    )
                };
                let Some(path) = path else { return };
                let tx = load_tx.clone();
                std::thread::spawn(move || {
                    let result = match adjacent_wav(&path, -1) {
                        Ok(Some(path)) => Some(load_sample_at_path(&path)),
                        Ok(None) => None,
                        Err(error) => Some(Err(error)),
                    };
                    let _ = tx.send(LoadResult {
                        channel,
                        source_revision,
                        result,
                    });
                });
            });
        }
        {
            let st = state.clone();
            let load_tx = load_tx.clone();
            window.on_next_sample_clicked(move || {
                let (channel, source_revision, path) = {
                    let st = st.borrow();
                    (
                        st.selected,
                        st.source_revision,
                        st.channels[st.selected].sample_path.clone(),
                    )
                };
                let Some(path) = path else { return };
                let tx = load_tx.clone();
                std::thread::spawn(move || {
                    let result = match adjacent_wav(&path, 1) {
                        Ok(Some(path)) => Some(load_sample_at_path(&path)),
                        Ok(None) => None,
                        Err(error) => Some(Err(error)),
                    };
                    let _ = tx.send(LoadResult {
                        channel,
                        source_revision,
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
                            state.bundle_path = None;
                            state.dirty = false;
                            state.revision = state.revision.wrapping_add(1);
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
                            state.bundle_path = Some(path.clone());
                            if state.revision == revision {
                                state.dirty = false;
                                apply_sample_references(&mut state.channels, sample_references);
                            }
                            state.update_document_title(&window);
                            window.set_embed_assets(mode == AssetMode::Embedded);
                            window.set_status_message(
                                operation_status("Song saved", &path, &report.warnings).into(),
                            );
                        }
                        DocumentResult::SavedOther { label, report } => {
                            window.set_status_message(
                                format!("{label}{}", warning_suffix(report.warnings.len())).into(),
                            );
                        }
                        DocumentResult::SavedPreset { label, report } => {
                            window.set_status_message(
                                format!("{label}{}", warning_suffix(report.warnings.len())).into(),
                            );
                            window.set_save_preset_open(false);
                            refresh_preset_menus(&st, &window);
                        }
                        DocumentResult::Exported { path } => {
                            window
                                .set_status_message(format!("Exported {}", path.display()).into());
                        }
                        DocumentResult::SaveFailed { action, error } => {
                            window.set_save_error_title(format!("Could not save {action}").into());
                            window.set_save_error_detail(error.clone().into());
                            window.set_save_error_open(true);
                            window.set_status_message(format!("Save {action} failed").into());
                        }
                        DocumentResult::Failed(error) => {
                            window.set_status_message(format!("Error: {error}").into());
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
                            } = report;
                            let current = st
                                .borrow()
                                .project_snapshot(window.get_bpm(), window.get_swing_percent());
                            let current_samples = st.borrow().sample_snapshots();
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
                                            .map(|(index, setup)| {
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
                                    project.channels[selected].setup = setup;
                                    let mut samples = current_samples;
                                    samples[selected] = loaded_samples.into_iter().next().flatten();
                                    Some((project, samples, false))
                                }
                                (LoadTarget::Generator, LoadedDocument::Generator(source)) => {
                                    let mut project = current;
                                    let selected = project.selected_channel as usize;
                                    project.channels[selected].setup.channel.kind = source.kind();
                                    project.channels[selected].setup.source = source;
                                    let mut samples = current_samples;
                                    samples[selected] = loaded_samples.into_iter().next().flatten();
                                    Some((project, samples, false))
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
                                let mut state = st.borrow_mut();
                                if is_song {
                                    state.bundle_path = Some(path.clone());
                                    state.dirty = false;
                                    window.set_embed_assets(asset_mode == AssetMode::Embedded);
                                } else {
                                    state.dirty = true;
                                    state.revision = state.revision.wrapping_add(1);
                                }
                                state.update_document_title(&window);
                                window.set_status_message(
                                    format!(
                                        "Loaded {}{}",
                                        path.display(),
                                        warning_suffix(warnings.len())
                                    )
                                    .into(),
                                );
                            }
                        }
                    }
                }
                while let Ok(load) = load_rx.try_recv() {
                    let still_current = {
                        let st = st.borrow();
                        load.source_revision == st.source_revision
                            && st
                                .channels
                                .get(load.channel)
                                .is_some_and(|channel| channel.kind == DeviceKind::Sampler)
                    };
                    if !still_current {
                        continue;
                    }
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
                    handle.load_sample(load.channel, loaded.sample.clone());
                    let mut st = st.borrow_mut();
                    if let Some(ch) = st.channels.get_mut(load.channel) {
                        ch.sample_name = name;
                        ch.sample_description = description;
                        ch.sample_duration = duration;
                        ch.sample_path = Some(loaded.path);
                        ch.sample_embedded = false;
                        ch.sample_data = Some(loaded.sample.clone());
                        ch.waveform = waveform;
                        ch.can_previous_sample = loaded.can_previous;
                        ch.can_next_sample = loaded.can_next;
                    }
                    st.dirty = true;
                    st.revision = st.revision.wrapping_add(1);
                    if load.channel == st.selected {
                        if let Some(w) = weak.upgrade() {
                            st.refresh_editor(&w);
                            st.update_document_title(&w);
                        }
                    }
                }
                while let Ok(channel) = sample_reset_rx.try_recv() {
                    if let Some(sample) = default_sample_for_pump.as_ref() {
                        handle.load_sample(channel, sample.clone());
                    } else {
                        handle.clear_sample(channel);
                    }
                }
                let mut forwarded = 0usize;
                let mut document_title_needs_refresh = false;
                while let Ok(message) = pending_rx.try_recv() {
                    match message {
                        PendingEngineMessage::Command(cmd) => {
                            if !matches!(
                                cmd,
                                EngineCommand::Play | EngineCommand::Pause | EngineCommand::Stop
                            ) {
                                let mut state = st.borrow_mut();
                                document_title_needs_refresh |= !state.dirty;
                                state.dirty = true;
                                state.revision = state.revision.wrapping_add(1);
                            }
                            if autodrive_verbose {
                                eprintln!("autodrive cmd: {cmd:?}");
                            }
                            handle.send(cmd);
                            forwarded += 1;
                        }
                        PendingEngineMessage::ResizeBuffers { bpm } => {
                            // Each replacement allocates its ring on this UI
                            // pump thread. The ordered realtime queue then
                            // swaps the ready node at a block boundary.
                            let buffers: Vec<_> = {
                                let state = st.borrow();
                                state
                                    .channels
                                    .iter()
                                    .enumerate()
                                    .flat_map(|(channel, state)| {
                                        state.effects.iter().enumerate().filter_map(
                                            move |(slot, effect)| {
                                                effect.params.buffer().copied().map(|params| {
                                                    (EffectTarget::Channel(channel as u8), slot as u8, params)
                                                })
                                            },
                                        )
                                    })
                                    .chain(state.buses.iter().enumerate().flat_map(|(bus, state)| {
                                        state.effects.iter().enumerate().filter_map(
                                            move |(slot, effect)| {
                                                effect.params.buffer().copied().map(|params| {
                                                    (EffectTarget::Bus(bus as u8), slot as u8, params)
                                                })
                                            },
                                        )
                                    }))
                                    .collect()
                            };
                            for (target, slot, params) in buffers {
                                let _ = handle.replace_buffer(target, slot, params, params, bpm);
                            }
                        }
                        PendingEngineMessage::Structural(cmd) => {
                            // Any structural change is an unsaved edit.
                            {
                                let mut state = st.borrow_mut();
                                document_title_needs_refresh |= !state.dirty;
                                state.dirty = true;
                                state.revision = state.revision.wrapping_add(1);
                            }
                            handle.send_structural(cmd);
                        }
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
                                state.dirty = true;
                                state.revision = state.revision.wrapping_add(1);
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
                        PendingEngineMessage::Telemetry(action) => match action {
                            TelemetryAction::SetEffectSpectrumEnabled {
                                target,
                                slot,
                                enabled,
                            } => handle.set_effect_spectrum_enabled(target, slot, enabled),
                        },
                        PendingEngineMessage::Audio(action) => {
                            let Some(window) = weak.upgrade() else { return };
                            match action {
                                AudioAction::ApplyPersisted(config) => {
                                    if let Some(target) = config.output_target.clone() {
                                        if let Err(error) = handle.set_output_target(Some(target)) {
                                            eprintln!(
                                                "mooloop: could not apply saved output target: {error}"
                                            );
                                        }
                                    }
                                    if let Some(frames) = config.buffer_size {
                                        if let Err(error) = handle.set_buffer_size(frames) {
                                            eprintln!(
                                                "mooloop: could not apply saved buffer size: {error}"
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
                            let st = st.borrow();
                            let length = st.pattern_lengths[st.current_pattern] as u64;
                            let ticks_per_step = (Ppq::DEFAULT.ticks_per_beat() / 4) as u64;
                            let ticks_per_beat = Ppq::DEFAULT.ticks_per_beat() as u64;
                            w.set_current_step(((tick / ticks_per_step) % length) as i32);
                            let position_ticks = if st.song_mode {
                                let song_length = u64::from(st.song_length_ticks());
                                let song_position = tick % song_length;
                                w.set_playlist_position_ticks(song_position as i32);
                                song_position
                            } else {
                                tick % (length * ticks_per_step)
                            };
                            let ticks_per_bar = u64::from(TICKS_PER_BAR);
                            let tick_in_bar = position_ticks % ticks_per_bar;
                            w.set_position_bar((position_ticks / ticks_per_bar) as i32 + 1);
                            w.set_position_beat((tick_in_bar / ticks_per_beat) as i32 + 1);
                            w.set_position_tick((tick_in_bar % ticks_per_beat) as i32);
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
                let selected_channel = st.borrow().selected;
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
                                if row.eq_analyzer_enabled {
                                    let spectrum = handle.effect_spectrum(state.effect_target, slot as u8);
                                    row.eq_spectrum_data = spectrum.as_slice().into();
                                }
                                // A forced return to live leaves no other
                                // trace, so the buffer face reads the count
                                // rather than waiting for an audible cue.
                                let collisions = if row.kind == effect_kind_index(EffectKind::Buffer) {
                                    handle.effect_buffer_collisions(state.effect_target, slot as u8)
                                        as i32
                                } else {
                                    row.buffer_collisions
                                };
                                let collisions_changed = collisions != row.buffer_collisions;
                                if meter_changed || collisions_changed || row.eq_analyzer_enabled {
                                    row.input_left_db = input_left_db;
                                    row.input_right_db = input_right_db;
                                    row.output_left_db = output_left_db;
                                    row.output_right_db = output_right_db;
                                    row.buffer_collisions = collisions;
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
                        .channels
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

// Mirrors MockupItem in ui/mockup.slint - keep the fields in sync.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct MockupSavedItem {
    kind: i32,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    label: String,
    value: f32,
    text: String,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct MockupSavedLayout {
    items: Vec<MockupSavedItem>,
}

impl From<&MockupItem> for MockupSavedItem {
    fn from(item: &MockupItem) -> Self {
        MockupSavedItem {
            kind: item.kind,
            x: item.x,
            y: item.y,
            width: item.width,
            height: item.height,
            label: item.label.to_string(),
            value: item.value,
            text: item.text.to_string(),
        }
    }
}

impl From<MockupSavedItem> for MockupItem {
    fn from(saved: MockupSavedItem) -> Self {
        MockupItem {
            kind: saved.kind,
            x: saved.x,
            y: saved.y,
            width: saved.width,
            height: saved.height,
            label: saved.label.into(),
            value: saved.value,
            text: saved.text.into(),
        }
    }
}

// Default width/height per palette kind, in the same order as
// MockupKinds.names in ui/mockup.slint. Keep in sync with the identical
// table in examples/mockup.rs.
const MOCKUP_PALETTE_SIZES: &[(f32, f32)] = &[
    (220.0, 140.0), // Module frame
    (120.0, 12.0),  // Section label
    (56.0, 80.0),   // Knob
    (22.0, 22.0),   // Mini knob
    (176.0, 22.0),  // Fader (h)
    (32.0, 112.0),  // Fader (v)
    (30.0, 110.0),  // Mixer fader
    (60.0, 24.0),   // Toggle
    (60.0, 24.0),   // Button
    (26.0, 26.0),   // Icon button
    (34.0, 26.0),   // Transport button
    (220.0, 26.0),  // Segmented
    (160.0, 26.0),  // Selector
    (8.0, 8.0),     // LED
    (60.0, 18.0),   // Value readout
    (260.0, 120.0), // Envelope
    (120.0, 10.0),  // Peak meter
    (120.0, 18.0),  // Stereo meter
    (180.0, 120.0), // Sticky note
    (18.0, 18.0),   // Mute button
    (18.0, 18.0),   // Solo button
    (22.0, 22.0),   // Trim knob
    (120.0, 14.0),  // Zoom scrollbar
    (22.0, 120.0),  // Meter scale
    (60.0, 120.0),  // Segmented meter
    (9.0, 9.0),     // Clip indicator
    (22.0, 90.0),   // Channel meter
    (150.0, 26.0),  // Master meter
    (80.0, 10.0),   // Gain reduction meter
    (90.0, 16.0),   // Correlation meter
    (30.0, 22.0),   // Bus picker
    (200.0, 28.0),  // Device header
    (220.0, 268.0), // Effect shell
    (280.0, 268.0), // Device frame
];

/// Opens the drag-and-drop UI mockup tool as a standalone window, shown
/// alongside the main app rather than blocking it. This is the same
/// component `cargo run -p mooloop-ui --example mockup` runs standalone;
/// the developer preferences page gives quick access to it without leaving
/// the running app.
fn open_mockup_window() -> Result<MockupCanvas, slint::PlatformError> {
    let canvas = MockupCanvas::new()?;

    let items: Rc<VecModel<MockupItem>> = Rc::new(VecModel::default());
    canvas.set_items(ModelRc::from(items.clone()));

    {
        let canvas_weak = canvas.as_weak();
        let items = items.clone();
        canvas.on_add_item(move |kind| {
            let canvas = canvas_weak.unwrap();
            let (w, h) = MOCKUP_PALETTE_SIZES
                .get(kind as usize)
                .copied()
                .unwrap_or((120.0, 60.0));
            let count = items.row_count() as f32;
            let x = 40.0 + (count * 24.0) % 900.0;
            let y = 40.0 + (count * 18.0) % 560.0;
            items.push(MockupItem {
                kind,
                x,
                y,
                width: w,
                height: h,
                label: Default::default(),
                value: 0.5,
                text: "Note".into(),
            });
            canvas.set_selected_index(items.row_count() as i32 - 1);
        });
    }

    {
        let canvas_weak = canvas.as_weak();
        let items = items.clone();
        canvas.on_remove_item(move |index| {
            if index >= 0 && (index as usize) < items.row_count() {
                items.remove(index as usize);
            }
            canvas_weak.unwrap().set_selected_index(-1);
        });
    }

    {
        let canvas_weak = canvas.as_weak();
        let items = items.clone();
        canvas.on_save_requested(move |path| {
            let layout = MockupSavedLayout {
                items: items
                    .iter()
                    .map(|item| MockupSavedItem::from(&item))
                    .collect(),
            };
            let canvas = canvas_weak.unwrap();
            let result = toml::to_string_pretty(&layout)
                .map_err(|e| e.to_string())
                .and_then(|s| std::fs::write(path.as_str(), s).map_err(|e| e.to_string()));
            match result {
                Ok(()) => canvas.set_status_text(format!("saved {path}").into()),
                Err(e) => canvas.set_status_text(format!("save failed: {e}").into()),
            }
        });
    }

    {
        let canvas_weak = canvas.as_weak();
        let items = items.clone();
        canvas.on_load_requested(move || {
            let canvas = canvas_weak.unwrap();
            match std::fs::read_to_string("mockup.toml")
                .map_err(|e| e.to_string())
                .and_then(|s| toml::from_str::<MockupSavedLayout>(&s).map_err(|e| e.to_string()))
            {
                Ok(layout) => {
                    items.set_vec(
                        layout
                            .items
                            .into_iter()
                            .map(MockupItem::from)
                            .collect::<Vec<_>>(),
                    );
                    canvas.set_selected_index(-1);
                    canvas.set_status_text("loaded mockup.toml".into());
                }
                Err(e) => canvas.set_status_text(format!("load failed: {e}").into()),
            }
        });
    }

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

fn apply_sample_references(
    channels: &mut [ChannelState],
    references: impl IntoIterator<Item = Option<SampleReference>>,
) {
    for (channel, sample) in channels.iter_mut().zip(references) {
        match sample {
            Some(SampleReference::Builtin { .. } | SampleReference::Empty) => {
                channel.sample_path = None;
                channel.sample_embedded = false;
            }
            Some(SampleReference::File { path, embedded }) => {
                channel.sample_path = Some(path);
                channel.sample_embedded = embedded;
            }
            None => {}
        }
    }
}

fn warning_suffix(count: usize) -> String {
    if count == 0 {
        String::new()
    } else {
        format!(
            " ({count} sample warning{})",
            if count == 1 { "" } else { "s" }
        )
    }
}

fn operation_status(label: &str, path: &Path, warnings: &[AssetWarning]) -> String {
    format!(
        "{label}: {}{}",
        path.display(),
        warning_suffix(warnings.len())
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
    // Queue the complete state first. If the bounded realtime queue is full,
    // leave both the sample slots and visible project untouched.
    if !handle.install_project(Arc::new(project.clone())) {
        return false;
    }
    for index in 0..MAX_CHANNELS {
        let sample = project
            .channels
            .get(index)
            .and_then(|channel| match &channel.setup.source {
                ChannelSource::Sampler(sampler) => {
                    samples.get(index).cloned().flatten().or_else(|| {
                        matches!(sampler.sample, SampleReference::Builtin { .. })
                            .then(|| default_sample.cloned())
                            .flatten()
                    })
                }
                ChannelSource::DrumSynth(_)
                | ChannelSource::MonoSynth(_)
                | ChannelSource::PolySynth(_) => default_sample.cloned(),
            });
        if let Some(sample) = sample {
            handle.load_sample(index, sample);
        } else {
            handle.clear_sample(index);
        }
    }
    state.borrow_mut().replace_project(project, samples, window);
    sync_effect_spectrum_subscriptions(&state.borrow(), handle);
    window.set_playing(false);
    window.set_playlist_position_ticks(0);
    refresh_preset_menus(state, window);
    true
}

fn sync_effect_spectrum_subscriptions(state: &UiState, handle: &EngineHandle) {
    for (channel, setup) in state.channels.iter().enumerate() {
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
    for (bus, setup) in state.buses.iter().enumerate() {
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
        st.channels
            .get(st.selected)
            .map(|channel| channel.kind)
            .unwrap_or(DeviceKind::Sampler)
    };
    let generator_presets = mooloop_project::list_presets(&settings::generator_presets_dir(kind));
    let channel_presets = mooloop_project::list_presets(&settings::channel_presets_dir());
    {
        let mut st = state.borrow_mut();
        st.generator_presets = generator_presets;
        st.channel_presets = channel_presets;
    }
    let st = state.borrow();
    st.sync_generator_preset_menu(window);
    st.sync_channel_preset_menu(window);
}

fn resolve_document(path: &Path) -> Result<ResolvedDocument, String> {
    let mut report = mooloop_project::load_bundle(path).map_err(|error| error.to_string())?;
    let sample_references = match &report.document {
        LoadedDocument::Song(project) => project
            .channels
            .iter()
            .map(|channel| match &channel.setup.source {
                ChannelSource::Sampler(sampler) => Some(sampler.sample.clone()),
                ChannelSource::DrumSynth(_)
                | ChannelSource::MonoSynth(_)
                | ChannelSource::PolySynth(_) => None,
            })
            .collect::<Vec<_>>(),
        LoadedDocument::Kit(kit) => kit
            .channels
            .iter()
            .map(|channel| match &channel.source {
                ChannelSource::Sampler(sampler) => Some(sampler.sample.clone()),
                ChannelSource::DrumSynth(_)
                | ChannelSource::MonoSynth(_)
                | ChannelSource::PolySynth(_) => None,
            })
            .collect(),
        LoadedDocument::Channel(channel) => vec![match &channel.source {
            ChannelSource::Sampler(sampler) => Some(sampler.sample.clone()),
            ChannelSource::DrumSynth(_)
            | ChannelSource::MonoSynth(_)
            | ChannelSource::PolySynth(_) => None,
        }],
        LoadedDocument::Generator(source) => vec![match source {
            ChannelSource::Sampler(sampler) => Some(sampler.sample.clone()),
            ChannelSource::DrumSynth(_)
            | ChannelSource::MonoSynth(_)
            | ChannelSource::PolySynth(_) => None,
        }],
    };
    let mut samples = Vec::with_capacity(sample_references.len());
    for (channel, reference) in sample_references.into_iter().enumerate() {
        match reference {
            None | Some(SampleReference::Builtin { .. } | SampleReference::Empty) => {
                samples.push(None)
            }
            Some(SampleReference::File { path, .. }) if path.is_file() => match decode_wav(&path) {
                Ok(sample) => samples.push(Some(sample)),
                Err(error) => {
                    report.warnings.push(AssetWarning {
                        channel,
                        path,
                        message: error,
                    });
                    samples.push(None);
                }
            },
            Some(SampleReference::File { .. }) => samples.push(None),
        }
    }
    Ok(ResolvedDocument { report, samples })
}

fn zenity_path(mut command: std::process::Command) -> Option<PathBuf> {
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then(|| PathBuf::from(value))
}

fn pick_bundle_via_zenity(title: &str) -> Option<PathBuf> {
    let mut command = std::process::Command::new("zenity");
    command
        .arg("--file-selection")
        .arg("--directory")
        .arg(format!("--title={title}"));
    zenity_path(command)
}

fn pick_song_via_zenity(title: &str) -> Option<PathBuf> {
    let mut command = std::process::Command::new("zenity");
    command
        .arg("--file-selection")
        .arg(format!("--title={title}"))
        .arg("--file-filter=Mooloop songs | *.mooloop manifest.toml");
    zenity_path(command).map(normalize_song_selection)
}

fn normalize_song_selection(path: PathBuf) -> PathBuf {
    let is_legacy_manifest = path
        .file_name()
        .is_some_and(|name| name == mooloop_project::MANIFEST_FILE)
        && path
            .parent()
            .and_then(Path::extension)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mooloop"));
    if is_legacy_manifest {
        path.parent()
            .expect("manifest selection has a parent")
            .into()
    } else {
        path
    }
}

fn pick_save_via_zenity(title: &str, suggested: &str) -> Option<PathBuf> {
    let mut command = std::process::Command::new("zenity");
    command
        .arg("--file-selection")
        .arg("--save")
        .arg("--confirm-overwrite")
        .arg(format!("--title={title}"))
        .arg(format!("--filename={suggested}"));
    zenity_path(command)
}

fn pick_export_via_zenity(extension: &str) -> Option<PathBuf> {
    let mut command = std::process::Command::new("zenity");
    command
        .arg("--file-selection")
        .arg("--save")
        .arg("--confirm-overwrite")
        .arg("--title=Export audio")
        .arg(format!("--filename=mooloop-export.{extension}"))
        .arg(format!("--file-filter=*.{extension}"));
    let mut path = zenity_path(command)?;
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_none_or(|value| !value.eq_ignore_ascii_case(extension))
    {
        path.set_extension(extension);
    }
    Some(path)
}

fn confirm_via_zenity(question: &str) -> bool {
    std::process::Command::new("zenity")
        .arg("--question")
        .arg(format!("--text={question}"))
        .arg("--ok-label=Continue")
        .arg("--cancel-label=Cancel")
        .status()
        .is_ok_and(|status| status.success())
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
    peaks_from_frames(&sample.frames, max_bins)
}

/// Like `waveform_peaks`, but bins only the frames in `[start_frame,
/// end_frame)`. Used to re-derive real detail for whatever range the
/// waveform view is zoomed/scrolled to, rather than just stretching the
/// full-sample overview's fixed bins.
fn waveform_peaks_windowed(
    sample: &SampleData,
    max_bins: usize,
    start_frame: usize,
    end_frame: usize,
) -> Vec<f32> {
    let len = sample.frames.len();
    let start = start_frame.min(len);
    let end = end_frame.clamp(start, len);
    peaks_from_frames(&sample.frames[start..end], max_bins)
}

fn peaks_from_frames(frames: &[[f32; 2]], max_bins: usize) -> Vec<f32> {
    if frames.is_empty() || max_bins == 0 {
        return Vec::new();
    }
    let bins = max_bins.min(frames.len());
    let mut peaks = (0..bins)
        .map(|bin| {
            let start = bin * frames.len() / bins;
            let end = ((bin + 1) * frames.len() / bins).max(start + 1);
            frames[start..end]
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

const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// Nearest note name for a (possibly fractional) MIDI note number. A4 = 69.
fn midi_to_note_name(midi: f64) -> String {
    let rounded = midi.round().clamp(0.0, 127.0) as i64;
    let name = NOTE_NAMES[rounded.rem_euclid(12) as usize];
    let octave = rounded / 12 - 1;
    format!("{name}{octave}")
}

fn midi_to_frequency_hz(midi: f64) -> f32 {
    (440.0 * 2f64.powf((midi - 69.0) / 12.0)) as f32
}

/// The note name and frequency the sampler's root note actually plays at
/// once coarse/fine tuning are applied — the musically meaningful readout
/// for the Coarse/Fine knob pair, since "+3 st / +40 ct" alone doesn't say
/// what pitch that is.
fn tune_label(params: SamplerParams) -> String {
    let midi = f64::from(params.root_note)
        + f64::from(params.tune_semitones)
        + f64::from(params.tune_cents) / 100.0;
    format!(
        "{} · {:.1} Hz",
        midi_to_note_name(midi),
        midi_to_frequency_hz(midi)
    )
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

    #[test]
    fn copied_channel_names_are_readable_and_unique() {
        let mut project = Project::default();
        project.channels[0].setup.channel.name = "Kick".into();
        assert_eq!(copied_channel_name(&project, "Kick"), "Kick copy");

        let mut first_copy = project.channels[0].clone();
        first_copy.setup.channel.name = "Kick copy".into();
        project.channels.push(first_copy);
        assert_eq!(copied_channel_name(&project, "Kick"), "Kick copy 2");
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

    #[test]
    fn channel_assigns_stable_note_ids() {
        let mut channel = ChannelState::new(0);
        let first = channel.create_note(0, 0, DEFAULT_NOTE_DURATION_TICKS, 60);
        let second = channel.create_note(0, TICKS_PER_64TH, DEFAULT_NOTE_DURATION_TICKS, 62);
        assert_ne!(first.id, second.id);
        assert_eq!(channel.notes[0][0].id, first.id);
        assert_eq!(channel.notes[0][1].id, second.id);
    }

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

    #[test]
    fn saved_bundle_sample_paths_replace_external_paths() {
        let mut channel = ChannelState::new(0);
        channel.sample_path = Some(PathBuf::from("/samples/source.wav"));

        apply_sample_references(
            std::slice::from_mut(&mut channel),
            [Some(SampleReference::File {
                path: PathBuf::from("/songs/beat.mooloop-assets/samples/00-source.wav"),
                embedded: true,
            })],
        );

        assert_eq!(
            channel.sample_path,
            Some(PathBuf::from(
                "/songs/beat.mooloop-assets/samples/00-source.wav"
            ))
        );
        assert!(channel.sample_embedded);
    }

    #[test]
    fn song_selection_accepts_new_files_and_legacy_manifests() {
        let file = PathBuf::from("/songs/beat.mooloop");
        assert_eq!(normalize_song_selection(file.clone()), file);

        let legacy_manifest = PathBuf::from("/songs/old.mooloop/manifest.toml");
        assert_eq!(
            normalize_song_selection(legacy_manifest),
            PathBuf::from("/songs/old.mooloop")
        );
    }
}
