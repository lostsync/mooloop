//! MOO-222's case, headless, with the in-repo test plugin: a hosted plugin
//! device with a moved parameter saved as an effect preset, and the preset
//! loaded onto a plugin device on another channel of another song. The
//! plugin opens with that state in a new slot, and the one it replaced is
//! retired only once its processor has come back.
//!
//! The test thread is the plugins' main thread, and every processor runs on
//! a thread of its own.

use std::path::{Path, PathBuf};

use mooloop_core::{
    EffectParams, NoteEvent, PluginFormat, PluginRef, PluginSlotId, PluginStateText, Project,
    ProjectChannel,
};
use mooloop_dsp::{AudioNode, Event, EventList, ProcessContext, StereoBus, TimedEvent};
use mooloop_engine::{CommandSink, StructuralCommand};
use mooloop_plugin_host::clap::{ClapOpener, STATE_TAG};
use mooloop_plugin_host::scan::PluginCache;
use mooloop_project::{AssetMode, LoadedDocument, PresetInfo};
use mooloop_session::session::{PresetSaveTarget, Session};
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

/// `channels` drum-synth channels, a one-bar loop.
fn drum_loop(channels: usize) -> Project {
    let mut project = Project::default();
    project.channels.clear();
    project.pattern_lengths[0] = 16;
    for index in 0..channels {
        let mut channel = ProjectChannel::drum_synth(index, 1);
        channel.setup.channel.volume = 0.4;
        for (step, pitch) in [(0u32, 36u8), (4, 38), (8, 36), (12, 42)] {
            channel.notes[0].push(NoteEvent::new(step + 1, step * 24, 12, pitch, 110));
        }
        project.channels.push(channel);
    }
    project.assign_channel_ids();
    project
}

/// Stands in for the engine: takes every command, and holds every node it
/// is handed for as long as the engine would, in the order it was handed.
#[derive(Default)]
struct Engine {
    nodes: Vec<(u64, Box<dyn AudioNode + Send>)>,
}

impl CommandSink for Engine {
    fn send(&mut self, _cmd: mooloop_core::EngineCommand) -> bool {
        true
    }
    fn send_structural(&mut self, cmd: StructuralCommand) -> bool {
        match cmd {
            StructuralCommand::InstallEffect {
                node, resource_key, ..
            } => self.nodes.push((resource_key.expect("a plugin is keyed by its slot"), node)),
            StructuralCommand::ReplaceEffect {
                node, resource_key, ..
            } => self.nodes.push((resource_key, node)),
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

fn session_with(project: &Project) -> Session {
    let mut session = Session::default();
    session.set_plugin_opener(Box::new(ClapOpener::with_cache(cache_listing(&test_plugin_path()))));
    session.replace_project(project, &[]);
    session
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
                // Stopped where it ran, as the engine retires a node leaving
                // the audio thread (MOO-311).
                node.retire();
                node
            })
            .join()
            .expect("the processing thread did not panic")
    })
}

/// The test gain inserted on `channel` of `session`, and its slot.
fn insert_gain(session: &mut Session, engine: &mut Engine, channel: i32) -> PluginSlotId {
    session.select_channel(channel);
    let inserted = session
        .insert_plugin_effect(gain_ref(), 0, engine)
        .expect("the device is inserted");
    let EffectParams::Plugin(slot) = inserted.params else {
        unreachable!("a plugin device");
    };
    assert!(session.plugin_problem(slot).is_none(), "the test plugin opened");
    slot
}

