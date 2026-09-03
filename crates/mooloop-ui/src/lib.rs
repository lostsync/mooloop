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
mod mockup;
mod settings;

slint::include_modules!();

use meter::MeterBallistics;
use mooloop_core::gain::{db_to_linear, linear_to_db, MIN_DB as METER_FLOOR_DB};
use mooloop_core::log::Level;
use mooloop_core::{log_debug, log_error, log_info, log_warn};
use mooloop_core::{
    compile_bus_graph, snap_bars_to_power_of_two, default_buses, sanitize_route, strip_descriptor, would_create_cycle,
    AutomationLane, AutomationPoint, BufferDuration, BufferEvent, BusSetup, Channel, ChannelSetup,
    ChannelSource, DeviceKind, DrumMode, DrumSynthParams, DrumSynthState, EffectKind, EffectParams,
    insert_effect, move_effect, remove_effect, retarget_lanes, SlotRemap,
    EffectSlotState, EffectTarget, EngineCommand, EngineEvent, EnvTrigger, FilterModel,
    GeneratorParams, GlideMode, HatCharacter,
    KickCharacter, Kit, LfoWave, LoopMode, ModDestinationDescriptor, ModEnvelopeParams,
    ModPolarity, ModRack, ModRandomTrigger, ModRoute, ModStepTrigger,
    ModulatorKind, ModulatorParams, MonoSynthParams, MonoSynthState, MlM1Params, MlM1State,
    ds01, Ds01Params, Ds01State, MlP8Params, MlP8State,
    NoteEvent,
    NoteId, NotePriority, OscWave, ParamAddr,
    ParamCurve, ParamDescriptor, ParamOwner, PatternPlacement, PlaybackMode, PointId,
    PolySynthParams,
    PolySynthState, Ppq, Project, ProjectChannel, RetriggerMode, SampleReference,
    PlayMode, SampleCommit, SamplerParams, SamplerState, SliceMap, SnareCharacter, StretchMode,
    VoiceMode, MAX_SLICES,
    DEFAULT_NOTE_DURATION_TICKS,
    DEFAULT_STEPS, DEFAULT_SWING_PERCENT, MASTER_BUS, MAX_AUTOMATION_LANES_PER_CHANNEL, MAX_BUSES,
    MAX_CHANNELS, MAX_LINEAR_GAIN, MAX_MODULATORS_PER_CHANNEL,
    MAX_MOD_ROUTES_PER_CHANNEL,
    MOD_STEP_MAX_STEPS,
    MAX_SAMPLER_VOICES, MAX_STRETCH_BARS, MAX_STRETCH_GRAIN, MAX_STRETCH_RATIO,
    MIN_STRETCH_BARS, MIN_STRETCH_GRAIN, MIN_STRETCH_RATIO,
    MAX_PATTERNS, MAX_PATTERN_STEPS, MAX_PLAYLIST_BARS, MAX_PLAYLIST_PLACEMENTS,
    MAX_PLAYLIST_TICKS, MAX_POLY_VOICES, MAX_SWING_PERCENT, MIN_SWING_PERCENT, STRIP_DESCRIPTORS,
    TICKS_PER_64TH, TICKS_PER_BAR, TICKS_PER_STEP,
};
use mooloop_dsp::{
    buffer_allocation_key, build_effect_at_tempo,
    sample_analysis::{
        fraction_from_frame, frame_from_fraction, snap_to_zero_crossing, snap_window_frames,
        SnapResult, DEFAULT_SNAP_WINDOW_MS,
    },
    Ds01, DrumSynth, DryAlign, SampleData, SpectrumAnalyzer, StretchPool,
};
use mooloop_engine::{
    EffectSlot, EngineHandle, ExportFormat, ExportSpec, Mp3Bitrate, OfflineRenderer,
    PreviewCommand,
    RenderScope, StructuralCommand, WavEncoding,
};
use mooloop_project::{AssetMode, AssetWarning, Issue, LoadReport, LoadedDocument, PresetInfo, PresetSummary};
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
use mooloop_session::history::Entry as HistoryEntry;
use mooloop_session::notes::ScaleBase;
use mooloop_session::project::{
    fresh_starter_seed, normalize_project_pattern_banks, HistoryMove, ProjectEdit, ProjectSnapshot,
};
use mooloop_session::sample::{
    adjacent_sample, inspect_sample, load_sample_at_path, sample_description, sample_duration,
    sample_files_in_directory, sample_index, tune_label, waveform_peaks, waveform_peaks_windowed,
    LoadResult, LoadedSample, SampleInspection,
};
use mooloop_session::values::{
    format_bars, measured_loop_bars, parse_typed_value, stretch_bars_from_norm,
    stretch_bars_to_norm, stretch_grain_from_norm, stretch_grain_to_norm, stretch_ratio_from_norm,
    stretch_ratio_to_norm,
};
pub use mockup::{load_mockup_layout, wire_mockup};
use settings::{AppearanceSettings, ThemePalette, ThemeScheme, UiSettings};
use slint::{
    CloseRequestResponse, ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode,
    VecModel,
};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

const PUMP_INTERVAL_MS: u64 = 8;
const INITIAL_BPM: i32 = 120;
/// Fader positions for time-based params map onto [0, MAX_TIME_S] seconds.
const MAX_TIME_S: f32 = 2.0;

/// UI callbacks all run on one thread, but boxed structural edits and POD
/// commands used to enter separate relay queues and lose their relative
/// order. These typed senders share one queue while preserving the convenient
/// `.send(...)` call shape used by the callback wiring below.
///
/// The width is `StructuralCommand`'s, and `EngineCommand` (`bridge.rs`
/// documents what sets that) is the runner-up. This queue is drained on the
/// UI thread into the preallocated ring, so evening the variants out with a
/// `Box` would trade a fixed stack copy for an allocation per command and
/// cost the `Copy` the wiring relies on.
#[allow(clippy::large_enum_variant)]
enum PendingEngineMessage {
    Command(EngineCommand),
    ResizeBuffers {
        bpm: f64,
    },
    Structural(StructuralCommand),
    /// Adding a channel allocates its strip, event list and control-output
    /// buffer, so it is structural rather than POD. The pump expands it: the
    /// engine handle owns the sample slot the new strip needs.
    AddChannel {
        channel: usize,
        source: DeviceKind,
    },
    ProjectEdit(ProjectEdit),
    Audio(AudioAction),
    Telemetry(TelemetryAction),
    /// Linear preview gain. A plain value rather than a command because the
    /// engine reads it from a shared cell, live, while a preview plays.
    PreviewGain(f32),
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

