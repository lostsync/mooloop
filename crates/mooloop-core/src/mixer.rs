//! Mixer buses: the destinations channels feed, and the second thing (after a
//! channel) that can own an effect chain.
//!
//! The model is FL Studio's: every sequencer channel names one bus, buses can
//! feed other buses, and everything eventually reaches the master. The master
//! is bus 0; it always exists and its own `output` is unused.
//!
//! Any bus may feed any other. The realtime thread still never sorts a graph:
//! `compile_bus_graph` validates and topologically sorts the bank here, off the
//! audio thread, and the engine walks the resulting plan. This is how REAPER
//! and Ardour work — the graph is compiled into a flat schedule by whoever
//! edits it, and the audio callback only ever executes that schedule.
//!
//! We get off unusually lightly compared to those hosts, because every bus
//! owns a permanently allocated buffer and no two nodes ever share one. That
//! removes the pooled, reference-counted buffer assignment a general graph
//! engine needs, and leaves the entire schedule as a `[u8; MAX_BUSES]`
//! permutation.
//!
//! Cycles are refused rather than delayed. Allowing them would mean reading a
//! bus's previous block to break the loop, which is a deliberate feature
//! (feedback routing) rather than a fallback, and it needs a latency story
//! this engine does not have yet.

use crate::EffectSlotState;
use crate::MAX_CHANNELS;

/// Insert buses available in addition to the master.
pub const INSERT_BUSES: usize = 16;

/// Total addressable buses: the master plus every insert.
pub const MAX_BUSES: usize = INSERT_BUSES + 1;

/// Index of the master bus. Channels and buses default to feeding it.
pub const MASTER_BUS: u8 = 0;

/// Where an effect chain lives. Effect commands address a target rather than a
/// channel so one set of install/remove/param messages serves both.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum EffectTarget {
    Channel(u8),
    Bus(u8),
}

/// One mixer bus's non-effect state.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MixerBus {
    pub name: String,
    pub muted: bool,
    /// Linear output volume in [0, `crate::gain::MAX_LINEAR_GAIN`] (+12 dB),
    /// the same range a channel's gets: a bus is a gain stage too.
    pub volume: f32,
    /// Stereo pan in [-1, 1].
    pub pan: f32,
    /// Destination bus index. Any other bus is legal when it does not close a
    /// cycle; the master's value is unused.
    pub output: u8,
    /// Whether this track's *output* is console-encoded, so the summing point
    /// it reaches decodes it together with everything else that opted in.
    ///
    /// **A track's switch, and only a track's.** A sequencer channel has no
    /// analog sum of its own: the console this models puts its Channel stage
    /// on a mixer strip, and mooloop's mixer strip is a track. Adam,
    /// 2026-09-09: *"the summing thing for now is tracks-only."*
    ///
    /// Nesting needs no special case: a console-on track encodes at its own
    /// output and whatever it feeds decodes it. Meaningless on the master,
    /// which feeds nothing, and ignored there. Defaulted on load; `false`
    /// reproduces a linear mixer exactly.
    #[serde(default)]
    pub console: bool,
}

impl MixerBus {
    /// Build bus `index` with its default name, unity gain, and routing to the
    /// master.
    pub fn new(index: usize) -> Self {
        Self {
            name: if index == MASTER_BUS as usize {
                "Master".into()
            } else {
                format!("Bus {index}")
            },
            muted: false,
            // Buses are summing points, not sources: they start at unity so
            // assigning a channel to one never quietly attenuates it.
            volume: 1.0,
            pan: 0.0,
            output: MASTER_BUS,
            console: false,
        }
    }
}

/// Where on a strip a send takes its signal.
///
/// Both taps are *after* the effect chain, because the fader is: in the block
/// loop a strip's chain runs, then its fader and pan, then its compensation.
/// So the only difference between the two is whether the fader and pan have
/// been applied, and — worth stating because the latency plan depends on it —
/// **they arrive at the same time**, since a fader declares no latency.
///
/// Taps further up the chain (after a named device, or at one of a device's
/// declared audio outlets) are the second half of `docs/plans/console/`'s
/// step 05 and are not here yet: they need a per-tap chain prefix latency,
/// where these two need none.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum SendTap {
    /// After the fader and pan, so the send follows the strip's own level.
    /// The default, and what a reverb send usually wants.
    #[default]
    PostFader,
    /// After the chain but before the fader and pan, so the send holds its
    /// level while the strip's fader moves.
    PreFader,
}

/// One route from a strip to a track, in addition to that strip's output.
///
/// A send is a *route*, not a kind of track: `docs/TERMINOLOGY.md` is the
/// vocabulary, and the track a send arrives at is an ordinary track that
/// happens to be fed by sends. There is no return object, and there is no
/// upper bound on how many of these a strip has -- `docs/CAPACITY_POLICY.md`
/// forbids one, and nothing here reserves for them.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AuxSend {
    /// Track this send feeds. Not the strip's own index, and never a track
    /// that reaches this one -- see [`would_create_cycle`].
    pub target: u8,
    /// Linear send level in [0, `crate::gain::MAX_LINEAR_GAIN`], the same
    /// range a fader gets.
    pub level: f32,
    #[serde(default)]
    pub tap: SendTap,
    /// Whether the send passes audio.
    ///
    /// Separate from `level` because a level of zero is a valid setting and
    /// not the same statement -- a send turned down is still routed, and gets
    /// its level back when it is turned up.
    ///
    /// A disabled send is still **part of the graph**: it orders its target
    /// after its source and it still counts at the summing point it reaches.
    /// The alternative -- dropping it from the plan -- would make a mute
    /// switch re-time the whole mix, which is a surprise nobody asked for.
    #[serde(default = "crate::mixer::enabled_by_default")]
    pub enabled: bool,
}

pub(crate) fn enabled_by_default() -> bool {
    true
}

impl AuxSend {
    /// A new post-fader send to `target`, at unity.
    ///
    /// Unity rather than silence because a send you have just made and cannot
    /// hear is indistinguishable from one that did not work.
    pub fn new(target: u8) -> Self {
        Self {
            target,
            level: 1.0,
            tap: SendTap::PostFader,
            enabled: true,
        }
    }
}

/// Whether `from` could send to `target` at all, ignoring the rest of the
/// graph. Nothing sends to itself, and both ends must be addressable.
pub fn is_legal_send(from: u8, target: u8) -> bool {
    from != target && (from as usize) < MAX_BUSES && (target as usize) < MAX_BUSES
}

/// A bus plus the effect chain inserted on it, mirroring `ChannelSetup`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BusSetup {
    pub bus: MixerBus,
    #[serde(default)]
    pub effects: Vec<EffectSlotState>,
    /// Next device identity to mint for `effects`, for the same reason and
    /// with the same defaulting as `ChannelSetup::next_device_id`.
    #[serde(default)]
    pub next_device_id: u32,
    /// Routes from this track to other tracks, in addition to its output.
    ///
    /// Defaulted, so every project written before sends existed loads with
    /// none and plays identically.
    #[serde(default)]
    pub sends: Vec<AuxSend>,
}

impl BusSetup {
    pub fn new(index: usize) -> Self {
        Self {
            bus: MixerBus::new(index),
            effects: Vec::new(),
            next_device_id: 0,
            sends: Vec::new(),
        }
    }

    /// Append `effect` to the chain, minting it an identity. See
    /// `ChannelSetup::push_effect`.
    pub fn push_effect(&mut self, effect: EffectSlotState) -> Option<crate::DeviceId> {
        let at = self.effects.len();
        crate::insert_effect(&mut self.effects, &mut self.next_device_id, at, effect)?;
        Some(self.effects[at].id)
    }

    /// Give this chain's devices their identities and put the mint past them.
    pub fn assign_device_ids(&mut self) {
        crate::assign_device_ids(&mut self.effects, &mut self.next_device_id);
    }
}

/// The tracks a project starts with: the master, and nothing else.
///
/// **This used to return all seventeen**, on the reasoning that assigning a
/// channel to bus 12 should never have to create one. That is a fixed bank
/// wearing a `Vec`, and `docs/CAPACITY_POLICY.md` is about exactly this:
/// reserving an address space is free, dimensioning by it is not. A track now
/// exists because somebody made it.
///
/// The master is not optional -- it is the sink every route eventually
/// reaches, and a project without one has nowhere to send audio -- so it is
/// the one entry a bank always has. [`sanitize_bank`] enforces that on load.
pub fn default_buses() -> Vec<BusSetup> {
    vec![BusSetup::new(MASTER_BUS as usize)]
}

/// Repair a loaded track bank: guarantee the master, drop anything past the
/// addressable space, and coerce individually illegal routing to the master.
///
/// Per-edge nonsense is fixed first and the graph as a whole second, so a file
/// whose routing contains a loop is flattened to everything-to-master rather
/// than rejected -- it still opens and plays, which is the same treatment the
/// engine gives it.
pub fn sanitize_bank(buses: &[BusSetup]) -> Vec<BusSetup> {
    let mut bank: Vec<BusSetup> = buses
        .iter()
        .take(MAX_BUSES)
        .enumerate()
        .map(|(index, setup)| {
            let mut setup = setup.clone();
            setup.bus.output = sanitize_route(index as u8, setup.bus.output);
            setup
        })
        .collect();
    if bank.is_empty() {
        bank.push(BusSetup::new(MASTER_BUS as usize));
    }
    // A route naming a track that is not there lands on the master, the same
    // repair `sanitize_route` makes for an out-of-range one.
    let count = bank.len();
    for (index, setup) in bank.iter_mut().enumerate() {
        if index != MASTER_BUS as usize && setup.bus.output as usize >= count {
            setup.bus.output = MASTER_BUS;
        }
        // A send naming a track that is not there is **dropped**, not
        // re-pointed at the master. A channel's output has to land somewhere
        // or the channel is silently unheard, which is why that one falls
        // back; a send is an addition, and the honest repair for one whose
        // destination is gone is that it is gone too.
        setup
            .sends
            .retain(|send| is_legal_send(index as u8, send.target) && (send.target as usize) < count);
    }
    if compile_bus_graph(&bank).is_none() {
        for setup in &mut bank {
            setup.bus.output = MASTER_BUS;
            setup.sends.clear();
        }
    }
    bank
}

