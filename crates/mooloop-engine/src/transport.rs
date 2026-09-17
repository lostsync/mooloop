//! Transport clock. Lives on the realtime thread.
//!
//! Two position representations, both updated together:
//! - `position_ticks: f64` — musical time. Fractional so tempo changes don't
//!   cause phase quantisation noise; the scheduler converts tick deltas to
//!   sample offsets per block, which keeps note timing sample-accurate as
//!   long as tempo only changes between blocks (it does — commands drain at
//!   block start).
//! - `frames_played: u64` — absolute frames since transport start. Ground
//!   truth that never accumulates float error; the future tempo-map /
//!   playlist layer will anchor on this.
//!
//! The transport only moves the clock; scheduling note events for the step
//! grid is the job of [`crate::sequencer::Sequencer`], which reads the
//! before/after tick each block.
//!
//! It is also where the song loop lives, and that is a deliberate choice
//! rather than an incidental one. The sequencer folds every position into a
//! period already -- the current pattern's length, or the arrangement's --
//! so a loop expressed there would have been a second fold layered on the
//! first, and every reader of a position would have had to know about both.
//! Folded here instead, the position handed downstream is an ordinary
//! arrangement tick inside the loop, and nothing below this file learns that
//! looping exists. The price is that one process block can span the loop
//! point, so a block is no longer one stretch of musical time but a short
//! ordered list of them.

use mooloop_core::{ticks_per_sample, BbtPosition, Ppq, Ticks};

/// How many stretches of musical time one process block may be cut into.
///
/// A block is cut once per loop pass it contains. The largest block the
/// engine accepts is 8192 frames, which is 170 ms at 48 kHz, so reaching even
/// two of these means a loop shorter than a sixteenth at an ordinary tempo.
/// The cap exists because the audio thread cannot grow a list, not because
/// the count is expected to climb.
pub const MAX_BLOCK_SPANS: usize = 8;

/// One contiguous stretch of musical time inside one process block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockSpan {
    /// Frame within the block at which this stretch begins.
    pub frame: usize,
    /// Frames it covers. The scheduler needs this to place an event's offset,
    /// and it is not `end - start` in ticks because tempo is per-block.
    pub frames: usize,
    pub start_tick: f64,
    pub end_tick: f64,
    /// Whether the transport jumped to `start_tick` rather than arriving
    /// there. Every voice sounding at a jump has to be released: the
    /// note-offs it was waiting for sit at positions the transport is no
    /// longer travelling towards.
    pub jumped: bool,
}

impl BlockSpan {
    const SILENT: Self = Self {
        frame: 0,
        frames: 0,
        start_tick: 0.0,
        end_tick: 0.0,
        jumped: false,
    };
}

pub struct Transport {
    pub playing: bool,
    pub bpm: f64,
    pub sample_rate: u32,
    pub ppq: Ppq,
    pub position_ticks: f64,
    frames_played: u64,
}

