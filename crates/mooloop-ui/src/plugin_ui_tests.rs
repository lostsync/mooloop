//! A plugin put in a chain from the window (MOO-83, plugin-hosting 08).
//!
//! The window is driven the way a user drives it -- the join's menu, its
//! "Plugin…" row, the browser's PLUGINS tab, a double-click, a knob on the
//! plugin's face -- through the same handlers `AppUi::new` wires
//! (`plugin_ui::wire`), with the in-repo CLAP test double found through a
//! scanner cache, as the app finds a plugin. What reaches the engine is
//! played through the plugin's own processor, so the song's saved state is
//! the plugin's and not a guess. Then the song is saved, reopened and
//! exported, and the export is held to the gain the knob set.
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
    /// The key each `InstallEffect` gave its slot, in the order sent.
    install_keys: Vec<Option<u64>>,
    /// The key each `ReplaceEffect` expected to find, in the order sent.
    /// The engine lets a replacement in only where the slot's key is
    /// `Some` of this (`EffectChain::replace_if_kind`).
    replace_keys: Vec<u64>,
}

impl Engine {
    fn drain(&mut self) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                PendingEngineMessage::Structural(StructuralCommand::InstallEffect {
                    node, resource_key, ..
                }) => {
                    self.install_keys.push(resource_key);
                    self.nodes.push(node);
                }
                PendingEngineMessage::Structural(StructuralCommand::ReplaceEffect {
                    node,
                    expected_resource_key,
                    ..
                }) => {
                    self.replace_keys.push(expected_resource_key);
                    self.nodes.push(node);
                }
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
                // Stopped where it ran, as the engine retires a node leaving
                // the audio thread (MOO-311).
                node.retire();
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
            let main = crate::plugin_gui::MainWindowState::of(self.window.window());
            st.pump_plugin_guis(&main);
        }
        self.engine.drain();
        record_finished_plugin_edits(&self.state, &self.commands, &self.window);
        self.state.borrow_mut().refresh_plugin_faces();
        self.state.borrow().refresh_plugin_param_list(&self.window);
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
    plugin_ui::wire(&window, &state, &commands, &tx, &stx, &reset_tx, Rc::new(|| true));
    Harness {
        window,
        state,
        commands,
        engine: Engine {
            rx,
            nodes: Vec::new(),
            sent: Vec::new(),
            install_keys: Vec::new(),
            replace_keys: Vec::new(),
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
    // Latency names each of its positions, so it is a selector (MOO-229).
    assert!(controls(&h.window, AccessibleRole::Button)
        .iter()
        .any(|button| button.label == "Latency: 64 frames"));

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
        plugin_rows(&catalog, filter, false).iter().map(|row| row.name.to_string()).collect()
    };
    assert_eq!(names(""), ["Test Gain", "Test Sine", "broken.clap"]);
    assert_eq!(names("gain"), ["Test Gain"]);
    assert_eq!(names("mooloop sine"), ["Test Sine"]);
    assert_eq!(names("instrument"), ["Test Sine"]);
    assert_eq!(names("signal"), ["broken.clap"]);
    assert!(names("nothing like it").is_empty());
}

/// [`cache_text`]'s file and broken file, and beside them a plugin mooloop
/// can't use yet (a four-channel effect: it loads, the host does not wire
/// it) and one that could not be created (its `instantiate` failed).
fn unusable_cache_text(library: &Path) -> String {
    format!(
        "{}\n\
         [[file]]\npath = \"/usr/lib/clap/odd.clap\"\nmodified-ns = 0\nsize = 0\n\n\
         [[file.plugin]]\npath = \"/usr/lib/clap/odd.clap\"\nformat = \"clap\"\nid = \"com.example.quad\"\n\
         name = \"Quad Bus\"\nvendor = \"Odd\"\nfeatures = [\"audio-effect\"]\naudio-inputs = [4]\naudio-outputs = [4]\n\n\
         [[file.plugin]]\npath = \"/usr/lib/clap/odd.clap\"\nformat = \"clap\"\nid = \"com.example.crashy\"\n\
         name = \"Crashy Verb\"\nvendor = \"Odd\"\nfeatures = [\"audio-effect\"]\naudio-inputs = [2]\naudio-outputs = [2]\n\
         error = \"instantiate returned null\"\n",
        cache_text(library),
    )
}

/// **"Hide plugins mooloop can't use yet" leaves out the unsupported, and
/// never what failed** (MOO-298): a usable plugin, one refused as
/// unsupported, one that could not be created and a file that failed to
/// scan, with the switch off and on. Off, everything is listed and the
/// unusable greyed with their reasons, as before the switch existed.
#[test]
fn hiding_unsupported_plugins_never_hides_a_failed_one() {
    let cache = PluginCache::from_toml(&unusable_cache_text(Path::new("/x/test.clap"))).expect("a cache");
    let catalog = PluginCatalog::from_cache(&cache);
    let rows = |hide: bool| -> Vec<(String, bool, String)> {
        plugin_rows(&catalog, "", hide)
            .iter()
            .map(|row| (row.name.to_string(), row.loadable, row.detail.to_string()))
            .collect()
    };
    let crashy = (
        "Crashy Verb".to_owned(),
        false,
        "could not be created: instantiate returned null".to_owned(),
    );
    let quad = (
        "Quad Bus".to_owned(),
        false,
        "its main input has 4 channels; 1 or 2 are hosted".to_owned(),
    );
    let broken = ("broken.clap".to_owned(), false, "failed to scan: killed by signal 11".to_owned());
    let usable = |all: &[(String, bool, String)]| -> Vec<String> {
        all.iter().filter(|row| row.1).map(|row| row.0.clone()).collect()
    };

    let shown = rows(false);
    assert_eq!(usable(&shown), ["Test Gain", "Test Sine"]);
    for unusable in [&crashy, &quad, &broken] {
        assert!(shown.contains(unusable), "off: {unusable:?} is listed greyed, in {shown:?}");
    }

    let hidden = rows(true);
    assert_eq!(usable(&hidden), ["Test Gain", "Test Sine"], "a usable plugin is never hidden");
    assert!(hidden.contains(&crashy), "a plugin that failed stays, with its reason: {hidden:?}");
    assert!(hidden.contains(&broken), "and so does a file that failed: {hidden:?}");
    assert!(!hidden.iter().any(|row| row.0 == "Quad Bus"), "the unsupported one is gone: {hidden:?}");
    assert_eq!(hidden.len(), shown.len() - 1);

    // The page: what failed to load, plugin and file, and how many are hidden.
    let failed: Vec<(String, String)> = catalog
        .failure_rows()
        .iter()
        .map(|row| (row.name.to_string(), row.reason.to_string()))
        .collect();
    assert_eq!(
        failed,
        [
            ("Crashy Verb".to_owned(), "could not be created: instantiate returned null".to_owned()),
            ("broken.clap".to_owned(), "killed by signal 11".to_owned()),
        ]
    );
    assert_eq!(catalog.hideable_count(), 1);
    assert_eq!(plugin_ui::hidden_status(1, true), "1 plugin hidden");
    assert_eq!(plugin_ui::hidden_status(3, false), "3 plugins shown greyed");
    assert_eq!(plugin_ui::hidden_status(0, true), "");
}

