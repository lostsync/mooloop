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
mod bus_comp;
mod preamp;
mod container;
mod delay;
mod drive;
mod dynamics;
mod eq;
mod filter;
mod modulation;
mod plate;
mod plugin_placeholder;
mod reverb;

pub use bitcrush::BitcrushEffect;
pub use bus_comp::BusCompEffect;
pub use preamp::PreampEffect;
pub use delay::DelayEffect;
pub use drive::DriveEffect;
pub use dynamics::{CompressorEffect, GateEffect, LimiterEffect};
pub use eq::{eq_response_db, EqEffect};
pub use filter::FilterEffect;
pub use modulation::ModulationEffect;
pub use plate::PlateEffect;
pub use plugin_placeholder::PluginPlaceholder;
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

/// Whether a block came out of a device carrying a NaN or an infinity, and
/// if it did, silence in their place (MOO-176).
///
/// For a device whose own recursive state would keep a non-finite value for
/// good -- a feedback network, a detector -- to call once after its block
/// and clear that state when it answers yes. One pass over the block when it
/// is clean, which is every block that matters, and it writes nothing then,
/// so a finite block is untouched to the bit.
pub fn scrub_non_finite(bus: &mut StereoBus, frames: usize) -> bool {
    let frames = frames.min(bus.capacity());
    let finite = |samples: &[f32]| samples.iter().all(|sample| sample.is_finite());
    if finite(&bus.l[..frames]) && finite(&bus.r[..frames]) {
        return false;
    }
    for sample in bus.l[..frames].iter_mut().chain(&mut bus.r[..frames]) {
        if !sample.is_finite() {
            *sample = 0.0;
        }
    }
    true
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
            kind: crate::node::CurveKind::Value,
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
        EffectParams::BusComp(p) => Box::new(BusCompEffect::new(p, sample_rate)),
        EffectParams::Limiter(p) => Box::new(LimiterEffect::new(p, sample_rate)),
        EffectParams::Buffer(p) => Box::new(crate::BufferDevice::new(p, sample_rate, bpm)),
        // Transparent, and deliberately so: a container's mix belongs to the
        // host beside the per-slot dry path, not to a node. See
        // `container.rs`.
        EffectParams::Chain(p) | EffectParams::Layer(p) => Box::new(ContainerEffect::new(p)),
        // A pass-through that names its slot. The session swaps the hosted
        // plugin's processor in; while the plugin is missing, this is what
        // plays. See `plugin_placeholder.rs`.
        EffectParams::Plugin(slot) => Box::new(PluginPlaceholder::new(slot)),
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
            .map(|&(id, values)| ControlCurve { id, values, kind: crate::node::CurveKind::Value })
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

    /// **Tails survive a loop fold, and not a seek** -- Adam's ruling on
    /// MOO-59, checked against every kind rather than only the four it
    /// changed, so a device that starts clearing on a discontinuity later has
    /// to answer the fold as well.
    ///
    /// Each kind is rendered twice from the same noise burst, once told
    /// nothing and once told the transport folded, and what follows must be
    /// identical: a fold is inaudible in every effect. The four that hold a
    /// tail are then told of a seek instead, which must be audible. Without
    /// that half the first would pass just as well for a device whose line
    /// was already empty, and would prove nothing.
    #[test]
    fn a_loop_fold_is_inaudible_in_every_effect_kind_and_a_seek_is_not() {
        use crate::node::Discontinuity;

        let holds_a_tail = [
            EffectKind::Delay,
            EffectKind::Modulation,
            EffectKind::Reverb,
            EffectKind::Plate,
        ];
        let render_after = |kind: EffectKind, told: Option<Discontinuity>| {
            let mut node = build_effect_at_tempo(kind.default_params(), SAMPLE_RATE, 120.0);
            let mut bus = StereoBus::with_capacity(BLOCK);
            let events = EventList::empty();
            let mut state = 0x5eed_f01d;
            for _ in 0..(SAMPLE_RATE as usize / 2 / BLOCK) {
                fill_burst(&mut bus, BLOCK, &mut state);
                node.process(&context(BLOCK), &mut bus, &events, None);
            }
            if let Some(discontinuity) = told {
                node.on_discontinuity(discontinuity);
            }
            let mut out = Vec::with_capacity(8 * BLOCK);
            for _ in 0..4 {
                bus.clear(BLOCK);
                node.process(&context(BLOCK), &mut bus, &events, None);
                out.extend_from_slice(&bus.l[..BLOCK]);
                out.extend_from_slice(&bus.r[..BLOCK]);
            }
            out
        };
        let largest_difference = |a: &[f32], b: &[f32]| {
            a.iter()
                .zip(b)
                .fold(0.0f32, |peak, (a, b)| peak.max((a - b).abs()))
        };

        for kind in EffectKind::ALL {
            let untold = render_after(kind, None);
            let folded = render_after(kind, Some(Discontinuity::LoopFold));
            let moved = largest_difference(&untold, &folded);
            assert!(
                moved == 0.0,
                "{kind:?} changed its output on a loop fold, by up to {moved}: \
                 a tail that should have wrapped into the loop's start was cut"
            );
            if holds_a_tail.contains(&kind) {
                let seeked = render_after(kind, Some(Discontinuity::Seek));
                let cleared = largest_difference(&untold, &seeked);
                assert!(
                    cleared > 1.0e-3,
                    "{kind:?} sounded the same after a seek as after nothing, \
                     so its line held nothing and the fold half proves nothing: \
                     {cleared}"
                );
            }
        }
    }

    /// **How far each insert can raise its input** (MOO-175), measured at
    /// the corners most likely to add gain, with a sine at the reference
    /// level (`shaper::DRIVE_REFERENCE_LINEAR`, -12 dBFS) and at full scale,
    /// at 60 Hz, 220 Hz, 1 kHz and 5 kHz, as the steady-state peak over the
    /// last of three seconds.
    ///
    /// One table here rather than a test in each module, so that a kind
    /// nobody thought about fails `every_insert_kind_has_a_gain_bound`
    /// instead of being skipped. Every bound sits a little above what was
    /// measured on 2026-09-23 (in the comments), so a change that adds gain
    /// fails here first.
    ///
    /// What these numbers say: the feedback devices are resonant. A steady
    /// sine that lands on a reverb or plate mode builds up to about +14 dB,
    /// and the Modulation device's loop would reach `1 / (1 - 0.92)`, +22 dB,
    /// at full feedback, which is what a comb at that feedback does, if its
    /// output were not trimmed to hold the peak at +12 dB (MOO-200). The
    /// master's safety limiter (MOO-93) protects the speakers. The next
    /// device and the meters see the rest.
    fn gain_bounds() -> Vec<(&'static str, EffectParams, f32)> {
        use mooloop_core::*;
        let modulation = |mode: i32, feedback: f32| {
            EffectParams::Modulation(ModulationParams {
                mode: ModulationMode::from_index(mode),
                depth: 1.0,
                feedback,
                color: 1.0,
                ..ModulationParams::default()
            })
        };
        let mut bounds = vec![
            // Measured +13.9 dB (1 kHz).
            (
                "reverb, largest and longest",
                EffectParams::Reverb(ReverbParams {
                    size: 1.0,
                    decay_s: 20.0,
                    damping: 0.0,
                    diffusion: 1.0,
                    width: 1.0,
                    ..ReverbParams::default()
                }),
                16.0,
            ),
            // Measured +8.6 dB (1 kHz).
            ("reverb as it arrives", EffectParams::Reverb(ReverbParams::default()), 10.0),
            // Measured +9.7 dB (1 kHz).
            (
                "plate, largest and longest",
                EffectParams::Plate(PlateParams {
                    size: 1.0,
                    decay_s: 10.0,
                    damping: 0.0,
                    width: 1.0,
                    ..PlateParams::default()
                }),
                12.0,
            ),
            // Silent at the reference level (one bit rounds it to zero) and
            // unity at full scale.
            (
                "bitcrush at one bit",
                EffectParams::Bitcrush(BitcrushParams {
                    bits: 1.0,
                    downsample: 64.0,
                    mix: 1.0,
                    ..BitcrushParams::default()
                }),
                0.1,
            ),
            // Makeup is the user asking for level; with none, it only cuts.
            (
                "compressor at its hardest, no makeup",
                EffectParams::Compressor(CompressorParams {
                    threshold_db: -60.0,
                    ratio: 20.0,
                    attack_ms: 0.05,
                    release_ms: 5.0,
                    knee_db: 24.0,
                    makeup_db: 0.0,
                    mix: 1.0,
                }),
                0.1,
            ),
            // The same: with no makeup a bus comp only turns down.
            (
                "bus comp at its lowest threshold, no makeup",
                EffectParams::BusComp(BusCompParams {
                    threshold_db: -40.0,
                    makeup_db: 0.0,
                    ..BusCompParams::default()
                }),
                0.1,
            ),
            ("gate", EffectParams::Gate(GateParams::default()), 0.1),
            // Gain is the user driving it into the ceiling; with none, it
            // only cuts.
            (
                "limiter with no gain",
                EffectParams::Limiter(LimiterParams {
                    ceiling_db: 0.0,
                    release_ms: 1.0,
                    gain_db: 0.0,
                }),
                0.1,
            ),
            ("buffer, following", EffectParams::Buffer(BufferParams::default()), 0.1),
            // A band's gain is the user's to set; flat, it adds nothing.
            ("EQ as it arrives", EffectParams::Eq(EqParams::default()), 0.1),
        ];
        // Every Modulation mode at the trim's knee and at both ends of the
        // Feedback knob (MOO-200). The ceiling is `RESONANCE_CEILING`, 4 or
        // +12.04 dB, and the half dB over it is for a sweeping delay, which
        // is not quite a static comb. Before the trim these read up to +21.6
        // (flanger), +21.3 (phaser), +16.4 (chorus), +16.2 (ensemble, 81 Hz)
        // and +14.3 dB (ADT, 164 Hz). Measured 2026-09-26 with it: +12.02 dB
        // was the most, the phaser at 0.75.
        const MODULATION_CASES: [[&str; 3]; 5] = [
            ["chorus at the knee", "chorus at full feedback", "chorus at full negative feedback"],
            ["flanger at the knee", "flanger at full feedback", "flanger at full negative feedback"],
            ["phaser at the knee", "phaser at full feedback", "phaser at full negative feedback"],
            ["ensemble at the knee", "ensemble at full feedback", "ensemble at full negative feedback"],
            ["ADT at the knee", "ADT at full feedback", "ADT at full negative feedback"],
        ];
        for (mode, names) in MODULATION_CASES.iter().enumerate() {
            for (name, feedback) in names.iter().zip([0.75, 0.92, -0.92]) {
                bounds.push((name, modulation(mode as i32, feedback), 12.5));
            }
        }
        // Level-compensated at the reference level (+1.4 dB at 5 kHz was
        // the most) and under unity at full scale. The Tone tilt is the
        // user's: at full treble it lifts 1 kHz by about 15 dB.
        for curve in 0..4 {
            bounds.push((
                "drive at full drive, tone flat",
                EffectParams::Drive(DriveParams {
                    drive: 64.0,
                    curve: DriveCurve::from_index(curve),
                    tone: 0.0,
                    mix: 1.0,
                    output: 1.0,
                }),
                2.0,
            ));
        }
        bounds
    }

    /// The kinds whose bound is pinned somewhere other than [`gain_bounds`],
    /// and where. Held apart so that "nobody bounded it" and "it is bounded
    /// elsewhere" are different answers.
    const GAIN_ELSEWHERE: [(EffectKind, &str); 5] = [
        (
            EffectKind::Filter,
            "effects::filter::tests::no_mode_slope_or_resonance_peaks_past_its_bound (MOO-124)",
        ),
        (
            EffectKind::Delay,
            "effects::delay::tests::maximum_feedback_on_a_loud_input_stays_bounded (MOO-124)",
        ),
        (
            EffectKind::Preamp,
            "its drive and output trims are the user asking for level: \
             the_users_own_gain_is_all_the_preamp_adds",
        ),
        (
            EffectKind::Chain,
            "a container's node passes its input through, and its run's devices are bounded here",
        ),
        (
            EffectKind::Layer,
            "a layer sums its branches, two identical ones exactly +6 dB: \
             container_tests::two_identical_branches_are_exactly_six_db",
        ),
    ];

    #[test]
    fn every_insert_kind_stays_under_its_gain_bound() {
        let mut past = Vec::new();
        for (what, params, bound_db) in gain_bounds() {
            for level in [crate::shaper::DRIVE_REFERENCE_LINEAR, 1.0] {
                for freq in [60.0f32, 220.0, 1_000.0, 5_000.0] {
                    let gain = peak_gain_db(params, level, freq);
                    if gain > bound_db {
                        past.push(format!(
                            "{what}: {gain:+.2} dB at {freq} Hz, level {level:.3} (bound {bound_db:+.1})"
                        ));
                    }
                }
            }
        }
        assert!(past.is_empty(), "past their bounds:\n{}", past.join("\n"));
    }

    /// **The Modulation device's gain across the spectrum** (MOO-200), for
    /// choosing its bounds: every mode at four feedback settings, the
    /// loudest steady sine over 48 frequencies from 40 Hz to 10 kHz, at the
    /// reference level. `gain_bounds` samples four frequencies; this is the
    /// fuller picture. Slow, so ignored:
    /// `cargo test -p mooloop-dsp --release --lib modulation_gain_table -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn modulation_gain_table() {
        use mooloop_core::{ModulationMode, ModulationParams};
        for mode in 0..5 {
            let mut row = format!("{:>9}", format!("{:?}", ModulationMode::from_index(mode)));
            for feedback in [0.5f32, 0.75, 0.92, -0.92] {
                let params = EffectParams::Modulation(ModulationParams {
                    mode: ModulationMode::from_index(mode),
                    depth: 1.0,
                    feedback,
                    color: 1.0,
                    ..ModulationParams::default()
                });
                let (mut worst, mut at) = (f32::MIN, 0.0f32);
                for step in 0..48 {
                    let freq = 40.0 * 250.0f32.powf(step as f32 / 47.0);
                    let gain = peak_gain_db(params, crate::shaper::DRIVE_REFERENCE_LINEAR, freq);
                    if gain > worst {
                        (worst, at) = (gain, freq);
                    }
                }
                row.push_str(&format!("  fb {feedback:+.2}: {worst:+6.2} dB at {at:6.0} Hz"));
            }
            println!("{row}");
        }
    }

    /// Every kind is either in [`gain_bounds`] or says where its bound lives.
    #[test]
    fn every_insert_kind_has_a_gain_bound() {
        let bounded: Vec<EffectKind> =
            gain_bounds().iter().map(|(_, params, _)| params.kind()).collect();
        for kind in EffectKind::ALL {
            assert!(
                bounded.contains(&kind)
                    || GAIN_ELSEWHERE.iter().any(|(elsewhere, _)| *elsewhere == kind),
                "{kind:?} has no gain bound and no note saying where it is decided"
            );
        }
    }

    /// The Preamp's gain is its two trims, and what a voicing's measured
    /// tilt adds on top: at the loudest drive, in every voicing, a sine comes
    /// out no louder than the drive and output the user dialled plus
    /// `VOICING_LIFT_DB`.
    #[test]
    fn the_users_own_gain_is_all_the_preamp_adds() {
        // Measured 2026-09-23: Grip at +24 dB of drive peaks 3.0 dB past
        // its trims (its odd-order profile, driven hard, lines its harmonics
        // up on the crest); Moo, Punch and Iron stay within a tenth.
        const VOICING_LIFT_DB: f32 = 3.5;
        let mut past = Vec::new();
        use mooloop_core::{PreampParams, PreampVoicing};
        for voicing in [
            PreampVoicing::Moo,
            PreampVoicing::Grip,
            PreampVoicing::Punch,
            PreampVoicing::Iron,
        ] {
            for (drive_db, output_db) in [(24.0f32, 0.0f32), (24.0, 24.0), (0.0, 24.0)] {
                let params = EffectParams::Preamp(PreampParams {
                    drive_db,
                    output_db,
                    voicing,
                    mix: 1.0,
                    ..PreampParams::default()
                });
                for freq in [220.0f32, 1_000.0] {
                    let gain = peak_gain_db(params, crate::shaper::DRIVE_REFERENCE_LINEAR, freq);
                    if gain > drive_db + output_db + VOICING_LIFT_DB {
                        past.push(format!(
                            "{voicing:?} at +{drive_db}/+{output_db} dB added {gain:+.2} dB at {freq} Hz"
                        ));
                    }
                }
            }
        }
        assert!(past.is_empty(), "past the trims:\n{}", past.join("\n"));
    }

    /// A sine of `level` at `freq` through `params` for three seconds; the
    /// output's peak over the last second, either side, in dB over `level`.
    fn peak_gain_db(params: EffectParams, level: f32, freq: f32) -> f32 {
        let frames = SAMPLE_RATE as usize * 3;
        let measure_from = SAMPLE_RATE as usize * 2;
        let mut node = build_effect(params, SAMPLE_RATE);
        let context = ProcessContext {
            sample_rate: SAMPLE_RATE,
            frames: BLOCK,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        };
        let mut bus = StereoBus::with_capacity(BLOCK);
        let mut peak = 0.0f32;
        let mut n = 0usize;
        while n < frames {
            for i in 0..BLOCK {
                let t = (n + i) as f32 / SAMPLE_RATE as f32;
                let s = (t * freq * core::f32::consts::TAU).sin() * level;
                bus.l[i] = s;
                bus.r[i] = s;
            }
            node.process(&context, &mut bus, &EventList::empty(), None);
            if n >= measure_from {
                peak = peak
                    .max(crate::testkit::peak(&bus.l[..BLOCK]))
                    .max(crate::testkit::peak(&bus.r[..BLOCK]));
            }
            n += BLOCK;
        }
        crate::testkit::db(peak / level)
    }

    /// Every insert, with the settings most likely to hold on to what it is
    /// fed: the longest tails, the most feedback.
    fn every_kind_at_its_stickiest() -> Vec<EffectParams> {
        let mut kinds = Vec::new();
        for kind in EffectKind::ALL {
            let params = match kind.default_params() {
                EffectParams::Compressor(mut p) => {
                    p.threshold_db = -40.0;
                    EffectParams::Compressor(p)
                }
                EffectParams::Reverb(mut p) => {
                    p.decay_s = 20.0;
                    p.size = 1.0;
                    EffectParams::Reverb(p)
                }
                EffectParams::Plate(mut p) => {
                    p.decay_s = 10.0;
                    p.size = 1.0;
                    EffectParams::Plate(p)
                }
                EffectParams::Delay(mut p) => {
                    p.feedback = 0.95;
                    EffectParams::Delay(p)
                }
                other => other,
            };
            kinds.push(params);
        }
        for mode in 0..5 {
            kinds.push(EffectParams::Modulation(mooloop_core::ModulationParams {
                mode: mooloop_core::ModulationMode::from_index(mode),
                feedback: 0.92,
                ..mooloop_core::ModulationParams::default()
            }));
        }
        kinds
    }

    /// **A NaN in the input is gone within a block, in every insert**
    /// (MOO-176). MOO-174 fixed the shared filter blocks; this is the rest:
    /// the reverb's and plate's networks, the modulation effect's line and
    /// all-passes, the dynamics detectors, the limiter's lookahead and the
    /// Buffer's ring. One NaN frame goes in, finite audio follows, and from
    /// the block after it every sample is finite and the device still makes
    /// sound.
    #[test]
    fn a_nan_in_the_input_is_gone_within_a_block_in_every_insert() {
        for params in every_kind_at_its_stickiest() {
            let mut node = build_effect(params, SAMPLE_RATE);
            let context = ProcessContext {
                sample_rate: SAMPLE_RATE,
                frames: BLOCK,
                playing: true,
                bpm: 120.0,
                position_ticks: 0.0,
                position_frames: 0,
            };
            let mut phase = 0usize;
            for block in 0..8 {
                let mut bus = StereoBus::with_capacity(BLOCK);
                for index in 0..BLOCK {
                    let s = (phase as f32 * 440.0 / SAMPLE_RATE as f32 * core::f32::consts::TAU)
                        .sin()
                        * 0.25;
                    bus.l[index] = s;
                    bus.r[index] = s;
                    phase += 1;
                }
                if block == 1 {
                    bus.l[100] = f32::NAN;
                    bus.r[100] = f32::INFINITY;
                }
                node.process(&context, &mut bus, &EventList::empty(), None);
                if block >= 2 {
                    assert!(
                        crate::testkit::all_finite(&bus.l[..BLOCK])
                            && crate::testkit::all_finite(&bus.r[..BLOCK]),
                        "{:?}: block {block} still carries the NaN",
                        params.kind()
                    );
                }
            }
        }
    }

    /// The host's helper leaves a finite block untouched to the bit, and
    /// silences only what is not finite.
    #[test]
    fn scrubbing_a_finite_block_changes_nothing() {
        let mut bus = StereoBus::with_capacity(BLOCK);
        for index in 0..BLOCK {
            bus.l[index] = (index as f32 * 0.37).sin() * 0.9;
            bus.r[index] = -bus.l[index];
        }
        let (left, right) = (bus.l[..BLOCK].to_vec(), bus.r[..BLOCK].to_vec());
        assert!(!scrub_non_finite(&mut bus, BLOCK));
        assert_eq!(bus.l[..BLOCK], left[..]);
        assert_eq!(bus.r[..BLOCK], right[..]);
        bus.l[7] = f32::NAN;
        bus.r[9] = f32::NEG_INFINITY;
        assert!(scrub_non_finite(&mut bus, BLOCK));
        assert_eq!((bus.l[7], bus.r[9]), (0.0, 0.0));
        assert_eq!(bus.l[8], left[8]);
    }
}
