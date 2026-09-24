//! Step 07's case, headless, with the in-repo test plugin
//! (`docs/plans/plugin-hosting/07-parameters-and-state.md`, MOO-82): a hosted
//! plugin's parameters automated and edited through the session, its state
//! saved and reopened, and its own edits kept by undo.
//!
//! `examples/clap_automation_case.rs` is the same case with a real
//! third-party plugin (LSP's filter), for a machine that has one.
//!
//! The test thread is the plugins' main thread; every processor runs on a
//! thread of its own, and every export renders on one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mooloop_core::{
    EffectParams, EffectTarget, NoteEvent, ParamAddr, PluginFormat, PluginRef, PluginSlotId,
    PluginState, PluginStateChunk, PluginStateText, Project, ProjectChannel,
};
use mooloop_dsp::{AudioNode, Event, EventList, ProcessContext, StereoBus, TimedEvent};
use mooloop_engine::{
    CommandSink, ExportFormat, ExportProgress, ExportSpec, OfflineRenderer, RenderScope,
    StructuralCommand, WavEncoding,
};
use mooloop_plugin_host::clap::{ClapOpener, STATE_TAG};
use mooloop_plugin_host::scan::PluginCache;
use mooloop_session::session::Session;
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

fn gain_ref() -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: test_plugin::GAIN_ID.to_owned(),
        name: "Test Gain".to_owned(),
        vendor: test_plugin::VENDOR.to_owned(),
        version: String::new(),
    }
}

/// A scan cache that lists the test plugin's library, as the scanner would
/// have written it.
fn cache_listing(library: &Path) -> PluginCache {
    let path = library.display().to_string().replace('\\', "/");
    let text = format!(
        "version = 1\n\n[[file]]\npath = \"{path}\"\nmodified-ns = 0\nsize = 0\n\n\
         [[file.plugin]]\npath = \"{path}\"\nformat = \"clap\"\nid = \"{id}\"\nname = \"Test Gain\"\n\
         vendor = \"{vendor}\"\nfeatures = [\"audio-effect\"]\naudio-inputs = [2]\naudio-outputs = [2]\n",
        id = test_plugin::GAIN_ID,
        vendor = test_plugin::VENDOR,
    );
    PluginCache::from_toml(&text).expect("a cache the scanner could have written")
}

/// The test gain's gain, in dB, read out of a saved state.
fn saved_gain(state: &PluginStateText) -> f64 {
    let chunk = state.0.chunks.iter().find(|chunk| chunk.tag == STATE_TAG).expect("a CLAP chunk");
    assert_eq!(chunk.data[..4], test_plugin::STATE_MAGIC);
    f64::from_le_bytes(chunk.data[4..12].try_into().expect("eight bytes"))
}

/// One drum-synth channel, a one-bar loop.
fn drum_loop() -> Project {
    let mut project = Project::default();
    project.channels.clear();
    project.pattern_lengths[0] = 16;
    let mut channel = ProjectChannel::drum_synth(0, 1);
    channel.setup.channel.volume = 0.4;
    for (step, pitch) in [(0u32, 36u8), (4, 38), (8, 36), (12, 42)] {
        channel.notes[0].push(NoteEvent::new(step + 1, step * 24, 12, pitch, 110));
    }
    project.channels.push(channel);
    project.assign_channel_ids();
    project
}

/// Stands in for the engine: takes every command, and holds every node it
/// is handed for as long as the engine would.
#[derive(Default)]
struct Engine {
    nodes: Vec<Box<dyn AudioNode + Send>>,
}

impl CommandSink for Engine {
    fn send(&mut self, _cmd: mooloop_core::EngineCommand) -> bool {
        true
    }
    fn send_structural(&mut self, cmd: StructuralCommand) -> bool {
        match cmd {
            StructuralCommand::InstallEffect { node, .. }
            | StructuralCommand::ReplaceEffect { node, .. } => self.nodes.push(node),
            _ => {}
        }
        true
    }
    fn send_deferred(&mut self, _cmd: mooloop_core::EngineCommand, _when: mooloop_core::MusicalEdge) -> bool {
        true
    }
    fn sample_rate(&self) -> u32 {
        RATE
    }
}