/// **The hide switch on Preferences > Plugins is saved, read back at the
/// next start, and the PLUGINS tab follows it at once**; a save that fails
/// keeps the browser as it was. Saved to a scratch file: a test must not
/// write the settings of whoever runs it.
#[test]
fn the_hide_switch_is_saved_and_the_browser_follows_it() {
    use crate::settings::{SettingsError, UiSettings};
    let h = harness_with(&drum_loop());
    let cache = h.state.borrow().plugin_cache_path.clone();
    std::fs::write(&cache, unusable_cache_text(&test_plugin_path())).expect("the cache is written");
    h.state.borrow_mut().enter_browser_tab(BrowserTab::Plugins);
    let file = h.dir.path().join("settings.toml");
    let saver: plugin_ui::SettingsSaver = {
        let file = file.clone();
        Rc::new(move |settings: &UiSettings| settings.save_to(&file))
    };
    let listed = |h: &Harness| -> Vec<String> {
        let st = h.state.borrow();
        crate::refresh_browser(&st);
        st.browser_rows.iter().map(|row| row.name.to_string()).collect()
    };
    let settings = Rc::new(RefCell::new(UiSettings::default()));
    plugin_ui::wire_hide_unsupported_toggle(&h.window, &h.state, &settings, saver.clone());
    plugin_ui::show_plugin_preferences(&h.window, &settings.borrow().plugins, &cache);
    assert!(!h.window.get_preferences_plugin_hide_unsupported(), "off by default");
    assert!(listed(&h).contains(&"Quad Bus".to_owned()));
    assert_eq!(h.window.get_preferences_plugin_hidden_status(), "1 plugin shown greyed");
    assert_eq!(h.window.get_preferences_plugin_failures().row_count(), 2, "a plugin and a file failed");

    h.window.invoke_preferences_plugin_hide_unsupported_toggled(true);
    assert!(h.window.get_preferences_plugin_hide_unsupported());
    let shown = listed(&h);
    assert!(!shown.contains(&"Quad Bus".to_owned()), "{shown:?}");
    assert!(shown.contains(&"Crashy Verb".to_owned()) && shown.contains(&"broken.clap".to_owned()));
    assert_eq!(h.window.get_preferences_plugin_hidden_status(), "1 plugin hidden");

    // The next start: the saved file read, the switch wired again, the
    // browser hiding from the first time it lists.
    let restarted = Rc::new(RefCell::new(UiSettings::load_or_default_from(&file)));
    assert!(restarted.borrow().plugins.hide_unsupported, "saved, for the next start to read");
    h.state.borrow_mut().hide_unsupported_plugins = false;
    plugin_ui::wire_hide_unsupported_toggle(&h.window, &h.state, &restarted, saver);
    assert!(!listed(&h).contains(&"Quad Bus".to_owned()), "hidden again after the restart");

    // A save that fails keeps the switch, and the browser, as they were.
    let failing: plugin_ui::SettingsSaver =
        Rc::new(|_: &UiSettings| Err(SettingsError::Io(std::io::Error::other("read-only"))));
    plugin_ui::wire_hide_unsupported_toggle(&h.window, &h.state, &restarted, failing);
    h.window.invoke_preferences_plugin_hide_unsupported_toggled(false);
    assert!(restarted.borrow().plugins.hide_unsupported);
    assert!(h.window.get_preferences_plugin_hide_unsupported());
    assert!(!listed(&h).contains(&"Quad Bus".to_owned()));
    assert!(h.window.get_preferences_error().contains("read-only"));
}

/// A song naming a plugin this machine does not have, with the list of
/// `count` parameters the song remembers of it: `P0`, `P1`, ... with
/// "Frequency" at index 14 under the group "Filter", as LSP's is past the
/// first page.
fn big_missing_plugin(count: u32) -> Project {
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
        id: "com.example.big-filter".into(),
        name: "Big Filter".into(),
        vendor: "Nobody".into(),
        version: String::new(),
    };
    let inserted = session.insert_plugin_effect(gone, 0, &mut sink).expect("the device is inserted");
    let EffectParams::Plugin(slot) = inserted.params else {
        unreachable!("a plugin device");
    };
    session.plugins.get_mut(&slot).expect("its slot").params = (0..count)
        .map(|index| mooloop_core::PluginParamInfo {
            id: 1_000 + index,
            name: if index == 14 { "Frequency".into() } else { format!("P{index}") },
            module: if (12..16).contains(&index) { "Filter".into() } else { String::new() },
            min: 0.0,
            max: 1.0,
            default: 0.5,
            stepped: None,
            automatable: true,
            modulatable: true,
            hidden: false,
        })
        .collect();
    session.project_snapshot(120, 0)
}

fn face_labels(h: &Harness) -> Vec<String> {
    sliders(&h.window)
        .into_iter()
        .map(|slider| slider.label)
        .filter(|label| label == "Frequency" || label.starts_with('P'))
        .collect()
}

/// **A big plugin's face shows eight, and the sidebar finds and pins the
/// rest** (MOO-229): Frequency, past the first page, is found by typing
/// part of its name into the PARAMETERS list and pinned to the face, as one
/// undo step, and the pin is saved with the song.
#[test]
fn a_big_plugin_shows_eight_and_the_sidebar_pins_the_rest() {
    let mut h = harness_with(&big_missing_plugin(40));
    h.window.set_channel_sidebar_visible(true);
    h.tick();
    h.state.borrow().refresh_plugin_param_list(&h.window);
    assert_eq!(h.window.get_plugin_param_total(), 0, "no list until the plugin is selected");

    h.state.borrow_mut().session.select_device(Some(0));
    h.state.borrow().sync_effects();
    h.tick();
    h.state.borrow().refresh_plugin_param_list(&h.window);
    assert_eq!(face_labels(&h), ["P0", "P1", "P2", "P3", "P4", "P5", "P6", "P7"]);
    assert_eq!(h.window.get_plugin_param_total(), 40);
    assert_eq!(h.window.get_plugin_param_rows().row_count(), 40);

    // Typed into the list's field, as a user types it.
    h.window.invoke_plugin_param_filter_edited("freq".into());
    let rows = h.window.get_plugin_param_rows();
    assert_eq!(rows.row_count(), 1, "the filter leaves Frequency alone");
    let row = rows.row_data(0).expect("a row");
    assert_eq!((row.name.as_str(), row.group.as_str(), row.pinned), ("Frequency", "Filter", false));

    // Its pin, pressed in the real sidebar.
    let pin = controls(&h.window, AccessibleRole::Checkbox)
        .into_iter()
        .find(|control| control.label == "Frequency")
        .expect("the sidebar draws Frequency's pin");
    assert!(!pin.checked);
    click(&h.window, pin.centre);
    h.tick();
    assert_eq!(labels(&h.commands.borrow()), ["Pin Parameter"], "a pin is one undo step");
    assert!(face_labels(&h).contains(&"Frequency".to_string()), "{:?}", face_labels(&h));
    let slot = h.plugin_slot();
    let pinned = h.state.borrow().session.plugins[&slot].pinned.clone();
    assert_eq!(pinned, [1_000, 1_001, 1_002, 1_003, 1_004, 1_005, 1_006, 1_007, 1_014]);
    assert!(h.window.get_plugin_param_rows().row_data(0).expect("a row").pinned);
    // Nine controls on a six-across face: two rows, and one page.
    assert_eq!(h.state.borrow().effect_slot_model.row_data(0).expect("a row").units, 2);

    // A second press takes it off again.
    h.window.invoke_plugin_param_pin_toggled(14);
    h.tick();
    assert!(!face_labels(&h).contains(&"Frequency".to_string()));
}

/// **A stepped parameter the plugin names at every position is a selector**
/// (MOO-229): the test gain's Latency (three named positions) is drawn as
/// three segments under the plugin's own words, and a press on one sets it.
#[test]
fn a_named_stepped_parameter_is_a_selector_that_sets_its_position() {
    let mut h = live_gain();
    let segment = |h: &Harness, label: &str| {
        controls(&h.window, AccessibleRole::Button)
            .into_iter()
            .find(|button| button.label == label)
            .unwrap_or_else(|| panic!("the face draws no {label:?} segment"))
    };
    assert!(!sliders(&h.window).iter().any(|slider| slider.label == "Latency"), "not a knob");
    segment(&h, "Latency: 0 frames");
    let at = segment(&h, "Latency: 512 frames").centre;
    click(&h.window, at);
    h.tick();
    let slot = h.plugin_slot();
    let index = h
        .state
        .borrow()
        .session
        .plugin_param_index(slot, test_plugin::PARAM_LATENCY)
        .expect("the gain lists Latency");
    assert_eq!(h.state.borrow().session.plugin_param_value(slot, index), Some(2.0));
    assert!(segment(&h, "Latency: 512 frames").checked, "the face shows the position it set");
}

