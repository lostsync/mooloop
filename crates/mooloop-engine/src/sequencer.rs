//! Step-grid sequencer with a pattern bank. Lives on the realtime thread.
//!
//! Holds a fixed, pre-allocated bank of patterns (each with a row per possible
//! channel) so `SetStep`, pattern switching and channel add/remove only ever
//! mutate existing memory — no allocation on the RT thread.
//!
//! Each block, the engine hands the transport's tick interval
//! `[start_tick, end_tick)`. The sequencer finds every step boundary in that
//! interval and, for steps that are on in the **current** pattern, schedules a
//! note-on into the channel's device at the matching sample offset.
//!
//! Step resolution is 16th notes: `ticks_per_step = ppq / 4`. Steps wrap at
//! the pattern length, so a 16-step pattern loops every bar.

use mooloop_core::{Pattern, Ppq, MAX_CHANNELS};
use mooloop_dsp::Device;

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
            .map(|_| Pattern::with_steps(MAX_CHANNELS, num_steps))
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

    pub fn active_channels(&self) -> usize {
        self.active_channels
    }

    pub fn set_active_channels(&mut self, n: usize) {
        self.active_channels = n.min(MAX_CHANNELS);
    }

    /// Apply a step change coming in from the UI. Addressed by pattern so
    /// edits to non-playing patterns take effect when they're selected.
    pub fn set_step(&mut self, pattern: usize, channel: usize, step: usize, on: bool, velocity: u8) {
        if let Some(pat) = self.patterns.get_mut(pattern) {
            if let Some(ch) = pat.channel_mut(channel) {
                if let Some(s) = ch.steps.get_mut(step) {
                    s.on = on;
                    s.velocity = velocity;
                }
            }
        }
    }

    fn ticks_per_step(&self) -> f64 {
        (self.ppq.ticks_per_beat() / 4) as f64
    }

    /// For each step boundary in `[start, end)`, schedule note-ons into
    /// `devices` for any active step of the current pattern.
    /// `ticks_per_sample` converts tick deltas to sample offsets inside the
    /// block.
    pub fn schedule<D: Device>(
        &self,
        start_tick: f64,
        end_tick: f64,
        frames: usize,
        ticks_per_sample: f64,
        devices: &mut [D],
    ) {
        if !end_tick.is_finite() || end_tick <= start_tick {
            return;
        }
        let Some(pattern) = self.patterns.get(self.current) else {
            return;
        };

        let tps = self.ticks_per_step();
        let pattern_steps = pattern.length_steps.max(1) as i64;

        // First step boundary at or after `start` (half-open: a boundary
        // exactly at `start` is ours; the previous block's `[., start)` did
        // not include it).
        let mut step_idx = (start_tick / tps).ceil() as i64;
        let mut step_tick = step_idx as f64 * tps;

        while step_tick < end_tick {
            let offset = ((step_tick - start_tick) / ticks_per_sample).round() as i64;
            if (0..frames as i64).contains(&offset) {
                let step_in_pattern = step_idx.rem_euclid(pattern_steps) as usize;
                for ch_idx in 0..self.active_channels.min(devices.len()) {
                    let Some(velocity) = pattern
                        .channel(ch_idx)
                        .and_then(|c| c.steps.get(step_in_pattern))
                        .filter(|s| s.on)
                        .map(|s| s.velocity)
                    else {
                        continue;
                    };
                    // Root note 60 (C4); per-step pitch arrives with the
                    // piano roll.
                    devices[ch_idx].note_on(offset as usize, 60, velocity);
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
    use mooloop_core::{ticks_per_sample, SamplerParams};
    use mooloop_dsp::{ProcessContext, SampleData, Sampler};
    use std::sync::Arc;

    /// Drive Sequencer -> Sampler offline exactly the way `Graph::process`
    /// does, and verify audible output appears at the right density.
    #[test]
    fn sequencer_drives_sampler() {
        let sr = 48_000u32;
        let bpm = 120.0;
        let ppq = Ppq::DEFAULT;
        let tps = ticks_per_sample(bpm, sr, ppq);
        let steps = 16;

        let mut seq = Sequencer::new(1, 8, steps, ppq);
        for s in [0, 4, 8, 12] {
            seq.set_step(0, 0, s, true, 100);
        }

        let kick = SampleData::default_kick(sr);
        let slot: Arc<arc_swap::ArcSwapOption<SampleData>> =
            Arc::new(arc_swap::ArcSwapOption::from(Some(kick)));
        let sampler = Sampler::new(slot, SamplerParams::default(), sr);
        let mut devices = [sampler];

        let block = 1024usize;
        let mut tick = 0.0f64;
        let mut max_peak = 0.0f32;
        let mut nonzero_blocks = 0usize;

        // 10 seconds of audio.
        for _ in 0..(10 * sr as usize / block) {
            let start = tick;
            let end = start + block as f64 * tps;
            tick = end;

            seq.schedule(start, end, block, tps, &mut devices);

            let mut l = vec![0.0f32; block];
            let mut r = vec![0.0f32; block];
            let ctx = ProcessContext {
                sample_rate: sr,
                frames: block,
            };
            devices[0].process(ctx, &mut l, &mut r);

            let peak = l.iter().fold(0.0f32, |a, x| a.max(x.abs()));
            if peak > 0.0 {
                nonzero_blocks += 1;
            }
            max_peak = max_peak.max(peak);
        }

        // Four-on-the-floor at 120 bpm = 2 hits/second = ~20 hits in 10 s.
        assert!(nonzero_blocks >= 20, "too few nonzero blocks: {nonzero_blocks}");
        assert!(max_peak > 0.1, "output too quiet: {max_peak}");
    }

    /// Steps edited into a non-current pattern must not sound until that
    /// pattern is selected — and must sound once it is.
    #[test]
    fn pattern_bank_isolation() {
        let sr = 48_000u32;
        let ppq = Ppq::DEFAULT;
        let tps = ticks_per_sample(120.0, sr, ppq);
        let mut seq = Sequencer::new(1, 8, 16, ppq);

        // Pattern 1: every step on. Pattern 0 (current): all off.
        for s in 0..16 {
            seq.set_step(1, 0, s, true, 100);
        }

        let kick = SampleData::default_kick(sr);
        let make_sampler = || {
            let slot: Arc<arc_swap::ArcSwapOption<SampleData>> =
                Arc::new(arc_swap::ArcSwapOption::from(Some(kick.clone())));
            Sampler::new(slot, SamplerParams::default(), sr)
        };
        let mut devices = [make_sampler()];

        let run_second = |seq: &Sequencer, devices: &mut [Sampler]| -> f32 {
            let block = 1024usize;
            let mut tick = 0.0f64;
            let mut peak = 0.0f32;
            for _ in 0..(sr as usize / block) {
                let start = tick;
                let end = start + block as f64 * tps;
                tick = end;
                seq.schedule(start, end, block, tps, devices);
                let mut l = vec![0.0f32; block];
                let mut r = vec![0.0f32; block];
                let ctx = ProcessContext {
                    sample_rate: sr,
                    frames: block,
                };
                devices[0].process(ctx, &mut l, &mut r);
                peak = peak.max(l.iter().fold(0.0f32, |a, x| a.max(x.abs())));
            }
            peak
        };

        // Current pattern (0) is empty: silence.
        assert_eq!(run_second(&seq, &mut devices), 0.0);
        // Switch to pattern 1: output.
        seq.set_current_pattern(1);
        assert!(run_second(&seq, &mut devices) > 0.1);
    }
}