fn session_with(cache: PluginCache, project: &Project) -> Session {
    let mut session = Session::default();
    session.set_plugin_opener(Box::new(ClapOpener::with_cache(cache)));
    session.replace_project(project, &[]);
    session
}

fn snapshot(session: &Session) -> Project {
    session.project_snapshot(120, 0)
}

/// Run one 256-frame block of `events` through `node` on a thread of its
/// own, as the audio thread would, and hand the node back.
fn process(mut node: Box<dyn AudioNode + Send>, events: Vec<TimedEvent>) -> Box<dyn AudioNode + Send> {
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let mut bus = StereoBus::with_capacity(256);
                let mut list = EventList::empty();
                for event in events {
                    assert!(list.push_ordered(event));
                }
                let ctx = ProcessContext {
                    sample_rate: RATE,
                    frames: 256,
                    playing: true,
                    bpm: 120.0,
                    position_ticks: 0.0,
                    position_frames: 0,
                };
                node.process(&ctx, &mut bus, &list, None);
                node
            })
            .join()
            .expect("the processing thread did not panic")
    })
}

/// A session with the test gain inserted on channel 0, the engine holding
/// its processor, and the plugin's slot.
fn hosted() -> (Session, Engine, PluginSlotId) {
    let project = drum_loop();
    let mut session = session_with(cache_listing(&test_plugin_path()), &project);
    let mut engine = Engine::default();
    let inserted = session
        .insert_plugin_effect(gain_ref(), 0, &mut engine)
        .expect("the device is inserted");
    let EffectParams::Plugin(slot) = inserted.params else {
        unreachable!("a plugin device");
    };
    assert!(session.plugin_problem(slot).is_none());
    (session, engine, slot)
}

/// Where the live instance in `slot` sits in memory: the same address
/// after an install means the instance was kept, not reopened.
fn instance_address(session: &Session, slot: PluginSlotId) -> usize {
    session
        .plugin_rack
        .instance(slot)
        .map(|instance| instance as *const dyn mooloop_plugin_host::HostedInstance as *const () as usize)
        .expect("a live instance")
}

/// **A plugin's own edit is one undo step, and an unrelated undo leaves it
/// alone** (the orchestrator's condition on MOO-82).
///
/// The test gain is nudged, as a click in its GUI would: it moves its own
/// gain a decibel and reports the gesture. The session reads that off the
/// plugin's ring, sends nothing back, and has one edit ready; the pump
/// records it as a step around `capture_plugin_edits`, which is simulated
/// here with the same two snapshots. Then an unrelated edit and its undo:
/// the plugin's instance is the same one, still a decibel up. Undoing the
/// plugin's own step reopens it at 0 dB.
#[test]
fn a_plugins_own_edit_is_one_undo_step_and_survives_an_unrelated_undo() {
    let (mut session, mut engine, slot) = hosted();
    // Saved once, so the song holds the plugin's state at 0 dB.
    session.capture_plugin_states();
    let gain = session.plugin_param_index(slot, test_plugin::PARAM_GAIN).expect("listed");
    assert_eq!(session.plugin_param_value(slot, gain), Some(0.0));

    // The click: the processor hears a nudge and moves its own gain.
    let node = engine.nodes.pop().expect("the processor");
    let node = process(
        node,
        vec![TimedEvent {
            offset: 10,
            event: Event::ParamValue {
                id: test_plugin::PARAM_NUDGE,
                value: 1.0,
            },
        }],
    );
    engine.nodes.push(node);
    session.service_plugins(&mut engine);
    assert_eq!(
        session.plugin_param_value(slot, gain),
        Some(test_plugin::NUDGE_DB),
        "the value the plugin reported is what the session knows"
    );
    assert!(session.plugin_edits_pending(), "the ended gesture is one edit");

    // The pump's step: before, capture, after.
    let before_nudge = snapshot(&session);
    assert!(session.capture_plugin_edits());
    assert!(!session.plugin_edits_pending());
    let after_nudge = snapshot(&session);
    assert_eq!(saved_gain(&before_nudge.plugins[&slot].state), 0.0);
    assert_eq!(saved_gain(&after_nudge.plugins[&slot].state), test_plugin::NUDGE_DB);

    // An unrelated edit, then its undo: the snapshot either side carries
    // the nudged state, so the same instance stays, still nudged.
    let kept = instance_address(&session, slot);
    session.channels[0].name = "renamed".into();
    let _after_rename = snapshot(&session);
    session.replace_project(&after_nudge, &[]);
    session.service_plugins(&mut engine);
    assert_eq!(instance_address(&session, slot), kept, "the unrelated undo reopened the plugin");
    assert_eq!(session.plugin_param_value(slot, gain), Some(test_plugin::NUDGE_DB));

    // Undoing the plugin's own step takes the nudge back.
    session.replace_project(&before_nudge, &[]);
    for node in engine.nodes.drain(..) {
        drop(node);
    }
    for _ in 0..3 {
        session.service_plugins(&mut engine);
    }
    assert_eq!(session.plugin_param_value(slot, gain), Some(0.0), "the undo restored the plugin's gain");
}