/// **Preferences → Plugins lists the files that could not be read, and
/// Rescan All scans again and reads the result back** (MOO-229). The scan
/// itself is a stand-in here, which finishes at once having rewritten the
/// cache without the failure: the real one would launch children over this
/// machine's plugin folders (`plugin_scan`'s own test holds the one-scan
/// rule).
#[test]
fn the_plugins_page_shows_failures_and_rescan_all_reads_them_again() {
    use crate::plugin_scan::{ScanProgress, ScanState};
    let h = harness_with(&drum_loop());
    let settings = Rc::new(RefCell::new(crate::settings::UiSettings::default()));
    let asked = Rc::new(RefCell::new(0));
    let cache = h.state.borrow().plugin_cache_path.clone();
    let starter: plugin_ui::ScanStarter = {
        let (asked, cache) = (asked.clone(), cache.clone());
        Rc::new(move |_settings: &PluginSettings, progress: ScanProgress| {
            *asked.borrow_mut() += 1;
            let text = std::fs::read_to_string(&cache).expect("the cache");
            let kept = text.split("[[file]]\npath = \"/usr/lib/clap/broken.clap\"").next().unwrap();
            std::fs::write(&cache, kept).expect("the cache is rewritten");
            *progress.lock().unwrap() = ScanState::Done { plugins: 2, failed: 0 };
            true
        })
    };
    plugin_ui::wire_plugin_preferences(&h.window, &h.state, &settings, starter);
    plugin_ui::show_plugin_preferences(&h.window, &settings.borrow().plugins, &cache);

    let failures = h.window.get_preferences_plugin_failures();
    assert_eq!(failures.row_count(), 1);
    let broken = failures.row_data(0).expect("a row");
    assert_eq!(broken.name, "broken.clap");
    assert!(broken.reason.contains("signal 11"), "{}", broken.reason);
    assert_eq!(h.window.get_preferences_plugin_scan_timeout_s(), 10);
    assert!(h.window.get_preferences_plugin_default_paths().row_count() > 0);

    // The shortcut's way in and the page's button are the same callback.
    h.window.invoke_preferences_plugin_rescan_requested();
    assert_eq!(*asked.borrow(), 1);
    h.state.borrow_mut().poll_plugin_scan(&h.window);
    assert_eq!(h.window.get_preferences_plugin_scan_status(), "Plugin scan done: 2 plugins");
    assert!(!h.window.get_preferences_plugin_scanning());
    assert_eq!(h.window.get_preferences_plugin_failures().row_count(), 0, "read again after the scan");
    assert!(h.state.borrow().plugin_catalog.failures.is_empty());
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

// ---------------------------------------------------------------------------
// Lanes and routes on a plugin's parameters (MOO-228).

/// The test gain in a live chain, ticked until its face is up.
fn live_gain() -> Harness {
    let mut h = harness_with(&drum_loop());
    // The catalogue is read when the PLUGINS tab opens, as the app reads it.
    h.state.borrow_mut().enter_browser_tab(BrowserTab::Plugins);
    assert!(plugin_ui::add_plugin(
        &h.state,
        &h.commands,
        &h.window,
        test_plugin::GAIN_ID,
        None,
        &plugin_ui::Queues {
            tx: h.tx.clone(),
            stx: h.stx.clone(),
            reset_tx: mpsc::channel().0,
        },
    ));
    for _ in 0..3 {
        h.tick();
    }
    h
}

fn plugin_device(h: &Harness) -> mooloop_core::DeviceId {
    h.state.borrow().session.effect_chain().expect("a chain")[0].id
}

fn nudge_index(h: &Harness) -> usize {
    let slot = h.plugin_slot();
    h.state
        .borrow()
        .session
        .plugin_param_index(slot, test_plugin::PARAM_NUDGE)
        .expect("the gain lists Nudge")
}

/// A modulator in slot 0 and a route from it to `destination`, written into
/// the rack directly, as a song that carries one would: the shelf's own
/// gesture refuses a destination that takes no modulation.
fn add_route(h: &Harness, destination: ParamAddr) {
    let mut st = h.state.borrow_mut();
    if st.session.channel_rack(0).slots[0].is_none() {
        assert!(
            !st.session.add_modulation_source(mooloop_core::ModulatorKind::Lfo).is_empty(),
            "room for a modulator"
        );
    }
    st.session
        .edit_channel_rack(0, |rack| {
            rack.add_route(mooloop_core::ModRoute::to_slot(
                0,
                destination,
                0.5,
                mooloop_core::ModPolarity::Bipolar,
            ))
        })
        .expect("room for a route");
    st.refresh_modulation(&h.window);
}

/// **A plugin knob arms a route like a native knob.** With a modulator
/// armed, a wheel step on the face's Gain knob authors a route on the
/// plugin's own id, and the knob and the shelf both show it.
#[test]
fn a_plugin_knob_arms_a_route_and_shows_its_ring() {
    let mut h = live_gain();
    let device = plugin_device(&h);
    {
        let mut st = h.state.borrow_mut();
        assert!(
            !st.session.add_modulation_source(mooloop_core::ModulatorKind::Lfo).is_empty(),
            "room for a modulator"
        );
        st.session.set_modulation_armed_slot(Some(0));
        st.refresh_modulation(&h.window);
    }
    h.window.set_modulation_armed_slot(0);
    let gain = h.slider("Gain");
    wheel(&h.window, &gain);
    h.tick();

    let gain_address = ParamAddr::plugin_param(EffectTarget::Channel(0), device, test_plugin::PARAM_GAIN);
    let st = h.state.borrow();
    let routes: Vec<_> = st.session.channel_rack(0).destinations().collect();
    assert_eq!(routes, [gain_address], "the wheel authored one route, on the plugin's id");
    let row = st.modulation_route_model.row_data(0).expect("the shelf lists it");
    assert!(row.destination.contains("Test Gain 1 · Gain"), "{}", row.destination);
    assert!(row.allowed && !row.missing, "{row:?}");
    assert_eq!(row.param, 0, "the route row carries the dense index");
    let face = st.effect_slot_model.row_data(0).expect("the face's row");
    assert_eq!(face.modulation_route_counts.row_data(0), Some(1), "the knob counts its route");
    assert!(face.modulation_allowed.row_data(0) == Some(true));
}

/// **Index in, id out, for the parameter whose id is four billion.** The
/// face names Nudge by its dense index, and what Rust makes of it is the
/// plugin's `u32` id whole; a route on it lists under its name with the
/// index, never a wrapped id, in the row.
#[test]
fn nudge_is_named_by_index_and_addressed_by_its_whole_id() {
    let h = live_gain();
    let device = plugin_device(&h);
    let index = nudge_index(&h);
    let nudge = ParamAddr::plugin_param(EffectTarget::Channel(0), device, test_plugin::PARAM_NUDGE);

    // A naming press, as the knob's context menu makes one.
    h.window.global::<ControlRequest>().set_naming(true);
    h.window.invoke_plugin_modulation_edit_started(0, index as i32);
    h.window.global::<ControlRequest>().set_naming(false);
    assert_eq!(h.state.borrow().named_param, Some(nudge));

    // The lane picker offers it under the plugin's device, and opens it.
    let at = {
        let st = h.state.borrow();
        st.refresh_automation(&h.window);
        plugin_ui::lane_destinations(&st.session)
            .iter()
            .position(|row| row.address == nudge)
            .expect("the picker offers Nudge")
    };
    let rows = h.window.get_automation_targets();
    let row = rows.row_data(at).expect("a menu row");
    assert_eq!((row.device.as_str(), row.param_name.as_str()), ("Test Gain 1", "Nudge"));
    assert!(!row.missing);

    // A route on it -- not modulatable, so kept and inert -- lists by index.
    add_route(&h, nudge);
    let row = h.state.borrow().modulation_route_model.row_data(0).expect("a route row");
    assert_eq!(row.param, index as i32, "the dense index, not {}", test_plugin::PARAM_NUDGE as i32);
    assert!(row.destination.contains("Nudge") && !row.allowed && !row.missing, "{row:?}");
}

/// The picker's list is the session's native list with the plugin rows
/// placed in it, so every native row keeps its order, and a song with no
/// plugin gets exactly the list -- and so the lane at each index -- it
/// always did.
#[test]
fn the_picker_keeps_every_native_row_in_order() {
    let natives = |session: &Session| -> Vec<ParamAddr> {
        session
            .automation_destinations()
            .into_iter()
            .map(|(address, _, _)| address)
            .collect()
    };
    let h = harness_with(&drum_loop());
    {
        let st = h.state.borrow();
        let picker: Vec<ParamAddr> =
            plugin_ui::lane_destinations(&st.session).iter().map(|row| row.address).collect();
        assert_eq!(picker, natives(&st.session), "no plugin, no change");
    }
    let h = live_gain();
    let st = h.state.borrow();
    let picker = plugin_ui::lane_destinations(&st.session);
    let native_rows: Vec<ParamAddr> = picker
        .iter()
        .map(|row| row.address)
        .filter(|address| !matches!(address.owner, ParamOwner::PluginParam { .. }))
        .collect();
    assert_eq!(native_rows, natives(&st.session));
    let first_plugin = picker
        .iter()
        .position(|row| matches!(row.address.owner, ParamOwner::PluginParam { .. }))
        .expect("the gain's parameters are offered");
    assert_eq!(
        picker[first_plugin + 4].address.owner,
        ParamOwner::Strip,
        "the plugin's four parameters sit just before the strip"
    );
}

/// **A lane a device change left inert is in the picker as missing, right
/// after the source's own rows** (MOO-329, MOO-270's picker half). Three
/// lanes drawn on the sampler, the channel switched to the drum synth: the
/// three are kept and still take lane slots, so the picker lists them --
/// missing, in the pattern's order, after the drum synth's rows and before
/// anything else -- where the window can open and remove them. Switched
/// back, they are the sampler's live rows again and nothing is missing.
#[test]
fn a_lane_left_by_a_device_change_is_a_missing_row_after_the_source() {
    let is_source = |row: &plugin_ui::LaneDestination| {
        matches!(row.address.owner, ParamOwner::Source { .. } | ParamOwner::SourceRoute { .. })
    };
    let mut session = Session::default();
    session.change_selected_source(DeviceKind::Sampler);
    let sampler: Vec<ParamAddr> = DeviceKind::Sampler
        .descriptors()
        .iter()
        .take(3)
        .map(|descriptor| session.selected_source_address(descriptor.id).expect("a sampler parameter"))
        .collect();
    for &address in &sampler {
        session.open_automation_lane_at(address).expect("within the lane ceiling");
    }
    assert!(
        plugin_ui::lane_destinations(&session).iter().all(|row| !row.missing),
        "a lane on the device the channel runs is not missing"
    );

    session.change_selected_source(DeviceKind::DrumSynth);
    let picker = plugin_ui::lane_destinations(&session);
    let missing: Vec<usize> = (0..picker.len()).filter(|&i| picker[i].missing).collect();
    assert_eq!(
        missing.iter().map(|&i| picker[i].address).collect::<Vec<_>>(),
        sampler,
        "every inert lane is listed as missing, in the pattern's order"
    );
    let last_live_source = picker
        .iter()
        .rposition(|row| !row.missing && is_source(row))
        .expect("the drum synth's own rows are offered");
    assert_eq!(
        missing,
        (last_live_source + 1..last_live_source + 1 + sampler.len()).collect::<Vec<_>>(),
        "the inert lanes sit right after the drum synth's rows"
    );
    assert!(
        picker[..last_live_source].iter().all(|row| !row.missing && is_source(row)),
        "nothing but the drum synth's rows comes before them"
    );
    assert_eq!(picker[missing[0]].device, DeviceKind::Sampler.label());
    assert_eq!(picker[missing[0]].name, DeviceKind::Sampler.descriptors()[0].name);

    session.change_selected_source(DeviceKind::Sampler);
    let picker = plugin_ui::lane_destinations(&session);
    assert!(picker.iter().all(|row| !row.missing), "switched back, nothing is missing");
    for address in &sampler {
        assert_eq!(
            picker.iter().filter(|row| row.address == *address).count(),
            1,
            "each sampler lane is its live row again, once"
        );
    }
}

/// **A parameter the plugin stops listing is kept and drawn as missing, and
/// comes back when it returns** (Adam, MOO-74; plugin-hosting 08). A lane
/// and a route on Nudge; the plugin's list loses it, as a `params.rescan`
/// leaves it; both read as missing -- the menu row italic, the lane greyed,
/// the route row greyed -- and nothing is dropped. The list regains it and
/// they read normally, with no repair step.
#[test]
fn a_missing_plugin_parameter_is_drawn_missing_and_reunited() {
    let h = live_gain();
    let device = plugin_device(&h);
    let slot = h.plugin_slot();
    let nudge = ParamAddr::plugin_param(EffectTarget::Channel(0), device, test_plugin::PARAM_NUDGE);
    h.state
        .borrow_mut()
        .session
        .open_automation_lane_at(nudge)
        .expect("a lane on Nudge");
    add_route(&h, nudge);
    let listed = h.state.borrow().session.plugins[&slot].params.clone();

    let read = |h: &Harness| {
        let st = h.state.borrow();
        st.refresh_automation(&h.window);
        st.refresh_modulation(&h.window);
        let at = plugin_ui::lane_destinations(&st.session)
            .iter()
            .position(|row| row.address == nudge)
            .expect("the lane's destination is still offered");
        let menu = h.window.get_automation_targets().row_data(at).expect("its menu row");
        let route = st.modulation_route_model.row_data(0).expect("its route row");
        (menu, h.window.get_automation_lane_missing(), route)
    };

    // Gone from the list, as a rescan that drops it leaves it.
    h.state
        .borrow_mut()
        .session
        .plugins
        .get_mut(&slot)
        .expect("the slot")
        .params
        .retain(|info| info.id != test_plugin::PARAM_NUDGE);
    let (menu, lane_missing, route) = read(&h);
    assert!(menu.missing, "the menu row reads as missing: {menu:?}");
    assert_eq!(menu.param_name.as_str(), "Parameter 4000000000", "named by its id");
    assert!(menu.current && lane_missing, "the lane shown is drawn missing");
    assert!(route.missing && !route.allowed, "the route row reads as missing: {route:?}");
    assert_eq!(route.param, -1, "no index names a parameter the plugin does not list");
    {
        let st = h.state.borrow();
        assert_eq!(st.session.automation_lanes().map(Vec::len), Some(1), "the lane is kept");
        assert!(st.session.channel_rack(0).routes[0].is_some(), "the route is kept");
    }

    // Back in the list: the same rows, normal again.
    h.state.borrow_mut().session.plugins.get_mut(&slot).expect("the slot").params = listed;
    let (menu, lane_missing, route) = read(&h);
    assert!(!menu.missing && menu.param_name.as_str() == "Nudge", "{menu:?}");
    assert!(!lane_missing);
    assert!(!route.missing && route.destination.contains("Nudge"), "{route:?}");
}

/// **A container preset holding a plugin lets that plugin's processor in**
/// (MOO-322). A Chain holding the test gain is saved as a preset and loaded
/// back over the Chain, through `load_effect_run` and the window's mirror of
/// it, `install_loaded_run`. The pump opens the gain in the slot the load
/// minted and sends its processor down as a `ReplaceEffect` expecting that
/// slot's key; the engine lets a replacement in only where the slot was
/// installed with `Some` of that key (`EffectChain::replace_if_kind`). So
/// the placeholder the mirror installed has to carry it, or the device is a
/// silent pass-through for good, with no problem reported anywhere.
#[test]
fn a_container_preset_holding_a_plugin_lets_its_processor_in() {
    let mut h = harness_with(&drum_loop());
    h.state.borrow_mut().enter_browser_tab(BrowserTab::Plugins);
    let queues = plugin_ui::Queues {
        tx: h.tx.clone(),
        stx: h.stx.clone(),
        reset_tx: mpsc::channel().0,
    };
    assert!(plugin_ui::add_plugin(&h.state, &h.commands, &h.window, test_plugin::GAIN_ID, Some(0), &queues));
    for _ in 0..3 {
        h.tick();
    }
    let original = h.plugin_slot();

    // The gain boxed in a Chain, and the Chain saved as a preset.
    let path = h.dir.path().join("boxed.mooloop-effect");
    {
        let mut st = h.state.borrow_mut();
        st.session.wrap_effects_in_container(0..1).expect("wrapped");
        let device = st.session.effect_chain().expect("a chain")[0].id;
        st.session.pending_preset_save = st
            .session
            .chain_key(st.session.effect_target)
            .map(|target| PresetSaveTarget::Effect { target, device });
        let source = st.session.take_preset_save(120, 0).expect("a save was pending");
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
    }
    let LoadedDocument::EffectRun(run) = mooloop_project::load_bundle(&path).expect("it loads").document else {
        panic!("not a container preset");
    };

    // Loaded back over the Chain, as the rail's preset menu does.
    h.engine.drain();
    let (installs, replaces) = (h.engine.install_keys.len(), h.engine.replace_keys.len());
    {
        let mut st = h.state.borrow_mut();
        let loaded = st.session.load_effect_run(0, &run, "Boxed").expect("the preset fits the Chain");
        st.install_loaded_run(&loaded, 120.0, RATE, &h.tx, &h.stx);
        st.sync_effects();
    }
    let landed = h
        .state
        .borrow()
        .session
        .effect_chain()
        .expect("a chain")
        .iter()
        .find_map(|effect| match effect.params {
            EffectParams::Plugin(slot) => Some(slot),
            _ => None,
        })
        .expect("the preset's gain is in the chain");
    assert_ne!(landed, original, "test premise: the load mints a slot of its own");
    for _ in 0..3 {
        h.tick();
    }
    assert!(h.state.borrow().session.plugin_problem(landed).is_none(), "the gain opened");

    let key = u64::from(landed.0);
    let replaced = &h.engine.replace_keys[replaces..];
    assert!(
        replaced.contains(&key),
        "the rack sent the gain's processor, keyed by its slot: {replaced:?}"
    );
    let installed = &h.engine.install_keys[installs..];
    assert!(
        installed.contains(&Some(key)),
        "the placeholder the load installed carries the slot's key, so the engine lets the \
         processor in rather than refusing it on every retry: installed {installed:?}, key {key}"
    );
}

/// **A container preset holding a plugin, added as a new device, lets the
/// plugin's processor in too** (MOO-466). The add mirrors the run one slot
/// at a time instead of installing a project, and the placeholder it
/// installs for the plugin row carries the slot's key, as the load over a
/// Chain above does.
#[test]
fn a_container_preset_holding_a_plugin_added_as_a_new_device_lets_its_processor_in() {
    let mut h = harness_with(&drum_loop());
    h.state.borrow_mut().enter_browser_tab(BrowserTab::Plugins);
    let queues = plugin_ui::Queues {
        tx: h.tx.clone(),
        stx: h.stx.clone(),
        reset_tx: mpsc::channel().0,
    };
    assert!(plugin_ui::add_plugin(&h.state, &h.commands, &h.window, test_plugin::GAIN_ID, Some(0), &queues));
    for _ in 0..3 {
        h.tick();
    }
    let original = h.plugin_slot();

    let path = h.dir.path().join("boxed.mooloop-effect");
    {
        let mut st = h.state.borrow_mut();
        st.session.wrap_effects_in_container(0..1).expect("wrapped");
        let device = st.session.effect_chain().expect("a chain")[0].id;
        st.session.pending_preset_save = st
            .session
            .chain_key(st.session.effect_target)
            .map(|target| PresetSaveTarget::Effect { target, device });
        let source = st.session.take_preset_save(120, 0).expect("a save was pending");
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
    }

    h.engine.drain();
    let (installs, replaces) = (h.engine.install_keys.len(), h.engine.replace_keys.len());
    let rows = h.state.borrow().session.effect_chain().expect("a chain").len();
    assert!(crate::append_effect_preset(
        &h.state,
        &h.window,
        &h.commands,
        (&h.tx, &h.stx),
        &path,
        EffectKind::Chain,
        "Boxed",
    ));
    let landed = h.state.borrow().session.effect_chain().expect("a chain")[rows..]
        .iter()
        .find_map(|effect| match effect.params {
            EffectParams::Plugin(slot) => Some(slot),
            _ => None,
        })
        .expect("the preset's gain is in the added run");
    assert_ne!(landed, original, "test premise: the add mints a slot of its own");
    for _ in 0..3 {
        h.tick();
    }
    assert!(h.state.borrow().session.plugin_problem(landed).is_none(), "the gain opened");

    let key = u64::from(landed.0);
    let replaced = &h.engine.replace_keys[replaces..];
    assert!(replaced.contains(&key), "the rack sent the gain's processor: {replaced:?}");
    let installed = &h.engine.install_keys[installs..];
    assert!(
        installed.contains(&Some(key)),
        "the placeholder the add installed carries the slot's key: installed {installed:?}, key {key}"
    );
}

// ---------------------------------------------------------------------------
// A plugin's own GUI in a window of its own (step 11, MOO-302). No test here
// opens a window: the window side is `plugin_gui::fake`, which records what
// it was asked, and the plugin is the test double's GUI variant, which draws
// nothing.
//
// Every test here runs on Linux and macOS alike: the host asks the plugin
// for the platform's API (X11 or Cocoa), which the test double offers, and
// the fake stands in for the X11 window or the `NSPanel`. On macOS a fake's
// window id names no panel, so the plugin is parented to a null view, which
// the test double, drawing nothing, never reads.

/// The scanner's cache with the test gain's and the test sine's GUI variants
/// beside the rest.
fn gui_cache_text(library: &Path) -> String {
    let path = library.display().to_string().replace('\\', "/");
    let broken = "[[file]]\npath = \"/usr/lib/clap/broken.clap\"";
    let gui = format!(
        "[[file.plugin]]\npath = \"{path}\"\nformat = \"clap\"\nid = \"{id}\"\nname = \"Test Gain (GUI)\"\n\
         vendor = \"{vendor}\"\nfeatures = [\"audio-effect\"]\naudio-inputs = [2]\naudio-outputs = [2]\n\n\
         [[file.plugin]]\npath = \"{path}\"\nformat = \"clap\"\nid = \"{sine}\"\nname = \"Test Sine (GUI)\"\n\
         vendor = \"{vendor}\"\nfeatures = [\"instrument\"]\naudio-outputs = [2]\nnote-inputs = 1\n\n{broken}",
        id = test_plugin::GAIN_GUI_ID,
        sine = test_plugin::SINE_GUI_ID,
        vendor = test_plugin::VENDOR,
    );
    cache_text(library).replacen(broken, &gui, 1)
}

type FakeLog = Rc<RefCell<crate::plugin_gui::fake::Log>>;

/// The test gain and its GUI variant in a live chain, in that order, the
/// window side faked; ticked until both faces are up.
fn gui_harness() -> (Harness, FakeLog) {
    let mut h = harness_with(&drum_loop());
    let cache = h.state.borrow().plugin_cache_path.clone();
    std::fs::write(&cache, gui_cache_text(&test_plugin_path())).expect("the cache is written");
    let (guis, log) = crate::plugin_gui::fake::fake();
    {
        let mut st = h.state.borrow_mut();
        st.session
            .set_plugin_opener(mooloop_session::plugin_rack::clap_opener(cache));
        st.plugin_guis = guis;
        st.enter_browser_tab(BrowserTab::Plugins);
    }
    let queues = plugin_ui::Queues {
        tx: h.tx.clone(),
        stx: h.stx.clone(),
        reset_tx: mpsc::channel().0,
    };
    for (id, at) in [(test_plugin::GAIN_ID, 0), (test_plugin::GAIN_GUI_ID, 1)] {
        assert!(plugin_ui::add_plugin(&h.state, &h.commands, &h.window, id, Some(at), &queues), "{id} added");
    }
    for _ in 0..3 {
        h.tick();
    }
    (h, log)
}

/// The open-window controls the rack draws.
fn window_buttons(h: &Harness) -> Vec<Control> {
    controls(&h.window, AccessibleRole::Button)
        .into_iter()
        .filter(|button| button.label.ends_with("plugin's window") || button.label.ends_with("window to the front"))
        .collect()
}

fn gui_slot(h: &Harness) -> PluginSlotId {
    match h.state.borrow().session.effect_chain().expect("a chain")[1].params {
        EffectParams::Plugin(slot) => slot,
        _ => panic!("the second device is not a plugin"),
    }
}

fn calls(log: &FakeLog) -> Vec<String> {
    log.borrow().calls.clone()
}

fn open_gui(h: &Harness) {
    let main = crate::plugin_gui::MainWindowState::of(h.window.window());
    h.state.borrow_mut().open_plugin_gui_at(1, &main).expect("it opens");
}

/// **The face draws the open-window control only for a plugin with a GUI
/// of its own**, and pressing it opens the GUI in a window of mooloop's,
/// sized to the plugin and shown; pressed again, it brings that window to
/// the front rather than opening a second.
#[test]
fn only_a_plugin_with_a_gui_has_the_control_and_it_opens_or_raises() {
    let (mut h, log) = gui_harness();
    let rows: Vec<EffectSlotRow> = h.state.borrow().effect_slot_model.iter().collect();
    assert!(rows[0].is_plugin && !rows[0].plugin_has_gui, "the plain gain has no GUI");
    assert!(rows[1].is_plugin && rows[1].plugin_has_gui, "its GUI variant has one");
    let buttons = window_buttons(&h);
    assert_eq!(buttons.len(), 1, "one control, on the face with a GUI: {buttons:?}");
    assert!(calls(&log).is_empty(), "nothing is opened until asked");

    click(&h.window, buttons[0].centre);
    let slot = gui_slot(&h);
    assert!(h.state.borrow_mut().session.plugin_gui_is_open(slot), "the plugin's GUI is open");
    let made = calls(&log);
    assert!(made[0].starts_with("create ") && made[0].contains("Test Gain (GUI)"), "{made:?}");
    assert_eq!(made.last().map(String::as_str), Some("show 0x101"), "shown once placed: {made:?}");
    assert!(!made.iter().any(|call| call.starts_with("transient")), "no native main window to belong to");
    h.tick();
    assert!(h.state.borrow().effect_slot_model.row_data(1).expect("the row").plugin_gui_open);
    assert!(window_buttons(&h)[0].label.ends_with("window to the front"));

    // Again: the same window, unmapped and mapped, which puts it on top.
    let before = calls(&log).len();
    click(&h.window, window_buttons(&h)[0].centre);
    assert_eq!(calls(&log)[before..], ["hide 0x101".to_string(), "show 0x101".to_string()]);
    assert_eq!(
        calls(&log).iter().filter(|call| call.starts_with("create ")).count(),
        1,
        "no second window"
    );
}

/// **Teardown, in the order that matters: the plugin's GUI, then its
/// window.** The window's close button closes the GUI through the session
/// and only then destroys the window; the processor keeps playing. A device
/// removed with its GUI open has the GUI closed by the session, and its
/// window destroyed on the tick that reports it. Quit does the same for all.
#[test]
fn a_window_goes_only_after_its_plugin_gui() {
    use mooloop_plugin_window::{GuiSize, PluginWindowEvent, PluginWindowId};
    let destroyed = |log: &FakeLog| calls(log).iter().filter(|call| call.starts_with("destroy ")).count();
    let (mut h, log) = gui_harness();
    let slot = gui_slot(&h);

    // The close button.
    open_gui(&h);
    log.borrow_mut()
        .events
        .push((PluginWindowId(0x101), PluginWindowEvent::CloseRequested));
    h.tick();
    assert!(!h.state.borrow_mut().session.plugin_gui_is_open(slot), "the GUI is closed");
    assert_eq!(calls(&log).last().map(String::as_str), Some("destroy 0x101"));
    assert!(h.state.borrow().session.plugin_rack.instance(slot).is_some(), "the plugin still plays");
    assert!(!h.state.borrow().effect_slot_model.row_data(1).expect("the row").plugin_gui_open);

    // Reopened; a resize from outside reaches the plugin and leaves it open.
    open_gui(&h);
    let resized = GuiSize { width: 333, height: 222 };
    log.borrow_mut()
        .events
        .push((PluginWindowId(0x102), PluginWindowEvent::Resized(resized)));
    h.tick();
    assert!(h.state.borrow().plugin_guis.is_open(slot));

    // The device removed while its GUI is open: nothing is destroyed until
    // the session has closed the GUI, which it does on the next tick.
    assert_eq!(destroyed(&log), 1);
    h.state.borrow_mut().session.remove_effect_at(1).expect("removed");
    assert_eq!(destroyed(&log), 1, "the window outlives the GUI, never the reverse");
    h.tick();
    assert_eq!(destroyed(&log), 2, "gone on the tick the session closed the GUI");
    assert!(!h.state.borrow().plugin_guis.any_open());

    // Quit with a GUI open.
    let (h, log) = gui_harness();
    let slot = gui_slot(&h);
    open_gui(&h);
    {
        let mut st = h.state.borrow_mut();
        let mut sink = plugin_ui::QueuedSink {
            tx: &h.tx,
            stx: &h.stx,
            sample_rate: RATE,
        };
        st.session.close_plugins(&mut sink);
        assert_eq!(destroyed(&log), 0, "the GUIs go first");
        st.close_plugin_windows();
        assert!(!st.session.plugin_gui_is_open(slot));
        assert!(!st.plugin_guis.any_open());
    }
    assert_eq!(calls(&log).last().map(String::as_str), Some("destroy 0x101"));
}

/// **Where the main window can hold a plugin window above it, the pump
/// makes it transient and never hides it for focus**: X11 and macOS
/// (`DisplayBackend::can_set_transient`). On macOS the panel floats above
/// mooloop and AppKit hides it with the application, so the pump's
/// hide-on-focus-loss, native Wayland's workaround, must not run there; a
/// native Wayland session, which cannot, is the contrast.
#[test]
fn a_transient_capable_main_window_keeps_its_plugin_window_above_it() {
    use crate::plugin_gui::fake::main_on;
    use crate::plugin_gui::FOCUS_GRACE;
    use mooloop_plugin_window::DisplayBackend;
    for backend in [DisplayBackend::Cocoa, DisplayBackend::X11, DisplayBackend::Wayland] {
        let (h, log) = gui_harness();
        let start = std::time::Instant::now();
        let opened = main_on(Some(backend), true, start);
        h.state.borrow_mut().open_plugin_gui_at(1, &opened).expect("it opens");
        let made = calls(&log);
        let transient = made.iter().find(|call| call.starts_with("transient "));
        if backend.can_set_transient() {
            let parent = opened.native_parent.expect("a main window to belong to");
            assert_eq!(
                transient.map(String::as_str),
                Some(format!("transient 0x101 Some({})", parent.id).as_str()),
                "{backend:?}: {made:?}"
            );
            let at = |position: &str| made.iter().position(|call| call == position);
            assert!(at("transient 0x101") < at("show 0x101"), "placed before it is shown: {made:?}");
        } else {
            assert_eq!(transient, None, "{backend:?}: nothing to belong to");
        }

        // Focus is away from mooloop for far longer than the grace period.
        let before = calls(&log).len();
        for after in [FOCUS_GRACE * 2, FOCUS_GRACE * 20] {
            h.state.borrow_mut().pump_plugin_guis(&main_on(Some(backend), false, start + after));
        }
        let hidden = calls(&log)[before..].contains(&"hide 0x101".to_string());
        assert_eq!(hidden, backend == DisplayBackend::Wayland, "{backend:?}: {:?}", &calls(&log)[before..]);
    }
}

/// **A window that cannot open is the face's badge, and the face stays.**
/// The plugin accepts the platform's GUI API (X11 on Linux, Cocoa on macOS),
/// so the failure is the window side's: its fake has no display, and the
/// reason is its own on every platform.
#[test]
fn a_window_that_cannot_open_is_the_badge() {
    let reason = "DISPLAY";
    let (mut h, _log) = gui_harness();
    h.state.borrow_mut().plugin_guis =
        crate::plugin_gui::PluginGuis::new(Box::new(|| Err(mooloop_plugin_window::WindowError::NoDisplay)));
    click(&h.window, window_buttons(&h)[0].centre);
    h.tick();
    let row = h.state.borrow().effect_slot_model.row_data(1).expect("the row");
    assert!(row.is_plugin && row.plugin_has_gui, "the face stays, control and all");
    assert!(!row.plugin_gui_open);
    assert!(row.plugin_status.contains("window did not open"), "{}", row.plugin_status);
    assert!(row.plugin_status.contains(reason), "the reason is said: {}", row.plugin_status);
    assert_eq!(window_buttons(&h).len(), 1, "and it can be pressed again");
}

/// **Preferences → Plugins' "Run under XWayland" round-trips to the saved
/// setting**, off by default, and a save that fails puts it back. The
/// section is shown only where the setting applies: not on macOS. Saved to a
/// scratch file: a test must not write the settings of whoever runs it.
#[test]
fn the_xwayland_toggle_round_trips_to_the_setting() {
    use crate::settings::{SettingsError, UiSettings};
    let h = harness_with(&drum_loop());
    let settings = Rc::new(RefCell::new(UiSettings::default()));
    let file = h.dir.path().join("settings.toml");
    let saver: plugin_ui::SettingsSaver = {
        let file = file.clone();
        Rc::new(move |settings: &UiSettings| settings.save_to(&file))
    };
    plugin_ui::wire_xwayland_toggle(&h.window, &settings, saver);
    let cache = h.state.borrow().plugin_cache_path.clone();
    plugin_ui::show_plugin_preferences(&h.window, &settings.borrow().plugins, &cache);
    assert!(!h.window.get_preferences_plugin_run_under_xwayland(), "off by default");
    assert_eq!(
        h.window.get_preferences_plugin_xwayland_applies(),
        !cfg!(target_os = "macos"),
        "shown on Linux, hidden on macOS, where the setting changes nothing"
    );

    h.window.invoke_preferences_plugin_run_under_xwayland_toggled(true);
    assert!(settings.borrow().plugins.run_under_xwayland);
    assert!(h.window.get_preferences_plugin_run_under_xwayland());
    assert!(
        UiSettings::load_or_default_from(&file).plugins.run_under_xwayland,
        "saved, for the next start to read"
    );

    h.window.invoke_preferences_plugin_run_under_xwayland_toggled(false);
    assert!(!UiSettings::load_or_default_from(&file).plugins.run_under_xwayland);
    assert!(!h.window.get_preferences_plugin_run_under_xwayland());

    // A save that fails keeps what was there, and says so.
    let failing: plugin_ui::SettingsSaver =
        Rc::new(|_: &UiSettings| Err(SettingsError::Io(std::io::Error::other("read-only"))));
    plugin_ui::wire_xwayland_toggle(&h.window, &settings, failing);
    h.window.invoke_preferences_plugin_run_under_xwayland_toggled(true);
    assert!(!settings.borrow().plugins.run_under_xwayland);
    assert!(!h.window.get_preferences_plugin_run_under_xwayland());
    assert!(h.window.get_preferences_error().contains("read-only"));
}

// ---------------------------------------------------------------------------
// A plugin instrument's face (MOO-304, MOO-316): the face a plugin on a
// chain has, in the source's place at the head of the chain.

/// A new channel whose source is the instrument `id`, from the same cache as
/// [`gui_harness`], the window side faked; ticked until its face is up.
fn instrument_harness(id: &str) -> (Harness, FakeLog) {
    let mut h = harness_with(&drum_loop());
    let cache = h.state.borrow().plugin_cache_path.clone();
    std::fs::write(&cache, gui_cache_text(&test_plugin_path())).expect("the cache is written");
    let (guis, log) = crate::plugin_gui::fake::fake();
    {
        let mut st = h.state.borrow_mut();
        st.session
            .set_plugin_opener(mooloop_session::plugin_rack::clap_opener(cache));
        st.plugin_guis = guis;
        st.enter_browser_tab(BrowserTab::Plugins);
    }
    let queues = plugin_ui::Queues {
        tx: h.tx.clone(),
        stx: h.stx.clone(),
        reset_tx: mpsc::channel().0,
    };
    assert!(plugin_ui::add_plugin(&h.state, &h.commands, &h.window, id, None, &queues), "{id} added");
    for _ in 0..3 {
        h.tick();
    }
    (h, log)
}

/// The selected channel's instrument: its source slot's device id and the
/// plugin's slot.
fn instrument(h: &Harness) -> (mooloop_core::DeviceId, PluginSlotId) {
    h.state.borrow().session.plugin_source().expect("the selected channel's source is a plugin")
}

/// **A plugin instrument's channel shows the open-window control when its
/// plugin has a GUI, and it opens and raises the same window a plugin
/// effect's does** (MOO-304), through `PluginGuis::open_or_raise` with the
/// source's slot.
#[test]
fn a_plugin_instrument_with_a_gui_opens_it_from_its_face() {
    let (mut h, log) = instrument_harness(test_plugin::SINE_GUI_ID);
    let (_, slot) = instrument(&h);
    assert_eq!(h.window.get_source_kind(), 8, "the rack shows the plugin channel's source");
    let face = h.window.get_source_plugin_face().row_data(0).expect("the instrument has a face");
    assert_eq!(face.name, "Test Sine (GUI)");
    assert!(face.has_gui && !face.gui_open, "{face:?}");
    let buttons = window_buttons(&h);
    assert_eq!(buttons.len(), 1, "one control, on the instrument's face: {buttons:?}");
    assert!(calls(&log).is_empty(), "nothing is opened until asked");

    click(&h.window, buttons[0].centre);
    assert!(h.state.borrow_mut().session.plugin_gui_is_open(slot), "the instrument's GUI is open");
    let made = calls(&log);
    assert!(made[0].starts_with("create ") && made[0].contains("Test Sine (GUI)"), "{made:?}");
    assert_eq!(made.last().map(String::as_str), Some("show 0x101"), "shown once placed: {made:?}");
    h.tick();
    assert!(h.window.get_source_plugin_face().row_data(0).expect("the face").gui_open);
    assert!(window_buttons(&h)[0].label.ends_with("window to the front"));

    // Again: the same window, brought to the front.
    let before = calls(&log).len();
    click(&h.window, window_buttons(&h)[0].centre);
    assert_eq!(calls(&log)[before..], ["hide 0x101".to_string(), "show 0x101".to_string()]);
    assert_eq!(calls(&log).iter().filter(|call| call.starts_with("create ")).count(), 1, "no second window");
}

/// **The test sine's face: its parameter as a knob, the searchable list, and
/// its knob's presses naming `PluginParam { device: source_device }`**
/// (MOO-316). No GUI, so no open-window control. Turning the knob is
/// `Session::set_plugin_source_param`; MIDI learn, the naming press the
/// control menu makes, and the lane picker all address the instrument's own
/// parameter, by the plugin's id.
#[test]
fn the_sine_source_shows_its_parameters_and_its_knob_names_the_instrument() {
    let (mut h, _log) = instrument_harness(test_plugin::SINE_ID);
    let (device, slot) = instrument(&h);
    let channel = h.state.borrow().session.selected;
    let level = ParamAddr::plugin_param(EffectTarget::Channel(channel as u8), device, test_plugin::PARAM_LEVEL);
    let index = h
        .state
        .borrow()
        .session
        .plugin_param_index(slot, test_plugin::PARAM_LEVEL)
        .expect("the sine lists Level");
    assert!(window_buttons(&h).is_empty(), "a plugin without a GUI has its face and nothing else");
    let knob = h.slider("Level");
    assert!(!knob.value.is_empty(), "the plugin writes its own readout");

    // The knob: the instrument's own verb, its id and plain value.
    wheel(&h.window, &knob);
    h.tick();
    assert!(
        h.engine.sent.iter().any(|command| matches!(
            command,
            EngineCommand::SetChannelGeneratorParam { id, .. } if *id == test_plugin::PARAM_LEVEL
        )),
        "the knob sent {:?}",
        h.engine.sent
    );

    // The PARAMETERS list, for the selected source.
    h.window.set_channel_sidebar_visible(true);
    h.state.borrow_mut().session.select_source(true);
    h.tick();
    assert_eq!(h.window.get_plugin_param_total(), 1);
    let row = h.window.get_plugin_param_rows().row_data(0).expect("a row");
    assert_eq!((row.index, row.name.as_str(), row.pinned), (index as i32, "Level", true));
    h.window.invoke_plugin_param_filter_edited("lev".into());
    assert_eq!(h.window.get_plugin_param_rows().row_count(), 1);
    h.window.invoke_plugin_param_filter_edited("cutoff".into());
    assert_eq!(h.window.get_plugin_param_rows().row_count(), 0, "the filter finds nothing else");

    // A naming press, as the knob's context menu makes one: the address a
    // route or a lane takes.
    h.window.global::<ControlRequest>().set_naming(true);
    h.window.invoke_source_plugin_modulation_edit_started(index as i32);
    h.window.global::<ControlRequest>().set_naming(false);
    assert_eq!(h.state.borrow().named_param, Some(level));

    // MIDI learn, armed, then a press on the knob.
    h.state.borrow_mut().midi_learn_armed = true;
    h.window.invoke_source_plugin_modulation_edit_started(index as i32);
    let key = h.state.borrow().session.param_key(level).expect("a durable key");
    let learning = h.state.borrow().session.control_learn.clone().expect("the press started a learn");
    assert_eq!(learning.target, mooloop_core::ControlTarget::Param(key));

    // The lane picker offers it under the instrument's name, before the
    // strip, as the source comes before everything after it.
    let st = h.state.borrow();
    let picker = plugin_ui::lane_destinations(&st.session);
    let at = picker.iter().position(|row| row.address == level).expect("the picker offers Level");
    assert_eq!((picker[at].device.as_str(), picker[at].name.as_str()), ("Test Sine", "Level"));
    let strip = picker
        .iter()
        .position(|row| row.address.owner == ParamOwner::Strip)
        .expect("the strip's rows");
    assert!(at < strip, "the instrument's parameters come before the strip");
}

/// A song whose first channel's source is an instrument this machine does
/// not have, with the list of `count` parameters the song remembers of it:
/// [`big_missing_plugin`]'s list, on a channel's source.
fn big_missing_instrument(count: u32) -> Project {
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
        id: "com.example.big-synth".into(),
        name: "Big Synth".into(),
        vendor: "Nobody".into(),
        version: String::new(),
    };
    let slot = session.set_plugin_source(0, gone, &mut sink).expect("the source is set");
    session.plugins.get_mut(&slot).expect("its slot").params = (0..count)
        .map(|index| mooloop_core::PluginParamInfo {
            id: 1_000 + index,
            name: if index == 14 { "Frequency".into() } else { format!("P{index}") },
            module: if (12..16).contains(&index) { "Filter".into() } else { String::new() },
            min: 0.0,
            max: 1.0,
            default: 0.5,
            stepped: None,
            automatable: true,
            modulatable: true,
            hidden: false,
        })
        .collect();
    session.project_snapshot(120, 0)
}

