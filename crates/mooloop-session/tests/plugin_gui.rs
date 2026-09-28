//! Step 11's session half, headless (`docs/plans/plugin-hosting/11-plugin-gui-windows.md`,
//! MOO-300): a hosted plugin's GUI opened, closed, reopened, resized and
//! removed through the session's verbs, the way the window side will call
//! them from the pump, and the audio running on while it opens and closes.
//!
//! The plugin is the in-repo test gain's GUI variant, found through a scan
//! cache as the app finds it. Its GUI draws nothing and needs no display.
//! Its counters are read through probe ids (`mooloop_test_plugin::PROBE_IDS`)
//! on a second instance opened directly, which also keeps the library
//! loaded. The library-wide counts are process-global, so every test here
//! holds [`SERIAL`].

#![cfg(all(unix, not(target_os = "macos")))]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mooloop_core::{EffectParams, NoteEvent, PluginFormat, PluginRef, PluginSlotId, PluginState, Project, ProjectChannel};
use mooloop_dsp::AudioNode;
use mooloop_engine::{CommandSink, StructuralCommand};
use mooloop_plugin_host::clap::{ClapInstance, ClapOpener};
use mooloop_plugin_host::scan::PluginCache;
use mooloop_plugin_host::{AudioConfig, GuiConfig, GuiError, GuiSize, HostedInstance};
use mooloop_session::plugin_rack::{GuiPlacement, PluginGuiEvent, PluginGuiOpen};
use mooloop_session::session::Session;
use mooloop_test_plugin as test_plugin;

const RATE: u32 = 48_000;

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

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

fn gain_gui_ref() -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: test_plugin::GAIN_GUI_ID.to_owned(),
        name: "Test Gain (GUI)".to_owned(),
        vendor: test_plugin::VENDOR.to_owned(),
        version: String::new(),
    }
}

fn cache_listing(library: &Path) -> PluginCache {
    let path = library.display().to_string().replace('\\', "/");
    let text = format!(
        "version = 1\n\n[[file]]\npath = \"{path}\"\nmodified-ns = 0\nsize = 0\n\n\
         [[file.plugin]]\npath = \"{path}\"\nformat = \"clap\"\nid = \"{id}\"\nname = \"Test Gain (GUI)\"\n\
         vendor = \"{vendor}\"\nfeatures = [\"audio-effect\"]\naudio-inputs = [2]\naudio-outputs = [2]\nhas-gui = true\n",
        id = test_plugin::GAIN_GUI_ID,
        vendor = test_plugin::VENDOR,
    );
    PluginCache::from_toml(&text).expect("a cache the scanner could have written")
}

/// A second instance, opened outside the session, whose probes read the
/// library-wide counts and whose life keeps the library loaded.
fn witness() -> ClapInstance {
    ClapInstance::open(
        &test_plugin_path(),
        &gain_gui_ref(),
        &PluginState::default(),
        AudioConfig {
            sample_rate: RATE,
            max_frames: 512,
        },
    )
    .expect("the test plugin opens")
}

/// GUIs, timers and fds live across the library, and GUIs leaked.
fn live(witness: &mut ClapInstance) -> [u64; 4] {
    [
        test_plugin::PROBE_GUIS_LIVE,
        test_plugin::PROBE_TIMERS_LIVE,
        test_plugin::PROBE_FDS_LIVE,
        test_plugin::PROBE_GUIS_LEAKED,
    ]
    .map(|probe| witness.param_value(probe).expect("a probe") as u64)
}

