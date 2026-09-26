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

use mooloop_core::{
    DeviceId, EffectParams, EffectSlotState, EffectTarget, EngineCommand, MusicalEdge, ParamAddr,
    ParamOwner, PluginParamInfo, PluginSlotId, PluginSlotState,
};
use mooloop_engine::{CommandSink, StructuralCommand};
use mooloop_plugin_host::scan::{PluginCache, ScannedPlugin};
use mooloop_plugin_host::HostError;
use mooloop_session::command::CommandState;
use mooloop_session::engine::{EngineCommandSender, StructuralCommandSender};
use mooloop_session::plugin_params::{plugin_normalized, plugin_plain};
use mooloop_session::session::Session;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::{
    BrowserRow, EffectSlotRow, MainWindow, PluginListRow, PluginParamRow, UiState, BROWSER_PLUGIN,
};

// ---------------------------------------------------------------------------
// What the face shows, and where (MOO-229).

/// How many parameters a plugin's face shows before anything is pinned: the
/// first this many the plugin does not hide (plugin-hosting 08). A plugin
/// with this many or fewer shows them all, which is every Airwindows plugin.
pub(crate) const DEFAULT_PINNED: usize = 8;

/// The ids the face of `saved` shows, in the plugin's order: the pinned ones
/// the plugin still lists, or -- while nothing has been pinned -- the first
/// [`DEFAULT_PINNED`] it does not hide.
///
/// `pinned` empty means "the default" rather than "nothing", so a song saved
/// before pins existed and a plugin nobody has touched both show their first
/// eight, and unpinning the last one brings the default back rather than
/// leaving a face with nothing on it. A pinned id the plugin no longer lists
/// is kept (MOO-74's "never drop") and simply not drawn.
pub(crate) fn face_ids(saved: &PluginSlotState) -> Vec<u32> {
    let visible = saved.params.iter().filter(|info| !info.hidden);
    if saved.pinned.is_empty() {
        return visible.take(DEFAULT_PINNED).map(|info| info.id).collect();
    }
    visible
        .filter(|info| saved.pinned.contains(&info.id))
        .map(|info| info.id)
        .collect()
}

/// `saved`'s pins with parameter `id` flipped. The default is made explicit
/// first, so the first pin or unpin on an untouched plugin starts from what
/// its face already shows. Pins the plugin no longer lists are kept.
pub(crate) fn toggled_pins(saved: &PluginSlotState, id: u32) -> Vec<u32> {
    let mut pins = if saved.pinned.is_empty() {
        face_ids(saved)
    } else {
        saved.pinned.clone()
    };
    match pins.iter().position(|pinned| *pinned == id) {
        Some(at) => {
            pins.remove(at);
        }
        None => pins.push(id),
    }
    pins
}

/// Positions a stepped parameter may have and still be drawn as a selector
/// (plugin-hosting 08): past this the labels do not fit a row.
pub(crate) const SELECTOR_MAX_POSITIONS: u16 = 8;

/// The plugin's names for a stepped parameter's positions, when the face can
/// trust them as a selector's segments. `name(plain)` asks the plugin for its
/// text at a plain value.
///
/// **The count is checked against the names.** LSP reports its filter type
/// as 0..1 in two positions, yet names more choices than two (plugin-hosting
/// `00-status.md`, step 07). A selector built from its two ends would offer
/// two of them and hide the rest. So a parameter is a selector only when its
/// positions are the whole values from min to max, every position has a name
/// of its own, and the value halfway between two neighbours reads as one of
/// them. A plugin with a choice in between fails the last check and keeps
/// its knob, which reaches every value.
pub(crate) fn selector_options(
    info: &PluginParamInfo,
    mut name: impl FnMut(f64) -> Option<String>,
) -> Option<Vec<String>> {
    let steps = info.stepped?;
    if !(2..=SELECTOR_MAX_POSITIONS).contains(&steps) || info.hidden {
        return None;
    }
    if (info.max - info.min - f64::from(steps - 1)).abs() > 1e-9 {
        return None;
    }
    let options: Vec<String> = (0..steps)
        .map(|position| name(info.min + f64::from(position)))
        .collect::<Option<_>>()?;
    let distinct = options
        .iter()
        .enumerate()
        .all(|(at, option)| !option.is_empty() && !options[..at].contains(option));
    if !distinct {
        return None;
    }
    for position in 0..steps - 1 {
        let between = name(info.min + f64::from(position) + 0.5)?;
        let (low, high) = (&options[position as usize], &options[position as usize + 1]);
        if between != *low && between != *high {
            return None;
        }
    }
    Some(options)
}

