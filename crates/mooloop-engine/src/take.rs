//! A take: one channel recording its audio input into a ring.
//!
//! `docs/plans/audio-recording/03-capture.md`. Recording is clip recording,
//! started from the sampler's face (decision 7): a press arms a take, the
//! take waits for the next bar line -- the pre-roll (decision 9) -- then
//! copies its channel's audio input into a ring, sample-exact, until it is
//! stopped, the transport stops, or its clip length has been recorded
//! (decision 8). Any number of channels may be taking at once (decision 6).
//!
//! The take lives on the recording channel's strip, so an install that carries
//! the strip carries the take (`channel-identity/05`, `incremental-structure`).
//! An install that rebuilds the strip retires it with the old generation; the
//! producer is then dropped off the audio thread and the drain sees the ring
//! abandoned, which ends the take visibly rather than silently.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use mooloop_dsp::StereoBus;
use rtrb::Producer;

use crate::transport::BlockSpan;

/// One stereo frame, as the ring carries it.
pub type TakeFrame = [f32; 2];

/// Where a take is, as the control thread reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakePhase {
    /// Armed, waiting for the next bar line. The face draws this as the
    /// pre-roll.
    Waiting,
    Recording,
    /// Finished, by any route. [`TakeStatus::frames`] is final.
    Ended,
}

impl From<TakePhase> for mooloop_core::RecordFace {
    /// What the RECORD page shows for a take in this phase.
    ///
    /// `Ended` maps to `Idle` because a finished take is not on the page: the
    /// interface filters it out before it asks (`TakeView::is_live`), so
    /// `Ended` reaches this arm only if that filter changes. Written here
    /// rather than in core because `TakePhase` is this crate's and core
    /// depends on nothing (MOO-54).
    fn from(phase: TakePhase) -> Self {
        match phase {
            TakePhase::Waiting => Self::Waiting,
            TakePhase::Recording => Self::Recording,
            TakePhase::Ended => Self::Idle,
        }
    }
}

/// What a running take publishes: its phase, how many frames it has put in
/// the ring, how many it could not, and where it started.
///
/// Shared between the engine, the drain and the interface. The counts are
/// written only by the engine; the phase has one control-side writer,
/// [`Self::end`], for the quit path where the engine is going away and would
/// never write it. The drain reads [`Self::frames`] once the phase is `Ended`
/// to know when it has everything.
#[derive(Debug, Default)]
pub struct TakeStatus {
    phase: AtomicU8,
    frames: AtomicU64,
    dropped: AtomicU64,
    start_tick: AtomicU64,
}

impl TakeStatus {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn phase(&self) -> TakePhase {
        match self.phase.load(Ordering::Acquire) {
            0 => TakePhase::Waiting,
            1 => TakePhase::Recording,
            _ => TakePhase::Ended,
        }
    }

    /// Frames written into the ring so far.
    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Acquire)
    }

    /// Frames the ring had no room for. Anything but zero means the take has
    /// a gap in it and is damaged (`AUDIO_ARCHITECTURE.md`: overflow is
    /// visible to the sender, never silent).
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Acquire)
    }

    /// The bar line the take started on, in song ticks. Meaningful once the
    /// phase has left `Waiting`.
    pub fn start_tick(&self) -> f64 {
        f64::from_bits(self.start_tick.load(Ordering::Acquire))
    }

    /// End the take from the control side. The one phase write that is not
    /// the engine's, and it exists for quit: the drain leaves only when the
    /// phase is `Ended` and it has every frame, so a take still running when
    /// the engine goes away would keep a drain waiting for a stop that can no
    /// longer be sent.
    ///
    /// Deliberately *not* a general stop -- it does not touch [`Take::state`],
    /// so the engine would go on feeding a take whose status says `Ended`.
    /// Use [`Take::stop`] anywhere the engine is still alive. It is the same
    /// store `stop` makes and is idempotent, so the two racing settles on
    /// `Ended` either way.
    pub fn end(&self) {
        self.set_phase(TakePhase::Ended);
    }

    fn set_phase(&self, phase: TakePhase) {
        let value = match phase {
            TakePhase::Waiting => 0,
            TakePhase::Recording => 1,
            TakePhase::Ended => 2,
        };
        self.phase.store(value, Ordering::Release);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Waiting,
    /// `remaining` is the clip length still to record, in frames, or `None`
    /// when the take runs until it is stopped.
    Recording {
        remaining: Option<u64>,
        /// Frames still to skip before the first one is written: the start
        /// delay, counted down across block boundaries.
        skip: u64,
    },
    Ended,
}