/// **A value sent from the session is an edit too, closed when it goes
/// quiet**, and it is the ordinary `SetEffectParam` with the plugin's own
/// id -- above `i32::MAX` for the nudge -- and plain value.
#[test]
fn a_knob_edit_sends_the_plugins_id_and_closes_as_one_edit_when_quiet() {
    let (mut session, mut engine, slot) = hosted();
    let index = session.plugin_param_index(slot, test_plugin::PARAM_GAIN).expect("listed");
    let command = session.set_plugin_param(0, index, 0.5).expect("a command");
    assert_eq!(
        command,
        mooloop_core::EngineCommand::SetEffectParam {
            target: EffectTarget::Channel(0),
            slot: 0,
            id: test_plugin::PARAM_GAIN,
            value: -24.0,
        }
    );
    assert_eq!(session.plugin_param_value(slot, index), Some(-24.0));
    let quiet = mooloop_session::plugin_rack::EDIT_QUIET_TICKS as usize;
    for tick in 0..quiet {
        assert!(!session.plugin_edits_pending(), "closed early, at tick {tick}");
        session.service_plugins(&mut engine);
    }
    assert!(session.plugin_edits_pending(), "a quiet run is one edit");
}

/// **A state the plugin refuses opens it with its defaults, and the song
/// keeps the state.** Neither a save's capture nor anything else writes the
/// defaults over the only copy of what was saved.
#[test]
fn a_refused_state_plays_the_defaults_and_is_kept_byte_for_byte() {
    let mut project = drum_loop();
    let mut engine = Engine::default();
    let refused = PluginState {
        chunks: vec![PluginStateChunk {
            tag: STATE_TAG.to_owned(),
            data: b"not a test-gain state".to_vec(),
        }],
    };
    let slot = project.add_plugin_slot(mooloop_core::PluginSlotState {
        state: PluginStateText(refused.clone()),
        ..mooloop_core::PluginSlotState::new(gain_ref())
    });
    let mut device = mooloop_core::EffectSlotState::of_kind(mooloop_core::EffectKind::Plugin);
    device.params = EffectParams::Plugin(slot);
    project.channels[0].setup.push_effect(device).expect("room");
    let mut session = session_with(cache_listing(&test_plugin_path()), &project);
    session.service_plugins(&mut engine);
    assert!(session.plugin_rack.instance(slot).is_some(), "it opened, with its defaults");
    assert!(session.plugin_problem(slot).is_some(), "and the device says why");
    let gain = session.plugin_param_index(slot, test_plugin::PARAM_GAIN).expect("listed");
    assert_eq!(session.plugin_param_value(slot, gain), Some(test_plugin::GAIN_DB_DEFAULT));
    session.capture_plugin_states();
    assert_eq!(snapshot(&session).plugins[&slot].state.0, refused, "the song kept the refused state");
}

