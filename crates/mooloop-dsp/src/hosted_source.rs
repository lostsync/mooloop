//! A channel source the engine did not build: a hosted plugin instrument
//! (`docs/plans/plugin-hosting/`, step 09, MOO-84).
//!
//! The strip holds one boxed [`SourceNode`] (MOO-56), and the eight native
//! generators are built by `build_source` from their kind. A plugin cannot be
//! built there -- the host crate opens it, on the control thread, and the
//! session swaps its processor in -- so a plugin channel's slot holds this
//! wrapper instead. It names the song's plugin slot, and runs whatever
//! processor it has been handed as the channel's instrument.
//!
//! **Without one it is silent.** That is both the moment between a song
//! opening and the rack swapping the processor in, and the missing-instrument
//! placeholder: a plugin that is not installed, not found, or refusing to
//! load plays nothing while its slot, lanes and routes are kept
//! (`00-status.md`, "Failure"). An effect's placeholder passes its input
//! through; an instrument has no input, so its placeholder's answer is
//! silence.
//!
//! The processor is swapped in and out of the wrapper rather than the
//! wrapper being replaced, so the swap is one pointer move on the audio
//! thread and the channel's source slot never changes kind under a command
//! addressed to it ([`SourceNode::host_processor`]).
//!
//! **A swap under a sounding processor fades (MOO-230).** A plugin restart
//! or a sample-rate change pulls the processor out, and a replacement goes
//! in; either is a step in the channel's output if it lands in one sample.
//! So the executor asks [`SourceNode::ready_for_processor_swap`] before it
//! applies one, and this source fades its output to silence first -- the
//! ~35 ms, 100 ms-at-most rule MOO-213 gave a hosted effect's swap -- and
//! fades back in with whatever processor arrives. A swap into a source that
//! was never asked to fade (the first install, an export's) starts at full
//! level: nothing was heard to step from.
//!
//! **The notes the outgoing processor held end with its fade.** They are not
//! sent to the incoming one: it is a new instance with no voices, and
//! re-striking a note half-way through would be an attack nobody played. The
//! releases that later arrive for them are addressed to ids the incoming
//! processor never started, which a processor ignores (`ClapProcessor` finds
//! no held note and sends nothing).

use mooloop_core::{DeviceKind, GeneratorParams, PluginSlotId};

use crate::bus::StereoBus;
use crate::event::EventList;
use crate::node::{
    AudioNode, ControlCurve, Discontinuity, HostedNode, HostedParam, ProcessContext, SourceNode,
};
use crate::taps::AudioTaps;

/// How long the swap fade takes each way: the time MOO-213's hosted effect
/// swap takes to fade its slot out of the path (a 5 ms one-pole to -60 dB,
/// about 35 ms), as a straight line that arrives at silence exactly.
const SWAP_FADE_S: f32 = 0.035;

/// The longest a swap waits for its fade before going anyway, as a removal
/// does (the engine's `REMOVAL_MAX_WAIT_S`). The fade only runs while the
/// strip renders the source, and a strip asleep is silent already.
const SWAP_MAX_WAIT_S: f32 = 0.1;

/// A channel's hosted instrument: the plugin slot it plays, and the
/// processor it plays it with, if the rack has swapped one in.
pub struct HostedSource {
    slot: PluginSlotId,
    node: Option<HostedNode>,
    /// The level the output is at, 0..=1: 1 but for a swap fade.
    gain: f32,
    /// Frames a pending swap has waited for its fade-out, while one does.
    swap_waited: Option<u32>,
    /// The rate the source last rendered at; 0 before its first block.
    sample_rate: u32,
}

impl HostedSource {
    /// The silent source for `slot`: what a song opening builds, and what a
    /// missing plugin stays.
    pub fn new(slot: PluginSlotId) -> Self {
        Self {
            slot,
            node: None,
            gain: 1.0,
            swap_waited: None,
            sample_rate: 0,
        }
    }

    /// `slot` already running `node`: what the session installs when it has
    /// just opened the plugin itself.
    pub fn with_processor(slot: PluginSlotId, node: HostedNode) -> Self {
        Self {
            node: Some(node),
            ..Self::new(slot)
        }
    }

    /// The `Project::plugins` slot this source plays.
    pub fn slot(&self) -> PluginSlotId {
        self.slot
    }

    /// Whether a processor is running here, rather than silence.
    pub fn is_hosted(&self) -> bool {
        self.node.is_some()
    }