/// One channel's take. Built on the control thread and handed to the audio
/// thread by `StructuralCommand::StartTake`.
pub struct Take {
    producer: Producer<TakeFrame>,
    status: Arc<TakeStatus>,
    /// The clip length in ticks, turned into frames at the bar line where the
    /// take starts, at that block's tempo.
    clip_ticks: Option<f64>,
    /// Frames to wait after the bar line before recording starts: the round
    /// trip latency of a hardware input, so a take lines up with what the
    /// performer heard (`03-capture.md`, "Latency"). Zero for an app source,
    /// which is read in the block that produced it.
    start_delay: u64,
    state: State,
}

impl Take {
    pub fn new(
        producer: Producer<TakeFrame>,
        status: Arc<TakeStatus>,
        clip_ticks: Option<u32>,
        start_delay_frames: u32,
    ) -> Box<Self> {
        status.set_phase(TakePhase::Waiting);
        Box::new(Self {
            producer,
            status,
            clip_ticks: clip_ticks.map(f64::from),
            start_delay: u64::from(start_delay_frames),
            state: State::Waiting,
        })
    }

    pub fn status(&self) -> &Arc<TakeStatus> {
        &self.status
    }

    /// End the take now: the record button pressed again, or the transport
    /// stopped. Idempotent.
    pub fn stop(&mut self) {
        self.state = State::Ended;
        self.status.set_phase(TakePhase::Ended);
    }

    /// Record `frames` as though a block had delivered them, without an
    /// engine: for the control side's tests of the drain, which cannot build
    /// a renderer. Starts the take if it was waiting.
    #[doc(hidden)]
    pub fn push_for_test(&mut self, frames: &[TakeFrame]) {
        if self.state == State::Waiting {
            self.state = State::Recording {
                remaining: None,
                skip: 0,
            };
            self.status.set_phase(TakePhase::Recording);
        }
        for frame in frames {
            if self.producer.push(*frame).is_ok() {
                self.status.frames.fetch_add(1, Ordering::AcqRel);
            } else {
                self.status.dropped.fetch_add(1, Ordering::AcqRel);
            }
        }
    }

    pub(crate) fn is_live(&self) -> bool {
        self.state != State::Ended
    }

