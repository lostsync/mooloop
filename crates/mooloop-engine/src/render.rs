//! JACK-independent render state shared by realtime playback and file export.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwapOption;
use mooloop_core::{
    compile_bus_graph, AutomationLane, ChannelSource, CompiledBusGraph, DeviceKind,
    DrumSynthParams, EffectTarget, EngineCommand, GeneratorParams, ModDestinationDescriptor,
    ModRack, MonoSynthParams, MlM1Params, ParamAddr, ParamOwner, PolySynthParams, Project,
    SamplerParams,
    DEFAULT_STEPS, MAX_SAMPLER_VOICES, MASTER_BUS, MAX_BUSES, MAX_CHANNELS, MAX_EFFECTS_PER_CHANNEL, MAX_LINEAR_GAIN,
    MAX_MODULATORS_PER_CHANNEL, STRIP_DESCRIPTORS, STRIP_PARAM_VOLUME,
};
#[cfg(test)]
use mooloop_dsp::build_effect;
use mooloop_dsp::{
    balance_gains, buffer_allocation_key, build_effect_at_tempo, pan_gains, AudioNode, DrumSynth,
    DryAlign, Event, EventList, ModulatorRack, MonoSynth, MlM1, NoteGateEvents, PolySynth,
    ProcessContext, SampleData, Sampler, SpectrumAnalyzer, StereoBus, StretchPool, TimedEvent,
    CONTROL_RATE_FRAMES, MAX_BLOCK_SIZE,
};

use crate::meters::{BusMeters, DeviceMeters, DeviceTelemetry, ModulatorMeters, PlayheadMeters};
use crate::sequencer::Sequencer;
use crate::transport::Transport;
use crate::{PreviewCommand, StructuralCommand, StructuralReclaim};

/// A displaced effect-slot occupant: the node plus the dry-align delay the
/// container allocated alongside it. Both halves are heap objects built on
/// the non-realtime side, so they must also be dropped there — the realtime
/// thread never frees a `Box` itself.
pub(crate) struct ReclaimedEffect {
    pub node: Option<Box<dyn AudioNode + Send>>,
    pub align: Option<Box<DryAlign>>,
    pub analyzer: Option<Box<SpectrumAnalyzer>>,
    /// The slot's own state, which is a box like the rest and so must leave
    /// the realtime thread the same way rather than being dropped on it.
    pub state: Option<Box<EffectSlot>>,
    /// Channel storage the graph turned out not to need, handed back for the
    /// same reason as the rest: the audio thread never drops a box.
    pub channel: Option<Box<ChannelStorage>>,
}

impl ReclaimedEffect {
    fn is_empty(&self) -> bool {
        self.node.is_none()
            && self.align.is_none()
            && self.analyzer.is_none()
            && self.state.is_none()
            && self.channel.is_none()
    }
}

/// Occupants displaced from effect slots, handed back so the non-realtime
/// side can drop them.
type Reclaim = Vec<ReclaimedEffect>;

/// A compact, preallocated per-effect mailbox. Knob traffic is coalesced by
/// parameter ID; retaining every intermediate mouse position is neither
/// audible nor necessary, while allocating a full `EventList` for every one
/// of 256 possible slots would make empty chains prohibitively expensive.
const MAX_PENDING_EFFECT_PARAMS: usize = 8;

/// `MAX_BLOCK_SIZE` is the executor's explicit block-size boundary. Capturing
/// one value for each rack slot at every control-rate boundary lets every
/// effect in a channel read the exact same LFO timeline without allocating or
/// advancing the source more than once.
const MAX_CONTROL_TICKS_PER_BLOCK: usize = MAX_BLOCK_SIZE / CONTROL_RATE_FRAMES;
type ControlOutputs = [[f32; MAX_MODULATORS_PER_CHANNEL]; MAX_CONTROL_TICKS_PER_BLOCK];

fn default_generator_params(kind: DeviceKind) -> GeneratorParams {
    match kind {
        DeviceKind::Sampler => GeneratorParams::Sampler(SamplerParams::default()),
        DeviceKind::MonoSynth => GeneratorParams::MonoSynth(MonoSynthParams::default()),
        DeviceKind::PolySynth => GeneratorParams::PolySynth(PolySynthParams::default()),
        DeviceKind::MlM1 => GeneratorParams::MlM1(MlM1Params::default()),
        DeviceKind::DrumSynth => GeneratorParams::DrumSynth,
    }
}

/// Control-side identity of an effect's asynchronously prepared resource, for
/// the kinds that have one. Reverb was the original case and no longer is:
/// the FDN hall reallocates nothing, so its parameters travel as ordinary
/// `SetEffectParam` commands. The mechanism stays for the retained-audio
/// buffer, whose ring size genuinely cannot change on the audio thread.
fn effect_resource_key(params: mooloop_core::EffectParams) -> Option<u64> {
    params.buffer().copied().map(buffer_allocation_key)
}

#[derive(Clone, Copy)]
struct PendingEffectParams {
    events: [Option<TimedEvent>; MAX_PENDING_EFFECT_PARAMS],
}

/// The already-ticked control signal for one channel's current block. It is
/// deliberately a read-only view: only `RenderState` advances modulators, so
/// graph order cannot accidentally change their phase.
struct ModulationBlock<'a> {
    rack: &'a ModRack,
    outputs: &'a ControlOutputs,
    ticks: usize,
}

/// The clip automation covering this block. Unlike modulation this is not
/// pre-ticked: a lane is a sorted breakpoint list, so resolving it per control
/// tick is a binary search rather than state that must advance exactly once.
struct AutomationBlock<'a> {
    sequencer: &'a Sequencer,
    /// Transport position at frame 0, in song ticks.
    start_tick: f64,
    ticks_per_sample: f64,
    ticks: usize,
}

/// One destination's lane, already resolved to the pattern driving it.
struct AutomationCurve<'a> {
    lane: &'a AutomationLane,
    /// Pattern-local tick at frame 0.
    start_tick: f64,
    length_ticks: u32,
}

impl<'a> AutomationBlock<'a> {
    fn curve_for(&self, destination: ParamAddr) -> Option<AutomationCurve<'a>> {
        let (lane, start_tick, length_ticks) = self
            .sequencer
            .automation_lane_at(destination, self.start_tick)?;
        Some(AutomationCurve {
            lane,
            start_tick,
            length_ticks,
        })
    }

    /// Normalized value at control tick `tick`. The pattern wraps underneath a
    /// block that straddles the loop point, which is why the position is
    /// recomputed per tick instead of advanced.
    fn value_at(&self, curve: &AutomationCurve<'_>, tick: usize) -> Option<f32> {
        let elapsed = (tick * CONTROL_RATE_FRAMES) as f64 * self.ticks_per_sample;
        let position = curve.start_tick + elapsed;
        let wrapped = if curve.length_ticks == 0 {
            position
        } else {
            position.rem_euclid(curve.length_ticks as f64)
        };
        curve.lane.value_at(wrapped)
    }
}

impl PendingEffectParams {
    const fn empty() -> Self {
        Self {
            events: [None; MAX_PENDING_EFFECT_PARAMS],
        }
    }

    fn clear(&mut self) {
        self.events.fill(None);
    }

    fn queue(&mut self, event: TimedEvent) {
        let Event::ParamValue { id, .. } = event.event else {
            if let Some(empty) = self.events.iter_mut().find(|entry| entry.is_none()) {
                *empty = Some(event);
            }
            return;
        };
        if let Some(existing) = self.events.iter_mut().find(|existing| {
            matches!(existing, Some(TimedEvent { event: Event::ParamValue { id: existing_id, .. }, .. }) if *existing_id == id)
        }) {
            *existing = Some(event);
            return;
        }
        if let Some(empty) = self.events.iter_mut().find(|entry| entry.is_none()) {
            *empty = Some(event);
        } else {
            // The command queue is already bounded. Under pathological
            // automation traffic, keep the newest value rather than retaining
            // a stale one indefinitely.
            self.events[0] = Some(event);
        }
    }

    fn copy_to(&self, destination: &mut EventList) {
        for event in self.events.iter().flatten() {
            let _ = destination.push_ordered(*event);
        }
    }
}

/// A fixed-size chain of optional effect nodes plus the per-slot machinery
/// that feeds them. Channels and mixer buses both own one, which is the whole
/// reason effect commands address an `EffectTarget` rather than a channel.
/// Everything a populated effect slot carries besides its node, its dry-path
/// aligner, and its analyzer — all of which are already boxed.
///
/// Grouped and boxed so an addressable-but-empty slot costs a pointer rather
/// than its full state. A chain addresses `MAX_EFFECTS_PER_CHANNEL` slots
/// because that is the width of the index, and a project populates a handful;
/// holding a 320-byte event queue and a 140-byte parameter set for each of
/// the 256 was 140 KiB per chain, and a chain lives on every one of 256
/// channels (`docs/plans/modulator-capacity/`).
///
/// Allocated on the control thread and installed, like the node beside it.
pub struct EffectSlot {
    /// The slot's persisted device identity, tracked independently of the
    /// trait object so prepared resource replacements can refuse stale work.
    kind: Option<mooloop_core::EffectKind>,
    /// The authoritative knob value. Nodes retain only the resolved value they
    /// were last sent; keeping the base here is what lets a knob move
    /// underneath an active modulator without fighting it.
    base_params: Option<mooloop_core::EffectParams>,
    /// Control-side identity of an asynchronously prepared device resource.
    resource_key: Option<u64>,
    /// Parameter events queued between blocks by `EngineCommand::SetEffectParam`
    /// and consumed by the next block.
    events: PendingEffectParams,
    /// Host controls. These belong to the slot rather than to the device in
    /// it, so replacing an effect keeps the wet/dry and trims dialled there.
    bypassed: bool,
    wet_dry: f32,
    input_trim: f32,
    output_trim: f32,
}

impl EffectSlot {
    /// A fresh slot, allocated on the control thread to be installed.
    pub fn new() -> Self {
        Self {
            kind: None,
            base_params: None,
            resource_key: None,
            events: PendingEffectParams::empty(),
            bypassed: false,
            wet_dry: 1.0,
            input_trim: 1.0,
            output_trim: 1.0,
        }
    }
}

impl Default for EffectSlot {
    fn default() -> Self {
        Self::new()
    }
}

struct EffectChain {
    /// Processed in order after whatever produced the audio. Slots are `None`
    /// until a node is installed structurally.
    nodes: [Option<Box<dyn AudioNode + Send>>; MAX_EFFECTS_PER_CHANNEL],
    /// One past the highest occupied node slot. Keeps the realtime pass
    /// proportional to the populated chain instead of its addressable size.
    bound: usize,
    /// Per-slot host and control state, present only where a slot is (or was)
    /// populated. See [`EffectSlot`] for why this is one boxed struct rather
    /// than a parallel array per field.
    slots: [Option<Box<EffectSlot>>; MAX_EFFECTS_PER_CHANNEL],
    /// Reused while each sequential slot processes. See
    /// `PendingEffectParams` for why this is not stored per slot.
    event_scratch: EventList,
    /// Per-slot dry-path delay matching the installed node's reported
    /// latency, so the wet/dry blend never mixes time-misaligned signals.
    /// Allocated off the realtime thread, next to the node it belongs to.
    dry_align: [Option<Box<DryAlign>>; MAX_EFFECTS_PER_CHANNEL],
    /// Input analyzers follow effect slots during reorders. They are generic
    /// host instrumentation, not EQ-specific DSP state. The boxes are built
    /// with nodes so empty addressable slots stay compact.
    analyzers: [Option<Box<SpectrumAnalyzer>>; MAX_EFFECTS_PER_CHANNEL],
    /// One scratch buffer is enough: chain slots process sequentially, so no
    /// slot needs to retain its dry signal once its mix has been applied.
    /// Keeping this per-chain rather than per-slot makes the full 256-slot
    /// addressable chain practical.
    dry: StereoBus,
}

impl EffectChain {
    fn new() -> Self {
        Self {
            nodes: std::array::from_fn(|_| None),
            bound: 0,
            slots: std::array::from_fn(|_| None),
            event_scratch: EventList::empty(),
            dry_align: std::array::from_fn(|_| None),
            analyzers: std::array::from_fn(|_| None),
            dry: StereoBus::with_capacity(MAX_BLOCK_SIZE),
        }
    }

    /// One slot's host and control state, if it has any.
    fn slot(&self, slot: usize) -> Option<&EffectSlot> {
        self.slots.get(slot)?.as_deref()
    }

    fn slot_mut(&mut self, slot: usize) -> Option<&mut EffectSlot> {
        self.slots.get_mut(slot)?.as_deref_mut()
    }

    /// Host controls read on the realtime path, where an empty slot must
    /// answer with its resting value rather than be absent.
    fn bypassed(&self, slot: usize) -> bool {
        self.slot(slot).is_some_and(|slot| slot.bypassed)
    }

    fn wet_dry(&self, slot: usize) -> f32 {
        self.slot(slot).map_or(1.0, |slot| slot.wet_dry)
    }

    fn input_trim(&self, slot: usize) -> f32 {
        self.slot(slot).map_or(1.0, |slot| slot.input_trim)
    }

    fn output_trim(&self, slot: usize) -> f32 {
        self.slot(slot).map_or(1.0, |slot| slot.output_trim)
    }

    /// Remove every node, queuing the boxes for off-thread disposal.
    fn clear(&mut self, reclaim: &mut Reclaim) {
        for slot in 0..MAX_EFFECTS_PER_CHANNEL {
            let displaced = ReclaimedEffect {
                node: self.nodes[slot].take(),
                align: self.dry_align[slot].take(),
                analyzer: self.analyzers[slot].take(),
                state: self.slots[slot].take(),
                channel: None,
            };
            if !displaced.is_empty() {
                reclaim.push(displaced);
            }
        }
        self.bound = 0;
    }

    fn refresh_bound(&mut self) {
        self.bound = self
            .nodes
            .iter()
            .rposition(Option::is_some)
            .map_or(0, |slot| slot + 1);
    }

    /// Install a node together with its dry-path delay, returning whichever
    /// occupants must be reclaimed. An invalid slot returns the incoming
    /// pieces so they are never dropped by the realtime caller.
    #[allow(clippy::too_many_arguments)]
    fn install(
        &mut self,
        slot: usize,
        kind: mooloop_core::EffectKind,
        resource_key: Option<u64>,
        node: Box<dyn AudioNode + Send>,
        align: Option<Box<DryAlign>>,
        analyzer: Box<SpectrumAnalyzer>,
        mut state: Box<EffectSlot>,
    ) -> ReclaimedEffect {
        if slot < MAX_EFFECTS_PER_CHANNEL {
            // Host controls belong to the slot, not to the device in it, so
            // an effect swapped into an occupied slot keeps the wet/dry and
            // trims already dialled there.
            if let Some(previous) = self.slot(slot) {
                state.bypassed = previous.bypassed;
                state.wet_dry = previous.wet_dry;
                state.input_trim = previous.input_trim;
                state.output_trim = previous.output_trim;
            }
            state.kind = Some(kind);
            state.base_params = Some(kind.default_params());
            state.resource_key = resource_key;
            self.bound = self.bound.max(slot + 1);
            ReclaimedEffect {
                node: self.nodes[slot].replace(node),
                align: std::mem::replace(&mut self.dry_align[slot], align),
                analyzer: self.analyzers[slot].replace(analyzer),
                state: self.slots[slot].replace(state),
                channel: None,
            }
        } else {
            ReclaimedEffect {
                node: Some(node),
                align,
                analyzer: Some(analyzer),
                state: Some(state),
                channel: None,
            }
        }
    }

    /// Replace only the realtime node and latency aligner. Host controls and
    /// display instrumentation deliberately remain attached to the slot.
    fn replace_if_kind(
        &mut self,
        slot: usize,
        expected_kind: mooloop_core::EffectKind,
        expected_resource_key: u64,
        resource_key: u64,
        node: Box<dyn AudioNode + Send>,
        align: Option<Box<DryAlign>>,
    ) -> ReclaimedEffect {
        let matches = self.slot(slot).is_some_and(|state| {
            state.kind == Some(expected_kind) && state.resource_key == Some(expected_resource_key)
        });
        if matches {
            if let Some(state) = self.slot_mut(slot) {
                state.resource_key = Some(resource_key);
            }
            ReclaimedEffect {
                node: self.nodes[slot].replace(node),
                align: std::mem::replace(&mut self.dry_align[slot], align),
                analyzer: None,
                state: None,
                channel: None,
            }
        } else {
            ReclaimedEffect {
                node: Some(node),
                align,
                analyzer: None,
                state: None,
                channel: None,
            }
        }
    }

    fn remove(&mut self, slot: usize) -> ReclaimedEffect {
        let removed = if slot < MAX_EFFECTS_PER_CHANNEL {
            ReclaimedEffect {
                node: self.nodes[slot].take(),
                align: self.dry_align[slot].take(),
                analyzer: self.analyzers[slot].take(),
                state: self.slots[slot].take(),
                channel: None,
            }
        } else {
            ReclaimedEffect {
                node: None,
                align: None,
                analyzer: None,
                state: None,
                channel: None,
            }
        };
        self.refresh_bound();
        removed
    }

    fn swap(&mut self, slot_a: usize, slot_b: usize) {
        if slot_a < MAX_EFFECTS_PER_CHANNEL && slot_b < MAX_EFFECTS_PER_CHANNEL {
            self.nodes.swap(slot_a, slot_b);
            self.slots.swap(slot_a, slot_b);
            self.dry_align.swap(slot_a, slot_b);
            self.analyzers.swap(slot_a, slot_b);
            self.refresh_bound();
        }
    }

    fn set_bypassed(&mut self, slot: usize, bypassed: bool) {
        if let Some(state) = self.slot_mut(slot) {
            state.bypassed = bypassed;
        }
    }

    fn queue_param(&mut self, slot: usize, id: u32, value: f32) {
        if let Some(state) = self.slot_mut(slot) {
            // Queued between blocks, so it lands at the next block's first
            // frame. Repeated writes to a parameter coalesce to its newest
            // value, matching the command ring's latest-state semantics.
            state.events.queue(TimedEvent {
                offset: 0,
                event: Event::ParamValue { id, value },
            });
        }
    }

    /// Update a knob's base value through the descriptor table. This is the
    /// only path that writes base effect state after installation.
    fn set_base_param(&mut self, slot: usize, id: u32, value: f32) -> Option<f32> {
        self.slot_mut(slot)?.base_params.as_mut()?.set(id, value)
    }

    fn base_param(&self, slot: usize, id: u32) -> Option<f32> {
        self.slot(slot)?.base_params.as_ref()?.get(id)
    }

    /// Resolve every control signal aimed at this slot into `ParamValue`
    /// events on the shared scratch list.
    ///
    /// Automation and modulation compose rather than compete: a lane supplies
    /// the **base** the knob would otherwise supply, and the matrix adds its
    /// offsets on top. That ordering is what lets an LFO wobble around a drawn
    /// curve instead of one of them winning.
    fn control_events_for_slot(
        &mut self,
        slot: usize,
        scope: EffectTarget,
        modulation: Option<&ModulationBlock<'_>>,
        automation: Option<&AutomationBlock<'_>>,
    ) {
        let Some(state) = self.slot(slot) else {
            return;
        };
        let (Some(kind), Some(params)) = (state.kind, state.base_params) else {
            return;
        };
        let ticks = modulation
            .map(|modulation| modulation.ticks)
            .into_iter()
            .chain(automation.map(|automation| automation.ticks))
            .max()
            .unwrap_or(0);

        for descriptor in kind.descriptors() {
            let destination = ParamAddr::effect(scope, slot as u8, descriptor.id);
            // The destination's own declaration decides whether modulation is
            // legal here at all -- a stepped mode selector refuses it, so an
            // LFO cannot flap an algorithm switch. Automation is unaffected: a
            // lane is explicit authored intent, not a continuous signal.
            let policy = ModDestinationDescriptor::for_param(descriptor);
            let modulated = modulation
                .is_some_and(|modulation| modulation.rack.modulates(destination, &policy));
            let curve = automation.and_then(|automation| automation.curve_for(destination));
            if !modulated && curve.is_none() {
                continue;
            }
            let Some(base) = params.get(descriptor.id) else {
                continue;
            };
            let knob_normalized = descriptor.to_normalized(base);
            for tick in 0..ticks {
                let offset = (tick * CONTROL_RATE_FRAMES) as u32;
                let base_normalized = curve
                    .as_ref()
                    .zip(automation)
                    .and_then(|(curve, automation)| automation.value_at(curve, tick))
                    .unwrap_or(knob_normalized);
                let offset_normalized = match (modulated, modulation) {
                    (true, Some(modulation)) => {
                        modulation
                            .rack
                            .offset_for(destination, &modulation.outputs[tick], &policy)
                    }
                    _ => 0.0,
                };
                let value = descriptor
                    .from_normalized((base_normalized + offset_normalized).clamp(0.0, 1.0));
                let _ = self.event_scratch.push_ordered(TimedEvent {
                    offset,
                    event: Event::ParamValue {
                        id: descriptor.id,
                        value,
                    },
                });
            }
        }
    }

