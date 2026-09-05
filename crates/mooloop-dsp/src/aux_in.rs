//! Aux In: the generator whose sound is another channel's published audio
//! outlet.
//!
//! Almost nothing happens here, and that is the point. The work of the typed
//! audio edge is the compiled order in `mooloop_core::compile_audio_graph` and
//! the tap the engine hands both ends; this device is a level and a copy. What
//! makes it worth building is the edge underneath it rather than the breadth
//! of the device on top.

use crate::bus::StereoBus;
use crate::event::{Event, EventList};
use crate::node::{AudioNode, ProcessContext};
use crate::smooth::Smoothed;
use crate::taps::AudioTaps;
use mooloop_core::aux_in::{self, AuxInParams};

/// This device's own tap number: its single audio outlet is the first entry
/// in its declared audio run.
pub const TAP_OUT: usize = 0;

/// Level smoothing time. Short enough that a lane step lands where it was
/// drawn, long enough that it is a ramp rather than a click.
const LEVEL_SMOOTH_S: f32 = 0.005;

pub struct AuxIn {
    params: AuxInParams,
    level: Smoothed,
}

impl AuxIn {
    pub fn new(params: AuxInParams, sample_rate: u32) -> Self {
        Self {
            params,
            level: Smoothed::new(params.level, LEVEL_SMOOTH_S, sample_rate),
        }
    }

    pub fn set_params(&mut self, params: AuxInParams) {
        self.params = params;
        self.level.set_target(params.level);
    }

    pub fn params(&self) -> &AuxInParams {
        &self.params
    }

    /// Jump the level to where it was authored, with nothing to click.
    pub fn reset(&mut self) {
        self.level.reset_to(self.params.level);
    }

    fn apply_param(&mut self, id: u32, value: f32) {
        if aux_in::set(&mut self.params, id, value) && id == aux_in::PARAM_LEVEL {
            self.level.set_target(self.params.level);
        }
    }

    /// Render one block from `source`, and publish the result to this
    /// device's own tap.
    ///
    /// `source` is the tap buffer the compiled graph resolved this edge to,
    /// already filled this block because the producer rendered first --
    /// that ordering is the whole content of the plan this device closes.
    /// `None` means the subscription did not resolve, and an Aux In reading
    /// nothing is silent rather than holding its last block.
    ///
    /// It **publishes even when it is reading nothing**, for the same reason
    /// a producer with no active voices writes silence rather than stale
    /// samples: a downstream Aux In must hear the break in the chain.
    pub fn process_from(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        source: Option<&StereoBus>,
        events_in: &EventList,
        taps: &mut AudioTaps,
    ) {
        let mut frames = ctx.frames.min(bus.capacity());
        if let Some(source) = source {
            frames = frames.min(source.capacity());
        }
        let mut pos = 0usize;
        for event in events_in.iter() {
            let offset = (event.offset as usize).min(frames).max(pos);
            self.render_range(bus, source, pos, offset, taps);
            if let Event::ParamValue { id, value } = event.event {
                self.apply_param(id, value);
            }
            pos = offset;
        }
        self.render_range(bus, source, pos, frames, taps);
    }

    fn render_range(
        &mut self,
        bus: &mut StereoBus,
        source: Option<&StereoBus>,
        start: usize,
        end: usize,
        taps: &mut AudioTaps,
    ) {
        if start >= end {
            return;
        }
        // The level still advances with no source, so that reconnecting one
        // does not find the smoother parked where a lane left it before the
        // edge was broken.
        let Some(source) = source else {
            self.level.advance_by(end - start);
            if let Some(tap) = taps.port(TAP_OUT) {
                for index in start..end {
                    tap.l[index] = 0.0;
                    tap.r[index] = 0.0;
                }
            }
            return;
        };
        if let Some(tap) = taps.port(TAP_OUT) {
            // Written and summed in one pass, so the tap is what the channel
            // heard rather than a second computation of it.
            for index in start..end {
                let level = self.level.advance();
                let (left, right) = (source.l[index] * level, source.r[index] * level);
                tap.l[index] = left;
                tap.r[index] = right;
                bus.l[index] += left;
                bus.r[index] += right;
            }
            return;
        }
        for index in start..end {
            let level = self.level.advance();
            bus.l[index] += source.l[index] * level;
            bus.r[index] += source.r[index] * level;
        }
    }
}

