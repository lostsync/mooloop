//! A plugin put in a chain from the window (MOO-83, plugin-hosting 08).
//!
//! FOCUS.md calls this lane 2's acceptance case: *the step that puts a
//! plugin in a chain from the window*. So the window is driven the way a
//! user drives it -- the join's menu, its "Plugin…" row, the browser's
//! PLUGINS tab, a double-click, a knob on the plugin's face -- through the
//! same handlers `AppUi::new` wires (`plugin_ui::wire`), with the in-repo
//! CLAP test double found through a scanner cache, as the app finds a
//! plugin. What reaches the engine is played through the plugin's own
//! processor, so the song's saved state is the plugin's and not a guess.
//! Then the song is saved, reopened and exported, and the export is held to
//! the gain the knob set.
//!
//! LSP's filter is the same case with a real plugin, from the window, and
//! it is Adam's to hear (FOCUS.md, "Listening is a step").

use super::*;
use crate::plugin_ui::{plugin_rows, PluginCatalog};
use crate::window_probe::{click, controls, install_backend, sliders, wheel, Control};
use i_slint_core::items::AccessibleRole;
use mooloop_core::{EffectParams, NoteEvent, PluginFormat, PluginRef, PluginSlotId, ProjectChannel};
use mooloop_dsp::{AudioNode, Event, EventList, ProcessContext, StereoBus, TimedEvent};
use mooloop_engine::{ExportFormat, ExportProgress, ExportSpec, OfflineRenderer, RenderScope, WavEncoding};
use mooloop_plugin_host::scan::PluginCache;
use mooloop_session::engine::PendingEngineMessage;
use mooloop_test_plugin as test_plugin;
use slint::LogicalSize;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc;

const RATE: u32 = 48_000;
const WIDTH: f32 = 2400.0;
const HEIGHT: f32 = 1400.0;

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

/// A scanner cache listing the test gain, the test sine (an instrument), and
/// a file that failed to scan, as the scanner would have written them.
fn cache_text(library: &Path) -> String {
    let path = library.display().to_string().replace('\\', "/");
    format!(
        "version = 1\n\n\
         [[file]]\npath = \"{path}\"\nmodified-ns = 0\nsize = 0\n\n\
         [[file.plugin]]\npath = \"{path}\"\nformat = \"clap\"\nid = \"{gain}\"\nname = \"Test Gain\"\n\
         vendor = \"{vendor}\"\nfeatures = [\"audio-effect\"]\naudio-inputs = [2]\naudio-outputs = [2]\n\n\
         [[file.plugin]]\npath = \"{path}\"\nformat = \"clap\"\nid = \"{sine}\"\nname = \"Test Sine\"\n\
         vendor = \"{vendor}\"\nfeatures = [\"instrument\"]\naudio-outputs = [2]\nnote-inputs = 1\n\n\
         [[file]]\npath = \"/usr/lib/clap/broken.clap\"\nmodified-ns = 0\nsize = 0\n\
         failed = {{ kind = \"crashed\", reason = \"killed by signal 11\" }}\n",
        gain = test_plugin::GAIN_ID,
        sine = test_plugin::SINE_ID,
        vendor = test_plugin::VENDOR,
    )
}

/// One drum-synth channel playing a one-bar loop: the lane's case, small.
fn drum_loop() -> Project {
    let mut project = Project::default();
    project.channels.clear();
    project.pattern_lengths[0] = 16;
    let mut channel = ProjectChannel::drum_synth(0, 1);
    // Quiet, so the knob's boost stays under the master's safety limiter
    // and the export's level is the plugin's gain and nothing else.
    channel.setup.channel.volume = 0.1;
    for (step, pitch) in [(0u32, 36u8), (4, 38), (8, 36), (12, 42)] {
        channel.notes[0].push(NoteEvent::new(step + 1, step * 24, 12, pitch, 110));
    }
    project.channels.push(channel);
    project.assign_channel_ids();
    project
}

/// Stands in for the engine behind the window's two queues: keeps every
/// processor it is handed, and plays every parameter change through the
/// processor that owns it, on a thread of its own, as the callback would.
struct Engine {
    rx: mpsc::Receiver<PendingEngineMessage>,
    nodes: Vec<Box<dyn AudioNode + Send>>,
    sent: Vec<EngineCommand>,
}