    /// Advance the take by one block.
    ///
    /// `source` is the channel's audio input as it sounded this block, or
    /// `None` for silence: a source that is muted, asleep, deleted or off
    /// records silence rather than whatever its buffer last held. Audio
    /// thread: no allocation, one chunk write into the ring per recorded
    /// stretch.
    pub(crate) fn advance(
        &mut self,
        spans: &[BlockSpan],
        playing: bool,
        ticks_per_sample: f64,
        ticks_per_bar: f64,
        source: Option<&StereoBus>,
    ) {
        if self.state == State::Ended {
            return;
        }
        if !playing {
            // A stopped transport ends a take, and a take still waiting for
            // its bar is cancelled the same way: it ends with no frames.
            self.stop();
            return;
        }
        for span in spans {
            let end = span.frame + span.frames;
            let mut from = span.frame;
            if self.state == State::Waiting {
                // The first bar line *strictly after* the span's start, and
                // no later than its end. Strictly after, so a transport
                // started from a bar line waits that whole bar -- the count-in
                // -- and a bar line on a block boundary is taken, at the end
                // of the block that reaches it, as a start with no frames yet.
                let next = (span.start_tick / ticks_per_bar).floor() * ticks_per_bar
                    + ticks_per_bar;
                if next > span.end_tick + 1e-9 || ticks_per_sample <= 0.0 {
                    continue;
                }
                let offset = ((next - span.start_tick) / ticks_per_sample).round() as usize;
                from = (span.frame + offset).min(end);
                let remaining = self
                    .clip_ticks
                    .map(|ticks| (ticks / ticks_per_sample).round() as u64);
                self.state = State::Recording {
                    remaining,
                    skip: self.start_delay,
                };
                self.status
                    .start_tick
                    .store(next.to_bits(), Ordering::Release);
                self.status.set_phase(TakePhase::Recording);
            }
            let State::Recording { remaining, skip } = self.state else {
                continue;
            };
            // The start delay first: it moves where the take begins, and the
            // clip length counts from there.
            let skipped = skip.min((end - from) as u64);
            from += skipped as usize;
            if skip > 0 {
                self.state = State::Recording {
                    remaining,
                    skip: skip - skipped,
                };
                if skipped < skip {
                    continue;
                }
            }
            let mut count = (end - from) as u64;
            if let Some(remaining) = remaining {
                count = count.min(remaining);
            }
            self.write(source, from, count as usize);
            match remaining {
                Some(remaining) if remaining <= count => {
                    self.stop();
                    return;
                }
                Some(remaining) => {
                    self.state = State::Recording {
                        remaining: Some(remaining - count),
                        skip: 0,
                    };
                }
                None => {}
            }
        }
    }

