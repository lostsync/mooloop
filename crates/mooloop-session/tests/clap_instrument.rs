//! Step 10's case, headless, with the in-repo test sine
//! (`docs/plans/plugin-hosting/10-clap-instruments.md`, MOO-85): a CLAP
//! instrument made a channel's source through the session, found the way
//! the app finds it (the scanner's cache), played from the channel's
//! pattern, exported, saved, and reopened present and missing.
//!
//! Also the rule the session holds on where a plugin may go: an instrument
//! (no audio input, one output, a note input) only as a channel's source, an
//! effect (one stereo input and output) only in a chain.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mooloop_core::{
    ChannelSource, DeviceKind, NoteEvent, PluginFormat, PluginRef, PluginSlotId, Project,
    ProjectChannel,
};
use mooloop_dsp::{AudioNode, SourceNode};
use mooloop_engine::{
    CommandSink, ExportFormat, ExportProgress, ExportSpec, OfflineRenderer, RenderScope,
    StructuralCommand, WavEncoding,
};
use mooloop_plugin_host::clap::ClapOpener;
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

/// One channel with a one-bar melody: four notes, one of them on an odd
/// step.
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
    /// Processors swapped in: into a source, or into a device's place.
    nodes: Vec<Box<dyn AudioNode + Send>>,
    /// Devices inserted, placeholders included.
    installed: Vec<Box<dyn AudioNode + Send>>,
}

impl CommandSink for Engine {
    fn send(&mut self, _cmd: mooloop_core::EngineCommand) -> bool {
        true
    }
    fn send_structural(&mut self, cmd: StructuralCommand) -> bool {
        match cmd {
            StructuralCommand::InstallSource { node, .. } => self.sources.push(node),
            StructuralCommand::HostSourceProcessor { node: Some(node), .. }
            | StructuralCommand::ReplaceEffect { node, .. } => self.nodes.push(node),
            StructuralCommand::InstallEffect { node, .. } => self.installed.push(node),
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
    let song = melody();
    session.project_snapshot(song.bpm.into(), song.swing_percent.into())
}

fn export(session: &mut Session, dir: &Path, name: &str) -> Vec<f32> {
    let plugins = session.export_plugin_processors(RATE);
    render(&snapshot(session), plugins, dir, name)
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

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

#[test]
fn a_clap_instrument_plays_its_channels_pattern_saves_reopens_and_exports() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let library = test_plugin_path();

    let mut session = session_with(cache_listing(&library), &melody());
    let mut engine = Engine::default();
    let slot = session
        .set_plugin_source(0, sine_ref(), &mut engine)
        .expect("channel 0 exists");
    assert!(session.plugin_problem(slot).is_none(), "the sine is hosted: {:?}", session.plugin_problem(slot));
    assert_eq!(session.channels[0].kind(), DeviceKind::Plugin);
    assert_eq!(engine.sources.len(), 1, "the source was installed");
    assert!(!engine.sources[0].is_at_rest(), "with its processor in it");

    let played = export(&mut session, dir.path(), "played.wav");
    assert!(peak(&played) > 0.05, "the sine is heard, peak {}", peak(&played));
    assert_eq!(played, export(&mut session, dir.path(), "again.wav"), "deterministic");

    // Saved, reopened with the plugin present: the same export.
    let song = dir.path().join("instrument.mooloop");
    session.capture_plugin_states();
    let saved = snapshot(&session);
    assert_eq!(saved.channels[0].setup.source, ChannelSource::Plugin(slot));
    mooloop_project::save_song(&song, &saved, mooloop_project::AssetMode::Referenced).expect("it saves");
    let reopened = reopen(&song);
    assert_eq!(reopened.plugins, saved.plugins);
    let mut present = session_with(cache_listing(&library), &reopened);
    let mut engine = Engine::default();
    present.service_plugins(&mut engine);
    assert!(present.plugin_problem(slot).is_none());
    assert_eq!(engine.nodes.len(), 1, "the processor went into the source");
    assert_eq!(export(&mut present, dir.path(), "present.wav"), played);

    // Reopened with it missing: silence, and the slot kept byte for byte.
    let mut missing = session_with(PluginCache::default(), &reopened);
    let mut engine = Engine::default();
    missing.service_plugins(&mut engine);
    assert_eq!(missing.plugin_problem(slot), Some(HostError::Missing));
    let silent = export(&mut missing, dir.path(), "missing.wav");
    assert_eq!(peak(&silent), 0.0, "a missing instrument is silent");
    assert_eq!(snapshot(&missing).plugins, saved.plugins, "the slot is kept");
}

/// An effect is not an instrument, and an instrument is not an effect: each
/// placed where the other goes is refused with the reason, and the song
/// keeps it (the device plays its placeholder, the channel silence).
#[test]
fn a_plugin_is_refused_where_its_ports_do_not_fit() {
    let library = test_plugin_path();
    let mut session = session_with(cache_listing(&library), &melody());
    let mut engine = Engine::default();

    let slot = session
        .set_plugin_source(0, gain_ref(), &mut engine)
        .expect("channel 0 exists");
    assert!(
        matches!(session.plugin_problem(slot), Some(HostError::Incompatible(_))),
        "an effect as a source: {:?}",
        session.plugin_problem(slot)
    );
    assert!(engine.sources[0].is_at_rest(), "the source is silent");

    let inserted = session
        .insert_plugin_effect(sine_ref(), 0, &mut engine)
        .expect("the device is inserted");
    let mooloop_core::EffectParams::Plugin(effect_slot) = inserted.params else {
        panic!("a plugin device");
    };
    assert!(
        matches!(session.plugin_problem(effect_slot), Some(HostError::Incompatible(_))),
        "an instrument as an effect: {:?}",
        session.plugin_problem(effect_slot)
    );

    // Nothing reopens either of them on later ticks.
    for _ in 0..4 {
        session.service_plugins(&mut engine);
    }
    assert!(engine.nodes.is_empty(), "no processor was ever installed");
}