    /// Move the swap fade along one block and apply it to `bus`: down while
    /// a swap waits, up otherwise. Level 1 and not leaving touches nothing,
    /// so a source that never swaps under sound is bit-identical to one
    /// without the fade. With no processor the level holds where the fade
    /// left it: rising over silence would let the next processor arrive at
    /// full level, which is the step the fade is for.
    fn apply_fade(&mut self, bus: &mut StereoBus, frames: usize) {
        let leaving = self.swap_waited.is_some();
        if self.node.is_none() || (!leaving && self.gain >= 1.0) {
            return;
        }
        let step = 1.0 / (SWAP_FADE_S * self.sample_rate.max(1) as f32);
        let step = if leaving { -step } else { step };
        for frame in 0..frames {
            self.gain = (self.gain + step).clamp(0.0, 1.0);
            bus.l[frame] *= self.gain;
            bus.r[frame] *= self.gain;
        }
    }
}

impl AudioNode for HostedSource {
    fn tail_frames(&self) -> u32 {
        self.node.as_ref().map_or(0, |node| node.tail_frames())
    }

    fn is_at_rest(&self) -> bool {
        self.node.as_ref().is_none_or(|node| node.is_at_rest())
    }

    fn latency_frames(&self) -> u32 {
        self.node.as_ref().map_or(0, |node| node.latency_frames())
    }

    fn hosted_param(&self, id: u32) -> Option<HostedParam> {
        self.node.as_ref().and_then(|node| node.hosted_param(id))
    }

    fn on_discontinuity(&mut self, kind: Discontinuity) {
        if let Some(node) = self.node.as_mut() {
            node.on_discontinuity(kind);
        }
    }

    fn skip_block(&mut self, ctx: &ProcessContext) {
        if let Some(node) = self.node.as_mut() {
            node.skip_block(ctx);
        }
    }

    /// The processor's own curve path, when it has one; the fallback events
    /// otherwise, which is the default's answer too.
    fn apply_curves(
        &mut self,
        curves: &[ControlCurve<'_>],
        tick_frames: usize,
        fallback: &mut EventList,
    ) -> u64 {
        match self.node.as_mut() {
            Some(node) => node.apply_curves(curves, tick_frames, fallback),
            None => 0,
        }
    }

    /// The processor renders onto a cleared bus: an instrument has no input,
    /// and whatever the strip's bus held from its last block is not one.
    /// Every event the channel carries -- notes, chokes, parameter values --
    /// reaches it at its own frame, as it reaches a native generator.
    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        events_out: Option<&mut EventList>,
    ) {
        let frames = ctx.frames.min(bus.capacity());
        bus.clear(frames);
        self.sample_rate = ctx.sample_rate;
        if let Some(node) = self.node.as_mut() {
            node.process(ctx, bus, events_in, events_out);
        }
        self.apply_fade(bus, frames);
    }
}

impl SourceNode for HostedSource {
    fn process_source(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _source: Option<&StereoBus>,
        _ports: &mut AudioTaps<'_>,
    ) {
        self.process(ctx, bus, events_in, None);
    }

    fn kind(&self) -> DeviceKind {
        DeviceKind::Plugin
    }

    /// A native block is refused, as every source refuses another kind's.
    /// A plugin block is this source's when it names the same slot, or when
    /// this one was built from its kind alone (`EngineCommand::SetChannelSource`
    /// knows only the kind) and has neither a slot nor a processor yet.
    fn set_generator_params(&mut self, params: &GeneratorParams) -> bool {
        let GeneratorParams::Plugin(slot) = *params else {
            return false;
        };
        if slot == self.slot {
            return true;
        }
        if !self.slot.is_assigned() && self.node.is_none() {
            self.slot = slot;
            return true;
        }
        false
    }

    fn generator_params(&self) -> GeneratorParams {
        GeneratorParams::Plugin(self.slot)
    }

    fn host_processor(
        &mut self,
        slot: PluginSlotId,
        node: Option<HostedNode>,
    ) -> Result<Option<HostedNode>, Option<HostedNode>> {
        if slot != self.slot || !slot.is_assigned() {
            return Err(node);
        }
        // Whatever arrives fades back in from where the fade-out left the
        // level (silence, after a held swap; full, after an unheld one).
        self.swap_waited = None;
        Ok(std::mem::replace(&mut self.node, node))
    }

