//! A channel that runs hosted plugins, copied and pasted or cloned, with the
//! in-repo test plugins: the sine as its source (MOO-317), the gain on its
//! chain (MOO-331), and the state each carries (MOO-332).
//!
//! Copy, paste and clone all go through `Session::channel_clipboard` and
//! `Session::paste_channel` -- clone is a copy pasted straight after itself
//! -- so these drive the two the way the window's handlers do, install the
//! pasted snapshot, and let the pump's plugin upkeep open what it names.
//! Before the paste minted a slot, the copy kept the original's
//! `PluginSlotId`: two channels named one slot, the rack hosts one instance
//! per slot, and the pasted channel was silent.
//!
//! The test thread is the plugins' main thread, and a processor the test
//! runs, runs on a thread of its own.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mooloop_core::{
    ChannelSource, EffectParams, NoteEvent, PluginFormat, PluginRef, PluginSlotId,
    PluginStateText, Project, ProjectChannel,
};
use mooloop_dsp::{AudioNode, Event, EventList, ProcessContext, SourceNode, StereoBus, TimedEvent};
use mooloop_engine::{
    CommandSink, ExportFormat, ExportProgress, ExportSpec, OfflineRenderer, RenderScope,
    StructuralCommand, WavEncoding,
};
use mooloop_plugin_host::clap::{ClapOpener, STATE_TAG};
use mooloop_plugin_host::scan::PluginCache;
use mooloop_session::project::{normalize_project_pattern_banks, ProjectSnapshot};
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

/// The scan cache the scanner would have written for the test library.
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

/// Stands in for the engine, and keeps every node it is handed: a
/// processor dropped here would count as come back, and its instance retired.
#[derive(Default)]
struct Engine {
    _sources: Vec<Box<dyn SourceNode + Send>>,
    hosted: Vec<(u8, PluginSlotId, Box<dyn AudioNode + Send>)>,
    /// A chain's devices and processors, by resource key: a plugin device's
    /// is its slot.
    effects: Vec<(Option<u64>, Box<dyn AudioNode + Send>)>,
}

