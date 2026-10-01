//! The control thread's hosted plugins (`docs/plans/plugin-hosting/04-the-plugin-rack.md`,
//! `06-a-headless-clap-effect.md`).
//!
//! Once a processor is installed, nothing on the control thread can reach
//! it: the node belongs to the audio thread. The rack is what the control
//! thread keeps instead -- each plugin's [`HostedInstance`], by the song's
//! [`PluginSlotId`] -- so that a plugin's requests have somewhere to land,
//! its latency has somewhere to be read from, and its instance is dropped
//! only after its processor is (blocker 5).
//!
//! **Teardown order is the whole point.** Removing a plugin moves its entry
//! to the graveyard; the entry is dropped by [`PluginRack::collect`] only once
//! every processor tied to its [`Lifeline`] has been dropped. Processors are
//! only ever dropped on the control thread -- `EngineHandle::poll` drains the
//! reclaim ring, and a retired project goes the same way -- so the instance
//! never outlives-by-accident or dies-before its processor, and neither is
//! ever freed in the callback.
//!
//! **Every live instance whose processor is not out gets one** (step 06).
//! That one rule covers every way a processor goes missing, because a plugin
//! format may allow only one processor per instance (CLAP does) and so a new
//! one can only be built once the old one is back:
//!
//! - a song opens, or an install rebuilds a chain the carry plan (MOO-137)
//!   did not carry: the chain holds `build_effect`'s placeholder, and the
//!   old processor comes back with the retired generation;
//! - the plugin asks for a restart, or the sample rate changes: the rack
//!   pulls the processor back by swapping the placeholder into its slot;
//! - the instance is new: the opener made it and nothing was built yet.
//!
//! In each case the next pump tick after the lifeline is alone builds a
//! processor and swaps it in with `ReplaceEffect`, keyed by the slot, so a
//! swap that arrives after the device went finds nothing and does nothing.
//!
//! **A channel's source can be a plugin too** (step 09, MOO-84). The same
//! rule holds, with the channel's `HostedSource` in place of the effect's
//! placeholder: a song opening, or an install that did not carry the strip,
//! builds it silent, and the processor goes in with
//! `StructuralCommand::HostSourceProcessor`, keyed by the slot the same way.
//! A pull-back takes the processor out and leaves the source silent, since an
//! instrument's placeholder has nothing to pass through.

use std::collections::{BTreeMap, BTreeSet};

use mooloop_core::{
    log_warn, mint_plugin_slot, DeviceKind, EffectKind, EffectParams,
    EffectSlotState, EffectTarget, EngineCommand, GeneratorParams, PluginParamInfo, PluginRef,
    PluginSlotId, PluginSlotState, PluginSlots, PluginState, PluginStateText,
};
use mooloop_dsp::effects::PluginPlaceholder;
use mooloop_dsp::{AudioNode, HostedSource, IntegerDelay, SpectrumAnalyzer};
use mooloop_engine::{CommandSink, EffectSlot, StructuralCommand};
use mooloop_plugin_host::{
    AudioConfig, GuiConfig, GuiError, GuiRequest, GuiSize, HostError, HostedGui, HostedInstance,
    IoActivity, Lifeline, NativeWindow, PluginOpener, PluginParamEvent, Requests,
};

use crate::effects::{EffectInserted, EffectPlace};

/// The largest block a hosted processor is activated for: the most frames
/// the executor ever hands a node (`mooloop_dsp::MAX_BLOCK_SIZE`).
pub const PLUGIN_MAX_FRAMES: u32 = mooloop_plugin_host::clap::MAX_FRAMES;

/// The app's opener: CLAP plugins found through the scanner's cache at
/// `cache_path` (`<config>/plugins.toml`), read again whenever a scan
/// rewrites it, so a song opened before the startup scan finishes finds its
/// plugins once it has.
pub fn clap_opener(cache_path: std::path::PathBuf) -> Box<dyn PluginOpener> {
    Box::new(mooloop_plugin_host::clap::ClapOpener::new(cache_path))
}

/// How many times in a row the rack builds a processor that never stays
/// out before it stops trying. A processor that is refused by the engine
/// comes straight back, and without a limit that would be an activate and
/// a deactivate every pump tick.
const MAX_ATTEMPTS: u8 = 3;

/// Ticks a processor has to stay out before it counts as having arrived,
/// which resets [`MAX_ATTEMPTS`]. About 400 ms at the 8 ms pump.
const SETTLED_TICKS: u32 = 50;

/// Quiet ticks after the last of a run of the plugin's own changes that
/// came with no gesture around them, before the run counts as one edit.
/// About half a second at the 8 ms pump: the pause a mapped controller's
/// moves are cut into undo steps by (`CONTROLLER_IDLE` in the interface).
pub const EDIT_QUIET_TICKS: u32 = 60;

/// Where the plugin's own editing stands, for one undo step per gesture
/// (MOO-82).
#[derive(Debug, Default)]
struct EditRun {
    /// Parameters the plugin has a gesture open on.
    open: Vec<u32>,
    /// Something changed that is not in the song yet.
    pending: bool,
    /// Pump ticks since the last change arrived.
    quiet: u32,
}

impl EditRun {
    /// Take one of the plugin's reports. Returns whether it closes an edit
    /// at once: the end of its last open gesture.
    fn take(&mut self, event: PluginParamEvent) -> bool {
        self.quiet = 0;
        match event {
            PluginParamEvent::GestureBegin { id } => {
                if !self.open.contains(&id) {
                    self.open.push(id);
                }
                false
            }
            PluginParamEvent::GestureEnd { id } => {
                self.open.retain(|&open| open != id);
                self.pending = true;
                self.open.is_empty()
            }
            PluginParamEvent::Value { .. } => {
                self.pending = true;
                false
            }
        }
    }

    /// One pump tick has passed. Returns whether a run of changes with no
    /// gesture open has now been quiet long enough to be one edit.
    fn tick(&mut self) -> bool {
        if !self.pending || !self.open.is_empty() {
            return false;
        }
        self.quiet = self.quiet.saturating_add(1);
        self.quiet >= EDIT_QUIET_TICKS
    }

    fn finish(&mut self) {
        self.pending = false;
        self.quiet = 0;
    }
}

struct RackEntry {
    instance: Box<dyn HostedInstance>,
    lifeline: Lifeline,
    /// The processor that is out is stale -- a restart or a new rate -- and
    /// has to come back so a new one can be built.
    rebuild: bool,
    /// A pull-back was already sent for the stale processor.
    pulled: bool,
    /// Processors built in a row that did not stay out.
    attempts: u8,
    /// Ticks the current processor has been out.
    out_ticks: u32,
    /// Each parameter's current value in the plugin's plain units, by its
    /// dense index in `instance.params()` (step 03): what a knob draws and
    /// what a route's depth is shown against. Read from the plugin when the
    /// instance is made and whenever its list changes, and kept up to date
    /// from the plugin's own reports and the session's edits.
    values: Vec<f64>,
    /// The plugin's own editing, cut into undo steps.
    edits: EditRun,
    /// [`HostedInstance::dropped_param_events`] when it was last read, so a
    /// growing count is logged once per growth.
    dropped_seen: u64,
}

impl RackEntry {
    fn new(mut instance: Box<dyn HostedInstance>, lifeline: Lifeline) -> Self {
        let values = read_values(instance.as_mut());
        Self {
            instance,
            lifeline,
            rebuild: false,
            pulled: false,
            attempts: 0,
            out_ticks: 0,
            values,
            edits: EditRun::default(),
            dropped_seen: 0,
        }
    }

    /// Take the plugin's own reports since the last tick into `values`.
    /// Returns whether an edit finished: a gesture ended, or a run of lone
    /// values has gone quiet. Nothing here sends anything to the plugin:
    /// what it reports about itself never comes back to it as a command.
    fn drain(&mut self, slot: PluginSlotId) -> bool {
        let Self {
            instance,
            values,
            edits,
            ..
        } = self;
        let ids: Vec<u32> = instance.params().iter().map(|param| param.id).collect();
        let mut finished = false;
        instance.drain_param_events(&mut |event| {
            if let PluginParamEvent::Value { id, value } = event {
                if let Some(stored) = ids
                    .iter()
                    .position(|&known| known == id)
                    .and_then(|index| values.get_mut(index))
                {
                    *stored = value;
                }
            }
            finished |= edits.take(event);
        });
        finished |= edits.tick();
        let dropped = instance.dropped_param_events();
        if dropped != self.dropped_seen {
            if dropped > self.dropped_seen {
                log_warn!(
                    "plugin",
                    "slot {}: {} of the plugin's own parameter changes were lost (its ring was full)",
                    slot.0,
                    dropped - self.dropped_seen
                );
            }
            self.dropped_seen = dropped;
        }
        if finished {
            self.edits.finish();
        }
        finished
    }
}

/// Every parameter's value as the plugin holds it now, by dense index.
fn read_values(instance: &mut dyn HostedInstance) -> Vec<f64> {
    let ids: Vec<(u32, f64)> = instance
        .params()
        .iter()
        .map(|param| (param.id, param.default))
        .collect();
    ids.into_iter()
        .map(|(id, default)| instance.param_value(id).unwrap_or(default))
        .collect()
}

/// Whether two songs' records of one slot name the same plugin in the same
/// state: an instance made for one serves the other.
fn same_plugin(was: Option<&PluginSlotState>, now: Option<&PluginSlotState>) -> bool {
    match (was, now) {
        (Some(was), Some(now)) => was.plugin == now.plugin && was.state == now.state,
        _ => false,
    }
}

/// A slot that could not be hosted, and the opener's generation when it
/// was tried: it is tried again when the generation moves.
struct Problem {
    error: HostError,
    generation: u64,
}

/// What the rack's owner has to act on after [`PluginRack::service`].
pub enum RackEvent {
    /// A new processor for the plugin in `slot`, to be swapped into its
    /// device's placeholder (`StructuralCommand::ReplaceEffect`).
    Install {
        slot: PluginSlotId,
        node: Box<dyn AudioNode + Send>,
    },
    /// The processor out for `slot` is stale: swap the placeholder in so it
    /// comes back and a new one can be built.
    PullBack { slot: PluginSlotId },
    /// The opener made an instance for `slot`, and this is its parameter
    /// list.
    Opened {
        slot: PluginSlotId,
        params: Vec<PluginParamInfo>,
    },
    /// The plugin's latency changed: compensation has to be derived again.
    LatencyChanged { slot: PluginSlotId },
    /// The plugin's parameter list changed. It replaces
    /// `PluginSlotState::params`; lanes and routes whose ids went away are
    /// kept and shown as missing (Adam, 2026-09-23, MOO-74).
    ParamsRescanned {
        slot: PluginSlotId,
        params: Vec<PluginParamInfo>,
    },
    /// The plugin could not be opened, or could not build a processor.
    Failed { slot: PluginSlotId, error: HostError },
}

impl std::fmt::Debug for RackEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Install { slot, .. } => write!(f, "Install({})", slot.0),
            Self::PullBack { slot } => write!(f, "PullBack({})", slot.0),
            Self::Opened { slot, params } => write!(f, "Opened({}, {} params)", slot.0, params.len()),
            Self::LatencyChanged { slot } => write!(f, "LatencyChanged({})", slot.0),
            Self::ParamsRescanned { slot, params } => {
                write!(f, "ParamsRescanned({}, {} params)", slot.0, params.len())
            }
            Self::Failed { slot, error } => write!(f, "Failed({}, {error})", slot.0),
        }
    }
}

/// Every hosted plugin the control thread holds, by song slot.
#[derive(Default)]
pub struct PluginRack {
    entries: BTreeMap<PluginSlotId, RackEntry>,
    /// Removed from the song, or built for an export, and waiting for their
    /// processors to come back.
    graveyard: Vec<(Box<dyn HostedInstance>, Lifeline)>,
    problems: BTreeMap<PluginSlotId, Problem>,
    opener: Option<Box<dyn PluginOpener>>,
    /// The configuration processors are built for, as of the last tick.
    config: Option<AudioConfig>,
    /// Slots whose plugin finished an edit of its own that the song does not
    /// have yet: for the pump to record as one undo step around
    /// `Session::capture_plugin_edits` (MOO-82).
    edited: BTreeSet<PluginSlotId>,
    /// Slots whose saved state the plugin refused when the song opened. The
    /// plugin runs with its defaults, and the song keeps the refused state
    /// byte for byte until the plugin is edited: capturing it here would
    /// write the defaults over the only copy of what was saved.
    refused_state: BTreeSet<PluginSlotId>,
    /// What the window side has to act on about plugin GUIs: the plugin's
    /// own requests, and GUIs the rack closed by itself (step 11, MOO-300).
    gui_events: Vec<(PluginSlotId, PluginGuiEvent)>,
}

/// Where a plugin's GUI opens (step 11, policies 2 to 4). The ids are
/// [`mooloop_plugin_host::GuiApi::native`] window ids
/// ([`NativeWindow::native`]): an X11 window id on Linux, an `NSView*`'s
/// address on macOS. Each window must outlive the GUI, which
/// [`PluginGuiEvent::Closed`] or
/// [`crate::session::Session::close_plugin_gui`] returning says is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuiPlacement {
    /// Embedded in the host's bare window `parent`.
    Embedded { parent: u64 },
    /// In the plugin's own window, kept above `transient_for` where the
    /// session allows it (not under a native Wayland session).
    Floating { transient_for: Option<u64> },
}

impl GuiPlacement {
    pub fn config(self) -> GuiConfig {
        match self {
            Self::Embedded { .. } => GuiConfig::native_embedded(),
            Self::Floating { .. } => GuiConfig::native_floating(),
        }
    }
}

/// How to open a plugin's GUI.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginGuiOpen {
    pub placement: GuiPlacement,
    /// The window's title, which a floating GUI is offered as well.
    pub title: String,
    /// The window's scale factor. A plugin that reads it from the system
    /// may refuse it, which does not stop the GUI opening.
    pub scale: Option<f64>,
}

/// A GUI that opened: what the window should now be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginGuiOpened {
    pub config: GuiConfig,
    /// The size the plugin wants, when it says.
    pub size: Option<GuiSize>,
    /// Whether the user may resize the window. An embedded GUI that cannot
    /// gets fixed min and max size hints, so a tiling compositor floats it.
    pub can_resize: bool,
}

