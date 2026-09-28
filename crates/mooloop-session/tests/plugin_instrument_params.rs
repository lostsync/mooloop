//! MOO-82's case for a plugin **instrument** (MOO-312, MOO-315), headless,
//! with the in-repo test sine found through the scanner's cache: its level
//! automated and modulated through the session, exported, saved, reopened
//! present and missing; its own edits kept by undo; its parameters in the
//! lane menu; and a MIDI binding onto it, and onto a plugin effect's.
//!
//! A plugin instrument's parameter is addressed exactly as a plugin
//! effect's: `ParamOwner::PluginParam { device }`, with the id the channel's
//! source slot is given (`ChannelSetup::source_device`) and the plugin's own
//! parameter id.
//!
//! The test thread is the plugins' main thread; every export renders on a
//! thread of its own.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mooloop_core::{
    ChainKey, ControlTarget, EffectParams, EffectTarget, EngineCommand, MidiKind, MidiMessage,
    MidiPortId, MidiPortInfo, ModulatorKind, NoteEvent, ParamAddr, PluginFormat, PluginRef,
    PluginSlotId, PluginState, PluginStateChunk, Project, ProjectChannel,
};
use mooloop_dsp::{AudioNode, SourceNode};
use mooloop_engine::{
    CommandSink, ExportFormat, ExportProgress, ExportSpec, OfflineRenderer, RenderScope,
    StructuralCommand, WavEncoding,
};
use mooloop_plugin_host::clap::ClapOpener;
use mooloop_plugin_host::scan::PluginCache;
use mooloop_plugin_host::{
    AudioConfig, HostError, HostedGui, HostedInstance, IoActivity, IoRegistrations, Lifeline,
    PluginOpener, PluginParamEvent, Requests,
};
use mooloop_session::session::{ArmedRoute, Session};
use mooloop_test_plugin as test_plugin;

const RATE: u32 = 48_000;

fn test_plugin_path() -> PathBuf {
    let exe = std::env::current_exe().expect("the test binary has a path");
    let deps = exe.parent().expect("the test binary is in a directory");
    let name = format!(
        "{}mooloop_test_plugin{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    for dir in [deps, deps.parent().unwrap_or(deps)] {
        let candidate = dir.join(&name);
        if candidate.is_file() {
            return candidate;
        }
    }
    panic!("{name} is not next to {}", exe.display());
}

fn plugin_ref(id: &str, name: &str) -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: id.to_owned(),
        name: name.to_owned(),
        vendor: test_plugin::VENDOR.to_owned(),
        version: String::new(),
    }
}

fn sine_ref() -> PluginRef {
    plugin_ref(test_plugin::SINE_ID, "Test Sine")
}

fn gain_ref() -> PluginRef {
    plugin_ref(test_plugin::GAIN_ID, "Test Gain")
}

/// A scan cache that lists the test sine and the test gain in the test
/// plugin's library, as the scanner would have written it.
fn cache_listing(library: &Path) -> PluginCache {
    let path = library.display().to_string().replace('\\', "/");
    let text = format!(
        "version = 1\n\n[[file]]\npath = \"{path}\"\nmodified-ns = 0\nsize = 0\n\n\
         [[file.plugin]]\npath = \"{path}\"\nformat = \"clap\"\nid = \"{sine}\"\nname = \"Test Sine\"\n\
         vendor = \"{vendor}\"\nfeatures = [\"instrument\"]\naudio-inputs = []\naudio-outputs = [2]\nnote-inputs = 1\n\n\
         [[file.plugin]]\npath = \"{path}\"\nformat = \"clap\"\nid = \"{gain}\"\nname = \"Test Gain\"\n\
         vendor = \"{vendor}\"\nfeatures = [\"audio-effect\"]\naudio-inputs = [2]\naudio-outputs = [2]\n",
        sine = test_plugin::SINE_ID,
        gain = test_plugin::GAIN_ID,
        vendor = test_plugin::VENDOR,
    );
    PluginCache::from_toml(&text).expect("a cache the scanner could have written")
}

/// One channel with a one-bar melody.
fn melody() -> Project {
    let mut project = Project::default();
    project.channels.clear();
    project.pattern_lengths[0] = 16;
    let mut channel = ProjectChannel::drum_synth(0, 1);
    channel.setup.channel.volume = 0.8;
    for (id, (step, key)) in [(0u32, 57u8), (4, 60), (9, 64), (12, 69)].into_iter().enumerate() {
        channel.notes[0].push(NoteEvent::new(id as u32 + 1, step * 24, 18, key, 100));
    }
    project.channels.push(channel);
    project.assign_channel_ids();
    project
}

