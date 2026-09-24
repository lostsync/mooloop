//! Hosted plugins in the window: the browser's PLUGINS tab, the insert
//! menu's "Plugin…" row, and the face of a plugin with no GUI of its own
//! (`docs/plans/plugin-hosting/08-the-plugin-face.md`, MOO-83).
//!
//! This is the step that puts a plugin in a chain from the window. The
//! session already does the work -- `Session::insert_plugin_effect` opens
//! the plugin and installs its processor, `Session::set_plugin_param` edits
//! one of its parameters by dense index -- so what is here is what a user
//! meets: a list of what the scanner found, filtered like the presets, and a
//! grid of the plugin's own parameters under its own names.
//!
//! **Only the neutral types cross into this crate.** The scanner's cache
//! (`PluginCache`, `ScannedPlugin`) and the host's `HostError` are what the
//! window reads; no plugin format's type does (`plugin_formats_stay_out` in
//! this crate's tests holds it, beside core's own guard).
//!
//! **An id never crosses into Slint.** A parameter goes to the face as its
//! index in the plugin's list and comes back as that index; only
//! `Session::set_plugin_param` turns it into the plugin's id, which may be
//! any `u32` and so cannot ride a Slint `int` (`session/src/plugin_params.rs`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use mooloop_core::{EffectParams, EffectSlotState, EngineCommand, MusicalEdge, PluginSlotId};
use mooloop_engine::{CommandSink, StructuralCommand};
use mooloop_plugin_host::scan::{PluginCache, ScannedPlugin};
use mooloop_plugin_host::HostError;
use mooloop_session::command::CommandState;
use mooloop_session::engine::{EngineCommandSender, StructuralCommandSender};
use mooloop_session::plugin_params::plugin_normalized;
use mooloop_session::session::Session;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::{
    BrowserRow, EffectSlotRow, MainWindow, PluginParamRow, UiState, BROWSER_PLUGIN,
};

/// Parameters a one-unit face holds: three across, two rows. Past this the
/// face takes a second unit, six across, and pages beyond twelve. The same
/// `columns` rule is in `plugin-device.slint`, which lays the knobs out.
const ONE_UNIT_PARAMS: usize = 6;

/// The width of a plugin's face: one unit when its parameters fit in one,
/// two otherwise. A face with more than a page pages rather than growing.
pub(crate) fn plugin_units(visible_params: usize) -> i32 {
    if visible_params <= ONE_UNIT_PARAMS {
        1
    } else {
        2
    }
}

// ---------------------------------------------------------------------------
// The browser's PLUGINS tab.

/// One installed plugin, as the browser offers it.
#[derive(Clone, Debug)]
pub(crate) struct CatalogEntry {
    pub plugin: ScannedPlugin,
    /// Why it cannot go in a chain, or `None` when it can.
    pub refusal: Option<String>,
}

/// What the scanner's cache lists, one row per plugin, and the files that
/// yielded none. Read from the cache file whenever the tab is opened, which
/// is also how a scan that finished after startup reaches the window.
#[derive(Clone, Debug, Default)]
pub(crate) struct PluginCatalog {
    pub entries: Vec<CatalogEntry>,
    /// A file that yielded nothing: its name and the scanner's reason.
    pub failures: Vec<(String, String)>,
}

/// Why `plugin` cannot be inserted as an effect, or `None` when it can.
///
/// The host runs exactly one stereo input and one stereo output
/// (`docs/plans/plugin-hosting/00-status.md`, step 06), and the scan says
/// so without loading anything. An instrument is a channel's source, which
/// is step 09's; the browser shows it and says so rather than hiding it.
pub(crate) fn effect_refusal(plugin: &ScannedPlugin) -> Option<String> {
    if let Some(error) = &plugin.error {
        return Some(format!("could not be created: {error}"));
    }
    if plugin.is_instrument() && !plugin.is_effect() {
        return Some("instrument: not yet a channel source".into());
    }
    if plugin.audio_inputs != [2] || plugin.audio_outputs != [2] {
        return Some("not stereo in and stereo out".into());
    }
    None
}

impl PluginCatalog {
    /// The catalogue in the cache file at `path`. One entry per plugin id,
    /// the copy [`PluginCache::resolve`] would open, sorted by name.
    pub(crate) fn load(path: &Path) -> Self {
        Self::from_cache(&PluginCache::load(path))
    }

