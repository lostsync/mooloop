//! The control thread's hosted plugins (`docs/plans/plugin-hosting/04-the-plugin-rack.md`).
//!
//! Once a processor is installed, nothing on the control thread can reach
//! it: the node belongs to the audio thread. The rack is what the control
//! thread keeps instead -- each plugin's [`HostedInstance`], by the song's
//! [`PluginSlotId`] -- so that a plugin's requests have somewhere to land,
//! its latency has somewhere to be read from, and its instance is dropped
//! only after its processor is (blocker 5).
//!
//! **Teardown order is the whole point.** Removing a plugin marks its entry
//! *dying*; the entry is dropped by [`PluginRack::collect`] only once every
//! processor tied to its [`Lifeline`] has been dropped. Processors are only
//! ever dropped on the control thread -- `EngineHandle::poll` drains the
//! reclaim ring, and a retired project goes the same way -- so the instance
//! never outlives-by-accident or dies-before its processor, and neither is
//! ever freed in the callback.

use std::collections::BTreeMap;

use mooloop_core::{
    log_warn, EffectKind, EffectParams, EffectSlotState, EffectTarget, PluginParamInfo,
    PluginSlotId,
};
use mooloop_dsp::AudioNode;
use mooloop_engine::{CommandSink, StructuralCommand};
use mooloop_plugin_host::{HostError, HostedInstance, Lifeline, Requests};

struct RackEntry {
    instance: Box<dyn HostedInstance>,
    lifeline: Lifeline,
    /// Removed from the song and waiting for its processors to come back.
    /// A dying entry's requests are drained and ignored: a restart that
    /// arrives after its slot was removed must not reinstall anything.
    dying: bool,
}

/// What the rack's owner has to act on after [`PluginRack::service`].
pub enum RackEvent {
    /// The plugin restarted and built a new processor, to be swapped into
    /// its device's slot (`StructuralCommand::ReplaceEffect`).
    Restarted {
        slot: PluginSlotId,
        node: Box<dyn AudioNode + Send>,
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
    /// The plugin changed its own state: the song is modified.
    StateDirty { slot: PluginSlotId },
    /// A request the plugin could not carry out.
    Failed { slot: PluginSlotId, error: HostError },
}

impl std::fmt::Debug for RackEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Restarted { slot, .. } => write!(f, "Restarted({})", slot.0),
            Self::LatencyChanged { slot } => write!(f, "LatencyChanged({})", slot.0),
            Self::ParamsRescanned { slot, params } => {
                write!(f, "ParamsRescanned({}, {} params)", slot.0, params.len())
            }
            Self::StateDirty { slot } => write!(f, "StateDirty({})", slot.0),
            Self::Failed { slot, error } => write!(f, "Failed({}, {error})", slot.0),
        }
    }
}

/// Every hosted plugin the control thread holds, by song slot.
#[derive(Default)]
pub struct PluginRack {
    entries: BTreeMap<PluginSlotId, RackEntry>,
}

impl std::fmt::Debug for PluginRack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(self.entries.iter().map(|(slot, entry)| (slot.0, entry.dying)))
            .finish()
    }
}

impl PluginRack {
    pub fn new() -> Self {
        Self::default()
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
        self.entries.insert(
            slot,
            RackEntry {
                instance,
                lifeline,
                dying: false,
            },
        );
        Ok(node)
    }

    /// The live instance in `slot`, if there is one.
    pub fn instance(&self, slot: PluginSlotId) -> Option<&dyn HostedInstance> {
        self.entries
            .get(&slot)
            .filter(|entry| !entry.dying)
            .map(|entry| entry.instance.as_ref())
    }

    /// The latency the plugin in `slot` reports now, or `None` when no live
    /// instance is hosted there -- a missing plugin plays as a pass-through,
    /// which adds nothing.
    pub fn latency_frames(&self, slot: PluginSlotId) -> Option<u32> {
        self.instance(slot).map(|instance| instance.latency_frames())
    }

    /// Mark `slot` as removed. The caller sends the device's removal; the
    /// instance stays until [`Self::collect`] sees its processors gone.
    /// Returns whether a live entry was there.
    pub fn remove(&mut self, slot: PluginSlotId) -> bool {
        match self.entries.get_mut(&slot) {
            Some(entry) if !entry.dying => {
                entry.dying = true;
                true
            }
            _ => false,
        }
    }

    /// Mark every entry as removed: the song is closing or being replaced.
    pub fn close(&mut self) {
        for entry in self.entries.values_mut() {
            entry.dying = true;
        }
    }

