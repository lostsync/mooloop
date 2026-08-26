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
    /// Linear output volume in [0, 1].
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
