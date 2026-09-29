//! A hosted plugin put at a place rather than an index (MOO-339, part of
//! MOO-299): **Plugin…** on the join at a Chain's end lands inside the box,
//! after its devices, as a native device from the same join does. An index
//! cannot say that -- the one past a Chain's run means "after the Chain" --
//! so `Session::place_plugin_effect` takes Effects' `EffectPlace`.
//!
//! The in-repo test gain, found through a scan cache that lists its library,
//! as the app finds a plugin. The test thread is the plugin's main thread;
//! nothing here processes audio.

use std::path::{Path, PathBuf};

use mooloop_core::{
    depth_at, EffectKind, EffectParams, EffectTarget, EngineCommand, NoteEvent, PluginFormat,
    PluginRef, PluginSlotId, Project, ProjectChannel,
};
use mooloop_dsp::AudioNode;
use mooloop_engine::{CommandSink, StructuralCommand};
use mooloop_plugin_host::clap::ClapOpener;
use mooloop_plugin_host::scan::PluginCache;
use mooloop_session::effects::EffectPlace;
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

/// What the stand-in engine was told, in order: the effect-chain commands
/// only.
#[derive(Debug, PartialEq, Eq)]
enum Heard {
    /// `InstallEffect` at this engine row, keyed by this resource.
    Install { row: u8, key: Option<u64> },
    /// `MoveEffect` from one engine row to another.
    Move { from: u8, to: u8 },
}

/// Stands in for the engine: records what it is told about effect chains,
/// and holds every node it is handed, as the engine would.
#[derive(Default)]
struct Engine {
    heard: Vec<Heard>,
    nodes: Vec<Box<dyn AudioNode + Send>>,
}