/// Whether `bus` could address `output` at all, ignoring what the rest of the
/// graph looks like. The master is a sink and has no output of its own, and
/// nothing may feed itself.
pub fn is_legal_route(bus: u8, output: u8) -> bool {
    bus != MASTER_BUS
        && (bus as usize) < MAX_BUSES
        && (output as usize) < MAX_BUSES
        && output != bus
}

/// Coerce an individually nonsensical routing (an older or hand-edited file)
/// to the master. This does not consider cycles; use `compile_bus_graph`
/// for that, since a cycle is a property of the whole graph rather than of
/// one edge.
pub fn sanitize_route(bus: u8, output: u8) -> u8 {
    if is_legal_route(bus, output) {
        output
    } else {
        MASTER_BUS
    }
}

/// Whether `from` reaches `target` by following outputs **or sends**.
///
/// This used to walk a single-successor chain, which was exact while a track
/// had one outgoing edge. A send is a second one, so it is a search: depth
/// first over a fixed visited set, which needs no allocation and terminates on
/// a graph that is already cyclic because a node is only pushed once.
fn reaches(buses: &[BusSetup], from: u8, target: u8) -> bool {
    let mut seen = [false; MAX_BUSES];
    let mut stack = [MASTER_BUS; MAX_BUSES];
    let mut depth = 0usize;

    if from == target {
        return true;
    }
    if (from as usize) < MAX_BUSES {
        seen[from as usize] = true;
        stack[depth] = from;
        depth += 1;
    }

    while depth > 0 {
        depth -= 1;
        let at = stack[depth];
        // The master is a sink: its `output` is unused, and a send from it
        // could only ever close a loop, so neither is followed.
        if at == MASTER_BUS {
            continue;
        }
        let Some(setup) = buses.get(at as usize) else {
            continue;
        };
        let successors = std::iter::once(setup.bus.output)
            .chain(setup.sends.iter().map(|send| send.target))
            .filter(|&next| is_legal_send(at, next));
        for next in successors {
            if next == target {
                return true;
            }
            if seen[next as usize] {
                continue;
            }
            seen[next as usize] = true;
            // Each node is pushed at most once, so this cannot run past the
            // stack: that is also what terminates the walk on a bank whose
            // routing is already cyclic.
            stack[depth] = next;
            depth += 1;
        }
    }
    false
}

/// Whether routing `bus` into `output` would close a loop. The interface uses
/// this to decline the connection rather than offering it and then silently
/// rewriting it to something the user did not ask for.
///
/// The same question answers a send: adding a send from `bus` to `output`
/// loops exactly when `output` already reaches `bus`, which is what this is.
/// One rule for both edges, so a send cannot creep past a check the output
/// picker makes.
pub fn would_create_cycle(buses: &[BusSetup], bus: u8, output: u8) -> bool {
    reaches(buses, output, bus)
}

/// Order in which the engine renders the bank, sources before destinations.
pub type RenderOrder = [u8; MAX_BUSES];

/// A complete, fixed-capacity bus execution plan. Destinations and their
/// topological order are one value so the realtime executor can never observe
/// an edge from one graph generation with the schedule from another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompiledBusGraph {
    destinations: [u8; MAX_BUSES],
    render_order: RenderOrder,
}

impl CompiledBusGraph {
    pub fn destination(&self, bus: usize) -> u8 {
        self.destinations.get(bus).copied().unwrap_or(MASTER_BUS)
    }

    pub fn destinations(&self) -> &[u8; MAX_BUSES] {
        &self.destinations
    }

    pub fn render_order(&self) -> &RenderOrder {
        &self.render_order
    }
}

impl Default for CompiledBusGraph {
    fn default() -> Self {
        Self {
            destinations: [MASTER_BUS; MAX_BUSES],
            render_order: default_render_order(),
        }
    }
}

/// The order that always works: every bus straight to the master, highest
/// index first. Used for a fresh bank and as the repair for a cyclic one.
pub fn default_render_order() -> RenderOrder {
    let mut order = [MASTER_BUS; MAX_BUSES];
    for (slot, index) in order.iter_mut().zip((0..MAX_BUSES as u8).rev()) {
        *slot = index;
    }
    order
}

/// Compile editable bus data into the complete plan consumed by the engine.
///
/// Short banks are padded with default buses, while invalid individual edges
/// are repaired to the master. A genuine multi-bus cycle has no valid plan and
/// returns `None`.
pub fn compile_bus_graph(buses: &[BusSetup]) -> Option<CompiledBusGraph> {
    let mut destinations = [MASTER_BUS; MAX_BUSES];
    for (index, setup) in buses.iter().take(MAX_BUSES).enumerate().skip(1) {
        destinations[index] = sanitize_route(index as u8, setup.bus.output);
    }

    // Number of buses feeding each bus, by output **or by send**. Channels are
    // not counted: they are all rendered before any bus, so they constrain
    // nothing, and that stays true of a channel's sends.
    //
    // `u16` rather than `u8` because the outputs are one per track and the
    // sends are not: nothing bounds how many a track has, and a count that
    // wrapped would emit a schedule in the wrong order rather than refuse.
    let mut feeding = [0u16; MAX_BUSES];
    for &destination in destinations.iter().skip(1) {
        feeding[destination as usize] += 1;
    }
    for (index, setup) in buses.iter().take(MAX_BUSES).enumerate() {
        for send in &setup.sends {
            if is_legal_send(index as u8, send.target) {
                feeding[send.target as usize] += 1;
            }
        }
    }

    let mut queue = [MASTER_BUS; MAX_BUSES];
    let (mut head, mut tail) = (0usize, 0usize);
    for (index, count) in feeding.iter().enumerate() {
        if *count == 0 {
            queue[tail] = index as u8;
            tail += 1;
        }
    }

    let mut render_order = [MASTER_BUS; MAX_BUSES];
    let mut emitted = 0usize;
    while head < tail {
        let node = queue[head];
        head += 1;
        render_order[emitted] = node;
        emitted += 1;

        if node == MASTER_BUS {
            continue;
        }
        let release = |destination: usize, feeding: &mut [u16; MAX_BUSES], queue: &mut [u8; MAX_BUSES], tail: &mut usize| {
            feeding[destination] -= 1;
            if feeding[destination] == 0 {
                queue[*tail] = destination as u8;
                *tail += 1;
            }
        };
        release(
            destinations[node as usize] as usize,
            &mut feeding,
            &mut queue,
            &mut tail,
        );
        if let Some(setup) = buses.get(node as usize) {
            for send in &setup.sends {
                if is_legal_send(node, send.target) {
                    release(send.target as usize, &mut feeding, &mut queue, &mut tail);
                }
            }
        }
    }

    // Every slot in the fixed address space is sorted, present or not: an
    // absent track is a phantom feeding the master, which constrains nothing
    // and keeps `render_order` a complete permutation. The executor skips the
    // ones that do not exist, and must walk the *whole* order to do that --
    // iterating only the first `buses.len()` slots would visit an arbitrary
    // subset and silently drop real tracks.
    (emitted == MAX_BUSES).then_some(CompiledBusGraph {
        destinations,
        render_order,
    })
}

/// Keep a channel's bus assignment inside the bank. A stale index from the GUI
/// lands on the master rather than silently muting the channel.
///
/// Here rather than in the engine, which held the only copy, because the
/// latency plan below has to agree with the executor about which bus a channel
/// actually feeds — two answers to that would compensate a channel against a
/// summing point it does not sum into.
pub fn clamp_bus(bus: u8) -> u8 {
    if (bus as usize) < MAX_BUSES {
        bus
    } else {
        MASTER_BUS
    }
}

/// Total declared latency of one effect chain, in base-rate frames.
///
/// A bypassed slot counts. That is the convention every host follows and the
/// reason is that the alternative is worse: a bypass that shortened the chain
/// would move the channel in time relative to every other one, so A/B-ing an
/// effect would also A/B the timing and neither answer would be about the
/// effect. Removing the device is what gives the latency back.
pub fn chain_latency(effects: &[EffectSlotState]) -> u32 {
    effects
        .iter()
        .map(|effect| effect.kind().latency_frames())
        .sum()
}

/// Declared latency of the run the container in `slot` encloses.
///
/// What a container's dry copy has to be delayed by before it is crossfaded
/// back in, or the blend combs. Zero when `slot` holds a leaf, or a container
/// whose run holds nothing that declares a latency -- which is every run
/// without a Drive in it, and so most of them.
///
/// This is `chain_latency` over a sub-range and it is deliberately not the
/// container's *own* declared latency, which is zero: the mixer's plan sums
/// the whole chain and already counts these rows, so declaring the run here
/// too would compensate the channel twice. See question 4 in
/// `docs/plans/containers/README.md`.
pub fn run_latency(effects: &[EffectSlotState], slot: usize) -> u32 {
    let run = crate::span_of(effects, slot);
    effects[run].iter().map(|e| e.kind().latency_frames()).sum()
}