impl Engine {
    fn drain(&mut self) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                PendingEngineMessage::Structural(
                    StructuralCommand::InstallEffect { node, .. }
                    | StructuralCommand::ReplaceEffect { node, .. },
                ) => self.nodes.push(node),
                PendingEngineMessage::Command(command) => {
                    if let EngineCommand::SetEffectParam { id, value, .. } = command {
                        if let Some(node) = self.nodes.pop() {
                            self.nodes.push(process(node, id, value));
                        }
                    }
                    self.sent.push(command);
                }
                _ => {}
            }
        }
    }
}

/// One 256-frame block through `node` carrying one parameter change.
fn process(mut node: Box<dyn AudioNode + Send>, id: u32, value: f32) -> Box<dyn AudioNode + Send> {
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let mut bus = StereoBus::with_capacity(256);
                let mut list = EventList::empty();
                assert!(list.push_ordered(TimedEvent {
                    offset: 0,
                    event: Event::ParamValue { id, value },
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
                node
            })
            .join()
            .expect("the processing thread did not panic")
    })
}

struct Harness {
    window: MainWindow,
    state: Rc<RefCell<UiState>>,
    commands: Rc<RefCell<CommandState>>,
    engine: Engine,
    tx: EngineCommandSender,
    stx: StructuralCommandSender,
    dir: tempfile::TempDir,
    _reset_rx: mpsc::Receiver<usize>,
}

impl Harness {
    /// The pump's plugin work for one tick: the engine takes what was sent,
    /// the session services its plugins, the faces follow, and a finished
    /// plugin edit is recorded -- `AppUi`'s pump, in its order.
    fn tick(&mut self) {
        self.engine.drain();
        {
            let mut st = self.state.borrow_mut();
            let mut sink = plugin_ui::QueuedSink {
                tx: &self.tx,
                stx: &self.stx,
                sample_rate: RATE,
            };
            st.session.service_plugins(&mut sink);
        }
        self.engine.drain();
        record_finished_plugin_edits(&self.state, &self.commands, &self.window);
        self.state.borrow_mut().refresh_plugin_faces();
    }

    fn slider(&self, label: &str) -> Control {
        sliders(&self.window)
            .into_iter()
            .find(|slider| slider.label == label)
            .unwrap_or_else(|| panic!("the rack draws no {label:?} control"))
    }

    fn plugin_slot(&self) -> PluginSlotId {
        let st = self.state.borrow();
        match st.session.effect_chain().expect("a chain")[0].params {
            EffectParams::Plugin(slot) => slot,
            _ => panic!("the first device is not a plugin"),
        }
    }
}

/// The real window on a drum loop, the rack in the main pane, the plugin
/// handlers wired as `AppUi::new` wires them, and the scanner's cache.
fn harness_with(project: &Project) -> Harness {
    install_backend();
    let window = MainWindow::new().expect("the testing backend builds a window");
    window.window().set_size(LogicalSize::new(WIDTH, HEIGHT));
    install_strip_spec(&window);
    install_eq_spec(&window);
    let dir = tempfile::tempdir().expect("a scratch directory");
    let cache = dir.path().join("plugins.toml");
    std::fs::write(&cache, cache_text(&test_plugin_path())).expect("the cache is written");
    let state = Rc::new(RefCell::new(UiState::new(None, RATE, &window)));
    {
        let mut st = state.borrow_mut();
        st.plugin_cache_path = cache.clone();
        st.session
            .set_plugin_opener(mooloop_session::plugin_rack::clap_opener(cache));
        st.replace_project(project, &[None], &window);
        st.sync_effects();
    }
    window.invoke_move_view(view::DEVICES, 0);
    window.invoke_show_view(view::DEVICES);
    window.set_bottom_pane_visible(false);
    let (sender, rx) = mpsc::channel();
    let tx = EngineCommandSender(sender.clone());
    let stx = StructuralCommandSender(sender);
    let commands = Rc::new(RefCell::new(CommandState::default()));
    // Held for the harness's life: a dropped receiver fails the send.
    let (reset_tx, reset_rx) = mpsc::channel::<usize>();
    plugin_ui::wire(&window, &state, &commands, &tx, &stx, &reset_tx);
    Harness {
        window,
        state,
        commands,
        engine: Engine {
            rx,
            nodes: Vec::new(),
            sent: Vec::new(),
        },
        tx,
        stx,
        dir,
        _reset_rx: reset_rx,
    }
}