    fn write(&mut self, source: Option<&StereoBus>, from: usize, count: usize) {
        if count == 0 {
            return;
        }
        let room = count.min(self.producer.slots());
        if room > 0 {
            if let Ok(chunk) = self.producer.write_chunk_uninit(room) {
                let frames = (from..from + room).map(|frame| match source {
                    Some(bus) => [bus.l[frame], bus.r[frame]],
                    None => [0.0, 0.0],
                });
                chunk.fill_from_iter(frames);
            }
        }
        self.status.frames.fetch_add(room as u64, Ordering::AcqRel);
        if room < count {
            self.status
                .dropped
                .fetch_add((count - room) as u64, Ordering::AcqRel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAR: f64 = 384.0;

    fn span(frame: usize, frames: usize, start_tick: f64, tps: f64) -> BlockSpan {
        BlockSpan {
            frame,
            frames,
            start_tick,
            end_tick: start_tick + frames as f64 * tps,
            jumped: false,
        }
    }

    fn ramp(frames: usize) -> StereoBus {
        let mut bus = StereoBus::with_capacity(frames);
        for i in 0..frames {
            bus.l[i] = i as f32;
            bus.r[i] = -(i as f32);
        }
        bus
    }

    fn take(capacity: usize, clip: Option<u32>) -> (Box<Take>, rtrb::Consumer<TakeFrame>) {
        let (producer, consumer) = rtrb::RingBuffer::new(capacity);
        (Take::new(producer, TakeStatus::new(), clip, 0), consumer)
    }

    /// Pressed mid-bar, the take starts on the next bar line, at the frame
    /// that line falls on, and records from there.
    #[test]
    fn a_take_starts_on_the_next_bar_line_to_the_frame() {
        let (mut take, mut rx) = take(1024, None);
        let bus = ramp(64);
        // One tick per frame; the block covers ticks 370..434, so the bar
        // line at 384 is frame 14.
        take.advance(&[span(0, 64, 370.0, 1.0)], true, 1.0, BAR, Some(&bus));
        assert_eq!(take.status.phase(), TakePhase::Recording);
        assert_eq!(take.status.start_tick(), 384.0);
        assert_eq!(take.status.frames(), 50);
        assert_eq!(rx.pop().unwrap(), [14.0, -14.0]);
    }

    /// Started from a parked bar line, the whole bar is the count-in.
    #[test]
    fn a_take_started_on_a_bar_line_waits_the_whole_bar() {
        let (mut take, _rx) = take(1024, None);
        take.advance(&[span(0, 64, 0.0, 1.0)], true, 1.0, BAR, None);
        assert_eq!(take.status.phase(), TakePhase::Waiting);
        take.advance(&[span(0, 320, 64.0, 1.0)], true, 1.0, BAR, None);
        assert_eq!(take.status.phase(), TakePhase::Recording, "the bar line at the block end");
        assert_eq!(take.status.frames(), 0);
        assert_eq!(take.status.start_tick(), 384.0);
    }

    /// A clip length ends the take by itself, to the frame.
    #[test]
    fn a_clip_length_ends_the_take_to_the_frame() {
        let (mut take, _rx) = take(4096, Some(100));
        let bus = ramp(256);
        take.advance(&[span(0, 64, 350.0, 1.0)], true, 1.0, BAR, Some(&bus));
        assert_eq!(take.status.frames(), 30);
        take.advance(&[span(0, 256, 414.0, 1.0)], true, 1.0, BAR, Some(&bus));
        assert_eq!(take.status.frames(), 100);
        assert_eq!(take.status.phase(), TakePhase::Ended);
    }

    #[test]
    fn a_stopped_transport_ends_the_take() {
        let (mut take, _rx) = take(1024, None);
        take.advance(&[span(0, 64, 370.0, 1.0)], true, 1.0, BAR, None);
        take.advance(&[span(0, 64, 434.0, 0.0)], false, 1.0, BAR, None);
        assert_eq!(take.status.phase(), TakePhase::Ended);
        assert!(!take.is_live());
    }

    /// A ring the drain has not kept up with drops frames and counts them.
    #[test]
    fn a_full_ring_counts_what_it_drops() {
        let (mut take, _rx) = take(10, None);
        let bus = ramp(64);
        take.advance(&[span(0, 64, 383.0, 1.0)], true, 1.0, BAR, Some(&bus));
        assert_eq!(take.status.frames(), 10);
        assert_eq!(take.status.dropped(), 53);
    }

    /// A hardware input's start is delayed by its round trip, across a block
    /// boundary if need be, and the clip length counts from there.
    #[test]
    fn a_start_delay_moves_the_first_frame_and_the_clip_counts_from_it() {
        let (producer, mut rx) = rtrb::RingBuffer::new(1024);
        let mut take = Take::new(producer, TakeStatus::new(), Some(40), 30);
        let bus = ramp(64);
        // Bar line at frame 14; the take starts 30 frames later, at 44.
        take.advance(&[span(0, 64, 370.0, 1.0)], true, 1.0, BAR, Some(&bus));
        assert_eq!(rx.pop().unwrap(), [44.0, -44.0]);
        assert_eq!(take.status.frames(), 20);
        // A delay longer than the rest of the block carries into the next.
        let (producer, mut rx) = rtrb::RingBuffer::new(1024);
        let mut late = Take::new(producer, TakeStatus::new(), None, 60);
        late.advance(&[span(0, 64, 370.0, 1.0)], true, 1.0, BAR, Some(&bus));
        assert_eq!(late.status.frames(), 0, "still inside the delay");
        late.advance(&[span(0, 64, 434.0, 1.0)], true, 1.0, BAR, Some(&bus));
        assert_eq!(rx.pop().unwrap(), [10.0, -10.0], "the last 10 frames of the delay, then block 2 from frame 10");
        take.advance(&[span(0, 64, 434.0, 1.0)], true, 1.0, BAR, Some(&bus));
        assert_eq!(take.status.frames(), 40, "the clip counted from the delayed start");
    }

    /// A muted, asleep or missing source records silence rather than stale
    /// audio.
    #[test]
    fn no_source_records_silence() {
        let (mut take, mut rx) = take(1024, None);
        take.advance(&[span(0, 64, 383.0, 1.0)], true, 1.0, BAR, None);
        assert_eq!(rx.pop().unwrap(), [0.0, 0.0]);
    }
}