/// **MOO-222's case.** A plugin device whose parameter was moved -- in the
/// plugin, and not yet captured by the song -- is saved as a preset with
/// that state. Loaded onto the plugin device on the second channel of
/// another song, the plugin opens with that state in a slot minted there;
/// the device keeps its identity; the slot it had leaves the song, and its
/// instance stays until the processor the install displaced has come back,
/// then goes. An undo brings the old slot back as it was.
#[test]
fn a_plugin_preset_saves_the_live_state_and_loads_into_a_new_slot_elsewhere() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("nudged.mooloop-effect");

    // The first song: the gain on channel 0, nudged by its own gesture, so
    // the plugin holds a state the song has not captured.
    let mut first = session_with(&drum_loop(1));
    let mut engine = Engine::default();
    let slot = insert_gain(&mut first, &mut engine, 0);
    first.capture_plugin_states();
    assert_eq!(saved_gain(&first.plugins[&slot].state), 0.0);
    let (key, node) = engine.nodes.pop().expect("the processor");
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
    engine.nodes.push((key, node));
    first.service_plugins(&mut engine);
    let gain = first.plugin_param_index(slot, test_plugin::PARAM_GAIN).expect("listed");
    assert_eq!(first.plugin_param_value(slot, gain), Some(test_plugin::NUDGE_DB));
    assert_eq!(saved_gain(&first.plugins[&slot].state), 0.0, "test setup: not captured yet");

    // Saved as a preset, the way the rail's Save does it.
    let device = first.effect_chain().expect("a chain")[0].id;
    first.pending_preset_save = first
        .chain_key(first.effect_target)
        .map(|target| PresetSaveTarget::Effect { target, device });
    let source = first.take_preset_save(120, 0).expect("a save was pending");
    let plugin = source.plugin.expect("a plugin row saves with its plugin");
    assert_eq!(saved_gain(&plugin.state), test_plugin::NUDGE_DB, "the live state, not the song's copy");
    assert_eq!(saved_gain(&first.plugins[&slot].state), 0.0, "a preset save is not an edit");
    mooloop_project::save_plugin_effect_preset(
        &path,
        &source.effect.expect("the row"),
        &plugin,
        PresetInfo {
            name: "Nudged".into(),
            category: String::new(),
            tags: Vec::new(),
        },
        AssetMode::Embedded,
    )
    .expect("saved");
    let manifest = std::fs::read_to_string(path.join(mooloop_project::MANIFEST_FILE)).unwrap();
    assert!(
        manifest.contains("\"effect_plugin\""),
        "the bundle names what an older reader must refuse"
    );

    // The second song: two channels, the gain at its defaults on the second,
    // in a slot numbered after one the first song never had.
    let mut project = drum_loop(2);
    project.next_plugin_slot = 5;
    let mut second = session_with(&project);
    let mut engine = Engine::default();
    let old = insert_gain(&mut second, &mut engine, 1);
    second.capture_plugin_states();
    let device = second.effect_chain().expect("a chain")[0].id;
    let before = second.project_snapshot(120, 0);

    let LoadedDocument::PluginEffect { effect, plugin } =
        mooloop_project::load_bundle(&path).expect("it loads").document
    else {
        panic!("not a plugin preset");
    };
    assert!(second
        .load_plugin_effect_preset(0, &effect, &plugin, "Nudged", &mut engine)
        .is_some());

    let row = second.effect_chain().expect("a chain")[0];
    assert_eq!(row.id, device, "the device keeps its identity, and its lanes and routes");
    let EffectParams::Plugin(new) = row.params else {
        panic!("still a plugin device");
    };
    assert_ne!(new, old, "a new slot");
    assert!(!second.plugins.contains_key(&old), "the old slot left the song");
    assert_eq!(second.plugins[&new].plugin, gain_ref());
    let gain = second.plugin_param_index(new, test_plugin::PARAM_GAIN).expect("listed");
    assert_eq!(
        second.plugin_param_value(new, gain),
        Some(test_plugin::NUDGE_DB),
        "the plugin opened with the preset's state"
    );
    assert!(second.plugin_problem(new).is_none());
    assert_eq!(
        engine.nodes.last().map(|(key, _)| *key),
        Some(u64::from(new.0)),
        "the new processor is installed keyed by the new slot"
    );
    assert_eq!(second.effect_preset_name(second.effect_target, device), Some("Nudged"));

    // The old instance waits for the processor the install displaced.
    assert!(second.plugin_rack.instance(old).is_none(), "retired from the rack");
    assert_eq!(second.plugin_rack.dying(), 1, "and kept while its processor is out");
    assert_eq!(second.collect_plugins(), 0);
    let displaced = engine
        .nodes
        .iter()
        .position(|(key, _)| *key == u64::from(old.0))
        .expect("the old processor");
    drop(engine.nodes.remove(displaced));
    assert_eq!(second.collect_plugins(), 1, "collected once its processor came back");
    assert_eq!(second.plugin_rack.dying(), 0);

    // Undo: the snapshot before names the old slot, at its defaults.
    second.replace_project(&before, &[]);
    for _ in 0..3 {
        second.service_plugins(&mut engine);
    }
    assert!(second.plugin_rack.instance(new).is_none(), "the preset's slot is retired");
    let gain = second.plugin_param_index(old, test_plugin::PARAM_GAIN).expect("listed");
    assert_eq!(
        second.plugin_param_value(old, gain),
        Some(test_plugin::GAIN_DB_DEFAULT),
        "the old slot came back as it was"
    );
}

/// A preset is not how a device changes plugin: loaded onto a device that
/// runs another plugin, it is refused and nothing moves.
#[test]
fn a_plugin_preset_does_not_load_onto_another_plugin() {
    let mut session = session_with(&drum_loop(1));
    let mut engine = Engine::default();
    let slot = insert_gain(&mut session, &mut engine, 0);
    let mut other = mooloop_core::PluginSlotState::new(PluginRef {
        id: "org.example.other".into(),
        ..gain_ref()
    });
    other.pinned.push(1);
    let mut preset = mooloop_core::EffectSlotState::of_kind(mooloop_core::EffectKind::Plugin);
    preset.params = EffectParams::Plugin(PluginSlotId::UNASSIGNED);
    let before = session.project_snapshot(120, 0);
    assert!(session
        .load_plugin_effect_preset(0, &preset, &other, "Other", &mut engine)
        .is_none());
    assert_eq!(session.project_snapshot(120, 0), before);
    assert!(session.plugin_rack.instance(slot).is_some());
}