    fn add_channel(&self, channel: usize, source: DeviceKind) -> bool {
        self.0
            .send(PendingEngineMessage::AddChannel { channel, source })
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

#[derive(Clone)]
struct PreviewSender(std::sync::mpsc::Sender<PendingEngineMessage>);

impl PreviewSender {
    fn send_gain(&self, gain: f32) -> bool {
        self.0.send(PendingEngineMessage::PreviewGain(gain)).is_ok()
    }
}

/// Fixed JACK buffer size choices offered by the segmented control on the
/// Audio preferences page. Index-addressed to match `SegmentedControl`.
const JACK_BUFFER_SIZES: [u32; 6] = [64, 128, 256, 512, 1024, 2048];
const WAVEFORM_BINS: usize = 256;
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

fn snapshot_channel_clipboard(
    state: &UiState,
    window: &MainWindow,
    index: usize,
) -> Option<ChannelClipboard> {
    let snapshot = project_snapshot(state, window);
    Some(ChannelClipboard {
        channel: snapshot.project.channels.get(index)?.clone(),
        sample: snapshot.samples.get(index)?.clone(),
    })
}

fn project_snapshot(state: &UiState, window: &MainWindow) -> ProjectSnapshot {
    let mut project = state.project_snapshot(window.get_bpm(), window.get_swing_percent());
    normalize_project_pattern_banks(&mut project);
    ProjectSnapshot {
        project,
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

fn selection_including(
    state: &UiState,
    channel: usize,
    pattern: usize,
    anchor: NoteId,
) -> HashSet<NoteId> {
    let live: HashSet<NoteId> = state.channels[channel].notes[pattern]
        .iter()
        .map(|note| note.id)
        .collect();
    if !state.selected_note_ids.contains(&anchor) {
        return HashSet::from([anchor]);
    }
    let mut acting: HashSet<NoteId> = state
        .selected_note_ids
        .intersection(&live)
        .copied()
        .collect();
    acting.insert(anchor);
    acting
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

/// The four sample markers a snap can move. Each one's legal range is
/// derived from the others, so resolving a marker can never invert or
/// collapse the region it belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SampleMarker {
    Start,
    End,
    LoopStart,
    LoopEnd,
}

impl SampleMarker {
    fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::End => "End",
            Self::LoopStart => "Loop start",
            Self::LoopEnd => "Loop end",
        }
    }

    fn get(self, params: &SamplerParams) -> f32 {
        match self {
            Self::Start => params.start,
            Self::End => params.end,
            Self::LoopStart => params.loop_start,
            Self::LoopEnd => params.loop_end,
        }
    }

    fn set(self, params: &mut SamplerParams, value: f32) {
        match self {
            Self::Start => params.start = value,
            Self::End => params.end = value,
            Self::LoopStart => params.loop_start = value,
            Self::LoopEnd => params.loop_end = value,
        }
    }
}

/// Resolve one marker onto a zero crossing, returning the fraction to store
/// and what happened, or `None` when there is no sample to search.
///
/// Runs entirely on the UI thread against decoded frames the control side
/// already owns. Nothing here is reachable from `process()`.
/// Resolve a proposed slice boundary onto a zero crossing.
///
/// Slice boundaries need this more than the trim markers do, not less: a
/// slice cut out of the middle of a break can end at full amplitude, and the
/// voice is deactivated outright when it reaches its end rather than faded.
/// A boundary on a crossing is what keeps that from clicking on every note.
///
/// Penned in by the playback region rather than by neighbouring markers: two
/// slices may sit as close together as the user likes, and the map refuses a
/// collision itself.
fn snap_slice_frame(params: &SamplerParams, sample: &SampleData, frame: usize) -> usize {
    let len = sample.frames.len();
    if len < 2 {
        return frame;
    }
    let last = len - 1;
    let bounds = frame_from_fraction(params.start, len).min(last)
        ..=frame_from_fraction(params.end, len).min(last);
    let window = snap_window_frames(DEFAULT_SNAP_WINDOW_MS, sample.sample_rate);
    snap_to_zero_crossing(&sample.frames, frame.min(last), window, bounds).resolved
}

/// Re-derive everything the editor shows about a channel's audio.
///
/// Committing and reverting swap the published buffer underneath the view,
/// and the waveform, the readout, and the duration all describe that buffer
/// rather than the source. Kept in one place so a commit cannot update two of
/// the three.
fn refresh_sample_view(channel: &mut ChannelState) {
    let Some(sample) = channel.published_sample().cloned() else {
        channel.waveform.clear();
        channel.sample_description.clear();
        channel.sample_duration = 0.0;
        return;
    };
    channel.waveform = waveform_peaks(&sample, WAVEFORM_BINS);
    channel.sample_description = sample_description(&sample);
    channel.sample_duration = sample_duration(&sample);
}

/// A channel's audio, on its way to the pump.
///
/// Neither half can ride the command ring: `EngineCommand` is `Copy` and
/// unboxed by design, and both of these live in `ArcSwap` slots the pump
/// exclusively owns. Same route the built-in sample reset already takes.
///
/// Both are always sent together because they are one fact: after a commit
/// the published buffer and the map that indexes it change at the same
/// instant, and delivering one without the other would leave the voice
/// reading markers that name frames in a buffer it no longer holds.
struct ChannelAudio {
    channel: usize,
    sample: Option<Arc<SampleData>>,
    slices: Option<Arc<SliceMap>>,
}

#[derive(Clone)]
struct ChannelAudioSender(std::sync::mpsc::Sender<ChannelAudio>);

fn publish_channel_audio_to(tx: &ChannelAudioSender, channel: usize, state: &ChannelState) {
    let _ = tx.0.send(ChannelAudio {
        channel,
        sample: state.published_sample().cloned(),
        slices: (!state.slices.is_empty()).then(|| Arc::new(state.slices.clone())),
    });
}

/// Turn a normalized position from the face into a frame of the published
/// buffer, snapped to a zero crossing when AUTO is on.
fn resolve_slice_frame(channel: &ChannelState, position: f32, snap: bool) -> Option<u32> {
    let sample = channel.published_sample()?;
    let len = sample.frames.len();
    if len == 0 {
        return None;
    }
    let frame = frame_from_fraction(position.clamp(0.0, 1.0), len);
    let frame = if snap {
        snap_slice_frame(&channel.params, sample, frame)
    } else {
        frame
    };
    Some(frame as u32)
}

/// Whether a commit no longer describes what the stretch controls say.
///
/// Two ways to drift. A bar-synced commit goes stale when the project tempo
/// moves, because the ratio it baked was derived from that tempo; that is
/// re-derived through the sampler's own `effective_ratio` rather than by
/// storing the tempo, so this asks the same question the commit answered.
/// Any commit goes stale when the mode, the free speed, or -- in `Grain` --
/// the grain size is edited after the bake: those knobs still read as the
/// patch's stretch settings while the audio was rendered under other ones.
/// Reported and offered a re-bake, never acted on by itself: re-rendering a
/// loop under someone without being asked is worse than telling them.
fn commit_is_stale(channel: &ChannelState, commit: &SampleCommit, bpm: f64) -> bool {
    let params = channel.params;
    if commit.mode != params.stretch_mode {
        return true;
    }
    if params.stretch_mode == StretchMode::Grain && commit.grain != params.stretch_grain {
        return true;
    }
    if !params.stretch_sync {
        return (commit.ratio - params.stretch_ratio).abs() > 1.0e-3;
    }
    let Some(source) = channel.sample_data.as_ref() else {
        return false;
    };
    let params = SamplerParams {
        start: commit.source_start,
        end: commit.source_end,
        loop_start: commit.source_loop_start,
        loop_end: commit.source_loop_end,
        ..channel.params
    };
    let now = mooloop_dsp::Sampler::effective_ratio(
        params,
        source.frames.len(),
        source.sample_rate,
        bpm,
        1.0,
    );
    (now - f64::from(commit.ratio)).abs() > 1.0e-3
}

/// The slice boundaries of the selected channel, as fractions for the face.
fn slice_fractions(channel: &ChannelState) -> Vec<f32> {
    let len = channel
        .published_sample()
        .map_or(0, |sample| sample.frames.len());
    if len == 0 {
        return Vec::new();
    }
    channel
        .slices
        .markers()
        .iter()
        .map(|marker| fraction_from_frame(marker.frame as usize, len))
        .collect()
}

fn snap_marker(
    params: &SamplerParams,
    sample: &SampleData,
    marker: SampleMarker,
    requested: f32,
) -> Option<(f32, SnapResult)> {
    let len = sample.frames.len();
    if len < 2 {
        return None;
    }
    let last = len - 1;
    let frame = |fraction: f32| frame_from_fraction(fraction, len);
    // Each marker is penned in by its neighbours. `saturating_sub` and the
    // `min(last)` guards keep a degenerate region (an empty or inverted one
    // already in the params) from producing a reversed range.
    let bounds = match marker {
        SampleMarker::Start => 0..=frame(params.end).saturating_sub(1),
        SampleMarker::End => (frame(params.start) + 1).min(last)..=last,
        SampleMarker::LoopStart => frame(params.start)..=frame(params.loop_end).saturating_sub(1),
        SampleMarker::LoopEnd => (frame(params.loop_start) + 1).min(last)..=frame(params.end),
    };
    let window = snap_window_frames(DEFAULT_SNAP_WINDOW_MS, sample.sample_rate);
    let result = snap_to_zero_crossing(&sample.frames, frame(requested), window, bounds);
    Some((fraction_from_frame(result.resolved, len), result))
}

/// What to tell the user about a snap. Silence would make a marker that moved
/// look like drift, and a marker that did not move look like a broken button.
fn snap_status(marker: SampleMarker, result: SnapResult) -> String {
    if result.moved() {
        format!(
            "{} snapped to frame {} ({:+} frames)",
            marker.label(),
            result.resolved,
            result.offset()
        )
    } else {
        format!(
            "{} kept at frame {}: no zero crossing within {} ms",
            marker.label(),
            result.requested,
            DEFAULT_SNAP_WINDOW_MS as i32
        )
    }
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
fn descriptor_slots(descriptors: &[ParamDescriptor]) -> usize {
    descriptors
        .iter()
        .map(|descriptor| descriptor.id as usize + 1)
        .max()
        .unwrap_or(0)
}

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
    /// Slice boundaries of the selected channel, normalized against the
    /// published buffer so they ride the same `to-view` zoom the waveform and
    /// every other marker already go through.
    slice_model: Rc<VecModel<f32>>,
    /// The channel and note of the slice a handle is currently holding down,
    /// so its release goes to exactly the note that was struck. Kept rather
    /// than re-derived from the handle's index on the way up: a drag past a
    /// neighbour reorders the map underneath the handle, and the index it
    /// releases with is then a different slice's.
    slice_audition: Option<(u8, u8)>,
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
    modulation_shelf_open: bool,
    /// Source whose editor is open in the shelf. Selection is intentionally
    /// separate from assignment: looking at an LFO must not hijack knob
    /// gestures throughout the rack.
    modulation_selected_slot: Cell<Option<u8>>,
    modulation_armed_slot: Cell<Option<u8>>,
    /// The selected channel's latest modulator outputs, refreshed from the
    /// engine on the pump tick. Held here rather than recomputed per knob
    /// so one read of the audio thread's cells feeds every destination.
    modulation_outputs: Cell<[f32; MAX_MODULATORS_PER_CHANNEL]>,
    /// Channel that owns the transient selection/assignment state. Changing
    /// channels clears both even when the new channel happens to occupy the
    /// same runtime slot.
    modulation_ui_channel: Cell<Option<usize>>,
    /// Snapshot captured at the start of a direct knob gesture. Intermediate
    /// control updates still reach audio immediately, while one release
    /// becomes one undoable route edit.
    modulation_edit_before: Option<ProjectSnapshot>,
    modulation_edit_changed: bool,
    mixer_strip_model: Rc<VecModel<MixerStripRow>>,
    /// Flattened sample-browser tree, rebuilt whenever locations or folder
    /// expansion change.
    browser_rows: Rc<VecModel<BrowserRow>>,
    /// Sample-browser folders in display order, mirroring the persisted
    /// settings for this session.
    browser_locations: Vec<PathBuf>,
    /// Folders currently expanded, by path, so a refresh survives reordering.
    browser_expanded: HashSet<PathBuf>,
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
    /// The selection a marquee started from, plus how it should combine with
    /// what the band catches. `None` when no band is in flight.
    marquee_base: Option<(i32, HashSet<NoteId>)>,
    /// The selection's geometry when a scale drag started, plus the tick it
    /// scales about. Every frame is applied to this rather than to the live
    /// notes, so repeated scaling does not compound its own rounding.
    scale_base: Option<ScaleBase>,
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
            DeviceKind::MlM1 => format!("ML-M1 {}", index + 1),
            DeviceKind::MlP8 => format!("ML-P8 {}", index + 1),
            DeviceKind::Ds01 => format!("DS-01 {}", index + 1),
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
                channel.committed_sample = None;
                channel.commit = None;
                channel.slices.clear();
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
                channel.committed_sample = None;
                channel.commit = None;
                channel.slices.clear();
                channel.waveform.clear();
                channel.can_previous_sample = false;
                channel.can_next_sample = false;
            }
            DeviceKind::MlM1 => {
                channel.mlm1_params = MlM1Params::default();
                channel.sample_name.clear();
                channel.sample_description.clear();
                channel.sample_duration = 0.0;
                channel.sample_path = None;
                channel.sample_embedded = false;
                channel.sample_data = None;
                channel.committed_sample = None;
                channel.commit = None;
                channel.slices.clear();
                channel.waveform.clear();
                channel.can_previous_sample = false;
                channel.can_next_sample = false;
            }
            DeviceKind::Ds01 => {
                channel.ds01_params = Ds01Params::default();
                channel.sample_name.clear();
                channel.sample_description.clear();
                channel.sample_duration = 0.0;
                channel.sample_path = None;
                channel.sample_embedded = false;
                channel.sample_data = None;
                channel.committed_sample = None;
                channel.commit = None;
                channel.slices.clear();
                channel.waveform.clear();
                channel.can_previous_sample = false;
                channel.can_next_sample = false;
            }
            DeviceKind::MlP8 => {
                channel.mlp8_params = MlP8Params::default();
                channel.sample_name.clear();
                channel.sample_description.clear();
                channel.sample_duration = 0.0;
                channel.sample_path = None;
                channel.sample_embedded = false;
                channel.sample_data = None;
                channel.committed_sample = None;
                channel.commit = None;
                channel.slices.clear();
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
                channel.committed_sample = None;
                channel.commit = None;
                channel.slices.clear();
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
                channel.committed_sample = None;
                channel.commit = None;
                channel.slices.clear();
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
                            slices: channel.slices.clone(),
                            commit: channel.commit.clone(),
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
                    DeviceKind::MlM1 => ChannelSource::MlM1(MlM1State {
                        params: channel.mlm1_params,
                    }),
                    DeviceKind::MlP8 => ChannelSource::MlP8(MlP8State {
                        params: channel.mlp8_params,
                    }),
                    DeviceKind::Ds01 => ChannelSource::Ds01(Ds01State {
                        params: channel.ds01_params,
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
                // One accessor a kind rather than one tuple arm a kind: the
                // shape was a five-tuple whose every arm restated the four
                // defaults it was not, which is a line of edit per synth per
                // synth added.
                let source = &setup.source;
                let sampler = source.sampler_state();
                let drum_params = source.drum_synth_state().map(|s| s.params).unwrap_or_default();
                let mono_params = source.mono_synth_state().map(|s| s.params).unwrap_or_default();
                let poly_params = source.poly_synth_state().map(|s| s.params).unwrap_or_default();
                let mlm1_params = source.mlm1_state().map(|s| s.params).unwrap_or_default();
                let mlp8_params = source.mlp8_state().map(|s| s.params).unwrap_or_default();
                let ds01_params = source.ds01_state().map(|s| s.params).unwrap_or_default();
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
                // A committed stretch is re-rendered rather than reloaded:
                // the spec is length-determined, so the buffer that comes
                // back is the one that was baked, and the project never had
                // to carry the audio.
                //
                // Only when it has to, though. Undo and every other project
                // edit reinstall the whole document through here, and a
                // commit is a couple of hundred milliseconds of rendering per
                // channel -- paid on the UI thread, so it is a visible stall.
                // A buffer already in hand, baked from the same source under
                // the same spec, is the same buffer.
                let commit = sampler.and_then(|state| state.commit.clone());
                let committed = commit.as_ref().zip(sample.as_ref()).and_then(
                    |(commit, source)| {
                        let held = self.channels.get(index).filter(|held| {
                            held.commit.as_ref() == Some(commit)
                                && held
                                    .sample_data
                                    .as_ref()
                                    .is_some_and(|held| Arc::ptr_eq(held, source))
                        });
                        held.and_then(|held| held.committed_sample.clone())
                            .or_else(|| mooloop_dsp::commit::rerender_commit(source, commit))
                    },
                );
                let published = committed.as_ref().or(sample.as_ref());
                let waveform = published
                    .map(|sample| waveform_peaks(sample, WAVEFORM_BINS))
                    .unwrap_or_else(|| {
                        if is_builtin {
                            self.default_waveform.clone()
                        } else {
                            Vec::new()
                        }
                    });
                let description = published
                    .map(|sample| sample_description(sample))
                    .unwrap_or_else(|| {
                        if missing {
                            "Missing sample - load an audio file to relink".into()
                        } else if is_builtin {
                            self.default_sample_description.clone()
                        } else {
                            String::new()
                        }
                    });
                let duration = published
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
                    .and_then(|path| {
                        sample_files_in_directory(path)
                            .ok()
                            .map(|files| (path, files))
                    })
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
                    mlm1_params,
                    mlp8_params,
                    ds01_params,
                    sample_name,
                    sample_description: description,
                    sample_duration: duration,
                    sample_path,
                    sample_embedded: embedded,
                    sample_data: sample,
                    committed_sample: committed,
                    commit,
                    slices: sampler.map(|state| state.slices.clone()).unwrap_or_default(),
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
        // Modulation source selection and assignment are session gestures,
        // never document state. A newly loaded project must start unarmed
        // even if it selects the same channel index as the previous one.
        self.modulation_ui_channel.set(None);
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
            // The generator first: it is the top of the signal path, and it is
            // what most channels have instead of an effect chain.
            let generator = state.generator_params();
            let device = state.name.clone();
            for descriptor in generator.kind().descriptors() {
                rows.push((
                    ParamAddr {
                        scope: channel,
                        owner: ParamOwner::Source,
                        param: descriptor.id,
                    },
                    device.clone(),
                    descriptor,
                ));
            }
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
                        ParamAddr::effect(
                            EffectTarget::Bus(index as u8),
                            slot as u8,
                            descriptor.id,
                        ),
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
        match target.owner {
            ParamOwner::Source => {
                let EffectTarget::Channel(channel) = target.scope else {
                    return None;
                };
                self.channels
                    .get(channel as usize)?
                    .generator_params()
                    .kind()
                    .descriptor(target.param)
            }
            // A route amount is not in the device's table; its descriptor
            // belongs to the route.
            ParamOwner::SourceRoute { .. } => {
                let EffectTarget::Channel(channel) = target.scope else {
                    return None;
                };
                self.channels
                    .get(channel as usize)?
                    .generator_params()
                    .kind()
                    .route_descriptor(target.param)
            }
            ParamOwner::Effect { slot } => {
                let effects = match target.scope {
                    EffectTarget::Channel(channel) => &self.channels.get(channel as usize)?.effects,
                    EffectTarget::Bus(bus) => &self.buses.get(bus as usize)?.effects,
                };
                effects.get(slot as usize)?.kind().descriptor(target.param)
            }
            ParamOwner::Modulator { .. } | ParamOwner::Strip => None,
        }
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

    /// The selection's bounding box in ticks and MIDI notes, which is what
    /// the grid draws its selection frame and scale handles from.
    ///
    /// Published as plain properties rather than answered by a callback: the
    /// frame's geometry is a binding, and a callback there would be re-run
    /// on every layout pass rather than when the selection actually changes.
    fn refresh_selection_bounds(&self, window: &MainWindow) {
        let Some(channel) = self.channels.get(self.selected) else {
            return;
        };
        let mut count = 0;
        let (mut start, mut end) = (u32::MAX, 0u32);
        let (mut low, mut high) = (u8::MAX, 0u8);
        for note in channel.notes[self.current_pattern]
            .iter()
            .filter(|note| self.selected_note_ids.contains(&note.id))
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
        let mut lengths = channel.notes[self.current_pattern]
            .iter()
            .filter(|note| self.selected_note_ids.contains(&note.id))
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
        window.set_has_note_selection(!self.selected_note_ids.is_empty());
        self.refresh_selection_bounds(window);
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
    /// Drops one note from the selection, leaving the rest alone. The
    /// subtract-from-selection role needs this to be idempotent: dragging a
    /// remove-marquee back and forth over a note must not re-add it, which a
    /// toggle would.
    fn remove_note_from_selection(&mut self, id: NoteId) {
        self.selected_note_ids.remove(&id);
        self.selected_note_id = (self.selected_note_ids.len() == 1)
            .then(|| *self.selected_note_ids.iter().next().unwrap());
    }

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

    /// Run one chain edit's permutation over everything on this side that
    /// names a slot in `target`'s chain: the channel's routes, every lane in
    /// every pattern, and the lane the editor is showing. The engine runs the
    /// same table for the same command, which is what keeps a route meaning
    /// the same knob on both sides after the rack is reordered.
    fn retarget_effect_slots(&mut self, target: EffectTarget, remap: &SlotRemap) {
        let channels: &mut [ChannelState] = match target {
            EffectTarget::Channel(channel) => match self.channels.get_mut(channel as usize) {
                Some(channel) => std::slice::from_mut(channel),
                None => &mut [],
            },
            // A bus chain can be automated from any channel's clip.
            EffectTarget::Bus(_) => &mut self.channels,
        };
        for channel in channels {
            channel.modulation.retarget_effect_slots(target, remap);
            for lanes in &mut channel.automation {
                retarget_lanes(lanes, target, remap);
            }
        }
        self.automation_target.set(
            self.automation_target
                .get()
                .and_then(|shown| remap.address(target, shown)),
        );
    }

    /// Resolve every tempo-synced delay to the new transport BPM. The engine
    /// remains millisecond-only: the resulting values take its normal
    /// sample-timed parameter path, so all delays move at the next block
    /// without allocating or rebuilding their rings.
    fn update_tempo_synced_delay_times(&mut self, bpm: f64) -> Vec<(EffectTarget, u8, f32)> {
        let mut changes = Vec::new();
        for (channel, state) in self.channels.iter_mut().enumerate() {
            for (slot, effect) in state.effects.iter_mut().enumerate() {
                let EffectParams::Delay(params) = &mut effect.params else {
                    continue;
                };
                if params.tempo_sync {
                    params.time_ms = params.time_division.time_ms(bpm);
                    changes.push((
                        EffectTarget::Channel(channel as u8),
                        slot as u8,
                        params.time_ms,
                    ));
                }
            }
        }
        for (bus, state) in self.buses.iter_mut().enumerate() {
            for (slot, effect) in state.effects.iter_mut().enumerate() {
                let EffectParams::Delay(params) = &mut effect.params else {
                    continue;
                };
                if params.tempo_sync {
                    params.time_ms = params.time_division.time_ms(bpm);
                    changes.push((EffectTarget::Bus(bus as u8), slot as u8, params.time_ms));
                }
            }
        }
        self.sync_effects();
        changes
    }

    /// Rebuild the edited chain's rows. The model itself is installed on the
    /// window once; this refreshes its contents after structural changes
    /// (add/remove/reorder) and after the rack is pointed somewhere else.
    fn sync_effects(&self) {
        let armed = self.modulation_armed_slot.get();
        let rows: Vec<EffectSlotRow> = match self.effect_target {
            // Modulation state belongs to the selected channel, so an insert
            // rack pointed at a bus -- or at another channel -- renders its
            // rows without overlays rather than borrowing this channel's.
            EffectTarget::Channel(channel) if channel as usize == self.selected => self
                .channels
                .get(channel as usize)
                .map(|state| {
                    state
                        .effects
                        .iter()
                        .enumerate()
                        .map(|(slot, effect)| {
                            let mut row = effect_slot_row(effect);
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
                .effect_chain()
                .map(|effects| effects.iter().map(effect_slot_row).collect())
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
        let mut depths = vec![0.0; descriptor_slots(descriptors)];
        for descriptor in descriptors {
            depths[descriptor.id as usize] = armed.map_or(0.0, |slot| {
                self.modulation_depth_for(slot, address(descriptor.id))
            });
        }
        depths.as_slice().into()
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
        let mut offsets = vec![0.0; descriptor_slots(descriptors)];
        let Some(channel) = self.channels.get(self.selected) else {
            return offsets.as_slice().into();
        };
        let outputs = self.modulation_outputs.get();
        for descriptor in descriptors {
            let policy = ModDestinationDescriptor::for_param(descriptor);
            offsets[descriptor.id as usize] =
                channel
                    .modulation
                    .offset_for(address(descriptor.id), &outputs, &policy);
        }
        offsets.as_slice().into()
    }

    fn modulation_depth_for(&self, source_slot: u8, destination: ParamAddr) -> f32 {
        self.channels
            .get(self.selected)
            .and_then(|channel| {
                channel.modulation.routes.iter().flatten().find(|route| {
                    route.source_slot == source_slot && route.destination == destination
                })
            })
            .map_or(0.0, |route| route.depth)
    }

    fn modulation_envelope_mut(&mut self, slot: usize) -> Option<&mut ModEnvelopeParams> {
        let selected = self.selected;
        let params = self
            .channels
            .get_mut(selected)?
            .modulation
            .params_mut(slot)?;
        match params {
            ModulatorParams::Envelope(envelope) => Some(envelope),
            _ => None,
        }
    }

    /// The modulation shelf may address only the selected channel's own
    /// generator, inserts, and strip. Buses and another channel's controls
    /// stay deliberately outside this pass even though `ParamAddr` can name
    /// them, matching the per-channel routing policy.
    fn channel_modulation_destination(
        &self,
        address: ParamAddr,
    ) -> Option<(String, &'static ParamDescriptor)> {
        let EffectTarget::Channel(channel) = address.scope else {
            return None;
        };
        if channel as usize != self.selected {
            return None;
        }
        let state = self.channels.get(self.selected)?;
        match address.owner {
            ParamOwner::Source => state
                .generator_params()
                .kind()
                .descriptor(address.param)
                .map(|descriptor| (state.name.clone(), descriptor)),
            ParamOwner::Effect { slot } => state
                .effects
                .get(slot as usize)
                .and_then(|effect| effect.kind().descriptor(address.param))
                .map(|descriptor| {
                    (
                        format!(
                            "{} {}",
                            state.effects[slot as usize].kind().label(),
                            slot + 1
                        ),
                        descriptor,
                    )
                }),
            ParamOwner::Strip => strip_descriptor(address.param)
                .map(|descriptor| ("Channel strip".to_string(), descriptor)),
            // Modulators are sources in this first UI pass, not destinations.
            // An instrument's own routes are not channel destinations either:
            // the shelf reaches a device's controls, and a route amount
            // belongs to the patch's internal modulation rather than to the
            // device's control surface.
            ParamOwner::Modulator { .. } | ParamOwner::SourceRoute { .. } => None,
        }
    }

    /// Push the current live modulation offsets onto the generator face and
    /// the effect rows. Called on the pump tick, so it touches only the
    /// offsets: rebuilding the rows here would fight the meter and spectrum
    /// updates landing on the same models.
    fn refresh_modulation_offsets(&self, window: &MainWindow) {
        let scope = EffectTarget::Channel(self.selected as u8);
        let Some(channel) = self.channels.get(self.selected) else {
            return;
        };
        // Each grid tile's meter, touched in place: rebuilding the source
        // rows on the pump tick would fight selection and the add menu for
        // the same reason the effect rows are updated field-wise here.
        let outputs = self.modulation_outputs.get();
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
        if self.effect_target != scope {
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
        if self.modulation_ui_channel.get() != Some(self.selected) {
            self.modulation_ui_channel.set(Some(self.selected));
            self.modulation_selected_slot.set(None);
            self.modulation_armed_slot.set(None);
        }
        let Some(channel) = self.channels.get(self.selected) else {
            self.modulation_source_model.set_vec(Vec::new());
            self.modulation_route_model.set_vec(Vec::new());
            self.modulation_selected_slot.set(None);
            self.modulation_armed_slot.set(None);
            window.set_modulation_selected_slot(-1);
            window.set_modulation_armed_slot(-1);
            return;
        };

        let selected = self.modulation_selected_slot.get().filter(|slot| {
            channel
                .modulation
                .slots
                .get(*slot as usize)
                .is_some_and(Option::is_some)
        });
        let armed = self.modulation_armed_slot.get().filter(|slot| {
            channel
                .modulation
                .slots
                .get(*slot as usize)
                .is_some_and(Option::is_some)
        });
        self.modulation_selected_slot.set(selected);
        self.modulation_armed_slot.set(armed);
        let bpm = f64::from(window.get_bpm().max(1));
        let outputs = self.modulation_outputs.get();
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
                    .channel_modulation_destination(route.destination)
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
        window.set_modulation_shelf_open(self.modulation_shelf_open);
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
            .channels
            .iter()
            .enumerate()
            .map(|(index, channel)| format!("{} · {}", index + 1, channel.name).into())
            .collect();
        window
            .set_modulation_input_channels(ModelRc::from(Rc::new(VecModel::from(input_channels))));
        window.set_modulation_selected_envelope_input_channel(
            selected_envelope.map_or(self.selected as i32, |env| i32::from(env.input_channel)),
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
        let scope = EffectTarget::Channel(self.selected as u8);
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

    /// Mirror one modulation edit to the engine, persist the UI-owned base
    /// state, then re-render the shelf.
    ///
    /// The command names one fact — a parameter, a slot, a route — rather
    /// than the rack it lives in, so the preallocated command ring is sized
    /// by the widest single module instead of by modulator capacity
    /// (`docs/plans/archive/modulator-capacity/03-per-slot-commands.md`). The channel
    /// index is supplied here rather than by the caller so no gesture can
    /// address a channel other than the one it just edited.
    fn send_modulation(
        &mut self,
        window: &MainWindow,
        tx: &EngineCommandSender,
        command: impl FnOnce(u8) -> EngineCommand,
    ) {
        let channel = self.selected;
        if self.channels.get(channel).is_none() {
            return;
        }
        let _ = tx.send(command(channel as u8));
        self.dirty = true;
        self.revision = self.revision.wrapping_add(1);
        self.update_document_title(window);
        self.refresh_modulation(window);
    }

    /// Ship one slot's module entire, identity included. For the edits that
    /// are not a descriptor parameter — filling an empty slot, or repatching
    /// the envelope's gate jack — where there is no id to name the change by.
    fn send_modulator_slot(
        &mut self,
        window: &MainWindow,
        tx: &EngineCommandSender,
        slot: usize,
    ) {
        let Some(rack) = self.channels.get(self.selected).map(|channel| channel.modulation) else {
            return;
        };
        let (Some(source), Some(params)) = (rack.source_id(slot), rack.params(slot)) else {
            return;
        };
        self.send_modulation(window, tx, |channel| EngineCommand::InstallModulator {
            channel,
            slot: slot as u8,
            source,
            params,
        });
    }

    fn begin_modulation_edit(&mut self, window: &MainWindow) {
        if self.modulation_edit_before.is_none() {
            self.modulation_edit_before = Some(project_snapshot(self, window));
            self.modulation_edit_changed = false;
        }
    }

    fn finish_modulation_edit(&mut self) -> Option<ProjectSnapshot> {
        let before = self.modulation_edit_before.take();
        let changed = std::mem::replace(&mut self.modulation_edit_changed, false);
        if changed {
            before
        } else {
            None
        }
    }

    /// Retune (or first create) the armed source's one explicit route. The
    /// base parameter is deliberately absent from this mutation: a normal
    /// knob drag in armed mode moves only the depth, and the renderer keeps
    /// resolving the same authored base underneath it.
    fn set_armed_modulation_depth(
        &mut self,
        window: &MainWindow,
        tx: &EngineCommandSender,
        destination: ParamAddr,
        depth: f32,
    ) -> bool {
        let Some(source_slot) = self.modulation_armed_slot.get() else {
            return false;
        };
        let Some((_, descriptor)) = self.channel_modulation_destination(destination) else {
            return false;
        };
        let policy = ModDestinationDescriptor::for_param(descriptor);
        if !policy.allowed {
            return false;
        }
        let depth = policy.clamp_depth(depth);
        let Some(channel) = self.channels.get_mut(self.selected) else {
            return false;
        };
        let default_polarity = match channel.modulation.params(source_slot as usize) {
            // Sources that only ever swing one way default to a unipolar
            // route, so their resting value is the destination's base.
            Some(ModulatorParams::Envelope(_)) => ModPolarity::Unipolar,
            Some(ModulatorParams::Random(random)) if !random.bipolar => ModPolarity::Unipolar,
            _ => policy.default_polarity,
        };
        let current = channel
            .modulation
            .routes
            .iter()
            .flatten()
            .find(|route| route.source_slot == source_slot && route.destination == destination)
            .map(|route| route.depth);
        if current.is_some_and(|current| (current - depth).abs() < f32::EPSILON) {
            return false;
        }
        let Some(index) = channel.modulation.add_route(ModRoute::to_slot(
            source_slot,
            destination,
            depth,
            default_polarity,
        )) else {
            // The armed slot was checked above, so the only way the rack
            // refuses is a full matrix. Say so: an assignment gesture that
            // does nothing at all reads as a broken knob.
            window.set_status_message(
                format!(
                    "This channel already has its {MAX_MOD_ROUTES_PER_CHANNEL} modulation \
                     assignments; remove one to add another"
                )
                .as_str()
                .into(),
            );
            return false;
        };
        // The rack stamped the durable source id on the way in; that stamped
        // row is what travels, so the engine resolves the route against the
        // module the gesture meant rather than against a slot number.
        let Some(route) = channel.modulation.routes[index] else {
            return false;
        };
        self.modulation_edit_changed = true;
        self.send_modulation(window, tx, |channel| EngineCommand::SetModRoute {
            channel,
            route,
        });
        true
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
            channels: vec![first],
            rows: rows_model,
            step_models: vec![step_model],
            note_model,
            playlist_model,
            waveform_model,
            slice_model,
            slice_audition: None,
            playhead_model,
            effect_slot_model,
            modulation_source_model,
            modulation_route_model,
            modulation_shelf_open: false,
            modulation_selected_slot: Cell::new(None),
            modulation_armed_slot: Cell::new(None),
            modulation_outputs: Cell::new([0.0; MAX_MODULATORS_PER_CHANNEL]),
            modulation_ui_channel: Cell::new(None),
            modulation_edit_before: None,
            modulation_edit_changed: false,
            mixer_strip_model,
            browser_rows: browser_row_model,
            browser_locations: Vec::new(),
            browser_expanded: HashSet::new(),
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
            marquee_base: None,
            scale_base: None,
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
        let mockup_window: Rc<RefCell<Option<MockupCanvas>>> = Rc::new(RefCell::new(None));
        {
            let settings = ui_settings.borrow();
            apply_appearance(&window, &settings.appearance);
            sync_preferences_properties(&window, &settings);
            audio_tx.send(AudioAction::ApplyPersisted(settings.audio.engine_config()));
            // The browser opens with every top-level location expanded: the
            // point of the sidebar is seeing samples without extra clicks.
            let mut st = state.borrow_mut();
            st.browser_locations = settings.browser.locations.clone();
            st.browser_expanded = st.browser_locations.iter().cloned().collect();
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
            let st = state.clone();
            let weak = window.as_weak();
            window.on_bpm_changed(move |bpm| {
                let bpm = bpm as f64;
                // Preserve stream order: the transport adopts the tempo
                // first, then every synced delay receives its resolved ms
                // value before any beat-relative buffer replacement.
                let _ = tx.send(EngineCommand::SetTempo(bpm));
                let changes = {
                    let mut state = st.borrow_mut();
                    let changes = state.update_tempo_synced_delay_times(bpm);
                    state.dirty = true;
                    state.revision = state.revision.wrapping_add(1);
                    changes
                };
                for (target, slot, time_ms) in changes {
                    let _ = tx.send(EngineCommand::SetEffectParam {
                        target,
                        slot,
                        id: mooloop_core::DELAY_PARAM_TIME_MS,
                        value: time_ms,
                    });
                }
                let _ = tx.resize_buffers(bpm);
                if let Some(window) = weak.upgrade() {
                    let st = st.borrow();
                    st.update_document_title(&window);
                    // A bar-synced bake was measured against the tempo, so
                    // its stale badge follows the tempo rather than waiting
                    // for the next full editor refresh.
                    if let Some(channel) = st.channels.get(st.selected) {
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
                let count = st.borrow().pattern_lengths.len();
                if p < 0 || p as usize >= count {
                    return;
                }
                let p = p as usize;
                log_debug!("ui", "pattern {p} selected");
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
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_add_pattern_clicked(move || {
                if commands.borrow().project_edit_pending {
                    return;
                }
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
                let pattern = st.current_pattern;
                let channel = st.selected;
                let length_ticks = st.pattern_lengths[pattern] as u32 * TICKS_PER_STEP;
                let selected = st.selected_note_ids.clone();
                let mut edited = Vec::new();
                for note in st.channels[channel].notes[pattern]
                    .iter_mut()
                    .filter(|note| selected.contains(&note.id))
                {
                    note.duration_ticks =
                        ticks.min(length_ticks.saturating_sub(note.start_tick).max(1));
                    edited.push(*note);
                }
                if edited.is_empty() {
                    return;
                }
                for note in &edited {
                    st.refresh_rack_cell(channel, (note.start_tick / TICKS_PER_STEP) as usize);
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
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                let channel = st.selected;
                let length_ticks = st.pattern_lengths[pattern] as u32 * TICKS_PER_STEP;
                let moving: HashSet<NoteId> = st.selected_note_ids.clone();
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
                let tick_delta =
                    (tick_delta as i64).clamp(-min_tick, (last_start - max_tick).max(-min_tick));
                let note_delta = note_delta.clamp(-min_note, (127 - max_note).max(-min_note));
                if tick_delta == 0 && note_delta == 0 {
                    return;
                }

                let mut edited = Vec::new();
                let mut touched_steps = Vec::new();
                for note in st.channels[channel].notes[pattern]
                    .iter_mut()
                    .filter(|note| moving.contains(&note.id))
                {
                    touched_steps.push(note.start_tick / TICKS_PER_STEP);
                    note.start_tick = (note.start_tick as i64 + tick_delta).max(0) as u32;
                    note.note = (note.note as i32 + note_delta).clamp(0, 127) as u8;
                    touched_steps.push(note.start_tick / TICKS_PER_STEP);
                    edited.push(*note);
                }
                st.channels[channel].notes[pattern].sort_by_key(|note| (note.start_tick, note.id));
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
                let pattern = st.current_pattern;
                let channel = st.selected;
                let mut copied: Vec<NoteEvent> = st.channels[channel].notes[pattern]
                    .iter()
                    .copied()
                    .filter(|note| st.selected_note_ids.contains(&note.id))
                    .collect();
                if copied.is_empty() {
                    return;
                }
                // Stored relative to the earliest note, so a paste is a
                // phrase that can land anywhere rather than a set of
                // absolute positions that only fit where they came from.
                let origin = copied.iter().map(|note| note.start_tick).min().unwrap_or(0);
                for note in &mut copied {
                    note.start_tick -= origin;
                }
                copied.sort_by_key(|note| (note.start_tick, note.note));
                commands.borrow_mut().note_clipboard = copied.clone();
                if !cut {
                    window.set_status_message(
                        format!("Copied {} note(s)", copied.len()).into(),
                    );
                    return;
                }
                let ids: Vec<NoteId> = st.selected_note_ids.iter().copied().collect();
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
                window.set_status_message(format!("Cut {} note(s)", ids.len()).into());
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
                let pattern = st.current_pattern;
                let channel = st.selected;
                let length_ticks = st.pattern_lengths[pattern] as u32 * TICKS_PER_STEP;
                // Land the phrase after whatever is selected, or at the top
                // of the pattern when nothing is -- pasting on top of the
                // originals looks like nothing happened.
                let origin = st.channels[channel].notes[pattern]
                    .iter()
                    .filter(|note| st.selected_note_ids.contains(&note.id))
                    .map(|note| note.end_tick())
                    .max()
                    .unwrap_or(0);
                let mut pasted = Vec::with_capacity(clipboard.len());
                for note in clipboard {
                    let start = origin.saturating_add(note.start_tick);
                    if start >= length_ticks {
                        continue;
                    }
                    let id = st.channels[channel].next_note_id;
                    st.channels[channel].next_note_id = id.wrapping_add(1).max(1);
                    let mut copy = NoteEvent { id, ..note };
                    copy.start_tick = start;
                    copy.duration_ticks = copy
                        .duration_ticks
                        .min(length_ticks.saturating_sub(start).max(1));
                    st.channels[channel].notes[pattern].push(copy);
                    pasted.push(copy);
                }
                if pasted.is_empty() {
                    window.set_status_message("Nothing fits at the paste position".into());
                    return;
                }
                st.channels[channel].notes[pattern].sort_by_key(|note| (note.start_tick, note.id));
                // Select what was pasted, so it can be moved straight away.
                st.selected_note_ids = pasted.iter().map(|note| note.id).collect();
                st.selected_note_id = (pasted.len() == 1).then(|| pasted[0].id);
                st.refresh_rack_row(channel);
                st.refresh_note_editor(&window);
                for note in &pasted {
                    let _ = tx.send(EngineCommand::UpsertNote {
                        pattern: pattern as u8,
                        channel: channel as u8,
                        note: *note,
                    });
                }
                window.set_status_message(format!("Pasted {} note(s)", pasted.len()).into());
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
                let pattern = st.current_pattern;
                let channel = st.selected;
                let length_ticks = st.pattern_lengths[pattern] as u32 * TICKS_PER_STEP;
                let start_tick = (start_tick.max(0) as u32).min(length_ticks.saturating_sub(1));
                let mut note = st.channels[channel].create_note(
                    pattern,
                    start_tick,
                    duration_ticks.max(1) as u32,
                    midi_note.clamp(0, 127) as u8,
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
            window.on_piano_note_selected(move |id, mode| {
                let mut st = st.borrow_mut();
                let id = id as NoteId;
                let pattern = st.current_pattern;
                let channel = st.selected;
                if st.channels[channel].notes[pattern]
                    .iter()
                    .any(|note| note.id == id)
                {
                    // The grid resolves which of the gesture roles the held
                    // modifiers satisfied; this only applies the result.
                    match mode {
                        1 => st.toggle_note_selection(id),
                        2 => st.remove_note_from_selection(id),
                        // A plain click always collapses to just this note.
                        _ => st.select_note(Some(id)),
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
                let moving = selection_including(&st, channel, pattern, anchor.id);
                let wanted_tick = (start_tick.max(0) as u32).min(length_ticks.saturating_sub(1));
                let wanted_note = midi_note.clamp(0, 127) as u8;
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
                let tick_delta =
                    tick_delta.clamp(-min_tick, (last_start - max_tick).max(-min_tick));
                let note_delta =
                    note_delta.clamp(-min_note, (127 - max_note).max(-min_note));

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
                let Some(window) = weak.upgrade() else {
                    return -1;
                };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                let channel = st.selected;
                let anchor_id = anchor_id.max(0) as NoteId;
                let originals: Vec<NoteEvent> = st.channels[channel].notes[pattern]
                    .iter()
                    .copied()
                    .filter(|note| note.id == anchor_id || st.selected_note_ids.contains(&note.id))
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
                let Some(anchor) = st.channels[channel].notes[pattern]
                    .iter()
                    .copied()
                    .find(|note| note.id == id as NoteId)
                else {
                    return;
                };
                // The gesture reports the length the grabbed note should end
                // up with; every other selected note changes by the same
                // amount, so a chord keeps its rhythm. This mirrors
                // `on_piano_note_moved`, which is why a single selection has
                // no separate path here either.
                let resizing = selection_including(&st, channel, pattern, anchor.id);
                let delta = duration.max(1) as i64 - anchor.duration_ticks as i64;
                // Clamp by the group, not per note: letting members clip
                // individually would quietly flatten a chord's rhythm at the
                // limit instead of stopping the whole gesture.
                let mut floor = i64::MIN;
                let mut ceiling = i64::MAX;
                for note in st.channels[channel].notes[pattern]
                    .iter()
                    .filter(|note| resizing.contains(&note.id))
                {
                    floor = floor.max(1 - note.duration_ticks as i64);
                    ceiling = ceiling.min(
                        length_ticks.saturating_sub(note.start_tick).max(1) as i64
                            - note.duration_ticks as i64,
                    );
                }
                if floor == i64::MIN {
                    return;
                }
                let delta = delta.clamp(floor, ceiling.max(floor));

                let mut edited = Vec::with_capacity(resizing.len());
                for note in st.channels[channel].notes[pattern]
                    .iter_mut()
                    .filter(|note| resizing.contains(&note.id))
                {
                    note.duration_ticks = (note.duration_ticks as i64 + delta).max(1) as u32;
                    edited.push(*note);
                }
                if edited.len() == 1 {
                    st.select_note(Some(edited[0].id));
                }
                for note in &edited {
                    st.refresh_rack_cell(channel, (note.start_tick / TICKS_PER_STEP) as usize);
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
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                let channel = st.selected;
                let Some(anchor) = st.channels[channel].notes[pattern]
                    .iter()
                    .copied()
                    .find(|note| note.id == id as NoteId)
                else {
                    return;
                };
                let resizing = selection_including(&st, channel, pattern, anchor.id);
                let delta = start_tick.max(0) as i64 - anchor.start_tick as i64;
                // Each note's end tick is what stays put, so the start may
                // travel until it would reach it. Group-clamped for the same
                // reason the other two gestures are.
                let mut floor = i64::MIN;
                let mut ceiling = i64::MAX;
                for note in st.channels[channel].notes[pattern]
                    .iter()
                    .filter(|note| resizing.contains(&note.id))
                {
                    floor = floor.max(-(note.start_tick as i64));
                    ceiling = ceiling.min(note.duration_ticks as i64 - 1);
                }
                if floor == i64::MIN {
                    return;
                }
                let delta = delta.clamp(floor, ceiling.max(floor));

                let mut edited = Vec::with_capacity(resizing.len());
                let mut touched_steps = Vec::with_capacity(resizing.len() * 2);
                for note in st.channels[channel].notes[pattern]
                    .iter_mut()
                    .filter(|note| resizing.contains(&note.id))
                {
                    touched_steps.push(note.start_tick / TICKS_PER_STEP);
                    note.start_tick = (note.start_tick as i64 + delta).max(0) as u32;
                    note.duration_ticks = (note.duration_ticks as i64 - delta).max(1) as u32;
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
                let mut st = st.borrow_mut();
                let base = st.selected_note_ids.clone();
                st.marquee_base = Some((mode, base));
            });
        }
        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_piano_marquee_updated(
                move |start_tick, end_tick, low_note, high_note| {
                    let Some(window) = weak.upgrade() else { return };
                    let mut st = st.borrow_mut();
                    let Some((mode, base)) = st.marquee_base.clone() else {
                        return;
                    };
                    let pattern = st.current_pattern;
                    let channel = st.selected;
                    let (start_tick, end_tick) = (start_tick.min(end_tick), start_tick.max(end_tick));
                    let (low_note, high_note) = (low_note.min(high_note), low_note.max(high_note));
                    let caught: HashSet<NoteId> = st.channels[channel].notes[pattern]
                        .iter()
                        .filter(|note| {
                            // Overlap, not containment: clipping a long
                            // note's tail catches it, which is what every
                            // other editor does and what a band drawn across
                            // a bar of held chords has to do to be useful.
                            let note_start = note.start_tick as i32;
                            let note_end = note.end_tick() as i32;
                            let pitch = note.note as i32;
                            note_start <= end_tick
                                && note_end > start_tick
                                && pitch >= low_note
                                && pitch <= high_note
                        })
                        .map(|note| note.id)
                        .collect();
                    st.selected_note_ids = match mode {
                        1 => base.union(&caught).copied().collect(),
                        2 => base.difference(&caught).copied().collect(),
                        _ => caught,
                    };
                    st.selected_note_id = (st.selected_note_ids.len() == 1)
                        .then(|| *st.selected_note_ids.iter().next().unwrap());
                    st.refresh_note_editor(&window);
                },
            );
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_piano_note_sliced(move |id, tick| {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                let channel = st.selected;
                let Some(original) = st.channels[channel].notes[pattern]
                    .iter()
                    .copied()
                    .find(|note| note.id == id as NoteId)
                else {
                    return;
                };
                let cut = tick.max(0) as u32;
                // The grid guards this too, but a cut at either end would
                // silently delete half a note, so it is worth refusing here
                // as well rather than trusting one caller.
                if cut <= original.start_tick || cut >= original.end_tick() {
                    return;
                }
                let tail_id = st.channels[channel].next_note_id;
                st.channels[channel].next_note_id = tail_id.wrapping_add(1).max(1);
                let tail = NoteEvent {
                    id: tail_id,
                    start_tick: cut,
                    duration_ticks: original.end_tick() - cut,
                    ..original
                };
                let mut head = original;
                head.duration_ticks = cut - original.start_tick;
                for note in st.channels[channel].notes[pattern].iter_mut() {
                    if note.id == head.id {
                        *note = head;
                    }
                }
                st.channels[channel].notes[pattern].push(tail);
                st.channels[channel].notes[pattern].sort_by_key(|note| (note.start_tick, note.id));
                // Both halves selected, so the next gesture can act on the
                // whole of what used to be one note.
                st.selected_note_ids = HashSet::from([head.id, tail.id]);
                st.selected_note_id = None;
                st.refresh_rack_row(channel);
                st.refresh_note_editor(&window);
                for note in [head, tail] {
                    let _ = tx.send(EngineCommand::UpsertNote {
                        pattern: pattern as u8,
                        channel: channel as u8,
                        note,
                    });
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
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                let channel = st.selected;
                let selected: Vec<NoteEvent> = st.channels[channel].notes[pattern]
                    .iter()
                    .copied()
                    .filter(|note| st.selected_note_ids.contains(&note.id))
                    .collect();
                if selected.len() < 2 {
                    return;
                }
                // Per pitch row, not across the whole selection: joining a
                // chord into one note would throw away every pitch but one.
                let mut rows: BTreeMap<u8, Vec<NoteEvent>> = BTreeMap::new();
                for note in selected {
                    rows.entry(note.note).or_default().push(note);
                }
                let mut kept = Vec::new();
                let mut removed = Vec::new();
                for (_, mut row) in rows {
                    if row.len() < 2 {
                        kept.extend(row.iter().map(|note| note.id));
                        continue;
                    }
                    row.sort_by_key(|note| (note.start_tick, note.id));
                    let end = row.iter().map(|note| note.end_tick()).max().unwrap_or(0);
                    let mut merged = row[0];
                    merged.duration_ticks = end.saturating_sub(merged.start_tick).max(1);
                    // The earliest note survives, so the join keeps a stable
                    // id and whatever velocity the phrase started on.
                    for note in st.channels[channel].notes[pattern].iter_mut() {
                        if note.id == merged.id {
                            *note = merged;
                        }
                    }
                    kept.push(merged.id);
                    removed.extend(row[1..].iter().map(|note| note.id));
                }
                if removed.is_empty() {
                    return;
                }
                st.channels[channel].notes[pattern].retain(|note| !removed.contains(&note.id));
                let edited: Vec<NoteEvent> = st.channels[channel].notes[pattern]
                    .iter()
                    .copied()
                    .filter(|note| kept.contains(&note.id))
                    .collect();
                st.prune_note_selection(&removed);
                st.refresh_rack_row(channel);
                st.refresh_note_editor(&window);
                for id in removed {
                    let _ = tx.send(EngineCommand::RemoveNote {
                        pattern: pattern as u8,
                        channel: channel as u8,
                        id,
                    });
                }
                for note in edited {
                    let _ = tx.send(EngineCommand::UpsertNote {
                        pattern: pattern as u8,
                        channel: channel as u8,
                        note,
                    });
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
                let pattern = st.current_pattern;
                let channel = st.selected;
                let length_ticks = st.pattern_lengths[pattern] as u32 * TICKS_PER_STEP;
                let start_tick = (start_tick.max(0) as u32).min(length_ticks.saturating_sub(1));
                let mut note = st.channels[channel].create_note(
                    pattern,
                    start_tick,
                    duration_ticks.max(1) as u32,
                    midi_note.clamp(0, 127) as u8,
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
                // A stroke selects what it laid down, so the run can be
                // moved or lengthened without re-selecting it by hand.
                st.selected_note_ids.insert(note.id);
                st.selected_note_id = (st.selected_note_ids.len() == 1).then_some(note.id);
                st.refresh_rack_cell(channel, (start_tick / TICKS_PER_STEP) as usize);
                st.refresh_note_editor(&window);
                let _ = tx.send(EngineCommand::UpsertNote {
                    pattern: pattern as u8,
                    channel: channel as u8,
                    note,
                });
                drop(st);
                record_project_history(&commands, before, &history_state, &window, "Notes painted");
            });
        }
        {
            let st = state.clone();
            window.on_piano_scale_begin(move |from_left| {
                let mut st = st.borrow_mut();
                let pattern = st.current_pattern;
                let channel = st.selected;
                let notes: Vec<(NoteId, u32, u32)> = st.channels[channel].notes[pattern]
                    .iter()
                    .filter(|note| st.selected_note_ids.contains(&note.id))
                    .map(|note| (note.id, note.start_tick, note.duration_ticks))
                    .collect();
                if notes.len() < 2 {
                    st.scale_base = None;
                    return;
                }
                // Scale about the edge the drag is not moving, so that edge
                // stays put and only the span changes.
                let anchor = if from_left {
                    notes
                        .iter()
                        .map(|(_, start, duration)| start + duration)
                        .max()
                        .unwrap_or(0)
                } else {
                    notes.iter().map(|(_, start, _)| *start).min().unwrap_or(0)
                };
                st.scale_base = Some(ScaleBase { anchor, notes });
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_piano_selection_scaled(move |factor| {
                let Some(window) = weak.upgrade() else { return };
                let before = project_snapshot(&st.borrow(), &window);
                let mut st = st.borrow_mut();
                let Some(base) = st.scale_base.take() else {
                    return;
                };
                let pattern = st.current_pattern;
                let channel = st.selected;
                let length_ticks = st.pattern_lengths[pattern] as u32 * TICKS_PER_STEP;
                let last_start = length_ticks.saturating_sub(1);
                let factor = factor.clamp(0.02, 64.0) as f64;
                let anchor = base.anchor as f64;

                let mut edited = Vec::with_capacity(base.notes.len());
                let mut touched_steps = Vec::with_capacity(base.notes.len() * 2);
                for (id, start, duration) in &base.notes {
                    let Some(note) = st.channels[channel].notes[pattern]
                        .iter_mut()
                        .find(|note| note.id == *id)
                    else {
                        continue;
                    };
                    touched_steps.push(note.start_tick / TICKS_PER_STEP);
                    let scaled_start = anchor + (*start as f64 - anchor) * factor;
                    // Lengths scale with the span, which is the point: double
                    // a selection's width and an eighth becomes a quarter.
                    let scaled_duration = *duration as f64 * factor;
                    note.start_tick = (scaled_start.round().max(0.0) as u32).min(last_start);
                    note.duration_ticks = (scaled_duration.round().max(1.0) as u32)
                        .min(length_ticks.saturating_sub(note.start_tick).max(1));
                    touched_steps.push(note.start_tick / TICKS_PER_STEP);
                    edited.push(*note);
                }
                st.channels[channel].notes[pattern].sort_by_key(|note| (note.start_tick, note.id));
                st.scale_base = Some(base);
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
                record_project_history(
                    &commands,
                    before,
                    &history_state,
                    &window,
                    "Selection scaled",
                );
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
                let Some(window) = weak.upgrade() else {
                    return -1;
                };
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
                if st.channels.len() >= MAX_CHANNELS {
                    return;
                }
                log_debug!("ui", "add channel");
                let index = st.channels.len();
                let mut ch = ChannelState::new(index);
                ch.notes.resize_with(st.pattern_lengths.len(), Vec::new);
                ch.automation
                    .resize_with(st.pattern_lengths.len(), Vec::new);
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
                let selected = st.borrow().selected;
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
                // The bus fader's throw reaches +6 dB and the engine's
                // output stage accepts +12, same as a channel's: clamping
                // here at unity left the top of every bus fader dead.
                let volume = volume.clamp(0.0, MAX_LINEAR_GAIN);
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

        // --- Channel modulation shelf -------------------------------------
        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_modulation_shelf_toggled(move || {
                let Some(window) = weak.upgrade() else { return };
                let mut state = st.borrow_mut();
                state.modulation_shelf_open = !state.modulation_shelf_open;
                state.refresh_modulation(&window);
            });
        }
        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_modulation_source_selected(move |slot| {
                let (Some(window), Ok(slot)) = (weak.upgrade(), u8::try_from(slot)) else {
                    return;
                };
                let mut state = st.borrow_mut();
                let exists = state
                    .channels
                    .get(state.selected)
                    .is_some_and(|channel| channel.modulation.params(slot as usize).is_some());
                if !exists {
                    return;
                }
                // Selection opens an editor. If assignment is already active,
                // it follows the newly selected source; otherwise this click
                // has no effect on ordinary parameter gestures.
                state.modulation_selected_slot.set(Some(slot));
                if state.modulation_armed_slot.get().is_some() {
                    state.modulation_armed_slot.set(Some(slot));
                }
                state.modulation_shelf_open = true;
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
                let (Some(window), Ok(slot), Ok(target)) = (
                    weak.upgrade(),
                    usize::try_from(slot),
                    usize::try_from(target),
                ) else {
                    return;
                };
                let before = {
                    let state = st.borrow();
                    project_snapshot(&state, &window)
                };
                let moved = {
                    let mut state = st.borrow_mut();
                    let selected = state.selected;
                    // Selection and arming follow the module, not the slot it
                    // used to be in, or a reorder would silently retarget the
                    // assignment gesture.
                    let selected_slot = state.modulation_selected_slot.get();
                    let armed_slot = state.modulation_armed_slot.get();
                    let Some(channel) = state.channels.get_mut(selected) else {
                        return;
                    };
                    let source_of = |slot: Option<u8>| {
                        slot.and_then(|slot| channel.modulation.source_id(slot as usize))
                    };
                    let selected_id = source_of(selected_slot);
                    let armed_id = source_of(armed_slot);
                    if !channel.modulation.move_module(slot, target) {
                        return;
                    }
                    let next_selected = selected_id.and_then(|id| channel.modulation.slot_of(id));
                    let next_armed = armed_id.and_then(|id| channel.modulation.slot_of(id));
                    state.modulation_selected_slot.set(next_selected);
                    state.modulation_armed_slot.set(next_armed);
                    // Both racks run the same permutation, so the engine's
                    // copy carries routes and a math module's input slot
                    // across the move exactly as this one did.
                    state.send_modulation(&window, &tx, |channel| {
                        EngineCommand::MoveModulator {
                            channel,
                            from: slot as u8,
                            to: target as u8,
                        }
                    });
                    true
                };
                if moved {
                    record_project_history(&commands, before, &st, &window, "Module moved");
                }
            });
        }
        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_modulation_assignment_toggled(move || {
                let Some(window) = weak.upgrade() else { return };
                let mut state = st.borrow_mut();
                let next = if state.modulation_armed_slot.get().is_some() {
                    None
                } else {
                    state.modulation_selected_slot.get()
                };
                state.modulation_armed_slot.set(next);
                state.modulation_shelf_open = true;
                let source_name = next.and_then(|slot| {
                    state
                        .channels
                        .get(state.selected)
                        .and_then(|channel| channel.modulation.params(slot as usize))
                        .map(|params| format!("{} {}", params.kind().badge(), slot + 1))
                });
                state.refresh_modulation(&window);
                window.set_status_message(if let Some(source_name) = source_name {
                    format!(
                        "Assigning {source_name} \u{2014} drag a highlighted control to set route depth"
                    )
                    .into()
                } else {
                    "Modulation assignment off \u{2014} controls edit their base values".into()
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
                let (Some(window), Some(kind)) =
                    (weak.upgrade(), ModulatorKind::from_index(kind))
                else {
                    return;
                };
                let before = {
                    let state = st.borrow();
                    project_snapshot(&state, &window)
                };
                let added = {
                    let mut state = st.borrow_mut();
                    let selected = state.selected;
                    let Some(channel) = state.channels.get_mut(selected) else {
                        return;
                    };
                    let Some(slot) = channel.modulation.free_slot() else {
                        return;
                    };
                    let mut params = kind.default_params();
                    // The envelope's gate is a jack rather than a descriptor
                    // id, so its only sensible default is set here.
                    if let ModulatorParams::Envelope(envelope) = &mut params {
                        envelope.input_channel = selected as u8;
                    }
                    channel.modulation.install(slot, params);
                    state.modulation_selected_slot.set(Some(slot as u8));
                    state.modulation_armed_slot.set(None);
                    state.modulation_shelf_open = true;
                    state.send_modulator_slot(&window, &tx, slot);
                    true
                };
                if added {
                    // History labels are `&'static str`, so the per-kind
                    // wording is a match rather than a format.
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
                }
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
                let (Some(window), Ok(slot), Ok(id)) =
                    (weak.upgrade(), usize::try_from(slot), u32::try_from(id))
                else {
                    return;
                };
                let before = {
                    let mut state = st.borrow_mut();
                    let in_gesture = state.modulation_edit_before.is_some();
                    let before = (!in_gesture).then(|| project_snapshot(&state, &window));
                    let selected = state.selected;
                    let Some(params) = state
                        .channels
                        .get_mut(selected)
                        .and_then(|channel| channel.modulation.params_mut(slot))
                    else {
                        return;
                    };
                    let previous = params.get(id);
                    params.set(id, value);
                    if params.get(id) == previous {
                        return;
                    }
                    if in_gesture {
                        state.modulation_edit_changed = true;
                    }
                    state.send_modulation(&window, &tx, |channel| {
                        EngineCommand::SetModulatorParam {
                            channel,
                            slot: slot as u8,
                            id,
                            value,
                        }
                    });
                    before
                };
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
                let before = st.borrow_mut().finish_modulation_edit();
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
                let (Some(window), Ok(slot)) = (weak.upgrade(), u8::try_from(slot)) else {
                    return;
                };
                let before = {
                    let state = st.borrow();
                    project_snapshot(&state, &window)
                };
                let removed = {
                    let mut state = st.borrow_mut();
                    let selected = state.selected;
                    let Some(channel) = state.channels.get_mut(selected) else {
                        return;
                    };
                    // The rack drops the module's routes by identity, so a
                    // route aimed at a different module in the same slot
                    // cannot be caught up in the removal.
                    if !channel.modulation.clear(slot as usize) {
                        false
                    } else {
                        if state.modulation_selected_slot.get() == Some(slot) {
                            state.modulation_selected_slot.set(None);
                        }
                        if state.modulation_armed_slot.get() == Some(slot) {
                            state.modulation_armed_slot.set(None);
                        }
                        state.send_modulation(&window, &tx, |channel| {
                            EngineCommand::ClearModulator { channel, slot }
                        });
                        true
                    }
                };
                if removed {
                    record_project_history(&commands, before, &st, &window, "Modulator removed");
                }
            });
        }
        {
            let st = state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_modulation_envelope_input_channel_changed(move |slot, channel| {
                let (Some(window), Ok(slot), Ok(channel)) =
                    (weak.upgrade(), usize::try_from(slot), u8::try_from(channel))
                else {
                    return;
                };
                let mut state = st.borrow_mut();
                if channel as usize >= state.channels.len() {
                    return;
                }
                let Some(envelope) = state.modulation_envelope_mut(slot) else {
                    return;
                };
                envelope.input_channel = channel;
                // The gate is a jack rather than a descriptor id, so there is
                // no parameter to name: the module travels entire.
                state.send_modulator_slot(&window, &tx, slot);
            });
        }
        {
            let st = state.clone();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_modulation_route_polarity_changed(move |index, polarity| {
                let (Some(window), Ok(index)) = (weak.upgrade(), usize::try_from(index)) else {
                    return;
                };
                let before = {
                    let state = st.borrow();
                    project_snapshot(&state, &window)
                };
                let changed = {
                    let mut state = st.borrow_mut();
                    let selected = state.selected;
                    let Some(route) = state
                        .channels
                        .get_mut(selected)
                        .and_then(|channel| channel.modulation.routes.get_mut(index))
                        .and_then(Option::as_mut)
                    else {
                        return;
                    };
                    let next = if polarity == 1 {
                        ModPolarity::Unipolar
                    } else {
                        ModPolarity::Bipolar
                    };
                    if route.polarity == next {
                        false
                    } else {
                        route.polarity = next;
                        let route = *route;
                        state.send_modulation(&window, &tx, |channel| {
                            EngineCommand::SetModRoute { channel, route }
                        });
                        true
                    }
                };
                if changed {
                    record_project_history(
                        &commands,
                        before,
                        &st,
                        &window,
                        "Modulation polarity changed",
                    );
                }
            });
        }
        {
            let st = state.clone();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_modulation_route_removed(move |index| {
                let (Some(window), Ok(index)) = (weak.upgrade(), usize::try_from(index)) else {
                    return;
                };
                let before = {
                    let state = st.borrow();
                    project_snapshot(&state, &window)
                };
                let removed = {
                    let mut state = st.borrow_mut();
                    let selected = state.selected;
                    let Some(route) = state
                        .channels
                        .get_mut(selected)
                        .and_then(|channel| channel.modulation.routes.get_mut(index))
                    else {
                        return;
                    };
                    // The row's durable source is read before it is taken:
                    // the engine is told which assignment ended, not which
                    // matrix position emptied, so the two racks cannot drift
                    // into removing different routes.
                    match route.take() {
                        None => false,
                        Some(removed) => {
                            state.send_modulation(&window, &tx, |channel| {
                                EngineCommand::RemoveModRoute {
                                    channel,
                                    source: removed.source,
                                    destination: removed.destination,
                                }
                            });
                            true
                        }
                    }
                };
                if removed {
                    record_project_history(
                        &commands,
                        before,
                        &st,
                        &window,
                        "Modulation route removed",
                    );
                }
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
                    scope: EffectTarget::Channel(state.selected as u8),
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
                let before = st.borrow_mut().finish_modulation_edit();
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
                    ParamAddr::strip(EffectTarget::Channel(state.selected as u8), param);
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
                let before = st.borrow_mut().finish_modulation_edit();
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
                let valid = matches!(state.effect_target, EffectTarget::Channel(channel) if channel as usize == state.selected)
                    && state
                        .channels
                        .get(state.selected)
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
                let destination = match state.effect_target {
                    EffectTarget::Channel(channel) if channel as usize == state.selected => state
                        .channels
                        .get(state.selected)
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
                let before = st.borrow_mut().finish_modulation_edit();
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
                let before = project_snapshot(&st.borrow(), &window);
                let added = {
                    let mut st = st.borrow_mut();
                    let target = st.effect_target;
                    let inserted = st.effect_chain_mut().and_then(|effects| {
                        let tail = effects.len();
                        let effect = EffectSlotState::of_kind(kind);
                        insert_effect(effects, insert_before as usize, effect)
                            .map(|(slot, remap)| (slot, tail, remap, effect.params))
                    });
                    let Some((slot, tail, remap, params)) = inserted else {
                        return;
                    };
                    st.retarget_effect_slots(target, &remap);
                    st.sync_effects();
                    st.refresh_automation(&window);
                    st.refresh_modulation(&window);
                    // Install into the vacant tail then move it left. Keeping
                    // this on the ordered stream means the realtime chain
                    // sees the same order as the model without allocating in
                    // its callback. The dry-align ring is built here for the
                    // same reason as the node: construction allocates, so it
                    // happens off the audio thread and rides the same
                    // structural command.
                    let bpm = window.get_bpm() as f64;
                    let node = build_effect_at_tempo(params, sample_rate, bpm);
                    let align = DryAlign::new(node.dry_path_latency_frames()).map(Box::new);
                    let _ = stx.send(StructuralCommand::InstallEffect {
                        target,
                        slot: tail as u8,
                        kind,
                        resource_key: params.buffer().copied().map(buffer_allocation_key),
                        node,
                        align,
                        analyzer: Box::new(SpectrumAnalyzer::new()),
                        // Allocated here with the node: an empty addressable
                        // slot costs a pointer rather than its full host state.
                        state: Box::new(EffectSlot::new()),
                    });
                    if slot != tail {
                        let _ = tx.send(EngineCommand::MoveEffect {
                            target,
                            from: tail as u8,
                            to: slot as u8,
                        });
                    }
                    true
                };
                if added {
                    record_project_history(&commands, before, &st, &window, "Effect added");
                }
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
                let before = project_snapshot(&st.borrow(), &window);
                let removed = {
                    let mut st = st.borrow_mut();
                    let target = st.effect_target;
                    let slot = slot as usize;
                    let removed = st.effect_chain_mut().and_then(|effects| {
                        remove_effect(effects, slot).map(|(_, remap)| (effects.len(), remap))
                    });
                    let Some((tail, remap)) = removed else {
                        return;
                    };
                    st.retarget_effect_slots(target, &remap);
                    st.sync_effects();
                    st.refresh_automation(&window);
                    st.refresh_modulation(&window);
                    // Mirror on the engine: move the device to the vacated
                    // tail, then drop the tail. Its routes and lanes ride
                    // along and are dropped with it.
                    if slot != tail {
                        let _ = tx.send(EngineCommand::MoveEffect {
                            target,
                            from: slot as u8,
                            to: tail as u8,
                        });
                    }
                    let _ = stx.send(StructuralCommand::RemoveEffect {
                        target,
                        slot: tail as u8,
                    });
                    true
                };
                if removed {
                    record_project_history(&commands, before, &st, &window, "Effect removed");
                }
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
                let Some(value) = effect
                    .params
                    .set(id, descriptor.from_normalized(normalized))
                else {
                    return;
                };
                let row = effect_slot_row(effect);
                st.effect_slot_model.set_row_data(slot, row);
                let _ = tx.send(EngineCommand::SetEffectParam {
                    target,
                    slot: slot as u8,
                    id,
                    value,
                });
            });
        }

        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_delay_tempo_sync_changed(move |slot, enabled| {
                let mut state = st.borrow_mut();
                let slot = slot as usize;
                let Some(effects) = state.effect_chain_mut() else {
                    return;
                };
                let Some(effect) = effects.get_mut(slot) else {
                    return;
                };
                let EffectParams::Delay(params) = &mut effect.params else {
                    return;
                };
                params.tempo_sync = enabled;
                let row = effect_slot_row(effect);
                state.effect_slot_model.set_row_data(slot, row);
                state.dirty = true;
                state.revision = state.revision.wrapping_add(1);
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
                let slot = slot as usize;
                let Some(effects) = state.effect_chain_mut() else {
                    return;
                };
                let Some(effect) = effects.get_mut(slot) else {
                    return;
                };
                let EffectParams::Delay(params) = &mut effect.params else {
                    return;
                };
                params.time_division = mooloop_core::DelayTimeDivision::from_index(division);
                let row = effect_slot_row(effect);
                state.effect_slot_model.set_row_data(slot, row);
                state.dirty = true;
                state.revision = state.revision.wrapping_add(1);
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
                let before = project_snapshot(&st.borrow(), &window);
                let moved = {
                    let mut st = st.borrow_mut();
                    let target = st.effect_target;
                    let (from, to) = (from as usize, to as usize);
                    let Some(remap) = st
                        .effect_chain_mut()
                        .and_then(|effects| move_effect(effects, from, to))
                    else {
                        return;
                    };
                    st.retarget_effect_slots(target, &remap);
                    st.sync_effects();
                    st.refresh_automation(&window);
                    st.refresh_modulation(&window);
                    let _ = tx.send(EngineCommand::MoveEffect {
                        target,
                        from: from as u8,
                        to: to as u8,
                    });
                    true
                };
                if moved {
                    record_project_history(&commands, before, &st, &window, "Effect moved");
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
                        let ch = st.selected;
                        let Some(channel) = st.channels.get_mut(ch) else {
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
                let (resolved, moved, searched) = {
                    let mut st = st.borrow_mut();
                    let ch = st.selected;
                    let Some(channel) = st.channels.get_mut(ch) else {
                        return;
                    };
                    let Some(sample) = channel.published_sample().cloned() else {
                        window.set_status_message("No sample to snap".into());
                        return;
                    };
                    let markers = [
                        SampleMarker::Start,
                        SampleMarker::End,
                        SampleMarker::LoopStart,
                        SampleMarker::LoopEnd,
                    ];
                    let mut resolved = Vec::with_capacity(markers.len());
                    let mut moved = 0usize;
                    let mut searched = 0usize;
                    for marker in markers {
                        let requested = marker.get(&channel.params);
                        let Some((value, result)) =
                            snap_marker(&channel.params, &sample, marker, requested)
                        else {
                            continue;
                        };
                        searched += 1;
                        if result.moved() {
                            moved += 1;
                        }
                        marker.set(&mut channel.params, value);
                        resolved.push((marker, value));
                    }
                    let p = channel.params;
                    let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                        channel: ch as u8,
                        params: p,
                    });
                    (resolved, moved, searched)
                };
                for (marker, value) in resolved {
                    set_marker_property(&window, marker, value);
                }
                window.set_status_message(
                    format!("Snapped {moved} of {searched} markers to zero crossings").into(),
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
                    let ch = st.selected;
                    let Some(channel) = st.channels.get_mut(ch) else {
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
                let Some(channel) = st.channels.get(st.selected) else {
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
            window.on_retune_live_changed(move |on| {
                let mut st = st.borrow_mut();
                let ch = st.selected;
                let Some(channel) = st.channels.get_mut(ch) else {
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
                let ch = st.selected;
                let Some(channel) = st.channels.get_mut(ch) else {
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
                let ch = st.selected;
                let Some(channel) = st.channels.get_mut(ch) else {
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
                    let ch = st.selected;
                    let Some(channel) = st.channels.get_mut(ch) else {
                        return;
                    };
                    let Some(frame) = resolve_slice_frame(channel, position, window.get_snap_to_zero())
                    else {
                        return;
                    };
                    if channel.slices.add(frame).is_none() {
                        window.set_status_message(
                            format!("No slice added: {MAX_SLICES} is the limit, or one is already there")
                                .into(),
                        );
                        return;
                    }
                    publish_channel_audio_to(&audio_out, ch, channel);
                    let markers = slice_fractions(channel);
                    st.slice_model.set_vec(markers);
                    st.dirty = true;
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
                    let ch = st.selected;
                    let Some(channel) = st.channels.get_mut(ch) else {
                        return;
                    };
                    // By id, not by position: a drag past a neighbour
                    // reorders the map, and the next move frame still means
                    // the marker under the pointer.
                    let Some(id) = channel
                        .slices
                        .get(index.max(0) as usize)
                        .map(|marker| marker.id)
                    else {
                        return;
                    };
                    // Not snapped while dragging: a marker that jumps to a
                    // crossing under the pointer fights the drag. The AUTO
                    // snap lands it on release, below.
                    let Some(frame) = resolve_slice_frame(channel, position, false) else {
                        return;
                    };
                    if !channel.slices.move_to(id, frame) {
                        return;
                    }
                    publish_channel_audio_to(&audio_out, ch, channel);
                    let markers = slice_fractions(channel);
                    st.slice_model.set_vec(markers);
                    st.dirty = true;
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
                    let ch = st.selected;
                    let Some(channel) = st.channels.get_mut(ch) else {
                        return;
                    };
                    let Some(id) = channel
                        .slices
                        .get(index.max(0) as usize)
                        .map(|marker| marker.id)
                    else {
                        return;
                    };
                    channel.slices.remove(id);
                    publish_channel_audio_to(&audio_out, ch, channel);
                    let markers = slice_fractions(channel);
                    st.slice_model.set_vec(markers);
                    st.dirty = true;
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
                    let ch = st.selected;
                    let Some(channel) = st.channels.get_mut(ch) else {
                        return;
                    };
                    let Some(sample) = channel.published_sample().cloned() else {
                        window.set_status_message("No sample to slice".into());
                        return;
                    };
                    let len = sample.frames.len();
                    let start = frame_from_fraction(channel.params.start, len) as u32;
                    let end = frame_from_fraction(channel.params.end, len) as u32;
                    channel
                        .slices
                        .divide_evenly(count.max(1) as usize, start, end);
                    // Grid divisions land wherever the arithmetic puts them,
                    // which is as likely to be mid-waveform as a hand-placed
                    // marker is. Snapping them is the same reason the trim
                    // markers snap, multiplied by the slice count.
                    if window.get_snap_to_zero() {
                        let params = channel.params;
                        let snapped: Vec<mooloop_core::SliceMarker> = channel
                            .slices
                            .markers()
                            .iter()
                            .map(|marker| mooloop_core::SliceMarker {
                                id: marker.id,
                                frame: snap_slice_frame(&params, &sample, marker.frame as usize)
                                    as u32,
                            })
                            .collect();
                        channel.slices.rebuild(snapped);
                    }
                    publish_channel_audio_to(&audio_out, ch, channel);
                    let markers = slice_fractions(channel);
                    st.slice_model.set_vec(markers);
                    st.dirty = true;
                    window.set_status_message(
                        format!("Divided into {} slices", count.max(1)).into(),
                    );
                }
                record_project_history(
                    &commands,
                    before,
                    &history_state,
                    &window,
                    "Slices divided",
                );
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
                    let ch = st.selected;
                    let Some(channel) = st.channels.get_mut(ch) else {
                        return;
                    };
                    channel.slices.clear();
                    publish_channel_audio_to(&audio_out, ch, channel);
                    st.slice_model.set_vec(Vec::new());
                    st.dirty = true;
                }
                record_project_history(
                    &commands,
                    before,
                    &history_state,
                    &window,
                    "Slices cleared",
                );
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_slice_auditioned(move |index| {
                let mut st = st.borrow_mut();
                let ch = st.selected;
                let Some(channel) = st.channels.get(ch) else {
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
                st.slice_audition = Some((ch as u8, note as u8));
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
                let Some((channel, note)) = st.borrow_mut().slice_audition.take() else {
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
                    let ch = st.selected;
                    let Some(channel) = st.channels.get_mut(ch) else {
                        return;
                    };
                    // Always from the source, never from a buffer that has
                    // already been baked: re-committing at a new tempo has to
                    // be a fresh render or repeated tempo changes accumulate
                    // stretch on stretch.
                    let Some(source) = channel.sample_data.clone() else {
                        window.set_status_message("No sample to commit".into());
                        return;
                    };
                    let (params, slices) = match channel.commit.as_ref() {
                        Some(commit) => {
                            let (params, slices) = mooloop_dsp::commit::revert_commit(
                                channel.params,
                                commit,
                            );
                            (params, slices)
                        }
                        None => (channel.params, channel.slices.clone()),
                    };
                    let bpm = window.get_bpm() as f64;
                    let Some(committed) =
                        mooloop_dsp::commit::commit_stretch(&source, params, &slices, bpm)
                    else {
                        window.set_status_message("Nothing to commit".into());
                        return;
                    };
                    let ratio = committed.commit.ratio;
                    channel.params = committed.params;
                    channel.slices = committed.slices;
                    channel.commit = Some(Box::new(committed.commit));
                    channel.committed_sample = Some(committed.sample);
                    refresh_sample_view(channel);
                    publish_channel_audio_to(&audio_out, ch, channel);
                    let p = channel.params;
                    let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                        channel: ch as u8,
                        params: p,
                    });
                    // The stretch is in the audio now and the patch no longer
                    // asks for it, so the pool goes back the way it came
                    // rather than holding ~1.6 MB for a stretcher that will
                    // not run. Same reconciliation the ON toggle does.
                    let _ = stx.send(StructuralCommand::SetSamplerStretch {
                        channel: ch as u8,
                        pool: None,
                    });
                    st.dirty = true;
                    window.set_status_message(
                        format!("Committed the stretch at {ratio:.2}x").into(),
                    );
                }
                st.borrow().refresh_editor(&window);
                record_project_history(
                    &commands,
                    before,
                    &history_state,
                    &window,
                    "Stretch committed",
                );
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
                    let ch = st.selected;
                    let Some(channel) = st.channels.get_mut(ch) else {
                        return;
                    };
                    let Some(commit) = channel.commit.take() else {
                        return;
                    };
                    let (params, slices) =
                        mooloop_dsp::commit::revert_commit(channel.params, &commit);
                    channel.params = params;
                    channel.slices = slices;
                    channel.committed_sample = None;
                    refresh_sample_view(channel);
                    publish_channel_audio_to(&audio_out, ch, channel);
                    let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                        channel: ch as u8,
                        params,
                    });
                    // The patch is stretching live again, and the state to
                    // do it cannot be assumed: a project saved committed and
                    // reloaded never provisioned a pool, because its patch
                    // did not ask for one. Without this, revert after a
                    // reload put the switch on and played unstretched.
                    let _ = stx.send(StructuralCommand::SetSamplerStretch {
                        channel: ch as u8,
                        pool: Some(Box::new(StretchPool::new(
                            params.stretch_mode,
                            sample_rate,
                            MAX_SAMPLER_VOICES as usize,
                        ))),
                    });
                    st.dirty = true;
                    window.set_status_message("Reverted to the source sample".into());
                }
                st.borrow().refresh_editor(&window);
                record_project_history(
                    &commands,
                    before,
                    &history_state,
                    &window,
                    "Stretch reverted",
                );
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
                let channel_index = st.selected;
                let channel = &mut st.channels[channel_index];
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
                let channel_index = st.selected;
                let channel = &mut st.channels[channel_index];
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
                let channel_index = st.selected;
                let channel = &mut st.channels[channel_index];
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
                        let channel_index = st.selected;
                        let channel = &mut st.channels[channel_index];
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
                let channel_index = st.selected;
                let channel = &mut st.channels[channel_index];
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
                let channel_index = st.selected;
                let channel = &mut st.channels[channel_index];
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
                let channel_index = st.selected;
                let channel = &mut st.channels[channel_index];
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

        macro_rules! wire_mlm1_param {
            ($callback:ident, $($field:ident).+) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                window.$callback(move |value: f32| {
                    let mut st = st.borrow_mut();
                    let channel_index = st.selected;
                    let channel = &mut st.channels[channel_index];
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
                    let channel_index = st.selected;
                    let channel = &mut st.channels[channel_index];
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
                    let channel_index = st.selected;
                    let channel = &mut st.channels[channel_index];
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
                    let channel_index = st.selected;
                    let channel = &mut st.channels[channel_index];
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
                    let channel_index = st.selected;
                    let channel = &mut st.channels[channel_index];
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
                            st.channels[st.selected].ds01_params
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
                let channel_index = st.selected;
                let channel = &mut st.channels[channel_index];
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
                let channel_index = st.selected;
                let channel = &mut st.channels[channel_index];
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
                let channel_index = st.selected;
                let channel = &mut st.channels[channel_index];
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
                    let index = st.selected;
                    if st.channels[index].kind != DeviceKind::MlP8 {
                        return;
                    }
                    // A closure so an edit can bail with `?` on an id that
                    // names no route -- which is what a stale click during a
                    // list rebuild looks like.
                    let edit = |$routes: &mut mooloop_core::MlP8Routes,
                                $channel: u8|
                     -> Option<EngineCommand> { $body };
                    let Some(command) = edit(
                        &mut st.channels[index].mlp8_params.routes,
                        index as u8,
                    ) else {
                        return;
                    };
                    let _ = tx.send(command);
                    let routes = st.channels[index].mlp8_params.routes;
                    refresh_mlp8_routes(&window, &routes);
                    st.dirty = true;
                    st.revision = st.revision.wrapping_add(1);
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
                let channel_index = st.selected;
                if st.channels[channel_index].kind != DeviceKind::MlP8 {
                    return;
                }
                if !st.channels[channel_index]
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
                st.dirty = true;
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
                    let channel_index = st.selected;
                    let channel = &mut st.channels[channel_index];
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
                let path = PathBuf::from(path.to_string());
                let mut st = st.borrow_mut();
                // Insert-or-remove: a path never expanded collapses to a
                // no-op remove, so the set only ever holds expanded folders.
                if !st.browser_expanded.remove(&path) {
                    st.browser_expanded.insert(path);
                }
                refresh_browser(&st);
            });
        }
        {
            let st = state.clone();
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_browser_location_removed(move |path| {
                let path = PathBuf::from(path.to_string());
                let window = match weak.upgrade() {
                    Some(window) => window,
                    None => return,
                };
                // Only top-level rows offer removal, so anything the tree
                // hands back that is not a location is a stale no-op.
                settings
                    .borrow_mut()
                    .browser
                    .locations
                    .retain(|p| p != &path);
                let saved = settings.borrow().save();
                {
                    let mut st = st.borrow_mut();
                    st.browser_locations.retain(|p| p != &path);
                    st.browser_expanded.remove(&path);
                    refresh_browser(&st);
                }
                match saved {
                    Ok(()) => window.set_status_message(
                        format!("Removed sample folder {}", path.display()).into(),
                    ),
                    Err(error) => window
                        .set_status_message(format!("Could not save settings: {error}").into()),
                };
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
                    (st.selected, st.source_revision)
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
                    (st.channels.len(), st.source_revision)
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
                    (st.selected, st.source_revision)
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
                    let result = match adjacent_sample(&path, -1) {
                        Ok(Some(path)) => Some(load_sample_at_path(&path)),
                        Ok(None) => None,
                        Err(error) => Some(Err(error)),
                    };
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
                    let result = match adjacent_sample(&path, 1) {
                        Ok(Some(path)) => Some(load_sample_at_path(&path)),
                        Ok(None) => None,
                        Err(error) => Some(Err(error)),
                    };
                    let _ = tx.send(LoadResult {
                        channel,
                        source_revision,
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
                    if st.borrow().browser_locations.contains(&path) {
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
                        state.browser_locations.push(path.clone());
                        state.browser_expanded.insert(path.clone());
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
                        load.source_revision == st.source_revision
                            && (load.new_channel && st.channels.len() < MAX_CHANNELS
                                || !load.new_channel
                                    && st
                                        .channels
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
                        let channel = st.borrow().channels.len().saturating_sub(1);
                        apply_loaded_sample(&handle, &st, &weak, channel, loaded);
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
                        PendingEngineMessage::PreviewGain(gain) => {
                            handle.set_preview_gain(gain);
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
                        PendingEngineMessage::AddChannel { channel, source } => {
                            {
                                let mut state = st.borrow_mut();
                                document_title_needs_refresh |= !state.dirty;
                                state.dirty = true;
                                state.revision = state.revision.wrapping_add(1);
                            }
                            handle.add_channel(channel, source);
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
                {
                    // Live modulation on the knobs. The engine publishes the
                    // channel's four modulator outputs; resolving those into a
                    // per-destination offset is the UI's job, so this is a
                    // read of four cells plus arithmetic over the visible
                    // descriptors -- not a per-parameter feed.
                    let state = st.borrow();
                    let outputs = handle.modulator_outputs(selected_channel);
                    let routed = state
                        .channels
                        .get(selected_channel)
                        .is_some_and(|channel| {
                            channel.modulation.routes.iter().flatten().next().is_some()
                        });
                    // An unrouted channel has nothing to animate, and once the
                    // outputs stop moving the arcs are already where they
                    // belong -- so neither case is worth a model write.
                    if routed && !editing_bus && outputs != state.modulation_outputs.get() {
                        state.modulation_outputs.set(outputs);
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
fn open_mockup_window() -> Result<MockupCanvas, slint::PlatformError> {
    let canvas = MockupCanvas::new()?;
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
        for (index, channel) in st.channels.iter().enumerate() {
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
    if let Some(ch) = st.channels.get_mut(channel) {
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
    st.dirty = true;
    st.revision = st.revision.wrapping_add(1);
    if channel == st.selected {
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
        &st.browser_locations,
        &st.browser_expanded,
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
