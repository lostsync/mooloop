//! Mixer buses: the destinations channels feed, and the second thing (after a
//! channel) that can own an effect chain.
//!
//! The model is FL Studio's: every sequencer channel names one bus, buses can
//! feed other buses, and everything eventually reaches the master. Two rules
//! keep that graph cheap enough to walk on the realtime thread:
//!
//! 1. The master is bus 0. It always exists and its own `output` is ignored.
//! 2. A bus may only route to a *lower-numbered* bus. Bus 7 can feed bus 3 or
//!    the master; it cannot feed bus 9.
//!
//! Rule 2 is what makes the graph acyclic by construction. The engine renders
//! buses in descending index order and every bus's destination is guaranteed
//! to be rendered after it, so no topological sort, cycle check, or scratch
//! allocation is needed in the audio callback. The cost is that a pair of
//! buses cannot feed each other — which is a feedback loop, not a routing.

use crate::EffectSlotState;

/// Insert buses available in addition to the master.
pub const INSERT_BUSES: usize = 16;

/// Total addressable buses: the master plus every insert.
pub const MAX_BUSES: usize = INSERT_BUSES + 1;

/// Index of the master bus. Channels and buses default to feeding it.
pub const MASTER_BUS: u8 = 0;

/// Where an effect chain lives. Effect commands address a target rather than a
/// channel so one set of install/remove/param messages serves both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// Destination bus index. Must be lower than this bus's own index; the
    /// master's value is unused.
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

/// Whether `bus` may legally send to `output`, per the descending-order rule.
/// The master has no output, so nothing is legal for it.
pub fn is_legal_route(bus: u8, output: u8) -> bool {
    bus != MASTER_BUS && (bus as usize) < MAX_BUSES && output < bus
}

/// Coerce a possibly-invalid routing (an older or hand-edited file) into a
/// legal one by falling back to the master.
pub fn sanitize_route(bus: u8, output: u8) -> u8 {
    if is_legal_route(bus, output) {
        output
    } else {
        MASTER_BUS
    }
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

    /// The descending-order invariant is the whole reason the render loop can
    /// be a single pass, so pin it: only strictly-lower destinations pass.
    #[test]
    fn routing_is_only_ever_downhill() {
        assert!(is_legal_route(7, 3));
        assert!(is_legal_route(7, MASTER_BUS));
        assert!(!is_legal_route(7, 7), "a bus cannot feed itself");
        assert!(!is_legal_route(3, 7), "uphill routing would allow a cycle");
        assert!(!is_legal_route(MASTER_BUS, 0), "the master has no output");
        assert!(!is_legal_route(MAX_BUSES as u8, 0));
    }

    #[test]
    fn illegal_routes_fall_back_to_the_master() {
        assert_eq!(sanitize_route(7, 3), 3);
        assert_eq!(sanitize_route(3, 7), MASTER_BUS);
        assert_eq!(sanitize_route(3, 3), MASTER_BUS);
    }
}