    fn queue_buffer(&mut self, slot: usize, event: mooloop_core::BufferEvent) {
        if let Some(state) = self.slot_mut(slot) {
            state.events.queue(TimedEvent {
                offset: 0,
                event: Event::Buffer(event),
            });
        }
    }

    fn queue_buffer_scrub(&mut self, slot: usize, delta_frames: f32) {
        if let Some(state) = self.slot_mut(slot) {
            state.events.queue(TimedEvent {
                offset: 0,
                event: Event::BufferScrub { delta_frames },
            });
        }
    }

    fn queue_buffer_release(&mut self, slot: usize) {
        if let Some(state) = self.slot_mut(slot) {
            state.events.queue(TimedEvent {
                offset: 0,
                event: Event::BufferRelease,
            });
        }
    }

    /// Load a project's saved chain. Construction allocates, so this is a
    /// load-time operation only, never a per-block one.
    fn load(
        &mut self,
        slots: &[mooloop_core::EffectSlotState],
        sample_rate: u32,
        bpm: f64,
        reclaim: &mut Reclaim,
    ) {
        self.clear(reclaim);
        for (slot, effect) in slots.iter().take(MAX_EFFECTS_PER_CHANNEL).enumerate() {
            let node = build_effect_at_tempo(effect.params, sample_rate, bpm);
            let align = DryAlign::new(node.dry_path_latency_frames()).map(Box::new);
            let displaced = self.install(
                slot,
                effect.kind(),
                effect_resource_key(effect.params),
                node,
                align,
                Box::new(SpectrumAnalyzer::new()),
                Box::new(EffectSlot::new()),
            );
            if !displaced.is_empty() {
                reclaim.push(displaced);
            }
            // `install` seeds the kind's defaults; a saved chain overrides
            // them with what was persisted.
            if let Some(state) = self.slot_mut(slot) {
                state.base_params = Some(effect.params);
                state.bypassed = effect.bypassed;
                state.wet_dry = effect.wet_dry.clamp(0.0, 1.0);
                state.input_trim = effect.input_trim.clamp(0.0, MAX_LINEAR_GAIN);
                state.output_trim = effect.output_trim.clamp(0.0, MAX_LINEAR_GAIN);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn process(
        &mut self,
        context: &ProcessContext,
        bus: &mut StereoBus,
        scope: EffectTarget,
        device_display: Option<(&DeviceMeters, &DeviceTelemetry, usize)>,
        modulation: Option<&ModulationBlock<'_>>,
        automation: Option<&AutomationBlock<'_>>,
    ) {
        for slot in 0..self.bound {
            if let Some((_, telemetry, target)) = device_display {
                if self.nodes[slot].is_some() && telemetry.spectrum_enabled(target, slot + 1) {
                    if let Some(analyzer) = &mut self.analyzers[slot] {
                        if let Some(levels) =
                            analyzer.push(context.sample_rate, bus, context.frames)
                        {
                            telemetry.publish_spectrum(target, slot + 1, &levels);
                        }
                    }
                }
            }
            if self.bypassed(slot) {
                // A bypassed slot keeps its queued events until re-enabled, so
                // knob turns made while bypassed are not lost.
                if let Some(align) = &mut self.dry_align[slot] {
                    // Keep the dry ring tracking the passing signal, so
                    // re-enabling the slot never blends in audio captured
                    // before the bypass.
                    self.dry.l[..context.frames].copy_from_slice(&bus.l[..context.frames]);
                    self.dry.r[..context.frames].copy_from_slice(&bus.r[..context.frames]);
                    align.process(
                        &mut self.dry.l[..context.frames],
                        &mut self.dry.r[..context.frames],
                    );
                }
                if let Some((meters, _, target)) = device_display {
                    let (left, right) = bus.peak(context.frames);
                    meters.publish_input(target, slot + 1, left, right);
                    meters.publish_output(target, slot + 1, left, right);
                }
                continue;
            }
            if self.nodes[slot].is_some() {
                let input_trim = self.input_trim(slot);
                for frame in 0..context.frames {
                    bus.l[frame] *= input_trim;
                    bus.r[frame] *= input_trim;
                }
                self.dry.l[..context.frames].copy_from_slice(&bus.l[..context.frames]);
                self.dry.r[..context.frames].copy_from_slice(&bus.r[..context.frames]);
                if let Some(align) = &mut self.dry_align[slot] {
                    align.process(
                        &mut self.dry.l[..context.frames],
                        &mut self.dry.r[..context.frames],
                    );
                }
                if let Some((meters, _, target)) = device_display {
                    let (left, right) = bus.peak(context.frames);
                    meters.publish_input(target, slot + 1, left, right);
                }
                self.event_scratch.clear();
                if let Some(state) = self.slots[slot].as_mut() {
                    state.events.copy_to(&mut self.event_scratch);
                }
                self.control_events_for_slot(slot, scope, modulation, automation);
                // Read the slot's host controls before the node borrow: they
                // now live behind the same `&self` the node is taken from.
                let wet = self.wet_dry(slot);
                let trim = self.output_trim(slot);
                // `control_events_for_slot` mutates the shared scratch
                // list, so take the node borrow only after that work.
                let node = self.nodes[slot].as_mut().expect("checked above");
                node.process(context, bus, &self.event_scratch, None);
                // Equal-power crossfade. The wet paths people actually blend
                // (reverb, chorus, delay) are decorrelated from dry, where a
                // linear fade dips ~3 dB at the midpoint; correlated paths
                // (filters, EQ) now sum slightly hot at 50%. Trade-off noted
                // in docs/GAIN_STRUCTURE.md.
                let blend = wet * core::f32::consts::FRAC_PI_2;
                let (dry_gain, wet_gain) = (blend.cos(), blend.sin());
                for frame in 0..context.frames {
                    bus.l[frame] =
                        (self.dry.l[frame] * dry_gain + bus.l[frame] * wet_gain) * trim;
                    bus.r[frame] =
                        (self.dry.r[frame] * dry_gain + bus.r[frame] * wet_gain) * trim;
                }
                if let Some((meters, telemetry, target)) = device_display {
                    let (left, right) = bus.peak(context.frames);
                    meters.publish_output(target, slot + 1, left, right);
                    // Retained-audio forced returns are otherwise invisible:
                    // the device recovers silently and the only trace is this
                    // counter. Publishing it here keeps the audio thread free
                    // of logging.
                    telemetry.publish_buffer_collisions(target, slot + 1, node.buffer_collisions());
                    // Only the dynamics devices answer this; for everything
                    // else it is one `None` and the cells stay at rest.
                    if let Some(frame) = node.dynamics_frame() {
                        meters.publish_dynamics(target, slot + 1, frame);
                    }
                }
            }
            if let Some(state) = self.slots[slot].as_mut() {
                state.events.clear();
            }
        }
    }
}

/// The strip's resolved `(gain, pan)` for each control subdivision of one
/// block. Fixed-size and `Copy`: it is built on the audio thread.
#[derive(Debug, Clone, Copy)]
struct StripSegments {
    values: [(f32, f32); MAX_CONTROL_TICKS_PER_BLOCK],
    count: usize,
}

/// Resolve the strip's fader and pan for this block.
///
/// The strip is an ordinary destination -- `ParamOwner::Strip` with the
/// descriptor ids in `STRIP_DESCRIPTORS` -- but unlike a device it keeps no
/// parameter state between blocks: the output stage multiplies the bus by its
/// knob value from scratch every time. So there is no base/resolved split to
/// maintain here and nothing to restore when a route is removed; the knob is
/// already the base, and a control signal simply resolves into per-subdivision
/// gain segments on top of it.
///
/// Returns `None` when nothing drives either parameter, so the overwhelmingly
/// common still-fader case stays one pass over the block.
fn resolve_strip_segments(
    base_gain: f32,
    base_pan: f32,
    scope: EffectTarget,
    modulation: &ModulationBlock<'_>,
    automation: Option<&AutomationBlock<'_>>,
) -> Option<StripSegments> {
    let ticks = modulation
        .ticks
        .max(automation.map_or(0, |automation| automation.ticks))
        .min(MAX_CONTROL_TICKS_PER_BLOCK);
    if ticks == 0 {
        return None;
    }
    let mut segments = StripSegments {
        values: [(base_gain, base_pan); MAX_CONTROL_TICKS_PER_BLOCK],
        count: ticks,
    };
    let mut driven = false;
    for descriptor in STRIP_DESCRIPTORS.iter() {
        let destination = ParamAddr::strip(scope, descriptor.id);
        let policy = ModDestinationDescriptor::for_param(descriptor);
        let modulated = modulation.rack.modulates(destination, &policy);
        let curve = automation.and_then(|automation| automation.curve_for(destination));
        if !modulated && curve.is_none() {
            continue;
        }
        driven = true;
        let knob = if descriptor.id == STRIP_PARAM_VOLUME {
            base_gain
        } else {
            base_pan
        };
        let knob_normalized = descriptor.to_normalized(knob);
        for tick in 0..ticks {
            let base_normalized = curve
                .as_ref()
                .zip(automation)
                .and_then(|(curve, automation)| automation.value_at(curve, tick))
                .unwrap_or(knob_normalized);
            let offset_normalized = if modulated {
                modulation
                    .rack
                    .offset_for(destination, &modulation.outputs[tick], &policy)
            } else {
                0.0
            };
            let value =
                descriptor.from_normalized((base_normalized + offset_normalized).clamp(0.0, 1.0));
            if descriptor.id == STRIP_PARAM_VOLUME {
                segments.values[tick].0 = value;
            } else {
                segments.values[tick].1 = value;
            }
        }
    }
    driven.then_some(segments)
}

/// Shared output stage: linear gain, a source-pan or bus-balance application,
/// and a mute that stops the strip contributing without stopping it processing
/// (so effect tails on a muted strip still decay instead of freezing).
struct OutputStage {
    gain: f32,
    pan: f32,
    muted: bool,
}

impl OutputStage {
    fn new(gain: f32) -> Self {
        Self {
            gain,
            pan: 0.0,
            muted: false,
        }
    }

    fn set_volume(&mut self, volume: f32) {
        // Channels and buses gain up to +12 dB, same headroom as the effect
        // container's trims.
        self.gain = volume.clamp(0.0, MAX_LINEAR_GAIN);
    }

    fn set_pan(&mut self, pan: f32) {
        self.pan = pan.clamp(-1.0, 1.0);
    }

    fn apply_pan(&self, bus: &mut StereoBus, frames: usize) {
        let (pan_l, pan_r) = pan_gains(self.pan);
        bus.apply_stereo_gain(self.gain * pan_l, self.gain * pan_r, frames);
    }

    /// Apply gain and pan, stepping them per control subdivision when a
    /// source or a lane is driving them. `segments` is `None` for the ordinary
    /// case of a still fader, which stays a single pass over the block.
    fn apply_pan_segments(
        &self,
        bus: &mut StereoBus,
        frames: usize,
        segments: Option<&StripSegments>,
    ) {
        let Some(segments) = segments else {
            self.apply_pan(bus, frames);
            return;
        };
        for (tick, &(gain, pan)) in segments.values.iter().take(segments.count).enumerate() {
            let start = tick * CONTROL_RATE_FRAMES;
            if start >= frames {
                break;
            }
            let end = (start + CONTROL_RATE_FRAMES).min(frames);
            let (pan_l, pan_r) = pan_gains(pan);
            bus.apply_stereo_gain_range(gain * pan_l, gain * pan_r, start, end);
        }
    }

    fn apply_balance(&self, bus: &mut StereoBus, frames: usize) {
        let (balance_l, balance_r) = balance_gains(self.pan);
        bus.apply_stereo_gain(self.gain * balance_l, self.gain * balance_r, frames);
    }
}

/// One mixer bus: an effect chain, an output stage, and the index of the bus
/// it feeds. `output` is always lower than the bus's own index (see
/// `mooloop_core::mixer`), which is what lets `process_block` render the whole
/// bank in one descending pass with no sorting or scratch buffers.
struct BusStrip {
    effects: EffectChain,
    bus: StereoBus,
    output: OutputStage,
}

impl BusStrip {
    fn new() -> Self {
        Self {
            effects: EffectChain::new(),
            bus: StereoBus::with_capacity(MAX_BLOCK_SIZE),
            // Unity, not a channel's 0.8: see `mooloop_core::MixerBus::new`.
            output: OutputStage::new(1.0),
        }
    }

    fn reset(&mut self, reclaim: &mut Reclaim) {
        self.effects.clear(reclaim);
        self.output = OutputStage::new(1.0);
    }
}

/// One channel's storage, moved as a unit between the control thread and the
/// graph. The graph unpacks it into its parallel vectors immediately: those
/// stay separate because the block loop borrows them with different
/// mutabilities at once, and bundling would make that a conflict.
pub struct ChannelStorage {
    strip: Box<ChannelStrip>,
    events: Box<EventList>,
    control_outputs: Box<ControlOutputs>,
}

pub struct ChannelStrip {
    sampler: Sampler,
    drum_synth: DrumSynth,
    mono_synth: MonoSynth,
    poly_synth: PolySynth,
    mlm1: MlM1,
    active_source: DeviceKind,
    /// The knob value for the active generator's parameters. The device
    /// retains only the value it was last sent, so this is what lets a knob
    /// move underneath an active lane without the two fighting -- the same
    /// split `EffectChain::base_params` makes for effects.
    source_base: GeneratorParams,
    effects: EffectChain,
    bus: StereoBus,
    output: OutputStage,
    /// Mixer bus this channel feeds.
    destination: u8,
}

impl ChannelStrip {
    fn new(sample_slot: Arc<ArcSwapOption<SampleData>>, sample_rate: u32) -> Self {
        Self {
            sampler: Sampler::new(sample_slot, SamplerParams::default(), sample_rate),
            drum_synth: DrumSynth::new(DrumSynthParams::default(), sample_rate),
            mono_synth: MonoSynth::new(MonoSynthParams::default(), sample_rate),
            poly_synth: PolySynth::new(PolySynthParams::default(), sample_rate),
            mlm1: MlM1::new(MlM1Params::default(), sample_rate),
            active_source: DeviceKind::Sampler,
            source_base: GeneratorParams::Sampler(SamplerParams::default()),
            effects: EffectChain::new(),
            bus: StereoBus::with_capacity(MAX_BLOCK_SIZE),
            output: OutputStage::new(0.8),
            destination: MASTER_BUS,
        }
    }

    fn reset_sources_to_defaults(&mut self, source: DeviceKind) {
        self.source_base = default_generator_params(source);
        self.sampler.reset();
        self.drum_synth.reset();
        self.mono_synth.reset();
        self.poly_synth.reset();
        self.mlm1.reset();
        self.sampler.set_params(SamplerParams::default());
        self.drum_synth.set_params(DrumSynthParams::default());
        self.mono_synth.set_params(MonoSynthParams::default());
        self.poly_synth.set_params(PolySynthParams::default());
        self.mlm1.set_params(MlM1Params::default());
        self.active_source = source;
    }

    fn reset_slot(&mut self, source: DeviceKind, reclaim: &mut Reclaim) {
        self.reset_sources_to_defaults(source);
        self.effects.clear(reclaim);
        self.output = OutputStage::new(0.8);
        self.destination = MASTER_BUS;
    }

    /// Install a channel's source device state.
    ///
    /// Takes `sample_rate` because the sampler's stretch pool is sized to it
    /// and provisioned here. That is safe precisely because `load_project`
    /// only ever runs while a `RenderState` is being prepared on the control
    /// thread or for an offline render -- never from the audio callback -- so
    /// this is the one path that may allocate the pool inline instead of
    /// installing it structurally.
    fn load_source(&mut self, source: &ChannelSource, sample_rate: u32) {
        self.reset_sources_to_defaults(source.kind());
        self.source_base = match source {
            ChannelSource::Sampler(state) => {
                self.sampler.set_params(state.params);
                // Reconcile intent with state, so a saved project plays
                // stretched from its first note rather than after a round
                // trip through the structural queue.
                if state.params.stretch_enabled {
                    self.sampler.install_stretch(Box::new(StretchPool::new(
                        state.params.stretch_mode,
                        sample_rate,
                        MAX_SAMPLER_VOICES as usize,
                    )));
                } else {
                    self.sampler.take_stretch();
                }
                GeneratorParams::Sampler(state.params)
            }
            ChannelSource::DrumSynth(state) => {
                self.drum_synth.set_params(state.params);
                GeneratorParams::DrumSynth
            }
            ChannelSource::MonoSynth(state) => {
                self.mono_synth.set_params(state.params);
                GeneratorParams::MonoSynth(state.params)
            }
            ChannelSource::PolySynth(state) => {
                self.poly_synth.set_params(state.params);
                GeneratorParams::PolySynth(state.params)
            }
            ChannelSource::MlM1(state) => {
                self.mlm1.set_params(state.params);
                GeneratorParams::MlM1(state.params)
            }
        };
    }

    fn choke_group(&self) -> u8 {
        match self.active_source {
            DeviceKind::Sampler => self.sampler.choke_group(),
            DeviceKind::DrumSynth => self.drum_synth.choke_group(),
            DeviceKind::MonoSynth | DeviceKind::PolySynth | DeviceKind::MlM1 => 0,
        }
    }

    fn process(&mut self, context: &ProcessContext, events: &EventList) {
        match self.active_source {
            DeviceKind::Sampler => self.sampler.process(context, &mut self.bus, events, None),
            DeviceKind::DrumSynth => self
                .drum_synth
                .process(context, &mut self.bus, events, None),
            DeviceKind::MonoSynth => self
                .mono_synth
                .process(context, &mut self.bus, events, None),
            DeviceKind::PolySynth => self
                .poly_synth
                .process(context, &mut self.bus, events, None),
            DeviceKind::MlM1 => self.mlm1.process(context, &mut self.bus, events, None),
        }
    }
}

/// Sum one bus into another. The two indices are unrelated now that routing
/// is arbitrary, so the disjoint borrow is taken by splitting at whichever is
/// higher rather than assuming the destination is lower.
fn mix_into(buses: &mut [BusStrip], from: usize, into: usize, frames: usize) {
    if from == into || from >= buses.len() || into >= buses.len() {
        return;
    }
    let (left, right) = buses.split_at_mut(from.max(into));
    if from < into {
        let source = &left[from];
        right[0].bus.add_from(&source.bus, frames);
    } else {
        let source = &right[0];
        left[into].bus.add_from(&source.bus, frames);
    }
}

/// Keep a channel's bus assignment inside the bank. A stale index from the GUI
/// lands on the master rather than silently muting the channel.
fn clamp_bus(bus: u8) -> u8 {
    if (bus as usize) < MAX_BUSES {
        bus
    } else {
        MASTER_BUS
    }
}

fn inject_choke_events(choke_groups: &[u8], events: &mut [Box<EventList>]) {
    let active = choke_groups.len().min(events.len());
    for source in 0..active {
        let group = choke_groups[source];
        if group == 0 {
            continue;
        }
        for target in 0..active {
            if source == target || choke_groups[target] != group {
                continue;
            }
            let (source_events, target_events) = if source < target {
                let (left, right) = events.split_at_mut(target);
                (&left[source], &mut right[0])
            } else {
                let (left, right) = events.split_at_mut(source);
                (&right[0], &mut left[target])
            };
            for event in source_events.iter() {
                if matches!(event.event, Event::NoteOn { .. }) {
                    target_events.push_ordered(TimedEvent {
                        offset: event.offset,
                        event: Event::Choke,
                    });
                }
            }
        }
    }
}

/// 4/4 throughout, matching the sequencer's grid.
const BEATS_PER_BAR: f32 = 4.0;

/// Tuple fields a control change has retuned, waiting for the next note to
/// fire. Held rather than applied immediately so turning a knob mid-gesture
/// bends the *next* edit instead of restarting the current one.
#[derive(Debug, Clone, Copy, Default)]
struct BufferCcState {
    window_beats: Option<f32>,
    offset_beats: Option<f32>,
    repeat: Option<u32>,
}

impl BufferCcState {
    fn apply(&self, mut event: mooloop_core::BufferEvent) -> mooloop_core::BufferEvent {
        if let Some(window) = self.window_beats {
            event.window_beats = Some(window);
        }
        if let Some(offset) = self.offset_beats {
            event.offset_beats = offset;
        }
        if let Some(repeat) = self.repeat {
            event.repeat = Some(repeat);
        }
        event
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RenderReport {
    pub position_tick: u64,
    pub beat_in_bar: u8,
    pub playing: bool,
    pub peak_l: f32,
    pub peak_r: f32,
}

pub(crate) struct RenderState {
    transport: Transport,
    sequencer: Sequencer,
    /// One entry per channel the project actually has, not per addressable
    /// channel. Boxed so the vector reserves its full addressable length in
    /// pointers and grows without ever reallocating on the audio thread.
    strips: Vec<Box<ChannelStrip>>,
    /// Kept so a channel can be materialized after construction: a strip
    /// needs its channel's sample slot, and the control thread builds them.
    sample_slots: Arc<Vec<Arc<ArcSwapOption<SampleData>>>>,
    /// The full bus bank, master first. Always `MAX_BUSES` long, so assigning
    /// a channel to any bus is a bounded mutation rather than an allocation.
    buses: Vec<BusStrip>,
    /// Destinations and their matching render order, compiled together off the
    /// audio thread. The executor only installs or walks this value.
    bus_graph: CompiledBusGraph,
    events: Vec<Box<EventList>>,
    /// The saved matrix and the runnable sources are deliberately separate:
    /// the former is editable/persisted configuration; the latter contains
    /// LFO phase and other realtime-only state.
    modulation: Vec<ModRack>,
    modulators: Vec<ModulatorRack>,
    /// A full block of resolved control signal is 8 KiB; reserving one for
    /// every addressable channel cost 2 MiB before a project existed
    /// (`docs/plans/modulator-capacity/`).
    control_outputs: Vec<Box<ControlOutputs>>,
    sample_rate: u32,
    /// Nodes displaced from effect slots this block, awaiting handoff to the
    /// reclaim ring (realtime playback) or plain drop (offline render).
    reclaim: Reclaim,
    /// Where per-bus peaks are published for the mixer. Offline renders keep
    /// their own unread instance rather than paying for an `Option` check per
    /// bus per block.
    meters: Arc<BusMeters>,
    device_meters: Arc<DeviceMeters>,
    device_telemetry: Arc<DeviceTelemetry>,
    /// How MIDI input drives a buffer insert. `None` until the control layer
    /// configures one, so an unmapped project pays nothing for MIDI beyond
    /// decoding it.
    buffer_midi: Arc<ArcSwapOption<mooloop_core::midi::BufferMidiMap>>,
    buffer_cc: BufferCcState,
    playhead_meters: Arc<PlayheadMeters>,
    modulator_meters: Arc<ModulatorMeters>,
    /// The sample browser's audition voice, if something is playing. One at
    /// a time: a new preview replaces the old, and the retired sample's
    /// ownership returns to the UI thread through the reclaim ring.
    preview: Option<PreviewVoice>,
    preview_retired: Vec<Arc<SampleData>>,
    /// Linear preview gain, shared with the GUI so the knob is heard live.
    /// It starts at the operating level rather than unity: an audition is
    /// usually a full-scale commercial file, and the browser should not be
    /// 12 dB louder than the project it plays over.
    ///
    /// `REFERENCE_PEAK_DBFS` here, not the generator output reference the
    /// sampler's trim uses, because a preview goes straight to the master
    /// with no channel strip -- so it never pays the pan law the sampler's
    /// 3 dB of extra trim exists to cancel. Both land at -12 dBFS.
    preview_gain: Arc<AtomicU32>,
}

/// One-shot straight to the master output: no envelope, no channel strip.
/// A browser preview should sound like the file, not like the project.
struct PreviewVoice {
    sample: Arc<SampleData>,
    position: usize,
}

impl RenderState {
    pub fn new(sample_rate: u32, sample_slots: Arc<Vec<Arc<ArcSwapOption<SampleData>>>>) -> Self {
        #![allow(clippy::let_and_return)]
        // Deliberately empty. Channels are materialized from a project on
        // this thread, or pushed one at a time through the structural ring.
        let strips = Vec::with_capacity(MAX_CHANNELS);
        let slots_for_growth = sample_slots.clone();
        let mut state = Self {
            transport: Transport::new(sample_rate),
            sequencer: Sequencer::new(1, 1, DEFAULT_STEPS as usize, mooloop_core::Ppq::DEFAULT),
            strips,
            sample_slots: slots_for_growth,
            buses: (0..MAX_BUSES).map(|_| BusStrip::new()).collect(),
            bus_graph: CompiledBusGraph::default(),
            events: Vec::with_capacity(MAX_CHANNELS),
            // Small enough that reserving the addressable length outright
            // costs 433 KiB and saves boxing every control-path access.
            modulation: (0..MAX_CHANNELS).map(|_| ModRack::default()).collect(),
            modulators: (0..MAX_CHANNELS).map(|_| ModulatorRack::new()).collect(),
            control_outputs: Vec::with_capacity(MAX_CHANNELS),
            sample_rate,
            reclaim: Vec::new(),
            meters: BusMeters::new(),
            device_meters: DeviceMeters::new(),
            device_telemetry: DeviceTelemetry::new(),
            buffer_midi: Arc::new(ArcSwapOption::empty()),
            buffer_cc: BufferCcState::default(),
            playhead_meters: PlayheadMeters::new(),
            modulator_meters: ModulatorMeters::new(),
            preview: None,
            preview_retired: Vec::new(),
            preview_gain: Arc::new(AtomicU32::new(mooloop_core::gain::db_to_linear(mooloop_core::gain::REFERENCE_PEAK_DBFS).to_bits())),
        };
        // The sequencer starts with one channel, so the graph starts with
        // storage for one. `live_channels` tolerates the two disagreeing, but
        // they should not disagree at rest.
        let initial = state.sequencer.active_channels();
        state.grow_channels(initial);
        state
    }

    /// Point bus metering at the array the GUI reads. Called once at startup,
    /// before the realtime thread exists.
    pub(crate) fn attach_meters(&mut self, meters: Arc<BusMeters>) {
        self.meters = meters;
    }

    pub(crate) fn attach_device_meters(&mut self, meters: Arc<DeviceMeters>) {
        self.device_meters = meters;
    }

    /// Points the preview voice at the gain cell the GUI's volume knob
    /// writes. Read once per block, so knob turns are heard live.
    pub(crate) fn attach_preview_gain(&mut self, gain: Arc<AtomicU32>) {
        self.preview_gain = gain;
    }

    /// Starts, restarts, or stops the preview voice. Returns the replaced
    /// sample, if there was one, for off-thread disposal.
    pub(crate) fn apply_preview(&mut self, command: PreviewCommand) -> Option<Arc<SampleData>> {
        let replaced = self.preview.take().map(|voice| voice.sample);
        match command {
            PreviewCommand::Play { sample } => {
                self.preview = Some(PreviewVoice {
                    sample,
                    position: 0,
                });
            }
            PreviewCommand::Stop => {}
        }
        replaced
    }

    /// Hands back a sample whose preview finished, for disposal off the
    /// realtime thread.
    pub(crate) fn pop_retired_preview(&mut self) -> Option<Arc<SampleData>> {
        self.preview_retired.pop()
    }

    /// Sums the preview voice into the master bus. Deliberately after the
    /// bus walk: the preview bypasses the project's chains, balance, and
    /// mute so the file is heard as the file.
    fn render_preview(&mut self, frames: usize) {
        let Some(voice) = self.preview.as_mut() else {
            return;
        };
        let gain = f32::from_bits(self.preview_gain.load(Ordering::Relaxed));
        let samples = &voice.sample.frames;
        let start = voice.position.min(samples.len());
        let count = (start + frames).min(samples.len()) - start;
        let master = &mut self.buses[MASTER_BUS as usize].bus;
        for index in 0..count {
            let frame = samples[start + index];
            master.l[index] += frame[0] * gain;
            master.r[index] += frame[1] * gain;
        }
        let played = start + count;
        if played >= samples.len() {
            let voice = self.preview.take().expect("preview checked above");
            self.preview_retired.push(voice.sample);
        } else {
            self.preview.as_mut().expect("preview checked above").position = played;
        }
    }

    pub(crate) fn attach_device_telemetry(&mut self, telemetry: Arc<DeviceTelemetry>) {
        self.device_telemetry = telemetry;
    }

    pub(crate) fn attach_playhead_meters(&mut self, meters: Arc<PlayheadMeters>) {
        self.playhead_meters = meters;
    }

    pub(crate) fn attach_modulator_meters(&mut self, meters: Arc<ModulatorMeters>) {
        self.modulator_meters = meters;
    }

    pub fn from_project(
        sample_rate: u32,
        project: &Project,
        samples: &[Option<Arc<SampleData>>],
    ) -> Self {
        let fallback = SampleData::default_kick(sample_rate);
        let slots = Arc::new(
            (0..MAX_CHANNELS)
                .map(|index| {
                    let sample = samples.get(index).cloned().flatten().or_else(|| {
                        project.channels.get(index).and_then(|channel| {
                            match &channel.setup.source {
                                ChannelSource::Sampler(state)
                                    if matches!(
                                        state.sample,
                                        mooloop_core::SampleReference::Builtin { .. }
                                    ) =>
                                {
                                    Some(fallback.clone())
                                }
                                _ => None,
                            }
                        })
                    });
                    Arc::new(ArcSwapOption::from(sample))
                })
                .collect(),
        );
        let mut state = Self::new(sample_rate, slots);
        state.load_project(project);
        state
    }

    /// Build one channel's storage. Allocates, so it belongs on the control
    /// thread — either here during a project install, or in the `AddChannel`
    /// structural command that carries the result across.
    pub(crate) fn build_channel(
        sample_slot: Arc<ArcSwapOption<SampleData>>,
        sample_rate: u32,
    ) -> Box<ChannelStorage> {
        Box::new(ChannelStorage {
            strip: Box::new(ChannelStrip::new(sample_slot, sample_rate)),
            events: Box::new(EventList::empty()),
            control_outputs: Box::new(
                [[0.0; MAX_MODULATORS_PER_CHANNEL]; MAX_CONTROL_TICKS_PER_BLOCK],
            ),
        })
    }

    /// Take one channel's storage into the graph's parallel vectors. The box
    /// is the transport, not an optimisation: it was allocated on the control
    /// thread and the audio thread only ever moves what is inside it.
    #[allow(clippy::boxed_local)]
    fn push_channel(&mut self, storage: Box<ChannelStorage>) {
        let ChannelStorage {
            strip,
            events,
            control_outputs,
        } = *storage;
        self.strips.push(strip);
        self.events.push(events);
        self.control_outputs.push(control_outputs);
    }

    /// Channels that both exist to the sequencer and have storage behind
    /// them. Those two can disagree for one block — a project can declare
    /// more channels than the graph has been handed storage for — and a
    /// per-channel pass must render the ones it has rather than panic on the
    /// ones it does not.
    fn live_channels(&self) -> usize {
        self.sequencer.active_channels().min(self.strips.len())
    }

    /// Materialize channels up to `count`. Allocates; control thread only.
    fn grow_channels(&mut self, count: usize) {
        let sample_rate = self.sample_rate;
        while self.strips.len() < count.min(MAX_CHANNELS) {
            let slot = self.sample_slots[self.strips.len()].clone();
            self.push_channel(Self::build_channel(slot, sample_rate));
        }
    }

    pub fn load_project(&mut self, project: &Project) {
        // A loaded project decides how many channels exist. This runs on the
        // control thread inside `install_project`, so allocating here is the
        // point rather than a hazard.
        self.grow_channels(project.channels.len());
        self.transport.stop();
        self.transport.set_tempo(project.bpm.into());
        self.sequencer.load_project(project);
        for (index, strip) in self.strips.iter_mut().enumerate() {
            if let Some(channel) = project.channels.get(index) {
                strip.load_source(&channel.setup.source, self.sample_rate);
                strip.output.muted = channel.setup.channel.muted;
                strip.output.set_volume(channel.setup.channel.volume);
                strip.output.set_pan(channel.setup.channel.pan);
                strip.destination = clamp_bus(channel.setup.channel.bus);
                // `load_project` runs while a complete RenderState is prepared
                // on the control thread (or for offline export), never from the
                // JACK callback, so constructing boxed nodes is acceptable.
                // Displaced nodes still collect in `reclaim` for callers that
                // deliberately reuse a state off-thread.
                strip.effects.load(
                    &channel.setup.effects,
                    self.sample_rate,
                    self.transport.bpm,
                    &mut self.reclaim,
                );
            } else {
                strip.reset_slot(DeviceKind::Sampler, &mut self.reclaim);
            }
        }
        for index in 0..MAX_CHANNELS {
            let modulation = project
                .channels
                .get(index)
                .map(|channel| channel.setup.modulation)
                .unwrap_or_default();
            self.set_channel_modulation(index, modulation);
        }
        for (index, strip) in self.buses.iter_mut().enumerate() {
            match project.buses.get(index) {
                Some(setup) => {
                    strip.output.muted = setup.bus.muted;
                    strip.output.set_volume(setup.bus.volume);
                    strip.output.set_pan(setup.bus.pan);
                    strip.effects.load(
                        &setup.effects,
                        self.sample_rate,
                        self.transport.bpm,
                        &mut self.reclaim,
                    );
                }
                None => strip.reset(&mut self.reclaim),
            }
        }
        // A file whose routing does not sort is repaired to everything-to-master
        // rather than rejected, so a hand-edited or future-format song still
        // opens and makes sound.
        self.bus_graph = compile_bus_graph(&project.buses).unwrap_or_default();
    }

    /// Resolve an effect address to the chain that owns it. Both arms are
    /// bounds-checked, so a stale index from the GUI is a no-op rather than a
    /// panic on the audio thread.
    fn chain_for<'a>(
        strips: &'a mut [Box<ChannelStrip>],
        buses: &'a mut [BusStrip],
        target: EffectTarget,
    ) -> Option<&'a mut EffectChain> {
        match target {
            EffectTarget::Channel(index) => strips.get_mut(index as usize).map(|s| &mut s.effects),
            EffectTarget::Bus(index) => buses.get_mut(index as usize).map(|b| &mut b.effects),
        }
    }

    fn chain_mut(&mut self, target: EffectTarget) -> Option<&mut EffectChain> {
        Self::chain_for(&mut self.strips, &mut self.buses, target)
    }

    fn chain(&self, target: EffectTarget) -> Option<&EffectChain> {
        match target {
            EffectTarget::Channel(index) => self.strips.get(index as usize).map(|s| &s.effects),
            EffectTarget::Bus(index) => self.buses.get(index as usize).map(|b| &b.effects),
        }
    }

    /// Install a complete saved rack while retaining a same-kind LFO's phase.
    /// Copying the small matrix is realtime-safe; `ModulatorRack::set_slot`
    /// owns the phase-preserving detail.
    /// Replace one channel's whole rack. Used where a rack genuinely arrives
    /// entire — project load, and a channel added or removed — not for
    /// ordinary edits, which name one fact each through `edit_modulation`.
    fn set_channel_modulation(&mut self, channel: usize, modulation: ModRack) {
        self.edit_modulation(channel, |rack| {
            *rack = modulation;
            true
        });
    }

    /// Apply one edit to a channel's rack and put everything that depends on
    /// it back in step.
    ///
    /// Every modulation command is this shape: change one fact in the saved
    /// rack, mirror the slots whose behaviour actually moved into the DSP
    /// rack, and hand back any destination that just lost its last route.
    /// The diff lives here rather than at each call site because it is the
    /// only part a narrow command can get *wrong* rather than merely
    /// expensive: without it, a removed route leaves the device holding
    /// whatever the control signal last resolved, until someone happens to
    /// touch that knob again.
    ///
    /// `edit` reports whether it changed anything, so a command that names a
    /// slot or a route this rack does not hold costs a comparison and stops.
    fn edit_modulation(&mut self, channel: usize, edit: impl FnOnce(&mut ModRack) -> bool) {
        let Some(saved) = self.modulation.get_mut(channel) else {
            return;
        };
        let previous = *saved;
        if !edit(saved) {
            return;
        }
        let modulation = *saved;
        // The runtime rack holds behaviour, not identity: it is addressed by
        // slot, and the durable ids stay on the control side where routes are
        // resolved. Only a slot whose parameters moved is re-set, so turning
        // one knob does not touch the seven modules beside it -- and a module
        // that keeps its kind keeps its phase, cursor and envelope stage
        // because `set_slot` retunes in place.
        if let Some(runtime) = self.modulators.get_mut(channel) {
            for (slot, entry) in modulation.slots.into_iter().enumerate() {
                let params = entry.map(|entry| entry.params);
                if previous.slots[slot].map(|entry| entry.params) == params {
                    continue;
                }
                runtime.set_slot(slot, params);
            }
        }
        for destination in previous.destinations() {
            if modulation
                .destinations()
                .any(|current| current == destination)
            {
                continue;
            }
            self.restore_base_param(destination);
        }
    }

    /// Return one destination to its knob value at the next block. Removing a
    /// lane or a matrix route otherwise leaves the device holding whatever the
    /// control signal last resolved, until someone happens to touch that knob.
    fn restore_base_param(&mut self, destination: ParamAddr) {
        match destination.owner {
            ParamOwner::Effect { slot } => {
                let Some(chain) = self.chain_mut(destination.scope) else {
                    return;
                };
                if let Some(base) = chain.base_param(slot as usize, destination.param) {
                    chain.queue_param(slot as usize, destination.param, base);
                }
            }
            // A generator has no queue between blocks; its base is applied
            // directly, which is safe because `set_params` allocates nothing.
            ParamOwner::Source => {
                let EffectTarget::Channel(channel) = destination.scope else {
                    return;
                };
                let Some(strip) = self.strips.get_mut(channel as usize) else {
                    return;
                };
                let base = strip.source_base;
                match base {
                    GeneratorParams::Sampler(params) => strip.sampler.set_params(params),
                    GeneratorParams::MonoSynth(params) => strip.mono_synth.set_params(params),
                    GeneratorParams::PolySynth(params) => strip.poly_synth.set_params(params),
                    GeneratorParams::MlM1(params) => strip.mlm1.set_params(params),
                    GeneratorParams::DrumSynth => {}
                }
            }
            // Neither needs one. The strip's output stage keeps no parameter
            // state between blocks -- it re-reads its knob every block -- and
            // a modulator's own parameters are not modulation destinations
            // yet, so there is nothing left holding a stale resolved value.
            ParamOwner::Modulator { .. } | ParamOwner::Strip => {}
        }
    }

    /// Whether a source will overwrite this effect parameter this block, and
    /// so whether writing the knob's base straight through would be undone.
    /// A route aimed at a destination that refuses modulation does not count:
    /// it resolves to nothing, so the knob must still reach the device.
    fn effect_is_modulated(&self, target: EffectTarget, slot: u8, id: u32) -> bool {
        let EffectTarget::Channel(channel) = target else {
            return false;
        };
        let Some(descriptor) = self
            .chain(target)
            .and_then(|chain| chain.slot(slot as usize).and_then(|state| state.kind))
            .and_then(|kind| kind.descriptor(id))
        else {
            return false;
        };
        let policy = ModDestinationDescriptor::for_param(descriptor);
        self.modulation
            .get(channel as usize)
            .is_some_and(|rack| rack.modulates(ParamAddr::effect(target, slot, id), &policy))
    }

    /// Change the stored base, then immediately queue it only if a control
    /// signal is not about to resolve that destination for this block.
    fn set_effect_param(&mut self, target: EffectTarget, slot: u8, id: u32, value: f32) {
        let Some(value) = self
            .chain_mut(target)
            .and_then(|chain| chain.set_base_param(slot as usize, id, value))
        else {
            return;
        };
        if !self.effect_is_modulated(target, slot, id) {
            if let Some(chain) = self.chain_mut(target) {
                chain.queue_param(slot as usize, id, value);
            }
        }
    }

    /// Tick one channel's source rack for every 32-frame subdivision and
    /// capture each output before advancing it. The final subdivision can be
    /// shorter; its event still starts at its exact frame offset.
    fn tick_channel_modulators(
        &mut self,
        channel: usize,
        frames: usize,
        gate_ticks: &[[NoteGateEvents; MAX_CHANNELS]; MAX_CONTROL_TICKS_PER_BLOCK],
    ) -> usize {
        let sample_rate = self.sample_rate;
        let bpm = self.transport.bpm;
        let Some(runtime) = self.modulators.get_mut(channel) else {
            return 0;
        };
        let Some(outputs) = self.control_outputs.get_mut(channel) else {
            return 0;
        };
        let mut tick = 0;
        for offset in (0..frames).step_by(CONTROL_RATE_FRAMES) {
            let span = (frames - offset).min(CONTROL_RATE_FRAMES);
            runtime.tick_with_note_gates(sample_rate, span, bpm, channel, &gate_ticks[tick]);
            outputs[tick] = *runtime.outputs();
            tick += 1;
        }
        tick
    }

    /// Apply a structural change (install/remove of a boxed node). Called on
    /// the realtime thread from the ordered control stream; the boxes
    /// themselves were allocated on the control thread. Returns whatever the
    /// edit displaced, so the caller can hand it to the reclaim ring.
    /// Apply a structural edit, returning whatever it displaced so the caller
    /// can send it back for off-thread disposal. Returns the reclaim variant
    /// rather than a bare effect, because not everything structural is an
    /// effect any more.
    pub(crate) fn apply_structural(
        &mut self,
        cmd: StructuralCommand,
    ) -> Option<StructuralReclaim> {
        match cmd {
            StructuralCommand::InstallEffect {
                target,
                slot,
                kind,
                resource_key,
                node,
                align,
                analyzer,
                state,
            } => {
                // `chain_for` borrows the two strip vectors rather than all of
                // `self`, so `reclaim` stays independently borrowable here.
                if let Some(chain) = Self::chain_for(&mut self.strips, &mut self.buses, target) {
                    Some(chain.install(
                        slot as usize,
                        kind,
                        resource_key,
                        node,
                        align,
                        analyzer,
                        state,
                    ))
                } else {
                    Some(ReclaimedEffect {
                        node: Some(node),
                        align,
                        analyzer: Some(analyzer),
                        state: Some(state),
                        channel: None,
                    })
                }
                .filter(|displaced| !displaced.is_empty())
                .map(StructuralReclaim::Effect)
            }
            StructuralCommand::ReplaceEffect {
                target,
                slot,
                expected_kind,
                expected_resource_key,
                resource_key,
                node,
                align,
            } => {
                if let Some(chain) = Self::chain_for(&mut self.strips, &mut self.buses, target) {
                    Some(chain.replace_if_kind(
                        slot as usize,
                        expected_kind,
                        expected_resource_key,
                        resource_key,
                        node,
                        align,
                    ))
                } else {
                    Some(ReclaimedEffect {
                        node: Some(node),
                        align,
                        analyzer: None,
                        state: None,
                        channel: None,
                    })
                }
                .filter(|displaced| !displaced.is_empty())
                .map(StructuralReclaim::Effect)
            }
            StructuralCommand::AddChannel { storage, source } => {
                let channel = self.sequencer.active_channels();
                if channel >= MAX_CHANNELS {
                    return Some(StructuralReclaim::Effect(ReclaimedEffect {
                        node: None,
                        align: None,
                        analyzer: None,
                        state: None,
                        channel: Some(storage),
                    }));
                }
                // The graph only grows. A channel removed earlier left its
                // storage behind, so this may already have somewhere to go —
                // in which case the storage that arrived goes straight back
                // rather than being dropped on this thread.
                let spare = channel < self.strips.len();
                let returned = if spare { Some(storage) } else { self.push_channel(storage); None };
                if let Some(strip) = self.strips.get_mut(channel) {
                    strip.reset_slot(source, &mut self.reclaim);
                }
                self.set_channel_modulation(channel, ModRack::default());
                self.sequencer.clear_channel(channel);
                self.sequencer.set_active_channels(channel + 1);
                returned
                    .map(|storage| ReclaimedEffect {
                        node: None,
                        align: None,
                        analyzer: None,
                        state: None,
                        channel: Some(storage),
                    })
                    .map(StructuralReclaim::Effect)
            }
            StructuralCommand::RemoveEffect { target, slot } => {
                Self::chain_for(&mut self.strips, &mut self.buses, target)
                    .map(|chain| chain.remove(slot as usize))
            }
            .filter(|displaced| !displaced.is_empty())
            .map(StructuralReclaim::Effect),
            StructuralCommand::SetSamplerStretch { channel, pool } => {
                let Some(strip) = self.strips.get_mut(channel as usize) else {
                    // Nothing to install into. Hand the pool straight back
                    // rather than dropping it here -- this is the realtime
                    // thread, and an unaddressable channel is not a reason to
                    // free 1.6 MB on it.
                    return pool.map(StructuralReclaim::SamplerStretch);
                };
                match pool {
                    Some(pool) => strip.sampler.install_stretch(pool),
                    None => strip.sampler.take_stretch(),
                }
                .map(StructuralReclaim::SamplerStretch)
            }
        }
    }

    pub fn apply_command(&mut self, cmd: EngineCommand) {
        match cmd {
            EngineCommand::Play => self.transport.play(),
            EngineCommand::Pause => self.transport.pause(),
            EngineCommand::Stop => self.transport.stop(),
            EngineCommand::SetTempo(bpm) => self.transport.set_tempo(bpm),
            EngineCommand::SetSwing(percent) => self.sequencer.set_swing(percent),
            EngineCommand::SetCurrentPattern(pattern) => {
                self.sequencer.set_current_pattern(pattern as usize)
            }
            EngineCommand::AddPattern => {
                self.sequencer.add_pattern();
            }
            EngineCommand::SetPlaybackMode(mode) => self.sequencer.set_playback_mode(mode),
            EngineCommand::SetPatternLength {
                pattern,
                length_steps,
            } => self
                .sequencer
                .set_pattern_length(pattern as usize, length_steps as usize),
            EngineCommand::SetPlaylistPlacement {
                pattern,
                start_tick,
                on,
            } => {
                self.sequencer
                    .set_playlist_placement(pattern as usize, start_tick, on);
            }
            EngineCommand::RemoveChannel => {
                let active = self.sequencer.active_channels();
                if let Some(channel) = active.checked_sub(1) {
                    self.strips[channel].reset_slot(DeviceKind::Sampler, &mut self.reclaim);
                    self.set_channel_modulation(channel, ModRack::default());
                    self.sequencer.set_active_channels(channel);
                }
            }
            EngineCommand::SetChannelMuted { channel, muted } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.output.muted = muted;
                }
            }
            EngineCommand::SetChannelVolume { channel, volume } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.output.set_volume(volume);
                }
            }
            EngineCommand::SetChannelPan { channel, pan } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.output.set_pan(pan);
                }
            }
            EngineCommand::SetChannelBus { channel, bus } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.destination = clamp_bus(bus);
                }
            }
            EngineCommand::SetBusMuted { bus, muted } => {
                if let Some(strip) = self.buses.get_mut(bus as usize) {
                    strip.output.muted = muted;
                }
            }
            EngineCommand::SetBusVolume { bus, volume } => {
                if let Some(strip) = self.buses.get_mut(bus as usize) {
                    strip.output.set_volume(volume);
                }
            }
            EngineCommand::SetBusPan { bus, pan } => {
                if let Some(strip) = self.buses.get_mut(bus as usize) {
                    strip.output.set_pan(pan);
                }
            }
            EngineCommand::InstallBusGraph { graph } => self.bus_graph = graph,
            EngineCommand::SetStep {
                pattern,
                channel,
                step,
                on,
                note,
                velocity,
            } => self.sequencer.set_step(
                pattern as usize,
                channel as usize,
                step as usize,
                on,
                note,
                velocity,
            ),
            EngineCommand::UpsertNote {
                pattern,
                channel,
                note,
            } => {
                self.sequencer
                    .upsert_note(pattern as usize, channel as usize, note);
            }
            EngineCommand::RemoveNote {
                pattern,
                channel,
                id,
            } => {
                self.sequencer
                    .remove_note(pattern as usize, channel as usize, id);
            }
            EngineCommand::OpenAutomationLane {
                pattern,
                channel,
                target,
            } => {
                self.sequencer
                    .open_automation_lane(pattern as usize, channel as usize, target);
            }
            EngineCommand::RemoveAutomationLane {
                pattern,
                channel,
                target,
            } => {
                if self
                    .sequencer
                    .remove_automation_lane(pattern as usize, channel as usize, target)
                {
                    self.restore_base_param(target);
                }
            }
            EngineCommand::ClearAutomationLane {
                pattern,
                channel,
                target,
            } => {
                if self
                    .sequencer
                    .clear_automation_lane(pattern as usize, channel as usize, target)
                {
                    self.restore_base_param(target);
                }
            }
            EngineCommand::UpsertAutomationPoint {
                pattern,
                channel,
                target,
                point,
            } => {
                self.sequencer.upsert_automation_point(
                    pattern as usize,
                    channel as usize,
                    target,
                    point,
                );
            }
            EngineCommand::RemoveAutomationPoint {
                pattern,
                channel,
                target,
                id,
            } => {
                self.sequencer.remove_automation_point(
                    pattern as usize,
                    channel as usize,
                    target,
                    id,
                );
            }
            EngineCommand::SetChannelSamplerParams { channel, params } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.sampler.set_params(params);
                    strip.source_base = GeneratorParams::Sampler(params);
                }
            }
            EngineCommand::SetChannelSource { channel, source } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.reset_sources_to_defaults(source);
                    strip.source_base = default_generator_params(source);
                }
            }
            EngineCommand::SetChannelDrumSynthParams { channel, params } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.drum_synth.set_params(params);
                    strip.source_base = GeneratorParams::DrumSynth;
                }
            }
            EngineCommand::SetChannelMonoSynthParams { channel, params } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.mono_synth.set_params(params);
                    strip.source_base = GeneratorParams::MonoSynth(params);
                }
            }
            EngineCommand::SetChannelMlM1Params { channel, params } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.mlm1.set_params(params);
                    strip.source_base = GeneratorParams::MlM1(params);
                }
            }
            EngineCommand::SetChannelPolySynthParams { channel, params } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.poly_synth.set_params(params);
                    strip.source_base = GeneratorParams::PolySynth(params);
                }
            }
            EngineCommand::SwapEffectSlots {
                target,
                slot_a,
                slot_b,
            } => {
                if let Some(chain) = self.chain_mut(target) {
                    chain.swap(slot_a as usize, slot_b as usize);
                }
            }
            EngineCommand::SetEffectBypassed {
                target,
                slot,
                bypassed,
            } => {
                if let Some(chain) = self.chain_mut(target) {
                    chain.set_bypassed(slot as usize, bypassed);
                }
            }
            EngineCommand::SetEffectWetDry {
                target,
                slot,
                wet_dry,
            } => {
                if let Some(chain) = self.chain_mut(target) {
                    if let Some(value) = chain.slot_mut(slot as usize).map(|state| &mut state.wet_dry) {
                        *value = wet_dry.clamp(0.0, 1.0);
                    }
                }
            }
            EngineCommand::SetEffectInputTrim {
                target,
                slot,
                input_trim,
            } => {
                if let Some(chain) = self.chain_mut(target) {
                    if let Some(value) = chain.slot_mut(slot as usize).map(|state| &mut state.input_trim) {
                        *value = input_trim.clamp(0.0, MAX_LINEAR_GAIN);
                    }
                }
            }
            EngineCommand::SetEffectOutputTrim {
                target,
                slot,
                output_trim,
            } => {
                if let Some(chain) = self.chain_mut(target) {
                    if let Some(value) = chain.slot_mut(slot as usize).map(|state| &mut state.output_trim) {
                        *value = output_trim.clamp(0.0, MAX_LINEAR_GAIN);
                    }
                }
            }
            EngineCommand::SetEffectParam {
                target,
                slot,
                id,
                value,
            } => self.set_effect_param(target, slot, id, value),
            // Every modulation edit names one fact. The rack-wide diff still
            // runs behind each of them, so a route that disappears here
            // returns its destination to its knob value at the next block
            // exactly as it did when the whole rack travelled.
            EngineCommand::SetModulatorParam {
                channel,
                slot,
                id,
                value,
            } => self.edit_modulation(channel as usize, |rack| {
                let Some(params) = rack.params_mut(slot as usize) else {
                    return false;
                };
                params.set(id, value);
                true
            }),
            EngineCommand::InstallModulator {
                channel,
                slot,
                source,
                params,
            } => self.edit_modulation(channel as usize, |rack| {
                rack.install_with_id(slot as usize, source, params)
            }),
            EngineCommand::ClearModulator { channel, slot } => {
                self.edit_modulation(channel as usize, |rack| rack.clear(slot as usize))
            }
            EngineCommand::MoveModulator { channel, from, to } => self
                .edit_modulation(channel as usize, |rack| {
                    rack.move_module(from as usize, to as usize)
                }),
            // A route names its source by durable id, so one that arrives
            // before (or after) the module it names is refused rather than
            // aimed at whatever else occupies that slot.
            EngineCommand::SetModRoute { channel, route } => {
                self.edit_modulation(channel as usize, |rack| rack.apply_route(route).is_some())
            }
            EngineCommand::RemoveModRoute {
                channel,
                source,
                destination,
            } => self.edit_modulation(channel as usize, |rack| {
                rack.remove_route_by_source(source, destination)
            }),
            EngineCommand::TriggerBuffer {
                target,
                slot,
                event,
            } => {
                if let Some(chain) = self.chain_mut(target) {
                    chain.queue_buffer(slot as usize, event);
                }
            }
            EngineCommand::ReleaseBuffer { target, slot } => {
                if let Some(chain) = self.chain_mut(target) {
                    chain.queue_buffer_release(slot as usize);
                }
            }
        }
    }

    /// Share the control layer's mapping cell. Same transport as the sample
    /// slots: the non-realtime side swaps a whole map in, and the audio
    /// thread only ever loads it, so no map is built or dropped here.
    pub(crate) fn attach_buffer_midi_map(
        &mut self,
        map: Arc<ArcSwapOption<mooloop_core::midi::BufferMidiMap>>,
    ) {
        self.buffer_midi = map;
    }

    /// Translate one block's MIDI input into buffer events. Runs before the
    /// block renders, so input acts on the audio it arrived with.
    ///
    /// Note says what and how long, velocity says how hard, and a CC carries
    /// whatever else the tuple needs — the note table holds the shape of an
    /// edit and the controls bend it.
    pub(crate) fn apply_midi(&mut self, messages: &[mooloop_core::MidiMessage]) {
        use mooloop_core::midi::BufferCcTarget;
        use mooloop_core::MidiKind;

        if messages.is_empty() {
            return;
        }
        let map = self.buffer_midi.load();
        let Some(map) = map.as_deref().copied() else {
            return;
        };
        for message in messages {
            if !map.accepts(message) {
                continue;
            }
            let slot = map.slot as usize;
            match message.kind {
                MidiKind::NoteOn { note, velocity } => {
                    if let Some(event) = map.note_event(note, velocity) {
                        let event = self.buffer_cc.apply(event);
                        if let Some(chain) = self.chain_mut(map.target) {
                            chain.queue_buffer(slot, event);
                        }
                    }
                }
                MidiKind::NoteOff { note } => {
                    // Only a note this map owns may release; an unmapped key
                    // must not cancel an edit it never started.
                    if map.note_event(note, 1).is_some() {
                        if let Some(chain) = self.chain_mut(map.target) {
                            chain.queue_buffer_release(slot);
                        }
                    }
                }
                MidiKind::ControlChange { controller, value } => {
                    let Some(target) = map.cc_target(controller) else {
                        continue;
                    };
                    match target {
                        BufferCcTarget::Scrub { encoding } => {
                            let ticks = encoding.delta(value);
                            if ticks != 0 {
                                let delta = f64::from(ticks) * self.scrub_frames_per_tick();
                                if let Some(chain) = self.chain_mut(map.target) {
                                    chain.queue_buffer_scrub(slot, delta as f32);
                                }
                            }
                        }
                        // Absolute assignments retune the *next* edit rather
                        // than re-firing one: turning a knob mid-gesture
                        // should not restart the gesture.
                        BufferCcTarget::WindowBars { bars } => {
                            let bucket = mooloop_core::cc_bucket(value, bars.max(1));
                            self.buffer_cc.window_beats =
                                Some(f32::from(bucket + 1) * BEATS_PER_BAR);
                        }
                        BufferCcTarget::OffsetBeats { beats } => {
                            let bucket = mooloop_core::cc_bucket(value, beats.max(1));
                            self.buffer_cc.offset_beats = Some(-f32::from(bucket + 1));
                        }
                        BufferCcTarget::Repeat { max } => {
                            let bucket = mooloop_core::cc_bucket(value, max.max(1));
                            self.buffer_cc.repeat = Some(u32::from(bucket) + 1);
                        }
                    }
                }
                MidiKind::PitchBend { .. } => {}
            }
        }
    }

    /// Frames the head travels per encoder tick. One tick is a 128th of a
    /// beat, so a 128-tick-per-revolution wheel turns one beat per turn —
    /// close enough to a platter's feel to be playable without calibration.
    fn scrub_frames_per_tick(&self) -> f64 {
        self.sample_rate as f64 * 60.0 / self.transport.bpm.max(1.0) / 128.0
    }

    pub fn process_block(&mut self, frames: usize) -> RenderReport {
        self.process_block_inner(frames, true)
    }

    pub fn process_once_block(&mut self, frames: usize) -> RenderReport {
        self.process_block_inner(frames, false)
    }

    fn process_block_inner(&mut self, frames: usize, looping: bool) -> RenderReport {
        let frames = frames.min(MAX_BLOCK_SIZE);
        let ticks_per_sample = self.transport.ticks_per_sample();
        let position_frames = self.transport.frames_played();
        let (start_tick, end_tick) = self.transport.advance(frames);

        for events in &mut self.events {
            events.clear();
        }
        if self.transport.playing {
            if looping {
                self.sequencer.schedule(
                    start_tick,
                    end_tick,
                    frames,
                    ticks_per_sample,
                    &mut self.events,
                );
            } else {
                self.sequencer.schedule_once(
                    start_tick,
                    end_tick,
                    frames,
                    ticks_per_sample,
                    &mut self.events,
                );
            }
            let mut choke_groups = [0; MAX_CHANNELS];
            for (index, strip) in self
                .strips
                .iter()
                .enumerate()
                .take(self.live_channels())
            {
                if !strip.output.muted {
                    choke_groups[index] = strip.choke_group();
                }
            }
            inject_choke_events(
                &choke_groups[..self.live_channels()],
                &mut self.events,
            );
        }

        let context = ProcessContext {
            sample_rate: self.sample_rate,
            frames,
            playing: self.transport.playing,
            bpm: self.transport.bpm,
            position_ticks: start_tick,
            position_frames,
        };
        // Modulators must all advance before anything borrows the sequencer for
        // automation, and every channel's rack advances even while muted so
        // unmuting does not restart its phase.
        let active_channels = self.live_channels();
        let mut modulator_ticks = [0usize; MAX_CHANNELS];
        let mut gate_ticks =
            [[NoteGateEvents::default(); MAX_CHANNELS]; MAX_CONTROL_TICKS_PER_BLOCK];
        // Not an iterator loop: `gate_ticks` is indexed by control tick first
        // and by channel second, so the loop variable is not this array's
        // outer index and enumerating it would walk the wrong axis.
        #[allow(clippy::needless_range_loop)]
        for source_channel in 0..active_channels {
            for event in self.events[source_channel].iter() {
                let tick = (event.offset as usize / CONTROL_RATE_FRAMES)
                    .min(MAX_CONTROL_TICKS_PER_BLOCK - 1);
                let gate = &mut gate_ticks[tick][source_channel];
                match event.event {
                    Event::NoteOn { .. } => gate.note_ons = gate.note_ons.saturating_add(1),
                    Event::NoteOff { .. } => gate.note_offs = gate.note_offs.saturating_add(1),
                    Event::Choke => gate.choke = true,
                    _ => {}
                }
            }
        }
        for (index, ticks) in modulator_ticks.iter_mut().enumerate().take(active_channels) {
            *ticks = self.tick_channel_modulators(index, frames, &gate_ticks);
        }
        // Lanes resolve whether or not the transport is running: stopped, the
        // playhead simply holds still and the destination sits at the value
        // drawn under it. Making automation conditional on playback would mean
        // a knob that jumps the moment you press play.
        let automation = (frames > 0).then(|| AutomationBlock {
            sequencer: &self.sequencer,
            start_tick,
            ticks_per_sample,
            ticks: frames.div_ceil(CONTROL_RATE_FRAMES),
        });
        for strip in &mut self.buses {
            strip.bus.clear(frames);
        }
        for (index, &ticks) in modulator_ticks.iter().take(active_channels).enumerate() {
            // Published before the mute check: a muted channel's modulators
            // still run, so its knobs should still animate rather than freeze
            // on whatever the last audible block left behind.
            if ticks > 0 {
                self.modulator_meters
                    .publish(index, &self.control_outputs[index][ticks - 1]);
            }
            if self.strips[index].output.muted {
                continue;
            }
            let modulation = ModulationBlock {
                rack: &self.modulation[index],
                outputs: &self.control_outputs[index],
                ticks,
            };
            // The generator's control events go into the channel's own note
            // list, which is the event stream it already splits its block on.
            // Written inline rather than as a method because the automation
            // block holds `&self.sequencer` for the whole loop, and only the
            // compiler's field-level borrow splitting can see that
            // `self.events` and `self.strips` are disjoint from it.
            {
                let base = self.strips[index].source_base;
                let scope = EffectTarget::Channel(index as u8);
                for descriptor in base.kind().descriptors() {
                    let destination = ParamAddr {
                        scope,
                        owner: ParamOwner::Source,
                        param: descriptor.id,
                    };
                    let policy = ModDestinationDescriptor::for_param(descriptor);
                    let modulated = modulation.rack.modulates(destination, &policy);
                    let curve = automation
                        .as_ref()
                        .and_then(|automation| automation.curve_for(destination));
                    if !modulated && curve.is_none() {
                        continue;
                    }
                    let Some(knob) = base.get(descriptor.id) else {
                        continue;
                    };
                    let knob_normalized = descriptor.to_normalized(knob);
                    for tick in 0..ticks.max(automation.as_ref().map_or(0, |a| a.ticks)) {
                        let base_normalized = curve
                            .as_ref()
                            .zip(automation.as_ref())
                            .and_then(|(curve, automation)| automation.value_at(curve, tick))
                            .unwrap_or(knob_normalized);
                        let offset_normalized = if modulated {
                            modulation.rack.offset_for(
                                destination,
                                &modulation.outputs[tick],
                                &policy,
                            )
                        } else {
                            0.0
                        };
                        let value = descriptor
                            .from_normalized((base_normalized + offset_normalized).clamp(0.0, 1.0));
                        let _ = self.events[index].push_ordered(TimedEvent {
                            offset: (tick * CONTROL_RATE_FRAMES) as u32,
                            event: Event::ParamValue {
                                id: descriptor.id,
                                value,
                            },
                        });
                    }
                }
            }
            let strip_segments = resolve_strip_segments(
                self.strips[index].output.gain,
                self.strips[index].output.pan,
                EffectTarget::Channel(index as u8),
                &modulation,
                automation.as_ref(),
            );
            let strip = &mut self.strips[index];
            strip.bus.clear(frames);
            strip.process(&context, &self.events[index]);
            let source_peak = strip.bus.peak(frames);
            self.device_meters
                .publish_output(index, 0, source_peak.0, source_peak.1);
            self.playhead_meters
                .publish(index, &strip.sampler.voice_positions());
            strip.effects.process(
                &context,
                &mut strip.bus,
                EffectTarget::Channel(index as u8),
                Some((&self.device_meters, &self.device_telemetry, index)),
                Some(&modulation),
                automation.as_ref(),
            );
            strip
                .output
                .apply_pan_segments(&mut strip.bus, frames, strip_segments.as_ref());
            if let Some(destination) = self.buses.get_mut(strip.destination as usize) {
                destination.bus.add_from(&strip.bus, frames);
            }
        }

        // Walk the compiled schedule. Every bus is guaranteed to appear after
        // everything feeding it, so one pass suffices whatever the routing
        // looks like; the master sorts last and keeps its audio, since it is
        // what the caller reads.
        let mut master_peak = (0.0, 0.0);
        for slot in 0..self.buses.len() {
            let index = self.bus_graph.render_order()[slot] as usize;
            let Some(strip) = self.buses.get_mut(index) else {
                continue;
            };
            // The bus head's input meter reads what the bus received this
            // block, before its own chain touches it.
            let (input_l, input_r) = strip.bus.peak(frames);
            self.device_meters
                .publish_input(MAX_CHANNELS + index, 0, input_l, input_r);
            strip.effects.process(
                &context,
                &mut strip.bus,
                EffectTarget::Bus(index as u8),
                Some((
                    &self.device_meters,
                    &self.device_telemetry,
                    MAX_CHANNELS + index,
                )),
                None,
                automation.as_ref(),
            );
            strip.output.apply_balance(&mut strip.bus, frames);
            // A muted bus still processes, so a delay or reverb tail on it
            // decays instead of freezing, but contributes nothing — and meters
            // as silent, matching what is heard rather than what is running.
            let (peak_l, peak_r) = if strip.output.muted {
                (0.0, 0.0)
            } else {
                strip.bus.peak(frames)
            };
            self.meters.publish(index, peak_l, peak_r);

            if index == MASTER_BUS as usize {
                master_peak = (peak_l, peak_r);
            } else if !strip.output.muted {
                let destination = self.bus_graph.destination(index) as usize;
                mix_into(&mut self.buses, index, destination, frames);
            }
        }
        // After the walk on purpose: the preview bypasses every chain, so it
        // is heard raw and does not move the mixer's meters.
        self.render_preview(frames);
        let (peak_l, peak_r) = master_peak;
        RenderReport {
            position_tick: self.transport.position_ticks as u64,
            beat_in_bar: self.transport.beat_in_bar(),
            playing: self.transport.playing,
            peak_l,
            peak_r,
        }
    }

    pub fn master(&self) -> &StereoBus {
        &self.buses[MASTER_BUS as usize].bus
    }

    pub fn play(&mut self) {
        self.transport.play();
    }

    pub fn pause(&mut self) {
        self.transport.pause();
    }

    pub fn ticks_per_sample(&self) -> f64 {
        self.transport.ticks_per_sample()
    }

    pub fn song_length_ticks(&self) -> u32 {
        self.sequencer.song_length_ticks()
    }

    pub fn pattern_length_ticks(&self, pattern: usize) -> Option<u32> {
        self.sequencer.pattern_length_ticks(pattern)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{NoteEvent, ProjectChannel};

    fn test_strip() -> ChannelStrip {
        let slot = Arc::new(ArcSwapOption::empty());
        ChannelStrip::new(slot, 48_000)
    }

    #[test]
    fn channel_output_controls_are_bounded() {
        let mut strip = test_strip();
        strip.output.set_volume(MAX_LINEAR_GAIN + 1.0);
        strip.output.set_pan(-2.0);
        assert_eq!(strip.output.gain, MAX_LINEAR_GAIN);
        assert_eq!(strip.output.pan, -1.0);
        strip.output.set_volume(-1.0);
        strip.output.set_pan(2.0);
        assert_eq!(strip.output.gain, 0.0);
        assert_eq!(strip.output.pan, 1.0);
    }

    #[test]
    fn matching_choke_group_receives_sample_timed_choke() {
        let mut events = [
            Box::new(EventList::empty()),
            Box::new(EventList::empty()),
            Box::new(EventList::empty()),
        ];
        events[0].push(TimedEvent {
            offset: 37,
            event: Event::NoteOn {
                id: 1,
                note: 60,
                velocity: 100,
            },
        });
        inject_choke_events(&[2, 2, 3], &mut events[..]);
        assert_eq!(events[0].len(), 1);
        assert_eq!(events[1].iter().next().unwrap().event, Event::Choke);
        assert!(events[2].is_empty());
    }

    #[test]
    fn choke_is_ordered_before_a_simultaneous_note_on() {
        let mut events = [Box::new(EventList::empty()), Box::new(EventList::empty())];
        for (channel, id) in events.iter_mut().zip([1, 2]) {
            channel.push(TimedEvent {
                offset: 0,
                event: Event::NoteOn {
                    id,
                    note: 60,
                    velocity: 100,
                },
            });
        }

        inject_choke_events(&[1, 1], &mut events[..]);

        for channel in &events {
            assert!(matches!(channel.iter().next().unwrap().event, Event::Choke));
            assert!(matches!(
                channel.iter().nth(1).unwrap().event,
                Event::NoteOn { .. }
            ));
        }
    }

    #[test]
    fn preview_voice_plays_replaces_and_retires() {
        let slots = Arc::new(
            (0..MAX_CHANNELS)
                .map(|_| Arc::new(ArcSwapOption::empty()))
                .collect(),
        );
        let mut render = RenderState::new(48_000, slots);
        let first = Arc::new(SampleData {
            frames: vec![[0.5, -0.5]; 1_000],
            sample_rate: 48_000,
            root_note: 60,
        });
        assert!(
            render
                .apply_preview(PreviewCommand::Play {
                    sample: first.clone(),
                })
                .is_none()
        );
        render.process_block(512);
        let master = render.master();
        assert!(
            master.l[..512].iter().any(|sample| *sample != 0.0),
            "the preview must reach the master bus while the transport is stopped"
        );

        // Replacing a playing preview hands the old sample back for UI-side
        // disposal before it can ever be dropped on the realtime thread.
        let second = Arc::new(SampleData {
            frames: vec![[1.0, 1.0]; 10],
            sample_rate: 48_000,
            root_note: 60,
        });
        let replaced = render.apply_preview(PreviewCommand::Play { sample: second.clone() });
        assert!(Arc::ptr_eq(&replaced.expect("a preview was playing"), &first));

        // Ten frames are gone after one block; retirement follows.
        render.process_block(512);
        let retired = render.pop_retired_preview().expect("voice finished");
        assert!(Arc::ptr_eq(&retired, &second));
        assert!(render.pop_retired_preview().is_none());

        // And the preview is silent again.
        render.process_block(512);
        assert!(
            !render
                .master()
                .l[..512]
                .iter()
                .any(|sample| *sample != 0.0)
        );
    }

    #[test]
    fn preview_gain_cell_is_heard_live() {
        let slots = Arc::new(
            (0..MAX_CHANNELS)
                .map(|_| Arc::new(ArcSwapOption::empty()))
                .collect(),
        );
        let mut render = RenderState::new(48_000, slots);
        let loud = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        render.attach_preview_gain(loud.clone());
        render.apply_preview(PreviewCommand::Play {
            sample: Arc::new(SampleData {
                frames: vec![[0.5, 0.5]; 4_000],
                sample_rate: 48_000,
                root_note: 60,
            }),
        });
        loud.store(0.25f32.to_bits(), Ordering::Relaxed);
        render.process_block(512);
        for sample in &render.master().l[..512] {
            assert!(
                (*sample - 0.125).abs() < 1e-6,
                "the shared gain cell must gate the preview immediately"
            );
        }
    }

    #[test]
    fn project_load_replaces_preallocated_state() {
        let mut project = Project {
            bpm: 173,
            ..Project::default()
        };
        project.pattern_lengths[0] = 32;
        project.channels[0].notes[0].push(mooloop_core::NoteEvent::new(1, 24, 12, 60, 100));
        let render = RenderState::from_project(48_000, &project, &[]);
        assert_eq!(render.pattern_length_ticks(0), Some(32 * 24));
        assert!((render.ticks_per_sample() - (173.0 * 96.0 / 60.0 / 48_000.0)).abs() < 1e-12);
    }

    #[test]
    fn project_load_installs_the_last_addressable_effect_slot() {
        let mut project = Project::default();
        project.channels[0].setup.effects = (0..MAX_EFFECTS_PER_CHANNEL)
            .map(|_| mooloop_core::EffectSlotState::of_kind(mooloop_core::EffectKind::Filter))
            .collect();

        let render = RenderState::from_project(48_000, &project, &[]);
        assert!(render.strips[0].effects.nodes[MAX_EFFECTS_PER_CHANNEL - 1].is_some());
        assert_eq!(render.strips[0].effects.bound, MAX_EFFECTS_PER_CHANNEL);
    }

    /// Builds a project around `channel` with one triggering note. A fresh
    /// sampler channel has no sample loaded (and would render silent), so
    /// these generic routing/mixing/metering tests — which need *some*
    /// audible source, not specifically sampler behavior — point a sampler
    /// channel at the legacy builtin kick the same way an old saved project
    /// would.
    /// A saved project that stretches must play stretched from its first
    /// note. `load_project` runs on the control thread, so it provisions the
    /// pool inline rather than waiting for a round trip through the
    /// structural queue.
    #[test]
    fn loading_a_project_provisions_the_stretch_it_asks_for() {
        let mut project = synth_project(ProjectChannel::sampler(0, 1));
        if let Some(state) = project.channels[0].setup.sampler_state_mut() {
            state.params.stretch_enabled = true;
            state.params.stretch_ratio = 2.0;
        }
        let render = RenderState::from_project(48_000, &project, &[]);
        assert!(render.strips[0].sampler.has_stretch());
        assert!(render.strips[0].sampler.wants_stretch());
        // Channels the project does not describe are not provisioned because
        // they are not built at all -- a stronger statement than "built and
        // left empty", and the one the graph actually makes now.
        assert_eq!(render.strips.len(), 1);
    }

    /// The inverse, which is the part that actually saves the memory: a
    /// project that does not stretch must not carry the state for it.
    #[test]
    fn loading_a_project_without_stretch_provisions_nothing() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let render = RenderState::from_project(48_000, &project, &[]);
        assert!(!render.strips[0].sampler.has_stretch());
    }

    /// Installing and removing through the structural path, with the displaced
    /// state coming back rather than being freed on the audio thread.
    #[test]
    fn the_structural_path_installs_and_reclaims_stretch_state() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let mut render = RenderState::from_project(48_000, &project, &[]);

        let installed = render.apply_structural(StructuralCommand::SetSamplerStretch {
            channel: 0,
            pool: Some(Box::new(StretchPool::new(
                mooloop_core::StretchMode::Music,
                48_000,
                MAX_SAMPLER_VOICES as usize,
            ))),
        });
        assert!(installed.is_none(), "nothing was displaced by the first install");
        assert!(render.strips[0].sampler.has_stretch());

        let replaced = render.apply_structural(StructuralCommand::SetSamplerStretch {
            channel: 0,
            pool: Some(Box::new(StretchPool::new(
                mooloop_core::StretchMode::Grain,
                48_000,
                MAX_SAMPLER_VOICES as usize,
            ))),
        });
        assert!(
            matches!(replaced, Some(StructuralReclaim::SamplerStretch(_))),
            "the displaced pool must be handed back, not dropped here"
        );

        let removed = render.apply_structural(StructuralCommand::SetSamplerStretch {
            channel: 0,
            pool: None,
        });
        assert!(matches!(
            removed,
            Some(StructuralReclaim::SamplerStretch(_))
        ));
        assert!(!render.strips[0].sampler.has_stretch());
    }

    /// `apply_structural` hands the pool back when the channel does not
    /// exist, because dropping it would free megabytes on the realtime
    /// thread.
    ///
    /// That guard was written when it could not fire: `MAX_CHANNELS` is 256
    /// and the command addresses channels with a `u8`, so every address was
    /// backed by a strip that had been reserved up front. Reserving them
    /// stopped (`docs/plans/modulator-capacity/`) -- the graph now builds
    /// only the channels a project describes -- so an in-range `u8` can
    /// address a strip that is simply not there, and the guard became load
    /// bearing rather than defensive. This is the case that reaches it.
    #[test]
    fn a_pool_aimed_at_a_channel_that_does_not_exist_comes_back() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        // Addressable, and beyond what this project materialized.
        assert!(usize::from(u8::MAX) < MAX_CHANNELS);
        assert!(render.strips.len() <= usize::from(u8::MAX));

        let turned_away = render.apply_structural(StructuralCommand::SetSamplerStretch {
            channel: u8::MAX,
            pool: Some(Box::new(StretchPool::new(
                mooloop_core::StretchMode::Music,
                48_000,
                1,
            ))),
        });
        assert!(
            matches!(turned_away, Some(StructuralReclaim::SamplerStretch(_))),
            "a pool with nowhere to go must be handed back, not freed here"
        );
    }

    fn synth_project(mut channel: ProjectChannel) -> Project {
        if let Some(sampler) = channel.setup.sampler_state_mut() {
            sampler.sample = mooloop_core::SampleReference::Builtin {
                id: "default_kick".into(),
            };
        }
        let mut project = Project {
            channels: vec![channel],
            ..Project::default()
        };
        project.channels[0].notes[0].push(NoteEvent::new(1, 0, 96, 60, 127));
        project
    }

    #[test]
    fn synth_sources_render_without_sample_data() {
        for channel in [
            ProjectChannel::drum_synth(0, 1),
            ProjectChannel::mono_synth(0, 1),
            ProjectChannel::poly_synth(0, 1),
        ] {
            let project = synth_project(channel);
            let mut render = RenderState::from_project(48_000, &project, &[]);
            render.play();
            let report = render.process_block(512);
            assert!(report.peak_l > 0.001, "synth source was silent");
        }
    }

    #[test]
    fn source_switch_resets_inactive_voice_state() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        assert!(render.process_block(256).peak_l > 0.001);

        render.apply_command(EngineCommand::SetChannelSource {
            channel: 0,
            source: DeviceKind::MonoSynth,
        });
        render.process_block(256);
        render.apply_command(EngineCommand::SetChannelSource {
            channel: 0,
            source: DeviceKind::Sampler,
        });
        assert_eq!(render.process_block(256).peak_l, 0.0);
    }

    /// Adding a channel allocates, so it goes through the structural ring
    /// with storage built off-thread — the same route an effect node takes.
    fn add_channel(render: &mut RenderState, source: DeviceKind) {
        let storage = RenderState::build_channel(Arc::new(ArcSwapOption::from(None)), 48_000);
        let returned = render.apply_structural(StructuralCommand::AddChannel { storage, source });
        // Reused storage comes straight back rather than being dropped here.
        drop(returned);
    }

    #[test]
    fn readding_a_channel_resets_its_preallocated_slot() {
        let mut render = RenderState::from_project(48_000, &Project::default(), &[]);
        add_channel(&mut render, DeviceKind::DrumSynth);
        render.apply_command(EngineCommand::SetStep {
            pattern: 0,
            channel: 1,
            step: 0,
            on: true,
            note: 60,
            velocity: 127,
        });
        render.play();
        assert!(render.process_block(256).peak_l > 0.001);

        render.apply_command(EngineCommand::RemoveChannel);
        add_channel(&mut render, DeviceKind::DrumSynth);
        render.apply_command(EngineCommand::Stop);
        render.apply_command(EngineCommand::Play);
        assert_eq!(render.process_block(256).peak_l, 0.0);
    }

    fn strip_route(param: u32, depth: f32) -> ModRack {
        let mut rack = ModRack::default();
        rack.install(
            0,
            mooloop_core::ModulatorParams::Lfo(mooloop_core::ModLfoParams::default()),
        );
        rack.add_route(mooloop_core::ModRoute::to_slot(
            0,
            ParamAddr::strip(EffectTarget::Channel(0), param),
            depth,
            mooloop_core::ModPolarity::Bipolar,
        ))
        .expect("route fits the matrix");
        rack
    }

    /// The strip's fader is an ordinary destination: a source resolves it into
    /// one gain per control subdivision, centred on the knob value, and leaves
    /// pan untouched. The offset sums in normalized space and clamps there, so
    /// a swing that would drive the fader below zero lands on silence rather
    /// than a negative gain.
    #[test]
    fn a_source_resolves_the_strip_fader_into_control_rate_segments() {
        let rack = strip_route(mooloop_core::STRIP_PARAM_VOLUME, 0.25);
        let mut outputs: ControlOutputs =
            [[0.0; MAX_MODULATORS_PER_CHANNEL]; MAX_CONTROL_TICKS_PER_BLOCK];
        outputs[0][0] = 1.0;
        outputs[1][0] = -1.0;
        let modulation = ModulationBlock {
            rack: &rack,
            outputs: &outputs,
            ticks: 2,
        };

        let segments =
            resolve_strip_segments(0.8, 0.0, EffectTarget::Channel(0), &modulation, None)
                .expect("a routed fader resolves");
        assert_eq!(segments.count, 2);

        // 0.8 of the 0..MAX_LINEAR_GAIN range is 0.2 normalized. +0.25 lands
        // at 0.45; -0.25 would land at -0.05 and clamps to silence.
        let volume = mooloop_core::strip_descriptor(mooloop_core::STRIP_PARAM_VOLUME).unwrap();
        assert!((segments.values[0].0 - volume.from_normalized(0.45)).abs() < 1e-6);
        assert_eq!(segments.values[1].0, 0.0);
        assert_eq!(segments.values[0].1, 0.0);
        assert_eq!(segments.values[1].1, 0.0);
    }

    /// A still fader resolves to no segments at all, so the ordinary block
    /// stays a single pass over the bus rather than a per-subdivision walk.
    #[test]
    fn an_undriven_strip_resolves_to_no_segments() {
        let outputs: ControlOutputs =
            [[0.0; MAX_MODULATORS_PER_CHANNEL]; MAX_CONTROL_TICKS_PER_BLOCK];
        let modulation = ModulationBlock {
            rack: &ModRack::default(),
            outputs: &outputs,
            ticks: 2,
        };
        assert!(
            resolve_strip_segments(0.8, 0.0, EffectTarget::Channel(0), &modulation, None).is_none()
        );
    }

    /// A modulated fader must actually reach the audio. Two subdivisions with
    /// opposite source outputs scale the same block by different gains, which
    /// an unmodulated render does not do.
    #[test]
    fn strip_modulation_reaches_the_rendered_block() {
        let mut channel = ProjectChannel::sampler(0, 1);
        channel.setup.modulation.install(0, mooloop_core::ModulatorParams::Lfo(
            mooloop_core::ModLfoParams {
                // A quarter-cycle per 32-frame subdivision at 48 kHz.
                rate_hz: 375.0,
                waveform: mooloop_core::ModLfoWaveform::Square,
                ..mooloop_core::ModLfoParams::default()
            },
        ));
        channel.setup.modulation = {
            let mut rack = strip_route(mooloop_core::STRIP_PARAM_VOLUME, 0.5);
            rack.slots = channel.setup.modulation.slots;
            rack
        };
        let project = synth_project(channel);

        let mut flat_project = project.clone();
        flat_project.channels[0].setup.modulation.routes =
            [None; mooloop_core::MAX_MOD_ROUTES_PER_CHANNEL];
        let mut flat = RenderState::from_project(48_000, &flat_project, &[]);
        flat.play();
        flat.process_block(256);
        let flat_master: Vec<f32> = flat.master().l[..256].to_vec();

        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        render.process_block(256);
        let modulated: Vec<f32> = render.master().l[..256].to_vec();

        // Compare the two renders subdivision by subdivision. A square LFO
        // alternates the fader between two gains, so the ratio to the
        // unmodulated render must not be the same in every subdivision.
        let ratio_at = |tick: usize| -> Option<f32> {
            (tick * CONTROL_RATE_FRAMES..(tick + 1) * CONTROL_RATE_FRAMES)
                .filter(|&i| flat_master[i].abs() > 1e-4)
                .map(|i| modulated[i] / flat_master[i])
                .next()
        };
        let first = ratio_at(0).expect("the first subdivision must carry audio");
        let differs = (1..256 / CONTROL_RATE_FRAMES)
            .filter_map(ratio_at)
            .any(|ratio| (ratio - first).abs() > 1e-3);
        assert!(
            differs,
            "a source on the fader must change gain across subdivisions"
        );
    }

    #[test]
    fn a_channel_note_trigger_restarts_its_played_lfo_on_the_control_tick() {
        let mut channel = ProjectChannel::sampler(0, 1);
        channel.setup.modulation.install(0, mooloop_core::ModulatorParams::Lfo(
            mooloop_core::ModLfoParams {
                rate_hz: 375.0,
                waveform: mooloop_core::ModLfoWaveform::Saw,
                retrigger: true,
                ..mooloop_core::ModLfoParams::default()
            },
        ));
        let project = synth_project(channel);
        let mut render = RenderState::from_project(48_000, &project, &[]);
        let none = [[NoteGateEvents::default(); MAX_CHANNELS]; MAX_CONTROL_TICKS_PER_BLOCK];
        assert_eq!(render.tick_channel_modulators(0, 64, &none), 2);
        assert_eq!(render.control_outputs[0][0][0], -1.0);
        assert_eq!(render.control_outputs[0][1][0], -0.5);

        // `process_block_inner` builds this fixed bitmap from scheduled
        // NoteOn offsets. The tick method applies it before sampling, so the
        // destination sees the reset phase on that subdivision rather than
        // one control tick later.
        let mut note_on = [[NoteGateEvents::default(); MAX_CHANNELS]; MAX_CONTROL_TICKS_PER_BLOCK];
        note_on[0][0].note_ons = 1;
        render.tick_channel_modulators(0, 32, &note_on);
        assert_eq!(render.control_outputs[0][0][0], -1.0);
    }

    #[test]
    fn an_envelope_can_subscribe_to_another_channels_note_gate() {
        let mut target = ProjectChannel::sampler(0, 1);
        target.setup.modulation.install(0, mooloop_core::ModulatorParams::Envelope(
            mooloop_core::ModEnvelopeParams {
                input_channel: 1,
                attack_seconds: 0.0,
                decay_seconds: 0.0,
                sustain: 1.0,
                ..mooloop_core::ModEnvelopeParams::default()
            },
        ));
        let mut project = synth_project(target);
        project.channels.push(ProjectChannel::sampler(1, 1));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        let mut gates = [[NoteGateEvents::default(); MAX_CHANNELS]; MAX_CONTROL_TICKS_PER_BLOCK];
        gates[0][1].note_ons = 1;

        render.tick_channel_modulators(0, 32, &gates);
        assert_eq!(render.control_outputs[0][0][0], 1.0);
    }

    /// A stepped parameter refuses modulation, so a route aimed at one is
    /// inert -- and, just as importantly, does not suppress the knob. Without
    /// the policy check the engine would treat the destination as modulated,
    /// withhold the base write, and leave the mode stuck.
    #[test]
    fn a_route_on_a_stepped_parameter_neither_moves_nor_blocks_its_knob() {
        let mut channel = ProjectChannel::sampler(0, 1);
        channel
            .setup
            .effects
            .push(mooloop_core::EffectSlotState::of_kind(
                mooloop_core::EffectKind::Eq,
            ));
        channel.setup.modulation.install(0, mooloop_core::ModulatorParams::Lfo(
            mooloop_core::ModLfoParams {
                rate_hz: 375.0,
                ..mooloop_core::ModLfoParams::default()
            },
        ));
        let stepped = ParamAddr::effect(EffectTarget::Channel(0), 0, mooloop_core::EQ_PARAM_TARGET);
        assert!(channel
            .setup
            .modulation
            .add_route(mooloop_core::ModRoute::to_slot(
                0,
                stepped,
                1.0,
                mooloop_core::ModPolarity::Bipolar,
            ))
            .is_some());

        let project = synth_project(channel);
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        render.process_block(128);

        // No control events are emitted for a destination that refuses
        // modulation.
        assert!(!render.strips[0]
            .effects
            .event_scratch
            .iter()
            .any(|event| matches!(
                event.event,
                Event::ParamValue {
                    id: mooloop_core::EQ_PARAM_TARGET,
                    ..
                }
            )));

        // And the knob still reaches the device, because the parked route does
        // not count as modulating it.
        assert!(!render.effect_is_modulated(
            EffectTarget::Channel(0),
            0,
            mooloop_core::EQ_PARAM_TARGET
        ));
    }

    #[test]
    fn lfo_resolves_filter_cutoff_at_the_control_rate_from_its_base_value() {
        let mut channel = ProjectChannel::sampler(0, 1);
        channel
            .setup
            .effects
            .push(mooloop_core::EffectSlotState::filter(
                mooloop_core::FilterParams {
                    cutoff_hz: 1_000.0,
                    resonance: 0.0,
                    mode: mooloop_core::FilterMode::LowPass,
                    ..mooloop_core::FilterParams::default()
                },
            ));
        channel.setup.modulation.install(0, mooloop_core::ModulatorParams::Lfo(
            mooloop_core::ModLfoParams {
                // 32 frames advance the LFO by a quarter-cycle at 48 kHz,
                // giving this block four clear control-rate landmarks.
                rate_hz: 375.0,
                ..mooloop_core::ModLfoParams::default()
            },
        ));
        assert!(channel
            .setup
            .modulation
            .add_route(mooloop_core::ModRoute::to_slot(
                0,
                ParamAddr::effect(
                    EffectTarget::Channel(0),
                    0,
                    mooloop_core::FILTER_PARAM_CUTOFF_HZ,
                ),
                0.25,
                mooloop_core::ModPolarity::Bipolar,
            ))
            .is_some());

        let project = synth_project(channel);
        let mut fixed_project = project.clone();
        fixed_project.channels[0].setup.modulation = ModRack::default();
        let mut fixed = RenderState::from_project(48_000, &fixed_project, &[]);
        fixed.play();
        fixed.process_block(128);
        let mut render = RenderState::from_project(48_000, &project, &[]);
        // This command changes the base, not an absolute value that the LFO
        // will overwrite. It is deliberately issued before the block whose
        // event list we inspect.
        render.apply_command(EngineCommand::SetEffectParam {
            target: EffectTarget::Channel(0),
            slot: 0,
            id: mooloop_core::FILTER_PARAM_CUTOFF_HZ,
            value: 1_000.0,
        });
        render.play();
        render.process_block(128);

        let cutoff_events: Vec<_> = render.strips[0]
            .effects
            .event_scratch
            .iter()
            .filter_map(|event| match event.event {
                Event::ParamValue {
                    id: mooloop_core::FILTER_PARAM_CUTOFF_HZ,
                    value,
                } => Some((event.offset, value)),
                _ => None,
            })
            .collect();
        assert_eq!(
            cutoff_events
                .iter()
                .map(|(offset, _)| *offset)
                .collect::<Vec<_>>(),
            vec![0, 32, 64, 96],
        );
        let values: Vec<_> = cutoff_events.iter().map(|(_, value)| *value).collect();
        assert!(
            (values[0] - 1_000.0).abs() < 1.0,
            "base event was {values:?}"
        );
        assert!(
            values[1] > values[0] * 3.0,
            "LFO did not open cutoff: {values:?}"
        );
        assert!(
            values[3] < values[0] * 0.4,
            "LFO did not close cutoff: {values:?}"
        );
        assert_eq!(
            render.strips[0].effects.slot(0).and_then(|state| state.base_params)
                .unwrap()
                .get(mooloop_core::FILTER_PARAM_CUTOFF_HZ),
            Some(1_000.0),
        );
        let audible_difference: f32 = render.master().l[..128]
            .iter()
            .zip(&fixed.master().l[..128])
            .map(|(modulated, fixed)| (modulated - fixed).abs())
            .sum();
        assert!(
            audible_difference > 0.01,
            "LFO modulation did not change the rendered signal"
        );
    }

    fn filter_channel(cutoff_hz: f32) -> ProjectChannel {
        let mut channel = ProjectChannel::sampler(0, 1);
        channel
            .setup
            .effects
            .push(mooloop_core::EffectSlotState::filter(
                mooloop_core::FilterParams {
                    cutoff_hz,
                    resonance: 0.0,
                    mode: mooloop_core::FilterMode::LowPass,
                    ..mooloop_core::FilterParams::default()
                },
            ));
        channel
    }

    const CUTOFF: ParamAddr = ParamAddr::effect(
        EffectTarget::Channel(0),
        0,
        mooloop_core::FILTER_PARAM_CUTOFF_HZ,
    );

    fn cutoff_events(render: &RenderState) -> Vec<(u32, f32)> {
        render.strips[0]
            .effects
            .event_scratch
            .iter()
            .filter_map(|event| match event.event {
                Event::ParamValue {
                    id: mooloop_core::FILTER_PARAM_CUTOFF_HZ,
                    value,
                } => Some((event.offset, value)),
                _ => None,
            })
            .collect()
    }

    /// One LFO on the filter cutoff, and the durable id it was installed
    /// under. Narrow commands name that id rather than the slot it landed in,
    /// so the tests below can address the module the way the UI does.
    fn lfo_on_cutoff(depth: f32) -> (mooloop_core::Project, mooloop_core::ModSourceId) {
        let mut channel = filter_channel(1_000.0);
        let source = channel
            .setup
            .modulation
            .install(
                0,
                mooloop_core::ModulatorParams::Lfo(mooloop_core::ModLfoParams {
                    // A quarter-cycle every 32 frames at 48 kHz, so a
                    // 128-frame block reads four clearly different points.
                    rate_hz: 375.0,
                    ..mooloop_core::ModLfoParams::default()
                }),
            )
            .expect("slot 0 accepts a module");
        assert!(channel
            .setup
            .modulation
            .add_route(mooloop_core::ModRoute::to_slot(
                0,
                CUTOFF,
                depth,
                mooloop_core::ModPolarity::Bipolar,
            ))
            .is_some());
        (synth_project(channel), source)
    }

    /// The property a narrow command could get wrong rather than merely
    /// cheap: dropping one route has to hand the device back its knob value.
    /// Without it the filter would hold whatever the LFO last resolved, until
    /// someone happened to touch that knob again.
    #[test]
    fn a_narrow_route_removal_restores_the_destinations_base() {
        let (project, source) = lfo_on_cutoff(0.25);
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        render.process_block(128);

        let modulated: Vec<_> = cutoff_events(&render)
            .iter()
            .map(|(_, value)| *value)
            .collect();
        assert_eq!(modulated.len(), 4, "LFO was not resolving: {modulated:?}");
        assert!(
            modulated.iter().any(|value| (value - 1_000.0).abs() > 100.0),
            "cutoff never left its base: {modulated:?}"
        );

        render.apply_command(EngineCommand::RemoveModRoute {
            channel: 0,
            source,
            destination: CUTOFF,
        });
        render.process_block(128);

        // One event, at the top of the block, carrying the knob value back.
        let restored = cutoff_events(&render);
        assert_eq!(
            restored.iter().map(|(offset, _)| *offset).collect::<Vec<_>>(),
            vec![0],
            "expected exactly one restoring event: {restored:?}"
        );
        assert!(
            (restored[0].1 - 1_000.0).abs() < 1.0,
            "the base was not restored: {restored:?}"
        );
        assert!(!render.effect_is_modulated(
            EffectTarget::Channel(0),
            0,
            mooloop_core::FILTER_PARAM_CUTOFF_HZ
        ));
    }

    /// Emptying a slot is the same fact stated once for every route it drove.
    #[test]
    fn clearing_a_module_restores_what_it_was_driving() {
        let (project, _) = lfo_on_cutoff(0.25);
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        render.process_block(128);

        render.apply_command(EngineCommand::ClearModulator {
            channel: 0,
            slot: 0,
        });
        render.process_block(128);

        let restored = cutoff_events(&render);
        assert_eq!(
            restored.iter().map(|(offset, _)| *offset).collect::<Vec<_>>(),
            vec![0],
            "expected exactly one restoring event: {restored:?}"
        );
        assert!((restored[0].1 - 1_000.0).abs() < 1.0, "{restored:?}");
    }

    /// The ordinary knob turn: one small command, and the module it names
    /// keeps running rather than being rebuilt around the new value.
    #[test]
    fn a_narrow_parameter_command_retunes_the_running_module() {
        let (project, _) = lfo_on_cutoff(0.25);
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        render.process_block(128);

        render.apply_command(EngineCommand::SetModulatorParam {
            channel: 0,
            slot: 0,
            id: mooloop_core::LFO_PARAM_DEPTH,
            value: 0.0,
        });
        render.process_block(128);

        // Still resolving four times a block, because the route is intact --
        // but at zero depth every tick lands on the base.
        let values: Vec<_> = cutoff_events(&render)
            .iter()
            .map(|(_, value)| *value)
            .collect();
        assert_eq!(values.len(), 4, "the route stopped resolving: {values:?}");
        assert!(
            values.iter().all(|value| (value - 1_000.0).abs() < 1.0),
            "depth 0 still moved the cutoff: {values:?}"
        );
    }

    /// The ordering trap this step had to avoid. Slot edits and route edits
    /// arrive as separate ring entries, so a route may name a module the
    /// engine does not hold. Because a route names a durable id and not a
    /// slot number, that route is refused outright rather than aimed at
    /// whatever else happens to occupy the slot.
    #[test]
    fn a_route_naming_an_absent_module_is_inert() {
        let project = synth_project(filter_channel(1_000.0));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.apply_command(EngineCommand::SetModRoute {
            channel: 0,
            route: mooloop_core::ModRoute {
                source: mooloop_core::ModSourceId(7),
                source_slot: 0,
                destination: CUTOFF,
                depth: 1.0,
                polarity: mooloop_core::ModPolarity::Bipolar,
            },
        });
        render.play();
        render.process_block(128);

        assert!(!render.effect_is_modulated(
            EffectTarget::Channel(0),
            0,
            mooloop_core::FILTER_PARAM_CUTOFF_HZ
        ));
        assert!(
            cutoff_events(&render).is_empty(),
            "an unresolvable route reached the device"
        );
    }

    #[test]
    fn an_automation_lane_resolves_a_param_at_the_control_rate() {
        let project = synth_project(filter_channel(1_000.0));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        let descriptor = mooloop_core::EffectKind::Filter
            .descriptor(mooloop_core::FILTER_PARAM_CUTOFF_HZ)
            .expect("cutoff is a described parameter");

        // A ramp across the first sixteenth, so a 128-frame block at 120 BPM
        // sits entirely inside the rising segment.
        for (id, tick, value) in [(1u32, 0u32, 0.0f32), (2, 24, 1.0)] {
            render.apply_command(EngineCommand::UpsertAutomationPoint {
                pattern: 0,
                channel: 0,
                target: CUTOFF,
                point: mooloop_core::AutomationPoint::new(id, tick, value),
            });
        }
        render.play();
        render.process_block(128);

        let events = cutoff_events(&render);
        assert_eq!(
            events.iter().map(|(offset, _)| *offset).collect::<Vec<_>>(),
            vec![0, 32, 64, 96],
        );
        let values: Vec<_> = events.iter().map(|(_, value)| *value).collect();
        assert!(
            (values[0] - descriptor.min).abs() < 1.0,
            "the lane did not start at its first point: {values:?}"
        );
        assert!(
            values.windows(2).all(|pair| pair[1] > pair[0]),
            "the ramp did not rise across the block: {values:?}"
        );
        // The knob is untouched: a lane supplies the base, it does not
        // overwrite what the user set.
        assert_eq!(
            render.strips[0].effects.slot(0).and_then(|state| state.base_params)
                .unwrap()
                .get(mooloop_core::FILTER_PARAM_CUTOFF_HZ),
            Some(1_000.0),
        );
    }

    #[test]
    fn a_lane_supplies_the_base_that_modulation_then_offsets() {
        let mut channel = filter_channel(1_000.0);
        channel.setup.modulation.install(0, mooloop_core::ModulatorParams::Lfo(
            mooloop_core::ModLfoParams {
                rate_hz: 375.0,
                ..mooloop_core::ModLfoParams::default()
            },
        ));
        assert!(channel
            .setup
            .modulation
            .add_route(mooloop_core::ModRoute::to_slot(
                0,
                CUTOFF,
                0.25,
                mooloop_core::ModPolarity::Bipolar,
            ))
            .is_some());
        let project = synth_project(channel);

        // A flat lane at half scale. With no modulation every control tick
        // would read the same value; the LFO is the only thing that can make
        // them differ, and it must differ *around the lane*, not the knob.
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.apply_command(EngineCommand::UpsertAutomationPoint {
            pattern: 0,
            channel: 0,
            target: CUTOFF,
            point: mooloop_core::AutomationPoint::new(1, 0, 0.5),
        });
        render.play();
        render.process_block(128);
        let values: Vec<_> = cutoff_events(&render)
            .iter()
            .map(|(_, value)| *value)
            .collect();

        let flat = mooloop_core::EffectKind::Filter
            .descriptor(mooloop_core::FILTER_PARAM_CUTOFF_HZ)
            .expect("cutoff is a described parameter")
            .from_normalized(0.5);
        assert!(
            (values[0] - flat).abs() < flat * 0.02,
            "the first tick should sit on the lane, not the 1 kHz knob: {values:?}"
        );
        assert!(
            values[1] > values[0] * 1.5 && values[3] < values[0] * 0.7,
            "the LFO did not swing around the lane value: {values:?}"
        );
    }

    #[test]
    fn clearing_a_lane_returns_the_destination_to_its_knob() {
        let project = synth_project(filter_channel(1_000.0));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.apply_command(EngineCommand::UpsertAutomationPoint {
            pattern: 0,
            channel: 0,
            target: CUTOFF,
            point: mooloop_core::AutomationPoint::new(1, 0, 0.1),
        });
        render.play();
        render.process_block(128);
        let automated = cutoff_events(&render)[0].1;
        assert!(automated < 900.0, "lane did not take the base: {automated}");

        render.apply_command(EngineCommand::ClearAutomationLane {
            pattern: 0,
            channel: 0,
            target: CUTOFF,
        });
        render.process_block(128);
        let restored = cutoff_events(&render);
        assert_eq!(
            restored.len(),
            1,
            "an empty lane should stop resolving per control tick: {restored:?}"
        );
        assert_eq!(restored[0], (0, 1_000.0));
    }

    #[test]
    fn a_lane_survives_a_project_round_trip_through_the_sequencer() {
        let mut project = synth_project(filter_channel(1_000.0));
        let mut lane = mooloop_core::AutomationLane::new(CUTOFF);
        lane.upsert(mooloop_core::AutomationPoint::new(1, 0, 0.25));
        project.channels[0].automation[0].push(lane);

        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        render.process_block(128);
        let loaded = cutoff_events(&render)[0].1;
        let expected = mooloop_core::EffectKind::Filter
            .descriptor(mooloop_core::FILTER_PARAM_CUTOFF_HZ)
            .expect("cutoff is a described parameter")
            .from_normalized(0.25);
        assert!(
            (loaded - expected).abs() < expected * 0.02,
            "a loaded lane did not drive the destination: {loaded} vs {expected}"
        );
    }

    #[test]
    fn a_lane_drives_the_buffer_read_head() {
        // The point of the whole exercise: a curve drawn in a clip moves a
        // retained-audio read head, with no gesture and no MIDI involved.
        let mut channel = ProjectChannel::sampler(0, 1);
        channel
            .setup
            .effects
            .push(mooloop_core::EffectSlotState::new(
                mooloop_core::EffectParams::Buffer(mooloop_core::BufferParams {
                    bars: 1,
                    ..mooloop_core::BufferParams::default()
                }),
            ));
        let project = synth_project(channel);
        let target = ParamAddr::effect(
            EffectTarget::Channel(0),
            0,
            mooloop_core::BUFFER_PARAM_OFFSET_BEATS,
        );

        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        // Fill the ring before asking the head to look backward into it.
        render.process_block(2048);

        for (id, tick, value) in [(1u32, 0u32, 0.0f32), (2, 96, 0.25)] {
            render.apply_command(EngineCommand::UpsertAutomationPoint {
                pattern: 0,
                channel: 0,
                target,
                point: mooloop_core::AutomationPoint::new(id, tick, value),
            });
        }
        for _ in 0..8 {
            render.process_block(2048);
        }

        let events: Vec<f32> = render.strips[0]
            .effects
            .event_scratch
            .iter()
            .filter_map(|event| match event.event {
                Event::ParamValue {
                    id: mooloop_core::BUFFER_PARAM_OFFSET_BEATS,
                    value,
                } => Some(value),
                _ => None,
            })
            .collect();
        assert!(
            events.len() > 1,
            "the lane did not resolve at the control rate: {events:?}"
        );
        assert!(
            events.iter().any(|value| *value > 0.0),
            "the lane never opened the offset: {events:?}"
        );
    }

    #[test]
    fn a_lane_drives_a_generator_parameter() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let target = ParamAddr {
            scope: EffectTarget::Channel(0),
            owner: ParamOwner::Source,
            param: mooloop_core::SAMPLER_PARAM_FILTER_CUTOFF,
        };
        let mut render = RenderState::from_project(48_000, &project, &[]);
        for (id, tick, value) in [(1u32, 0u32, 1.0f32), (2, 96, 0.0)] {
            render.apply_command(EngineCommand::UpsertAutomationPoint {
                pattern: 0,
                channel: 0,
                target,
                point: mooloop_core::AutomationPoint::new(id, tick, value),
            });
        }
        render.play();
        render.process_block(128);

        let values: Vec<f32> = render.events[0]
            .iter()
            .filter_map(|event| match event.event {
                Event::ParamValue {
                    id: mooloop_core::SAMPLER_PARAM_FILTER_CUTOFF,
                    value,
                } => Some(value),
                _ => None,
            })
            .collect();
        assert_eq!(
            values.len(),
            4,
            "the lane should resolve once per control tick: {values:?}"
        );
        assert!(
            values.windows(2).all(|pair| pair[1] < pair[0]),
            "the ramp did not fall across the block: {values:?}"
        );
        // The knob is untouched: a lane supplies the base, it does not
        // overwrite what the user set.
        assert_eq!(
            render.strips[0]
                .source_base
                .get(mooloop_core::SAMPLER_PARAM_FILTER_CUTOFF),
            Some(1.0),
        );
    }

    #[test]
    fn a_generator_parameter_reaches_the_device() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        let target = ParamAddr {
            scope: EffectTarget::Channel(0),
            owner: ParamOwner::Source,
            param: mooloop_core::SAMPLER_PARAM_DRIVE,
        };
        render.apply_command(EngineCommand::UpsertAutomationPoint {
            pattern: 0,
            channel: 0,
            target,
            point: mooloop_core::AutomationPoint::new(1, 0, 1.0),
        });
        render.play();
        render.process_block(128);
        assert!(
            (render.strips[0].sampler.params().drive - 1.0).abs() < 1e-3,
            "the lane did not reach the sampler: {}",
            render.strips[0].sampler.params().drive
        );

        // Clearing it returns the device to the knob rather than leaving it
        // holding the last resolved value.
        render.apply_command(EngineCommand::ClearAutomationLane {
            pattern: 0,
            channel: 0,
            target,
        });
        render.process_block(128);
        assert_eq!(render.strips[0].sampler.params().drive, 0.0);
    }

    #[test]
    fn mixed_source_project_renders_all_preallocated_nodes() {
        let mut project = Project {
            channels: vec![
                ProjectChannel::sampler(0, 1),
                ProjectChannel::drum_synth(1, 1),
                ProjectChannel::mono_synth(2, 1),
                ProjectChannel::poly_synth(3, 1),
            ],
            ..Project::default()
        };
        for (index, channel) in project.channels.iter_mut().enumerate() {
            channel.notes[0].push(NoteEvent::new(index as u32 + 1, 0, 96, 60, 127));
        }
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        assert!(render.process_block(512).peak_l > 0.01);
    }

    /// Route `bus` into `output` the way the interface does: compile the
    /// schedule for the resulting graph and send both together.
    fn route(render: &mut RenderState, buses: &mut [mooloop_core::BusSetup], bus: u8, output: u8) {
        buses[bus as usize].bus.output = output;
        let graph = compile_bus_graph(buses).expect("test graph should be acyclic");
        render.apply_command(EngineCommand::InstallBusGraph { graph });
    }

    fn rendered_energy(project: &Project, configure: impl FnOnce(&mut RenderState)) -> f32 {
        let mut render = RenderState::from_project(48_000, project, &[]);
        configure(&mut render);
        render.play();
        render.process_block(1024);
        let master = render.master();
        master.l[..1024].iter().map(|s| s * s).sum::<f32>()
    }

    fn muffling_filter() -> Box<dyn AudioNode + Send> {
        build_effect(
            mooloop_core::EffectParams::Filter(mooloop_core::FilterParams {
                cutoff_hz: 100.0,
                resonance: 0.0,
                mode: mooloop_core::FilterMode::LowPass,
                ..mooloop_core::FilterParams::default()
            }),
            48_000,
        )
    }

    fn default_effect(kind: mooloop_core::EffectKind) -> Box<dyn AudioNode + Send> {
        build_effect(kind.default_params(), 48_000)
    }

    fn install_effect(
        target: EffectTarget,
        slot: u8,
        node: Box<dyn AudioNode + Send>,
    ) -> StructuralCommand {
        let align = DryAlign::new(node.dry_path_latency_frames()).map(Box::new);
        StructuralCommand::InstallEffect {
            target,
            slot,
            kind: mooloop_core::EffectKind::Filter,
            resource_key: None,
            node,
            align,
            analyzer: Box::new(SpectrumAnalyzer::new()),
            state: Box::new(EffectSlot::new()),
        }
    }

    #[test]
    fn installed_filter_changes_channel_output() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let dry = rendered_energy(&project, |_| {});
        let filtered = rendered_energy(&project, |render| {
            let _ = render.apply_structural(install_effect(
                EffectTarget::Channel(0),
                0,
                muffling_filter(),
            ));
        });
        assert!(dry > 0.0, "reference render was silent");
        assert!(
            filtered < dry * 0.5,
            "100 Hz low-pass should eat most of a kick: dry {dry}, filtered {filtered}"
        );
    }

    /// Every effect kind must be constructible through the shared builder and
    /// audibly change the signal at a setting that is obviously not neutral.
    /// This is the test a new kind trips if it is added to `EffectKind` but
    /// never wired into `build_effect`.
    #[test]
    fn every_effect_kind_installs_and_alters_the_signal() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let dry = rendered_energy(&project, |_| {});
        assert!(dry > 0.0, "reference render was silent");

        for kind in mooloop_core::EffectKind::ALL {
            // Push each kind well away from neutral; defaults are chosen to
            // be transparent, so they would prove nothing here.
            let mut params = kind.default_params();
            match kind {
                mooloop_core::EffectKind::Eq => {
                    params.set(mooloop_core::EQ_PARAM_GAIN_DB, 18.0);
                }
                mooloop_core::EffectKind::Modulation => {
                    params.set(mooloop_core::MODULATION_PARAM_MODE, 2.0);
                    params.set(mooloop_core::MODULATION_PARAM_DEPTH, 0.85);
                }
                mooloop_core::EffectKind::Filter => {
                    params.set(mooloop_core::FILTER_PARAM_CUTOFF_HZ, 100.0);
                }
                mooloop_core::EffectKind::Drive => {
                    params.set(mooloop_core::DRIVE_PARAM_DRIVE, 64.0);
                }
                mooloop_core::EffectKind::Bitcrush => {
                    params.set(mooloop_core::BITCRUSH_PARAM_BITS, 1.0);
                    params.set(mooloop_core::BITCRUSH_PARAM_DOWNSAMPLE, 32.0);
                }
                mooloop_core::EffectKind::Gate => {
                    // Threshold at the top of its range shuts on anything.
                    params.set(mooloop_core::GATE_PARAM_THRESHOLD_DB, 0.0);
                    params.set(mooloop_core::GATE_PARAM_ATTACK_MS, 0.05);
                }
                mooloop_core::EffectKind::Compressor => {
                    params.set(mooloop_core::COMP_PARAM_THRESHOLD_DB, -40.0);
                    params.set(mooloop_core::COMP_PARAM_RATIO, 20.0);
                    params.set(mooloop_core::COMP_PARAM_ATTACK_MS, 0.05);
                }
                mooloop_core::EffectKind::Limiter => {
                    params.set(mooloop_core::LIMITER_PARAM_CEILING_DB, -24.0);
                }
                mooloop_core::EffectKind::Delay => {
                    // Short enough that a repeat lands inside the rendered
                    // block, fully wet so the dry signal cannot mask it.
                    params.set(mooloop_core::DELAY_PARAM_TIME_MS, 5.0);
                    params.set(mooloop_core::DELAY_PARAM_FEEDBACK, 0.6);
                    params.set(mooloop_core::DELAY_PARAM_MIX, 1.0);
                }
                mooloop_core::EffectKind::Reverb => {
                    // Entirely wet, and its shortest delay line plus the
                    // default pre-delay outlast this render window, so the
                    // output here is silence — clearly different from the
                    // nonzero dry reference.
                }
                mooloop_core::EffectKind::Plate => {
                    // Also entirely wet, and its shortest comb tap is longer
                    // than this render window, so the output is silence here
                    // — clearly different from the nonzero dry reference.
                }
                mooloop_core::EffectKind::Buffer => {
                    // Follow is deliberately transparent until an atomic
                    // buffer event arrives.
                }
            }

            let wet = rendered_energy(&project, |render| {
                let _ = render.apply_structural(install_effect(
                    EffectTarget::Channel(0),
                    0,
                    build_effect(params, 48_000),
                ));
            });
            if kind == mooloop_core::EffectKind::Buffer {
                // Follow passes audio through untouched; equal-power leaks a
                // cos(pi/2) ~ 6e-8 of the aligned dry alongside it, which is
                // inaudible but not bit-exact.
                assert!(
                    (wet - dry).abs() < dry * 1.0e-5,
                    "buffer Follow must be transparent: dry {dry}, wet {wet}"
                );
            } else {
                assert!(
                    (wet - dry).abs() > dry * 0.01,
                    "{} left the signal unchanged: dry {dry}, wet {wet}",
                    kind.label()
                );
            }
        }
    }

    /// A channel routed to a bus must reach the master *through* that bus, so
    /// an effect inserted on the bus shapes everything feeding it.
    #[test]
    fn a_bus_effect_processes_every_channel_feeding_it() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let dry = rendered_energy(&project, |render| {
            render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 3 });
        });
        assert!(dry > 0.0, "routing through a bus must not lose the signal");

        let filtered = rendered_energy(&project, |render| {
            render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 3 });
            let _ =
                render.apply_structural(install_effect(EffectTarget::Bus(3), 0, muffling_filter()));
        });
        assert!(
            filtered < dry * 0.5,
            "bus filter should muffle the channel: dry {dry}, filtered {filtered}"
        );
    }

    #[test]
    fn muting_a_bus_silences_what_feeds_it() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let muted = rendered_energy(&project, |render| {
            render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 2 });
            render.apply_command(EngineCommand::SetBusMuted {
                bus: 2,
                muted: true,
            });
        });
        assert_eq!(muted, 0.0);
    }

    #[test]
    fn bus_volume_scales_its_contribution() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let unity = rendered_energy(&project, |render| {
            render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 1 });
            render.apply_command(EngineCommand::SetBusVolume {
                bus: 1,
                volume: 1.0,
            });
        });
        let halved = rendered_energy(&project, |render| {
            render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 1 });
            render.apply_command(EngineCommand::SetBusVolume {
                bus: 1,
                volume: 0.5,
            });
        });
        // Energy is the square of amplitude, so halving the gain quarters it.
        let ratio = halved / unity;
        assert!((0.2..0.3).contains(&ratio), "expected ~0.25, got {ratio}");
    }

    /// A bus's input must be complete before it runs. Chain two and put the
    /// filter on the *second* hop: it can only be heard if the schedule
    /// rendered bus 5 first.
    #[test]
    fn a_bus_can_feed_another_bus() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let chained = rendered_energy(&project, |render| {
            let mut buses = mooloop_core::default_buses();
            render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 5 });
            route(render, &mut buses, 5, 2);
        });
        assert!(chained > 0.0, "chained buses must still reach the master");

        let filtered = rendered_energy(&project, |render| {
            let mut buses = mooloop_core::default_buses();
            render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 5 });
            route(render, &mut buses, 5, 2);
            let _ =
                render.apply_structural(install_effect(EffectTarget::Bus(2), 0, muffling_filter()));
        });
        assert!(
            filtered < chained * 0.5,
            "bus 5 must be rendered before bus 2: {chained} -> {filtered}"
        );
    }

    /// The whole point of compiling a schedule: routing a low-numbered bus
    /// into a high-numbered one is ordinary now. The old descending pass could
    /// not express this at all, and rewrote the edge to the master.
    #[test]
    fn a_bus_can_feed_a_higher_numbered_bus() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let chained = rendered_energy(&project, |render| {
            let mut buses = mooloop_core::default_buses();
            render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 2 });
            route(render, &mut buses, 2, 9);
        });
        assert!(chained > 0.0, "an uphill route must still reach the master");

        let filtered = rendered_energy(&project, |render| {
            let mut buses = mooloop_core::default_buses();
            render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 2 });
            route(render, &mut buses, 2, 9);
            let _ =
                render.apply_structural(install_effect(EffectTarget::Bus(9), 0, muffling_filter()));
        });
        assert!(
            filtered < chained * 0.5,
            "bus 9's filter must be in the path: {chained} -> {filtered}"
        );
    }

    /// A three-hop chain that runs against index order end to end, to prove
    /// the schedule is genuinely driving the pass rather than index order
    /// happening to agree with it.
    #[test]
    fn a_chain_that_runs_entirely_against_index_order_still_sums() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let dry = rendered_energy(&project, |_| {});
        let routed = rendered_energy(&project, |render| {
            let mut buses = mooloop_core::default_buses();
            render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 1 });
            buses[1].bus.output = 4;
            buses[4].bus.output = 11;
            buses[11].bus.output = MASTER_BUS;
            let graph = compile_bus_graph(&buses).expect("acyclic");
            render.apply_command(EngineCommand::InstallBusGraph { graph });
        });
        // Three unity-gain buses in series should not change the level.
        let ratio = routed / dry;
        assert!(
            (0.9..1.1).contains(&ratio),
            "1 -> 4 -> 11 -> master should be level-neutral: {routed} vs {dry}"
        );
    }

    /// A cyclic project cannot be scheduled, so loading one must fall back to
    /// everything-to-master rather than dropping the audio or looping.
    #[test]
    fn a_cyclic_project_loads_as_everything_to_master() {
        let mut project = synth_project(ProjectChannel::sampler(0, 1));
        project.channels[0].setup.channel.bus = 3;
        project.buses[3].bus.output = 6;
        project.buses[6].bus.output = 3;

        let dry = {
            let mut clean = synth_project(ProjectChannel::sampler(0, 1));
            clean.buses = mooloop_core::default_buses();
            rendered_energy(&clean, |_| {})
        };
        let repaired = rendered_energy(&project, |_| {});
        let ratio = repaired / dry;
        assert!(
            (0.9..1.1).contains(&ratio),
            "a cyclic file should still play: {repaired} vs {dry}"
        );
    }

    #[test]
    fn malformed_and_short_bus_banks_still_reach_the_master() {
        let dry = rendered_energy(&synth_project(ProjectChannel::sampler(0, 1)), |_| {});

        let mut malformed = synth_project(ProjectChannel::sampler(0, 1));
        malformed.channels[0].setup.channel.bus = 3;
        malformed.buses[3].bus.output = MAX_BUSES as u8;
        let repaired = rendered_energy(&malformed, |_| {});
        assert!(
            (0.9..1.1).contains(&(repaired / dry)),
            "an invalid destination must be repaired to master"
        );

        let mut short = synth_project(ProjectChannel::sampler(0, 1));
        short.channels[0].setup.channel.bus = 9;
        short.buses.truncate(2);
        let padded = rendered_energy(&short, |_| {});
        assert!(
            (0.9..1.1).contains(&(padded / dry)),
            "missing bus definitions must compile as default buses"
        );
    }

    /// An out-of-range bus index must not mute the channel; it lands on the
    /// master, which is the same thing a freshly-defaulted project does.
    #[test]
    fn an_out_of_range_bus_lands_on_the_master() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let dry = rendered_energy(&project, |_| {});
        let clamped = rendered_energy(&project, |render| {
            render.apply_command(EngineCommand::SetChannelBus {
                channel: 0,
                bus: 200,
            });
        });
        assert_eq!(clamped, dry);
    }

    /// The mixer's strips are only useful if the bus they name is the one
    /// being metered, so check that the audio shows up on the routed bus and
    /// the master and nowhere else.
    #[test]
    fn peaks_are_published_for_the_bus_that_carries_the_audio() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let meters = BusMeters::new();
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.attach_meters(meters.clone());
        render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 6 });
        render.play();
        render.process_block(1024);

        assert!(meters.take(6).0 > 0.001, "the routed bus should meter");
        assert!(meters.take(MASTER_BUS as usize).0 > 0.001);
        assert_eq!(meters.take(5), (0.0, 0.0), "an unused bus must read silent");
    }

    #[test]
    fn device_meters_follow_the_host_signal_flow() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let meters = DeviceMeters::new();
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.attach_device_meters(meters.clone());
        let _ = render.apply_structural(install_effect(
            EffectTarget::Channel(0),
            0,
            muffling_filter(),
        ));
        render.play();
        render.process_block(1024);

        let (source_in, source_out) = meters.take(0, 0);
        assert_eq!(source_in, (0.0, 0.0), "sources have no device input");
        assert!(source_out.0 > 0.001, "source output should meter");
        let (effect_in, effect_out) = meters.take(0, 1);
        assert!(effect_in.0 > 0.001, "effect sees the source output");
        assert!(
            effect_out.0 < effect_in.0,
            "filter output should differ from its input"
        );
    }

    #[test]
    fn bus_effect_slots_meter_like_channel_slots() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let meters = DeviceMeters::new();
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.attach_device_meters(meters.clone());
        render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 3 });
        let _ = render.apply_structural(install_effect(EffectTarget::Bus(3), 0, muffling_filter()));
        render.play();
        render.process_block(1024);

        let (effect_in, effect_out) = meters.take(MAX_CHANNELS + 3, 1);
        assert!(
            effect_in.0 > 0.001,
            "a bus effect sees what its bus received"
        );
        assert!(
            effect_out.0 < effect_in.0,
            "filter output should differ from its input"
        );
        let (channel_in, _) = meters.take(0, 1);
        assert_eq!(
            channel_in,
            (0.0, 0.0),
            "the channel has no effect in slot 1"
        );
    }

    /// Test node: an honest pure delay. What the container's dry path must be
    /// aligned against whenever a node reports latency.
    struct LatentDelay {
        left: std::collections::VecDeque<f32>,
        right: std::collections::VecDeque<f32>,
    }

    impl LatentDelay {
        fn new(frames: usize) -> Self {
            Self {
                left: std::iter::repeat_n(0.0, frames).collect(),
                right: std::iter::repeat_n(0.0, frames).collect(),
            }
        }
    }

    impl AudioNode for LatentDelay {
        fn latency_frames(&self) -> u32 {
            self.left.len() as u32
        }

        fn process(
            &mut self,
            ctx: &ProcessContext,
            bus: &mut StereoBus,
            _events_in: &EventList,
            _events_out: Option<&mut EventList>,
        ) {
            for frame in 0..ctx.frames {
                self.left.push_back(bus.l[frame]);
                self.right.push_back(bus.r[frame]);
                bus.l[frame] = self.left.pop_front().unwrap_or(0.0);
                bus.r[frame] = self.right.pop_front().unwrap_or(0.0);
            }
        }
    }

    #[test]
    fn effect_chain_bound_tracks_sparse_slots() {
        let mut chain = EffectChain::new();
        assert_eq!(chain.bound, 0);

        for slot in [2, 5] {
            let displaced = chain.install(
                slot,
                mooloop_core::EffectKind::Delay,
                None,
                Box::new(LatentDelay::new(1)),
                None,
                Box::new(SpectrumAnalyzer::new()),
                Box::new(EffectSlot::new()),
            );
            assert!(displaced.is_empty());
        }
        assert_eq!(chain.bound, 6);

        chain.swap(5, 1);
        assert_eq!(chain.bound, 3);
        assert!(chain.nodes[1].is_some());
        assert!(chain.nodes[2].is_some());

        assert!(chain.remove(2).node.is_some());
        assert_eq!(chain.bound, 2);
        assert!(chain.remove(1).node.is_some());
        assert_eq!(chain.bound, 0);
    }

    #[test]
    fn the_container_aligns_its_dry_path_to_node_latency() {
        const LATENCY: usize = 15;
        let mut chain = EffectChain::new();
        let node = Box::new(LatentDelay::new(LATENCY));
        let align = DryAlign::new(node.latency_frames()).map(Box::new);
        let displaced = chain.install(
            0,
            mooloop_core::EffectKind::Delay,
            None,
            node,
            align,
            Box::new(SpectrumAnalyzer::new()),
            Box::new(EffectSlot::new()),
        );
        assert!(displaced.is_empty());
        chain.slot_mut(0).unwrap().wet_dry = 0.5;

        let context = ProcessContext {
            sample_rate: 48_000,
            frames: 64,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        };
        let mut bus = StereoBus::with_capacity(MAX_BLOCK_SIZE);
        bus.l[0] = 1.0;
        bus.r[0] = 1.0;
        chain.process(&context, &mut bus, EffectTarget::Channel(0), None, None, None);

        assert!(
            bus.l[..LATENCY].iter().all(|s| *s == 0.0),
            "a latent node must not pass dry audio early: {:?}",
            &bus.l[..LATENCY]
        );
        assert!(
            (bus.l[LATENCY] - core::f32::consts::SQRT_2).abs() < 1e-5,
            "aligned dry + wet recombine to equal-power unity (sqrt(2) for a \
             correlated path at 50%), got {}",
            bus.l[LATENCY]
        );
        assert!(
            bus.l[LATENCY + 1..].iter().all(|s| *s == 0.0),
            "no second, misaligned copy of the impulse may follow"
        );
    }

    /// The prepared-resource guard is generic, but Buffer is now its only
    /// user: reverb used to key on an IR fingerprint and no longer allocates
    /// anything a parameter change could invalidate.
    #[test]
    fn prepared_resource_replacement_refuses_a_stale_slot_key() {
        let mut chain = EffectChain::new();
        let initial = Box::new(LatentDelay::new(1));
        let initial_align = DryAlign::new(initial.latency_frames()).map(Box::new);
        let displaced = chain.install(
            0,
            mooloop_core::EffectKind::Buffer,
            Some(10),
            initial,
            initial_align,
            Box::new(SpectrumAnalyzer::new()),
            Box::new(EffectSlot::new()),
        );
        assert!(displaced.is_empty());

        let stale = Box::new(LatentDelay::new(2));
        let stale_align = DryAlign::new(stale.latency_frames()).map(Box::new);
        let rejected = chain.replace_if_kind(
            0,
            mooloop_core::EffectKind::Buffer,
            9,
            11,
            stale,
            stale_align,
        );
        assert!(rejected.node.is_some());
        assert_eq!(chain.slot(0).unwrap().resource_key, Some(10));

        let current = Box::new(LatentDelay::new(2));
        let current_align = DryAlign::new(current.latency_frames()).map(Box::new);
        let replaced = chain.replace_if_kind(
            0,
            mooloop_core::EffectKind::Buffer,
            10,
            11,
            current,
            current_align,
        );
        assert!(replaced.node.is_some());
        assert_eq!(chain.slot(0).unwrap().resource_key, Some(11));
    }

    #[test]
    fn a_muted_bus_meters_silent() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let meters = BusMeters::new();
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.attach_meters(meters.clone());
        render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 6 });
        render.apply_command(EngineCommand::SetBusMuted {
            bus: 6,
            muted: true,
        });
        render.play();
        render.process_block(1024);
        assert_eq!(meters.take(6), (0.0, 0.0));
    }

    #[test]
    fn a_master_effect_chain_processes_the_whole_mix() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let dry = rendered_energy(&project, |_| {});
        let filtered = rendered_energy(&project, |render| {
            let _ = render.apply_structural(install_effect(
                EffectTarget::Bus(mooloop_core::MASTER_BUS),
                0,
                muffling_filter(),
            ));
        });
        assert!(filtered < dry * 0.5, "dry {dry}, filtered {filtered}");
    }

    #[test]
    fn generic_host_mix_and_input_output_trims_wrap_every_effect() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let dry = rendered_energy(&project, |_| {});
        let host_dry = rendered_energy(&project, |render| {
            let _ = render.apply_structural(install_effect(
                EffectTarget::Channel(0),
                0,
                muffling_filter(),
            ));
            render.apply_command(EngineCommand::SetEffectWetDry {
                target: EffectTarget::Channel(0),
                slot: 0,
                wet_dry: 0.0,
            });
        });
        assert!(
            (host_dry / dry - 1.0).abs() < 0.1,
            "wet=0 must pass dry signal"
        );
        let trimmed = rendered_energy(&project, |render| {
            let _ = render.apply_structural(install_effect(
                EffectTarget::Channel(0),
                0,
                muffling_filter(),
            ));
            render.apply_command(EngineCommand::SetEffectWetDry {
                target: EffectTarget::Channel(0),
                slot: 0,
                wet_dry: 0.0,
            });
            render.apply_command(EngineCommand::SetEffectOutputTrim {
                target: EffectTarget::Channel(0),
                slot: 0,
                output_trim: 0.5,
            });
        });
        assert!(
            (trimmed / dry - 0.25).abs() < 0.08,
            "trim is amplitude, energy scales squared"
        );
        let input_trimmed = rendered_energy(&project, |render| {
            let _ = render.apply_structural(install_effect(
                EffectTarget::Channel(0),
                0,
                muffling_filter(),
            ));
            render.apply_command(EngineCommand::SetEffectWetDry {
                target: EffectTarget::Channel(0),
                slot: 0,
                wet_dry: 0.0,
            });
            render.apply_command(EngineCommand::SetEffectInputTrim {
                target: EffectTarget::Channel(0),
                slot: 0,
                input_trim: 0.5,
            });
        });
        assert!(
            (input_trimmed / dry - 0.25).abs() < 0.08,
            "input trim must feed the hosted effect at reduced amplitude"
        );
    }

    #[test]
    fn effect_param_bypass_and_reorder_plumbing() {
        let project = synth_project(ProjectChannel::sampler(0, 1));

        // A wide-open filter passes (nearly) everything; closing it via a
        // queued ParamValue event must change the output.
        let open = rendered_energy(&project, |render| {
            let _ = render.apply_structural(install_effect(
                EffectTarget::Channel(0),
                0,
                default_effect(mooloop_core::EffectKind::Filter),
            ));
        });
        // The cutoff now ramps (see
        // docs/plans/share-dsp-primitives/01-smooth-effect-parameters.md)
        // rather than snapping, so a queued change doesn't fully close the
        // filter within the same 1024-frame block it's queued in. Render
        // one block to let the ramp settle, discard it, then measure the
        // next — this still asserts the param change lands, just not
        // instantaneously.
        let mut render = RenderState::from_project(48_000, &project, &[]);
        let _ = render.apply_structural(install_effect(
            EffectTarget::Channel(0),
            0,
            default_effect(mooloop_core::EffectKind::Filter),
        ));
        render.apply_command(EngineCommand::SetEffectParam {
            target: EffectTarget::Channel(0),
            slot: 0,
            id: mooloop_core::FILTER_PARAM_CUTOFF_HZ,
            value: 100.0,
        });
        render.play();
        render.process_block(1024);
        render.process_block(1024);
        let closed: f32 = render.master().l[..1024].iter().map(|s| s * s).sum();
        assert!(closed < open * 0.5, "open {open}, closed {closed}");

        // Bypass restores the dry sound.
        let bypassed = rendered_energy(&project, |render| {
            let _ = render.apply_structural(install_effect(
                EffectTarget::Channel(0),
                0,
                muffling_filter(),
            ));
            render.apply_command(EngineCommand::SetEffectBypassed {
                target: EffectTarget::Channel(0),
                slot: 0,
                bypassed: true,
            });
        });
        let dry = rendered_energy(&project, |_| {});
        let ratio = bypassed / dry;
        assert!(
            (0.9..1.1).contains(&ratio),
            "bypassed should match dry: {bypassed} vs {dry}"
        );

        // Swapping an occupied slot with an empty one moves the filter.
        let mut render = RenderState::from_project(48_000, &project, &[]);
        let _ = render.apply_structural(install_effect(
            EffectTarget::Channel(0),
            0,
            muffling_filter(),
        ));
        render.apply_command(EngineCommand::SwapEffectSlots {
            target: EffectTarget::Channel(0),
            slot_a: 0,
            slot_b: 3,
        });
        render.play();
        render.process_block(1024);
        let moved = render.master().l[..1024].iter().map(|s| s * s).sum::<f32>();
        assert!(moved < dry * 0.5, "filter should still muffle after swap");

        // Removing the slot reclaims the node instead of dropping it here.
        let reclaimed = render.apply_structural(StructuralCommand::RemoveEffect {
            target: EffectTarget::Channel(0),
            slot: 3,
        });
        assert!(reclaimed.is_some());
    }

    /// Render `blocks` blocks of a one-buffer-insert project, optionally
    /// firing one event as a command before block `trigger_block`, and return
    /// the concatenated master output.
    fn render_with_buffer(
        project: &Project,
        blocks: usize,
        frames: usize,
        trigger_block: usize,
        trigger: Option<mooloop_core::BufferEvent>,
        telemetry: Option<&Arc<DeviceTelemetry>>,
        device: Box<dyn AudioNode + Send>,
    ) -> Vec<f32> {
        let mut render = RenderState::from_project(48_000, project, &[]);
        if let Some(telemetry) = telemetry {
            render.attach_device_telemetry(telemetry.clone());
        }
        let _ = render.apply_structural(install_effect(EffectTarget::Channel(0), 0, device));
        render.play();
        let mut out = Vec::with_capacity(blocks * frames);
        for block in 0..blocks {
            if block == trigger_block {
                if let Some(event) = trigger {
                    render.apply_command(EngineCommand::TriggerBuffer {
                        target: EffectTarget::Channel(0),
                        slot: 0,
                        event,
                    });
                }
            }
            render.process_block(frames);
            out.extend_from_slice(&render.master().l[..frames]);
        }
        out
    }

    fn jump_event(offset_beats: f32, rate: f32) -> mooloop_core::BufferEvent {
        mooloop_core::BufferEvent {
            offset_beats,
            rate,
            window_beats: None,
            repeat: None,
            duration: mooloop_core::BufferDuration::UntilNextEvent,
            // Zero, so the divergence a test observes is the edit itself and
            // not a fade that would blur the first frames after it.
            crossfade_ms: 0.0,
        }
    }

    /// The whole command path a debug trigger takes: an `EngineCommand`
    /// carrying one event tuple, reaching an inserted buffer device, and
    /// changing what the master renders. Follow is deliberately transparent,
    /// so nothing short of a fired event proves this plumbing works.
    #[test]
    fn triggered_buffer_event_alters_rendered_output() {
        const BLOCKS: usize = 8;
        const FRAMES: usize = 1024;
        const TRIGGER_BLOCK: usize = 4;
        let project = synth_project(ProjectChannel::sampler(0, 1));

        let follow = render_with_buffer(
            &project,
            BLOCKS,
            FRAMES,
            TRIGGER_BLOCK,
            None,
            None,
            default_effect(mooloop_core::EffectKind::Buffer),
        );
        let jumped = render_with_buffer(
            &project,
            BLOCKS,
            FRAMES,
            TRIGGER_BLOCK,
            Some(jump_event(-0.05, 1.0)),
            None,
            default_effect(mooloop_core::EffectKind::Buffer),
        );

        let split = TRIGGER_BLOCK * FRAMES;
        assert_eq!(
            follow[..split],
            jumped[..split],
            "audio before the trigger must be untouched"
        );
        assert!(
            follow[split..].iter().any(|sample| *sample != 0.0),
            "reference tail was silent, so divergence would prove nothing"
        );
        assert!(
            follow[split..] != jumped[split..],
            "TriggerBuffer never reached the inserted device"
        );
    }

    /// A reverse head and the ring's trailing edge close on each other at 2x,
    /// so a backward jump must force a return to live and surface as device
    /// telemetry — the only trace a forced return leaves, since the audio
    /// thread cannot log. The ring is deliberately tiny here so the collision
    /// lands inside a short render instead of eight retained bars later.
    #[test]
    fn writer_collision_surfaces_as_device_telemetry() {
        const BLOCKS: usize = 8;
        const FRAMES: usize = 1024;
        const TRIGGER_BLOCK: usize = 4;
        const RING: usize = 4_096;
        let project = synth_project(ProjectChannel::sampler(0, 1));

        let telemetry = DeviceTelemetry::new();
        let _ = render_with_buffer(
            &project,
            BLOCKS,
            FRAMES,
            TRIGGER_BLOCK,
            Some(jump_event(-0.02, -1.0)),
            Some(&telemetry),
            Box::new(mooloop_dsp::BufferDevice::with_capacity(RING)),
        );
        // Stage 0 is the source; the insert in slot 0 publishes as stage 1.
        assert_eq!(telemetry.read_buffer_collisions(0, 1), 1);

        // Follow never detaches, so it can never be overtaken.
        let quiet = DeviceTelemetry::new();
        let _ = render_with_buffer(
            &project,
            BLOCKS,
            FRAMES,
            TRIGGER_BLOCK,
            None,
            Some(&quiet),
            Box::new(mooloop_dsp::BufferDevice::with_capacity(RING)),
        );
        assert_eq!(quiet.read_buffer_collisions(0, 1), 0);
    }

    /// The whole control path: a note-on carrying a mapped tuple reaches the
    /// insert and changes the render, and the matching note-off ends it.
    /// Note says what and how long; velocity says how hard.
    #[test]
    fn mapped_midi_notes_drive_a_buffer_insert() {
        use mooloop_core::midi::{BufferMidiMap, BufferNoteMapping};
        use mooloop_core::{MidiKind, MidiMessage};

        const FRAMES: usize = 1024;
        let project = synth_project(ProjectChannel::sampler(0, 1));

        let render_with_midi = |messages_at: Option<usize>| -> Vec<f32> {
            let mut render = RenderState::from_project(48_000, &project, &[]);
            let _ = render.apply_structural(install_effect(
                EffectTarget::Channel(0),
                0,
                default_effect(mooloop_core::EffectKind::Buffer),
            ));
            let mut map = BufferMidiMap::new(EffectTarget::Channel(0), 0);
            map.notes[0] = Some(BufferNoteMapping {
                note: 60,
                event: mooloop_core::BufferEvent {
                    offset_beats: -0.05,
                    ..mooloop_core::BufferEvent::live()
                },
            });
            render.attach_buffer_midi_map(Arc::new(ArcSwapOption::from_pointee(map)));
            render.play();

            let mut out = Vec::new();
            for block in 0..8 {
                if Some(block) == messages_at {
                    render.apply_midi(&[MidiMessage {
                        offset: 0,
                        channel: 0,
                        kind: MidiKind::NoteOn {
                            note: 60,
                            velocity: 127,
                        },
                    }]);
                }
                render.process_block(FRAMES);
                out.extend_from_slice(&render.master().l[..FRAMES]);
            }
            out
        };

        let quiet = render_with_midi(None);
        let played = render_with_midi(Some(4));
        let split = 4 * FRAMES;
        assert_eq!(quiet[..split], played[..split]);
        assert!(
            quiet[split..].iter().any(|sample| *sample != 0.0),
            "reference tail was silent"
        );
        assert!(
            quiet[split..] != played[split..],
            "a mapped note never reached the insert"
        );
    }

    /// An unmapped key must not release an edit it never started, and a
    /// mapped one must. Both halves matter: a keyboard is full of notes this
    /// map does not own.
    #[test]
    fn only_a_mapped_note_releases_the_edit() {
        use mooloop_core::midi::{BufferMidiMap, BufferNoteMapping};
        use mooloop_core::{MidiKind, MidiMessage};

        let project = synth_project(ProjectChannel::sampler(0, 1));
        let note_off = |note| MidiMessage {
            offset: 0,
            channel: 0,
            kind: MidiKind::NoteOff { note },
        };

        let mut map = BufferMidiMap::new(EffectTarget::Channel(0), 0);
        map.notes[0] = Some(BufferNoteMapping {
            note: 60,
            event: mooloop_core::BufferEvent::live(),
        });
        assert!(map.note_event(60, 100).is_some());
        assert!(map.note_event(61, 100).is_none());

        // Velocity drives the crossfade: hard is abrupt, soft is declicked.
        let hard = map.note_event(60, 127).unwrap();
        let soft = map.note_event(60, 1).unwrap();
        assert_eq!(hard.crossfade_ms, 0.0);
        assert!(soft.crossfade_ms > hard.crossfade_ms);
        assert_eq!(hard.duration, mooloop_core::BufferDuration::Gate);

        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.attach_buffer_midi_map(Arc::new(ArcSwapOption::from_pointee(map)));
        render.play();
        // Neither of these should panic or route anywhere unexpected; the
        // unmapped note is simply ignored.
        render.apply_midi(&[note_off(61), note_off(60)]);
        render.process_block(256);
    }

    /// A relative CC drives the platter rather than re-firing an event, and
    /// a centred message means no movement at all.
    #[test]
    fn a_relative_cc_scrubs_without_refiring() {
        use mooloop_core::midi::{BufferCcMapping, BufferCcTarget, BufferMidiMap};
        use mooloop_core::{MidiKind, MidiMessage, RelativeEncoding};

        let project = synth_project(ProjectChannel::sampler(0, 1));
        let mut map = BufferMidiMap::new(EffectTarget::Channel(0), 0);
        map.controls[0] = Some(BufferCcMapping {
            controller: 21,
            target: BufferCcTarget::Scrub {
                encoding: RelativeEncoding::BinaryOffset,
            },
        });

        let cc = |value| MidiMessage {
            offset: 0,
            channel: 0,
            kind: MidiKind::ControlChange {
                controller: 21,
                value,
            },
        };

        let mut render = RenderState::from_project(48_000, &project, &[]);
        let _ = render.apply_structural(install_effect(
            EffectTarget::Channel(0),
            0,
            default_effect(mooloop_core::EffectKind::Buffer),
        ));
        render.attach_buffer_midi_map(Arc::new(ArcSwapOption::from_pointee(map)));
        render.play();
        for _ in 0..4 {
            render.process_block(1024);
        }

        // 64 is the rest position of a binary-offset encoder.
        render.apply_midi(&[cc(64)]);
        render.process_block(1024);

        // A real turn detaches the head and moves it back in time.
        render.apply_midi(&[cc(32)]);
        render.process_block(1024);
        assert!(
            render.scrub_frames_per_tick() > 0.0,
            "scrub must resolve against tempo"
        );
    }
}