    /// Fade out under a sounding processor before it leaves (MOO-230). A
    /// swap this source will refuse, one into silence (no processor), and
    /// one before the source has ever rendered go at once and fade nothing.
    fn ready_for_processor_swap(&mut self, slot: PluginSlotId, frames: usize) -> bool {
        if slot != self.slot || !slot.is_assigned() || self.node.is_none() || self.sample_rate == 0
        {
            return true;
        }
        let waited = self
            .swap_waited
            .map_or(0, |waited| waited.saturating_add(frames as u32));
        self.swap_waited = Some(waited);
        if self.gain <= 0.0 {
            return true;
        }
        if waited >= (SWAP_MAX_WAIT_S * self.sample_rate as f32) as u32 {
            // Not rendered while it waited: asleep, so silent. The incoming
            // processor still fades in from here.
            self.gain = 0.0;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, TimedEvent};

    fn ctx(frames: usize) -> ProcessContext {
        ProcessContext {
            sample_rate: 48_000,
            frames,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        }
    }

    /// Writes 1.0 from each note-on's frame to the end of the block, adding
    /// to what the bus held, so a test sees both the clear and the timing.
    struct Step;

    impl AudioNode for Step {
        fn process(
            &mut self,
            ctx: &ProcessContext,
            bus: &mut StereoBus,
            events_in: &EventList,
            _events_out: Option<&mut EventList>,
        ) {
            for event in events_in.iter() {
                if let Event::NoteOn { .. } = event.event {
                    for frame in event.offset as usize..ctx.frames {
                        bus.l[frame] += 1.0;
                        bus.r[frame] += 1.0;
                    }
                }
            }
        }
    }

    fn note_at(offset: u32) -> EventList {
        let mut events = EventList::empty();
        assert!(events.push_ordered(TimedEvent {
            offset,
            event: Event::NoteOn {
                id: 1,
                note: 60,
                velocity: 100,
            },
        }));
        events
    }

    #[test]
    fn without_a_processor_it_is_silent_and_at_rest() {
        let mut source = HostedSource::new(PluginSlotId(2));
        let mut bus = StereoBus::with_capacity(8);
        bus.l[..8].fill(0.5);
        bus.r[..8].fill(0.5);
        source.process_source(&ctx(8), &mut bus, &note_at(0), None, &mut AudioTaps::none());
        assert!(bus.l[..8].iter().chain(&bus.r[..8]).all(|&s| s == 0.0));
        assert!(source.is_at_rest());
        assert_eq!(source.tail_frames(), 0);
        assert_eq!(source.kind(), DeviceKind::Plugin);
        assert_eq!(source.generator_params(), GeneratorParams::Plugin(PluginSlotId(2)));
    }

    #[test]
    fn a_processor_plays_notes_at_their_frame_on_a_cleared_bus() {
        let mut source = HostedSource::with_processor(PluginSlotId(2), Box::new(Step));
        let mut bus = StereoBus::with_capacity(8);
        bus.l[..8].fill(0.5);
        source.process_source(&ctx(8), &mut bus, &note_at(3), None, &mut AudioTaps::none());
        assert_eq!(bus.l[..8], [0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
        assert!(!source.is_at_rest(), "a node that has not opted in is never at rest");
    }

    #[test]
    fn a_processor_goes_only_into_its_own_slot_and_comes_back_out() {
        let mut source = HostedSource::new(PluginSlotId(2));
        let refused = source.host_processor(PluginSlotId(3), Some(Box::new(Step)));
        assert!(matches!(refused, Err(Some(_))), "another slot's processor is handed back");
        let displaced = source.host_processor(PluginSlotId(2), Some(Box::new(Step)));
        assert!(matches!(displaced, Ok(None)));
        assert!(source.is_hosted());
        let pulled = source.host_processor(PluginSlotId(2), None);
        assert!(matches!(pulled, Ok(Some(_))), "a pull-back hands the processor back");
        assert!(!source.is_hosted());
    }

    /// Writes a constant 1.0, notes or not: a drone, so a fade is the only
    /// thing that moves the output.
    struct Drone;

    impl AudioNode for Drone {
        fn process(
            &mut self,
            ctx: &ProcessContext,
            bus: &mut StereoBus,
            _events_in: &EventList,
            _events_out: Option<&mut EventList>,
        ) {
            bus.l[..ctx.frames].fill(1.0);
            bus.r[..ctx.frames].fill(1.0);
        }
    }

    fn block(source: &mut HostedSource, frames: usize) -> Vec<f32> {
        let mut bus = StereoBus::with_capacity(frames);
        let empty = EventList::empty();
        source.process_source(&ctx(frames), &mut bus, &empty, None, &mut AudioTaps::none());
        bus.l[..frames].to_vec()
    }

    /// MOO-230: asked before a swap, a sounding source ramps to silence in
    /// about 35 ms and only then says yes; the processor that arrives ramps
    /// back up over the same time. No sample steps by more than one ramp
    /// increment.
    #[test]
    fn a_swap_under_a_sounding_processor_fades_out_and_the_next_fades_in() {
        let slot = PluginSlotId(2);
        let mut source = HostedSource::with_processor(slot, Box::new(Drone));
        let block_frames = 64;
        assert!(block(&mut source, block_frames).iter().all(|&s| s == 1.0));
        let fade = (SWAP_FADE_S * 48_000.0) as usize;
        let mut out = Vec::new();
        let mut asks = 0;
        while !source.ready_for_processor_swap(slot, block_frames) {
            asks += 1;
            out.extend(block(&mut source, block_frames));
        }
        let heard = asks * block_frames;
        assert!(
            heard >= fade && heard < fade + 2 * block_frames,
            "the swap waited {heard} frames for a {fade}-frame fade"
        );
        assert_eq!(*out.last().unwrap(), 0.0, "the swap lands on silence");
        assert!(source.host_processor(slot, Some(Box::new(Drone))).is_ok());
        for _ in 0..(fade / block_frames + 2) {
            out.extend(block(&mut source, block_frames));
        }
        assert_eq!(*out.last().unwrap(), 1.0, "the incoming processor is back at full level");
        let largest = out.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(largest <= 1.5 / fade as f32, "a step of {largest}");
    }

    /// The first processor into a source that has never been asked to fade
    /// plays at full level from its first frame, as an export's install
    /// does; and a pull-back from silence goes at once.
    #[test]
    fn a_swap_with_nothing_sounding_goes_at_once_and_fades_nothing() {
        let slot = PluginSlotId(2);
        let mut source = HostedSource::new(slot);
        assert!(source.ready_for_processor_swap(slot, 64), "no processor: nothing to fade");
        assert!(source.host_processor(slot, Some(Box::new(Drone))).is_ok());
        assert!(
            source.ready_for_processor_swap(PluginSlotId(3), 64),
            "another slot's swap is refused anyway, so it fades nothing"
        );
        assert!(block(&mut source, 64).iter().all(|&s| s == 1.0));
    }

    /// A restart pulls the processor out and sends the new one later: the
    /// silence between must not raise the level again, or the new processor
    /// would arrive at full level in one sample.
    #[test]
    fn a_processor_arriving_after_a_pull_out_fades_in_however_late() {
        let slot = PluginSlotId(2);
        let mut source = HostedSource::with_processor(slot, Box::new(Drone));
        block(&mut source, 64);
        while !source.ready_for_processor_swap(slot, 64) {
            block(&mut source, 64);
        }
        assert!(matches!(source.host_processor(slot, None), Ok(Some(_))));
        for _ in 0..100 {
            assert!(block(&mut source, 64).iter().all(|&s| s == 0.0));
        }
        assert!(source.host_processor(slot, Some(Box::new(Drone))).is_ok());
        assert!(block(&mut source, 64)[0] < 0.01, "the late processor fades in");
    }

    /// A source that stops being rendered while its swap waits (a strip
    /// that fell asleep) goes after 100 ms, and what arrives fades in.
    #[test]
    fn a_swap_that_is_not_rendered_goes_after_the_longest_wait() {
        let slot = PluginSlotId(2);
        let mut source = HostedSource::with_processor(slot, Box::new(Drone));
        block(&mut source, 64);
        let limit = (SWAP_MAX_WAIT_S * 48_000.0) as usize;
        let mut waited = 0;
        while !source.ready_for_processor_swap(slot, 64) {
            waited += 64;
            assert!(waited <= limit + 64, "held past the longest wait");
        }
        assert!(waited + 64 >= limit);
        assert!(source.host_processor(slot, Some(Box::new(Drone))).is_ok());
        assert!(block(&mut source, 64)[0] < 0.01, "the incoming processor fades in");
    }

    #[test]
    fn a_source_built_from_its_kind_takes_the_first_slot_it_is_given_and_no_other() {
        let mut source = HostedSource::new(PluginSlotId::UNASSIGNED);
        assert!(source.host_processor(PluginSlotId::UNASSIGNED, None).is_err());
        assert!(source.set_generator_params(&GeneratorParams::Plugin(PluginSlotId(4))));
        assert_eq!(source.slot(), PluginSlotId(4));
        assert!(source.set_generator_params(&GeneratorParams::Plugin(PluginSlotId(4))));
        assert!(!source.set_generator_params(&GeneratorParams::Plugin(PluginSlotId(5))));
        assert!(!source.set_generator_params(&DeviceKind::Sampler.default_generator_params()));
    }
}
