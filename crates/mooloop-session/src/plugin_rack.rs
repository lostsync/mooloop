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

use std::collections::{BTreeMap, BTreeSet};

use mooloop_core::{
    insert_effect, log_warn, mint_plugin_slot, EffectKind, EffectParams, EffectSlotState,
    EffectTarget, EngineCommand, PluginParamInfo, PluginRef, PluginSlotId, PluginSlotState,
    PluginSlots, PluginState, PluginStateText,
};
use mooloop_dsp::effects::PluginPlaceholder;
use mooloop_dsp::{AudioNode, IntegerDelay, SpectrumAnalyzer};
use mooloop_engine::{CommandSink, EffectSlot, StructuralCommand};
use mooloop_plugin_host::{
    AudioConfig, HostError, HostedInstance, Lifeline, PluginOpener, PluginParamEvent, Requests,
};

use crate::effects::EffectInserted;

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
    pub fn remove(&mut self, slot: PluginSlotId) -> bool {
        self.problems.remove(&slot);
        self.edited.remove(&slot);
        self.refused_state.remove(&slot);
        match self.entries.remove(&slot) {
            Some(entry) => {
                self.graveyard.push((entry.instance, entry.lifeline));
                true
            }
            None => false,
        }
    }

    /// Retire every entry: the song is closing. Returns the slots whose
    /// processors are still out, for the caller to pull back.
    pub fn close(&mut self) -> Vec<PluginSlotId> {
        self.problems.clear();
        self.edited.clear();
        self.refused_state.clear();
        let mut out = Vec::new();
        for (slot, entry) in std::mem::take(&mut self.entries) {
            if !entry.lifeline.is_alone() {
                out.push(slot);
            }
            self.graveyard.push((entry.instance, entry.lifeline));
        }
        out
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
        for (_, entry) in std::mem::take(&mut self.entries) {
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
                }
            }
            self.config = Some(config);
        }

        // Open what the song names and nothing hosts.
        if let Some(opener) = self.opener.as_mut() {
            let generation = opener.refresh();
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
            let requests: Requests = entry.instance.take_requests();
            if requests.has(Requests::CALLBACK) {
                entry.instance.on_main_thread();
            }
            if requests.has(Requests::RESTART) {
                entry.rebuild = true;
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

    /// Every plugin slot a device on some chain names.
    pub fn named_plugin_slots(&self) -> BTreeSet<PluginSlotId> {
        let channels = self.channels.iter().map(|channel| &channel.effects);
        let buses = self.buses.iter().map(|bus| &bus.effects);
        channels
            .chain(buses)
            .flatten()
            .filter_map(|effect| match effect.params {
                EffectParams::Plugin(slot) => Some(slot),
                _ => None,
            })
            .collect()
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
                RackEvent::PullBack { slot } => {
                    let (placeholder, align) = self.pull_back_placeholder(slot);
                    let _ = self.replace_plugin_processor(slot, placeholder, align, handle);
                }
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
    /// the chain the rack is pointed at, open it, and install its processor
    /// -- or the placeholder, when it cannot be opened, with the reason kept
    /// for [`Self::plugin_problem`].
    ///
    /// The session path of step 06: there is no menu row for it until step
    /// 08. Mirrors the native insert (`Session::insert_effect_at` and the
    /// interface's `install_added_effect`): installed at the chain's tail
    /// and moved into place. It does not publish container spans, so it is
    /// for a position outside any container.
    pub fn insert_plugin_effect(
        &mut self,
        plugin: PluginRef,
        insert_before: usize,
        handle: &mut impl CommandSink,
    ) -> Option<EffectInserted> {
        let target = self.effect_target;
        let slot = mint_plugin_slot(
            &mut self.plugins,
            &mut self.next_plugin_slot,
            PluginSlotState::new(plugin.clone()),
        );
        let mut effect = EffectSlotState::of_kind(EffectKind::Plugin);
        effect.params = EffectParams::Plugin(slot);
        let inserted = self.effect_chain_parts_mut().and_then(|(effects, next_id)| {
            let tail = effects.len();
            let row = insert_effect(effects, next_id, insert_before, effect)?;
            Some((tail, row, effects[row].id))
        });
        let Some((tail, row, device)) = inserted else {
            self.plugins.remove(&slot);
            return None;
        };
        let config = Self::plugin_config(handle);
        let opened = self
            .plugin_rack
            .open(&plugin, &PluginState::default(), config)
            .and_then(|instance| {
                let params = instance.params().to_vec();
                let node = self.plugin_rack.insert(slot, instance)?;
                Ok((node, params))
            });
        let node: Box<dyn AudioNode + Send> = match opened {
            Ok((node, params)) => {
                if let Some(state) = self.plugins.get_mut(&slot) {
                    state.params = params;
                }
                node
            }
            Err(error) => {
                log_warn!("plugin", "{}: {error}", plugin.name);
                self.plugin_rack.record_problem(slot, error);
                Box::new(PluginPlaceholder::new(slot))
            }
        };
        let align = IntegerDelay::new(node.dry_path_latency_frames()).map(Box::new);
        let _ = handle.send_structural(StructuralCommand::InstallEffect {
            target,
            slot: tail as u8,
            kind: EffectKind::Plugin,
            resource_key: Some(u64::from(slot.0)),
            node,
            align,
            analyzer: Box::new(SpectrumAnalyzer::new()),
            state: Box::new(EffectSlot::for_device(device)),
        });
        if row != tail {
            let _ = handle.send(EngineCommand::MoveEffect {
                target,
                from: tail as u8,
                to: row as u8,
            });
        }
        self.dirty = true;
        Some(EffectInserted {
            target,
            slot: row,
            tail,
            device,
            kind: EffectKind::Plugin,
            params: EffectParams::Plugin(slot),
        })
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

    /// Begin retiring every hosted plugin, for a quit: every processor out
    /// is pulled back by swapping the placeholder in. The caller then polls
    /// the engine and calls [`Self::collect_plugins`] until
    /// [`Self::plugins_retired`], or its bounded wait runs out.
    pub fn close_plugins(&mut self, handle: &mut impl CommandSink) {
        for slot in self.plugin_rack.close() {
            let (placeholder, align) = self.pull_back_placeholder(slot);
            let _ = self.replace_plugin_processor(slot, placeholder, align, handle);
        }
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
    }

    /// The rack's test double: a plugin with no library behind it.
    pub(crate) struct FakeInstance {
        plugin: PluginRef,
        params: Vec<PluginParamInfo>,
        config: AudioConfig,
        probe: Arc<FakeProbe>,
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
            })
        }
    }

    impl Drop for FakeInstance {
        fn drop(&mut self) {
            self.probe.instances_dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct FakeProcessor {
        _lifeline: Lifeline,
        probe: Arc<FakeProbe>,
    }

    impl Drop for FakeProcessor {
        fn drop(&mut self) {
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
        fn set_audio_config(&mut self, config: AudioConfig) {
            self.config = config;
        }
        fn build_processor(
            &mut self,
            lifeline: Lifeline,
        ) -> Result<Box<dyn AudioNode + Send>, HostError> {
            self.probe.builds.fetch_add(1, Ordering::SeqCst);
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
}