/// Stands in for the engine, and keeps every node it is handed.
#[derive(Default)]
struct Engine {
    sources: Vec<Box<dyn SourceNode + Send>>,
    nodes: Vec<Box<dyn AudioNode + Send>>,
}

impl Engine {
    /// Hand every processor back, as the engine does once it has retired
    /// them.
    fn release(&mut self) {
        self.sources.clear();
        self.nodes.clear();
    }
}

impl CommandSink for Engine {
    fn send(&mut self, _cmd: EngineCommand) -> bool {
        true
    }
    fn send_structural(&mut self, cmd: StructuralCommand) -> bool {
        match cmd {
            StructuralCommand::InstallSource { node, .. } => self.sources.push(node),
            StructuralCommand::HostSourceProcessor { node: Some(node), .. }
            | StructuralCommand::ReplaceEffect { node, .. }
            | StructuralCommand::InstallEffect { node, .. } => self.nodes.push(node),
            _ => {}
        }
        true
    }
    fn send_deferred(&mut self, _cmd: EngineCommand, _when: mooloop_core::MusicalEdge) -> bool {
        true
    }
    fn sample_rate(&self) -> u32 {
        RATE
    }
}

fn session_with(opener: Box<dyn PluginOpener>, project: &Project) -> Session {
    let mut session = Session::default();
    session.set_plugin_opener(opener);
    session.replace_project(project, &[]);
    session
}

fn found(library: &Path) -> Box<dyn PluginOpener> {
    Box::new(ClapOpener::with_cache(cache_listing(library)))
}

fn snapshot(session: &Session) -> Project {
    let song = melody();
    session.project_snapshot(song.bpm.into(), song.swing_percent.into())
}

/// A session whose channel 0 plays the test sine, and the address of the
/// sine's level.
fn with_sine(opener: Box<dyn PluginOpener>, engine: &mut Engine) -> (Session, PluginSlotId, ParamAddr) {
    let mut session = session_with(opener, &melody());
    let slot = session.set_plugin_source(0, sine_ref(), engine).expect("channel 0 exists");
    assert!(session.plugin_problem(slot).is_none(), "{:?}", session.plugin_problem(slot));
    let device = session.channels[0].source_device;
    assert!(device.is_assigned(), "the source slot has an id");
    let level = ParamAddr::plugin_param(EffectTarget::Channel(0), device, test_plugin::PARAM_LEVEL);
    (session, slot, level)
}

fn render(
    project: &Project,
    plugins: BTreeMap<PluginSlotId, Box<dyn AudioNode + Send>>,
    dir: &Path,
    name: &str,
) -> Vec<f32> {
    let path = dir.join(name);
    let spec = ExportSpec {
        path: path.clone(),
        scope: RenderScope::Pattern { index: 0 },
        tail_seconds: 0.0,
        format: ExportFormat::Wav(WavEncoding::Float32),
    };
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                OfflineRenderer::render_with_plugins(project, &[], RATE, &spec, &ExportProgress::new(), plugins)
            })
            .join()
            .expect("the export thread did not panic")
    })
    .expect("the export succeeds");
    hound::WavReader::open(&path)
        .expect("a WAV")
        .into_samples::<f32>()
        .map(|sample| sample.expect("a float sample"))
        .collect()
}

fn export(session: &mut Session, dir: &Path, name: &str) -> Vec<f32> {
    let plugins = session.export_plugin_processors(RATE);
    render(&snapshot(session), plugins, dir, name)
}

fn reopen(path: &Path) -> Project {
    let report = mooloop_project::load_bundle(path).expect("the song reopens");
    assert!(report.repairs.is_empty(), "the song needed repairs: {:?}", report.repairs);
    match report.document {
        mooloop_project::LoadedDocument::Song(project) => project,
        _ => panic!("a song came back as something else"),
    }
}

fn save(session: &mut Session, path: &Path) -> Project {
    session.capture_plugin_states();
    let song = snapshot(session);
    mooloop_project::save_song(path, &song, mooloop_project::AssetMode::Referenced).expect("it saves");
    song
}