impl AudioNode for AuxIn {
    /// An Aux In outside a channel strip has no edge to read, so it renders
    /// silence. The engine calls [`Self::process_from`] instead; this exists
    /// so the device fits the trait every other generator implements rather
    /// than becoming a special case in the node vocabulary.
    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        self.process_from(ctx, bus, None, events_in, &mut AudioTaps::none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::TimedEvent;
    use mooloop_core::{audio_tap_index, DeviceKind, PublishesOutlets};

    const SR: u32 = 48_000;

    fn ctx(frames: usize) -> ProcessContext {
        ProcessContext {
            sample_rate: SR,
            frames,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        }
    }

    fn ramp(frames: usize) -> StereoBus {
        let mut bus = StereoBus::with_capacity(frames);
        for index in 0..frames {
            bus.l[index] = index as f32 / frames as f32;
            bus.r[index] = -(index as f32) / frames as f32;
        }
        bus
    }

    fn unity() -> AuxInParams {
        AuxInParams {
            level: 1.0,
            ..AuxInParams::default()
        }
    }

    /// The tap number a device indexes its port group by is derived from the
    /// declared outlet order, so the constant above and the table have to
    /// agree or the samples go to the wrong buffer.
    #[test]
    fn the_declared_outlet_order_is_the_tap_numbering() {
        let outlets = DeviceKind::AuxIn.outlets();
        assert_eq!(
            audio_tap_index(outlets, mooloop_core::aux_in::OUTLET_OUT),
            Some(TAP_OUT)
        );
    }

    #[test]
    fn a_resolved_source_arrives_at_level_in_the_same_block() {
        let frames = 256;
        let source = ramp(frames);
        let mut device = AuxIn::new(unity(), SR);
        device.reset();
        let mut bus = StereoBus::with_capacity(frames);
        device.process_from(
            &ctx(frames),
            &mut bus,
            Some(&source),
            &EventList::empty(),
            &mut AudioTaps::none(),
        );
        for index in 0..frames {
            assert_eq!(bus.l[index], source.l[index], "frame {index}");
            assert_eq!(bus.r[index], source.r[index], "frame {index}");
        }
    }

    /// An unresolved edge is silence, not the last block that did resolve.
    /// A user who deletes the producing channel hears nothing rather than a
    /// held buffer.
    #[test]
    fn an_unresolved_edge_is_silent_rather_than_stale() {
        let frames = 128;
        let source = ramp(frames);
        let mut device = AuxIn::new(unity(), SR);
        device.reset();
        let mut bus = StereoBus::with_capacity(frames);
        device.process_from(
            &ctx(frames),
            &mut bus,
            Some(&source),
            &EventList::empty(),
            &mut AudioTaps::none(),
        );
        assert!(bus.peak(frames).0 > 0.0);
        bus.clear(frames);
        device.process_from(
            &ctx(frames),
            &mut bus,
            None,
            &EventList::empty(),
            &mut AudioTaps::none(),
        );
        assert_eq!(bus.peak(frames), (0.0, 0.0));
    }

    /// An Aux In publishes what it rendered, so Aux Ins can be chained. The
    /// break in a chain has to propagate: a device reading nothing publishes
    /// silence rather than leaving the buffer as it found it.
    #[test]
    fn it_publishes_what_it_rendered_and_publishes_the_break_too() {
        let frames = 64;
        let source = ramp(frames);
        let mut device = AuxIn::new(unity(), SR);
        device.reset();
        let mut bus = StereoBus::with_capacity(frames);
        let mut published = StereoBus::with_capacity(frames);
        {
            let mut taps = AudioTaps::none();
            taps.set(TAP_OUT, &mut published);
            device.process_from(&ctx(frames), &mut bus, Some(&source), &EventList::empty(), &mut taps);
        }
        for index in 0..frames {
            assert_eq!(published.l[index], bus.l[index]);
            assert_eq!(published.r[index], bus.r[index]);
        }
        assert!(published.peak(frames).0 > 0.0);
        {
            let mut taps = AudioTaps::none();
            taps.set(TAP_OUT, &mut published);
            device.process_from(&ctx(frames), &mut bus, None, &EventList::empty(), &mut taps);
        }
        assert_eq!(published.peak(frames), (0.0, 0.0));
    }

    /// Level is an ordinary continuous parameter and takes a lane like any
    /// other, which means it arrives sample-timed and must not step.
    #[test]
    fn level_lands_where_a_lane_puts_it_without_stepping() {
        // Long enough to hold five time constants after the event, which is
        // where the one-pole has settled.
        let frames = 2048;
        let mut source = StereoBus::with_capacity(frames);
        source.l[..frames].fill(1.0);
        source.r[..frames].fill(1.0);
        let mut device = AuxIn::new(unity(), SR);
        device.reset();
        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        events.push(TimedEvent {
            offset: 128,
            event: Event::ParamValue {
                id: aux_in::PARAM_LEVEL,
                value: 0.0,
            },
        });
        device.process_from(&ctx(frames), &mut bus, Some(&source), &events, &mut AudioTaps::none());
        assert_eq!(bus.l[0], 1.0);
        // The step is a ramp, not a cliff: no two neighbouring samples move
        // by more than a fraction of the whole travel.
        for index in 1..frames {
            assert!(
                (bus.l[index] - bus.l[index - 1]).abs() < 0.02,
                "level stepped at frame {index}"
            );
        }
        assert!(bus.l[frames - 1] < 0.01, "level never arrived");
    }
}
