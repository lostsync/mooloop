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
        }
    }
}

/// A bus plus the effect chain inserted on it, mirroring `ChannelSetup`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BusSetup {
    pub bus: MixerBus,
    #[serde(default)]
    pub effects: Vec<EffectSlotState>,
}

impl BusSetup {
    pub fn new(index: usize) -> Self {
        Self {
            bus: MixerBus::new(index),
            effects: Vec::new(),
        }
    }
}

/// The full bus bank a project starts with. Every index exists whether or not
/// anything feeds it, so assigning a channel to bus 12 never has to create one.
pub fn default_buses() -> Vec<BusSetup> {
    (0..MAX_BUSES).map(BusSetup::new).collect()
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

/// Whether `from` reaches `target` by following outputs. Bounded by the bank
/// size, so a graph that is already cyclic terminates instead of spinning.
fn reaches(buses: &[BusSetup], from: u8, target: u8) -> bool {
    let mut at = from;
    for _ in 0..MAX_BUSES {
        if at == target {
            return true;
        }
        if at == MASTER_BUS {
            return false;
        }
        match buses.get(at as usize) {
            Some(setup) => at = setup.bus.output,
            None => return false,
        }
    }
    false
}

/// Whether routing `bus` into `output` would close a loop. The interface uses
/// this to decline the connection rather than offering it and then silently
/// rewriting it to something the user did not ask for.
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

    // Number of buses feeding each bus. Channels are not counted: they are all
    // rendered before any bus, so they constrain nothing.
    let mut feeding = [0u8; MAX_BUSES];
    for &destination in destinations.iter().skip(1) {
        feeding[destination as usize] += 1;
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
        let destination = destinations[node as usize] as usize;
        feeding[destination] -= 1;
        if feeding[destination] == 0 {
            queue[tail] = destination as u8;
            tail += 1;
        }
    }

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

/// What each producer must be delayed by so that everything summing at a
/// point arrives from the same moment.
///
/// One number per channel and per bus, in the same fixed-capacity shape and
/// for the same reason as [`CompiledBusGraph`]: the whole plan crosses to the
/// executor by value, so it can never observe one generation's delays against
/// another's edges. What allocates is the delay *storage* the engine sizes
/// from this, never this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompiledLatency {
    channels: [u32; MAX_CHANNELS],
    buses: [u32; MAX_BUSES],
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
            total: 0,
        }
    }
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
/// longest one -- collapsed to the tree the mixer actually is. Each producer
/// has exactly one destination, so "per edge" and "per producer" are the same
/// thing and the simpler one is honest until step 6 makes them differ.
///
/// One descending pass over `graph.render_order()`, which is already
/// topological: every bus feeding `b` is visited before `b`, so `b`'s input
/// arrival is complete by the time it is read. No recursion, no second sort,
/// and no allocation.
pub fn compile_latency(
    graph: &CompiledBusGraph,
    channel_latency: &[u32],
    channel_bus: &[u8],
    bus_latency: &[u32],
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

    let mut arrival = [0u32; MAX_BUSES];
    for &bus in graph.render_order() {
        let bus = bus as usize;
        arrival[bus] = input_arrival[bus] + at(bus_latency, bus);
        if bus == MASTER_BUS as usize {
            continue;
        }
        let destination = graph.destination(bus) as usize;
        input_arrival[destination] = input_arrival[destination].max(arrival[bus]);
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

    CompiledLatency {
        channels: channels_out,
        buses: buses_out,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bank_is_master_plus_every_insert() {
        let buses = default_buses();
        assert_eq!(buses.len(), MAX_BUSES);
        assert_eq!(buses[MASTER_BUS as usize].bus.name, "Master");
        assert_eq!(buses[1].bus.name, "Bus 1");
        assert_eq!(buses[INSERT_BUSES].bus.name, format!("Bus {INSERT_BUSES}"));
        assert!(buses.iter().all(|setup| setup.bus.output == MASTER_BUS));
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
        let mut buses = default_buses();
        for (bus, output) in edges {
            buses[*bus].bus.output = *output;
        }
        buses
    }

    // --- Latency compensation ------------------------------------------

    /// The plan for a flat bank: `channels` channels, all on the master,
    /// carrying the given latencies and no bus latency anywhere.
    fn flat(channel_latency: &[u32]) -> CompiledLatency {
        let graph = compile_bus_graph(&default_buses()).expect("a default bank is acyclic");
        let on_master = vec![MASTER_BUS; channel_latency.len()];
        compile_latency(&graph, channel_latency, &on_master, &[])
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
        let plan = compile_latency(&graph, &[0, 0], &[1, MASTER_BUS], &bus_latency);

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
        let plan = compile_latency(&graph, &[0, 0], &[1, 2], &bus_latency);

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
        let plan = compile_latency(&graph, &[0, 0], &[2, MASTER_BUS], &bus_latency);

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
        let plan = compile_latency(&graph, &[0, 0], &[3, 250], &bus_latency);
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
        let empty = compile_latency(&graph, &[], &[], &[]);
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