impl CommandSink for Engine {
    fn send(&mut self, _cmd: mooloop_core::EngineCommand) -> bool {
        true
    }
    fn send_structural(&mut self, cmd: StructuralCommand) -> bool {
        match cmd {
            StructuralCommand::InstallSource { node, .. } => self._sources.push(node),
            StructuralCommand::HostSourceProcessor {
                channel,
                slot,
                node: Some(node),
            } => self.hosted.push((channel, slot, node)),
            StructuralCommand::InstallEffect {
                node, resource_key, ..
            } => self.effects.push((resource_key, node)),
            StructuralCommand::ReplaceEffect {
                node, resource_key, ..
            } => self.effects.push((Some(resource_key), node)),
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

/// The `before` the window's paste hands the session.
fn snapshot(session: &Session) -> ProjectSnapshot {
    let mut project = session.project_snapshot(120, 0);
    normalize_project_pattern_banks(&mut project);
    ProjectSnapshot {
        project,
        samples: session.keyed_sample_snapshots(),
    }
}

fn source_slot(project: &Project, seat: usize) -> PluginSlotId {
    match project.channels[seat].setup.source {
        ChannelSource::Plugin(slot) => slot,
        ref other => panic!("channel {seat}'s source is not a plugin: {other:?}"),
    }
}

/// The session's song rendered with only channel `seat`'s notes, through
/// the export's own plugin processors.
fn export_solo(session: &mut Session, seat: usize, dir: &Path, name: &str) -> Vec<f32> {
    let plugins = session.export_plugin_processors(RATE);
    let mut project = session.project_snapshot(120, 0);
    for (index, channel) in project.channels.iter_mut().enumerate() {
        if index != seat {
            channel.notes.iter_mut().for_each(Vec::clear);
        }
    }
    render(&project, plugins, dir, name)
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

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

/// **A pasted plugin-instrument channel plays its own instance of the
/// plugin** (MOO-317). The sine channel is copied and pasted after itself in
/// the same song: the two hold different slots, the paste's slot carries the
/// original's plugin and state, the pump opens it and swaps its processor
/// into the pasted channel, and each channel sounds on its own. An undo --
/// the snapshot before the paste installed again -- leaves the song with the
/// one slot it had.
#[test]
fn a_pasted_plugin_instrument_channel_gets_its_own_slot_and_both_sound() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let mut session = session_with(cache_listing(&test_plugin_path()), &melody());
    let mut engine = Engine::default();
    let original = session
        .set_plugin_source(0, sine_ref(), &mut engine)
        .expect("channel 0 exists");
    assert!(session.plugin_problem(original).is_none(), "the sine is hosted");
    // What the pump does before anyone copies: the song holds the plugin's
    // state, so the copy has something real to carry.
    session.capture_plugin_states();

    let before = snapshot(&session);
    let copy = session.channel_clipboard(0, 120, 0).expect("a channel to copy");
    let (pasted, index) = session
        .paste_channel(&before, 0, copy)
        .expect("room to paste");
    assert_eq!(index, 1);

    let (first, second) = (source_slot(&pasted.project, 0), source_slot(&pasted.project, 1));
    assert_eq!(first, original, "the original keeps its slot");
    assert_ne!(first, second, "the pasted channel shares the original's slot");
    assert!(second.is_assigned());
    assert_eq!(
        pasted.project.plugins.get(&second),
        pasted.project.plugins.get(&first),
        "the paste carries the plugin, its parameters and its state"
    );
    assert!(pasted.project.next_plugin_slot > second.0, "the mint moved on");
    assert!(!before.project.plugins.contains_key(&second), "the snapshot before is untouched");

    // Installed as the window installs it, then the pump's upkeep.
    session.replace_project(&pasted.project, &pasted.seated());
    session.service_plugins(&mut engine);
    assert!(session.plugin_problem(second).is_none(), "{:?}", session.plugin_problem(second));
    let hosted: Vec<(u8, PluginSlotId)> =
        engine.hosted.iter().map(|(channel, slot, _)| (*channel, *slot)).collect();
    assert!(
        hosted.contains(&(1, second)),
        "the pasted channel's own processor went into its source: {hosted:?}"
    );

    let heard_original = export_solo(&mut session, 0, dir.path(), "original.wav");
    let heard_pasted = export_solo(&mut session, 1, dir.path(), "pasted.wav");
    assert!(peak(&heard_original) > 0.05, "the original is silent, peak {}", peak(&heard_original));
    assert!(peak(&heard_pasted) > 0.05, "the paste is silent, peak {}", peak(&heard_pasted));
    assert_eq!(heard_original, heard_pasted, "the same plugin, state and notes");

    // Undo.
    session.replace_project(&before.project, &before.seated());
    session.service_plugins(&mut engine);
    assert_eq!(session.project_snapshot(120, 0).plugins, before.project.plugins);
}

/// **Pasted into another song, the plugin comes with the channel** rather
/// than the slot number. The second song already keeps a different plugin
/// (the test gain, on a chain) in the slot the copy was made from; the paste
/// lands the sine in a slot of its own and leaves the gain where it was.
#[test]
fn a_plugin_instrument_channel_pasted_into_another_song_brings_its_plugin() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let mut session = session_with(cache_listing(&test_plugin_path()), &melody());
    let mut engine = Engine::default();
    let original = session
        .set_plugin_source(0, sine_ref(), &mut engine)
        .expect("channel 0 exists");
    let copy = session.channel_clipboard(0, 120, 0).expect("a channel to copy");

    // New song, whose first plugin is a gain in channel 0's chain.
    session.replace_project(&melody(), &[]);
    let inserted = session
        .insert_plugin_effect(gain_ref(), 0, &mut engine)
        .expect("the gain is inserted");
    let mooloop_core::EffectParams::Plugin(gain) = inserted.params else {
        panic!("a plugin device");
    };
    assert_eq!(gain, original, "test setup: the two songs use the same slot number");

    let before = snapshot(&session);
    let (pasted, index) = session
        .paste_channel(&before, 0, copy)
        .expect("room to paste");
    let slot = source_slot(&pasted.project, index);
    assert_ne!(slot, gain, "the pasted instrument names the other song's gain");
    assert_eq!(pasted.project.plugins[&slot].plugin, sine_ref());
    assert_eq!(pasted.project.plugins[&gain].plugin, gain_ref(), "the gain is left alone");

    session.replace_project(&pasted.project, &pasted.seated());
    session.service_plugins(&mut engine);
    assert!(session.plugin_problem(slot).is_none(), "{:?}", session.plugin_problem(slot));
    let heard = export_solo(&mut session, index, dir.path(), "pasted.wav");
    assert!(peak(&heard) > 0.05, "the paste is silent, peak {}", peak(&heard));
}

/// The test gain's gain, in dB, read out of a saved state.
fn saved_gain(state: &PluginStateText) -> f64 {
    let chunk = state.0.chunks.iter().find(|chunk| chunk.tag == STATE_TAG).expect("a CLAP chunk");
    assert_eq!(chunk.data[..4], test_plugin::STATE_MAGIC);
    f64::from_le_bytes(chunk.data[4..12].try_into().expect("eight bytes"))
}

/// The test gain inserted at the head of `seat`'s chain, and its slot.
fn insert_gain(session: &mut Session, engine: &mut Engine, seat: i32) -> PluginSlotId {
    session.select_channel(seat);
    let inserted = session
        .insert_plugin_effect(gain_ref(), 0, engine)
        .expect("the device is inserted");
    let EffectParams::Plugin(slot) = inserted.params else {
        unreachable!("a plugin device");
    };
    assert!(session.plugin_problem(slot).is_none(), "the test gain opened");
    slot
}

/// The plugin slot row `row` of `seat`'s chain runs.
fn chain_slot(project: &Project, seat: usize, row: usize) -> PluginSlotId {
    match project.channels[seat].setup.effects[row].params {
        EffectParams::Plugin(slot) => slot,
        other => panic!("channel {seat}'s row {row} is not a plugin device: {other:?}"),
    }
}

/// Nudge the test gain in `slot` from inside the plugin, as its own window
/// would: its processor runs one block with the nudge raised, and the plugin
/// moves its own gain. The live instance then holds a state the song has
/// not captured (the pump's capture is not run here).
fn nudge(session: &mut Session, engine: &mut Engine, slot: PluginSlotId) {
    let at = engine
        .effects
        .iter()
        .rposition(|(key, _)| *key == Some(u64::from(slot.0)))
        .expect("the processor");
    let (key, mut node) = engine.effects.remove(at);
    node = std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let mut bus = StereoBus::with_capacity(256);
                let mut list = EventList::empty();
                assert!(list.push_ordered(TimedEvent {
                    offset: 10,
                    event: Event::ParamValue {
                        id: test_plugin::PARAM_NUDGE,
                        value: 1.0,
                    },
                }));
                let ctx = ProcessContext {
                    sample_rate: RATE,
                    frames: 256,
                    playing: true,
                    bpm: 120.0,
                    position_ticks: 0.0,
                    position_frames: 0,
                };
                node.process(&ctx, &mut bus, &list, None);
                // Stopped where it ran, as the engine retires a node leaving
                // the audio thread (MOO-311).
                node.retire();
                node
            })
            .join()
            .expect("the processing thread did not panic")
    });
    engine.effects.push((key, node));
    session.service_plugins(engine);
    let gain = session.plugin_param_index(slot, test_plugin::PARAM_GAIN).expect("listed");
    assert_eq!(session.plugin_param_value(slot, gain), Some(test_plugin::NUDGE_DB));
}