#[cfg(test)]
mod footprint {
    use super::*;

    /// What the render graph costs, and why. Both `MAX_CHANNELS` and
    /// `MAX_EFFECTS_PER_CHANNEL` are `u8::MAX + 1`, so the graph *addresses*
    /// the product of two full index spaces — 65,536 effect slots. It used to
    /// reserve all of them: 42.8 MiB before a project existed.
    ///
    /// Now it allocates neither ceiling up front. An empty effect slot costs a
    /// pointer, and a channel that does not exist costs nothing at all, so the
    /// price tracks the project rather than the limits.
    ///
    /// This is pinned rather than described because it is invisible at every
    /// individual definition — no single array looks unreasonable, and the
    /// number only appeared when they were multiplied. If this test fails,
    /// something changed a ceiling or widened a per-slot or per-channel
    /// struct; check the new figure is one worth paying before updating it.
    ///
    /// `docs/plans/modulator-capacity/`.
    #[test]
    fn the_render_graph_costs_what_the_project_uses() {
        use core::mem::size_of;

        assert_eq!(MAX_CHANNELS, 256);
        assert_eq!(MAX_EFFECTS_PER_CHANNEL, 256);

        // An occupied effect slot's state is half a kilobyte; an empty one is
        // the pointer that would otherwise have reserved it.
        assert_eq!(size_of::<EffectSlot>(), 496);
        assert_eq!(size_of::<Option<Box<EffectSlot>>>(), 8);
        assert_eq!(size_of::<EffectChain>(), 20_544);
        // 27,512 before the sampler learned to stretch. The extra 160 bytes
        // are a pointer to the stretch pool (the ~1.6 MB pool itself is
        // allocated only for a sampler that asks to stretch), the device's
        // copy of the tempo that fit-to-tempo derives its ratio from, and one
        // f64 per voice holding the pitch its note was struck at so a tune
        // edit can be applied to an already-sounding voice.
        assert_eq!(size_of::<ChannelStrip>(), 27_672);

        // Reserved whatever the project holds: the two small modulation
        // vectors, plus three vectors of pointers to per-channel storage.
        let fixed = (size_of::<ModRack>() + size_of::<ModulatorRack>()) * MAX_CHANNELS
            + MAX_CHANNELS * size_of::<usize>() * 3;
        assert_eq!(fixed / 1024, 439);

        // Paid per channel the project actually has.
        let per_live =
            size_of::<ChannelStrip>() + size_of::<EventList>() + size_of::<ControlOutputs>();
        assert_eq!(per_live, 46_112);

        // 42.8 MiB reserved at startup became 1.1 MiB for a sixteen-channel
        // project, with both ceilings untouched. Stretch added 2.5 KiB across
        // those sixteen channels; the pools it can allocate are not counted
        // here because a sampler only gets one by asking.
        assert_eq!((fixed + per_live * 16) / 1024, 1_159);
    }
}


