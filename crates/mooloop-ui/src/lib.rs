//! Slint UI wrapper. Owns the `EngineHandle`, wires Slint callbacks to engine
//! commands, and runs a high-frequency timer that forwards commands and drains
//! audio events onto window properties.
//!
//! The UI owns the project state (channels, pattern bank, per-channel sampler
//! params) as the source of truth and mirrors every mutation to the engine
//! via commands. The engine keeps its own pre-allocated copy.

mod actions;
mod channel_colors;
mod gestures;
mod meter;
#[cfg(feature = "mockup")]
mod mockup;
mod settings;
mod signals;
mod theme;

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
use mooloop_core::strip::{
    strip_band_param, StripParams, STRIP_BAND_FREQ, STRIP_BAND_GAIN, STRIP_BAND_KIND, STRIP_BAND_Q,
    STRIP_BAND_STRIDE, STRIP_COMP_ATTACK_MS, STRIP_COMP_IN, STRIP_COMP_KNEE_DB, STRIP_COMP_MAKEUP_DB, STRIP_COMP_MIX, STRIP_COMP_RATIO,
    STRIP_COMP_RELEASE_MS, STRIP_COMP_THRESHOLD_DB, STRIP_DRIVE_DB, STRIP_EQ_BANDS, STRIP_EQ_IN,
    STRIP_FIRST, STRIP_PRE_IN, STRIP_VOICING,
};
use mooloop_core::{log_debug, log_error, log_info, log_warn};
use mooloop_core::{
    snap_bars_to_power_of_two,
    BusSetup, ChannelEdit, ListEdit, TrackEdit, ENV_MAX_SECONDS, ENV_MIN_SECONDS,
    DeviceKind, DrumMode, DrumSynthParams, EffectKind,
    EffectSlotState, EffectTarget, EngineCommand, EngineEvent, EnvTrigger, EqFaceControl,
    EqParams, EQ_FACE_CONTROLS, FilterModel,
    GeneratorParams, GlideMode, HatCharacter,
    KickCharacter, Kit, LfoWave, LoopMode, ModDestinationDescriptor,
    ModPolarity, ModRack, ModRandomTrigger, ModStepTrigger,
    ControlRate, ControlTarget, ModulatorKind, ModulatorParams, OutletDescriptor,
    PublishesOutlets, RecordFace, SendTap, Takeover, TransportControl,
    SignalShape,
    modulation::outlet_slot,
    aux_in, AuxInParams, EdgeRefusal,
    ds01, Ds01Params,
    NoteEvent,
    NoteId, NotePriority, OscWave, ParamAddr,
    ParamCurve, ParamDescriptor, ParamOwner, PointId,
    Project, ProjectChannel, RetriggerMode, SampleReference,
    PlayMode, SamplerParams, SnareCharacter, StretchMode,
    SAMPLER_TUNE_SEMITONE_CLAMP,
    VoiceMode, MAX_SLICES,
    DEFAULT_STEPS, DEFAULT_SWING_PERCENT, MASTER_BUS, MAX_BUSES,
    MAX_CHANNELS, MAX_MODULATORS_PER_CHANNEL,
    MAX_MOD_ROUTES_PER_CHANNEL,
    MOD_STEP_MAX_STEPS,
    MAX_SAMPLER_VOICES, MAX_STRETCH_BARS, MAX_STRETCH_GRAIN, MAX_STRETCH_RATIO,
    MIN_STRETCH_BARS, MIN_STRETCH_GRAIN, MIN_STRETCH_RATIO,
    MAX_PATTERNS, MAX_PATTERN_STEPS, MAX_PLAYLIST_BARS,
    MAX_POLY_VOICES, STRIP_DESCRIPTORS,
    TICKS_PER_64TH, TICKS_PER_BAR, TICKS_PER_STEP,
};
use mooloop_dsp::{
    buffer_allocation_key, build_effect_at_tempo, ChannelAudioSnapshot, Ds01, DrumSynth,
    IntegerDelay, SampleData, SpectrumAnalyzer, StretchPool,
};
use mooloop_engine::{
    CommandSink, ContainerScratch, EffectSlot, EngineHandle, ExportSpec, OfflineRenderer,
    PreviewCommand, StructuralCommand,
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
use mooloop_session::effects::EffectParamWrite;
use mooloop_session::dialogs::{
    pick_bundle_dialog, pick_export_dialog, pick_sample_dialog,
    pick_save_dialog, pick_song_dialog, Picked,
};
use mooloop_session::document::{
    chosen_path, log_asset_warnings, log_repairs, quarantine_song, repair_suffix,
    resolve_document, warning_suffix, DocumentProblem,
    DocumentResult, LoadTarget, PresetNaming, ResolvedDocument,
};
use mooloop_session::engine::{
    discard_document_messages, publish_channel_audio_to, AudioAction, AudioActionSender,
    ChannelAudio, ChannelAudioSender, EngineCommandSender, PendingEngineMessage, PreviewSender,
    ProjectEditSender, StructuralCommandSender, TelemetryAction, TelemetryActionSender,
};
use mooloop_session::history::{Entry as HistoryEntry, Stream};
use mooloop_session::recordings;
use mooloop_session::roll::NoteEdit;
use mooloop_session::steps::StepEdit;
use mooloop_session::take::{FinishedTake, TakeRecorder};
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
use settings::{AppearanceSettings, LayoutSettings, ThemePalette, UiSettings};
use slint::{
    CloseRequestResponse, ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode,
    VecModel,
};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

const PUMP_INTERVAL_MS: u64 = 8;
const INITIAL_BPM: i32 = 120;

/// Fixed buffer size choices offered by the segmented control on the Audio
/// preferences page. Index-addressed to match `SegmentedControl`.
const BUFFER_SIZES: [u32; 6] = [64, 128, 256, 512, 1024, 2048];

/// What the Audio preferences page says that depends on the driver this build
/// was compiled with. Spelled here, once, so the markup names no driver.
struct DriverCopy {
    name: &'static str,
    note: &'static str,
    targets_empty: &'static str,
    buffer_note: &'static str,
    auto_reconnect_hint: &'static str,
    sample_rate_source: &'static str,
}

#[cfg(not(target_os = "macos"))]
const DRIVER_COPY: DriverCopy = DriverCopy {
    name: "JACK",
    note: "ALSA support is planned.",
    targets_empty: "No connectable JACK inputs found.",
    buffer_note: "Changes the buffer for every JACK client on this machine.",
    auto_reconnect_hint: "When the output playing goes away, moves to the most recent output picked here that is still there. An output that is playing is never moved.",
    sample_rate_source: "set by the JACK server",
};

#[cfg(target_os = "macos")]
const DRIVER_COPY: DriverCopy = DriverCopy {
    name: "Core Audio",
    note: "",
    targets_empty: "No audio output devices found.",
    buffer_note: "Changes the buffer of the device mooloop plays through, and nothing else.",
    auto_reconnect_hint: "Returns to the output above when its device comes back — for example after unplugging and replugging it. Until then mooloop plays through the system default.",
    sample_rate_source: "the system output's rate when mooloop started",
};

impl DriverCopy {
    fn to_slint(&self) -> AudioDriverCopy {
        AudioDriverCopy {
            name: self.name.into(),
            note: self.note.into(),
            targets_empty: self.targets_empty.into(),
            buffer_note: self.buffer_note.into(),
            auto_reconnect_hint: self.auto_reconnect_hint.into(),
        }
    }
}
const DRUM_PREVIEW_BINS: usize = 144;

/// Bins in DS-01's rendered hit. Wider than v1's because its scope is wider:
/// the AMP page gives it half of a four-unit face rather than a corner of a
/// three-unit one.
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

/// How many points of the compressor's curve the face is handed. The
/// display draws 140 samples of its own, so anything past a hundred-odd is
/// resampling noise; 64 is the last power of two that reads smooth at the
/// 80px the paned face gives it.
const STRIP_CURVE_SAMPLES: usize = 64;

/// How many points of an EQ's magnitude response a response plot is handed.
///
/// One per column `EqResponseDisplay` draws, so its nearest-point lookup
/// lands on a published sample rather than between two of them. A frequency
/// plot cannot take the compressor curve's sixty-four: a 72 dB/oct pass
/// filter falls most of the plot's height inside two columns, and a curve
/// sampled coarser than it is drawn would step down that edge.
const EQ_CURVE_SAMPLES: usize = 140;

/// How many floats a response plot's handle takes for one band, and for one
/// pass filter.
///
/// A flat array with an implicit layout, produced here and consumed in
/// `device-displays.slint`, which is the shape `02-the-curve-tells-the-truth`
/// opened on: the pass filters used to be *appended to the band array* at a
/// different stride, so one model had two layouts and nothing asserted either.
/// They have their own property now, and `tests/eq_face.rs` reads both strides
/// back out of the markup.
///
/// Three and two rather than five and four because the curve is no longer
/// drawn from them: a handle needs a position, a height and whether it exists,
/// and a pass filter has no height of its own -- it rides the response.
pub const EQ_PLOT_BAND_STRIDE: usize = 3;
pub const EQ_PLOT_PASS_STRIDE: usize = 2;

/// The gain axis a response plot places a band's handle on, in decibels
/// either side of flat.
///
/// `EqResponseDisplay`'s convention rather than a parameter range -- but it
/// **coincides** with one, and the coincidence is load-bearing in both faces:
/// a dragged point reports its height as 0..1 and that number is written
/// straight to the band's Gain parameter, so the axis and the range have to
/// be the same span or a point dragged to the top writes something other than
/// the top. `a_dragged_point_writes_the_parameter_it_looks_like` is what
/// holds the two together, on both banks.
///
/// Note that it is *not* the plot's vertical span, which is wider because
/// bands sum: `EqResponseDisplay.ceiling-db`.
const EQ_PLOT_GAIN_DB: f32 = 18.0;

/// One band's handle, as `EqResponseDisplay` reads it: where it sits on the
/// plot's frequency axis, where it sits on the gain axis, and whether to draw
/// it at all.
///
/// Shared by the seven-band device and the channel strip because it is one
/// display, and a flat array with an implicit layout written out twice is the
/// fault this codebase keeps finding. It carried a band's Q and kind as well
/// until 2026-09-14, for a curve approximation in the markup that no longer
/// exists.
pub fn eq_plot_band(frequency_hz: f32, gain_db: f32, enabled: bool) -> [f32; EQ_PLOT_BAND_STRIDE] {
    [
        mooloop_core::eq_plot_position(frequency_hz),
        (gain_db + EQ_PLOT_GAIN_DB) / (2.0 * EQ_PLOT_GAIN_DB),
        if enabled { 1.0 } else { 0.0 },
    ]
}

/// One pass filter's handle. It has no gain, so it rides the response curve
/// rather than an axis of its own and needs no height here.
pub fn eq_plot_pass(frequency_hz: f32, enabled: bool) -> [f32; EQ_PLOT_PASS_STRIDE] {
    [
        mooloop_core::eq_plot_position(frequency_hz),
        if enabled { 1.0 } else { 0.0 },
    ]
}

/// One track's strip, as the faces take it.
///
/// Public for the reason `install_strip_spec` is: `tests/mixer_snapshot.rs`
/// renders a real strip on a real face, and building the row by hand there
/// would be a second answer to what the response plot's band array means.
///
/// Natural units, because that is what the engine holds and what the
/// readouts say; the knobs normalize against the descriptor table the
/// `StripSpec` global carries. The two derived fields are the ones a face
/// cannot compute: the response plot's flat band array, and the compressor's
/// curve as the voicing is actually bending it.
pub fn strip_row(params: &StripParams, sample_rate: u32) -> StripRow {
    let table = mooloop_dsp::strip::strip_voicing(params.voicing).eq;
    let mut band_data = Vec::with_capacity(STRIP_EQ_BANDS * EQ_PLOT_BAND_STRIDE);
    let mut positions = Vec::with_capacity(STRIP_EQ_BANDS);
    let mut frequencies = Vec::with_capacity(STRIP_EQ_BANDS);
    let mut gains = Vec::with_capacity(STRIP_EQ_BANDS);
    let mut qs = Vec::with_capacity(STRIP_EQ_BANDS);
    let mut shelves = Vec::with_capacity(STRIP_EQ_BANDS);
    for (index, band) in params.bands.iter().enumerate() {
        // The knob holds the position and reads out the hertz, which only
        // the voicing's table knows.
        let frequency_hz = table.frequency(index, band.position);
        positions.push(band.position as f32);
        frequencies.push(frequency_hz);
        gains.push(band.gain_db);
        qs.push(band.q);
        shelves.push(band.kind != mooloop_core::EqBandKind::Bell);
        // `EqResponseDisplay`'s own convention, the same one the EQ device's
        // rows use: where the handle goes and whether to draw it. It carried
        // the band's Q and kind as well until 2026-09-14, for a curve
        // approximation in the markup that no longer exists -- the curve is
        // sampled below, from the coefficients the strip is running.
        band_data.extend_from_slice(&eq_plot_band(frequency_hz, band.gain_db, true));
    }
    StripRow {
        voicing: params.voicing.to_index(),
        pre_in: params.pre_in,
        drive_db: params.drive_db,
        eq_in: params.eq_in,
        band_position: positions.as_slice().into(),
        band_frequency_hz: frequencies.as_slice().into(),
        band_gain_db: gains.as_slice().into(),
        band_q: qs.as_slice().into(),
        band_shelf: shelves.as_slice().into(),
        band_data: band_data.as_slice().into(),
        eq_curve_db: mooloop_dsp::strip::strip_eq_response_db(
            params,
            sample_rate,
            EQ_CURVE_SAMPLES,
        )
        .as_slice()
        .into(),
        comp_in: params.comp_in,
        threshold_db: params.threshold_db,
        ratio: params.ratio,
        attack_ms: params.attack_ms,
        release_ms: params.release_ms,
        knee_db: params.knee_db,
        mix: params.mix,
        makeup_db: params.makeup_db,
        curve_db: mooloop_dsp::strip::static_curve_db(
            params,
            METER_FLOOR_DB,
            STRIP_CURVE_SAMPLES,
        )
        .as_slice()
        .into(),
    }
}

/// Hand the markup the EQ's table, every label it draws, and every target's
/// resting values.
///
/// `install_strip_spec`'s argument, on the other device whose face is a view
/// over a table: a knob's range and an automation lane's range are two views
/// of one number, and a number written twice is how they come to disagree.
/// The EQ needed it for a second reason the strip does not have -- its face
/// is one control set over *nine* targets, so a resting value is a column of
/// nine rather than a number, and spelling one in the markup meant a
/// double-click returned to band 2's default whatever band was selected.
///
/// Public for the reason `install_strip_spec` is: `tests/eq_face.rs` reads
/// the table back out of the window and holds it to the descriptors.
/// Generic over the window because the EQ's face is driven by two of them:
/// the application's, and `EqDeviceDragHarness`, which exists so a drag test
/// can send real pointer events at the real face. A harness that gets a blank
/// table does not draw a wrong number, it divides by a target count of zero.
pub fn install_eq_spec<'a, C>(window: &'a C)
where
    C: slint::ComponentHandle,
    EqSpec<'a>: slint::Global<'a, C>,
{
    let spec = window.global::<EqSpec>();
    let kind = mooloop_core::EffectKind::Eq;

    // A control's range is read off the first target that has that control
    // at all, rather than from a hand-written mapping: a pass filter has no
    // gain and a band has no slope, and `id_for_selected` already knows
    // which is which. Every band's Freq descriptor has every other band's
    // range -- `eq_band_shape` writes it once -- so which target answers does
    // not matter, only that one does.
    let targets = 0..=EqParams::LOW_PASS_TARGET;
    let controls: Vec<EqControlSpec> = (0..EQ_FACE_CONTROLS as u32)
        .map(|index| {
            let descriptor = EqFaceControl::from_face_index(index)
                .and_then(|control| {
                    targets
                        .clone()
                        .find_map(|target| EqParams::id_for_selected(target, control))
                })
                .and_then(|id| kind.descriptor(id));
            match descriptor {
                Some(descriptor) => EqControlSpec {
                    unit: descriptor.unit.into(),
                    minimum: descriptor.min,
                    maximum: descriptor.max,
                    logarithmic: matches!(descriptor.curve, ParamCurve::Exponential),
                },
                // Face index 0 is the target selector, which is not a
                // parameter and has not been one since `eq-v2/01`.
                None => EqControlSpec::default(),
            }
        })
        .collect();
    spec.set_controls(controls.as_slice().into());

    // Every target's resting position for every control, in the order the
    // markup indexes them: `target * EQ_FACE_CONTROLS + face index`.
    let mut defaults = Vec::with_capacity((EqParams::LOW_PASS_TARGET + 1) * EQ_FACE_CONTROLS);
    for target in targets.clone() {
        for index in 0..EQ_FACE_CONTROLS as u32 {
            let rest = EqFaceControl::from_face_index(index)
                .and_then(|control| EqParams::id_for_selected(target, control))
                .and_then(|id| kind.descriptor(id))
                .map(|descriptor| descriptor.to_normalized(descriptor.default))
                .unwrap_or_default();
            defaults.push(rest);
        }
    }
    spec.set_defaults(defaults.as_slice().into());

    spec.set_band_count(mooloop_core::EQ_MAX_BANDS as i32);
    spec.set_target_count(EqParams::LOW_PASS_TARGET as i32 + 1);

    // The selector's labels. A band is called by the number its own
    // parameters are called by -- band 0's descriptors are "B1 Freq" and
    // friends -- so the face and the automation menu count the same way. The
    // first button said LOW until 2026-09-14, which made them count
    // differently from the day per-band ids landed.
    let target_names: Vec<slint::SharedString> = targets
        .clone()
        .map(|target| match target {
            _ if target == EqParams::HIGH_PASS_TARGET => "HP".into(),
            _ if target == EqParams::LOW_PASS_TARGET => "LP".into(),
            band => format!("{}", band + 1).into(),
        })
        .collect();
    spec.set_target_names(target_names.as_slice().into());

    // The slope selector, from what the bank actually rolls off at rather
    // than from what the enum's variants are spelled. See
    // `EqSlope::db_per_octave`.
    let slope_names: Vec<slint::SharedString> = mooloop_core::EqSlope::all()
        .iter()
        .map(|slope| format!("{}", slope.db_per_octave()).into())
        .collect();
    spec.set_slope_names(slope_names.as_slice().into());
}

/// Hand the markup the strip's parameter table and every id it addresses.
///
/// Public for the reason `effect_kind_index` is: a UI test should reach the
/// strip the way the application does, and `tests/strip_face.rs` holds the
/// table it installs to `StripParams::descriptors()`.
///
/// Called once, before the window is shown. It is why no range, default or
/// id is spelled in `strip.slint`: `StripSpec.drive-db` *is*
/// `STRIP_DRIVE_DB`, and a knob's minimum *is* its descriptor's -- the
/// duplication `AGENTS.md` opens on, closed by construction rather than by a
/// test that notices afterwards.
pub fn install_strip_spec(window: &MainWindow) {
    let spec = window.global::<StripSpec>();
    let params: Vec<StripParamSpec> = StripParams::descriptors()
        .iter()
        .map(|descriptor| StripParamSpec {
            id: descriptor.id as i32,
            name: descriptor.name.into(),
            unit: descriptor.unit.into(),
            minimum: descriptor.min,
            maximum: descriptor.max,
            default_value: descriptor.default,
            steps: match descriptor.curve {
                ParamCurve::Stepped(steps) => steps as i32,
                _ => 0,
            },
            logarithmic: matches!(descriptor.curve, ParamCurve::Exponential),
        })
        .collect();
    spec.set_params(params.as_slice().into());
    spec.set_first(STRIP_FIRST as i32);
    spec.set_voicing(STRIP_VOICING as i32);
    spec.set_pre_in(STRIP_PRE_IN as i32);
    spec.set_drive_db(STRIP_DRIVE_DB as i32);
    spec.set_eq_in(STRIP_EQ_IN as i32);
    spec.set_band_base(strip_band_param(0, STRIP_BAND_FREQ) as i32);
    spec.set_band_stride(STRIP_BAND_STRIDE as i32);
    spec.set_band_frequency(STRIP_BAND_FREQ as i32);
    spec.set_band_gain(STRIP_BAND_GAIN as i32);
    spec.set_band_q(STRIP_BAND_Q as i32);
    spec.set_band_kind(STRIP_BAND_KIND as i32);
    spec.set_comp_in(STRIP_COMP_IN as i32);
    spec.set_threshold_db(STRIP_COMP_THRESHOLD_DB as i32);
    spec.set_ratio(STRIP_COMP_RATIO as i32);
    spec.set_attack_ms(STRIP_COMP_ATTACK_MS as i32);
    spec.set_release_ms(STRIP_COMP_RELEASE_MS as i32);
    spec.set_knee_db(STRIP_COMP_KNEE_DB as i32);
    spec.set_mix(STRIP_COMP_MIX as i32);
    spec.set_makeup_db(STRIP_COMP_MAKEUP_DB as i32);
    let voicings: Vec<slint::SharedString> = [
        mooloop_core::PreampVoicing::Moo,
        mooloop_core::PreampVoicing::Grip,
        mooloop_core::PreampVoicing::Punch,
        mooloop_core::PreampVoicing::Iron,
    ]
    .iter()
    .map(|voicing| voicing.label().into())
    .collect();
    spec.set_voicings(voicings.as_slice().into());
    // The bands' own names, in the reading order the faces draw them in.
    // Spelled once, here, rather than in three faces.
    let band_names: Vec<slint::SharedString> = ["HIGH SHELF", "HIGH MID", "LOW MID", "LOW SHELF"]
        .iter()
        .map(|name| (*name).into())
        .collect();
    spec.set_band_names(band_names.as_slice().into());
    // The pin, which both the rack's drawing and the engine's block loop
    // read from `mooloop_core::mixer::STRIP_PIN`.
    window.set_strip_pin_head(mooloop_core::mixer::STRIP_PIN == mooloop_core::mixer::StripPin::Head);
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

/// Turns the MIDI learn arm on or off on both sides of the boundary.
///
/// The toolbar button reads a window property and every parameter control
/// reads the `ControlAssign` global. They are one fact, so it is written in
/// one place: a control left armed after the button went dark would name a
/// parameter the next time it was touched, and nothing on screen would say
/// why.
fn set_midi_learn_armed(window: &MainWindow, armed: bool) {
    window.set_midi_learn_armed(armed);
    window.global::<ControlAssign>().set_midi_learn(armed);
}

/// Pushes one appearance state -- colors and the shared radius scale -- into
/// the running window. Live preview and a committed save both go through here,
/// so what the user sees while dragging is exactly what Apply persists.
fn apply_appearance(window: &MainWindow, appearance: &AppearanceSettings) {
    apply_theme(window, appearance.palette());
    let theme = window.global::<Theme>();
    theme.set_roundness(appearance.roundness);
    theme.set_type_scale(appearance.type_scale);
    theme.set_density(appearance.density);
    theme.set_font_family(appearance.font_family.as_str().into());
    theme.set_font_family_mono(appearance.font_family_mono.as_str().into());
    theme.set_font_weight(appearance.font_weight);
    theme.set_hairline(appearance.hairline);
    theme.set_stroke_emphasis(appearance.stroke_emphasis);
    // The swatch palette follows the colourscheme, so it is pushed from the
    // same funnel the palette is: selecting Nord has to change the channel
    // pickers in the same frame it changes everything else, and a live preview
    // has to show it.
    window.set_color_choices(ModelRc::from(Rc::new(VecModel::from(
        channel_colors::color_choices(&appearance.swatches()),
    ))));
    window
        .global::<DisplayPrefs>()
        .set_smooth_curves(appearance.smooth_curves);
    // Motion deliberately not applied here: this also runs on every live
    // color preview, which would clobber the Appearance page's in-progress
    // motion edits. Motion reaches the window through
    // `sync_preferences_properties` (startup, cancel, and Apply) instead.
}

/// Reads back the Appearance page's live, uncommitted state. The dialog holds
/// the edit in its properties until Apply, so this is what preview, theme
/// selection, and Save Theme all have to work from.
fn window_appearance(window: &MainWindow, stored: &AppearanceSettings) -> AppearanceSettings {
    let motion = window.global::<Motion>();
    AppearanceSettings {
        theme: window.get_preferences_appearance_theme().into(),
        customized: window.get_preferences_appearance_customized(),
        mode: crate::theme::Mode::from_index(window.get_preferences_appearance_mode())
            .name()
            .to_owned(),
        base: window.get_preferences_appearance_base().into(),
        accent: window.get_preferences_appearance_accent().into(),
        alert: window.get_preferences_appearance_alert().into(),
        contrast: window.get_preferences_appearance_contrast(),
        roundness: window.get_preferences_appearance_roundness(),
        type_scale: window.get_preferences_appearance_type_scale(),
        density: window.get_preferences_appearance_density(),
        font_family: window.get_preferences_appearance_font_family().into(),
        font_family_mono: window.get_preferences_appearance_font_family_mono().into(),
        font_weight: window.get_preferences_appearance_font_weight(),
        hairline: window.get_preferences_appearance_hairline(),
        stroke_emphasis: window.get_preferences_appearance_stroke_emphasis(),
        smooth_curves: window.get_preferences_smooth_curves(),
        motion_speed: settings::motion_speed_name(motion.get_speed()).to_owned(),
        motion_easing: settings::motion_easing_name(motion.get_easing()).to_owned(),
        // From the live global, not from `stored`, for the same reason motion
        // is: this is what a scheme selection and Save Scheme are built from,
        // so reading the persisted value here would quietly undo a falloff
        // the user had just picked every time they touched the palette.
        meter_falloff: settings::meter_falloff_name(
            window.global::<MeterPrefs>().get_falloff(),
        )
        .to_owned(),
        user_schemes: stored.user_schemes.clone(),
    }
}

/// One row per theme, with the swatches drawn from the variant the mode
/// resolves to -- so the list repaints when Light/Dark/Auto moves, and a theme
/// shows what choosing it would actually do.
fn theme_rows(appearance: &AppearanceSettings) -> ModelRc<AppearanceSchemeRow> {
    let dark = appearance.wants_dark();
    let rows: Vec<AppearanceSchemeRow> = appearance
        .themes()
        .into_iter()
        .map(|theme| {
            let ramp = theme.variant(dark).ramp();
            AppearanceSchemeRow {
                base: ramp.slot(0).color(),
                accent: ramp.accent().color(),
                alert: ramp.slot(0x0A).color(),
                is_user: !crate::theme::catalog::is_protected(&theme.name),
                // A derived variant is not the scheme's own, and somebody
                // comparing it against a screenshot deserves to know which
                // one they are looking at.
                derived: !theme.authored(dark),
                description: theme.description.as_str().into(),
                name: theme.name.into(),
            }
        })
        .collect();
    ModelRc::from(Rc::new(VecModel::from(rows)))
}

/// The sixteen slots of the ramp in force, for the strip the Appearance page
/// draws under the theme list. Read-only: a ramp is edited by editing its
/// file, and the three seed pickers are the in-app way to change colours.
fn ramp_swatches(appearance: &AppearanceSettings) -> ModelRc<slint::Color> {
    let ramp = appearance.ramp();
    let colors: Vec<slint::Color> = (0..16).map(|slot| ramp.slot(slot).color()).collect();
    ModelRc::from(Rc::new(VecModel::from(colors)))
}

/// The three colour pickers, the ramp strip and the theme name: everything
/// that moves when the *colours* change but not when a fader does.
fn push_appearance_colors(window: &MainWindow, appearance: &AppearanceSettings) {
    window.set_preferences_appearance_theme(appearance.theme.as_str().into());
    window.set_preferences_appearance_customized(appearance.customized);
    window.set_preferences_appearance_mode(appearance.mode().index());
    window.set_preferences_appearance_base(appearance.base.as_str().into());
    window.set_preferences_appearance_accent(appearance.accent.as_str().into());
    window.set_preferences_appearance_alert(appearance.alert.as_str().into());
    window.set_preferences_appearance_schemes(theme_rows(appearance));
    window.set_preferences_appearance_ramp(ramp_swatches(appearance));
    window.set_preferences_appearance_variant_derived(variant_is_derived(appearance));
    push_appearance_contrast(window, appearance);
    push_appearance_swatches(window, appearance);
}

/// The quick swatches beside the three colour fields.
///
/// **Base offers the built-in themes' backgrounds; accent and alert offer the
/// hues of the scheme in force.** Both rows used to be six hex literals in the
/// markup, which was defensible when the page's own colours were the only
/// palette there was and stopped being so the moment a theme could be Nord:
/// the accent row was suggesting a lime while Nord was on screen.
///
/// Six of each, because that is what the row draws. The hues are slots 08-0D
/// -- red, orange, yellow, green, cyan, blue -- which every ramp has and which
/// are in a predictable order, so the row does not reshuffle itself as the
/// theme changes.
fn push_appearance_swatches(window: &MainWindow, appearance: &AppearanceSettings) {
    let dark = appearance.wants_dark();
    let mut bases: Vec<ColorChoice> = Vec::new();
    for theme in appearance.themes() {
        let background = theme.variant(dark).ramp().slot(0);
        let value: SharedString = background.to_hex().into();
        if bases.iter().all(|choice| choice.value != value) {
            bases.push(ColorChoice {
                value,
                tint: background.color(),
            });
        }
        if bases.len() == 6 {
            break;
        }
    }
    let ramp = appearance.ramp();
    let hues: Vec<ColorChoice> = (0x08..=0x0D)
        .map(|slot| ColorChoice {
            value: ramp.slot(slot).to_hex().into(),
            tint: ramp.slot(slot).color(),
        })
        .collect();
    window.set_preferences_appearance_base_choices(ModelRc::from(Rc::new(VecModel::from(bases))));
    window.set_preferences_appearance_hue_choices(ModelRc::from(Rc::new(VecModel::from(hues))));
}

/// The two WCAG ratios the Appearance page reports.
///
/// **The palette derivation has been able to compute this since it was first
/// written and nothing ever asked it.** A user could pick seeds that produce
/// an unreadable interface and the page would show it to them without
/// comment, which is `04-accessibility.md`'s first item. It costs two
/// divisions over `theme::color::relative_luminance`.
fn push_appearance_contrast(window: &MainWindow, appearance: &AppearanceSettings) {
    let ramp = appearance.ramp();
    window.set_preferences_appearance_text_ratio(ramp.text_contrast(appearance.contrast));
    window.set_preferences_appearance_accent_ratio(ramp.accent_contrast(appearance.contrast));
}

/// Whether the variant on screen was worked out rather than written down.
/// Only meaningful while a theme is actually being worn: a Custom palette is
/// three colours somebody typed and is not derived from anything.
fn variant_is_derived(appearance: &AppearanceSettings) -> bool {
    !appearance.customized
        && appearance
            .definition()
            .is_some_and(|theme| !theme.authored(appearance.wants_dark()))
}

/// The scalars a theme may carry. Pushed only when a theme is *selected*,
/// never on an ordinary preview: these are the controls the user is dragging,
/// and writing them back mid-drag is how a slider fights the pointer.
fn push_appearance_scalars(window: &MainWindow, appearance: &AppearanceSettings) {
    window.set_preferences_appearance_contrast(appearance.contrast);
    window.set_preferences_appearance_roundness(appearance.roundness);
    window.set_preferences_appearance_type_scale(appearance.type_scale);
    window.set_preferences_appearance_density(appearance.density);
    window.set_preferences_appearance_font_family(appearance.font_family.as_str().into());
    window.set_preferences_appearance_font_family_mono(appearance.font_family_mono.as_str().into());
    window.set_preferences_appearance_font_weight(appearance.font_weight);
    window.set_preferences_appearance_hairline(appearance.hairline);
    window.set_preferences_appearance_stroke_emphasis(appearance.stroke_emphasis);
}

fn sync_preferences_properties(window: &MainWindow, settings: &UiSettings) {
    let appearance = &settings.appearance;
    window.set_preferences_appearance_theme(appearance.theme.as_str().into());
    window.set_preferences_appearance_customized(appearance.customized);
    window.set_preferences_appearance_mode(appearance.mode().index());
    window.set_preferences_appearance_schemes(theme_rows(appearance));
    window.set_preferences_appearance_ramp(ramp_swatches(appearance));
    window.set_preferences_appearance_base(appearance.base.as_str().into());
    window.set_preferences_appearance_accent(appearance.accent.as_str().into());
    window.set_preferences_appearance_alert(appearance.alert.as_str().into());
    window.set_preferences_appearance_contrast(appearance.contrast);
    window.set_preferences_appearance_roundness(appearance.roundness);
    window.set_preferences_appearance_type_scale(appearance.type_scale);
    window.set_preferences_appearance_density(appearance.density);
    window.set_preferences_appearance_font_family(appearance.font_family.as_str().into());
    window
        .set_preferences_appearance_font_family_mono(appearance.font_family_mono.as_str().into());
    window.set_preferences_appearance_font_weight(appearance.font_weight);
    window.set_preferences_appearance_hairline(appearance.hairline);
    window.set_preferences_appearance_stroke_emphasis(appearance.stroke_emphasis);
    window.set_preferences_appearance_variant_derived(variant_is_derived(appearance));
    push_appearance_contrast(window, appearance);
    push_appearance_swatches(window, appearance);
    window.set_preferences_developer_mode(settings.general.developer_mode);
    window.set_preferences_log_path(settings::log_path().display().to_string().into());
    window.set_snap_to_zero(settings.general.snap_markers_to_zero);
    window.set_preferences_smooth_curves(appearance.smooth_curves);
    window
        .global::<DisplayPrefs>()
        .set_smooth_curves(appearance.smooth_curves);
    let motion = window.global::<Motion>();
    motion.set_speed(settings::motion_speed_index(&appearance.motion_speed));
    motion.set_easing(settings::motion_easing_index(&appearance.motion_easing));
    window
        .global::<MeterPrefs>()
        .set_falloff(settings::meter_falloff_index(&appearance.meter_falloff));
    window.set_preferences_error("".into());
    window.set_preferences_audio_driver(DRIVER_COPY.to_slint());
    let buffer_index = settings
        .audio
        .active()
        .buffer_size
        .and_then(|frames| BUFFER_SIZES.iter().position(|&f| f == frames))
        .map(|i| i as i32)
        .unwrap_or(-1);
    window.set_preferences_audio_buffer_size_index(buffer_index);
    window.set_preferences_audio_auto_reconnect(settings.audio.active().auto_reconnect);
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
                // Empty for a global action rather than the word
                // "Anywhere" on thirty-odd rows: the column exists to mark
                // the exceptions, and a value on every row would stop
                // marking anything.
                context: if spec.scope == actions::Scope::Anywhere {
                    Default::default()
                } else {
                    spec.scope.label().into()
                },
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

/// Re-read live driver status and connectable output targets, and push them
/// onto the window. Called from the pump, which is the only place that holds
/// `EngineHandle`; a non-realtime driver query, not something to run every
/// tick.
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
    let buffer_index = BUFFER_SIZES
        .iter()
        .position(|&f| f == status.buffer_size)
        .map(|i| i as i32)
        .unwrap_or(-1);
    window.set_preferences_audio_buffer_size_index(buffer_index);
    window.set_preferences_audio_sample_rate_text(
        format!("{} Hz — {}", status.sample_rate, DRIVER_COPY.sample_rate_source).into(),
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

/// The view ids `main.slint`'s `PaneViews` global declares, in the order
/// `Ctrl+1..5` already used, so a chord is `Ctrl+(id + 1)`.
///
/// Public because the UI tests reveal a view the same way the application
/// does. They used to set `editor_page` and `mixer_visible` directly, which
/// stopped being expressible once a view could move between panes.
pub mod view {
    pub const STEPS: i32 = 0;
    pub const MIXER: i32 = 1;
    pub const DEVICES: i32 = 2;
    pub const NOTES: i32 = 3;
    pub const PLAYLIST: i32 = 4;
}

/// Restore the pane arrangement. Called once at startup, from a
/// `LayoutSettings` that `sanitized()` has already made coherent -- so this
/// only has to hand the numbers over, not defend against them.
///
/// Zoom is not restored, because it is not saved: see `LayoutSettings`.
fn apply_layout(window: &MainWindow, layout: &LayoutSettings) {
    let slots = &layout.view_slots;
    window.set_steps_slot(slots[view::STEPS as usize]);
    window.set_mixer_slot(slots[view::MIXER as usize]);
    window.set_devices_slot(slots[view::DEVICES as usize]);
    window.set_notes_slot(slots[view::NOTES as usize]);
    window.set_playlist_slot(slots[view::PLAYLIST as usize]);
    window.set_main_active(layout.slot_active[0]);
    window.set_split_active(layout.slot_active[1]);
    window.set_bottom_active(layout.slot_active[2]);
    window.set_split_fraction(layout.split_fraction);
    window.set_steps_dock_height(layout.steps_dock_height);
    window.set_mixer_dock_height(layout.mixer_dock_height);
    window.set_notes_dock_height(layout.notes_dock_height);
    window.set_playlist_dock_height(layout.playlist_dock_height);
    window.set_bottom_pane_visible(layout.bottom_pane_visible);
    window.set_sidebar_visible(layout.sidebar_visible);
    window.set_sidebar_width(layout.sidebar_width);
    window.set_channel_sidebar_visible(layout.channel_sidebar_visible);
    window.set_channel_sidebar_width(layout.channel_sidebar_width);
}

/// Read the arrangement back off the window. The window is the live truth
/// while the app runs; this is only ever called to write that truth down.
fn read_layout(window: &MainWindow) -> LayoutSettings {
    LayoutSettings {
        view_slots: vec![
            window.get_steps_slot(),
            window.get_mixer_slot(),
            window.get_devices_slot(),
            window.get_notes_slot(),
            window.get_playlist_slot(),
        ],
        slot_active: vec![
            window.get_main_active(),
            window.get_split_active(),
            window.get_bottom_active(),
        ],
        split_fraction: window.get_split_fraction(),
        steps_dock_height: window.get_steps_dock_height(),
        mixer_dock_height: window.get_mixer_dock_height(),
        notes_dock_height: window.get_notes_dock_height(),
        playlist_dock_height: window.get_playlist_dock_height(),
        bottom_pane_visible: window.get_bottom_pane_visible(),
        sidebar_visible: window.get_sidebar_visible(),
        sidebar_width: window.get_sidebar_width(),
        channel_sidebar_visible: window.get_channel_sidebar_visible(),
        channel_sidebar_width: window.get_channel_sidebar_width(),
    }
}

/// The one place `Pane` and the Slint view ids meet.
fn view_id(pane: Pane) -> i32 {
    match pane {
        Pane::Steps => view::STEPS,
        Pane::Mixer => view::MIXER,
        Pane::Source => view::DEVICES,
        Pane::Notes => view::NOTES,
        Pane::Playlist => view::PLAYLIST,
    }
}

/// Reveal a pane. It used to have to say *where* -- clear `mixer_visible`,
/// then set an `editor_page` index -- which stopped being expressible once a
/// view could be moved between panes. The view now knows which slot it is in,
/// so this asks for the view and nothing else.
fn apply_pane(window: &MainWindow, pane: Pane) {
    window.invoke_show_view(view_id(pane));
}

fn project_snapshot(state: &UiState, window: &MainWindow) -> ProjectSnapshot {
    let mut project = state.session.project_snapshot(window.get_bpm(), window.get_swing_percent());
    normalize_project_pattern_banks(&mut project);
    ProjectSnapshot {
        project,
        samples: state.session.keyed_sample_snapshots(),
    }
}

fn queue_project_edit(
    tx: &ProjectEditSender,
    before: ProjectSnapshot,
    after: ProjectSnapshot,
    status: &'static str,
) -> bool {
    queue_structural_edit(tx, before, after, status, None)
}

/// [`queue_project_edit`] for an edit that moved the channel or track list,
/// carrying the edit so the pump can renumber the session state the snapshot
/// does not contain. See `ProjectEdit::edit`.
fn queue_structural_edit(
    tx: &ProjectEditSender,
    before: ProjectSnapshot,
    after: ProjectSnapshot,
    status: &'static str,
    edit: Option<ListEdit>,
) -> bool {
    let entry = HistoryEntry {
        before,
        after: after.clone(),
        label: status,
        gesture: None,
    };
    // The engine addresses channels by seat, so the keyed table is flattened
    // once, here, against the project it is being installed with.
    let samples = after.seated();
    tx.send(ProjectEdit {
        project: after.project,
        samples,
        status: status.into(),
        history: Some((HistoryMove::Record, entry)),
        edit,
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
    let samples = snapshot.seated();
    tx.send(ProjectEdit {
        project: snapshot.project,
        samples,
        status,
        history: Some((movement, entry)),
        // An undo restores a whole document rather than applying an edit to
        // one, so there is no edit to follow.
        edit: None,
    })
}

fn sync_command_availability(window: &MainWindow, commands: &CommandState) {
    window.set_can_undo(!commands.project_edit_pending && commands.history.can_undo());
    window.set_can_redo(!commands.project_edit_pending && commands.history.can_redo());
    window.set_channel_clipboard_available(commands.channel_clipboard.is_some());
    window.set_project_edit_pending(commands.project_edit_pending);
}

/// How far a meter's dB must move before rewriting its model row is worth the
/// repaint it costs.
///
/// **This was one segment of the meter that draws it, until 2026-09-15.** Two
/// constants mirrored `mixer.slint` and `device-rack.slint`, and
/// `slint_meter_segment_counts_match_the_throttle` held them together. Meters
/// became continuous bars that day and the quantum stopped existing: a bar
/// moves at every dB, so a throttle still working in fourteenths of the scale
/// would have drawn the new continuous meter in fourteen visible steps. The
/// bars would have been redrawn and nothing would have looked any smoother --
/// the change landing invisibly, which is worse than it failing.
///
/// A quarter of a decibel is one pixel on the widest meter here -- the
/// transport's 200px master, 60 dB across -- and finer than a pixel on every
/// other. It costs nothing in the case the throttle exists for, a meter
/// sitting still; while one is falling, every tick moves it and is meant to.
const METER_DISPLAY_STEP_DB: f32 = 0.25;

/// Clamped to the drawn range first, which is what the segment count used to
/// do for free: a meter decaying from -70 to -90 dB is not moving on screen,
/// and repainting it would be work for a change nobody can see.
fn meter_display_changed(previous: f32, next: f32) -> bool {
    let drawn = |db: f32| db.clamp(METER_FLOOR_DB, 0.0);
    (drawn(previous) - drawn(next)).abs() >= METER_DISPLAY_STEP_DB
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

/// Which panel a `Scope::Focused` action resolves against.
///
/// The roll wins whenever it is on screen with something selected, ahead of
/// whatever was clicked last. That is the rule the clipboard chords have
/// shipped with since 2026-09-07, and it is still the right one: a user who
/// has just dragged a marquee is not thinking about the browser row they
/// opened before it. Everything else is the stored surface, which falls
/// back to the channel list -- what these chords meant before there was a
/// second clipboard to mean.
fn focused_surface(window: &MainWindow) -> actions::Surface {
    if notes_have_focus(window) {
        return actions::Surface::Notes;
    }
    actions::Surface::from_name(window.get_focused_surface().as_str())
}

/// Records where the user just clicked, so the next contextual chord knows.
fn set_focused_surface(window: &MainWindow, surface: actions::Surface) {
    window.set_focused_surface(surface.name().into());
}

fn notes_have_focus(window: &MainWindow) -> bool {
    window.get_showing_notes() && window.get_has_note_selection()
}

/// A browser row's kind, as `BrowserRow.kind` spells it. One model serves
/// both tabs (`build_browser_rows`, `build_preset_rows`), so the keyboard
/// asks the same questions the row's own `TouchArea` does -- a preset (3)
/// needs no constant, because it is what a row that is not one of these is.
const BROWSER_FOLDER: i32 = 0;
const BROWSER_SAMPLE: i32 = 1;
const BROWSER_GROUP: i32 = 2;

fn browser_row_at(st: &Rc<RefCell<UiState>>, index: i32) -> Option<BrowserRow> {
    let index = usize::try_from(index).ok()?;
    st.borrow().browser_rows.row_data(index)
}

fn browser_row_count(st: &Rc<RefCell<UiState>>) -> i32 {
    st.borrow().browser_rows.row_count() as i32
}

fn browser_row_expands(row: &BrowserRow) -> bool {
    row.kind == BROWSER_FOLDER || row.kind == BROWSER_GROUP
}

/// Whether landing on a row with the arrows should audition it -- inspect
/// the file, fill the info pane, and, with the preview armed, sound it.
///
/// Only a sample row, and the exclusion is the point rather than an
/// oversight: a *preset* row's click loads the preset into the selected
/// channel, so a walk down the PRESETS tab that did what a click does would
/// install a device per keypress. A folder or a group opens on Right or
/// Enter and has nothing to hear. Every sample row is playable already --
/// `build_browser_rows` filters the tree to the formats that decode -- so
/// the kind is the whole question.
fn browser_row_auditions(kind: i32) -> bool {
    kind == BROWSER_SAMPLE
}

/// Where the keyboard's row lands when it moves by `delta`, or `None` if it
/// does not move. Pure, and separate from the window for that reason: the
/// enter-from-either-end rule and the clamp are the whole of the behaviour
/// and neither needs a rendered tree to be checked.
///
/// An unset focus (`current < 0`) enters from whichever end the key came
/// from, so the first Down after Ctrl+B lands on the first row rather than
/// the second.
fn browser_focus_step(count: i32, current: i32, delta: i32) -> Option<i32> {
    if count <= 0 {
        return None;
    }
    let next = if current < 0 {
        if delta > 0 {
            0
        } else {
            count - 1
        }
    } else {
        (current + delta).clamp(0, count - 1)
    };
    (next != current).then_some(next)
}

/// The row containing the one at `index`: the nearest earlier row that is
/// shallower. The model is flattened, so a row carries no pointer to its
/// parent and this is the only way to ask.
fn browser_parent_of(depths: &[i32], index: usize) -> Option<usize> {
    let depth = *depths.get(index)?;
    (0..index).rev().find(|candidate| depths[*candidate] < depth)
}

/// Move the keyboard's row, auditioning whatever it lands on.
///
/// The audition is what makes the arrows a way of *listening* through a
/// folder rather than a way of pointing at it: Adam asked for Up/Down to
/// play what they select, and this is the only place a keyboard move
/// happens, so Left and Right get it too where they fall through to a step.
/// It goes through `browser-row-previewed`, the same callback the row's own
/// click invokes, so the arrows and the pointer cannot drift into meaning
/// two different things -- including the arm: that callback inspects, and
/// only an armed preview turns the inspection into sound.
fn browser_move_focus(st: &Rc<RefCell<UiState>>, window: &MainWindow, delta: i32) -> bool {
    let Some(next) = browser_focus_step(
        browser_row_count(st),
        window.get_browser_focus_index(),
        delta,
    ) else {
        return false;
    };
    window.set_browser_focus_index(next);
    if let Some(row) = browser_row_at(st, next) {
        if browser_row_auditions(row.kind) {
            window.invoke_browser_row_previewed(row.path.clone());
        }
    }
    true
}

/// Collapsing a folder takes rows away underneath the keyboard, so the row
/// it is on can end up past the end of a shorter tree. Called after every
/// toggle rather than guarded for at every read.
fn browser_clamp_focus(st: &Rc<RefCell<UiState>>, window: &MainWindow) {
    let count = browser_row_count(st);
    let current = window.get_browser_focus_index();
    if current >= count {
        window.set_browser_focus_index((count - 1).max(-1));
    }
}

/// Right: open a closed folder, otherwise step into it. Left: close an open
/// one, otherwise climb to its parent -- found by walking back to the first
/// shallower row, because the model is flattened and a row does not carry a
/// pointer to the one that contains it.
fn browser_step_horizontally(
    st: &Rc<RefCell<UiState>>,
    window: &MainWindow,
    forward: bool,
) -> bool {
    let index = window.get_browser_focus_index();
    let Some(row) = browser_row_at(st, index) else {
        return browser_move_focus(st, window, if forward { 1 } else { -1 });
    };
    if browser_row_expands(&row) && row.expanded != forward {
        window.invoke_browser_row_toggled(row.path.clone());
        browser_clamp_focus(st, window);
        return true;
    }
    if forward {
        return browser_move_focus(st, window, 1);
    }
    let depths: Vec<i32> = st
        .borrow()
        .browser_rows
        .iter()
        .map(|row| row.depth)
        .collect();
    match browser_parent_of(&depths, index.max(0) as usize) {
        Some(parent) => {
            window.set_browser_focus_index(parent as i32);
            true
        }
        None => false,
    }
}

/// Enter. The same three answers the row's own click gives, so a row does
/// not mean one thing to the pointer and another to the keyboard.
fn browser_activate_focused(st: &Rc<RefCell<UiState>>, window: &MainWindow) -> bool {
    let Some(row) = browser_row_at(st, window.get_browser_focus_index()) else {
        return false;
    };
    if browser_row_expands(&row) {
        window.invoke_browser_row_toggled(row.path.clone());
        browser_clamp_focus(st, window);
    } else if row.kind == BROWSER_SAMPLE {
        window.invoke_browser_row_previewed(row.path.clone());
    } else if row.loadable {
        window.invoke_browser_preset_loaded(row.path.clone());
    } else {
        return false;
    }
    true
}

/// Ctrl+Enter: the load the sample row's context menu offers, and the only
/// thing a preset row can do, so the chord is never dead on a leaf.
fn browser_load_focused(st: &Rc<RefCell<UiState>>, window: &MainWindow) -> bool {
    let Some(row) = browser_row_at(st, window.get_browser_focus_index()) else {
        return false;
    };
    if row.kind == BROWSER_SAMPLE {
        window.invoke_browser_sample_loaded(row.path.clone());
    } else if !browser_row_expands(&row) && row.loadable {
        window.invoke_browser_preset_loaded(row.path.clone());
    } else {
        return false;
    }
    true
}

/// Ctrl+B. Reveals the panel as well as aiming the keys at it: a shortcut
/// that silently targets a collapsed sidebar is indistinguishable from one
/// that does nothing.
fn browser_take_focus(st: &Rc<RefCell<UiState>>, window: &MainWindow) {
    window.set_sidebar_visible(true);
    set_focused_surface(window, actions::Surface::Browser);
    if window.get_browser_focus_index() < 0 && browser_row_count(st) > 0 {
        window.set_browser_focus_index(0);
    }
}

/// The rack device a `Scope::Rack` action acts on.
fn selected_device_slot(st: &Rc<RefCell<UiState>>) -> Option<i32> {
    st.borrow()
        .session
        .selected_device_slot()
        .and_then(|slot| i32::try_from(slot).ok())
}

/// Says in the status bar why a note edit was refused, when the session
/// recorded a reason -- a full pattern (MOO-133) -- and reports whether it
/// did, so a caller with a message of its own for the ordinary no-op can
/// fall back to it.
fn report_note_refusal(session: &mut Session, window: &MainWindow) -> bool {
    match session.take_note_refusal() {
        Some(message) => {
            window.set_status_message(message.as_str().into());
            true
        }
        None => false,
    }
}

fn record_project_history(
    commands: &Rc<RefCell<CommandState>>,
    before: ProjectSnapshot,
    state: &Rc<RefCell<UiState>>,
    window: &MainWindow,
    label: &'static str,
) {
    let gesture = commands.borrow().gesture;
    record_project_history_as(commands, before, state, window, label, gesture);
}

/// [`record_project_history`] under a gesture token the caller chose, rather
/// than the piano roll's pointer gesture. An entry carrying the same token as
/// the one below it is folded into it by `History::record`.
fn record_project_history_as(
    commands: &Rc<RefCell<CommandState>>,
    before: ProjectSnapshot,
    state: &Rc<RefCell<UiState>>,
    window: &MainWindow,
    label: &'static str,
    gesture: Option<u64>,
) {
    let after = project_snapshot(&state.borrow(), window);
    {
        let mut open = commands.borrow_mut();
        open.history.record(HistoryEntry {
            before,
            after,
            label,
            gesture,
        });
    }
    sync_command_availability(window, &commands.borrow());
}

/// Start collecting a stream of pump-side edits into one undo entry.
///
/// `before` is the document as the stream found it, taken before the first
/// edit landed. See [`Stream`] for what a stream is and why a stream is one
/// entry rather than one per message or none at all.
fn open_edit_stream(
    commands: &Rc<RefCell<CommandState>>,
    window: &MainWindow,
    stream: Stream,
    before: ProjectSnapshot,
    label: &'static str,
) {
    let mut open = commands.borrow_mut();
    open.history.open(stream, before, label);
    sync_command_availability(window, &open);
}

/// Record the open stream, if there is one, at the document as it is now.
///
/// Anything that records goes through `History::record`, which closes a
/// stream by itself at the new entry's `before`. What needs this is what
/// *reads* the history rather than adding to it: Undo and Redo, and the
/// pump when a stream has gone quiet.
fn close_edit_stream(
    state: &Rc<RefCell<UiState>>,
    commands: &Rc<RefCell<CommandState>>,
    window: &MainWindow,
) {
    if commands.borrow().history.open_stream().is_none() {
        return;
    }
    let after = project_snapshot(&state.borrow(), window);
    let mut open = commands.borrow_mut();
    open.history.close(after);
    sync_command_availability(window, &open);
}

/// How long a mapped hardware control has to rest before its moves are one
/// undo step.
///
/// A desk has no press and release to say where a gesture ends, so the pause
/// has to. Half a second is the gap the issue asked for (MOO-96) and the one
/// [`VALUE_RUN_GAP`] uses for a run of wheel notches: long enough that one
/// sweep with a hand repositioned mid-way is one step, short enough that the
/// next deliberate move is another.
const CONTROLLER_IDLE: std::time::Duration = std::time::Duration::from_millis(500);

/// Close a stream whose edits have stopped arriving: a controller that has
/// been still for [`CONTROLLER_IDLE`], or a take of notes whose transport has
/// stopped or whose record arm is off.
///
/// Called on every pump tick, after the control drain, so notes the engine
/// reports in the same tick as the stop are in the take they were played in.
fn settle_edit_streams(
    state: &Rc<RefCell<UiState>>,
    commands: &Rc<RefCell<CommandState>>,
    window: &MainWindow,
    playing: bool,
    controller_idle: bool,
) {
    let settled = match commands.borrow().history.open_stream() {
        Some(Stream::Controller) => controller_idle,
        Some(Stream::Recording) => !(playing && state.borrow().session.record_armed()),
        None => false,
    };
    if settled {
        close_edit_stream(state, commands, window);
    }
}

/// What one pump's worth of control-surface input did, for the pump to act
/// on once the session is no longer borrowed.
#[derive(Default)]
struct ControlDrain {
    /// Engine commands, in the order the messages produced them.
    commands: Vec<EngineCommand>,
    /// `(source, target)` of each binding a learn gesture completed.
    learned: Vec<(String, String)>,
    /// Whether anything visible moved, so the editor is republished.
    moved: bool,
    /// Whether the document changed at all.
    edited: bool,
    /// Whether a mapped control moved a parameter, which restarts the
    /// controller's idle clock.
    controller_moved: bool,
    /// Channels a recorded note was written to.
    written: Vec<usize>,
    /// Why a recorded note was refused -- a full pattern (MOO-133) -- said
    /// once for the drain rather than once per key.
    note_refusal: Option<String>,
}

/// Apply the control messages and recorded notes the engine forwarded, and
/// put every document change they made into the history (MOO-96, MOO-97).
///
/// These used to be applied, marked dirty and recorded nowhere, so the next
/// Ctrl+Z -- which installs a snapshot taken before them -- destroyed them
/// with no redo: eight knobs mapped in one LEARN pass, gone with an undo of
/// an earlier step edit; sixteen bars played in, gone with an undo of a stray
/// note before them.
///
/// - **A learned binding is one entry**, labelled "MIDI learn". It is a
///   discrete edit, like a menu pick.
/// - **A mapped control's moves are one entry per gesture**, held open as a
///   [`Stream::Controller`] until the controls go idle for
///   [`CONTROLLER_IDLE`] (see [`settle_edit_streams`]). One snapshot opens
///   it and one closes it, however many messages a sweep sends.
/// - **Recorded notes are one entry per take**, a [`Stream::Recording`] that
///   stays open until the transport stops or recording is disarmed.
///
/// The snapshot a stream opens with has to be the document *before* the
/// first edit, so it is taken before anything is applied -- and only when
/// [`Session::control_input_may_edit`] says one of the messages can change
/// the document, because a transport message or an unmapped knob must not
/// cost a whole-project clone every tick.
///
/// A free function rather than the pump's body so it can be tested without
/// an `EngineHandle`: the commands come back for the pump to send.
fn drain_control_surface(
    state: &Rc<RefCell<UiState>>,
    commands: &Rc<RefCell<CommandState>>,
    window: &MainWindow,
    messages: &mut Vec<mooloop_core::MidiMessage>,
    recorded: &mut Vec<(u8, u8, u8, u8, u32, u32)>,
    playing: bool,
) -> ControlDrain {
    let mut drain = ControlDrain::default();
    if !messages.is_empty() {
        let ports = state.borrow().midi_ports.clone();
        let (may_edit, learning) = {
            let st = state.borrow();
            (
                messages
                    .iter()
                    .any(|message| st.session.control_input_may_edit(message, &ports)),
                st.session.control_learn.is_some(),
            )
        };
        let streaming = commands.borrow().history.open_stream() == Some(Stream::Controller);
        // A learn is its own entry, so it needs its own `before` even when a
        // controller stream is already open; a move inside an open stream
        // needs nothing.
        let before =
            (may_edit && (learning || !streaming)).then(|| project_snapshot(&state.borrow(), window));
        let mut first_moved = None;
        {
            let mut st = state.borrow_mut();
            for message in messages.drain(..) {
                let effects = st.session.apply_control_input(&message, &ports, playing);
                drain.commands.extend(effects.commands.iter().copied());
                if let Some(binding) = &effects.learned {
                    drain.learned.push((
                        binding.source.detail_label(),
                        st.session.control_target_label(&binding.target),
                    ));
                }
                first_moved = first_moved.or(effects.moved.first().copied());
                drain.moved |= !effects.is_empty();
                // A parameter moved by a knob is an edit; a transport gesture
                // is not. `ControlEffects` has already drawn that line.
                drain.edited |= effects.edits;
            }
            if drain.edited {
                st.session.mark_dirty();
            }
        }
        if !drain.learned.is_empty() {
            if let Some(before) = before {
                record_project_history(commands, before, state, window, "MIDI learn");
            }
        } else if drain.edited {
            drain.controller_moved = true;
            if !streaming {
                if let Some(before) = before {
                    // The parameter's own name, as a knob on screen records
                    // under: the descriptor table cannot drift from the face.
                    let label = first_moved
                        .and_then(|address| state.borrow().session.param_descriptor(address))
                        .map_or("Controller move", |descriptor| descriptor.name);
                    open_edit_stream(commands, window, Stream::Controller, before, label);
                }
            }
        }
    }
    if !recorded.is_empty() {
        let taking = commands.borrow().history.open_stream() == Some(Stream::Recording);
        let before = (!taking).then(|| project_snapshot(&state.borrow(), window));
        let mut wrote = false;
        {
            let mut st = state.borrow_mut();
            for (channel, pattern, note, velocity, start, length) in recorded.drain(..) {
                let channel = usize::from(channel);
                let Some(edit) = st.session.record_note(
                    channel,
                    usize::from(pattern),
                    note,
                    velocity,
                    start,
                    length,
                ) else {
                    continue;
                };
                drain.commands.extend(edit.commands.iter().copied());
                drain.written.push(channel);
                wrote = true;
            }
            drain.note_refusal = st.session.take_note_refusal();
            if wrote {
                st.session.mark_dirty();
            }
        }
        drain.edited |= wrote;
        if wrote {
            if let Some(before) = before {
                open_edit_stream(commands, window, Stream::Recording, before, "Record notes");
            }
        }
    }
    drain
}

/// Snapshot, run one console or rack verb, and record the undo entry for it.
///
/// Eleven mixer verbs -- mute, volume, pan, bus pick, console on, polarity,
/// solo -- sent their command and dirtied the document without recording
/// anything, so a later undo installed a snapshot taken before them and put
/// every one of them back (`reports/fable-2026-09-21.md`, finding 7). `edit`
/// answers whether anything actually changed; a refused verb records nothing.
fn with_project_history(
    state: &Rc<RefCell<UiState>>,
    commands: &Rc<RefCell<CommandState>>,
    window: &MainWindow,
    label: &'static str,
    edit: impl FnOnce() -> bool,
) {
    let before = project_snapshot(&state.borrow(), window);
    if !edit() {
        return;
    }
    record_project_history(commands, before, state, window, label);
}

/// [`with_project_history`] for an edit that may be part of a gesture.
///
/// The general recorder. Its whole rule is four lines and the fourth is what
/// makes the other three cheap:
///
/// - **Gesture open, first change:** the `before` was snapshotted when the
///   gesture opened; apply the edit, name the entry, record nothing yet.
/// - **Gesture open, later changes:** apply the edit. Nothing else.
/// - **Gesture closes:** one entry, that `before` to the project as it now
///   is. See [`gesture_closed`].
/// - **No gesture open:** snapshot, apply, record — one entry, exactly as a
///   discrete edit already does.
///
/// That last line is why a wheel notch, an arrow-key nudge, a menu pick and
/// a colour swatch are each correct without bracketing anything, and it is
/// why a control that has not yet learned to call [`Gesture`] is no worse
/// off than it was.
///
/// **One snapshot pair per gesture, not per frame.** This is what it buys
/// over `History::record`'s token coalescing (`history.rs`), which works —
/// the piano roll uses it — but pays two whole-project clones on every move
/// frame and then throws all but the first and last away.
///
/// It replaced a 400 ms timer that guessed where a drag ended, because the
/// mixer faces had no press/release callback to ask. They have one now:
/// `MixerFader`'s `pointer-event` grew an `.up` arm, and `MiniKnob` already
/// carried the pair.
/// A generator parameter's own name, for the undo entry it records.
///
/// The descriptor table is the one the face draws from, so a label taken
/// from it cannot drift from what the user is looking at -- which is the
/// whole reason not to write sixty literals instead. A kind whose table has
/// no such id falls back to the kind's own name rather than to nothing.
fn generator_param_label(kind: DeviceKind, id: u32) -> &'static str {
    kind.descriptors()
        .iter()
        .find(|descriptor| descriptor.id == id)
        .map_or(kind.label(), |descriptor| descriptor.name)
}

fn with_gesture_history(
    state: &Rc<RefCell<UiState>>,
    commands: &Rc<RefCell<CommandState>>,
    window: &MainWindow,
    label: &'static str,
    edit: impl FnOnce() -> bool,
) {
    with_gesture_history_named(state, commands, window, || edit().then_some(label))
}

/// [`with_gesture_history`] where the *edit* knows what to call the entry.
///
/// One caller today and it is the reason this exists:
/// `on_effect_param_changed` serves every parameter of every effect kind,
/// and the descriptor's own name is resolved inside the session, where the
/// id is. Naming it from the outside would mean the caller indexing
/// `descriptors()` by the face index — right on every effect but the EQ,
/// whose face indices are not descriptor positions, which is the
/// lost-semantic-type fault `AGENTS.md` records at this boundary.
fn with_gesture_history_named(
    state: &Rc<RefCell<UiState>>,
    commands: &Rc<RefCell<CommandState>>,
    window: &MainWindow,
    edit: impl FnOnce() -> Option<&'static str>,
) {
    if !state.borrow().session.gesture_open() {
        let before = project_snapshot(&state.borrow(), window);
        let Some(label) = edit() else {
            return;
        };
        record_project_history(commands, before, state, window, label);
        return;
    }
    let Some(label) = edit() else {
        return;
    };
    state.borrow_mut().session.mark_gesture_changed();
    // The first edit inside a gesture names it. A drag that crosses two
    // parameters — which nothing can do today, but a future control might —
    // is one entry under the first one's name rather than two entries.
    commands.borrow_mut().gesture_label.get_or_insert(label);
}

/// `Gesture.begin()`: a value edit is starting.
///
/// A begin while one is open closes it and hands the `before` back rather
/// than dropping it — see `Session::begin_gesture`, which is where that rule
/// lives so it can be tested without a window.
fn gesture_opened(
    state: &Rc<RefCell<UiState>>,
    commands: &Rc<RefCell<CommandState>>,
    window: &MainWindow,
) {
    let snapshot = project_snapshot(&state.borrow(), window);
    let abandoned = state.borrow_mut().session.begin_gesture(snapshot);
    let label = commands.borrow_mut().gesture_label.take();
    if let Some(before) = abandoned {
        record_project_history(commands, before, state, window, label.unwrap_or("Edit"));
    }
}

/// `Gesture.end()`: record the whole gesture as one entry.
///
/// Nothing open, or nothing changed while it was, records nothing — a press
/// and release on a knob is not an edit, and neither is a MIDI learn press.
fn gesture_closed(
    state: &Rc<RefCell<UiState>>,
    commands: &Rc<RefCell<CommandState>>,
    window: &MainWindow,
) {
    let Some(before) = state.borrow_mut().session.finish_gesture() else {
        return;
    };
    // A gesture the modulation shelf closed first already recorded under its
    // own name, and took the snapshot with it, so this never runs twice for
    // one press.
    let label = commands.borrow_mut().gesture_label.take().unwrap_or("Edit");
    // A wheel notch, an arrow press and a double-click reset are each a
    // gesture of their own, so a sweep of notches on one knob joins one run
    // and undoes as one step (MOO-95, `CommandState::value_run_token`).
    let scope = {
        let st = state.borrow();
        (st.session.selected, st.session.effect_target)
    };
    let token = commands
        .borrow_mut()
        .value_run_token(label, scope, std::time::Instant::now());
    record_project_history_as(commands, before, state, window, label, Some(token));
}

/// Append a channel, show it, tell the engine about it, and record the undo
/// entry for it.
///
/// Add is the one channel-structure verb that does not stop the song: it is
/// incremental rather than a whole-project reinstall, so it goes through
/// [`Session::add_channel`] and `StructuralCommandSender::add_channel` rather
/// than [`queue_structural_edit`]. That is why it is also the one verb that
/// used to reach the engine and the dirty flag without reaching the history --
/// an undo then installed some *earlier* edit's `before`, taking the new
/// channel and every unrecorded edit since it with no redo path.
///
/// The borrow order is the load-bearing part. `record_project_history` takes
/// `state.borrow()` and `commands.borrow_mut()` itself, so it must be called
/// with no `RefMut` on `UiState` alive -- which is a `RefCell` panic at
/// runtime, not a compile error. The mutation is therefore scoped to a block
/// and the recording happens after it.
///
/// A free function rather than the closure body so it can be tested without an
/// `AppUi`, which needs an `EngineHandle` and so a live audio driver.
fn add_channel_with_history(
    state: &Rc<RefCell<UiState>>,
    commands: &Rc<RefCell<CommandState>>,
    window: &MainWindow,
    tx: &StructuralCommandSender,
    reset_tx: &std::sync::mpsc::Sender<usize>,
    source: DeviceKind,
) -> Option<usize> {
    let before = project_snapshot(&state.borrow(), window);
    let index = {
        let mut st = state.borrow_mut();
        // A full rack adds nothing, so it records nothing either.
        let index = st.session.add_channel(source)?;
        log_debug!("ui", "add channel");
        let pattern = st.session.current_pattern;
        let ch = &st.session.channels[index];
        let cells: Vec<StepCell> = (0..st.session.pattern_lengths[pattern])
            .map(|step| rack_cell(&ch.notes[pattern], step))
            .collect();
        let model = Rc::new(VecModel::from(cells));
        let row = ChannelRow {
            name: ch.name.as_str().into(),
            color: channel_colors::to_slint(ch.color),
            has_color: ch.color.is_some(),
            track_color: channel_colors::to_slint(feeding_track_color(&st.session.buses, ch.bus)),
            has_track_color: feeding_track_color(&st.session.buses, ch.bus).is_some(),
            muted: false,
            solo: false,
            solo_silenced: false,
            volume_db: linear_to_db(ch.volume),
            pan: ch.pan,
            selected: true,
            bus: ch.bus as i32,
            steps: ModelRc::from(model.clone()),
        };
        st.rows.push(row);
        st.step_models.push(model);
        st.sync_row_flags();
        st.sync_mixer(window);
        window.set_selected_channel(index as i32);
        st.refresh_editor(window);
        index
    };
    let _ = reset_tx.send(index);
    let _ = tx.add_channel(index, source);
    // `after` is taken from the live session, which the incremental add has
    // already mutated -- there is no hand-built snapshot to get wrong. A click
    // carries no gesture token, so this never coalesces into the entry below
    // it and the add is its own undo step.
    record_project_history(commands, before, state, window, "Add channel");
    Some(index)
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
    // **A paste carries no foreign input picks.** Both fields name something
    // in the document the channel was copied *from*: paste into another song
    // and the same numbers name whatever that song happens to have there, so
    // a pasted channel arrived listening to a stranger
    // (`reports/fable-2026-09-21.md`, finding 7). Cleared on every paste
    // rather than only across documents, because the same-song case is not
    // sound either -- `rescope_after` renumbers routes and lanes and does not
    // touch these (`docs/LOOSE_ENDS.md`, "A pasted channel's inputs"). The
    // status message says so, so the pick is re-made deliberately.
    channel.setup.channel.audio_input = mooloop_core::AudioInputSource::Off;
    channel.setup.channel.midi_input = mooloop_core::midi::ChannelMidiInput::default();
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
    // The paste selects what it just made, and names it: `insert_channel`
    // minted its identity on the way in -- and the same id is what its audio
    // is filed under, so the sample table needs no insert.
    let pasted = project.channels[index].id;
    project.selected_channel = pasted;
    if let Some(sample) = clipboard.sample {
        samples.insert(pasted, sample);
    }
    queue_structural_edit(
        tx,
        before,
        ProjectSnapshot { project, samples },
        status,
        Some(ListEdit::Channel(ChannelEdit::Inserted(index as u8))),
    )
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
    let samples = before.samples.clone();
    if project.channels.len() <= 1 || index >= project.channels.len() {
        return false;
    }
    // The song drops what named this channel and renumbers what named the
    // ones after it; a lane left on the old index would otherwise automate
    // whichever channel slid into the seat.
    if project.remove_channel(index).is_none() {
        return false;
    }
    // Nothing removed from the sample table: it is keyed by channel, so the
    // departed channel's entry is simply never looked up again -- and an undo
    // brings the channel back to find its audio still there.
    //
    // No clamp here either. `remove_channel` moves the selection only when
    // it was the deleted channel that held it, which is the case this line
    // used to get wrong for every other channel.
    queue_structural_edit(
        tx,
        before,
        ProjectSnapshot { project, samples },
        status,
        Some(ListEdit::Channel(ChannelEdit::Removed(index as u8))),
    )
}

/// Move the channel at `from` to `to`, undoably.
///
/// The third channel edit, built on the same snapshot path as the two above
/// rather than on an incremental engine command, and the reason is that those
/// two do not have one either: `install_project_in_ui` rebuilds the whole
/// `RenderState`. A move through the same door is consistent with a paste
/// rather than being a special case, and an incremental rotate would have to
/// rotate `EngineHandle`'s sample and slice slots with the strips or hand the
/// moved channel its neighbour's audio. See
/// `docs/plans/archive/console/01-a-channel-can-be-moved.md`.
fn queue_channel_move(
    tx: &ProjectEditSender,
    state: &Rc<RefCell<UiState>>,
    window: &MainWindow,
    from: usize,
    to: usize,
    status: &'static str,
) -> bool {
    let before = {
        let state = state.borrow();
        project_snapshot(&state, window)
    };
    let mut project = before.project.clone();
    let samples = before.samples.clone();
    // The song renumbers every route, lane and Aux In subscription that named
    // a channel the move passed, and carries the mover's own with it.
    let Some(edit) = project.move_channel(from, to) else {
        return false;
    };
    // The sample table used to be rotated here, because it was parallel to
    // `project.channels` and hand maintained: miss this and every sampler
    // between the two seats plays the wrong file. Keyed by channel there is
    // no second list to move.
    //
    // The selection is untouched for the same reason: it names the channel
    // being dragged, which is still the same channel wherever it lands.
    queue_structural_edit(
        tx,
        before,
        ProjectSnapshot { project, samples },
        status,
        Some(ListEdit::Channel(edit)),
    )
}

/// Add a mixer track, undoably.
///
/// A whole-document edit rather than an incremental command, because the
/// engine builds a strip per track when a project loads -- `grow_buses` --
/// and there is no structural command that adds one. The same door
/// `queue_channel_insert` uses, and undoable for the same reason.
fn queue_track_add(
    tx: &ProjectEditSender,
    state: &Rc<RefCell<UiState>>,
    window: &MainWindow,
) -> bool {
    let before = {
        let state = state.borrow();
        project_snapshot(&state, window)
    };
    let mut project = before.project.clone();
    let samples = before.samples.clone();
    if project.add_track().is_none() {
        return false;
    }
    queue_project_edit(tx, before, ProjectSnapshot { project, samples }, "Track added")
}

/// Remove a mixer track, undoably.
///
/// `Project::remove_track` renumbers everything that named a later track and
/// falls anything routed *here* back to the master, so a channel does not go
/// silently unheard.
fn queue_track_remove(
    tx: &ProjectEditSender,
    state: &Rc<RefCell<UiState>>,
    window: &MainWindow,
    track: usize,
) -> bool {
    let before = {
        let state = state.borrow();
        project_snapshot(&state, window)
    };
    let mut project = before.project.clone();
    let samples = before.samples.clone();
    if project.remove_track(track).is_none() {
        return false;
    }
    // Carried, so the session's own track-keyed state -- the selected device,
    // the open lane, the preset labels -- is renumbered along with the song.
    queue_structural_edit(
        tx,
        before,
        ProjectSnapshot { project, samples },
        "Track removed",
        Some(ListEdit::Track(TrackEdit::Removed(track as u8))),
    )
}

/// Move a mixer track from seat `from` to seat `to`, undoably.
///
/// `Project::move_track` refuses the master's seat at either end and carries
/// everything that named the track. There is no samples sidecar to rotate, as
/// `queue_channel_move` has to: samples belong to channels. The rack follows
/// the moved track in the pump, through `Session::rescope_after_track`.
fn queue_track_move(
    tx: &ProjectEditSender,
    state: &Rc<RefCell<UiState>>,
    window: &MainWindow,
    from: usize,
    to: usize,
) -> bool {
    let before = {
        let state = state.borrow();
        project_snapshot(&state, window)
    };
    let mut project = before.project.clone();
    let samples = before.samples.clone();
    let Some(edit) = project.move_track(from, to) else {
        return false;
    };
    queue_structural_edit(
        tx,
        before,
        ProjectSnapshot { project, samples },
        "Track moved",
        Some(ListEdit::Track(edit)),
    )
}

/// Duplicates pattern `index` immediately after itself -- see
/// `Project::clone_pattern`, which moves every list parallel to the bank
/// together. The new clone becomes the selected pattern, mirroring
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
    if !project.clone_pattern(index) {
        return false;
    }
    queue_project_edit(tx, before, ProjectSnapshot { project, samples }, status)
}

/// Removes pattern `index` -- see `Project::remove_pattern`. Playlist
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
    if !project.remove_pattern(index) {
        return false;
    }
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

/// The colour of the track a channel feeds, which is what that channel's rack
/// plate is washed with.
///
/// A lookup rather than a field on the channel: a track's colour belongs to
/// the track, and a copy on every channel routed to it would be a copy to keep
/// in step with every reroute and every recolour. The rack rebuilds its rows
/// from the session anyway.
fn feeding_track_color(
    buses: &[mooloop_core::BusSetup],
    bus: u8,
) -> Option<mooloop_core::ProjectColor> {
    buses.get(bus as usize).and_then(|setup| setup.bus.color)
}

/// The takes made this run that nothing in the app still refers to
/// (`audio-recording/06`).
///
/// Crash leftovers are filtered out here rather than in the scan: what a quit
/// offers is this session's takes, and a file that may be the only copy of an
/// unsaved song's take is not the quit's to sweep. Those belong to File >
/// Clean Up Takes, which lists them unticked and says why.
fn unused_session_takes(st: &UiState, commands: &CommandState) -> Vec<recordings::UnusedTake> {
    let referenced = recordings::referenced_by(&st.session, &commands.history);
    recordings::unused_takes(
        &settings::recordings_dir(),
        &referenced,
        st.session_start,
    )
    .into_iter()
    .filter(|take| !take.from_earlier_session)
    .collect()
}

/// The takes dialog's state while it is up (MOO-38): the two lists, a tick
/// for each row, and whether it was opened by a quit -- which then goes on to
/// quit whichever button is pressed.
struct TakesReview {
    lists: recordings::CleanUp,
    not_used: Vec<bool>,
    earlier: Vec<bool>,
    quitting: bool,
}

impl TakesReview {
    /// "Not used by this song" starts ticked; "left from earlier sessions"
    /// does not, and says why (`recordings::EARLIER_SESSIONS_NOTE`).
    fn new(lists: recordings::CleanUp, quitting: bool) -> Self {
        Self {
            not_used: vec![true; lists.not_used.len()],
            earlier: vec![false; lists.earlier.len()],
            lists,
            quitting,
        }
    }

    fn toggle(&mut self, earlier: bool, index: usize) {
        let ticks = if earlier { &mut self.earlier } else { &mut self.not_used };
        if let Some(tick) = ticks.get_mut(index) {
            *tick = !*tick;
        }
    }

    /// Every ticked take, in list order.
    fn ticked(&self) -> Vec<recordings::UnusedTake> {
        let pick = |takes: &[recordings::UnusedTake], ticks: &[bool]| {
            takes
                .iter()
                .zip(ticks)
                .filter(|(_, ticked)| **ticked)
                .map(|(take, _)| take.clone())
                .collect::<Vec<_>>()
        };
        let mut ticked = pick(&self.lists.not_used, &self.not_used);
        ticked.extend(pick(&self.lists.earlier, &self.earlier));
        ticked
    }

    /// The running total the confirm button acts on.
    fn total(&self) -> String {
        let ticked = self.ticked();
        if ticked.is_empty() {
            "Nothing ticked".into()
        } else {
            format!("Move {} to the trash", recordings::summary(&ticked))
        }
    }
}

fn take_rows(takes: &[recordings::UnusedTake], ticks: &[bool]) -> ModelRc<TakeRow> {
    let rows: Vec<TakeRow> = takes
        .iter()
        .zip(ticks)
        .map(|(take, ticked)| TakeRow {
            name: take
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
                .into(),
            length: recordings::length_text(&take.path).into(),
            size: recordings::size_text(take.bytes).into(),
            date: recordings::date_text(take.modified).into(),
            ticked: *ticked,
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

/// Put `review` on screen: rows, ticks, the total and the buttons' words.
fn show_takes_review(window: &MainWindow, review: &TakesReview) {
    window.set_takes_session_rows(take_rows(&review.lists.not_used, &review.not_used));
    window.set_takes_earlier_rows(take_rows(&review.lists.earlier, &review.earlier));
    window.set_takes_earlier_note(recordings::EARLIER_SESSIONS_NOTE.into());
    window.set_takes_total(review.total().into());
    window.set_takes_can_confirm(!review.ticked().is_empty());
    if review.quitting {
        window.set_takes_title("Before you quit".into());
        window.set_takes_intro(
            "These takes were recorded this session and this song does not use them. Ticked \
             ones go to the desktop trash, where they can still be restored."
                .into(),
        );
        window.set_takes_confirm_label("Trash and Quit".into());
        window.set_takes_cancel_label("Keep and Quit".into());
    } else {
        window.set_takes_title("Clean up recordings".into());
        window.set_takes_intro(
            "Recordings nothing in this song or its undo history uses. Ticked ones go to the \
             desktop trash, where they can still be restored."
                .into(),
        );
        window.set_takes_confirm_label("Move to Trash".into());
        window.set_takes_cancel_label("Cancel".into());
    }
    window.set_takes_open(true);
}

/// Put the recordings folder right before anything records into it or reads
/// from it (MOO-75): move it out of the config directory, and patch the
/// header of any take a crash left unfinished.
///
/// At startup because nothing is writing takes yet, so a file's length on
/// disk is all it will ever hold. What was done is logged; what went wrong is
/// said in the status bar too, because a take stranded in the old folder is
/// one the clean-up dialog will not see.
fn prepare_recordings_folder(window: &MainWindow) {
    let folder = settings::recordings_dir();
    match recordings::migrate_folder(&settings::legacy_recordings_dir(), &folder) {
        Ok(true) => log_info!("ui", "moved the recordings folder to {}", folder.display()),
        Ok(false) => {}
        Err(error) => {
            log_error!("ui", "could not move the recordings folder: {error}");
            window.set_status_message(format!("Recordings folder not moved: {error}").into());
        }
    }
    let (repaired, failures) = mooloop_session::take::repair_headers(&folder);
    for path in &repaired {
        log_info!("ui", "repaired the header of {}, left unfinished by a crash", path.display());
    }
    for failure in &failures {
        log_error!("ui", "could not repair a take's header: {failure}");
    }
    if !repaired.is_empty() {
        let noun = if repaired.len() == 1 { "take" } else { "takes" };
        window.set_status_message(
            format!("Repaired {} {noun} a crash left unfinished", repaired.len()).into(),
        );
    }
}

pub struct AppUi {
    window: MainWindow,
    _pump: Timer,
    /// Held so [`Self::finish_takes`] can reach the recorder once the event
    /// loop has stopped, which is the last moment a take in flight can still
    /// be written out and reported.
    state: Rc<RefCell<UiState>>,
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

/// An envelope stage in seconds, on the range its descriptor declares.
///
/// The sampler's face used to carry these normalised and this side used to
/// map them onto a two-second range, while the table says 1 ms to 8 s in
/// ratio and the face printed the number against five. Three answers for one
/// value. The face carries seconds now, like every other generator's, so
/// there is nothing here to map -- only a clamp, so a value arriving from
/// anywhere lands inside the range the lane resolves against.
fn envelope_seconds(t: f32) -> f32 {
    t.clamp(ENV_MIN_SECONDS, ENV_MAX_SECONDS)
}

/// Number of parameter fields `EffectSlotRow` carries. Raising it means
/// adding matching `pN` fields to the Slint struct too.
const EFFECT_ROW_PARAMS: usize = 18;

/// How many of those a descriptor table may fill, **indexed by descriptor
/// id**. The last two are reserved for what a device keeps beside its
/// parameters -- a tempo-sync flag and a musical division, neither of which
/// is a continuous, addressable value. The delay's pair fits inside its six
/// descriptors; the modulation effect has eight of its own and does not,
/// which is why the reservation is a constant rather than "whatever is left".
///
/// **By id and not by position**, which used to be the same statement.
/// `modulation_allowed` and `destination_depths` have always indexed by id --
/// `descriptor_slots` sizes them `max(id) + 1` -- so a face reading
/// `p2` and `modulation-allowed[2]` was reading two different schemes that
/// happened to agree. The Buffer retiring `Offset` parted them: its table
/// starts at id 1 and reaches id 9 with a hole at 0. One scheme, and it is
/// the one an on-disk identifier already uses.
///
/// It reached fifteen on 2026-09-16, when the Buffer became three gestures
/// with settings of their own: eleven live parameters over four retired ids,
/// topping out at 14. A retired id is a hole here forever, which is the cost
/// of ids that outlive the control model that minted them and is cheaper than
/// the alternative -- a saved lane that silently means something else.
///
/// Sixteen since 2026-09-17: the Reverb's ids are 8..15 (0..7 are its
/// retired convolution-era parameters), so its Low Cut is id 15, which was
/// the reserved tempo-sync field while this was fifteen. Its face also read
/// `p0..p7` by position, so every knob opened at zero;
/// `tests/slint_face_agreement.rs` (`every_face_reads_its_parameters_by_id`)
/// now checks every face's reads against its ids.
const EFFECT_ROW_DESCRIPTOR_PARAMS: usize = EFFECT_ROW_PARAMS - 2;

/// The number that binds a kind to its face, and the only thing that does.
///
/// `main.slint` dispatches on it (`if slot.kind == 12 : PreampDeviceFace`)
/// and the insert menu in `device-rack.slint` calls `kind-selected` with it,
/// so a kind's number is a contract with two pieces of markup and nothing
/// else. It is a *runtime* binding -- a project persists the `EffectKind` by
/// its serde name and this is recomputed on every publish -- so renumbering
/// breaks no saved file. It still breaks both markup lists at once, so the
/// rule for a new kind is: **append the next free number, and leave the
/// existing ones alone.** Nothing here constrains the order of `ALL`, of the
/// insert menu, or of the arms in `main.slint`; the numbers are the only
/// agreement.
///
/// Public for the same reason [`view`] is: the UI tests address a device the
/// way the application does, rather than spelling the integer a second time
/// and drifting when it moves. `the_menu_and_the_faces_cover_every_kind`
/// (`tests/effect_preset_menu.rs`) checks this map against both markup lists.
pub fn effect_kind_index(kind: EffectKind) -> i32 {
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
        EffectKind::Preamp => 12,
        EffectKind::Chain => 13,
        EffectKind::Layer => 14,
    }
}

/// Rack units a kind's device face occupies. Devices with more working
/// controls take more width rather than compressing them (docs/UI_DESIGN.md,
/// "Device Rack Layout").
///
/// Public alongside [`effect_kind_index`], so a test fixture builds a row at
/// the width the application would give it rather than at a width of its own.
pub fn effect_kind_units(kind: EffectKind) -> i32 {
    match kind {
        EffectKind::Filter
        | EffectKind::Drive
        | EffectKind::Bitcrush
        | EffectKind::Preamp
        | EffectKind::Limiter => 1,
        // Nine parameters and a waveform. `UI_DESIGN.md` is explicit that a
        // face which outgrows its width takes another unit rather than
        // compressing, and the waveform earns its place here in a way it does
        // not on a synth: the memory *is* the instrument.
        EffectKind::Buffer => 2,
        EffectKind::Gate | EffectKind::Compressor | EffectKind::Plate => 2,
        EffectKind::Delay => 3,
        EffectKind::Reverb => 3,
        EffectKind::Modulation => 2,
        EffectKind::Eq => 2,
        // One unit: a name, a mix, and a collapse. The devices it holds are
        // rows of their own and carry their own width. A layer is the same
        // head -- what differs is how its branches are drawn, which is
        // `containers/09` and may yet want more height rather than more width.
        EffectKind::Chain | EffectKind::Layer => 1,
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

/// The containers whose run ends at `slot`, innermost first.
///
/// The rack draws each one's output rail here, past everything it holds. An
/// empty container closes on its own row: its span covers nothing, so there
/// is no later row for it to end at.
///
/// Innermost first is the greatest index first, because containers nest: the
/// rails then read outwards from the device, which is the order the boxes
/// close in.
fn containers_closing_at(effects: &[EffectSlotState], slot: usize) -> Vec<i32> {
    (0..=slot)
        .rev()
        .filter(|container| {
            effects
                .get(*container)
                .is_some_and(|effect| effect.params.is_container())
        })
        .filter(|container| {
            let span = mooloop_core::span_of(effects, *container);
            if span.is_empty() {
                *container == slot
            } else {
                span.end == slot + 1
            }
        })
        .map(|container| container as i32)
        .collect()
}

/// The parameter id behind each of the EQ face's controls, for the target it
/// is currently showing, indexed by the face's own control number.
///
/// `None` where the target has no such control: a pass filter has no gain and
/// no Q profile, and a band has no slope. Index 0 is the band selector, which
/// is not a parameter at all any more and so is never `Some`.
///
/// This is the whole of the EQ's 2026-09-14 change as the interface sees it.
/// The face did not move -- one Freq knob, one Gain knob, a selector saying
/// which band they are aimed at, which is the right interface and always was.
/// What moved is that the selection is resolved *here*, into a real per-band
/// id, instead of being a parameter the DSP had to consult before it knew what
/// a Freq event meant.
fn eq_face_ids(eq: &EqParams) -> [Option<u32>; EQ_FACE_CONTROLS] {
    let mut ids = [None; EQ_FACE_CONTROLS];
    let target = eq.selected_target();
    for index in 0..EQ_FACE_CONTROLS as u32 {
        if let Some(control) = EqFaceControl::from_face_index(index) {
            ids[index as usize] = EqParams::id_for_selected(target, control);
        }
    }
    ids
}

/// Re-index an EQ overlay array from descriptor ids to the face's control
/// numbers.
///
/// Every other face reads `modulation-allowed[<id>]` and the ids are dense
/// from zero, so the id *is* the index. The EQ's are not: fifty descriptors
/// spread over a strided space, behind seven controls. Rather than teach the
/// markup an id layout -- which would be this codebase's characteristic fault,
/// a table spelled in Rust and again in `.slint` -- the four overlay arrays
/// are gathered down to the seven the face actually reads, in the order it
/// reads them.
///
/// A control the current target does not have (a pass filter's gain) gathers
/// the default: no route, no depth, not allowed. The face already greys those
/// controls, so this agrees with what is drawn.
fn eq_overlay_view<T: Copy + Default>(ids: &[Option<u32>; EQ_FACE_CONTROLS], full: &[T]) -> Vec<T> {
    ids.iter()
        .map(|id| {
            id.and_then(|id| full.get(id as usize).copied())
                .unwrap_or_default()
        })
        .collect()
}

/// The parameter id a face's control number names, for one effect.
///
/// For every kind but the EQ a face already sends the id -- the ids are dense
/// from zero and the control's index *is* its id, which is why the markup can
/// write `modulation-allowed[2]` and mean it. The EQ's seven controls are a
/// view over fifty descriptors, so its number is resolved against the
/// selection here, in the one place that knows what the face is showing.
fn effect_face_param_id(effect: &EffectSlotState, control: u32) -> Option<u32> {
    match effect.params.eq() {
        Some(eq) => eq_face_ids(eq)
            .get(control as usize)
            .copied()
            .flatten(),
        None => Some(control),
    }
}

/// Where a row sits in the chain it is being drawn into, as opposed to what
/// device is in it.
///
/// Four facts that travel together and are all about the *chain* rather than
/// the slot, grouped when the sample rate made this function's argument list
/// eight long: a row builder nobody can call correctly by eye is one that
/// will eventually be called wrongly.
struct RackPlacement {
    depth: i32,
    /// The containers that close at this row.
    closing: Vec<i32>,
    selected: bool,
    /// Whether wrapping this row would leave every container inside
    /// `MAX_CONTAINER_DEPTH`. Answered by `mooloop_core::can_wrap` rather than
    /// by comparing `depth` here, so the cap is not a second number in the
    /// interface -- the markup asks this and the gesture asks the same
    /// function, which is what stopped the button lying.
    wrap_enabled: bool,
}

fn effect_slot_row(
    slot: &EffectSlotState,
    presets: &[PresetSummary],
    preset_name: Option<&str>,
    placement: RackPlacement,
    // What the engine is running at, for the EQ's response curve: the plot
    // is the bank's own coefficients evaluated, and coefficients are designed
    // against a sample rate.
    sample_rate: u32,
) -> EffectSlotRow {
    let RackPlacement {
        depth,
        closing,
        selected,
        wrap_enabled,
    } = placement;
    let kind = slot.kind();
    let preset_options: Vec<slint::SharedString> = effect_presets_of_kind(presets, kind)
        .map(preset_menu_label)
        .collect();
    let mut p = [0.0f32; EFFECT_ROW_PARAMS];
    if let Some(eq) = slot.params.eq() {
        // The EQ's face is a view over one target and its controls are
        // numbered by `EqFaceControl`, not by descriptor position -- there
        // are fifty descriptors and seven controls. See
        // `docs/plans/eq-v2/01-per-band-parameters.md`.
        for (index, id) in eq_face_ids(eq).into_iter().enumerate() {
            let Some(id) = id else { continue };
            let (Some(descriptor), Some(natural)) = (kind.descriptor(id), slot.params.get(id))
            else {
                continue;
            };
            p[index] = descriptor.to_normalized(natural);
        }
        p[EqFaceControl::SELECTOR as usize] =
            eq.selected_target() as f32 / EqParams::LOW_PASS_TARGET as f32;
    } else {
        for descriptor in kind.descriptors() {
            let slot_index = descriptor.id as usize;
            if slot_index >= EFFECT_ROW_DESCRIPTOR_PARAMS {
                continue;
            }
            if let Some(natural) = slot.params.get(descriptor.id) {
                p[slot_index] = descriptor.to_normalized(natural);
            }
        }
        debug_assert!(
            kind.descriptors()
                .iter()
                .all(|descriptor| (descriptor.id as usize) < EFFECT_ROW_DESCRIPTOR_PARAMS),
            "{} has a parameter id past what EffectSlotRow can carry",
            kind.label()
        );
    }
    // The two reserved fields, for the two devices that follow the tempo.
    if let Some(delay) = slot.params.delay() {
        p[EFFECT_ROW_DESCRIPTOR_PARAMS] = if delay.tempo_sync { 1.0 } else { 0.0 };
        p[EFFECT_ROW_DESCRIPTOR_PARAMS + 1] = delay.time_division.to_index() as f32;
    }
    if let mooloop_core::EffectParams::Modulation(modulation) = &slot.params {
        p[EFFECT_ROW_DESCRIPTOR_PARAMS] = if modulation.tempo_sync { 1.0 } else { 0.0 };
        p[EFFECT_ROW_DESCRIPTOR_PARAMS + 1] = modulation.rate_division.to_index() as f32;
    }
    // The response plot's three arrays. Two of them place handles -- where
    // a band or a pass filter sits and whether to draw it -- and the third
    // is the bank's magnitude response, which is the filter the engine is
    // running, evaluated. The pass filters were appended to the band array
    // at their own stride until 2026-09-14: one model with two layouts,
    // written here and read in `device-displays.slint`, with nothing
    // asserting the two files agreed.
    let mut eq_band_data = Vec::new();
    let mut eq_band_kinds = Vec::new();
    let mut eq_pass_data = Vec::new();
    let mut eq_curve_db = Vec::new();
    if let Some(eq) = slot.params.eq() {
        for band in eq.bands {
            eq_band_data
                .extend_from_slice(&eq_plot_band(band.frequency_hz, band.gain_db, band.enabled));
            // Not part of the plot's flat array, which carries only what
            // places a handle: the face's target row reads this to draw a
            // shelf as a shelf, and the band's kind moved out of
            // `eq_plot_band` on 2026-09-14 when the markup stopped drawing
            // its own curve.
            eq_band_kinds.push(band.kind.to_index());
        }
        for pass in [&eq.high_pass, &eq.low_pass] {
            eq_pass_data.extend_from_slice(&eq_plot_pass(pass.frequency_hz, pass.enabled));
        }
        eq_curve_db = mooloop_dsp::effects::eq_response_db(eq, sample_rate, EQ_CURVE_SAMPLES);
    }
    EffectSlotRow {
        kind: effect_kind_index(kind),
        units: effect_kind_units(kind),
        preset_options: ModelRc::from(Rc::new(VecModel::from(preset_options))),
        preset_name: preset_name.unwrap_or_default().into(),
        bypassed: slot.bypassed,
        p0: p[0],
        p1: p[1],
        p2: p[2],
        p3: p[3],
        p4: p[4],
        p5: p[5],
        p6: p[6],
        p7: p[7],
        p8: p[8],
        p9: p[9],
        p10: p[10],
        p11: p[11],
        p12: p[12],
        p13: p[13],
        p14: p[14],
        p15: p[15],
        p16: p[16],
        p17: p[17],
        modulation_depths: Vec::<f32>::new().as_slice().into(),
        modulation_allowed: Vec::<bool>::new().as_slice().into(),
        modulation_offsets: Vec::<f32>::new().as_slice().into(),
        modulation_route_counts: Vec::<i32>::new().as_slice().into(),
        eq_band_data: eq_band_data.as_slice().into(),
        eq_band_kinds: eq_band_kinds.as_slice().into(),
        eq_pass_data: eq_pass_data.as_slice().into(),
        eq_curve_db: eq_curve_db.as_slice().into(),
        eq_spectrum_data: Vec::<f32>::new().as_slice().into(),
        eq_analyzer_enabled: slot.params.eq().is_some_and(|eq| eq.analyzer_enabled),
        preamp_deviation: Vec::<f32>::new().as_slice().into(),
        preamp_display_enabled: slot
            .params
            .preamp()
            .is_some_and(|preamp| preamp.display_enabled),
        wet_dry: slot.wet_dry,
        input_trim_db: linear_to_db(slot.input_trim),
        output_trim_db: linear_to_db(slot.output_trim),
        input_left_db: METER_FLOOR_DB,
        input_right_db: METER_FLOOR_DB,
        output_left_db: METER_FLOOR_DB,
        output_right_db: METER_FLOOR_DB,
        buffer_collisions: 0,
        buffer_peaks: Vec::<f32>::new().as_slice().into(),
        // -1 is "no head", which a fraction of a ring can never be.
        buffer_head: -1.0,
        buffer_write: 0.0,
        buffer_window_start: -1.0,
        buffer_window_end: -1.0,
        buffer_frozen: false,
        buffer_armed_freeze: 0,
        buffer_armed_gesture: false,
        buffer_history_bars: slot
            .params
            .buffer()
            .map_or(0, |buffer| i32::from(buffer.bars.max(1))),
        buffer_position_bar: 1,
        buffer_position_beat: 1,
        buffer_position_tick: 0,
        detector_db: METER_FLOOR_DB,
        gain_reduction_db: 0.0,
        children: slot.params.container_children().unwrap_or(0) as i32,
        // The markup asked `kind == 13` in seven places, which is the Rust
        // predicate re-derived from a number the comment above `kind` calls a
        // *runtime* binding that may be renumbered. Carried on the row like
        // `children` and `closing`, so the markup asks what a row is rather
        // than what index it happens to have published this frame.
        is_container: slot.params.is_container(),
        // The kind's own name, for the one face that draws two kinds.
        label: kind.label().into(),
        depth,
        closing: ModelRc::from(Rc::new(VecModel::from(closing))),
        selected,
        wrap_enabled,
    }
}

// The Buffer's held buttons borrow nothing and therefore have nothing to put
// back: each is one gate parameter that is high while the button is down.
// `BorrowedBufferParams` and its per-slot map are gone with the macros that
// needed them -- REV writing `1 - rate` and STUT overriding `Length` were the
// only reason a press had to remember anything.

/// Whether the rack's wrap button on `slot` should be live.
fn wrap_enabled_at(effects: &[EffectSlotState], slot: usize) -> bool {
    mooloop_core::can_wrap(effects, slot..mooloop_core::run_of(effects, slot).end)
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
        7 => DeviceKind::AuxIn,
        _ => DeviceKind::Sampler,
    }
}

/// The generator's counterpart to [`effect_kind_index`]: the number
/// `main.slint` dispatches the source face on (`root.source-kind == 4`).
///
/// Public for the same reason, and it was worth the same pass: the UI tests
/// reached a device by writing `set_source_kind(5)`, which says nothing about
/// which synth that is and stops being true the day a kind is inserted rather
/// than appended.
pub fn device_kind_to_int(kind: DeviceKind) -> i32 {
    match kind {
        DeviceKind::Sampler => 0,
        DeviceKind::DrumSynth => 1,
        DeviceKind::MonoSynth => 2,
        DeviceKind::PolySynth => 3,
        DeviceKind::MlM1 => 4,
        DeviceKind::MlP8 => 5,
        DeviceKind::Ds01 => 6,
        DeviceKind::AuxIn => 7,
    }
}

/// The name a device kind wears in the interface.
///
/// The table moved to [`DeviceKind::label`] on 2026-09-12, when a second copy
/// of it -- the one both channel-creation paths build a default channel name
/// out of -- was found to have drifted on three of the eight kinds. This stays
/// as the name the preset browser calls to title a group.
///
/// `main.slint` holds the same eight strings once, in `SourceKinds.labels`,
/// because a picker row is markup. That copy no Rust table can reach, but
/// `tests/source_kind_menu.rs` reads it out of the production markup and
/// holds it against this one -- which is what the add-channel menu, spelling
/// its own four of them, never had.
fn device_kind_label(kind: DeviceKind) -> &'static str {
    kind.label()
}

/// Every source device, in the order the interface offers them -- which is
/// the order [`device_kind_to_int`] encodes, so a kind's position here is its
/// number on the other side of the markup boundary.
///
/// `DeviceKind` has no `ALL` of its own and this is the only place that wants
/// one; adding it to `mooloop-core` for one caller would be putting a UI
/// concern in the model. Public because `tests/source_kind_menu.rs` needs the
/// same eight kinds to hold `SourceKinds.labels` against.
pub const SOURCE_KINDS_IN_PICKER_ORDER: [DeviceKind; 8] = [
    DeviceKind::Sampler,
    DeviceKind::DrumSynth,
    DeviceKind::MonoSynth,
    DeviceKind::PolySynth,
    DeviceKind::MlM1,
    DeviceKind::MlP8,
    DeviceKind::Ds01,
    DeviceKind::AuxIn,
];

/// The sources the interface no longer offers: a new channel cannot start as
/// one, and a channel cannot be switched to one. Songs and presets that use
/// them load and play unchanged, and a channel already on one still shows
/// its name in the source picker.
///
/// Adam, 2026-09-22: the ML-M1 and ML-P8 do everything the Mono Synth and
/// Poly Synth did. The Drum Synth stays: it is quicker to a standard kit
/// than the DS-01. `SourceKinds.retired` in `channel-rack.slint` is the
/// markup's copy, and `tests/source_kind_menu.rs` holds the two together.
pub const RETIRED_SOURCE_KINDS: [DeviceKind; 2] = [DeviceKind::MonoSynth, DeviceKind::PolySynth];

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

/// Put one route's depth back into the row the face is drawing, without
/// rebuilding the list.
///
/// The list must not be rebuilt here: this runs on every frame of a drag, and
/// replacing the model destroys the row being dragged along with the gesture
/// in it. Touching the one field leaves the row's element alone, which is the
/// same reason the modulation grid's meters are updated field-wise.
///
/// It has to happen at all because the row *is* where the depth lives -- the
/// depth knob reports rather than writes, so nothing else would move the
/// number under it.
fn touch_mlp8_route_amount(window: &MainWindow, id: u16, amount: f32) {
    let rows = window.get_mlp8_routes();
    let Some(index) = (0..rows.row_count())
        .find(|index| rows.row_data(*index).is_some_and(|row| row.id == i32::from(id)))
    else {
        return;
    };
    let Some(mut row) = rows.row_data(index) else {
        return;
    };
    row.amount = amount;
    rows.set_row_data(index, row);
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
    // Eight over the group size is the pool's own arithmetic. The face shows
    // it beside the Unison selector and does not compute it: a second copy of
    // that division is a second place for it to disagree with the allocator.
    let polyphony: Vec<i32> = mooloop_core::MlP8Unison::ALL
        .iter()
        .map(|unison| unison.note_polyphony() as i32)
        .collect();
    window.set_mlp8_unison_note_counts(ModelRc::from(Rc::new(VecModel::from(polyphony))));
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
/// Each described parameter's resting value, indexed by descriptor id.
///
/// What a knob's double-click returns to, and the base a modulation arc is
/// drawn from. Published rather than written into the markup because the one
/// place that needed it most could not have a literal at all: the oscillator
/// strip is instantiated three times from one component, and the table gives
/// OSC 2 a default of +12 semitones and OSC 3 one of -12, so a single literal
/// in the shared markup was necessarily wrong for two of the three.
fn descriptor_defaults(descriptors: &[ParamDescriptor]) -> ModelRc<f32> {
    let mut defaults = vec![0.0f32; descriptor_slots(descriptors)];
    for descriptor in descriptors {
        defaults[descriptor.id as usize] = descriptor.default;
    }
    defaults.as_slice().into()
}

fn descriptor_policy_flags(descriptors: &[ParamDescriptor]) -> Vec<bool> {
    let mut allowed = vec![false; descriptor_slots(descriptors)];
    for descriptor in descriptors {
        allowed[descriptor.id as usize] = ModDestinationDescriptor::for_param(descriptor).allowed;
    }
    allowed
}

fn descriptor_policies(descriptors: &[ParamDescriptor]) -> ModelRc<bool> {
    descriptor_policy_flags(descriptors).as_slice().into()
}

/// How many routes land on each parameter, indexed by descriptor id. Drawn as
/// dots on the knob, so a control says how many sources reach it without the
/// shelf being open. Counted from every route, including ones whose
/// destination currently refuses modulation: the assignment is still authored
/// work the user made and can remove.
fn descriptor_route_count_slots(
    rack: &ModRack,
    descriptors: &[ParamDescriptor],
    address: impl Fn(u32) -> ParamAddr,
) -> Vec<i32> {
    let mut counts = vec![0i32; descriptor_slots(descriptors)];
    for descriptor in descriptors {
        counts[descriptor.id as usize] = rack
            .destinations()
            .filter(|destination| *destination == address(descriptor.id))
            .count() as i32;
    }
    counts
}

fn descriptor_route_counts(
    rack: &ModRack,
    descriptors: &[ParamDescriptor],
    address: impl Fn(u32) -> ParamAddr,
) -> ModelRc<i32> {
    descriptor_route_count_slots(rack, descriptors, address)
        .as_slice()
        .into()
}

/// Starts diagnostic logging: the console, the log file, and the panic hook
/// that leaves a crash report.
///
/// Separate from [`AppUi::new`] and public so the binary can call it first:
/// bringing up the audio engine is one of the things worth having in the log,
/// and it happens before there is any UI to attach to.
pub fn start_logging() {
    init_logging();
}

/// The audio configuration the user saved, for opening the engine on.
///
/// The engine has to *start* on it rather than be re-pointed once the window
/// is up. Startup is where a saved output that has gone -- headphones
/// unplugged, a device renamed -- falls back to one that works, and it can
/// only do that for the output it was asked for: started on the default and
/// re-targeted afterwards, the saved pair was tried last, failed, and took
/// the working fallback down with it (P3 in `reports/teams-2026-09-22.md`).
pub fn saved_audio_config() -> mooloop_engine::AudioConfig {
    UiSettings::load_or_default().audio.engine_config()
}

/// Brings up diagnostic logging for the run and says what build is running.
///
/// The console threshold comes from `MOOLOOP_LOG` (`error`, `warn`, `info`, or
/// `debug`), falling back to the older `MOOLOOP_DEBUG=1`, which now means the
/// same as `MOOLOOP_LOG=debug`. With neither set it stays at `info`, so a run
/// started from a terminal still reports what it opened, saved, and repaired
/// without being asked.
///
/// Everything, `debug` included, also goes to [`settings::log_path`], on every
/// run. It used to be a preference, off by default, on the Developer page --
/// so the runs that ended in a crash or a logout were the ones with no log,
/// since nobody turns a log on before the problem they did not expect (P5 in
/// `reports/teams-2026-09-22.md`). A file that cannot be opened is reported
/// on stderr and the run goes on: a log is not worth refusing to start over.
///
/// The panic hook writes the first panic of a run to a crash report, with a
/// backtrace, in [`settings::crash_dir`].
fn init_logging() {
    let level = match std::env::var("MOOLOOP_LOG") {
        Ok(name) => Level::parse(&name).unwrap_or_else(|| {
            eprintln!("mooloop: MOOLOOP_LOG={name:?} is not a level, using info");
            Level::Info
        }),
        Err(_) if std::env::var_os("MOOLOOP_DEBUG").is_some() => Level::Debug,
        Err(_) => Level::Info,
    };
    mooloop_core::log::set_level(level);
    let path = settings::log_path();
    if let Err(error) = mooloop_core::log::start_file(&path, &build_description()) {
        eprintln!("mooloop: could not write the log to {}: {error}", path.display());
    }
    // A panic is the one failure with no dialog and no status bar to carry it,
    // which makes it the one that most needs to reach the file -- and the one
    // whose backtrace is worth keeping, since the stderr of a program started
    // from a desktop file reaches nobody.
    mooloop_core::log::install_panic_hook(settings::crash_dir(), build_description());
    log_info!("app", "mooloop {} starting", build_description());
    log_info!(
        "app",
        "settings: {}, log: {}, log level: {level:?}",
        settings::config_dir().display(),
        path.display()
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

/// An outlet's declaration, as its header states it: shape, update rate, and
/// how many blocks behind a consumer reads it.
///
/// Read off the descriptor rather than written out, because the three are the
/// contract a consumer is entitled to rely on. Upper case to sit where a
/// module's `kind-signal` sits in the shelf.
fn outlet_declaration(outlet: &OutletDescriptor) -> String {
    let shape = match outlet.signal {
        SignalShape::Bipolar => "BIPOLAR",
        SignalShape::Unipolar => "UNIPOLAR",
        SignalShape::Gate => "GATE",
        SignalShape::Stepped => "STEPPED",
        SignalShape::Trigger => "TRIGGER",
    };
    let rate = match outlet.update {
        ControlRate::Subdivision32 => "PER TICK",
        ControlRate::NoteEvent => "PER NOTE",
        ControlRate::PerBlock => "PER BLOCK",
        ControlRate::Manual => "MANUAL",
    };
    let blocks = outlet.latency.blocks;
    format!(
        "{shape}  ·  {rate}  ·  {blocks} BLOCK{}",
        if blocks == 1 { "" } else { "S" }
    )
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
    // Through `display_unit`, like every other value readout in the program.
    // Printing the descriptor's own units directly is what made this the one
    // path that could not show the range it was editing: an envelope attack
    // of 5 ms read `0.01 s`, and everything below 5 ms read `0.00 s`, so a
    // lane on the fastest part of an envelope was a row of identical zeroes.
    let (scale, unit) = display_unit(descriptor, natural);
    let shown = natural / scale;
    let magnitude = shown.abs();
    let text = if magnitude >= 10_000.0 {
        format!("{:.2}k", shown / 1_000.0)
    } else if unit == "ms" {
        // Milliseconds are already the small unit, so they get the rule the
        // DS-01 face uses rather than the general one: `240`, `45`, `5`,
        // and a tenth only below 1 ms, where it is the whole value.
        if magnitude >= 1.0 || magnitude == 0.0 {
            format!("{shown:.0}")
        } else {
            format!("{shown:.1}")
        }
    } else if magnitude >= 100.0 {
        format!("{shown:.0}")
    } else if magnitude >= 10.0 {
        format!("{shown:.1}")
    } else {
        format!("{shown:.2}")
    };
    if unit.is_empty() {
        text
    } else {
        format!("{text} {unit}")
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
    /// Whether the parameter swings both ways about zero, which decides
    /// whether its dial fills from the centre or from the floor. Sent rather
    /// than written out per knob: it is `descriptor.min < 0`, and the face
    /// does not get a second copy of the parameter table.
    bipolars: Vec<bool>,
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

/// The unit a face is showing a value in, and the factor that converts a
/// number in it back to the parameter's own unit.
///
/// Two departures from the shared [`format_param_value`], both because the
/// short end of a range is where the useful values are:
///
/// - **A time under a second is milliseconds.** Two decimals of seconds makes
///   a 5 ms attack, a 1 ms one and a zero all read `0.00 s`, and sub-100 ms
///   percussion is the range a drum is *for*. This is the same rule
///   `TimeFormat.seconds` prints by, so a knob's readout and a typed field
///   agree.
/// - **A frequency over a kilohertz is kilohertz.** The shared formatter's
///   `k` starts at ten thousand, which leaves the noise cutoff reading as a
///   bare `7500`.
///
/// One function, used by the formatter and by the parser, so a field always
/// reads back what it shows: a bare number typed into one is read in
/// whichever unit it was showing, because that is the number the person was
/// looking at when they started typing.
fn display_unit(descriptor: &ParamDescriptor, natural: f32) -> (f32, &'static str) {
    match descriptor.unit {
        "s" if natural.abs() < 1.0 => (0.001, "ms"),
        "Hz" if natural.abs() >= 1_000.0 => (1_000.0, "kHz"),
        unit => (1.0, unit),
    }
}

/// The above, plus DS-01's own third departure: **a route's depth is a
/// percentage**, which is what a share of a destination's range is called
/// everywhere else in the program.
fn ds01_display_unit(descriptor: &ParamDescriptor, natural: f32) -> (f32, &'static str) {
    if ds01::matrix_offset(descriptor.id) == Some(ds01::MATRIX_OFFSET_AMOUNT) {
        return (0.01, "%");
    }
    display_unit(descriptor, natural)
}

/// The number itself, at a precision that suits its unit and its size.
///
/// Milliseconds get their own rule because they are already the small unit:
/// `240`, `45`, `5`, `0` rather than `240`, `45.0`, `5.00`, `0.00`.
fn ds01_number(shown: f32, unit: &str) -> String {
    let magnitude = shown.abs();
    // A depth is a share of a range, and a tenth of a percent of one is not a
    // distinction anybody makes.
    if unit == "%" {
        return format!("{shown:.0}");
    }
    if unit == "ms" {
        return if magnitude >= 1.0 || magnitude == 0.0 {
            format!("{shown:.0}")
        } else {
            format!("{shown:.1}")
        };
    }
    if magnitude >= 100.0 {
        format!("{shown:.0}")
    } else if magnitude >= 10.0 {
        format!("{shown:.1}")
    } else {
        format!("{shown:.2}")
    }
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
    let natural = descriptor.from_normalized(normalized);
    if matches!(descriptor.curve, ParamCurve::Stepped(_)) {
        let natural = natural.round();
        // A tuning reads as an interval, so it keeps its sign in both
        // directions; a count has no other direction to be in.
        let sign = if descriptor.min < 0.0 && natural > 0.0 {
            "+"
        } else {
            ""
        };
        return if descriptor.unit.is_empty() {
            format!("{sign}{natural:.0}").into()
        } else {
            format!("{sign}{natural:.0} {}", descriptor.unit).into()
        };
    }
    let (scale, unit) = ds01_display_unit(descriptor, natural);
    let shown = natural / scale;
    let sign = if descriptor.min < 0.0 && shown > 0.0 {
        "+"
    } else {
        ""
    };
    let number = ds01_number(shown, unit);
    if unit.is_empty() {
        format!("{sign}{number}").into()
    } else if unit == "%" {
        format!("{sign}{number}{unit}").into()
    } else {
        format!("{sign}{number} {unit}").into()
    }
}

/// What a typed DS-01 value means.
///
/// The unit the user wrote wins; with none written, the number means whatever
/// the field was already showing. That is the only self-consistent rule for a
/// field whose unit follows its value: `240` typed over `240 ms` is 240
/// milliseconds, and a second and a half is written `1.5 s`.
fn typed_value(text: &str, bare_scale: f32) -> Option<f32> {
    let number = parse_typed_value(text)?;
    let suffix = text
        .trim()
        .trim_start_matches(|c: char| c.is_ascii_digit() || matches!(c, '.' | '-' | '+'))
        .trim()
        .to_ascii_lowercase();
    let scale = if suffix.starts_with("ms") {
        0.001
    } else if suffix.starts_with('k') {
        1_000.0
    } else if suffix.starts_with('%') {
        0.01
    } else if suffix.is_empty() {
        bare_scale
    } else {
        // A unit that is not a prefix — `s`, `Hz`, `st` — is the natural one.
        1.0
    };
    Some(number * scale)
}

fn ds01_typed_value(descriptor: &ParamDescriptor, text: &str, current: f32) -> Option<f32> {
    typed_value(text, ds01_display_unit(descriptor, current).0)
}

fn ds01_face_values(params: &Ds01Params) -> Ds01FaceValues {
    let len = ds01_array_len();
    let mut out = Ds01FaceValues {
        values: vec![0.0; len],
        defaults: vec![0.0; len],
        texts: vec![SharedString::new(); len],
        steps: vec![0; len],
        bipolars: vec![false; len],
    };
    for descriptor in ds01::DESCRIPTORS.iter() {
        let index = descriptor.id as usize;
        let natural = ds01::get(params, descriptor.id).unwrap_or(descriptor.default);
        let normalized = descriptor.to_normalized(natural);
        out.values[index] = normalized;
        out.defaults[index] = descriptor.to_normalized(descriptor.default);
        out.texts[index] = ds01_text(descriptor, params, normalized);
        out.bipolars[index] = descriptor.min < 0.0;
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
/// What the matrix's two pickers offer, labelled by `mooloop_core`.
///
/// Nine sources and forty-seven destinations is well past what a cycling chip
/// can carry, so the face picks from a list — and the list is built from the
/// same descriptor table the routes resolve against, so a name on the face
/// and a name in the engine cannot drift.
fn ds01_matrix_names() -> (Vec<SharedString>, Vec<SharedString>) {
    (
        ds01::Ds01ModSource::ALL
            .iter()
            .map(|source| SharedString::from(source.label()))
            .collect(),
        ds01::destinations()
            .map(|descriptor| SharedString::from(descriptor.name))
            .collect(),
    )
}

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

// --- Aux In ---------------------------------------------------------------

/// The channels an Aux In on `consumer` may read: every channel whose
/// generator publishes at least one audio outlet, and not itself.
///
/// A channel cannot read itself -- `compile_audio_graph` refuses that as
/// `SelfSubscribed` -- so the picker does not offer it. That is the one
/// refusal it is worth preventing rather than reporting, because there is no
/// state of the project in which the answer changes.
fn aux_in_sources(session: &Session, consumer: usize) -> Vec<(u8, String)> {
    session
        .channels
        .iter()
        .enumerate()
        .filter(|(index, channel)| {
            *index != consumer && channel.kind.outlets().iter().any(|o| !o.is_control())
        })
        .map(|(index, channel)| (index as u8, channel.name.clone()))
        .collect()
}

/// One channel's audio outlets, in declared order. The domain does the
/// refusing structurally, so this picker never has to remember that a control
/// outlet exists.
fn aux_in_outlets(session: &Session, channel: u8) -> Vec<&'static OutletDescriptor> {
    session
        .channels
        .get(channel as usize)
        .map(|channel| {
            channel
                .kind
                .outlets()
                .iter()
                .filter(|outlet| !outlet.is_control())
                .collect()
        })
        .unwrap_or_default()
}

/// What a refused edge tells the user.
///
/// One sentence each, and each one names a different fix: an edge into a
/// channel that stopped publishing and an edge that closes a ring both
/// produce silence, and a user cannot repair either without being told them
/// apart. That is why `EdgeRefusal` is a value rather than a bare `None`.
fn aux_in_refusal_text(refusal: EdgeRefusal) -> &'static str {
    match refusal {
        EdgeRefusal::NoSuchChannel => "The channel this read is gone. Pick another source.",
        EdgeRefusal::SelfSubscribed => "A channel cannot read itself.",
        EdgeRefusal::NotAProducer => "That channel's device publishes no audio.",
        EdgeRefusal::NoSuchOutlet => "That device no longer publishes this outlet.",
        EdgeRefusal::NotAudio => "That outlet is a control signal, not audio.",
        EdgeRefusal::TapIsLate => "That outlet is tapped after its channel's effects, so it \
                                   would arrive late.",
        EdgeRefusal::Cycle => "Refused: this would close a loop of inputs.",
    }
}

/// Send both halves of a subscription to the engine.
///
/// Both, always: the audio thread's copy of an Aux In's parameters is only
/// what the device reads back, and sending one half would leave the two
/// disagreeing with the model the render plan is compiled from. The plan
/// itself does not travel here -- the pump derives it from the session and
/// sends it whole, once, when it changes.
fn send_aux_in_subscription(
    tx: &EngineCommandSender,
    channel: usize,
    params: AuxInParams,
) {
    for (id, value) in [
        (aux_in::PARAM_SOURCE_CHANNEL, f32::from(params.source_channel)),
        (aux_in::PARAM_SOURCE_OUTLET, f32::from(params.source_outlet)),
    ] {
        let _ = tx.send(EngineCommand::SetChannelGeneratorParam {
            channel: channel as u8,
            id,
            value,
        });
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
    window.set_ds01_bipolars(face.bipolars.as_slice().into());
    let (sources, destinations) = ds01_matrix_names();
    window.set_ds01_matrix_source_names(sources.as_slice().into());
    window.set_ds01_matrix_dest_names(destinations.as_slice().into());
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
    /// Every take that is running or waiting to be turned into a sample
    /// (`audio-recording/03` and `04`). Runtime state, never saved.
    takes: TakeRecorder,
    /// When this run of the application began.
    ///
    /// What tells a take made in this session apart from one left in the
    /// shared recordings folder by a crash (`audio-recording/06`). Only the
    /// first kind is offered at quit: the second may be the only copy of a
    /// take from a song that was never saved.
    session_start: SystemTime,
    /// The driver's name for its hardware input, or `None` when it has none:
    /// the AUDIO menu's input row.
    audio_input_label: Option<String>,
    /// The hardware input's round trip, in frames, refreshed by the pump. A
    /// take from the input starts this much after its bar line.
    input_latency_frames: u32,
    /// The MIDI inputs the driver is offering, as of the last scan. Cached
    /// rather than asked per use: under Core MIDI the answer is a lock and a
    /// list of `String` clones, and the input picker, the routing table and
    /// every control binding's port all want it.
    midi_ports: Vec<mooloop_core::MidiPortInfo>,
    /// Whether the toolbar's MIDI Learn arm is on.
    ///
    /// The arm and the *pending* target are two different things and both are
    /// needed. `Session::control_learn` holds the target a control was pressed
    /// for; this holds whether pressing one would name it. The arm outlives a
    /// binding landing, so a desk can be mapped knob after knob without
    /// reaching for the toolbar between each.
    midi_learn_armed: bool,
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
    /// The generator's published control outlets. A separate model rather
    /// than more rows in the source one: an outlet is not a module, and a
    /// combined model would have to carry a discriminator into every reorder,
    /// remove and editor lookup the source rows already do by position.
    modulation_outlet_model: Rc<VecModel<ModulationOutletRow>>,
    modulation_route_model: Rc<VecModel<ModulationRouteRow>>,
    mixer_strip_model: Rc<VecModel<MixerStripRow>>,
    /// Flattened sample-browser tree, rebuilt whenever locations or folder
    /// expansion change.
    browser_rows: Rc<VecModel<BrowserRow>>,
    /// Which tab the browser panel is showing. View state rather than session
    /// state: it is not a fact about the song, so it has no business in a
    /// project file or in undo.
    browser_tab: BrowserTab,
    /// The preset catalogue behind the PRESETS tab, rescanned when the tab is
    /// opened rather than held live. Presets change on disk only when this
    /// application writes one, and it rescans then too.
    preset_catalog: Vec<PresetGroup>,
    /// Raised whenever the effect rack is re-synced, so the pump knows the
    /// engine's spectrum subscriptions may be pointing at the wrong slots.
    ///
    /// A subscription is keyed by `(target, slot)` and every structural rack
    /// edit renumbers slots, so enabling the analyzer on an EQ in slot 2 and
    /// then deleting slot 1 used to leave the engine publishing for a stage
    /// nothing draws while the EQ, now slot 1, drew a flat line behind a lit
    /// button. The orphan also held one of the sixty-four `SPECTRUM_SLOTS`
    /// for the rest of the session.
    ///
    /// `Cell` because `sync_effects` takes `&self`, and that is the one
    /// function every rack edit already calls.
    effect_spectra_stale: std::cell::Cell<bool>,
    /// Raised when a project has been installed, so the pump knows the track
    /// its per-bus meter ballistics belong to may have changed underneath
    /// them.
    ///
    /// The ballistics are keyed by index and the pump owns them as `move`d
    /// locals, so this is the same flag handoff `bus_clip_clear` uses: raised
    /// here, consumed on the next tick. See [`meter::MeterBallistics::reset`]
    /// for what an inherited latch looks like.
    bus_meters_stale: bool,
    /// What the audio driver came up at. Held because the response plots are
    /// the *coefficients* a device is running, evaluated, and a biquad's
    /// coefficients are designed against a sample rate -- so a row cannot be
    /// published without one. The window carries the same number as
    /// `audio-sample-rate` for the readouts; this is the copy the publishers
    /// reach, which take `&self` and no window.
    audio_sample_rate: u32,
}

/// The browser panel's two halves.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum BrowserTab {
    #[default]
    Samples,
    Presets,
}

impl UiState {
    /// Build the rack state for a fresh, one-channel document and install
    /// every model on the window.
    ///
    /// Extracted from `AppUi::new` so a `UiState` can be had without an
    /// `EngineHandle`, which only `Engine::new` produces and only by opening
    /// a real audio driver. That is what made every channel-rack callback
    /// untestable; `add_channel_with_history` is tested through this.
    fn new(
        default_sample: Option<&SampleData>,
        audio_sample_rate: u32,
        window: &MainWindow,
    ) -> Self {
        let default_waveform = default_sample
            .map(|sample| waveform_peaks(sample, WAVEFORM_BINS))
            .unwrap_or_default();
        let default_sample_description = default_sample
            .map(sample_description)
            .unwrap_or_default();
        let default_sample_duration = default_sample
            .map(sample_duration)
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
            color: channel_colors::to_slint(first.color),
            has_color: first.color.is_some(),
            // A new song's one track is the master, which nobody has
            // coloured yet; the first refresh fills this in if they do.
            track_color: Default::default(),
            has_track_color: false,
            muted: false,
            solo: false,
            // A new song has one channel and nothing soloed, so nothing is
            // silenced; the first refresh derives it from the bank if that
            // changes.
            solo_silenced: false,
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
        let modulation_outlet_model = Rc::new(VecModel::from(Vec::<ModulationOutletRow>::new()));
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
        window.set_modulation_outlets(ModelRc::from(modulation_outlet_model.clone()));
        window.set_modulation_routes(ModelRc::from(modulation_route_model.clone()));
        window.set_mixer_strips(ModelRc::from(mixer_strip_model.clone()));
        window.set_browser_rows(ModelRc::from(browser_row_model.clone()));
        window.set_pattern_count(1);

        Self {
            // Filled by the pump's first scan, which also installs the
            // routing. Starting empty rather than scanning here keeps one
            // path for "the ports changed", and the first pump is 16 ms away.
            midi_ports: Vec::new(),
            midi_learn_armed: false,
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
            modulation_outlet_model,
            modulation_route_model,
            mixer_strip_model,
            browser_rows: browser_row_model,
            browser_tab: BrowserTab::default(),
            preset_catalog: Vec::new(),
            effect_spectra_stale: std::cell::Cell::new(false),
            bus_meters_stale: false,
            automation_point_model,
            automation_target_model,
            audio_sample_rate,
            takes: TakeRecorder::new(settings::recordings_dir()),
            session_start: SystemTime::now(),
            // Both come from the driver, which this constructor deliberately
            // cannot reach: it takes no `EngineHandle` so the channel-rack
            // callbacks stay testable without one. `AppUi::new` fills them in
            // from its handle, and the pump refreshes the latency as the
            // driver reports it.
            audio_input_label: None,
            input_latency_frames: 0,
        }
    }

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
        let solo_silenced = self.session.channel_solo_silenced();
        let rows: Vec<ChannelRow> = self
            .session.channels
            .iter()
            .enumerate()
            .map(|(index, channel)| ChannelRow {
                name: channel.name.as_str().into(),
                color: channel_colors::to_slint(channel.color),
                has_color: channel.color.is_some(),
                track_color: channel_colors::to_slint(feeding_track_color(
                    &self.session.buses,
                    channel.bus,
                )),
                has_track_color: feeding_track_color(&self.session.buses, channel.bus).is_some(),
                muted: channel.muted,
                solo: channel.solo,
                solo_silenced: solo_silenced[index],
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
        // Belongs with the other three, and was the one missing: pattern
        // names arrive with the document like everything else here, and only
        // `on_pattern_selected`, `on_add_pattern` and `on_pattern_renamed`
        // pushed them. So opening a song, starting a new kit, or any edit
        // that reinstalls the project -- a channel paste, an undo -- left the
        // toolbar field and the pattern menu naming the *previous* document's
        // patterns until something happened to select one.
        self.sync_pattern_menu(window);
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
        // Derived once for the bank rather than per row: what a solo silences
        // is a property of every channel at once, so asking per row would
        // re-derive the whole plan once per row.
        let solo_silenced = self.session.channel_solo_silenced();
        for (i, ch) in self.session.channels.iter().enumerate() {
            if let Some(mut row) = self.rows.row_data(i) {
                row.selected = i == self.session.selected;
                row.muted = ch.muted;
                row.solo = ch.solo;
                row.solo_silenced = solo_silenced[i];
                row.volume_db = linear_to_db(ch.volume);
                row.pan = ch.pan;
                row.bus = ch.bus as i32;
                row.name = ch.name.as_str().into();
                row.color = channel_colors::to_slint(ch.color);
                row.has_color = ch.color.is_some();
                // Re-read every refresh rather than stored: this is the
                // *track's* colour, so it moves when the track is recoloured
                // or when the channel is routed somewhere else, and neither
                // of those is an edit to the channel.
                let track = feeding_track_color(&self.session.buses, ch.bus);
                row.track_color = channel_colors::to_slint(track);
                row.has_track_color = track.is_some();
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

    /// Apply a step edit: redraw the cells it touched, refresh the note editor
    /// when the edited channel is the one on screen, and send its commands in
    /// the order the session produced them.
    ///
    /// Six callbacks did this by hand -- `on_step_clicked`, `on_step_removed`,
    /// `on_step_velocity_edited`, `on_step_painted`, `on_step_sliced` and
    /// `on_step_length_dragged` -- with the same thirteen lines in all six. The
    /// one that would have been quiet to get wrong is the note editor: a step
    /// operation that forgot it leaves the piano roll drawing the pattern as it
    /// was before the edit, on the channel the user is looking at.
    fn apply_step_edit(
        &self,
        channel: i32,
        edit: StepEdit,
        window: &slint::Weak<MainWindow>,
        tx: &EngineCommandSender,
    ) {
        for cell in edit.redraw {
            self.refresh_rack_cell(channel as usize, cell);
        }
        if channel as usize == self.session.selected {
            if let Some(window) = window.upgrade() {
                self.refresh_note_editor(&window);
            }
        }
        for command in edit.commands {
            let _ = tx.send(command);
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
            .session.pattern_meta
            .iter()
            .enumerate()
            .map(|(i, meta)| {
                let label = if meta.name.is_empty() {
                    format!("Pattern {}", i + 1)
                } else {
                    meta.name.clone()
                };
                format!("{:02}  {label}", i + 1).into()
            })
            .collect();
        window.set_pattern_menu_options(ModelRc::from(Rc::new(VecModel::from(options))));
        // The undecorated names and colours, for the surfaces that draw a row
        // or a clip per pattern and number it themselves. The playlist gutter
        // drew "Pattern N" from its own loop index and so was the one place a
        // rename never reached.
        //
        // The ink is computed here rather than in the markup because the
        // luminance weights that decide it belong with the colour type that
        // has a test for them, and a second copy in Slint would be a second
        // place for the threshold to sit.
        let info: Vec<PatternInfo> = self
            .session.pattern_meta
            .iter()
            .map(|meta| PatternInfo {
                name: meta.name.as_str().into(),
                color: channel_colors::to_slint(meta.color),
                has_color: meta.color.is_some(),
                ink: channel_colors::to_slint(meta.color.map(|color| color.ink())),
            })
            .collect();
        window.set_pattern_info(ModelRc::from(Rc::new(VecModel::from(info))));
        let current = self.session.pattern_meta.get(self.session.current_pattern);
        window.set_current_pattern_name(
            current.map(|meta| meta.name.clone()).unwrap_or_default().into(),
        );
        let color = current.and_then(|meta| meta.color);
        window.set_current_pattern_has_color(color.is_some());
        window.set_current_pattern_color_hex(
            color.map(|color| color.to_hex()).unwrap_or_default().into(),
        );
        window.set_current_pattern_color(channel_colors::to_slint(color));
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
        self.sync_loop_range(window);
    }

    /// Publish the loop's points and whether it is live.
    ///
    /// Separate from the clips because the toggle changes it without touching
    /// them, and called from `sync_playlist` because the song's length is
    /// derived from the clips and a loop is drawn against that length.
    fn sync_loop_range(&self, window: &MainWindow) {
        let range = self.session.loop_range;
        window.set_playlist_loop_start_ticks(range.start_tick as i32);
        window.set_playlist_loop_end_ticks(range.end_tick as i32);
        window.set_playlist_loop_enabled(range.enabled);
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
        let Some(chain) = self.session.effect_chain() else {
            return;
        };
        if let Some(effect) = chain.get(slot) {
            let depth = mooloop_core::depth_at(chain, slot) as i32;
            self.effect_slot_model.set_row_data(
                slot,
                effect_slot_row(
                    effect,
                    &self.session.effect_presets,
                    self.session
                        .effect_preset_name(self.session.effect_target, effect.id),
                    RackPlacement {
                        depth,
                        closing: containers_closing_at(chain, slot),
                        selected: self.session.selected_device_slot() == Some(slot),
                        wrap_enabled: wrap_enabled_at(chain, slot),
                    },
                    self.audio_sample_rate,
                ),
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
    /// Tell the engine how far every container on `target`'s chain reaches,
    /// and hand each one a ring sized to its run's declared latency.
    ///
    /// Every container, every time, rather than a diff. A chain holds a
    /// handful of boxes and this runs on a hand gesture, so the cost is
    /// nothing and the alternative -- working out which spans an edit moved --
    /// is exactly the class of bookkeeping `docs/plans/containers/01` deleted.
    ///
    /// A span is structure, not a control: it has no descriptor id and cannot
    /// arrive on the value ring, because a curve drawn on it would reshape
    /// the chain from the audio thread.
    fn publish_container_spans(
        &self,
        target: EffectTarget,
        stx: &StructuralCommandSender,
    ) {
        let Some(effects) = self.session.effect_chain_of(target) else {
            return;
        };
        let mut scratch = ContainerScratch::for_chain(effects).map(Box::new);
        for (slot, effect) in effects.iter().enumerate() {
            let Some(children) = effect.params.container_children() else {
                continue;
            };
            let Ok(slot_index) = u8::try_from(slot) else {
                continue;
            };
            // Allocated here, off the audio thread, for the same reason the
            // node beside it is.
            let align = (children > 0)
                .then(|| IntegerDelay::new(mooloop_core::run_latency(effects, slot)))
                .flatten()
                .map(Box::new);
            stx.send(StructuralCommand::SetContainerSpan {
                target,
                slot: slot_index,
                children,
                align,
                // Rides the first container's command; the chain keeps the
                // first one it is given and hands every later one back,
                // unless this one can run a layer and its own cannot.
                scratch: scratch.take(),
            });
            // A layer's branches, each held back to meet the longest. Every
            // branch, every time, `None` included, so a branch that has
            // become the longest lets go of the ring it no longer needs.
            if effect.params.container_flow() != Some(mooloop_core::ContainerFlow::Parallel) {
                continue;
            }
            for branch in mooloop_core::layer_branches(effects, slot) {
                let Ok(branch_index) = u8::try_from(branch) else {
                    continue;
                };
                let align =
                    IntegerDelay::new(mooloop_core::branch_alignment(effects, slot, branch))
                        .map(Box::new);
                stx.send(StructuralCommand::SetBranchAlign {
                    target,
                    slot: branch_index,
                    align,
                });
            }
        }
    }

    /// Mirror a freshly inserted device onto the engine.
    ///
    /// Installed into the vacant tail and moved into place, which is what
    /// lets the realtime chain reach the model's order without allocating in
    /// its callback. Shared by the two ways of inserting -- before a row, and
    /// into a container -- because only the model verb differs.
    fn install_added_effect(
        &self,
        added: &mooloop_session::effects::EffectInserted,
        bpm: f64,
        sample_rate: u32,
        tx: &EngineCommandSender,
        stx: &StructuralCommandSender,
    ) {
        // The node and its dry-align ring are built here because construction
        // allocates: off the audio thread, riding the same structural command
        // as the slot they belong to.
        let node = build_effect_at_tempo(added.params, sample_rate, bpm);
        let align = IntegerDelay::new(node.dry_path_latency_frames()).map(Box::new);
        stx.send(StructuralCommand::InstallEffect {
            target: added.target,
            slot: added.tail as u8,
            kind: added.kind,
            resource_key: added.params.buffer().copied().map(buffer_allocation_key),
            node,
            align,
            analyzer: Box::new(SpectrumAnalyzer::new()),
            // Allocated here with the node: an empty addressable slot costs a
            // pointer rather than its full host state. It carries the identity
            // the model just minted, which is what lets a route find this
            // device again.
            state: Box::new(EffectSlot::for_device(added.device)),
        });
        if added.slot != added.tail {
            let _ = tx.send(EngineCommand::MoveEffect {
                target: added.target,
                from: added.tail as u8,
                to: added.slot as u8,
            });
        }
        // Inserting inside a box grew that box, and every box around it, and
        // may have changed what its dry path has to wait for.
        self.publish_container_spans(added.target, stx);
    }

    fn sync_effects(&self) {
        // Every structural rack edit calls this, which is what makes it the
        // place to notice that slot numbers may have moved under the engine's
        // spectrum subscriptions. Idempotent and cheap, so the non-structural
        // callers raising it too costs nothing -- and a preset can carry an
        // analyzer flag, so some of them need it anyway.
        self.effect_spectra_stale.set(true);
        let armed = self.session.modulation_armed_slot.get();
        let selected = self.session.selected_device_slot();
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
                            let mut row = effect_slot_row(
                                effect,
                                &self.session.effect_presets,
                                self.session.effect_preset_name(
                                    EffectTarget::Channel(channel),
                                    effect.id,
                                ),
                                RackPlacement {
                                    depth: mooloop_core::depth_at(&state.effects, slot) as i32,
                                    closing: containers_closing_at(&state.effects, slot),
                                    selected: selected == Some(slot),
                                    wrap_enabled: wrap_enabled_at(&state.effects, slot),
                                },
                                self.audio_sample_rate,
                            );
                            let descriptors = effect.kind().descriptors();
                            let address = |param| {
                                ParamAddr::effect(EffectTarget::Channel(channel), effect.id, param)
                            };
                            let depths =
                                self.session.destination_depths(armed, descriptors, address);
                            let allowed = descriptor_policy_flags(descriptors);
                            let offsets = self.session.destination_offsets(descriptors, address);
                            let counts =
                                descriptor_route_count_slots(&state.modulation, descriptors, address);
                            // The EQ's seven controls are a view over fifty
                            // descriptors, so its overlays are gathered down
                            // to what the face reads.
                            match effect.params.eq() {
                                Some(eq) => {
                                    let ids = eq_face_ids(eq);
                                    row.modulation_depths =
                                        eq_overlay_view(&ids, &depths).as_slice().into();
                                    row.modulation_allowed =
                                        eq_overlay_view(&ids, &allowed).as_slice().into();
                                    row.modulation_offsets =
                                        eq_overlay_view(&ids, &offsets).as_slice().into();
                                    row.modulation_route_counts =
                                        eq_overlay_view(&ids, &counts).as_slice().into();
                                }
                                None => {
                                    row.modulation_depths = depths.as_slice().into();
                                    row.modulation_allowed = allowed.as_slice().into();
                                    row.modulation_offsets = offsets.as_slice().into();
                                    row.modulation_route_counts = counts.as_slice().into();
                                }
                            }
                            row
                        })
                        .collect()
                })
                .unwrap_or_default(),
            _ => {
                let target = self.session.effect_target;
                self.session
                    .effect_chain()
                    .map(|effects| {
                        effects
                            .iter()
                            .enumerate()
                            .map(|(slot, effect)| {
                                effect_slot_row(
                                    effect,
                                    &self.session.effect_presets,
                                    self.session.effect_preset_name(target, effect.id),
                                    RackPlacement {
                                        depth: mooloop_core::depth_at(effects, slot) as i32,
                                        closing: containers_closing_at(effects, slot),
                                        selected: selected == Some(slot),
                                        wrap_enabled: wrap_enabled_at(effects, slot),
                                    },
                                    self.audio_sample_rate,
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            }
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
        // The outlet chips carry the same telemetry, and are touched the
        // same way: the engine already publishes the whole flat row, so an
        // outlet's meter costs nothing the module band was not paying.
        for index in 0..self.modulation_outlet_model.row_count() {
            let Some(mut row) = self.modulation_outlet_model.row_data(index) else {
                continue;
            };
            let next = usize::try_from(row.slot)
                .ok()
                .and_then(|slot| outputs.get(slot).copied())
                .unwrap_or(0.0);
            if row.output != next {
                row.output = next;
                self.modulation_outlet_model.set_row_data(index, row);
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
            let offsets = self
                .session
                .destination_offsets(effect.kind().descriptors(), |param| {
                    ParamAddr::effect(scope, effect.id, param)
                });
            row.modulation_offsets = match effect.params.eq() {
                Some(eq) => eq_overlay_view(&eq_face_ids(eq), &offsets).as_slice().into(),
                None => offsets.as_slice().into(),
            };
            self.effect_slot_model.set_row_data(slot, row);
        }
    }

    /// Rebuild the channel-owned source collection and destination inspector.
    /// Selection and assignment are transient UI state: project reloads and
    /// channel changes never leave an invisible armed slot behind.
    fn refresh_modulation(&self, window: &MainWindow) {
        // By identity, so switching to a channel that happens to occupy the
        // seat the last one did still clears the arm -- which is what the
        // field's own comment has always claimed and what a `usize` seat
        // could not deliver.
        let selected_id = self.session.channel_id(self.session.selected);
        if self.session.modulation_ui_channel.get() != selected_id {
            self.session.modulation_ui_channel.set(selected_id);
            self.session.modulation_selected_slot.set(None);
            self.session.modulation_armed_slot.set(None);
        }
        let Some(channel) = self.session.channels.get(self.session.selected) else {
            self.modulation_source_model.set_vec(Vec::new());
            self.modulation_outlet_model.set_vec(Vec::new());
            self.modulation_route_model.set_vec(Vec::new());
            self.session.modulation_selected_slot.set(None);
            self.session.modulation_armed_slot.set(None);
            window.set_modulation_selected_slot(-1);
            window.set_modulation_armed_slot(-1);
            window.set_modulation_armed_name(Default::default());
            window.set_modulation_outlet_device(Default::default());
            window.set_modulation_selected_outlet_name(Default::default());
            window.set_modulation_selected_outlet_signal(Default::default());
            return;
        };

        // A slot survives only while it still names something. That is now
        // two questions rather than one -- an occupied rack slot, or a
        // control outlet this generator still publishes -- and the second is
        // why a channel that swaps its ML-P8 for a sampler drops a selection
        // pointed at `Trigger` instead of keeping an invisible one.
        let selected = self
            .session
            .modulation_selected_slot
            .get()
            .filter(|slot| self.session.control_source_exists(*slot));
        let armed = self
            .session
            .modulation_armed_slot
            .get()
            .filter(|slot| self.session.control_source_exists(*slot));
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
                // A module by its badge and slot, an outlet by its name, and
                // "SOURCE ?" for a route whose source has left -- which is
                // now also how a route reads when the channel's generator
                // has been swapped out from under it.
                let source_name = self
                    .session
                    .control_source_name(route.source_slot)
                    .unwrap_or_else(|| "SOURCE ?".to_string());
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
                    // The row this route points at, as a *position*: this
                    // token is a Slint model index, not an address, so the
                    // device id has to be resolved against the chain the
                    // shelf is drawing. A destination on another chain -- a
                    // bus device, which the shelf already labels as
                    // unavailable -- resolves to nothing and takes the
                    // no-row token rather than colliding with `Source`.
                    ParamOwner::Effect { device } => self
                        .session
                        .channels
                        .get(self.session.selected)
                        .filter(|_| {
                            route.destination.scope
                                == EffectTarget::Channel(self.session.selected as u8)
                        })
                        .and_then(|state| mooloop_core::device_slot(&state.effects, device))
                        .map_or(i32::MIN, |slot| slot as i32),
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
        // The generator's published control outlets, offered as sources on
        // the same terms as a module. Only the control run: an audio outlet
        // is not a control signal that happens to be fast, and offering one
        // here is exactly the confusion `OutletDomain` exists to prevent.
        let outlets: Vec<ModulationOutletRow> = channel
            .kind
            .control_outlets()
            .iter()
            .map(|outlet| {
                let slot = outlet_slot(outlet.id);
                ModulationOutletRow {
                    slot: i32::from(slot),
                    name: outlet.name.into(),
                    bipolar: matches!(outlet.signal, SignalShape::Bipolar),
                    output: outputs.get(slot as usize).copied().unwrap_or(0.0),
                    selected: selected == Some(slot),
                }
            })
            .collect();
        let selected_outlet = selected.and_then(|slot| self.session.selected_channel_outlet(slot));
        self.modulation_source_model.set_vec(sources);
        self.modulation_outlet_model.set_vec(outlets);
        self.modulation_route_model.set_vec(routes);
        // The publishing device, named the way a route's destination names
        // it -- the channel's own name -- so "ML-P8 1 publishes this" and
        // "ML-P8 1 · Cutoff" are visibly the same device.
        window.set_modulation_outlet_device(channel.name.as_str().into());
        window.set_modulation_armed_name(
            armed
                .and_then(|slot| self.session.control_source_name(slot))
                .unwrap_or_default()
                .into(),
        );
        window.set_modulation_selected_outlet_name(
            selected_outlet.map_or("", |outlet| outlet.name).into(),
        );
        window.set_modulation_selected_outlet_signal(
            selected_outlet
                .map(outlet_declaration)
                .unwrap_or_default()
                .into(),
        );
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
        window.set_source_param_defaults(descriptor_defaults(generator.descriptors()));
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

    /// Make sure a gesture is open, without disturbing one that already is.
    ///
    /// The modulation shelf's knob reports the same press twice — once
    /// through `Gesture.begin()` and once through its own `edit-started`,
    /// which is the route that proved the pair — so this has to be the
    /// idempotent half of `Session::begin_gesture`, never the closing one.
    fn begin_gesture(&mut self, window: &MainWindow) {
        if !self.session.gesture_open() {
            let snapshot = project_snapshot(self, window);
            let closed = self.session.begin_gesture(snapshot);
            debug_assert!(closed.is_none(), "nothing was open to close");
        }
    }

    /// Name `target` as the thing the next control touched should move, when
    /// the toolbar's learn arm is on. Answers whether the press was a learn
    /// gesture, so the caller can stop rather than also starting a
    /// modulation edit.
    ///
    /// Both gestures arrive on the same callback — `modulation-edit-started`,
    /// the one thing every device face carries that knows which parameter was
    /// pressed. The alternative was a second callback threaded through fifty
    /// faces to say the same thing, and each face missed is an eight-minute
    /// build to find out.
    fn learn_if_armed(
        &mut self,
        window: &MainWindow,
        binds_port: bool,
        target: ControlTarget,
    ) -> bool {
        if !self.midi_learn_armed {
            return false;
        }
        self.begin_control_learn(window, binds_port, target);
        true
    }

    /// [`Self::learn_if_armed`] for a parameter, which has to be named
    /// durably before it can be stored.
    ///
    /// Every one of the three faces that reaches here has a `ParamAddr` in
    /// hand, because that is what a press on a knob produces and what the
    /// modulation edit beside it needs. Turning it into the [`ParamKey`] a
    /// binding holds is a question only the session can answer, so it is
    /// asked once here rather than at each face. An address whose seat has no
    /// channel in it is not a learn gesture at all -- there is nothing to
    /// learn onto -- and the press falls through to the modulation edit.
    fn learn_param_if_armed(
        &mut self,
        window: &MainWindow,
        binds_port: bool,
        address: ParamAddr,
    ) -> bool {
        let Some(key) = self.session.param_key(address) else {
            return false;
        };
        self.learn_if_armed(window, binds_port, ControlTarget::Param(key))
    }

    /// Wait for a control to bind to `target`, and say so.
    ///
    /// Three surfaces ask for this — a press on a control, a mapping row's
    /// RELEARN, and a transport row's LEARN — and the sentence they put in the
    /// status bar is one sentence, so it is written once.
    fn begin_control_learn(
        &mut self,
        window: &MainWindow,
        binds_port: bool,
        target: ControlTarget,
    ) {
        self.session.begin_control_learn(target, binds_port);
        self.announce_control_learn(window, target);
    }

    /// The status line and the mapping list for a learn gesture that has just
    /// started, whichever of the session's two ways began it.
    fn announce_control_learn(&self, window: &MainWindow, target: ControlTarget) {
        let label = self.session.control_target_label(&target);
        window.set_status_message(format!("Move a control to map {label}").as_str().into());
        self.refresh_midi_mappings(window);
    }

    /// Publish the ports, the mappings and the transport list onto the
    /// Preferences MIDI page.
    ///
    /// Rebuilt whole rather than touched field-wise, unlike the meters: this
    /// is a dialog page that changes when something is learned or removed,
    /// not something redrawn sixty times a second.
    fn refresh_midi_mappings(&self, window: &MainWindow) {
        let names: Vec<slint::SharedString> = self
            .midi_ports
            .iter()
            .map(|port| port.name.as_str().into())
            .collect();
        // JACK merges every hardware source into one port, so its picker has
        // one entry and no message on it says which keyboard sent it. That is
        // a driver fact rather than a mapping one, and a reader who does not
        // know it will read a single entry as a bug (`CONTROL_SURFACES.md`).
        let note = if self
            .midi_ports
            .iter()
            .any(|port| port.name == mooloop_engine::MERGED_MIDI_IN_LABEL)
        {
            "This driver merges every connected keyboard into one input, so a \
             message does not say which one sent it. Tell two controllers apart \
             by their MIDI channel."
        } else {
            ""
        };
        let views = self.session.control_binding_views(&self.midi_ports);
        let learning = self.session.control_learn_target();
        let transport: Vec<MidiTransportRow> = TransportControl::ALL
            .iter()
            .enumerate()
            .map(|(index, gesture)| {
                let target = ControlTarget::Transport(*gesture);
                let bound = views
                    .iter()
                    .find(|view| self.session.control_binding_target(view.index) == Some(target));
                MidiTransportRow {
                    gesture: index as i32,
                    label: gesture.label().into(),
                    source: bound.map_or("", |view| view.source.as_str()).into(),
                    listening: learning == Some(target),
                    binding: bound.map_or(-1, |view| view.index as i32),
                }
            })
            .collect();
        // Transport bindings are drawn in the transport list, beside the
        // gestures that have none. Listing them here as well would be the
        // same mapping in two places, each with its own remove button.
        let bindings: Vec<MidiBindingRow> = views
            .iter()
            .filter(|view| !view.transport)
            .map(|view| MidiBindingRow {
                index: view.index as i32,
                source: view.source.as_str().into(),
                target: view.target.as_str().into(),
                mode: view.mode.as_str().into(),
                has_takeover: view.takeover.is_some(),
                jump: view.takeover == Some(Takeover::Jump),
                inverted: view.inverted,
                unresolved: view.unresolved,
            })
            .collect();
        window.set_midi_learn_target(
            learning
                .map(|target| self.session.control_target_label(&target))
                .unwrap_or_default()
                .as_str()
                .into(),
        );
        window.set_preferences_midi_ports(ModelRc::from(Rc::new(VecModel::from(names))));
        window.set_preferences_midi_port_note(note.into());
        window.set_preferences_midi_bindings(ModelRc::from(Rc::new(VecModel::from(bindings))));
        window
            .set_preferences_midi_transport_rows(ModelRc::from(Rc::new(VecModel::from(transport))));
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
        window.set_can_add_track(self.session.buses.len() < MAX_BUSES);
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
        let solo_silenced = mooloop_core::mixer::solo_silenced(&self.session.buses);
        MixerStripRow {
            name: setup.bus.name.as_str().into(),
            color: channel_colors::to_slint(setup.bus.color),
            has_color: setup.bus.color.is_some(),
            muted: setup.bus.muted,
            volume: setup.bus.volume,
            pan: setup.bus.pan,
            output: setup.bus.output as i32,
            selected: self.session.effect_target == EffectTarget::Bus(index as u8),
            is_master: index == MASTER_BUS as usize,
            console: setup.bus.console,
            polarity: setup.bus.polarity,
            solo: setup.bus.solo,
            // Derived rather than stored, by the same function the engine is
            // told through: what a solo silences is a property of the whole
            // graph, and the strip dims its name rather than looking muted.
            solo_silenced: solo_silenced.get(index).copied().unwrap_or(false),
            strip: strip_row(&setup.bus.strip, self.audio_sample_rate),
            sends: self.send_rows(index),
            send_allowed: self.allowed_destinations(index),
            feed_count: self.session.bus_feed_count(index) as i32,
            allowed: self.allowed_destinations(index),
            // Levels are owned by the metering timer, which writes them in
            // place; rebuilding a row must not stamp them back to silence.
            // The clip latch is the same, and more so: it is the one field
            // here a user has to be shown, so a rename or a reroute clearing
            // it would be the strip forgetting what it had just reported.
            left_db: self.retained_meter(index, |row| row.left_db, METER_FLOOR_DB),
            right_db: self.retained_meter(index, |row| row.right_db, METER_FLOOR_DB),
            held_left_db: self.retained_meter(index, |row| row.held_left_db, METER_FLOOR_DB),
            held_right_db: self.retained_meter(index, |row| row.held_right_db, METER_FLOOR_DB),
            clipping: self.retained_meter(index, |row| row.clipping, false),
        }
    }

    /// One metering field of the strip as it stands, or its resting value if
    /// there is no strip there yet.
    fn retained_meter<T>(
        &self,
        index: usize,
        field: impl Fn(&MixerStripRow) -> T,
        fallback: T,
    ) -> T {
        self.mixer_strip_model
            .row_data(index)
            .as_ref()
            .map(field)
            .unwrap_or(fallback)
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
        window.set_editing_bus_console(setup.bus.console);
        window.set_editing_bus_polarity(setup.bus.polarity);
        window.set_editing_bus_solo(setup.bus.solo);
        window.set_editing_bus_has_color(setup.bus.color.is_some());
        window.set_editing_bus_color(channel_colors::to_slint(setup.bus.color));
        window.set_editing_bus_color_hex(
            setup.bus.color.map(|color| color.to_hex()).unwrap_or_default().into(),
        );
        window.set_editing_bus_strip(strip_row(&setup.bus.strip, self.audio_sample_rate));
        window.set_editing_bus_can_remove(self.session.can_remove_track(index));
        window.set_editing_bus_can_move_left(self.session.can_move_track(index, -1));
        window.set_editing_bus_can_move_right(self.session.can_move_track(index, 1));
        window.set_editing_bus_allowed(self.allowed_destinations(index));
        window.set_editing_bus_send_feed_count(self.session.track_send_count(index) as i32);
        window.set_editing_bus_sends(self.send_rows(index));
        // The same mask the output picker uses: an output and a send are
        // legal under one rule, so a send cannot creep past a check the
        // picker makes.
        window.set_editing_bus_send_allowed(self.allowed_destinations(index));
    }

    /// The sends on `bus`, in the order they were authored -- which is the
    /// order the engine's bank groups them in, so a row's position is the
    /// address a level change is sent to.
    fn send_rows(&self, bus: usize) -> ModelRc<MixerSendRow> {
        let Some(setup) = self.session.buses.get(bus) else {
            return ModelRc::from(Rc::new(VecModel::from(Vec::new())));
        };
        let rows: Vec<MixerSendRow> = setup
            .sends
            .iter()
            .map(|send| MixerSendRow {
                target: send.target as i32,
                target_name: self
                    .session
                    .buses
                    .get(send.target as usize)
                    .map(|track| track.bus.name.as_str())
                    .unwrap_or("--")
                    .into(),
                level: send.level,
                pre_fader: send.tap == SendTap::PreFader,
                enabled: send.enabled,
            })
            .collect();
        ModelRc::from(Rc::new(VecModel::from(rows)))
    }

    /// Push the selected channel's Aux In into its face.
    ///
    /// Everything the face shows comes from one walk of the project: the
    /// pickers, the tap the outlet declares, and the refusal, which is read
    /// from the compiled graph rather than re-derived here. A silent `None`
    /// would be indistinguishable from a working edge into a silent producer,
    /// which is why `compile_audio_graph` keeps a refused subscription
    /// inspectable in the first place.
    fn refresh_aux_in(&self, window: &MainWindow) {
        let consumer = self.session.selected;
        let Some(channel) = self.session.channels.get(consumer) else {
            return;
        };
        let params = channel.aux_in_params;

        let sources = aux_in_sources(&self.session, consumer);
        let mut source_names: Vec<SharedString> = vec!["None".into()];
        source_names.extend(sources.iter().map(|(_, name)| SharedString::from(name.as_str())));
        // Row zero is "None", so a channel's row is one past its position in
        // the filtered list.
        //
        // A subscription naming a channel that is no longer *offered* --
        // deleted, or switched to a device that publishes nothing -- shows
        // row zero, and the refusal below is what says so. That is a
        // deliberate split rather than the picker forgetting: the picker's
        // job is what can be chosen, and a channel publishing no audio is not
        // that, while the subscription itself is retained underneath and
        // resolves again if the producer comes back. The face is only honest
        // because both halves are drawn -- a "None" with no explanation beside
        // it would be the bug this arrangement looks like.
        let subscription = params.subscription();
        let source_index = subscription
            .and_then(|s| sources.iter().position(|(index, _)| *index == s.channel))
            .map_or(0, |row| row as i32 + 1);
        window.set_aux_in_source_names(source_names.as_slice().into());
        window.set_aux_in_source_index(source_index);

        let outlets = subscription
            .map(|s| aux_in_outlets(&self.session, s.channel))
            .unwrap_or_default();
        let outlet_names: Vec<SharedString> =
            outlets.iter().map(|o| SharedString::from(o.name)).collect();
        let outlet_row = outlets
            .iter()
            .position(|o| Some(o.id) == subscription.map(|s| s.outlet));
        window.set_aux_in_outlet_names(outlet_names.as_slice().into());
        window.set_aux_in_outlet_index(outlet_row.map_or(0, |row| row as i32));
        window.set_aux_in_tap_text(
            outlet_row
                .map(|row| outlets[row].tap.status())
                .unwrap_or_default()
                .into(),
        );

        // The pilot row (`docs/plans/generator-face-rows/`): `p2` carries
        // Level -- `aux_in::PARAM_LEVEL` -- normalized, the same way
        // `effect_slot_row` fills `p0..p17`.
        let level_normalized = aux_in::descriptor(aux_in::PARAM_LEVEL)
            .map_or(0.0, |descriptor| descriptor.to_normalized(params.level));
        window.set_source(SourceRow {
            kind: device_kind_to_int(DeviceKind::AuxIn),
            p2: level_normalized,
            ..Default::default()
        });
        window.set_aux_in_level_text(format!("{:.1} dB", linear_to_db(params.level)).into());

        let graph = self.session.audio_graph_plan();
        window.set_aux_in_refusal_text(
            graph
                .edge(consumer)
                .refusal()
                .map(aux_in_refusal_text)
                .unwrap_or_default()
                .into(),
        );
    }

    /// The selected channel's MIDI input, for the sidebar's IN and CH rows.
    ///
    /// Both lists come from `mooloop_core`, which owns the rows *and* the
    /// index-to-value mapping with round-trip tests behind it. Building them
    /// here from a list spelled in the markup, or reading a row back into a
    /// value by a `match` written here, would be the second copy.
    /// The selected channel's AUDIO row, and the static half of its sampler's
    /// RECORD page: the clip settings and what it records from. The live
    /// half -- the take's state, length and waveform -- is the pump's
    /// (`publish_take`), because it moves while nothing is edited.
    fn publish_audio_input(&self, window: &MainWindow, channel: &ChannelState) {
        use mooloop_core::AudioInputPicker;
        let rows = self.session.audio_source_rows(self.audio_input_label.as_deref());
        let picker = AudioInputPicker::new(&rows);
        let labels: Vec<SharedString> =
            picker.labels().into_iter().map(SharedString::from).collect();
        window.set_audio_input_options(ModelRc::from(Rc::new(VecModel::from(labels))));
        window.set_audio_input_index(picker.row(channel.audio_input) as i32);
        let missing = picker.is_missing(channel.audio_input);
        window.set_audio_input_missing(missing);
        let hardware = channel.audio_input == mooloop_core::AudioInputSource::Input;
        window.set_audio_input_is_hardware(hardware && !missing);
        window.set_audio_monitor(self.session.input_monitor.contains(&channel.id));
        let source = if channel.audio_input.is_off() || missing {
            String::new()
        } else {
            picker.labels()[picker.row(channel.audio_input)].clone()
        };
        window.set_sampler_record_source(source.into());
        window.set_sampler_record_clip(channel.record.clip);
        window.set_sampler_record_bars(i32::from(channel.record.bars));
        window.set_sampler_record_max_bars(i32::from(mooloop_core::MAX_RECORD_BARS));
    }

    /// The selected channel's take, for its sampler's RECORD page. Called by
    /// the pump every tick; cheap when nothing is recording.
    fn publish_take(&self, window: &MainWindow) {
        let Some(channel) = self.session.channels.get(self.session.selected) else {
            return;
        };
        let view = self.takes.view(channel.id).filter(|view| view.is_live());
        let Some(view) = view else {
            if window.get_sampler_record_state() != RecordFace::Idle.as_i32() {
                window.set_sampler_record_state(RecordFace::Idle.as_i32());
                window.set_sampler_record_peaks(ModelRc::default());
                window.set_sampler_record_elapsed(SharedString::new());
            }
            return;
        };
        let bpm = f64::from(window.get_bpm().max(1));
        let frames_per_bar =
            f64::from(self.audio_sample_rate) * 60.0 / bpm * f64::from(mooloop_core::time::BEATS_PER_BAR);
        let elapsed = view.frames as f64 / frames_per_bar.max(1.0);
        let (state, text, fill) = match view.phase {
            phase @ mooloop_engine::TakePhase::Waiting => {
                (RecordFace::from(phase), String::new(), 1.0)
            }
            phase if channel.record.clip => {
                let bars = f64::from(channel.record.bars);
                (
                    RecordFace::from(phase),
                    format!("{elapsed:.1} / {} BARS", channel.record.bars),
                    (elapsed / bars).clamp(0.0, 1.0) as f32,
                )
            }
            phase => (RecordFace::from(phase), format!("{elapsed:.1} BARS"), 1.0),
        };
        window.set_sampler_record_state(state.as_i32());
        window.set_sampler_record_elapsed(text.into());
        window.set_sampler_record_fill(fill);
        let bars = view.bars(RECORD_PAGE_BARS);
        window.set_sampler_record_peaks(ModelRc::from(Rc::new(VecModel::from(bars))));
    }

    fn publish_midi_input(&self, window: &MainWindow, channel: &ChannelState) {
        use mooloop_core::{MidiChannelFilter, MidiInputSource, MIDI_CHANNEL_FILTER_ROWS};

        let ports = &self.midi_ports;
        let rows: Vec<SharedString> = MidiInputSource::picker_rows(ports)
            .into_iter()
            .map(SharedString::from)
            .collect();
        window.set_midi_input_options(ModelRc::from(Rc::new(VecModel::from(rows))));
        window.set_midi_input_index(channel.midi_input.source.row(ports) as i32);
        // A port the project names and the system does not have. The row falls
        // back to "Follow Selection", so the panel has to say this out loud or
        // it is misreporting what the channel is set to.
        let missing = channel.midi_input.source.is_missing(ports);
        window.set_midi_input_missing(missing);
        window.set_midi_input_missing_name(
            channel
                .midi_input
                .source
                .port_name()
                .filter(|_| missing)
                .unwrap_or_default()
                .into(),
        );
        let channels: Vec<SharedString> = (0..MIDI_CHANNEL_FILTER_ROWS)
            .map(|row| SharedString::from(MidiChannelFilter::from_row(row).label()))
            .collect();
        window.set_midi_channel_options(ModelRc::from(Rc::new(VecModel::from(channels))));
        window.set_midi_channel_index(channel.midi_input.channel.row() as i32);
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
        // Three properties for one optional colour, because Slint has no
        // `Option`: whether there is one, what it is to draw, and what it is
        // to store and to compare a swatch against. The hex is the canonical
        // upper-case spelling, which is what makes the selected swatch
        // findable by string comparison.
        window.set_selected_channel_has_color(ch.color.is_some());
        window.set_selected_channel_color_hex(
            ch.color.map(|color| color.to_hex()).unwrap_or_default().into(),
        );
        window.set_selected_channel_color(channel_colors::to_slint(ch.color));
        self.publish_midi_input(window, ch);
        self.publish_audio_input(window, ch);
        window.set_selected_channel_volume_db(linear_to_db(ch.volume));
        window.set_source_kind(device_kind_to_int(ch.kind));
        // Derived rather than remembered per channel: the selection names one
        // chain, so switching chains unselects without anything being cleared.
        window.set_source_selected(self.session.source_is_selected());
        window.set_source_preset_name(
            self.session
                .source_preset_name(self.session.selected as u8)
                .unwrap_or_default()
                .into(),
        );
        self.sync_effects();
        self.refresh_modulation(window);
        // The lane's destination catalogue is built from the effect chains, so
        // it has to be rebuilt wherever the chains or the selection can have
        // moved -- which is exactly this function's job.
        self.refresh_automation(window);
        window.set_drum_mode(drum.mode.to_index());
        window.set_drum_kick_character(drum.kick_character.to_index());
        window.set_drum_snare_character(drum.snare_character.to_index());
        window.set_drum_hat_character(drum.hat_character.to_index());
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
        window.set_mlp8_master_volume(mlp8.master_volume);
        window.set_mlp8_master_pan(mlp8.master_pan);
        window.set_mlp8_drift(mlp8.drift);
        window.set_mlp8_unison(mlp8.unison.to_index());
        window.set_mlp8_detune(mlp8.detune);
        window.set_mlp8_spread(mlp8.spread);
        window.set_mlp8_chorus(mlp8.chorus.to_index());
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
        if ch.kind == DeviceKind::AuxIn {
            self.refresh_aux_in(window);
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
        window.set_poly_mono_mode(poly.mono_mode);
        window.set_poly_env_trigger(poly.env_trigger.to_index());
        window.set_poly_note_priority(poly.note_priority.to_index());
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
        window.set_attack(p.attack);
        window.set_decay(p.decay);
        window.set_sustain(p.sustain);
        window.set_release(p.release);
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
        window.set_sampler_filter_attack(filter_env.attack);
        window.set_sampler_filter_decay(filter_env.decay);
        window.set_sampler_filter_sustain(filter_env.sustain);
        window.set_sampler_filter_release(filter_env.release);
        self.refresh_note_editor(window);
    }
}

impl AppUi {
    pub fn new(mut handle: EngineHandle) -> Result<Self, slint::PlatformError> {
        let window = MainWindow::new()?;
        prepare_recordings_folder(&window);
        // From here on a quit signal is the pump's to answer; see `signals`.
        signals::install();

        // A Wayland compositor identifies a window by its xdg app id, and
        // Slint sends none unless it is set: Hyprland reported `class: ""`,
        // which is every window rule, every taskbar grouping, and the icon
        // lookup into `mooloop.desktop` failing at once. The string has to
        // match that file's basename. It is set here rather than in `main`
        // because the call needs a platform, which creating the window
        // established, and the id is read when the window is first shown.
        if let Err(error) = slint::set_xdg_app_id("mooloop") {
            log_warn!("app", "could not set the xdg app id: {error}");
        }

        // The shipped patches become ordinary user presets the first time
        // the app runs, and are not touched again. A failure here is not
        // worth refusing to start over: the bank is content, not
        // configuration, and the browser simply shows one fewer category.
        if let Err(error) = mooloop_project::seed_mlm1_bank(&settings::channel_presets_dir()) {
            log_warn!("app", "could not write the ML-M1 factory bank: {error}");
        }
        // The DS-01 bank, in that device's own generator directory. Generator
        // rather than channel because DS-01's modulation is inside its voice:
        // a patch carries its own matrix and needs no channel rack.
        if let Err(error) =
            mooloop_project::seed_ds01_bank(&settings::generator_presets_dir(DeviceKind::Ds01))
        {
            log_warn!("app", "could not write the DS-01 factory bank: {error}");
        }
        // The ML-P8 bank, on the same terms and for the same reason: its
        // modulation is its own routes and its own LFO, so a patch reaches
        // its sound with no channel rack at all.
        if let Err(error) =
            mooloop_project::seed_mlp8_bank(&settings::generator_presets_dir(DeviceKind::MlP8))
        {
            log_warn!("app", "could not write the ML-P8 factory bank: {error}");
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
        window.invoke_show_view(view_id(Pane::Source));
        // The two ceilings the markup enables Add/Clone/Paste against. They
        // are handed over once, from the core's own constants, so raising a
        // cap never leaves a menu row disabled at the old number --
        // `docs/CAPACITY_POLICY.md` names that exact symptom.
        window.set_max_channels(MAX_CHANNELS as i32);
        window.set_max_patterns(MAX_PATTERNS as i32);
        // Two lists the device declares once; nothing about a patch moves
        // them, so they are installed here rather than on every refresh.
        install_mlp8_route_vocabularies(&window);
        // The channel sidebar's swatches are pushed by `apply_appearance`
        // now, because they follow the colourscheme: choosing Nord has to
        // change the pickers in the same frame it changes everything else.
        // A project stores the colour it was given rather than which swatch
        // was clicked, so the list can change without touching a saved file.
        // Startup, on an engine that has consumed nothing yet: a refusal here
        // is not a full ring, it is a broken one, and there is no UI up yet to
        // say so with. Both sends run unconditionally -- the results are
        // collected first and judged after, because a `&&` would make the
        // second depend on the first.
        let tempo_sent = handle.send(EngineCommand::SetTempo(INITIAL_BPM as f64));
        let swing_sent = handle.send(EngineCommand::SetSwing(DEFAULT_SWING_PERCENT));
        if !(tempo_sent && swing_sent) {
            log_error!(
                "engine",
                "the command queue refused the opening tempo and swing on an \
                 engine that has not started: the transport will run at the \
                 engine's defaults"
            );
        }

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
        let state = Rc::new(RefCell::new(UiState::new(
            default_sample.as_deref(),
            audio_sample_rate,
            &window,
        )));
        {
            // What `UiState::new` cannot ask for itself; see its comment.
            let mut st = state.borrow_mut();
            st.audio_input_label = handle.audio_input_label();
            st.input_latency_frames = handle.input_latency_frames();
        }
        let starter = Project::starter_kit(fresh_starter_seed());
        let starter_samples = vec![None; starter.channels.len()];
        install_project_in_ui(
            &mut handle,
            default_sample.as_ref(),
            &state,
            &window,
            &starter,
            &starter_samples,
            // Startup: there is nothing playing to keep.
            false,
        );
        state.borrow().update_document_title(&window);
        state.borrow().sync_pattern_menu(&window);
        state.borrow().sync_mixer(&window);
        window.set_app_version(env!("CARGO_PKG_VERSION").into());

        let (document_tx, document_rx) = std::sync::mpsc::channel::<DocumentResult>();
        let command_state = Rc::new(RefCell::new(CommandState::default()));
        sync_command_availability(&window, &command_state.borrow());
        let export_sample_rate = handle.sample_rate();

        // Set when Quit or the window's close button arrives during a save,
        // and read by the pump once the operation has reported (MOO-92).
        let quit_after_document = Rc::new(Cell::new(false));
        // The in-app question and what it is asking (MOO-91).
        let question: Rc<RefCell<Option<Question>>> = Rc::new(RefCell::new(None));
        // Set by an answer that has settled "unsaved changes?", just before
        // it re-enters Quit, New or Open, which read and clear it -- always,
        // so it can never outlive the one command it was set for.
        let unsaved_settled = Rc::new(Cell::new(false));
        // What a Save answer goes on to once the song has been saved. The
        // pump takes it with the save's result, so a failed or cancelled
        // save drops it and leaves the user where they were.
        let after_save: Rc<Cell<Option<AfterUnsaved>>> = Rc::new(Cell::new(None));
        // Set by a yes to "this kit drops channels", read by the pump when
        // the waiting result comes back round.
        let kit_confirmed = Rc::new(Cell::new(false));
        // The takes dialog, while it is up (MOO-38).
        let takes_review: Rc<RefCell<Option<TakesReview>>> = Rc::new(RefCell::new(None));
        {
            let st = state.clone();
            let quit_commands = command_state.clone();
            let weak = window.as_weak();
            let quit_after_document = quit_after_document.clone();
            let question = question.clone();
            let unsaved_settled = unsaved_settled.clone();
            let takes_review = takes_review.clone();
            window.on_quit_requested(move || {
                let Some(window) = weak.upgrade() else {
                    return;
                };
                // A save in flight finishes first; the pump asks again after
                // it (MOO-92).
                if defer_quit_while_busy(&window, &quit_after_document) {
                    return;
                }
                // **Never blocks on a program that may not be there** (MOO-91):
                // the question is the app's own, and every answer leads
                // somewhere -- saved and gone, dropped and gone, or still
                // open. With a zenity question, no zenity meant no quit.
                //
                // **A live take is not asked about** (open question 9,
                // answered 2026-09-21): quit finishes it the way Stop would and leaves,
                // because what is outstanding is a fraction of a second rather
                // than something worth a dialog. `AppUi::finish_takes` does
                // that work after the loop. It is not `dirty` either way -- a
                // take is not an edit until it lands on a channel.
                let settled = unsaved_settled.replace(false);
                if st.borrow().session.dirty && !settled {
                    let name = song_name(&st.borrow());
                    ask_unsaved(&window, &question, AfterUnsaved::Quit, name.as_deref());
                    return;
                }
                // After the decision to quit, never before it: a cancelled
                // quit must not have tidied anything away.
                let unused = unused_session_takes(&st.borrow(), &quit_commands.borrow());
                if unused.is_empty() {
                    slint::quit_event_loop().ok();
                    return;
                }
                let review = TakesReview::new(
                    recordings::CleanUp {
                        not_used: unused,
                        earlier: Vec::new(),
                    },
                    true,
                );
                show_takes_review(&window, &review);
                *takes_review.borrow_mut() = Some(review);
            });
        }
        {
            let st = state.clone();
            let tx = document_tx.clone();
            let weak = window.as_weak();
            let question = question.clone();
            let unsaved_settled = unsaved_settled.clone();
            window.on_new_song(move || {
                let dirty = st.borrow().session.dirty;
                let settled = unsaved_settled.replace(false);
                let Some(window) = weak.upgrade() else {
                    return;
                };
                if dirty && !settled {
                    if window.get_document_busy() {
                        say_busy(&window);
                    } else {
                        let name = song_name(&st.borrow());
                        ask_unsaved(&window, &question, AfterUnsaved::NewSong, name.as_deref());
                    }
                    return;
                }
                if !begin_document_operation(&window, "Creating new song...") {
                    return;
                }
                let tx = tx.clone();
                std::thread::spawn(move || {
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
            let question = question.clone();
            let unsaved_settled = unsaved_settled.clone();
            window.on_open_song(move || {
                let dirty = st.borrow().session.dirty;
                let settled = unsaved_settled.replace(false);
                let Some(window) = weak.upgrade() else {
                    return;
                };
                if dirty && !settled {
                    if window.get_document_busy() {
                        say_busy(&window);
                    } else {
                        let name = song_name(&st.borrow());
                        ask_unsaved(&window, &question, AfterUnsaved::OpenSong, name.as_deref());
                    }
                    return;
                }
                if !begin_document_operation(&window, "Opening song...") {
                    return;
                }
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let path = match chosen_path(pick_song_dialog("Open mooloop song"), "open a song") {
                        Ok(path) => path,
                        Err(result) => {
                            let _ = tx.send(result);
                            return;
                        }
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
                let generation = st.borrow().session.document_generation;
                if !begin_document_operation(&window, "Saving song...") {
                    return;
                }
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let picked = current.map(Picked::Path).unwrap_or_else(|| {
                        pick_save_dialog("Save mooloop song", "Untitled.mooloop")
                    });
                    let path = match chosen_path(picked, "save this song") {
                        Ok(path) => path,
                        Err(result) => {
                            let _ = tx.send(result);
                            return;
                        }
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
                                generation,
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
                if !begin_document_operation(&window, "Saving kit...") {
                    return;
                }
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let path = match chosen_path(
                        pick_save_dialog("Save mooloop kit", "Untitled.mooloop-kit"),
                        "save this kit",
                    ) {
                        Ok(path) => path,
                        Err(result) => {
                            let _ = tx.send(result);
                            return;
                        }
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
                let channel = snapshot.channels[snapshot.selected_index()]
                    .setup
                    .clone();
                let mode = asset_mode_from_window(&window);
                if !begin_document_operation(&window, "Saving channel...") {
                    return;
                }
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let path = match chosen_path(
                        pick_save_dialog("Save mooloop channel", "Untitled.mooloop-channel"),
                        "save this channel",
                    ) {
                        Ok(path) => path,
                        Err(result) => {
                            let _ = tx.send(result);
                            return;
                        }
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
                let Some(window) = weak.upgrade() else {
                    return;
                };
                let status = if kit { "Loading kit..." } else { "Loading channel..." };
                if !begin_document_operation(&window, status) {
                    return;
                }
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let path = match chosen_path(pick_bundle_dialog(title), "open this file") {
                        Ok(path) => path,
                        Err(result) => {
                            let _ = tx.send(result);
                            return;
                        }
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
                let Some((path, preset_name)) = ({
                    let st = st.borrow();
                    let presets = if generator {
                        &st.session.generator_presets
                    } else {
                        &st.session.channel_presets
                    };
                    presets
                        .get(index as usize)
                        .map(|preset| (preset.path.clone(), preset.name.clone()))
                }) else {
                    return;
                };
                let Some(window) = weak.upgrade() else { return };
                let target = if generator {
                    LoadTarget::Generator { preset_name }
                } else {
                    LoadTarget::Channel
                };
                load_preset_document(&tx, &window, path, target, label);
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
            let question = question.clone();
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
                // Which device will wear the name it was saved under, the
                // same way it wears the name of a preset loaded into it --
                // *resolved* here and *applied* when the write comes back, so
                // a save that fails leaves the rack row saying what is
                // actually on disk. The channel has to be read here, while
                // the dialog's own selection is still the current one.
                let named = match source.target {
                    PresetSaveTarget::Effect { target, device } => Some(PresetNaming::Effect {
                        target,
                        device,
                        name: name.clone(),
                    }),
                    PresetSaveTarget::Generator => {
                        let st = st.borrow();
                        st.session.channel_id(st.session.selected).map(|channel| {
                            PresetNaming::Source {
                                channel,
                                name: name.clone(),
                            }
                        })
                    }
                    // A channel preset spans the generator and the mixer, so
                    // no one device is the thing it names.
                    PresetSaveTarget::Channel => None,
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
                    // preset can only ever be offered to a delay row -- and a
                    // container's run only to another container.
                    PresetSaveTarget::Effect { .. } => match (&source.run, source.effect) {
                        (Some(_), _) => (
                            settings::effect_presets_dir(EffectKind::Chain),
                            "mooloop-effect-run",
                            "Container preset saved",
                        ),
                        (None, Some(effect)) => (
                            settings::effect_presets_dir(effect.kind()),
                            "mooloop-effect",
                            "Effect preset saved",
                        ),
                        (None, None) => return,
                    },
                };
                let path = dir.join(format!("{file_stem}.{extension}"));
                // `replace_bundle` backs the old one up, installs the new one
                // and deletes the backup, so a name collision is a silent
                // overwrite of somebody's work. `factory.rs` refuses to do
                // this and says why -- "a first launch after an update
                // silently replacing someone's work is the worst thing this
                // code could do" -- and the ordinary save dialog was doing it
                // on every confirm. Note the collision can also come from
                // sanitising: "My Delay" and "My/Delay" are both `My_Delay`.
                //
                // Asked in the app's own dialog (MOO-91): the zenity question
                // this was ran on the UI thread and froze it, meters and MIDI
                // included, until it was answered.
                let target = source.target;
                let taken = path.exists();
                let tx = tx.clone();
                let save = move |window: &MainWindow| {
                    if !begin_document_operation(window, "Saving preset...") {
                        return;
                    }
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
                            PresetSaveTarget::Effect { .. } => match (&source.run, source.effect) {
                                // A container saves as its run: the box and
                                // everything in it, which is the whole point of
                                // there being a box.
                                (Some(run), _) => mooloop_project::save_effect_run_preset(
                                    &path,
                                    run,
                                    info,
                                    AssetMode::Embedded,
                                ),
                                (None, Some(effect)) => mooloop_project::save_effect_preset(
                                    &path,
                                    &effect,
                                    info,
                                    AssetMode::Embedded,
                                ),
                                (None, None) => return,
                            },
                        };
                        let result = result
                            .map(|report| DocumentResult::SavedPreset {
                                label,
                                report,
                                named,
                            })
                            .unwrap_or_else(|error| DocumentResult::Failed {
                                action: "save this preset",
                                problem: error.into(),
                            });
                        let _ = tx.send(result);
                    });
                };
                if taken {
                    ask_question(
                        &window,
                        &question,
                        Question::ReplacePreset {
                            target,
                            save: Box::new(save),
                        },
                        &format!("Replace the preset \"{file_stem}\"?"),
                        "A preset of that name is already saved here. Replacing it cannot be undone.",
                        "Replace",
                        "",
                    );
                    return;
                }
                save(&window);
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
                if !begin_document_operation(&window, "Rendering audio...") {
                    return;
                }
                window.set_export_open(false);
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let path = match chosen_path(
                        pick_export_dialog(request.extension()),
                        "export this song",
                    ) {
                        Ok(path) => path,
                        Err(result) => {
                            let _ = tx.send(result);
                            return;
                        }
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
            let close_commands = command_state.clone();
            let weak = window.as_weak();
            let quit_after_document = quit_after_document.clone();
            let question = question.clone();
            let takes_review = takes_review.clone();
            window.window().on_close_requested(move || {
                let Some(window) = weak.upgrade() else {
                    return CloseRequestResponse::HideWindow;
                };
                // A save in flight finishes first; the pump quits after it.
                if defer_quit_while_busy(&window, &quit_after_document) {
                    return CloseRequestResponse::KeepWindowShown;
                }
                // Closing the window is the other way out, and it asks the
                // same questions as the Quit menu row, in the same order and
                // the same words. The window stays up only while a question
                // is on screen: every answer to it goes through Quit's own
                // path (MOO-91), which leaves by `quit_event_loop`.
                // No prompt for a live take here either; see the Quit row.
                if st.borrow().session.dirty {
                    let name = song_name(&st.borrow());
                    ask_unsaved(&window, &question, AfterUnsaved::Quit, name.as_deref());
                    return CloseRequestResponse::KeepWindowShown;
                }
                let unused = unused_session_takes(&st.borrow(), &close_commands.borrow());
                if unused.is_empty() {
                    return CloseRequestResponse::HideWindow;
                }
                let review = TakesReview::new(
                    recordings::CleanUp {
                        not_used: unused,
                        earlier: Vec::new(),
                    },
                    true,
                );
                show_takes_review(&window, &review);
                *takes_review.borrow_mut() = Some(review);
                CloseRequestResponse::KeepWindowShown
            });
        }
        // --- The in-app question's answers (MOO-91) ---
        {
            let st = state.clone();
            let weak = window.as_weak();
            let question = question.clone();
            let unsaved_settled = unsaved_settled.clone();
            let after_save = after_save.clone();
            let kit_confirmed = kit_confirmed.clone();
            let tx = document_tx.clone();
            window.on_question_answered(move |answer| {
                let Some(window) = weak.upgrade() else {
                    return;
                };
                let Some(asked) = question.borrow_mut().take() else {
                    return;
                };
                match asked {
                    Question::Unsaved(after) => match unsaved_step(after, answer) {
                        UnsavedStep::SaveThen(after) => {
                            after_save.set(Some(after));
                            // The ordinary save, chooser and all. Refused only
                            // if something is already running, and then the
                            // continuation must not wait for a save that never
                            // started.
                            window.invoke_save_song();
                            if !window.get_document_busy() {
                                after_save.set(None);
                            }
                        }
                        UnsavedStep::Go(after) => {
                            go_on_after_unsaved(&window, &unsaved_settled, after);
                        }
                        UnsavedStep::Stay => window.set_status_message("".into()),
                    },
                    Question::ReplacePreset { target, save } => {
                        if answer == 1 {
                            save(&window);
                        } else {
                            // Back to the preset dialog, which still saves
                            // under another name.
                            st.borrow_mut().session.pending_preset_save = Some(target);
                        }
                    }
                    Question::LoadKit(result) => {
                        if answer == 1 {
                            kit_confirmed.set(true);
                            let _ = tx.send(*result);
                        } else {
                            window.set_document_busy(false);
                            window.set_status_message("Kit load cancelled".into());
                        }
                    }
                }
            });
        }
        // --- Unused takes (MOO-38) ---
        {
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            let takes_review = takes_review.clone();
            window.on_clean_up_takes(move || {
                let Some(window) = weak.upgrade() else {
                    return;
                };
                let lists = {
                    let st = st.borrow();
                    let referenced = recordings::referenced_by(&st.session, &commands.borrow().history);
                    // The song's own `recordings/`, held against the song as
                    // saved on disk as well: a file it plays there is not
                    // unused because an unsaved edit stopped playing it.
                    let song = st.session.bundle_path.as_deref().and_then(|song| {
                        let folder = mooloop_project::song_assets_dir(song)?
                            .join(mooloop_project::RECORDINGS_DIR);
                        Some((folder, recordings::saved_song_references(song)?))
                    });
                    recordings::clean_up(
                        &settings::recordings_dir(),
                        song.as_ref().map(|(folder, saved)| (folder.as_path(), saved)),
                        &referenced,
                        st.session_start,
                    )
                };
                if lists.not_used.is_empty() && lists.earlier.is_empty() {
                    window.set_status_message(
                        "No unused takes: this song or its undo history uses every recording"
                            .into(),
                    );
                    return;
                }
                let review = TakesReview::new(lists, false);
                show_takes_review(&window, &review);
                *takes_review.borrow_mut() = Some(review);
            });
        }
        {
            let weak = window.as_weak();
            let takes_review = takes_review.clone();
            window.on_take_toggled(move |earlier, index| {
                let (Some(window), Ok(index)) = (weak.upgrade(), usize::try_from(index)) else {
                    return;
                };
                let mut review = takes_review.borrow_mut();
                let Some(review) = review.as_mut() else {
                    return;
                };
                review.toggle(earlier, index);
                show_takes_review(&window, review);
            });
        }
        {
            let weak = window.as_weak();
            let takes_review = takes_review.clone();
            window.on_takes_confirmed(move || {
                let Some(review) = takes_review.borrow_mut().take() else {
                    return;
                };
                let ticked = review.ticked();
                let quitting = review.quitting;
                let weak = weak.clone();
                // Off the UI thread: the desktop's trash is a file move on
                // Linux and a conversation with Finder on macOS.
                std::thread::spawn(move || {
                    let (moved, failures) =
                        recordings::discard_all(&recordings::DesktopTrash, &ticked);
                    log_info!("ui", "moved {moved} unused take(s) to the trash");
                    for failure in &failures {
                        log_error!("ui", "a take could not be moved to the trash: {failure}");
                    }
                    let _ = weak.upgrade_in_event_loop(move |window| {
                        if quitting {
                            slint::quit_event_loop().ok();
                            return;
                        }
                        let noun = if moved == 1 { "take" } else { "takes" };
                        let message = if failures.is_empty() {
                            format!("Moved {moved} {noun} to the trash")
                        } else {
                            format!(
                                "Moved {moved} {noun} to the trash; {} could not be moved (see the log)",
                                failures.len()
                            )
                        };
                        window.set_status_message(message.into());
                    });
                });
            });
        }
        {
            let takes_review = takes_review.clone();
            window.on_takes_cancelled(move || {
                // Opened by a quit: "Keep and Quit" still quits.
                if takes_review.borrow_mut().take().is_some_and(|review| review.quitting) {
                    slint::quit_event_loop().ok();
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
        // The pump's own sender, for the messages a project load keeps.
        let requeue_tx = pending_tx.clone();
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
        // A theme file that will not parse is skipped, and saying so is the
        // whole of what "skipped with a message" means -- a themes directory
        // somebody has been editing by hand is the ordinary case, and a file
        // that silently does not appear is indistinguishable from one that was
        // never saved.
        for warning in crate::theme::catalog::warnings() {
            eprintln!("mooloop: ignoring theme {warning}");
        }
        // Kept alive here for as long as the app runs so the window survives
        // after this constructor returns; re-opening while it is already up
        // just refocuses it instead of spawning a second one.
        #[cfg(feature = "mockup")]
        let mockup_window: Rc<RefCell<Option<mockup_ui::MockupCanvas>>> =
            Rc::new(RefCell::new(None));
        // The Developer page hides its tools row entirely rather than offering
        // a button that would open nothing.
        window.set_preferences_mockup_tool_available(cfg!(feature = "mockup"));
        // Once, before anything is drawn: the strip's faces read every range
        // and every parameter id out of this rather than spelling them.
        install_strip_spec(&window);
        install_eq_spec(&window);
        {
            let settings = ui_settings.borrow();
            apply_appearance(&window, &settings.appearance);
            apply_layout(&window, &settings.layout);
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
            let st = state.clone();
            let settings = ui_settings.clone();
            let panic_tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_shortcut_key(move |key, ctrl, shift, alt, meta| {
                let Some(window) = weak.upgrade() else {
                    return false;
                };
                // Read and dropped before anything is dispatched: an arm
                // below can open Preferences, which borrows the settings in
                // its turn.
                let super_key = settings.borrow().shortcuts.super_key;
                let chord =
                    actions::KeyChord::from_event(super_key, ctrl, shift, alt, meta, key.as_str());
                let Some(action_id) = table.borrow().resolve(&chord) else {
                    return false;
                };
                let channel = window.get_selected_channel();
                // Where a `Scope::Focused` action points. Read once, before
                // any arm runs: an arm that changes the selection would
                // otherwise be answering a question its own effect moved.
                let surface = focused_surface(&window);
                match action_id {
                    "transport.play-pause" => window.invoke_toggle_play(),
                    // Stop already rewinds -- `on_stop_clicked` zeroes the
                    // position as well as sending `EngineCommand::Stop` --
                    // so return-to-start is the half of it that leaves the
                    // transport running, and it is the playhead drag's own
                    // callback with the tick it clamps to.
                    "transport.stop" => window.invoke_stop_clicked(),
                    "transport.return-to-start" => window.invoke_playlist_seek(0),
                    "transport.loop-toggle" => window.invoke_playlist_loop_enabled_changed(
                        !window.get_playlist_loop_enabled(),
                    ),
                    "transport.record-arm-toggle" => window.invoke_record_armed_toggled(),
                    // Performance state, not an edit: nothing to record.
                    "transport.panic" => {
                        let _ = panic_tx.send(EngineCommand::Panic);
                        window.set_status_message("All notes off".into());
                    }
                    "midi.learn-toggle" => window.invoke_midi_learn_toggled(),
                    "file.new" => window.invoke_new_song(),
                    "file.open" => window.invoke_open_song(),
                    "file.save" => window.invoke_save_song(),
                    "file.save-as" => window.invoke_save_song_as(),
                    "file.export" => window.invoke_export_audio(),
                    "recording.clean-up" => window.invoke_clean_up_takes(),
                    "file.quit" => window.invoke_quit_requested(),
                    "edit.undo" => window.invoke_edit_command_requested(0, channel),
                    "edit.redo" => window.invoke_edit_command_requested(1, channel),
                    // The three clipboards, resolved against the focused
                    // surface (`docs/ACTIONS.md`): notes on the roll, the
                    // selected device in the rack, the channel everywhere
                    // else -- which is the fallback these chords have always
                    // been, so nothing a user relied on changed shape.
                    // A rack surface with nothing selected falls through to
                    // the channel rather than doing nothing, because "the
                    // rack is where I clicked last" and "I have a device
                    // picked out" are different claims.
                    "edit.cut-channel" => match surface {
                        actions::Surface::Notes => window.invoke_piano_notes_copied(true),
                        actions::Surface::Rack if selected_device_slot(&st).is_some() => {
                            window.invoke_device_clipboard_action(1)
                        }
                        _ => window.invoke_edit_command_requested(2, channel),
                    },
                    "edit.copy-channel" => match surface {
                        actions::Surface::Notes => window.invoke_piano_notes_copied(false),
                        actions::Surface::Rack if selected_device_slot(&st).is_some() => {
                            window.invoke_device_clipboard_action(0)
                        }
                        _ => window.invoke_edit_command_requested(3, channel),
                    },
                    "edit.paste-channel" => {
                        // Paste does not need a selection -- it needs
                        // something on the clipboard it is about to use, so
                        // each arm asks about its own.
                        let has_notes = window.get_showing_notes()
                            && !commands.borrow().note_clipboard.is_empty();
                        let has_device = commands.borrow().device_clipboard.is_some();
                        match surface {
                            actions::Surface::Rack if has_device => {
                                window.invoke_device_clipboard_action(2)
                            }
                            _ if has_notes => window.invoke_piano_notes_pasted(),
                            _ => window.invoke_edit_command_requested(4, channel),
                        }
                    }
                    // The four arrows. Left/Right nudge on the roll and
                    // walk the tree in the browser; Up/Down transpose,
                    // change the browser's row, or -- the fallback, and what
                    // `main.slint` used to do in markup -- pick a channel.
                    "notes.nudge-earlier" | "notes.nudge-later" => {
                        let forward = action_id == "notes.nudge-later";
                        match surface {
                            actions::Surface::Notes => {
                                let step = if window.get_piano_snap_enabled() {
                                    window.get_piano_snap_ticks().max(1)
                                } else {
                                    1
                                };
                                let sign = if forward { 1 } else { -1 };
                                window.invoke_piano_notes_nudged(sign * step, 0);
                            }
                            actions::Surface::Browser => {
                                if !browser_step_horizontally(&st, &window, forward) {
                                    return false;
                                }
                            }
                            _ => return false,
                        }
                    }
                    "notes.nudge-up" | "notes.nudge-down" => {
                        let delta = if action_id == "notes.nudge-up" { -1 } else { 1 };
                        match surface {
                            actions::Surface::Notes => {
                                window.invoke_piano_notes_nudged(0, -delta);
                            }
                            actions::Surface::Browser => {
                                if !browser_move_focus(&st, &window, delta) {
                                    return false;
                                }
                            }
                            _ => {
                                let last = window.get_channels().row_count() as i32 - 1;
                                let next =
                                    (window.get_selected_channel() + delta).clamp(0, last.max(0));
                                window.invoke_channel_selected(next);
                            }
                        }
                    }
                    // The device clipboard's own unambiguous chords. Bare
                    // Ctrl+C reaches the same three verbs when the rack is
                    // the focused surface; these reach them from anywhere,
                    // which is what makes them worth keeping.
                    "device.copy" => window.invoke_device_clipboard_action(0),
                    "device.cut" => window.invoke_device_clipboard_action(1),
                    "device.paste" => window.invoke_device_clipboard_action(2),
                    "device.duplicate" => window.invoke_device_clipboard_action(3),
                    // The rest of the rack's row of rail buttons, aimed at
                    // the selected device. Every one of them is the callback
                    // that button already invokes, so the keyboard and the
                    // rail cannot disagree about what a verb does.
                    "device.bypass" | "device.remove" | "device.wrap" | "device.save-preset" => {
                        let Some(slot) = selected_device_slot(&st) else {
                            return false;
                        };
                        match action_id {
                            "device.bypass" => window.invoke_effect_bypass_toggled(slot),
                            "device.remove" => window.invoke_remove_effect_clicked(slot),
                            "device.wrap" => window.invoke_wrap_effect_clicked(slot),
                            _ => window.invoke_save_effect_preset_requested(slot),
                        }
                    }
                    // Walking the chain. With nothing selected these take
                    // the first or last device, which is how the rack is
                    // reached from the keyboard at all -- so they are not
                    // `Scope::Rack`, unlike everything above.
                    "device.next" | "device.prev" => {
                        let count = window.get_effect_slots().row_count() as i32;
                        if count == 0 {
                            return false;
                        }
                        let delta = if action_id == "device.next" { 1 } else { -1 };
                        let next = match selected_device_slot(&st) {
                            Some(slot) => (slot + delta).clamp(0, count - 1),
                            None if delta > 0 => 0,
                            None => count - 1,
                        };
                        // `device-selected` clears the selection when it is
                        // handed the slot already selected, which is the
                        // click-again-to-deselect gesture; a step that did
                        // not move must not trip it.
                        if selected_device_slot(&st) == Some(next) {
                            return false;
                        }
                        set_focused_surface(&window, actions::Surface::Rack);
                        window.invoke_device_selected(next);
                    }
                    "browser.focus" => browser_take_focus(&st, &window),
                    "browser.activate" => {
                        if !browser_activate_focused(&st, &window) {
                            return false;
                        }
                    }
                    "browser.load" => {
                        if !browser_load_focused(&st, &window) {
                            return false;
                        }
                    }
                    "channel.clone" => window.invoke_edit_command_requested(5, channel),
                    "channel.remove" => window.invoke_edit_command_requested(6, channel),
                    "channel.add" => window.invoke_add_channel_clicked(0),
                    "channel.mute" => window.invoke_channel_muted(channel),
                    "channel.solo" => window.invoke_channel_soloed(channel),
                    // The *track's* pair, which is a different seat and a
                    // different solo from the channel's above. The track is
                    // the one the rack is editing, which is the one a mixer
                    // click put there -- so with a channel open these do
                    // nothing and say so by falling through.
                    "track.solo" | "track.mute" => {
                        if !window.get_editing_bus() {
                            return false;
                        }
                        let bus = window.get_editing_bus_index();
                        if action_id == "track.solo" {
                            window.invoke_bus_solo_toggled(bus);
                        } else {
                            window.invoke_bus_muted(bus);
                        }
                    }
                    // The menu rows' own condition, from the one predicate
                    // `Project::move_track` applies, and the drag's own
                    // callback: one mutation path whichever surface asked.
                    "track.move-left" | "track.move-right" => {
                        if !window.get_editing_bus() {
                            return false;
                        }
                        let bus = window.get_editing_bus_index();
                        let delta = if action_id == "track.move-left" { -1 } else { 1 };
                        let movable = usize::try_from(bus)
                            .is_ok_and(|bus| st.borrow().session.can_move_track(bus, delta));
                        if !movable {
                            return false;
                        }
                        window.invoke_track_reorder_requested(bus, bus + delta as i32);
                    }
                    "pattern.add" => window.invoke_add_pattern_clicked(),
                    "pattern.clone" => window.invoke_pattern_clone_requested(),
                    "pattern.remove" => window.invoke_pattern_remove_requested(),
                    "pattern.clear" => window.invoke_pattern_clear_requested(),
                    // A beat, not a step: `STEPS_PER_BEAT` is what the grid
                    // is drawn in and what a pattern length is chosen in.
                    // The clamp is the field's own range, so a hotkey cannot
                    // reach a length the control could not.
                    "pattern.length-grow" | "pattern.length-shrink" => {
                        let beat = i32::from(mooloop_core::STEPS_PER_BEAT);
                        let by = if action_id == "pattern.length-grow" {
                            beat
                        } else {
                            -beat
                        };
                        let next =
                            (window.get_pattern_length() + by).clamp(1, i32::from(MAX_PATTERN_STEPS));
                        if next == window.get_pattern_length() {
                            return false;
                        }
                        window.set_pattern_length(next);
                        window.invoke_pattern_length_changed(next);
                    }
                    // Guarded by what the matching menu row is enabled by:
                    // keyboard and menu are two surfaces over one action and
                    // must not disagree about where it applies. Select All's
                    // row additionally greys with no notes on screen, which
                    // is a no-op rather than a second condition.
                    "edit.select-all" => {
                        if !window.get_showing_notes() {
                            return false;
                        }
                        window.invoke_select_all_requested();
                    }
                    "edit.delete-note" => {
                        if !window.get_has_note_selection() {
                            return false;
                        }
                        window.invoke_delete_selected_notes_requested();
                    }
                    // Bare digits, and only while the roll is on screen: a
                    // number key means something else on every other page,
                    // and an unconditional binding would be a trap there.
                    "notes.tool-select"
                    | "notes.tool-draw"
                    | "notes.tool-paint"
                    | "notes.tool-slice"
                    | "notes.tool-erase" => {
                        if !window.get_showing_notes() {
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
                        if !window.get_showing_notes() {
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
                    "view.split-toggle" => window.invoke_toggle_split(),
                    "view.zoom-pane" => window.invoke_toggle_zoom_active(),
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
        window.set_preferences_shortcut_super_key_choices(ModelRc::from(Rc::new(VecModel::from(
            actions::super_key_labels()
                .into_iter()
                .map(slint::SharedString::from)
                .collect::<Vec<_>>(),
        ))));
        window.set_preferences_shortcut_super_key(ui_settings.borrow().shortcuts.super_key.index());
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
        // The falloff rows, built from the rate table so the label a user
        // reads is the number the meter runs. Set once: the table is a
        // constant, unlike the gesture and shortcut rows above it.
        window.set_preferences_meter_falloff_options(ModelRc::from(Rc::new(VecModel::from(
            meter::falloff_options()
                .into_iter()
                .map(slint::SharedString::from)
                .collect::<Vec<_>>(),
        ))));
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
                    // Recorded through the same reading the dispatcher uses,
                    // so the chord stored is the chord that will arrive:
                    // with Super acting as Alt, a recorded Super+K is
                    // written down as Alt+K.
                    let super_key = settings.borrow().shortcuts.super_key;
                    let chord = actions::KeyChord::from_event(
                        super_key,
                        ctrl,
                        shift,
                        alt,
                        meta,
                        key.as_str(),
                    );
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
            let st = state.clone();
            let tx = audio_tx.clone();
            let weak = window.as_weak();
            window.on_preferences_opened(move || {
                let Some(window) = weak.upgrade() else { return };
                // A hand-edited theme file, a fresh `wal` run and a desktop
                // that changed its mind since launch all show up here, which
                // is the one moment somebody is looking at the theme list.
                crate::theme::catalog::refresh();
                crate::theme::system::forget();
                let settings = settings.borrow();
                apply_appearance(&window, &settings.appearance);
                sync_preferences_properties(&window, &settings);
                window.set_preferences_midi_learn_binds_port(settings.midi.learn_binds_port);
                window.set_preferences_shortcut_super_key(settings.shortcuts.super_key.index());
                // Built on open rather than kept current: the ports move, the
                // map moves, and nothing outside this page reads either.
                st.borrow().refresh_midi_mappings(&window);
                tx.send(AudioAction::RefreshTargets);
            });
        }
        {
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_preferences_shortcut_super_key_changed(move |index| {
                let Some(window) = weak.upgrade() else { return };
                let chosen = actions::SuperKeyMode::from_index(index);
                let mut settings = settings.borrow_mut();
                // This arrives whenever the property moves, including when
                // this side pushed the stored value into it as the dialog
                // opened. Re-saving the file it was just read from is the
                // kind of write that turns into a lost setting when it
                // fails, so an unchanged value does nothing -- and that is
                // also what makes the write-back below safe to re-enter.
                if chosen == settings.shortcuts.super_key {
                    return;
                }
                let previous = std::mem::replace(&mut settings.shortcuts.super_key, chosen);
                // The table is deliberately not rebuilt: the mode decides
                // which key event makes which chord, not which chord an
                // action holds, so every row on the page stays true.
                let result = settings.save();
                if result.is_err() {
                    settings.shortcuts.super_key = previous;
                }
                let stored = settings.shortcuts.super_key.index();
                // Before the window is touched, because putting a refused
                // choice back moves the property, which calls this again.
                drop(settings);
                if let Err(error) = result {
                    window
                        .set_preferences_error(format!("Could not save settings: {error}").into());
                }
                window.set_preferences_shortcut_super_key(stored);
            });
        }
        {
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_preferences_midi_learn_binds_port_toggled(move |value| {
                let Some(window) = weak.upgrade() else { return };
                let mut settings = settings.borrow_mut();
                let previous = std::mem::replace(&mut settings.midi.learn_binds_port, value);
                if let Err(error) = settings.save() {
                    settings.midi.learn_binds_port = previous;
                    window
                        .set_preferences_error(format!("Could not save settings: {error}").into());
                }
                window.set_preferences_midi_learn_binds_port(settings.midi.learn_binds_port);
            });
        }
        {
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_preferences_appearance_preview(move || {
                let Some(window) = weak.upgrade() else { return };
                let mut candidate = window_appearance(&window, &settings.borrow().appearance);
                // A colour edited away from the theme it came from is Custom,
                // which is the rule the `scheme` field has always followed --
                // it just has a ramp to compare against now as well as three
                // seeds. The *name* is kept either way, because saving a
                // warmed-up Dracula has to keep Alucard.
                candidate.customized =
                    !candidate.theme.is_empty() && !candidate.seeds_match_theme();
                // A hand-typed hex is invalid for the few keystrokes it takes
                // to finish typing it, so a rejected preview reports the
                // reason and leaves the last good theme on screen.
                match candidate.validated() {
                    Ok(appearance) => {
                        window
                            .set_preferences_appearance_theme(appearance.theme.as_str().into());
                        window.set_preferences_appearance_customized(appearance.customized);
                        window.set_preferences_appearance_ramp(ramp_swatches(&appearance));
                        window.set_preferences_appearance_variant_derived(
                            variant_is_derived(&appearance),
                        );
                        push_appearance_contrast(&window, &appearance);
                        push_appearance_swatches(&window, &appearance);
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
            window.on_preferences_appearance_mode_changed(move |index| {
                let Some(window) = weak.upgrade() else { return };
                let mut candidate = window_appearance(&window, &settings.borrow().appearance);
                candidate.mode = crate::theme::Mode::from_index(index).name().to_owned();
                // Asking the desktop again rather than trusting a cached
                // answer: picking Auto is exactly the moment somebody wants to
                // know what the desktop currently says.
                crate::theme::system::forget();
                candidate.sync_seeds();
                match candidate.validated() {
                    Ok(appearance) => {
                        push_appearance_colors(&window, &appearance);
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
            window.on_preferences_appearance_select_theme(move |name| {
                let Some(window) = weak.upgrade() else { return };
                let mut candidate = window_appearance(&window, &settings.borrow().appearance);
                let Some(theme) = crate::theme::catalog::find(name.as_str()) else {
                    return;
                };
                candidate.apply_theme(&theme);
                // Selecting a theme is a preview like any other: it only
                // reaches settings.toml through Apply or OK.
                match candidate.validated() {
                    Ok(appearance) => {
                        push_appearance_colors(&window, &appearance);
                        push_appearance_scalars(&window, &appearance);
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
            window.on_preferences_appearance_save_theme(move |name| {
                let Some(window) = weak.upgrade() else { return };
                let mut settings = settings.borrow_mut();
                let candidate = window_appearance(&window, &settings.appearance);
                let mut appearance = match candidate.validated() {
                    Ok(appearance) => appearance,
                    Err(error) => {
                        window.set_preferences_error(error.to_string().into());
                        return;
                    }
                };
                if let Err(error) = appearance.save_theme(name.as_str()) {
                    window.set_preferences_error(error.to_string().into());
                    return;
                }
                let previous = std::mem::replace(&mut settings.appearance, appearance);
                if let Err(error) = settings.save() {
                    settings.appearance = previous;
                    window
                        .set_preferences_error(format!("Could not save settings: {error}").into());
                    return;
                }
                window.set_preferences_appearance_theme_name("".into());
                apply_appearance(&window, &settings.appearance);
                sync_preferences_properties(&window, &settings);
            });
        }
        {
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_preferences_appearance_remove_theme(move |name| {
                let Some(window) = weak.upgrade() else { return };
                let mut settings = settings.borrow_mut();
                let mut appearance = settings.appearance.clone();
                if let Err(error) = appearance.remove_theme(name.as_str()) {
                    window.set_preferences_error(error.to_string().into());
                    return;
                }
                let previous = std::mem::replace(&mut settings.appearance, appearance);
                if let Err(error) = settings.save() {
                    settings.appearance = previous;
                    window
                        .set_preferences_error(format!("Could not save settings: {error}").into());
                    return;
                }
                // Removing a theme drops the stored name but keeps the colours
                // on screen, so the list refreshes without the interface
                // flickering through somebody else's palette.
                push_appearance_colors(&window, &settings.appearance);
            });
        }
        {
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_preferences_save(move || {
                let Some(window) = weak.upgrade() else {
                    return false;
                };
                let mut settings = settings.borrow_mut();
                // Every value comes back off the window, which is where the
                // page has been writing it all along -- `window_appearance`
                // already reads motion and metering straight out of their
                // globals for the same reason.
                let candidate = window_appearance(&window, &settings.appearance);
                let appearance = match candidate.validated() {
                    Ok(appearance) => appearance,
                    Err(error) => {
                        window.set_preferences_error(error.to_string().into());
                        return false;
                    }
                };
                apply_appearance(&window, &appearance);
                let previous = std::mem::replace(&mut settings.appearance, appearance);
                let previous_developer_mode = settings.general.developer_mode;
                settings.general.developer_mode = window.get_preferences_developer_mode();
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
                if let Some(&frames) = BUFFER_SIZES.get(index as usize) {
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
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_playback_mode_changed(move |song_mode| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Playback mode", || {
                    let command = st.borrow_mut().session.set_playback_mode(song_mode);
                    if let Some(window) = weak.upgrade() {
                        window.set_song_mode(song_mode);
                    }
                    let _ = tx.send(command);
                    true
                });
            });
        }
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_bpm_changed(move |bpm| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Tempo", || {
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
                    true
                });
            });
        }
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_swing_changed(move |percent| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Swing", || {
                    let mut st = st.borrow_mut();
                    let _ = tx.send(st.session.set_swing(percent));
                    if let Some(window) = weak.upgrade() {
                        st.update_document_title(&window);
                    }
                    true
                });
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
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Add pattern", || {
                    if commands.borrow().project_edit_pending {
                        return false;
                    }
                    let mut st = st.borrow_mut();
                    let Some(pattern) = st.session.add_pattern() else {
                        return false;
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
                    true
                });
            });
        }

        // Pattern renaming. An empty name is legal and falls back to
        // "Pattern N" in the menu.
        {
            let commands = command_state.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_pattern_renamed(move |index, name| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Rename pattern", || {
                    let mut st = st.borrow_mut();
                    if !st.session.rename_pattern(index as usize, &name) {
                        return false;
                    }
                    // **This did not mark the document dirty until 2026-09-13**,
                    // and until the same day it did not need to: the name lived
                    // only in the session and was thrown away by the next save,
                    // so there was nothing for a dirty flag to protect. Persisting
                    // the name is what turned a harmless omission into a rename
                    // that could be lost at quit without being asked about.
                    st.session.mark_dirty();
                    if let Some(window) = weak.upgrade() {
                        st.sync_pattern_menu(&window);
                        st.update_document_title(&window);
                    }
                    true
                });
            });
        }

        // Per-pattern logical length. Channel storage stays at the maximum so
        // shortening and re-extending a pattern does not discard hidden steps.
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_pattern_length_changed(move |length| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Pattern length", || {
                    let mut st = st.borrow_mut();
                    let Some(applied) = st.session.set_pattern_length(length) else {
                        return false;
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
                    true
                });
            });
        }

        // Placement callbacks already carry musical-grid-snapped PPQ ticks.
        // Clip duration follows the referenced pattern's logical length.
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_playlist_placement_added(move |pattern, start_tick| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Add clip", || {
                    let mut st = st.borrow_mut();
                    let Some(placement) = st.session.add_playlist_placement(pattern, start_tick) else {
                        return false;
                    };
                    if let Some(window) = weak.upgrade() {
                        st.sync_playlist(&window);
                    }
                    let _ = tx.send(EngineCommand::SetPlaylistPlacement {
                        pattern: placement.pattern,
                        start_tick: placement.start_tick,
                        on: true,
                    });
                    true
                });
            });
        }
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_playlist_placement_removed(move |pattern, tick| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Remove clip", || {
                    let mut st = st.borrow_mut();
                    let Some(placement) = st.session.remove_playlist_placement(pattern, tick) else {
                        return false;
                    };
                    if let Some(window) = weak.upgrade() {
                        st.sync_playlist(&window);
                    }
                    let _ = tx.send(EngineCommand::SetPlaylistPlacement {
                        pattern: placement.pattern,
                        start_tick: placement.start_tick,
                        on: false,
                    });
                    true
                });
            });
        }

        // Playhead and loop. The playhead is a transport position and not
        // document state, so it goes straight to the engine; the loop is part
        // of the song and goes through the session first.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_playlist_seek(move |tick| {
                let command = st.borrow_mut().session.seek_playlist(tick);
                // The playhead follows the drag rather than waiting for the
                // engine to report back. A stopped transport still emits a
                // position every block, so this only covers the gesture
                // itself -- but the gesture is where the lag would be seen.
                if let Some(window) = weak.upgrade() {
                    window.set_playlist_position_ticks(tick.max(0));
                }
                let _ = tx.send(command);
            });
        }
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_playlist_loop_set(move |from_tick, to_tick| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Loop range", || {
                    let mut st = st.borrow_mut();
                    let Some(command) = st.session.set_loop_range(from_tick, to_tick) else {
                        return false;
                    };
                    if let Some(window) = weak.upgrade() {
                        st.sync_loop_range(&window);
                        st.update_document_title(&window);
                    }
                    let _ = tx.send(command);
                    true
                });
            });
        }
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            // An edge handle, which fires on every pointer move over a new
            // snap unit rather than once on release. `adjust_loop_range`
            // returns `None` for a range it already holds, so the moves that
            // land inside the unit the loop already ends on cost nothing --
            // and the ones that do not are what makes the edge follow the
            // pointer instead of jumping when it is let go.
            window.on_playlist_loop_adjusted(move |from_tick, to_tick| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Loop range", || {
                    let mut st = st.borrow_mut();
                    let Some(command) = st.session.adjust_loop_range(from_tick, to_tick) else {
                        return false;
                    };
                    if let Some(window) = weak.upgrade() {
                        st.sync_loop_range(&window);
                        st.update_document_title(&window);
                    }
                    let _ = tx.send(command);
                    true
                });
            });
        }
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_playlist_loop_enabled_changed(move |enabled| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Loop", || {
                    let mut st = st.borrow_mut();
                    let Some(command) = st.session.set_loop_enabled(enabled) else {
                        // Put the toggle back: it drives the callback rather than
                        // being driven by it, so a refused change would otherwise
                        // leave the button lit over a loop that is not running.
                        if let Some(window) = weak.upgrade() {
                            st.sync_loop_range(&window);
                        }
                        return false;
                    };
                    if let Some(window) = weak.upgrade() {
                        st.sync_loop_range(&window);
                        st.update_document_title(&window);
                    }
                    let _ = tx.send(command);
                    true
                });
            });
        }
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_playlist_loop_cleared(move || {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Clear loop", || {
                    let mut st = st.borrow_mut();
                    let Some(command) = st.session.clear_loop_range() else {
                        return false;
                    };
                    if let Some(window) = weak.upgrade() {
                        st.sync_loop_range(&window);
                        st.update_document_title(&window);
                    }
                    let _ = tx.send(command);
                    true
                });
            });
        }

        // Rack cells summarize all notes starting in their sixteenth. A click
        // adds an anchor note to an empty cell or clears every substep in a
        // populated one; right-click always clears.
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_step_clicked(move |channel, step| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Step", || {
                    let mut st = st.borrow_mut();
                    let Some(edit) = st.session.toggle_step(channel, step) else {
                        report_note_refusal(&mut st.session, &window);
                        return false;
                    };
                    st.apply_step_edit(channel, edit, &weak, &tx);
                    true
                });
            });
        }
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_step_removed(move |channel, step| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Clear step", || {
                    let mut st = st.borrow_mut();
                    let Some(edit) = st.session.clear_step(channel, step) else {
                        return false;
                    };
                    st.apply_step_edit(channel, edit, &weak, &tx);
                    true
                });
            });
        }
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_step_velocity_edited(move |channel, step, value| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Step velocity", || {
                    let mut st = st.borrow_mut();
                    let Some(edit) = st.session.set_step_velocity(channel, step, value) else {
                        report_note_refusal(&mut st.session, &window);
                        return false;
                    };
                    st.apply_step_edit(channel, edit, &weak, &tx);
                    true
                });
            });
        }

        // Paint-drag step editing: idempotent per call so a mouse drag can
        // call this repeatedly over the same cell without toggling it.
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_step_painted(move |channel, step, on| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Paint steps", || {
                    let mut st = st.borrow_mut();
                    let Some(edit) = st.session.paint_step(channel, step, on) else {
                        report_note_refusal(&mut st.session, &window);
                        return false;
                    };
                    st.apply_step_edit(channel, edit, &weak, &tx);
                    true
                });
            });
        }

        // Slice a sixteenth into `divisions` evenly spaced notes.
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_step_sliced(move |channel, step, divisions| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Slice step", || {
                    let mut st = st.borrow_mut();
                    let Some(edit) = st.session.slice_step(channel, step, divisions) else {
                        report_note_refusal(&mut st.session, &window);
                        return false;
                    };
                    st.apply_step_edit(channel, edit, &weak, &tx);
                    true
                });
            });
        }

        // Drag-resize every note starting in a sixteenth. Called repeatedly
        // during a drag, so no-op durations are skipped and only the cells
        // the note used to (or now does) span are refreshed.
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_step_length_dragged(move |channel, step, length_in_steps| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Note length", || {
                    let mut st = st.borrow_mut();
                    let Some(edit) = st.session.drag_step_length(channel, step, length_in_steps) else {
                        return false;
                    };
                    st.apply_step_edit(channel, edit, &weak, &tx);
                    true
                });
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
                    if !report_note_refusal(&mut st.session, &window) {
                        window.set_status_message("Nothing fits at the paste position".into());
                    }
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
                let Some((id, edit)) = st
                    .session
                    .create_roll_note(start_tick, midi_note, duration_ticks)
                else {
                    report_note_refusal(&mut st.session, &window);
                    // No note has id 0, so the grid's drag finds nothing.
                    return 0;
                };
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
                    report_note_refusal(&mut st.session, &window);
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
                    report_note_refusal(&mut st.session, &window);
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
                let Some(edit) = st
                    .session
                    .paint_roll_note(start_tick, midi_note, duration_ticks)
                else {
                    report_note_refusal(&mut st.session, &window);
                    return;
                };
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
            window.on_automation_lane_opened(move |index| {
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
            // Inside the lane's press-to-release gesture, so a point created
            // and then dragged is one entry named for the creation.
            window.on_automation_point_created(move |tick, value| {
                let Some(window) = weak.upgrade() else {
                    return -1;
                };
                let mut created = -1;
                with_gesture_history(
                    &history_state,
                    &commands,
                    &window,
                    "Automation point added",
                    || {
                        let mut st = st.borrow_mut();
                        let Some((id, command)) = st.session.create_automation_point(tick, value)
                        else {
                            return false;
                        };
                        let _ = tx.send(command);
                        st.refresh_automation_points(&window);
                        created = id as i32;
                        true
                    },
                );
                created
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let history_state = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            // Every pointer frame of a drag lands here. The lane opens a
            // gesture on the press, so the whole drag is one undo entry
            // rather than one per frame -- which also kept a single drag from
            // spending most of a heavy song's history.
            window.on_automation_point_moved(move |id, tick, value| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(
                    &history_state,
                    &commands,
                    &window,
                    "Automation point moved",
                    || {
                        let mut st = st.borrow_mut();
                        let Some(command) = st
                            .session
                            .move_automation_point(id.max(0) as PointId, tick, value)
                        else {
                            return false;
                        };
                        let _ = tx.send(command);
                        st.refresh_automation_points(&window);
                        true
                    },
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
                let commands = command_state.clone();
                let weak = window.as_weak();
                window.$callback(move |value| {
                    let Some(window) = weak.upgrade() else { return };
                    with_gesture_history(&st, &commands, &window, "Note", || {
                        let mut st = st.borrow_mut();
                        let pattern = st.session.current_pattern;
                        let channel = st.session.selected;
                        let Some(id) = st.session.selected_note_id else {
                            return false;
                        };
                        let length_ticks =
                            st.session.pattern_lengths[pattern] as u32 * TICKS_PER_STEP;
                        let Some(note) = st.session.channels[channel].notes[pattern]
                            .iter_mut()
                            .find(|note| note.id == id)
                        else {
                            return false;
                        };
                        note.$field = $value(value, &mut *note, length_ticks);
                        let edited = *note;
                        st.refresh_rack_cell(channel, (edited.start_tick / TICKS_PER_STEP) as usize);
                        st.refresh_note_editor(&window);
                        let _ = tx.send(EngineCommand::UpsertNote {
                            pattern: pattern as u8,
                            channel: channel as u8,
                            note: edited,
                        });
                        true
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
                    // Picking a channel is what aims the contextual chords
                    // back at the channel list. There is no Escape-to-
                    // nowhere: the fallback surface is one a user reaches by
                    // doing the ordinary thing.
                    set_focused_surface(&w, actions::Surface::Channels);
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
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_channel_muted(move |ch| {
                let Some(window) = weak.upgrade() else { return };
                with_project_history(&st, &commands, &window, "Mute Channel", || {
                    let mut st = st.borrow_mut();
                    let Some(command) = st.session.toggle_channel_mute(ch) else {
                        return false;
                    };
                    st.sync_row_flags();
                    let _ = tx.send(command);
                    true
                });
            });
        }

        // Solo, in place. The button says which channel is soloed; what that
        // silences is derived by the pump's `sync_channel_solo`, because it
        // is a property of the whole bank -- so this sends no command, and
        // every *other* row's name dims or undims, which is why the flags for
        // all of them are pushed rather than one row's.
        {
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_channel_soloed(move |ch| {
                let Some(window) = weak.upgrade() else { return };
                with_project_history(&st, &commands, &window, "Solo Channel", || {
                    let mut st = st.borrow_mut();
                    if !st.session.toggle_channel_solo(ch) {
                        return false;
                    }
                    st.sync_row_flags();
                    st.update_document_title(&window);
                    true
                });
            });
        }

        // Channel output level and pan.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            let commands = command_state.clone();
            window.on_channel_volume_changed(move |ch, volume| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(
                    &st,
                    &commands,
                    &window,
                    "Channel Volume",
                    || {
                        let mut st = st.borrow_mut();
                        let Some(command) = st.session.set_channel_volume(ch, volume) else {
                            return false;
                        };
                        st.sync_row_flags();
                        // The source device's output-trim knob is the same
                        // parameter; restate it or its readout freezes at
                        // whatever the channel had when it was selected.
                        if ch as usize == st.session.selected {
                            window.set_selected_channel_volume_db(linear_to_db(
                                st.session.channels[st.session.selected].volume,
                            ));
                        }
                        let _ = tx.send(command);
                        true
                    },
                );
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_channel_pan_changed(move |ch, pan| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Channel Pan", || {
                    let mut st = st.borrow_mut();
                    let Some(command) = st.session.set_channel_pan(ch, pan) else {
                        return false;
                    };
                    st.sync_row_flags();
                    let _ = tx.send(command);
                    true
                });
            });
        }

        // Add, replace, or remove channel sources.
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let reset_tx = sample_reset_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_channel_source_changed(move |value| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Change device", || {
                    let source = device_kind_from_int(value);
                    let channel = {
                        let mut guard = st.borrow_mut();
                        let Some(channel) = guard.session.change_selected_source(source) else {
                            return false;
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
                    true
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
                // The window is upgraded first because the `before` snapshot
                // needs it, which is the whole reason this used to record
                // nothing.
                let Some(window) = weak.upgrade() else { return };
                if commands.borrow().project_edit_pending {
                    return;
                }
                add_channel_with_history(
                    &st,
                    &commands,
                    &window,
                    &add_channel_tx,
                    &reset_tx,
                    device_kind_from_int(value),
                );
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
        {
            let st = state.clone();
            let commands = command_state.clone();
            let tx = project_edit_tx.clone();
            let weak = window.as_weak();
            window.on_channel_reorder_requested(move |from, to| {
                let Some(window) = weak.upgrade() else { return };
                if commands.borrow().project_edit_pending {
                    return;
                }
                let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else {
                    return;
                };
                if queue_channel_move(&tx, &st, &window, from, to, "Channel moved") {
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
                    //
                    // A stream still collecting -- a knob on a desk that has
                    // not settled, a take still running -- is closed first,
                    // at the document as it is now, so Ctrl+Z undoes *it*
                    // rather than reaching past it into the entry below.
                    0 => {
                        close_edit_stream(&st, &commands, &window);
                        let entry = commands.borrow().history.undo_target().cloned();
                        if let Some(entry) = entry {
                            if queue_history_target(&tx, entry, HistoryMove::Undo) {
                                commands.borrow_mut().project_edit_pending = true;
                                sync_command_availability(&window, &commands.borrow());
                            }
                        }
                    }
                    1 => {
                        close_edit_stream(&st, &commands, &window);
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
            let commands = command_state.clone();
            window.on_bus_muted(move |bus| {
                let Some(window) = weak.upgrade() else { return };
                with_project_history(&st, &commands, &window, "Mute Bus", || {
                    let mut guard = st.borrow_mut();
                    let Some(command) = guard.session.toggle_bus_mute(bus) else {
                        return false;
                    };
                    guard.sync_mixer_strip(bus as usize);
                    guard.sync_bus_editor(&window);
                    let _ = tx.send(command);
                    true
                });
            });
        }

        {
            let telemetry2 = telemetry_tx.clone();
            let st2 = state.clone();
            let telemetry = telemetry_tx.clone();
            let st = state.clone();
            window.on_preamp_display_changed(move |slot, enabled| {
                let mut st = st2.borrow_mut();
                let Some((target, slot)) = st.session.set_preamp_display(slot, enabled) else {
                    return;
                };
                st.refresh_effect_row(slot as usize);
                let _ = telemetry2.send(TelemetryAction::SetEffectSpectrumEnabled {
                    target,
                    slot,
                    enabled,
                });
            });
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
            let commands = command_state.clone();
            window.on_bus_volume_changed(move |bus, volume| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Bus Volume", || {
                    let mut guard = st.borrow_mut();
                    let Some(command) = guard.session.set_bus_volume(bus, volume) else {
                        return false;
                    };
                    guard.sync_mixer_strip(bus as usize);
                    guard.sync_bus_editor(&window);
                    let _ = tx.send(command);
                    true
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            let st = state.clone();
            let commands = command_state.clone();
            window.on_bus_pan_changed(move |bus, pan| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Bus Pan", || {
                    let mut guard = st.borrow_mut();
                    let Some(command) = guard.session.set_bus_pan(bus, pan) else {
                        return false;
                    };
                    guard.sync_mixer_strip(bus as usize);
                    guard.sync_bus_editor(&window);
                    let _ = tx.send(command);
                    true
                });
            });
        }

        {
            // No command sender: a routing edit sends nothing itself now. The
            // schedule travels with the sends that ride on it, and the pump's
            // `sync_track_graph` derives and installs both.
            let weak = window.as_weak();
            let st = state.clone();
            let commands = command_state.clone();
            window.on_bus_output_changed(move |bus, output| {
                let Some(window) = weak.upgrade() else { return };
                with_project_history(&st, &commands, &window, "Bus Output", || {
                let mut guard = st.borrow_mut();
                match guard.session.set_bus_output(bus, output) {
                    Some(Ok(())) => {
                        // Every strip's legal destinations move when an edge does.
                        {
                            let w = &window;
                            guard.sync_mixer(w);
                            // The session marks the edit; the title is
                            // refreshed here because this no longer travels
                            // through the pump's command drain, which is what
                            // used to do it.
                            guard.update_document_title(w);
                        }
                        // The schedule itself is not sent from here any more:
                        // it travels with the sends that ride on it, which
                        // carry compensation rings, so the pump's
                        // `sync_track_graph` derives and installs both.
                    }
                    Some(Err(refused)) => {
                        window.set_status_message(
                            format!(
                                "{} already feeds this bus - routing would loop",
                                refused.feeder
                            )
                            .into(),
                        );
                        return false;
                    }
                    None => return false,
                }
                true
                });
            });
        }

        // Sends. Adding and removing one is a routing change, so it goes
        // through the same door the output picker does and sends no command
        // of its own -- the pump's `sync_track_graph` derives the plan and
        // installs it with the compensation rings a send needs. Level, tap
        // and enable are POD and reach audio directly, so a drag does not
        // rebuild a plan sixty times a second.
        {
            let commands = command_state.clone();
            let weak = window.as_weak();
            let st = state.clone();
            window.on_send_added(move |bus, target| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Add send", || {
                    let mut guard = st.borrow_mut();
                    match guard.session.add_send(bus, target) {
                        Some(Ok(_)) => {
                            if let Some(w) = weak.upgrade() {
                                // `sync_mixer`, not `sync_bus_editor`: the mixer
                                // strip draws `MixerStripRow.sends`, which only
                                // `sync_mixer` writes, so the row the gesture was
                                // made on never appeared and the empty-state text
                                // stayed up. And a send is a graph edge, so every
                                // strip's `allowed` mask moves with it -- the
                                // reason `on_bus_output_changed` already calls
                                // this. `sync_mixer` ends by calling
                                // `sync_bus_editor`, so the rack face is still
                                // covered.
                                guard.sync_mixer(&w);
                                guard.update_document_title(&w);
                            }
                        }
                        Some(Err(refused)) => {
                            if let Some(w) = weak.upgrade() {
                                w.set_status_message(
                                    format!(
                                        "{} already leads back here - the send would loop",
                                        refused.feeder
                                    )
                                    .into(),
                                );
                            }
                        }
                        None => {}
                    }
                    true
                });
            });
        }
        {
            let commands = command_state.clone();
            let weak = window.as_weak();
            let st = state.clone();
            window.on_send_removed(move |bus, send| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Remove send", || {
                    let mut guard = st.borrow_mut();
                    if guard.session.remove_send(bus, send) {
                        if let Some(w) = weak.upgrade() {
                            // As above: the row has to leave the mixer strip too,
                            // and removing an edge reopens destinations for every
                            // other track.
                            guard.sync_mixer(&w);
                            guard.update_document_title(&w);
                        }
                    }
                    true
                });
            });
        }
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            let st = state.clone();
            window.on_send_level_changed(move |bus, send, level| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Send level", || {
                    let mut guard = st.borrow_mut();
                    let Some(command) = guard.session.set_send_level(bus, send, level) else {
                        return false;
                    };
                    if let Some(w) = weak.upgrade() {
                        // The mixer strip's own back face draws these rows too,
                        // and only `sync_mixer_strip` writes them. One strip and
                        // no edge changed, so this rather than a whole
                        // `sync_mixer`.
                        guard.sync_mixer_strip(bus.max(0) as usize);
                        guard.sync_bus_editor(&w);
                    }
                    let _ = tx.send(command);
                    true
                });
            });
        }
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            let st = state.clone();
            window.on_send_tap_picked(move |bus, send, tap| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Send tap", || {
                    let mut guard = st.borrow_mut();
                    let tap = if tap == 1 {
                        SendTap::PreFader
                    } else {
                        SendTap::PostFader
                    };
                    let Some(command) = guard.session.set_send_tap(bus, send, tap) else {
                        return false;
                    };
                    if let Some(w) = weak.upgrade() {
                        // The mixer strip's own back face draws these rows too,
                        // and only `sync_mixer_strip` writes them. One strip and
                        // no edge changed, so this rather than a whole
                        // `sync_mixer`.
                        guard.sync_mixer_strip(bus.max(0) as usize);
                        guard.sync_bus_editor(&w);
                    }
                    let _ = tx.send(command);
                    true
                });
            });
        }
        {
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            let st = state.clone();
            let commands = command_state.clone();
            window.on_send_enable_toggled(move |bus, send| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Send", || {
                    let mut guard = st.borrow_mut();
                    let wanted = guard
                        .session
                        .buses
                        .get(bus.max(0) as usize)
                        .and_then(|setup| setup.sends.get(send.max(0) as usize))
                        .map(|entry| !entry.enabled);
                    let Some(wanted) = wanted else { return false };
                    let Some(command) = guard.session.set_send_enabled(bus, send, wanted) else {
                        return false;
                    };
                    // The mixer strip's own back face draws these rows too,
                    // and only `sync_mixer_strip` writes them. One strip and
                    // no edge changed, so this rather than a whole
                    // `sync_mixer`.
                    guard.sync_mixer_strip(bus.max(0) as usize);
                    guard.sync_bus_editor(&window);
                    let _ = tx.send(command);
                    true
                });
            });
        }

        // Clip latches. The ballistics live inside the metering timer's
        // closure, and a click on an indicator arrives on the UI thread
        // between two of its ticks, so the flag is the handoff: the callback
        // raises it, the next tick lowers it and clears the latch it names.
        // Nothing else can clear one -- see `MeterBallistics::clear_clip`.
        let master_clip_clear = Rc::new(Cell::new(false));
        let bus_clip_clear = Rc::new(RefCell::new(vec![false; MAX_BUSES]));
        {
            let flag = master_clip_clear.clone();
            window.on_master_clip_reset(move || flag.set(true));
        }
        {
            let flags = bus_clip_clear.clone();
            window.on_bus_clip_reset(move |bus| {
                if let Some(flag) = flags.borrow_mut().get_mut(bus.max(0) as usize) {
                    *flag = true;
                }
            });
        }

        {
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            let st = state.clone();
            let commands = command_state.clone();
            window.on_channel_bus_changed(move |channel, bus| {
                let Some(window) = weak.upgrade() else { return };
                with_project_history(&st, &commands, &window, "Channel Output", || {
                    let mut guard = st.borrow_mut();
                    let Some(command) = guard.session.set_channel_bus(channel, bus) else {
                        return false;
                    };
                    guard.sync_row_flags();
                    // Feed counts moved, so the old and new bus both restate.
                    guard.sync_mixer(&window);
                    let _ = tx.send(command);
                    true
                });
            });
        }

        // The analog-sum switch. The switch itself is all that travels; the
        // buffer its encoded output lands in is reconciled by
        // `sync_console_sums` on the next pump tick, which is why this does
        // not have to know which tracks need one.
        {
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            let st = state.clone();
            let commands = command_state.clone();
            window.on_bus_console_toggled(move |bus| {
                let Some(window) = weak.upgrade() else { return };
                with_project_history(&st, &commands, &window, "Console Sum", || {
                    let mut guard = st.borrow_mut();
                    let Some(command) = guard.session.toggle_bus_console(bus) else {
                        return false;
                    };
                    guard.session.dirty = true;
                    guard.sync_mixer(&window);
                    // The bus device face carries the same switch, so it has
                    // to restate it -- the toggle can be thrown from either.
                    guard.sync_bus_editor(&window);
                    guard.update_document_title(&window);
                    let _ = tx.send(command);
                    true
                });
            });
        }

        // Polarity. One switch, drawn twice -- the mixer strip and the
        // track's rack face both carry it -- so both are restated.
        {
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            let st = state.clone();
            let commands = command_state.clone();
            window.on_bus_polarity_toggled(move |bus| {
                let Some(window) = weak.upgrade() else { return };
                with_project_history(&st, &commands, &window, "Polarity", || {
                    let mut guard = st.borrow_mut();
                    let Some(command) = guard.session.toggle_track_polarity(bus) else {
                        return false;
                    };
                    guard.session.dirty = true;
                    guard.sync_mixer(&window);
                    guard.sync_bus_editor(&window);
                    guard.update_document_title(&window);
                    let _ = tx.send(command);
                    true
                });
            });
        }

        // Solo, in place. The button says which track is soloed; what that
        // silences is derived by the pump's `sync_solo`, because it is a
        // property of the whole graph. Both faces carry the button, and
        // every *other* strip's name dims or undims, so the whole model is
        // republished rather than one row.
        {
            let weak = window.as_weak();
            let st = state.clone();
            let commands = command_state.clone();
            window.on_bus_solo_toggled(move |bus| {
                let Some(window) = weak.upgrade() else { return };
                with_project_history(&st, &commands, &window, "Solo", || {
                    let mut guard = st.borrow_mut();
                    if !guard.session.toggle_track_solo(bus) {
                        return false;
                    }
                    guard.sync_mixer(&window);
                    guard.sync_bus_editor(&window);
                    guard.update_document_title(&window);
                    true
                });
            });
        }

        // A point dragged on the EQ's response plot. The face hands over
        // hertz and `StripEqTable::nearest` snaps it to one of the band's
        // positions -- here rather than in the markup, because that search
        // is in log distance and a second copy of it is what would drift.
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            let st = state.clone();
            window.on_bus_strip_point_dragged(move |bus, band, hz, gain_db| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Channel strip", || {
                    let mut guard = st.borrow_mut();
                    let Some(params) = guard.session.strip_params(bus) else {
                        return false;
                    };
                    let band = band.max(0) as usize;
                    let table = mooloop_dsp::strip::strip_voicing(params.voicing).eq;
                    let position = table.nearest(band, hz);
                    let moves = [
                        (strip_band_param(band, STRIP_BAND_FREQ), position as f32),
                        (strip_band_param(band, STRIP_BAND_GAIN), gain_db),
                    ];
                    let mut moved = false;
                    for (param, value) in moves {
                        if let Some(command) = guard.session.set_strip_param(bus, param as i32, value)
                        {
                            let _ = tx.send(command);
                            moved = true;
                        }
                    }
                    if !moved {
                        return false;
                    }
                    guard.session.dirty = true;
                    if let Some(w) = weak.upgrade() {
                        guard.sync_mixer_strip(bus.max(0) as usize);
                        guard.sync_bus_editor(&w);
                        guard.update_document_title(&w);
                    }
                    true
                });
            });
        }

        // The channel strip. One callback for every parameter of every
        // section, because `StripParams::set` is the only thing that knows
        // what an id means -- and the id the face sends is the one
        // `mooloop_core::strip` minted, handed to the markup by
        // `install_strip_spec` rather than spelled there.
        //
        // The strip is drawn in two places at once, so both are restated: a
        // knob turned on the mixer's back face has to move the same knob on
        // the rack's pinned row.
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            let st = state.clone();
            window.on_bus_strip_param(move |bus, param, value| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Channel strip", || {
                    let mut guard = st.borrow_mut();
                    let Some(command) = guard.session.set_strip_param(bus, param, value) else {
                        return false;
                    };
                    guard.session.dirty = true;
                    if let Some(w) = weak.upgrade() {
                        guard.sync_mixer_strip(bus.max(0) as usize);
                        guard.sync_bus_editor(&w);
                        guard.update_document_title(&w);
                    }
                    let _ = tx.send(command);
                    true
                });
            });
        }
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            let st = state.clone();
            window.on_bus_voicing_picked(move |bus, voicing| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Strip voicing", || {
                    let mut guard = st.borrow_mut();
                    let Some(command) =
                        guard
                            .session
                            .set_strip_param(bus, STRIP_VOICING as i32, voicing as f32)
                    else {
                        return false;
                    };
                    guard.session.dirty = true;
                    if let Some(w) = weak.upgrade() {
                        guard.sync_mixer_strip(bus.max(0) as usize);
                        guard.sync_bus_editor(&w);
                        guard.update_document_title(&w);
                    }
                    let _ = tx.send(command);
                    true
                });
            });
        }

        // Track structure. Adding and removing a track reinstalls the
        // document, because the engine materialises a strip per track when a
        // project loads and everything that named a later track has to
        // renumber -- the same path a channel paste takes, and undoable for
        // the same reason. A rename touches neither, so it is a plain session
        // edit like a pattern rename.
        {
            let tx = project_edit_tx.clone();
            let commands = command_state.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_track_added(move || {
                let Some(window) = weak.upgrade() else { return };
                if commands.borrow().project_edit_pending {
                    return;
                }
                if queue_track_add(&tx, &st, &window) {
                    commands.borrow_mut().project_edit_pending = true;
                    sync_command_availability(&window, &commands.borrow());
                }
            });
        }
        // Every track move arrives here: the strip's drag and the Track
        // menu's two rows, which invoke this callback rather than growing a
        // path of their own.
        {
            let tx = project_edit_tx.clone();
            let commands = command_state.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_track_reorder_requested(move |from, to| {
                let Some(window) = weak.upgrade() else { return };
                // A second drop must not land on a document the first one
                // has not finished installing.
                if commands.borrow().project_edit_pending {
                    return;
                }
                let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else {
                    return;
                };
                if queue_track_move(&tx, &st, &window, from, to) {
                    commands.borrow_mut().project_edit_pending = true;
                    sync_command_availability(&window, &commands.borrow());
                }
            });
        }
        {
            let tx = project_edit_tx.clone();
            let commands = command_state.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_track_removed(move |track| {
                let Some(window) = weak.upgrade() else { return };
                if commands.borrow().project_edit_pending {
                    return;
                }
                let Ok(track) = usize::try_from(track) else {
                    return;
                };
                if queue_track_remove(&tx, &st, &window, track) {
                    commands.borrow_mut().project_edit_pending = true;
                    sync_command_availability(&window, &commands.borrow());
                }
            });
        }
        {
            let commands = command_state.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_track_renamed(move |track, name| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Rename track", || {
                    let mut guard = st.borrow_mut();
                    if !guard.session.rename_track(track, &name) {
                        return false;
                    }
                    guard.session.mark_dirty();
                    if let Some(window) = weak.upgrade() {
                        guard.sync_mixer(&window);
                        guard.sync_bus_editor(&window);
                        guard.update_document_title(&window);
                    }
                    true
                });
            });
        }

        // Channel renaming, the same shape as a track rename and for the same
        // reason: the name is not in the render graph, so nothing has to reach
        // the engine and nothing has to reinstall the document.
        {
            let commands = command_state.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_channel_renamed(move |channel, name| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Rename channel", || {
                    let mut guard = st.borrow_mut();
                    if !guard.session.rename_channel(channel, &name) {
                        return false;
                    }
                    guard.session.mark_dirty();
                    if let Some(window) = weak.upgrade() {
                        // The rack plate is the name's home, and the device-chain
                        // header is where it was just typed; both are redrawn
                        // because neither reads the other.
                        guard.sync_row_flags();
                        guard.refresh_editor(&window);
                        guard.update_document_title(&window);
                    }
                    true
                });
            });
        }

        // The channel sidebar's MIDI IN and CH rows. Both hand back a row
        // index and nothing else: `mooloop_core` owns the lists and the
        // mapping, so neither this nor the markup holds a second copy.
        //
        // A change here rebuilds the *whole* routing table rather than
        // patching one entry. The table is one small struct per channel and is
        // rebuilt on a menu pick, not in a loop; a patch would be a second
        // path to the same state, which is what `Session::midi_routing`
        // exists to avoid.
        {
            let commands = command_state.clone();
            let st = state.clone();
            let weak = window.as_weak();
            let tx = cmd_tx.clone();
            window.on_midi_input_picked(move |row| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "MIDI input", || {
                    let mut guard = st.borrow_mut();
                    let channel = guard.session.selected;
                    let ports = guard.midi_ports.clone();
                    let mut input = guard.session.channel_midi_input(channel);
                    input.source = mooloop_core::MidiInputSource::from_row(row.max(0) as usize, &ports);
                    if !guard.session.set_channel_midi_input(channel, input) {
                        return false;
                    }
                    guard.session.mark_dirty();
                    tx.send_routing(guard.session.midi_routing(&ports));
                    if let Some(window) = weak.upgrade() {
                        guard.refresh_editor(&window);
                        guard.update_document_title(&window);
                    }
                    true
                });
            });
        }
        // The AUDIO row, and the sampler's RECORD page (`audio-recording/05`).
        {
            let commands = command_state.clone();
            let st = state.clone();
            let weak = window.as_weak();
            let tx = cmd_tx.clone();
            window.on_audio_input_picked(move |row| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Audio input", || {
                    let mut guard = st.borrow_mut();
                    let channel = guard.session.selected;
                    let rows = guard.session.audio_source_rows(guard.audio_input_label.as_deref());
                    let source = mooloop_core::AudioInputPicker::new(&rows).pick(row.max(0) as usize);
                    if guard.session.set_channel_audio_input(channel, source) {
                        guard.session.mark_dirty();
                        tx.send_audio_input_routing(guard.session.audio_input_taps());
                    }
                    // Republished whatever happened: the menu moved its own
                    // highlight to the row that was clicked.
                    if let Some(window) = weak.upgrade() {
                        guard.refresh_editor(&window);
                        guard.update_document_title(&window);
                    }
                    true
                });
            });
        }
        // Monitoring the hardware input. Not an edit: it is performance
        // state, and a song does not reopen monitoring.
        {
            let st = state.clone();
            let weak = window.as_weak();
            let tx = cmd_tx.clone();
            window.on_audio_monitor_toggled(move |on| {
                let mut guard = st.borrow_mut();
                let channel = guard.session.selected;
                if let Some(command) = guard.session.set_input_monitor(channel, on) {
                    let _ = tx.send(command);
                }
                if let Some(window) = weak.upgrade() {
                    window.set_audio_monitor(guard.session.is_monitoring(channel));
                }
            });
        }
        {
            let commands = command_state.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_sampler_record_clip_changed(move |on| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Record clip", || {
                    let mut guard = st.borrow_mut();
                    let channel = guard.session.selected;
                    if guard.session.set_record_clip(channel, on) {
                        guard.session.mark_dirty();
                    }
                    // Written back rather than left to the toggle's own state: the
                    // page is rebuilt whenever it is switched to, and it reads the
                    // window's copy, which a click sets but nothing else does.
                    if let Some(window) = weak.upgrade() {
                        window.set_sampler_record_clip(
                            guard.session.channels.get(channel).is_some_and(|c| c.record.clip),
                        );
                        guard.update_document_title(&window);
                    }
                    true
                });
            });
        }
        {
            let commands = command_state.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_sampler_record_bars_changed(move |bars| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Record length", || {
                    let mut guard = st.borrow_mut();
                    let channel = guard.session.selected;
                    if guard.session.set_record_bars(channel, bars) {
                        guard.session.mark_dirty();
                    }
                    if let Some(window) = weak.upgrade() {
                        window.set_sampler_record_bars(i32::from(
                            guard.session.channels.get(channel).map_or(1, |c| c.record.bars),
                        ));
                        guard.update_document_title(&window);
                    }
                    true
                });
            });
        }
        {
            let st = state.clone();
            let weak = window.as_weak();
            let tx = cmd_tx.clone();
            let structural = structural_tx.clone();
            window.on_sampler_record_clicked(move || {
                use mooloop_session::take::RecordPress;
                let Some(window) = weak.upgrade() else {
                    return;
                };
                let mut guard = st.borrow_mut();
                let seat = guard.session.selected;
                let live = guard
                    .session
                    .channels
                    .get(seat)
                    .and_then(|channel| guard.takes.view(channel.id))
                    .is_some_and(|view| view.is_live());
                let input_label = guard.audio_input_label.clone();
                match guard.session.record_press(seat, live, input_label.as_deref()) {
                    Some(RecordPress::Stop { seat }) => {
                        let _ = tx.send(EngineCommand::StopTake { channel: seat });
                    }
                    Some(RecordPress::NoInput) => {
                        window.set_status_message(
                            "Pick an AUDIO input in the channel sidebar to record".into(),
                        );
                    }
                    // Two causes, two messages (open question 10): one sends the
                    // user back to the AUDIO row, the other to their audio
                    // hardware, and a shared wording would send half of them
                    // to the wrong one. Neither is `NoInput`'s "pick an
                    // input", which reads as the app not knowing what it is
                    // already showing.
                    Some(RecordPress::SourceGone) => {
                        window.set_status_message(
                            "This channel records a source that has been deleted; \
                             pick another in the AUDIO row"
                                .into(),
                        );
                    }
                    Some(RecordPress::NoInputDevice) => {
                        window.set_status_message(
                            "There is no audio input to record; check the input device in \
                             Preferences > Audio"
                                .into(),
                        );
                    }
                    Some(RecordPress::Arm { channel, seat, name, clip_ticks, from_input }) => {
                        let sample_rate = guard.audio_sample_rate;
                        let delay = if from_input { guard.input_latency_frames } else { 0 };
                        match guard.takes.arm(channel, seat, &name, clip_ticks, sample_rate, delay) {
                            Ok(command) => {
                                // The routing first, so the take reads the
                                // source this channel names now; then the
                                // transport, because a take waits for the next
                                // bar of a running one (decision 9); then the
                                // take. One ordered queue carries all three.
                                tx.send_audio_input_routing(guard.session.audio_input_taps());
                                if !window.get_playing() {
                                    let _ = tx.send(EngineCommand::Play);
                                }
                                if !structural.send(command) {
                                    window.set_status_message(
                                        "The engine did not take the recording".into(),
                                    );
                                }
                            }
                            Err(error) => {
                                log_error!("ui", "could not arm a take: {error}");
                                window.set_status_message(
                                    format!("Could not start recording: {error}").into(),
                                );
                            }
                        }
                    }
                    None => {}
                }
            });
        }
        {
            let commands = command_state.clone();
            let st = state.clone();
            let weak = window.as_weak();
            let tx = cmd_tx.clone();
            window.on_midi_channel_picked(move |row| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "MIDI channel", || {
                    let mut guard = st.borrow_mut();
                    let channel = guard.session.selected;
                    let ports = guard.midi_ports.clone();
                    let mut input = guard.session.channel_midi_input(channel);
                    input.channel = mooloop_core::MidiChannelFilter::from_row(row.max(0) as usize);
                    if !guard.session.set_channel_midi_input(channel, input) {
                        return false;
                    }
                    guard.session.mark_dirty();
                    tx.send_routing(guard.session.midi_routing(&ports));
                    if let Some(window) = weak.upgrade() {
                        guard.refresh_editor(&window);
                        guard.update_document_title(&window);
                    }
                    true
                });
            });
        }

        // Record arm. Not an edit: a song does not reopen armed, so arming it
        // must not make an untouched document look unsaved -- the same rule
        // the transport commands already follow. That rule is
        // `EngineCommand::edits_document`, and it is the only copy of it:
        // this comment used to state it beside a list, a crate away, that had
        // never heard of `SetRecordArmed` (`reports/fable-2026-09-21.md`,
        // finding 6).
        {
            let st = state.clone();
            let weak = window.as_weak();
            let tx = cmd_tx.clone();
            window.on_record_armed_toggled(move || {
                let mut guard = st.borrow_mut();
                let armed = !guard.session.record_armed();
                if let Some(command) = guard.session.set_record_armed(armed) {
                    let _ = tx.send(command);
                }
                if let Some(window) = weak.upgrade() {
                    window.set_record_armed(armed);
                }
            });
        }
        // The value gesture, wired once for every control in every face.
        //
        // Beside the MIDI learn arm below on purpose: `ControlAssign` and
        // `Gesture` are the same idea pointing in opposite directions.
        // `ControlAssign.midi-learn` is one fact every parameter control
        // *reads*; `Gesture.begin`/`end` is one fact every parameter control
        // *says*. Both are globals so that no face declares, forwards, or
        // knows about them — which is the difference between this and a
        // callback pair threaded through a hundred and ten `main.slint`
        // wiring sites.
        {
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.global::<Gesture>().on_begin(move || {
                let Some(window) = weak.upgrade() else { return };
                gesture_opened(&st, &commands, &window);
            });
        }
        {
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.global::<Gesture>().on_end(move || {
                let Some(window) = weak.upgrade() else { return };
                gesture_closed(&st, &commands, &window);
            });
        }

        // MIDI Learn: the arm, and the four things the mapping editor can do
        // to a row. Every one of them ends by republishing the page, because
        // removing a binding renumbers the ones after it and a stale index is
        // a remove button that deletes somebody else's mapping.
        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_midi_learn_toggled(move || {
                let Some(window) = weak.upgrade() else { return };
                let mut state = st.borrow_mut();
                let armed = !state.midi_learn_armed;
                state.midi_learn_armed = armed;
                if !armed {
                    // Turning the arm off abandons a gesture that was waiting
                    // for a control. Leaving it armed would mean the next
                    // knob touched on the desk bound itself to a parameter
                    // nobody could see had been chosen.
                    state.session.cancel_control_learn();
                }
                set_midi_learn_armed(&window, armed);
                window.set_status_message(
                    if armed {
                        "MIDI Learn: press a control, then move the knob to map to it"
                    } else {
                        ""
                    }
                    .into(),
                );
                state.refresh_midi_mappings(&window);
            });
        }
        {
            let commands = command_state.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_midi_binding_removed(move |index| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Remove MIDI binding", || {
                    let (Some(window), Ok(index)) = (weak.upgrade(), usize::try_from(index)) else {
                        return false;
                    };
                    let mut state = st.borrow_mut();
                    let ports = state.midi_ports.clone();
                    if state.session.remove_control_binding(index, &ports) {
                        state.session.mark_dirty();
                        state.refresh_midi_mappings(&window);
                        state.update_document_title(&window);
                    }
                    true
                });
            });
        }
        {
            let commands = command_state.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_midi_binding_takeover_toggled(move |index| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "MIDI takeover", || {
                    let (Some(window), Ok(index)) = (weak.upgrade(), usize::try_from(index)) else {
                        return false;
                    };
                    let mut state = st.borrow_mut();
                    let jump = state
                        .session
                        .control_map
                        .bindings
                        .get(index)
                        .and_then(|binding| binding.mode.takeover())
                        == Some(Takeover::Jump);
                    let next = if jump { Takeover::Pickup } else { Takeover::Jump };
                    if state.session.set_control_binding_takeover(index, next) {
                        state.session.mark_dirty();
                        state.refresh_midi_mappings(&window);
                        state.update_document_title(&window);
                    }
                    true
                });
            });
        }
        {
            let commands = command_state.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_midi_binding_inverted_toggled(move |index| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "MIDI invert", || {
                    let (Some(window), Ok(index)) = (weak.upgrade(), usize::try_from(index)) else {
                        return false;
                    };
                    let mut state = st.borrow_mut();
                    let inverted = state
                        .session
                        .control_map
                        .bindings
                        .get(index)
                        .is_some_and(|binding| binding.inverted());
                    if state.session.set_control_binding_inverted(index, !inverted) {
                        state.session.mark_dirty();
                        state.refresh_midi_mappings(&window);
                        state.update_document_title(&window);
                    }
                    true
                });
            });
        }
        {
            let st = state.clone();
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_midi_binding_relearn(move |index| {
                let (Some(window), Ok(index)) = (weak.upgrade(), usize::try_from(index)) else {
                    return;
                };
                let mut state = st.borrow_mut();
                // Relearn is the arm and the target at once: there is no
                // control on screen to press, because the row already says
                // which parameter it means. The session remembers the row, so
                // the control that answers replaces it rather than joining it.
                let binds_port = settings.borrow().midi.learn_binds_port;
                let Some(target) = state.session.begin_control_relearn(index, binds_port) else {
                    return;
                };
                state.announce_control_learn(&window, target);
            });
        }
        {
            let st = state.clone();
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_midi_transport_learn(move |index| {
                let (Some(window), Ok(index)) = (weak.upgrade(), usize::try_from(index)) else {
                    return;
                };
                let Some(gesture) = TransportControl::ALL.get(index).copied() else {
                    return;
                };
                let mut state = st.borrow_mut();
                let target = ControlTarget::Transport(gesture);
                // The button is a toggle: pressing it while it is listening
                // stops waiting, which is the only way out of a gesture whose
                // controller turns out not to be plugged in.
                if state.session.control_learn_target() == Some(target) {
                    state.session.cancel_control_learn();
                    window.set_status_message("".into());
                    state.refresh_midi_mappings(&window);
                } else {
                    let binds_port = settings.borrow().midi.learn_binds_port;
                    state.begin_control_learn(&window, binds_port, target);
                }
            });
        }

        // The channel sidebar's colour swatches and its hex field, which are
        // one gesture as far as this is concerned: both hand over the string
        // the project should store, and "" means no colour at all.
        //
        // **An unparseable hex is ignored rather than corrected.** The field
        // reports every keystroke, so a half-typed `#84CC1` arrives here on
        // the way to a real colour; treating it as an error would fight the
        // user typing, and treating it as "clear the colour" would lose the
        // colour they are in the middle of replacing.
        {
            let commands = command_state.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_channel_color_chosen(move |hex| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Channel colour", || {
                    let mut guard = st.borrow_mut();
                    let color = if hex.trim().is_empty() {
                        None
                    } else {
                        match mooloop_core::ProjectColor::from_hex(hex.trim()) {
                            Some(color) => Some(color),
                            None => return false,
                        }
                    };
                    let channel = guard.session.selected as i32;
                    if !guard.session.set_channel_color(channel, color) {
                        return false;
                    }
                    guard.session.mark_dirty();
                    if let Some(window) = weak.upgrade() {
                        // The same pair the rename handler refreshes, and for the
                        // same reason: the sidebar and the rack row each read the
                        // channel rather than each other.
                        guard.sync_row_flags();
                        guard.refresh_editor(&window);
                        guard.update_document_title(&window);
                    }
                    true
                });
            });
        }

        // A track's colour. The same gesture as a channel's, one target over,
        // with one extra consequence: a channel routed to this track wears
        // its colour as a wash, so every rack plate has to be redrawn rather
        // than just the strip that was recoloured.
        {
            let commands = command_state.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_track_color_chosen(move |track, hex| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Track colour", || {
                    let mut guard = st.borrow_mut();
                    let color = if hex.trim().is_empty() {
                        None
                    } else {
                        match mooloop_core::ProjectColor::from_hex(hex.trim()) {
                            Some(color) => Some(color),
                            None => return false,
                        }
                    };
                    if !guard.session.set_track_color(track, color) {
                        return false;
                    }
                    if let Some(window) = weak.upgrade() {
                        guard.sync_row_flags();
                        guard.sync_mixer(&window);
                        guard.update_document_title(&window);
                    }
                    true
                });
            });
        }

        // A pattern's colour, which is the channel handler one index over: the
        // pattern is named rather than assumed, the way `pattern-renamed`
        // takes one, because the toolbar can outlive the selection it was
        // drawn for.
        {
            let commands = command_state.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_pattern_color_chosen(move |pattern, hex| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Pattern colour", || {
                    let mut guard = st.borrow_mut();
                    let color = if hex.trim().is_empty() {
                        None
                    } else {
                        match mooloop_core::ProjectColor::from_hex(hex.trim()) {
                            Some(color) => Some(color),
                            None => return false,
                        }
                    };
                    let Ok(pattern) = usize::try_from(pattern) else {
                        return false;
                    };
                    if !guard.session.set_pattern_color(pattern, color) {
                        return false;
                    }
                    guard.session.mark_dirty();
                    if let Some(window) = weak.upgrade() {
                        guard.sync_pattern_menu(&window);
                        guard.update_document_title(&window);
                    }
                    true
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
                st.borrow_mut().begin_gesture(&window);
            });
        }
        {
            let st = state.clone();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_modulation_param_changed(move |slot, id, value| {
                let Some(window) = weak.upgrade() else { return };
                // A knob gesture owns one undo entry, recorded on release;
                // outside one, every change is its own. This handler used to
                // spell that rule out; it is the general one now.
                with_gesture_history(&st, &commands, &window, "Modulator edited", || {
                    let mut state = st.borrow_mut();
                    let Some(command) = state.session.set_modulator_param(slot, id, value) else {
                        return false;
                    };
                    state.send_modulation(&window, &tx, command);
                    true
                });
            });
        }
        {
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_modulation_param_edit_finished(move || {
                let Some(window) = weak.upgrade() else { return };
                let before = st.borrow_mut().session.finish_gesture();
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
            let commands = command_state.clone();
            let st = state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_modulation_envelope_input_channel_changed(move |slot, channel| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Envelope input", || {                    let mut state = st.borrow_mut();
                    // The gate is a jack rather than a descriptor id, so there is no
                    // parameter to name: the module travels entire.
                    let Some(command) = state.session.set_envelope_input_channel(slot, channel) else {
                        return false;
                    };
                    state.send_modulation(&window, &tx, command);
                    true
                });
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
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_source_modulation_edit_started(move |param| {
                let (Some(window), Ok(param)) = (weak.upgrade(), u32::try_from(param)) else {
                    return;
                };
                let mut state = st.borrow_mut();
                // The press that starts a modulation edit is the same press
                // that names a control for MIDI learn, so which of the two it
                // is comes from this side's arm rather than from the face.
                let address = ParamAddr {
                    scope: EffectTarget::Channel(state.session.selected as u8),
                    owner: ParamOwner::Source,
                    param,
                };
                let binds_port = settings.borrow().midi.learn_binds_port;
                if state.learn_param_if_armed(&window, binds_port, address) {
                    return;
                }
                state.begin_gesture(&window);
            });
        }
        {
            let commands = command_state.clone();
            let st = state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_source_modulation_depth_changed(move |param, depth| {
                let (Some(window), Ok(param)) = (weak.upgrade(), u32::try_from(param)) else {
                    return;
                };
                with_gesture_history(&st, &commands, &window, "Modulation depth", || {
                    let mut state = st.borrow_mut();
                    let destination = ParamAddr {
                        scope: EffectTarget::Channel(state.session.selected as u8),
                        owner: ParamOwner::Source,
                        param,
                    };
                    if !state.set_armed_modulation_depth(&window, &tx, destination, depth) {
                        // A full matrix or invalid target must snap the
                        // transient UI depth back to persisted truth rather
                        // than pretending a parked route was written.
                        state.refresh_modulation(&window);
                        return false;
                    }
                    true
                });
            });
        }
        {
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_source_modulation_edit_finished(move |_| {
                let Some(window) = weak.upgrade() else { return };
                let before = st.borrow_mut().session.finish_gesture();
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
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_strip_modulation_edit_started(move |param| {
                let (Some(window), Ok(param)) = (weak.upgrade(), u32::try_from(param)) else {
                    return;
                };
                let mut state = st.borrow_mut();
                let address =
                    ParamAddr::strip(EffectTarget::Channel(state.session.selected as u8), param);
                let binds_port = settings.borrow().midi.learn_binds_port;
                if state.learn_param_if_armed(&window, binds_port, address) {
                    return;
                }
                state.begin_gesture(&window);
            });
        }
        {
            let commands = command_state.clone();
            let st = state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_strip_modulation_depth_changed(move |param, depth| {
                let (Some(window), Ok(param)) = (weak.upgrade(), u32::try_from(param)) else {
                    return;
                };
                with_gesture_history(&st, &commands, &window, "Modulation depth", || {
                    let mut state = st.borrow_mut();
                    let destination = ParamAddr::strip(
                        EffectTarget::Channel(state.session.selected as u8),
                        param,
                    );
                    if !state.set_armed_modulation_depth(&window, &tx, destination, depth) {
                        state.refresh_modulation(&window);
                        return false;
                    }
                    true
                });
            });
        }
        {
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_strip_modulation_edit_finished(move |_| {
                let Some(window) = weak.upgrade() else { return };
                let before = st.borrow_mut().session.finish_gesture();
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
            let settings = ui_settings.clone();
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
                // The address rather than a bare "is this legal": a learn
                // gesture needs the address, and asking the same question two
                // ways is how the two answers come to disagree.
                let address = match state.session.effect_target {
                    EffectTarget::Channel(channel)
                        if channel as usize == state.session.selected =>
                    {
                        state
                            .session
                            .channels
                            .get(state.session.selected)
                            .and_then(|state| state.effects.get(slot))
                            .and_then(|effect| {
                                let id = effect_face_param_id(effect, param)?;
                                effect.kind().descriptor(id).map(|_| (effect.id, id))
                            })
                            .map(|(device, id)| {
                                ParamAddr::effect(EffectTarget::Channel(channel), device, id)
                            })
                    }
                    _ => None,
                };
                let Some(address) = address else { return };
                let binds_port = settings.borrow().midi.learn_binds_port;
                if state.learn_param_if_armed(&window, binds_port, address) {
                    return;
                }
                state.begin_gesture(&window);
            });
        }
        {
            let commands = command_state.clone();
            let st = state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            window.on_effect_modulation_depth_changed(move |slot, param, depth| {
                let (Some(window), Ok(slot), Ok(param)) =
                    (weak.upgrade(), usize::try_from(slot), u32::try_from(param))
                else {
                    return;
                };
                with_gesture_history(&st, &commands, &window, "Modulation depth", || {
                    let mut state = st.borrow_mut();
                    let destination = match state.session.effect_target {
                    EffectTarget::Channel(channel) if channel as usize == state.session.selected => state
                        .session.channels
                        .get(state.session.selected)
                        .and_then(|channel| channel.effects.get(slot))
                        .and_then(|effect| {
                            let id = effect_face_param_id(effect, param)?;
                            effect.kind().descriptor(id).map(|_| (effect.id, id))
                        })
                        .map(|(device, id)| {
                            ParamAddr::effect(EffectTarget::Channel(channel), device, id)
                        }),
                    _ => None,
                };
                    let Some(destination) = destination else {
                        return false;
                    };
                    if !state.set_armed_modulation_depth(&window, &tx, destination, depth) {
                        state.refresh_modulation(&window);
                        return false;
                    }
                    true
                });
            });
        }
        {
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_effect_modulation_edit_finished(move |_, _| {
                let Some(window) = weak.upgrade() else { return };
                let before = st.borrow_mut().session.finish_gesture();
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
                    st.install_added_effect(&added, window.get_bpm() as f64, sample_rate, &tx, &stx);
                }
                record_project_history(&commands, before, &st, &window, "Effect added");
            });
        }

        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_source_select_toggled(move || {
                let Some(window) = weak.upgrade() else { return };
                let mut st = st.borrow_mut();
                // Clicking the selected generator again clears it, the same
                // way clicking a selected effect row does.
                let want = !st.session.source_is_selected();
                let selected = st.session.select_source(want);
                st.sync_effects();
                window.set_source_selected(selected);
                set_focused_surface(&window, actions::Surface::Rack);
                window.set_status_message(if selected {
                    "Instrument selected".into()
                } else {
                    "".into()
                });
            });
        }

        {
            let st = state.clone();
            let weak = window.as_weak();
            window.on_device_selected(move |slot| {
                let Some(window) = weak.upgrade() else { return };
                set_focused_surface(&window, actions::Surface::Rack);
                let mut st = st.borrow_mut();
                let slot = usize::try_from(slot).ok();
                // Clicking the selected device again clears it, so there is a
                // way back to "nothing selected" without a second gesture.
                let next = if slot.is_some() && st.session.selected_device_slot() == slot {
                    None
                } else {
                    slot
                };
                st.session.select_device(next);
                st.sync_effects();
                window.set_source_selected(st.session.source_is_selected());
                window.set_status_message(match next {
                    Some(_) => "Device selected".into(),
                    None => "".into(),
                });
            });
        }
        {
            let st = state.clone();
            let tx = project_edit_tx.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_duplicate_effect_clicked(move |slot| {
                let Some(window) = weak.upgrade() else { return };
                let Ok(slot) = usize::try_from(slot) else { return };
                apply_device_clipboard(&st, &window, &tx, &commands, DeviceClipboardVerb::Duplicate, Some(slot));
            });
        }
        {
            let st = state.clone();
            let tx = project_edit_tx.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_device_clipboard_action(move |verb| {
                let Some(window) = weak.upgrade() else { return };
                let Some(verb) = DeviceClipboardVerb::from_int(verb) else { return };
                apply_device_clipboard(&st, &window, &tx, &commands, verb, None);
            });
        }

        // A container's own `+`. Same mirror as an ordinary insert; only the
        // model verb differs, because an index cannot say "into this box".
        {
            let tx = cmd_tx.clone();
            let stx = structural_tx.clone();
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_add_effect_into_container(move |kind_index, container| {
                let Some(kind) = effect_kind_from_index(kind_index) else {
                    return;
                };
                let Some(window) = weak.upgrade() else { return };
                let Ok(container) = usize::try_from(container) else {
                    return;
                };
                let before = project_snapshot(&st.borrow(), &window);
                {
                    let mut st = st.borrow_mut();
                    let Some(added) = st.session.insert_effect_into_container(kind, container)
                    else {
                        return;
                    };
                    st.sync_effects();
                    st.refresh_automation(&window);
                    st.refresh_modulation(&window);
                    st.install_added_effect(&added, window.get_bpm() as f64, sample_rate, &tx, &stx);
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
                    // Once per removed row, always from `slot`: after each
                    // removal the next row of the run has slid into that
                    // position. More than one row when the device was a
                    // container, because a box goes with its contents.
                    for step in 0..removed.devices.len() {
                        let tail = removed.tail - step;
                        if removed.slot != tail {
                            let _ = tx.send(EngineCommand::MoveEffect {
                                target: removed.target,
                                from: removed.slot as u8,
                                to: tail as u8,
                            });
                        }
                        let _ = stx.send(StructuralCommand::RemoveEffect {
                            target: removed.target,
                            slot: tail as u8,
                        });
                    }
                    st.publish_container_spans(removed.target, &stx);
                }
                record_project_history(&commands, before, &st, &window, "Effect removed");
            });
        }

        // --- Containers: wrap a device in a box, or take the box away ---
        //
        // Wrapping is the gesture that makes containers, because one is far
        // more often made around a device that already exists than inserted
        // empty. It wraps the clicked row's *run*, so wrapping a container
        // puts a box around that whole box; devices join it afterwards by
        // being dragged onto a row already inside it, which the existing
        // reorder already handles -- the model decides what a landing index
        // falls inside, so the drag needed nothing new.
        {
            let tx = cmd_tx.clone();
            let stx = structural_tx.clone();
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_wrap_effect_clicked(move |slot| {
                let Some(window) = weak.upgrade() else { return };
                let Ok(slot) = usize::try_from(slot) else {
                    return;
                };
                let before = project_snapshot(&st.borrow(), &window);
                {
                    let mut st = st.borrow_mut();
                    let run = match st.session.effect_chain() {
                        Some(effects) => mooloop_core::run_of(effects, slot),
                        None => return,
                    };
                    let Some(added) = st.session.wrap_effects_in_container(run) else {
                        return;
                    };
                    st.sync_effects();
                    st.refresh_automation(&window);
                    st.refresh_modulation(&window);
                    // Installed at the tail and moved into place, exactly as
                    // an inserted device is: the rows it now encloses do not
                    // move, so nothing else has to be told they were wrapped.
                    let bpm = window.get_bpm() as f64;
                    let node = build_effect_at_tempo(added.params, sample_rate, bpm);
                    let align = IntegerDelay::new(node.dry_path_latency_frames()).map(Box::new);
                    stx.send(StructuralCommand::InstallEffect {
                        target: added.target,
                        slot: added.tail as u8,
                        kind: added.kind,
                        resource_key: None,
                        node,
                        align,
                        analyzer: Box::new(SpectrumAnalyzer::new()),
                        state: Box::new(EffectSlot::for_device(added.device)),
                    });
                    if added.slot != added.tail {
                        let _ = tx.send(EngineCommand::MoveEffect {
                            target: added.target,
                            from: added.tail as u8,
                            to: added.slot as u8,
                        });
                    }
                    st.publish_container_spans(added.target, &stx);
                }
                record_project_history(&commands, before, &st, &window, "Devices wrapped");
            });
        }

        {
            let tx = cmd_tx.clone();
            let stx = structural_tx.clone();
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_unwrap_effect_clicked(move |slot| {
                let Some(window) = weak.upgrade() else { return };
                let Ok(slot) = usize::try_from(slot) else {
                    return;
                };
                let before = project_snapshot(&st.borrow(), &window);
                {
                    let mut st = st.borrow_mut();
                    let Some(removed) = st.session.unwrap_container_at(slot) else {
                        return;
                    };
                    st.sync_effects();
                    st.refresh_automation(&window);
                    st.refresh_modulation(&window);
                    // One row leaves and its children stay, so the engine
                    // mirror is an ordinary single removal.
                    if removed.slot != removed.tail {
                        let _ = tx.send(EngineCommand::MoveEffect {
                            target: removed.target,
                            from: removed.slot as u8,
                            to: removed.tail as u8,
                        });
                    }
                    stx.send(StructuralCommand::RemoveEffect {
                        target: removed.target,
                        slot: removed.tail as u8,
                    });
                    st.publish_container_spans(removed.target, &stx);
                }
                record_project_history(&commands, before, &st, &window, "Container removed");
            });
        }

        // --- Effect presets: one rack row, replaced in place ---
        //
        // Loaded on this thread and queued as an ordinary project edit,
        // exactly like every other rack mutation. An effect bundle is a few
        // hundred bytes of TOML that references no audio, so the document
        // pipeline's worker thread bought nothing and cost the guarantee
        // that matters here: that the row the click named is still the row
        // the edit lands on.
        {
            let tx = project_edit_tx.clone();
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_effect_preset_selected(move |slot, index| {
                let Some(window) = weak.upgrade() else { return };
                let (Ok(slot), Ok(index)) = (usize::try_from(slot), usize::try_from(index)) else {
                    return;
                };
                // The index names an entry of this row's kind, in the order
                // `effect_presets_of_kind` built the row's menu from.
                let Some(chosen) = ({
                    let st = st.borrow();
                    st.session
                        .effect_chain()
                        .and_then(|chain| chain.get(slot))
                        .map(EffectSlotState::kind)
                        .and_then(|kind| {
                            effect_presets_of_kind(&st.session.effect_presets, kind)
                                .nth(index)
                                .map(|preset| (preset.path.clone(), preset.name.clone()))
                        })
                }) else {
                    return;
                };
                let (path, name) = chosen;
                // A container's rail offers run presets, so what comes back
                // is one row or a whole run. Both replace what is in the
                // slot; a run replaces the box and everything in it.
                let loaded = match mooloop_project::load_bundle(&path) {
                    Ok(report) => match report.document {
                        LoadedDocument::Effect(effect) => Ok(*effect),
                        LoadedDocument::EffectRun(run) => Err(*run),
                        _ => {
                            log_warn!("project", "{} is not an effect preset", path.display());
                            window.set_status_message(
                                "That bundle is not an effect preset".into(),
                            );
                            return;
                        }
                    },
                    Err(error) => {
                        log_warn!("project", "could not open {}: {error}", path.display());
                        window.set_status_message(
                            format!("Could not open this preset: {error}").into(),
                        );
                        return;
                    }
                };

                let before = project_snapshot(&st.borrow(), &window);
                {
                    let mut state = st.borrow_mut();
                    let landed = match &loaded {
                        Ok(effect) => state
                            .session
                            .load_effect_preset(slot, effect, &name)
                            .is_some(),
                        Err(run) => state.session.load_effect_run(slot, run, &name).is_some(),
                    };
                    if !landed {
                        log_warn!(
                            "project",
                            "{name} does not fit slot {slot} on this chain"
                        );
                        drop(state);
                        window.set_status_message(
                            "That preset is for a different kind of device".into(),
                        );
                        return;
                    }
                    state.sync_effects();
                }
                let after = project_snapshot(&st.borrow(), &window);
                if queue_project_edit(&tx, before, after, "Effect preset loaded") {
                    commands.borrow_mut().project_edit_pending = true;
                    sync_command_availability(&window, &commands.borrow());
                }
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
                // The row is named by the device sitting in it, resolved once
                // here, so the dialog cannot land on whatever takes that
                // position while it is open.
                let Some(device) = st
                    .session
                    .effect_chain()
                    .and_then(|chain| chain.get(slot as usize))
                    .map(|effect| effect.id)
                else {
                    return;
                };
                st.session.pending_preset_save = st
                    .session
                    .chain_key(target)
                    .map(|target| PresetSaveTarget::Effect { target, device });
                if let Some(window) = weak.upgrade() {
                    window.set_save_preset_title("Save Effect Preset".into());
                    window.set_save_preset_name("".into());
                    window.set_save_preset_category("".into());
                    window.set_save_preset_open(true);
                }
            });
        }

        {
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_effect_bypass_toggled(move |slot| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Effect bypass", || {
                    let mut st = st.borrow_mut();
                    let Some(command) = st.session.toggle_effect_bypass(slot) else {
                        return false;
                    };
                    st.refresh_effect_row(slot as usize);
                    let _ = tx.send(command);
                    true
                });
            });
        }

        {
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_effect_wet_dry_changed(move |slot, wet_dry| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Wet/dry", || {
                    let mut st = st.borrow_mut();
                    let Some(command) = st.session.set_effect_wet_dry(slot, wet_dry) else {
                        return false;
                    };
                    st.refresh_effect_row(slot as usize);
                    let _ = tx.send(command);
                    true
                });
            });
        }
        {
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_effect_input_trim_changed(move |slot, input_trim_db| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Input trim", || {
                    let mut st = st.borrow_mut();
                    let Some(command) = st.session.set_effect_input_trim(slot, input_trim_db) else {
                        return false;
                    };
                    st.refresh_effect_row(slot as usize);
                    let _ = tx.send(command);
                    true
                });
            });
        }
        {
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_effect_output_trim_changed(move |slot, output_trim_db| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Output trim", || {
                    let mut st = st.borrow_mut();
                    let Some(command) = st.session.set_effect_output_trim(slot, output_trim_db) else {
                        return false;
                    };
                    st.refresh_effect_row(slot as usize);
                    let _ = tx.send(command);
                    true
                });
            });
        }


        // The Buffer face's buttons.
        //
        // Every one of them is a write to a published parameter, so nothing
        // the mouse can reach is unreachable from an automation lane or a
        // modulator. They used to be *macros* over shared parameters -- REV
        // wrote `1 - rate`, STUT overrode `Length` and restored it on release
        // -- which meant each press had to remember what it had borrowed, and
        // two of them down at once lost the record. A gesture owns its own
        // settings now, so a press is one value going high.
        {
            let st = state.clone();
            let tx = cmd_tx.clone();
            // One place that writes a Buffer parameter, so the handlers below
            // cannot each invent their own way of doing it.
            //
            // It does not record, and its two recording callers wrap it. The
            // third caller is the gate, where a press and a release are the
            // same write with different values: recording it would put two
            // undo entries on one button press, and the release restores the
            // resting value anyway, so there is nothing an undo could
            // return. That is also its line on the check's exemption list.
            let write = move |slot: i32, param_index: i32, normalized: f32| -> bool {
                let mut st = st.borrow_mut();
                let EffectParamWrite::Applied { command, .. } =
                    st.session.set_effect_param(slot, param_index, normalized)
                else {
                    return false;
                };
                st.refresh_effect_row(slot as usize);
                if let Some(command) = command {
                    let _ = tx.send(command);
                }
                true
            };

            // These callbacks name controls by stable wire id, while the
            // session's face API takes positions in the descriptor table.
            // Derive the position from the table rather than duplicating it.
            let buffer_param_index = |id| {
                mooloop_core::EffectKind::Buffer
                    .descriptors()
                    .iter()
                    .position(|descriptor| descriptor.id == id)
                    .expect("Buffer face parameter is absent from its descriptor table")
                    as i32
            };

            let w = write.clone();
            let quantize = buffer_param_index(mooloop_core::BUFFER_PARAM_QUANTIZE);
            let st_q = state.clone();
            let commands_q = command_state.clone();
            let weak_q = window.as_weak();
            window.on_effect_buffer_quantize(move |slot, on| {
                let Some(window) = weak_q.upgrade() else { return };
                with_gesture_history(&st_q, &commands_q, &window, "Buffer QUANT", || {
                    w(slot, quantize, if on { 1.0 } else { 0.0 })
                });
            });
            let w = write.clone();
            let freeze = buffer_param_index(mooloop_core::BUFFER_PARAM_FREEZE);
            let st_f = state.clone();
            let commands_f = command_state.clone();
            let weak_f = window.as_weak();
            window.on_effect_buffer_freeze(move |slot, on| {
                let Some(window) = weak_f.upgrade() else { return };
                with_gesture_history(&st_f, &commands_f, &window, "Buffer FREEZE", || {
                    w(slot, freeze, if on { 1.0 } else { 0.0 })
                });
            });
            let w = write.clone();
            window.on_effect_buffer_gate(move |slot, param_index, down| {
                // A gate, so the press and the release are the same write
                // with different values and the device needs no edge
                // detector. Like every other face edit, the markup sends the
                // parameter's position in the descriptor table.
                w(slot, param_index, if down { 1.0 } else { 0.0 });
            });

            let st = state.clone();
            let tx = cmd_tx.clone();
            let weak = window.as_weak();
            let commands = command_state.clone();
            window.on_effect_buffer_history_bars(move |slot, bars| {
                // Not a parameter write: the ring is reallocated for it, off
                // the audio thread, and swapped in at a block boundary. See
                // `Session::set_buffer_bars`.
                let Some(window) = weak.upgrade() else { return };
                let bpm = f64::from(window.get_bpm());
                with_gesture_history(&st, &commands, &window, "Buffer length", || {
                    let mut st = st.borrow_mut();
                    let Some(resize) = st.session.set_buffer_bars(slot, bars.clamp(1, 255) as u8)
                    else {
                        return false;
                    };
                    st.refresh_effect_row(slot as usize);
                    let _ = tx.resize_buffer(resize, bpm);
                    st.update_document_title(&window);
                    true
                });
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            // One callback for every parameter of every effect kind: the
            // rack sends a descriptor index and a normalized position, and
            // the descriptor table converts to the natural units the wire
            // and the DSP use.
            window.on_effect_param_changed(move |slot, param_index, normalized| {
                let Some(window) = weak.upgrade() else { return };
                // The label is the descriptor's own name, resolved where the
                // id is. It arrives with the write rather than being looked
                // up here, because the EQ's face indices are not descriptor
                // positions and a second copy of that mapping in the caller
                // would name the wrong parameter on exactly one effect.
                with_gesture_history_named(&st, &commands, &window, || {
                    let mut st = st.borrow_mut();
                    // Republish on anything that moved, not only on anything
                    // the engine has to hear about. Choosing an EQ band is
                    // the case that separates the two: it emits no command,
                    // and skipping the republish leaves the selector
                    // highlight and all six knobs on the band you just left.
                    let EffectParamWrite::Applied { command, name } =
                        st.session.set_effect_param(slot, param_index, normalized)
                    else {
                        return None;
                    };
                    st.refresh_effect_row(slot as usize);
                    if let Some(command) = command {
                        let _ = tx.send(command);
                    }
                    Some(name)
                });
            });
        }

        {
            let commands = command_state.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_delay_tempo_sync_changed(move |slot, enabled| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Delay sync", || {
                    let mut state = st.borrow_mut();
                    if !state.session.set_delay_tempo_sync(slot, enabled) {
                        return false;
                    }
                    state.refresh_effect_row(slot as usize);
                    if let Some(window) = weak.upgrade() {
                        state.update_document_title(&window);
                    }
                    true
                });
            });
        }

        {
            let commands = command_state.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_delay_time_division_changed(move |slot, division| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Delay division", || {
                    let mut state = st.borrow_mut();
                    if !state.session.set_delay_time_division(slot, division) {
                        return false;
                    }
                    state.refresh_effect_row(slot as usize);
                    if let Some(window) = weak.upgrade() {
                        state.update_document_title(&window);
                    }
                    true
                });
            });
        }

        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_modulation_tempo_sync_changed(move |slot, enabled| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Modulator sync", || {                    let mut state = st.borrow_mut();
                    let bpm = f64::from(window.get_bpm());
                    let Some(command) = state.session.set_modulation_tempo_sync(slot, enabled, bpm)
                    else {
                        return false;
                    };
                    if let Some(command) = command {
                        let _ = tx.send(command);
                    }
                    state.refresh_effect_row(slot as usize);
                    state.update_document_title(&window);
                    true
                });
            });
        }

        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_modulation_rate_division_changed(move |slot, division| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Modulator division", || {                    let mut state = st.borrow_mut();
                    let bpm = f64::from(window.get_bpm());
                    let Some(command) =
                        state.session.set_modulation_rate_division(slot, division, bpm)
                    else {
                        return false;
                    };
                    if let Some(command) = command {
                        let _ = tx.send(command);
                    }
                    state.refresh_effect_row(slot as usize);
                    state.update_document_title(&window);
                    true
                });
            });
        }

        {
            let tx = cmd_tx.clone();
            let stx = structural_tx.clone();
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
                    let Some(moved) = st.session.move_effect_to(from, to) else {
                        return;
                    };
                    st.sync_effects();
                    st.refresh_automation(&window);
                    st.refresh_modulation(&window);
                    // One command per row, because a container takes its run
                    // with it and the engine's chain moves one row at a time.
                    // For a leaf this is the single move it always was.
                    for (from, to) in &moved.moves {
                        let _ = tx.send(EngineCommand::MoveEffect {
                            target: moved.target,
                            from: *from,
                            to: *to,
                        });
                    }
                    // A run that moved may have landed inside a different box,
                    // or taken its own devices out of one.
                    st.publish_container_spans(moved.target, &stx);
                }
                record_project_history(&commands, before, &st, &window, "Effect moved");
            });
        }

        // --- Sampler parameter callbacks (edit the selected channel) ---
        // One label a family rather than one a callback. The plan's rule is
        // that a label comes from somewhere that already exists rather than
        // being invented sixty times, and for a *descriptor-addressed*
        // parameter that place is the descriptor's own name -- which is what
        // `on_effect_param_changed` uses. These write struct fields instead,
        // so there is no descriptor to read and no id to read it by: the
        // honest answer is the family, not a literal per invocation that
        // nothing can keep in step with the face.
        macro_rules! wire_time_param {
            ($on:ident, $field:ident) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                let commands = command_state.clone();
                let weak = window.as_weak();
                window.$on(move |v: f32| {
                    let Some(window) = weak.upgrade() else { return };
                    with_gesture_history(&st, &commands, &window, "Sampler envelope", || {
                        let mut st = st.borrow_mut();
                        let ch = st.session.selected;
                        let Some(channel) = st.session.channels.get_mut(ch) else {
                            return false;
                        };
                        let value = envelope_seconds(v);
                        if channel.params.$field == value {
                            return false;
                        }
                        channel.params.$field = value;
                        let p = channel.params;
                        let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                            channel: ch as u8,
                            params: p,
                        });
                        true
                    });
                });
            }};
        }
        macro_rules! wire_unit_param {
            ($on:ident, $field:ident) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                let commands = command_state.clone();
                let weak = window.as_weak();
                window.$on(move |v: f32| {
                    let Some(window) = weak.upgrade() else { return };
                    with_gesture_history(&st, &commands, &window, "Sampler parameter", || {
                        let mut st = st.borrow_mut();
                        let ch = st.session.selected;
                        let Some(channel) = st.session.channels.get_mut(ch) else {
                            return false;
                        };
                        if channel.params.$field == v {
                            return false;
                        }
                        channel.params.$field = v;
                        let p = channel.params;
                        let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                            channel: ch as u8,
                            params: p,
                        });
                        true
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
                let commands = command_state.clone();
                let window_weak = window.as_weak();
                window.$on(move |v: f32| {
                    let Some(window) = window_weak.upgrade() else {
                        return;
                    };
                    let marker = $marker;
                    // `RefCell` rather than `Cell`: the snap status is a
                    // `String`, which is not `Copy`.
                    let resolved = std::cell::RefCell::new((v, None));
                    with_gesture_history(&st, &commands, &window, "Sample marker", || {
                        let mut st = st.borrow_mut();
                        let ch = st.session.selected;
                        let Some(channel) = st.session.channels.get_mut(ch) else {
                            return false;
                        };
                        let mut value = v;
                        let mut status = None;
                        if window.get_snap_to_zero() {
                            if let Some(sample) = channel.published_sample().cloned() {
                                if let Some((snapped, result)) =
                                    snap_marker(&channel.params, &sample, marker, v)
                                {
                                    value = snapped;
                                    status = Some(snap_status(marker, result));
                                }
                            }
                        }
                        *resolved.borrow_mut() = (value, status);
                        marker.set(&mut channel.params, value);
                        let p = channel.params;
                        let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                            channel: ch as u8,
                            params: p,
                        });
                        true
                    });
                    let (value, status) = resolved.into_inner();
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
            // The pane arrangement outlives both the session and the project:
            // it is how this user works, not what this song is. Fired on
            // discrete changes and at the end of a drag, never per frame, so
            // this is a handful of small writes rather than one per pointer
            // move. A failed save costs the memory of the arrangement and
            // nothing else, so it is not worth interrupting anyone over.
            let settings = ui_settings.clone();
            let weak = window.as_weak();
            window.on_layout_changed(move || {
                let Some(window) = weak.upgrade() else {
                    return;
                };
                let mut settings = settings.borrow_mut();
                settings.layout = read_layout(&window);
                let _ = settings.save();
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
            let commands = command_state.clone();
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
                with_gesture_history(&st, &commands, &window, "Snap markers", || {                    let Some(snapped) = st.borrow_mut().session.snap_all_markers() else {
                        window.set_status_message("No sample to snap".into());
                        return false;
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
                    true
                });
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
                let commands = command_state.clone();
                let weak = window.as_weak();
                window.$on(move |v: f32| {
                    let Some(window) = weak.upgrade() else { return };
                    with_gesture_history(&st, &commands, &window, "Filter envelope", || {
                        let mut st = st.borrow_mut();
                        let ch = st.session.selected;
                        let Some(channel) = st.session.channels.get_mut(ch) else {
                            return false;
                        };
                        #[allow(clippy::redundant_closure_call)]
                        let value = ($map)(v);
                        if channel.params.filter_env_mut().$field == value {
                            return false;
                        }
                        channel.params.filter_env_mut().$field = value;
                        let p = channel.params;
                        let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                            channel: ch as u8,
                            params: p,
                        });
                        true
                    });
                });
            }};
        }
        wire_filter_env_param!(on_sampler_filter_attack_changed, attack, envelope_seconds);
        wire_filter_env_param!(on_sampler_filter_decay_changed, decay, envelope_seconds);
        wire_filter_env_param!(on_sampler_filter_sustain_changed, sustain, |v: f32| v);
        wire_filter_env_param!(on_sampler_filter_release_changed, release, envelope_seconds);

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
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_reverse_playback_changed(move |reverse| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Reverse", || {
                    let mut st = st.borrow_mut();
                    let ch = st.session.selected;
                    let Some(channel) = st.session.channels.get_mut(ch) else {
                        return false;
                    };
                    channel.params.reverse = reverse;
                    let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                        channel: ch as u8,
                        params: channel.params,
                    });
                    true
                });
            });
        }

        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_root_note_changed(move |note| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Root note", || {
                    let mut st = st.borrow_mut();
                    let ch = st.session.selected;
                    let Some(channel) = st.session.channels.get_mut(ch) else {
                        return false;
                    };
                    channel.params.root_note = note.clamp(0, 127) as u8;
                    if let Some(window) = weak.upgrade() {
                        window.set_tune_label(tune_label(channel.params).into());
                    }
                    let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                        channel: ch as u8,
                        params: channel.params,
                    });
                    true
                });
            });
        }

        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_tune_semitones_changed(move |v: f32| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Tune", || {
                    let mut st = st.borrow_mut();
                    let ch = st.session.selected;
                    let Some(channel) = st.session.channels.get_mut(ch) else {
                        return false;
                    };
                    channel.params.tune_semitones = v;
                    if let Some(window) = weak.upgrade() {
                        window.set_tune_label(tune_label(channel.params).into());
                    }
                    let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                        channel: ch as u8,
                        params: channel.params,
                    });
                    true
                });
            });
        }

        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_tune_cents_changed(move |v: f32| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Tune", || {
                    let mut st = st.borrow_mut();
                    let ch = st.session.selected;
                    let Some(channel) = st.session.channels.get_mut(ch) else {
                        return false;
                    };
                    channel.params.tune_cents = v;
                    if let Some(window) = weak.upgrade() {
                        window.set_tune_label(tune_label(channel.params).into());
                    }
                    let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                        channel: ch as u8,
                        params: channel.params,
                    });
                    true
                });
            });
        }

        {
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_retune_live_changed(move |on| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Retune", || {
                    let mut st = st.borrow_mut();
                    let ch = st.session.selected;
                    let Some(channel) = st.session.channels.get_mut(ch) else {
                        return false;
                    };
                    channel.params.retune_live = on;
                    let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                        channel: ch as u8,
                        params: channel.params,
                    });
                    true
                });
            });
        }

        {
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_filter_env_changed(move |v| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Filter envelope", || {
                    let mut st = st.borrow_mut();
                    let ch = st.session.selected;
                    let Some(channel) = st.session.channels.get_mut(ch) else {
                        return false;
                    };
                    channel.params.filter_env_amount = v.clamp(0.0, 1.0) * 2.0 - 1.0;
                    let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                        channel: ch as u8,
                        params: channel.params,
                    });
                    true
                });
            });
        }

        {
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_loop_mode_changed(move |i| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Loop mode", || {
                    let mut st = st.borrow_mut();
                    let ch = st.session.selected;
                    let Some(channel) = st.session.channels.get_mut(ch) else {
                        return false;
                    };
                    channel.params.loop_mode = loop_mode_from_int(i);
                    let p = channel.params;
                    let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                        channel: ch as u8,
                        params: p,
                    });
                    true
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
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_play_mode_changed(move |value| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Play mode", || {
                    let mut st = st.borrow_mut();
                    let ch = st.session.selected;
                    let Some(channel) = st.session.channels.get_mut(ch) else {
                        return false;
                    };
                    channel.params.play_mode = PlayMode::from_index(value);
                    let p = channel.params;
                    let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                        channel: ch as u8,
                        params: p,
                    });
                    true
                });
            });
        }
        {
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_slice_base_note_changed(move |note| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Slice base note", || {
                    let mut st = st.borrow_mut();
                    let ch = st.session.selected;
                    let Some(channel) = st.session.channels.get_mut(ch) else {
                        return false;
                    };
                    channel.params.slice_base_note = note.clamp(0, 127) as u8;
                    let p = channel.params;
                    let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                        channel: ch as u8,
                        params: p,
                    });
                    true
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
                // The channel, not the seat: a structural edit between the
                // press and the release would otherwise send the note-off to
                // whoever had slid into this row.
                st.session.slice_audition =
                    st.session.channel_id(ch).map(|id| (id, note as u8));
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
                let mut st = st.borrow_mut();
                let Some((id, note)) = st.session.slice_audition.take() else {
                    return;
                };
                // The engine works in seats, so the identity is resolved here
                // and nowhere earlier. A channel that has gone resolves to
                // nothing and the release is dropped, which is the right
                // answer: there is no voice of its to release.
                let Some(channel) = st.session.channel_index(id) else {
                    return;
                };
                let _ = tx.send(EngineCommand::ReleaseChannelNote {
                    channel: channel as u8,
                    note,
                });
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
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_voice_mode_changed(move |value| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Voice mode", || {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.params.voice_mode = voice_mode_from_int(value);
                    let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                        channel: channel_index as u8,
                        params: channel.params,
                    });
                    true
                });
            });
        }

        {
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_sampler_polyphony_changed(move |value| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Polyphony", || {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.params.polyphony = value.clamp(1, 16) as u8;
                    let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                        channel: channel_index as u8,
                        params: channel.params,
                    });
                    true
                });
            });
        }

        {
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_retrigger_mode_changed(move |value| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Retrigger", || {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.params.retrigger_mode = retrigger_mode_from_int(value);
                    let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                        channel: channel_index as u8,
                        params: channel.params,
                    });
                    true
                });
            });
        }

        {
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_choke_group_changed(move |value| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Choke group", || {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.params.choke_group = value.clamp(0, 16) as u8;
                    let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                        channel: channel_index as u8,
                        params: channel.params,
                    });
                    true
                });
            });
        }

        // Mode, ratio and grain are ordinary parameters. The enable is not:
        // the pool it needs is ~1.6 MB and must be built here rather than on
        // the audio thread, so it rides a structural command alongside the
        // parameter write. The two can arrive in either order -- a sampler
        // whose intent is on but whose pool has not landed plays unstretched.
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let stx = structural_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_stretch_enabled_changed(move |on| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Stretch", || {
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
                    true
                });
            });
        }

        {
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_stretch_sync_changed(move |on| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Stretch sync", || {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.params.stretch_sync = on;
                    let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                        channel: channel_index as u8,
                        params: channel.params,
                    });
                    true
                });
            });
        }

        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_stretch_bars_changed(move |norm| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Stretch bars", || {
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
                    true
                });
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
                let commands = command_state.clone();
                let weak = window.as_weak();
                window.$callback(move |text| {
                    let Some(window) = weak.upgrade() else { return };
                    let Some(typed) = parse_typed_value(text.as_str()) else {
                        st.borrow().refresh_editor(&window);
                        return;
                    };
                    with_gesture_history(&st, &commands, &window, "Typed value", || {
                        let mut st = st.borrow_mut();
                        let channel_index = st.session.selected;
                        let channel = &mut st.session.channels[channel_index];
                        let was = channel.params;
                        #[allow(clippy::redundant_closure_call)]
                        ($apply)(&mut channel.params, typed);
                        if channel.params == was {
                            return false;
                        }
                        let _ = tx.send(EngineCommand::SetChannelSamplerParams {
                            channel: channel_index as u8,
                            params: channel.params,
                        });
                        true
                    });
                    st.borrow().refresh_editor(&window);
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
            p.tune_semitones =
                v.clamp(SAMPLER_TUNE_SEMITONE_CLAMP.0, SAMPLER_TUNE_SEMITONE_CLAMP.1);
        });

        {
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_stretch_mode_changed(move |value| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Stretch mode", || {
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
                    true
                });
            });
        }

        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_stretch_ratio_changed(move |norm| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Stretch ratio", || {
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
                    true
                });
            });
        }

        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_stretch_grain_changed(move |norm| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Stretch grain", || {
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
                    true
                });
            });
        }

        macro_rules! wire_drum_param {
            ($callback:ident, $field:ident) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                let commands = command_state.clone();
                let window_weak = window.as_weak();
                window.$callback(move |value: f32| {
                    let Some(window) = window_weak.upgrade() else { return };
                    let params = std::cell::Cell::new(None);
                    with_gesture_history(&st, &commands, &window, "Drum parameter", || {
                        let mut st = st.borrow_mut();
                        let channel_index = st.session.selected;
                        let channel = &mut st.session.channels[channel_index];
                        let next = value;
                        if channel.drum_params.$field == next {
                            return false;
                        }
                        channel.drum_params.$field = next;
                        params.set(Some(channel.drum_params));
                        let _ = tx.send(EngineCommand::SetChannelDrumSynthParams {
                            channel: channel_index as u8,
                            params: channel.drum_params,
                        });
                        true
                    });
                    if let Some(params) = params.get() {
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
            ($callback:ident, $field:ident, $map:path) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                let commands = command_state.clone();
                let window_weak = window.as_weak();
                window.$callback(move |value| {
                    let Some(window) = window_weak.upgrade() else { return };
                    let params = std::cell::Cell::new(None);
                    with_gesture_history(&st, &commands, &window, "Drum parameter", || {
                        let mut st = st.borrow_mut();
                        let channel_index = st.session.selected;
                        let channel = &mut st.session.channels[channel_index];
                        let next = $map(value);
                        if channel.drum_params.$field == next {
                            return false;
                        }
                        channel.drum_params.$field = next;
                        params.set(Some(channel.drum_params));
                        let _ = tx.send(EngineCommand::SetChannelDrumSynthParams {
                            channel: channel_index as u8,
                            params: channel.drum_params,
                        });
                        true
                    });
                    if let Some(params) = params.get() {
                        sync_drum_preview(&window, params);
                    }
                });
            }};
        }

        wire_drum_int_param!(
            on_drum_kick_character_changed,
            kick_character,
            KickCharacter::from_index
        );
        wire_drum_int_param!(
            on_drum_snare_character_changed,
            snare_character,
            SnareCharacter::from_index
        );
        wire_drum_int_param!(
            on_drum_hat_character_changed,
            hat_character,
            HatCharacter::from_index
        );

        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let window_weak = window.as_weak();
            window.on_drum_mode_changed(move |value| {
                let Some(window) = window_weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Drum mode", || {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.drum_params.mode = DrumMode::from_index(value);
                    let params = channel.drum_params;
                    let _ = tx.send(EngineCommand::SetChannelDrumSynthParams {
                        channel: channel_index as u8,
                        params,
                    });
                    drop(st);
                    if let Some(window) = window_weak.upgrade() {
                        sync_drum_preview(&window, params);
                    }
                    true
                });
            });
        }
        {
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_drum_choke_group_changed(move |value| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Choke group", || {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.drum_params.choke_group = value.clamp(0, 16) as u8;
                    let _ = tx.send(EngineCommand::SetChannelDrumSynthParams {
                        channel: channel_index as u8,
                        params: channel.drum_params,
                    });
                    true
                });
            });
        }

        macro_rules! wire_source_param {
            ($params:ident, $command:ident, $callback:ident, $($field:ident).+) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                let commands = command_state.clone();
                let weak = window.as_weak();
                window.$callback(move |value: f32| {
                    let Some(window) = weak.upgrade() else { return };
                    with_gesture_history(&st, &commands, &window, "Synth parameter", || {
                        let mut st = st.borrow_mut();
                        let channel_index = st.session.selected;
                        let channel = &mut st.session.channels[channel_index];
                        if channel.$params.$($field).+ == value {
                            return false;
                        }
                        channel.$params.$($field).+ = value;
                        let _ = tx.send(EngineCommand::$command {
                            channel: channel_index as u8,
                            params: channel.$params,
                        });
                        true
                    });
                });
            }};
        }
        macro_rules! wire_mono_param {
            ($callback:ident, $($field:ident).+) => { wire_source_param!(mono_params, SetChannelMonoSynthParams, $callback, $($field).+) };
        }
        macro_rules! wire_mlm1_param {
            ($callback:ident, $($field:ident).+) => { wire_source_param!(mlm1_params, SetChannelMlM1Params, $callback, $($field).+) };
        }
        macro_rules! wire_poly_param {
            ($callback:ident, $($field:ident).+) => { wire_source_param!(poly_params, SetChannelPolySynthParams, $callback, $($field).+) };
        }
        macro_rules! wire_source_osc_float {
            ($params:ident, $command:ident, $callback:ident, $index:expr, $field:ident) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                let commands = command_state.clone();
                let weak = window.as_weak();
                window.$callback(move |value: f32| {
                    let Some(window) = weak.upgrade() else { return };
                    with_gesture_history(&st, &commands, &window, "Oscillator", || {
                        let mut st = st.borrow_mut();
                        let channel_index = st.session.selected;
                        let channel = &mut st.session.channels[channel_index];
                        if channel.$params.osc[$index].$field == value {
                            return false;
                        }
                        channel.$params.osc[$index].$field = value;
                        let _ = tx.send(EngineCommand::$command {
                            channel: channel_index as u8,
                            params: channel.$params,
                        });
                        true
                    });
                });
            }};
        }
        macro_rules! wire_mono_osc_float {
            ($callback:ident, $index:expr, $field:ident) => { wire_source_osc_float!(mono_params, SetChannelMonoSynthParams, $callback, $index, $field) };
        }
        macro_rules! wire_mlm1_osc_float {
            ($callback:ident, $index:expr, $field:ident) => { wire_source_osc_float!(mlm1_params, SetChannelMlM1Params, $callback, $index, $field) };
        }
        macro_rules! wire_poly_osc_float {
            ($callback:ident, $index:expr, $field:ident) => { wire_source_osc_float!(poly_params, SetChannelPolySynthParams, $callback, $index, $field) };
        }
        macro_rules! wire_source_osc_wave {
            ($params:ident, $command:ident, $callback:ident, $index:expr) => {{
                let tx = cmd_tx.clone();
                let st = state.clone();
                let commands = command_state.clone();
                let weak = window.as_weak();
                window.$callback(move |value| {
                    let Some(window) = weak.upgrade() else { return };
                    with_gesture_history(&st, &commands, &window, "Oscillator wave", || {
                        let mut st = st.borrow_mut();
                        let channel_index = st.session.selected;
                        let channel = &mut st.session.channels[channel_index];
                        let wave = osc_wave_from_int(value);
                        if channel.$params.osc[$index].wave == wave {
                            return false;
                        }
                        channel.$params.osc[$index].wave = wave;
                        let _ = tx.send(EngineCommand::$command {
                            channel: channel_index as u8,
                            params: channel.$params,
                        });
                        true
                    });
                });
            }};
        }
        macro_rules! wire_mono_osc_wave {
            ($callback:ident, $index:expr) => { wire_source_osc_wave!(mono_params, SetChannelMonoSynthParams, $callback, $index) };
        }
        macro_rules! wire_mlm1_osc_wave {
            ($callback:ident, $index:expr) => { wire_source_osc_wave!(mlm1_params, SetChannelMlM1Params, $callback, $index) };
        }
        macro_rules! wire_poly_osc_wave {
            ($callback:ident, $index:expr) => { wire_source_osc_wave!(poly_params, SetChannelPolySynthParams, $callback, $index) };
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
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_mono_lfo_wave_changed(move |value| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "LFO wave", || {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.mono_params.lfo.wave = lfo_wave_from_int(value);
                    let _ = tx.send(EngineCommand::SetChannelMonoSynthParams {
                        channel: channel_index as u8,
                        params: channel.mono_params,
                    });
                    true
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
                let commands = command_state.clone();
                let weak = window.as_weak();
                window.$callback(move |value: i32| {
                    let Some(window) = weak.upgrade() else { return };
                    with_gesture_history(&st, &commands, &window, "Synth mode", || {
                        let mut st = st.borrow_mut();
                        let channel_index = st.session.selected;
                        let channel = &mut st.session.channels[channel_index];
                        let next = $from_index(value);
                        if channel.mlm1_params.$field == next {
                            return false;
                        }
                        channel.mlm1_params.$field = next;
                        let _ = tx.send(EngineCommand::SetChannelMlM1Params {
                            channel: channel_index as u8,
                            params: channel.mlm1_params,
                        });
                        true
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
                let commands = command_state.clone();
                let weak = window.as_weak();
                window.$callback(move |value: $ty| {
                    let Some(window) = weak.upgrade() else { return };
                    let id: u32 = $id;
                    let value = value as f32;
                    with_gesture_history(&st, &commands, &window, generator_param_label(DeviceKind::MlP8, id), || {
                        let mut st = st.borrow_mut();
                        let channel_index = st.session.selected;
                        let channel = &mut st.session.channels[channel_index];
                        let mut params = GeneratorParams::MlP8(channel.mlp8_params);
                        let Some(value) = params.set(id, value) else {
                            return false;
                        };
                        if let GeneratorParams::MlP8(updated) = params {
                            channel.mlp8_params = updated;
                        }
                        let _ = tx.send(EngineCommand::SetChannelGeneratorParam {
                            channel: channel_index as u8,
                            id,
                            value,
                        });
                        true
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
            let commands = command_state.clone();
            window.on_ds01_value_changed(move |id, normalized| {
                let id = id as u32;
                let (Some(window), Some(descriptor)) = (weak.upgrade(), ds01::descriptor(id))
                else {
                    return;
                };
                with_gesture_history(&st, &commands, &window, descriptor.name, || {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    let mut params = GeneratorParams::Ds01(channel.ds01_params);
                    let Some(value) = params.set(id, descriptor.from_normalized(normalized)) else {
                        return false;
                    };
                    if let GeneratorParams::Ds01(updated) = params {
                        channel.ds01_params = updated;
                    }
                    let _ = tx.send(EngineCommand::SetChannelGeneratorParam {
                        channel: channel_index as u8,
                        id,
                        value,
                    });
                    touch_ds01_param(&window, &channel.ds01_params, id);
                    true
                });
                redraw();
            });
        }

        // Aux In. Two closures of its own, for the two parameters that are
        // pickers whose *rows* are not their values: the source list is
        // filtered to the channels that publish audio, so a row has to be
        // mapped back to a channel index here, where the project is in hand.
        // The value that goes on the wire is still the descriptor's, so a
        // lane and a knob agree. Level, the one continuous parameter, is the
        // pilot for `on_source_param_changed` below
        // (`docs/plans/generator-face-rows/`) and has no closure here.
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_aux_in_source_picked(move |row| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Aux input", || {
                    let (consumer, params) = {
                        let mut st = st.borrow_mut();
                        let consumer = st.session.selected;
                        let sources = aux_in_sources(&st.session, consumer);
                        // Row zero is "None"; every other row indexes the
                        // filtered list, whose entries carry their real index.
                        let picked = (row > 0)
                            .then(|| sources.get(row as usize - 1).map(|(index, _)| *index))
                            .flatten();
                        let outlets = picked
                            .map(|source| {
                                aux_in_outlets(&st.session, source)
                                    .iter()
                                    .map(|outlet| outlet.id)
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        // Both halves of the subscription, from the one place
                        // that can see both: the row names a seat, and the
                        // identity beside it is what survives the next reorder.
                        let source_id = picked
                            .and_then(|source| st.session.channel_id(usize::from(source)))
                            .unwrap_or_default();
                        let Some(channel) = st.session.channels.get_mut(consumer) else {
                            return false;
                        };
                        channel.aux_in_params.source_channel = picked.map_or(-1, i16::from);
                        channel.aux_in_params.source_id = source_id;
                        // A fresh pick lands on something rather than on a
                        // refusal: if the outlet it was reading is not published
                        // by the new source, take that source's first.
                        if !outlets.is_empty()
                            && !outlets.contains(&channel.aux_in_params.source_outlet)
                        {
                            channel.aux_in_params.source_outlet = outlets[0];
                        }
                        (consumer, channel.aux_in_params)
                    };
                    send_aux_in_subscription(&tx, consumer, params);
                    if let Some(window) = weak.upgrade() {
                        st.borrow().refresh_editor(&window);
                    }
                    true
                });
            });
        }
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_aux_in_outlet_picked(move |row| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Aux outlet", || {
                    let (consumer, params) = {
                        let mut st = st.borrow_mut();
                        let consumer = st.session.selected;
                        let Some(source) = st
                            .session
                            .channels
                            .get(consumer)
                            .and_then(|channel| channel.aux_in_params.subscription())
                            .map(|subscription| subscription.channel)
                        else {
                            return false;
                        };
                        let outlets = aux_in_outlets(&st.session, source);
                        let Some(outlet) = outlets.get(row.max(0) as usize).map(|o| o.id) else {
                            return false;
                        };
                        let Some(channel) = st.session.channels.get_mut(consumer) else {
                            return false;
                        };
                        channel.aux_in_params.source_outlet = outlet;
                        (consumer, channel.aux_in_params)
                    };
                    send_aux_in_subscription(&tx, consumer, params);
                    if let Some(window) = weak.upgrade() {
                        st.borrow().refresh_editor(&window);
                    }
                    true
                });
            });
        }
        // The pilot for the shared generator-row callback finding 4 asks for
        // (`docs/plans/generator-face-rows/`): `main.slint` sends the
        // descriptor id and a normalized position for every parameter drawn
        // from `source.pK`, exactly as `on_effect_param_changed` does for
        // `EffectSlotRow`, and DS-01's `on_ds01_value_changed` above already
        // does for its own ninety-two. Aux In's Level is the only source
        // wired to it today, so `id` always resolves through its table; a
        // second generator arriving here would need the same kind check
        // `on_ds01_value_changed` does not need either, because nothing else
        // fires this callback yet.
        {
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_source_param_changed(move |id, normalized| {
                let id = id as u32;
                let (Some(window), Some(descriptor)) = (weak.upgrade(), aux_in::descriptor(id))
                else {
                    return;
                };
                with_gesture_history(&st, &commands, &window, descriptor.name, || {
                    let mut st = st.borrow_mut();
                    let consumer = st.session.selected;
                    let Some(channel) = st.session.channels.get_mut(consumer) else {
                        return false;
                    };
                    let mut params = GeneratorParams::AuxIn(channel.aux_in_params);
                    let Some(value) = params.set(id, descriptor.from_normalized(normalized))
                    else {
                        return false;
                    };
                    if let GeneratorParams::AuxIn(updated) = params {
                        channel.aux_in_params = updated;
                    }
                    let _ = tx.send(EngineCommand::SetChannelGeneratorParam {
                        channel: consumer as u8,
                        id,
                        value,
                    });
                    drop(st);
                    if let Some(window) = weak.upgrade() {
                        window.set_aux_in_level_text(format!("{:.1} dB", linear_to_db(value)).into());
                    }
                    true
                });
            });
        }

        // A typed value field commits through one handler, because the
        // descriptor id travels with the text. `GeneratorParams::set` does the
        // clamping, so a typed number lands under exactly the same rules as a
        // dragged one.
        //
        // Typed entry is what the paged face buys that the one-screen one
        // could not: at a 21px dial there was no room for a field, so a time
        // was dragged on a scope and a number could only be approached.
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            let redraw = schedule_ds01_preview.clone();
            let commands = command_state.clone();
            window.on_ds01_text_committed(move |id, text| {
                let id = id.max(0) as u32;
                let Some(descriptor) = ds01::descriptor(id) else {
                    return;
                };
                let refresh = || {
                    if let Some(window) = weak.upgrade() {
                        let params = {
                            let st = st.borrow();
                            st.session.channels[st.session.selected].ds01_params
                        };
                        touch_ds01_param(&window, &params, id);
                    }
                };
                // What the number means is decided against the value the
                // field was showing, so `ms`, `kHz` and `%` read back as
                // themselves and a bare number means the unit on the face.
                let current = {
                    let st = st.borrow();
                    let params = st.session.channels[st.session.selected].ds01_params;
                    ds01::get(&params, id).unwrap_or(descriptor.default)
                };
                let Some(typed) = ds01_typed_value(descriptor, text.as_str(), current) else {
                    // Put the field back to what the patch says rather than
                    // leaving a half-typed string standing where a value goes.
                    refresh();
                    return;
                };
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, descriptor.name, || {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    let mut params = GeneratorParams::Ds01(channel.ds01_params);
                    let Some(value) = params.set(id, typed.clamp(descriptor.min, descriptor.max))
                    else {
                        return false;
                    };
                    if let GeneratorParams::Ds01(updated) = params {
                        channel.ds01_params = updated;
                    }
                    let _ = tx.send(EngineCommand::SetChannelGeneratorParam {
                        channel: channel_index as u8,
                        id,
                        value,
                    });
                    true
                });
                refresh();
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
        wire_mlp8!(on_mlp8_master_volume_changed, p8::PARAM_MASTER_VOLUME, f32);
        wire_mlp8!(on_mlp8_master_pan_changed, p8::PARAM_MASTER_PAN, f32);
        wire_mlp8!(on_mlp8_drift_changed, p8::PARAM_DRIFT, f32);
        wire_mlp8!(on_mlp8_unison_changed, p8::PARAM_UNISON, i32);
        wire_mlp8!(on_mlp8_detune_changed, p8::PARAM_DETUNE, f32);
        wire_mlp8!(on_mlp8_spread_changed, p8::PARAM_SPREAD, f32);
        wire_mlp8!(on_mlp8_chorus_changed, p8::PARAM_CHORUS, i32);
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
            let weak = window.as_weak();
            let commands = command_state.clone();
            // Sync is a lamp rather than a selector, so it arrives as a bool
            // and reaches the same stepped descriptor as everything else.
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_mlp8_lfo_sync_changed(move |on| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "LFO sync", || {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    let mut params = GeneratorParams::MlP8(channel.mlp8_params);
                    let Some(value) = params.set(p8::PARAM_LFO_SYNC, f32::from(u8::from(on))) else {
                        return false;
                    };
                    if let GeneratorParams::MlP8(updated) = params {
                        channel.mlp8_params = updated;
                    }
                    let _ = tx.send(EngineCommand::SetChannelGeneratorParam {
                        channel: channel_index as u8,
                        id: p8::PARAM_LFO_SYNC,
                        value,
                    });
                    true
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
                let commands = command_state.clone();
                let weak = window.as_weak();
                window.$callback(move |$($arg),*| {
                    let Some(window) = weak.upgrade() else {
                        return;
                    };
                    with_gesture_history(&st, &commands, &window, "ML-P8 route", || {
                        let mut st = st.borrow_mut();
                        let index = st.session.selected;
                        if st.session.channels[index].kind != DeviceKind::MlP8 {
                            return false;
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
                            return false;
                        };
                        let _ = tx.send(command);
                        let routes = st.session.channels[index].mlp8_params.routes;
                        refresh_mlp8_routes(&window, &routes);
                        st.session.dirty = true;
                        st.session.revision = st.session.revision.wrapping_add(1);
                        st.update_document_title(&window);
                        true
                    });
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
            let commands = command_state.clone();
            // The depth is a drag, so it neither redraws the list nor takes
            // the structural path: it is the one part of a route that is an
            // ordinary automatable value.
            let tx = cmd_tx.clone();
            let st = state.clone();
            let weak = window.as_weak();
            window.on_mlp8_route_amount_changed(move |id, amount| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "ML-P8 route", || {
                    let (Some(window), Ok(id)) = (weak.upgrade(), u16::try_from(id)) else {
                        return false;
                    };
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    if st.session.channels[channel_index].kind != DeviceKind::MlP8 {
                        return false;
                    }
                    if !st.session.channels[channel_index]
                        .mlp8_params
                        .routes
                        .set_amount(id, amount)
                    {
                        return false;
                    }
                    let _ = tx.send(EngineCommand::SetSourceRouteAmount {
                        channel: channel_index as u8,
                        route: id,
                        amount,
                    });
                    // The stored value, not the one that arrived: `set_amount`
                    // clamps, and the row has to show what the patch holds.
                    let stored = st.session.channels[channel_index]
                        .mlp8_params
                        .routes
                        .get(id)
                        .map_or(amount, |route| route.amount);
                    touch_mlp8_route_amount(&window, id, stored);
                    st.session.dirty = true;
                    true
                });
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
            let commands = command_state.clone();
            window.on_mlp8_text_committed(move |id, text| {
                let id = id.max(0) as u32;
                let refresh_only = || {
                    if let Some(window) = weak.upgrade() {
                        st.borrow().refresh_editor(&window);
                    }
                };
                // What a bare number means is decided against the value the
                // field was showing, the way DS-01's fields already do it: an
                // envelope stage prints milliseconds under a second, so `500`
                // typed into one is half a second and not eight (the ceiling
                // it used to clamp to).
                let bare_scale = mooloop_core::mlp8::descriptor(id)
                    .map(|descriptor| {
                        let current = {
                            let st = st.borrow();
                            let params = st.session.channels[st.session.selected].mlp8_params;
                            mooloop_core::mlp8::get(&params, id).unwrap_or(descriptor.default)
                        };
                        display_unit(descriptor, current).0
                    })
                    .unwrap_or(1.0);
                let Some(typed) = typed_value(text.as_str(), bare_scale) else {
                    refresh_only();
                    return;
                };
                // The five mix levels read in dB and store linear; core owns
                // which those are so the face and this handler cannot drift.
                // A gain field reads in dB and stores linear. `dB` is not a
                // prefix, so `typed` is the number as typed either way.
                let value = if mooloop_core::mlp8::is_gain_param(id) {
                    mooloop_core::gain::db_to_linear(typed)
                } else {
                    typed
                };
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, generator_param_label(DeviceKind::MlP8, id), || {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    let mut params = GeneratorParams::MlP8(channel.mlp8_params);
                    let Some(clamped) = params.set(id, value) else {
                        return false;
                    };
                    let GeneratorParams::MlP8(updated) = params else {
                        return false;
                    };
                    channel.mlp8_params = updated;
                    let _ = tx.send(EngineCommand::SetChannelGeneratorParam {
                        channel: channel_index as u8,
                        id,
                        value: clamped,
                    });
                    true
                });
                st.borrow().refresh_editor(&window);
            });
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
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_poly_lfo_wave_changed(move |value| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "LFO wave", || {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.poly_params.lfo.wave = lfo_wave_from_int(value);
                    let _ = tx.send(EngineCommand::SetChannelPolySynthParams {
                        channel: channel_index as u8,
                        params: channel.poly_params,
                    });
                    true
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
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_poly_mono_mode_changed(move |value| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Mono mode", || {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.poly_params.mono_mode = value;
                    let _ = tx.send(EngineCommand::SetChannelPolySynthParams {
                        channel: channel_index as u8,
                        params: channel.poly_params,
                    });
                    true
                });
            });
        }
        {
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_poly_env_trigger_changed(move |value| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Envelope trigger", || {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.poly_params.env_trigger = EnvTrigger::from_index(value);
                    let _ = tx.send(EngineCommand::SetChannelPolySynthParams {
                        channel: channel_index as u8,
                        params: channel.poly_params,
                    });
                    true
                });
            });
        }
        {
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_poly_note_priority_changed(move |value| {
                let mut st = st.borrow_mut();
                let channel_index = st.session.selected;
                let channel = &mut st.session.channels[channel_index];
                channel.poly_params.note_priority = NotePriority::from_index(value);
                let _ = tx.send(EngineCommand::SetChannelPolySynthParams {
                    channel: channel_index as u8,
                    params: channel.poly_params,
                });
            });
        }
        {
            let weak = window.as_weak();
            let commands = command_state.clone();
            let tx = cmd_tx.clone();
            let st = state.clone();
            window.on_poly_polyphony_changed(move |value| {
                let Some(window) = weak.upgrade() else { return };
                with_gesture_history(&st, &commands, &window, "Polyphony", || {
                    let mut st = st.borrow_mut();
                    let channel_index = st.session.selected;
                    let channel = &mut st.session.channels[channel_index];
                    channel.poly_params.polyphony = value.clamp(1, MAX_POLY_VOICES as i32) as u8;
                    let _ = tx.send(EngineCommand::SetChannelPolySynthParams {
                        channel: channel_index as u8,
                        params: channel.poly_params,
                    });
                    true
                });
            });
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
        //     worker thread like every other dialog call, handing the picked
        //     path to the pump, which applies it on the UI thread. ---
        let (browser_pick_tx, browser_pick_rx) =
            std::sync::mpsc::channel::<Result<PathBuf, String>>();
        let (browser_info_tx, browser_info_rx) = std::sync::mpsc::channel::<(
            PathBuf,
            Result<SampleInspection, (String, String)>,
        )>();
        // The sample the browser is currently waiting to hear about, so a
        // reply about any other one can be dropped.
        //
        // An inspection decodes the whole file on a worker thread, and since
        // the arrow keys audition what they land on, a walk down a folder
        // faster than a decode leaves several in flight at once -- which do
        // not finish in the order they were asked for. Without this, arrowing
        // past a long file lands its waveform, its stats and its *sound* on
        // top of the short one below it that has already been selected and
        // played. The pointer could always race this too; it just took a held
        // key to make it ordinary.
        let browser_inspecting: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
        {
            let browser_info_tx = browser_info_tx.clone();
            let inspecting = browser_inspecting.clone();
            window.on_browser_row_previewed(move |path| {
                let path = PathBuf::from(path.to_string());
                *inspecting.borrow_mut() = Some(path.clone());
                let tx = browser_info_tx.clone();
                std::thread::spawn(move || {
                    let result =
                        inspect_sample(&path).map_err(|error| (path.display().to_string(), error));
                    let _ = tx.send((path, result));
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
                std::thread::spawn(move || match pick_bundle_dialog("Add sample folder") {
                    Picked::Path(path) => {
                        let _ = tx.send(Ok(path));
                    }
                    Picked::Cancelled => {}
                    // Said, not dropped: without it the button does nothing
                    // on a desktop with no chooser (MOO-90).
                    Picked::Unavailable(none) => {
                        let _ = tx.send(Err(none.one_line()));
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
            window.on_browser_tab_changed(move |tab| {
                let mut st = st.borrow_mut();
                st.browser_tab = if tab == 1 {
                    BrowserTab::Presets
                } else {
                    BrowserTab::Samples
                };
                // Rescanned on entry rather than kept live. A watcher over
                // four directory trees would be the only way to be sure, and
                // the only writer that matters is this application saving a
                // preset -- which lands here too, by way of the tab being
                // re-entered.
                if st.browser_tab == BrowserTab::Presets {
                    st.preset_catalog = scan_preset_catalog();
                }
                refresh_browser(&st);
            });
        }
        {
            let st = state.clone();
            // Two senders, because the two halves of "load a preset" are two
            // different mechanisms. A generator or channel preset is a whole
            // document and goes down the asynchronous document path; an
            // effect preset is a rack edit and goes down the project-edit
            // path, which reinstalls the project and so carries the inserted
            // row's structure without a separate engine command.
            let doc_tx = document_tx.clone();
            let edit_tx = project_edit_tx.clone();
            let commands = command_state.clone();
            let weak = window.as_weak();
            window.on_browser_preset_loaded(move |path| {
                let Some(window) = weak.upgrade() else { return };
                let path = PathBuf::from(path.to_string());
                let Some((slot, name)) = ({
                    let st = st.borrow();
                    st.preset_catalog
                        .iter()
                        .find_map(|group| {
                            let preset = group
                                .presets
                                .iter()
                                .find(|preset| preset.path == path)?;
                            Some((group.slot, preset.name.clone()))
                        })
                }) else {
                    return;
                };

                match slot {
                    // The generator and channel halves are documents, and
                    // they already have a load path that runs off the UI
                    // thread. Reuse it rather than growing a second one.
                    PresetSlot::Generator(kind) => {
                        let channel_kind = {
                            let st = st.borrow();
                            st.session.channels.get(st.session.selected).map(|c| c.kind)
                        };
                        if channel_kind != Some(kind) {
                            window.set_status_message(
                                format!(
                                    "Select a {} channel to load this preset",
                                    device_kind_label(kind)
                                )
                                .into(),
                            );
                            return;
                        }
                        load_preset_document(
                            &doc_tx,
                            &window,
                            path,
                            LoadTarget::Generator { preset_name: name },
                            "generator preset",
                        );
                    }
                    PresetSlot::Channel => {
                        load_preset_document(
                            &doc_tx,
                            &window,
                            path,
                            LoadTarget::Channel,
                            "channel preset",
                        );
                    }
                    // An effect preset *adds* a device. See `PresetSlot`.
                    PresetSlot::Effect(kind) => {
                        let landed = append_effect_preset(&st, &window, &path, kind, &name);
                        if let Some((before, after)) = landed {
                            if queue_project_edit(
                                &edit_tx,
                                before,
                                after,
                                "Effect preset added",
                            ) {
                                commands.borrow_mut().project_edit_pending = true;
                                sync_command_availability(&window, &commands.borrow());
                            }
                        }
                    }
                }
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

        // --- Sample loading via a file dialog + Symphonia (selected channel) ---
        // The dialog + decode run on a worker thread so the UI stays
        // responsive (a blocking dialog makes the OS mark the app frozen and
        // offer to kill it). Results come back through `load_rx` and are
        // applied by the pump on the UI thread.
        let (load_tx, load_rx) = std::sync::mpsc::channel::<LoadResult>();
        {
            let st = state.clone();
            let load_tx = load_tx.clone();
            window.on_browser_sample_loaded(move |path| {
                let (channel, source_revision, request) = {
                    let mut st = st.borrow_mut();
                    let channel = st.session.selected;
                    let revision = st.session.source_revision;
                    let request = st.session.next_sample_request(channel);
                    (channel, revision, request)
                };
                spawn_browser_sample_load(
                    &path,
                    channel,
                    source_revision,
                    request,
                    false,
                    &load_tx,
                );
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
                // No token: the channel does not exist yet, so there is
                // nothing to key one by. This path is already correct for a
                // different reason -- the pump defers these into a `Vec` and
                // creates one channel per load -- and the comment there
                // records that somebody got it wrong once. Forcing one
                // mechanism over both would make the working case worse.
                spawn_browser_sample_load(&path, channel, source_revision, 0, true, &load_tx);
            });
        }
        {
            let st = state.clone();
            let load_tx = load_tx.clone();
            window.on_load_sample_clicked(move || {
                let (channel, source_revision, request) = {
                    let mut st = st.borrow_mut();
                    let channel = st.session.selected;
                    let revision = st.session.source_revision;
                    let request = st.session.next_sample_request(channel);
                    (channel, revision, request)
                };
                let tx = load_tx.clone();
                log_debug!("ui", "loading sample for channel {channel}");
                std::thread::spawn(move || {
                    let result = match pick_sample_dialog() {
                        Picked::Path(path) => Some(load_sample_at_path(&path)),
                        Picked::Cancelled => None,
                        // A failure, not a cancel (MOO-90): it reaches the
                        // status bar the way a sample that will not decode
                        // does.
                        Picked::Unavailable(none) => Some(Err(none.one_line())),
                    };
                    let _ = tx.send(LoadResult {
                        channel,
                        source_revision,
                        request,
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
                let request = st.borrow_mut().session.next_sample_request(target.channel);
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
                        request,
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
                let request = st.borrow_mut().session.next_sample_request(target.channel);
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
                        request,
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
        // Finished takes, decoded off the UI thread like any other file.
        let (take_tx, take_rx) = std::sync::mpsc::channel::<TakeLoad>();
        let pump = Timer::default();
        // The in-app question's state, for the answers only the pump can
        // give: a save that finished, a kit that would drop notes (MOO-91).
        let (after_save, unsaved_settled, question, kit_confirmed) = (
            after_save.clone(),
            unsaved_settled.clone(),
            question.clone(),
            kit_confirmed.clone(),
        );
        // Diagnostics shared with the autodrive self-test (MOOLOOP_AUTODRIVE=1).
        let stats = Rc::new(Cell::new((0.0f32, false, 0usize)));
        let stats_in = stats.clone();
        let master_clip_clear_in = master_clip_clear.clone();
        let bus_clip_clear_in = bus_clip_clear.clone();
        // One pair per bus, so a strip's decay is its own rather than shared.
        //
        // **Including the toolbar's.** The master used to be metered twice,
        // through two transports and with two clip latches: `executor.rs`
        // pushes `EngineEvent::Metering` every block and `render.rs` publishes
        // the same two numbers into `BusMeters` cell 0, the toolbar read the
        // event and the mixer's master strip read the cell. The event push is
        // `let _ = evt_tx.push(..)`, so under ring pressure the
        // *always-visible* meter was the lossy one while the atomic cell
        // cannot drop a block -- and clicking one clip lamp did not clear the
        // other. Both read bus 0 through this pair now.
        let mut input_meter = (MeterBallistics::default(), MeterBallistics::default());
        let mut bus_meters: Vec<(MeterBallistics, MeterBallistics)> =
            (0..MAX_BUSES).map(|_| Default::default()).collect();
        let mut last_meter_update = std::time::Instant::now();
        // The audio callback's own health, read once a second rather than
        // once a frame: every field is a count over a window, so polling it
        // faster would only make the window smaller.
        let mut last_load_report = std::time::Instant::now();
        let mut xruns_this_window = 0u32;
        let mut reported_time_shared = false;
        // Reused across pumps rather than allocated per pump: a desk sending
        // a fader stream fills these sixty times a second.
        let mut control_input: Vec<mooloop_core::MidiMessage> = Vec::new();
        let mut recorded: Vec<(u8, u8, u8, u8, u32, u32)> = Vec::new();
        // When a mapped hardware control last moved a parameter, which is
        // how the pump knows a controller gesture has ended: a desk sends no
        // release.
        let mut controller_moved_at: Option<std::time::Instant> = None;
        let mut last_port_scan = std::time::Instant::now()
            - std::time::Duration::from_secs(2);
        let autodrive_verbose = std::env::var_os("MOOLOOP_AUTODRIVE_VERBOSE").is_some();
        let mut playhead_was_nonempty = false;
        // The device-meter target the last tick drained, so the one it is
        // *leaving* can be emptied. A device meter is a `fetch_max` hold and
        // only a read empties one, so a chain nobody is looking at keeps its
        // loudest block forever and shows it for one tick the moment the rack
        // is turned back to it.
        let mut last_device_target: Option<usize> = None;
        pump.start(
            TimerMode::Repeated,
            std::time::Duration::from_millis(PUMP_INTERVAL_MS),
            move || {
                // A logout, a `kill` or Ctrl+C: leave the way Quit does, but
                // with no dialog, because nobody is there to answer one.
                // `main` finishes the takes after the loop. Unsaved changes
                // are not kept (that is autosave, MOO-103); the log says so.
                if let Some(signal) = signals::take() {
                    log_warn!("app", "quitting on {signal}");
                    if st.borrow().session.dirty {
                        log_warn!("app", "the song had unsaved changes, which were not saved");
                    }
                    slint::quit_event_loop().ok();
                    return;
                }
                // Applied here rather than in the callback because the
                // settings and state live in non-Send Rc/RefCells, while the
                // picked path crosses the thread boundary as plain data.
                while let Ok(picked) = browser_pick_rx.try_recv() {
                    let Some(window) = weak.upgrade() else {
                        continue;
                    };
                    let path = match picked {
                        Ok(path) => path,
                        Err(message) => {
                            window.set_status_message(message.into());
                            continue;
                        }
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
                // is armed, hand the decoded sample to the preview voice --
                // unless the selection has moved on since it was asked
                // for, which is what `browser_inspecting` decides.
                while let Ok((path, inspection)) = browser_info_rx.try_recv() {
                    let Some(window) = weak.upgrade() else {
                        continue;
                    };
                    if browser_inspecting.borrow().as_deref() != Some(path.as_path()) {
                        continue;
                    }
                    match inspection {
                        Ok(inspection) => {
                            window.set_browser_info_name(inspection.name.into());
                            window.set_browser_info_stats(inspection.stats.into());
                            window.set_browser_info_waveform(ModelRc::from(Rc::new(
                                VecModel::from(inspection.peaks),
                            )));
                            if window.get_browser_autoplay()
                                && !handle.preview(PreviewCommand::Play {
                                    sample: inspection.sample,
                                })
                            {
                                // A refused preview is silence where the user
                                // asked to hear something, and nothing else
                                // will ever mention it.
                                window.set_status_message(
                                    "Busy — could not start the preview".into(),
                                );
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
                    // Whatever a Save answer to "unsaved changes?" was
                    // waiting to do. Only a song save that succeeded goes on
                    // to it; every other result -- a failure, a cancelled
                    // chooser -- drops it, and the user stays where they are.
                    let continuation = after_save.take();
                    match result {
                        DocumentResult::Cancelled => {
                            window.set_status_message("".into());
                        }
                        DocumentResult::NewSong(project) => {
                            let samples = vec![None; project.channels.len()];
                            // The same guard the Open path runs, and for the
                            // same reason: this is an install, so anything
                            // still queued is addressed to the song being
                            // replaced. Without it a routing table indexed by
                            // the outgoing channel order, or a `sample_reset`
                            // naming a channel that is now somebody else,
                            // lands on top of the starter kit -- the drains
                            // below this arm run later in the same tick.
                            discard_document_messages(&pending_rx, &requeue_tx);
                            while sample_reset_rx.try_recv().is_ok() {}
                            install_project_in_ui(
                                &mut handle,
                                default_sample_for_pump.as_ref(),
                                &st,
                                &window,
                                &project,
                                &samples,
                                // A new song starts at the beginning, stopped.
                                false,
                            );
                            let mut state = st.borrow_mut();
                            state.session.bundle_path = None;
                            state.session.dirty = false;
                            state.session.revision = state.session.revision.wrapping_add(1);
                            state.session.document_generation =
                                state.session.document_generation.wrapping_add(1);
                            state.update_document_title(&window);
                            drop(state);
                            // An entry is a snapshot of the document it was
                            // taken from, so carrying one across a document
                            // boundary means an undo installs the *other*
                            // song -- and the save path would then write it
                            // to this one's path.
                            {
                                let mut commands = commands.borrow_mut();
                                commands.history.clear();
                                sync_command_availability(&window, &commands);
                            }
                            window.set_status_message("New randomized kit".into());
                        }
                        DocumentResult::SavedSong {
                            path,
                            mode,
                            revision,
                            generation,
                            report,
                            sample_references,
                        } => {
                            let mut state = st.borrow_mut();
                            if !apply_saved_song(
                                &mut state,
                                &window,
                                generation,
                                revision,
                                &path,
                                sample_references,
                            ) {
                                log_info!(
                                    "project",
                                    "song saved to {} after another was opened; the open song keeps its own path",
                                    path.display()
                                );
                                window.set_status_message(
                                    format!("Saved the previous song to {}", path.display()).into(),
                                );
                                continue;
                            }
                            // What the save *delivered*, not what it was
                            // asked for. A sample the bundle already owns is
                            // kept there whatever the mode says, so a box
                            // driven by the mode would go unticked on a song
                            // whose samples are all still embedded.
                            window.set_embed_assets(
                                mode == AssetMode::Embedded
                                    || state.session.has_embedded_samples(),
                            );
                            log_info!(
                                "project",
                                "song saved: {} ({} warnings, {} repairs)",
                                path.display(),
                                report.warnings.len(),
                                report.repairs.len()
                            );
                            log_repairs("saving the song", &report.repairs);
                            log_asset_warnings("saving the song", &report.warnings);
                            window.set_status_message(
                                operation_status(
                                    "Song saved",
                                    &path,
                                    &report.warnings,
                                    &report.repairs,
                                )
                                .into(),
                            );
                            let still_dirty = state.session.dirty;
                            let name = song_name(&state);
                            drop(state);
                            if let Some(after) = continuation {
                                // Changed while it was saving -- a MIDI take,
                                // a controller -- so it is not what was saved:
                                // ask again rather than drop the difference.
                                if still_dirty {
                                    ask_unsaved(&window, &question, after, name.as_deref());
                                } else {
                                    go_on_after_unsaved(&window, &unsaved_settled, after);
                                }
                            }
                        }
                        DocumentResult::SavedOther { label, report } => {
                            log_info!("project", "{label}");
                            log_repairs(label, &report.repairs);
                            log_asset_warnings(label, &report.warnings);
                            window.set_status_message(
                                format!(
                                    "{label}{}{}",
                                    warning_suffix(report.warnings.len()),
                                    repair_suffix(report.repairs.len())
                                )
                                .into(),
                            );
                        }
                        DocumentResult::SavedPreset {
                            label,
                            report,
                            named,
                        } => {
                            // Now, and not on confirm: the file is on disk.
                            match named {
                                Some(PresetNaming::Effect {
                                    target,
                                    device,
                                    name,
                                }) => {
                                    let mut state = st.borrow_mut();
                                    // Resolved now rather than when the
                                    // dialog was confirmed: the write has
                                    // been on disk in between, and the rack
                                    // may have been edited under it.
                                    if let Some(seat) = state.session.chain_target(target) {
                                        state.session.set_effect_preset_name(seat, device, &name);
                                        state.sync_effects();
                                    }
                                }
                                Some(PresetNaming::Source { channel, name }) => {
                                    let mut state = st.borrow_mut();
                                    if let Some(seat) = state.session.channel_index(channel) {
                                        state
                                            .session
                                            .set_source_preset_name(seat as u8, &name);
                                        window.set_source_preset_name(name.as_str().into());
                                    }
                                }
                                None => {}
                            }
                            log_info!("project", "{label}");
                            log_repairs(label, &report.repairs);
                            log_asset_warnings(label, &report.warnings);
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
                            // A kit shorter than the song drops channels, and
                            // with them their notes: asked first, in the app
                            // (MOO-91). The result waits in the question and
                            // comes back round on a yes, with the flag set.
                            if matches!(target, LoadTarget::Kit) && !kit_confirmed.replace(false) {
                                let current = st
                                    .borrow()
                                    .session
                                    .project_snapshot(window.get_bpm(), window.get_swing_percent());
                                if kit_drops_notes(&current, &document.report.document) {
                                    window.set_document_busy(true);
                                    ask_question(
                                        &window,
                                        &question,
                                        Question::LoadKit(Box::new(DocumentResult::Loaded {
                                            path,
                                            target,
                                            document,
                                        })),
                                        "Load this kit?",
                                        "It has fewer channels than the song, so the channels past its end go, and the notes on them with them.",
                                        "Load Kit",
                                        "",
                                    );
                                    continue;
                                }
                            }
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
                            log_asset_warnings("opening the file", &warnings);
                            let current = st
                                .borrow()
                                .session.project_snapshot(window.get_bpm(), window.get_swing_percent());
                            let current_samples = st.borrow().session.sample_snapshots();
                            let opens = load_opens_document(&target);
                            // Cloned before the merge consumes `target`, and
                            // applied after the install: a preset label
                            // describes the device in front of the user, and a
                            // load has just replaced some or all of them.
                            let load_target = target.clone();
                            let (project, samples) = match merge_loaded_document(
                                current,
                                current_samples,
                                target,
                                document,
                                loaded_samples,
                                // Asked above, before anything was taken
                                // apart.
                                |_| true,
                            ) {
                                Ok(merged) => merged,
                                Err(status) => {
                                    window.set_status_message(status.into());
                                    continue;
                                }
                            };
                            // A preset or a kit edits the song that is open,
                            // so it is an undo step like any other edit, and
                            // the snapshot it is undone to is taken before
                            // the install replaces it (MOO-95).
                            let before = (!opens).then(|| project_snapshot(&st.borrow(), &window));
                            // UI edits are mirrored into `project` before
                            // they enter this relay. Discard any that have
                            // not reached the engine yet: the prepared
                            // project already contains them (or, for a
                            // song load, deliberately supersedes them).
                            // What is addressed to the machine rather
                            // than the document goes back on the queue,
                            // in order, for the drain below. New Song
                            // runs the same two lines, which is why they
                            // are a function in `mooloop-session` and not
                            // written out here.
                            discard_document_messages(&pending_rx, &requeue_tx);
                            while sample_reset_rx.try_recv().is_ok() {}
                            if !install_project_in_ui(
                                &mut handle,
                                default_sample_for_pump.as_ref(),
                                &st,
                                &window,
                                &project,
                                &samples,
                                // Opening a song stops and rewinds: it is a
                                // different song, and its playhead is not
                                // this one's. A preset or a kit is an edit to
                                // the song that is playing, and keeps it
                                // playing, as every `ProjectEdit` does.
                                !opens,
                            ) {
                                window.set_status_message(
                                    "Audio engine is busy; project was not installed".into(),
                                );
                                continue;
                            }
                            finish_document_load(
                                &st,
                                &commands,
                                &window,
                                &path,
                                &load_target,
                                asset_mode,
                                before,
                            );
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
                // A quit that arrived during a save asks again now that the
                // save has reported, so its "unsaved changes?" question is
                // asked about the document as the save left it (MOO-92).
                let idle = weak
                    .upgrade()
                    .is_some_and(|window| !window.get_document_busy());
                if idle && quit_after_document.replace(false) {
                    if let Some(window) = weak.upgrade() {
                        window.invoke_quit_requested();
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
                    // A reset is a channel that has just become a fresh
                    // sampler, so it has no markers either -- which the two
                    // separate stores this replaced left standing.
                    handle.set_channel_audio(
                        channel,
                        match default_sample_for_pump.as_ref() {
                            Some(sample) => ChannelAudioSnapshot::sample(sample.clone()),
                            None => ChannelAudioSnapshot::default(),
                        },
                    );
                }
                // After the resets, and both halves together: a slice edit or
                // a commit is the most specific statement about what a
                // channel is playing, and its buffer and its map change at
                // the same instant.
                while let Ok(update) = channel_audio_rx.try_recv() {
                    handle.set_channel_audio(update.channel, update.audio);
                }
                // A `Vec`, not an `Option`: two "Load in New Channel"
                // decodes can land in the same 60 Hz tick, and an `Option`
                // silently kept the last -- one sample gone and one channel
                // created where two were asked for.
                let mut deferred_new_channel_loads = Vec::new();
                while let Ok(load) = load_rx.try_recv() {
                    let still_current = {
                        let st = st.borrow();
                        load.source_revision == st.session.source_revision
                            && (load.new_channel && st.session.channels.len() < MAX_CHANNELS
                                || !load.new_channel
                                    && st
                                        .session.channels
                                        .get(load.channel)
                                        .is_some_and(|channel| channel.kind == DeviceKind::Sampler)
                                    // And it must be the load this channel is
                                    // still waiting for. `source_revision` is
                                    // a property of the project, so two
                                    // in-flight decodes for one channel both
                                    // pass it and the last to *finish* wins --
                                    // which is decode time, so a long file
                                    // chosen first can overwrite the short one
                                    // chosen after it. A superseded completion
                                    // is dropped silently: it is not an error,
                                    // and saying so would be noise.
                                    && st.session.sample_request_is_current(
                                        load.channel,
                                        load.request,
                                    ))
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
                        deferred_new_channel_loads.push(loaded);
                        continue;
                    }
                    load_sample_with_history(
                        |channel, audio| handle.set_channel_audio(channel, audio),
                        &st,
                        &commands,
                        &weak,
                        load.channel,
                        loaded,
                    );
                }
                for loaded in deferred_new_channel_loads {
                    if let Some(window) = weak.upgrade() {
                        window.invoke_add_channel_clicked(0);
                        // Creating the channel queues its own default-sample
                        // reset. Spend it here, so the sample this whole
                        // branch exists to deliver is the last write to the
                        // slot rather than the first.
                        while let Ok(channel) = sample_reset_rx.try_recv() {
                            handle.set_channel_audio(
                                channel,
                                match default_sample_for_pump.as_ref() {
                                    Some(sample) => {
                                        ChannelAudioSnapshot::sample(sample.clone())
                                    }
                                    None => ChannelAudioSnapshot::default(),
                                },
                            );
                        }
                        let channel = st.borrow().session.channels.len().saturating_sub(1);
                        // Its own entry, after the add's: undo leaves the
                        // new channel holding the default sample, and undo
                        // again removes the channel.
                        load_sample_with_history(
                            |channel, audio| handle.set_channel_audio(channel, audio),
                            &st,
                            &commands,
                            &weak,
                            channel,
                            loaded,
                        );
                    }
                }
                // Takes whose drain has finished go to a worker to be decoded,
                // the way a dragged-in file is, and come back to be applied.
                {
                    let (finished, failures) = st.borrow_mut().takes.collect();
                    for failure in failures {
                        log_error!("ui", "a take could not be written: {failure}");
                    }
                    for take in finished {
                        let tx = take_tx.clone();
                        std::thread::spawn(move || {
                            let result = load_sample_at_path(&take.path);
                            let _ = tx.send(TakeLoad { take, result });
                        });
                    }
                }
                while let Ok(load) = take_rx.try_recv() {
                    apply_take(&handle, &st, &weak, &commands, load);
                }
                if let Some(window) = weak.upgrade() {
                    st.borrow().publish_take(&window);
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
                            // Read before the install, which sends the rack
                            // back to a channel; a track move puts it back.
                            let rack_was = st.borrow().session.effect_target;
                            if install_project_in_ui(
                                &mut handle,
                                default_sample_for_pump.as_ref(),
                                &st,
                                &window,
                                &edit.project,
                                &edit.samples,
                                // **Every project edit keeps the song
                                // running.** `LOOSE_ENDS.md`, "Every
                                // structural edit stops the song": a paste, a
                                // delete, a move, a track added, a preset
                                // loaded -- all of them stopped and rewound
                                // the transport, including for the channels
                                // the edit never touched.
                                //
                                // The plan proposed testing `edit.edit` for a
                                // `ListEdit`, which would have covered the
                                // three channel edits and left a track add or
                                // a preset load still stopping the song. The
                                // distinction that matters is edit versus
                                // open, and every `ProjectEdit` is an edit:
                                // the three install sites that are opens are
                                // the other callers of this function.
                                //
                                // Undo and redo are included, and should be.
                                // Undoing a channel delete mid-song is an
                                // edit to the song you are listening to.
                                true,
                            ) {
                                let mut state = st.borrow_mut();
                                // The song's own addresses were renumbered
                                // before this was queued; the session's --
                                // the selected device, the open lane, the
                                // preset labels -- are not in the snapshot
                                // and are renumbered here.
                                match edit.edit {
                                    Some(ListEdit::Channel(edit)) => {
                                        state.session.rescope_after(edit);
                                    }
                                    Some(ListEdit::Track(edit)) => {
                                        state.session.rescope_after_track(edit, rack_was);
                                        // The install left the rack on a
                                        // channel, so a track here is the
                                        // rescope putting it back.
                                        if matches!(
                                            state.session.effect_target,
                                            EffectTarget::Bus(_)
                                        ) {
                                            state.sync_mixer_selection();
                                            state.sync_effects();
                                            state.sync_bus_editor(&window);
                                        }
                                    }
                                    None => {}
                                }
                                // An undo carries no `edit`, so
                                // nothing above renumbered the label maps --
                                // and the snapshot does not restore them
                                // either, which is what three doc comments
                                // already say ("It does not survive undo,
                                // which restores the project but not this
                                // map"). Left alone they stay keyed to the
                                // *pre-undo* numbering: a label lands on a
                                // channel that never wore it, and
                                // `effect_preset_names` is keyed by a
                                // `DeviceId` minted per channel, so ids
                                // alias and the label can land on another
                                // channel's device rather than merely
                                // vanishing. Dropping them is what the
                                // comments describe.
                                if matches!(
                                    edit.history,
                                    Some((HistoryMove::Undo | HistoryMove::Redo, _))
                                ) {
                                    state.session.source_preset_names.clear();
                                    state.session.effect_preset_names.clear();
                                    window.set_source_preset_name(Default::default());
                                    state.sync_effects();
                                }
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
                                    // The output and the buffer size were
                                    // applied when the engine opened, on the
                                    // same saved config (`saved_audio_config`),
                                    // which is the only point where a missing
                                    // output can fall back to a working one.
                                    // Re-applying them here tried the missing
                                    // output again after the fallback had
                                    // landed.
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
                                            settings.audio.active_mut().pick_output((port_l, port_r));
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
                                            settings.audio.active_mut().buffer_size = Some(frames);
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
                                    settings.audio.active_mut().auto_reconnect = enabled;
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
                // After the drain, so a chain edit queued this tick is already
                // in the model the plan is derived from. Sends nothing unless
                // the plan actually moved, which is every tick but the few
                // after a structural edit
                // (`docs/plans/latency-compensation/04-preallocated-delays.md`).
                st.borrow_mut().session.sync_compensation(&mut handle);
                // Beside it and for the same reasons: an edge's fate is a
                // property of every channel at once, so deriving and diffing
                // once a tick cannot be forgotten the way a per-edit call site
                // can. Allocates the taps only when the plan says somebody is
                // listening.
                st.borrow_mut().session.sync_audio_graph(&mut handle);
                // And beside both, for the third time and the same reason:
                // which buses need a console accumulator is a property of
                // every strip's switch and every route at once. Allocates a
                // buffer only for the buses something encoded actually
                // reaches, so a project with console off costs nothing.
                st.borrow_mut().session.sync_console_sums(&mut handle);
                // And the solo, which is the fifth and the cheapest: a bank
                // with nothing soloed derives all false, matches what was
                // sent, and returns without a command. What a solo silences
                // is a property of the whole graph -- a soloed track's
                // feeders and its destination stay up -- so it is derived
                // here rather than worked out at the button.
                st.borrow_mut().session.sync_solo(&mut handle);
                // And the channel half of it, which is the same derivation
                // over the other address space: a bank with nothing soloed
                // derives all false and sends nothing.
                st.borrow_mut().session.sync_channel_solo(&mut handle);
                // And the track graph, which is the fourth of these and the
                // one that used to be sent from the edit that caused it.
                // Routing stopped being one `u8` per track when a send became
                // a second outgoing edge: the plan now carries a compensation
                // ring per send, which is a heap object, so it is derived and
                // installed here like the rest. **Last of the four**, so the
                // compensation a send's arrival moves has already been sent
                // for the generation this schedule belongs to.
                st.borrow_mut().session.sync_track_graph(&mut handle);
                // Which keys the control map takes from the instruments: its
                // pads, and every key while a learn gesture waits (MOO-129).
                // Derived and diffed here like the plans above, because a
                // learn arming, a binding landing, a removal, an undo, a load
                // and a port appearing all change it, and a call at each of
                // them is a call somebody forgets. Sends nothing when it has
                // not changed; a refused send is retried on the next tick.
                {
                    let state = st.borrow();
                    let claimed = state.session.claimed_notes(&state.midi_ports);
                    drop(state);
                    let _ = handle.set_claimed_notes(claimed);
                }
                if document_title_needs_refresh {
                    let Some(window) = weak.upgrade() else { return };
                    st.borrow().update_document_title(&window);
                }
                let Some(w) = weak.upgrade() else { return };
                let mut saw_nonzero = false;
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
                        // The master's level comes off `BusMeters` cell 0
                        // below, beside the mixer strip's, so this carries
                        // nothing the interface draws any more. The event
                        // stays because `engine-selftest` is built on
                        // counting it: it reports whether the *callback*
                        // produced audio, where a held cell only says the
                        // loudest it ever was.
                        EngineEvent::Metering { .. } => {}
                        EngineEvent::Xrun { count } => {
                            // Read off the event queue on the UI thread. The
                            // audio thread only ever pushes the count; it
                            // does no formatting and takes no lock.
                            xruns_this_window += count;
                        }
                        // Both are answered after the loop: acting on one
                        // needs the session *and* the engine handle, and the
                        // handle is borrowed for the drain.
                        EngineEvent::ControlInput(message) => control_input.push(message),
                        EngineEvent::RecordedNote {
                            channel,
                            pattern,
                            note,
                            velocity,
                            start_tick,
                            length_ticks,
                        } => recorded
                            .push((channel, pattern, note, velocity, start_tick, length_ticks)),
                        EngineEvent::ProjectInstalled { .. } => {
                            unreachable!("EngineHandle filters project acknowledgements")
                        }
                    }
                }
                if !control_input.is_empty() || !recorded.is_empty() {
                    let playing = w.get_playing();
                    let drain = drain_control_surface(
                        &st,
                        &commands,
                        &w,
                        &mut control_input,
                        &mut recorded,
                        playing,
                    );
                    // A refused command is a command the session has already
                    // recorded as delivered -- the seam `control-plane-seams`
                    // closed everywhere else. Counted across the whole drain
                    // and reported once: a fader sweep is a hundred messages a
                    // second, and a line per message would bury the log it is
                    // meant to warn in.
                    let refused = drain
                        .commands
                        .iter()
                        .filter(|command| !handle.send(**command))
                        .count();
                    if drain.controller_moved {
                        controller_moved_at = Some(std::time::Instant::now());
                    }
                    if let Some(message) = &drain.note_refusal {
                        w.set_status_message(message.as_str().into());
                    }
                    if refused > 0 {
                        log_error!(
                            "midi",
                            "the command queue refused {refused} command(s) from a \
                             control surface: the model has moved where the engine \
                             has not"
                        );
                    }
                    // A binding landing leaves the arm on, so a desk is
                    // mapped knob after knob without reaching for the toolbar
                    // between each. The status line is what says the last one
                    // took.
                    if let Some((source, target)) = drain.learned.last() {
                        w.set_status_message(
                            format!("{source} now moves {target}").as_str().into(),
                        );
                        st.borrow().refresh_midi_mappings(&w);
                    }
                    // One republish for the whole drain rather than one per
                    // message: a fader sweep is a hundred messages a second,
                    // and redrawing the editor for each would cost more than
                    // the sweep.
                    if drain.moved {
                        let state = st.borrow();
                        state.refresh_editor(&w);
                        state.sync_row_flags();
                    }
                    if !drain.written.is_empty() {
                        let state = st.borrow();
                        let mut written = drain.written;
                        written.sort_unstable();
                        written.dedup();
                        for channel in &written {
                            state.refresh_rack_row(*channel);
                        }
                    }
                    if drain.edited {
                        st.borrow().update_document_title(&w);
                    }
                }
                // Every tick, not only a tick with input: a stream closes
                // because its input *stopped*.
                settle_edit_streams(
                    &st,
                    &commands,
                    &w,
                    w.get_playing(),
                    controller_moved_at
                        .is_none_or(|moved| moved.elapsed() >= CONTROLLER_IDLE),
                );
                let now = std::time::Instant::now();
                let elapsed = now.duration_since(last_meter_update).as_secs_f32();
                last_meter_update = now;
                // Once a second: what the audio callback actually cost, and
                // whether it was given the thread it needs. An xrun is the
                // last symptom rather than the first, and on its own it does
                // not say which of the two faults produced it -- a block that
                // took too long, or a block that was never run in time.
                // Reported together so the difference is legible without
                // guessing, and only when there is something to say.
                // The port list, once a second. A keyboard plugged in
                // mid-session is a channel whose stored port name resolves for
                // the first time and a binding that stops being inert, so the
                // routing and the control map are both rebuilt when it moves
                // -- and only when it moves, because neither is free.
                if now.duration_since(last_port_scan) >= std::time::Duration::from_secs(1) {
                    last_port_scan = now;
                    let ports = handle.midi_ports();
                    // The round trip moves with the buffer size, so it is read
                    // again at the same cadence as the ports.
                    st.borrow_mut().input_latency_frames = handle.input_latency_frames();
                    let changed = st.borrow().midi_ports != ports;
                    if changed {
                        let mut state = st.borrow_mut();
                        state.midi_ports = ports;
                        let ports = state.midi_ports.clone();
                        state.session.resolve_control_map(&ports);
                        if !handle.set_midi_routing(state.session.midi_routing(&ports)) {
                            log_error!("ui", "the command queue refused the MIDI routing");
                        }
                        drop(state);
                        st.borrow().refresh_editor(&w);
                        // The mapping page marks bindings whose controller is
                        // not plugged in, and that is exactly what just
                        // changed. Only while the page is open: rebuilding
                        // four models for a dialog nobody is looking at is
                        // the kind of once-a-second cost that adds up.
                        if w.get_preferences_open() {
                            st.borrow().refresh_midi_mappings(&w);
                        }
                    }
                }
                if now.duration_since(last_load_report) >= std::time::Duration::from_secs(1) {
                    last_load_report = now;
                    let load = handle.take_load();
                    // Once, not once a second: this cannot change without a
                    // new callback thread, and a warning that repeats forever
                    // is one that gets scrolled past.
                    if !reported_time_shared
                        && load.realtime == mooloop_engine::load::RealtimeStatus::TimeShared
                    {
                        reported_time_shared = true;
                        log_warn!(
                            "audio",
                            "the audio callback is running on an ordinary time-shared thread, \
                             not a realtime one; audio will drop out whenever the machine is \
                             busy no matter how light the project is. Under PipeWire, \
                             `systemctl --user restart pipewire pipewire-pulse wireplumber` \
                             asks for realtime scheduling again, and putting your user in the \
                             `pipewire` group makes the grant survive a busy machine"
                        );
                    }
                    if load.blocks > 0 && (load.had_trouble() || xruns_this_window > 0) {
                        log_warn!(
                            "audio",
                            "audio dropout in the last second: {} of {} blocks over budget, \
                             {} late wake-ups, {} xruns reported \
                             (load {:.0}% mean, {:.0}% worst block, \
                             worst wake-up {:.1}x the block period)",
                            load.over_budget,
                            load.blocks,
                            load.late_wakeups,
                            xruns_this_window,
                            load.mean_load * 100.0,
                            load.peak_load * 100.0,
                            load.peak_period
                        );
                    }
                    xruns_this_window = 0;
                }
                // Bus peaks come from the shared atomic array, not the event
                // ring. Always drain them, even while the mixer is hidden, so
                // a strip does not open showing a peak from minutes ago; only
                // write the models when something is actually displaying them.
                let showing_mixer = w.get_showing_mixer();
                let showing_device_rack = w.get_showing_devices();
                let editing_bus = w.get_editing_bus();
                let edited_bus = w.get_editing_bus_index().max(0) as usize;
                let selected_channel = st.borrow().session.selected;
                // A MIDI keyboard plays the channel the editor is on. Set
                // every tick rather than on each selection change, because the
                // selection moves from clicks, keys, loads, deletes and undo,
                // and an atomic store is cheaper than finding all of them.
                handle.set_keyboard_channel(u8::try_from(selected_channel).ok());
                // The strips' gain-reduction lamps, as one model rather than
                // a field on every row: this is the only thing about a strip
                // that moves at frame rate, and both faces index the same
                // array so the mixer's lamp and the rack row's curve cannot
                // disagree. Drained every tick whether or not anything is
                // drawing it, for the reason the peaks are -- a held cell
                // nobody read would light the lamp with a minute-old
                // transient the moment a strip was turned to.
                let mut reduction = Vec::with_capacity(MAX_BUSES);
                for bus in 0..MAX_BUSES {
                    reduction.push(handle.take_strip_reduction(bus));
                }
                if showing_mixer || showing_device_rack {
                    w.global::<StripMeters>()
                        .set_reduction_db(reduction.as_slice().into());
                }
                // The master is always track 0, so its meter is never
                // reading somebody else's audio and its latch is never
                // inherited. Every other index can have moved.
                // Bound before the `if` rather than written into its
                // condition: a temporary in an `if` condition lives until the
                // end of the whole statement, so the `RefMut` would still be
                // held inside the block, and the block below is one edit away
                // from touching `st` again.
                // Slot numbers may have moved under the engine's spectrum
                // subscriptions since the last tick. Cheap and idempotent, so
                // it rides the same once-a-tick handoff as the meters rather
                // than being called from each of the nineteen places that
                // re-sync the rack.
                let rack_may_have_moved = st.borrow().effect_spectra_stale.replace(false);
                if rack_may_have_moved {
                    sync_effect_spectrum_subscriptions(&st.borrow(), &handle);
                }
                let bank_may_have_moved = {
                    let mut state = st.borrow_mut();
                    std::mem::replace(&mut state.bus_meters_stale, false)
                };
                if bank_may_have_moved {
                    for meters in bus_meters.iter_mut().skip(1) {
                        meters.0.reset();
                        meters.1.reset();
                    }
                }
                let master_clip_cleared = master_clip_clear_in.replace(false);
                // Read once per tick rather than per meter: it is one
                // preference, every meter falls at it, and a strip that got a
                // different rate from its neighbour would be the thing this
                // whole indirection exists to prevent.
                let falloff = meter::falloff_db_per_second(w.global::<MeterPrefs>().get_falloff());
                // The hardware input's meter, beside the AUDIO row. Read every
                // tick so the cell does not hold a peak from before the row was
                // shown; one reading, the louder side, because the row is one
                // bar wide.
                {
                    let (peak_l, peak_r) = handle.take_input_peak();
                    let left = input_meter.0.update(peak_l, elapsed, falloff);
                    let right = input_meter.1.update(peak_r, elapsed, falloff);
                    w.set_audio_input_level_db(left.level_db.max(right.level_db));
                    w.set_audio_input_held_db(left.held_db.max(right.held_db));
                }
                for (bus, meters) in bus_meters.iter_mut().enumerate() {
                    let strip_clip_cleared = bus_clip_clear_in
                        .borrow_mut()
                        .get_mut(bus)
                        .map(|flag| std::mem::replace(flag, false))
                        .unwrap_or(false);
                    // The master has two lamps on two faces and one latch
                    // behind them now, so either click clears it. That is the
                    // half of being metered twice a user could actually see.
                    let is_master = bus == MASTER_BUS as usize;
                    if strip_clip_cleared || (is_master && master_clip_cleared) {
                        meters.0.clear_clip();
                        meters.1.clear_clip();
                    }
                    let (peak_l, peak_r) = handle.take_bus_peak(bus);
                    let left = meters.0.update(peak_l, elapsed, falloff);
                    let right = meters.1.update(peak_r, elapsed, falloff);
                    if is_master {
                        w.set_meter_l_db(left.level_db);
                        w.set_meter_r_db(right.level_db);
                        w.set_meter_l_held_db(left.held_db);
                        w.set_meter_r_held_db(right.held_db);
                        w.set_meter_l_clipping(left.clipping);
                        w.set_meter_r_clipping(right.clipping);
                        if peak_l > 0.0 || peak_r > 0.0 {
                            saw_nonzero = true;
                        }
                    }
                    if showing_mixer {
                        let strips = st.borrow();
                        if let Some(mut row) = strips.mixer_strip_model.row_data(bus) {
                            // The held level and the clip latch are stepped
                            // changes rather than a continuous level, so they
                            // get their own reasons to repaint: throttling
                            // them behind the level's own quantiser is how a
                            // peak marker comes to sit one segment behind
                            // where the audio put it.
                            let clipping = left.clipping || right.clipping;
                            if meter_display_changed(row.left_db, left.level_db)
                                || meter_display_changed(row.right_db, right.level_db)
                                || meter_display_changed(row.held_left_db, left.held_db)
                                || meter_display_changed(row.held_right_db, right.held_db)
                                || row.clipping != clipping
                            {
                                row.left_db = left.level_db;
                                row.right_db = right.level_db;
                                row.held_left_db = left.held_db;
                                row.held_right_db = right.held_db;
                                row.clipping = clipping;
                                strips.mixer_strip_model.set_row_data(bus, row);
                            }
                        }
                    }
                    if editing_bus && bus == edited_bus {
                        w.set_editing_bus_left_db(left.level_db);
                        w.set_editing_bus_right_db(right.level_db);
                        // The hold and the latch come off the same reading and
                        // were being dropped here, so the fader row's meter
                        // held nothing and its clip lamp could not light --
                        // one track, two faces, two behaviours.
                        w.set_editing_bus_held_left_db(left.held_db);
                        w.set_editing_bus_held_right_db(right.held_db);
                        w.set_editing_bus_clipping(left.clipping || right.clipping);
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
                // Empty whatever the rack has just moved off, once, rather
                // than draining every target every tick -- which is
                // `(MAX_CHANNELS + MAX_BUSES) x (MAX_EFFECTS + 1) x 6` atomic
                // swaps at 125 Hz, the cost the spectrum pool exists to
                // avoid in the analogous case.
                if last_device_target != Some(device_target) {
                    if let Some(left) = last_device_target {
                        handle.clear_device_meters(left);
                    }
                    last_device_target = Some(device_target);
                }
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
                        // Drained whether or not anything is drawing it, the
                        // same argument the bus-strip reduction above makes:
                        // these are `fetch_max` holds, so a cell nobody read
                        // lights the lamp with a minutes-old transient the
                        // moment the rack is opened. It used to sit inside
                        // the `showing_device_rack` arm below.
                        let (detector, reduction_db) =
                            handle.take_device_dynamics(device_target, slot + 1);
                        if showing_device_rack {
                            if let Some(mut row) = state.effect_slot_model.row_data(slot) {
                                let input_left_db = linear_to_db(in_l);
                                let input_right_db = linear_to_db(in_r);
                                let output_left_db = linear_to_db(out_l);
                                let output_right_db = linear_to_db(out_r);
                                let meter_changed = meter_display_changed(row.input_left_db, input_left_db)
                                    || meter_display_changed(row.input_right_db, input_right_db)
                                    || meter_display_changed(row.output_left_db, output_left_db)
                                    || meter_display_changed(row.output_right_db, output_right_db);
                                // Non-dynamics stages never publish here, so
                                // they read the resting pair and need no
                                // check for what kind of device they hold.
                                // Taken above, unconditionally.
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
                                // The same stage, carrying a different
                                // meaning: the preamp publishes what it did
                                // to the signal rather than what arrived.
                                if row.preamp_display_enabled {
                                    let deviation = handle.effect_spectrum(state.session.effect_target, slot as u8);
                                    row.preamp_deviation = deviation.as_slice().into();
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
                                // The Buffer's picture: peaks while something
                                // is looking, marks always. Subscribing is
                                // idempotent and one atomic load, so it is
                                // re-asserted rather than tracked -- which is
                                // also what re-subscribes after a graph
                                // replacement clears the bank.
                                let buffer_drawn = row.kind == effect_kind_index(EffectKind::Buffer);
                                if buffer_drawn {
                                    let target = state.session.effect_target;
                                    handle.set_buffer_waveform_enabled(target, slot as u8, true);
                                    let peaks = handle.effect_buffer_waveform(target, slot as u8);
                                    let marks = handle.effect_buffer_marks(target, slot as u8);
                                    row.buffer_peaks = peaks.as_slice().into();
                                    row.buffer_head = marks.head.unwrap_or(-1.0);
                                    row.buffer_write = marks.write;
                                    let (start, end) = marks.region.unwrap_or((-1.0, -1.0));
                                    row.buffer_window_start = start;
                                    row.buffer_window_end = end;
                                    row.buffer_frozen = marks.frozen;
                                    row.buffer_armed_freeze = match marks.armed_freeze {
                                        Some(true) => 1,
                                        Some(false) => 2,
                                        None => 0,
                                    };
                                    row.buffer_armed_gesture = marks.armed_gesture;
                                    let bars = row.buffer_history_bars.max(1) as f64;
                                    let ppq = mooloop_core::Ppq::DEFAULT;
                                    let history_ticks = bars
                                        * f64::from(mooloop_core::BEATS_PER_BAR)
                                        * f64::from(ppq.ticks_per_beat());
                                    let head = f64::from(marks.head.unwrap_or(0.0).max(0.0));
                                    let at = mooloop_core::BbtPosition::from_ticks(
                                        mooloop_core::Ticks((head * history_ticks) as u64),
                                        ppq,
                                    );
                                    row.buffer_position_bar = at.bar as i32;
                                    row.buffer_position_beat = at.beat as i32;
                                    row.buffer_position_tick = at.tick as i32;
                                }
                                if meter_changed
                                    || dynamics_changed
                                    || collisions_changed
                                    || buffer_drawn
                                    || row.eq_analyzer_enabled
                                    || row.preamp_display_enabled
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
                w.invoke_show_view(view_id(Pane::Mixer));
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
                w.invoke_show_view(view_id(Pane::Source));
                w.set_song_mode(true);
                w.invoke_playback_mode_changed(true);
                w.invoke_show_view(view_id(Pane::Playlist));
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

        // --- Optional recording self-test (MOOLOOP_AUTODRIVE_RECORD=1) ---
        // `audio-recording/05`'s acceptance, driven through the real
        // callbacks with the engine running, because the AUDIO row's menu is
        // a popup the MCP tools cannot click (`OPERATIONS.md`). A kick plays,
        // a new sampler takes the master as its AUDIO input and records one
        // bar with Clip on; the report says whether the page showed the
        // pre-roll and the take, and whether the take became the sampler's
        // sample as one undo step. Run it with `MOOLOOP_CONFIG_DIR` pointed
        // somewhere disposable: the take is written to its recordings folder.
        if let Ok(which) = std::env::var("MOOLOOP_AUTODRIVE_RECORD") {
            // `input` records the hardware input instead of the master --
            // `audio-recording/01`'s check, with whatever the capture ports
            // hear.
            let wanted = if which == "input" {
                mooloop_core::AudioInputSource::Input
            } else {
                mooloop_core::AudioInputSource::Master
            };
            let seen = Rc::new(Cell::new((false, false, 0usize)));
            {
                let weak = window.as_weak();
                let st = state.clone();
                slint::Timer::single_shot(std::time::Duration::from_millis(300), move || {
                    let Some(w) = weak.upgrade() else { return };
                    for step in [0, 4, 8, 12] {
                        w.invoke_step_clicked(0, step);
                    }
                    w.invoke_add_channel_clicked(0);
                    // The row is looked up rather than assumed: whether the
                    // input row exists depends on the driver.
                    let row = {
                        let st = st.borrow();
                        let rows = st.session.audio_source_rows(st.audio_input_label.as_deref());
                        rows.iter().position(|row| row.source == wanted).unwrap_or(0)
                    };
                    println!("recording from AUDIO row {row} ({wanted:?})");
                    w.invoke_audio_input_picked(row as i32);
                    w.invoke_sampler_record_clip_changed(true);
                    w.invoke_sampler_record_bars_changed(1);
                    w.invoke_sampler_record_clicked();
                });
            }
            {
                let weak = window.as_weak();
                let seen = seen.clone();
                let watch = Box::leak(Box::new(Timer::default()));
                watch.start(
                    TimerMode::Repeated,
                    std::time::Duration::from_millis(40),
                    move || {
                        let Some(w) = weak.upgrade() else { return };
                        let (mut waited, mut recorded, mut peaks) = seen.get();
                        match RecordFace::from_i32(w.get_sampler_record_state()) {
                            Some(RecordFace::Waiting) => waited = true,
                            Some(RecordFace::Recording) => {
                                recorded = true;
                                peaks = peaks.max(w.get_sampler_record_peaks().row_count());
                            }
                            Some(RecordFace::Idle) | None => {}
                        }
                        seen.set((waited, recorded, peaks));
                    },
                );
            }
            {
                let st = state.clone();
                let commands = command_state.clone();
                slint::Timer::single_shot(std::time::Duration::from_millis(9000), move || {
                    let (waited, recorded, peaks) = seen.get();
                    let st = st.borrow();
                    let channel = st.session.channels.get(st.session.selected);
                    let path = channel.and_then(|channel| channel.sample_path.clone());
                    let in_recordings = path
                        .as_ref()
                        .is_some_and(|path| path.starts_with(settings::recordings_dir()));
                    let embedded = channel.is_some_and(|channel| channel.sample_embedded);
                    let label = commands
                        .borrow()
                        .history
                        .undo_target()
                        .map(|entry| entry.label)
                        .unwrap_or("");
                    println!("--- ui record autodrive report ---");
                    println!("page showed the pre-roll   : {waited}");
                    println!("page showed the take       : {recorded} ({peaks} bars drawn)");
                    println!("sampler's sample           : {path:?}");
                    println!("  from the recordings folder: {in_recordings}");
                    println!("  owned by the song         : {embedded}");
                    println!("last undo entry            : {label:?}");
                    let ok = waited
                        && recorded
                        && peaks > 0
                        && in_recordings
                        && embedded
                        && label == "Record Take";
                    println!("RESULT: {}", if ok { "PASS" } else { "FAIL" });
                    slint::quit_event_loop().ok();
                });
            }
        }

        Ok(AppUi {
            window,
            _pump: pump,
            state,
        })
    }

    pub fn show(&self) -> Result<(), slint::PlatformError> {
        self.window.show()
    }

    pub fn run(&self) -> Result<(), slint::PlatformError> {
        self.window.run()
    }

    /// End every take still recording and write its file out. Call once the
    /// event loop has returned, before the process ends.
    ///
    /// `hound` patches the real frame count into the WAV header when the
    /// drain finalizes, at the end of the drain and nowhere else, so a take
    /// whose drain is still running when the process exits leaves a file that
    /// says it holds nothing. Nothing else on the quit path knows a take is
    /// there: it is runtime state, not document state, so `Session::dirty` is
    /// false for it.
    ///
    /// [`mooloop_session::take::TakeRecorder`] does the same thing from its
    /// `Drop`, for the routes that never reach here. This is the one that can
    /// still say something about what happened.
    pub fn finish_takes(&self) {
        let (finished, failures) = self.state.borrow_mut().takes.finish_all();
        for take in &finished {
            log_info!(
                "ui",
                "finished a take at quit: {} ({} frames)",
                take.path.display(),
                take.frames
            );
        }
        for failure in &failures {
            log_error!("ui", "a take could not be finished at quit: {failure}");
        }
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
    keep_transport: bool,
) -> bool {
    let mut project = project.clone();
    normalize_project_pattern_banks(&mut project);
    // The bank is composed **before** anything is queued, and travels with
    // the project as one command.
    //
    // It used to be sixteen `ArcSwap` stores made *after* the install was
    // queued, into a bank every generation shared -- so for the block or two
    // before the audio thread consumed the install, the outgoing project's
    // graph was reading the incoming project's samples, and mid-loop, a
    // half-replaced set of them. Composing first means the existing early
    // return covers the assets too, with no second bail-out path.
    let audio: Vec<ChannelAudioSnapshot> = (0..MAX_CHANNELS)
        .map(|index| {
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
            // The markers come from the project being installed, in the same
            // pass. Published separately they were a second write of half of
            // one fact, and the half that arrived first indexed the other
            // half's buffer.
            let slices = project
                .channels
                .get(index)
                .and_then(|channel| channel.setup.source.sampler_state())
                .map(|state| state.slices.clone())
                .filter(|slices| !slices.is_empty())
                .map(Arc::new);
            ChannelAudioSnapshot { sample, slices }
        })
        .collect();
    // If the bounded realtime queue is full, leave the sample bank, the
    // engine and the visible project untouched.
    // What the session set by command rather than by document goes with the
    // install, because the renderer it replaces takes that state with it. The
    // routing is resolved from the *incoming* project: every structural edit,
    // paste, undo and load comes through here, and any of them can renumber
    // the channels the routing is indexed by.
    let input = {
        let state = state.borrow();
        mooloop_engine::InputState {
            record_armed: state.session.record_armed(),
            midi_routing: Session::project_midi_routing(
                &project,
                &state.midi_ports,
            ),
            audio_input: Session::project_audio_input_taps(&project),
            monitor: state.session.monitor_seats(&project),
        }
    };
    if !handle.install_project(Arc::new(project.clone()), audio, input, keep_transport) {
        return false;
    }
    // A project install is the only thing that can change which track a strip
    // index names -- a removal shifts every later one down, and an undo of one
    // shifts them back. The per-bus ballistics are keyed by that index, so
    // from here they are about a track that may not be the one they were
    // reading. The pump resets them on its next tick.
    state.borrow_mut().bus_meters_stale = true;
    state.borrow_mut().replace_project(&project, samples, window);
    if !keep_transport {
        // `input_monitor`'s own doc comment promises "off for every channel
        // of a song that has just opened" -- `replace_project` only prunes
        // ids the incoming document does not have, so a load whose channel
        // ids happen to coincide with the outgoing document's (every fresh
        // project starts back at `ChannelId(0)`) would otherwise inherit a
        // stale monitor toggle instead of starting unarmed. `keep_transport`
        // is false exactly for a load and true for every in-song edit, which
        // is what must not lose a channel's toggle on a move.
        state.borrow_mut().session.input_monitor.clear();
    }
    // **Still needed, and now only where it says something the bank could
    // not.** `samples` carries a project's *sources*; `replace_project`
    // re-renders any committed stretch, and a channel with a commit plays
    // that render. The re-render happens on the line above and nowhere
    // earlier, so this is the first moment that buffer exists.
    //
    // It is also no longer a race. `install_project` left the handle
    // addressing the bank it just prepared, so these stores land in the
    // incoming generation's own slots and are read the instant it goes live
    // -- where before they landed in a bank the outgoing generation was still
    // playing from.
    //
    // **The `commit` guard is a fix, not a shortcut.** This ran over every
    // sampler channel, and `published_sample()` is
    // `committed_sample.or(sample_data)` where `replace_project` fills
    // `sample_data` from `samples` alone -- it does not apply the legacy
    // `SampleReference::Builtin` substitution the bank above does. So opening
    // a project old enough to carry a `Builtin` reference published the
    // default kick and then immediately cleared it, leaving the channel
    // silent while its name, waveform and duration all described a kick. A
    // channel with no commit has nothing to add here by construction.
    {
        let st = state.borrow();
        for (index, channel) in st.session.channels.iter().enumerate() {
            if channel.kind == DeviceKind::Sampler && channel.commit.is_some() {
                publish_channel_audio(handle, index, channel);
            }
        }
    }
    sync_effect_spectrum_subscriptions(&state.borrow(), handle);
    // An **edit** leaves the transport alone; the engine carries it across the
    // swap and these two would only make the interface disagree with what is
    // audibly still playing. An **open** stops and rewinds, which is what
    // opening a document means.
    if !keep_transport {
        window.set_playing(false);
        window.set_playlist_position_ticks(0);
    }
    // A new project brings its own control map, so a learn gesture waiting on
    // the old one has nothing left to bind to -- `Session::load` has already
    // dropped it. The arm goes with it rather than staying lit over a gesture
    // that is no longer pending.
    {
        let mut st = state.borrow_mut();
        st.midi_learn_armed = false;
        let ports = st.midi_ports.clone();
        st.session.resolve_control_map(&ports);
    }
    set_midi_learn_armed(window, false);
    state.borrow().refresh_midi_mappings(window);
    refresh_preset_menus(state, window);
    true
}

/// Tell the engine exactly which stages should be publishing a spectrum.
///
/// **It must say `false` as well as `true`, and that is the whole fix.** The
/// version this replaced walked the devices that *are* analyzers and enabled
/// or disabled each one, so a stage whose analyzer had moved away -- or been
/// deleted, or wrapped into a container -- was never visited and kept its
/// subscription. The engine went on running a Goertzel bank every hop for a
/// display nobody was drawing, the device that had taken that slot number
/// drew a flat line behind a lit button, and the orphan held one of the
/// sixty-four `SPECTRUM_SLOTS` until the project was reloaded.
///
/// So this walks *stages*, not devices, and states the answer for each.
/// Re-stating a subscription that is already correct is a relaxed load and an
/// early return in `DeviceTelemetry::set_spectrum_enabled`, so the common
/// case costs nothing and -- importantly -- does not clear the bins, which is
/// what would make every open analyzer blink on an unrelated device drag.
///
/// **One past the end of each chain is enough, and only because of an
/// invariant.** No stage above a chain's length can be subscribed when this
/// returns, so the only stage that can be stale next time is the one a single
/// removal vacates. A project install is the one edit that can shorten a
/// chain by more than that, and `DeviceTelemetry::clear_spectra` runs there.
fn sync_effect_spectrum_subscriptions(state: &UiState, handle: &EngineHandle) {
    let sync = |target: EffectTarget, effects: &[mooloop_core::EffectSlotState]| {
        for (slot, enabled) in spectrum_subscription_plan(effects) {
            handle.set_effect_spectrum_enabled(target, slot, enabled);
        }
    };

    for (channel, setup) in state.session.channels.iter().enumerate() {
        sync(EffectTarget::Channel(channel as u8), &setup.effects);
    }
    for (bus, setup) in state.session.buses.iter().enumerate() {
        sync(EffectTarget::Bus(bus as u8), &setup.effects);
    }
}

/// What every stage of one chain should be publishing, the vacated tail
/// included.
///
/// Split out from the call above so the part that decides can be read back:
/// the whole defect was a walk that only ever said `true`, and the only way
/// to see that a walk says `false` where it should is to look at what it
/// says.
fn spectrum_subscription_plan(
    effects: &[mooloop_core::EffectSlotState],
) -> impl Iterator<Item = (u8, bool)> + '_ {
    /// Whether this slot holds a device that is asking to be analyzed.
    fn wanted(effect: Option<&mooloop_core::EffectSlotState>) -> bool {
        let Some(effect) = effect else {
            return false;
        };
        if let Some(eq) = effect.params.eq() {
            return eq.analyzer_enabled;
        }
        if let Some(preamp) = effect.params.preamp() {
            return preamp.display_enabled;
        }
        false
    }

    let past_the_end = effects.len().min(mooloop_core::MAX_EFFECTS_PER_CHANNEL - 1);
    (0..=past_the_end).map(move |slot| (slot as u8, wanted(effects.get(slot))))
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
        // The browser's own catalogue, which `refresh_browser` below renders
        // from. It was rescanned only on entering the PRESETS tab, so a
        // preset saved *while that tab was open* did not appear in it -- the
        // rail menus updated and the browser re-rendered from the stale
        // catalogue. The comment at the save site claimed this landed "by way
        // of the tab being re-entered", which only happens if the user
        // actually leaves and comes back.
        if st.browser_tab == BrowserTab::Presets {
            st.preset_catalog = scan_preset_catalog();
        }
    }
    let st = state.borrow();
    st.sync_generator_preset_menu(window);
    st.sync_channel_preset_menu(window);
    st.sync_effects();
    // Whether a generator preset is loadable depends on the selected
    // channel's device, so the browser's rows go stale on exactly the
    // switches this function already exists to catch. Cheap: it rebuilds a
    // row list from a catalogue that is already in memory, and does nothing
    // at all while the SAMPLES tab is showing.
    refresh_browser(&st);
}


/// Hand one channel's audio and slice map to the engine.
///
/// The *published* buffer, not the source: after a commit the engine plays
/// the render. Both travel out of band through `ArcSwap` slots rather than on
/// the command ring, so this is wait-free and safe to call from the UI thread.
fn publish_channel_audio(handle: &EngineHandle, index: usize, channel: &ChannelState) {
    handle.set_channel_audio(
        index,
        ChannelAudioSnapshot {
            sample: channel.published_sample().cloned(),
            slices: (!channel.slices.is_empty()).then(|| Arc::new(channel.slices.clone())),
        },
    );
}

/// Publish a finished background load to `channel`: hand the decoded sample
/// to the engine, record it on the channel state, and refresh the visible
/// editor when that channel is the one on screen.
///
/// Records nothing. A load from the browser or the file dialog goes through
/// [`load_sample_with_history`] and a take through [`apply_take`], and both
/// record around this.
fn apply_loaded_sample(
    handle: &EngineHandle,
    st: &Rc<RefCell<UiState>>,
    weak: &slint::Weak<MainWindow>,
    channel: usize,
    loaded: LoadedSample,
) {
    adopt_loaded_sample(
        |channel, audio| handle.set_channel_audio(channel, audio),
        st,
        weak,
        channel,
        loaded,
    );
}

/// A sample loaded from the browser or the file dialog, as one undo step
/// (MOO-98).
///
/// It was applied and marked dirty with nothing recorded, and a load is not a
/// small edit: it retires the channel's slices and its stretch commit, which
/// named frames in the old audio. The next Ctrl+Z then installed a snapshot
/// from before some earlier edit -- reverting the load with no redo, and with
/// the old slices unrecoverable. Recorded the way [`apply_take`] records a
/// take, so undo brings back the old sample *and* its slices, and redo the
/// new one.
///
/// `publish` hands the audio to the engine; the pump passes the handle's
/// store, and a test passes nothing, because the engine handle needs a live
/// driver.
fn load_sample_with_history(
    publish: impl FnOnce(usize, ChannelAudioSnapshot),
    st: &Rc<RefCell<UiState>>,
    commands: &Rc<RefCell<CommandState>>,
    weak: &slint::Weak<MainWindow>,
    channel: usize,
    loaded: LoadedSample,
) {
    let Some(window) = weak.upgrade() else {
        return;
    };
    let before = project_snapshot(&st.borrow(), &window);
    adopt_loaded_sample(publish, st, weak, channel, loaded);
    record_project_history(commands, before, st, &window, "Load sample");
}

/// [`apply_loaded_sample`] with the engine store passed in.
fn adopt_loaded_sample(
    publish: impl FnOnce(usize, ChannelAudioSnapshot),
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
    // The markers went with the old file, so the snapshot carries none: the
    // engine must not keep playing a map that names frames in audio it no
    // longer holds, and now it cannot, because the buffer and the map arrive
    // as one store.
    publish(channel, ChannelAudioSnapshot::sample(loaded.sample.clone()));
    let mut st = st.borrow_mut();
    if let Some(ch) = st.session.channels.get_mut(channel) {
        ch.sample_name = name;
        ch.sample_description = description;
        ch.sample_duration = duration;
        ch.sample_path = Some(loaded.path.clone());
        // Where it came from, which is what "next sample" walks. A load from
        // the browser or the file dialog is the only thing that sets this;
        // the save's write-back deliberately does not.
        ch.sample_browse_path = Some(loaded.path);
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

/// How many bars the RECORD page draws a take with, at most.
const RECORD_PAGE_BARS: usize = 256;

/// A finished take, decoded and on its way to its channel.
struct TakeLoad {
    take: FinishedTake,
    result: Result<LoadedSample, String>,
}

/// **A finished take becomes its channel's sample** (`audio-recording/04`).
///
/// Found by identity, so a take lands on the channel that recorded it however
/// the bank was edited while it ran, and on that channel whether or not it is
/// the one on screen. Nothing is written into any pattern: the take is heard
/// through whatever already triggers the sampler (decision 2's answer).
///
/// **One undo step**, which an ordinary sample load is not -- a take
/// overwrites what was there, so it is recorded around the same
/// `apply_loaded_sample` a dragged-in file uses. And the sample is marked as
/// the song's own, so a save copies it out of the shared recordings folder
/// into the bundle whatever the save mode.
fn apply_take(
    handle: &EngineHandle,
    st: &Rc<RefCell<UiState>>,
    weak: &slint::Weak<MainWindow>,
    commands: &Rc<RefCell<CommandState>>,
    load: TakeLoad,
) {
    let Some(window) = weak.upgrade() else {
        return;
    };
    let loaded = match load.result {
        Ok(loaded) => loaded,
        Err(error) => {
            log_error!("ui", "a take could not be loaded: {error}");
            window.set_status_message(format!("The take could not be loaded: {error}").into());
            return;
        }
    };
    // The session owns this rule: a take has to land on a channel that still
    // exists *and* still holds a sampler. Writing it onto a channel whose
    // device changed mid-take gave the channel sample state that
    // `reset_channel_source` had already cleared -- a sample the face cannot
    // draw and `project_snapshot` will not save, behind a "Record Take" undo
    // entry that restores nothing visible.
    // `reports/fable-2026-09-21.md` finding 3 reported the same edge and fixed
    // it inline here; the check lives in the session instead, so the rule has
    // one home and one wording.
    let channel = match st.borrow().session.take_target(load.take.channel) {
        Ok(seat) => seat,
        Err(miss) => {
            window.set_status_message(miss.message().into());
            return;
        }
    };
    let before = project_snapshot(&st.borrow(), &window);
    apply_loaded_sample(handle, st, weak, channel, loaded);
    if let Some(state) = st.borrow_mut().session.channels.get_mut(channel) {
        state.sample_embedded = true;
    }
    record_project_history(commands, before, st, &window, "Record Take");
    if load.take.is_damaged() {
        window.set_status_message(
            format!(
                "The take has a gap: {} frames did not reach the file",
                load.take.dropped
            )
            .into(),
        );
    }
}

/// Whether loading `target` opens a document rather than editing the one
/// that is open.
///
/// Only a song does. A kit, a channel preset and a generator preset all
/// change the song in front of the user, which decides three things
/// together (MOO-95): they keep the transport running, as every `ProjectEdit`
/// does; they are recorded as undo steps; and they leave the history alone,
/// where an open clears it because its entries are snapshots of another
/// song.
fn load_opens_document(target: &LoadTarget) -> bool {
    matches!(target, LoadTarget::Song)
}

/// The undo entry a load that edits the song is recorded under.
fn load_label(target: &LoadTarget) -> &'static str {
    match target {
        LoadTarget::Song => "Open song",
        LoadTarget::Kit => "Load kit",
        LoadTarget::Channel => "Load channel preset",
        LoadTarget::Generator { .. } => "Load preset",
    }
}

/// One decoded sample per channel of a project being installed, `None` for a
/// channel that plays none.
type LoadedSamples = Vec<Option<Arc<SampleData>>>;

/// Whether loading `document` as a kit over `current` would drop channels
/// that hold notes: a kit shorter than the song ends the channels past it.
fn kit_drops_notes(current: &Project, document: &LoadedDocument) -> bool {
    let LoadedDocument::Kit(kit) = document else {
        return false;
    };
    let kept = kit.channels.len();
    kept < current.channels.len()
        && current.channels[kept..]
            .iter()
            .any(|channel| channel.notes.iter().any(|lane| !lane.is_empty()))
}

/// The project a finished load installs, built from the song that is open.
///
/// A song replaces it; a kit replaces its channels' setups and keeps their
/// notes, ids and automation; a channel or generator preset replaces the
/// selected channel's setup or source. `Err` is the status line for a load
/// that installs nothing: a kit the user declined, or a bundle whose type is
/// not the one that was asked for.
///
/// `confirm` asks whether to go on when a kit is shorter than the song and
/// would drop channels that hold notes. The pump has already asked in the
/// app's own dialog by then (`kit_drops_notes`), so it passes `|_| true`.
fn merge_loaded_document(
    current: Project,
    current_samples: Vec<Option<Arc<SampleData>>>,
    target: LoadTarget,
    document: LoadedDocument,
    loaded_samples: Vec<Option<Arc<SampleData>>>,
    confirm: impl FnOnce(&str) -> bool,
) -> Result<(Project, LoadedSamples), &'static str> {
    let dropping_notes = kit_drops_notes(&current, &document);
    match (target, document) {
        (LoadTarget::Song, LoadedDocument::Song(project)) => Ok((project, loaded_samples)),
        (LoadTarget::Kit, LoadedDocument::Kit(kit)) => {
            if dropping_notes && !confirm("This kit removes channels containing notes. Continue?")
            {
                return Err("Kit load cancelled");
            }
            let mut project = current.clone();
            let mut next_channel_id = project.next_channel_id;
            project.channels = kit
                .channels
                .into_iter()
                .enumerate()
                .map(|(index, mut setup)| {
                    // A kit entry's routes name the channel they were saved
                    // from; they mean this one.
                    setup.rescope_modulation(index as u8);
                    if let Some(mut channel) = current.channels.get(index).cloned() {
                        channel.setup = setup;
                        channel
                    } else {
                        ProjectChannel {
                            // A kit entry past the end of the song makes a
                            // channel, and a channel that joins the bank is
                            // minted. An entry landing on a live channel
                            // keeps that channel's id above, because it
                            // changes what the channel plays rather than
                            // which one it is.
                            id: mooloop_core::mint_channel_id(&mut next_channel_id),
                            setup,
                            notes: vec![Vec::new(); current.pattern_lengths.len()],
                            automation: vec![Vec::new(); current.pattern_lengths.len()],
                            next_note_id: 1,
                        }
                    }
                })
                .collect();
            project.next_channel_id = next_channel_id;
            // A kit can be shorter than the song, so the selected channel may
            // be one of the ones it dropped.
            if project.channel_index(project.selected_channel).is_none() {
                if let Some(first) = project.channels.first().map(|c| c.id) {
                    project.selected_channel = first;
                }
            }
            let mut samples = current_samples;
            samples.resize(project.channels.len(), None);
            samples.truncate(project.channels.len());
            for (index, sample) in loaded_samples.into_iter().enumerate() {
                if let Some(slot) = samples.get_mut(index) {
                    *slot = sample;
                }
            }
            Ok((project, samples))
        }
        (LoadTarget::Channel, LoadedDocument::Channel(setup)) => {
            let mut project = current;
            let selected = project.selected_index();
            project.channels[selected].setup = *setup;
            // A saved rack still names the channel it was authored on. Point
            // it at this one, or a preset saved from channel 3 would modulate
            // channel 3 from wherever it landed.
            mooloop_project::rescope_modulation(
                &mut project.channels[selected].setup,
                selected as u8,
            );
            let mut samples = current_samples;
            samples[selected] = loaded_samples.into_iter().next().flatten();
            Ok((project, samples))
        }
        (LoadTarget::Generator { .. }, LoadedDocument::Generator(source)) => {
            let mut project = current;
            let selected = project.selected_index();
            project.channels[selected].setup.channel.kind = source.kind();
            project.channels[selected].setup.source = *source;
            let mut samples = current_samples;
            samples[selected] = loaded_samples.into_iter().next().flatten();
            Ok((project, samples))
        }
        _ => Err("Selected bundle has the wrong document type"),
    }
}

/// Everything a load does once its project is installed: the dirty flag, the
/// preset labels, and the history.
///
/// `before` is the song as it was, for a load that edits it; `None` for a
/// song, which is opened rather than recorded.
///
/// **A load that edits the song is one undo step (MOO-95).** Generator and
/// channel presets marked the song dirty and recorded nothing, so the next
/// Ctrl+Z installed a snapshot from before them and took the preset away with
/// no redo; a kit load cleared the history outright, as though a different
/// song had been opened. `CURRENT.md` said undo covered presets. Now all
/// three record here, and only opening a song clears the history.
fn finish_document_load(
    st: &Rc<RefCell<UiState>>,
    commands: &Rc<RefCell<CommandState>>,
    window: &MainWindow,
    path: &Path,
    target: &LoadTarget,
    asset_mode: AssetMode,
    before: Option<ProjectSnapshot>,
) {
    let mut state = st.borrow_mut();
    if load_opens_document(target) {
        state.session.bundle_path = Some(path.to_path_buf());
        state.session.dirty = false;
        state.session.document_generation = state.session.document_generation.wrapping_add(1);
        // The per-sample flags, not the document's mode: a bundle saved
        // `referenced` can still hold every sample, because un-embedding is
        // refused rather than performed.
        window.set_embed_assets(
            asset_mode == AssetMode::Embedded || state.session.has_embedded_samples(),
        );
    } else {
        state.session.mark_dirty();
    }
    // A preset label says where a device's settings came from, so a load
    // either sets it -- the device now wears the preset it came from, the
    // same way a rack row does -- or drops the ones it just invalidated.
    match target {
        LoadTarget::Generator { preset_name } => {
            let channel = state.session.selected as u8;
            state.session.set_source_preset_name(channel, preset_name);
            window.set_source_preset_name(preset_name.as_str().into());
        }
        // A channel preset brought its own generator, which did not come
        // from whatever the seat was wearing.
        LoadTarget::Channel => {
            let channel = state.session.selected as u8;
            state.session.set_source_preset_name(channel, "");
            window.set_source_preset_name(Default::default());
        }
        // A song or kit replaced every device in the rack, so every label
        // describes one that is no longer there.
        LoadTarget::Song | LoadTarget::Kit => {
            state.session.source_preset_names.clear();
            state.session.effect_preset_names.clear();
            window.set_source_preset_name(Default::default());
            state.sync_effects();
        }
    }
    // And a song clears the history, for the sharper version of the same
    // reason: a label that outlives its document is wrong on screen, where
    // an undo that outlives its document installs the closed song over this
    // one and saves it to this one's path. A kit does not: it edits this
    // song, and undoing it is exactly what the history is for.
    if load_opens_document(target) {
        let mut commands = commands.borrow_mut();
        commands.history.clear();
        sync_command_availability(window, &commands);
    }
    state.update_document_title(window);
    drop(state);
    if let Some(before) = before {
        record_project_history(commands, before, st, window, load_label(target));
    }
}

/// Decode a browser sample off the UI thread and deliver it to the pump as a
/// `LoadResult`. `new_channel` targets the sampler channel the pump will
/// create on arrival, so `channel` is the index it will take.
fn spawn_browser_sample_load(
    path: &str,
    channel: usize,
    source_revision: u64,
    request: u64,
    new_channel: bool,
    load_tx: &std::sync::mpsc::Sender<LoadResult>,
) {
    let path = PathBuf::from(path.to_string());
    let tx = load_tx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(LoadResult {
            channel,
            source_revision,
            request,
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
        detail: Default::default(),
        loadable: true,
    });
    if !is_expanded {
        return;
    }
    for (is_dir, child) in scan_browser_dir(path) {
        if is_dir {
            // A folder with nothing playable below it is noise, however
            // legitimately it exists on disk.
            //
            // From zero, not from `depth`: that argument is the *scan's* own
            // recursion counter, which exists to terminate a symlink cycle,
            // and `depth` here is how deep the row is drawn. Passing the
            // display depth made the scan give up early, so a folder nested
            // past sixteen rows vanished from the tree even when it was full
            // of samples.
            if has_playable_descendant(&child, 0) {
                push_browser_rows(rows, &child, depth + 1, expanded);
            }
        } else {
            rows.push(BrowserRow {
                depth: depth as i32 + 1,
                kind: 1,
                name: browser_display_name(&child).into(),
                path: child.to_string_lossy().to_string().into(),
                expanded: false,
                detail: Default::default(),
                loadable: true,
            });
        }
    }
}

/// What loading a preset from the browser lands on.
///
/// This is the whole of the browser's half of `preset-system/`'s rule that a
/// preset's unit is a device: the taxonomy on disk already says which device,
/// so the slot follows from the directory rather than from anything read out
/// of the bundle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PresetSlot {
    /// Replaces the selected channel's source device. Offered only when that
    /// channel already holds this kind -- loading a DS-01 patch onto an ML-P8
    /// would have to change what the channel *is*, which is a different
    /// gesture and belongs to the source picker.
    Generator(DeviceKind),
    /// Replaces the selected channel outright.
    Channel,
    /// **Appends** a device to the selected channel's chain, rather than
    /// replacing one.
    ///
    /// This is the browser's one real departure from the rack rail, and it is
    /// deliberate: the rail button belongs to a row and so can only mean
    /// "make this row sound like that", while the browser belongs to no row.
    /// Appending is also the gesture that makes an effect preset worth
    /// browsing -- it is how you audition a reverb you do not already have.
    /// It is why an effect preset is always loadable and a generator preset
    /// is not.
    Effect(EffectKind),
}

/// One group of presets in the browser's PRESETS tab.
struct PresetGroup {
    /// The directory the presets came from, which doubles as the group row's
    /// identity for expansion -- which is how the sample tree's
    /// `browser_expanded` set carries preset groups without knowing what a
    /// preset is.
    dir: PathBuf,
    label: String,
    slot: PresetSlot,
    presets: Vec<PresetSummary>,
}

/// Reads every well-known preset directory.
///
/// `refresh_preset_menus` scans three of these for the rail menus and keeps
/// only the selected channel's generator kind; the browser wants all of them,
/// which is the whole difference between a menu and a browser. The cost
/// argument that function records still holds -- these are small TOML
/// manifests, and there are ninety-nine of them on a seeded machine.
fn scan_preset_catalog() -> Vec<PresetGroup> {
    let mut groups = Vec::new();

    let dir = settings::channel_presets_dir();
    let presets = mooloop_project::list_presets(&dir);
    if !presets.is_empty() {
        groups.push(PresetGroup {
            dir,
            label: "Channels".to_string(),
            slot: PresetSlot::Channel,
            presets,
        });
    }

    for kind in SOURCE_KINDS_IN_PICKER_ORDER {
        let dir = settings::generator_presets_dir(kind);
        let presets = mooloop_project::list_presets(&dir);
        if !presets.is_empty() {
            groups.push(PresetGroup {
                dir,
                label: device_kind_label(kind).to_string(),
                slot: PresetSlot::Generator(kind),
                presets,
            });
        }
    }

    for kind in EffectKind::ALL {
        let dir = settings::effect_presets_dir(kind);
        let presets = mooloop_project::list_presets(&dir);
        if !presets.is_empty() {
            groups.push(PresetGroup {
                dir,
                label: kind.label().to_string(),
                slot: PresetSlot::Effect(kind),
                presets,
            });
        }
    }

    groups
}

/// The line a preset row shows to the right of its name.
///
/// Deliberately not the raw `category`. Of the ninety-nine presets a seeded
/// machine ships, sixty-six are categorised `"Factory"` and seventeen
/// `"DS-01"` -- restatements of the group they are already filed under. A
/// category earns the space only when it says something the group does not,
/// which is why this is a filter rather than a format.
fn preset_detail(summary: &PresetSummary, group_label: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let category = summary.category.trim();
    if !category.is_empty()
        && !category.eq_ignore_ascii_case("factory")
        && !category.eq_ignore_ascii_case(group_label)
    {
        parts.push(category);
    }
    parts.extend(summary.tags.iter().map(|tag| tag.as_str()).filter(|tag| !tag.is_empty()));
    parts.join(" · ")
}

/// Flattens the preset catalogue into rows: every group, then the presets
/// inside the expanded ones.
///
/// `channel_kind` is the selected channel's device, and it decides only
/// whether a *generator* row is loadable -- see [`PresetSlot::Effect`] for
/// why an effect row is always offered. An unloadable row is still drawn,
/// because a browser you cannot look through is a menu.
fn build_preset_rows(
    groups: &[PresetGroup],
    expanded: &HashSet<PathBuf>,
    channel_kind: Option<DeviceKind>,
) -> Vec<BrowserRow> {
    let mut rows = Vec::new();
    for group in groups {
        let is_expanded = expanded.contains(&group.dir);
        rows.push(BrowserRow {
            depth: 0,
            kind: 2,
            name: group.label.as_str().into(),
            path: group.dir.to_string_lossy().to_string().into(),
            expanded: is_expanded,
            detail: group.presets.len().to_string().into(),
            loadable: true,
        });
        if !is_expanded {
            continue;
        }
        let loadable = match group.slot {
            PresetSlot::Generator(kind) => channel_kind == Some(kind),
            PresetSlot::Channel | PresetSlot::Effect(_) => true,
        };
        for preset in &group.presets {
            rows.push(BrowserRow {
                depth: 1,
                kind: 3,
                name: preset.name.as_str().into(),
                path: preset.path.to_string_lossy().to_string().into(),
                expanded: false,
                detail: preset_detail(preset, &group.label).into(),
                loadable,
            });
        }
    }
    rows
}

/// What the in-app question is asking (MOO-91), so its answer knows what to
/// go on to. Held by Rust rather than the markup, because what an answer
/// does -- save and then quit, load the kit that was waiting -- is data the
/// markup has no business holding.
enum Question {
    /// Unsaved changes, before doing `AfterUnsaved`.
    Unsaved(AfterUnsaved),
    /// A preset of that name exists. `save` writes over it; a cancel puts
    /// `target` back so the preset dialog still saves.
    ReplacePreset {
        target: PresetSaveTarget,
        save: Box<dyn FnOnce(&MainWindow)>,
    },
    /// A kit shorter than the song would drop channels holding notes. The
    /// loaded result waits here and goes back through the pump on a yes.
    LoadKit(Box<DocumentResult>),
}

/// What the unsaved-changes question was standing in front of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AfterUnsaved {
    Quit,
    NewSong,
    OpenSong,
}

/// What an answer to the unsaved-changes question does. `answer` is the
/// dialog's: 1 Save, 2 Don't Save, anything else Cancel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UnsavedStep {
    /// Save, and go on only once the save has succeeded.
    SaveThen(AfterUnsaved),
    /// Go on, dropping the changes.
    Go(AfterUnsaved),
    /// Stay where you are.
    Stay,
}

fn unsaved_step(after: AfterUnsaved, answer: i32) -> UnsavedStep {
    match answer {
        1 => UnsavedStep::SaveThen(after),
        2 => UnsavedStep::Go(after),
        _ => UnsavedStep::Stay,
    }
}

/// Ask `question` in the app's own dialog. `secondary` empty hides the third
/// button.
fn ask_question(
    window: &MainWindow,
    slot: &RefCell<Option<Question>>,
    question: Question,
    title: &str,
    detail: &str,
    primary: &str,
    secondary: &str,
) {
    *slot.borrow_mut() = Some(question);
    window.set_question_title(title.into());
    window.set_question_detail(detail.into());
    window.set_question_primary(primary.into());
    window.set_question_secondary(secondary.into());
    window.set_question_open(true);
}

/// Save / Don't Save / Cancel, in front of `after` (MOO-91). One wording for
/// Quit and the window's close button, which used to ask two different
/// questions. `song` is the open song's name, when it has one.
fn ask_unsaved(
    window: &MainWindow,
    slot: &RefCell<Option<Question>>,
    after: AfterUnsaved,
    song: Option<&str>,
) {
    let what = song.map_or_else(|| "this song".to_string(), |name| format!("\"{name}\""));
    let before = match after {
        AfterUnsaved::Quit => "quitting",
        AfterUnsaved::NewSong => "starting a new song",
        AfterUnsaved::OpenSong => "opening another song",
    };
    ask_question(
        window,
        slot,
        Question::Unsaved(after),
        &format!("Save changes to {what} before {before}?"),
        "If you don't save, the changes since the last save are lost.",
        "Save",
        "Don't Save",
    );
}

/// The open song's name for a question: its file's stem, or `None` while it
/// has never been saved.
fn song_name(state: &UiState) -> Option<String> {
    state
        .session
        .bundle_path
        .as_deref()
        .and_then(Path::file_stem)
        .map(|stem| stem.to_string_lossy().into_owned())
}

/// Do what the unsaved-changes question stood in front of, now that it has
/// been answered. `settled` tells the command it re-enters not to ask again.
fn go_on_after_unsaved(window: &MainWindow, settled: &Cell<bool>, after: AfterUnsaved) {
    settled.set(true);
    match after {
        AfterUnsaved::Quit => window.invoke_quit_requested(),
        AfterUnsaved::NewSong => window.invoke_new_song(),
        AfterUnsaved::OpenSong => window.invoke_open_song(),
    }
}

/// Say that a document operation is running, without starting one.
fn say_busy(window: &MainWindow) {
    window.set_status_message("Busy: wait for the file operation in progress to finish".into());
}

/// Starts a document operation -- a save, an open, a load, an export -- or
/// refuses it because one is already running, and says so.
///
/// **The one gate every document command goes through** (MOO-92). The File
/// menu has long greyed itself out on `document-busy`, but shortcuts and the
/// browser reach the callbacks directly, so Ctrl+S on a slow song followed by
/// Ctrl+N started the new song while the save was still writing the old one,
/// and the save's late result then gave the new song the old one's path. Two
/// saves at once raced for the same files. Checking and setting the flag here,
/// on the UI thread, makes "one at a time" true of every entry point at once.
fn begin_document_operation(window: &MainWindow, status: &str) -> bool {
    if window.get_document_busy() {
        say_busy(window);
        return false;
    }
    window.set_document_busy(true);
    window.set_status_message(status.into());
    true
}

/// Whether a quit has to wait for a document operation in flight, and if so
/// remember it: the pump asks again once the operation has reported
/// (MOO-92). Quitting underneath a save killed the save thread mid-write.
fn defer_quit_while_busy(window: &MainWindow, pending: &Cell<bool>) -> bool {
    if !window.get_document_busy() {
        return false;
    }
    pending.set(true);
    window.set_status_message("Quitting when the file operation in progress finishes...".into());
    true
}

/// What a finished song save changes about the open document, if it is still
/// the document the save was started from. Returns whether it was.
///
/// The path is the part that must not cross (MOO-92): a result that arrives
/// after New or Open has replaced the song would otherwise make the next
/// Ctrl+S write the new song over the old one's file. `dirty` was already
/// guarded by the revision, which a new song also bumps.
fn apply_saved_song(
    state: &mut UiState,
    window: &MainWindow,
    generation: u64,
    revision: u64,
    path: &Path,
    sample_references: Vec<Option<SampleReference>>,
) -> bool {
    if state.session.document_generation != generation {
        return false;
    }
    state.session.bundle_path = Some(path.to_path_buf());
    if state.session.revision == revision {
        state.session.dirty = false;
        apply_sample_references(&mut state.session.channels, sample_references);
    }
    state.update_document_title(window);
    true
}

/// Opens a generator or channel preset off the UI thread.
///
/// Shared by the device rail's preset menus and the browser panel, which
/// differ only in how they choose the path: both of these presets are whole
/// documents, and a document load is already asynchronous because it can
/// touch a sample on disk.
fn load_preset_document(
    tx: &std::sync::mpsc::Sender<DocumentResult>,
    window: &MainWindow,
    path: PathBuf,
    target: LoadTarget,
    label: &str,
) {
    if !begin_document_operation(window, &format!("Loading {label}...")) {
        return;
    }
    let tx = tx.clone();
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
}

/// Adds the effect preset at `path` to the end of the selected channel's
/// chain, and returns the before/after snapshots that make it one undoable
/// edit.
///
/// Two steps, because a preset carries what a device *sounds like* and not
/// which device it is: insert a row of the right kind to mint an identity,
/// then load the preset over it. A run preset does the same through a
/// container, since `load_effect_run` will only replace a `Chain`.
///
/// `None` when the bundle will not open, is not an effect preset, or does not
/// fit -- each of which has already been reported to the status bar.
fn append_effect_preset(
    st: &Rc<RefCell<UiState>>,
    window: &MainWindow,
    path: &Path,
    kind: EffectKind,
    name: &str,
) -> Option<(ProjectSnapshot, ProjectSnapshot)> {
    let loaded = match mooloop_project::load_bundle(path) {
        Ok(report) => match report.document {
            LoadedDocument::Effect(effect) => Ok(*effect),
            LoadedDocument::EffectRun(run) => Err(*run),
            _ => {
                log_warn!("project", "{} is not an effect preset", path.display());
                window.set_status_message("That bundle is not an effect preset".into());
                return None;
            }
        },
        Err(error) => {
            log_warn!("project", "could not open {}: {error}", path.display());
            window.set_status_message(format!("Could not open this preset: {error}").into());
            return None;
        }
    };

    let before = project_snapshot(&st.borrow(), window);
    {
        let mut state = st.borrow_mut();
        let tail = state.session.effect_chain().map(Vec::len)?;
        // The row the preset lands on has to exist before it can be loaded
        // over, and it has to be the preset's own kind -- a run always starts
        // with the container that `load_effect_run` insists on.
        if state.session.insert_effect_at(kind, tail).is_none() {
            drop(state);
            window.set_status_message("This chain is full".into());
            return None;
        }
        let landed = match &loaded {
            Ok(effect) => state.session.load_effect_preset(tail, effect, name).is_some(),
            Err(run) => state.session.load_effect_run(tail, run, name).is_some(),
        };
        if !landed {
            // Take the row back out rather than leaving an empty device the
            // musician did not ask for.
            let _ = state.session.remove_effect_at(tail);
            drop(state);
            log_warn!("project", "{name} does not fit the end of this chain");
            window.set_status_message("That preset is for a different kind of device".into());
            return None;
        }
        state.sync_effects();
    }
    let after = project_snapshot(&st.borrow(), window);
    window.set_status_message(format!("Added {name}").into());
    Some((before, after))
}

/// The four things the device clipboard can do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DeviceClipboardVerb {
    Copy,
    Cut,
    Paste,
    Duplicate,
}

impl DeviceClipboardVerb {
    fn from_int(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::Copy),
            1 => Some(Self::Cut),
            2 => Some(Self::Paste),
            3 => Some(Self::Duplicate),
            _ => None,
        }
    }
}

/// Runs a clipboard verb against `slot`, or against the selected device when
/// `slot` is `None` -- which is the difference between the rail's own button
/// and a keyboard shortcut.
///
/// Copy is not an edit and does not touch history. The other three go through
/// the project-edit path, which reinstalls the project and so carries the
/// structural change without a separate engine command; that is the same
/// route a loaded effect preset takes.
fn apply_device_clipboard(
    st: &Rc<RefCell<UiState>>,
    window: &MainWindow,
    tx: &ProjectEditSender,
    commands: &Rc<RefCell<CommandState>>,
    verb: DeviceClipboardVerb,
    slot: Option<usize>,
) {
    let slot = match slot.or_else(|| st.borrow().session.selected_device_slot()) {
        Some(slot) => slot,
        None if verb == DeviceClipboardVerb::Paste => {
            // A paste with nothing selected still has somewhere to go: the
            // end of the chain. That makes the first paste onto a fresh
            // channel work without a selection gesture first.
            st.borrow().session.effect_chain().map(Vec::len).unwrap_or(0)
        }
        None => {
            window.set_status_message("Select a device first".into());
            return;
        }
    };

    if verb == DeviceClipboardVerb::Copy || verb == DeviceClipboardVerb::Cut {
        let Some(run) = st.borrow().session.copy_device(slot) else {
            window.set_status_message("Select a device first".into());
            return;
        };
        let rows = run.effects.len();
        commands.borrow_mut().device_clipboard = Some(run);
        if verb == DeviceClipboardVerb::Copy {
            window.set_status_message(copied_message(rows).into());
            return;
        }
    }

    let before = project_snapshot(&st.borrow(), window);
    let status = {
        let mut state = st.borrow_mut();
        match verb {
            DeviceClipboardVerb::Copy => unreachable!("copy returned above"),
            DeviceClipboardVerb::Cut => {
                if state.session.remove_effect_at(slot).is_none() {
                    return;
                }
                "Device cut"
            }
            DeviceClipboardVerb::Paste | DeviceClipboardVerb::Duplicate => {
                let landed = if verb == DeviceClipboardVerb::Duplicate {
                    state.session.duplicate_device(slot)
                } else {
                    let Some(run) = commands.borrow().device_clipboard.clone() else {
                        drop(state);
                        window.set_status_message("Nothing to paste".into());
                        return;
                    };
                    state.session.paste_device(&run, slot)
                };
                if landed.is_none() {
                    drop(state);
                    window.set_status_message("This chain is full".into());
                    return;
                }
                if verb == DeviceClipboardVerb::Duplicate {
                    "Device duplicated"
                } else {
                    "Device pasted"
                }
            }
        }
    };
    {
        let st = st.borrow();
        st.sync_effects();
        st.refresh_automation(window);
        st.refresh_modulation(window);
    }
    let after = project_snapshot(&st.borrow(), window);
    if queue_project_edit(tx, before, after, status) {
        commands.borrow_mut().project_edit_pending = true;
        sync_command_availability(window, &commands.borrow());
    }
}

/// What a copy says it took. A container takes its run, and saying so is the
/// difference between "I copied the box" and "I copied the box and the three
/// devices in it" -- which is what the musician needs to know before pasting.
fn copied_message(rows: usize) -> String {
    if rows > 1 {
        format!("Copied a container and the {} devices in it", rows - 1)
    } else {
        "Device copied".to_string()
    }
}

/// Rebuilds the visible tree from the session's locations and expansion set.
///
/// One row model serves both tabs, so this is also what switches them.
fn refresh_browser(st: &UiState) {
    if st.browser_tab == BrowserTab::Presets {
        let channel_kind = st.session.channels.get(st.session.selected).map(|c| c.kind);
        st.browser_rows.set_vec(build_preset_rows(
            &st.preset_catalog,
            &st.session.browser_expanded,
            channel_kind,
        ));
        return;
    }
    st.browser_rows.set_vec(build_browser_rows(
        &st.session.browser_locations,
        &st.session.browser_expanded,
    ));
}

/// The browser's keyboard walk, in the part of it that is arithmetic.
///
/// Step 01 shipped the tree with "no keyboard navigation" written into its
/// own status file; this is the half that can be checked without a rendered
/// tree, which is most of the rules worth stating.
#[cfg(test)]
mod browser_keyboard_tests {
    use super::*;

    #[test]
    fn an_unset_focus_enters_from_the_end_the_key_came_from() {
        assert_eq!(browser_focus_step(5, -1, 1), Some(0));
        assert_eq!(browser_focus_step(5, -1, -1), Some(4));
    }

    #[test]
    fn the_walk_clamps_instead_of_wrapping() {
        // Wrapping would put Down at the bottom of a list back at the top,
        // which is how a key held down loses the row a user was reading.
        assert_eq!(browser_focus_step(3, 2, 1), None);
        assert_eq!(browser_focus_step(3, 0, -1), None);
        assert_eq!(browser_focus_step(3, 1, 1), Some(2));
    }

    #[test]
    fn an_empty_tree_answers_nothing() {
        assert_eq!(browser_focus_step(0, -1, 1), None);
        assert_eq!(browser_focus_step(0, 0, -1), None);
    }

    /// Walking onto a sample plays it; walking onto anything else does not
    /// do that row's click.
    ///
    /// The second half is the one worth pinning. A preset row's click
    /// *loads* the preset into the selected channel, so an arrow walk that
    /// did what a click does would install a device per keypress on the way
    /// down the PRESETS tab -- and the tab shares this model and this
    /// keyboard with the samples.
    #[test]
    fn the_arrows_audition_a_sample_and_nothing_else() {
        assert!(browser_row_auditions(BROWSER_SAMPLE));
        assert!(!browser_row_auditions(BROWSER_FOLDER));
        assert!(!browser_row_auditions(BROWSER_GROUP));
        // 3 is a preset, the kind that has no constant because it is what a
        // row that is none of the others is.
        assert!(!browser_row_auditions(3));
    }

    /// Left on a leaf climbs to the folder holding it, which in a flattened
    /// model means the nearest earlier row that is shallower -- not the
    /// previous row, and not the previous row at depth zero.
    #[test]
    fn the_parent_is_the_nearest_earlier_shallower_row() {
        let depths = [0, 1, 2, 2, 1, 0];
        assert_eq!(browser_parent_of(&depths, 3), Some(1));
        assert_eq!(browser_parent_of(&depths, 2), Some(1));
        assert_eq!(browser_parent_of(&depths, 4), Some(0));
        assert_eq!(browser_parent_of(&depths, 0), None);
        assert_eq!(browser_parent_of(&depths, 5), None);
        assert_eq!(browser_parent_of(&depths, 9), None);
    }
}

#[cfg(test)]
mod preset_browser_tests {
    use super::*;
    use mooloop_project::PresetKind;

    fn summary(name: &str, category: &str, tags: &[&str]) -> PresetSummary {
        PresetSummary {
            path: PathBuf::from(format!("/presets/{name}.mooloop-effect")),
            name: name.to_string(),
            category: category.to_string(),
            tags: tags.iter().map(|tag| tag.to_string()).collect(),
            kind: PresetKind::Effect(EffectKind::Delay),
        }
    }

    fn group(label: &str, slot: PresetSlot, presets: Vec<PresetSummary>) -> PresetGroup {
        PresetGroup {
            dir: PathBuf::from(format!("/presets/{label}")),
            label: label.to_string(),
            slot,
            presets,
        }
    }

    #[test]
    fn a_group_lists_its_presets_only_when_expanded() {
        let groups = vec![group(
            "Delay",
            PresetSlot::Effect(EffectKind::Delay),
            vec![summary("Slapback", "Factory", &[])],
        )];

        let collapsed = build_preset_rows(&groups, &HashSet::new(), None);
        assert_eq!(collapsed.len(), 1, "a collapsed group is one row");
        assert_eq!(collapsed[0].kind, 2);
        assert_eq!(collapsed[0].detail, "1", "a group shows what it holds");

        let expanded = HashSet::from([PathBuf::from("/presets/Delay")]);
        let open = build_preset_rows(&groups, &expanded, None);
        assert_eq!(open.len(), 2);
        assert_eq!(open[1].kind, 3);
        assert_eq!(open[1].name, "Slapback");
    }

    /// The rule that makes an effect preset worth browsing: loading one adds
    /// a device, so there is no row for it to have to match.
    #[test]
    fn an_effect_preset_is_loadable_whatever_the_channel_holds() {
        let groups = vec![group(
            "Delay",
            PresetSlot::Effect(EffectKind::Delay),
            vec![summary("Slapback", "Factory", &[])],
        )];
        let expanded = HashSet::from([PathBuf::from("/presets/Delay")]);

        for channel in [None, Some(DeviceKind::Sampler), Some(DeviceKind::Ds01)] {
            let rows = build_preset_rows(&groups, &expanded, channel);
            assert!(rows[1].loadable, "an effect preset appends, so it always fits");
        }
    }

    /// A generator preset replaces the channel's source, so it is offered
    /// only where it would mean something.
    #[test]
    fn a_generator_preset_is_loadable_only_on_its_own_kind() {
        let groups = vec![group(
            "DS-01",
            PresetSlot::Generator(DeviceKind::Ds01),
            vec![summary("Deep Kick", "DS-01", &[])],
        )];
        let expanded = HashSet::from([PathBuf::from("/presets/DS-01")]);

        let matching = build_preset_rows(&groups, &expanded, Some(DeviceKind::Ds01));
        assert!(matching[1].loadable);

        let mismatched = build_preset_rows(&groups, &expanded, Some(DeviceKind::MlP8));
        assert!(!mismatched[1].loadable, "a DS-01 patch does not fit an ML-P8");

        let empty = build_preset_rows(&groups, &expanded, None);
        assert!(!empty[1].loadable, "and fits nothing at all with no channel");
    }

    /// Sixty-six of the ninety-nine presets a seeded machine ships are
    /// categorised "Factory" and seventeen "DS-01", both of which restate the
    /// group the row is already filed under. Showing them would spend the
    /// only spare column in the panel on saying nothing.
    #[test]
    fn a_category_that_restates_its_group_is_not_shown() {
        assert_eq!(preset_detail(&summary("A", "Factory", &[]), "Delay"), "");
        assert_eq!(preset_detail(&summary("B", "DS-01", &[]), "DS-01"), "");
        assert_eq!(preset_detail(&summary("C", "ds-01", &[]), "DS-01"), "");
        assert_eq!(preset_detail(&summary("D", "", &[]), "Delay"), "");
    }

    #[test]
    fn a_category_that_says_something_is_shown_with_the_tags() {
        assert_eq!(preset_detail(&summary("A", "Pad", &[]), "ML-P8"), "Pad");
        assert_eq!(
            preset_detail(&summary("B", "Pad", &["warm", "wide"]), "ML-P8"),
            "Pad · warm · wide"
        );
        assert_eq!(
            preset_detail(&summary("C", "Factory", &["bright"]), "Delay"),
            "bright",
            "tags survive a category that does not"
        );
    }

    #[test]
    fn an_empty_group_contributes_only_its_own_row() {
        let groups = vec![group("Reverb", PresetSlot::Effect(EffectKind::Reverb), vec![])];
        let expanded = HashSet::from([PathBuf::from("/presets/Reverb")]);
        let rows = build_preset_rows(&groups, &expanded, None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].detail, "0");
    }
}

#[cfg(test)]
mod tests {
    use mooloop_session::browser::is_playable_sample;

    /// **The published row says what the chain says**, for every kind.
    ///
    /// Written because its absence was measured rather than suspected:
    /// publishing `is_container: false` unconditionally passed the whole
    /// `mooloop-ui` suite. That mutation turns off the enclosure, makes a
    /// container's wet/dry knob live -- the "convincing but inert control"
    /// the rack's own rule forbids -- disables unwrap, makes `+` insert
    /// beside the box instead of into it, and draws no container face at
    /// all. The entire container interface, silently gone, with every test
    /// green.
    ///
    /// The gap is older than the flag: `slot.kind == 13` was exactly as
    /// unwatched, and nothing has ever checked that the rack draws a
    /// container as one. This closes the half a unit test can reach -- the
    /// value crossing the boundary -- and it is the half a later edit to the
    /// publisher would break. Whether the *markup* honours it is a rendered
    /// question and is `containers/09`'s to answer, where the drawing
    /// changes anyway.
    #[test]
    fn a_published_row_reports_containment_for_every_kind() {
        use mooloop_core::{EffectKind, EffectSlotState};

        for kind in EffectKind::ALL {
            let slot = EffectSlotState::of_kind(kind);
            let row = super::effect_slot_row(
                &slot,
                &[],
                None,
                super::RackPlacement {
                    depth: 0,
                    closing: Vec::new(),
                    selected: false,
                    wrap_enabled: true,
                },
                48_000,
            );
            assert_eq!(
                row.is_container,
                kind.is_container(),
                "the row published for {} disagrees with the chain about \
                 whether it holds a run",
                kind.label()
            );
            // The container face titles itself from this, so a layer that
            // published "Chain" would be drawn as one.
            assert_eq!(row.label.as_str(), kind.label());
        }
    }

    /// The subscription plan states an answer for the slot past the end of
    /// the chain, which is the one a removal vacates.
    ///
    /// This is the whole of the defect it replaced. The old walk visited the
    /// devices that *are* analyzers and said `true` or `false` for each, so a
    /// stage an analyzer had moved off was never mentioned and kept its
    /// subscription: the engine ran a Goertzel bank every hop for a display
    /// nobody drew, whatever landed on that slot number drew a flat line
    /// behind a lit button, and the orphan held one of the sixty-four
    /// `SPECTRUM_SLOTS` until the project was reloaded.
    ///
    /// One past the end is enough only because nothing above a chain's length
    /// can be subscribed when the sync returns, and a project install -- the
    /// one edit that can shorten a chain by more than one -- clears the whole
    /// table first. Both halves of that are asserted here.
    #[test]
    fn the_spectrum_plan_speaks_for_the_slot_a_removal_vacates() {
        use mooloop_core::{EffectKind, EffectSlotState};

        let slot = |kind: EffectKind| EffectSlotState {
            id: Default::default(),
            params: kind.default_params(),
            bypassed: false,
            wet_dry: 1.0,
            input_trim: 1.0,
            output_trim: 1.0,
        };
        let mut analyzing = slot(EffectKind::Eq);
        if let mooloop_core::EffectParams::Eq(eq) = &mut analyzing.params {
            eq.analyzer_enabled = true;
        }

        // A delay, then the EQ whose analyzer is on: three answers for a
        // two-device chain.
        let chain = vec![slot(EffectKind::Delay), analyzing];
        let plan: Vec<(u8, bool)> = super::spectrum_subscription_plan(&chain).collect();
        assert_eq!(
            plan,
            vec![(0, false), (1, true), (2, false)],
            "the plan must speak for slot 2, which is where a removal leaves an orphan"
        );

        // The EQ is deleted. The plan's job is to say `false` for slot 1,
        // which is the stage the engine is still publishing into.
        let shortened = vec![chain[0]];
        let after: Vec<(u8, bool)> = super::spectrum_subscription_plan(&shortened).collect();
        assert_eq!(after, vec![(0, false), (1, false)]);

        // An empty chain still answers, because a chain can be emptied.
        let empty: Vec<(u8, bool)> = super::spectrum_subscription_plan(&[]).collect();
        assert_eq!(empty, vec![(0, false)]);
    }

    /// The published defaults reach each oscillator's *own* resting value.
    ///
    /// The reason this array exists rather than a literal in the markup: the
    /// three oscillator strips are one component instantiated three times, and
    /// their Semi and Fine defaults differ by design -- OSC 1 at unison, OSC 2
    /// an octave up, OSC 3 an octave down. One literal in the shared markup was
    /// necessarily wrong for two of the three, and `default-value` is what a
    /// double-click resets to, so two of every synth's six tuning knobs reset
    /// to a value no fresh patch has.
    ///
    /// The second assertion is the one that matters: it fails if the defaults
    /// are ever flattened to agree, which is exactly when someone would be
    /// tempted to put the literal back.
    #[test]
    fn each_oscillator_publishes_its_own_resting_tuning() {
        use mooloop_core::{synth_osc_param, OSC_OFFSET_CENTS, OSC_OFFSET_LEVEL, OSC_OFFSET_SEMITONES};

        let descriptors = DeviceKind::MonoSynth.descriptors();
        let defaults = descriptor_defaults(descriptors);
        let at = |id: u32| {
            slint::Model::row_data(&defaults, id as usize).expect("id is in the published array")
        };

        for oscillator in 0..3u32 {
            for offset in [OSC_OFFSET_SEMITONES, OSC_OFFSET_CENTS] {
                let id = synth_osc_param(oscillator, offset);
                let descriptor = DeviceKind::MonoSynth
                    .descriptor(id)
                    .expect("every oscillator tuning parameter is described");
                assert!(
                    (at(id) - descriptor.default).abs() < 1e-6,
                    "osc {oscillator} {}: published {}, table says {}",
                    descriptor.name,
                    at(id),
                    descriptor.default
                );
            }
        }

        let semis = |oscillator| at(synth_osc_param(oscillator, OSC_OFFSET_SEMITONES));
        assert_eq!(
            (semis(0), semis(1), semis(2)),
            (0.0, 12.0, -12.0),
            "the three oscillators rest at different tunings; one shared literal \
             cannot serve them, which is why this array is published at all"
        );

        // Level is the same fault with the audible symptom. Only OSC 1 rests
        // at unity, and the face's literal was -60 dB for all three -- so a
        // double-click on the one oscillator a fresh patch has turned up muted
        // it. The face reads this through `GainMath.linear-to-db`, which floors
        // at `MIN_DB`, so 0.0 here is the -60 the other two should show.
        let level = |oscillator| at(synth_osc_param(oscillator, OSC_OFFSET_LEVEL));
        assert_eq!(
            (level(0), level(1), level(2)),
            (1.0, 0.0, 0.0),
            "OSC 1 rests at unity and the other two at silence"
        );
    }

    #[test]
    fn modulation_uses_two_rack_units() {
        assert_eq!(effect_kind_units(EffectKind::Modulation), 2);
    }

    use super::*;

    /// A lane readout must show the range it is editing.
    ///
    /// `format_param_value` printed the descriptor's own units directly, so a
    /// time parameter -- always declared in seconds -- was rendered at two
    /// decimal places. An envelope attack of 5 ms read `0.01 s` and anything
    /// shorter read `0.00 s`, which makes the fastest and most-edited part of
    /// an envelope a row of identical zeroes. Every other readout in the
    /// program already went through `display_unit`; this was the one that
    /// did not.
    #[test]
    fn a_lane_reads_a_short_time_in_milliseconds() {
        let seconds = ParamDescriptor {
            id: 0,
            name: "Attack",
            unit: "s",
            min: 0.0,
            max: 2.0,
            curve: ParamCurve::Linear,
            default: 0.0,
        };

        // The cases that used to collapse to "0.01 s" and "0.00 s".
        assert_eq!(super::format_param_value(&seconds, 0.0025), "5 ms");
        assert_eq!(super::format_param_value(&seconds, 0.00025), "0.5 ms");
        // A second and over keeps its own unit, and its two decimals.
        assert_eq!(super::format_param_value(&seconds, 0.75), "1.50 s");
    }

    /// The other half of `display_unit`: a frequency at or above a kilohertz
    /// reads in kHz. Without it the general formatter's own large-number
    /// branch produced `12.00k Hz`, which is a magnitude prefix and a unit
    /// that disagree about the scale of the same number.
    #[test]
    fn a_lane_reads_a_high_frequency_in_kilohertz() {
        let hertz = ParamDescriptor {
            id: 1,
            name: "Cutoff",
            unit: "Hz",
            min: 0.0,
            max: 20_000.0,
            curve: ParamCurve::Linear,
            default: 0.0,
        };

        assert_eq!(super::format_param_value(&hertz, 0.6), "12.0 kHz");
        // Below a kilohertz it stays in hertz, whole numbers at that size.
        assert_eq!(super::format_param_value(&hertz, 0.022), "440 Hz");
    }

    /// The markup, as text. These tests read the two division tables out of it
    /// rather than out of a copy, which is the whole point of them.
    const MAIN_SLINT: &str = include_str!("../ui/main.slint");
    const CONTROLS_SLINT: &str = include_str!("../ui/controls.slint");

    /// Pull an `index == 0 ? a : index == 1 ? b : ... : z` chain out of one
    /// Slint function body, as the values in index order.
    ///
    /// Slint has no loop, so every table in the markup is a chain like this.
    /// The trailing branch is the last index rather than a fallback -- that is
    /// what makes the length one more than the highest `index ==` it names.
    fn ternary_chain(markup: &str, signature: &str) -> Vec<String> {
        let at = markup
            .find(signature)
            .unwrap_or_else(|| panic!("the markup no longer declares `{signature}`"));
        let open = markup[at..].find('{').expect("a function body") + at;
        let close = markup[open..].find("\n    }").expect("a function end") + open;
        let body = &markup[open..close];

        let mut values: Vec<String> = Vec::new();
        let mut rest = body;
        while let Some(hit) = rest.find("index == ") {
            let after = &rest[hit + "index == ".len()..];
            let (index, after) = after
                .split_once('?')
                .unwrap_or_else(|| panic!("`{signature}`: an `index ==` with no `?`"));
            assert_eq!(
                index.trim().parse::<usize>().ok(),
                Some(values.len()),
                "`{signature}` names its branches out of order at position {}",
                values.len()
            );
            let end = after.find(':').unwrap_or(after.len());
            values.push(after[..end].trim().to_string());
            rest = &after[end..];
        }
        // Whatever follows the last `:` is the final entry.
        let tail = rest
            .trim_end()
            .trim_end_matches(';')
            .rsplit(':')
            .next()
            .expect("a trailing branch");
        values.push(tail.trim().to_string());
        values
    }

    /// **This test used to assert against a copy of its own subject.** It was
    /// named for `snap-ticks()` in `main.slint` and compared
    /// `MUSICAL_DIVISIONS` to a literal `[384, 192, ...]` written inside the
    /// test -- a third spelling of the same eleven numbers. Changing the markup
    /// would not have failed it, which is the one thing it existed to do.
    ///
    /// Both halves of the table are mirrored, so both are read here: the ticks
    /// from `snap-ticks()` and the names from `musical-snap-options`. The
    /// length picker indexes them by the same position, so a drift in either
    /// sets the wrong length silently.
    #[test]
    fn musical_divisions_match_the_snap_table_in_main_slint() {
        let ticks = ternary_chain(MAIN_SLINT, "pure function snap-ticks(index: int) -> int");
        let parsed: Vec<u32> = ticks
            .iter()
            .map(|value| {
                value
                    .parse()
                    .unwrap_or_else(|_| panic!("snap-ticks branch {value:?} is not a tick count"))
            })
            .collect();
        assert_eq!(
            super::MUSICAL_DIVISIONS.map(|(ticks, _)| ticks).to_vec(),
            parsed,
            "MUSICAL_DIVISIONS and main.slint's snap-ticks() disagree"
        );

        let options = MAIN_SLINT
            .split_once("property <[string]> musical-snap-options: [")
            .expect("main.slint declares musical-snap-options")
            .1;
        let options = &options[..options.find(']').expect("an unterminated list")];
        let names: Vec<&str> = options
            .split('"')
            .skip(1)
            .step_by(2)
            .collect();
        assert_eq!(
            super::MUSICAL_DIVISIONS.map(|(_, name)| name).to_vec(),
            names,
            "MUSICAL_DIVISIONS and main.slint's musical-snap-options disagree"
        );
    }

    /// `Divisions.beats` in `controls.slint` against `ModTimeDivision::beats`.
    ///
    /// Twenty-one values, mirrored, and nothing checked them. The markup says
    /// plainly what the stake is -- it is "the reason a delay and an LFO cannot
    /// disagree about what `1/2` is worth, which they did, by a factor of four,
    /// for as long as the delay had a grid of its own" -- and a table written to
    /// end a factor-of-four disagreement had no guard against becoming one
    /// again.
    ///
    /// Both callers turn these into time: `controls.slint` into the delay's
    /// milliseconds, `modulation-device.slint` into the modulation rate in
    /// hertz. A drift here is audibly wrong and silently arrived at.
    #[test]
    fn the_slint_division_table_matches_mod_time_division() {
        use mooloop_core::modulation::ModTimeDivision;

        let branches = ternary_chain(
            CONTROLS_SLINT,
            "public pure function beats(index: int) -> float",
        );
        assert_eq!(
            branches.len(),
            ModTimeDivision::ALL.len(),
            "controls.slint offers {} divisions, ModTimeDivision has {}",
            branches.len(),
            ModTimeDivision::ALL.len()
        );

        for (index, division) in ModTimeDivision::ALL.iter().enumerate() {
            // The markup spells thirds and sixths as divisions rather than as
            // rounded decimals, the same way the Rust table does.
            let branch = &branches[index];
            let slint = match branch.split_once('/') {
                Some((numerator, denominator)) => {
                    numerator.trim().parse::<f32>().expect("a numerator")
                        / denominator.trim().parse::<f32>().expect("a denominator")
                }
                None => branch.parse::<f32>().expect("a beat count"),
            };
            assert!(
                (slint - division.beats()).abs() < 1e-6,
                "division {index} ({division:?}): controls.slint says {slint} beats, \
                 ModTimeDivision says {}",
                division.beats()
            );
        }

        // And the top index beside the table, which is the divisor every
        // stepped grid knob decodes its normalized value through. It was
        // written `20` by hand at three call sites before it was one value.
        // The table above checks the *entries*; nothing checked the count, so
        // a twenty-second division would have left every stepped grid knob in
        // the markup decoding one step short, silently.
        let top = CONTROLS_SLINT
            .split("out property <int> top:")
            .nth(1)
            .and_then(|rest| rest.split(';').next())
            .expect("Divisions.top in controls.slint")
            .trim()
            .parse::<f32>()
            .expect("a grid top index");
        assert!(
            (top - mooloop_core::MOD_TIME_DIVISION_TOP).abs() < 1e-6,
            "controls.slint's Divisions.top is {top}, MOD_TIME_DIVISION_TOP is {}",
            mooloop_core::MOD_TIME_DIVISION_TOP
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
        spawn_browser_sample_load("/nonexistent/missing.wav", 3, 7, 11, true, &tx);
        let load = rx.recv().unwrap();
        assert_eq!(load.channel, 3);
        assert_eq!(load.source_revision, 7);
        assert_eq!(load.request, 11);
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


    /// A DS-01 field has to read back what it shows, in the unit it shows it
    /// in. That is one property over the pair of functions, and it is the
    /// only thing keeping a typed number from meaning something the caption
    /// contradicts.
    #[test]
    fn a_ds01_field_reads_back_what_it_shows() {
        for descriptor in ds01::DESCRIPTORS.iter() {
            if matches!(descriptor.curve, ParamCurve::Stepped(_)) {
                continue;
            }
            for position in [0.0_f32, 0.17, 0.5, 0.83, 1.0] {
                let natural = descriptor.from_normalized(position);
                let shown = ds01_text(descriptor, &Ds01Params::default(), position);
                let parsed = ds01_typed_value(descriptor, shown.as_str(), natural)
                    .unwrap_or_else(|| panic!("{} showed {shown:?}", descriptor.name));
                // Compared as the field writes them rather than as floats: a
                // display rounds, so "reads back the same value" can only
                // mean "reads back to the same reading".
                let again = ds01_text(
                    descriptor,
                    &Ds01Params::default(),
                    descriptor.to_normalized(parsed),
                );
                assert_eq!(
                    shown, again,
                    "{} showed {shown:?} at {natural} and read back as {parsed}",
                    descriptor.name
                );
            }
        }
    }

    /// The three departures from the shared formatter, stated as cases rather
    /// than as a paragraph. Each exists because a drum lives at the short end
    /// of the range: `0.00 s` is three different attacks.
    #[test]
    fn ds01_writes_values_in_the_units_a_drum_patch_uses() {
        let amp_decay = ds01::descriptor(ds01::PARAM_AMP_DECAY).unwrap();
        assert_eq!(ds01_display_unit(amp_decay, 0.24), (0.001, "ms"));
        assert_eq!(ds01_display_unit(amp_decay, 4.0), (1.0, "s"));

        let cutoff = ds01::descriptor(ds01::PARAM_FILTER_CUTOFF).unwrap();
        assert_eq!(ds01_display_unit(cutoff, 220.0), (1.0, "Hz"));
        assert_eq!(ds01_display_unit(cutoff, 7_500.0), (1_000.0, "kHz"));

        let amount = ds01::descriptor(ds01::matrix_param(0, ds01::MATRIX_OFFSET_AMOUNT)).unwrap();
        assert_eq!(ds01_display_unit(amount, 0.35), (0.01, "%"));

        // A written unit wins; a bare number means the one on the face.
        let reads = |descriptor: &ParamDescriptor, text: &str, current: f32, expected: f32| {
            let got = ds01_typed_value(descriptor, text, current)
                .unwrap_or_else(|| panic!("{text:?} did not parse"));
            assert!(
                (got - expected).abs() <= expected.abs() * 1.0e-5 + 1.0e-9,
                "{text:?} read as {got}, not {expected}"
            );
        };
        reads(amp_decay, "240 ms", 0.24, 0.24);
        reads(amp_decay, "1.5 s", 0.24, 1.5);
        reads(amp_decay, "300", 0.24, 0.3);
        reads(amp_decay, "2", 4.0, 2.0);
        reads(cutoff, "8 kHz", 7_500.0, 8_000.0);
        reads(amount, "-50%", 0.35, -0.5);
    }

    /// Adding a channel from the toolbar leaves an undo entry behind.
    ///
    /// It did not. Add is the only channel-structure verb that is incremental
    /// rather than a whole-project reinstall, and it reached the engine and
    /// the dirty flag without ever reaching the history. Undo then had two
    /// ways to be wrong and no way to be right: with an empty history it did
    /// nothing at all, and with a previous recorded edit it installed *that*
    /// edit's `before` -- a snapshot taken before the channel existed -- so
    /// the new channel and every unrecorded edit made since disappeared
    /// together, with no redo to get them back because the entry's `after`
    /// predated them too.
    ///
    /// The assertions are about the entry rather than about what undo does,
    /// because undo is asynchronous here: it queues a `ProjectEdit` that only
    /// lands when the pump runs, and the pump is an `AppUi` timer. What this
    /// can see, and what was missing, is the recorded pair itself.
    #[test]
    fn adding_a_channel_records_its_own_undo_entry() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().expect("the testing backend builds a window");
        let state = Rc::new(RefCell::new(UiState::new(None, 48_000, &window)));
        let commands = Rc::new(RefCell::new(CommandState::default()));
        // Both receivers are held for the duration: a dropped one makes the
        // send fail, and this test is about what the send is accompanied by.
        let (pending_tx, _pending_rx) = std::sync::mpsc::channel::<PendingEngineMessage>();
        let structural_tx = StructuralCommandSender(pending_tx);
        let (reset_tx, _reset_rx) = std::sync::mpsc::channel::<usize>();

        assert!(
            commands.borrow().history.undo_target().is_none(),
            "a document nobody has edited has nothing to undo"
        );

        let index = add_channel_with_history(
            &state,
            &commands,
            &window,
            &structural_tx,
            &reset_tx,
            DeviceKind::Sampler,
        )
        .expect("a one-channel rack has room for a second");

        let open = commands.borrow();
        let entry = open
            .history
            .undo_target()
            .expect("the add is now the edit an undo would reverse");
        assert_eq!(entry.label, "Add channel");
        assert_eq!(
            entry.before.project.channels.len() + 1,
            entry.after.project.channels.len(),
            "the entry brackets exactly the one channel that was added"
        );
        // `selected_channel` is a `ChannelId` rather than a seat number, so
        // the question "is the new channel the selected one" is asked through
        // `selected_index` rather than by comparing a number to a position.
        assert_eq!(
            entry.after.project.selected_index(),
            index,
            "and the channel it added is the one left selected"
        );
    }

    /// **Every document command is refused while one is running** (MOO-92).
    /// Ctrl+S, Ctrl+O and Ctrl+N reach `invoke_save_song`, `invoke_open_song`
    /// and `invoke_new_song` straight from the shortcut table, past the File
    /// menu's `enabled`, so the gate is the first thing each callback does.
    #[test]
    fn a_document_command_is_refused_while_another_is_running() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().expect("the testing backend builds a window");

        assert!(begin_document_operation(&window, "Saving song..."));
        assert!(window.get_document_busy());
        assert_eq!(window.get_status_message(), "Saving song...");

        for next in ["Creating new song...", "Opening song...", "Saving song..."] {
            assert!(!begin_document_operation(&window, next), "{next} started under a save");
            assert!(window.get_document_busy());
            assert!(window.get_status_message().starts_with("Busy"));
        }

        window.set_document_busy(false);
        assert!(begin_document_operation(&window, "Creating new song..."));
    }

    /// **The unsaved-changes question's three answers** (MOO-91): Save goes
    /// on only through a save, Don't Save goes on, and Cancel -- or anything
    /// the dialog did not mean, such as Escape's 0 -- stays.
    #[test]
    fn the_unsaved_question_answers_save_then_go_or_stay() {
        for after in [AfterUnsaved::Quit, AfterUnsaved::NewSong, AfterUnsaved::OpenSong] {
            assert_eq!(unsaved_step(after, 1), UnsavedStep::SaveThen(after));
            assert_eq!(unsaved_step(after, 2), UnsavedStep::Go(after));
            assert_eq!(unsaved_step(after, 0), UnsavedStep::Stay);
            assert_eq!(unsaved_step(after, 7), UnsavedStep::Stay);
        }
    }

    /// **Asking never quits and never blocks** (MOO-91): it puts the
    /// question on the window and remembers what it stands in front of, in
    /// one wording for Quit and the close button.
    #[test]
    fn asking_about_unsaved_changes_opens_the_apps_own_question() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().expect("the testing backend builds a window");
        let slot = RefCell::new(None);

        ask_unsaved(&window, &slot, AfterUnsaved::Quit, Some("beat"));

        assert!(window.get_question_open());
        assert_eq!(window.get_question_title(), "Save changes to \"beat\" before quitting?");
        assert_eq!(window.get_question_primary(), "Save");
        assert_eq!(window.get_question_secondary(), "Don't Save");
        assert!(matches!(*slot.borrow(), Some(Question::Unsaved(AfterUnsaved::Quit))));

        ask_unsaved(&window, &slot, AfterUnsaved::OpenSong, None);
        assert_eq!(
            window.get_question_title(),
            "Save changes to this song before opening another song?"
        );
    }

    /// **The takes dialog's ticks and total** (MOO-38): this session's takes
    /// start ticked, earlier sessions' do not, and the total is what the
    /// confirm will move.
    #[test]
    fn the_takes_dialog_starts_with_only_this_sessions_takes_ticked() {
        let take = |name: &str, bytes: u64, earlier: bool| recordings::UnusedTake {
            path: PathBuf::from(format!("/r/{name}.wav")),
            bytes,
            modified: SystemTime::UNIX_EPOCH,
            from_earlier_session: earlier,
        };
        let mut review = TakesReview::new(
            recordings::CleanUp {
                not_used: vec![take("a", 1024 * 1024, false), take("b", 1024 * 1024, false)],
                earlier: vec![take("old", 4 * 1024 * 1024, true)],
            },
            false,
        );
        assert_eq!(review.total(), "Move 2 takes (2.0 MB) to the trash");

        review.toggle(true, 0);
        review.toggle(false, 1);
        let ticked: Vec<_> = review.ticked().into_iter().map(|take| take.path).collect();
        assert_eq!(ticked, [PathBuf::from("/r/a.wav"), PathBuf::from("/r/old.wav")]);
        assert_eq!(review.total(), "Move 2 takes (5.0 MB) to the trash");

        review.toggle(false, 0);
        review.toggle(true, 0);
        review.toggle(false, 99);
        assert!(review.ticked().is_empty());
        assert_eq!(review.total(), "Nothing ticked");
    }

    /// **Quit waits for a save in flight** (MOO-92): it is remembered rather
    /// than acted on, and acted on only once nothing is busy.
    #[test]
    fn a_quit_during_a_save_waits_for_it() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().expect("the testing backend builds a window");
        let pending = Cell::new(false);

        assert!(!defer_quit_while_busy(&window, &pending), "nothing running: quit now");
        assert!(!pending.get());

        window.set_document_busy(true);
        assert!(defer_quit_while_busy(&window, &pending));
        assert!(pending.get());
    }

    /// **Ctrl+S then Ctrl+N: the late result does not change the new song's
    /// path or title** (MOO-92). The save started in one document and
    /// finished in the next; before, it set `bundle_path` unconditionally,
    /// and the next Ctrl+S wrote the starter song over the saved one.
    #[test]
    fn a_save_that_finishes_after_new_song_leaves_the_new_song_alone() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().expect("the testing backend builds a window");
        let state = Rc::new(RefCell::new(UiState::new(None, 48_000, &window)));
        let (generation, revision) = {
            let state = state.borrow();
            (state.session.document_generation, state.session.revision)
        };

        // New Song, as the pump installs it.
        {
            let mut state = state.borrow_mut();
            state.session.revision = state.session.revision.wrapping_add(1);
            state.session.document_generation = state.session.document_generation.wrapping_add(1);
            state.session.dirty = true;
            state.update_document_title(&window);
        }
        let title = window.get_document_title();

        let applied = apply_saved_song(
            &mut state.borrow_mut(),
            &window,
            generation,
            revision,
            Path::new("/tmp/slow.mooloop"),
            Vec::new(),
        );

        assert!(!applied);
        assert_eq!(state.borrow().session.bundle_path, None);
        assert!(state.borrow().session.dirty, "the new song's edits are still unsaved");
        assert_eq!(window.get_document_title(), title);

        // The same result in the document it was started from does land.
        let (generation, revision) = {
            let state = state.borrow();
            (state.session.document_generation, state.session.revision)
        };
        assert!(apply_saved_song(
            &mut state.borrow_mut(),
            &window,
            generation,
            revision,
            Path::new("/tmp/slow.mooloop"),
            Vec::new(),
        ));
        assert_eq!(
            state.borrow().session.bundle_path.as_deref(),
            Some(Path::new("/tmp/slow.mooloop"))
        );
        assert!(!state.borrow().session.dirty);
        assert_eq!(window.get_document_title(), "slow - mooloop");
    }

    /// A window, a one-channel song and an empty history: what every undo
    /// test below starts from.
    fn undo_fixture() -> (MainWindow, Rc<RefCell<UiState>>, Rc<RefCell<CommandState>>) {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().expect("the testing backend builds a window");
        let state = Rc::new(RefCell::new(UiState::new(None, 48_000, &window)));
        let commands = Rc::new(RefCell::new(CommandState::default()));
        (window, state, commands)
    }

    /// An ordinary recorded edit, for the edits under test to land on top
    /// of: the failure they fix is that the *next* Ctrl+Z reached past them
    /// to this one and destroyed them.
    fn an_earlier_edit(
        state: &Rc<RefCell<UiState>>,
        commands: &Rc<RefCell<CommandState>>,
        window: &MainWindow,
    ) {
        with_project_history(state, commands, window, "Rename channel", || {
            state.borrow_mut().session.channels[0].name = "Earlier".to_owned();
            true
        });
    }

    /// Undo once, and say what the next undo would reach.
    fn label_under_top(commands: &Rc<RefCell<CommandState>>) -> &'static str {
        commands.borrow_mut().history.commit_undo();
        commands
            .borrow()
            .history
            .undo_target()
            .map_or("nothing", |entry| entry.label)
    }

    fn desk() -> Vec<mooloop_core::MidiPortInfo> {
        vec![mooloop_core::MidiPortInfo {
            id: mooloop_core::MidiPortId(0),
            name: "Desk".to_owned(),
        }]
    }

    fn cc(controller: u8, value: u8) -> mooloop_core::MidiMessage {
        mooloop_core::MidiMessage {
            offset: 0,
            port: mooloop_core::MidiPortId(0),
            channel: 0,
            kind: mooloop_core::MidiKind::ControlChange { controller, value },
        }
    }

    fn channel_volume(state: &Rc<RefCell<UiState>>) -> mooloop_core::ControlTarget {
        let id = state.borrow().session.channels[0].id;
        mooloop_core::ControlTarget::Param(mooloop_core::ParamKey::strip(
            mooloop_core::ChainKey::Channel(id),
            mooloop_core::STRIP_PARAM_VOLUME,
        ))
    }

    /// A binding made by MIDI learn is an undo step of its own (MOO-96).
    ///
    /// It was applied in the pump, marked dirty and recorded nowhere: map
    /// eight knobs in one LEARN pass, press Ctrl+Z for an earlier edit, and
    /// the undo installed a snapshot from before all eight. Now the binding
    /// is the entry Ctrl+Z reaches first, the earlier edit is one further
    /// back, and Redo brings the binding back.
    #[test]
    fn a_learned_binding_is_an_undo_step_of_its_own() {
        let (window, state, commands) = undo_fixture();
        state.borrow_mut().midi_ports = desk();
        an_earlier_edit(&state, &commands, &window);
        let target = channel_volume(&state);
        state.borrow_mut().session.begin_control_learn(target, false);

        let drain = drain_control_surface(
            &state,
            &commands,
            &window,
            &mut vec![cc(7, 64)],
            &mut Vec::new(),
            false,
        );
        assert_eq!(drain.learned.len(), 1, "the press bound the knob");

        {
            let open = commands.borrow();
            let entry = open.history.undo_target().expect("the binding is an undo step");
            assert_eq!(entry.label, "MIDI learn");
            assert!(entry.before.project.control_map.bindings.is_empty());
            assert_eq!(entry.after.project.control_map.bindings.len(), 1);
        }
        assert_eq!(label_under_top(&commands), "Rename channel");
        let redo = commands.borrow().history.redo_target().map(|entry| {
            entry.after.project.control_map.bindings.len()
        });
        assert_eq!(redo, Some(1), "and Redo brings the binding back");
    }

    /// A sweep of a mapped knob is one undo step, however many messages it
    /// sends, and the step closes when the knob goes idle (MOO-96).
    #[test]
    fn a_sweep_of_a_mapped_knob_is_one_undo_step() {
        let (window, state, commands) = undo_fixture();
        state.borrow_mut().midi_ports = desk();
        // A binding names its channel by identity, and the window's first
        // channel has none until a project install mints one.
        state.borrow_mut().session.channels[0].id = mooloop_core::ChannelId(0);
        let target = channel_volume(&state);
        {
            let mut st = state.borrow_mut();
            let mut binding = mooloop_core::ControlBinding::new(
                mooloop_core::ControlSource::Cc {
                    port: mooloop_core::MidiPortFilter::Any,
                    channel: mooloop_core::MidiChannelFilter::Omni,
                    controller: 7,
                },
                target,
            );
            binding.mode = mooloop_core::ControlMode::Absolute {
                takeover: mooloop_core::Takeover::Jump,
            };
            st.session.control_map.bind(binding);
            let ports = st.midi_ports.clone();
            st.session.resolve_control_map(&ports);
        }
        an_earlier_edit(&state, &commands, &window);
        let start = state.borrow().session.channels[0].volume;

        for value in [10, 60, 120] {
            let drain = drain_control_surface(
                &state,
                &commands,
                &window,
                &mut vec![cc(7, value)],
                &mut Vec::new(),
                false,
            );
            assert!(drain.controller_moved && drain.edited);
            // A tick where the knob is still moving leaves the step open.
            settle_edit_streams(&state, &commands, &window, false, false);
            assert_eq!(
                commands.borrow().history.open_stream(),
                Some(Stream::Controller)
            );
            assert!(commands.borrow().history.can_undo(), "Undo is live mid-sweep");
        }
        let end = state.borrow().session.channels[0].volume;
        assert_ne!(start, end, "the sweep moved the fader");

        settle_edit_streams(&state, &commands, &window, false, true);
        {
            let open = commands.borrow();
            assert_eq!(open.history.open_stream(), None, "an idle knob closes its step");
            let entry = open.history.undo_target().expect("the sweep is an undo step");
            assert_eq!(entry.before.project.channels[0].setup.channel.volume, start);
            assert_eq!(entry.after.project.channels[0].setup.channel.volume, end);
        }
        assert_eq!(label_under_top(&commands), "Rename channel");
    }

    /// Notes played into an armed pattern are one undo step per take, and
    /// the edit made before the take is one step further back (MOO-97).
    #[test]
    fn a_take_of_recorded_notes_is_one_undo_step() {
        let (window, state, commands) = undo_fixture();
        an_earlier_edit(&state, &commands, &window);
        let _ = state.borrow_mut().session.set_record_armed(true);
        let notes = |project: &Project| project.channels[0].notes[0].len();
        let before = state.borrow().session.channels[0].notes[0].len();

        for (start, note) in [(0u32, 60u8), (96, 62), (192, 64)] {
            let drain = drain_control_surface(
                &state,
                &commands,
                &window,
                &mut Vec::new(),
                &mut vec![(0, 0, note, 100, start, 24)],
                true,
            );
            assert_eq!(drain.written, vec![0]);
            settle_edit_streams(&state, &commands, &window, true, true);
        }
        assert_eq!(
            commands.borrow().history.open_stream(),
            Some(Stream::Recording),
            "a take still playing is still one take"
        );

        // The transport stops: the take is done.
        settle_edit_streams(&state, &commands, &window, false, true);
        {
            let open = commands.borrow();
            let entry = open.history.undo_target().expect("the take is an undo step");
            assert_eq!(entry.label, "Record notes");
            assert_eq!(notes(&entry.before.project), before);
            assert_eq!(notes(&entry.after.project), before + 3);
        }
        assert_eq!(label_under_top(&commands), "Rename channel");
        let redo = commands
            .borrow()
            .history
            .redo_target()
            .map(|entry| notes(&entry.after.project));
        assert_eq!(redo, Some(before + 3), "and Redo brings the take back");
    }

    /// A sample loaded onto a sliced channel is one undo step, and undoing it
    /// brings back the old sample *and* its slices, which the load retired
    /// (MOO-98).
    #[test]
    fn loading_a_sample_onto_a_sliced_channel_is_one_undo_step() {
        let (window, state, commands) = undo_fixture();
        let old = SampleData::default_kick(48_000);
        {
            let mut st = state.borrow_mut();
            let channel = &mut st.session.channels[0];
            assert_eq!(channel.kind, DeviceKind::Sampler);
            channel.sample_data = Some(old.clone());
            channel.sample_path = Some(PathBuf::from("/tmp/old.wav"));
            channel.slices.add(100);
            channel.slices.add(2_000);
        }
        an_earlier_edit(&state, &commands, &window);
        let new = SampleData::default_kick(48_000);
        let mut published = None;
        load_sample_with_history(
            |channel, _| published = Some(channel),
            &state,
            &commands,
            &window.as_weak(),
            0,
            LoadedSample {
                path: PathBuf::from("/tmp/new.wav"),
                sample: new.clone(),
                can_previous: false,
                can_next: false,
            },
        );
        assert_eq!(published, Some(0), "the engine is handed the new audio");

        {
            let open = commands.borrow();
            let entry = open.history.undo_target().expect("the load is an undo step");
            assert_eq!(entry.label, "Load sample");
            let slices = |project: &Project| {
                project.channels[0]
                    .setup
                    .source
                    .sampler_state()
                    .map_or(0, |sampler| sampler.slices.len())
            };
            assert_eq!(slices(&entry.before.project), 2, "undo brings the slices back");
            assert_eq!(slices(&entry.after.project), 0, "the load retired them");
            let id = entry.before.project.channels[0].id;
            assert!(Arc::ptr_eq(&entry.before.samples[&id], &old));
            assert!(Arc::ptr_eq(&entry.after.samples[&id], &new), "and redo the new file");
        }
        assert_eq!(label_under_top(&commands), "Rename channel");
    }

    /// A generator preset load is one undo step, keeps the song playing, and
    /// leaves the history it lands on alone (MOO-95).
    #[test]
    fn a_generator_preset_load_is_one_undo_step_and_keeps_the_song_playing() {
        let (window, state, commands) = undo_fixture();
        an_earlier_edit(&state, &commands, &window);
        let target = LoadTarget::Generator {
            preset_name: "Thump".to_owned(),
        };
        assert!(
            !load_opens_document(&target),
            "a preset edits the song that is playing, so its install keeps the transport"
        );

        let current = state
            .borrow()
            .session
            .project_snapshot(window.get_bpm(), window.get_swing_percent());
        let samples = state.borrow().session.sample_snapshots();
        let (project, samples) = merge_loaded_document(
            current,
            samples,
            target.clone(),
            LoadedDocument::Generator(Box::new(mooloop_core::ChannelSetup::drum_synth("Thump").source)),
            vec![None],
            |_| panic!("a preset has nothing to ask"),
        )
        .expect("a generator preset lands on the selected channel");
        let before = project_snapshot(&state.borrow(), &window);
        // What the pump's install does to the session.
        state.borrow_mut().replace_project(&project, &samples, &window);
        finish_document_load(
            &state,
            &commands,
            &window,
            Path::new("/tmp/thump.mooloop"),
            &target,
            AssetMode::Referenced,
            Some(before),
        );

        {
            let open = commands.borrow();
            let entry = open.history.undo_target().expect("the preset is an undo step");
            assert_eq!(entry.label, "Load preset");
            assert_eq!(entry.before.project.channels[0].setup.source.kind(), DeviceKind::Sampler);
            assert_eq!(entry.after.project.channels[0].setup.source.kind(), DeviceKind::DrumSynth);
        }
        assert!(state.borrow().session.dirty);
        assert_eq!(label_under_top(&commands), "Rename channel");
    }

    /// A kit load edits the song that is open, so it is an undo step and the
    /// history survives it (MOO-95). It used to clear the history, as if
    /// another song had been opened.
    #[test]
    fn a_kit_load_is_one_undo_step_and_keeps_the_history() {
        let (window, state, commands) = undo_fixture();
        an_earlier_edit(&state, &commands, &window);
        let target = LoadTarget::Kit;
        assert!(!load_opens_document(&target));

        let current = state
            .borrow()
            .session
            .project_snapshot(window.get_bpm(), window.get_swing_percent());
        let samples = state.borrow().session.sample_snapshots();
        let kit = mooloop_core::Kit {
            channels: vec![mooloop_core::ChannelSetup::drum_synth("A"), mooloop_core::ChannelSetup::drum_synth("B")],
        };
        let (project, samples) = merge_loaded_document(
            current,
            samples,
            target.clone(),
            LoadedDocument::Kit(kit),
            vec![None, None],
            |_| panic!("a kit longer than the song drops nothing"),
        )
        .expect("the kit merges");
        let before = project_snapshot(&state.borrow(), &window);
        state.borrow_mut().replace_project(&project, &samples, &window);
        finish_document_load(
            &state,
            &commands,
            &window,
            Path::new("/tmp/kit.mooloop"),
            &target,
            AssetMode::Referenced,
            Some(before),
        );

        {
            let open = commands.borrow();
            let entry = open.history.undo_target().expect("the kit is an undo step");
            assert_eq!(entry.label, "Load kit");
            assert_eq!(entry.before.project.channels.len(), 1, "undo brings the old channels back");
            assert_eq!(entry.after.project.channels.len(), 2);
        }
        assert_eq!(label_under_top(&commands), "Rename channel", "the history survived");
    }

    /// Wheel notches on one knob within half a second undo as one step
    /// (MOO-95). Each notch is a `Gesture.begin()`/`end()` of its own, so a
    /// trackpad sweep was forty whole-project entries.
    #[test]
    fn wheel_notches_on_one_knob_undo_as_one_step() {
        let (window, state, commands) = undo_fixture();
        an_earlier_edit(&state, &commands, &window);
        let start = state.borrow().session.channels[0].volume;
        for _ in 0..3 {
            // What `ParameterKnob`'s scroll-event does: begin, one change,
            // end.
            gesture_opened(&state, &commands, &window);
            with_gesture_history(&state, &commands, &window, "Volume", || {
                state.borrow_mut().session.channels[0].volume += 0.05;
                true
            });
            gesture_closed(&state, &commands, &window);
        }
        let end = state.borrow().session.channels[0].volume;

        {
            let open = commands.borrow();
            let entry = open.history.undo_target().expect("the notches are an undo step");
            assert_eq!(entry.label, "Volume");
            assert_eq!(entry.before.project.channels[0].setup.channel.volume, start);
            assert_eq!(entry.after.project.channels[0].setup.channel.volume, end);
        }
        assert_eq!(label_under_top(&commands), "Rename channel", "three notches, one step");
    }
}