/// Export `project` with `plugins` on a thread of its own and read it back.
fn export(project: &Project, plugins: BTreeMap<PluginSlotId, Box<dyn AudioNode + Send>>, dir: &Path, name: &str) -> Vec<f32> {
    let path = dir.join(name);
    let spec = ExportSpec {
        path: path.clone(),
        scope: RenderScope::Pattern { index: 0 },
        tail_seconds: 0.0,
        format: ExportFormat::Wav(WavEncoding::Float32),
    };
    std::thread::scope(|scope| {
        scope
            .spawn(|| OfflineRenderer::render_with_plugins(project, &[], RATE, &spec, &ExportProgress::new(), plugins))
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

/// **An automated plugin round-trips.** A lane drawn on the test gain
/// through the session is heard in the export; saved and reopened, the song
/// exports the same file. Reopened with the plugin missing, saved, and
/// reopened with it back, it exports the same file again: the lane and the
/// state came through the missing plugin untouched (MOO-74's never-drop).
#[test]
fn an_automated_plugin_saves_reopens_and_survives_going_missing() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let library = test_plugin_path();
    let (mut session, _engine, slot) = hosted();
    let device = session.channels[0].effects[0].id;
    let gain = ParamAddr::plugin_param(EffectTarget::Channel(0), device, test_plugin::PARAM_GAIN);
    session.open_automation_lane_at(gain).expect("a lane opens on the gain");
    session.create_automation_point(0, 0.8).expect("a point");
    session.create_automation_point(16 * 24 - 1, 0.2).expect("a point");

    let static_song = {
        let mut song = snapshot(&session);
        song.channels[0].automation[0].clear();
        song
    };
    let plugins = session.export_plugin_processors(RATE);
    let unautomated = export(&static_song, plugins, dir.path(), "static.wav");
    let plugins = session.export_plugin_processors(RATE);
    let automated = export(&snapshot(&session), plugins, dir.path(), "automated.wav");
    assert_ne!(automated, unautomated, "the lane is heard");

    // Saved and reopened, present.
    let song_path = dir.path().join("song.mooloop");
    session.capture_plugin_states();
    let saved = snapshot(&session);
    mooloop_project::save_song(&song_path, &saved, mooloop_project::AssetMode::Referenced).expect("it saves");
    let reopen = |path: &Path| {
        let report = mooloop_project::load_bundle(path).expect("it reopens");
        assert!(report.repairs.is_empty(), "no repairs: {:?}", report.repairs);
        let mooloop_project::LoadedDocument::Song(song) = report.document else {
            unreachable!("a song");
        };
        song
    };
    let reopened = reopen(&song_path);
    assert_eq!(reopened.plugins, saved.plugins);
    let mut present = session_with(cache_listing(&library), &reopened);
    let mut engine = Engine::default();
    present.service_plugins(&mut engine);
    let plugins = present.export_plugin_processors(RATE);
    assert_eq!(export(&snapshot(&present), plugins, dir.path(), "reopened.wav"), automated);

    // Missing, saved, and back.
    let mut missing = session_with(PluginCache::default(), &reopened);
    missing.service_plugins(&mut engine);
    assert!(missing.plugin_problem(slot).is_some());
    missing.capture_plugin_states();
    let kept_path = dir.path().join("kept.mooloop");
    mooloop_project::save_song(&kept_path, &snapshot(&missing), mooloop_project::AssetMode::Referenced)
        .expect("it saves");
    let kept = reopen(&kept_path);
    assert_eq!(kept.plugins, saved.plugins, "the plugin's slot came through untouched");
    assert_eq!(kept.channels[0].automation, saved.channels[0].automation, "and so did its lane");
    let mut back = session_with(cache_listing(&library), &kept);
    back.service_plugins(&mut engine);
    let plugins = back.export_plugin_processors(RATE);
    assert_eq!(export(&snapshot(&back), plugins, dir.path(), "back.wav"), automated);
}