fn probe(session: &mut Session, slot: PluginSlotId, probe: u32) -> u64 {
    session
        .plugin_rack
        .instance_mut(slot)
        .expect("a live instance")
        .param_value(probe)
        .expect("a probe") as u64
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
            StructuralCommand::InstallEffect { node, .. } | StructuralCommand::ReplaceEffect { node, .. } => {
                self.nodes.push(node)
            }
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

/// A session playing the loop with the test gain's GUI variant on channel
/// 0, and its slot.
fn hosting(engine: &mut Engine) -> (Session, PluginSlotId, usize) {
    let mut session = Session::default();
    session.set_plugin_opener(Box::new(ClapOpener::with_cache(cache_listing(&test_plugin_path()))));
    session.replace_project(&drum_loop(), &[]);
    let inserted = session
        .insert_plugin_effect(gain_gui_ref(), 0, engine)
        .expect("the device is inserted");
    let EffectParams::Plugin(slot) = inserted.params else {
        panic!("a plugin device");
    };
    assert!(session.plugin_problem(slot).is_none(), "the plugin is hosted");
    (session, slot, inserted.slot)
}

fn embedded(parent: u64) -> PluginGuiOpen {
    PluginGuiOpen {
        placement: GuiPlacement::Embedded { parent },
        title: "Test Gain (GUI) - Drums".to_owned(),
        scale: Some(1.0),
    }
}

#[test]
fn a_gui_opens_closes_reopens_resizes_and_goes_with_its_device() {
    let _serial = serial();
    let mut witness = witness();
    let mut engine = Engine::default();
    let (mut session, slot, row) = hosting(&mut engine);
    assert_eq!(engine.nodes.len(), 1, "the processor is out");

    assert_eq!(session.plugin_gui_kind(slot), Ok(GuiConfig::X11_EMBEDDED));
    let title = session.plugin_gui_title(slot).expect("a title");
    assert!(title.starts_with("Test Gain (GUI)"), "{title}");

    // Open.
    let opened = session.open_plugin_gui(slot, &embedded(0x40_0001)).expect("it opens");
    assert_eq!(opened.config, GuiConfig::X11_EMBEDDED);
    assert_eq!(
        opened.size,
        Some(GuiSize {
            width: test_plugin::GUI_DEFAULT_SIZE.width,
            height: test_plugin::GUI_DEFAULT_SIZE.height,
        })
    );
    assert!(opened.can_resize);
    assert_eq!(session.open_plugin_gui(slot, &embedded(1)), Err(GuiError::AlreadyOpen));
    assert_eq!(live(&mut witness), [1, 1, 1, 0]);

    // Its event loop runs from the pump's call, and never waits.
    let start = Instant::now();
    let tick = Duration::from_millis(u64::from(test_plugin::GUI_TIMER_MS) + 1);
    let mut fired = session.service_plugin_io_at(start + tick);
    fired += session.service_plugin_io_at(start + tick * 2);
    assert_eq!((fired.timers_fired, fired.fds_fired), (2, 2));
    assert_eq!(probe(&mut session, slot, test_plugin::PROBE_TIMER_TICKS), 2);
    assert_eq!(probe(&mut session, slot, test_plugin::PROBE_FD_READS), 2);

    // Resize: the user drags the window small, and the plugin's minimum
    // is what the window snaps to.
    let taken = session
        .resize_plugin_gui(slot, GuiSize { width: 10, height: 10 })
        .expect("it resizes");
    assert_eq!(
        taken,
        GuiSize {
            width: test_plugin::GUI_MIN_SIZE.width,
            height: test_plugin::GUI_MIN_SIZE.height,
        }
    );
    // A new scale: the test GUI asks for its window at that scale.
    session.set_plugin_gui_scale(slot, 2.0).expect("a scale");
    assert_eq!(
        session.drain_plugin_gui_events(),
        [(
            slot,
            PluginGuiEvent::Resize(GuiSize {
                width: taken.width * 2,
                height: taken.height * 2,
            })
        )]
    );

    // Close, and nothing about the audio moves.
    assert!(session.close_plugin_gui(slot));
    assert!(!session.plugin_gui_is_open(slot));
    assert_eq!(live(&mut witness), [0, 0, 0, 0], "its timer and fd went with it");
    let quiet = session.service_plugin_io_at(start + tick * 10);
    assert_eq!((quiet.timers_fired, quiet.fds_fired), (0, 0));
    for _ in 0..3 {
        session.service_plugins(&mut engine);
    }
    assert_eq!(engine.nodes.len(), 1, "no pull-back, no rebuild");

    // Reopen, then remove the device with the GUI open.
    session.open_plugin_gui(slot, &embedded(0x40_0002)).expect("it reopens");
    assert!(session.plugin_gui_is_open(slot));
    session.remove_effect_at(row).expect("the device is removed");
    session.service_plugins(&mut engine);
    assert!(!session.plugin_gui_is_open(slot));
    assert_eq!(session.drain_plugin_gui_events(), [(slot, PluginGuiEvent::Closed)]);
    assert_eq!(live(&mut witness), [0, 0, 0, 0]);

    // Quit: everything retires once its processor is back, and no GUI was
    // ever dropped open.
    session.close_plugins(&mut engine);
    engine.nodes.clear();
    session.collect_plugins();
    assert!(session.plugins_retired());
    assert_eq!(live(&mut witness), [0, 0, 0, 0], "no GUI leaked");
    assert_eq!(witness.misbehaviour(), 0);
}

#[test]
fn quitting_with_a_gui_open_destroys_it_before_the_instance() {
    let _serial = serial();
    let mut witness = witness();
    let mut engine = Engine::default();
    let (mut session, slot, _) = hosting(&mut engine);
    session.open_plugin_gui(slot, &embedded(0x40_0003)).expect("it opens");
    session.close_plugins(&mut engine);
    assert_eq!(session.drain_plugin_gui_events(), [(slot, PluginGuiEvent::Closed)]);
    assert_eq!(live(&mut witness)[0], 0, "the GUI went before the processor came back");
    engine.nodes.clear();
    session.collect_plugins();
    assert!(session.plugins_retired());
    assert_eq!(live(&mut witness), [0, 0, 0, 0]);
}

/// The audio thread does not notice a GUI opening and closing: a hundred
/// cycles, with the plugin's processor in the chain of an executor paced
/// as a driver would call it, and not one callback overran its block.
#[test]
fn audio_runs_on_through_a_hundred_gui_opens_and_closes() {
    let _serial = serial();
    let mut witness = witness();
    let mut engine = Engine::default();
    let (mut session, slot, _) = hosting(&mut engine);
    let node = engine.nodes.pop().expect("the processor");
    let song = session.project_snapshot(120, 0);
    let plugins = BTreeMap::from([(slot, node)]);

    let (run, cycles) = mooloop_engine::live_check::run_paced_while(&song, plugins, RATE, 512, || {
        let mut cycles = 0;
        for cycle in 0..100u64 {
            session
                .open_plugin_gui(slot, &embedded(0x50_0000 + cycle))
                .expect("it opens");
            session.service_plugin_io();
            let _ = session.drain_plugin_gui_events();
            assert!(session.close_plugin_gui(slot));
            cycles += 1;
            // Let the audio thread take its turns in between.
            std::thread::sleep(Duration::from_millis(1));
        }
        cycles
    });
    assert_eq!(cycles, 100);
    assert!(run.callbacks >= 10, "the audio ran throughout: {run:?}");
    assert_eq!(run.xruns, 0, "no callback overran its block: {run:?}");
    assert_eq!(live(&mut witness), [0, 0, 0, 0]);

    // The processor came back when the executor went: the instance retires.
    session.close_plugins(&mut engine);
    session.collect_plugins();
    assert!(session.plugins_retired());
}
