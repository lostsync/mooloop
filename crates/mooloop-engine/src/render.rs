//! JACK-independent render state shared by realtime playback and file export.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwapOption;
use mooloop_core::{
    audio_tap_index, compile_bus_graph, AutomationLane, AuxInParams, ChannelSource,
    CompiledAudioGraph, CompiledBusGraph, DeviceKind, OutletDescriptor, PublishesOutlets,
    Ds01Params, DrumSynthParams, EffectTarget, EngineCommand, GeneratorParams,
    ModDestinationDescriptor,
    ModRack, MonoSynthParams, MlM1Params, MlP8Params, ParamAddr, ParamOwner, PolySynthParams,
    Project,
    SamplerParams, SliceMap,
    chain_latency, clamp_bus, compile_latency, DEFAULT_STEPS, MAX_CONTAINER_DEPTH, MAX_SAMPLER_VOICES, MASTER_BUS, MAX_BUSES, MAX_CHANNELS, MAX_EFFECTS_PER_CHANNEL, MAX_LINEAR_GAIN,
    MAX_MODULATORS_PER_CHANNEL, STRIP_DESCRIPTORS, STRIP_PARAM_VOLUME,
};
use mooloop_core::modulation::{CONTROL_SOURCE_SLOTS, MAX_GENERATOR_OUTLETS};
#[cfg(test)]
use mooloop_dsp::build_effect;
use mooloop_dsp::{
    balance_gains, buffer_allocation_key, build_effect_at_tempo, pan_gains, AudioNode, Ds01,
    DrumSynth,
    AudioTaps, AuxIn, IntegerDelay, Event, EventList, ModulatorRack, MonoSynth, MlM1, MlP8,
    NoteGateEvents, PolySynth,
    ProcessContext, SampleData, Sampler, SpectrumAnalyzer, StereoBus, StretchPool, TimedEvent,
    CONTROL_RATE_FRAMES, MAX_BLOCK_SIZE, SILENCE_PEAK,
};

use crate::meters::{BusMeters, DeviceMeters, DeviceTelemetry, ModulatorMeters, PlayheadMeters};
use crate::sequencer::Sequencer;
use crate::transport::Transport;
use crate::{PreviewCommand, StructuralCommand, StructuralReclaim};

/// One generation's audio edges and the buffers they carry.
///
/// Prepared whole on the control thread and installed as one value, so the
/// executor can never observe a schedule against another generation's
/// buffers -- the same reason `CompiledBusGraph` and its render order travel
/// together.
///
/// **A tap exists only when somebody is subscribed to it.** ML-P8 declares
/// seven stereo outlets; materializing them all would be 448 KB a channel and
/// 7 MB across a full bank for a feature that is off by default. `buffers` is
/// as long as the number of distinct (producer, outlet) pairs somebody reads,
/// which on every project that has never authored an edge is zero.
pub struct AudioTapBank {
    graph: CompiledAudioGraph,
    /// One buffer per tap index the graph handed out, deduplicated by the
    /// (producer, outlet) pair: two channels reading the same `Osc 3` share
    /// one buffer rather than getting two copies of the same samples.
    buffers: Vec<StereoBus>,
}

impl AudioTapBank {
    /// Allocate the storage `graph` asks for. Control thread only: this is
    /// where the megabytes would be, which is why they are conditional.
    pub fn new(graph: CompiledAudioGraph) -> Self {
        Self {
            buffers: (0..graph.tap_count())
                .map(|_| StereoBus::with_capacity(MAX_BLOCK_SIZE))
                .collect(),
            graph,
        }
    }

    /// Whether `producer` owes a tap this block.
    ///
    /// The question the mute check asks: a muted producer still fills its
    /// tap, because mute is an output-stage decision about what reaches the
    /// bus and a pre-level tap is exactly the signal a muted source still
    /// has. A muted channel nobody reads still skips, which is what keeps
    /// mute a way of not spending the work.
    fn produces(&self, producer: usize) -> bool {
        self.graph
            .taps()
            .any(|(_, tap)| usize::from(tap.channel) == producer)
    }

    /// The buffer `consumer`'s resolved edge reads, if it has one.
    fn source(&self, consumer: usize) -> Option<&StereoBus> {
        self.buffers.get(self.graph.edge(consumer).tap()? as usize)
    }

    /// The port group `producer` owes this block, indexed by its own tap
    /// numbers.
    ///
    /// Built by walking the buffers rather than the device's outlet table, so
    /// each buffer is borrowed exactly once and the whole group is one set of
    /// disjoint mutable references with no unsafe code.
    fn ports(&mut self, producer: usize, outlets: &'static [OutletDescriptor]) -> AudioTaps<'_> {
        let mut ports = AudioTaps::none();
        let Self { graph, buffers } = self;
        for (index, buffer) in buffers.iter_mut().enumerate() {
            let Some(subscription) = graph.tap(index) else {
                continue;
            };
            if usize::from(subscription.channel) != producer {
                continue;
            }
            let Some(tap) = audio_tap_index(outlets, subscription.outlet) else {
                continue;
            };
            ports.set(tap, buffer);
        }
        ports
    }

    /// Empty every tap for the block about to render.
    ///
    /// Once for the whole bank rather than per producer, so a producer that
    /// stops playing -- or stops existing -- publishes silence rather than
    /// the block before.
    fn clear(&mut self, frames: usize) {
        for buffer in &mut self.buffers {
            buffer.clear(frames);
        }
    }
}

/// A displaced effect-slot occupant: the node plus the dry-align delay the
/// container allocated alongside it. Both halves are heap objects built on
/// the non-realtime side, so they must also be dropped there — the realtime
/// thread never frees a `Box` itself.
pub(crate) struct ReclaimedEffect {
    pub node: Option<Box<dyn AudioNode + Send>>,
    pub align: Option<Box<IntegerDelay>>,
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

/// One channel's modulator outputs for one block, captured at each control
/// subdivision.
///
/// Only the rack: a generator's outlets are published once a block, so they
/// ride beside this table rather than in it. Copying eight block-constant
/// values into all 256 tick rows would be 8 KB a live channel for numbers
/// that do not change, and `ControlSources` is what puts the two halves back
/// into one flat address space at the point a route reads them.
type ControlOutputs = [[f32; MAX_MODULATORS_PER_CHANNEL]; MAX_CONTROL_TICKS_PER_BLOCK];

/// Every channel's note gates at every control subdivision of one block, for
/// the modulators that key off them.
///
/// Tick-major, because a rack is advanced one subdivision at a time and needs
/// every channel's gates at that subdivision -- an envelope can follow another
/// channel's notes. At full size this is 192 KB, so it is kept and cleared a
/// row at a time rather than built on the stack of the audio callback.
type GateTable = [[NoteGateEvents; MAX_CHANNELS]; MAX_CONTROL_TICKS_PER_BLOCK];

fn default_generator_params(kind: DeviceKind) -> GeneratorParams {
    match kind {
        DeviceKind::Sampler => GeneratorParams::Sampler(SamplerParams::default()),
        DeviceKind::MonoSynth => GeneratorParams::MonoSynth(MonoSynthParams::default()),
        DeviceKind::PolySynth => GeneratorParams::PolySynth(PolySynthParams::default()),
        DeviceKind::MlM1 => GeneratorParams::MlM1(MlM1Params::default()),
        DeviceKind::MlP8 => GeneratorParams::MlP8(MlP8Params::default()),
        DeviceKind::Ds01 => GeneratorParams::Ds01(Ds01Params::default()),
        DeviceKind::DrumSynth => GeneratorParams::DrumSynth(DrumSynthParams::default()),
        DeviceKind::AuxIn => GeneratorParams::AuxIn(AuxInParams::default()),
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
    /// What this channel's generator published at the end of the *previous*
    /// block, constant for the whole of this one. Held here rather than in
    /// `outputs` because it is per block, not per tick.
    outlets: &'a [f32; MAX_GENERATOR_OUTLETS],
    ticks: usize,
}

impl ModulationBlock<'_> {
    /// The channel's control sources at one tick, as one flat address space.
    fn sources(&self, tick: usize) -> mooloop_core::modulation::ControlSources<'_> {
        mooloop_core::modulation::ControlSources {
            modulators: &self.outputs[tick],
            outlets: self.outlets,
        }
    }
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
/// channels (`docs/plans/archive/modulator-capacity/`).
///
/// Allocated on the control thread and installed, like the node beside it.
pub struct EffectSlot {
    /// Which device this is, as routes and lanes name it.
    ///
    /// The engine keeps indexing by position -- that is what makes the inner
    /// loop cheap -- and this is the one field that lets a position be turned
    /// back into an address without a table. `control_events_for_slot` reads
    /// it once per slot per block to build the address it looks a route up
    /// by; `EffectChain::slot_of` walks it the other way, on removal only.
    device: mooloop_core::DeviceId,
    /// The slot's persisted device kind, tracked independently of the
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
    /// For a container: how many of the rows after this one are inside it,
    /// and the ring that delays its dry copy by that run's declared latency.
    ///
    /// Both arrive on [`StructuralCommand::SetContainerSpan`] rather than on
    /// the value ring, for the reason `SetCompensation` is structural: the
    /// ring is allocated on the control thread and the displaced one is
    /// reclaimed there, because the audio thread may do neither. Zero and
    /// `None` on every leaf, which costs a byte and a pointer in a struct
    /// that is only allocated for occupied slots anyway.
    container_children: u8,
    container_align: Option<Box<IntegerDelay>>,
    /// Consecutive frames of silent input this slot has seen.
    ///
    /// Here rather than in a `[u32; MAX_EFFECTS_PER_CHANNEL]` beside the
    /// nodes for the reason this whole struct exists: an addressable-but-empty
    /// slot should cost a pointer, and there are 256 of them on each of 272
    /// chains. A slot that has never held a device never counts anything.
    silent_frames: u32,
}

impl EffectSlot {
    /// A fresh slot, allocated on the control thread to be installed.
    pub fn new() -> Self {
        Self {
            device: mooloop_core::DeviceId::UNASSIGNED,
            kind: None,
            base_params: None,
            resource_key: None,
            events: PendingEffectParams::empty(),
            bypassed: false,
            wet_dry: 1.0,
            input_trim: 1.0,
            output_trim: 1.0,
            container_children: 0,
            container_align: None,
            silent_frames: 0,
        }
    }
}

impl EffectSlot {
    /// A fresh slot that already knows which device it is about to hold.
    ///
    /// Every install goes through this rather than `new`, because a slot that
    /// does not know its identity is a slot no route can find.
    pub fn for_device(device: mooloop_core::DeviceId) -> Self {
        Self {
            device,
            ..Self::new()
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
    dry_align: [Option<Box<IntegerDelay>>; MAX_EFFECTS_PER_CHANNEL],
    /// Input analyzers follow effect slots during reorders. They are generic
    /// host instrumentation, not EQ-specific DSP state. The boxes are built
    /// with nodes so empty addressable slots stay compact.
    analyzers: [Option<Box<SpectrumAnalyzer>>; MAX_EFFECTS_PER_CHANNEL],
    /// One scratch buffer is enough: chain slots process sequentially, so no
    /// slot needs to retain its dry signal once its mix has been applied.
    /// Keeping this per-chain rather than per-slot makes the full 256-slot
    /// addressable chain practical.
    dry: StereoBus,
    /// One dry copy per *open* container, allocated the first time a
    /// container is installed on this chain and kept thereafter.
    ///
    /// Indexed by nesting depth rather than by slot, which is the whole
    /// saving: what a chain needs at once is one buffer per box it is
    /// currently inside, not one per box it holds. Ten sibling containers
    /// need one of these; four nested ones need four. A chain with no
    /// container at all pays a pointer.
    ///
    /// The graph only grows, in the same way and for the same reason channel
    /// storage does: freeing this would be a deallocation reached from a
    /// structural edit, and a chain that held a container once is likely to
    /// again.
    container_dry: Option<Box<ContainerScratch>>,
}

/// The dry copies a chain needs while it is inside containers.
///
/// Allocated on the control thread and installed, like every other piece of
/// chain state that owns heap.
pub struct ContainerScratch {
    dry: [StereoBus; MAX_CONTAINER_DEPTH],
}

impl ContainerScratch {
    pub fn new() -> Self {
        Self {
            dry: std::array::from_fn(|_| StereoBus::with_capacity(MAX_BLOCK_SIZE)),
        }
    }
}

impl Default for ContainerScratch {
    fn default() -> Self {
        Self::new()
    }
}

/// One container the chain is currently inside.
#[derive(Clone, Copy)]
struct OpenRun {
    /// One past the last row of the run: the index at which it closes.
    end: usize,
    /// The container's own row, which owns the mix and the dry-path ring.
    slot: usize,
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
            container_dry: None,
        }
    }