/// **An automated, modulated instrument round-trips.** A lane and a route
/// on the sine's level, authored through the session, are heard in the
/// export; saved and reopened, the song exports the same file. Reopened
/// with the plugin missing and saved, the lane, the route, the slot and the
/// source's id all come through; reopened with the plugin back, it exports
/// the same file again (MOO-74's never-drop, for a source).
#[test]
fn an_automated_modulated_instrument_saves_reopens_and_survives_going_missing() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let library = test_plugin_path();
    let mut engine = Engine::default();
    let (mut session, slot, level) = with_sine(found(&library), &mut engine);

    // What the session knows about the level, through the source arm.
    let info = session.plugin_param_info(level).expect("the song knows the level");
    assert_eq!((info.min, info.max), (test_plugin::LEVEL_DB_MIN, test_plugin::LEVEL_DB_MAX));
    assert!(session.lane_allowed(level));
    assert!(session.modulation_policy(level).expect("a policy").allowed);

    let plain = export(&mut session, dir.path(), "plain.wav");

    session.open_automation_lane_at(level).expect("a lane opens on the level");
    session.create_automation_point(0, 0.9).expect("a point");
    session.create_automation_point(16 * 24 - 1, 0.3).expect("a point");
    let automated = export(&mut session, dir.path(), "automated.wav");
    assert_ne!(automated, plain, "the lane is heard");

    session.add_modulation_source(ModulatorKind::Lfo).expect("room for a modulator");
    assert!(session.toggle_modulation_assignment().is_some(), "the LFO is armed");
    assert!(
        matches!(session.arm_modulation_route(level, 0.5), ArmedRoute::Added(_)),
        "the route is authored on the instrument's level"
    );
    let modulated = export(&mut session, dir.path(), "modulated.wav");
    assert_ne!(modulated, automated, "the route is heard");

    // Saved and reopened, present: the same export.
    let song_path = dir.path().join("song.mooloop");
    let saved = save(&mut session, &song_path);
    assert!(saved.channels[0].setup.source_device.is_assigned());
    let reopened = reopen(&song_path);
    assert_eq!(reopened.plugins, saved.plugins);
    assert_eq!(reopened.channels[0].setup.source_device, saved.channels[0].setup.source_device);
    let mut present = session_with(found(&library), &reopened);
    let mut engine = Engine::default();
    present.service_plugins(&mut engine);
    assert!(present.plugin_problem(slot).is_none());
    assert_eq!(export(&mut present, dir.path(), "reopened.wav"), modulated);

    // Missing, saved: nothing it named is dropped.
    let mut missing = session_with(Box::new(ClapOpener::with_cache(PluginCache::default())), &reopened);
    missing.service_plugins(&mut engine);
    assert_eq!(missing.plugin_problem(slot), Some(HostError::Missing));
    let kept_path = dir.path().join("kept.mooloop");
    save(&mut missing, &kept_path);
    let kept = reopen(&kept_path);
    assert_eq!(kept.plugins, saved.plugins, "the slot came through untouched");
    assert_eq!(kept.channels[0].setup.source, saved.channels[0].setup.source);
    assert_eq!(kept.channels[0].setup.source_device, saved.channels[0].setup.source_device);
    assert_eq!(kept.channels[0].automation, saved.channels[0].automation, "and the lane");
    assert_eq!(kept.channels[0].setup.modulation, saved.channels[0].setup.modulation, "and the route");

    // Back: the same export.
    let mut back = session_with(found(&library), &kept);
    back.service_plugins(&mut engine);
    assert!(back.plugin_problem(slot).is_none());
    assert_eq!(export(&mut back, dir.path(), "back.wav"), modulated);
}

/// A knob on the instrument's face: an index in, the plugin's id and plain
/// value out, in the command the engine takes for a plugin source, and
/// counted as an edit of the plugin, closed when it goes quiet.
#[test]
fn a_knob_on_the_instrument_sends_its_id_and_plain_value_and_is_an_edit() {
    let library = test_plugin_path();
    let mut engine = Engine::default();
    let (mut session, slot, _) = with_sine(found(&library), &mut engine);
    let index = session.plugin_param_index(slot, test_plugin::PARAM_LEVEL).expect("listed");
    assert_eq!(
        session.set_plugin_source_param(index, 0.5),
        Some(EngineCommand::SetChannelGeneratorParam {
            channel: 0,
            id: test_plugin::PARAM_LEVEL,
            value: -18.0,
        })
    );
    assert_eq!(session.plugin_param_value(slot, index), Some(-18.0));
    assert_eq!(session.set_plugin_source_param(index + 1, 0.5), None, "no such parameter");
    let quiet = mooloop_session::plugin_rack::EDIT_QUIET_TICKS as usize;
    for _ in 0..quiet {
        assert!(!session.plugin_edits_pending());
        session.service_plugins(&mut engine);
    }
    assert!(session.plugin_edits_pending(), "a quiet run is one edit");
}