/// **An instrument's face shows eight, and the sidebar finds and pins the
/// rest, as one "Pin Parameter" step that survives a reload** (MOO-316):
/// MOO-229's case on a channel's source.
#[test]
fn an_instruments_pin_is_one_step_and_survives_a_reload() {
    let mut h = harness_with(&big_missing_instrument(40));
    h.window.set_channel_sidebar_visible(true);
    h.tick();
    assert_eq!(face_labels(&h), ["P0", "P1", "P2", "P3", "P4", "P5", "P6", "P7"]);
    let badge = controls(&h.window, AccessibleRole::Text)
        .into_iter()
        .find(|text| text.label.starts_with("Missing"))
        .expect("the instrument's face says it is missing");
    assert!(badge.label.contains("Big Synth"), "{}", badge.label);
    h.state.borrow().refresh_plugin_param_list(&h.window);
    assert_eq!(h.window.get_plugin_param_total(), 0, "no list until the source is selected");

    h.state.borrow_mut().session.select_source(true);
    h.tick();
    assert_eq!(h.window.get_plugin_param_total(), 40);
    h.window.invoke_plugin_param_filter_edited("freq".into());
    let pin = controls(&h.window, AccessibleRole::Checkbox)
        .into_iter()
        .find(|control| control.label == "Frequency")
        .expect("the sidebar draws Frequency's pin");
    assert!(!pin.checked);
    click(&h.window, pin.centre);
    h.tick();
    assert_eq!(labels(&h.commands.borrow()), ["Pin Parameter"], "a pin is one undo step");
    assert!(face_labels(&h).contains(&"Frequency".to_string()), "{:?}", face_labels(&h));
    let (_, slot) = instrument(&h);
    let pinned = h.state.borrow().session.plugins[&slot].pinned.clone();
    assert_eq!(pinned, [1_000, 1_001, 1_002, 1_003, 1_004, 1_005, 1_006, 1_007, 1_014]);
    assert_eq!(h.window.get_source_plugin_face().row_data(0).expect("the face").units, 2);

    // Saved as the window saves, and reopened: the pin is the song's.
    let song = project_snapshot(&h.state.borrow(), &h.window).project;
    let path = h.dir.path().join("instrument-pins.mooloop");
    mooloop_project::save_song(&path, &song, mooloop_project::AssetMode::Referenced).expect("it saves");
    let report = mooloop_project::load_bundle(&path).expect("it reopens");
    let mooloop_project::LoadedDocument::Song(reopened) = report.document else {
        panic!("a song came back as something else");
    };
    let mut again = harness_with(&reopened);
    again.tick();
    let (_, slot) = instrument(&again);
    assert_eq!(again.state.borrow().session.plugins[&slot].pinned, pinned);
    assert!(face_labels(&again).contains(&"Frequency".to_string()), "{:?}", face_labels(&again));
}