/// Something the window side of a plugin's GUI has to do, drained once a
/// pump tick ([`crate::session::Session::drain_plugin_gui_events`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginGuiEvent {
    /// The plugin asks for its window at this size.
    Resize(GuiSize),
    /// The plugin showed its GUI: map the window.
    Show,
    /// The plugin hid its GUI: unmap the window.
    Hide,
    /// Ask [`crate::session::Session::resize_plugin_gui`]'s rules again:
    /// the plugin's resize hints changed.
    ResizeHintsChanged,
    /// The GUI is destroyed: the plugin closed it, or the rack did because
    /// its device went, its song closed or the app is quitting. Destroy the
    /// window now, never before this.
    Closed,
}

/// Hide and destroy `instance`'s GUI if it is open. Returns whether it was.
/// Never touches the processor: a GUI's lifetime is its own.
fn destroy_gui(instance: &mut dyn HostedInstance) -> bool {
    let Some(gui) = instance.gui() else {
        return false;
    };
    if gui.open_config().is_none() {
        return false;
    }
    if gui.is_visible() {
        let _ = gui.hide();
    }
    gui.destroy();
    true
}

impl std::fmt::Debug for PluginRack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginRack")
            .field("live", &self.entries.keys().map(|slot| slot.0).collect::<Vec<_>>())
            .field("dying", &self.graveyard.len())
            .field("problems", &self.problems.keys().map(|slot| slot.0).collect::<Vec<_>>())
            .field("edited", &self.edited.iter().map(|slot| slot.0).collect::<Vec<_>>())
            .field("refused_state", &self.refused_state.iter().map(|slot| slot.0).collect::<Vec<_>>())
            .finish()
    }
}

impl PluginRack {
    pub fn new() -> Self {
        Self::default()
    }

    /// What finds and opens the plugins a song names. The app's is
    /// [`mooloop_plugin_host::clap::ClapOpener`] over the scanner's cache.
    pub fn set_opener(&mut self, opener: Box<dyn PluginOpener>) {
        self.opener = Some(opener);
        // Everything that failed is worth trying again with a new opener.
        self.problems.clear();
    }

    pub fn has_opener(&self) -> bool {
        self.opener.is_some()
    }

    /// Open `plugin` through the opener, outside the rack: for a caller that
    /// holds the instance itself (an export).
    pub fn open(
        &mut self,
        plugin: &PluginRef,
        state: &PluginState,
        config: AudioConfig,
    ) -> Result<Box<dyn HostedInstance>, HostError> {
        match self.opener.as_mut() {
            Some(opener) => opener.open(plugin, state, config),
            None => Err(HostError::Missing),
        }
    }

    /// Take `instance` in as `slot` and build the processor to install.
    ///
    /// The caller sends the processor with `InstallEffect`; the rack keeps
    /// the instance. A slot already live is refused rather than replaced, so
    /// an instance is never dropped while a processor of it may still be
    /// running.
    pub fn insert(
        &mut self,
        slot: PluginSlotId,
        mut instance: Box<dyn HostedInstance>,
    ) -> Result<Box<dyn AudioNode + Send>, HostError> {
        if self.entries.contains_key(&slot) {
            return Err(HostError::Plugin(format!("slot {} is already hosted", slot.0)));
        }
        let lifeline = Lifeline::new();
        let node = instance.build_processor(lifeline.tie())?;
        self.problems.remove(&slot);
        self.entries.insert(slot, RackEntry::new(instance, lifeline));
        Ok(node)
    }

    /// Keep `instance` and its `lifeline` until every processor tied to it
    /// is gone, as for a removed entry. For processors built outside the
    /// song's chains: an export's.
    pub fn bury(&mut self, instance: Box<dyn HostedInstance>, lifeline: Lifeline) {
        self.graveyard.push((instance, lifeline));
    }

    /// The live instance in `slot`, if there is one.
    pub fn instance(&self, slot: PluginSlotId) -> Option<&dyn HostedInstance> {
        self.entries.get(&slot).map(|entry| entry.instance.as_ref())
    }

    /// The live instance in `slot`, mutably.
    pub fn instance_mut(&mut self, slot: PluginSlotId) -> Option<&mut (dyn HostedInstance + 'static)> {
        self.entries.get_mut(&slot).map(|entry| entry.instance.as_mut())
    }

    /// Parameter `index`'s current value in the plugin's plain units, by
    /// its dense index in the live instance's list.
    pub fn param_value(&self, slot: PluginSlotId, index: usize) -> Option<f64> {
        self.entries.get(&slot)?.values.get(index).copied()
    }

    /// Record a value the session sent to parameter `index` of `slot`, and
    /// count it as an edit of the plugin's state, closed when the edits go
    /// quiet: the plugin holds the value, so the song only has it once the
    /// state is captured after the value has reached it.
    pub fn note_param_sent(&mut self, slot: PluginSlotId, index: usize, value: f64) {
        if let Some(entry) = self.entries.get_mut(&slot) {
            if let Some(stored) = entry.values.get_mut(index) {
                *stored = value;
            }
            entry.edits.pending = true;
            entry.edits.quiet = 0;
        }
    }

    /// Whether a plugin has finished an edit the song does not have yet.
    pub fn has_edits(&self) -> bool {
        !self.edited.is_empty()
    }

    /// The slots with a finished edit, emptied.
    pub fn take_edits(&mut self) -> BTreeSet<PluginSlotId> {
        std::mem::take(&mut self.edited)
    }

    /// Whether the song keeps a saved state the plugin in `slot` refused.
    pub fn refused_state(&self, slot: PluginSlotId) -> bool {
        self.refused_state.contains(&slot)
    }

    /// The plugin in `slot` was edited: the state it holds now is the one
    /// the song should keep, even over one it once refused.
    pub fn accept_state(&mut self, slot: PluginSlotId) {
        self.refused_state.remove(&slot);
    }

