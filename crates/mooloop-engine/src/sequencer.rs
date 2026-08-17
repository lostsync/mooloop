//! Step-grid sequencer with a pattern bank. Lives on the realtime thread.
//!
//! Holds a fixed, pre-allocated bank of patterns (each with a row per possible
//! channel) so `SetStep`, pattern switching and channel add/remove only ever
//! mutate existing memory — no allocation on the RT thread.
//!
//! Each block, the engine hands the transport's tick interval
//! `[start_tick, end_tick)`. The sequencer finds every step boundary in that
//! interval and, for steps that are on in the **current** pattern, pushes a
//! sample-timed `NoteOn` with that step's pitch and velocity into each
//! channel's [`EventList`]. Nodes render from those lists, so note timing is
//! sample-accurate.
//!
//! ## Boundary correctness
//!
//! The transport position is `f64`, so block boundaries land a hair either
//! side of the exact step tick. The interval is half-open (`[start, end)`,
//! so every boundary belongs to exactly one block) and boundary detection is
//! epsilon-tolerant ([`BOUNDARY_EPS`]): a start position that float-drifted a
//! femto-hair past a step boundary still counts that step, at offset 0.
//!
//! Step resolution is 16th notes: `ticks_per_step = ppq / 4`. Steps wrap at
//! the pattern length, so a 16-step pattern loops every bar.

use mooloop_core::{Pattern, Ppq, MAX_CHANNELS, MAX_PATTERN_STEPS};
use mooloop_dsp::{Event, EventList, TimedEvent};

/// Tolerance for float drift at step boundaries, in ticks. At 120 bpm one
/// sample is ~0.004 ticks, so 1e-6 is ~4000x below sample resolution — large
/// enough to absorb `f64` accumulation error, small enough to never swallow
/// a genuine step.
const BOUNDARY_EPS: f64 = 1.0e-6;

pub struct Sequencer {
    patterns: Vec<Pattern>,
    current: usize,
    active_channels: usize,
    ppq: Ppq,
}

impl Sequencer {
    /// Build a bank of `num_patterns` patterns, each with `MAX_CHANNELS` rows
    /// and `num_steps` steps, with `initial_channels` rows active.
    pub fn new(initial_channels: usize, num_patterns: usize, num_steps: usize, ppq: Ppq) -> Self {
        let patterns = (0..num_patterns)
            .map(|_| {
                let mut pattern = Pattern::with_steps(MAX_CHANNELS, MAX_PATTERN_STEPS as usize);
                pattern.set_length_steps(num_steps);
                pattern
            })
            .collect();
        Self {
            patterns,
            current: 0,
            active_channels: initial_channels.min(MAX_CHANNELS),
            ppq,
        }
    }

    pub fn set_current_pattern(&mut self, pattern: usize) {
        if pattern < self.patterns.len() {
            self.current = pattern;
        }
    }

    pub fn set_pattern_length(&mut self, pattern: usize, length_steps: usize) {
        if let Some(pattern) = self.patterns.get_mut(pattern) {
            pattern.set_length_steps(length_steps);
        }
    }

    pub fn active_channels(&self) -> usize {
        self.active_channels
    }

    pub fn set_active_channels(&mut self, n: usize) {
        self.active_channels = n.min(MAX_CHANNELS);
    }

    /// Apply a step change coming in from the UI. Addressed by pattern so
    /// edits to non-playing patterns take effect when they're selected.
    pub fn set_step(
        &mut self,
        pattern: usize,
        channel: usize,
        step: usize,
        on: bool,
        note: u8,
        velocity: u8,
    ) {
        if let Some(pat) = self.patterns.get_mut(pattern) {
            if let Some(ch) = pat.channel_mut(channel) {
                if let Some(s) = ch.steps.get_mut(step) {
                    s.on = on;
                    s.note = note.min(127);
                    s.velocity = velocity;
                }
            }
        }
    }

    fn ticks_per_step(&self) -> f64 {
        (self.ppq.ticks_per_beat() / 4) as f64
    }