    pub(crate) fn from_cache(cache: &PluginCache) -> Self {
        let mut entries: Vec<CatalogEntry> = Vec::new();
        for found in cache.plugins() {
            if entries.iter().any(|entry| {
                entry.plugin.plugin.format == found.plugin.format
                    && entry.plugin.plugin.id == found.plugin.id
            }) {
                continue;
            }
            let chosen = cache.resolve(&found.plugin).unwrap_or(found).clone();
            entries.push(CatalogEntry {
                refusal: effect_refusal(&chosen),
                plugin: chosen,
            });
        }
        entries.sort_by_key(|entry| entry.plugin.plugin.name.to_lowercase());
        let failures = cache
            .failures()
            .map(|(path, failure)| {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                (name, failure.reason.clone())
            })
            .collect();
        Self { entries, failures }
    }

    /// The plugin a browser row's `path` names: its id.
    pub(crate) fn find(&self, id: &str) -> Option<&CatalogEntry> {
        self.entries.iter().find(|entry| entry.plugin.plugin.id == id)
    }
}

/// Whether every word of `filter` is in `text`, ignoring case: the presets'
/// rule (MOO-9), by name, vendor, id and what the row says about itself.
fn matches(filter: &str, text: &str) -> bool {
    let text = text.to_lowercase();
    filter
        .split_whitespace()
        .all(|word| text.contains(&word.to_lowercase()))
}

/// The PLUGINS tab's rows: every plugin, those that cannot go in a chain
/// greyed with the reason, then the files that yielded none.
pub(crate) fn plugin_rows(catalog: &PluginCatalog, filter: &str) -> Vec<BrowserRow> {
    let mut rows = Vec::new();
    for entry in &catalog.entries {
        let plugin = &entry.plugin.plugin;
        let detail = match &entry.refusal {
            Some(reason) => reason.clone(),
            None if plugin.vendor.is_empty() => "FX".to_string(),
            None => format!("{} · FX", plugin.vendor),
        };
        let haystack = format!("{} {} {} {}", plugin.name, plugin.vendor, plugin.id, detail);
        if !matches(filter, &haystack) {
            continue;
        }
        rows.push(BrowserRow {
            depth: 0,
            kind: BROWSER_PLUGIN,
            name: plugin.name.as_str().into(),
            path: plugin.id.as_str().into(),
            expanded: false,
            detail: detail.into(),
            loadable: entry.refusal.is_none(),
            effect: entry.refusal.is_none(),
        });
    }
    for (name, reason) in &catalog.failures {
        if !matches(filter, &format!("{name} {reason}")) {
            continue;
        }
        rows.push(BrowserRow {
            depth: 0,
            kind: BROWSER_PLUGIN,
            name: name.as_str().into(),
            path: SharedString::default(),
            expanded: false,
            detail: format!("failed to scan: {reason}").into(),
            loadable: false,
            effect: false,
        });
    }
    rows
}

// ---------------------------------------------------------------------------
// The face.

/// What the rack's plugin faces were last drawn from, kept so a face is
/// updated in place and a knob under the pointer is never rebuilt.
///
/// Each plugin slot keeps one parameter model for its life. A row republished
/// with a *new* model would rebuild every knob on the face, and a knob being
/// dragged would lose its press; so a value changes by `set_row_data` on the
/// one model, the way every other face's knob follows its model row.
#[derive(Default)]
pub(crate) struct PluginFaces {
    models: RefCell<HashMap<PluginSlotId, Rc<VecModel<PluginParamRow>>>>,
    /// The plugin's own text for a parameter's value, and the value it was
    /// asked about. Refreshed when the value moves, never once a frame.
    texts: RefCell<HashMap<(PluginSlotId, usize), (f64, SharedString)>>,
}

/// Why the plugin in `slot` is not playing, in words, or empty when it is.
fn status_text(session: &Session, slot: PluginSlotId, name: &str) -> String {
    match session.plugin_problem(slot) {
        None => String::new(),
        Some(HostError::Missing) => format!(
            "Missing: {name} is not installed, or could not be opened. It plays dry, \
             and the song keeps it and its settings."
        ),
        Some(error) => format!("{name}: {error}."),
    }
}