    /// Every live slot.
    pub fn live_slots(&self) -> impl Iterator<Item = PluginSlotId> + '_ {
        self.entries.keys().copied()
    }

    /// The latency the plugin in `slot` reports now, or `None` when no live
    /// instance is hosted there -- a missing plugin plays as a pass-through,
    /// which adds nothing.
    pub fn latency_frames(&self, slot: PluginSlotId) -> Option<u32> {
        self.instance(slot).map(|instance| instance.latency_frames())
    }

    /// Why `slot` is not hosted, or why its plugin stopped: the opener could
    /// not find or open it, it could not build a processor, or its
    /// processor gave up on it.
    pub fn problem(&self, slot: PluginSlotId) -> Option<HostError> {
        if let Some(problem) = self.problems.get(&slot) {
            return Some(problem.error.clone());
        }
        if self.refused_state.contains(&slot) {
            return Some(HostError::Plugin(
                "it refused its saved state, so it plays with its defaults; the song keeps the saved state".into(),
            ));
        }
        self.instance(slot)
            .filter(|instance| instance.failed())
            .map(|_| HostError::Plugin("it failed while processing and is passed through".into()))
    }

    /// Record that `slot` could not be hosted, until the opener's catalogue
    /// changes.
    pub fn record_problem(&mut self, slot: PluginSlotId, error: HostError) {
        let generation = self.opener.as_mut().map_or(0, |opener| opener.refresh());
        self.problems.insert(slot, Problem { error, generation });
    }

    /// Retire `slot`. The caller sends the device's removal; the instance
    /// stays until [`Self::collect`] sees its processors gone. Returns
    /// whether a live entry was there.
    ///
    /// Step 04's order: its GUI is destroyed here, before the instance goes
    /// to wait for its processor, and a [`PluginGuiEvent::Closed`] tells the
    /// window side its window may go.
    pub fn remove(&mut self, slot: PluginSlotId) -> bool {
        self.problems.remove(&slot);
        self.edited.remove(&slot);
        self.refused_state.remove(&slot);
        match self.entries.remove(&slot) {
            Some(mut entry) => {
                if destroy_gui(entry.instance.as_mut()) {
                    self.gui_events.push((slot, PluginGuiEvent::Closed));
                }
                self.graveyard.push((entry.instance, entry.lifeline));
                true
            }
            None => false,
        }
    }

    /// Retire every entry: the song is closing. Returns the slots whose
    /// processors are still out, for the caller to pull back. Every GUI is
    /// destroyed first.
    pub fn close(&mut self) -> Vec<PluginSlotId> {
        self.problems.clear();
        self.edited.clear();
        self.refused_state.clear();
        let mut out = Vec::new();
        for (slot, mut entry) in std::mem::take(&mut self.entries) {
            if destroy_gui(entry.instance.as_mut()) {
                self.gui_events.push((slot, PluginGuiEvent::Closed));
            }
            if !entry.lifeline.is_alone() {
                out.push(slot);
            }
            self.graveyard.push((entry.instance, entry.lifeline));
        }
        out
    }

    /// Which way `slot`'s GUI opens, in the platform's API
    /// ([`GuiConfig::native_order`]): embedded where the plugin can
    /// (policy 2), floating where it only offers that (policy 4).
    pub fn gui_placement_kind(&mut self, slot: PluginSlotId) -> Result<GuiConfig, GuiError> {
        let gui = self.gui_of(slot)?;
        GuiConfig::native_order()
            .into_iter()
            .find(|&config| gui.is_api_supported(config))
            .ok_or(GuiError::Unsupported(GuiConfig::native_embedded()))
    }

    fn gui_of(&mut self, slot: PluginSlotId) -> Result<&mut dyn HostedGui, GuiError> {
        self.entries
            .get_mut(&slot)
            .ok_or(GuiError::Missing)?
            .instance
            .gui()
            .ok_or(GuiError::NoGui)
    }

    /// Open `slot`'s GUI as `open` says and show it. On any failure after it
    /// was created it is destroyed again, so nothing is left half open.
    pub fn open_gui(&mut self, slot: PluginSlotId, open: &PluginGuiOpen) -> Result<PluginGuiOpened, GuiError> {
        let gui = self.gui_of(slot)?;
        if gui.open_config().is_some() {
            return Err(GuiError::AlreadyOpen);
        }
        let config = open.placement.config();
        gui.create(config)?;
        let shown = (|| {
            if let Some(scale) = open.scale {
                // Refused by a plugin that reads the scale itself: not a
                // reason not to open.
                let _ = gui.set_scale(scale);
            }
            match open.placement {
                GuiPlacement::Embedded { parent } => gui.set_parent(NativeWindow::native(parent))?,
                GuiPlacement::Floating { transient_for } => {
                    gui.suggest_title(&open.title);
                    if let Some(window) = transient_for {
                        if let Err(error) = gui.set_transient(NativeWindow::native(window)) {
                            log_warn!("plugin", "slot {}: {error}; its window may fall behind", slot.0);
                        }
                    }
                }
            }
            let opened = PluginGuiOpened {
                config,
                size: gui.size(),
                can_resize: gui.can_resize(),
            };
            gui.show()?;
            Ok(opened)
        })();
        if shown.is_err() {
            gui.destroy();
        }
        shown
    }

    /// Hide and destroy `slot`'s GUI: its window's close button, or the
    /// device's own toggle. The processor keeps running. Returns whether a
    /// GUI was open; the caller's window may go once this returns.
    pub fn close_gui(&mut self, slot: PluginSlotId) -> bool {
        self.entries
            .get_mut(&slot)
            .is_some_and(|entry| destroy_gui(entry.instance.as_mut()))
    }

    /// Whether a GUI the rack closed by itself is waiting to be reported.
    pub fn has_gui_events(&self) -> bool {
        !self.gui_events.is_empty()
    }

    /// Whether `slot`'s GUI is open.
    pub fn gui_is_open(&mut self, slot: PluginSlotId) -> bool {
        self.gui_of(slot)
            .is_ok_and(|gui| gui.open_config().is_some())
    }

    /// Show `slot`'s GUI again (its window was mapped).
    pub fn show_gui(&mut self, slot: PluginSlotId) -> Result<(), GuiError> {
        self.gui_of(slot)?.show()
    }

    /// Hide `slot`'s GUI without destroying it (its window was unmapped).
    pub fn hide_gui(&mut self, slot: PluginSlotId) -> Result<(), GuiError> {
        self.gui_of(slot)?.hide()
    }

    /// The user resized `slot`'s window to `size`: the nearest size the
    /// plugin takes, which it now has and the window should snap to.
    pub fn resize_gui(&mut self, slot: PluginSlotId, size: GuiSize) -> Result<GuiSize, GuiError> {
        let gui = self.gui_of(slot)?;
        if gui.open_config().is_none() {
            return Err(GuiError::NotOpen);
        }
        if !gui.can_resize() {
            return gui.size().ok_or(GuiError::Refused("be resized"));
        }
        let adjusted = gui.adjust_size(size).unwrap_or(size);
        gui.set_size(adjusted)?;
        Ok(adjusted)
    }

    /// The window's scale factor changed.
    pub fn set_gui_scale(&mut self, slot: PluginSlotId, scale: f64) -> Result<(), GuiError> {
        self.gui_of(slot)?.set_scale(scale)
    }

    /// Fire every live plugin's due timers and ready fds (policy 1). Never
    /// waits. Retired instances are not serviced: their GUIs are gone.
    pub fn service_io(&mut self, now: std::time::Instant) -> IoActivity {
        let mut activity = IoActivity::default();
        for entry in self.entries.values_mut() {
            activity += entry.instance.service_io(now);
        }
        activity
    }

    /// The plugins' requests of their windows, carried out on the plugin's
    /// side where there is one to carry out, and every GUI the rack closed
    /// by itself since the last call.
    ///
    /// A plugin that says its window was closed has its GUI hidden and
    /// destroyed here, as the close button would (the step's "Lifetime"),
    /// and is reported as [`PluginGuiEvent::Closed`].
    pub fn drain_gui_events(&mut self) -> Vec<(PluginSlotId, PluginGuiEvent)> {
        let mut events = std::mem::take(&mut self.gui_events);
        for (&slot, entry) in &mut self.entries {
            let Some(gui) = entry.instance.gui() else {
                continue;
            };
            let mut requests = Vec::new();
            gui.take_requests(&mut |request| requests.push(request));
            for request in requests {
                let event = match request {
                    GuiRequest::Resize(size) => PluginGuiEvent::Resize(size),
                    GuiRequest::ResizeHintsChanged => PluginGuiEvent::ResizeHintsChanged,
                    GuiRequest::Show => {
                        if gui.show().is_err() {
                            continue;
                        }
                        PluginGuiEvent::Show
                    }
                    GuiRequest::Hide => {
                        if gui.hide().is_err() {
                            continue;
                        }
                        PluginGuiEvent::Hide
                    }
                    GuiRequest::Closed { .. } => {
                        if gui.is_visible() {
                            let _ = gui.hide();
                        }
                        gui.destroy();
                        PluginGuiEvent::Closed
                    }
                };
                events.push((slot, event));
                if event == PluginGuiEvent::Closed {
                    break;
                }
            }
        }
        events
    }

    /// What an install does to the rack: keep every instance whose slot the
    /// incoming song records as the same plugin in the same state as the
    /// outgoing one did, and retire the rest. The parameter list and the
    /// pinned ids are what the song remembers *about* the plugin, and an
    /// instance does not change when they do.
    ///
    /// A structural edit (paste, move, delete, undo) installs a snapshot of
    /// the session, so its slots are the session's own and every instance
    /// is kept -- and the carry plan keeps its processor running, which is
    /// why retiring it here left a processor whose instance could no longer
    /// restart or report its latency. Opening another song retires
    /// everything, unless a slot of it is identical in every field.
    pub fn retire_except(&mut self, outgoing: &PluginSlots, incoming: &PluginSlots) {
        let retired: Vec<PluginSlotId> = self
            .entries
            .keys()
            .copied()
            .filter(|slot| !same_plugin(outgoing.get(slot), incoming.get(slot)))
            .collect();
        for slot in retired {
            self.remove(slot);
        }
        self.problems
            .retain(|slot, _| same_plugin(outgoing.get(slot), incoming.get(slot)));
    }

    /// Drop every retired instance whose processors have all been dropped,
    /// and return how many were. Run after `EngineHandle::poll` has drained
    /// the reclaim ring, which is where a processor is dropped.
    pub fn collect(&mut self) -> usize {
        let before = self.graveyard.len();
        self.graveyard.retain(|(_, lifeline)| !lifeline.is_alone());
        before - self.graveyard.len()
    }

    /// Entries still held, retired ones included.
    pub fn len(&self) -> usize {
        self.entries.len() + self.graveyard.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.graveyard.is_empty()
    }

    /// Entries removed from the song whose processors have not come back
    /// yet. A song close waits on this reaching zero, with a bounded timeout.
    pub fn dying(&self) -> usize {
        self.graveyard.len()
    }

    /// Give up on every instance still held, without dropping it: for the
    /// end of a quit whose bounded wait ran out. An instance dropped while
    /// its processor may still be running on the audio thread is the one
    /// thing worse than a leak at exit.
    pub fn leak_remaining(&mut self) -> usize {
        let count = self.len();
        for (_, mut entry) in std::mem::take(&mut self.entries) {
            // A GUI does not wait on the processor: it goes even here.
            destroy_gui(entry.instance.as_mut());
            std::mem::forget(entry);
        }
        for held in std::mem::take(&mut self.graveyard) {
            std::mem::forget(held);
        }
        count
    }

    /// The rack's once-a-tick upkeep. `slots` is the song's plugin table,
    /// `named` the slots a device on some chain names, and `config` what the
    /// engine runs at now.
    ///
    /// Drops retired instances whose processors came back; opens the slots
    /// a device names that have no instance; carries out what each plugin
    /// asked for; pulls back stale processors; and builds a processor for
    /// every live instance a device names whose processor is not out.
    /// Everything that needs the engine comes back as a [`RackEvent`].
    pub fn service(
        &mut self,
        slots: &PluginSlots,
        named: &BTreeSet<PluginSlotId>,
        config: AudioConfig,
    ) -> Vec<RackEvent> {
        let mut events = Vec::new();
        self.collect();
        // A retired instance's requests are drained and ignored: a restart
        // that arrives after its slot was removed must not reinstall
        // anything, or fire later.
        for (instance, _) in &self.graveyard {
            let _ = instance.take_requests();
        }

        // A new rate, or a new block ceiling: every processor out is stale.
        if self.config != Some(config) {
            if self.config.is_some() {
                for entry in self.entries.values_mut() {
                    entry.instance.set_audio_config(config);
                    entry.rebuild = true;
                    // A build that failed at the old rate may not at this one
                    // (MOO-354).
                    entry.attempts = 0;
                }
            }
            self.config = Some(config);
        }

        // Open what the song names and nothing hosts.
        let mut catalogue = None;
        if let Some(opener) = self.opener.as_mut() {
            let generation = opener.refresh();
            catalogue = Some(generation);
            for &slot in named {
                if self.entries.contains_key(&slot) {
                    continue;
                }
                let Some(state) = slots.get(&slot) else {
                    continue;
                };
                if self
                    .problems
                    .get(&slot)
                    .is_some_and(|problem| problem.generation == generation)
                {
                    continue;
                }
                // A plugin that refuses the state the song saved for it opens
                // with its defaults instead, rather than not at all
                // (`07-parameters-and-state.md`, "Load"): tried again with no
                // state, and the song's copy is left as it is.
                let opened = opener.open(&state.plugin, &state.state.0, config).or_else(|error| {
                    if state.state.0.is_empty() {
                        return Err(error);
                    }
                    let fresh = opener.open(&state.plugin, &PluginState::default(), config)?;
                    log_warn!(
                        "plugin",
                        "{} refused its saved state ({error}); it plays with its defaults and the song keeps the state",
                        state.plugin.name
                    );
                    self.refused_state.insert(slot);
                    Ok(fresh)
                });
                match opened {
                    Ok(instance) => {
                        self.problems.remove(&slot);
                        events.push(RackEvent::Opened {
                            slot,
                            params: instance.params().to_vec(),
                        });
                        self.entries
                            .insert(slot, RackEntry::new(instance, Lifeline::new()));
                    }
                    Err(error) => {
                        self.problems.insert(
                            slot,
                            Problem {
                                error: error.clone(),
                                generation,
                            },
                        );
                        events.push(RackEvent::Failed { slot, error });
                    }
                }
            }
        }

        for (&slot, entry) in &mut self.entries {
            // A device removed from its chain leaves its instance live for an
            // undo, but not its window: the GUI of a device nobody can see
            // goes now, and the processor stays as it is.
            if !named.contains(&slot) && destroy_gui(entry.instance.as_mut()) {
                self.gui_events.push((slot, PluginGuiEvent::Closed));
            }
            let requests: Requests = entry.instance.take_requests();
            if requests.has(Requests::CALLBACK) {
                entry.instance.on_main_thread();
            }
            if requests.has(Requests::RESTART) {
                entry.rebuild = true;
                // A plugin that asks to be restarted is asking for a new
                // build, whether or not the last one worked (MOO-354).
                entry.attempts = 0;
            }
            // A rescan since the build failed: the plugin on disk may not be
            // the one that failed (MOO-354). The problem is re-recorded at
            // the new generation if the next build fails too.
            if catalogue.is_some_and(|now| {
                self.problems.get(&slot).is_some_and(|problem| problem.generation != now)
            }) {
                entry.attempts = 0;
            }
            if requests.has(Requests::LATENCY_CHANGED) {
                events.push(RackEvent::LatencyChanged { slot });
            }
            if requests.has(Requests::PARAMS_RESCAN) {
                entry.instance.refresh_params();
                entry.values = read_values(entry.instance.as_mut());
                events.push(RackEvent::ParamsRescanned {
                    slot,
                    params: entry.instance.params().to_vec(),
                });
            }
            // The plugin's own edits: a gesture that ended, a quiet run of
            // lone values, or a `mark_dirty` with no report beside it (a
            // plugin that changed something it has no parameter for). Each
            // is one undo step, recorded by the pump around capturing it.
            let mut finished = entry.drain(slot);
            if requests.has(Requests::STATE_DIRTY) && entry.edits.open.is_empty() {
                entry.edits.finish();
                finished = true;
            }
            if finished {
                self.edited.insert(slot);
            }

            if !entry.lifeline.is_alone() {
                entry.out_ticks = entry.out_ticks.saturating_add(1);
                if entry.out_ticks >= SETTLED_TICKS {
                    entry.attempts = 0;
                }
                if entry.rebuild && !entry.pulled {
                    entry.pulled = true;
                    events.push(RackEvent::PullBack { slot });
                }
                continue;
            }
            entry.out_ticks = 0;
            entry.pulled = false;
            if !named.contains(&slot) || entry.attempts >= MAX_ATTEMPTS {
                continue;
            }
            entry.attempts += 1;
            match entry.instance.build_processor(entry.lifeline.tie()) {
                Ok(node) => {
                    entry.rebuild = false;
                    // The error of an earlier build is over (MOO-354).
                    self.problems.remove(&slot);
                    events.push(RackEvent::Install { slot, node });
                    // Activation is when a plugin's latency is known.
                    events.push(RackEvent::LatencyChanged { slot });
                }
                Err(error) => {
                    entry.attempts = MAX_ATTEMPTS;
                    events.push(RackEvent::Failed { slot, error });
                }
            }
        }
        for event in &events {
            if let RackEvent::Failed { slot, error } = event {
                if self.entries.contains_key(slot) {
                    let generation = self.opener.as_mut().map_or(0, |opener| opener.refresh());
                    self.problems.insert(
                        *slot,
                        Problem {
                            error: error.clone(),
                            generation,
                        },
                    );
                }
            }
        }
        events
    }
}

/// Log the parameter ids a plugin's list gained or lost since the song last
/// saw it. Nothing is done to a lane or route whose id went: it is kept, and
/// found again if the id comes back.
fn log_param_changes(name: &str, was: &[PluginParamInfo], now: &[PluginParamInfo]) {
    let ids = |params: &[PluginParamInfo]| params.iter().map(|param| param.id).collect::<BTreeSet<u32>>();
    let (was, now) = (ids(was), ids(now));
    let added: Vec<u32> = now.difference(&was).copied().collect();
    let removed: Vec<u32> = was.difference(&now).copied().collect();
    if !added.is_empty() || !removed.is_empty() {
        log_warn!(
            "plugin",
            "{name}'s parameters changed since the song was saved: added {added:?}, removed {removed:?}"
        );
    }
}

impl crate::session::Session {
    /// A device's own latency for the compensation plan: the plugin rack's
    /// answer for a hosted plugin, the kind's for everything else.
    ///
    /// A plugin that is not hosted (missing, or not yet swapped in) plays as
    /// the pass-through placeholder, which adds nothing, so it counts zero.
    pub fn device_latency(&self, slot: &EffectSlotState) -> u32 {
        match slot.params {
            EffectParams::Plugin(plugin) => self.plugin_rack.latency_frames(plugin).unwrap_or(0),
            _ => slot.kind().latency_frames(),
        }
    }

    /// What finds and opens the plugins this session's songs name: the app
    /// sets the CLAP opener over the scanner's cache once, at startup.
    pub fn set_plugin_opener(&mut self, opener: Box<dyn PluginOpener>) {
        self.plugin_rack.set_opener(opener);
    }

    /// Why the plugin in `slot` is not heard, if it is not.
    pub fn plugin_problem(&self, slot: PluginSlotId) -> Option<HostError> {
        self.plugin_rack.problem(slot)
    }

    /// Every plugin slot a device on some chain names, or a channel plays
    /// as its source.
    pub fn named_plugin_slots(&self) -> BTreeSet<PluginSlotId> {
        let channels = self.channels.iter().map(|channel| &channel.effects);
        let buses = self.buses.iter().map(|bus| &bus.effects);
        let effects = channels
            .chain(buses)
            .flatten()
            .filter_map(|effect| match effect.params {
                EffectParams::Plugin(slot) => Some(slot),
                _ => None,
            });
        effects.chain(self.plugin_sources().map(|(_, slot)| slot)).collect()
    }

