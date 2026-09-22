//! Chainable effect nodes. Effects implement `AudioNode` like instruments
//! do, but read and modify the bus in place (see `node.rs`'s processing
//! model) and stay ignorant of channel-strip concepts (gain/pan/mute) so the
//! same node can later run on a master or send bus without changes.
//!
//! Every effect takes its parameters as sample-timed `Event::ParamValue`
//! values in **natural units** (Hz, bits, linear gain). The non-realtime side
//! converts from normalized knob positions through the descriptor tables in
//! `mooloop_core::effect`; nodes never see a curve. See
//! `docs/MODULATION.md` for why the split falls there.

mod bitcrush;
mod preamp;
mod container;
mod delay;
mod drive;
mod dynamics;
mod eq;
mod filter;
mod modulation;
mod plate;
mod reverb;

pub use bitcrush::BitcrushEffect;
pub use preamp::PreampEffect;
pub use delay::DelayEffect;
pub use drive::DriveEffect;
pub use dynamics::{CompressorEffect, GateEffect, LimiterEffect};
pub use eq::{eq_response_db, EqEffect};
pub use filter::FilterEffect;
pub use modulation::ModulationEffect;
pub use plate::PlateEffect;
pub use container::ContainerEffect;
pub use reverb::ReverbEffect;

use mooloop_core::EffectParams;

use crate::bus::StereoBus;
use crate::event::{Event, EventList};
use crate::node::{AudioNode, ControlCurve, MAX_CONTROL_TICKS_PER_BLOCK};

/// An effect that renders part of a block, so the block can be split at the
/// offsets its parameter events carry.
///
/// Every effect in this module implements it, and every one of them did the
/// splitting by hand until 2026-09-12: twelve copies of the same ten lines,
/// which `scripts/dupe-audit` reported as ten because two of them had renamed
/// the loop variables and one had reformatted the `if let` onto one line. A
/// text search cannot hold a policy together; a trait can.
pub(crate) trait RangeProcessor {
    /// Render `bus[start..end]`. Called once per gap between events, so it
    /// must be correct for an empty range as well as a whole block.
    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize);

    /// Take one parameter value, in natural units, between ranges.
    fn apply_param(&mut self, id: u32, value: f32);
}

/// Render `frames` of `bus`, splitting at each parameter event so a value
/// lands on the sample it was timed for.
///
/// Two clamps carry the whole policy, and both are the kind of thing that is
/// easy to get subtly wrong in the twelfth copy:
///
/// - `.min(frames)` — an event timed past the end of the block applies after
///   everything audible, rather than indexing off the end of the bus. The
///   engine can hand one over when a block is shortened.
/// - `.max(pos)` — a list that is not sorted cannot rewind the render head. A
///   negative-length range would otherwise be asked for, and the effect's own
///   `process_range` should not have to defend against it.
///
/// Events other than `ParamValue` still split the block. That is deliberate:
/// the split is where the *audio* is cut, and cutting it on a note event an
/// effect ignores costs one extra call with the same coefficients.
pub(crate) fn process_param_split(
    node: &mut (impl RangeProcessor + ?Sized),
    bus: &mut StereoBus,
    events_in: &EventList,
    frames: usize,
) {
    let mut pos = 0usize;
    for ev in events_in.iter() {
        let off = (ev.offset as usize).min(frames).max(pos);
        node.process_range(bus, pos, off);
        if let Event::ParamValue { id, value } = ev.event {
            node.apply_param(id, value);
        }
        pos = off;
    }
    node.process_range(bus, pos, frames);
}

/// The most destinations any single [`mooloop_core::EffectKind`]'s parameter
/// table can drive at once -- `EqParams`'s fifty (seven bands of six fields
/// plus two pass filters of four), by a wide margin over every other kind.
///
/// Shared by [`CurveFrame`] here and by `mooloop_engine::render`'s own
/// per-block curve pool, both derived from the same upstream constant rather
/// than repeating the number, so the two can never silently disagree about
/// how many rows a block's curves need.
pub(crate) const MAX_NODE_CURVE_DESTINATIONS: usize = mooloop_core::effect::EQ_DESCRIPTOR_COUNT;