/// A plain value as a readout, for when the plugin has not said how it
/// writes one (it is missing, or has not been asked yet).
fn plain_text(plain: f64) -> SharedString {
    let rounded = (plain * 100.0).round() / 100.0;
    format!("{rounded}").into()
}

impl PluginFaces {
    fn model(&self, slot: PluginSlotId) -> Rc<VecModel<PluginParamRow>> {
        self.models
            .borrow_mut()
            .entry(slot)
            .or_insert_with(|| Rc::new(VecModel::from(Vec::new())))
            .clone()
    }

    /// The face's rows for the plugin in `slot`: every parameter the plugin
    /// does not hide, in its own order, each carrying its dense index.
    fn param_rows(&self, session: &Session, slot: PluginSlotId) -> Vec<PluginParamRow> {
        let Some(saved) = session.plugins.get(&slot) else {
            return Vec::new();
        };
        let live = session
            .plugin_rack
            .instance(slot)
            .is_some_and(|instance| !instance.failed());
        let texts = self.texts.borrow();
        saved
            .params
            .iter()
            .enumerate()
            .filter(|(_, info)| !info.hidden)
            .map(|(index, info)| {
                let plain = session
                    .plugin_param_value(slot, index)
                    .unwrap_or(info.default);
                let text = match texts.get(&(slot, index)) {
                    Some((asked, text)) if *asked == plain => text.clone(),
                    _ => plain_text(plain),
                };
                PluginParamRow {
                    index: index as i32,
                    name: info.name.as_str().into(),
                    text,
                    value: plugin_normalized(info, plain),
                    default_value: plugin_normalized(info, info.default),
                    steps: info.stepped.map_or(0, i32::from),
                    enabled: live,
                }
            })
            .collect()
    }

    /// Ask the running plugin in `slot` for the text of every value that
    /// moved since it was last asked.
    fn refresh_texts(&self, session: &mut Session, slot: PluginSlotId) {
        let Some(saved) = session.plugins.get(&slot) else {
            return;
        };
        let asks: Vec<(usize, u32, f64)> = saved
            .params
            .iter()
            .enumerate()
            .filter(|(_, info)| !info.hidden)
            .filter_map(|(index, info)| {
                let plain = session.plugin_rack.param_value(slot, index)?;
                let known = self
                    .texts
                    .borrow()
                    .get(&(slot, index))
                    .is_some_and(|(asked, _)| *asked == plain);
                (!known).then_some((index, info.id, plain))
            })
            .collect();
        if asks.is_empty() {
            return;
        }
        let Some(instance) = session.plugin_rack.instance_mut(slot) else {
            return;
        };
        let mut texts = self.texts.borrow_mut();
        for (index, id, plain) in asks {
            let text = instance
                .value_text(id, plain)
                .map(SharedString::from)
                .unwrap_or_else(|| plain_text(plain));
            texts.insert((slot, index), (plain, text));
        }
    }

    /// Fill a rack row's plugin fields from the song and the running
    /// plugin, if the row is a plugin. Its parameters go into the slot's own
    /// model, updated in place.
    pub(crate) fn fill_row(&self, session: &Session, effect: &EffectSlotState, row: &mut EffectSlotRow) {
        let EffectParams::Plugin(slot) = effect.params else {
            return;
        };
        let name = session
            .plugins
            .get(&slot)
            .map_or("Plugin", |saved| saved.plugin.name.as_str());
        let params = self.param_rows(session, slot);
        row.is_plugin = true;
        row.units = plugin_units(params.len());
        row.plugin_name = name.into();
        row.plugin_status = status_text(session, slot, name).into();
        let model = self.model(slot);
        update_model(&model, params);
        row.plugin_params = ModelRc::from(model);
    }
}

/// Bring `model` to `rows`, touching only the rows that differ, so a knob
/// whose parameter did not move is left alone.
fn update_model(model: &VecModel<PluginParamRow>, rows: Vec<PluginParamRow>) {
    if model.row_count() != rows.len() {
        model.set_vec(rows);
        return;
    }
    for (at, row) in rows.into_iter().enumerate() {
        if model.row_data(at).as_ref() != Some(&row) {
            model.set_row_data(at, row);
        }
    }
}