/// Rows of controls a face page holds: two, under the header, with the page
/// buttons below (`plugin-device.slint`'s `row-pitch`).
const ROWS_PER_PAGE: usize = 2;

/// The controls across one row of a face `units` wide: three on one unit,
/// six on two. `plugin-device.slint` reads each control's place from its row
/// and counts no columns itself.
pub(crate) fn face_columns(units: i32) -> usize {
    if units <= 1 {
        3
    } else {
        6
    }
}

/// One control's place on the face: its page, its row on the page, its
/// first column, and how many columns it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Place {
    pub page: usize,
    pub row: usize,
    pub column: usize,
    pub span: usize,
}

/// Where each control lands, in the plugin's order, row by row and a page at
/// a time: a knob takes one cell, a selector (`true`) a whole row, since its
/// segments need the width. Returns the places and the page count.
pub(crate) fn pack(selectors: &[bool], columns: usize) -> (Vec<Place>, usize) {
    let mut places = Vec::with_capacity(selectors.len());
    let (mut page, mut row, mut column) = (0usize, 0usize, 0usize);
    let next_row = |page: &mut usize, row: &mut usize, column: &mut usize| {
        *column = 0;
        *row += 1;
        if *row == ROWS_PER_PAGE {
            *row = 0;
            *page += 1;
        }
    };
    for &selector in selectors {
        if selector {
            if column > 0 {
                next_row(&mut page, &mut row, &mut column);
            }
            places.push(Place { page, row, column: 0, span: columns });
            next_row(&mut page, &mut row, &mut column);
        } else {
            places.push(Place { page, row, column, span: 1 });
            column += 1;
            if column == columns {
                next_row(&mut page, &mut row, &mut column);
            }
        }
    }
    let pages = if row == 0 && column == 0 { page } else { page + 1 };
    (places, pages.max(1))
}

/// The width of a face: one unit when its controls fit one unit's page,
/// two otherwise. A face with more than a page pages rather than growing.
pub(crate) fn face_units(selectors: &[bool]) -> i32 {
    if pack(selectors, face_columns(1)).1 == 1 {
        1
    } else {
        2
    }
}

/// Each control's group caption: `(caption, span)` on the control that
/// starts a run of one group on its row, `("", 0)` elsewhere. A run is
/// broken by a new row, so a group that carries over is named again.
pub(crate) fn captions<'a>(groups: &[&'a str], places: &[Place]) -> Vec<(&'a str, usize)> {
    let mut out = vec![("", 0); groups.len()];
    let mut at = 0;
    while at < groups.len() {
        let start = at;
        let mut span = places[at].span;
        at += 1;
        while at < groups.len()
            && groups[at] == groups[start]
            && (places[at].page, places[at].row) == (places[start].page, places[start].row)
        {
            span += places[at].span;
            at += 1;
        }
        if !groups[start].is_empty() {
            out[start] = (groups[start], span);
        }
    }
    out
}