// ---------------------------------------------------------------------------
// A MIDI control on a plugin parameter (MOO-318).

fn desk_cc(controller: u8, value: u8) -> mooloop_core::MidiMessage {
    mooloop_core::MidiMessage {
        offset: 0,
        port: mooloop_core::MidiPortId(0),
        channel: 0,
        kind: mooloop_core::MidiKind::ControlChange { controller, value },
    }
}

/// **A CC moving a plugin parameter is one undo step** (MOO-318). The CC is
/// learned onto the test gain's Gain and swept; the plugin hears it through
/// the engine, and the pump runs until the plugin has gone quiet and the
/// controller idle. Undo takes the sweep back in one press: the step under
/// it is the learn, not an empty "Controller move".
#[test]
fn a_controller_on_a_plugin_parameter_is_one_undo_step() {
    let mut h = live_gain();
    let device = plugin_device(&h);
    let gain = ParamAddr::plugin_param(EffectTarget::Channel(0), device, test_plugin::PARAM_GAIN);
    h.state.borrow_mut().midi_ports = vec![mooloop_core::MidiPortInfo {
        id: mooloop_core::MidiPortId(0),
        name: "Desk".to_owned(),
    }];
    let key = h.state.borrow().session.param_key(gain).expect("a durable key");
    h.state
        .borrow_mut()
        .session
        .begin_control_learn(mooloop_core::ControlTarget::Param(key), false);
    let drain = |h: &mut Harness, value: u8| {
        let drained = drain_control_surface(
            &h.state,
            &h.commands,
            &h.window,
            &mut vec![desk_cc(21, value)],
            &mut Vec::new(),
            false,
        );
        for command in drained.commands {
            let _ = h.tx.send(command);
        }
        h.tick();
        settle_edit_streams(&h.state, &h.commands, &h.window, false, false);
    };
    drain(&mut h, 64);
    assert_eq!(labels(&h.commands.borrow()), ["MIDI learn"]);

    // The sweep, which a pickup catches on its way.
    let slot = h.plugin_slot();
    let index = h.state.borrow().session.plugin_param_index(slot, test_plugin::PARAM_GAIN).expect("listed");
    let start = h.state.borrow().session.plugin_param_value(slot, index).expect("live");
    for value in [0, 127, 0, 100] {
        drain(&mut h, value);
    }
    let end = h.state.borrow().session.plugin_param_value(slot, index).expect("live");
    assert_ne!(start, end, "the controller moved the gain");

    // The pump, until the plugin is quiet and the controller idle.
    let quiet = mooloop_session::plugin_rack::EDIT_QUIET_TICKS as usize + 2;
    for _ in 0..quiet {
        h.tick();
        settle_edit_streams(&h.state, &h.commands, &h.window, false, false);
    }
    settle_edit_streams(&h.state, &h.commands, &h.window, false, true);
    assert_eq!(labels(&h.commands.borrow()), ["Plugin Edit"], "the plugin's state is the step");
    h.commands.borrow_mut().history.commit_undo();
    assert_eq!(
        labels(&h.commands.borrow()),
        ["MIDI learn"],
        "one undo takes the whole sweep back; the step under it is the learn"
    );
}