/// A node's own copy of one block's curves, captured in
/// [`crate::node::AudioNode::apply_curves`] so they survive to the
/// following `process` call.
///
/// The borrowed `&[ControlCurve]` a node receives in `apply_curves` is only
/// valid for that one call -- `process` runs afterwards, as a separate call,
/// so a node with a native curve path has to keep its own copy in between.
/// Fixed-size and allocation-free, sized to [`MAX_NODE_CURVE_DESTINATIONS`]
/// rows of [`MAX_CONTROL_TICKS_PER_BLOCK`] ticks each, the same two bounds
/// the engine's pool this is fed from already respects.
pub(crate) struct CurveFrame {
    ids: [u32; MAX_NODE_CURVE_DESTINATIONS],
    ticks: [[f32; MAX_CONTROL_TICKS_PER_BLOCK]; MAX_NODE_CURVE_DESTINATIONS],
    lens: [usize; MAX_NODE_CURVE_DESTINATIONS],
    count: usize,
}

impl CurveFrame {
    pub(crate) fn empty() -> Self {
        Self {
            ids: [0; MAX_NODE_CURVE_DESTINATIONS],
            ticks: [[0.0; MAX_CONTROL_TICKS_PER_BLOCK]; MAX_NODE_CURVE_DESTINATIONS],
            lens: [0; MAX_NODE_CURVE_DESTINATIONS],
            count: 0,
        }
    }