/// The lane menu lists the instrument's parameters first, under the
/// plugin's name, then the chain's; a lane on an id the plugin stopped
/// listing is kept and listed as missing.
#[test]
fn plugin_destinations_list_the_instruments_parameters_first_missing_ones_marked() {
    let library = test_plugin_path();
    let mut engine = Engine::default();
    let (mut session, slot, level) = with_sine(found(&library), &mut engine);
    session.insert_plugin_effect(gain_ref(), 0, &mut engine).expect("the gain is inserted");
    let rows = session.plugin_destinations();
    let first = rows.first().expect("rows");
    assert_eq!(
        (first.address, first.device.as_str(), first.name.as_str(), first.missing),
        (level, "Test Sine", "Level", false)
    );
    assert!(rows[1..].iter().all(|row| row.device == "Test Gain 1"), "{rows:?}");
    assert!(rows.iter().any(|row| row.name == "Gain"));

    session.open_automation_lane_at(level).expect("a lane");
    session.plugins.get_mut(&slot).expect("the slot").params.clear();
    let rows = session.plugin_destinations();
    let first = rows.first().expect("rows");
    assert_eq!(
        (first.address, first.device.as_str(), first.name.as_str(), first.missing, first.lane_allowed),
        (level, "Test Sine", "Parameter 50", true, false)
    );
}

fn ports() -> Vec<MidiPortInfo> {
    vec![MidiPortInfo {
        id: MidiPortId(0),
        name: "Launchkey MK3".to_owned(),
    }]
}

fn cc(controller: u8, value: u8) -> MidiMessage {
    MidiMessage {
        offset: 0,
        port: MidiPortId(0),
        channel: 0,
        kind: MidiKind::ControlChange { controller, value },
    }
}

/// Learn `address` onto CC `controller`, then sweep the control down, up
/// and down again, so a pickup catches it on the way. Returns the label
/// the mapping list shows and the last command the sweep sent.
fn learn_and_sweep(session: &mut Session, address: ParamAddr, controller: u8) -> (String, EngineCommand) {
    let key = session.param_key(address).expect("a durable key");
    assert!(matches!(key.scope, ChainKey::Channel(_)));
    let target = ControlTarget::Param(key);
    session.begin_control_learn(target, false);
    let learned = session.apply_control_input(&cc(controller, 64), &ports(), false);
    assert!(learned.learned.is_some(), "the control was learned");
    let mut last = None;
    for value in [0, 127, 0] {
        let effects = session.apply_control_input(&cc(controller, value), &ports(), false);
        if let Some(command) = effects.commands.last() {
            assert_eq!(effects.moved, [address]);
            last = Some(command.clone());
        }
    }
    (session.control_target_label(&target), last.expect("the sweep moved the parameter"))
}

/// **MIDI learn binds a plugin's parameter**, the instrument's and an
/// effect's: the mapping names the parameter by the plugin's own name for
/// it, and a CC moves it with the command its knob sends.
#[test]
fn midi_learn_binds_an_instruments_and_an_effects_plugin_parameter() {
    let library = test_plugin_path();
    let mut engine = Engine::default();
    let (mut session, slot, level) = with_sine(found(&library), &mut engine);
    let inserted = session.insert_plugin_effect(gain_ref(), 0, &mut engine).expect("inserted");
    let EffectParams::Plugin(gain_slot) = inserted.params else {
        unreachable!("a plugin device");
    };
    let channel = session.channels[0].name.clone();

    let (label, command) = learn_and_sweep(&mut session, level, 20);
    assert_eq!(label, format!("{channel} \u{b7} Test Sine \u{b7} Level"));
    assert_eq!(
        command,
        EngineCommand::SetChannelGeneratorParam {
            channel: 0,
            id: test_plugin::PARAM_LEVEL,
            value: test_plugin::LEVEL_DB_MIN as f32,
        }
    );
    let index = session.plugin_param_index(slot, test_plugin::PARAM_LEVEL).expect("listed");
    assert_eq!(session.plugin_param_value(slot, index), Some(test_plugin::LEVEL_DB_MIN));

    let gain = ParamAddr::plugin_param(EffectTarget::Channel(0), inserted.device, test_plugin::PARAM_GAIN);
    let (label, command) = learn_and_sweep(&mut session, gain, 21);
    assert_eq!(label, format!("{channel} \u{b7} Test Gain 1 \u{b7} Gain"));
    assert_eq!(
        command,
        EngineCommand::SetEffectParam {
            target: EffectTarget::Channel(0),
            slot: 0,
            id: test_plugin::PARAM_GAIN,
            value: test_plugin::GAIN_DB_MIN as f32,
        }
    );
    let index = session.plugin_param_index(gain_slot, test_plugin::PARAM_GAIN).expect("listed");
    assert_eq!(session.plugin_param_value(gain_slot, index), Some(test_plugin::GAIN_DB_MIN));

    // A parameter the plugin stopped listing: the binding is kept, reads as
    // unavailable, and moves nothing.
    session.plugins.get_mut(&slot).expect("the slot").params.clear();
    let key = session.param_key(level).expect("a key");
    assert_eq!(session.control_target_label(&ControlTarget::Param(key)), "Unavailable parameter");
    assert_eq!(session.set_param_normalized(level, 0.5), None);
}

