//! The Bus Comp insert: the master's bus compressor as a device of its own
//! (MOO-216).
//!
//! **Not a second compressor.** It holds the master section's own
//! [`BusComp`] and translates ids, so the insert and the master's built-in
//! section run one implementation and cannot come to sound different. Its
//! ids are the master's moved down ([`mooloop_core::bus_comp_master_id`]),
//! and it is always switched in: an insert's in/out is its rail's bypass,
//! which is the host's, not the node's.
//!
//! Everything the master section's module says about the laws, the voicings
//! and why a knob reads a marking holds here unchanged; see
//! [`crate::strip::bus_comp`].

use mooloop_core::{bus_comp_master_id, BusCompParams};

use super::{process_param_split, scrub_non_finite, RangeProcessor};
use crate::bus::StereoBus;
use crate::event::EventList;
use crate::node::{AudioNode, DynamicsFrame, ProcessContext};
use crate::strip::bus_comp::BusComp;

pub struct BusCompEffect {
    comp: BusComp,
    sample_rate: u32,
}

impl BusCompEffect {
    pub fn new(params: BusCompParams, sample_rate: u32) -> Self {
        Self {
            comp: BusComp::new(params.section(), sample_rate),
            sample_rate,
        }
    }
}

impl RangeProcessor for BusCompEffect {
    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        self.comp.process_range(bus, start, end);
    }

    fn apply_param(&mut self, id: u32, value: f32) {
        // An id past the table would land on the master's retired 56, or
        // on nothing; the section refuses both.
        if id < mooloop_core::BUS_COMP_PARAM_COUNT {
            self.comp.apply_param(bus_comp_master_id(id), value);
        }
    }
}

impl AudioNode for BusCompEffect {
    /// Zero, and asserted against `EffectKind::BusComp`: the detector reads
    /// the sample it is turning down, with no lookahead.
    fn latency_frames(&self) -> u32 {
        0
    }

    /// Nothing here stores audio -- the output is the input times a gain --
    /// so what has to settle is the followers, the RMS power and the three
    /// smoothed controls, for the compressor effect's reason: a node frozen
    /// mid-release would wake holding a reduction the music stopped asking
    /// for. They snap onto rest rather than decaying forever.
    ///
    /// No `tail_frames`: the default (unbounded) is right, because the host
    /// skips on a tail *or* on rest, and a tail of zero would let it freeze a
    /// release halfway -- inaudible in the silence, and then applied to the
    /// next hit.
    fn is_at_rest(&self) -> bool {
        self.comp.is_at_rest()
    }

    fn dynamics_frame(&self) -> Option<DynamicsFrame> {
        self.comp.dynamics_frame()
    }

    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        if ctx.sample_rate != self.sample_rate {
            self.sample_rate = ctx.sample_rate;
            self.comp.set_sample_rate(ctx.sample_rate);
        }
        self.comp.begin_block();
        let frames = ctx.frames.min(bus.capacity());
        process_param_split(self, bus, events_in, frames);
        // A NaN in the RMS detector's running power would stay there for
        // good (MOO-176). It comes out as silence, and the section starts
        // released.
        if scrub_non_finite(bus, frames) {
            self.comp.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, TimedEvent};
    use mooloop_core::strip::BusCompVoicing;
    use mooloop_core::{
        BUS_COMP_PARAM_GRIP_ATTACK, BUS_COMP_PARAM_THRESHOLD_DB, BUS_COMP_PARAM_VOICING,
    };

    const RATE: u32 = 48_000;
    const BLOCK: usize = 256;

    /// A kick-like burst every quarter second over a steady low tone: enough
    /// to make every voicing attack and release.
    fn drums(frame: usize) -> f32 {
        let t = frame as f32 / RATE as f32;
        let beat = (frame % (RATE as usize / 4)) as f32 / RATE as f32;
        let kick = (-beat * 30.0).exp() * (std::f32::consts::TAU * 60.0 * beat).sin();
        0.8 * kick + 0.2 * (std::f32::consts::TAU * 110.0 * t).sin()
    }

    fn ctx(frames: usize) -> ProcessContext {
        ProcessContext {
            sample_rate: RATE,
            frames,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        }
    }

    /// The drum signal from `from`, on two buses: one for each side of a
    /// comparison.
    fn two_buses(from: usize) -> (StereoBus, StereoBus) {
        let mut a = StereoBus::with_capacity(BLOCK);
        let mut b = StereoBus::with_capacity(BLOCK);
        for i in 0..BLOCK {
            let x = drums(from + i);
            a.l[i] = x;
            a.r[i] = 0.9 * x;
            b.l[i] = x;
            b.r[i] = 0.9 * x;
        }
        (a, b)
    }

