//! The node a container occupies its rack slot with.
//!
//! **A container is not really a DSP device, and this is not really a DSP
//! node.** Its children are rows of the same chain, and the chain's own loop
//! already runs them in order, so there is nothing here to process: the
//! container's work -- keeping a copy of its input and crossfading it back in
//! at the end of the run -- belongs to the host, beside the per-slot dry path
//! that already exists (`docs/plans/containers/03-the-chain-mixes.md`).
//!
//! What this exists for is that the engine's chain is an array of nodes
//! indexed by position, and a container takes one of those positions. Giving
//! it a transparent node means the container becomes addressable, bypassable,
//! saveable and installable without the realtime loop learning anything new,
//! which is what lets step 02 land silent and step 03 be about the mix alone.

use mooloop_core::ChainParams;

use crate::bus::StereoBus;
use crate::event::EventList;
use crate::node::{AudioNode, ProcessContext};

/// A container's slot occupant: transparent, stateless, and always at rest.
pub struct ContainerEffect {
    params: ChainParams,
}

impl ContainerEffect {
    pub fn new(params: ChainParams) -> Self {
        Self { params }
    }

    pub fn set_params(&mut self, params: ChainParams) {
        self.params = params;
    }

    pub fn params(&self) -> ChainParams {
        self.params
    }
}

impl AudioNode for ContainerEffect {
    /// Nothing is retained, so silence in is silence out immediately. The
    /// host still only skips the slot once its input has gone quiet, which
    /// for a transparent node is exactly right.
    fn tail_frames(&self) -> u32 {
        0
    }

    fn is_at_rest(&self) -> bool {
        true
    }

    /// Zero, and it stays zero when the mix arrives in step 03: a container's
    /// declared latency is not the sum of its children's, because those
    /// children are rows of the same chain and `chain_latency` already counts
    /// them. See question 4 in `docs/plans/containers/README.md`.
    fn latency_frames(&self) -> u32 {
        0
    }

    fn process(
        &mut self,
        _ctx: &ProcessContext,
        _bus: &mut StereoBus,
        _events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::ProcessContext;

    fn context(frames: usize) -> ProcessContext {
        ProcessContext {
            sample_rate: 48_000,
            frames,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        }
    }

    /// The claim step 02 rests on, stated where it can fail: a container
    /// passes its input through untouched, whatever its mix says. The mix is
    /// step 03's, and until then a container that changed the sound would
    /// leave that step's null test nothing to stand on.
    #[test]
    fn a_container_is_transparent_at_every_mix() {
        for mix in [0.0, 0.25, 0.5, 1.0] {
            let mut node = ContainerEffect::new(ChainParams { children: 2, mix });
            let mut bus = StereoBus::with_capacity(64);
            for frame in 0..64 {
                bus.l[frame] = frame as f32 / 64.0;
                bus.r[frame] = -(frame as f32) / 64.0;
            }
            let before = (bus.l.clone(), bus.r.clone());
            node.process(&context(64), &mut bus, &EventList::empty(), None);
            assert_eq!((bus.l, bus.r), before, "a container moved the signal at mix {mix}");
        }
    }
}