/// Another plugin, which the test cache does not list: it never opens, and
/// that does not matter here. It is only something for a slot number to name.
fn other_ref() -> PluginRef {
    PluginRef {
        id: "org.example.other".into(),
        name: "Other".into(),
        ..gain_ref()
    }
}

/// The plugin slot a row runs, or a panic when it is not a plugin device.
fn plugin_slot(session: &Session, row: usize) -> PluginSlotId {
    match session.effect_chain().expect("a chain")[row].params {
        EffectParams::Plugin(slot) => slot,
        other => panic!("row {row} is not a plugin device: {other:?}"),
    }
}

/// `other_ref` inserted at the head of channel 0 of `session`, and its slot.
fn insert_other(session: &mut Session, engine: &mut Engine) -> PluginSlotId {
    session.select_channel(0);
    let inserted = session
        .insert_plugin_effect(other_ref(), 0, engine)
        .expect("the device is inserted");
    let EffectParams::Plugin(slot) = inserted.params else {
        unreachable!("a plugin device");
    };
    slot
}

/// Nudge the test gain in `slot` from inside the plugin, as its own window
/// would, so the live instance holds a state the song has not captured.
fn nudge(session: &mut Session, engine: &mut Engine, slot: PluginSlotId) {
    let at = engine
        .nodes
        .iter()
        .rposition(|(key, _)| *key == u64::from(slot.0))
        .expect("the processor");
    let (key, node) = engine.nodes.remove(at);
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
    engine.nodes.push((key, node));
    session.service_plugins(engine);
    let gain = session.plugin_param_index(slot, test_plugin::PARAM_GAIN).expect("listed");
    assert_eq!(session.plugin_param_value(slot, gain), Some(test_plugin::NUDGE_DB));
}

/// **MOO-271, duplicate.** A duplicated plugin device is a second instance
/// of its plugin, in a slot of its own, opened with the state the original
/// holds now -- not a second device on the original's slot, which the rack
/// keys one instance by.
#[test]
fn a_duplicated_plugin_device_is_a_second_instance_in_a_slot_of_its_own() {
    let mut session = session_with(&drum_loop(1));
    let mut engine = Engine::default();
    let original = insert_gain(&mut session, &mut engine, 0);
    nudge(&mut session, &mut engine, original);

    session.duplicate_device(0).expect("room");
    assert_eq!(plugin_slot(&session, 0), original, "the original keeps its slot");
    let copy = plugin_slot(&session, 1);
    assert_ne!(copy, original, "two devices, two slots");
    assert_eq!(session.plugins[&copy].plugin, gain_ref());
    assert_eq!(
        saved_gain(&session.plugins[&copy].state),
        test_plugin::NUDGE_DB,
        "the copy carries the live state, not the song's last capture"
    );

    // The pump opens it and swaps its processor into the copy's placeholder.
    session.service_plugins(&mut engine);
    assert!(session.plugin_rack.instance(copy).is_some(), "a second instance");
    assert!(session.plugin_problem(copy).is_none());
    let gain = session.plugin_param_index(copy, test_plugin::PARAM_GAIN).expect("listed");
    assert_eq!(session.plugin_param_value(copy, gain), Some(test_plugin::NUDGE_DB));
    assert_eq!(
        engine.nodes.last().map(|(key, _)| *key),
        Some(u64::from(copy.0)),
        "its processor goes to the copy, keyed by the copy's slot"
    );
}

/// **MOO-271, copy and paste.** A plugin device pasted into another song
/// lands in a slot minted there, running the plugin it was copied from --
/// never on whatever that song has at the number the copy came from. Pasted
/// back into its own song it is a second instance too.
#[test]
fn a_pasted_plugin_device_lands_in_a_slot_minted_where_it_lands() {
    let mut first = session_with(&drum_loop(1));
    let mut engine = Engine::default();
    let original = insert_gain(&mut first, &mut engine, 0);
    let clip = first.copy_device(0).expect("the device");

    // The second song already has another plugin at the number the gain
    // had in the first.
    let mut second = session_with(&drum_loop(1));
    let taken = insert_other(&mut second, &mut engine);
    assert_eq!(taken, original, "test setup: the same number in both songs");

    second.paste_device(&clip, 0).expect("room");
    let landed = plugin_slot(&second, 1);
    assert_ne!(landed, taken, "not the other plugin's slot");
    assert_eq!(second.plugins[&landed].plugin, gain_ref(), "the plugin that was copied");
    assert_eq!(second.plugins[&taken].plugin, other_ref(), "and the other one untouched");
    second.paste_device(&clip, 1).expect("room");
    let again = plugin_slot(&second, 2);
    assert!(again != landed && again != taken, "a second paste, a third slot");

    first.paste_device(&clip, 0).expect("room");
    assert_ne!(plugin_slot(&first, 1), original, "pasted into its own song, a second slot");
}