    /// Copy `curves` in, replacing whatever this frame held before.
    ///
    /// Truncates past `MAX_NODE_CURVE_DESTINATIONS` rows or
    /// `MAX_CONTROL_TICKS_PER_BLOCK` ticks -- both bounds the engine's own
    /// pool already refuses past, so this can only truncate if a future
    /// effect kind's descriptor table grows past both without this constant
    /// growing to match; `effects_never_receive_more_curve_ids_than_a_frame_holds`
    /// below guards that.
    pub(crate) fn capture(&mut self, curves: &[ControlCurve<'_>]) {
        self.count = curves.len().min(MAX_NODE_CURVE_DESTINATIONS);
        for (index, curve) in curves.iter().take(self.count).enumerate() {
            self.ids[index] = curve.id;
            let len = curve.values.len().min(MAX_CONTROL_TICKS_PER_BLOCK);
            self.lens[index] = len;
            self.ticks[index][..len].copy_from_slice(&curve.values[..len]);
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub(crate) fn curves(&self) -> impl Iterator<Item = ControlCurve<'_>> {
        (0..self.count).map(|index| ControlCurve {
            id: self.ids[index],
            values: &self.ticks[index][..self.lens[index]],
        })
    }
}

impl Default for CurveFrame {
    fn default() -> Self {
        Self::empty()
    }
}

/// The curve-aware twin of [`process_param_split`]: splits `bus` at each
/// control tick a curve holds a value for instead of at each event's sample
/// offset, so a node's `apply_param` runs at most once per tick per
/// parameter rather than once per pushed event.
///
/// `events_in` still carries whatever this block's ordinary event path
/// produced for this node -- a knob edited between blocks and queued once at
/// offset 0 (`RenderState::queue_param`'s doc comment), or a retained-audio
/// command. Those are not curve-driven (the write-precedence table in
/// `docs/MODULATION.md` is mutually exclusive: a destination is either
/// modulated/automated, and arrives here as a curve, or it is not, and
/// arrives as this one queued event), so they are applied first, at their
/// documented offset of zero, before the curve's own ticks begin. Every
/// entry is expected at offset zero for exactly that reason; a future event
/// that is not would be applied too early, which is why that is asserted
/// rather than assumed.
pub(crate) fn process_curve_split(
    node: &mut (impl RangeProcessor + ?Sized),
    bus: &mut StereoBus,
    events_in: &EventList,
    curves: &CurveFrame,
    tick_frames: usize,
    frames: usize,
) {
    for ev in events_in.iter() {
        debug_assert_eq!(
            ev.offset, 0,
            "process_curve_split assumes every queued effect event is timed \
             at offset zero; see PendingEffectParams::queue"
        );
        if let Event::ParamValue { id, value } = ev.event {
            node.apply_param(id, value);
        }
    }
    if curves.is_empty() {
        node.process_range(bus, 0, frames);
        return;
    }
    let ticks = curves
        .curves()
        .map(|curve| curve.values.len())
        .max()
        .unwrap_or(0);
    let mut pos = 0usize;
    for tick in 0..ticks {
        let offset = (tick * tick_frames).min(frames).max(pos);
        if offset > pos {
            node.process_range(bus, pos, offset);
            pos = offset;
        }
        for curve in curves.curves() {
            if let Some(&value) = curve.values.get(tick) {
                node.apply_param(curve.id, value);
            }
        }
    }
    node.process_range(bus, pos, frames);
}

/// Construct the DSP node for a parameter set.
///
/// This allocates, so it belongs on the non-realtime side: the engine calls
/// it at project load and the GUI calls it when the user adds an effect,
/// shipping the box through the ordered control stream. Keeping one
/// constructor means a new effect kind is wired in exactly once.
pub fn build_effect(params: EffectParams, sample_rate: u32) -> Box<dyn AudioNode + Send> {
    build_effect_at_tempo(params, sample_rate, 120.0)
}

/// Construct an effect node using the current transport tempo for devices
/// whose preallocated state is beat-relative. This remains a control-plane
/// operation: callers must never invoke it from the audio callback.
pub fn build_effect_at_tempo(
    params: EffectParams,
    sample_rate: u32,
    bpm: f64,
) -> Box<dyn AudioNode + Send> {
    match params {
        EffectParams::Eq(p) => Box::new(EqEffect::new(p, sample_rate)),
        EffectParams::Modulation(p) => Box::new(ModulationEffect::new(p, sample_rate)),
        EffectParams::Filter(p) => Box::new(FilterEffect::new(p, sample_rate)),
        EffectParams::Drive(p) => Box::new(DriveEffect::new(p, sample_rate)),
        EffectParams::Preamp(p) => Box::new(PreampEffect::new(p, sample_rate)),
        EffectParams::Bitcrush(p) => Box::new(BitcrushEffect::new(p)),
        EffectParams::Delay(mut p) => {
            if p.tempo_sync {
                p.time_ms = p.time_division.time_ms(bpm);
            }
            Box::new(DelayEffect::new(p, sample_rate))
        }
        EffectParams::Reverb(p) => Box::new(ReverbEffect::new(p, sample_rate)),
        EffectParams::Plate(p) => Box::new(PlateEffect::new(p, sample_rate)),
        EffectParams::Gate(p) => Box::new(GateEffect::new(p, sample_rate)),
        EffectParams::Compressor(p) => Box::new(CompressorEffect::new(p, sample_rate)),
        EffectParams::Limiter(p) => Box::new(LimiterEffect::new(p, sample_rate)),
        EffectParams::Buffer(p) => Box::new(crate::BufferDevice::new(p, sample_rate, bpm)),
        // Transparent, and deliberately so: a container's mix belongs to the
        // host beside the per-slot dry path, not to a node. See
        // `container.rs`.
        EffectParams::Chain(p) => Box::new(ContainerEffect::new(p)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::StereoBus;
    use crate::event::EventList;
    use crate::node::{ProcessContext, SILENCE_PEAK};
    use mooloop_core::{BitcrushParams, BitcrushStyle, EffectKind, EffectParams};

    const SAMPLE_RATE: u32 = 48_000;
    const BLOCK: usize = 512;

    /// Records what [`process_param_split`] asked of it, in order, so the
    /// splitting itself can be checked without a real effect's audio in the
    /// way. Every effect in this module used to carry its own copy of that
    /// loop, so its two clamps were never tested anywhere.
    #[derive(Default)]
    struct Recorder {
        ranges: Vec<(usize, usize)>,
        params: Vec<(u32, f32, usize)>,
    }

    impl RangeProcessor for Recorder {
        fn process_range(&mut self, _bus: &mut StereoBus, start: usize, end: usize) {
            self.ranges.push((start, end));
        }

        fn apply_param(&mut self, id: u32, value: f32) {
            // Where in the block this value landed, as the ranges so far say.
            let at = self.ranges.last().map_or(0, |(_, end)| *end);
            self.params.push((id, value, at));
        }
    }

    fn split(events: &EventList, frames: usize) -> Recorder {
        let mut bus = StereoBus::with_capacity(frames.max(1));
        let mut recorder = Recorder::default();
        process_param_split(&mut recorder, &mut bus, events, frames);
        recorder
    }

    fn events(offsets_and_ids: &[(u32, u32)]) -> EventList {
        let mut list = EventList::empty();
        for (offset, id) in offsets_and_ids {
            list.push(crate::event::TimedEvent {
                offset: *offset,
                event: Event::ParamValue {
                    id: *id,
                    value: *id as f32,
                },
            });
        }
        list
    }

    /// No events is one range covering the block, not zero ranges.
    #[test]
    fn an_empty_event_list_renders_the_block_in_one_call() {
        let recorder = split(&EventList::empty(), 64);
        assert_eq!(recorder.ranges, vec![(0, 64)]);
        assert!(recorder.params.is_empty());
    }

    /// The ordinary case: the block is cut at the event and the value lands
    /// on the sample it was timed for.
    #[test]
    fn a_value_lands_on_the_sample_it_was_timed_for() {
        let recorder = split(&events(&[(16, 7)]), 64);
        assert_eq!(recorder.ranges, vec![(0, 16), (16, 64)]);
        assert_eq!(recorder.params, vec![(7, 7.0, 16)]);
    }

    /// First clamp: an event timed past the end of the block applies after
    /// everything audible rather than indexing off the end of the bus. The
    /// engine hands one over when a block is shortened.
    #[test]
    fn an_event_past_the_block_end_applies_after_all_of_it() {
        let recorder = split(&events(&[(999, 3)]), 64);
        assert_eq!(recorder.ranges, vec![(0, 64), (64, 64)]);
        assert_eq!(recorder.params, vec![(3, 3.0, 64)]);
    }

    /// Second clamp: an unsorted list cannot rewind the render head, so no
    /// effect's `process_range` is ever handed a backwards range.
    #[test]
    fn an_out_of_order_event_does_not_rewind_the_render_head() {
        let recorder = split(&events(&[(40, 1), (8, 2)]), 64);
        assert_eq!(recorder.ranges, vec![(0, 40), (40, 40), (40, 64)]);
        for (start, end) in &recorder.ranges {
            assert!(start <= end, "range {start}..{end} runs backwards");
        }
    }

    fn curve_frame(rows: &[(u32, &[f32])]) -> CurveFrame {
        let mut frame = CurveFrame::empty();
        let curves: Vec<ControlCurve<'_>> = rows
            .iter()
            .map(|&(id, values)| ControlCurve { id, values })
            .collect();
        frame.capture(&curves);
        frame
    }

    /// The ordinary case: one destination's curve splits the block at every
    /// tick it holds a value for, and `apply_param` sees each one exactly
    /// once, in tick order.
    #[test]
    fn a_curve_lands_its_value_on_every_tick_boundary() {
        let values = [1.0, 2.0, 3.0];
        let frame = curve_frame(&[(9, &values)]);
        let mut bus = StereoBus::with_capacity(96);
        let mut recorder = Recorder::default();
        process_curve_split(&mut recorder, &mut bus, &EventList::empty(), &frame, 32, 96);
        assert_eq!(recorder.ranges, vec![(0, 32), (32, 64), (64, 96)]);
        assert_eq!(recorder.params, vec![(9, 1.0, 0), (9, 2.0, 32), (9, 3.0, 64)]);
    }

    /// Two destinations changing at the same tick both land at that tick's
    /// boundary in one range split, not one split each -- the coalescing
    /// `process_param_split`'s per-event splitting does not give for free.
    #[test]
    fn two_curves_at_the_same_tick_share_one_range_split() {
        let a = [10.0, 11.0];
        let b = [20.0, 21.0];
        let frame = curve_frame(&[(1, &a), (2, &b)]);
        let mut bus = StereoBus::with_capacity(64);
        let mut recorder = Recorder::default();
        process_curve_split(&mut recorder, &mut bus, &EventList::empty(), &frame, 32, 64);
        assert_eq!(recorder.ranges, vec![(0, 32), (32, 64)]);
        assert_eq!(
            recorder.params,
            vec![(1, 10.0, 0), (2, 20.0, 0), (1, 11.0, 32), (2, 21.0, 32)]
        );
    }

    /// A queued once-off event (a knob edited between blocks, always at
    /// offset zero per `PendingEffectParams::queue`) is applied before the
    /// curve's own first tick, not folded into it or dropped.
    #[test]
    fn a_queued_once_off_event_is_applied_before_the_first_curve_tick() {
        let values = [5.0];
        let frame = curve_frame(&[(3, &values)]);
        let mut once = EventList::empty();
        once.push(crate::event::TimedEvent {
            offset: 0,
            event: Event::ParamValue { id: 4, value: 9.0 },
        });
        let mut bus = StereoBus::with_capacity(32);
        let mut recorder = Recorder::default();
        process_curve_split(&mut recorder, &mut bus, &once, &frame, 32, 32);
        assert_eq!(recorder.params, vec![(4, 9.0, 0), (3, 5.0, 0)]);
    }

    /// An empty curve frame still renders the whole block in one call, the
    /// same "no events" case `an_empty_event_list_renders_the_block_in_one_call`
    /// covers for the event path.
    #[test]
    fn an_empty_curve_frame_renders_the_block_in_one_call() {
        let frame = CurveFrame::empty();
        let mut bus = StereoBus::with_capacity(64);
        let mut recorder = Recorder::default();
        process_curve_split(&mut recorder, &mut bus, &EventList::empty(), &frame, 32, 64);
        assert_eq!(recorder.ranges, vec![(0, 64)]);
        assert!(recorder.params.is_empty());
    }

    /// The point of the whole mechanism (`reports/fable-2026-09-22.md`,
    /// finding 3): a shared, 256-slot `EventList` cannot hold more than one
    /// fully-automated destination's worth of a maximal block without
    /// dropping the rest, and a curve frame's rows do not share that
    /// capacity with each other. Four destinations, each driven at every
    /// one of a maximal block's control ticks, is 4x what the old event
    /// path could carry for even *one* -- demonstrated against the event
    /// list directly, not asserted about it, so this fails if `EventList`'s
    /// capacity or `MAX_CONTROL_TICKS_PER_BLOCK` ever change relative to
    /// each other.
    #[test]
    fn many_simultaneous_curve_destinations_do_not_drop_where_the_shared_event_list_would() {
        const DESTINATIONS: usize = 4;
        let ticks = MAX_CONTROL_TICKS_PER_BLOCK;
        let frames = ticks * 32;

        // What the old path did: every tick of every destination pushed as
        // one `Event::ParamValue` onto the one list the node's block was
        // split on. The first destination alone already reaches the list's
        // capacity, so every event after it -- the entire second, third and
        // fourth destination -- is refused.
        let mut shared = EventList::empty();
        let mut accepted = 0usize;
        for destination in 0..DESTINATIONS {
            for tick in 0..ticks {
                let accepted_this_one = shared.push_ordered(crate::event::TimedEvent {
                    offset: (tick * 32) as u32,
                    event: Event::ParamValue {
                        id: destination as u32,
                        value: tick as f32,
                    },
                });
                accepted += accepted_this_one as usize;
            }
        }
        assert!(
            accepted > 0 && accepted < DESTINATIONS * ticks,
            "expected the shared event list to accept some ticks and then \
             start refusing the rest (accepted {accepted} of {}); if it \
             accepted all of them this test no longer demonstrates the \
             defect the curve pool fixes",
            DESTINATIONS * ticks
        );

        // The new path: each destination gets its own row, so nothing about
        // a second, third or fourth destination competes with the first for
        // room.
        let series: Vec<Vec<f32>> = (0..DESTINATIONS)
            .map(|_| (0..ticks).map(|tick| tick as f32).collect())
            .collect();
        let rows: Vec<(u32, &[f32])> = series
            .iter()
            .enumerate()
            .map(|(destination, values)| (destination as u32, values.as_slice()))
            .collect();
        let frame = curve_frame(&rows);
        let mut bus = StereoBus::with_capacity(frames);
        let mut recorder = Recorder::default();
        process_curve_split(&mut recorder, &mut bus, &EventList::empty(), &frame, 32, frames);
        assert_eq!(
            recorder.params.len(),
            DESTINATIONS * ticks,
            "every destination's every tick should have reached apply_param"
        );
        // Application order is tick-major: at each control tick every
        // destination's value is applied, then that tick's 32-frame range is
        // rendered, before advancing to the next tick. So the recorded order
        // is (tick 0: dest 0..N), (tick 1: dest 0..N), ...
        for tick in 0..ticks {
            for destination in 0..DESTINATIONS {
                assert_eq!(
                    recorder.params[tick * DESTINATIONS + destination],
                    (destination as u32, tick as f32, tick * 32)
                );
            }
        }
    }

    /// `MAX_NODE_CURVE_DESTINATIONS` is derived from `EQ_DESCRIPTOR_COUNT`
    /// on the claim that EQ's is the largest table; checked here against
    /// every kind rather than trusted, so a future table that grows past it
    /// fails this test instead of silently truncating in `CurveFrame::capture`.
    #[test]
    fn no_effect_kinds_descriptor_table_exceeds_the_curve_frames_capacity() {
        for kind in EffectKind::ALL {
            assert!(
                kind.descriptors().len() <= MAX_NODE_CURVE_DESTINATIONS,
                "{kind:?} has {} descriptors, more than \
                 MAX_NODE_CURVE_DESTINATIONS ({MAX_NODE_CURVE_DESTINATIONS})",
                kind.descriptors().len()
            );
        }
    }

    fn context(frames: usize) -> ProcessContext {
        ProcessContext {
            sample_rate: SAMPLE_RATE,
            frames,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        }
    }

    /// Deterministic broadband noise at -6 dBFS. Broadband because a tone
    /// leaves most of an EQ's or a reverb's structure untouched, and loud
    /// because a gate has to open before its release means anything.
    fn fill_burst(bus: &mut StereoBus, frames: usize, state: &mut u32) {
        for index in 0..frames {
            let mut next = || {
                *state ^= *state << 13;
                *state ^= *state >> 17;
                *state ^= *state << 5;
                ((*state >> 8) as f32 / 8_388_608.0 - 1.0) * 0.5
            };
            bus.l[index] = next();
            bus.r[index] = next();
        }
    }

    /// The host's own decision, written once so the test and `EffectChain`
    /// cannot drift: a slot may be skipped when its input is silent and
    /// either the node's state has settled or it has been fed silence for
    /// longer than the tail it declared.
    fn may_skip(node: &dyn AudioNode, silent_frames: u32) -> bool {
        silent_frames > 0 && (node.is_at_rest() || silent_frames > node.tail_frames())
    }

    /// The contract step 02 rests on, checked against each device rather than
    /// derived from it: **once a node says it can be left alone, leaving it
    /// alone is inaudible.** A device that under-reports here is a chopped
    /// reverb tail, which is the one way this whole mechanism can be heard.
    ///
    /// Ten seconds is the cap. It is not an assertion about any device's
    /// tail -- a delay at full feedback honestly reports minutes -- only a
    /// bound on how long this test will run before it accepts that a device
    /// with default settings has decided to stay awake.
    #[test]
    fn every_effect_kind_is_silent_once_it_says_it_can_be_skipped() {
        const CAP_BLOCKS: usize = 10 * SAMPLE_RATE as usize / BLOCK;
        // The one device that makes sound out of a silent input by design,
        // and so declines the whole mechanism. Named here so removing the
        // decision from `buffer_device.rs` fails this test rather than
        // quietly starting to skip a playing buffer.
        let never_sleeps = [EffectKind::Buffer];

        for kind in EffectKind::ALL {
            let mut node = build_effect_at_tempo(kind.default_params(), SAMPLE_RATE, 120.0);
            let mut bus = StereoBus::with_capacity(BLOCK);
            let events = EventList::empty();
            let mut state = 0x1234_5678;

            // Half a second of noise, so every ring in every device is
            // carrying something when the input stops.
            for _ in 0..(SAMPLE_RATE as usize / 2 / BLOCK) {
                fill_burst(&mut bus, BLOCK, &mut state);
                node.process(&context(BLOCK), &mut bus, &events, None);
            }

            let mut silent_frames = 0u32;
            let mut slept_at = None;
            for block in 0..CAP_BLOCKS {
                if may_skip(node.as_ref(), silent_frames) {
                    slept_at = Some(block);
                    break;
                }
                bus.clear(BLOCK);
                node.process(&context(BLOCK), &mut bus, &events, None);
                silent_frames = silent_frames.saturating_add(BLOCK as u32);
            }

            let Some(slept_at) = slept_at else {
                assert!(
                    never_sleeps.contains(&kind),
                    "{kind:?} never reported itself skippable in ten seconds of silence,                      and is not one of the devices that deliberately never does"
                );
                continue;
            };
            assert!(
                !never_sleeps.contains(&kind),
                "{kind:?} is declared as a device that never sleeps, but reported                  itself skippable after {slept_at} blocks"
            );

            // Keep processing past the point the host would have stopped. If
            // anything was still coming, this is where it shows up.
            for block in 0..64 {
                bus.clear(BLOCK);
                node.process(&context(BLOCK), &mut bus, &events, None);
                let (left, right) = bus.peak(BLOCK);
                assert!(
                    left.max(right) <= SILENCE_PEAK,
                    "{kind:?} said it could be skipped after {slept_at} blocks of                      silence, then produced {} on block {block} after that",
                    left.max(right)
                );
            }
        }
    }

    /// The other half of the contract, and the half that is easy to forget:
    /// a device that slept has to come back where it would have been.
    ///
    /// A chorus's LFO, a reverb's line modulation and a crusher's
    /// sample-and-hold counter all run on the clock rather than on the input,
    /// so freezing them would make a device sound different after a rest —
    /// and would make *how* different depend on the host's buffer size, which
    /// would take block-size-independent rendering with it. `skip_block` is
    /// what the host calls instead of `process` for a block it skips, and
    /// this drives every kind through burst, sleep and burst twice over to
    /// see that using it changes nothing.
    ///
    /// The tolerance is `SILENCE_PEAK`: a device falling asleep with its
    /// input sitting exactly on the threshold is entitled to differ by the
    /// threshold, and by nothing more.
    #[test]
    fn every_effect_kind_comes_back_where_it_would_have_been() {
        const SILENT_BLOCKS: usize = 10 * SAMPLE_RATE as usize / BLOCK;

        for kind in EffectKind::ALL {
            let render = |skip: bool| {
                let mut node = build_effect_at_tempo(kind.default_params(), SAMPLE_RATE, 120.0);
                let mut bus = StereoBus::with_capacity(BLOCK);
                let events = EventList::empty();
                let mut state = 0x0bad_c0de;
                let mut out = Vec::with_capacity((SILENT_BLOCKS + 100) * BLOCK);
                let mut silent_frames = 0u32;
                let burst_blocks = SAMPLE_RATE as usize / 2 / BLOCK;
                for block in 0..(burst_blocks * 2 + SILENT_BLOCKS) {
                    bus.clear(BLOCK);
                    if block < burst_blocks || block >= burst_blocks + SILENT_BLOCKS {
                        fill_burst(&mut bus, BLOCK, &mut state);
                    }
                    let context = context(BLOCK);
                    let peak = bus.peak(BLOCK);
                    silent_frames = if peak.0.max(peak.1) <= SILENCE_PEAK {
                        silent_frames.saturating_add(BLOCK as u32)
                    } else {
                        0
                    };
                    if skip && may_skip(node.as_ref(), silent_frames) {
                        node.skip_block(&context);
                    } else {
                        node.process(&context, &mut bus, &events, None);
                    }
                    out.extend_from_slice(&bus.l[..BLOCK]);
                }
                out
            };
            let slept = render(true);
            let ran = render(false);
            let mut worst = 0.0f32;
            let mut worst_at = 0;
            for (index, (a, b)) in slept.iter().zip(&ran).enumerate() {
                if (a - b).abs() > worst {
                    worst = (a - b).abs();
                    worst_at = index;
                }
            }
            assert!(
                worst <= SILENCE_PEAK,
                "{kind:?} came back somewhere else: {worst} at frame {worst_at}"
            );
        }
    }

    /// The exception the crusher's `is_at_rest` is written around, held here
    /// so it cannot be simplified away. TPDF dither spans a whole quantiser
    /// step, so it lands on `+/-step` about half the time whatever the input
    /// was; at four bits that is -18 dBFS of hiss on a channel that has
    /// stopped playing, and it is the device's noise floor rather than a tail.
    #[test]
    fn a_dithering_crusher_never_reports_rest_while_its_wet_is_heard() {
        let params = BitcrushParams {
            bits: 4.0,
            style: BitcrushStyle::Dither,
            mix: 1.0,
            ..BitcrushParams::default()
        };
        let mut node = build_effect(EffectParams::Bitcrush(params), SAMPLE_RATE);
        let mut bus = StereoBus::with_capacity(BLOCK);
        let events = EventList::empty();

        let mut loudest = 0.0f32;
        for _ in 0..16 {
            bus.clear(BLOCK);
            node.process(&context(BLOCK), &mut bus, &events, None);
            let (left, right) = bus.peak(BLOCK);
            loudest = loudest.max(left.max(right));
            assert!(
                !may_skip(node.as_ref(), BLOCK as u32),
                "a dithering crusher reported itself skippable"
            );
        }
        assert!(
            loudest > SILENCE_PEAK,
            "the premise of this test is that dither makes noise out of silence;              it produced at most {loudest}"
        );
    }

    /// A device's latency is written down twice: as a declaration in
    /// `mooloop-core`, which the control thread reads to size a compensation
    /// delay before any node exists, and on the running node itself. Two
    /// numbers for one fact is how they come to disagree, and a disagreement
    /// here is silent — the delay would be built to the wrong length and the
    /// misalignment it was meant to remove would move instead.
    ///
    /// This is the one crate that can see both, so this is where they are held
    /// together. Two sample rates, because a device whose latency depended on
    /// the rate could not be declared statically at all and this is where that
    /// would be found out.
    #[test]
    fn every_effect_kinds_declared_latency_matches_its_node() {
        for kind in EffectKind::ALL {
            for sample_rate in [44_100, 48_000] {
                let node = build_effect_at_tempo(kind.default_params(), sample_rate, 120.0);
                assert_eq!(
                    node.latency_frames(),
                    kind.latency_frames(),
                    "{kind:?} at {sample_rate} Hz: node reports {}, core declares {}",
                    node.latency_frames(),
                    kind.latency_frames()
                );
                // The container's bypass path delays the signal through the
                // ring it sized from the *dry path* length, while the latency
                // plan sums the declared `latency_frames`. They are the same
                // number for every device today, and a device that broke that
                // would make a bypassed slot cost a different number of frames
                // than the plan compensated for. Asserted rather than assumed,
                // because the symptom would be a misalignment nothing reports.
                assert_eq!(
                    node.dry_path_latency_frames(),
                    node.latency_frames(),
                    "{kind:?} declares a dry-path latency apart from its own; \
                     the bypass path and the latency plan would disagree"
                );
            }
        }
    }
}