/// What each producer must be delayed by so that everything summing at a
/// point arrives from the same moment.
///
/// One number per channel and per bus, in the same fixed-capacity shape and
/// for the same reason as [`CompiledBusGraph`]: the whole plan crosses to the
/// executor by value, so it can never observe one generation's delays against
/// another's edges. What allocates is the delay *storage* the engine sizes
/// from this, never this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledLatency {
    channels: [u32; MAX_CHANNELS],
    buses: [u32; MAX_BUSES],
    /// One entry per send, in the order the sends were handed to
    /// [`compile_latency`]. A `Vec` rather than a fixed bank because nothing
    /// bounds how many sends a project has, and this is the *answer* rather
    /// than the storage -- what allocates on the audio thread's behalf is the
    /// ring the engine sizes from it, off the audio thread, as before.
    ///
    /// This is the sentence that used to say "per edge" and "per producer" are
    /// the same thing. A send is a producer's second outgoing edge, and the
    /// two arrive at different summing points, so they are not.
    sends: Vec<u32>,
    total: u32,
}

impl CompiledLatency {
    /// Frames to delay `channel`'s output before it sums into its bus.
    pub fn channel(&self, channel: usize) -> u32 {
        self.channels.get(channel).copied().unwrap_or(0)
    }

    /// Frames to delay `bus`'s output before it sums into the bus it feeds.
    /// The master feeds nothing and is always zero.
    pub fn bus(&self, bus: usize) -> u32 {
        self.buses.get(bus).copied().unwrap_or(0)
    }

    /// Frames to delay send `index` before it sums into the track it feeds,
    /// indexed as the sends were handed to [`compile_latency`].
    pub fn send(&self, index: usize) -> u32 {
        self.sends.get(index).copied().unwrap_or(0)
    }

    /// How many sends this plan was compiled for.
    pub fn send_count(&self) -> usize {
        self.sends.len()
    }

    /// The whole graph's latency: how far behind the input the master's output
    /// now is. This is the figure a host would be told, and it is the price of
    /// alignment — the longest path does not move, so everything else waits
    /// for it.
    pub fn total(&self) -> u32 {
        self.total
    }
}

impl Default for CompiledLatency {
    fn default() -> Self {
        Self {
            channels: [0; MAX_CHANNELS],
            buses: [0; MAX_BUSES],
            sends: Vec::new(),
            total: 0,
        }
    }
}

/// One send flattened out of the model for compilation: who emits it and
/// which track it reaches.
///
/// The tap is not here because it does not change the answer. Both taps this
/// step has are after the strip's chain and a fader declares no latency, so a
/// pre-fader send and a post-fader one from the same strip arrive at the same
/// moment. A tap *inside* the chain would not, and is what would put a tap
/// point in this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SendEdge {
    pub producer: EffectTarget,
    pub target: u8,
}

/// Every send in a track bank, flattened in bank order.
///
/// The order is the contract between [`compile_latency`]'s answer and the
/// engine's bank: entry `n` here is entry `n` there. Bank order -- track by
/// track, and within a track the order the user made them in -- is the one
/// order both sides can derive without being told.
pub fn send_edges(buses: &[BusSetup]) -> Vec<SendEdge> {
    let mut edges = Vec::new();
    for (index, setup) in buses.iter().take(MAX_BUSES).enumerate() {
        for send in &setup.sends {
            edges.push(SendEdge {
                producer: EffectTarget::Bus(index as u8),
                target: send.target,
            });
        }
    }
    edges
}

/// Compile the tree's cumulative latency into a per-producer compensation.
///
/// `channel_latency[c]` is the sum of channel `c`'s own chain, and
/// `channel_bus[c]` the bus it feeds; `bus_latency[b]` is bus `b`'s own chain.
/// Shorter slices are read as zeros, which is what a project with four
/// channels hands in.
///
/// The rule is the general one from `AUDIO_ARCHITECTURE.md` -- at every
/// summing point, delay each input by the difference between it and the
/// longest one. **`sends` is what makes this a graph rather than a tree**: a
/// producer with a send has two outgoing edges reaching two summing points,
/// which arrive at different times and are owed different delays. The answer
/// is still one number per edge; there are simply more edges than producers
/// now, which is the sentence this doc comment used to deny.
///
/// A *disabled* send belongs in `sends` like any other. Leaving it out would
/// mean a mute switch re-timed the mix.
///
/// One descending pass over `graph.render_order()`, which is already
/// topological **over sends as well** -- `compile_bus_graph` counts them in
/// its in-degree -- so every track feeding `b` by either kind of edge is
/// visited before `b`, and `b`'s input arrival is complete by the time it is
/// read. No recursion, no second sort, and the only allocation is the answer.
pub fn compile_latency(
    graph: &CompiledBusGraph,
    channel_latency: &[u32],
    channel_bus: &[u8],
    bus_latency: &[u32],
    sends: &[SendEdge],
) -> CompiledLatency {
    let at = |table: &[u32], index: usize| table.get(index).copied().unwrap_or(0);

    // The latest anything feeding each bus arrives, before that bus's own
    // chain. Channels are known up front; buses fill in as they are visited.
    let mut input_arrival = [0u32; MAX_BUSES];
    let channels = channel_latency.len().min(channel_bus.len()).min(MAX_CHANNELS);
    for channel in 0..channels {
        let bus = clamp_bus(channel_bus[channel]) as usize;
        input_arrival[bus] = input_arrival[bus].max(channel_latency[channel]);
    }
    // A channel's sends are known here for the same reason its output is:
    // channels all render before any track, so nothing they emit waits on a
    // track's arrival.
    for edge in sends {
        if let EffectTarget::Channel(channel) = edge.producer {
            let arrival = at(channel_latency, channel as usize);
            let target = clamp_bus(edge.target) as usize;
            input_arrival[target] = input_arrival[target].max(arrival);
        }
    }

    let mut arrival = [0u32; MAX_BUSES];
    for &bus in graph.render_order() {
        let bus = bus as usize;
        arrival[bus] = input_arrival[bus] + at(bus_latency, bus);
        if bus == MASTER_BUS as usize {
            continue;
        }
        let destination = graph.destination(bus) as usize;
        input_arrival[destination] = input_arrival[destination].max(arrival[bus]);
        // A track's sends leave from the same place its output does, so they
        // carry the same arrival to a different summing point.
        for edge in sends {
            if edge.producer == EffectTarget::Bus(bus as u8) {
                let target = clamp_bus(edge.target) as usize;
                input_arrival[target] = input_arrival[target].max(arrival[bus]);
            }
        }
    }

    let mut channels_out = [0u32; MAX_CHANNELS];
    for channel in 0..channels {
        let bus = clamp_bus(channel_bus[channel]) as usize;
        channels_out[channel] = input_arrival[bus].saturating_sub(channel_latency[channel]);
    }

    let mut buses_out = [0u32; MAX_BUSES];
    for bus in 0..MAX_BUSES {
        if bus == MASTER_BUS as usize {
            continue;
        }
        let destination = graph.destination(bus) as usize;
        buses_out[bus] = input_arrival[destination].saturating_sub(arrival[bus]);
    }

    // Each send waits by the difference between the summing point it reaches
    // and its own arrival -- the same subtraction the two edge kinds above
    // make, against a different destination.
    let sends_out = sends
        .iter()
        .map(|edge| {
            let departs = match edge.producer {
                EffectTarget::Channel(channel) => at(channel_latency, channel as usize),
                EffectTarget::Bus(bus) => arrival[clamp_bus(bus) as usize],
            };
            input_arrival[clamp_bus(edge.target) as usize].saturating_sub(departs)
        })
        .collect();

    CompiledLatency {
        channels: channels_out,
        buses: buses_out,
        sends: sends_out,
        total: arrival[MASTER_BUS as usize],
    }
}

/// Topologically sort the bank so every bus is rendered before the bus it
/// feeds. Returns `None` if the routing contains a cycle.
///
/// This is Kahn's algorithm over fixed-size arrays: no allocation, no
/// recursion, and bounded by the bank size. It is cheap enough to run on the
/// audio thread, but deliberately is not — the point is that the realtime
/// side receives a finished schedule and never reasons about the graph.
pub fn compile_render_order(buses: &[BusSetup]) -> Option<RenderOrder> {
    compile_bus_graph(buses).map(|graph| *graph.render_order())
}

// --- The audio graph -------------------------------------------------------

/// Order in which the engine renders the channels, producers before consumers.
///
/// A separate permutation from [`RenderOrder`] because it sorts a different
/// set for a different reason: the bus order exists because buses sum into
/// each other, and this one exists because one channel's device can *read*
/// another's, which is a dependency rather than a summing point.
pub type AudioOrder = [u8; MAX_CHANNELS];

/// Why an authored subscription does not carry audio.
///
/// Refusals are values rather than a bare `None` because the consumer's face
/// has to say which one happened: an edge into a channel that stopped
/// publishing and an edge that closes a cycle both produce silence, and a user
/// cannot fix either one without being told them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeRefusal {
    /// The named channel is beyond the bank.
    NoSuchChannel,
    /// A channel cannot read itself. Kept distinct from [`Self::Cycle`]
    /// because the fix is different and so is the mistake.
    SelfSubscribed,
    /// The named channel's generator publishes no audio outlets at all --
    /// usually because the channel's device was changed after the edge was
    /// authored.
    NotAProducer,
    /// The channel publishes audio, but not under this id.
    NoSuchOutlet,
    /// The id names a control outlet. Refused structurally rather than
    /// converted: an envelope follower or another explicit adapter is what
    /// crosses that boundary.
    NotAudio,
    /// The outlet is tapped downstream of its channel's effect chain, so it
    /// would arrive late by however much that chain costs. See
    /// [`OutletTap::is_upstream_of_chain`].
    TapIsLate,
    /// Resolving this edge would close a ring of them.
    Cycle,
}

