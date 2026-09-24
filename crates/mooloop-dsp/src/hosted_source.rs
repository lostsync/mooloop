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

use mooloop_core::{DeviceKind, GeneratorParams, PluginSlotId};

use crate::bus::StereoBus;
use crate::event::EventList;
use crate::node::{
    AudioNode, ControlCurve, Discontinuity, HostedNode, HostedParam, ProcessContext, SourceNode,
};
use crate::taps::AudioTaps;

/// A channel's hosted instrument: the plugin slot it plays, and the
/// processor it plays it with, if the rack has swapped one in.
pub struct HostedSource {
    slot: PluginSlotId,
    node: Option<HostedNode>,
}

impl HostedSource {
    /// The silent source for `slot`: what a song opening builds, and what a
    /// missing plugin stays.
    pub fn new(slot: PluginSlotId) -> Self {
        Self { slot, node: None }
    }

    /// `slot` already running `node`: what the session installs when it has
    /// just opened the plugin itself.
    pub fn with_processor(slot: PluginSlotId, node: HostedNode) -> Self {
        Self {
            slot,
            node: Some(node),
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
        if let Some(node) = self.node.as_mut() {
            node.process(ctx, bus, events_in, events_out);
        }
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
        Ok(std::mem::replace(&mut self.node, node))
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
