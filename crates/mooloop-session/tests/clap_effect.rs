//! Step 06's case, headless, with the in-repo test plugin
//! (`docs/plans/plugin-hosting/06-a-headless-clap-effect.md`, MOO-81): a
//! CLAP effect inserted into a drum loop through the session, saved,
//! reopened with the plugin present and with it missing, and exported the
//! way the app exports.
//!
//! `examples/clap_effect_case.rs` is the same case with a real third-party
//! plugin, for a machine that has one. CI has none, so this is the one that
//! runs everywhere.
//!
//! The plugin is found the way the app finds it: through the scanner's cache
//! (`ClapOpener`), here a cache that lists the test plugin's library. The
//! test thread is the plugins' main thread, and every export renders on a
//! thread of its own, as the app's document worker does.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mooloop_core::{
    EffectParams, NoteEvent, PluginFormat, PluginRef, PluginSlotId, PluginState,
    PluginStateChunk, Project, ProjectChannel,
};
use mooloop_dsp::AudioNode;
use mooloop_engine::{
    CommandSink, ExportFormat, ExportProgress, ExportSpec, OfflineRenderer, RenderScope,
    StructuralCommand, WavEncoding,
};
use mooloop_plugin_host::clap::{ClapOpener, STATE_TAG};
use mooloop_plugin_host::scan::PluginCache;
use mooloop_plugin_host::HostError;
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

/// The test gain's saved state at `gain_db`: what turning its knob and
/// saving would leave behind.
fn gain_state(gain_db: f64) -> PluginState {
    let mut data = test_plugin::STATE_MAGIC.to_vec();
    data.extend_from_slice(&gain_db.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.push(0);
    PluginState {
        chunks: vec![PluginStateChunk {
            tag: STATE_TAG.to_owned(),
            data,
        }],
    }
}

/// Two drum-synth channels, a one-bar loop.
fn drum_loop() -> Project {
    let mut project = Project::default();
    project.channels.clear();
    project.pattern_lengths[0] = 16;
    for index in 0..2 {
        let mut channel = ProjectChannel::drum_synth(index, 1);
        channel.setup.channel.volume = 0.4;
        for (step, pitch) in [(0u32, 36u8), (4, 38), (8, 36), (12, 42)] {
            channel.notes[0].push(NoteEvent::new(step + 1, step * 24, 12, pitch + index as u8, 110));
        }
        project.channels.push(channel);
    }
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

/// Export `session`'s song the way the app does: its plugins' processors
/// from second instances, rendered on a worker thread.
fn export(session: &mut Session, dir: &Path, name: &str) -> Vec<f32> {
    let plugins = session.export_plugin_processors(RATE);
    render(&snapshot(session), plugins, dir, name)
}

/// The song as the app saves it, at the loop's own tempo and swing.
fn snapshot(session: &Session) -> Project {
    let song = drum_loop();
    session.project_snapshot(song.bpm.into(), song.swing_percent.into())
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

fn reopen(path: &Path) -> Project {
    let report = mooloop_project::load_bundle(path).expect("the song reopens");
    assert!(report.repairs.is_empty(), "the song needed repairs: {:?}", report.repairs);
    match report.document {
        mooloop_project::LoadedDocument::Song(project) => project,
        _ => panic!("a song came back as something else"),
    }
}

#[test]
fn a_clap_effect_is_inserted_saved_reopened_present_and_missing_and_exported() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let library = test_plugin_path();
    let dry_song = drum_loop();

    // Insert the effect on channel 0 through the session, as the app will
    // from its menu (step 08), and turn it down: the knob is step 07's, so
    // the state is loaded the way a preset would.
    let mut session = session_with(cache_listing(&library), &dry_song);
    let mut engine = Engine::default();
    let inserted = session
        .insert_plugin_effect(gain_ref(), 0, &mut engine)
        .expect("the device is inserted");
    let EffectParams::Plugin(slot) = inserted.params else {
        panic!("a plugin device");
    };
    assert!(session.plugin_problem(slot).is_none(), "the plugin is hosted");
    assert_eq!(engine.nodes.len(), 1, "its processor was installed");
    session
        .plugin_rack
        .instance_mut(slot)
        .expect("a live instance")
        .load_state(&gain_state(-6.0))
        .expect("the state loads");
    let params = &session.plugins[&slot].params;
    assert_eq!(params.iter().map(|p| p.id).collect::<Vec<_>>(), [10, 20, 30], "its parameters are recorded");

    let dry = render(&dry_song, BTreeMap::new(), dir.path(), "dry.wav");
    let wet = export(&mut session, dir.path(), "wet.wav");
    let again = export(&mut session, dir.path(), "again.wav");
    assert_eq!(wet, again, "the export is deterministic");
    assert_ne!(wet, dry, "the plugin is heard");
    let peak = |samples: &[f32]| samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak(&wet) < peak(&dry));

    // Saved, with the plugin's state in the song.
    let song = dir.path().join("with-a-plugin.mooloop");
    session.capture_plugin_states();
    let saved = snapshot(&session);
    assert_eq!(saved.plugins[&slot].state.0, gain_state(-6.0), "the state is captured");
    mooloop_project::save_song(&song, &saved, mooloop_project::AssetMode::Referenced).expect("it saves");
    let reopened = reopen(&song);
    assert_eq!(reopened.plugins, saved.plugins, "the plugin table round-trips exactly");

    // Reopened with the plugin present: the opener hosts it again, with its
    // state, and the export is the same file.
    let mut present = session_with(cache_listing(&library), &reopened);
    let mut engine = Engine::default();
    present.service_plugins(&mut engine);
    assert!(present.plugin_problem(slot).is_none());
    assert_eq!(engine.nodes.len(), 1, "the processor was swapped into the placeholder");
    assert_eq!(export(&mut present, dir.path(), "reopened.wav"), wet);

    // Reopened with it missing: the song opens, plays the placeholder, and
    // keeps everything that belonged to the plugin, byte for byte.
    let mut missing = session_with(PluginCache::default(), &reopened);
    let mut engine = Engine::default();
    for _ in 0..3 {
        missing.service_plugins(&mut engine);
    }
    assert_eq!(missing.plugin_problem(slot), Some(HostError::Missing));
    assert!(engine.nodes.is_empty(), "the placeholder stays");
    // A pass-through to within the last bit: a device slot mixes through
    // unity trims and a wet/dry blend, which rounds a few samples by 1.5e-8
    // (-156 dBFS) against a chain with no device at all.
    let missing_out = export(&mut missing, dir.path(), "missing.wav");
    let worst = missing_out
        .iter()
        .zip(&dry)
        .fold(0.0f32, |worst, (a, b)| worst.max((a - b).abs()));
    assert_eq!(missing_out.len(), dry.len());
    assert!(worst < 1.0e-6, "a missing plugin is a pass-through: {worst}");
    let kept = snapshot(&missing);
    assert_eq!(kept.plugins, reopened.plugins, "the missing plugin's slot is kept unchanged");
    let resaved = dir.path().join("resaved.mooloop");
    mooloop_project::save_song(&resaved, &kept, mooloop_project::AssetMode::Referenced).expect("it saves");
    assert_eq!(reopen(&resaved).plugins, reopened.plugins);
}