/// **A pasted channel's plugin inserts get slots of their own** (MOO-331).
/// The test gain on the channel's chain, and a second one boxed in a Chain
/// container, copied and pasted after the channel in the same song: every
/// plugin row of the paste names a slot minted for it, carrying the plugin
/// it was copied from, and the pump hosts a second instance in each and
/// swaps its processor into the pasted device. The export has a processor
/// for all four.
#[test]
fn a_pasted_channels_plugin_inserts_get_slots_of_their_own() {
    let mut session = session_with(cache_listing(&test_plugin_path()), &melody());
    let mut engine = Engine::default();
    let boxed = insert_gain(&mut session, &mut engine, 0);
    session.wrap_effects_in_container(0..1).expect("wrapped");
    let plain = insert_gain(&mut session, &mut engine, 0);
    session.capture_plugin_states();

    let before = snapshot(&session);
    // The chain: the plain gain, the Chain, and the gain inside it.
    assert_eq!(chain_slot(&before.project, 0, 0), plain, "test setup");
    assert_eq!(chain_slot(&before.project, 0, 2), boxed, "test setup");
    let copy = session.channel_clipboard(0, 120, 0).expect("a channel to copy");
    let (pasted, index) = session
        .paste_channel(&before, 0, copy)
        .expect("room to paste");

    let (plain_copy, boxed_copy) = (
        chain_slot(&pasted.project, index, 0),
        chain_slot(&pasted.project, index, 2),
    );
    assert_eq!(chain_slot(&pasted.project, 0, 0), plain, "the original keeps its slots");
    assert_eq!(chain_slot(&pasted.project, 0, 2), boxed, "the original keeps its slots");
    for (copied, original) in [(plain_copy, plain), (boxed_copy, boxed)] {
        assert!(
            copied.is_assigned() && copied != plain && copied != boxed,
            "the pasted row names slot {}, one of the original's",
            copied.0
        );
        assert_eq!(
            pasted.project.plugins.get(&copied),
            pasted.project.plugins.get(&original),
            "the paste carries the plugin, its parameters and its state"
        );
    }
    assert_ne!(plain_copy, boxed_copy);

    session.replace_project(&pasted.project, &pasted.seated());
    session.service_plugins(&mut engine);
    for slot in [plain_copy, boxed_copy] {
        assert!(session.plugin_problem(slot).is_none(), "{:?}", session.plugin_problem(slot));
        assert!(session.plugin_rack.instance(slot).is_some(), "a second instance");
        assert!(
            engine.effects.iter().any(|(key, _)| *key == Some(u64::from(slot.0))),
            "its processor went into the pasted device"
        );
    }
    let exported = session.export_plugin_processors(RATE);
    for slot in [plain, boxed, plain_copy, boxed_copy] {
        assert!(exported.contains_key(&slot), "the export has no processor for slot {}", slot.0);
    }
}