    /// **The insert is the master section, sample for sample** (MOO-216):
    /// the same settings on the same input, through the node and through a
    /// bare `BusComp` switched in, come out bit-identical under every
    /// voicing -- the claim "one implementation" makes, as an assertion.
    #[test]
    fn the_insert_is_the_master_section_to_the_bit() {
        for voicing in BusCompVoicing::ALL {
            let params = BusCompParams {
                voicing,
                threshold_db: -24.0,
                makeup_db: 3.0,
                mix: 0.8,
                ..BusCompParams::default()
            };
            let mut insert = BusCompEffect::new(params, RATE);
            let mut master = BusComp::new(params.section(), RATE);
            let mut reduced = 0.0f32;
            for block in 0..(RATE as usize / BLOCK) {
                let (mut a, mut b) = two_buses(block * BLOCK);
                insert.process(&ctx(BLOCK), &mut a, &EventList::empty(), None);
                master.process_block(&mut b, BLOCK);
                assert_eq!(a.l, b.l, "{voicing:?}, block {block}");
                assert_eq!(a.r, b.r, "{voicing:?}, block {block}");
                assert_eq!(insert.dynamics_frame(), master.dynamics_frame());
                reduced = reduced.max(master.reduction_db());
            }
            assert!(reduced > 3.0, "{voicing:?} never compressed: {reduced} dB");
        }
    }

    /// A parameter event mid-block lands where it says, exactly as the
    /// master section takes the same id at the same frame -- and the block's
    /// meter reading spans both halves.
    #[test]
    fn a_parameter_event_lands_on_its_frame_under_the_masters_id() {
        let params = BusCompParams::default();
        let mut insert = BusCompEffect::new(params, RATE);
        let mut master = BusComp::new(params.section(), RATE);
        let mut events = EventList::empty();
        let moves = [
            (40, BUS_COMP_PARAM_THRESHOLD_DB, -35.0),
            (90, BUS_COMP_PARAM_GRIP_ATTACK, 0.0),
            (200, BUS_COMP_PARAM_VOICING, 2.0),
        ];
        for (offset, id, value) in moves {
            events.push(TimedEvent { offset, event: Event::ParamValue { id, value } });
        }
        let (mut a, mut b) = two_buses(0);
        insert.process(&ctx(BLOCK), &mut a, &events, None);
        master.begin_block();
        let mut at = 0;
        for (offset, id, value) in moves {
            master.process_range(&mut b, at, offset as usize);
            master.apply_param(bus_comp_master_id(id), value);
            at = offset as usize;
        }
        master.process_range(&mut b, at, BLOCK);
        assert_eq!(a.l, b.l);
        assert_eq!(a.r, b.r);
        assert_eq!(insert.dynamics_frame(), master.dynamics_frame());
    }

    /// A NaN comes out as silence and leaves nothing behind: the next clean
    /// block is what a fresh insert makes of it.
    #[test]
    fn a_non_finite_block_is_scrubbed_and_forgotten() {
        let params = BusCompParams { voicing: BusCompVoicing::Punch, ..BusCompParams::default() };
        let mut insert = BusCompEffect::new(params, RATE);
        let mut poisoned = StereoBus::with_capacity(BLOCK);
        poisoned.l[3] = f32::NAN;
        insert.process(&ctx(BLOCK), &mut poisoned, &EventList::empty(), None);
        assert!(poisoned.l.iter().chain(&poisoned.r).all(|x| x.is_finite()));
        let mut fresh = BusCompEffect::new(params, RATE);
        let (mut a, mut b) = two_buses(0);
        insert.process(&ctx(BLOCK), &mut a, &EventList::empty(), None);
        fresh.process(&ctx(BLOCK), &mut b, &EventList::empty(), None);
        assert_eq!(a.l, b.l);
    }

    #[test]
    fn silence_comes_to_rest() {
        let mut insert = BusCompEffect::new(BusCompParams::default(), RATE);
        let mut loud = StereoBus::with_capacity(BLOCK);
        loud.l.fill(0.9);
        loud.r.fill(0.9);
        insert.process(&ctx(BLOCK), &mut loud, &EventList::empty(), None);
        assert!(!insert.is_at_rest(), "a reduction in flight is not rest");
        for _ in 0..(RATE as usize * 20 / BLOCK) {
            let mut quiet = StereoBus::with_capacity(BLOCK);
            insert.process(&ctx(BLOCK), &mut quiet, &EventList::empty(), None);
        }
        assert!(insert.is_at_rest(), "twenty seconds of silence did not settle it");
    }
}