impl UiState {
    /// Bring every plugin face on the rack up to date with its plugin: the
    /// values it reports, the text it gives them, whether it is running and
    /// why not. The pump calls it every tick after `service_plugins`, which
    /// is what makes a face follow the song -- an undo, the plugin's own
    /// edits, a plugin that opens late -- as a native face follows its row.
    pub(crate) fn refresh_plugin_faces(&mut self) {
        let Some(chain) = self.session.effect_chain() else {
            return;
        };
        let plugins: Vec<(usize, PluginSlotId)> = chain
            .iter()
            .enumerate()
            .filter_map(|(row, effect)| match effect.params {
                EffectParams::Plugin(slot) => Some((row, slot)),
                _ => None,
            })
            .collect();
        for (row, slot) in plugins {
            self.plugin_faces.refresh_texts(&mut self.session, slot);
            let Some(effect) = self
                .session
                .effect_chain()
                .and_then(|chain| chain.get(row))
                .cloned()
            else {
                continue;
            };
            let Some(mut data) = self.effect_slot_model.row_data(row) else {
                continue;
            };
            let was = (data.is_plugin, data.units, data.plugin_name.clone(), data.plugin_status.clone());
            // The same model as before, updated in place.
            self.plugin_faces.fill_row(&self.session, &effect, &mut data);
            let now = (data.is_plugin, data.units, data.plugin_name.clone(), data.plugin_status.clone());
            if was != now {
                self.effect_slot_model.set_row_data(row, data);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Adding a plugin, and editing one.

/// The engine's two command queues as the [`CommandSink`] the session's
/// plugin verbs take. A window callback holds the queues rather than the
/// `EngineHandle`, which the pump owns; the pump forwards both in the order
/// they were sent, which is the order the session sent them.
pub(crate) struct QueuedSink<'a> {
    pub(crate) tx: &'a EngineCommandSender,
    pub(crate) stx: &'a StructuralCommandSender,
    pub(crate) sample_rate: u32,
}

impl CommandSink for QueuedSink<'_> {
    fn send(&mut self, cmd: EngineCommand) -> bool {
        self.tx.send(cmd)
    }

    fn send_structural(&mut self, cmd: StructuralCommand) -> bool {
        self.stx.send(cmd)
    }

    /// Nothing that inserts a plugin sends a deferred command, and the
    /// queue has no deferred form; refused rather than sent early.
    fn send_deferred(&mut self, _cmd: EngineCommand, _when: MusicalEdge) -> bool {
        false
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

/// Where a plugin lands with no join to say: before the row a menu's
/// "Plugin…" aimed at, else just after the selected device (and the run a
/// selected container holds), else at the end of the chain.
fn default_position(st: &UiState) -> usize {
    let Some(chain) = st.session.effect_chain() else {
        return 0;
    };
    match st.session.selected_device_slot() {
        Some(selected) if selected < chain.len() => {
            let run = chain[selected].params.container_children().unwrap_or(0) as usize;
            (selected + 1 + run).min(chain.len())
        }
        _ => chain.len(),
    }
}

/// Put the plugin whose id is `id` in the chain the rack shows, before row
/// `before`, or where [`default_position`] says, as one undo step.
pub(crate) fn add_plugin(
    state: &Rc<RefCell<UiState>>,
    commands: &Rc<RefCell<CommandState>>,
    window: &MainWindow,
    id: &str,
    before: Option<usize>,
    tx: &EngineCommandSender,
    stx: &StructuralCommandSender,
) -> bool {
    let entry = state.borrow().plugin_catalog.find(id).cloned();
    let Some(entry) = entry else {
        window.set_status_message(format!("{id} is not in the plugin list").into());
        return false;
    };
    let name = entry.plugin.plugin.name.clone();
    if let Some(reason) = &entry.refusal {
        window.set_status_message(format!("{name} cannot go in a chain: {reason}").into());
        return false;
    }
    let position = {
        let mut st = state.borrow_mut();
        let aimed = st.plugin_insert_before.take();
        let length = st.session.effect_chain().map_or(0, |chain| chain.len());
        before.or(aimed).unwrap_or_else(|| default_position(&st)).min(length)
    };
    let snapshot = crate::project_snapshot(&state.borrow(), window);
    let inserted = {
        let mut st = state.borrow_mut();
        let mut sink = QueuedSink {
            tx,
            stx,
            sample_rate: st.audio_sample_rate,
        };
        st.session
            .insert_plugin_effect(entry.plugin.plugin.clone(), position, &mut sink)
    };
    let Some(inserted) = inserted else {
        window.set_status_message(format!("{name} could not be added: the rack is full").into());
        return false;
    };
    {
        let st = state.borrow();
        st.sync_effects();
        st.refresh_automation(window);
        st.refresh_modulation(window);
        // A plugin inserted inside a box grows it, and every box around it.
        st.publish_container_spans(inserted.target, stx);
    }
    state.borrow_mut().refresh_plugin_faces();
    crate::record_project_history(commands, snapshot, state, window, "Plugin added");
    let problem = {
        let st = state.borrow();
        match inserted.params {
            EffectParams::Plugin(slot) => st.session.plugin_problem(slot),
            _ => None,
        }
    };
    window.set_status_message(match problem {
        None => format!("Added {name}").into(),
        Some(error) => format!("Added {name}, but it is not playing: {error}").into(),
    });
    state.borrow().update_document_title(window);
    true
}

/// Open the browser on its PLUGINS tab, the catalogue read afresh.
pub(crate) fn show_plugin_browser(state: &Rc<RefCell<UiState>>, window: &MainWindow) {
    window.set_sidebar_visible(true);
    window.set_browser_tab(2);
    crate::set_focused_surface(window, crate::actions::Surface::Browser);
    let mut st = state.borrow_mut();
    st.enter_browser_tab(crate::BrowserTab::Plugins);
    crate::refresh_browser(&st);
}

/// Wire the plugin face and the plugin browser's callbacks. Called by
/// `AppUi::new`, and by the tests, which then drive the same handlers.
pub(crate) fn wire(
    window: &MainWindow,
    state: &Rc<RefCell<UiState>>,
    commands: &Rc<RefCell<CommandState>>,
    tx: &EngineCommandSender,
    stx: &StructuralCommandSender,
) {
    {
        // A knob on a plugin face: the value half of the edit. The plugin
        // holds the value, so the song has it once its state is captured;
        // the pump does that when the edits go quiet and no gesture is open,
        // and records it as one "Plugin Edit" undo step (MOO-82). The knob's
        // own `Gesture.begin()`/`end()` is what keeps a drag one step.
        // `dupe-audit unrecorded-edit` reports this handler, because the
        // record is the pump's (`record_finished_plugin_edits`), not here:
        // until the plugin has heard the value there is no state to record.
        let st = state.clone();
        let tx = tx.clone();
        window.on_plugin_param_changed(move |row, index, normalized| {
            let (Ok(row), Ok(index)) = (usize::try_from(row), usize::try_from(index)) else {
                return;
            };
            let mut st = st.borrow_mut();
            // A plugin that is not running has nobody to hear the value, and
            // the song keeps what it had: its face draws, and moves nothing.
            let running = st
                .session
                .effect_chain()
                .and_then(|chain| chain.get(row))
                .and_then(|effect| match effect.params {
                    EffectParams::Plugin(slot) => st.session.plugin_rack.instance(slot),
                    _ => None,
                })
                .is_some_and(|instance| !instance.failed());
            if !running {
                return;
            }
            let Some(command) = st.session.set_plugin_param(row, index, normalized) else {
                return;
            };
            let _ = tx.send(command);
            st.refresh_plugin_faces();
        });
    }
    {
        let st = state.clone();
        let weak = window.as_weak();
        window.on_add_plugin_requested(move |before| {
            let Some(window) = weak.upgrade() else { return };
            st.borrow_mut().plugin_insert_before = usize::try_from(before).ok();
            show_plugin_browser(&st, &window);
            window.set_status_message(
                "Pick a plugin: double-click it, press Enter, or drag it onto the rack".into(),
            );
        });
    }
    {
        let st = state.clone();
        let commands = commands.clone();
        let tx = tx.clone();
        let stx = stx.clone();
        let weak = window.as_weak();
        window.on_browser_plugin_added(move |id, before| {
            let Some(window) = weak.upgrade() else { return };
            add_plugin(&st, &commands, &window, &id, usize::try_from(before).ok(), &tx, &stx);
        });
    }
}