/// Pasted into another song, a channel's plugin insert brings its plugin,
/// not its slot number: the second song's sine instrument holds the number
/// the gain had, and is left alone.
#[test]
fn a_channels_plugin_insert_pasted_into_another_song_brings_its_plugin() {
    let mut session = session_with(cache_listing(&test_plugin_path()), &melody());
    let mut engine = Engine::default();
    let gain = insert_gain(&mut session, &mut engine, 0);
    let copy = session.channel_clipboard(0, 120, 0).expect("a channel to copy");

    session.replace_project(&melody(), &[]);
    let sine = session
        .set_plugin_source(0, sine_ref(), &mut engine)
        .expect("channel 0 exists");
    assert_eq!(sine, gain, "test setup: the same number in both songs");

    let before = snapshot(&session);
    let (pasted, index) = session
        .paste_channel(&before, 0, copy)
        .expect("room to paste");
    let landed = chain_slot(&pasted.project, index, 0);
    assert_ne!(landed, sine, "the pasted gain names the other song's sine");
    assert_eq!(pasted.project.plugins[&landed].plugin, gain_ref());
    assert_eq!(pasted.project.plugins[&sine].plugin, sine_ref(), "the sine is left alone");
}

/// **A channel copy carries its plugins as they are now** (MOO-332), not as
/// the song last captured them. The gain on the chain is nudged from inside
/// the plugin and not captured; the pasted channel's gain carries, and opens
/// with, the nudged state, and the copy is not an edit of the original. The
/// instrument's plugin is read through the same call.
#[test]
fn a_channel_copy_carries_its_plugins_live_state() {
    let mut session = session_with(cache_listing(&test_plugin_path()), &melody());
    let mut engine = Engine::default();
    let gain = insert_gain(&mut session, &mut engine, 0);
    session.capture_plugin_states();
    nudge(&mut session, &mut engine, gain);
    assert_eq!(
        saved_gain(&session.plugins[&gain].state),
        test_plugin::GAIN_DB_DEFAULT,
        "test setup: the nudge is not captured"
    );

    let before = snapshot(&session);
    let copy = session.channel_clipboard(0, 120, 0).expect("a channel to copy");
    assert_eq!(
        saved_gain(&session.plugins[&gain].state),
        test_plugin::GAIN_DB_DEFAULT,
        "a copy is not an edit"
    );
    let (pasted, index) = session
        .paste_channel(&before, 0, copy)
        .expect("room to paste");
    let landed = chain_slot(&pasted.project, index, 0);
    assert_eq!(
        saved_gain(&pasted.project.plugins[&landed].state),
        test_plugin::NUDGE_DB,
        "the copy carries the live state, not the song's last capture"
    );

    session.replace_project(&pasted.project, &pasted.seated());
    session.service_plugins(&mut engine);
    let param = session.plugin_param_index(landed, test_plugin::PARAM_GAIN).expect("listed");
    assert_eq!(
        session.plugin_param_value(landed, param),
        Some(test_plugin::NUDGE_DB),
        "the pasted gain opened with it"
    );
}