/// The chunk [`WithPanel`] keeps its level in.
const PANEL_TAG: &str = "mooloop.test.panel";

/// What the instrument's own GUI has been asked to do, shared between a
/// test and every instance [`PanelOpener`] makes.
#[derive(Default)]
struct Panel {
    /// Clicks not yet seen: each moves the level to this many dB.
    clicks: Vec<f64>,
}

/// The test sine, opened through the scan cache, with the two things a
/// plugin with a GUI has that the sine does not: a value the plugin moves
/// itself and reports as a gesture, and a state that holds it. The session
/// sees exactly what it sees of a real plugin's GUI edit -- a gesture on the
/// plugin's report ring, then a different saved state -- and nothing else
/// about the sine is changed.
struct PanelOpener {
    inner: ClapOpener,
    panel: Arc<Mutex<Panel>>,
}

impl PluginOpener for PanelOpener {
    fn open(
        &mut self,
        plugin: &PluginRef,
        state: &PluginState,
        config: AudioConfig,
    ) -> Result<Box<dyn HostedInstance>, HostError> {
        let (panel, rest): (Vec<_>, Vec<_>) =
            state.chunks.iter().cloned().partition(|chunk| chunk.tag == PANEL_TAG);
        let level = panel
            .first()
            .map(|chunk| f64::from_le_bytes(chunk.data[..8].try_into().expect("eight bytes")))
            .unwrap_or(test_plugin::LEVEL_DB_DEFAULT);
        let inner = self.inner.open(plugin, &PluginState { chunks: rest }, config)?;
        Ok(Box::new(WithPanel {
            inner,
            panel: self.panel.clone(),
            level,
        }))
    }

    fn refresh(&mut self) -> u64 {
        self.inner.refresh()
    }
}

struct WithPanel {
    inner: Box<dyn HostedInstance>,
    panel: Arc<Mutex<Panel>>,
    level: f64,
}

impl HostedInstance for WithPanel {
    fn plugin(&self) -> &PluginRef {
        self.inner.plugin()
    }
    fn params(&self) -> &[mooloop_core::PluginParamInfo] {
        self.inner.params()
    }
    fn latency_frames(&self) -> u32 {
        self.inner.latency_frames()
    }
    fn save_state(&mut self) -> Result<PluginState, HostError> {
        let mut state = self.inner.save_state()?;
        state.chunks.push(PluginStateChunk {
            tag: PANEL_TAG.to_owned(),
            data: self.level.to_le_bytes().to_vec(),
        });
        Ok(state)
    }
    fn load_state(&mut self, state: &PluginState) -> Result<(), HostError> {
        if let Some(chunk) = state.chunks.iter().find(|chunk| chunk.tag == PANEL_TAG) {
            self.level = f64::from_le_bytes(chunk.data[..8].try_into().expect("eight bytes"));
        }
        let rest = state.chunks.iter().filter(|chunk| chunk.tag != PANEL_TAG).cloned().collect();
        self.inner.load_state(&PluginState { chunks: rest })
    }
    fn value_text(&mut self, id: u32, value: f64) -> Option<String> {
        self.inner.value_text(id, value)
    }
    fn take_requests(&self) -> Requests {
        self.inner.take_requests()
    }
    fn on_main_thread(&mut self) {
        self.inner.on_main_thread();
    }
    fn refresh_params(&mut self) {
        self.inner.refresh_params();
    }
    fn param_value(&mut self, id: u32) -> Option<f64> {
        if id == test_plugin::PARAM_LEVEL {
            return Some(self.level);
        }
        self.inner.param_value(id)
    }
    fn drain_param_events(&mut self, sink: &mut dyn FnMut(PluginParamEvent)) {
        self.inner.drain_param_events(sink);
        let clicks = std::mem::take(&mut self.panel.lock().expect("the panel").clicks);
        for level in clicks {
            let id = test_plugin::PARAM_LEVEL;
            self.level = level;
            sink(PluginParamEvent::GestureBegin { id });
            sink(PluginParamEvent::Value { id, value: level });
            sink(PluginParamEvent::GestureEnd { id });
        }
    }
    fn dropped_param_events(&self) -> u64 {
        self.inner.dropped_param_events()
    }
    fn failed(&self) -> bool {
        self.inner.failed()
    }
    fn fits_effect(&self) -> bool {
        self.inner.fits_effect()
    }
    fn fits_source(&self) -> bool {
        self.inner.fits_source()
    }
    fn generated_notes(&self) -> u64 {
        self.inner.generated_notes()
    }
    fn gui(&mut self) -> Option<&mut dyn HostedGui> {
        self.inner.gui()
    }
    fn service_io(&mut self, now: std::time::Instant) -> IoActivity {
        self.inner.service_io(now)
    }
    fn io_registrations(&self) -> IoRegistrations {
        self.inner.io_registrations()
    }
    fn set_audio_config(&mut self, config: AudioConfig) {
        self.inner.set_audio_config(config);
    }
    fn build_processor(&mut self, lifeline: Lifeline) -> Result<Box<dyn AudioNode + Send>, HostError> {
        self.inner.build_processor(lifeline)
    }
}