impl CommandSink for Engine {
    fn send(&mut self, cmd: EngineCommand) -> bool {
        if let EngineCommand::MoveEffect { from, to, .. } = cmd {
            self.heard.push(Heard::Move { from, to });
        }
        true
    }
    fn send_structural(&mut self, cmd: StructuralCommand) -> bool {
        if let StructuralCommand::InstallEffect {
            slot,
            resource_key,
            node,
            ..
        } = cmd
        {
            self.heard.push(Heard::Install {
                row: slot,
                key: resource_key,
            });
            self.nodes.push(node);
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

/// One drum channel, the rack pointed at it, and the test plugin findable.
fn session() -> Session {
    let mut project = Project::default();
    project.channels.clear();
    let mut channel = ProjectChannel::drum_synth(0, 1);
    channel.notes[0].push(NoteEvent::new(1, 0, 12, 36, 110));
    project.channels.push(channel);
    project.assign_channel_ids();
    let mut session = Session::default();
    session.set_plugin_opener(Box::new(ClapOpener::with_cache(cache_listing(&test_plugin_path()))));
    session.replace_project(&project, &[]);
    session
}

/// The rows of the chain the rack is pointed at: kind and depth.
fn rows(session: &Session) -> Vec<(EffectKind, usize)> {
    let effects = session.effect_chain().expect("the rack is on a channel");
    (0..effects.len())
        .map(|row| (effects[row].kind(), depth_at(effects, row)))
        .collect()
}

fn plugin_slot_at(session: &Session, row: usize) -> PluginSlotId {
    match session.effect_chain().expect("a chain")[row].params {
        EffectParams::Plugin(slot) => slot,
        ref other => panic!("row {row} is not a plugin device: {other:?}"),
    }
}

fn plugin_slots(session: &Session) -> usize {
    session.project_snapshot(120, 0).plugins.len()
}

/// **Plugin… at a Chain's end lands inside it, after its devices.** A Chain
/// holding a Filter and a Drive, with a Reverb after the box: the gain put
/// `LastIn` the Chain lands after the Drive at the box's depth, the Reverb
/// stays after the box, the plugin is hosted, and the engine is told to
/// install it keyed by its slot at the old tail, then move it into place.
#[test]
fn a_plugin_put_last_in_a_chain_lands_inside_it_after_its_devices() {
    let mut session = session();
    session.insert_effect_at(EffectKind::Chain, 0).expect("a Chain");
    session.insert_effect_into_container(EffectKind::Filter, 0).expect("a Filter in it");
    session.append_effect_into_container(EffectKind::Drive, 0).expect("a Drive after it");
    session.insert_effect_at(EffectKind::Reverb, 3).expect("a Reverb after the box");
    assert_eq!(
        rows(&session),
        [
            (EffectKind::Chain, 0),
            (EffectKind::Filter, 1),
            (EffectKind::Drive, 1),
            (EffectKind::Reverb, 0),
        ]
    );

    let mut engine = Engine::default();
    let inserted = session
        .place_plugin_effect(gain_ref(), EffectPlace::LastIn(0), &mut engine)
        .expect("the gain is placed");
    assert_eq!((inserted.target, inserted.slot, inserted.tail), (EffectTarget::Channel(0), 3, 4));
    assert_eq!(inserted.kind, EffectKind::Plugin);
    assert_eq!(
        rows(&session),
        [
            (EffectKind::Chain, 0),
            (EffectKind::Filter, 1),
            (EffectKind::Drive, 1),
            (EffectKind::Plugin, 1),
            (EffectKind::Reverb, 0),
        ]
    );
    let slot = plugin_slot_at(&session, 3);
    assert_eq!(inserted.params, EffectParams::Plugin(slot));
    assert!(session.plugin_problem(slot).is_none(), "the gain is hosted");
    assert_eq!(
        engine.heard,
        [
            Heard::Install { row: 4, key: Some(u64::from(slot.0)) },
            Heard::Move { from: 4, to: 3 },
        ]
    );
    assert_eq!(engine.nodes.len(), 1);
}

/// **Plugin… inside an empty box lands inside it**: the other place an index
/// cannot name, the join inside a Chain with nothing in it yet.
#[test]
fn a_plugin_put_first_in_an_empty_chain_lands_inside_it() {
    let mut session = session();
    session.insert_effect_at(EffectKind::Chain, 0).expect("a Chain");
    let mut engine = Engine::default();
    let inserted = session
        .place_plugin_effect(gain_ref(), EffectPlace::FirstIn(0), &mut engine)
        .expect("the gain is placed");
    assert_eq!((inserted.slot, inserted.tail), (1, 1));
    assert_eq!(rows(&session), [(EffectKind::Chain, 0), (EffectKind::Plugin, 1)]);
    let slot = plugin_slot_at(&session, 1);
    assert_eq!(engine.heard, [Heard::Install { row: 1, key: Some(u64::from(slot.0)) }]);
}

/// **A layer's end is not a place for a plugin**: a device added there
/// would be a new branch, which is the layer face's `+`. Refused, with no
/// slot left behind and nothing sent to the engine.
#[test]
fn a_plugin_put_last_in_a_layer_is_refused_and_leaves_no_slot() {
    let mut session = session();
    session.insert_effect_at(EffectKind::Layer, 0).expect("a Layer");
    let before = rows(&session);
    let slots = plugin_slots(&session);
    let mut engine = Engine::default();
    assert!(session
        .place_plugin_effect(gain_ref(), EffectPlace::LastIn(0), &mut engine)
        .is_none());
    assert_eq!(rows(&session), before, "the chain is as it was");
    assert_eq!(plugin_slots(&session), slots, "the minted slot was let go");
    assert!(engine.heard.is_empty() && engine.nodes.is_empty(), "nothing was sent");
}

/// The index verb is the same placement at `Before`: past a Chain's run it
/// still means after the Chain.
#[test]
fn a_plugin_inserted_past_a_chains_run_lands_after_the_chain() {
    let mut session = session();
    session.insert_effect_at(EffectKind::Chain, 0).expect("a Chain");
    session.insert_effect_into_container(EffectKind::Filter, 0).expect("a Filter in it");
    let mut engine = Engine::default();
    let inserted = session
        .insert_plugin_effect(gain_ref(), 2, &mut engine)
        .expect("the gain is inserted");
    assert_eq!(inserted.slot, 2);
    assert_eq!(
        rows(&session),
        [(EffectKind::Chain, 0), (EffectKind::Filter, 1), (EffectKind::Plugin, 0)]
    );
}