/// **MOO-271, container preset.** The issue's case: the test gain in a
/// Chain, saved as a container preset, loaded onto a Chain in a second song
/// whose slot 0 holds a different plugin. The format does not carry the
/// plugin yet (MOO-321), so the preset's plugin row is a bare slot number,
/// and the load is refused rather than landing on the other plugin's slot.
///
/// Once MOO-321 lands, this load succeeds, into a minted slot running the
/// gain: the next test is what that landing does.
#[test]
fn a_container_preset_with_a_plugin_never_lands_on_the_target_songs_slot() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("boxed.mooloop-effect");

    let mut first = session_with(&drum_loop(1));
    let mut engine = Engine::default();
    let original = insert_gain(&mut first, &mut engine, 0);
    first.wrap_effects_in_container(0..1).expect("wrapped");
    let device = first.effect_chain().expect("a chain")[0].id;
    first.pending_preset_save = first
        .chain_key(first.effect_target)
        .map(|target| PresetSaveTarget::Effect { target, device });
    let source = first.take_preset_save(120, 0).expect("a save was pending");
    let run = source.run.expect("a container saves its run");
    mooloop_project::save_effect_run_preset(
        &path,
        &run,
        PresetInfo {
            name: "Boxed".into(),
            category: String::new(),
            tags: Vec::new(),
        },
        AssetMode::Embedded,
    )
    .expect("saved");
    let LoadedDocument::EffectRun(run) =
        mooloop_project::load_bundle(&path).expect("it loads").document
    else {
        panic!("not a container preset");
    };

    let mut second = session_with(&drum_loop(1));
    let taken = insert_other(&mut second, &mut engine);
    assert_eq!(taken, original, "test setup: the same number in both songs");
    second
        .insert_effect_at(mooloop_core::EffectKind::Chain, 1)
        .expect("room");
    let before = second.project_snapshot(120, 0);

    if second.load_effect_run(1, &run, "Boxed").is_some() {
        let landed = plugin_slot(&second, 2);
        panic!(
            "the preset's plugin row landed on slot {} ({}), a number from another song",
            landed.0,
            second
                .plugins
                .get(&landed)
                .map_or("nothing", |state| state.plugin.name.as_str()),
        );
    }
    assert_eq!(
        second.project_snapshot(120, 0),
        before,
        "refused whole, and nothing minted"
    );
}

/// **MOO-271, a container run that carries its plugin**: a copied Chain,
/// and what a container preset will be once MOO-321 writes the plugin.
/// Loaded onto a Chain in a song whose slot 0 holds another plugin, the
/// plugin row lands in a slot minted there, and the pump opens the gain in
/// it with the state it was copied with.
#[test]
fn a_container_run_carrying_its_plugin_lands_in_a_fresh_slot_in_another_song() {
    let mut first = session_with(&drum_loop(1));
    let mut engine = Engine::default();
    let original = insert_gain(&mut first, &mut engine, 0);
    nudge(&mut first, &mut engine, original);
    first.capture_plugin_states();
    first.wrap_effects_in_container(0..1).expect("wrapped");
    let run = first.copy_device(0).expect("the box and the gain");
    assert_eq!(run.effects.len(), 2);

    let mut second = session_with(&drum_loop(1));
    let taken = insert_other(&mut second, &mut engine);
    assert_eq!(taken, original, "test setup: the same number in both songs");
    second
        .insert_effect_at(mooloop_core::EffectKind::Chain, 1)
        .expect("room");

    second
        .load_effect_run(1, &run, "Boxed")
        .expect("a well-formed run onto a Chain");
    let landed = plugin_slot(&second, 2);
    assert_ne!(landed, taken, "not the other plugin's slot");
    assert_eq!(second.plugins[&landed].plugin, gain_ref());
    assert_eq!(second.plugins[&taken].plugin, other_ref(), "the other one untouched");

    second.service_plugins(&mut engine);
    assert!(second.plugin_problem(landed).is_none());
    let gain = second
        .plugin_param_index(landed, test_plugin::PARAM_GAIN)
        .expect("listed");
    assert_eq!(
        second.plugin_param_value(landed, gain),
        Some(test_plugin::NUDGE_DB),
        "opened with the state it was copied with"
    );
}