    /// Every channel whose source is a hosted plugin, with the slot it
    /// plays, by the channel's position (what the engine addresses).
    fn plugin_sources(&self) -> impl Iterator<Item = (u8, PluginSlotId)> + '_ {
        self.channels
            .iter()
            .enumerate()
            .filter_map(|(index, channel)| match channel.generator_params() {
                GeneratorParams::Plugin(slot) if slot.is_assigned() => {
                    Some((u8::try_from(index).ok()?, slot))
                }
                _ => None,
            })
    }

    /// The channel playing plugin `slot` as its source, if one is.
    fn plugin_source_channel(&self, slot: PluginSlotId) -> Option<u8> {
        self.plugin_sources()
            .find_map(|(channel, played)| (played == slot).then_some(channel))
    }

    fn plugin_config(handle: &impl CommandSink) -> AudioConfig {
        AudioConfig {
            sample_rate: handle.sample_rate(),
            max_frames: PLUGIN_MAX_FRAMES,
        }
    }

    /// The pump's once-a-tick plugin upkeep: drop instances whose processors
    /// have come back, open what the song names, act on what the plugins
    /// asked for, and swap a processor into every plugin device that lacks
    /// one.
    ///
    /// Run after `EngineHandle::poll`, which is where a reclaimed processor
    /// is dropped. A latency change needs nothing here:
    /// [`Self::sync_compensation`] derives the plan from
    /// [`Self::device_latency`] on the same tick and sends what moved.
    pub fn service_plugins(&mut self, handle: &mut impl CommandSink) {
        // Every 8 ms tick, so a song with no plugin in it pays one branch:
        // no walk, no allocation, no lock.
        if self.plugin_rack.is_empty() && self.plugins.is_empty() {
            return;
        }
        let named = self.named_plugin_slots();
        let config = Self::plugin_config(handle);
        for event in self.plugin_rack.service(&self.plugins, &named, config) {
            match event {
                RackEvent::Install { slot, node } => {
                    if self.misplaced(slot) {
                        // Dropped here: the instance is retired and not
                        // opened again until the catalogue changes.
                        drop(node);
                        continue;
                    }
                    if let Some(channel) = self.plugin_source_channel(slot) {
                        // Refused (the channel moved on), the processor comes
                        // back on the reclaim ring and the rack tries again.
                        let _ = handle.send_structural(StructuralCommand::HostSourceProcessor {
                            channel,
                            slot,
                            node: Some(node),
                        });
                        continue;
                    }
                    let align = IntegerDelay::new(node.dry_path_latency_frames()).map(Box::new);
                    // Refused or not found, the node is dropped here, which
                    // leaves the lifeline alone: the rack tries again, a
                    // bounded number of times.
                    let _ = self.replace_plugin_processor(slot, node, align, handle);
                    // A container holding it sized its rings for the
                    // placeholder; now the instance says what it adds
                    // (MOO-212, Effects').
                    self.resize_plugin_containers(slot, handle);
                }
                RackEvent::PullBack { slot } => self.pull_back(slot, handle),
                RackEvent::Opened { slot, params } => {
                    // What the plugin reports now replaces what the song last
                    // saw, without marking the song modified: opening a song
                    // is not an edit. The ids that came or went are logged; a
                    // lane or route on one that went is kept and reads as
                    // missing (step 03's rule).
                    if let Some(state) = self.plugins.get_mut(&slot) {
                        if !params.is_empty() && state.params != params {
                            log_param_changes(&state.plugin.name, &state.params, &params);
                            state.params = params;
                        }
                    }
                }
                // The channel's compensation follows by the plan each tick;
                // the rings inside a container are this chain's own.
                RackEvent::LatencyChanged { slot } => self.resize_plugin_containers(slot, handle),
                RackEvent::ParamsRescanned { slot, params } => {
                    // Replacing the list never touches an address: a lane
                    // whose id went away is kept and reads as missing, and
                    // reads as present again if a later rescan brings it
                    // back (Adam, 2026-09-23, MOO-74).
                    if let Some(state) = self.plugins.get_mut(&slot) {
                        if state.params != params {
                            state.params = params;
                            self.dirty = true;
                        }
                    }
                }
                RackEvent::Failed { slot, error } => {
                    let name = self
                        .plugins
                        .get(&slot)
                        .map_or_else(|| format!("slot {}", slot.0), |state| state.plugin.name.clone());
                    log_warn!("plugin", "{name}: {error}");
                }
            }
        }
    }

    /// Insert the plugin `plugin` as a new device before `insert_before` on
    /// the chain the rack is pointed at: [`Self::place_plugin_effect`] at
    /// [`EffectPlace::Before`], the way `insert_effect_at` is `place_effect`
    /// there. An index past a container's run lands after the container;
    /// inside a box, name the box with `place_plugin_effect`.
    pub fn insert_plugin_effect(
        &mut self,
        plugin: PluginRef,
        insert_before: usize,
        handle: &mut impl CommandSink,
    ) -> Option<EffectInserted> {
        self.place_plugin_effect(plugin, EffectPlace::Before(insert_before), handle)
    }

    /// Put the plugin `plugin` at `place` on the chain the rack is pointed
    /// at as a new device, open it, and install its processor -- or the
    /// placeholder, when it cannot be opened, with the reason kept for
    /// [`Self::plugin_problem`] (MOO-339).
    ///
    /// The row goes where `place_effect` puts any device, so **Plugin…** on
    /// the join at a Chain's end ([`EffectPlace::LastIn`]) or inside an
    /// empty box ([`EffectPlace::FirstIn`]) lands inside it, as a native
    /// device from the same join does. Mirrored the same way too: installed
    /// at the chain's tail and moved into place. It does not publish
    /// container spans; the interface's caller does, after it.
    ///
    /// `None`, with the slot it minted let go again, when `place_effect`
    /// refuses: the chain is full, `place` names no row, or it names a box
    /// that cannot take a device there (a layer's end is a new branch).
    pub fn place_plugin_effect(
        &mut self,
        plugin: PluginRef,
        place: EffectPlace,
        handle: &mut impl CommandSink,
    ) -> Option<EffectInserted> {
        let slot = mint_plugin_slot(
            &mut self.plugins,
            &mut self.next_plugin_slot,
            PluginSlotState::new(plugin.clone()),
        );
        let mut effect = EffectSlotState::of_kind(EffectKind::Plugin);
        effect.params = EffectParams::Plugin(slot);
        let Some(inserted) = self.place_effect(effect, place) else {
            self.plugins.remove(&slot);
            return None;
        };
        let node = self.host_new_plugin(slot, &plugin, &PluginState::default(), &*handle);
        let align = IntegerDelay::new(node.dry_path_latency_frames()).map(Box::new);
        let _ = handle.send_structural(StructuralCommand::InstallEffect {
            target: inserted.target,
            slot: inserted.tail as u8,
            kind: EffectKind::Plugin,
            resource_key: Some(u64::from(slot.0)),
            node,
            align,
            analyzer: Box::new(SpectrumAnalyzer::new()),
            state: Box::new(EffectSlot::for_device(inserted.device)),
        });
        if inserted.slot != inserted.tail {
            let _ = handle.send(EngineCommand::MoveEffect {
                target: inserted.target,
                from: inserted.tail as u8,
                to: inserted.slot as u8,
            });
        }
        self.dirty = true;
        Some(inserted)
    }

    /// Open `plugin` with `state` into the freshly minted `slot` and return
    /// the processor for its device to install -- or the placeholder, when it
    /// cannot be opened, with the reason kept for [`Self::plugin_problem`].
    ///
    /// The second half of [`Self::insert_plugin_effect`], and of an effect
    /// preset's load onto a plugin device (MOO-222, Effects'
    /// `load_plugin_effect_preset`), which opens its new slot with the
    /// preset's state. `slot` must already be in `plugins` and not hosted.
    pub(crate) fn host_new_plugin(
        &mut self,
        slot: PluginSlotId,
        plugin: &PluginRef,
        state: &PluginState,
        handle: &impl CommandSink,
    ) -> Box<dyn AudioNode + Send> {
        let config = Self::plugin_config(handle);
        let opened = self
            .plugin_rack
            .open(plugin, state, config)
            .and_then(|instance| {
                let params = instance.params().to_vec();
                let node = self.plugin_rack.insert(slot, instance)?;
                Ok((node, params))
            });
        match opened {
            Ok((node, params)) if !self.misplaced(slot) => {
                if let Some(state) = self.plugins.get_mut(&slot) {
                    state.params = params;
                }
                node
            }
            // An instrument: refused, and retired by `misplaced`.
            Ok(_) => Box::new(PluginPlaceholder::new(slot)),
            Err(error) => {
                log_warn!("plugin", "{}: {error}", plugin.name);
                self.plugin_rack.record_problem(slot, error);
                Box::new(PluginPlaceholder::new(slot))
            }
        }
    }

    /// Whether a plugin finished an edit of its own that the song does not
    /// have yet. The pump asks once a tick, and when it is so takes a
    /// snapshot, calls [`Self::capture_plugin_edits`], and records the two
    /// as one undo step (MOO-82).
    pub fn plugin_edits_pending(&self) -> bool {
        self.plugin_rack.has_edits()
    }

    /// Capture the state of every plugin that finished an edit of its own
    /// -- a gesture in its GUI, a quiet run of values it moved itself, or a
    /// value the session sent it -- into the song. Returns whether the song
    /// changed, and marks it modified when it did.
    ///
    /// **Why this is its own undo step.** Undo installs a whole snapshot,
    /// and a snapshot carries each plugin's state. A plugin edit that went
    /// into the song without a step of its own would sit inside whichever
    /// step came next, and undoing an *earlier* one would install that
    /// step's older state, reopening the plugin without the edit. As its own
    /// step, an undo takes the plugin's edit back first, and an unrelated
    /// edit's undo leaves it alone: the snapshot either side of that one
    /// carries the same state, so the instance is kept.
    pub fn capture_plugin_edits(&mut self) -> bool {
        let mut changed = false;
        for slot in self.plugin_rack.take_edits() {
            self.plugin_rack.accept_state(slot);
            changed |= self.capture_plugin_state(slot);
        }
        if changed {
            self.dirty = true;
        }
        changed
    }

    /// Save the plugin in `slot`'s state into the song, if it is hosted.
    /// Returns whether the song's copy changed. A state the plugin refused
    /// on opening is kept as the song has it, and a plugin that fails to
    /// save keeps its last good state: neither is ever overwritten by less.
    fn capture_plugin_state(&mut self, slot: PluginSlotId) -> bool {
        if self.plugin_rack.refused_state(slot) {
            return false;
        }
        let Some(instance) = self.plugin_rack.instance_mut(slot) else {
            return false;
        };
        let state = match instance.save_state() {
            Ok(state) => state,
            Err(error) => {
                log_warn!("plugin", "slot {}: {error}", slot.0);
                return false;
            }
        };
        match self.plugins.get_mut(&slot) {
            Some(saved) if saved.state.0 != state => {
                saved.state = PluginStateText(state);
                true
            }
            _ => false,
        }
    }

    /// Save every hosted plugin's state into the song, for a save or an
    /// export: what the plugin holds now, not what it held when the song
    /// last asked. A plugin that is not hosted keeps the state the song
    /// already has, byte for byte. Returns whether any copy changed.
    pub fn capture_plugin_states(&mut self) -> bool {
        let slots: Vec<PluginSlotId> = self.plugin_rack.live_slots().collect();
        let mut changed = false;
        for slot in slots {
            changed |= self.capture_plugin_state(slot);
        }
        changed
    }

    /// A processor of every plugin the song names, for an export to render
    /// with (`OfflineRenderer::render_with_plugins`) at `sample_rate`.
    ///
    /// The live instances' processors are in the engine, and a plugin may
    /// have one processor per instance, so each of these is a **second
    /// instance**, opened here on the control thread with the live one's
    /// state. The instances wait in the rack's graveyard and go once the
    /// export has dropped their processors. A plugin that cannot be opened
    /// is left out, and the export plays its placeholder, as playback does.
    pub fn export_plugin_processors(
        &mut self,
        sample_rate: u32,
    ) -> BTreeMap<PluginSlotId, Box<dyn AudioNode + Send>> {
        self.capture_plugin_states();
        let config = AudioConfig {
            sample_rate,
            max_frames: PLUGIN_MAX_FRAMES,
        };
        let mut processors = BTreeMap::new();
        for slot in self.named_plugin_slots() {
            let Some(state) = self.plugins.get(&slot) else {
                continue;
            };
            let (plugin, saved) = (state.plugin.clone(), state.state.0.clone());
            let built = self.plugin_rack.open(&plugin, &saved, config).and_then(|mut instance| {
                let lifeline = Lifeline::new();
                let node = instance.build_processor(lifeline.tie())?;
                Ok((instance, lifeline, node))
            });
            match built {
                Ok((instance, lifeline, node)) => {
                    self.plugin_rack.bury(instance, lifeline);
                    processors.insert(slot, node);
                }
                Err(error) => {
                    log_warn!("export", "{} is exported as a pass-through: {error}", plugin.name);
                }
            }
        }
        processors
    }

    /// Begin retiring every hosted plugin, for a quit: every GUI is
    /// destroyed first (reported by [`Self::drain_plugin_gui_events`] as
    /// [`PluginGuiEvent::Closed`]), then every processor out is pulled back
    /// by swapping the placeholder in. The caller then polls the engine and
    /// calls [`Self::collect_plugins`] until [`Self::plugins_retired`], or
    /// its bounded wait runs out.
    pub fn close_plugins(&mut self, handle: &mut impl CommandSink) {
        for slot in self.plugin_rack.close() {
            self.pull_back(slot, handle);
        }
    }

    /// Which way plugin `slot`'s GUI would open (step 11): embedded where
    /// the plugin can, floating where it only offers that. The error is the
    /// badge's text when it has no window at all.
    pub fn plugin_gui_kind(&mut self, slot: PluginSlotId) -> Result<GuiConfig, GuiError> {
        self.plugin_rack.gui_placement_kind(slot)
    }

    /// The title a plugin's window gets: the plugin's name and the track
    /// its device is on (policy 2).
    pub fn plugin_gui_title(&self, slot: PluginSlotId) -> Option<String> {
        let plugin = &self.plugins.get(&slot)?.plugin.name;
        let wanted = EffectParams::Plugin(slot);
        let channels = self.channels.iter().map(|channel| (&channel.name, &channel.effects));
        let buses = self.buses.iter().map(|bus| (&bus.bus.name, &bus.effects));
        let track = channels
            .chain(buses)
            .find(|(_, effects)| effects.iter().any(|effect| effect.params == wanted))
            .map(|(name, _)| name)
            .or_else(|| {
                self.plugin_source_channel(slot)
                    .and_then(|channel| self.channels.get(usize::from(channel)))
                    .map(|channel| &channel.name)
            });
        Some(match track {
            Some(track) if !track.is_empty() => format!("{plugin} - {track}"),
            _ => plugin.clone(),
        })
    }

    /// Open plugin `slot`'s GUI in the window `open` names, and show it.
    /// The processor is not touched. The caller sizes its window from what
    /// comes back.
    pub fn open_plugin_gui(
        &mut self,
        slot: PluginSlotId,
        open: &PluginGuiOpen,
    ) -> Result<PluginGuiOpened, GuiError> {
        self.plugin_rack.open_gui(slot, open)
    }

    /// Hide and destroy plugin `slot`'s GUI. The processor keeps running
    /// and is never reclaimed for this. Returns whether it was open; once it
    /// returns, the caller's window may be destroyed.
    pub fn close_plugin_gui(&mut self, slot: PluginSlotId) -> bool {
        self.plugin_rack.close_gui(slot)
    }

    pub fn plugin_gui_is_open(&mut self, slot: PluginSlotId) -> bool {
        self.plugin_rack.gui_is_open(slot)
    }

    /// Show plugin `slot`'s GUI (its window was mapped again).
    pub fn show_plugin_gui(&mut self, slot: PluginSlotId) -> Result<(), GuiError> {
        self.plugin_rack.show_gui(slot)
    }

    /// Hide plugin `slot`'s GUI without closing it.
    pub fn hide_plugin_gui(&mut self, slot: PluginSlotId) -> Result<(), GuiError> {
        self.plugin_rack.hide_gui(slot)
    }

    /// The user resized plugin `slot`'s window to `size`. Returns the size
    /// the plugin took (`can_resize`, then `adjust_size`, then `set_size`),
    /// which the window should snap to.
    pub fn resize_plugin_gui(&mut self, slot: PluginSlotId, size: GuiSize) -> Result<GuiSize, GuiError> {
        self.plugin_rack.resize_gui(slot, size)
    }

    /// The window's scale factor changed (from the Slint window's).
    pub fn set_plugin_gui_scale(&mut self, slot: PluginSlotId, scale: f64) -> Result<(), GuiError> {
        self.plugin_rack.set_gui_scale(slot, scale)
    }

    /// The pump's once-a-tick call for plugin event loops: fire every due
    /// timer and every ready fd of every hosted plugin (policy 1). Never
    /// waits: timers are compared with the clock and fds are polled with a
    /// zero timeout. A song with no plugin pays one branch.
    pub fn service_plugin_io(&mut self) -> IoActivity {
        self.service_plugin_io_at(std::time::Instant::now())
    }

    /// [`Self::service_plugin_io`] as of `now`.
    pub fn service_plugin_io_at(&mut self, now: std::time::Instant) -> IoActivity {
        if self.plugin_rack.is_empty() {
            return IoActivity::default();
        }
        self.plugin_rack.service_io(now)
    }

    /// Everything the window side has to do about plugin GUIs since the last
    /// call, once a pump tick: the plugins' own requests (a resize, a show,
    /// a hide, a close, which is carried out here before it is reported),
    /// and every GUI the session closed by itself because its device was
    /// removed, its song closed, or the app is quitting.
    pub fn drain_plugin_gui_events(&mut self) -> Vec<(PluginSlotId, PluginGuiEvent)> {
        if self.plugin_rack.is_empty() && !self.plugin_rack.has_gui_events() {
            return Vec::new();
        }
        self.plugin_rack.drain_gui_events()
    }

    /// Whether the plugin hosted in `slot` is in a place it cannot play: an
    /// instrument on a chain, or an effect as a channel's source (MOO-85).
    /// When it is, the instance is retired and the reason kept for
    /// [`Self::plugin_problem`]; the device plays its placeholder, the
    /// channel silence, and the song keeps the slot as it is.
    fn misplaced(&mut self, slot: PluginSlotId) -> bool {
        let Some((effect, source)) = self
            .plugin_rack
            .instance(slot)
            .map(|instance| (instance.fits_effect(), instance.fits_source()))
        else {
            return false;
        };
        let error = match (self.plugin_source_channel(slot).is_some(), effect, source) {
            (true, _, false) => HostError::Incompatible(
                "not an instrument: a channel's source is a plugin that declares itself one, takes notes and has one output"
                    .into(),
            ),
            (false, false, _) => HostError::Incompatible(
                "an instrument: it plays as a channel's source, not in a chain".into(),
            ),
            _ => return false,
        };
        self.plugin_rack.remove(slot);
        self.plugin_rack.record_problem(slot, error);
        true
    }

    /// Take plugin `slot`'s processor back from the engine: the placeholder
    /// into its device's place, or, for a channel's source, nothing -- the
    /// source goes silent, as a missing instrument is.
    fn pull_back(&mut self, slot: PluginSlotId, handle: &mut impl CommandSink) {
        if let Some(channel) = self.plugin_source_channel(slot) {
            let _ = handle.send_structural(StructuralCommand::HostSourceProcessor {
                channel,
                slot,
                node: None,
            });
            return;
        }
        let (placeholder, align) = self.pull_back_placeholder(slot);
        let _ = self.replace_plugin_processor(slot, placeholder, align, handle);
    }

    /// Make channel `index`'s source the plugin `plugin`, open it, and
    /// install its processor -- or a silent source, when it cannot be
    /// opened, with the reason kept for [`Self::plugin_problem`]. Returns the
    /// slot the song now keeps it in.
    ///
    /// The session path of step 09 (MOO-84), as [`Self::insert_plugin_effect`]
    /// is step 06's: there is no window path until step 08's browser
    /// (MOO-83). The channel's former source goes as a source change does
    /// (`reset_channel_source`: a name nobody typed follows the device, the
    /// sample state is cleared). A slot the previous plugin source held stays
    /// in the song's table, unnamed, as a removed plugin effect's does.
    pub fn set_plugin_source(
        &mut self,
        index: usize,
        plugin: PluginRef,
        handle: &mut impl CommandSink,
    ) -> Option<PluginSlotId> {
        let channel = u8::try_from(index).ok()?;
        self.channels.get(index)?;
        let slot = mint_plugin_slot(
            &mut self.plugins,
            &mut self.next_plugin_slot,
            PluginSlotState::new(plugin.clone()),
        );
        self.reset_channel_source(index, DeviceKind::Plugin);
        self.channels[index].set_generator(GeneratorParams::Plugin(slot));
        let config = Self::plugin_config(handle);
        let opened = self
            .plugin_rack
            .open(&plugin, &PluginState::default(), config)
            .and_then(|instance| {
                let params = instance.params().to_vec();
                let node = self.plugin_rack.insert(slot, instance)?;
                Ok((node, params))
            });
        let source = match opened {
            Ok((node, params)) if !self.misplaced(slot) => {
                if let Some(state) = self.plugins.get_mut(&slot) {
                    state.params = params;
                }
                HostedSource::with_processor(slot, node)
            }
            // An effect: refused, and retired by `misplaced`.
            Ok(_) => HostedSource::new(slot),
            Err(error) => {
                log_warn!("plugin", "{}: {error}", plugin.name);
                self.plugin_rack.record_problem(slot, error);
                HostedSource::new(slot)
            }
        };
        // With the id `reset_channel_source` just minted, so the engine drives
        // this instrument by what names it, and no longer the replaced one's
        // lanes and routes (MOO-314).
        let _ = handle.send_structural(StructuralCommand::InstallSource {
            channel,
            node: Box::new(source),
            device: self.channels[index].source_device,
        });
        self.dirty = true;
        Some(slot)
    }

    /// The placeholder that pulls a running processor back, and its dry
    /// ring: a pass-through as late as the plugin it replaces, which is the
    /// latency the chain is compensated for until the next processor reports
    /// its own (`Self::device_latency` reads the same instance). A plain
    /// placeholder would move the channel earlier by that much and back
    /// again, and the engine's fade could not hide a jump in time (MOO-213).
    fn pull_back_placeholder(
        &self,
        slot: PluginSlotId,
    ) -> (Box<dyn AudioNode + Send>, Option<Box<IntegerDelay>>) {
        let latency = self.plugin_rack.latency_frames(slot).unwrap_or(0);
        let align = IntegerDelay::new(latency).map(Box::new);
        (Box::new(PluginPlaceholder::with_latency(slot, latency)), align)
    }

    /// Drop the retired instances whose processors have come back.
    pub fn collect_plugins(&mut self) -> usize {
        self.plugin_rack.collect()
    }

    /// Whether every hosted plugin has been retired and dropped.
    pub fn plugins_retired(&self) -> bool {
        self.plugin_rack.is_empty()
    }

    /// The end of a quit whose bounded wait ran out: give up on the
    /// instances still held without dropping them, and return how many.
    /// Dropping one whose processor may still be running on the audio
    /// thread is worse than leaking it at exit.
    pub fn leak_plugins(&mut self) -> usize {
        self.plugin_rack.leak_remaining()
    }

    /// Swap `node` into the device that runs plugin `slot`, wherever it is.
    ///
    /// Keyed by the slot, not by the device's position: `ReplaceEffect`
    /// checks the occupant's kind and resource key, and a plugin device's key
    /// is its slot id, so a replacement that arrives after the device was
    /// removed or moved away finds nothing to replace and does nothing.
    fn replace_plugin_processor(
        &mut self,
        slot: PluginSlotId,
        node: Box<dyn AudioNode + Send>,
        align: Option<Box<IntegerDelay>>,
        handle: &mut impl CommandSink,
    ) -> bool {
        let wanted = EffectParams::Plugin(slot);
        let channels = self
            .channels
            .iter()
            .enumerate()
            .map(|(index, channel)| (EffectTarget::Channel(index as u8), &channel.effects));
        let buses = self
            .buses
            .iter()
            .enumerate()
            .map(|(index, bus)| (EffectTarget::Bus(index as u8), &bus.effects));
        let Some((target, position)) = channels.chain(buses).find_map(|(target, effects)| {
            effects
                .iter()
                .position(|effect| effect.params == wanted)
                .map(|position| (target, position))
        }) else {
            return false;
        };
        let Ok(position) = u8::try_from(position) else {
            return false;
        };
        let key = u64::from(slot.0);
        handle.send_structural(StructuralCommand::ReplaceEffect {
            target,
            slot: position,
            expected_kind: EffectKind::Plugin,
            expected_resource_key: key,
            resource_key: key,
            node,
            align,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
    use std::sync::Arc;

    use mooloop_core::{PluginFormat, PluginRef, PluginState};
    use mooloop_dsp::{EventList, ProcessContext, StereoBus};
    use mooloop_plugin_host::RequestFlags;

    use super::*;

    /// What a test can see of a fake plugin after handing it to the rack.
    #[derive(Default)]
    pub(crate) struct FakeProbe {
        pub requests: RequestFlags,
        pub latency: AtomicU32,
        pub builds: AtomicUsize,
        pub opens: AtomicUsize,
        pub instances_dropped: AtomicUsize,
        pub processors_dropped: AtomicUsize,
        /// The opener refuses while this is set.
        pub missing: AtomicBool,
        /// The opener's catalogue generation.
        pub generation: AtomicU64,
        /// The rate the last processor was built for.
        pub built_rate: AtomicU32,
        /// A processor cannot be built above this rate; zero for no limit.
        pub builds_fail_above: AtomicU32,
        /// It says it is an instrument (MOO-85): only a source may host it.
        pub instrument: AtomicBool,
        /// It has a GUI (step 11).
        pub has_gui: AtomicBool,
        /// Its GUI refuses to embed.
        pub refuses_parent: AtomicBool,
        pub gui_creates: AtomicUsize,
        pub gui_destroys: AtomicUsize,
        /// What its GUI asks of the window, until the rack drains it.
        pub gui_requests: std::sync::Mutex<Vec<GuiRequest>>,
        /// The order its GUI, processors and instances went in.
        pub order: std::sync::Mutex<Vec<&'static str>>,
    }

    impl FakeProbe {
        fn happened(&self, what: &'static str) {
            self.order.lock().unwrap().push(what);
        }

        pub(crate) fn order(&self) -> Vec<&'static str> {
            self.order.lock().unwrap().clone()
        }
    }

    /// The rack's test double: a plugin with no library behind it.
    pub(crate) struct FakeInstance {
        plugin: PluginRef,
        params: Vec<PluginParamInfo>,
        config: AudioConfig,
        probe: Arc<FakeProbe>,
        gui_open: Option<GuiConfig>,
        gui_parented: bool,
        gui_visible: bool,
    }

    pub(crate) fn fake_ref() -> PluginRef {
        PluginRef {
            format: PluginFormat::Clap,
            id: "org.mooloop.fake".to_owned(),
            name: "Fake".to_owned(),
            vendor: String::new(),
            version: String::new(),
        }
    }

    impl FakeInstance {
        pub(crate) fn new(probe: Arc<FakeProbe>) -> Box<Self> {
            Box::new(Self {
                plugin: fake_ref(),
                params: Vec::new(),
                config: AudioConfig {
                    sample_rate: 48_000,
                    max_frames: PLUGIN_MAX_FRAMES,
                },
                probe,
                gui_open: None,
                gui_parented: false,
                gui_visible: false,
            })
        }
    }

    impl Drop for FakeInstance {
        fn drop(&mut self) {
            if self.gui_open.is_some() {
                self.probe.happened("instance dropped with its GUI open");
            }
            self.probe.happened("instance dropped");
            self.probe.instances_dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl HostedGui for FakeInstance {
        fn is_api_supported(&mut self, _config: GuiConfig) -> bool {
            true
        }
        fn preferred_api(&mut self) -> Option<GuiConfig> {
            Some(GuiConfig::native_embedded())
        }
        fn open_config(&self) -> Option<GuiConfig> {
            self.gui_open
        }
        fn is_visible(&self) -> bool {
            self.gui_visible
        }
        fn create(&mut self, config: GuiConfig) -> Result<(), GuiError> {
            if self.gui_open.is_some() {
                return Err(GuiError::AlreadyOpen);
            }
            self.gui_open = Some(config);
            self.gui_parented = false;
            self.probe.gui_creates.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn set_scale(&mut self, _scale: f64) -> Result<(), GuiError> {
            Ok(())
        }
        fn size(&mut self) -> Option<GuiSize> {
            self.gui_open.map(|_| GuiSize {
                width: 300,
                height: 200,
            })
        }
        fn can_resize(&mut self) -> bool {
            false
        }
        fn adjust_size(&mut self, size: GuiSize) -> Option<GuiSize> {
            Some(size)
        }
        fn set_size(&mut self, _size: GuiSize) -> Result<(), GuiError> {
            Err(GuiError::Refused("take that size"))
        }
        fn set_parent(&mut self, _window: NativeWindow) -> Result<(), GuiError> {
            if self.probe.refuses_parent.load(Ordering::SeqCst) {
                return Err(GuiError::Refused("embed in the window"));
            }
            self.gui_parented = true;
            Ok(())
        }
        fn set_transient(&mut self, _window: NativeWindow) -> Result<(), GuiError> {
            Ok(())
        }
        fn suggest_title(&mut self, _title: &str) {}
        fn show(&mut self) -> Result<(), GuiError> {
            match self.gui_open {
                None => Err(GuiError::NotOpen),
                Some(config) if !config.floating && !self.gui_parented => Err(GuiError::Refused("show")),
                Some(_) => {
                    self.gui_visible = true;
                    Ok(())
                }
            }
        }
        fn hide(&mut self) -> Result<(), GuiError> {
            self.gui_open.ok_or(GuiError::NotOpen)?;
            self.gui_visible = false;
            Ok(())
        }
        fn destroy(&mut self) {
            if self.gui_open.take().is_some() {
                self.gui_visible = false;
                self.probe.gui_destroys.fetch_add(1, Ordering::SeqCst);
                self.probe.happened("gui destroyed");
            }
        }
        fn take_requests(&mut self, sink: &mut dyn FnMut(GuiRequest)) {
            let requests = std::mem::take(&mut *self.probe.gui_requests.lock().unwrap());
            if self.gui_open.is_some() {
                requests.into_iter().for_each(sink);
            }
        }
    }

    struct FakeProcessor {
        _lifeline: Lifeline,
        probe: Arc<FakeProbe>,
    }

    impl Drop for FakeProcessor {
        fn drop(&mut self) {
            self.probe.happened("processor dropped");
            self.probe.processors_dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl AudioNode for FakeProcessor {
        fn process(
            &mut self,
            _ctx: &ProcessContext,
            _bus: &mut StereoBus,
            _events_in: &EventList,
            _events_out: Option<&mut EventList>,
        ) {
        }
    }

    impl HostedInstance for FakeInstance {
        fn plugin(&self) -> &PluginRef {
            &self.plugin
        }
        fn params(&self) -> &[PluginParamInfo] {
            &self.params
        }
        fn latency_frames(&self) -> u32 {
            self.probe.latency.load(Ordering::SeqCst)
        }
        fn save_state(&mut self) -> Result<PluginState, HostError> {
            Ok(PluginState::default())
        }
        fn load_state(&mut self, _state: &PluginState) -> Result<(), HostError> {
            Ok(())
        }
        fn value_text(&mut self, _id: u32, _value: f64) -> Option<String> {
            None
        }
        fn take_requests(&self) -> Requests {
            self.probe.requests.take()
        }
        fn on_main_thread(&mut self) {}
        fn fits_effect(&self) -> bool {
            !self.probe.instrument.load(Ordering::SeqCst)
        }
        fn fits_source(&self) -> bool {
            self.probe.instrument.load(Ordering::SeqCst)
        }
        fn set_audio_config(&mut self, config: AudioConfig) {
            self.config = config;
        }
        fn gui(&mut self) -> Option<&mut dyn HostedGui> {
            if self.probe.has_gui.load(Ordering::SeqCst) {
                Some(self)
            } else {
                None
            }
        }
        fn build_processor(
            &mut self,
            lifeline: Lifeline,
        ) -> Result<Box<dyn AudioNode + Send>, HostError> {
            self.probe.builds.fetch_add(1, Ordering::SeqCst);
            let limit = self.probe.builds_fail_above.load(Ordering::SeqCst);
            if limit > 0 && self.config.sample_rate > limit {
                return Err(HostError::Plugin(format!("it cannot activate above {limit} Hz")));
            }
            self.probe.built_rate.store(self.config.sample_rate, Ordering::SeqCst);
            Ok(Box::new(FakeProcessor {
                _lifeline: lifeline,
                probe: Arc::clone(&self.probe),
            }))
        }
    }

    /// Opens a [`FakeInstance`] for anything, unless the probe says the
    /// plugin is missing.
    pub(crate) struct FakeOpener(pub Arc<FakeProbe>);

    impl PluginOpener for FakeOpener {
        fn open(
            &mut self,
            _plugin: &PluginRef,
            _state: &PluginState,
            config: AudioConfig,
        ) -> Result<Box<dyn HostedInstance>, HostError> {
            self.0.opens.fetch_add(1, Ordering::SeqCst);
            if self.0.missing.load(Ordering::SeqCst) {
                return Err(HostError::Missing);
            }
            let mut instance = FakeInstance::new(Arc::clone(&self.0));
            instance.config = config;
            Ok(instance)
        }

        fn refresh(&mut self) -> u64 {
            self.0.generation.load(Ordering::SeqCst)
        }
    }

    fn config() -> AudioConfig {
        AudioConfig {
            sample_rate: 48_000,
            max_frames: PLUGIN_MAX_FRAMES,
        }
    }

    fn names(events: &[RackEvent]) -> Vec<String> {
        events.iter().map(|event| format!("{event:?}")).collect()
    }

    /// Blocker 5's order: the instance outlives its processor, and is
    /// dropped as soon as the processor is gone rather than before.
    #[test]
    fn a_removed_instance_is_dropped_only_after_its_processor() {
        let probe = Arc::new(FakeProbe::default());
        let mut rack = PluginRack::new();
        let slot = PluginSlotId(0);
        let processor = rack.insert(slot, FakeInstance::new(Arc::clone(&probe))).unwrap();

        assert!(rack.remove(slot));
        assert!(rack.instance(slot).is_none(), "a dying entry is not live");
        assert_eq!(rack.collect(), 0, "the processor is still on the audio thread");
        assert_eq!(probe.instances_dropped.load(Ordering::SeqCst), 0);

        // What `EngineHandle::poll` does when the reclaim ring hands it back.
        drop(processor);
        assert_eq!(probe.processors_dropped.load(Ordering::SeqCst), 1);
        assert_eq!(rack.collect(), 1);
        assert_eq!(probe.instances_dropped.load(Ordering::SeqCst), 1);
        assert!(rack.is_empty());
    }

    /// A song closing retires everything, says which processors are still
    /// out, and drops nothing until they are back.
    #[test]
    fn closing_waits_for_every_processor() {
        let probe = Arc::new(FakeProbe::default());
        let mut rack = PluginRack::new();
        let first = rack.insert(PluginSlotId(0), FakeInstance::new(Arc::clone(&probe))).unwrap();
        let second = rack.insert(PluginSlotId(1), FakeInstance::new(Arc::clone(&probe))).unwrap();
        assert_eq!(rack.close(), [PluginSlotId(0), PluginSlotId(1)]);
        assert_eq!(rack.dying(), 2);
        drop(first);
        assert_eq!(rack.collect(), 1);
        drop(second);
        assert_eq!(rack.collect(), 1);
        assert_eq!(probe.instances_dropped.load(Ordering::SeqCst), 2);
    }

    /// A restart the plugin asked for after its slot was removed builds
    /// nothing and reinstalls nothing, and is not left to fire later.
    #[test]
    fn a_restart_requested_after_removal_does_nothing() {
        let probe = Arc::new(FakeProbe::default());
        let mut rack = PluginRack::new();
        let slot = PluginSlotId(3);
        let _processor = rack.insert(slot, FakeInstance::new(Arc::clone(&probe))).unwrap();
        rack.remove(slot);
        probe.requests.raise(Requests::RESTART | Requests::LATENCY_CHANGED);
        let named = BTreeSet::from([slot]);
        assert!(rack.service(&PluginSlots::new(), &named, config()).is_empty());
        assert_eq!(probe.builds.load(Ordering::SeqCst), 1, "only the first build");
        assert!(probe.requests.take().is_empty(), "the request was drained");
    }

    /// CLAP allows one processor per instance: a restart pulls the running
    /// one back, and the next is built only once it has been dropped.
    #[test]
    fn a_restart_pulls_the_processor_back_and_builds_the_next_once_it_is_gone() {
        let probe = Arc::new(FakeProbe::default());
        let mut rack = PluginRack::new();
        let slot = PluginSlotId(1);
        let named = BTreeSet::from([slot]);
        let slots = PluginSlots::new();
        let first = rack.insert(slot, FakeInstance::new(Arc::clone(&probe))).unwrap();
        assert!(rack.service(&slots, &named, config()).is_empty(), "the first is out and current");

        probe.requests.raise(Requests::RESTART | Requests::PARAMS_RESCAN | Requests::STATE_DIRTY);
        assert_eq!(
            names(&rack.service(&slots, &named, config())),
            ["ParamsRescanned(1, 0 params)", "PullBack(1)"]
        );
        // `mark_dirty` is a finished edit for the pump to record (MOO-82),
        // not an event the session acts on by itself.
        assert!(rack.has_edits());
        assert_eq!(rack.take_edits(), BTreeSet::from([slot]));
        assert!(rack.service(&slots, &named, config()).is_empty(), "one pull-back, not one a tick");
        assert_eq!(probe.builds.load(Ordering::SeqCst), 1, "nothing built while the first is out");

        drop(first);
        let mut events = rack.service(&slots, &named, config());
        assert_eq!(names(&events), ["Install(1)", "LatencyChanged(1)"]);
        assert_eq!(probe.builds.load(Ordering::SeqCst), 2);
        let RackEvent::Install { node, .. } = events.remove(0) else {
            panic!("the first event is the install");
        };
        // The new processor is tied too.
        rack.remove(slot);
        assert_eq!(rack.collect(), 0);
        drop(node);
        assert_eq!(rack.collect(), 1);
    }

    /// A processor the engine keeps refusing is not rebuilt every tick.
    #[test]
    fn a_processor_that_never_arrives_is_built_a_bounded_number_of_times() {
        let probe = Arc::new(FakeProbe::default());
        let mut rack = PluginRack::new();
        rack.set_opener(Box::new(FakeOpener(Arc::clone(&probe))));
        let slot = PluginSlotId(0);
        let mut slots = PluginSlots::new();
        slots.insert(slot, PluginSlotState::new(fake_ref()));
        let named = BTreeSet::from([slot]);
        for _ in 0..20 {
            // Every node dropped on the spot, as a refused send would be.
            drop(rack.service(&slots, &named, config()));
        }
        assert_eq!(probe.opens.load(Ordering::SeqCst), 1);
        assert_eq!(probe.builds.load(Ordering::SeqCst), usize::from(MAX_ATTEMPTS));
    }

    /// **A build that failed is tried again when something changed**
    /// (MOO-354): a new sample rate, or a restart the plugin asks for.
    ///
    /// The plugin cannot activate above 96 kHz. It fails at 192 kHz until it
    /// has used its attempts, and stays a placeholder; the interface then
    /// drops to 48 kHz, and the plugin has to be built and its problem gone.
    #[test]
    fn a_processor_that_failed_to_build_is_tried_again_at_a_new_rate() {
        let probe = Arc::new(FakeProbe::default());
        probe.builds_fail_above.store(96_000, Ordering::SeqCst);
        let mut rack = PluginRack::new();
        rack.set_opener(Box::new(FakeOpener(Arc::clone(&probe))));
        let slot = PluginSlotId(0);
        let mut slots = PluginSlots::new();
        slots.insert(slot, PluginSlotState::new(fake_ref()));
        let named = BTreeSet::from([slot]);
        let at = |sample_rate| AudioConfig {
            sample_rate,
            max_frames: PLUGIN_MAX_FRAMES,
        };

        let events = names(&rack.service(&slots, &named, at(192_000)));
        assert!(events.iter().any(|event| event.starts_with("Failed")), "{events:?}");
        for _ in 0..5 {
            drop(rack.service(&slots, &named, at(192_000)));
        }
        assert!(rack.problem(slot).is_some());

        let events = names(&rack.service(&slots, &named, at(48_000)));
        assert_eq!(events, ["Install(0)", "LatencyChanged(0)"], "the new rate was not tried");
        assert_eq!(rack.problem(slot), None, "the old error outlived the build that fixed it");
        assert_eq!(probe.built_rate.load(Ordering::SeqCst), 48_000);
    }

    /// A restart the plugin asks for after a failed build is a reason to try
    /// again (MOO-354).
    #[test]
    fn a_restart_request_tries_a_failed_build_again() {
        let probe = Arc::new(FakeProbe::default());
        probe.builds_fail_above.store(96_000, Ordering::SeqCst);
        let mut rack = PluginRack::new();
        rack.set_opener(Box::new(FakeOpener(Arc::clone(&probe))));
        let slot = PluginSlotId(0);
        let named = BTreeSet::from([slot]);
        let mut slots = PluginSlots::new();
        slots.insert(slot, PluginSlotState::new(fake_ref()));
        let high = AudioConfig {
            sample_rate: 192_000,
            max_frames: PLUGIN_MAX_FRAMES,
        };
        // Every attempt refused, as a plugin capped at 96 kHz does.
        for _ in 0..5 {
            drop(rack.service(&slots, &named, high));
        }
        // Now it can be built, and asks for the restart that says so.
        probe.builds_fail_above.store(0, Ordering::SeqCst);
        probe.requests.raise(Requests::RESTART);
        let events = names(&rack.service(&slots, &named, high));
        assert!(events.iter().any(|event| event.starts_with("Install")), "{events:?}");
    }

    /// Records what the session sends, and takes everything.
    #[derive(Default)]
    struct Sink {
        replaced: Vec<(EffectTarget, u8, u64)>,
        installed: Vec<(EffectTarget, u8, EffectKind, Option<u64>)>,
        moved: Vec<(u8, u8)>,
        /// Container rings sent, as `(slot, frames)` (MOO-212).
        spans: Vec<(u8, usize)>,
        branches: Vec<(u8, usize)>,
        /// The nodes sent, kept alive the way the engine would keep them.
        nodes: Vec<Box<dyn AudioNode + Send>>,
        /// Sources installed, as `(channel, what it plays, hosting)` (MOO-84).
        sources: Vec<(u8, GeneratorParams, bool)>,
        /// Processors sent into a source, as `(channel, slot, in or out)`.
        source_swaps: Vec<(u8, PluginSlotId, bool)>,
        /// The sources sent, kept alive with their processors.
        source_nodes: Vec<Box<dyn mooloop_dsp::SourceNode + Send>>,
        rate: u32,
    }

    impl CommandSink for Sink {
        fn send(&mut self, cmd: mooloop_core::EngineCommand) -> bool {
            if let mooloop_core::EngineCommand::MoveEffect { from, to, .. } = cmd {
                self.moved.push((from, to));
            }
            true
        }
        fn send_structural(&mut self, cmd: StructuralCommand) -> bool {
            match cmd {
                StructuralCommand::ReplaceEffect {
                    target,
                    slot,
                    expected_kind,
                    resource_key,
                    node,
                    ..
                } => {
                    assert_eq!(expected_kind, EffectKind::Plugin);
                    self.replaced.push((target, slot, resource_key));
                    self.nodes.push(node);
                }
                StructuralCommand::InstallEffect {
                    target,
                    slot,
                    kind,
                    resource_key,
                    node,
                    ..
                } => {
                    self.installed.push((target, slot, kind, resource_key));
                    self.nodes.push(node);
                }
                StructuralCommand::SetContainerSpan { slot, align, .. } => {
                    self.spans.push((slot, align.map_or(0, |ring| ring.frames())));
                }
                StructuralCommand::SetBranchAlign { slot, align, .. } => {
                    self.branches.push((slot, align.map_or(0, |ring| ring.frames())));
                }
                StructuralCommand::InstallSource { channel, node, .. } => {
                    // A hosted source with a processor in it is never at
                    // rest (the fake processor has not opted in); an empty
                    // one always is.
                    self.sources.push((channel, node.generator_params(), !node.is_at_rest()));
                    self.source_nodes.push(node);
                }
                StructuralCommand::HostSourceProcessor { channel, slot, node } => {
                    self.source_swaps.push((channel, slot, node.is_some()));
                    if let Some(node) = node {
                        self.nodes.push(node);
                    }
                }
                _ => {}
            }
            true
        }
        fn send_deferred(
            &mut self,
            _cmd: mooloop_core::EngineCommand,
            _when: mooloop_core::MusicalEdge,
        ) -> bool {
            true
        }
        fn sample_rate(&self) -> u32 {
            if self.rate == 0 {
                48_000
            } else {
                self.rate
            }
        }
    }

    /// Two channels, a plugin device second on channel 0.
    fn project_with_plugin() -> (mooloop_core::Project, PluginSlotId) {
        use mooloop_core::{ChannelId, Project, ProjectChannel};
        let mut project = Project {
            channels: vec![
                ProjectChannel::sampler(0, 1).with_id(ChannelId(0)),
                ProjectChannel::sampler(1, 1).with_id(ChannelId(1)),
            ],
            next_channel_id: 2,
            ..Project::default()
        };
        let slot = project.add_plugin_slot(PluginSlotState::new(fake_ref()));
        project.channels[0]
            .setup
            .push_effect(EffectSlotState::of_kind(EffectKind::Filter));
        let mut device = EffectSlotState::of_kind(EffectKind::Plugin);
        device.params = EffectParams::Plugin(slot);
        project.channels[0].setup.push_effect(device);
        (project, slot)
    }

    /// [`project_with_plugin`], installed, and hosted by a fake.
    fn session_hosting(
        probe: &Arc<FakeProbe>,
    ) -> (crate::session::Session, PluginSlotId, Box<dyn AudioNode + Send>) {
        let (project, slot) = project_with_plugin();
        let mut session = crate::session::Session::default();
        session.replace_project(&project, &[]);
        let processor = session
            .plugin_rack
            .insert(slot, FakeInstance::new(Arc::clone(probe)))
            .unwrap();
        (session, slot, processor)
    }

    /// Step 04's first test, on the plan side: a plugin that reports 0 and
    /// then 512 frames moves the other channel's compensation with it, with
    /// no install, because the plan is derived from the rack every tick.
    #[test]
    fn compensation_follows_a_plugins_reported_latency() {
        let probe = Arc::new(FakeProbe::default());
        let (mut session, slot, _processor) = session_hosting(&probe);
        assert_eq!(session.latency_plan().channel(1), 0);

        probe.latency.store(512, Ordering::SeqCst);
        probe.requests.raise(Requests::LATENCY_CHANGED);
        session.service_plugins(&mut Sink::default());
        let plan = session.latency_plan();
        assert_eq!(plan.channel(1), 512, "the dry channel waits for the plugin");
        assert_eq!(plan.channel(0), 0);

        // A plugin that is not hosted -- missing, or removed -- counts as
        // the pass-through it plays as.
        session.plugin_rack.remove(slot);
        assert_eq!(session.latency_plan().channel(1), 0);
    }

    /// **A plugin inside a container sizes the container's rings** (MOO-212).
    /// A layer on channel 0 whose first branch is a chain holding the plugin
    /// and whose second is a Filter: when the plugin reports 512 frames, the
    /// session resends the layer's dry ring at 512, the chain's at 512, the
    /// plugin's branch unheld and the Filter's branch held 512 frames. A
    /// plugin in no container resends nothing.
    #[test]
    fn a_plugin_inside_a_container_resizes_its_rings_when_its_latency_arrives() {
        use mooloop_core::{ChannelId, Project, ProjectChannel};
        let probe = Arc::new(FakeProbe::default());
        let mut project = Project {
            channels: vec![ProjectChannel::sampler(0, 1).with_id(ChannelId(0))],
            next_channel_id: 1,
            ..Project::default()
        };
        let slot = project.add_plugin_slot(PluginSlotState::new(fake_ref()));
        // [Layer(3), Chain(1), Plugin, Filter]
        let mut layer = EffectSlotState::of_kind(EffectKind::Layer);
        layer.params.set_container_children(3);
        let mut chain = EffectSlotState::of_kind(EffectKind::Chain);
        chain.params.set_container_children(1);
        let mut device = EffectSlotState::of_kind(EffectKind::Plugin);
        device.params = EffectParams::Plugin(slot);
        for effect in [layer, chain, device, EffectSlotState::of_kind(EffectKind::Filter)] {
            project.channels[0].setup.push_effect(effect);
        }
        let mut session = crate::session::Session::default();
        session.replace_project(&project, &[]);
        let _processor = session
            .plugin_rack
            .insert(slot, FakeInstance::new(Arc::clone(&probe)))
            .unwrap();

        probe.latency.store(512, Ordering::SeqCst);
        probe.requests.raise(Requests::LATENCY_CHANGED);
        let mut sink = Sink::default();
        session.service_plugins(&mut sink);
        assert_eq!(sink.spans, [(0, 512), (1, 512)], "the layer's and the chain's dry rings");
        assert_eq!(sink.branches, [(1, 0), (3, 512)], "the Filter's branch waits for the plugin's");

        // The same plugin in no container: the channel's plan follows it, and
        // no ring is sent.
        let probe = Arc::new(FakeProbe::default());
        let (mut session, _, _processor) = session_hosting(&probe);
        probe.latency.store(512, Ordering::SeqCst);
        probe.requests.raise(Requests::LATENCY_CHANGED);
        let mut sink = Sink::default();
        session.service_plugins(&mut sink);
        assert!(sink.spans.is_empty() && sink.branches.is_empty());
    }

    /// A restart pulls the processor back and swaps the next one in by the
    /// plugin's slot, wherever its device sits, marking nothing dirty; a
    /// rescan that changes the list replaces the song's copy and does.
    #[test]
    fn a_restart_is_swapped_in_by_slot_and_a_rescan_updates_the_song() {
        let probe = Arc::new(FakeProbe::default());
        let (mut session, slot, processor) = session_hosting(&probe);
        session.dirty = false;
        let mut sink = Sink::default();
        probe.requests.raise(Requests::RESTART);
        session.service_plugins(&mut sink);
        let key = u64::from(slot.0);
        assert_eq!(sink.replaced, [(EffectTarget::Channel(0), 1, key)], "the pull-back");
        // The engine hands the old processor back and `poll` drops it.
        drop(processor);
        session.service_plugins(&mut sink);
        assert_eq!(sink.replaced.len(), 2, "the new processor, by the same slot");
        assert_eq!(sink.replaced[1], (EffectTarget::Channel(0), 1, key));
        assert_eq!(probe.builds.load(Ordering::SeqCst), 2);
        assert!(!session.dirty);

        probe.requests.raise(Requests::PARAMS_RESCAN);
        session.service_plugins(&mut sink);
        assert!(!session.dirty, "the fake reports the same (empty) list the song has");
        session.plugins.get_mut(&slot).unwrap().params.push(PluginParamInfo {
            id: 9,
            name: "Gone".to_owned(),
            module: String::new(),
            min: 0.0,
            max: 1.0,
            default: 0.0,
            stepped: None,
            automatable: true,
            modulatable: true,
            hidden: false,
        });
        probe.requests.raise(Requests::PARAMS_RESCAN);
        session.service_plugins(&mut sink);
        assert!(session.plugins[&slot].params.is_empty(), "the rescan replaced the list");
        assert!(session.dirty);
    }

    /// **The zombie.** Every structural edit reinstalls the song from a
    /// snapshot of the session, and the carry plan keeps the plugin's
    /// processor running across it; the instance has to stay live with it,
    /// or nothing can restart it or read its latency again.
    #[test]
    fn a_structural_install_keeps_the_hosted_plugin_live() {
        let probe = Arc::new(FakeProbe::default());
        let (mut session, slot, _processor) = session_hosting(&probe);
        probe.latency.store(64, Ordering::SeqCst);
        let snapshot = session.project_snapshot(120, 0);
        session.replace_project(&snapshot, &[]);
        assert_eq!(session.plugin_rack.dying(), 0, "nothing retired");
        assert_eq!(session.plugin_rack.latency_frames(slot), Some(64));
        assert_eq!(session.latency_plan().channel(1), 64);
        assert_eq!(probe.instances_dropped.load(Ordering::SeqCst), 0);
    }

    /// Installing another song retires every hosted plugin: every instance
    /// goes once its processor has come back.
    #[test]
    fn a_song_install_retires_every_hosted_plugin() {
        let probe = Arc::new(FakeProbe::default());
        let (mut session, _slot, processor) = session_hosting(&probe);
        session.replace_project(&mooloop_core::Project::default(), &[]);
        assert_eq!(session.plugin_rack.dying(), 1);
        drop(processor);
        session.service_plugins(&mut Sink::default());
        assert!(session.plugin_rack.is_empty());
        assert_eq!(probe.instances_dropped.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_slot_already_hosted_is_refused() {
        let probe = Arc::new(FakeProbe::default());
        let mut rack = PluginRack::new();
        let _first = rack.insert(PluginSlotId(0), FakeInstance::new(Arc::clone(&probe))).unwrap();
        assert!(rack.insert(PluginSlotId(0), FakeInstance::new(Arc::clone(&probe))).is_err());
        assert_eq!(rack.len(), 1);
    }

    /// A song that opens with a plugin device gets the plugin: the opener
    /// makes the instance, and the next tick swaps its processor into the
    /// placeholder the install built.
    #[test]
    fn a_song_that_names_a_plugin_is_opened_and_its_processor_swapped_in() {
        let probe = Arc::new(FakeProbe::default());
        let (project, slot) = project_with_plugin();
        let mut session = crate::session::Session::default();
        session.set_plugin_opener(Box::new(FakeOpener(Arc::clone(&probe))));
        session.replace_project(&project, &[]);
        let mut sink = Sink::default();
        session.service_plugins(&mut sink);
        assert_eq!(probe.opens.load(Ordering::SeqCst), 1);
        assert_eq!(sink.replaced, [(EffectTarget::Channel(0), 1, u64::from(slot.0))]);
        assert!(session.plugin_problem(slot).is_none());
        session.service_plugins(&mut sink);
        assert_eq!(sink.replaced.len(), 1, "once, while the processor stays out");
        assert!(!session.dirty, "opening a song is not an edit");
    }

    /// A plugin that is not installed stays the placeholder, says why, and
    /// is asked for again only when the opener's catalogue changes -- a scan
    /// finishing after the song opened, say.
    #[test]
    fn a_missing_plugin_is_tried_again_only_when_the_catalogue_changes() {
        let probe = Arc::new(FakeProbe::default());
        probe.missing.store(true, Ordering::SeqCst);
        let (project, slot) = project_with_plugin();
        let mut session = crate::session::Session::default();
        session.set_plugin_opener(Box::new(FakeOpener(Arc::clone(&probe))));
        session.replace_project(&project, &[]);
        let mut sink = Sink::default();
        for _ in 0..5 {
            session.service_plugins(&mut sink);
        }
        assert_eq!(probe.opens.load(Ordering::SeqCst), 1);
        assert_eq!(session.plugin_problem(slot), Some(HostError::Missing));
        assert!(sink.replaced.is_empty(), "the placeholder stays");

        probe.missing.store(false, Ordering::SeqCst);
        probe.generation.store(1, Ordering::SeqCst);
        session.service_plugins(&mut sink);
        assert_eq!(probe.opens.load(Ordering::SeqCst), 2);
        assert_eq!(sink.replaced.len(), 1, "found, and swapped in");
        assert!(session.plugin_problem(slot).is_none());
    }

    /// A new sample rate pulls every processor back and builds the next one
    /// at the new rate.
    #[test]
    fn a_new_sample_rate_rebuilds_every_processor_at_it() {
        let probe = Arc::new(FakeProbe::default());
        let (project, _slot) = project_with_plugin();
        let mut session = crate::session::Session::default();
        session.set_plugin_opener(Box::new(FakeOpener(Arc::clone(&probe))));
        session.replace_project(&project, &[]);
        let mut sink = Sink::default();
        session.service_plugins(&mut sink);
        assert_eq!(probe.built_rate.load(Ordering::SeqCst), 48_000);

        sink.rate = 96_000;
        session.service_plugins(&mut sink);
        assert_eq!(sink.replaced.len(), 2, "the pull-back");
        // The engine hands back the 48 kHz processor (and the placeholder
        // that replaced it is what the chain holds).
        sink.nodes.clear();
        session.service_plugins(&mut sink);
        assert_eq!(sink.replaced.len(), 3);
        assert_eq!(probe.built_rate.load(Ordering::SeqCst), 96_000);
    }

    /// The session path: a plugin inserted by its reference is installed
    /// at the chain's tail as a real processor and moved into place.
    #[test]
    fn inserting_a_plugin_installs_its_processor_where_it_was_asked() {
        let probe = Arc::new(FakeProbe::default());
        let (project, _) = project_with_plugin();
        let mut session = crate::session::Session::default();
        session.set_plugin_opener(Box::new(FakeOpener(Arc::clone(&probe))));
        session.replace_project(&project, &[]);
        let mut sink = Sink::default();
        let inserted = session.insert_plugin_effect(fake_ref(), 0, &mut sink).expect("inserted");
        let EffectParams::Plugin(slot) = inserted.params else {
            panic!("a plugin device");
        };
        assert_eq!(inserted.slot, 0);
        assert_eq!(
            sink.installed,
            [(EffectTarget::Channel(0), 2, EffectKind::Plugin, Some(u64::from(slot.0)))]
        );
        assert_eq!(sink.moved, [(2, 0)]);
        assert_eq!(probe.builds.load(Ordering::SeqCst), 1);
        assert!(session.plugin_rack.instance(slot).is_some());
        assert_eq!(session.plugins[&slot].plugin, fake_ref());
        assert!(session.dirty);
        assert_eq!(session.channels[0].effects[0].params, EffectParams::Plugin(slot));
    }

    /// **A channel's source can be a plugin** (MOO-84). The session path
    /// makes the channel a plugin channel, keeps the plugin in the song's
    /// table, and installs a hosted source with the processor already in it;
    /// the song says so, and a song opened from that snapshot has its
    /// processor swapped into the silent source the install built -- by
    /// `HostSourceProcessor`, keyed by the slot, not by `ReplaceEffect`.
    #[test]
    fn a_plugin_source_is_installed_saved_and_swapped_in_when_the_song_opens() {
        let probe = Arc::new(FakeProbe::default());
        let (project, effect_slot) = project_with_plugin();
        let mut session = crate::session::Session::default();
        session.set_plugin_opener(Box::new(FakeOpener(Arc::clone(&probe))));
        session.replace_project(&project, &[]);
        let mut sink = Sink::default();
        session.service_plugins(&mut sink);
        // The effect is hosted; the next plugin opened is an instrument.
        probe.instrument.store(true, Ordering::SeqCst);
        let slot = session.set_plugin_source(1, fake_ref(), &mut sink).expect("a channel 1");
        assert_ne!(slot, effect_slot);
        assert_eq!(sink.sources, [(1, GeneratorParams::Plugin(slot), true)]);
        assert_eq!(session.channels[1].kind(), DeviceKind::Plugin);
        assert_eq!(session.channels[1].name, "Plugin 2", "a name nobody typed follows the device");
        assert!(session.dirty);
        assert!(session.named_plugin_slots().contains(&slot));

        let song = session.project_snapshot(120, 50);
        assert_eq!(song.channels[1].setup.source, mooloop_core::ChannelSource::Plugin(slot));
        assert_eq!(song.plugins[&slot].plugin, fake_ref());

        // The same song opened fresh, less the plugin effect so the fake's
        // one probe speaks for the source alone: the install built a silent
        // source, and the rack opens the plugin and swaps it into its place.
        let mut song = song;
        song.channels[0].setup.effects.retain(|effect| effect.params != EffectParams::Plugin(effect_slot));
        song.plugins.remove(&effect_slot);
        let probe = Arc::new(FakeProbe::default());
        probe.instrument.store(true, Ordering::SeqCst);
        let mut reopened = crate::session::Session::default();
        reopened.set_plugin_opener(Box::new(FakeOpener(Arc::clone(&probe))));
        reopened.replace_project(&song, &[]);
        let mut sink = Sink::default();
        reopened.service_plugins(&mut sink);
        assert_eq!(probe.opens.load(Ordering::SeqCst), 1);
        assert_eq!(sink.source_swaps, [(1, slot, true)]);
        assert!(sink.replaced.is_empty(), "a source is not an effect");

        // A restart pulls the source's processor out (silence, not a
        // placeholder) and swaps the next one in once the first is back.
        probe.requests.raise(Requests::RESTART);
        reopened.service_plugins(&mut sink);
        assert_eq!(sink.source_swaps, [(1, slot, true), (1, slot, false)]);
        sink.nodes.clear();
        reopened.service_plugins(&mut sink);
        assert_eq!(sink.source_swaps.last(), Some(&(1, slot, true)));
    }

    /// A plugin source that cannot be opened is still made, silent, with the
    /// reason kept; the channel and the song's slot are the same as if it
    /// had opened.
    #[test]
    fn a_missing_plugin_source_is_installed_silent() {
        let probe = Arc::new(FakeProbe::default());
        probe.missing.store(true, Ordering::SeqCst);
        let mut session = crate::session::Session::default();
        session.set_plugin_opener(Box::new(FakeOpener(Arc::clone(&probe))));
        session.replace_project(&project_with_plugin().0, &[]);
        let mut sink = Sink::default();
        let slot = session.set_plugin_source(0, fake_ref(), &mut sink).expect("a channel 0");
        assert_eq!(sink.sources, [(0, GeneratorParams::Plugin(slot), false)]);
        assert_eq!(session.plugin_problem(slot), Some(HostError::Missing));
        assert_eq!(session.channels[0].kind(), DeviceKind::Plugin);
        assert!(session.set_plugin_source(9, fake_ref(), &mut sink).is_none(), "no channel 9");
    }

    /// Inserting a plugin that is not installed still inserts the device,
    /// as the placeholder, and keeps the reason.
    #[test]
    fn inserting_a_missing_plugin_inserts_its_placeholder() {
        let probe = Arc::new(FakeProbe::default());
        probe.missing.store(true, Ordering::SeqCst);
        let mut session = crate::session::Session::default();
        session.set_plugin_opener(Box::new(FakeOpener(Arc::clone(&probe))));
        session.replace_project(&project_with_plugin().0, &[]);
        let mut sink = Sink::default();
        let inserted = session.insert_plugin_effect(fake_ref(), 9, &mut sink).expect("inserted");
        let EffectParams::Plugin(slot) = inserted.params else {
            panic!("a plugin device");
        };
        assert_eq!(sink.installed.len(), 1);
        assert!(sink.moved.is_empty(), "appended, so nothing to move");
        assert_eq!(session.plugin_problem(slot), Some(HostError::Missing));
        assert_eq!(probe.builds.load(Ordering::SeqCst), 0);
    }

    /// Quit: every processor is pulled back, and the rack empties once the
    /// engine has handed them back.
    #[test]
    fn closing_the_plugins_pulls_every_processor_back() {
        let probe = Arc::new(FakeProbe::default());
        let (mut session, slot, processor) = session_hosting(&probe);
        let mut sink = Sink::default();
        session.close_plugins(&mut sink);
        assert_eq!(sink.replaced, [(EffectTarget::Channel(0), 1, u64::from(slot.0))]);
        assert!(!session.plugins_retired());
        drop(processor);
        session.collect_plugins();
        assert!(session.plugins_retired());
    }

    /// An export gets its own processors, from second instances, which go
    /// once the export has dropped them.
    #[test]
    fn an_export_gets_processors_of_its_own() {
        let probe = Arc::new(FakeProbe::default());
        let (project, slot) = project_with_plugin();
        let mut session = crate::session::Session::default();
        session.set_plugin_opener(Box::new(FakeOpener(Arc::clone(&probe))));
        session.replace_project(&project, &[]);
        let mut sink = Sink::default();
        session.service_plugins(&mut sink);
        let processors = session.export_plugin_processors(44_100);
        assert_eq!(processors.keys().copied().collect::<Vec<_>>(), [slot]);
        assert_eq!(probe.opens.load(Ordering::SeqCst), 2, "a second instance");
        assert_eq!(probe.built_rate.load(Ordering::SeqCst), 44_100);
        assert_eq!(session.plugin_rack.dying(), 1);
        drop(processors);
        session.collect_plugins();
        assert_eq!(session.plugin_rack.dying(), 0);
        assert!(session.plugin_rack.instance(slot).is_some(), "the live one stays");
    }

    // Step 11 (MOO-300): a plugin's GUI, against the rack's fake.

    fn embedded(parent: u64) -> PluginGuiOpen {
        PluginGuiOpen {
            placement: GuiPlacement::Embedded { parent },
            title: "Fake - Channel 1".to_owned(),
            scale: Some(1.0),
        }
    }

    fn with_gui() -> Arc<FakeProbe> {
        let probe = Arc::new(FakeProbe::default());
        probe.has_gui.store(true, Ordering::SeqCst);
        probe
    }

    /// Step 04's order, with the GUI in front: GUI, processor, instance.
    #[test]
    fn removing_a_plugin_destroys_its_gui_then_its_processor_then_its_instance() {
        let probe = with_gui();
        let mut rack = PluginRack::new();
        let slot = PluginSlotId(0);
        let processor = rack.insert(slot, FakeInstance::new(Arc::clone(&probe))).unwrap();
        let opened = rack.open_gui(slot, &embedded(42)).expect("it opens");
        assert_eq!(opened.config, GuiConfig::native_embedded());
        assert_eq!(opened.size, Some(GuiSize { width: 300, height: 200 }));
        assert!(rack.gui_is_open(slot));

        assert!(rack.remove(slot));
        assert_eq!(probe.order(), ["gui destroyed"], "the GUI goes at once");
        assert_eq!(rack.drain_gui_events(), [(slot, PluginGuiEvent::Closed)], "and the window is told");
        assert_eq!(rack.collect(), 0, "the instance waits for its processor");
        drop(processor);
        assert_eq!(rack.collect(), 1);
        assert_eq!(probe.order(), ["gui destroyed", "processor dropped", "instance dropped"]);
    }

    /// Closing a GUI and opening it again never touches the processor.
    #[test]
    fn closing_and_reopening_a_gui_leaves_the_processor_running() {
        let probe = with_gui();
        let mut rack = PluginRack::new();
        let slot = PluginSlotId(2);
        let named = BTreeSet::from([slot]);
        let _processor = rack.insert(slot, FakeInstance::new(Arc::clone(&probe))).unwrap();
        for cycle in 1..=3 {
            rack.open_gui(slot, &embedded(7)).expect("it opens");
            assert_eq!(rack.open_gui(slot, &embedded(7)), Err(GuiError::AlreadyOpen));
            assert!(rack.close_gui(slot));
            assert!(!rack.close_gui(slot), "closed once");
            assert!(
                rack.service(&PluginSlots::new(), &named, config()).is_empty(),
                "cycle {cycle}: no pull-back, no rebuild"
            );
            assert_eq!(probe.gui_creates.load(Ordering::SeqCst), cycle);
            assert_eq!(probe.gui_destroys.load(Ordering::SeqCst), cycle);
        }
        assert_eq!(probe.builds.load(Ordering::SeqCst), 1);
        assert_eq!(probe.processors_dropped.load(Ordering::SeqCst), 0);
        assert!(rack.drain_gui_events().is_empty(), "a close the window asked for is not echoed back");
    }

    #[test]
    fn a_gui_that_fails_to_embed_is_not_left_open() {
        let probe = with_gui();
        probe.refuses_parent.store(true, Ordering::SeqCst);
        let mut rack = PluginRack::new();
        let slot = PluginSlotId(0);
        let _processor = rack.insert(slot, FakeInstance::new(Arc::clone(&probe))).unwrap();
        assert!(matches!(rack.open_gui(slot, &embedded(3)), Err(GuiError::Refused(_))));
        assert!(!rack.gui_is_open(slot));
        assert_eq!(probe.gui_destroys.load(Ordering::SeqCst), 1);
        // Floating needs no parent.
        let floating = PluginGuiOpen {
            placement: GuiPlacement::Floating { transient_for: None },
            ..embedded(0)
        };
        assert!(rack.open_gui(slot, &floating).is_ok());
    }

    #[test]
    fn a_plugin_without_a_gui_or_a_slot_without_a_plugin_says_so() {
        let probe = Arc::new(FakeProbe::default());
        let mut rack = PluginRack::new();
        let _processor = rack.insert(PluginSlotId(0), FakeInstance::new(Arc::clone(&probe))).unwrap();
        assert_eq!(rack.open_gui(PluginSlotId(0), &embedded(1)), Err(GuiError::NoGui));
        assert_eq!(rack.gui_placement_kind(PluginSlotId(0)), Err(GuiError::NoGui));
        assert_eq!(rack.open_gui(PluginSlotId(9), &embedded(1)), Err(GuiError::Missing));
        assert!(!GuiError::NoGui.to_string().is_empty(), "the badge has words to show");
    }

    /// The plugin's own requests reach the window side, and a plugin that
    /// closes its own window has its GUI hidden and destroyed for it.
    #[test]
    fn a_plugins_requests_are_carried_out_and_its_own_close_destroys_the_gui() {
        let probe = with_gui();
        let mut rack = PluginRack::new();
        let slot = PluginSlotId(4);
        let _processor = rack.insert(slot, FakeInstance::new(Arc::clone(&probe))).unwrap();
        rack.open_gui(slot, &embedded(8)).expect("it opens");
        let size = GuiSize { width: 500, height: 400 };
        probe.gui_requests.lock().unwrap().extend([
            GuiRequest::Resize(size),
            GuiRequest::Hide,
            GuiRequest::Closed { destroyed: false },
        ]);
        assert_eq!(
            rack.drain_gui_events(),
            [
                (slot, PluginGuiEvent::Resize(size)),
                (slot, PluginGuiEvent::Hide),
                (slot, PluginGuiEvent::Closed),
            ]
        );
        assert!(!rack.gui_is_open(slot));
        assert_eq!(probe.gui_destroys.load(Ordering::SeqCst), 1);
        assert_eq!(probe.processors_dropped.load(Ordering::SeqCst), 0, "the processor runs on");
    }

    /// A device removed from its chain keeps its instance for an undo, but
    /// its window closes on the next tick.
    #[test]
    fn a_device_nobody_names_loses_its_gui_on_the_next_tick() {
        let probe = with_gui();
        let mut rack = PluginRack::new();
        let slot = PluginSlotId(1);
        let _processor = rack.insert(slot, FakeInstance::new(Arc::clone(&probe))).unwrap();
        rack.open_gui(slot, &embedded(2)).expect("it opens");
        rack.service(&PluginSlots::new(), &BTreeSet::from([slot]), config());
        assert!(rack.gui_is_open(slot), "named, it stays");
        rack.service(&PluginSlots::new(), &BTreeSet::new(), config());
        assert!(!rack.gui_is_open(slot));
        assert_eq!(rack.drain_gui_events(), [(slot, PluginGuiEvent::Closed)]);
        assert!(rack.instance(slot).is_some(), "the instance is kept for an undo");
        assert_eq!(probe.processors_dropped.load(Ordering::SeqCst), 0);
    }

    /// Closing the song, or quitting, destroys every GUI before any
    /// processor comes back or any instance goes.
    #[test]
    fn closing_the_plugins_destroys_every_gui_first() {
        let probe = with_gui();
        let (mut session, slot, processor) = session_hosting(&probe);
        let title = session.plugin_gui_title(slot).expect("a title");
        assert!(title.starts_with("Fake"), "{title}");
        session.open_plugin_gui(slot, &embedded(5)).expect("it opens");
        assert!(session.plugin_gui_is_open(slot));
        let mut sink = Sink::default();
        session.close_plugins(&mut sink);
        assert_eq!(probe.order(), ["gui destroyed"]);
        assert_eq!(session.drain_plugin_gui_events(), [(slot, PluginGuiEvent::Closed)]);
        assert_eq!(sink.replaced.len(), 1, "then the processor is pulled back");
        drop(processor);
        session.collect_plugins();
        assert!(session.plugins_retired());
        assert_eq!(probe.order(), ["gui destroyed", "processor dropped", "instance dropped"]);
    }

    /// A quit whose wait ran out still destroys the GUI of an instance it
    /// has to leak.
    #[test]
    fn a_leaked_instance_still_has_its_gui_destroyed() {
        let probe = with_gui();
        let mut rack = PluginRack::new();
        let slot = PluginSlotId(0);
        let processor = rack.insert(slot, FakeInstance::new(Arc::clone(&probe))).unwrap();
        rack.open_gui(slot, &embedded(6)).expect("it opens");
        assert_eq!(rack.leak_remaining(), 1);
        assert_eq!(probe.gui_destroys.load(Ordering::SeqCst), 1);
        std::mem::forget(processor);
    }
}