/// What one channel's authored subscription came to.
///
/// A refusal keeps the subscription that caused it. `poly-synth-v2/` step 06
/// asks for exactly this -- "rejected visibly and retained as inspectable
/// orphan state" -- and it is the rule the modulation rack already follows for
/// a route to a departed module: a user who builds a cycle and then breaks it
/// gets the edge back rather than having to author it again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioEdge {
    Unsubscribed,
    Resolved {
        subscription: crate::AudioSubscription,
        /// Which buffer carries these samples.
        ///
        /// Assigned here rather than by the engine because this is the only
        /// place that sees every subscription at once, which is what
        /// deduplication needs: two channels reading the same outlet are one
        /// tap, not two copies of the same samples written twice.
        tap: u8,
    },
    Refused {
        subscription: crate::AudioSubscription,
        reason: EdgeRefusal,
    },
}

impl AudioEdge {
    /// The subscription that actually carries audio this generation.
    pub fn resolved(self) -> Option<crate::AudioSubscription> {
        match self {
            Self::Resolved { subscription, .. } => Some(subscription),
            _ => None,
        }
    }

    /// The buffer this edge reads, when it resolves to one.
    pub fn tap(self) -> Option<u8> {
        match self {
            Self::Resolved { tap, .. } => Some(tap),
            _ => None,
        }
    }

    /// Why this channel hears nothing, when it asked to hear something.
    pub fn refusal(self) -> Option<EdgeRefusal> {
        match self {
            Self::Refused { reason, .. } => Some(reason),
            _ => None,
        }
    }
}

/// The channels' audio edges and the order they force, as one value.
///
/// Fixed-capacity and `Copy` for the reason [`CompiledBusGraph`] is: the whole
/// plan crosses to the executor together, so it can never observe one
/// generation's edges against another's schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompiledAudioGraph {
    edges: [AudioEdge; MAX_CHANNELS],
    order: AudioOrder,
    /// The distinct (producer, outlet) pairs somebody reads, in the order
    /// their indices were handed out. This is the buffer table the engine
    /// allocates, and its length is what an unused feature costs: zero.
    taps: [Option<crate::AudioSubscription>; MAX_CHANNELS],
    tap_count: u16,
}

impl CompiledAudioGraph {
    pub fn edge(&self, channel: usize) -> AudioEdge {
        self.edges.get(channel).copied().unwrap_or(AudioEdge::Unsubscribed)
    }

    pub fn order(&self) -> &AudioOrder {
        &self.order
    }

    /// How many buffers the engine has to hold for this generation.
    pub fn tap_count(&self) -> usize {
        self.tap_count as usize
    }

    /// What tap `index` carries: whose outlet, and which one.
    pub fn tap(&self, index: usize) -> Option<crate::AudioSubscription> {
        self.taps.get(index).copied().flatten()
    }

    /// Every tap this generation needs, as `(index, producer, outlet)`.
    ///
    /// The engine walks this to prepare storage, and a producer walks it to
    /// find the buffers it owes -- which is why it is a list rather than a
    /// per-channel table: it is short, usually empty, and iterating it beats
    /// indexing a 256-entry array that is almost always all `None`.
    pub fn taps(&self) -> impl Iterator<Item = (usize, crate::AudioSubscription)> + '_ {
        self.taps[..self.tap_count as usize]
            .iter()
            .enumerate()
            .filter_map(|(index, tap)| tap.map(|tap| (index, tap)))
    }

    /// Whether any channel reads any other. The engine asks once per block to
    /// decide whether it needs the tap machinery at all, and the answer on
    /// every project that has never used an edge is `false`.
    pub fn is_empty(&self) -> bool {
        self.tap_count == 0
    }
}

impl Default for CompiledAudioGraph {
    fn default() -> Self {
        Self {
            edges: [AudioEdge::Unsubscribed; MAX_CHANNELS],
            order: identity_audio_order(),
            taps: [None; MAX_CHANNELS],
            tap_count: 0,
        }
    }
}

/// Channels in index order: the schedule a project with no edges compiles to,
/// and the one the engine has always used.
pub fn identity_audio_order() -> AudioOrder {
    let mut order = [0u8; MAX_CHANNELS];
    for (channel, slot) in order.iter_mut().enumerate() {
        *slot = channel as u8;
    }
    order
}