    /// For each step boundary in `[start, end)`, push note-ons into
    /// `events` (one list per channel) for active steps of the current
    /// pattern. `ticks_per_sample` converts tick deltas to sample offsets
    /// inside the block. Events are pushed in time order.
    pub fn schedule(
        &self,
        start_tick: f64,
        end_tick: f64,
        frames: usize,
        ticks_per_sample: f64,
        events: &mut [EventList],
    ) {
        if !end_tick.is_finite() || end_tick <= start_tick {
            return;
        }
        let Some(pattern) = self.patterns.get(self.current) else {
            return;
        };

        let tps = self.ticks_per_step();
        let pattern_steps = pattern.length_steps.max(1) as i64;

        // First step boundary at or after `start` — epsilon-tolerant so
        // float drift past a boundary doesn't skip the step (see module
        // docs). Half-open interval: a boundary exactly at `start` is ours;
        // one exactly at `end` belongs to the next block.
        let mut step_idx = ((start_tick - BOUNDARY_EPS) / tps).ceil() as i64;
        let mut step_tick = step_idx as f64 * tps;

        while step_tick < end_tick {
            let offset = ((step_tick - start_tick) / ticks_per_sample).round() as i64;
            // Clamp absorbs epsilon drift (a boundary a femto-sample before
            // the block start rounds to a tiny negative offset) and
            // sub-sample round-up at the block end.
            let offset = offset.clamp(0, frames as i64 - 1) as u32;
            {
                let step_in_pattern = step_idx.rem_euclid(pattern_steps) as usize;
                for ch_idx in 0..self.active_channels.min(events.len()) {
                    let Some((note, velocity)) = pattern
                        .channel(ch_idx)
                        .and_then(|c| c.steps.get(step_in_pattern))
                        .filter(|s| s.on)
                        .map(|s| (s.note, s.velocity))
                    else {
                        continue;
                    };
                    events[ch_idx].push(TimedEvent {
                        offset,
                        event: Event::NoteOn { note, velocity },
                    });
                }
            }
            step_idx += 1;
            step_tick = step_idx as f64 * tps;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::ticks_per_sample;

    /// Total note-ons scheduled over an exact number of bars must equal the
    /// exact number of active steps passed — no float-drift skips or doubles.
    #[test]
    fn no_skips_or_doubles_over_long_run() {
        let sr = 48_000u32;
        let bpm = 120.0;
        let ppq = Ppq::DEFAULT;
        let tps = ticks_per_sample(bpm, sr, ppq);

        let mut seq = Sequencer::new(1, 8, 16, ppq);
        for s in [0, 4, 8, 12] {
            seq.set_step(0, 0, s, true, 60, 100);
        }

        // 100 bars of irregular block sizes (simulating PipeWire quantum
        // changes), then check the event count is exact.
        let mut events = [EventList::empty()];
        let mut tick = 0.0f64;
        let mut total = 0usize;
        let blocks_per_bar = 47; // deliberately coprime-ish
        let block = (sr as f64 * 2.0 / blocks_per_bar as f64) as usize;
        for _ in 0..100 * blocks_per_bar {
            events[0].clear();
            let start = tick;
            let end = start + block as f64 * tps;
            seq.schedule(start, end, block, tps, &mut events);
            total += events[0].len();
            tick = end;
        }
        // 4 hits per bar * 100 bars = 400, minus whatever of the last bar was
        // truncated by the final partial block — assert within one step.
        assert!(
            (395..=400).contains(&total),
            "expected ~400 note-ons, got {total}"
        );
    }

    /// Boundary exactly at block start must fire at offset 0.
    #[test]
    fn boundary_at_block_start_fires_at_zero() {
        let sr = 48_000u32;
        let ppq = Ppq::DEFAULT;
        let tps = ticks_per_sample(120.0, sr, ppq);
        let ticks_per_step = (ppq.ticks_per_beat() / 4) as f64;

        let mut seq = Sequencer::new(1, 8, 16, ppq);
        seq.set_step(0, 0, 0, true, 60, 100);

        // Start exactly on the boundary of step index 16 (which wraps to
        // pattern step 0), plus a hair of float drift both ways.
        for drift in [-BOUNDARY_EPS * 0.5, 0.0, BOUNDARY_EPS * 0.5] {
            let start = 16.0 * ticks_per_step + drift;
            let end = start + 512.0 * tps;
            let mut events = [EventList::empty()];
            seq.schedule(start, end, 512, tps, &mut events);
            let first = events[0]
                .iter()
                .next()
                .unwrap_or_else(|| panic!("drift {drift}: no event scheduled"));
            assert_eq!(first.offset, 0, "drift {drift}");
        }
    }

    /// Steps edited into a non-current pattern must not schedule until that
    /// pattern is selected — and must schedule once it is.
    #[test]
    fn pattern_bank_isolation() {
        let sr = 48_000u32;
        let ppq = Ppq::DEFAULT;
        let tps = ticks_per_sample(120.0, sr, ppq);
        let mut seq = Sequencer::new(1, 8, 16, ppq);
        for s in 0..16 {
            seq.set_step(1, 0, s, true, 60, 100);
        }

        let count_bar = |seq: &Sequencer| {
            let mut events = [EventList::empty()];
            let bar_ticks = 16.0 * (ppq.ticks_per_beat() / 4) as f64;
            let frames = (bar_ticks / tps) as usize;
            let mut total = 0;
            let mut tick = 0.0;
            // 8 blocks per bar.
            for _ in 0..8 {
                events[0].clear();
                let (start, end) = (tick, tick + bar_ticks / 8.0);
                seq.schedule(start, end, frames / 8, tps, &mut events);
                total += events[0].len();
                tick = end;
            }
            total
        };

        assert_eq!(count_bar(&seq), 0, "current pattern is empty");
        seq.set_current_pattern(1);
        assert_eq!(count_bar(&seq), 16, "all steps of pattern 1 are on");
    }

    #[test]
    fn each_pattern_wraps_at_its_own_length() {
        let ppq = Ppq::DEFAULT;
        let tps = ticks_per_sample(120.0, 48_000, ppq);
        let ticks_per_step = (ppq.ticks_per_beat() / 4) as f64;
        let mut seq = Sequencer::new(1, 2, 16, ppq);
        seq.set_step(0, 0, 0, true, 48, 91);
        seq.set_pattern_length(0, 12);

        let mut events = [EventList::empty()];
        seq.schedule(
            12.0 * ticks_per_step,
            13.0 * ticks_per_step,
            (ticks_per_step / tps) as usize,
            tps,
            &mut events,
        );

        assert_eq!(events[0].len(), 1, "step zero repeats after twelve steps");
        let event = events[0].iter().next().unwrap();
        assert_eq!(event.offset, 0);
        assert_eq!(
            event.event,
            Event::NoteOn {
                note: 48,
                velocity: 91
            }
        );
    }
}