/// Where the live instance in `slot` sits in memory: the same address
/// after an install means the instance was kept, not reopened.
fn instance_address(session: &Session, slot: PluginSlotId) -> usize {
    session
        .plugin_rack
        .instance(slot)
        .map(|instance| instance as *const dyn HostedInstance as *const () as usize)
        .expect("a live instance")
}

/// **An edit in the instrument's own GUI is one undo step, and an unrelated
/// undo leaves it alone** -- MOO-82's case, on a channel's source.
///
/// The level is moved in the plugin's GUI and reported as a gesture. The
/// session sends nothing back and has one edit ready; the pump records it as
/// a step around `capture_plugin_edits` ("Plugin Edit"), simulated here with
/// the same two snapshots. An unrelated edit and its undo keep the same
/// instance, still moved; undoing the plugin's own step takes it back.
#[test]
fn an_instruments_own_edit_is_one_undo_step_and_survives_an_unrelated_undo() {
    let library = test_plugin_path();
    let panel = Arc::new(Mutex::new(Panel::default()));
    let opener = PanelOpener {
        inner: ClapOpener::with_cache(cache_listing(&library)),
        panel: panel.clone(),
    };
    let mut engine = Engine::default();
    let (mut session, slot, _) = with_sine(Box::new(opener), &mut engine);
    session.capture_plugin_states();
    let level = session.plugin_param_index(slot, test_plugin::PARAM_LEVEL).expect("listed");
    assert_eq!(session.plugin_param_value(slot, level), Some(0.0));

    // The click.
    panel.lock().expect("the panel").clicks.push(-6.0);
    session.service_plugins(&mut engine);
    assert_eq!(session.plugin_param_value(slot, level), Some(-6.0), "what the plugin reported");
    assert!(session.plugin_edits_pending(), "the ended gesture is one edit");

    // The pump's step: before, capture, after.
    let before_click = snapshot(&session);
    assert!(session.capture_plugin_edits(), "the instrument's state changed");
    assert!(!session.plugin_edits_pending());
    let after_click = snapshot(&session);
    assert_ne!(before_click.plugins[&slot].state, after_click.plugins[&slot].state);

    // An unrelated edit, then its undo: the same instance, still moved.
    let kept = instance_address(&session, slot);
    session.channels[0].name = "renamed".into();
    let _after_rename = snapshot(&session);
    session.replace_project(&after_click, &[]);
    session.service_plugins(&mut engine);
    assert_eq!(instance_address(&session, slot), kept, "the unrelated undo reopened the instrument");
    assert_eq!(session.plugin_param_value(slot, level), Some(-6.0));

    // Undoing the instrument's own step takes the click back.
    session.replace_project(&before_click, &[]);
    engine.release();
    for _ in 0..3 {
        session.service_plugins(&mut engine);
    }
    assert_eq!(session.plugin_param_value(slot, level), Some(0.0), "the undo restored the level");
}