impl Transport {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            playing: false,
            bpm: 120.0,
            sample_rate,
            ppq: Ppq::DEFAULT,
            position_ticks: 0.0,
            frames_played: 0,
        }
    }

    pub fn ticks_per_sample(&self) -> f64 {
        ticks_per_sample(self.bpm, self.sample_rate, self.ppq)
    }

    /// Absolute frames rendered since the transport last started from zero.
    pub fn frames_played(&self) -> u64 {
        self.frames_played
    }

    /// Current beat index within the bar (0-based).
    ///
    /// Zero-based where [`BbtPosition`] is one-based, because that is this
    /// method's long-standing contract and its callers -- `RenderReport`, the
    /// executor, the bridge, the UI -- count from zero. The derivation is
    /// shared rather than hand-rolled: it and `Session::transport_position`
    /// used to compute this independently in two crates and agreed only
    /// because two people had written the same 4.
    ///
    /// A negative position floors to zero rather than wrapping. [`Self::seek`]
    /// refuses a negative tick and advancing only adds, so the old
    /// `rem_euclid` was defending against a position nothing can produce.
    pub fn beat_in_bar(&self) -> u8 {
        let ticks = Ticks(self.position_ticks.max(0.0) as u64);
        BbtPosition::from_ticks(ticks, self.ppq).beat as u8 - 1
    }

    /// Advance the clock by `frames` samples with no loop installed,
    /// returning the `(start, end)` tick interval covered.
    ///
    /// The shorthand the tests are written against; the engine itself always
    /// goes through [`Self::advance_looped`], because whether a loop is
    /// installed is a per-block question there.
    #[cfg(test)]
    pub fn advance(&mut self, frames: usize) -> (f64, f64) {
        let (spans, _) = self.advance_looped(frames, None);
        (spans[0].start_tick, spans[0].end_tick)
    }

    /// Advance the clock by `frames` samples, returning to `loop_start` every
    /// time the block reaches `loop_end`.
    ///
    /// Returns the stretches of musical time the block covers and how many of
    /// them there are; the first always exists, and without a loop it is the
    /// only one and this is exactly [`Self::advance`]. A position already at
    /// or past the loop end is brought back at the top of the block rather
    /// than left to run away, which is what makes dragging the loop end back
    /// behind a running playhead, or seeking past it, recover on the next
    /// block instead of never.
    ///
    /// Past [`MAX_BLOCK_SPANS`] the remainder of the block plays straight on
    /// without folding. That leaves the position past the loop end, which the
    /// next block's opening fold catches, so an absurdly short loop under an
    /// absurdly long block degrades to a slower loop rather than to a fault.
    pub fn advance_looped(
        &mut self,
        frames: usize,
        loop_range: Option<(f64, f64)>,
    ) -> ([BlockSpan; MAX_BLOCK_SPANS], usize) {
        let mut spans = [BlockSpan::SILENT; MAX_BLOCK_SPANS];
        // A stopped transport holds its position, loop or no loop. Folding it
        // in here too would move a playhead the user parked past the loop
        // end on purpose, and a parked playhead is exactly what pressing play
        // from somewhere is.
        if !self.playing {
            spans[0] = BlockSpan {
                frame: 0,
                frames,
                start_tick: self.position_ticks,
                end_tick: self.position_ticks,
                jumped: false,
            };
            return (spans, 1);
        }
        let mut jumped = false;
        if let Some((start, end)) = loop_range {
            if self.position_ticks >= end {
                self.position_ticks = start;
                jumped = true;
            }
        }
        self.frames_played += frames as u64;

        let ticks_per_sample = self.ticks_per_sample();
        let mut count = 0;
        let mut frame = 0;
        while count < MAX_BLOCK_SPANS {
            let start_tick = self.position_ticks;
            let remaining = frames - frame;
            // The last span the cap allows never folds, so the loop end is
            // not consulted for it and the block finishes where it would
            // have without a loop at all.
            let split = loop_range
                .filter(|_| count + 1 < MAX_BLOCK_SPANS)
                .filter(|(_, end)| start_tick < *end)
                .map(|(_, end)| {
                    // The first frame whose tick would have reached the loop
                    // end. Nudged before rounding up because the position is
                    // an accumulated float: a block landing exactly on the
                    // loop end computes a whole number of frames plus a few
                    // parts in 10^13, and a bare `ceil` would spend a whole
                    // extra frame on the strength of it. A frame is 20 us and
                    // the error never is, so erring early is free.
                    ((end - start_tick) / ticks_per_sample - 1e-6).ceil().max(0.0) as usize
                })
                .filter(|split| *split < remaining);
            let span_frames = split.unwrap_or(remaining);
            let end_tick = match split {
                // Cut at the loop end exactly: the frame the fold happens on
                // is the first whose tick would have passed it, so the span
                // this closes owns every event up to the loop point and none
                // at it.
                Some(_) => loop_range.expect("split implies a loop").1,
                None => start_tick + span_frames as f64 * ticks_per_sample,
            };
            spans[count] = BlockSpan {
                frame,
                frames: span_frames,
                start_tick,
                end_tick,
                jumped: jumped || count > 0,
            };
            count += 1;
            frame += span_frames;
            self.position_ticks = match split {
                Some(_) => loop_range.expect("split implies a loop").0,
                None => end_tick,
            };
            if split.is_none() {
                break;
            }
        }
        (spans, count)
    }

    /// Move to an absolute tick without changing whether the transport runs.
    ///
    /// Non-finite and negative positions are refused rather than clamped: the
    /// only way to produce one is a defective caller, and silently landing at
    /// zero would hide it behind a plausible-looking jump to the start.
    pub fn seek(&mut self, tick: f64) {
        if tick.is_finite() && tick >= 0.0 {
            self.position_ticks = tick;
        }
    }

    pub fn play(&mut self) {
        self.playing = true;
    }

    pub fn pause(&mut self) {
        self.playing = false;
    }

    pub fn stop(&mut self) {
        self.playing = false;
        self.position_ticks = 0.0;
        self.frames_played = 0;
    }

    /// Take `other`'s running state: whether it plays, where it is, and how
    /// long it has been going.
    ///
    /// For a project install that must not interrupt the song -- a channel
    /// paste, delete or move. Tempo, sample rate and timebase are deliberately
    /// **not** taken: those belong to the project being installed, and a
    /// structural edit may well have changed the tempo in the same gesture.
    ///
    /// `frames_played` travels with the position because the two are one
    /// answer. Left behind, the incoming transport would report the song as
    /// having just started while its playhead sat two minutes in, and
    /// everything deriving elapsed time from it would step.
    pub fn adopt_running_state(&mut self, other: &Self) {
        self.playing = other.playing;
        self.position_ticks = other.position_ticks;
        self.frames_played = other.frames_played;
    }

    pub fn set_tempo(&mut self, bpm: f64) {
        self.bpm = bpm.clamp(1.0, 999.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_holds_both_clocks() {
        let mut t = Transport::new(48_000);
        t.play();
        let (s, e) = t.advance(256);
        assert_eq!(s, 0.0);
        assert!(e > s);
        assert_eq!(t.frames_played(), 256);
        t.pause();
        let (s2, e2) = t.advance(256);
        assert_eq!(s2, e2);
        assert_eq!(
            t.frames_played(),
            256,
            "paused transport must not count frames"
        );
        t.stop();
        assert_eq!(t.position_ticks, 0.0);
        assert_eq!(t.frames_played(), 0);
    }

    /// The loop end is a tick the transport leaves, never one it plays, and
    /// the frame it leaves on is the first that would have passed it.
    #[test]
    fn a_block_crossing_the_loop_end_is_cut_in_two() {
        let mut t = Transport::new(48_000);
        t.play();
        let tps = t.ticks_per_sample();
        // Start two hundred frames short of the loop end.
        let end = 400.0;
        t.seek(end - 200.0 * tps);
        let (spans, count) = t.advance_looped(512, Some((100.0, end)));

        assert_eq!(count, 2, "one loop pass inside the block is two spans");
        assert_eq!(spans[0].frame, 0);
        assert_eq!(spans[0].end_tick, end, "the first span closes on the loop end");
        assert!(!spans[0].jumped);
        assert_eq!(spans[1].frame, 200);
        assert_eq!(spans[1].frames, 312);
        assert_eq!(spans[1].start_tick, 100.0, "the second resumes at the loop start");
        assert!(spans[1].jumped, "the resume is a jump and owes a release");
        assert_eq!(spans[0].frames + spans[1].frames, 512, "the block is covered once");
        assert_eq!(t.position_ticks, 100.0 + 312.0 * tps);
        assert_eq!(t.frames_played(), 512, "looping does not rewind elapsed time");
    }

    /// Dragging the loop end back behind a running playhead, or seeking past
    /// it, has to recover on the next block rather than never.
    #[test]
    fn a_position_past_the_loop_end_is_brought_back_at_the_top_of_the_block() {
        let mut t = Transport::new(48_000);
        t.play();
        t.seek(9_000.0);
        let (spans, count) = t.advance_looped(256, Some((100.0, 400.0)));
        assert_eq!(count, 1);
        assert_eq!(spans[0].start_tick, 100.0);
        assert!(spans[0].jumped);
    }

    /// A stopped transport holds where it was put. Playing from a position
    /// before the loop plays into it rather than being teleported inside.
    #[test]
    fn a_parked_playhead_is_left_where_it_is() {
        let mut t = Transport::new(48_000);
        t.seek(9_000.0);
        let (spans, count) = t.advance_looped(256, Some((100.0, 400.0)));
        assert_eq!(count, 1);
        assert_eq!(spans[0].start_tick, 9_000.0);
        assert!(!spans[0].jumped);
        assert_eq!(t.position_ticks, 9_000.0);

        t.seek(0.0);
        t.play();
        let (spans, count) = t.advance_looped(64, Some((100.0, 400.0)));
        assert_eq!(count, 1);
        assert_eq!(spans[0].start_tick, 0.0);
        assert!(!spans[0].jumped, "arriving is not jumping");
    }

    /// A loop shorter than one block must still terminate, and must leave the
    /// position somewhere the next block's opening fold can recover from.
    #[test]
    fn a_loop_shorter_than_the_block_is_capped_rather_than_unbounded() {
        let mut t = Transport::new(48_000);
        t.play();
        let range = Some((0.0, 1.0));
        let (spans, count) = t.advance_looped(4_096, range);

        assert_eq!(count, MAX_BLOCK_SPANS);
        assert_eq!(
            spans[..count].iter().map(|span| span.frames).sum::<usize>(),
            4_096,
            "every frame of the block belongs to exactly one span"
        );
        assert!(
            t.position_ticks > 1.0,
            "the capped tail runs past the loop end, for the next block to fold"
        );
        let (spans, _) = t.advance_looped(64, range);
        assert_eq!(spans[0].start_tick, 0.0, "and the next block folds it");
    }

    /// The contract between this crate's beat-in-bar and `mooloop-core`'s.
    ///
    /// These were two hand-rolled derivations in two crates -- this one and
    /// `Session::transport_position` -- reading the same clock and agreeing
    /// only because two people had independently written `4`. They share
    /// `BbtPosition` now, and this is the check that would have reported them
    /// parting.
    #[test]
    fn beat_in_bar_is_the_cores_bbt_position_counted_from_zero() {
        let mut t = Transport::new(48_000);
        let ticks_per_beat = u64::from(t.ppq.ticks_per_beat());
        let mut checked = 0;
        // Five bars, landing on beat boundaries and between them.
        let mut tick = 0;
        while tick < ticks_per_beat * u64::from(mooloop_core::BEATS_PER_BAR) * 5 {
            t.position_ticks = tick as f64;
            let expected = BbtPosition::from_ticks(Ticks(tick), t.ppq).beat - 1;
            assert_eq!(
                u32::from(t.beat_in_bar()),
                expected,
                "the two derivations parted at tick {tick}"
            );
            checked += 1;
            tick += 13;
        }
        assert_eq!(
            checked, 148,
            "the sweep stopped covering five bars, so it proved nothing"
        );

        // A fractional position floors to the beat it is inside, which is what
        // `position_ticks` being an f64 means for a readout.
        t.position_ticks = (ticks_per_beat as f64) * 2.75;
        assert_eq!(t.beat_in_bar(), 2);

        // Negative positions floor to zero rather than wrapping. `seek`
        // refuses one and advancing only adds, so this is the posture the old
        // `rem_euclid` had, not a case the engine can reach.
        t.position_ticks = -(ticks_per_beat as f64);
        assert_eq!(t.beat_in_bar(), 0);
        t.position_ticks = f64::NAN;
        assert_eq!(t.beat_in_bar(), 0);
    }

    /// A seek refuses what it cannot represent instead of landing at zero,
    /// where a real jump to the start would be indistinguishable from a bug.
    #[test]
    fn a_seek_refuses_a_position_that_is_not_one() {
        let mut t = Transport::new(48_000);
        t.seek(500.0);
        t.seek(f64::NAN);
        t.seek(-1.0);
        assert_eq!(t.position_ticks, 500.0);
    }
}