/// What Undo would take back now, by its label.
fn labels(history: &CommandState) -> Vec<&'static str> {
    history.history.undo_target().map(|entry| entry.label).into_iter().collect()
}

fn export(project: &Project, plugins: BTreeMap<PluginSlotId, Box<dyn AudioNode + Send>>, path: &Path) -> Vec<f32> {
    let spec = ExportSpec {
        path: path.to_path_buf(),
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
    hound::WavReader::open(path)
        .expect("a WAV")
        .into_samples::<f32>()
        .map(|sample| sample.expect("a float sample"))
        .collect()
}

fn rms(samples: &[f32]) -> f64 {
    (samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len().max(1) as f64).sqrt()
}

/// **From the window only: the join's "Plugin…", a double-click in the
/// browser, a knob on the plugin's face, one undo step; then saved,
/// reopened and exported with the knob's value.**
#[test]
fn a_plugin_goes_in_a_chain_from_the_window_and_its_knob_is_saved_and_exported() {
    let mut h = harness_with(&drum_loop());

    // The join's menu, and its last row.
    let join = controls(&h.window, AccessibleRole::Button)
        .into_iter()
        .find(|button| button.label == "Add device")
        .expect("the rack draws a join");
    click(&h.window, join.centre);
    let row = controls(&h.window, AccessibleRole::Button)
        .into_iter()
        .find(|row| row.label == "Plugin…")
        .expect("the join's menu offers a plugin");
    click(&h.window, row.centre);
    assert_eq!(h.window.get_browser_tab(), 2, "the browser opened on PLUGINS");
    assert!(h.window.get_sidebar_visible(), "and it is showing");

    // The PLUGINS tab lists what the scanner found, the instrument and the
    // broken file greyed with their reasons.
    let rows: Vec<BrowserRow> = h.state.borrow().browser_rows.iter().collect();
    let named = |name: &str| rows.iter().find(|row| row.name == name).cloned();
    let gain = named("Test Gain").expect("the tab lists the test gain");
    assert!(gain.loadable, "an effect can go in a chain");
    let sine = named("Test Sine").expect("the tab lists the instrument");
    assert!(sine.loadable, "an instrument is offered, for a new channel");
    assert!(!sine.effect, "but not as an effect");
    assert!(sine.detail.contains("Instrument"), "{}", sine.detail);
    assert!(!sine.detail.contains("no notes"), "instruments play now (MOO-85): {}", sine.detail);
    let broken = named("broken.clap").expect("the tab lists the file that failed");
    assert!(broken.detail.contains("signal 11"), "{}", broken.detail);

    // A double-click on it puts it in the chain, where the join was.
    let at = controls(&h.window, AccessibleRole::ListItem)
        .into_iter()
        .find(|row| row.label == "Test Gain")
        .expect("the browser draws the plugin's row")
        .centre;
    click(&h.window, at);
    assert!(h.state.borrow().session.effect_chain().unwrap().is_empty(), "one click only selects");
    click(&h.window, at);
    let slot = h.plugin_slot();
    assert!(h.state.borrow().session.plugin_problem(slot).is_none(), "the plugin opened");
    assert_eq!(labels(&h.commands.borrow()), ["Plugin added"], "adding it is one undo step");
    h.tick();

    // Its face: the plugin's own parameters, under its own names, with the
    // plugin's own text. The Fail switch is the test double's; it is hidden
    // from nothing, and it is left alone here.
    let gain_knob = h.slider("Gain");
    assert_eq!(gain_knob.value, "0.0 dB", "the plugin writes its own readout");
    assert!(sliders(&h.window).iter().any(|slider| slider.label == "Latency"));

    // Turn it, inside a gesture as a drag is, and pause: nothing is recorded
    // while the gesture is open, however long the plugin is quiet.
    h.state.borrow_mut().begin_gesture(&h.window);
    wheel(&h.window, &gain_knob);
    let quiet = mooloop_session::plugin_rack::EDIT_QUIET_TICKS as usize + 2;
    for _ in 0..quiet {
        h.tick();
    }
    assert_eq!(labels(&h.commands.borrow()), ["Plugin added"], "recorded mid-gesture");
    gesture_closed(&h.state, &h.commands, &h.window);
    for _ in 0..quiet {
        h.tick();
    }
    assert_eq!(labels(&h.commands.borrow()), ["Plugin Edit"], "the turn is one undo step");

    // What the knob sent: the plugin's own id, never an index, and the
    // value the plugin now reports and writes.
    let index = h
        .state
        .borrow()
        .session
        .plugin_param_index(slot, test_plugin::PARAM_GAIN)
        .expect("listed");
    let turned = h.state.borrow().session.plugin_param_value(slot, index).expect("live");
    assert!(turned > 0.0, "the wheel turned the gain up: {turned}");
    assert!(h.engine.sent.iter().any(|command| matches!(
        command,
        EngineCommand::SetEffectParam { id, .. } if *id == test_plugin::PARAM_GAIN
    )));
    let shown = h.slider("Gain").value;
    assert_eq!(shown, format!("{turned:.1} dB"), "the face follows the plugin");

    // Saved as the window saves, and reopened.
    h.state.borrow_mut().session.capture_plugin_states();
    let song = project_snapshot(&h.state.borrow(), &h.window).project;
    let path = h.dir.path().join("plugin-case.mooloop");
    mooloop_project::save_song(&path, &song, mooloop_project::AssetMode::Referenced).expect("it saves");
    let report = mooloop_project::load_bundle(&path).expect("it reopens");
    assert!(report.repairs.is_empty(), "the song needed repairs: {:?}", report.repairs);
    let mooloop_project::LoadedDocument::Song(reopened) = report.document else {
        panic!("a song came back as something else");
    };
    let mut again = harness_with(&reopened);
    for _ in 0..3 {
        again.tick();
    }
    let slot = again.plugin_slot();
    let reopened_gain = again.state.borrow().session.plugin_param_value(slot, index).expect("live");
    // The plugin heard the value as the engine's `f32`.
    assert!(
        (reopened_gain - turned).abs() < 1e-4,
        "the reopened plugin's gain is {reopened_gain}, the knob set {turned}"
    );
    assert_eq!(again.slider("Gain").value, shown, "and its face says so");

    // Exported, as the window exports: the plugin's second instance plays
    // the loop at the knob's gain, against the same loop dry.
    let plugins = again.state.borrow_mut().session.export_plugin_processors(RATE);
    let wet = export(&reopened, plugins, &again.dir.path().join("wet.wav"));
    let dry = export(&drum_loop(), BTreeMap::new(), &again.dir.path().join("dry.wav"));
    let ratio = rms(&wet) / rms(&dry);
    let expected = 10f64.powf(turned / 20.0);
    assert!(
        (ratio - expected).abs() < 1e-3,
        "the export plays the loop at {ratio}x, the knob said {expected}x"
    );
}

/// **A missing plugin keeps its face and says it is missing**, drawn from
/// what the song remembers of it, its knobs moving nothing.
#[test]
fn a_missing_plugin_shows_its_face_and_says_it_is_missing() {
    // A song that names a plugin this machine does not have: inserted
    // through a session with no opener, which is what a missing plugin is,
    // and given the parameter list the song would remember from the machine
    // it was made on.
    let mut session = Session::default();
    session.replace_project(&drum_loop(), &[]);
    let (sender, _rx) = mpsc::channel();
    let (tx, stx) = (EngineCommandSender(sender.clone()), StructuralCommandSender(sender));
    let mut sink = plugin_ui::QueuedSink {
        tx: &tx,
        stx: &stx,
        sample_rate: RATE,
    };
    let gone = PluginRef {
        format: PluginFormat::Clap,
        id: "com.example.not-installed".into(),
        name: "Gone Filter".into(),
        vendor: "Nobody".into(),
        version: String::new(),
    };
    let inserted = session.insert_plugin_effect(gone, 0, &mut sink).expect("the device is inserted");
    let EffectParams::Plugin(slot) = inserted.params else {
        unreachable!("a plugin device");
    };
    session.plugins.get_mut(&slot).expect("its slot").params = vec![mooloop_core::PluginParamInfo {
        id: 7,
        name: "Cutoff".into(),
        module: String::new(),
        min: 20.0,
        max: 20_000.0,
        default: 1_000.0,
        stepped: None,
        automatable: true,
        modulatable: true,
        hidden: false,
    }];
    let project = session.project_snapshot(120, 0);

    let mut h = harness_with(&project);
    for _ in 0..3 {
        h.tick();
    }
    let badge = controls(&h.window, AccessibleRole::Text)
        .into_iter()
        .find(|text| text.label.starts_with("Missing"))
        .expect("the face says the plugin is missing");
    assert!(badge.label.contains("Gone Filter"), "{}", badge.label);
    let cutoff = h.slider("Cutoff");
    assert!(!cutoff.value.is_empty(), "the song's parameter list is drawn");
    // Its knob sends nothing: there is no plugin to hear it.
    wheel(&h.window, &cutoff);
    h.tick();
    assert!(h.engine.sent.is_empty(), "a missing plugin's knob sent {:?}", h.engine.sent);
}

/// The PLUGINS tab's filter, by name, vendor and what a row says.
#[test]
fn the_plugin_filter_matches_names_vendors_and_reasons() {
    let cache = PluginCache::from_toml(&cache_text(Path::new("/x/test.clap"))).expect("a cache");
    let catalog = PluginCatalog::from_cache(&cache);
    let names = |filter: &str| -> Vec<String> {
        plugin_rows(&catalog, filter).iter().map(|row| row.name.to_string()).collect()
    };
    assert_eq!(names(""), ["Test Gain", "Test Sine", "broken.clap"]);
    assert_eq!(names("gain"), ["Test Gain"]);
    assert_eq!(names("mooloop sine"), ["Test Sine"]);
    assert_eq!(names("instrument"), ["Test Sine"]);
    assert_eq!(names("signal"), ["broken.clap"]);
    assert!(names("nothing like it").is_empty());
}

/// **An instrument from the window: the add-channel menu's "Add Plugin…",
/// then a double-click in the browser, makes a new channel whose source is
/// that plugin**, as one undo step (MOO-83 on MOO-84). The row is pressed in
/// the real window: the menu it sits in is where MOO-53 shipped a row that
/// closed before it reported.
#[test]
fn an_instrument_from_the_add_channel_menu_becomes_a_new_plugin_channel() {
    let mut h = harness_with(&drum_loop());
    h.window.invoke_move_view(view::STEPS, 0);
    h.window.invoke_show_view(view::STEPS);
    let before = h.state.borrow().session.channels.len();

    let add = controls(&h.window, AccessibleRole::Button)
        .into_iter()
        .find(|button| button.label == "Add channel")
        .expect("the channel rack draws its + button");
    click(&h.window, add.centre);
    let row = controls(&h.window, AccessibleRole::Button)
        .into_iter()
        .find(|row| row.label == "Add Plugin…")
        .expect("the add-channel menu offers a plugin");
    click(&h.window, row.centre);
    assert_eq!(h.window.get_browser_tab(), 2, "the row opened the PLUGINS tab");
    assert_eq!(
        h.state.borrow().session.channels.len(),
        before,
        "the row itself adds no channel: the plugin is chosen first"
    );

    let at = controls(&h.window, AccessibleRole::ListItem)
        .into_iter()
        .find(|row| row.label == "Test Sine")
        .expect("the browser draws the instrument's row")
        .centre;
    click(&h.window, at);
    click(&h.window, at);
    h.tick();

    let st = h.state.borrow();
    assert_eq!(st.session.channels.len(), before + 1, "a channel was added");
    let channel = &st.session.channels[before];
    assert_eq!(channel.kind(), mooloop_core::DeviceKind::Plugin);
    let mooloop_core::GeneratorParams::Plugin(slot) = channel.generator_params() else {
        panic!("the new channel's source is not a plugin");
    };
    assert_eq!(
        st.session.plugins.get(&slot).map(|saved| saved.plugin.id.as_str()),
        Some(test_plugin::SINE_ID),
        "its source is the instrument picked"
    );
    assert_eq!(channel.name, "Test Sine", "named after the plugin");
    assert!(
        st.session.plugin_problem(slot).is_none(),
        "the sine is hosted as the channel's source: {:?}",
        st.session.plugin_problem(slot)
    );
    assert_eq!(st.session.selected, before, "and selected");
    drop(st);
    assert_eq!(
        labels(&h.commands.borrow()),
        ["Plugin channel added"],
        "the channel and its source are one undo step"
    );
}