/// Compile the channels' subscriptions into the edges that resolve and the
/// order that satisfies them.
///
/// `published[i]` is what channel `i`'s generator declares -- the descriptor
/// slice rather than a bare "does it publish anything", because three of the
/// seven refusals above are properties of the individual outlet and could not
/// be told apart from a `bool`.
///
/// Total rather than fallible: a cycle refuses its own edges and every other
/// channel still renders, which is not true of [`compile_bus_graph`], where a
/// cyclic bank has no valid schedule at all. The difference is that a bus edge
/// carries the audio a channel has already made, so refusing one silences work
/// that was already done; an audio edge is the consumer's whole input, so
/// refusing one silences only the device that asked for it.
pub fn compile_audio_graph(
    subscriptions: &[Option<crate::AudioSubscription>],
    published: &[&'static [crate::OutletDescriptor]],
) -> CompiledAudioGraph {
    let mut edges = [AudioEdge::Unsubscribed; MAX_CHANNELS];
    let mut taps: [Option<crate::AudioSubscription>; MAX_CHANNELS] = [None; MAX_CHANNELS];
    let mut tap_count = 0usize;
    let live = subscriptions.len().min(MAX_CHANNELS);

    // Pass one: every subscription judged on its own, without reference to the
    // rest of the graph. Everything here is a property of one edge.
    for (consumer, subscription) in subscriptions.iter().take(live).enumerate() {
        let Some(subscription) = *subscription else {
            continue;
        };
        let refuse = |reason| AudioEdge::Refused {
            subscription,
            reason,
        };
        let source = subscription.channel as usize;
        edges[consumer] = if source == consumer {
            refuse(EdgeRefusal::SelfSubscribed)
        } else {
            match published.get(source) {
                None => refuse(EdgeRefusal::NoSuchChannel),
                // An empty published table: the channel exists and its
                // generator designs no outlets at all, which is usually a
                // device that was changed after the edge was authored.
                Some([]) => refuse(EdgeRefusal::NotAProducer),
                Some(outlets) => match crate::outlet::find(outlets, subscription.outlet) {
                    None => refuse(EdgeRefusal::NoSuchOutlet),
                    Some(outlet) if outlet.is_control() => refuse(EdgeRefusal::NotAudio),
                    Some(outlet) if !outlet.tap.is_upstream_of_chain() => {
                        refuse(EdgeRefusal::TapIsLate)
                    }
                    Some(_) => {
                        // One buffer per distinct pair. The scan is over the
                        // taps handed out so far, which is at most the number
                        // of consumers and is zero on every project that has
                        // never used an edge.
                        let existing = taps[..tap_count]
                            .iter()
                            .position(|tap| *tap == Some(subscription));
                        let tap = match existing {
                            Some(index) => index,
                            None => {
                                taps[tap_count] = Some(subscription);
                                tap_count += 1;
                                tap_count - 1
                            }
                        };
                        AudioEdge::Resolved {
                            subscription,
                            tap: tap as u8,
                        }
                    }
                },
            }
        };
    }

    // Pass two: Kahn over what survived. Every channel has at most one
    // incoming edge, so an index is either waiting for one producer or ready.
    let mut waiting = [false; MAX_CHANNELS];
    for (consumer, edge) in edges.iter().enumerate().take(live) {
        waiting[consumer] = edge.resolved().is_some();
    }


    let mut order = [0u8; MAX_CHANNELS];
    let mut emitted = 0usize;
    let mut queue = [0u8; MAX_CHANNELS];
    let (mut head, mut tail) = (0usize, 0usize);
    for (channel, blocked) in waiting.iter().enumerate().take(live) {
        if !*blocked {
            queue[tail] = channel as u8;
            tail += 1;
        }
    }
    while head < tail {
        let producer = queue[head];
        head += 1;
        order[emitted] = producer;
        emitted += 1;
        // Which channels were waiting for this one. Scanned rather than
        // indexed through a reverse adjacency table: a producer may have any
        // number of consumers, so the table would be quadratic storage to
        // save a quadratic scan that runs on the control thread over at most
        // 256 entries.
        for (consumer, edge) in edges.iter().enumerate().take(live) {
            if !waiting[consumer] {
                continue;
            }
            if edge.resolved().map(|s| s.channel) == Some(producer) {
                waiting[consumer] = false;
                queue[tail] = consumer as u8;
                tail += 1;
            }
        }
    }

    // Anything still waiting is in a ring. Refuse its edge -- it renders, in
    // index order, reading nothing.
    for channel in 0..live {
        if !waiting[channel] {
            continue;
        }
        if let AudioEdge::Resolved { subscription, .. } = edges[channel] {
            edges[channel] = AudioEdge::Refused {
                subscription,
                reason: EdgeRefusal::Cycle,
            };
        }
        order[emitted] = channel as u8;
        emitted += 1;
    }

    // Channels beyond the live prefix keep their index, so the order is always
    // a permutation of the whole bank whatever the project's channel count.
    for channel in live..MAX_CHANNELS {
        order[emitted] = channel as u8;
        emitted += 1;
    }
    debug_assert_eq!(emitted, MAX_CHANNELS);

    // A tap assigned to an edge that a cycle went on to refuse is a buffer
    // nothing reads, so the table is rebuilt from what survived. Doing it in
    // one pass at the end rather than un-assigning as refusals happen keeps
    // the indices dense, which is what lets the engine treat the count as the
    // number of buffers it needs.
    if edges.iter().any(|edge| edge.refusal() == Some(EdgeRefusal::Cycle)) {
        taps = [None; MAX_CHANNELS];
        tap_count = 0;
        for edge in edges.iter_mut().take(live) {
            let AudioEdge::Resolved { subscription, .. } = *edge else {
                continue;
            };
            let existing = taps[..tap_count]
                .iter()
                .position(|tap| *tap == Some(subscription));
            let tap = match existing {
                Some(index) => index,
                None => {
                    taps[tap_count] = Some(subscription);
                    tap_count += 1;
                    tap_count - 1
                }
            };
            *edge = AudioEdge::Resolved {
                subscription,
                tap: tap as u8,
            };
        }
    }

    CompiledAudioGraph {
        edges,
        order,
        taps,
        tap_count: tap_count as u16,
    }
}

#[cfg(test)]
mod tests {

    // --- The audio graph ---------------------------------------------------

    use crate::outlet::{OutletDescriptor, OutletTap};
    use crate::AudioSubscription;
    use crate::mod_metadata::SignalShape;

    /// A producer publishing one control outlet and two audio ones, which is
    /// the shape every refusal below is measured against.
    static PUBLISHER: &[OutletDescriptor] = &[
        OutletDescriptor::control(0, "Gate", SignalShape::Gate),
        OutletDescriptor::audio(1, "Osc 3", OutletTap::PreLevel),
        OutletDescriptor::audio(2, "Filter", OutletTap::PreVca),
    ];

    /// An outlet tapped at the device's finished output, which is downstream
    /// of the effect chain and therefore late.
    static LATE_PUBLISHER: &[OutletDescriptor] =
        &[OutletDescriptor::audio(1, "Out", OutletTap::Output)];

    /// A channel whose generator publishes nothing, which is most of them.
    static SILENT: &[OutletDescriptor] = &[];

    /// `count` channels, all publishing, none subscribed.
    fn bank(count: usize) -> Vec<&'static [OutletDescriptor]> {
        vec![PUBLISHER; count]
    }

    fn none(count: usize) -> Vec<Option<AudioSubscription>> {
        vec![None; count]
    }

    /// Where `channel` sits in the compiled order.
    fn position(graph: &CompiledAudioGraph, channel: u8) -> usize {
        graph
            .order()
            .iter()
            .position(|slot| *slot == channel)
            .expect("every channel is in the order")
    }

    /// The order a project with no edges compiles to is the order the engine
    /// has always used, which is what makes this step provably inaudible on
    /// its own rather than merely believed to be.
    #[test]
    fn no_edges_compiles_to_the_order_the_engine_already_walks() {
        let graph = compile_audio_graph(&none(8), &bank(8));
        assert_eq!(graph.order(), &identity_audio_order());
        assert!(graph.is_empty());
        assert_eq!(graph.edge(3), AudioEdge::Unsubscribed);
    }

    #[test]
    fn a_resolved_edge_puts_its_producer_first() {
        let mut subscriptions = none(4);
        // Channel 1 reads channel 3, so 3 has to render before 1 -- which is
        // the opposite of index order, or the test would prove nothing.
        subscriptions[1] = Some(AudioSubscription::new(3, 1));
        let graph = compile_audio_graph(&subscriptions, &bank(4));

        assert_eq!(graph.edge(1).resolved(), Some(AudioSubscription::new(3, 1)));
        assert!(!graph.is_empty());
        assert!(
            position(&graph, 3) < position(&graph, 1),
            "producer 3 rendered after consumer 1: {:?}",
            &graph.order()[..4]
        );
    }

    /// Three deep, because a one-edge sort can be satisfied by accident and a
    /// chain cannot.
    #[test]
    fn a_chain_of_three_sorts_end_to_end() {
        let mut subscriptions = none(4);
        subscriptions[0] = Some(AudioSubscription::new(1, 1));
        subscriptions[1] = Some(AudioSubscription::new(2, 1));
        let graph = compile_audio_graph(&subscriptions, &bank(4));
        assert!(position(&graph, 2) < position(&graph, 1));
        assert!(position(&graph, 1) < position(&graph, 0));
    }

    /// One producer, several consumers. Nothing about an edge is exclusive:
    /// reading an outlet does not consume it, and two subscribers do not
    /// affect each other.
    #[test]
    fn one_outlet_feeds_as_many_consumers_as_ask_for_it() {
        let mut subscriptions = none(4);
        subscriptions[0] = Some(AudioSubscription::new(3, 1));
        subscriptions[1] = Some(AudioSubscription::new(3, 1));
        subscriptions[2] = Some(AudioSubscription::new(3, 2));
        let graph = compile_audio_graph(&subscriptions, &bank(4));
        for consumer in 0..3u8 {
            assert!(
                graph.edge(consumer as usize).resolved().is_some(),
                "consumer {consumer} was refused"
            );
            assert!(position(&graph, 3) < position(&graph, consumer));
        }
        // Reading an outlet does not consume it, so one producer renders once
        // however many channels asked.
        assert_eq!(position(&graph, 3), 0);
    }

    /// Two consumers of one outlet are one buffer. The alternative -- a tap
    /// per consumer -- would have the producer write the same samples twice
    /// and cost storage proportional to how many are listening rather than to
    /// what is being listened to.
    #[test]
    fn two_consumers_of_one_outlet_share_a_tap() {
        let mut subscriptions = none(4);
        subscriptions[0] = Some(AudioSubscription::new(3, 1));
        subscriptions[1] = Some(AudioSubscription::new(3, 1));
        subscriptions[2] = Some(AudioSubscription::new(3, 2));
        let graph = compile_audio_graph(&subscriptions, &bank(4));

        assert_eq!(graph.tap_count(), 2, "one outlet was tapped twice");
        assert_eq!(graph.edge(0).tap(), graph.edge(1).tap());
        assert_ne!(graph.edge(0).tap(), graph.edge(2).tap());
        // The table says what each buffer carries, which is what the engine
        // hands the producer.
        let taps: Vec<_> = graph.taps().collect();
        assert_eq!(taps.len(), 2);
        assert!(taps.iter().all(|(_, tap)| tap.channel == 3));
    }

    /// Nothing subscribed is nothing allocated. This is the figure that
    /// matters -- an unused feature costing 448 KB a channel is the design
    /// this plan exists to avoid.
    #[test]
    fn a_project_with_no_edges_needs_no_buffers() {
        let graph = compile_audio_graph(&none(16), &bank(16));
        assert_eq!(graph.tap_count(), 0);
        assert_eq!(graph.taps().count(), 0);
    }

    /// A refused edge holds no buffer. It is easy to assign a tap while
    /// resolving and then forget it when a cycle takes the edge away, which
    /// would leave the engine holding a buffer nothing ever reads.
    #[test]
    fn a_refused_edge_gives_its_tap_back() {
        let mut subscriptions = none(4);
        // One edge that survives, and a ring that does not.
        subscriptions[0] = Some(AudioSubscription::new(3, 1));
        subscriptions[1] = Some(AudioSubscription::new(2, 1));
        subscriptions[2] = Some(AudioSubscription::new(1, 1));
        let graph = compile_audio_graph(&subscriptions, &bank(4));

        assert_eq!(graph.edge(1).refusal(), Some(EdgeRefusal::Cycle));
        assert_eq!(graph.edge(2).refusal(), Some(EdgeRefusal::Cycle));
        assert_eq!(graph.tap_count(), 1, "the ring kept its buffers");
        // And the survivor's index still points at its own tap: the table is
        // rebuilt dense, so an index is never stale.
        let tap = graph.edge(0).tap().expect("the surviving edge has a tap");
        assert_eq!(graph.tap(tap as usize), Some(AudioSubscription::new(3, 1)));
    }

    #[test]
    fn an_edge_is_judged_on_the_outlet_rather_than_the_channel() {
        let cases = [
            (AudioSubscription::new(9, 1), EdgeRefusal::NoSuchChannel),
            (AudioSubscription::new(0, 1), EdgeRefusal::SelfSubscribed),
            (AudioSubscription::new(1, 1), EdgeRefusal::NotAProducer),
            (AudioSubscription::new(2, 7), EdgeRefusal::NoSuchOutlet),
            (AudioSubscription::new(2, 0), EdgeRefusal::NotAudio),
            (AudioSubscription::new(3, 1), EdgeRefusal::TapIsLate),
        ];
        let published = vec![PUBLISHER, SILENT, PUBLISHER, LATE_PUBLISHER];
        for (subscription, expected) in cases {
            let mut subscriptions = none(4);
            subscriptions[0] = Some(subscription);
            let graph = compile_audio_graph(&subscriptions, &published);
            assert_eq!(
                graph.edge(0).refusal(),
                Some(expected),
                "{subscription:?} was not refused as {expected:?}"
            );
            // Retained, not deleted: the face has to be able to show what was
            // authored, and a user who fixes the cause gets their edge back.
            assert_eq!(
                graph.edge(0),
                AudioEdge::Refused {
                    subscription,
                    reason: expected
                }
            );
        }
    }

    /// A ring cannot render: whichever channel goes first is reading audio
    /// that has not been made yet. Both edges are refused rather than one
    /// being picked, because picking would make the sound depend on which
    /// channel happened to have the lower index.
    #[test]
    fn two_channels_reading_each_other_are_both_refused() {
        let mut subscriptions = none(4);
        subscriptions[1] = Some(AudioSubscription::new(2, 1));
        subscriptions[2] = Some(AudioSubscription::new(1, 1));
        let graph = compile_audio_graph(&subscriptions, &bank(4));
        assert_eq!(graph.edge(1).refusal(), Some(EdgeRefusal::Cycle));
        assert_eq!(graph.edge(2).refusal(), Some(EdgeRefusal::Cycle));
        assert!(graph.is_empty());
        // Everything else still renders, and the order is still a complete
        // permutation -- a cycle silences the devices that asked for it and
        // nothing else.
        assert_eq!(graph.edge(0), AudioEdge::Unsubscribed);
        let mut seen = graph.order().to_vec();
        seen.sort_unstable();
        assert_eq!(seen, identity_audio_order().to_vec());
    }

    /// Three deep, because a two-cycle is also `a == b` seen from either end
    /// and an equality check would catch it while walking straight past a
    /// longer ring.
    #[test]
    fn a_three_channel_ring_is_refused_whole() {
        let mut subscriptions = none(4);
        subscriptions[0] = Some(AudioSubscription::new(1, 1));
        subscriptions[1] = Some(AudioSubscription::new(2, 1));
        subscriptions[2] = Some(AudioSubscription::new(0, 1));
        let graph = compile_audio_graph(&subscriptions, &bank(4));
        for channel in 0..3 {
            assert_eq!(
                graph.edge(channel).refusal(),
                Some(EdgeRefusal::Cycle),
                "channel {channel} survived a ring"
            );
        }
    }

    /// The point of retaining a refusal: break the ring and the surviving
    /// edge resolves, without the user authoring anything again.
    #[test]
    fn breaking_a_ring_gives_the_other_edge_back() {
        let mut subscriptions = none(4);
        subscriptions[1] = Some(AudioSubscription::new(2, 1));
        subscriptions[2] = Some(AudioSubscription::new(1, 1));
        assert!(compile_audio_graph(&subscriptions, &bank(4)).is_empty());

        subscriptions[2] = None;
        let graph = compile_audio_graph(&subscriptions, &bank(4));
        assert_eq!(graph.edge(1).resolved(), Some(AudioSubscription::new(2, 1)));
        assert!(position(&graph, 2) < position(&graph, 1));
    }

    /// A refused edge must not reorder anything. It carries no audio, so a
    /// channel that only has a refused subscription is an ordinary channel.
    #[test]
    fn a_refused_edge_leaves_the_order_alone() {
        let mut subscriptions = none(4);
        subscriptions[0] = Some(AudioSubscription::new(3, 99));
        let graph = compile_audio_graph(&subscriptions, &bank(4));
        assert_eq!(graph.edge(0).refusal(), Some(EdgeRefusal::NoSuchOutlet));
        assert_eq!(graph.order(), &identity_audio_order());
    }

    /// The bank is 256 channels and a project has however many it has. The
    /// order is a permutation of the whole bank either way, so the engine can
    /// walk it without knowing where the project stops.
    #[test]
    fn a_short_project_still_compiles_a_whole_bank_order() {
        let mut subscriptions = none(3);
        subscriptions[0] = Some(AudioSubscription::new(2, 1));
        let graph = compile_audio_graph(&subscriptions, &bank(3));
        let mut seen = graph.order().to_vec();
        seen.sort_unstable();
        assert_eq!(seen, identity_audio_order().to_vec());
        assert_eq!(graph.edge(200), AudioEdge::Unsubscribed);
    }
    use super::*;

    /// A bank the size of the whole address space, for the tests that are
    /// about the *compiler* rather than about how many tracks a project has.
    /// `default_buses` no longer returns one, and that is the point of it.
    fn full_bank() -> Vec<BusSetup> {
        (0..MAX_BUSES).map(BusSetup::new).collect()
    }

    /// A new song has one track, and it is the master.
    ///
    /// This used to assert the opposite -- seventeen, every index
    /// materialised -- which was a fixed bank wearing a `Vec` and is what
    /// `docs/CAPACITY_POLICY.md` calls dimensioning by a ceiling.
    #[test]
    fn a_new_bank_is_the_master_alone() {
        let buses = default_buses();
        assert_eq!(buses.len(), 1);
        assert_eq!(buses[MASTER_BUS as usize].bus.name, "Master");
        assert_eq!(buses[MASTER_BUS as usize].bus.output, MASTER_BUS);
    }

    /// The master is not optional: a bank without one has nowhere to send
    /// audio, so an empty or hand-emptied file gets one back.
    #[test]
    fn a_bank_always_has_a_master() {
        assert_eq!(sanitize_bank(&[]).len(), 1);
        assert_eq!(sanitize_bank(&[])[0].bus.name, "Master");
    }

    /// A route naming a track that is not there lands on the master, the same
    /// repair an out-of-range one gets. Without this a bank could be shorter
    /// than one of its own edges.
    #[test]
    fn a_route_to_a_track_that_is_not_there_falls_back() {
        let mut bank = full_bank();
        bank[1].bus.output = 9;
        bank.truncate(3);
        let repaired = sanitize_bank(&bank);
        assert_eq!(repaired.len(), 3);
        assert_eq!(repaired[1].bus.output, MASTER_BUS);
    }

    /// A short bank still compiles, and every track in it is rendered before
    /// what it feeds. The absent indices are phantoms feeding the master;
    /// what must not happen is a real track being left out of the order.
    #[test]
    fn a_short_bank_still_sorts_every_track_it_has() {
        let mut bank = full_bank();
        bank[1].bus.output = 2;
        bank.truncate(3);
        let graph = compile_bus_graph(&bank).expect("a short bank sorts");
        let order = graph.render_order();
        let position = |bus: u8| order.iter().position(|slot| *slot == bus).unwrap();
        assert!(position(1) < position(2), "1 feeds 2 and must render first");
        assert!(position(2) < position(MASTER_BUS));
        for track in 0..bank.len() as u8 {
            assert!(order.contains(&track), "track {track} was left out of the order");
        }
    }

    #[test]
    fn a_bus_may_address_any_other_bus() {
        assert!(is_legal_route(3, 7), "uphill routing is a legal edge now");
        assert!(is_legal_route(7, 3));
        assert!(is_legal_route(7, MASTER_BUS));
        assert!(!is_legal_route(7, 7), "a bus cannot feed itself");
        assert!(!is_legal_route(MASTER_BUS, 3), "the master is a sink");
        assert!(!is_legal_route(MAX_BUSES as u8, 0));
        assert!(!is_legal_route(3, MAX_BUSES as u8));
    }

    #[test]
    fn individually_nonsensical_routes_fall_back_to_the_master() {
        assert_eq!(sanitize_route(3, 7), 7, "uphill is no longer rewritten");
        assert_eq!(sanitize_route(3, 3), MASTER_BUS);
        assert_eq!(sanitize_route(3, MAX_BUSES as u8), MASTER_BUS);
    }

    fn routed(edges: &[(usize, u8)]) -> Vec<BusSetup> {
        let mut buses = full_bank();
        for (bus, output) in edges {
            buses[*bus].bus.output = *output;
        }
        buses
    }

    // --- Sends -----------------------------------------------------------

    /// `full_bank()` with sends added, as `(from, target)` pairs.
    fn sending(edges: &[(usize, u8)]) -> Vec<BusSetup> {
        let mut buses = full_bank();
        for (from, target) in edges {
            buses[*from].sends.push(AuxSend::new(*target));
        }
        buses
    }

    /// A send is a second outgoing edge, so it constrains the schedule the
    /// same way an output does. Without this the track a send feeds could
    /// render *before* the track sending to it and be a block late, which is
    /// silence on the first block and a smeared tail after.
    #[test]
    fn a_send_orders_its_target_after_its_source() {
        let bank = sending(&[(1, 5)]);
        let graph = compile_bus_graph(&bank).expect("a send is not a cycle");
        let order = graph.render_order();
        let position = |bus: u8| order.iter().position(|slot| *slot == bus).unwrap();
        assert!(
            position(1) < position(5),
            "1 sends to 5 and must render first"
        );
        // And the outputs are untouched: a send is an addition, not a move.
        assert_eq!(graph.destination(1), MASTER_BUS);
        assert_eq!(graph.destination(5), MASTER_BUS);
    }

    /// The one Adam asked about: a send from a track back into something that
    /// already reaches it. It is refused rather than delayed, which is the
    /// whole answer to "or it'd feedback like crazy" -- there is no return
    /// object to get this wrong with.
    #[test]
    fn a_send_that_would_loop_is_refused() {
        // 3 feeds 4, so a send from 4 back to 3 closes the ring.
        let bank = routed(&[(3, 4)]);
        assert!(would_create_cycle(&bank, 4, 3));
        // The other way round is fine: 3 already reaches 4, and a send along
        // the way it already goes adds no loop.
        assert!(!would_create_cycle(&bank, 3, 4));
        // Reachability now follows sends as well as outputs, which is the
        // half a single-successor walk could not see: 3 -> 4 by output,
        // 4 -> 6 by send, so a send from 6 to 3 loops.
        let mut bank = routed(&[(3, 4)]);
        bank[4].sends.push(AuxSend::new(6));
        assert!(would_create_cycle(&bank, 6, 3), "a loop closed through a send");
    }

    /// A bank whose sends loop has no valid schedule, and the repair is the
    /// one a looping output already gets: it opens and plays.
    #[test]
    fn a_cyclic_send_has_no_plan_and_is_repaired() {
        let mut bank = full_bank();
        bank[1].sends.push(AuxSend::new(2));
        bank[2].sends.push(AuxSend::new(1));
        assert!(compile_bus_graph(&bank).is_none());
        let repaired = sanitize_bank(&bank);
        assert!(repaired[1].sends.is_empty());
        assert!(repaired[2].sends.is_empty());
        assert!(compile_bus_graph(&repaired).is_some());
    }

    /// A send naming a track that is not there is dropped, where an *output*
    /// naming one falls back to the master. The asymmetry is deliberate: a
    /// channel with nowhere to go is silently unheard, and a send with
    /// nowhere to go is simply not a send.
    #[test]
    fn a_send_to_a_track_that_is_gone_is_dropped() {
        let mut bank = full_bank();
        bank.truncate(3);
        bank[1].sends.push(AuxSend::new(9));
        bank[1].sends.push(AuxSend::new(1));
        bank[1].sends.push(AuxSend::new(2));
        let repaired = sanitize_bank(&bank);
        assert_eq!(repaired[1].sends.len(), 1, "only the reachable one survives");
        assert_eq!(repaired[1].sends[0].target, 2);
    }

    /// A send is enabled and at unity when it is made. A send you have just
    /// created and cannot hear is indistinguishable from one that did not
    /// work.
    #[test]
    fn a_new_send_is_audible() {
        let send = AuxSend::new(4);
        assert!(send.enabled);
        assert_eq!(send.level, 1.0);
        assert_eq!(send.tap, SendTap::PostFader);
    }

    /// A song written before sends existed loads with none, plays identically,
    /// and does not have to be migrated. The defaulted-field rule
    /// `docs/PROJECT_FORMAT.md` asks for, stated where it can fail.
    #[test]
    fn a_track_written_before_sends_existed_loads_with_none() {
        let manifest = r#"
            [bus]
            name = "Drums"
            muted = false
            volume = 1.0
            pan = 0.0
            output = 0
        "#;
        let setup: BusSetup = toml::from_str(manifest).expect("a pre-sends track still loads");
        assert!(setup.sends.is_empty());
    }

    /// A hand-written send without an `enabled` passes audio. The other
    /// default would be a send that is routed, drawn, and silent for a reason
    /// nothing states.
    #[test]
    fn a_send_without_an_enabled_flag_is_on() {
        let manifest = r#"
            [bus]
            name = "Drums"
            muted = false
            volume = 1.0
            pan = 0.0
            output = 0

            [[sends]]
            target = 3
            level = 0.5
        "#;
        let setup: BusSetup = toml::from_str(manifest).expect("a minimal send loads");
        assert_eq!(setup.sends.len(), 1);
        assert!(setup.sends[0].enabled);
        assert_eq!(setup.sends[0].tap, SendTap::PostFader);
    }

    /// The starter song's last third: a Reverb track two others send to, and
    /// nothing anywhere that says "return".
    #[test]
    fn the_starter_song_has_a_reverb_send() {
        let project = crate::Project::starter_kit(7);
        let reverb = project
            .buses
            .iter()
            .position(|track| track.bus.name == "Reverb")
            .expect("the starter song has a reverb track");
        let feeding: Vec<usize> = project
            .buses
            .iter()
            .enumerate()
            .filter(|(_, track)| track.sends.iter().any(|send| send.target as usize == reverb))
            .map(|(index, _)| index)
            .collect();
        assert_eq!(feeding.len(), 2, "two tracks send to it");
        assert!(
            project.buses[reverb].effects.len() == 1,
            "the return carries the device that makes it one"
        );
        // Fully wet, because the dry path is already in the mix through each
        // track's own output.
        assert_eq!(project.buses[reverb].effects[0].wet_dry, 1.0);
        // And it is an ordinary track in every other way -- nothing marks it.
        assert_eq!(project.buses[reverb].bus.output, MASTER_BUS);
        assert!(compile_bus_graph(&project.buses).is_some());
    }

    /// Sends flatten in bank order, which is the contract between the plan's
    /// answer and the engine's bank: entry `n` here is entry `n` there.
    #[test]
    fn sends_flatten_in_bank_order() {
        let bank = sending(&[(1, 5), (3, 5), (1, 2)]);
        let edges = send_edges(&bank);
        assert_eq!(
            edges,
            vec![
                SendEdge { producer: EffectTarget::Bus(1), target: 5 },
                SendEdge { producer: EffectTarget::Bus(1), target: 2 },
                SendEdge { producer: EffectTarget::Bus(3), target: 5 },
            ]
        );
    }

    // --- Latency compensation ------------------------------------------

    /// The reverb-send case, which is what the default song is waiting on: a
    /// track sends to a track carrying a Drive, and both reach the master.
    /// The dry path has to wait for the wet one or the two are fifteen frames
    /// apart at the summing point where they meet again.
    #[test]
    fn a_dry_path_waits_for_the_send_it_will_meet_again() {
        let bank = sending(&[(1, 2)]);
        let graph = compile_bus_graph(&bank).expect("a send is not a cycle");
        let mut bus_latency = [0u32; MAX_BUSES];
        bus_latency[2] = 15;
        let sends = send_edges(&bank);
        let plan = compile_latency(&graph, &[0], &[1], &bus_latency, &sends);

        assert_eq!(plan.bus(1), 15, "the dry path waits for the wet one");
        assert_eq!(plan.bus(2), 0, "the wet path is the longest and does not");
        assert_eq!(plan.send(0), 0, "nothing else feeds the send's target");
        assert_eq!(plan.total(), 15);
    }

    /// Two sends into one track, from strips of different length. The shorter
    /// one waits by the difference -- the general rule, now applied at a
    /// summing point that only sends reach.
    #[test]
    fn two_sends_into_one_track_align() {
        let bank = sending(&[(1, 3), (2, 3)]);
        let graph = compile_bus_graph(&bank).expect("two sends are not a cycle");
        // Channel 0 carries a Drive and feeds track 1; channel 1 is clean and
        // feeds track 2.
        let sends = send_edges(&bank);
        let plan = compile_latency(&graph, &[15, 0], &[1, 2], &[0; MAX_BUSES], &sends);

        assert_eq!(plan.send(0), 0, "the long strip's send sets the arrival");
        assert_eq!(plan.send(1), 15, "the short strip's send waits for it");
        assert_eq!(plan.send_count(), 2);
    }

    /// A producer's two edges are owed different delays, which is the whole
    /// reason `CompiledLatency` grew a per-send answer. Track 1 sends to a
    /// track nothing else reaches, and outputs into one something long does.
    #[test]
    fn a_producers_two_edges_are_compensated_apart() {
        let mut bank = sending(&[(1, 3)]);
        bank[2].bus.output = MASTER_BUS;
        let graph = compile_bus_graph(&bank).expect("acyclic");
        // Channel 1 carries a Drive into the master, which track 1 also feeds.
        let sends = send_edges(&bank);
        let plan = compile_latency(&graph, &[0, 15], &[1, MASTER_BUS], &[0; MAX_BUSES], &sends);

        assert_eq!(plan.bus(1), 15, "its output waits for the Drive at the master");
        assert_eq!(plan.send(0), 0, "its send reaches a point nothing else does");
    }

    /// A send from a channel is known before any track renders, the same way
    /// its output is, so it needs no ordering and gets the same arrival.
    #[test]
    fn a_channels_send_carries_its_chain() {
        let bank = full_bank();
        let graph = compile_bus_graph(&bank).expect("acyclic");
        let sends = [SendEdge {
            producer: EffectTarget::Channel(0),
            target: 4,
        }];
        // Channel 0 has a Drive; channel 1 is clean and feeds the same track.
        let plan = compile_latency(&graph, &[15, 0], &[4, 4], &[0; MAX_BUSES], &sends);

        assert_eq!(plan.channel(0), 0, "the long channel sets the arrival");
        assert_eq!(plan.channel(1), 15);
        assert_eq!(plan.send(0), 0, "its send leaves from the same moment");
    }

    /// The plan for a flat bank: `channels` channels, all on the master,
    /// carrying the given latencies and no bus latency anywhere.
    fn flat(channel_latency: &[u32]) -> CompiledLatency {
        let graph = compile_bus_graph(&default_buses()).expect("a default bank is acyclic");
        let on_master = vec![MASTER_BUS; channel_latency.len()];
        compile_latency(&graph, channel_latency, &on_master, &[], &[])
    }

    /// The case that is silently wrong today: one channel carries a Drive and
    /// its neighbour does not, so they sum at the master fifteen frames apart.
    /// After compensation the shorter one waits and both arrive together.
    #[test]
    fn a_shorter_channel_waits_for_its_longer_sibling() {
        let plan = flat(&[0, 15, 0]);
        assert_eq!(plan.channel(0), 15);
        assert_eq!(plan.channel(1), 0, "the longest path must not move");
        assert_eq!(plan.channel(2), 15);
        // Alignment is not free: everything now arrives fifteen frames late,
        // which is the figure a host would be told.
        assert_eq!(plan.total(), 15);
    }

    /// Nothing to align is nothing to do. A bank where every path is the same
    /// length compensates nobody, which is what keeps the common project from
    /// paying for a delay it does not need.
    #[test]
    fn an_aligned_bank_compensates_nothing() {
        let plan = flat(&[0, 0, 0, 0]);
        assert!((0..4).all(|channel| plan.channel(channel) == 0));
        assert_eq!(plan.total(), 0);
        // And an equal non-zero latency everywhere is still aligned: what
        // matters is the difference, not the amount.
        let plan = flat(&[15, 15, 15]);
        assert!((0..3).all(|channel| plan.channel(channel) == 0));
        assert_eq!(plan.total(), 15);
    }

    /// A bus's own chain pushes everything that feeds it further out, so the
    /// channels on a *different* bus have to wait for it. This is the case a
    /// per-channel-only scheme gets wrong, and it is why the pass walks the
    /// compiled render order rather than looking at channels alone.
    #[test]
    fn a_buss_own_latency_delays_the_paths_beside_it() {
        let graph = compile_bus_graph(&default_buses()).expect("acyclic");
        // Channel 0 goes through bus 1, which carries fifteen frames of its
        // own; channel 1 goes straight to the master with nothing.
        let mut bus_latency = vec![0; MAX_BUSES];
        bus_latency[1] = 15;
        let plan = compile_latency(&graph, &[0, 0], &[1, MASTER_BUS], &bus_latency, &[]);

        // Nothing else feeds bus 1, so its own input needs no delay.
        assert_eq!(plan.channel(0), 0);
        // But bus 1 arrives at the master fifteen frames late, so the direct
        // channel waits for it.
        assert_eq!(plan.channel(1), 15);
        assert_eq!(plan.bus(1), 0, "the longest path into the master must not move");
        assert_eq!(plan.total(), 15);
    }

    /// Two buses into the master with different depths: the shallower bus is
    /// delayed, and so is everything that feeds it -- through the bus rather
    /// than on top of it, which is what stops a channel being compensated
    /// twice for the same deficit.
    #[test]
    fn a_shallower_bus_is_delayed_once_and_not_its_inputs_again() {
        let graph = compile_bus_graph(&default_buses()).expect("acyclic");
        let mut bus_latency = vec![0; MAX_BUSES];
        bus_latency[1] = 20;
        // Channel 0 into the deep bus 1; channel 1 into the shallow bus 2.
        let plan = compile_latency(&graph, &[0, 0], &[1, 2], &bus_latency, &[]);

        assert_eq!(plan.bus(1), 0, "the deepest bus must not move");
        assert_eq!(plan.bus(2), 20, "the shallow bus waits for the deep one");
        // Each channel is the only thing feeding its bus, so neither is
        // compensated at its own summing point. The correction happens once,
        // on bus 2's edge into the master.
        assert_eq!(plan.channel(0), 0);
        assert_eq!(plan.channel(1), 0);
        assert_eq!(plan.total(), 20);
    }

    /// A chain of buses accumulates, and the pass has to see through it: the
    /// render order guarantees bus 2 is finished before bus 1 reads it, which
    /// is the property this reuses rather than rebuilding.
    #[test]
    fn latency_accumulates_along_a_chain_of_buses() {
        // 2 -> 1 -> master, each adding ten frames.
        let graph = compile_bus_graph(&routed(&[(2, 1)])).expect("acyclic");
        let mut bus_latency = vec![0; MAX_BUSES];
        bus_latency[1] = 10;
        bus_latency[2] = 10;
        // Channel 0 at the top of the chain, channel 1 straight to master.
        let plan = compile_latency(&graph, &[0, 0], &[2, MASTER_BUS], &bus_latency, &[]);

        // Channel 0 travels 10 (bus 2) + 10 (bus 1) = 20 frames.
        assert_eq!(plan.total(), 20);
        assert_eq!(plan.channel(1), 20, "the direct channel waits for the chain");
        assert_eq!(plan.channel(0), 0);
        assert_eq!(plan.bus(2), 0);
        assert_eq!(plan.bus(1), 0);
    }

    /// A repaired bank still compiles a plan. `compile_bus_graph` sends an
    /// out-of-range edge to the master, and the latency pass must agree with
    /// it rather than reading the original -- a channel compensated against a
    /// summing point it does not sum into would be worse than no compensation.
    #[test]
    fn a_repaired_bank_still_compiles_a_coherent_plan() {
        let graph = compile_bus_graph(&routed(&[(3, MAX_BUSES as u8)])).expect("repairable");
        assert_eq!(graph.destination(3), MASTER_BUS);
        let mut bus_latency = vec![0; MAX_BUSES];
        bus_latency[3] = 15;
        // A channel naming a bus that does not exist lands on the master, the
        // same way the executor clamps it.
        let plan = compile_latency(&graph, &[0, 0], &[3, 250], &bus_latency, &[]);
        assert_eq!(plan.channel(1), 15, "the clamped channel waits for bus 3");
        assert_eq!(plan.total(), 15);
    }

    /// An empty project is a plan too, and asking for a channel or bus outside
    /// what was handed in answers zero rather than panicking: the executor
    /// reads this by index for all 256 channels whatever the project holds.
    #[test]
    fn an_empty_bank_compiles_to_no_compensation() {
        let plan = CompiledLatency::default();
        assert_eq!(plan.total(), 0);
        assert_eq!(plan.channel(MAX_CHANNELS + 10), 0);
        assert_eq!(plan.bus(MAX_BUSES + 10), 0);

        let graph = compile_bus_graph(&default_buses()).expect("acyclic");
        let empty = compile_latency(&graph, &[], &[], &[], &[]);
        assert_eq!(empty, CompiledLatency::default());
    }

    /// The whole point of the permutation: whatever the routing, a bus is
    /// rendered before the bus it feeds. Assert that property directly rather
    /// than pinning one expected ordering, since several are valid.
    #[test]
    fn the_compiled_order_puts_every_bus_before_its_destination() {
        // Deliberately uphill: 2 -> 5 -> 9 -> master, which the old
        // lower-numbered-only rule could not express at all.
        let buses = routed(&[(2, 5), (5, 9), (9, MASTER_BUS), (4, 2)]);
        let order = compile_render_order(&buses).expect("acyclic graph should sort");

        let mut position = [usize::MAX; MAX_BUSES];
        for (slot, bus) in order.iter().enumerate() {
            position[*bus as usize] = slot;
        }
        assert!(
            position.iter().all(|slot| *slot != usize::MAX),
            "every bus must appear exactly once"
        );
        for (index, setup) in buses.iter().enumerate() {
            if index == MASTER_BUS as usize {
                continue;
            }
            assert!(
                position[index] < position[setup.bus.output as usize],
                "bus {index} must render before bus {}",
                setup.bus.output
            );
        }
        assert_eq!(
            position[MASTER_BUS as usize],
            MAX_BUSES - 1,
            "everything drains to the master, so it renders last"
        );
    }

    #[test]
    fn a_fresh_bank_sorts() {
        let order = compile_render_order(&default_buses()).expect("default bank is acyclic");
        assert_eq!(order[MAX_BUSES - 1], MASTER_BUS);
    }

    #[test]
    fn a_short_bank_compiles_to_a_complete_plan() {
        let buses = vec![BusSetup::new(MASTER_BUS as usize), BusSetup::new(1)];
        let graph = compile_bus_graph(&buses).expect("padded default bank is acyclic");
        let mut seen = [false; MAX_BUSES];
        for &bus in graph.render_order() {
            assert!(!seen[bus as usize], "bus {bus} appeared twice");
            seen[bus as usize] = true;
        }
        assert!(seen.into_iter().all(|present| present));
        assert_eq!(graph.destination(MAX_BUSES - 1), MASTER_BUS);
    }

    #[test]
    fn malformed_edges_are_repaired_inside_the_compiled_plan() {
        let buses = routed(&[(3, MAX_BUSES as u8), (7, 7)]);
        let graph = compile_bus_graph(&buses).expect("individual bad edges are repairable");
        assert_eq!(graph.destination(3), MASTER_BUS);
        assert_eq!(graph.destination(7), MASTER_BUS);
        assert_eq!(graph.render_order()[MAX_BUSES - 1], MASTER_BUS);
    }

    #[test]
    fn a_cycle_has_no_order() {
        assert!(compile_render_order(&routed(&[(3, 5), (5, 3)])).is_none());
        assert!(
            compile_render_order(&routed(&[(1, 2), (2, 3), (3, 1)])).is_none(),
            "a longer loop is still a loop"
        );
        // A loop off to the side must not be excused by the rest of the bank
        // sorting cleanly.
        assert!(compile_render_order(&routed(&[(7, 8), (8, 7)])).is_none());
    }

    #[test]
    fn a_cycle_is_predicted_before_it_is_applied() {
        let buses = routed(&[(5, 2)]);
        // 5 already feeds 2, so pointing 2 at 5 would close the loop.
        assert!(would_create_cycle(&buses, 2, 5));
        // Anything that does not lead back to 2 is fine.
        assert!(!would_create_cycle(&buses, 2, 9));
        assert!(!would_create_cycle(&buses, 2, MASTER_BUS));
        // Reaching a bus that merely shares a destination is not a cycle.
        assert!(!would_create_cycle(&routed(&[(4, 6), (5, 6)]), 4, 5));
    }

    #[test]
    fn the_fallback_order_is_itself_valid() {
        let order = default_render_order();
        assert_eq!(order[MAX_BUSES - 1], MASTER_BUS);
        let mut seen = [false; MAX_BUSES];
        for bus in order {
            assert!(!seen[bus as usize], "bus {bus} appears twice");
            seen[bus as usize] = true;
        }
    }
}