/// The sidebar's PARAMETERS rows for `saved`: every parameter it does not
/// hide, in its own order, those `filter` matches (by name or group, the
/// presets' rule), each marked pinned when the face shows it. A row that
/// starts a run of a group carries the group's name.
pub(crate) fn param_list_rows(saved: &PluginSlotState, filter: &str) -> Vec<PluginListRow> {
    let shown = face_ids(saved);
    let mut previous: Option<&str> = None;
    saved
        .params
        .iter()
        .enumerate()
        .filter(|(_, info)| !info.hidden)
        .filter(|(_, info)| {
            filter.trim().is_empty() || matches(filter, &format!("{} {}", info.name, info.module))
        })
        .map(|(index, info)| {
            let starts = previous != Some(info.module.as_str());
            previous = Some(info.module.as_str());
            PluginListRow {
                index: index as i32,
                name: info.name.as_str().into(),
                group: if starts { info.module.as_str().into() } else { SharedString::default() },
                pinned: shown.contains(&info.id),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The browser's PLUGINS tab.

/// One installed plugin, as the browser offers it.
#[derive(Clone, Debug)]
pub(crate) struct CatalogEntry {
    pub plugin: ScannedPlugin,
    /// An instrument: picked, it becomes a new channel's source (MOO-84's
    /// `Session::set_plugin_source`) rather than a device in a chain.
    pub instrument: bool,
    /// Why it cannot be used, or `None` when it can.
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

/// Whether the browser offers `plugin` as an instrument (a new channel's
/// source) rather than an effect.
///
/// Where a plugin may go is the host's rule, read from the scan
/// (`ScannedPlugin::source_refusal` and `effect_refusal`, MOO-85), not a
/// copy of it here (MOO-232): it is an instrument when it fits a source and
/// either does not fit a chain or declares itself one. One that fits
/// nowhere is shown in the role it declares, greyed with that role's reason.
pub(crate) fn is_instrument(plugin: &ScannedPlugin) -> bool {
    let (effect, source) = (plugin.effect_refusal().is_none(), plugin.source_refusal().is_none());
    match (effect, source) {
        (_, true) => !effect || plugin.is_instrument(),
        (true, false) => false,
        (false, false) => {
            plugin.is_instrument() || (!plugin.is_effect() && plugin.note_inputs > 0)
        }
    }
}

/// Why `plugin` cannot be used in the role [`is_instrument`] gives it, or
/// `None` when it can: the host's own reason from the scan.
pub(crate) fn refusal(plugin: &ScannedPlugin) -> Option<String> {
    if is_instrument(plugin) {
        plugin.source_refusal()
    } else {
        plugin.effect_refusal()
    }
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
                instrument: is_instrument(&chosen),
                refusal: refusal(&chosen),
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
        // An instrument plays no notes until step 10 (MOO-85); the row says
        // so rather than let a silent channel be the first anyone hears of it.
        let role = if entry.instrument { "Instrument" } else { "FX" };
        let detail = match &entry.refusal {
            Some(reason) => reason.clone(),
            None if plugin.vendor.is_empty() => role.to_string(),
            None => format!("{} · {role}", plugin.vendor),
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
            effect: entry.refusal.is_none() && !entry.instrument,
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

/// A stepped parameter's selector segments, under the id they were asked
/// for, or `None` for a parameter that stays a knob.
type SelectorCache = (u32, Option<ModelRc<SharedString>>);

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
    /// Each stepped parameter's selector segments, asked of the running
    /// plugin once ([`selector_options`]), under the id it was asked for;
    /// `None` when it stays a knob. **One model per parameter for its life**:
    /// a row compares equal only while its `options` is the same model, so a
    /// fresh one each tick would republish every row and drop a drag.
    options: RefCell<HashMap<(PluginSlotId, usize), SelectorCache>>,
    /// The sidebar's PARAMETERS list: its model (set on the window once, by
    /// [`wire`]), the filter typed into it, and what it was last built from.
    list: Rc<VecModel<PluginListRow>>,
    list_filter: RefCell<String>,
    list_key: std::cell::Cell<Option<u64>>,
    /// The plugin the list last described: the filter is cleared when the
    /// selection moves to another, so a new plugin never opens filtered.
    list_slot: std::cell::Cell<Option<PluginSlotId>>,
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

    /// The face's rows for the plugin in `slot`, and its width in units: the
    /// parameters its face shows ([`face_ids`]), in the plugin's order, each
    /// carrying its dense index and its place ([`pack`]).
    fn param_rows(&self, session: &Session, slot: PluginSlotId) -> (Vec<PluginParamRow>, i32) {
        let Some(saved) = session.plugins.get(&slot) else {
            return (Vec::new(), 1);
        };
        let live = session
            .plugin_rack
            .instance(slot)
            .is_some_and(|instance| !instance.failed());
        let shown = face_ids(saved);
        let texts = self.texts.borrow();
        let options = self.options.borrow();
        let picked: Vec<(usize, &PluginParamInfo, Option<ModelRc<SharedString>>)> = saved
            .params
            .iter()
            .enumerate()
            .filter(|(_, info)| !info.hidden && shown.contains(&info.id))
            .map(|(index, info)| {
                let names = options
                    .get(&(slot, index))
                    .filter(|(id, _)| *id == info.id)
                    .and_then(|(_, names)| names.clone());
                (index, info, names)
            })
            .collect();
        let selectors: Vec<bool> = picked.iter().map(|(_, _, names)| names.is_some()).collect();
        let units = face_units(&selectors);
        let (places, _) = pack(&selectors, face_columns(units));
        let groups: Vec<&str> = picked.iter().map(|(_, info, _)| info.module.as_str()).collect();
        let captions = captions(&groups, &places);
        let rows = picked
            .into_iter()
            .zip(places)
            .zip(captions)
            .map(|(((index, info, names), place), (caption, caption_span))| {
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
                    page: place.page as i32,
                    row: place.row as i32,
                    column: place.column as i32,
                    span: place.span as i32,
                    options: names.unwrap_or_default(),
                    position: (plain - info.min).round() as i32,
                    caption: caption.into(),
                    caption_span: caption_span as i32,
                }
            })
            .collect();
        (rows, units)
    }

    /// Ask the running plugin in `slot` for the text of every value its face
    /// shows that moved since it was last asked, and for the segments of
    /// each stepped one it has not been asked about yet.
    fn refresh_texts(&self, session: &mut Session, slot: PluginSlotId) {
        let Some(saved) = session.plugins.get(&slot) else {
            return;
        };
        let shown = face_ids(saved);
        let unasked: Vec<(usize, PluginParamInfo)> = saved
            .params
            .iter()
            .enumerate()
            .filter(|(_, info)| info.stepped.is_some() && shown.contains(&info.id))
            .filter(|(index, info)| {
                self.options
                    .borrow()
                    .get(&(slot, *index))
                    .is_none_or(|(id, _)| *id != info.id)
            })
            .map(|(index, info)| (index, info.clone()))
            .collect();
        if !unasked.is_empty() {
            let running = session
                .plugin_rack
                .instance(slot)
                .is_some_and(|instance| !instance.failed());
            // A plugin that is not running cannot be asked, and is asked
            // again once it is: until then its stepped parameters are knobs.
            if let (true, Some(instance)) = (running, session.plugin_rack.instance_mut(slot)) {
                let mut options = self.options.borrow_mut();
                for (index, info) in unasked {
                    let names = selector_options(&info, |plain| instance.value_text(info.id, plain))
                        .map(|names| {
                            let names: Vec<SharedString> =
                                names.iter().map(|name| name.as_str().into()).collect();
                            ModelRc::from(names.as_slice())
                        });
                    options.insert((slot, index), (info.id, names));
                }
            }
        }
        let Some(saved) = session.plugins.get(&slot) else {
            return;
        };
        let asks: Vec<(usize, u32, f64)> = saved
            .params
            .iter()
            .enumerate()
            .filter(|(_, info)| !info.hidden && shown.contains(&info.id))
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
        let (params, units) = self.param_rows(session, slot);
        row.is_plugin = true;
        row.units = units;
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

    /// The hosted plugin the selected device is, if it is one: the device
    /// the sidebar's PARAMETERS list describes.
    pub(crate) fn selected_plugin_slot(&self) -> Option<PluginSlotId> {
        let row = self.session.selected_device_slot()?;
        match self.session.effect_chain()?.get(row)?.params {
            EffectParams::Plugin(slot) => Some(slot),
            _ => None,
        }
    }

    /// Bring the sidebar's PARAMETERS list up to date with the selected
    /// device, its pins and the filter. The pump calls it every tick; it
    /// rebuilds only when one of those changed, read through a hash so an
    /// unchanged tick allocates nothing, and a pin, an undo, a selection or
    /// a plugin that reports its list late all reach it the same way.
    pub(crate) fn refresh_plugin_param_list(&self, window: &MainWindow) {
        use std::hash::{Hash, Hasher};
        let slot = self.selected_plugin_slot();
        if self.plugin_faces.list_slot.replace(slot) != slot {
            self.plugin_faces.list_filter.borrow_mut().clear();
        }
        let saved = slot.and_then(|slot| self.session.plugins.get(&slot));
        let filter = self.plugin_faces.list_filter.borrow();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        slot.hash(&mut hasher);
        filter.hash(&mut hasher);
        if let Some(saved) = saved {
            saved.pinned.hash(&mut hasher);
            for info in &saved.params {
                (info.id, info.hidden, &info.name, &info.module).hash(&mut hasher);
            }
        }
        let key = hasher.finish();
        if self.plugin_faces.list_key.get() == Some(key) {
            return;
        }
        self.plugin_faces.list_key.set(Some(key));
        let (rows, total) = match saved {
            Some(saved) => (
                param_list_rows(saved, &filter),
                saved.params.iter().filter(|info| !info.hidden).count(),
            ),
            None => (Vec::new(), 0),
        };
        self.plugin_faces.list.set_vec(rows);
        window.set_plugin_param_total(total as i32);
        window.set_plugin_param_filter(filter.as_str().into());
    }
}

/// Wire the sidebar's PARAMETERS list: its filter, which is view state, and
/// its pins, which are saved with the song and so are an edit and an undo
/// step ("Pin Parameter").
fn wire_param_list(window: &MainWindow, state: &Rc<RefCell<UiState>>, commands: &Rc<RefCell<CommandState>>) {
    window.set_plugin_param_rows(ModelRc::from(state.borrow().plugin_faces.list.clone()));
    {
        let st = state.clone();
        let weak = window.as_weak();
        window.on_plugin_param_filter_edited(move |filter| {
            let Some(window) = weak.upgrade() else { return };
            let st = st.borrow();
            *st.plugin_faces.list_filter.borrow_mut() = filter.to_string();
            st.refresh_plugin_param_list(&window);
        });
    }
    {
        let st = state.clone();
        let commands = commands.clone();
        let weak = window.as_weak();
        window.on_plugin_param_pin_toggled(move |index| {
            let (Some(window), Ok(index)) = (weak.upgrade(), usize::try_from(index)) else {
                return;
            };
            if commands.borrow().project_edit_pending {
                return;
            }
            let Some(slot) = st.borrow().selected_plugin_slot() else { return };
            let before = crate::project_snapshot(&st.borrow(), &window);
            {
                let mut st = st.borrow_mut();
                let Some(saved) = st.session.plugins.get(&slot) else { return };
                let Some(id) = saved.params.get(index).map(|info| info.id) else { return };
                let pins = toggled_pins(saved, id);
                if let Some(saved) = st.session.plugins.get_mut(&slot) {
                    saved.pinned = pins;
                }
                st.session.mark_dirty();
                st.refresh_plugin_faces();
                st.refresh_plugin_param_list(&window);
            }
            crate::record_project_history(&commands, before, &st, &window, "Pin Parameter");
            st.borrow().update_document_title(&window);
        });
    }
}

// ---------------------------------------------------------------------------
// Lanes and routes on a plugin's parameters (MOO-228).

/// A plugin face's modulation overlays, each indexed by a parameter's dense
/// index -- the one number that crosses into Slint -- the way a native face's
/// are indexed by descriptor id.
#[derive(Default)]
pub(crate) struct PluginOverlays {
    pub depths: Vec<f32>,
    pub allowed: Vec<bool>,
    pub offsets: Vec<f32>,
    pub counts: Vec<i32>,
}

/// The overlays for the plugin device `device`, whose slot is `slot`, on the
/// selected channel's chain, with modulator `armed` (if any) assigning.
pub(crate) fn plugin_overlays(
    session: &Session,
    armed: Option<u8>,
    device: DeviceId,
    slot: PluginSlotId,
) -> PluginOverlays {
    let Some(saved) = session.plugins.get(&slot) else {
        return PluginOverlays::default();
    };
    let Some(channel) = session.channels.get(session.selected) else {
        return PluginOverlays::default();
    };
    let scope = EffectTarget::Channel(session.selected as u8);
    let mut overlays = PluginOverlays {
        offsets: session.plugin_destination_offsets(device, slot),
        ..PluginOverlays::default()
    };
    for info in &saved.params {
        let address = ParamAddr::plugin_param(scope, device, info.id);
        overlays
            .depths
            .push(armed.map_or(0.0, |armed| session.modulation_depth_for(armed, address)));
        overlays
            .allowed
            .push(session.modulation_policy(address).is_some_and(|policy| policy.allowed));
        // Every route, allowed or not, as a native knob counts them: an
        // assignment is authored work the user can see and remove.
        overlays.counts.push(
            channel
                .modulation
                .destinations()
                .filter(|destination| *destination == address)
                .count() as i32,
        );
    }
    overlays
}

/// The plugin parameter the face on chain row `row` names by `index`, as an
/// address on the selected channel: the one place a face's index becomes the
/// plugin's id for a route, a lane or a MIDI mapping. `None` when the rack is
/// not showing the selected channel, the row is not a plugin, or the plugin
/// lists no such index.
pub(crate) fn face_param_address(session: &Session, row: usize, index: usize) -> Option<ParamAddr> {
    let EffectTarget::Channel(channel) = session.effect_target else {
        return None;
    };
    if channel as usize != session.selected {
        return None;
    }
    let effect = session.channels.get(session.selected)?.effects.get(row)?;
    let EffectParams::Plugin(slot) = effect.params else {
        return None;
    };
    let id = session.plugin_param_id(slot, index)?;
    Some(ParamAddr::plugin_param(session.effect_target, effect.id, id))
}

/// One row of the lane picker: a native destination with its descriptor, or
/// a plugin parameter, which has none.
#[derive(Clone, Debug)]
pub(crate) struct LaneDestination {
    pub address: ParamAddr,
    pub device: String,
    pub name: String,
    /// A lane or route names it and the plugin no longer lists it (MOO-74):
    /// kept, saved, and drawn as missing.
    pub missing: bool,
}

/// Everything the lane picker offers, in its order: the session's native
/// destinations, with the selected channel's plugin parameters placed after
/// its inserts and before its strip -- where they sit in the signal path.
/// The picker's index, the Automate request and the header label all read
/// this one list, so they cannot disagree about what a position names.
pub(crate) fn lane_destinations(session: &Session) -> Vec<LaneDestination> {
    let native = session.automation_destinations();
    let mut plugins = session
        .plugin_destinations()
        .into_iter()
        .map(|row| LaneDestination {
            address: row.address,
            device: row.device,
            name: row.name,
            missing: row.missing,
        });
    let mut rows = Vec::with_capacity(native.len());
    for (address, device, descriptor) in native {
        if address.owner == ParamOwner::Strip {
            rows.extend(plugins.by_ref());
        }
        rows.push(LaneDestination {
            address,
            device,
            name: descriptor.name.to_string(),
            missing: false,
        });
    }
    rows.extend(plugins);
    rows
}

/// A plugin lane point's value in the parameter's own plain units, for the
/// lane header's readout. `None` for a native address, or a parameter the
/// plugin no longer lists.
pub(crate) fn plugin_value_text(session: &Session, address: ParamAddr, normalized: f32) -> Option<String> {
    let info = session.plugin_param_info(address)?;
    Some(plain_text(plugin_plain(info, normalized)).to_string())
}

/// The dense index a route row carries for a plugin parameter: its place in
/// the plugin's list, or -1 when the plugin no longer lists it.
pub(crate) fn route_param_index(session: &Session, address: ParamAddr) -> i32 {
    let ParamOwner::PluginParam { device } = address.owner else {
        return -1;
    };
    session
        .plugin_slot_of(address.scope, device)
        .and_then(|slot| session.plugin_param_index(slot, address.param))
        .and_then(|index| i32::try_from(index).ok())
        .unwrap_or(-1)
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

/// The window's engine queues, as a plugin verb needs them.
#[derive(Clone)]
pub(crate) struct Queues {
    pub(crate) tx: EngineCommandSender,
    pub(crate) stx: StructuralCommandSender,
    /// A new channel's sample state is reset through this, as every add is.
    pub(crate) reset_tx: std::sync::mpsc::Sender<usize>,
}

/// A new channel whose source is the instrument `entry`, selected, as one
/// undo step ("Plugin channel added"): the add and the source together, so
/// one Ctrl+Z takes the channel away rather than leaving an empty one.
pub(crate) fn add_plugin_channel(
    state: &Rc<RefCell<UiState>>,
    commands: &Rc<RefCell<CommandState>>,
    window: &MainWindow,
    entry: &CatalogEntry,
    queues: &Queues,
) -> bool {
    let name = entry.plugin.plugin.name.clone();
    if commands.borrow().project_edit_pending {
        return false;
    }
    let snapshot = crate::project_snapshot(&state.borrow(), window);
    let Some(index) = crate::add_channel_unrecorded(
        state,
        window,
        &queues.stx,
        &queues.reset_tx,
        mooloop_core::DeviceKind::Plugin,
    ) else {
        window.set_status_message(format!("{name} could not be added: the rack is full").into());
        return false;
    };
    let slot = {
        let mut st = state.borrow_mut();
        let mut sink = QueuedSink {
            tx: &queues.tx,
            stx: &queues.stx,
            sample_rate: st.audio_sample_rate,
        };
        let slot = st
            .session
            .set_plugin_source(index, entry.plugin.plugin.clone(), &mut sink);
        // Named after the plugin rather than "Plugin 2": the channel's row
        // is the only thing that says which instrument it is.
        st.session.rename_channel(index as i32, &name);
        slot
    };
    {
        let st = state.borrow();
        if let Some(mut row) = st.rows.row_data(index) {
            row.name = st.session.channels[index].name.as_str().into();
            st.rows.set_row_data(index, row);
        }
        st.sync_mixer(window);
        st.refresh_editor(window);
    }
    crate::record_project_history(commands, snapshot, state, window, "Plugin channel added");
    let problem = slot.and_then(|slot| state.borrow().session.plugin_problem(slot));
    window.set_status_message(match problem {
        None => format!("Added {name} on a new channel. It plays no notes until CLAP instruments land").into(),
        Some(error) => format!("Added {name} on a new channel, but it is not playing: {error}").into(),
    });
    state.borrow().update_document_title(window);
    true
}

/// Put the plugin whose id is `id` in the chain the rack shows, before row
/// `before`, or where [`default_position`] says, as one undo step. An
/// instrument goes on a new channel instead ([`add_plugin_channel`]).
pub(crate) fn add_plugin(
    state: &Rc<RefCell<UiState>>,
    commands: &Rc<RefCell<CommandState>>,
    window: &MainWindow,
    id: &str,
    before: Option<usize>,
    queues: &Queues,
) -> bool {
    let entry = state.borrow().plugin_catalog.find(id).cloned();
    let Some(entry) = entry else {
        window.set_status_message(format!("{id} is not in the plugin list").into());
        return false;
    };
    let name = entry.plugin.plugin.name.clone();
    if let Some(reason) = &entry.refusal {
        window.set_status_message(format!("{name} cannot be used: {reason}").into());
        return false;
    }
    if entry.instrument {
        return add_plugin_channel(state, commands, window, &entry, queues);
    }
    let (tx, stx) = (&queues.tx, &queues.stx);
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
    reset_tx: &std::sync::mpsc::Sender<usize>,
    // Whether a MIDI learn made from a plugin knob binds the controller that
    // taught it: the MIDI preference, read at the press.
    learn_binds_port: Rc<dyn Fn() -> bool>,
) {
    let queues = Queues {
        tx: tx.clone(),
        stx: stx.clone(),
        reset_tx: reset_tx.clone(),
    };
    wire_param_list(window, state, commands);
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
        // A plugin knob's press, when it is not a plain value edit: a MIDI
        // learn, a naming press for the control menu, or the start of a
        // route-depth drag -- the native `effect-modulation-edit-started`'s
        // three answers, for an address built from the face's index here.
        let st = state.clone();
        let weak = window.as_weak();
        let binds_port = learn_binds_port.clone();
        window.on_plugin_modulation_edit_started(move |row, index| {
            let (Some(window), Ok(row), Ok(index)) =
                (weak.upgrade(), usize::try_from(row), usize::try_from(index))
            else {
                return;
            };
            let mut state = st.borrow_mut();
            let Some(address) = face_param_address(&state.session, row, index) else {
                return;
            };
            if state.name_if_asked(&window, address) {
                return;
            }
            if state.learn_param_if_armed(&window, binds_port(), address) {
                return;
            }
            state.begin_gesture(&window);
        });
    }
    {
        let st = state.clone();
        let commands = commands.clone();
        let tx = tx.clone();
        let weak = window.as_weak();
        window.on_plugin_modulation_depth_changed(move |row, index, depth| {
            let (Some(window), Ok(row), Ok(index)) =
                (weak.upgrade(), usize::try_from(row), usize::try_from(index))
            else {
                return;
            };
            crate::with_gesture_history(&st, &commands, &window, "Modulation depth", || {
                let mut state = st.borrow_mut();
                let Some(destination) = face_param_address(&state.session, row, index) else {
                    return false;
                };
                if !state.set_armed_modulation_depth(&window, &tx, destination, depth) {
                    state.refresh_modulation(&window);
                    return false;
                }
                true
            });
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
        let queues = queues.clone();
        let weak = window.as_weak();
        window.on_browser_plugin_added(move |id, before| {
            let Some(window) = weak.upgrade() else { return };
            add_plugin(&st, &commands, &window, &id, usize::try_from(before).ok(), &queues);
        });
    }
    {
        // The add-channel menu's "Add Plugin…": the instrument is chosen in
        // the browser, which makes the channel.
        let st = state.clone();
        let weak = window.as_weak();
        window.on_add_plugin_channel_requested(move || {
            let Some(window) = weak.upgrade() else { return };
            st.borrow_mut().plugin_insert_before = None;
            show_plugin_browser(&st, &window);
            window.set_status_message(
                "Pick an instrument: double-click it or press Enter to add it on a new channel".into(),
            );
        });
    }
}

#[cfg(test)]
mod face_tests {
    use super::*;
    use mooloop_core::{PluginFormat, PluginRef};

    fn param(id: u32, name: &str) -> PluginParamInfo {
        PluginParamInfo {
            id,
            name: name.into(),
            module: String::new(),
            min: 0.0,
            max: 1.0,
            default: 0.0,
            stepped: None,
            automatable: true,
            modulatable: true,
            hidden: false,
        }
    }

    fn slot(count: u32) -> PluginSlotState {
        PluginSlotState {
            params: (0..count).map(|id| param(id * 10, &format!("P{id}"))).collect(),
            ..PluginSlotState::new(PluginRef {
                format: PluginFormat::Clap,
                id: "test".into(),
                name: "Test".into(),
                vendor: String::new(),
                version: String::new(),
            })
        }
    }

    /// Nothing pinned shows the first eight the plugin does not hide; a pin
    /// on an untouched plugin starts from those eight, and a pin the plugin
    /// no longer lists is kept but not drawn.
    #[test]
    fn the_face_shows_the_first_eight_until_something_is_pinned() {
        let mut saved = slot(20);
        saved.params[1].hidden = true;
        assert_eq!(face_ids(&saved), [0, 20, 30, 40, 50, 60, 70, 80]);
        saved.pinned = toggled_pins(&saved, 150);
        assert_eq!(face_ids(&saved), [0, 20, 30, 40, 50, 60, 70, 80, 150]);
        saved.pinned = toggled_pins(&saved, 20);
        assert_eq!(face_ids(&saved), [0, 30, 40, 50, 60, 70, 80, 150]);
        saved.pinned.push(9_999);
        assert_eq!(face_ids(&saved).len(), 8);
        saved.pinned = toggled_pins(&saved, 0);
        assert!(saved.pinned.contains(&9_999), "a pin the plugin stopped listing is kept");
    }

    /// Only whole-value positions that each have a name of their own, and no
    /// choice named between them, make a selector (the LSP check).
    #[test]
    fn a_selector_trusts_the_names_not_the_count() {
        let stepped = |steps: u16, max: f64| PluginParamInfo {
            stepped: Some(steps),
            max,
            ..param(1, "Mode")
        };
        let words = ["Off", "Low", "Band", "High"];
        let honest = |plain: f64| words.get(plain.round() as usize).map(|w| w.to_string());
        assert_eq!(
            selector_options(&stepped(4, 3.0), honest),
            Some(words.iter().map(|w| w.to_string()).collect())
        );
        // LSP: 0..1 in two positions, and a third choice named in between.
        let lsp = |plain: f64| {
            Some(if plain < 0.25 { "Off" } else if plain < 0.75 { "Band" } else { "High" }.to_string())
        };
        assert_eq!(selector_options(&stepped(2, 1.0), lsp), None);
        // Positions that are not whole values, or more than eight, stay knobs.
        assert_eq!(selector_options(&stepped(4, 1.0), honest), None);
        assert_eq!(selector_options(&stepped(9, 8.0), |p| Some(format!("{p}"))), None);
        // Two positions with one name are not two choices.
        assert_eq!(selector_options(&stepped(2, 1.0), |_| Some("Same".into())), None);
        // A plugin that cannot name a position keeps its knob.
        assert_eq!(selector_options(&stepped(2, 1.0), |_| None), None);
    }

    /// A selector takes a whole row, the knobs after it start the next, and
    /// the rows after two start a page.
    #[test]
    fn a_selector_takes_a_row_and_pages_follow() {
        let at = |page, row, column, span| Place { page, row, column, span };
        let (places, pages) = pack(&[false, false, true, false], 3);
        assert_eq!(places, [at(0, 0, 0, 1), at(0, 0, 1, 1), at(0, 1, 0, 3), at(1, 0, 0, 1)]);
        assert_eq!(pages, 2);
        let (places, pages) = pack(&[false; 6], 3);
        assert_eq!(places.last(), Some(&at(0, 1, 2, 1)));
        assert_eq!(pages, 1);
        assert_eq!(pack(&[], 6).1, 1);
        // Six knobs fit one unit; a seventh, or a selector and four, do not.
        assert_eq!(face_units(&[false; 6]), 1);
        assert_eq!(face_units(&[false; 7]), 2);
        assert_eq!(face_units(&[true, false, false, false, false]), 2);
        assert_eq!(face_units(&[true, false, false, false]), 1);
    }

    /// A group is named once per run on a row, over the run's width, and
    /// again where it carries onto the next row.
    #[test]
    fn a_group_is_captioned_over_its_run() {
        let groups = ["", "Filter", "Filter", "Filter", "Env"];
        let (places, _) = pack(&[false; 5], 3);
        assert_eq!(
            captions(&groups, &places),
            [("", 0), ("Filter", 2), ("", 0), ("Filter", 1), ("Env", 1)]
        );
    }

    /// The list names a group on the row that starts its run, marks what the
    /// face shows, and filters by name or group.
    #[test]
    fn the_list_groups_marks_pins_and_filters() {
        let mut saved = slot(12);
        for info in &mut saved.params[2..5] {
            info.module = "Filter".into();
        }
        saved.params[3].name = "Frequency".into();
        let rows = param_list_rows(&saved, "");
        assert_eq!(rows.len(), 12);
        assert_eq!(rows[2].group, "Filter");
        assert_eq!(rows[3].group, "");
        assert!(rows[7].pinned && !rows[8].pinned);
        let found = param_list_rows(&saved, "freq");
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].index, found[0].group.as_str()), (3, "Filter"));
        assert_eq!(param_list_rows(&saved, "filter").len(), 3);
    }
}