    /// Drop every dying entry whose processors have all been dropped, and
    /// return how many were. Run after `EngineHandle::poll` has drained the
    /// reclaim ring, which is where a processor is dropped.
    pub fn collect(&mut self) -> usize {
        let before = self.entries.len();
        self.entries
            .retain(|_, entry| !(entry.dying && entry.lifeline.is_alone()));
        before - self.entries.len()
    }

    /// Entries still held, dying ones included.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entries removed from the song whose processors have not come back
    /// yet. A song close waits on this reaching zero, with a bounded timeout.
    pub fn dying(&self) -> usize {
        self.entries.values().filter(|entry| entry.dying).count()
    }

    /// Drain every plugin's requests and carry out the ones that are the
    /// rack's to carry out, once a pump tick. The rest come back as events
    /// for the owner. A dying entry's requests are drained and ignored.
    pub fn service(&mut self) -> Vec<RackEvent> {
        let mut events = Vec::new();
        for (&slot, entry) in &mut self.entries {
            let requests: Requests = entry.instance.take_requests();
            if requests.is_empty() || entry.dying {
                continue;
            }
            if requests.has(Requests::CALLBACK) {
                entry.instance.on_main_thread();
            }
            if requests.has(Requests::RESTART) {
                match entry.instance.restart(entry.lifeline.tie()) {
                    Ok(node) => events.push(RackEvent::Restarted { slot, node }),
                    Err(error) => events.push(RackEvent::Failed { slot, error }),
                }
            }
            // A restart can change the latency whether or not the plugin
            // said so separately, so both ask for the plan to be derived
            // again.
            if requests.has(Requests::LATENCY_CHANGED) || requests.has(Requests::RESTART) {
                events.push(RackEvent::LatencyChanged { slot });
            }
            if requests.has(Requests::PARAMS_RESCAN) {
                events.push(RackEvent::ParamsRescanned {
                    slot,
                    params: entry.instance.params().to_vec(),
                });
            }
            if requests.has(Requests::STATE_DIRTY) {
                events.push(RackEvent::StateDirty { slot });
            }
        }
        events
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

    /// The pump's once-a-tick plugin upkeep: drop instances whose processors
    /// have come back, then act on what the plugins asked for.
    ///
    /// Run after `EngineHandle::poll`, which is where a reclaimed processor
    /// is dropped. A latency change needs nothing here:
    /// [`Self::sync_compensation`] derives the plan from
    /// [`Self::device_latency`] on the same tick and sends what moved.
    pub fn service_plugins(&mut self, handle: &mut impl CommandSink) {
        // Every 8 ms tick, so a song with no plugin in it pays one branch:
        // no walk, no allocation, no lock.
        if self.plugin_rack.is_empty() {
            return;
        }
        self.plugin_rack.collect();
        for event in self.plugin_rack.service() {
            match event {
                RackEvent::Restarted { slot, node } => {
                    if !self.replace_plugin_processor(slot, node, handle) {
                        log_warn!("plugin", "slot {}: the restarted plugin could not be swapped in", slot.0);
                    }
                }
                RackEvent::LatencyChanged { .. } => {}
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
                RackEvent::StateDirty { .. } => self.dirty = true,
                RackEvent::Failed { slot, error } => {
                    log_warn!("plugin", "slot {}: {error}", slot.0);
                }
            }
        }
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
            align: None,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
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
        pub restarts: AtomicUsize,
        pub instances_dropped: AtomicUsize,
        pub processors_dropped: AtomicUsize,
    }

    /// The rack's test double: a plugin with no library behind it.
    pub(crate) struct FakeInstance {
        plugin: PluginRef,
        params: Vec<PluginParamInfo>,
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
        fn value_text(&self, _id: u32, _value: f64) -> Option<String> {
            None
        }
        fn take_requests(&self) -> Requests {
            self.probe.requests.take()
        }
        fn on_main_thread(&mut self) {}
        fn build_processor(
            &mut self,
            lifeline: Lifeline,
        ) -> Result<Box<dyn AudioNode + Send>, HostError> {
            Ok(Box::new(FakeProcessor {
                _lifeline: lifeline,
                probe: Arc::clone(&self.probe),
            }))
        }
        fn restart(&mut self, lifeline: Lifeline) -> Result<Box<dyn AudioNode + Send>, HostError> {
            self.probe.restarts.fetch_add(1, Ordering::SeqCst);
            self.build_processor(lifeline)
        }
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

    /// A song closing marks everything dying, and nothing is dropped until
    /// the processors are.
    #[test]
    fn closing_waits_for_every_processor() {
        let probe = Arc::new(FakeProbe::default());
        let mut rack = PluginRack::new();
        let first = rack.insert(PluginSlotId(0), FakeInstance::new(Arc::clone(&probe))).unwrap();
        let second = rack.insert(PluginSlotId(1), FakeInstance::new(Arc::clone(&probe))).unwrap();
        rack.close();
        assert_eq!(rack.dying(), 2);
        drop(first);
        assert_eq!(rack.collect(), 1);
        drop(second);
        assert_eq!(rack.collect(), 1);
        assert_eq!(probe.instances_dropped.load(Ordering::SeqCst), 2);
    }

    /// Step 04's third test: a restart the plugin asked for after its slot
    /// was removed builds nothing and reinstalls nothing.
    #[test]
    fn a_restart_requested_after_removal_does_nothing() {
        let probe = Arc::new(FakeProbe::default());
        let mut rack = PluginRack::new();
        let slot = PluginSlotId(3);
        let _processor = rack.insert(slot, FakeInstance::new(Arc::clone(&probe))).unwrap();
        rack.remove(slot);
        probe.requests.raise(Requests::RESTART | Requests::LATENCY_CHANGED);
        assert!(rack.service().is_empty());
        assert_eq!(probe.restarts.load(Ordering::SeqCst), 0);
        assert!(probe.requests.take().is_empty(), "the request was drained, not left to fire later");
    }

    #[test]
    fn requests_become_events_and_a_restart_builds_a_tied_processor() {
        let probe = Arc::new(FakeProbe::default());
        let mut rack = PluginRack::new();
        let slot = PluginSlotId(1);
        let first = rack.insert(slot, FakeInstance::new(Arc::clone(&probe))).unwrap();
        assert_eq!(rack.latency_frames(slot), Some(0));

        probe.latency.store(512, Ordering::SeqCst);
        probe.requests.raise(Requests::RESTART | Requests::PARAMS_RESCAN | Requests::STATE_DIRTY);
        let mut events = rack.service();
        let names: Vec<String> = events.iter().map(|event| format!("{event:?}")).collect();
        assert_eq!(
            names,
            ["Restarted(1)", "LatencyChanged(1)", "ParamsRescanned(1, 0 params)", "StateDirty(1)"]
        );
        assert_eq!(rack.latency_frames(slot), Some(512));

        // The restarted processor is tied too: removing the slot waits for
        // both the old and the new one to be dropped.
        let RackEvent::Restarted { node, .. } = events.remove(0) else {
            panic!("the first event is the restart");
        };
        rack.remove(slot);
        drop(first);
        assert_eq!(rack.collect(), 0, "the restarted processor is still out");
        drop(node);
        assert_eq!(rack.collect(), 1);
    }

    /// Records what the session sends, and takes everything.
    #[derive(Default)]
    struct Sink {
        replaced: Vec<(EffectTarget, u8, u64)>,
    }

    impl CommandSink for Sink {
        fn send(&mut self, _cmd: mooloop_core::EngineCommand) -> bool {
            true
        }
        fn send_structural(&mut self, cmd: StructuralCommand) -> bool {
            if let StructuralCommand::ReplaceEffect {
                target,
                slot,
                expected_kind,
                resource_key,
                ..
            } = cmd
            {
                assert_eq!(expected_kind, EffectKind::Plugin);
                self.replaced.push((target, slot, resource_key));
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
            48_000
        }
    }

    /// Two channels, a plugin device second on channel 0, hosted by a fake.
    fn session_hosting(probe: &Arc<FakeProbe>) -> (crate::session::Session, PluginSlotId, Box<dyn AudioNode + Send>) {
        use mooloop_core::{ChannelId, EffectSlotState, PluginSlotState, Project, ProjectChannel};
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

    /// A restart swaps the new processor in by the plugin's slot, wherever
    /// its device sits, and marks nothing dirty; a rescan that changes the
    /// list replaces the song's copy and does.
    #[test]
    fn a_restart_is_swapped_in_by_slot_and_a_rescan_updates_the_song() {
        let probe = Arc::new(FakeProbe::default());
        let (mut session, slot, _processor) = session_hosting(&probe);
        session.dirty = false;
        let mut sink = Sink::default();
        probe.requests.raise(Requests::RESTART);
        session.service_plugins(&mut sink);
        assert_eq!(sink.replaced, [(EffectTarget::Channel(0), 1, u64::from(slot.0))]);
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

    /// Installing a song closes the rack: every instance is dying and goes
    /// once its processor has come back.
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
}