    /// Where `device` currently sits in this chain.
    ///
    /// Identity resolved to position, on the control-command drain rather
    /// than in a sample loop -- the same bargain `ModRack::slot_for` makes
    /// for modulator sources. Bounded by `bound` rather than by the 256
    /// addressable slots, so an empty chain answers in no time at all.
    fn slot_of(&self, device: mooloop_core::DeviceId) -> Option<usize> {
        if !device.is_assigned() {
            return None;
        }
        (0..self.bound).find(|slot| {
            self.slot(*slot)
                .is_some_and(|state| state.device == device)
        })
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

    /// Whether this slot holds a container rather than a leaf device.
    ///
    /// A container has no signal path of its own: its children are rows of
    /// this same chain, which the loop is already about to run in order.
    fn is_container(&self, slot: usize) -> bool {
        self.slot(slot)
            .and_then(|state| state.kind)
            .is_some_and(|kind| kind == mooloop_core::EffectKind::Chain)
    }

    /// How many rows the container in `slot` encloses. Zero for a leaf, and
    /// zero for an empty container, which is the same thing to this loop.
    fn container_children(&self, slot: usize) -> usize {
        self.slot(slot).map_or(0, |state| state.container_children as usize)
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
        align: Option<Box<IntegerDelay>>,
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
        align: Option<Box<IntegerDelay>>,
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

    /// Move the occupant of `from` to `to`, shifting everything between by
    /// one place: the same permutation `mooloop_core::structure::move_effect`
    /// performs on the model. A rotation of boxed pointers, so nothing is
    /// allocated or dropped here. Returns whether anything moved.
    fn move_slot(&mut self, from: usize, to: usize) -> bool {
        if from == to || from >= MAX_EFFECTS_PER_CHANNEL || to >= MAX_EFFECTS_PER_CHANNEL {
            return false;
        }
        let (low, high) = (from.min(to), from.max(to));
        if from < to {
            self.nodes[low..=high].rotate_left(1);
            self.slots[low..=high].rotate_left(1);
            self.dry_align[low..=high].rotate_left(1);
            self.analyzers[low..=high].rotate_left(1);
        } else {
            self.nodes[low..=high].rotate_right(1);
            self.slots[low..=high].rotate_right(1);
            self.dry_align[low..=high].rotate_right(1);
            self.analyzers[low..=high].rotate_right(1);
        }
        self.refresh_bound();
        true
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
        // No route in the channel's rack and no lane under the playhead means
        // every descriptor below can only reach its `continue`. Both are facts
        // about the channel, not the descriptor, and this runs once per effect
        // slot per block over everything the effect declares -- so asking here
        // is one question in place of a hundred and something.
        if !modulation.is_some_and(|modulation| modulation.rack.has_routes())
            && automation.is_none()
        {
            return;
        }
        let Some(state) = self.slot(slot) else {
            return;
        };
        let (Some(kind), Some(params)) = (state.kind, state.base_params) else {
            return;
        };
        // The address a route or lane could be naming this device by. Read
        // from the slot rather than derived from its position, which is the
        // whole of what durable identity costs the realtime path: one field
        // read per slot per block, in place of a cast.
        let device = state.device;
        let ticks = modulation
            .map(|modulation| modulation.ticks)
            .into_iter()
            .chain(automation.map(|automation| automation.ticks))
            .max()
            .unwrap_or(0);

        for descriptor in kind.descriptors() {
            let destination = ParamAddr::effect(scope, device, descriptor.id);
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
                            .offset_for(destination, modulation.sources(tick), &policy)
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
        // A chain arriving without identities is a project that skipped
        // `Project::assign_device_ids`, and the symptom would be every route
        // and lane on it quietly resolving to nothing. Caught here, on the
        // control thread, rather than found by ear.
        debug_assert!(
            slots.iter().all(|effect| effect.id.is_assigned()),
            "a chain reached the engine with unassigned device identities; \
             `Project::assign_device_ids` did not run over it"
        );
        for (slot, effect) in slots.iter().take(MAX_EFFECTS_PER_CHANNEL).enumerate() {
            let node = build_effect_at_tempo(effect.params, sample_rate, bpm);
            let align = IntegerDelay::new(node.dry_path_latency_frames()).map(Box::new);
            let displaced = self.install(
                slot,
                effect.kind(),
                effect_resource_key(effect.params),
                node,
                align,
                Box::new(SpectrumAnalyzer::new()),
                Box::new(EffectSlot::for_device(effect.id)),
            );
            if !displaced.is_empty() {
                reclaim.push(displaced);
            }
            // A container's span and the ring that delays its dry copy.
            // Allocated here rather than sent, because `load` already runs on
            // the control thread inside `install_project`.
            let children = match effect.params {
                mooloop_core::EffectParams::Chain(chain) => chain.children,
                _ => 0,
            };
            let align = (children > 0)
                .then(|| IntegerDelay::new(mooloop_core::run_latency(slots, slot)))
                .flatten()
                .map(Box::new);
            // `install` seeds the kind's defaults; a saved chain overrides
            // them with what was persisted.
            if let Some(state) = self.slot_mut(slot) {
                state.base_params = Some(effect.params);
                state.bypassed = effect.bypassed;
                state.wet_dry = effect.wet_dry.clamp(0.0, 1.0);
                state.input_trim = effect.input_trim.clamp(0.0, MAX_LINEAR_GAIN);
                state.output_trim = effect.output_trim.clamp(0.0, MAX_LINEAR_GAIN);
                state.container_children = children;
                state.container_align = align;
            }
        }
        // One allocation for the whole chain, and only for a chain that
        // actually holds a box.
        if self.container_dry.is_none()
            && slots.iter().any(|effect| effect.kind() == mooloop_core::EffectKind::Chain)
        {
            self.container_dry = Some(Box::new(ContainerScratch::new()));
        }
    }

    /// Record how loud this slot's input was, and return how many
    /// consecutive frames of silence it has now seen.
    ///
    /// Resets to zero the moment anything audible arrives, which is what
    /// makes waking up a property of the audio rather than of a timer.
    fn note_input_level(&mut self, slot: usize, peak: f32, frames: usize) -> u32 {
        let Some(state) = self.slot_mut(slot) else {
            return 0;
        };
        state.silent_frames = if peak <= SILENCE_PEAK {
            state.silent_frames.saturating_add(frames as u32)
        } else {
            0
        };
        state.silent_frames
    }

    /// Whether this slot's device can be left uncalled this block.
    ///
    /// Three conditions, and each one is load-bearing:
    ///
    /// - **Its input is silent.** `silent` is zero on any block that carried
    ///   audio, so this is the clause that makes waking instant.
    /// - **Its dry-path aligner has emptied.** The host stops feeding a
    ///   sleeping slot's ring, so a ring still holding pre-silence audio would
    ///   emit it on the wet/dry blend when the slot wakes. Only the
    ///   oversampled drive has one at all, and its tail is sixty times longer,
    ///   but this is stated rather than assumed.
    /// - **Its state has settled, or its tail has run out.** The two are
    ///   different answers to the same question and a device may give either:
    ///   a filter reads its own state exactly, a reverb declares how long its
    ///   decay takes. `>` rather than `>=` so a device that never converted
    ///   -- `u32::MAX`, which `silent` saturates at -- is never skipped.
    fn may_sleep(&self, slot: usize, silent: u32) -> bool {
        let Some(node) = self.nodes[slot].as_ref() else {
            return false;
        };
        silent > 0
            && silent >= node.dry_path_latency_frames()
            && (node.is_at_rest() || silent > node.tail_frames())
    }

    /// Whether every occupied slot in this chain would be skipped this
    /// block: the condition a whole channel strip has to satisfy before it
    /// can be left uncalled.
    ///
    /// A bypassed slot runs no device, so the only thing it can still be
    /// holding is its dry-path aligner, and that is empty once the slot has
    /// seen silence for as long as the ring is. An empty slot is trivially
    /// at rest.
    fn is_at_rest(&self) -> bool {
        (0..self.bound).all(|slot| {
            let Some(node) = self.nodes[slot].as_ref() else {
                return true;
            };
            let silent = self.slot(slot).map_or(0, |state| state.silent_frames);
            if silent == 0 {
                return false;
            }
            if silent < node.dry_path_latency_frames() {
                return false;
            }
            self.bypassed(slot) || node.is_at_rest() || silent > node.tail_frames()
        })
    }

    /// Sleep the whole chain for one block, because the strip around it is
    /// asleep and nothing will be handed to it.
    ///
    /// The bookkeeping `process` would have done, minus the audio: every
    /// occupied slot's silence counter grows so the chain stays at rest, and
    /// every device gets the chance to move whatever it runs on the clock. A
    /// chain is only ever slept while `is_at_rest` holds, so this cannot be
    /// reached with anything still decaying in it.
    fn sleep(&mut self, context: &ProcessContext) {
        for slot in 0..self.bound {
            // A bypassed slot's device is not called at all while the strip
            // is running, so bypass has already frozen whatever it runs on
            // the clock. Moving it here would make a sleeping strip and a
            // running one disagree, which is the one thing none of this may
            // do. Its counter still grows: what a bypassed slot holds is its
            // aligner, and that is empty once the silence outlasts the ring.
            let bypassed = self.bypassed(slot);
            let Some(node) = self.nodes[slot].as_mut() else {
                continue;
            };
            if !bypassed {
                node.skip_block(context);
            }
            if let Some(state) = self.slots[slot].as_deref_mut() {
                state.silent_frames = state.silent_frames.saturating_add(context.frames as u32);
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
        skip_idle: bool,
    ) {
        // The containers this chain is currently inside, innermost last.
        // Fixed at the depth cap and never grown, so nothing here allocates.
        let mut open = [OpenRun { end: 0, slot: 0 }; MAX_CONTAINER_DEPTH];
        let mut depth = 0usize;
        // Set past the end of a bypassed container's run: a bypassed box
        // skips its whole run rather than its own row.
        let mut skip_until = 0usize;

        for slot in 0..self.bound {
            // Close every run that ends here, innermost first. A `while`
            // rather than an `if` because several boxes can end on the same
            // row, and they nest, so the innermost is always on top.
            while depth > 0 && open[depth - 1].end == slot {
                depth -= 1;
                let run = open[depth];
                self.close_run(run, depth, bus, context);
            }
            if slot < skip_until {
                continue;
            }
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
            if self.is_container(slot) {
                // **A container never processes audio.** Its children are
                // rows of this same chain and the loop is about to run them;
                // all it does here is keep a copy of what is going in, so the
                // crossfade at the end of its run has something to blend.
                //
                // It also does not take the *slot's* leaf wet/dry, which
                // would be meaningless (there is no node to be wet with) and,
                // through the equal-power crossfade, not even bit-exact -- a
                // transparent node blended at unity still leaks a cos(pi/2)
                // of the dry.
                let (left, right) = bus.peak(context.frames);
                self.note_input_level(slot, left.max(right), context.frames);
                if let Some((meters, _, target)) = device_display {
                    meters.publish_input(target, slot + 1, left, right);
                    meters.publish_output(target, slot + 1, left, right);
                }
                if let Some(state) = self.slots[slot].as_mut() {
                    state.events.clear();
                }
                let children = self.container_children(slot);
                if children == 0 || depth >= MAX_CONTAINER_DEPTH {
                    // An empty box, or one nested past the cap, is a row that
                    // does nothing. Deeper than the cap is unreachable
                    // through the interface and loadable from a file, so it
                    // has to mean *something* rather than panic.
                    continue;
                }
                let open_run = OpenRun {
                    end: (slot + 1 + children).min(self.bound),
                    slot,
                };
                if self.bypassed(slot) {
                    // **Bypassing a box bypasses the run**, and costs exactly
                    // the frames that run declares. Same argument as a
                    // bypassed device: a bypass that shortened the path would
                    // move the channel in time against every other one, so
                    // A/B-ing the box would also A/B the timing.
                    if let Some(state) = self.slots[slot].as_mut() {
                        if let Some(align) = &mut state.container_align {
                            align.process(
                                &mut bus.l[..context.frames],
                                &mut bus.r[..context.frames],
                            );
                        }
                    }
                    skip_until = open_run.end;
                    continue;
                }
                if let Some(scratch) = self.container_dry.as_mut() {
                    scratch.dry[depth].l[..context.frames]
                        .copy_from_slice(&bus.l[..context.frames]);
                    scratch.dry[depth].r[..context.frames]
                        .copy_from_slice(&bus.r[..context.frames]);
                    open[depth] = open_run;
                    depth += 1;
                }
                continue;
            }
            if self.bypassed(slot) {
                // A bypassed slot keeps its queued events until re-enabled, so
                // knob turns made while bypassed are not lost.
                if let Some(align) = &mut self.dry_align[slot] {
                    // **Bypass is time transparent, not latency free.** The
                    // signal goes through the slot's own ring, so a bypassed
                    // node still costs exactly the frames it declares.
                    //
                    // Two things depend on this. The latency plan sums a
                    // chain's *declared* latency including bypassed slots
                    // (`mooloop_core::chain_latency`, and every host works this
                    // way), so a bypass that shortened the path would leave
                    // every other channel over-compensated against it. And
                    // toggling one would move the channel in time, so A/B-ing
                    // an effect would also A/B the timing and neither answer
                    // would be about the effect.
                    //
                    // This also subsumes what the branch did before, which was
                    // to push a shadow copy through the ring so re-enabling
                    // never blended in pre-bypass audio: the ring sees the same
                    // samples either way.
                    align.process(
                        &mut bus.l[..context.frames],
                        &mut bus.r[..context.frames],
                    );
                }
                // Measured *after* the aligner on purpose. A bypassed slot
                // is its ring and nothing else, so the level coming out of
                // the ring is exactly the level the slot still has to offer,
                // and counting that is what lets `is_at_rest` above know the
                // ring has emptied without looking inside it.
                let (left, right) = bus.peak(context.frames);
                self.note_input_level(slot, left.max(right), context.frames);
                if let Some((meters, _, target)) = device_display {
                    meters.publish_input(target, slot + 1, left, right);
                    meters.publish_output(target, slot + 1, left, right);
                }
                continue;
            }
            if self.nodes[slot].is_some() {
                // Taken once, before the trim, and used twice: for the
                // decision below and for this slot's input meter. Peak is
                // linear in a non-negative gain, so scaling it by the trim is
                // exactly what a second scan after the trim would have found.
                // Silence detection therefore costs nothing the meters were
                // not already paying, and the chain does one scan a slot
                // rather than two.
                let (peak_l, peak_r) = bus.peak(context.frames);
                let silent = self.note_input_level(slot, peak_l.max(peak_r), context.frames);
                if skip_idle && self.may_sleep(slot, silent) {
                    // The one thing a sleeping node is still owed: whatever
                    // it runs on the clock rather than on its input. A
                    // reverb's line modulation and a chorus's LFO have to
                    // arrive where the transport says, or the device would
                    // sound different after a gap and *how* different would
                    // depend on the host's buffer size.
                    if let Some(node) = self.nodes[slot].as_mut() {
                        node.skip_block(context);
                    }
                    // Nothing is published and nothing is cleared. The device
                    // meters are peak-hold cells the GUI empties as it reads
                    // them, so not writing one *is* publishing silence; and
                    // the queued parameter events stay queued exactly as they
                    // do under bypass, so a knob turned while a channel was
                    // quiet still lands when it wakes.
                    //
                    // Nothing else here touches the node, which is what makes
                    // waking instant: the first block with audio in it
                    // processes normally, from the state the node had when it
                    // stopped, with no ramp and no reset.
                    continue;
                }
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
                    meters.publish_input(target, slot + 1, peak_l * input_trim, peak_r * input_trim);
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
        // A run whose span reaches the end of the populated chain closes
        // here. `bound` clamps `end` on the way in, so this is the same
        // arithmetic rather than a second rule.
        while depth > 0 {
            depth -= 1;
            let run = open[depth];
            self.close_run(run, depth, bus, context);
        }
    }

    /// Crossfade a container's run back against the dry copy taken when it
    /// opened, delayed by the run's declared latency.
    ///
    /// The per-slot dry path generalised from one device to a span, and
    /// deliberately the same crossfade: equal-power, because the runs people
    /// will actually blend are the decorrelated ones a linear fade dips 3 dB
    /// in the middle of. `docs/GAIN_STRUCTURE.md` records the trade.
    fn close_run(
        &mut self,
        run: OpenRun,
        depth: usize,
        bus: &mut StereoBus,
        context: &ProcessContext,
    ) {
        let mix = self
            .slot(run.slot)
            .and_then(|state| state.base_params)
            .and_then(|params| params.get(mooloop_core::CHAIN_PARAM_MIX))
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        // Two disjoint fields at once: the ring lives on the container's slot
        // and the copy it delays lives on the chain.
        let Self {
            slots,
            container_dry,
            ..
        } = self;
        let Some(scratch) = container_dry.as_mut() else {
            return;
        };
        // Aligned before the blend, not after: the wet path came out of the
        // run's devices `run_latency` frames late, so the copy has to wait
        // the same amount or the two comb. Run unconditionally, including at
        // full wet, because the ring has to keep advancing or the mix would
        // read stale audio the first time it moved off unity.
        if let Some(state) = slots[run.slot].as_mut() {
            if let Some(align) = &mut state.container_align {
                align.process(
                    &mut scratch.dry[depth].l[..context.frames],
                    &mut scratch.dry[depth].r[..context.frames],
                );
            }
        }
        // Nothing to add at full wet, and skipping it is correctness rather
        // than an optimisation: the equal-power crossfade leaves a
        // `cos(pi/2)` of the dry -- about 6e-8 -- so *running* it would leak
        // a fraction of the dry into a run the user asked to hear whole, and
        // step 02's bit-exact null is what would break.
        if mix >= 1.0 {
            return;
        }
        let blend = mix * core::f32::consts::FRAC_PI_2;
        let (dry_gain, wet_gain) = (blend.cos(), blend.sin());
        for frame in 0..context.frames {
            bus.l[frame] = scratch.dry[depth].l[frame] * dry_gain + bus.l[frame] * wet_gain;
            bus.r[frame] = scratch.dry[depth].r[frame] * dry_gain + bus.r[frame] * wet_gain;
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
    // Asked before the table exists rather than after. `StripSegments` is two
    // kilobytes, every byte of it written by the initializer below, and a
    // channel with an empty rack and no lane throws all of it away -- which
    // is every channel on a song nobody has automated or routed.
    if !modulation.rack.has_routes() && automation.is_none() {
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
                    .offset_for(destination, modulation.sources(tick), &policy)
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
    /// How long this bus waits before summing into the bus it feeds. Same
    /// contract as a channel's, and always `None` on the master, which feeds
    /// nothing.
    compensation: Option<Box<IntegerDelay>>,
    /// Whether `bus` may hold anything but zeros: set when something sums
    /// into it and when the strip runs, since a chain with a tail writes into
    /// a buffer nothing fed. Read at the top of the next block to decide
    /// whether it needs emptying at all.
    dirty: bool,
    /// Frames since anything last reached this bus. The chain's tail is
    /// measured against it, and so is the compensation ring: once silence has
    /// been fed for the ring's whole length every slot in it is a zero.
    silent_frames: u32,
    /// Whether the strip has already done the once-only work of going to
    /// sleep, so that emptying the buffer is not repeated every idle block.
    sleeping: bool,
}

impl BusStrip {
    fn new() -> Self {
        Self {
            effects: EffectChain::new(),
            bus: StereoBus::with_capacity(MAX_BLOCK_SIZE),
            // Unity, not a channel's 0.8: see `mooloop_core::MixerBus::new`.
            output: OutputStage::new(1.0),
            compensation: None,
            // Nothing has been written yet, but the first block empties it
            // anyway rather than reasoning about a buffer it did not fill.
            dirty: true,
            silent_frames: 0,
            sleeping: false,
        }
    }

    /// Whether this bus can be left unrendered for a block in which nothing
    /// reached it.
    ///
    /// Two conditions, and the second is the one that is easy to miss. The
    /// chain has to have finished — a reverb on a bus must be allowed to
    /// decay after the last thing feeding it stops. And the compensation ring
    /// has to have been fed silence for at least its own length, because
    /// until then it is still holding audio it has not emitted yet, and
    /// freezing it would strand that audio until the bus woke.
    fn is_resting(&self) -> bool {
        self.effects.is_at_rest()
            && self.silent_frames
                >= self.compensation.as_ref().map_or(0, |delay| delay.frames() as u32)
    }

    fn reset(&mut self, reclaim: &mut Reclaim) {
        self.effects.clear(reclaim);
        self.output = OutputStage::new(1.0);
        // The displaced ring leaves on the same carrier a displaced dry-path
        // aligner does: it is the same type doing the same job one level out,
        // and inventing a second channel for it would only mean two things to
        // drain.
        if let Some(delay) = self.compensation.take() {
            reclaim.push(ReclaimedEffect {
                node: None,
                align: Some(delay),
                analyzer: None,
                state: None,
                channel: None,
            });
        }
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
    mlp8: MlP8,
    ds01: Ds01,
    aux_in: AuxIn,
    active_source: DeviceKind,
    /// What this channel's generator published during the block just
    /// rendered, indexed by outlet id.
    ///
    /// Read at the *start* of the next block, which is where the one
    /// declared block of outlet latency physically lives: it is not a delay
    /// line or a scheduling rule anybody has to remember, it is the fact
    /// that the control table is filled before the strips run. That is also
    /// what makes an offline render agree with a live take and stops graph
    /// order deciding what a route hears.
    published_outlets: [f32; MAX_GENERATOR_OUTLETS],
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
    /// How long this channel waits before summing into its bus, so that
    /// everything arriving there comes from the same moment.
    ///
    /// `None` is the common case and means this channel *is* the longest path
    /// into its bus, so nothing is owed. The length is decided off-thread by
    /// `mooloop_core::compile_latency` and the ring arrives preallocated;
    /// see `docs/plans/latency-compensation/`.
    compensation: Option<Box<IntegerDelay>>,
    /// Consecutive frames of silence the generator has put on this strip's
    /// bus, counted where the source meter is already read.
    ///
    /// A generator has no input to go quiet, so this is its *output*: the one
    /// measurement that covers every reason a device might still be making
    /// sound, including the finishing stages that outlive its voices.
    source_silent_frames: u32,
    /// Whether the strip was left uncalled last block. Only used to do the
    /// once-off tidying that falling asleep needs -- emptying the bus and the
    /// compensation ring -- rather than repeating it every idle block.
    sleeping: bool,
}

impl ChannelStrip {
    fn new(
        sample_slot: Arc<ArcSwapOption<SampleData>>,
        slice_slot: Arc<ArcSwapOption<SliceMap>>,
        sample_rate: u32,
    ) -> Self {
        Self {
            sampler: Sampler::new(sample_slot, slice_slot, SamplerParams::default(), sample_rate),
            drum_synth: DrumSynth::new(DrumSynthParams::default(), sample_rate),
            mono_synth: MonoSynth::new(MonoSynthParams::default(), sample_rate),
            poly_synth: PolySynth::new(PolySynthParams::default(), sample_rate),
            mlm1: MlM1::new(MlM1Params::default(), sample_rate),
            mlp8: MlP8::new(MlP8Params::default(), sample_rate),
            ds01: Ds01::new(Ds01Params::default(), sample_rate),
            aux_in: AuxIn::new(AuxInParams::default(), sample_rate),
            active_source: DeviceKind::Sampler,
            published_outlets: [0.0; MAX_GENERATOR_OUTLETS],
            source_base: GeneratorParams::Sampler(SamplerParams::default()),
            effects: EffectChain::new(),
            bus: StereoBus::with_capacity(MAX_BLOCK_SIZE),
            output: OutputStage::new(0.8),
            destination: MASTER_BUS,
            compensation: None,
            source_silent_frames: 0,
            sleeping: false,
        }
    }

    fn reset_sources_to_defaults(&mut self, source: DeviceKind) {
        self.source_base = default_generator_params(source);
        self.sampler.reset();
        self.drum_synth.reset();
        self.mono_synth.reset();
        self.poly_synth.reset();
        self.mlm1.reset();
        self.mlp8.reset();
        self.ds01.reset();
        self.aux_in.reset();
        self.sampler.set_params(SamplerParams::default());
        self.drum_synth.set_params(DrumSynthParams::default());
        self.mono_synth.set_params(MonoSynthParams::default());
        self.poly_synth.set_params(PolySynthParams::default());
        self.mlm1.set_params(MlM1Params::default());
        self.mlp8.set_params(MlP8Params::default());
        self.ds01.set_params(Ds01Params::default());
        self.aux_in.set_params(AuxInParams::default());
        self.active_source = source;
    }

    fn reset_slot(&mut self, source: DeviceKind, reclaim: &mut Reclaim) {
        self.reset_sources_to_defaults(source);
        self.effects.clear(reclaim);
        self.output = OutputStage::new(0.8);
        self.destination = MASTER_BUS;
    }

    /// Move one internal route's depth on both the base and the running node.
    ///
    /// Not routed through `push_source_base` like a knob is, because a route
    /// amount is the one authored value that must not rebuild the compiled
    /// topology: the node retunes the row it already has.
    fn set_source_route_amount(&mut self, route: u16, amount: f32) {
        let moved = self
            .source_base
            .internal_routes_mut()
            .is_some_and(|routes| routes.set_amount(route, amount));
        if !moved {
            return;
        }
        if matches!(self.source_base, GeneratorParams::MlP8(_)) {
            self.mlp8.set_route_amount(route, amount);
        }
    }

    /// Hand the authored base to whichever generator this channel is running.
    ///
    /// Allocation-free for every kind, which is what makes it callable from
    /// the audio thread: a generator keeps no queue between blocks, so its
    /// base is applied straight through rather than staged.
    fn push_source_base(&mut self) {
        match self.source_base {
            GeneratorParams::Sampler(params) => self.sampler.set_params(params),
            GeneratorParams::MonoSynth(params) => self.mono_synth.set_params(params),
            GeneratorParams::PolySynth(params) => self.poly_synth.set_params(params),
            GeneratorParams::MlM1(params) => self.mlm1.set_params(params),
            GeneratorParams::MlP8(params) => self.mlp8.set_params(params),
            GeneratorParams::Ds01(params) => self.ds01.set_params(params),
            GeneratorParams::DrumSynth(params) => self.drum_synth.set_params(params),
            GeneratorParams::AuxIn(params) => self.aux_in.set_params(params),
        }
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
                GeneratorParams::DrumSynth(state.params)
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
            ChannelSource::MlP8(state) => {
                self.mlp8.set_params(state.params);
                GeneratorParams::MlP8(state.params)
            }
            ChannelSource::Ds01(state) => {
                self.ds01.set_params(state.params);
                GeneratorParams::Ds01(state.params)
            }
            ChannelSource::AuxIn(state) => {
                self.aux_in.set_params(state.params);
                // Snapped rather than ramped: a loaded project starts at the
                // level it was saved at, with nothing to click.
                self.aux_in.reset();
                GeneratorParams::AuxIn(state.params)
            }
        };
    }

    /// The generator this channel is running, as the node it is.
    ///
    /// The rest contract is on `AudioNode`, so asking a channel whether its
    /// source has anything left to do should not mean a second `match` over
    /// the eight device kinds every time. This is that match, once.
    fn source_node(&self) -> &dyn AudioNode {
        match self.active_source {
            DeviceKind::Sampler => &self.sampler,
            DeviceKind::DrumSynth => &self.drum_synth,
            DeviceKind::MonoSynth => &self.mono_synth,
            DeviceKind::PolySynth => &self.poly_synth,
            DeviceKind::MlM1 => &self.mlm1,
            DeviceKind::MlP8 => &self.mlp8,
            DeviceKind::Ds01 => &self.ds01,
            DeviceKind::AuxIn => &self.aux_in,
        }
    }

    fn source_node_mut(&mut self) -> &mut dyn AudioNode {
        match self.active_source {
            DeviceKind::Sampler => &mut self.sampler,
            DeviceKind::DrumSynth => &mut self.drum_synth,
            DeviceKind::MonoSynth => &mut self.mono_synth,
            DeviceKind::PolySynth => &mut self.poly_synth,
            DeviceKind::MlM1 => &mut self.mlm1,
            DeviceKind::MlP8 => &mut self.mlp8,
            DeviceKind::Ds01 => &mut self.ds01,
            DeviceKind::AuxIn => &mut self.aux_in,
        }
    }

    /// Record how loud the generator was this block, and return how many
    /// consecutive frames of silence it has now produced.
    fn note_source_level(&mut self, peak: f32, frames: usize) -> u32 {
        self.source_silent_frames = if peak <= SILENCE_PEAK {
            self.source_silent_frames.saturating_add(frames as u32)
        } else {
            0
        };
        self.source_silent_frames
    }

    /// Whether this strip can be left unrendered for a block.
    ///
    /// Three questions, and the generator is asked two of them. `is_at_rest`
    /// is about its voices -- a device with one still releasing says no --
    /// and the silence count is about everything downstream of them inside
    /// the device, which is how a finishing stage that outlives its voices is
    /// covered without the host knowing one exists. Then the chain, which has
    /// been counting the same way slot by slot.
    ///
    /// Aux In answers the first question with a flat no, and that is the
    /// point: its sound is another channel's, and it can start without an
    /// event of its own.
    fn is_idle(&self) -> bool {
        let source = self.source_node();
        source.is_at_rest()
            && self.source_silent_frames > source.tail_frames()
            && self.effects.is_at_rest()
    }

    /// Spend a block asleep: move whatever runs on the clock, and the first
    /// time round, empty what would otherwise be emitted on waking.
    fn sleep(&mut self, context: &ProcessContext) {
        self.source_node_mut().skip_block(context);
        self.effects.sleep(context);
        self.source_silent_frames = self.source_silent_frames.saturating_add(context.frames as u32);
        if self.sleeping {
            return;
        }
        self.sleeping = true;
        // Once, on the way down. Nothing writes either of these while the
        // strip is asleep, so emptying them again every block would be work
        // to reach a state they are already in.
        //
        // The bus so nothing downstream can read what the last audible block
        // left in it, and the compensation ring for the reason the mute path
        // empties it: a silent producer's pipeline is silent too, and a ring
        // still holding pre-silence audio would emit it on the first block
        // after the strip wakes.
        self.bus.clear(context.frames.min(self.bus.capacity()));
        if let Some(delay) = self.compensation.as_mut() {
            delay.reset();
        }
    }

    fn choke_group(&self) -> u8 {
        match self.active_source {
            DeviceKind::Sampler => self.sampler.choke_group(),
            DeviceKind::DrumSynth => self.drum_synth.choke_group(),
            DeviceKind::Ds01 => self.ds01.choke_group(),
            DeviceKind::MonoSynth
            | DeviceKind::PolySynth
            | DeviceKind::MlM1
            | DeviceKind::MlP8
            | DeviceKind::AuxIn => 0,
        }
    }

    /// Render the generator, filling whatever audio outlets are subscribed
    /// and reading whatever edge resolved.
    ///
    /// `source` and `ports` are supplied for the duration of the call and not
    /// retained, which is `AUDIO_ARCHITECTURE.md`'s rule for auxiliary
    /// buffers. Both are empty on every project that has never authored an
    /// edge, and the devices that can use them branch once a render range on
    /// that fact.
    fn process(
        &mut self,
        context: &ProcessContext,
        events: &EventList,
        source: Option<&StereoBus>,
        ports: &mut AudioTaps,
    ) {
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
            DeviceKind::MlP8 => self
                .mlp8
                .process_publishing(context, &mut self.bus, events, ports),
            DeviceKind::Ds01 => self
                .ds01
                .process_publishing(context, &mut self.bus, events, ports),
            DeviceKind::AuxIn => self
                .aux_in
                .process_from(context, &mut self.bus, source, events, ports),
        }
        self.publish_outlets();
    }

    /// Take the generator's published control outlets for this block.
    ///
    /// Only the active generator publishes, and the rest of the band is
    /// zeroed rather than left holding the last device's values: replacing a
    /// source must not leave a route reading a signal from an instrument that
    /// is no longer there. A generator with nothing to publish clears the
    /// band, which is the same thing said for a device that has not
    /// implemented outlets yet.
    fn publish_outlets(&mut self) {
        self.published_outlets = [0.0; MAX_GENERATOR_OUTLETS];
        match self.active_source {
            DeviceKind::MlP8 => {
                let published = self.mlp8.publish_outlets();
                self.published_outlets[..published.len()].copy_from_slice(&published);
            }
            DeviceKind::Ds01 => {
                let published = self.ds01.publish_outlets();
                self.published_outlets[..published.len()].copy_from_slice(&published);
            }
            _ => {}
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
        right[0].dirty = true;
    } else {
        let source = &right[0];
        left[into].bus.add_from(&source.bus, frames);
        left[into].dirty = true;
    }
}


/// The most auditions one block may carry. A block is a couple of
/// milliseconds; anything past this is a stuck key, not playing.
const MAX_AUDITIONS_PER_BLOCK: usize = 16;

/// Note ids for auditioned notes, kept clear of the sequencer's.
///
/// Counted down from the top rather than up from zero because the sequencer
/// mints ids from its patterns upwards: a note-off has to find the note-on it
/// belongs to, and two id spaces that can meet would let an audition release
/// a sequenced note.
fn audition_note_id(note: u8) -> u64 {
    u64::MAX - u64::from(note)
}

/// One note the UI asked a channel to sound this block.
#[derive(Clone, Copy)]
struct Audition {
    channel: u8,
    event: Event,
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
    /// The channels' slice maps, published beside their samples and read by
    /// the sampler voice at note-on.
    slice_slots: Arc<Vec<Arc<ArcSwapOption<SliceMap>>>>,
    /// The full bus bank, master first. Always `MAX_BUSES` long, so assigning
    /// a channel to any bus is a bounded mutation rather than an allocation.
    buses: Vec<BusStrip>,
    /// Destinations and their matching render order, compiled together off the
    /// audio thread. The executor only installs or walks this value.
    bus_graph: CompiledBusGraph,
    /// The channels' audio edges, the order that satisfies them, and the
    /// buffers they carry. Boxed so a whole generation crosses to the
    /// executor as one pointer swap and the displaced one is freed off the
    /// audio thread.
    audio: Box<AudioTapBank>,
    /// One block of the tap a consumer is reading, copied in before its
    /// generator runs.
    ///
    /// A copy rather than a borrow because the same call may write this
    /// channel's own taps: those are provably different buffers -- a device
    /// cannot subscribe to itself -- but nothing in the type system knows it,
    /// and one scratch buffer for the whole bank is a far smaller price than
    /// unsafe aliasing or one buffer a channel.
    aux_scratch: StereoBus,
    events: Vec<Box<EventList>>,
    /// The saved matrix and the runnable sources are deliberately separate:
    /// the former is editable/persisted configuration; the latter contains
    /// LFO phase and other realtime-only state.
    modulation: Vec<ModRack>,
    modulators: Vec<ModulatorRack>,
    /// A full block of resolved control signal is 8 KiB; reserving one for
    /// every addressable channel cost 2 MiB before a project existed
    /// (`docs/plans/archive/modulator-capacity/`).
    control_outputs: Vec<Box<ControlOutputs>>,
    /// This block's note gates, kept rather than rebuilt. See [`GateTable`].
    gate_ticks: Box<GateTable>,
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
    /// Notes the UI asked for since the last block, waiting to be dispatched.
    ///
    /// Held rather than applied on arrival because the command drain runs
    /// before `process_block_inner` clears the event lists, so a note pushed
    /// straight into one would be thrown away before anything read it. A
    /// fixed array, so filling it allocates nothing.
    auditions: [Option<Audition>; MAX_AUDITIONS_PER_BLOCK],
    /// How many channel-blocks have been skipped since this state was built.
    ///
    /// One `u64` for the whole engine and one add per skipped strip. It is
    /// here so the equivalence tests can say that the two renders they
    /// compared were not simply the same render twice: a skip mechanism that
    /// never fires would pass every one of them.
    slept_strip_blocks: u64,
    /// Whether devices and strips with nothing to do may be left uncalled.
    ///
    /// On by default and not exposed as a user setting. It exists so the
    /// equivalence tests can render the same project both ways and compare
    /// the two sample for sample, which is the only way to hold a skip
    /// honest: a mechanism whose whole claim is that it changes nothing has
    /// to be checkable against the thing it claims not to change.
    skip_idle: bool,
}

/// One-shot straight to the master output: no envelope, no channel strip.
/// A browser preview should sound like the file, not like the project.
struct PreviewVoice {
    sample: Arc<SampleData>,
    position: usize,
}

impl RenderState {
    pub fn new(
        sample_rate: u32,
        sample_slots: Arc<Vec<Arc<ArcSwapOption<SampleData>>>>,
        slice_slots: Arc<Vec<Arc<ArcSwapOption<SliceMap>>>>,
    ) -> Self {
        #![allow(clippy::let_and_return)]
        // Deliberately empty. Channels are materialized from a project on
        // this thread, or pushed one at a time through the structural ring.
        let strips = Vec::with_capacity(MAX_CHANNELS);
        let slots_for_growth = sample_slots.clone();
        let slice_slots_for_growth = slice_slots.clone();
        let mut state = Self {
            transport: Transport::new(sample_rate),
            skip_idle: true,
            slept_strip_blocks: 0,
            sequencer: Sequencer::new(1, 1, DEFAULT_STEPS as usize, mooloop_core::Ppq::DEFAULT),
            strips,
            sample_slots: slots_for_growth,
            slice_slots: slice_slots_for_growth,
            buses: (0..MAX_BUSES).map(|_| BusStrip::new()).collect(),
            bus_graph: CompiledBusGraph::default(),
            // A project with no subscriptions holds no buffers at all, which
            // is the whole design: the identity order costs nothing and
            // renders exactly what the engine rendered before this existed.
            audio: Box::new(AudioTapBank::new(CompiledAudioGraph::default())),
            aux_scratch: StereoBus::with_capacity(MAX_BLOCK_SIZE),
            events: Vec::with_capacity(MAX_CHANNELS),
            // Small enough that reserving the addressable length outright
            // costs 433 KiB and saves boxing every control-path access.
            modulation: (0..MAX_CHANNELS).map(|_| ModRack::default()).collect(),
            modulators: (0..MAX_CHANNELS).map(|_| ModulatorRack::new()).collect(),
            control_outputs: Vec::with_capacity(MAX_CHANNELS),
            gate_ticks: Box::new([[NoteGateEvents::default(); MAX_CHANNELS];
                MAX_CONTROL_TICKS_PER_BLOCK]),
            sample_rate,
            reclaim: Vec::new(),
            meters: BusMeters::new(),
            device_meters: DeviceMeters::new(),
            device_telemetry: DeviceTelemetry::new(),
            buffer_midi: Arc::new(ArcSwapOption::empty()),
            buffer_cc: BufferCcState::default(),
            playhead_meters: PlayheadMeters::new(),
            modulator_meters: ModulatorMeters::new(),
            auditions: [None; MAX_AUDITIONS_PER_BLOCK],
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
        let master = &mut self.buses[MASTER_BUS as usize];
        // The preview writes into the master after the bus walk has already
        // decided whether to empty it, so it has to say that it did: a master
        // left holding the last frames of a retired preview would keep
        // playing them for as long as nothing else routed to it.
        master.dirty = true;
        let bus = &mut master.bus;
        for index in 0..count {
            let frame = samples[start + index];
            bus.l[index] += frame[0] * gain;
            bus.r[index] += frame[1] * gain;
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
                    // A committed stretch is baked here too, from the same
                    // spec the editor uses. `samples` carries sources -- that
                    // is what a project's assets are -- so without this an
                    // export would play the unstretched original while the
                    // app plays the render.
                    let sample = sample.map(|sample| {
                        match project
                            .channels
                            .get(index)
                            .and_then(|channel| channel.setup.source.sampler_state())
                            .and_then(|state| state.commit.as_ref())
                            .and_then(|commit| {
                                mooloop_dsp::commit::rerender_commit(&sample, commit)
                            }) {
                            Some(rendered) => rendered,
                            None => sample,
                        }
                    });
                    Arc::new(ArcSwapOption::from(sample))
                })
                .collect(),
        );
        // Slice maps travel with the project, not with `samples`. Omitting
        // them made every note in a sliced channel resolve out of range, so
        // an exported mix was silent exactly where the app was not.
        let slice_slots: Arc<Vec<Arc<ArcSwapOption<mooloop_core::SliceMap>>>> = Arc::new(
            (0..MAX_CHANNELS)
                .map(|index| {
                    let slices = project
                        .channels
                        .get(index)
                        .and_then(|channel| channel.setup.source.sampler_state())
                        .map(|state| state.slices.clone())
                        .filter(|slices| !slices.is_empty())
                        .map(Arc::new);
                    Arc::new(ArcSwapOption::from(slices))
                })
                .collect(),
        );
        let mut state = Self::new(sample_rate, slots, slice_slots);
        state.load_project(project);
        state
    }

    /// Build one channel's storage. Allocates, so it belongs on the control
    /// thread — either here during a project install, or in the `AddChannel`
    /// structural command that carries the result across.
    pub(crate) fn build_channel(
        sample_slot: Arc<ArcSwapOption<SampleData>>,
        slice_slot: Arc<ArcSwapOption<SliceMap>>,
        sample_rate: u32,
    ) -> Box<ChannelStorage> {
        Box::new(ChannelStorage {
            strip: Box::new(ChannelStrip::new(sample_slot, slice_slot, sample_rate)),
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
            let slices = self.slice_slots[self.strips.len()].clone();
            self.push_channel(Self::build_channel(slot, slices, sample_rate));
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
        self.install_compensation(project);
        // Here as well as through the session's incremental sync, and for the
        // same reason `install_compensation` is: an offline render builds its
        // own `RenderState` and never runs a pump, so without this an export
        // would be the one place the channels rendered in index order.
        *self.audio = AudioTapBank::new(project.audio_graph());
    }

    /// Build and install the tree's latency compensation from `project`.
    ///
    /// Here as well as through the session's incremental sync, because this is
    /// the path an **offline render** takes: it builds its own `RenderState`
    /// and never runs a pump, so without this an export would be the one
    /// place the mixer was not time aligned — which is exactly the disagreement
    /// between offline and live that everything else in this engine is
    /// arranged to prevent.
    ///
    /// Allocates, and is allowed to: `load_project` runs on the control thread
    /// while a state is being prepared, never from the callback.
    fn install_compensation(&mut self, project: &Project) {
        let mut channel_latency = [0u32; MAX_CHANNELS];
        let mut channel_bus = [MASTER_BUS; MAX_CHANNELS];
        for (index, channel) in project.channels.iter().take(MAX_CHANNELS).enumerate() {
            channel_latency[index] = chain_latency(&channel.setup.effects);
            channel_bus[index] = channel.setup.channel.bus;
        }
        let mut bus_latency = [0u32; MAX_BUSES];
        for (index, bus) in project.buses.iter().take(MAX_BUSES).enumerate() {
            bus_latency[index] = chain_latency(&bus.effects);
        }
        let plan = compile_latency(
            &self.bus_graph,
            &channel_latency,
            &channel_bus,
            &bus_latency,
        );
        for (index, strip) in self.strips.iter_mut().enumerate() {
            strip.compensation = IntegerDelay::new(plan.channel(index)).map(Box::new);
        }
        for (index, strip) in self.buses.iter_mut().enumerate() {
            strip.compensation = IntegerDelay::new(plan.bus(index)).map(Box::new);
        }
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

    /// Drop every route and every lane driving `device` in `target`, because
    /// that device has just been removed.
    ///
    /// All that is left of what used to run on every chain edit. A reorder is
    /// no longer an addressing event on either side: the devices move with
    /// their base values, event queues and host controls, and the addresses
    /// naming them were never positions to begin with.
    ///
    /// A channel's routes can only address that channel, so a channel edit
    /// touches one rack. A bus chain can be addressed from any channel's
    /// clip, so a bus edit walks them all -- a few thousand comparisons, on a
    /// gesture that happens by hand.
    fn forget_device(&mut self, target: EffectTarget, device: mooloop_core::DeviceId) {
        if !device.is_assigned() {
            return;
        }
        match target {
            EffectTarget::Channel(channel) => {
                if let Some(rack) = self.modulation.get_mut(channel as usize) {
                    rack.forget_device(target, device);
                }
            }
            EffectTarget::Bus(_) => {
                for rack in self.modulation.iter_mut() {
                    rack.forget_device(target, device);
                }
            }
        }
        self.sequencer.forget_device(target, device);
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
            ParamOwner::Effect { device } => {
                let Some(chain) = self.chain_mut(destination.scope) else {
                    return;
                };
                let Some(slot) = chain.slot_of(device) else {
                    return;
                };
                if let Some(base) = chain.base_param(slot, destination.param) {
                    chain.queue_param(slot, destination.param, base);
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
                strip.push_source_base();
            }
            // A route amount lives in the same parameter block as the rest of
            // the patch, so the base push that restores a generator knob
            // restores this too.
            ParamOwner::SourceRoute { .. } => {
                let EffectTarget::Channel(channel) = destination.scope else {
                    return;
                };
                let Some(strip) = self.strips.get_mut(channel as usize) else {
                    return;
                };
                strip.push_source_base();
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
        let Some(state) = self.chain(target).and_then(|chain| chain.slot(slot as usize)) else {
            return false;
        };
        let device = state.device;
        let Some(descriptor) = state.kind.and_then(|kind| kind.descriptor(id)) else {
            return false;
        };
        let policy = ModDestinationDescriptor::for_param(descriptor);
        self.modulation
            .get(channel as usize)
            .is_some_and(|rack| rack.modulates(ParamAddr::effect(target, device, id), &policy))
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
    ///
    /// Takes the two tables it advances rather than `&mut self`, because the
    /// gate table it reads is now a field too and only field-level borrows
    /// can see that the three are disjoint.
    fn tick_channel_modulators(
        modulators: &mut [ModulatorRack],
        control_outputs: &mut [Box<ControlOutputs>],
        sample_rate: u32,
        bpm: f64,
        channel: usize,
        frames: usize,
        gate_ticks: &GateTable,
    ) -> usize {
        let Some(runtime) = modulators.get_mut(channel) else {
            return 0;
        };
        let Some(outputs) = control_outputs.get_mut(channel) else {
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

    /// The block path's call to [`Self::tick_channel_modulators`], reading the
    /// gate table the tests set up on the state itself. Only the borrow
    /// splitting differs; the arguments are the ones `process_block_inner`
    /// passes.
    #[cfg(test)]
    fn tick_modulators_from_gate_table(&mut self, channel: usize, frames: usize) -> usize {
        Self::tick_channel_modulators(
            &mut self.modulators,
            &mut self.control_outputs,
            self.sample_rate,
            self.transport.bpm,
            channel,
            frames,
            &self.gate_ticks,
        )
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
            StructuralCommand::SetContainerSpan {
                target,
                slot,
                children,
                align,
                scratch,
            } => {
                let Some(chain) = Self::chain_for(&mut self.strips, &mut self.buses, target)
                else {
                    return Some(StructuralReclaim::Container { align, scratch });
                };
                // A chain keeps the first scratch it is given and hands every
                // later one straight back, the way the graph keeps the first
                // storage a channel index is ever given.
                let scratch = match (chain.container_dry.is_some(), scratch) {
                    (false, Some(scratch)) => {
                        chain.container_dry = Some(scratch);
                        None
                    }
                    (_, scratch) => scratch,
                };
                let displaced = chain.slot_mut(slot as usize).and_then(|state| {
                    state.container_children = children;
                    std::mem::replace(&mut state.container_align, align)
                });
                (displaced.is_some() || scratch.is_some()).then_some(
                    StructuralReclaim::Container {
                        align: displaced,
                        scratch,
                    },
                )
            }
            StructuralCommand::RemoveEffect { target, slot } => {
                let chain = Self::chain_for(&mut self.strips, &mut self.buses, target);
                // Read the departing device's identity before it goes, since
                // it is what the routes and lanes that drove it are named by.
                let device = chain
                    .as_ref()
                    .and_then(|chain| chain.slot(slot as usize))
                    .map_or(mooloop_core::DeviceId::UNASSIGNED, |state| state.device);
                let displaced = chain.map(|chain| chain.remove(slot as usize));
                // The routes and lanes that drove the departed device go with
                // it. Nothing else has to be told: what closed up behind it
                // was positions, and no address is one.
                self.forget_device(target, device);
                displaced
                    .filter(|displaced| !displaced.is_empty())
                    .map(StructuralReclaim::Effect)
            }
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
            StructuralCommand::SetCompensation { target, delay } => {
                let slot = match target {
                    EffectTarget::Channel(channel) => self
                        .strips
                        .get_mut(channel as usize)
                        .map(|strip| &mut strip.compensation),
                    EffectTarget::Bus(bus) => self
                        .buses
                        .get_mut(bus as usize)
                        .map(|strip| &mut strip.compensation),
                };
                let Some(slot) = slot else {
                    // Nothing to install into. Hand the ring straight back
                    // rather than dropping it here: this is the realtime
                    // thread, and an unaddressable producer is not a reason to
                    // free memory on it.
                    return delay.map(StructuralReclaim::Compensation);
                };
                std::mem::replace(slot, delay).map(StructuralReclaim::Compensation)
            }
            StructuralCommand::SetAudioGraph { bank } => {
                // One swap: the executor never sees an edge without its
                // schedule or a schedule against another generation's
                // buffers.
                Some(StructuralReclaim::AudioGraph(std::mem::replace(
                    &mut self.audio,
                    bank,
                )))
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
            EngineCommand::TriggerChannelNote {
                channel,
                note,
                velocity,
            } => self.queue_audition(
                channel,
                Event::NoteOn {
                    id: audition_note_id(note),
                    note,
                    velocity,
                },
            ),
            EngineCommand::ReleaseChannelNote { channel, note } => self.queue_audition(
                channel,
                Event::NoteOff {
                    id: audition_note_id(note),
                    note,
                },
            ),
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
                    strip.source_base = GeneratorParams::DrumSynth(params);
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
            EngineCommand::SetChannelGeneratorParam { channel, id, value } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    // The base is the authored value modulation offsets from,
                    // so this edits the base and lets the ordinary modulation
                    // pass re-derive what the node should hear. Writing the
                    // node directly here would be the same thing for an
                    // unmodulated parameter and would fight the rack for a
                    // modulated one.
                    if strip.source_base.set(id, value).is_some() {
                        strip.push_source_base();
                    }
                }
            }
            EngineCommand::SetSourceRoute { channel, route } => {
                // Structural, so it goes through the base and is pushed
                // whole: the node recompiles its flat table from the routes
                // it is handed.
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    let landed = strip
                        .source_base
                        .internal_routes_mut()
                        .is_some_and(|routes| routes.upsert(route));
                    if landed {
                        strip.push_source_base();
                    }
                }
            }
            EngineCommand::RemoveSourceRoute { channel, route } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    let removed = strip
                        .source_base
                        .internal_routes_mut()
                        .is_some_and(|routes| routes.remove(route));
                    if removed {
                        strip.push_source_base();
                    }
                }
            }
            EngineCommand::SetSourceRouteAmount {
                channel,
                route,
                amount,
            } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.set_source_route_amount(route, amount);
                }
            }
            EngineCommand::SetChannelPolySynthParams { channel, params } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.poly_synth.set_params(params);
                    strip.source_base = GeneratorParams::PolySynth(params);
                }
            }
            EngineCommand::MoveEffect { target, from, to } => {
                // The devices move; nothing else has to. This used to run a
                // permutation over every route in the rack and every lane in
                // every pattern.
                if let Some(chain) = self.chain_mut(target) {
                    chain.move_slot(from as usize, to as usize);
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

    /// Hold an auditioned note until the block's event lists exist.
    ///
    /// Silently dropped past the cap: sixteen notes inside one block is a
    /// stuck key, and refusing the seventeenth is better than growing a
    /// buffer on the audio thread.
    fn queue_audition(&mut self, channel: u8, event: Event) {
        if let Some(slot) = self.auditions.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(Audition { channel, event });
        }
    }

    /// Dispatch this block's auditions into the channels' event lists.
    ///
    /// At offset zero, and after the sequencer has scheduled: an audition is
    /// a live gesture that already happened, so it belongs at the top of the
    /// block rather than somewhere inside it. They go in whether or not the
    /// transport is running, which is the point -- auditioning a slice must
    /// not require pressing play.
    fn dispatch_auditions(&mut self) {
        for slot in self.auditions.iter_mut() {
            let Some(audition) = slot.take() else {
                continue;
            };
            if let Some(events) = self.events.get_mut(audition.channel as usize) {
                let _ = events.push_ordered(TimedEvent {
                    offset: 0,
                    event: audition.event,
                });
            }
        }
    }

    pub fn process_block(&mut self, frames: usize) -> RenderReport {
        self.process_block_inner(frames, true)
    }

    pub fn process_once_block(&mut self, frames: usize) -> RenderReport {
        self.process_block_inner(frames, false)
    }

    fn process_block_inner(&mut self, frames: usize, looping: bool) -> RenderReport {
        let frames = frames.min(MAX_BLOCK_SIZE);
        let skip_idle = self.skip_idle;
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
        self.dispatch_auditions();

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
        // The gate table is sized for the largest block the engine accepts,
        // which is 8192 frames and so 256 control ticks of 256 channels. That
        // is 192 KB, and it used to be a local: every block began by zeroing
        // all of it and then writing four rows. Kept here instead, only the
        // rows this block will read are cleared, and the audio thread stops
        // wiping an L2's worth of cache before it renders anything.
        let control_ticks = frames.div_ceil(CONTROL_RATE_FRAMES);
        for row in self.gate_ticks.iter_mut().take(control_ticks) {
            *row = [NoteGateEvents::default(); MAX_CHANNELS];
        }
        // A zero-frame block has no subdivision to file a gate under, and no
        // cleared row to file it in either; nothing will read the table.
        let gated_channels = if control_ticks == 0 { 0 } else { active_channels };
        // Not an iterator loop: `gate_ticks` is indexed by control tick first
        // and by channel second, so the loop variable is not this array's
        // outer index and enumerating it would walk the wrong axis.
        #[allow(clippy::needless_range_loop)]
        for source_channel in 0..gated_channels {
            for event in self.events[source_channel].iter() {
                // Clamped into the cleared region rather than into the array:
                // an offset past the end of the block is already nonsense,
                // and the rows beyond this block's own are stale.
                let tick = (event.offset as usize / CONTROL_RATE_FRAMES)
                    .min(control_ticks.saturating_sub(1));
                let gate = &mut self.gate_ticks[tick][source_channel];
                match event.event {
                    Event::NoteOn { .. } => gate.note_ons = gate.note_ons.saturating_add(1),
                    Event::NoteOff { .. } => gate.note_offs = gate.note_offs.saturating_add(1),
                    Event::Choke => gate.choke = true,
                    _ => {}
                }
            }
        }
        // Field-by-field rather than through `self`, so the gate table stays
        // borrowable while the racks it feeds are advanced.
        for (index, ticks) in modulator_ticks.iter_mut().enumerate().take(active_channels) {
            *ticks = Self::tick_channel_modulators(
                &mut self.modulators,
                &mut self.control_outputs,
                self.sample_rate,
                self.transport.bpm,
                index,
                frames,
                &self.gate_ticks,
            );
        }
        // Lanes resolve whether or not the transport is running: stopped, the
        // playhead simply holds still and the destination sits at the value
        // drawn under it. Making automation conditional on playback would mean
        // a knob that jumps the moment you press play.
        //
        // It *is* conditional on a lane existing, which is a different claim
        // and costs nothing to make: with none under the playhead every
        // `curve_for` below can only answer `None`, and each of those answers
        // is a walk over every active channel. Asking once here instead of
        // once per descriptor per channel is the whole of the saving.
        let automation = (frames > 0 && self.sequencer.has_automation_at(start_tick)).then(|| {
            AutomationBlock {
                sequencer: &self.sequencer,
                start_tick,
                ticks_per_sample,
                ticks: frames.div_ceil(CONTROL_RATE_FRAMES),
            }
        });
        // Only the buses that may hold something. A song uses one or two of
        // the seventeen every project carries, and emptying a buffer that is
        // already zero is the largest single line in an empty block.
        for strip in &mut self.buses {
            if strip.dirty {
                strip.bus.clear(frames);
                strip.dirty = false;
            }
        }
        // Emptied before anything renders, so a producer that stopped playing
        // -- or a channel that stopped existing -- publishes silence rather
        // than the block before.
        self.audio.clear(frames);
        // The compiled schedule, not index order: a producer has to render
        // before the consumer that reads it, in the same block, because a
        // block-sized delay is a delay whose length is the host's buffer
        // size. `order()` is the identity permutation on a project with no
        // subscriptions, which is what makes the edge inaudible until one is
        // authored.
        //
        // The modulator tick pass above stays in index order on purpose. A
        // modulator's phase must not depend on a subscription somebody made
        // on another channel, so the two passes are separate and only this
        // one is scheduled.
        for slot in 0..MAX_CHANNELS {
            let index = self.audio.graph.order()[slot] as usize;
            if index >= active_channels {
                continue;
            }
            let ticks = modulator_ticks[index];
            // Published before the mute check: a muted channel's modulators
            // still run, so its knobs should still animate rather than freeze
            // on whatever the last audible block left behind.
            // The generator's outlets, as published at the end of the block
            // before this one. Taken before the strip renders, so nothing in
            // this block can read its own publication and the one declared
            // block of latency is a fact about the order rather than a rule.
            let outlets = self.strips[index].published_outlets;
            if ticks > 0 {
                let mut row = [0.0; CONTROL_SOURCE_SLOTS];
                row[..MAX_MODULATORS_PER_CHANNEL]
                    .copy_from_slice(&self.control_outputs[index][ticks - 1]);
                row[MAX_MODULATORS_PER_CHANNEL..].copy_from_slice(&outlets);
                // The whole row, so the view can resolve a knob driven by an
                // outlet as well as one driven by a module. One snapshot a
                // block, unlike the per-tick table this is taken from.
                self.modulator_meters.publish(index, &row);
            }
            let muted = self.strips[index].output.muted;
            // A muted producer that nobody reads still skips, which is what
            // keeps mute a way of not spending the work. One that somebody
            // reads renders its generator and stops there: mute is an
            // output-stage decision about what reaches the bus, and a
            // pre-level tap is exactly the signal a source muted in its own
            // mix still has.
            if muted && !self.audio.produces(index) {
                // A muted channel renders nothing, so its compensation ring
                // would still be holding the audio from before the mute and
                // would emit it on unmute. Emptying it is fifteen writes, and
                // it is the honest state: a silent producer's pipeline is
                // silent too.
                if let Some(delay) = self.strips[index].compensation.as_mut() {
                    delay.reset();
                }
                // Nothing measured this block, so nothing may be concluded
                // from it: unmuting always renders at least one block before
                // the channel is allowed to decide it is idle.
                self.strips[index].source_silent_frames = 0;
                continue;
            }
            let modulation = ModulationBlock {
                rack: &self.modulation[index],
                outputs: &self.control_outputs[index],
                outlets: &outlets,
                ticks,
            };
            // The generator's control events go into the channel's own note
            // list, which is the event stream it already splits its block on.
            // Written inline rather than as a method because the automation
            // block holds `&self.sequencer` for the whole loop, and only the
            // compiler's field-level borrow splitting can see that
            // `self.events` and `self.strips` are disjoint from it.
            // Neither pass below can produce an event without either a route
            // in this channel's rack or a lane under the playhead, and both
            // questions are settled for the whole channel before either loop
            // starts. Asked per descriptor instead, a device the size of
            // ML-P8 pays two hundred route-table walks a block to be told
            // what one walk already said.
            if modulation.rack.has_routes() || automation.is_some() {
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
                                modulation.sources(tick),
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

                // The generator's own internal routes, whose amounts are
                // automatable but are not entries in the table above: they
                // are addressed by the route's durable id, so they resolve
                // through the same base-plus-offset pass and leave through
                // their own event.
                let internal: Option<mooloop_core::MlP8Routes> =
                    base.internal_routes().copied();
                for route in internal.iter().flat_map(|routes| routes.iter()) {
                    for descriptor in base.kind().route_descriptors() {
                        let destination = ParamAddr {
                            scope,
                            owner: ParamOwner::SourceRoute { route: route.id },
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
                        let knob_normalized = descriptor.to_normalized(route.amount);
                        for tick in 0..ticks.max(automation.as_ref().map_or(0, |a| a.ticks)) {
                            let base_normalized = curve
                                .as_ref()
                                .zip(automation.as_ref())
                                .and_then(|(curve, automation)| automation.value_at(curve, tick))
                                .unwrap_or(knob_normalized);
                            let offset_normalized = if modulated {
                                modulation.rack.offset_for(
                                    destination,
                                    modulation.sources(tick),
                                    &policy,
                                )
                            } else {
                                0.0
                            };
                            let amount = descriptor.from_normalized(
                                (base_normalized + offset_normalized).clamp(0.0, 1.0),
                            );
                            let _ = self.events[index].push_ordered(TimedEvent {
                                offset: (tick * CONTROL_RATE_FRAMES) as u32,
                                event: Event::SourceRouteAmount {
                                    route: route.id,
                                    amount,
                                },
                            });
                        }
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
            // A channel with nothing to answer, nothing sounding, and nothing
            // still decaying in its chain is not rendered at all. On a
            // thirty-two channel arrangement with four things playing, that
            // is twenty-eight generators, twenty-eight effect chains and
            // twenty-eight pan stages that do not run.
            //
            // The event list has to be *empty*, not merely free of notes. A
            // modulated or automated source parameter resolves into
            // `ParamValue` events just above, and a generator splits its
            // block at every event it is given -- so a strip that slept
            // through them would advance its free-running state in one stride
            // where a running one took several, and the two would not agree
            // to the bit. A channel whose source is being driven therefore
            // keeps rendering, which is also the honest reading: something is
            // still moving in it.
            if skip_idle && self.events[index].is_empty() && self.strips[index].is_idle() {
                self.strips[index].sleep(&context);
                self.slept_strip_blocks += 1;
                // Positions are stored rather than peak-held, so unlike the
                // meters they do have to be written: otherwise the last
                // sounding voice's playhead stays pinned in the UI.
                self.playhead_meters
                    .publish(index, &[0.0; MAX_SAMPLER_VOICES as usize]);
                continue;
            }
            self.strips[index].sleeping = false;
            // The edge this channel reads, taken before the port group so the
            // bank is borrowed once each way rather than both at once.
            let source = self.audio.source(index).is_some_and(|tap| {
                self.aux_scratch.l[..frames].copy_from_slice(&tap.l[..frames]);
                self.aux_scratch.r[..frames].copy_from_slice(&tap.r[..frames]);
                true
            });
            // Scoped, because the port group holds the bank borrowed and the
            // mute check below needs it back.
            {
                let published = self.strips[index].active_source.outlets();
                let mut ports = self.audio.ports(index, published);
                let strip = &mut self.strips[index];
                strip.bus.clear(frames);
                strip.process(
                    &context,
                    &self.events[index],
                    source.then_some(&self.aux_scratch),
                    &mut ports,
                );
            }
            if muted {
                // Its tap is filled and its bus is not read: a muted producer
                // publishes, and reaches nothing else.
                //
                // It also publishes its *control* outlets, where a muted
                // channel nobody reads freezes them at the last audible
                // block. That is a difference between two muted channels, and
                // the moving one is the honest answer: a muted channel's
                // modulators already keep running so its knobs keep animating,
                // and a device that has stopped sounding should publish a
                // decayed envelope rather than the one it had when it was
                // silenced.
                if let Some(delay) = self.strips[index].compensation.as_mut() {
                    delay.reset();
                }
                self.strips[index].source_silent_frames = 0;
                continue;
            }
            let strip = &mut self.strips[index];
            let source_peak = strip.bus.peak(frames);
            strip.note_source_level(source_peak.0.max(source_peak.1), frames);
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
                skip_idle,
            );
            strip
                .output
                .apply_pan_segments(&mut strip.bus, frames, strip_segments.as_ref());
            // Wait, if this channel is shorter than something else feeding the
            // same bus. Last, so what waits is the finished channel, and
            // immediately before the sum it is being aligned for.
            if let Some(delay) = strip.compensation.as_mut() {
                delay.process(&mut strip.bus.l[..frames], &mut strip.bus.r[..frames]);
            }
            if let Some(destination) = self.buses.get_mut(strip.destination as usize) {
                destination.bus.add_from(&strip.bus, frames);
                destination.dirty = true;
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
            // Everything feeding this bus has already run -- that is what the
            // compiled order guarantees -- so `dirty` is the settled answer to
            // whether anything reached it this block, and costs no pass over
            // the buffer to ask.
            if strip.dirty {
                strip.silent_frames = 0;
            } else {
                strip.silent_frames = strip.silent_frames.saturating_add(frames as u32);
            }
            // A bus nobody routed to, whose chain has finished and whose
            // compensation ring holds only the silence it has been fed, has
            // nothing to contribute. Every project carries all seventeen and
            // a song uses one or two, so this is most of what an empty block
            // was spending: sixteen buffers emptied, peaked twice, balanced
            // and metered to say nothing.
            if skip_idle && !strip.dirty && strip.is_resting() {
                strip.effects.sleep(&context);
                if !strip.sleeping {
                    strip.sleeping = true;
                    // The whole buffer, once, rather than this block's worth.
                    // A later block may be longer than the one that emptied
                    // it, and would then read past what was cleared into
                    // audio from before the silence.
                    let capacity = strip.bus.capacity();
                    strip.bus.clear(capacity);
                }
                // Written rather than left alone: a meter that stops being
                // published holds its last value, and a silent bus reading
                // its last audible peak is exactly the stale needle the
                // playhead publish below the channel loop exists to avoid.
                self.device_meters
                    .publish_input(MAX_CHANNELS + index, 0, 0.0, 0.0);
                self.meters.publish(index, 0.0, 0.0);
                if index == MASTER_BUS as usize {
                    master_peak = (0.0, 0.0);
                }
                continue;
            }
            // Anything below may leave audio in the buffer -- a chain with a
            // tail writes into one nothing fed -- so the next block empties it.
            strip.dirty = true;
            strip.sleeping = false;
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
                skip_idle,
            );
            strip.output.apply_balance(&mut strip.bus, frames);
            // Before the meter and before the mute check on purpose: from here
            // the bus's audio genuinely *is* delayed, so metering the delayed
            // signal is honest, and a muted bus still advances its ring rather
            // than holding stale audio to emit when it is unmuted.
            if let Some(delay) = strip.compensation.as_mut() {
                delay.process(&mut strip.bus.l[..frames], &mut strip.bus.r[..frames]);
            }
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

    /// Turn skipping idle devices and idle channels off, or back on.
    ///
    /// Not a user setting and not exposed as a command: it exists so a render
    /// can be run twice and the two compared sample for sample. A mechanism
    /// whose whole claim is that it changes nothing has to be checkable
    /// against the thing it claims not to change, and this is what makes that
    /// check a test rather than an argument.
    #[cfg(test)]
    pub fn set_idle_skipping(&mut self, enabled: bool) {
        self.skip_idle = enabled;
    }

    /// Channel-blocks skipped since this state was built. See the field.
    #[cfg(test)]
    pub fn slept_strip_blocks(&self) -> u64 {
        self.slept_strip_blocks
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
        ChannelStrip::new(slot, Arc::new(ArcSwapOption::empty()), 48_000)
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

    /// Auditioning has to work with the transport stopped -- that is the
    /// whole gesture -- and the note has to go through the channel's own
    /// device, not past it, or a slice would be auditioned without the
    /// envelopes, filter and drive it will actually play through.
    #[test]
    fn an_auditioned_note_sounds_a_stopped_channel_through_its_own_device() {
        // A full second, so the note outlasts the release window the check
        // below is looking through rather than simply running out of audio.
        let sample = Arc::new(SampleData {
            frames: vec![[0.5, -0.5]; 48_000],
            sample_rate: 48_000,
            root_note: 60,
        });
        let slots: Arc<Vec<Arc<ArcSwapOption<SampleData>>>> = Arc::new(
            (0..MAX_CHANNELS)
                .map(|_| Arc::new(ArcSwapOption::empty()))
                .collect(),
        );
        slots[0].store(Some(sample));
        let slice_slots = Arc::new(
            (0..MAX_CHANNELS)
                .map(|_| Arc::new(ArcSwapOption::empty()))
                .collect(),
        );
        let mut render = RenderState::new(48_000, slots, slice_slots);
        render.load_project(&Project::default());
        assert!(!render.transport.playing);

        // Stopped and untouched, the channel is silent.
        render.process_block(128);
        assert!(render.strips[0].sampler.voice_positions()[0].is_nan());

        render.apply_command(EngineCommand::TriggerChannelNote {
            channel: 0,
            note: 60,
            velocity: 127,
        });
        let opening = render.process_block(128);
        assert!(
            !render.strips[0].sampler.voice_positions()[0].is_nan(),
            "the audition should have started a voice on the channel's sampler"
        );
        assert!(opening.peak_l > 0.01, "the audition made no sound");

        // And it has to keep sounding at level. A stopped transport used to
        // release every voice on every block, which put an audition into
        // release one block after it began. Measured as level rather than as
        // playhead position on purpose: a releasing voice is still an active
        // voice and its head keeps advancing, so a position check passes
        // straight through the bug it is meant to catch.
        //
        // Fifty blocks is 133 ms, comfortably past the 50 ms default release.
        let mut peak = opening.peak_l;
        for block in 0..50 {
            let report = render.process_block(128);
            assert!(
                report.peak_l > opening.peak_l * 0.9,
                "the auditioned note decayed by block {block}: {} vs {}",
                report.peak_l,
                opening.peak_l
            );
            peak = report.peak_l;
        }
        assert!(peak > 0.01);

        // An audition is consumed by the block it arrives in, not replayed.
        assert!(render.auditions.iter().all(Option::is_none));

        // Pressing stop still cuts sounding voices: the release moved to the
        // transition, it did not go away. The default release is 50 ms, so
        // give it comfortably longer than that.
        render.transport.play();
        render.process_block(128);
        render.transport.stop();
        for _ in 0..40 {
            render.process_block(128);
        }
        assert!(
            render.strips[0].sampler.voice_positions()[0].is_nan(),
            "stopping the transport must still release what is sounding"
        );
    }

    #[test]
    fn preview_voice_plays_replaces_and_retires() {
        let slots = Arc::new(
            (0..MAX_CHANNELS)
                .map(|_| Arc::new(ArcSwapOption::empty()))
                .collect(),
        );
        let slice_slots = Arc::new(
            (0..MAX_CHANNELS)
                .map(|_| Arc::new(ArcSwapOption::empty()))
                .collect(),
        );
        let mut render = RenderState::new(48_000, slots, slice_slots);
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
        let slice_slots = Arc::new(
            (0..MAX_CHANNELS)
                .map(|_| Arc::new(ArcSwapOption::empty()))
                .collect(),
        );
        let mut render = RenderState::new(48_000, slots, slice_slots);
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
        for _ in 0..MAX_EFFECTS_PER_CHANNEL {
            project.channels[0]
                .setup
                .push_effect(mooloop_core::EffectSlotState::of_kind(
                    mooloop_core::EffectKind::Filter,
                ));
        }

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
    /// stopped (`docs/plans/archive/modulator-capacity/`) -- the graph now builds
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

    /// The offline renderer builds its own slots from the project, so it is a
    /// second place a channel's audio is assembled. Slice maps travel with the
    /// project rather than with `samples`, and leaving them out made every
    /// note in a sliced channel resolve out of range -- an exported mix silent
    /// exactly where the app was not. A committed stretch has the mirror
    /// problem: `samples` carries sources, so the export would play the
    /// unstretched original.
    #[test]
    fn an_offline_render_gets_the_slice_map_and_the_committed_buffer() {
        let sample = Arc::new(SampleData {
            frames: (0..8_000).map(|_| [0.5, -0.5]).collect(),
            sample_rate: 48_000,
            root_note: 60,
        });
        let mut channel = ProjectChannel::sampler(0, 1);
        {
            let sampler = channel.setup.sampler_state_mut().unwrap();
            sampler.params.play_mode = mooloop_core::PlayMode::Slice;
            sampler.params.attack = 0.0;
            sampler.slices.divide_evenly(4, 0, 8_000);
        }
        // The third slice, so a map that failed to arrive cannot be mistaken
        // for one that did.
        let note = mooloop_core::DEFAULT_SLICE_BASE_NOTE + 2;
        channel.notes[0].push(NoteEvent::new(1, 0, 96, note, 127));
        let project = Project {
            channels: vec![channel],
            ..Project::default()
        };

        let samples = vec![Some(sample.clone())];
        let mut render = RenderState::from_project(48_000, &project, &samples);
        render.play();
        let report = render.process_block(512);
        assert!(
            report.peak_l > 0.001,
            "a sliced channel rendered silent offline: the map never arrived"
        );

        // And a commit is baked here too, so the exported length is the
        // stretched one rather than the source's.
        let mut committed = project.clone();
        {
            let sampler = committed.channels[0].setup.sampler_state_mut().unwrap();
            sampler.params.play_mode = mooloop_core::PlayMode::Pitched;
            sampler.commit = Some(Box::new(mooloop_core::SampleCommit {
                mode: mooloop_core::StretchMode::Music,
                ratio: 2.0,
                grain: 1024,
                source_markers: Vec::new(),
                source_start: 0.0,
                source_end: 1.0,
                source_loop_start: 0.0,
                source_loop_end: 1.0,
            }));
            sampler.slices = mooloop_core::SliceMap::default();
        }
        let render = RenderState::from_project(48_000, &committed, &samples);
        let published = render.sample_slots[0]
            .load_full()
            .expect("the channel should have audio");
        assert_eq!(
            published.frames.len(),
            16_000,
            "the export played the source rather than the committed render"
        );
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
        let storage = RenderState::build_channel(
            Arc::new(ArcSwapOption::from(None)),
            Arc::new(ArcSwapOption::from(None)),
            48_000,
        );
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
            outlets: &[0.0; MAX_GENERATOR_OUTLETS],
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
            outlets: &[0.0; MAX_GENERATOR_OUTLETS],
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
        assert_eq!(render.tick_modulators_from_gate_table(0, 64), 2);
        assert_eq!(render.control_outputs[0][0][0], -1.0);
        assert_eq!(render.control_outputs[0][1][0], -0.5);

        // `process_block_inner` builds this fixed bitmap from scheduled
        // NoteOn offsets. The tick method applies it before sampling, so the
        // destination sees the reset phase on that subdivision rather than
        // one control tick later.
        render.gate_ticks[0][0].note_ons = 1;
        render.tick_modulators_from_gate_table(0, 32);
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
        render.gate_ticks[0][1].note_ons = 1;

        render.tick_modulators_from_gate_table(0, 32);
        assert_eq!(render.control_outputs[0][0][0], 1.0);
    }

    /// A stepped parameter refuses modulation, so a route aimed at one is
    /// inert -- and, just as importantly, does not suppress the knob. Without
    /// the policy check the engine would treat the destination as modulated,
    /// withhold the base write, and leave the mode stuck.
    #[test]
    fn a_route_on_a_stepped_parameter_neither_moves_nor_blocks_its_knob() {
        let mut channel = ProjectChannel::sampler(0, 1);
        let eq = channel
            .setup
            .push_effect(mooloop_core::EffectSlotState::of_kind(
                mooloop_core::EffectKind::Eq,
            ))
            .expect("pushed");
        channel.setup.modulation.install(0, mooloop_core::ModulatorParams::Lfo(
            mooloop_core::ModLfoParams {
                rate_hz: 375.0,
                ..mooloop_core::ModLfoParams::default()
            },
        ));
        let stepped = ParamAddr::effect(
            EffectTarget::Channel(0),
            eq,
            mooloop_core::EQ_PARAM_TARGET,
        );
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
            .push_effect(mooloop_core::EffectSlotState::filter(
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
                    mooloop_core::DeviceId(0),
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
            .push_effect(mooloop_core::EffectSlotState::filter(
                mooloop_core::FilterParams {
                    cutoff_hz,
                    resonance: 0.0,
                    mode: mooloop_core::FilterMode::LowPass,
                    ..mooloop_core::FilterParams::default()
                },
            ));
        channel
    }

    /// The filter added first by `filter_channel`, so its identity is the
    /// first one minted on that chain.
    const CUTOFF: ParamAddr = ParamAddr::effect(
        EffectTarget::Channel(0),
        mooloop_core::DeviceId(0),
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

    /// A generator outlet reaching another device's parameter: the whole
    /// point of step 06, and the thing that could not be authored at all
    /// before a route could name something that is not a rack module.
    ///
    /// `Gate` is used rather than an envelope because it is a step, which is
    /// what makes the *timing* legible: the block the note lands in must show
    /// the cutoff still at its base, and the block after it must show the
    /// gate. That one-block gap is not a scheduling accident to be tolerated
    /// — it is the declared contract from `MODULATOR_SYSTEM_SPEC.md`, and it
    /// is what makes an offline render agree with a live take.
    #[test]
    fn a_generator_outlet_drives_another_device_one_block_later() {
        use mooloop_core::mlp8::OUTLET_GATE;

        let mut channel = filter_channel(1_000.0);
        channel.setup.source = mooloop_core::ChannelSource::MlP8(mooloop_core::MlP8State::default());
        assert!(channel
            .setup
            .modulation
            .add_route(mooloop_core::ModRoute::from_outlet(
                OUTLET_GATE,
                CUTOFF,
                0.4,
                mooloop_core::ModPolarity::Bipolar,
            ))
            .is_some());

        let mut project = mooloop_core::Project {
            channels: vec![channel],
            ..mooloop_core::Project::default()
        };
        // One long note from the top of the pattern, so the gate goes high on
        // the first block and stays there.
        project.channels[0].notes[0].push(NoteEvent::new(1, 0, 384, 60, 127));

        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();

        // Block one: the note starts here, so the gate the control table is
        // holding is still the silence from before it.
        render.process_block(128);
        let first: Vec<f32> = cutoff_events(&render).iter().map(|(_, v)| *v).collect();
        // Not vacuous: a routed destination resolves every control tick, so
        // an empty list would mean the route was not running at all rather
        // than that it was running and reading silence.
        assert_eq!(first.len(), 4, "the route was not resolving: {first:?}");
        assert!(
            first.iter().all(|value| (value - 1_000.0).abs() < 1.0),
            "the outlet arrived in its own block: {first:?}"
        );

        // Block two: the gate published at the end of block one is what this
        // block's routes read.
        render.process_block(128);
        let second: Vec<f32> = cutoff_events(&render).iter().map(|(_, v)| *v).collect();
        assert_eq!(second.len(), 4, "the route stopped resolving: {second:?}");
        assert!(
            second.iter().all(|value| *value > 1_100.0),
            "the gate never reached the cutoff: {second:?}"
        );
    }

    /// The same contract from the other instrument, and the one DS-01's plan
    /// says is worth wanting soonest: a kick's `Trigger` reaching a later
    /// device without a sidechain graph.
    ///
    /// `Trigger` is the sharper timing test of the two. It is one publication
    /// wide, so three blocks tell the whole story -- base in the block the
    /// hit lands in, moved in the block after it, and back to base in the one
    /// after that. A trigger that leaked into a second block would be a
    /// device inventing a pulse width the table does not declare.
    #[test]
    fn a_ds01_trigger_drives_another_device_for_exactly_one_block() {
        use mooloop_core::ds01::DS01_OUTLET_TRIGGER;

        let mut channel = filter_channel(1_000.0);
        channel.setup.source = mooloop_core::ChannelSource::Ds01(mooloop_core::Ds01State::default());
        assert!(channel
            .setup
            .modulation
            .add_route(mooloop_core::ModRoute::from_outlet(
                DS01_OUTLET_TRIGGER,
                CUTOFF,
                0.4,
                // Bipolar is what passes a `0..1` outlet through unchanged;
                // `Session::arm_modulation_route` is where the two source
                // conventions are written down.
                mooloop_core::ModPolarity::Bipolar,
            ))
            .is_some());

        let mut project = mooloop_core::Project {
            channels: vec![channel],
            ..mooloop_core::Project::default()
        };
        // One hit at the top of the pattern. Its length does not matter: a
        // DS-01 one-shot ignores the note-off, and `Trigger` is about the
        // start either way.
        project.channels[0].notes[0].push(NoteEvent::new(1, 0, 24, 60, 127));

        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();

        let block = |render: &mut RenderState| {
            render.process_block(128);
            cutoff_events(render)
                .iter()
                .map(|(_, value)| *value)
                .collect::<Vec<f32>>()
        };

        let first = block(&mut render);
        assert_eq!(first.len(), 4, "the route was not resolving: {first:?}");
        assert!(
            first.iter().all(|value| (value - 1_000.0).abs() < 1.0),
            "the trigger arrived in its own block: {first:?}"
        );

        let second = block(&mut render);
        assert_eq!(second.len(), 4, "the route stopped resolving: {second:?}");
        assert!(
            second.iter().all(|value| *value > 1_100.0),
            "the trigger never reached the cutoff: {second:?}"
        );

        let third = block(&mut render);
        assert_eq!(third.len(), 4, "the route stopped resolving: {third:?}");
        assert!(
            third.iter().all(|value| (value - 1_000.0).abs() < 1.0),
            "the trigger stayed high for a second block: {third:?}"
        );
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
            source: mooloop_core::ModSourceRef::Id(source),
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

    /// The defect this guards used to be that a route and a lane named their
    /// destination by *slot*, so reordering the chain left both pointing at
    /// the old number and the LFO on the filter's cutoff started driving
    /// whatever slid into that slot.
    ///
    /// It is now guarding something stronger and simpler: **the addresses do
    /// not change at all.** `CUTOFF` is built once, before the reorder, and
    /// still resolves afterwards -- there is no `moved` address, because
    /// there is nothing for the reorder to move.
    #[test]
    fn a_route_and_a_lane_follow_their_device_through_a_reorder_and_die_with_it() {
        let (project, _source) = lfo_on_cutoff(0.25);
        let mut render = RenderState::from_project(48_000, &project, &[]);
        let channel = EffectTarget::Channel(0);
        let _ = render.apply_structural(install_effect(
            channel,
            1,
            default_effect(mooloop_core::EffectKind::Drive),
        ));
        render.apply_command(EngineCommand::UpsertAutomationPoint {
            pattern: 0,
            channel: 0,
            target: CUTOFF,
            point: mooloop_core::AutomationPoint::new(1, 0, 0.5),
        });
        assert!(render.effect_is_modulated(channel, 0, mooloop_core::FILTER_PARAM_CUTOFF_HZ));
        assert!(render.sequencer.automation_lane_at(CUTOFF, 0.0).is_some());

        // Filter to the end of the chain: drive first, filter second.
        render.apply_command(EngineCommand::MoveEffect {
            target: channel,
            from: 0,
            to: 1,
        });
        assert!(
            !render.effect_is_modulated(channel, 0, mooloop_core::FILTER_PARAM_CUTOFF_HZ),
            "the drive inherited the filter's route"
        );
        assert!(
            render.effect_is_modulated(channel, 1, mooloop_core::FILTER_PARAM_CUTOFF_HZ),
            "the route did not follow the filter"
        );
        // The address built before the reorder is the address after it. This
        // is the assertion the slot scheme could not make: there, `CUTOFF`
        // would now name the drive and the lane would have been rewritten.
        assert!(
            render.sequencer.automation_lane_at(CUTOFF, 0.0).is_some(),
            "the lane stopped resolving even though its device is still there"
        );
        assert_eq!(
            render.strips[0].effects.slot(1).and_then(|slot| slot.kind),
            Some(mooloop_core::EffectKind::Filter)
        );

        // And the filter in its new slot is actually being driven: the scratch
        // list holds the last slot's events after a block, which is now the
        // filter's.
        render.play();
        render.process_block(128);
        let events = cutoff_events(&render);
        assert!(
            events.iter().any(|(_, value)| (value - 1_000.0).abs() > 100.0),
            "the filter in slot 1 never left its base: {events:?}"
        );

        // Removing the filter takes its route and its lane with it rather than
        // leaving either parked on an empty slot for the next device to inherit.
        let _ = render.apply_structural(StructuralCommand::RemoveEffect {
            target: channel,
            slot: 1,
        });
        assert_eq!(render.modulation[0].routes.iter().flatten().count(), 0);
        assert!(render.sequencer.automation_lane_at(CUTOFF, 0.0).is_none());
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
                source: mooloop_core::ModSourceRef::Id(mooloop_core::ModSourceId(7)),
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
            .push_effect(mooloop_core::EffectSlotState::new(
                mooloop_core::EffectParams::Buffer(mooloop_core::BufferParams {
                    bars: 1,
                    ..mooloop_core::BufferParams::default()
                }),
            ));
        let project = synth_project(channel);
        let target = ParamAddr::effect(
            EffectTarget::Channel(0),
            mooloop_core::DeviceId(0),
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

    // --- ML-P8 internal routes ------------------------------------------

    /// An ML-P8 channel with one internal route already authored, and the
    /// route's durable id.
    fn mlp8_project_with_route() -> (Project, u16) {
        let mut channel = ProjectChannel::mlp8(0, 1);
        let state = channel
            .setup
            .mlp8_state_mut()
            .expect("an ML-P8 channel has ML-P8 state");
        let id = state
            .params
            .routes
            .add(
                mooloop_core::MlP8ModSource::Lfo,
                mooloop_core::MlP8ModDest::Param {
                    id: mooloop_core::mlp8::PARAM_FILTER_CUTOFF,
                },
            )
            .expect("the route should be accepted");
        assert!(state.params.routes.set_amount(id, 40.0));
        (synth_project(channel), id)
    }

    fn route_amounts(render: &RenderState, route: u16) -> Vec<f32> {
        render.events[0]
            .iter()
            .filter_map(|event| match event.event {
                Event::SourceRouteAmount { route: id, amount } if id == route => Some(amount),
                _ => None,
            })
            .collect()
    }

    fn authored_amount(render: &RenderState, route: u16) -> f32 {
        render.strips[0]
            .source_base
            .internal_routes()
            .and_then(|routes| routes.get(route))
            .expect("the route should still be authored")
            .amount
    }

    /// A lane drawn on a route's amount resolves at the control rate through
    /// the ordinary event path, exactly like a lane on a knob -- but through
    /// the route's durable id rather than a parameter of the device.
    #[test]
    fn a_lane_drives_an_internal_route_amount() {
        let (project, route) = mlp8_project_with_route();
        let target = ParamAddr::source_route(
            EffectTarget::Channel(0),
            route,
            mooloop_core::MLP8_ROUTE_PARAM_AMOUNT,
        );
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

        let values = route_amounts(&render, route);
        assert_eq!(
            values.len(),
            4,
            "the lane should resolve once per control tick: {values:?}"
        );
        assert!(
            values.windows(2).all(|pair| pair[1] < pair[0]),
            "the ramp did not fall across the block: {values:?}"
        );
        // Full scale at the top of the lane and centre at the bottom, which
        // is the route amount's own -100..100 range rather than a unit one.
        assert!((values[0] - 100.0).abs() < 1.0, "{values:?}");
        // The authored depth is untouched: a lane supplies the base.
        assert_eq!(authored_amount(&render, route), 40.0);
    }

    /// A route with no lane and no rack route on it emits nothing at all, so
    /// an ordinary ML-P8 patch does not pay for the machinery.
    #[test]
    fn an_unautomated_route_amount_emits_no_events() {
        let (project, route) = mlp8_project_with_route();
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        render.process_block(128);
        assert!(route_amounts(&render, route).is_empty());
    }

    /// Moving a depth is not a structural edit. The engine writes it to the
    /// authored base and to the running node without pushing the whole
    /// parameter block, which is what would rebuild the compiled topology.
    #[test]
    fn setting_a_route_amount_keeps_the_authored_topology() {
        let (project, route) = mlp8_project_with_route();
        let mut render = RenderState::from_project(48_000, &project, &[]);
        let before = *render.strips[0]
            .source_base
            .internal_routes()
            .expect("ML-P8 has internal routes");

        render.apply_command(EngineCommand::SetSourceRouteAmount {
            channel: 0,
            route,
            amount: -80.0,
        });
        let after = render.strips[0]
            .source_base
            .internal_routes()
            .expect("ML-P8 has internal routes");
        assert!(
            before.same_topology(after),
            "moving a depth changed the topology"
        );
        assert_eq!(authored_amount(&render, route), -80.0);
    }

    /// Adding and removing a route is structural, and both directions land on
    /// the authored base so a save records what is being heard.
    #[test]
    fn a_route_can_be_added_and_removed_through_the_command_ring() {
        let project = synth_project(ProjectChannel::mlp8(0, 1));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        let route = mooloop_core::MlP8Route {
            id: 7,
            source: mooloop_core::MlP8ModSource::FilterEnv,
            dest: mooloop_core::MlP8ModDest::Param {
                id: mooloop_core::mlp8::PARAM_DRIVE,
            },
            amount: 55.0,
        };

        render.apply_command(EngineCommand::SetSourceRoute { channel: 0, route });
        assert_eq!(authored_amount(&render, 7), 55.0);

        // Repointing under the same id replaces rather than adds.
        render.apply_command(EngineCommand::SetSourceRoute {
            channel: 0,
            route: mooloop_core::MlP8Route {
                dest: mooloop_core::MlP8ModDest::VcaLevel,
                ..route
            },
        });
        let routes = render.strips[0]
            .source_base
            .internal_routes()
            .expect("ML-P8 has internal routes");
        assert_eq!(routes.len(), 1);
        assert_eq!(routes.get(7).unwrap().dest, mooloop_core::MlP8ModDest::VcaLevel);

        render.apply_command(EngineCommand::RemoveSourceRoute { channel: 0, route: 7 });
        assert!(render.strips[0]
            .source_base
            .internal_routes()
            .expect("ML-P8 has internal routes")
            .is_empty());
    }

    /// A generator that has no internal routes ignores the commands entirely
    /// rather than misapplying them.
    #[test]
    fn a_device_without_internal_routes_ignores_route_commands() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.apply_command(EngineCommand::SetSourceRoute {
            channel: 0,
            route: mooloop_core::MlP8Route {
                id: 1,
                source: mooloop_core::MlP8ModSource::Lfo,
                dest: mooloop_core::MlP8ModDest::VcaLevel,
                amount: 100.0,
            },
        });
        render.apply_command(EngineCommand::SetSourceRouteAmount {
            channel: 0,
            route: 1,
            amount: 100.0,
        });
        assert!(render.strips[0].source_base.internal_routes().is_none());
    }

    /// Removing a lane returns the route to its authored depth, which is the
    /// same promise a knob gets. Without it the device would keep whatever
    /// the lane last resolved.
    #[test]
    fn removing_a_route_lane_restores_the_authored_depth() {
        let (project, route) = mlp8_project_with_route();
        let target = ParamAddr::source_route(
            EffectTarget::Channel(0),
            route,
            mooloop_core::MLP8_ROUTE_PARAM_AMOUNT,
        );
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.apply_command(EngineCommand::UpsertAutomationPoint {
            pattern: 0,
            channel: 0,
            target,
            point: mooloop_core::AutomationPoint::new(1, 0, 1.0),
        });
        render.play();
        render.process_block(128);
        assert!(!route_amounts(&render, route).is_empty());

        render.apply_command(EngineCommand::RemoveAutomationLane {
            pattern: 0,
            channel: 0,
            target,
        });
        render.process_block(128);
        assert!(
            route_amounts(&render, route).is_empty(),
            "a removed lane kept driving the route"
        );
        assert_eq!(authored_amount(&render, route), 40.0);
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

    // --- Latency compensation ------------------------------------------

    /// A Drive that is a pure fifteen-frame delay and nothing else.
    ///
    /// At `mix: 0` the effect outputs its own aligned dry path, so the shaper
    /// contributes nothing audible and what is left is exactly the
    /// oversampler's latency. That makes it the cleanest possible probe: any
    /// difference these tests find is timing rather than timbre.
    fn transparent_drive() -> mooloop_core::EffectSlotState {
        mooloop_core::EffectSlotState::drive(mooloop_core::DriveParams {
            drive: 1.0,
            curve: mooloop_core::DriveCurve::Soft,
            tone: 0.0,
            mix: 0.0,
            output: 1.0,
        })
    }

    /// One drum channel that hits on the downbeat, optionally through the
    /// transparent Drive.
    fn hit_channel(index: usize, latent: bool) -> ProjectChannel {
        let mut channel = ProjectChannel::drum_synth(index, 1);
        if latent {
            channel.setup.push_effect(transparent_drive());
        }
        channel.notes[0].push(NoteEvent::new(index as u32 + 1, 0, 24, 60, 127));
        channel
    }

    fn render_master(project: &Project, frames: usize) -> Vec<f32> {
        let mut render = RenderState::from_project(48_000, project, &[]);
        render.play();
        render.process_block(frames);
        render.master().l[..frames].to_vec()
    }

    /// The headline case, and the one that is silently wrong without this
    /// plan: two channels hitting on the same tick, one of them through a
    /// device that costs fifteen frames. They must land in the same frame.
    ///
    /// Both assertions fail on `main`. The plain channel would start at frame
    /// zero while its neighbour started at fifteen, and the master would carry
    /// one copy of the hit followed by a second — which is comb filtering,
    /// worst exactly when the two channels are most alike.
    #[test]
    fn two_channels_of_different_depths_land_in_the_same_frame() {
        const FRAMES: usize = 512;
        let latency = mooloop_core::effect::OVERSAMPLER_LATENCY_FRAMES as usize;

        let together = render_master(
            &Project {
                channels: vec![hit_channel(0, true), hit_channel(1, false)],
                ..Project::default()
            },
            FRAMES,
        );
        // Nothing arrives early: the shorter channel waited for the longer.
        assert!(
            together[..latency].iter().all(|sample| *sample == 0.0),
            "the plain channel arrived {latency} frames early"
        );

        // And they are aligned *exactly*, not merely both late. One channel
        // alone is the longest path and is compensated by nothing, so it is
        // the reference; two identical channels summed on top of each other
        // must be it, doubled, sample for sample.
        let alone = render_master(
            &Project {
                channels: vec![hit_channel(0, true)],
                ..Project::default()
            },
            FRAMES,
        );
        for (frame, (summed, single)) in together.iter().zip(alone.iter()).enumerate() {
            assert!(
                (summed - single * 2.0).abs() < 1.0e-6,
                "frame {frame}: two aligned copies gave {summed}, one copy doubled is {}",
                single * 2.0
            );
        }
    }

    /// The same through a bus, which is the case a per-channel-only scheme
    /// gets wrong: the latency is on the *bus*, so what has to wait is the
    /// channel that does not go through it.
    #[test]
    fn a_channel_waits_for_a_latent_bus_beside_it() {
        const FRAMES: usize = 512;
        let latency = mooloop_core::effect::OVERSAMPLER_LATENCY_FRAMES as usize;

        let mut project = Project {
            channels: vec![hit_channel(0, false), hit_channel(1, false)],
            ..Project::default()
        };
        // Channel 0 through bus 1, which carries the cost; channel 1 straight
        // to the master with nothing.
        project.channels[0].setup.channel.bus = 1;
        project.buses[1].push_effect(transparent_drive());

        let master = render_master(&project, FRAMES);
        assert!(
            master[..latency].iter().all(|sample| *sample == 0.0),
            "the direct channel did not wait for the bus"
        );
    }

    /// Bypass keeps its latency, which is the convention every host follows
    /// and the reason is audible: a bypass that shortened the chain would move
    /// the channel in time, so A/B-ing an effect would also A/B the timing.
    ///
    /// This is also what makes the plan safe to compute from the *declared*
    /// chain — it sums bypassed slots too, so the two would disagree if the
    /// container let a bypassed node pass audio through untouched.
    #[test]
    fn bypassing_a_device_does_not_move_the_channel_in_time() {
        const FRAMES: usize = 512;
        let live = Project {
            channels: vec![hit_channel(0, true), hit_channel(1, false)],
            ..Project::default()
        };
        let mut bypassed = live.clone();
        bypassed.channels[0].setup.effects[0].bypassed = true;

        let live_master = render_master(&live, FRAMES);
        let bypassed_master = render_master(&bypassed, FRAMES);

        let onset = |samples: &[f32]| samples.iter().position(|sample| sample.abs() > 1.0e-9);
        assert_eq!(
            onset(&live_master),
            onset(&bypassed_master),
            "bypassing the device moved the channel in time"
        );
    }

    /// Compensation must not make the output depend on where the block
    /// boundaries fall. A ring advanced per block rather than per frame would
    /// pass every alignment test above and fail this one.
    #[test]
    fn compensation_renders_the_same_at_any_block_size() {
        const FRAMES: usize = 1_024;
        let project = Project {
            channels: vec![hit_channel(0, true), hit_channel(1, false)],
            ..Project::default()
        };
        let render_in_blocks = |block: usize| {
            let mut render = RenderState::from_project(48_000, &project, &[]);
            render.play();
            let mut out = Vec::with_capacity(FRAMES);
            while out.len() < FRAMES {
                render.process_block(block);
                out.extend_from_slice(&render.master().l[..block]);
            }
            out.truncate(FRAMES);
            out
        };
        assert_eq!(render_in_blocks(128), render_in_blocks(256));
        assert_eq!(render_in_blocks(128), render_in_blocks(64));
    }

    /// The offline renderer builds its own `RenderState`, so it is a second
    /// place the plan is compiled -- and the only one a listener never hears
    /// until the file is finished. An export that skipped `install_compensation`
    /// would sound right in the room and arrive misaligned on disk.
    ///
    /// So this renders the aligned pair both ways and compares the file
    /// against the live block path sample for sample. Float32 WAV, because
    /// the comparison has to be exact: PCM24 would quantize the difference
    /// this test exists to find.
    #[test]
    fn an_offline_render_compiles_the_same_compensation_as_a_live_one() {
        const FRAMES: usize = 16_384;
        let latency = mooloop_core::effect::OVERSAMPLER_LATENCY_FRAMES as usize;
        let project = Project {
            channels: vec![hit_channel(0, true), hit_channel(1, false)],
            ..Project::default()
        };

        let temp = tempfile::tempdir().expect("a temporary directory");
        let path = temp.path().join("compensated.wav");
        crate::offline::OfflineRenderer::render(
            &project,
            &[],
            48_000,
            &crate::offline::ExportSpec {
                path: path.clone(),
                scope: crate::offline::RenderScope::Pattern { index: 0 },
                tail_seconds: 0.0,
                format: crate::offline::ExportFormat::Wav(
                    crate::offline::WavEncoding::Float32,
                ),
            },
        )
        .expect("the project renders offline");

        // Interleaved stereo; the left channel is what the live tests read.
        let offline: Vec<f32> = hound::WavReader::open(&path)
            .expect("the export is readable")
            .samples::<f32>()
            .map(|sample| sample.expect("a decoded sample"))
            .step_by(2)
            .take(FRAMES)
            .collect();
        assert_eq!(offline.len(), FRAMES, "the export was shorter than expected");

        let mut live_state = RenderState::from_project(48_000, &project, &[]);
        live_state.play();
        let mut live = Vec::with_capacity(FRAMES);
        while live.len() < FRAMES {
            live_state.process_block(512);
            live.extend_from_slice(&live_state.master().l[..512]);
        }
        live.truncate(FRAMES);

        // The comparison is only worth anything against audio, and both hits
        // are in the window: a silent pair of buffers would agree perfectly.
        assert!(
            offline.iter().any(|sample| sample.abs() > 0.01),
            "the offline render is silent, so the comparison proves nothing"
        );
        // And the file itself waited: without the plan the plain channel
        // would arrive `latency` frames before its neighbour, which is the
        // defect the export could carry on its own.
        assert!(
            offline[..latency].iter().all(|sample| *sample == 0.0),
            "the offline render arrived {latency} frames early"
        );

        for (frame, (exported, played)) in offline.iter().zip(live.iter()).enumerate() {
            assert_eq!(
                exported, played,
                "frame {frame}: the export gave {exported}, the live render {played}"
            );
        }
    }

    /// `docs/FOCUS.md` step 2's whole acceptance case: a modulation route and
    /// an automation lane both reach the v1 drum synth, which until now was
    /// the one source nothing could move.
    ///
    /// Both halves in one test because they share the resolve pass and the
    /// interesting question is whether a *generator* that had no table until
    /// today is now indistinguishable from one that always had one — nothing
    /// here is drum-specific, which is the point.
    #[test]
    fn a_lane_and_a_route_both_reach_the_v1_drum_synth() {
        let mut channel = ProjectChannel::drum_synth(0, 1);
        // Snare mode on purpose. The kick controls are inert here, so this is
        // also the "audibility gate" case: the route below still resolves,
        // still writes the parameter, and simply is not heard until the
        // device is switched back. That is documented behaviour rather than a
        // special case, and it is what a route onto a bypassed effect does.
        channel
            .setup
            .drum_synth_state_mut()
            .expect("a drum channel has drum state")
            .params
            .mode = mooloop_core::DrumMode::Snare;
        let source = channel
            .setup
            .modulation
            .install(
                0,
                mooloop_core::ModulatorParams::Lfo(mooloop_core::ModLfoParams {
                    rate_hz: 375.0,
                    ..mooloop_core::ModLfoParams::default()
                }),
            )
            .expect("slot 0 accepts a module");
        let punch = ParamAddr {
            scope: EffectTarget::Channel(0),
            owner: ParamOwner::Source,
            param: mooloop_core::DRUM_PARAM_PUNCH,
        };
        assert!(channel
            .setup
            .modulation
            .add_route(mooloop_core::ModRoute::to_slot(
                0,
                punch,
                0.4,
                mooloop_core::ModPolarity::Bipolar,
            ))
            .is_some());
        let _ = source;

        let project = synth_project(channel);
        let mut render = RenderState::from_project(48_000, &project, &[]);

        // The lane drives a different control, so the two are visible apart.
        let start = ParamAddr {
            scope: EffectTarget::Channel(0),
            owner: ParamOwner::Source,
            param: mooloop_core::DRUM_PARAM_KICK_START_HZ,
        };
        render.apply_command(EngineCommand::UpsertAutomationPoint {
            pattern: 0,
            channel: 0,
            target: start,
            point: mooloop_core::AutomationPoint::new(1, 0, 1.0),
        });
        render.play();
        render.process_block(128);

        // The lane reached the device: full-scale on a 20..1000 Hz control.
        assert!(
            (render.strips[0].drum_synth.params().kick_start_hz - 1_000.0).abs() < 1.0,
            "the lane did not reach the drum synth: {}",
            render.strips[0].drum_synth.params().kick_start_hz
        );

        // The route resolved every control tick and actually moved Punch off
        // the knob it was authored at. Four ticks a block, so an empty list
        // would mean the route never ran rather than that it ran flat.
        let punched: Vec<f32> = render.events[0]
            .iter()
            .filter_map(|event| match event.event {
                Event::ParamValue {
                    id: mooloop_core::DRUM_PARAM_PUNCH,
                    value,
                } => Some(value),
                _ => None,
            })
            .collect();
        assert_eq!(punched.len(), 4, "the route was not resolving: {punched:?}");
        let authored = mooloop_core::DrumSynthParams::default().punch;
        assert!(
            punched.iter().any(|value| (value - authored).abs() > 0.05),
            "the route never moved Punch off {authored}: {punched:?}"
        );

        // Mode is untouched by either, which is what makes the inert-kick
        // case an audibility gate rather than an addressing accident.
        assert_eq!(
            render.strips[0].drum_synth.params().mode,
            mooloop_core::DrumMode::Snare
        );

        // Clearing the lane hands the device back its knob rather than
        // leaving it holding the last resolved value.
        render.apply_command(EngineCommand::ClearAutomationLane {
            pattern: 0,
            channel: 0,
            target: start,
        });
        render.process_block(128);
        assert_eq!(
            render.strips[0].drum_synth.params().kick_start_hz,
            mooloop_core::DrumSynthParams::default().kick_start_hz
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
        // A slot number the model would never mint, so a test device cannot
        // be mistaken for one the project loaded.
        install_effect_as(target, slot, mooloop_core::DeviceId(900 + slot as u32), node)
    }

    fn install_effect_as(
        target: EffectTarget,
        slot: u8,
        device: mooloop_core::DeviceId,
        node: Box<dyn AudioNode + Send>,
    ) -> StructuralCommand {
        let align = IntegerDelay::new(node.dry_path_latency_frames()).map(Box::new);
        StructuralCommand::InstallEffect {
            target,
            slot,
            kind: mooloop_core::EffectKind::Filter,
            resource_key: None,
            node,
            align,
            analyzer: Box::new(SpectrumAnalyzer::new()),
            state: Box::new(EffectSlot::for_device(device)),
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
                mooloop_core::EffectKind::Chain => {
                    // A container is transparent by construction and stays
                    // that way: its mix belongs to the chain host, not to the
                    // node in the slot. `docs/plans/containers/03` gives the
                    // host that dry path, and this arm is where a container
                    // that started processing audio itself would be caught.
                }
            }

            let wet = rendered_energy(&project, |render| {
                let _ = render.apply_structural(install_effect(
                    EffectTarget::Channel(0),
                    0,
                    build_effect(params, 48_000),
                ));
            });
            // Two kinds are transparent on purpose, for two different
            // reasons: Follow passes audio through until an atomic buffer
            // event arrives, and a container has no signal path of its own.
            // Equal-power leaks a cos(pi/2) ~ 6e-8 of the aligned dry
            // alongside either, which is inaudible but not bit-exact.
            if matches!(
                kind,
                mooloop_core::EffectKind::Buffer | mooloop_core::EffectKind::Chain
            ) {
                assert!(
                    (wet - dry).abs() < dry * 1.0e-5,
                    "{} must be transparent: dry {dry}, wet {wet}",
                    kind.label()
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

        // Moving 5 to 1 rotates 1..=5 right: the delay in 5 lands on 1 and
        // the one in 2 shifts to 3.
        assert!(chain.move_slot(5, 1));
        assert_eq!(chain.bound, 4);
        assert!(chain.nodes[1].is_some());
        assert!(chain.nodes[2].is_none());
        assert!(chain.nodes[3].is_some());

        assert!(chain.remove(3).node.is_some());
        assert_eq!(chain.bound, 2);
        assert!(chain.remove(1).node.is_some());
        assert_eq!(chain.bound, 0);
    }

    #[test]
    fn the_container_aligns_its_dry_path_to_node_latency() {
        const LATENCY: usize = 15;
        let mut chain = EffectChain::new();
        let node = Box::new(LatentDelay::new(LATENCY));
        let align = IntegerDelay::new(node.latency_frames()).map(Box::new);
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
        chain.process(&context, &mut bus, EffectTarget::Channel(0), None, None, None, true);

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
        let initial_align = IntegerDelay::new(initial.latency_frames()).map(Box::new);
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
        let stale_align = IntegerDelay::new(stale.latency_frames()).map(Box::new);
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
        let current_align = IntegerDelay::new(current.latency_frames()).map(Box::new);
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
        // docs/plans/archive/share-dsp-primitives/01-smooth-effect-parameters.md)
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

        // Moving the filter into an empty slot keeps it in the chain.
        let mut render = RenderState::from_project(48_000, &project, &[]);
        let _ = render.apply_structural(install_effect(
            EffectTarget::Channel(0),
            0,
            muffling_filter(),
        ));
        render.apply_command(EngineCommand::MoveEffect {
            target: EffectTarget::Channel(0),
            from: 0,
            to: 3,
        });
        render.play();
        render.process_block(1024);
        let moved = render.master().l[..1024].iter().map(|s| s * s).sum::<f32>();
        assert!(moved < dry * 0.5, "filter should still muffle after the move");

        // Removing the slot reclaims the node instead of dropping it here.
        let reclaimed = render.apply_structural(StructuralCommand::RemoveEffect {
            target: EffectTarget::Channel(0),
            slot: 3,
        });
        assert!(reclaimed.is_some());
    }

    /// A four-device chain with a long tail in it, built the way the engine
    /// builds one so the aligners and slot state match a real project's.
    fn tailed_chain() -> EffectChain {
        let mut chain = EffectChain::new();
        let kinds = [
            mooloop_core::EffectKind::Reverb,
            mooloop_core::EffectKind::Delay,
            mooloop_core::EffectKind::Drive,
            mooloop_core::EffectKind::Eq,
        ];
        for (slot, kind) in kinds.into_iter().enumerate() {
            let node = build_effect(kind.default_params(), 48_000);
            let align = IntegerDelay::new(node.dry_path_latency_frames()).map(Box::new);
            let displaced = chain.install(
                slot,
                kind,
                None,
                node,
                align,
                Box::new(SpectrumAnalyzer::new()),
                Box::new(EffectSlot::new()),
            );
            assert!(displaced.is_empty());
        }
        chain
    }

    fn chain_context(frames: usize) -> ProcessContext {
        ProcessContext {
            sample_rate: 48_000,
            frames,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        }
    }

    /// Play `blocks` blocks through a fresh chain, with a burst of noise at
    /// the start and another after `wake_at`, and return everything it
    /// produced. `skip_idle` is the only thing that differs between the two
    /// runs the tests below compare.
    fn render_chain(blocks: usize, wake_at: usize, skip_idle: bool) -> Vec<f32> {
        const FRAMES: usize = 512;
        let mut chain = tailed_chain();
        let mut bus = StereoBus::with_capacity(MAX_BLOCK_SIZE);
        let mut out = Vec::with_capacity(blocks * FRAMES);
        let mut noise = 0x1234_5678u32;
        for block in 0..blocks {
            bus.clear(FRAMES);
            if block == 0 || block == wake_at {
                for index in 0..FRAMES {
                    noise ^= noise << 13;
                    noise ^= noise >> 17;
                    noise ^= noise << 5;
                    let sample = ((noise >> 8) as f32 / 8_388_608.0 - 1.0) * 0.5;
                    bus.l[index] = sample;
                    bus.r[index] = sample;
                }
            }
            chain.process(
                &chain_context(FRAMES),
                &mut bus,
                EffectTarget::Channel(0),
                None,
                None,
                None,
                skip_idle,
            );
            out.extend_from_slice(&bus.l[..FRAMES]);
        }
        out
    }

    /// The plainest form of the claim: a chain that is handed nothing renders
    /// nothing, sample for sample, whether or not it is allowed to sleep.
    #[test]
    fn a_chain_fed_silence_renders_identically_with_and_without_skipping() {
        let mut chain = tailed_chain();
        let mut bus = StereoBus::with_capacity(MAX_BLOCK_SIZE);
        // Long enough for the reverb in the chain to run out its declared
        // tail, which is what the chain waits on before any of it sleeps.
        for _ in 0..(10 * 48_000 / 512) {
            bus.clear(512);
            chain.process(
                &chain_context(512),
                &mut bus,
                EffectTarget::Channel(0),
                None,
                None,
                None,
                true,
            );
            assert!(
                bus.l[..512].iter().all(|sample| *sample == 0.0),
                "a sleeping chain must pass exact zeros, not nearly-zeros"
            );
        }
        assert!(
            chain.is_at_rest(),
            "a chain handed nothing for ten seconds should have gone to sleep;              if it has not, this test proves nothing"
        );
    }

    /// The one way this mechanism can be *heard*: a device that under-reports
    /// its tail gets cut off mid-decay. So render the same reverb-and-delay
    /// chain twice — once allowed to sleep and once forced to run every block
    /// — and hold the two against each other for the whole tail, across the
    /// sleep, and through the note that wakes it again.
    ///
    /// The tolerance is `SILENCE_PEAK` with room for the gain of whatever
    /// stands after the device that fell asleep. A slot going to sleep with
    /// its input sitting on the threshold hands the rest of the chain a
    /// signal up to `SILENCE_PEAK` different from what it would have had, and
    /// an EQ band may put 24 dB on that. Sixteen times the threshold is
    /// -116 dBFS, still under one step of a 20-bit render. A truncated tail
    /// would miss by five orders of magnitude, which is the distance this
    /// test is really measuring.
    #[test]
    fn skipping_never_changes_what_a_tail_sounds_like() {
        const BLOCKS: usize = 12 * 48_000 / 512;
        const WAKE_AT: usize = 10 * 48_000 / 512;
        let slept = render_chain(BLOCKS, WAKE_AT, true);
        let ran = render_chain(BLOCKS, WAKE_AT, false);
        assert_eq!(slept.len(), ran.len());

        let mut worst = 0.0f32;
        let mut worst_at = 0;
        for (index, (a, b)) in slept.iter().zip(&ran).enumerate() {
            let difference = (a - b).abs();
            if difference > worst {
                worst = difference;
                worst_at = index;
            }
        }
        assert!(
            worst <= SILENCE_PEAK * 16.0,
            "letting the chain sleep changed its output by {worst} at frame              {worst_at} ({:.3} s in)",
            worst_at as f32 / 48_000.0
        );

        // The premise: there has to be a tail there to truncate, and the
        // chain has to actually wake up for the second burst.
        let tail: f32 = ran[48_000..2 * 48_000]
            .iter()
            .fold(0.0f32, |peak, s| peak.max(s.abs()));
        assert!(tail > 1.0e-3, "no audible tail a second in: {tail}");
        // Half a second after the second burst rather than the block it
        // landed in: the reverb runs at full wet with a 12 ms pre-delay, so
        // the first block after a burst is genuinely almost empty.
        let woken: f32 = slept[WAKE_AT * 512..(WAKE_AT * 512 + 24_000).min(slept.len())]
            .iter()
            .fold(0.0f32, |peak, s| peak.max(s.abs()));
        assert!(
            woken > 1.0e-2,
            "the chain did not wake for the second burst: {woken}"
        );
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
    /// `docs/plans/archive/modulator-capacity/`.
    #[test]
    fn the_render_graph_costs_what_the_project_uses() {
        use core::mem::size_of;

        assert_eq!(MAX_CHANNELS, 256);
        assert_eq!(MAX_EFFECTS_PER_CHANNEL, 256);

        // An occupied effect slot's state is half a kilobyte; an empty one is
        // the pointer that would otherwise have reserved it.
        //
        // It grew by eight when the slot started carrying its device's
        // durable `DeviceId` (`docs/plans/containers/01`) -- four for the id
        // and four of padding -- and by eight more for a container's span and
        // the pointer to the ring that delays its dry copy
        // (`docs/plans/containers/03`). The span is a byte and falls in
        // padding; the eight is the `Option<Box<IntegerDelay>>`, and it is
        // `None` on every leaf.
        //
        // Both are paid per *occupied* slot only, so a project with twenty
        // devices on it pays 320 bytes for the pair. A per-container box
        // instead of a per-slot pointer was the alternative and it is worse:
        // it would need a second lookup on the realtime path to find the
        // container's ring from its slot, which is the table the identity
        // work exists to avoid.
        assert_eq!(size_of::<EffectSlot>(), 512);
        assert_eq!(size_of::<Option<Box<EffectSlot>>>(), 8);
        // Eight of this is the pointer to the per-depth dry buffers a chain
        // needs while it is *inside* containers. One pointer, not four
        // buffers: what a chain needs at once is one copy per box it is
        // currently in, not one per box it holds, so ten sibling containers
        // share what four nested ones would need. A chain with no container
        // pays only the pointer.
        assert_eq!(size_of::<EffectChain>(), 20_552);
        // A strip holds one node of every generator kind, so a new device is
        // paid for on every live channel whether or not anything uses it.
        // The ML-P8 is 5,776 bytes of it. Its eight voices are the bulk -- a
        // voice carries three oscillators, their modulation taps and sync
        // carries, a sub, coloured noise, two envelopes, two filter stages, a
        // drive follower and the feedback loop's delay.
        //
        // Step 04 of `docs/plans/archive/poly-synth-v2/` added 1,536 of it, and where
        // it went is the point. Just over a thousand is per *voice*: the
        // thirty-one destination offsets a voice resolves each sample, which
        // is the price of the modulation landing per voice rather than per
        // device, and is not reducible without giving that up. The rest is
        // held once per node -- the LFO, the parameter block's LFO controls
        // and route list, and the compiled route table with the destination
        // ranges it clamps through. The compiled rows carry byte indices
        // rather than words, and the voice clears its offsets by walking the
        // routes rather than a second list of the destinations they touch;
        // both were measured here, and together they were 576 bytes.
        // Sampler stretch adds another 160 bytes for its
        // pool pointer, tempo, and per-voice struck pitches; the ~1.6 MB pool
        // itself is allocated only when a sampler asks for stretching.
        // Slicing adds 392: the channel's slice-map slot pointer, plus 24
        // bytes on each of the sixteen voices for the span it was struck
        // with. The map itself lives in the slot, off the strip. The last 8
        // are the sampler's transport-edge flag, which is what lets a note
        // auditioned while stopped keep sounding.
        //
        // Step 05 added 984, and it divides cleanly. Seventy-two of it is per
        // *voice*, and fifty-nine of that is the slot's fixed drift table --
        // eleven offsets that exist so "how far is this voice off" is a
        // property of the slot rather than of runtime entropy, which is what
        // makes an offline render reproduce a live take. The rest of the
        // voice's share is the three multipliers Drift, Detune and Spread
        // resolve to once a render range instead of once a sample. Off the
        // voices: 296 for the finishing chorus, of which 280 is the shared
        // `ModulationEffect` it reuses rather than a second chorus; 96 for the
        // two scratch bus *headers*; and 16 for the five new parameters.
        //
        // The scratch buses are the one figure this test cannot see, because
        // `size_of` a `Vec` is its header. They are deliberately one 512-frame
        // chunk each rather than `MAX_BLOCK_SIZE`, which is 8 KB of heap a
        // materialized channel instead of 128 KB; the device renders in chunks
        // to afford that, and its bit-identity tests are what say the chunk
        // boundary is not audible.
        //
        // Step 06's published control outlets added 72 on top, and they are
        // the cheapest thing in this test because publication is a reduction
        // rather than a buffer: eight bytes on the node for the focus group's
        // age and its trigger latch, and eight on each voice for the velocity
        // its note was played at. The seven outlet *values* are computed on
        // demand from state the voices already carry, so nothing here stores
        // them.
        //
        // The typed audio edges added 64: eight bytes a voice for the sub,
        // pre-filter and filter samples it publishes, recorded as it runs
        // them. Nothing on the node at all, and nothing per outlet -- a tap's
        // *buffer* is 64 KB and is allocated only when somebody subscribes,
        // which is the whole shape of that plan.
        assert_eq!(size_of::<MlP8>(), 5_840);
        // DS-01 is 6,832, and almost all of it is the eight-voice pool: a
        // voice carries six tone oscillators for its partial bank, an FM
        // modulator, four noise generators' worth of state, a state-variable
        // filter, the rate reducer's hold, four envelopes, the body's three
        // resonators, the burst's schedule, and the eight source values it
        // presents to its own matrix. Its envelopes are the largest share --
        // an `Ahd` is 48 bytes against `ExpDecay`'s 8, four of them where v1
        // has two -- and they are the largest single reason DS-01's snare and
        // its hat do not sound like the same instrument. The shape stage costs
        // almost nothing on top, being a function of the sample rather than a
        // stage with state; step 07's matrix costs a parameter block its eight
        // rows dominate, plus one resolved control set per voice, because the
        // matrix is per voice and two hits sounding at once have to be able to
        // disagree about where the filter is.
        //
        // `mooloop_dsp::ds01`'s own size test splits that between the pool and
        // the parameter block. It does not widen `source_base`: `Ds01Params`
        // is smaller than `MlP8Params`, which is still the widest
        // `GeneratorParams` variant and therefore still what every channel
        // pays for.
        //
        // Step 07's publication is the last sixteen: the focus age and the
        // trigger flag, both node state rather than voice state, because the
        // focus is a fact about this channel's run of hits and a per-voice
        // copy would be eight numbers agreeing about one.
        // The typed audio edges added 128 on top: sixteen bytes a voice for
        // the three pre-Level layers and the layer mix it publishes.
        assert_eq!(size_of::<Ds01>(), 6_960);
        // The strip pays the parameter block twice: once inside the node
        // above, and once for `source_base`, whose `GeneratorParams` is as
        // wide as its widest variant and the ML-P8 is that variant. Step 05's
        // sixteen bytes of new parameters are therefore paid twice, which is
        // why the strip moved by 1,000 where the node moved by 984. Step
        // 06's routable outlets added 32 more, and none of it is in the node:
        // it is the eight `f32` the strip holds of what its generator
        // published last block, which is where the one block of declared
        // outlet latency physically lives.
        // Latency compensation adds eight: a nullable pointer to the ring, and
        // nothing else. The ring itself is heap and exists only on a channel
        // that actually owes a delay — `4 * 2 * frames`, so fifteen frames is
        // 120 bytes — which is why the common project, where every path is the
        // same length, pays exactly this pointer and no buffer at all.
        // The typed audio edges added 208. Sixty-four of it is ML-P8's
        // published samples and 128 is DS-01's, both paid inside the nodes
        // above; the Aux In node itself is 20 bytes -- a subscription, a
        // level and its smoother -- and `GeneratorParams` did not widen,
        // because the ML-P8's parameter block is still much the largest
        // variant. A channel that is not an Aux In and publishes nothing pays
        // 20 bytes for a node it never runs, which is what every generator
        // kind already costs every channel.
        // Letting an idle channel stop rendering added eight: a count of the
        // frames its generator has been silent for, and a flag saying whether
        // it is currently asleep. The chain's half of the same bookkeeping is
        // free -- a `u32` fits in `EffectSlot`'s existing padding, which is
        // why the assertion above did not move, and an addressable-but-empty
        // slot still costs a pointer.
        // Containers added the eight above and nothing else: the dry buffers
        // themselves are behind the pointer and only allocated for a chain
        // that holds a box.
        assert_eq!(size_of::<ChannelStrip>(), 41_936);

        // Reserved whatever the project holds: the two small modulation
        // vectors, plus three vectors of pointers to per-channel storage.
        // Sixteen KiB of the rise is `ParamAddr` growing four bytes to carry
        // a durable internal-route id, paid once per stored route across the
        // reserved channel count. Another sixteen is `ModRoute` growing four
        // the same way and for the same kind of reason: its source became a
        // `ModSourceRef`, because a generator outlet is not a rack module and
        // has no identity the rack could mint for it.
        //
        // And another sixteen for the third instance of that same trade
        // (`docs/plans/containers/01`): `ParamOwner::Effect` stopped naming a
        // `u8` slot and started naming a `u32` `DeviceId`, so a route's
        // destination no longer moves when the rack is reordered. Four bytes
        // an address, 64 a channel's rack, 16 KiB across the reserved count.
        // The same price and the same argument as the two above, and it buys
        // the thing they bought one level out.
        let fixed = (size_of::<ModRack>() + size_of::<ModulatorRack>()) * MAX_CHANNELS
            + MAX_CHANNELS * size_of::<usize>() * 3;
        assert_eq!(fixed / 1024, 487);

        // Paid per channel the project actually has.
        let per_live =
            size_of::<ChannelStrip>() + size_of::<EventList>() + size_of::<ControlOutputs>();
        assert_eq!(per_live, 60_376);

        // 42.8 MiB reserved at startup became 1.1 MiB for a sixteen-channel
        // project, with both ceilings untouched. A sixth generator kind moved
        // it by 41 KiB, which is what a device costs now: linear in the
        // channels a project has rather than in the channels it could address.
        // Slice mode moved it by 6 KiB across sixteen channels, the ML-P8's
        // native modulation another 40 -- 24 of it per-voice offset tables on
        // sixteen channels, which is what buys modulation that lands per
        // voice, and 16 of it the wider parameter address, paid once per
        // reserved channel whether or not anything routes -- and DS-01, the
        // seventh kind, another 106. The ML-P8's voice pool moved it 15 more:
        // nine of that is the eight slots' fixed drift tables, and the rest is
        // the finishing chorus and its two scratch headers. The chorus's
        // *buffers* are 8 KiB of heap a live channel on top of this, which is
        // the figure the 512-frame chunk exists to keep small. Step 06's
        // published outlets moved it by one more KiB across sixteen channels,
        // which is what a device's whole published interface costs when
        // publication is a reduction of state that already exists. Making
        // them routable moved it by half of one more: 32 bytes a live channel
        // for the published row, and 16 KiB of the reserved figure above for
        // the wider route. DS-01 publishing its own six added sixteen bytes a
        // live channel -- a focus age and a trigger flag on the node, and
        // nothing on any voice. Latency compensation added eight more, which
        // is a pointer: the ring is allocated only for a producer that is
        // genuinely shorter than its neighbours, so an aligned project pays
        // nothing beyond the pointer.
        //
        // It nearly cost 8 KiB a live channel instead. Putting the outlet
        // band in the per-tick control table would have stored eight
        // block-constant values in all 256 tick rows; `ControlSources` keeps
        // the flat *address space* routes and projects depend on while
        // storing each half at the rate it is actually captured. This test
        // is what asked the question.
        // The typed audio edges moved it by 4 KiB across sixteen channels,
        // and that is the entire cost of the feature on a project that never
        // uses it: no buffers, no plan storage beyond one boxed value for the
        // whole engine, and an identity render order. Materialising ML-P8's
        // seven stereo outlets unconditionally would have been 448 KB a
        // channel instead -- 7 MB across a full bank for something switched
        // off -- which is the design `03-materialized-taps.md` exists to
        // avoid, and this is the measurement that says it was avoided.
        // Durable device identity added 16 KiB of the reserved figure and
        // nothing per live channel: an effect slot is heap-allocated, so its
        // own growth is per *occupied* slot rather than per channel.
        // Containers added eight bytes a live channel -- one pointer to the
        // dry buffers a chain needs while it is inside a box -- and the
        // buffers themselves only exist on a chain that holds one. 128 bytes
        // across sixteen channels, which does not move the figure below.
        assert_eq!((fixed + per_live * 16) / 1024, 1_430);
    }
}

