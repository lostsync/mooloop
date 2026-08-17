//! Step-grid sequencer. Lives on the realtime thread.
//!
//! Each block, the engine hands the transport's tick interval
//! `[start_tick, end_tick)`. The sequencer finds every step boundary that
//! falls inside that interval and, for steps that are on, schedules a
//! note-on into the channel's device at the matching sample offset.
//!
//! Step resolution is 16th notes: `ticks_per_step = ppq / 4`. Steps wrap at
//! the pattern length, so a 16-step pattern loops every bar.

use mooloop_core::{Pattern, Ppq};
use mooloop_dsp::Device;

pub struct Sequencer {
    pattern: Pattern,
    ppq: Ppq,
}

impl Sequencer {
    pub fn new(pattern: Pattern, ppq: Ppq) -> Self {
        Self { pattern, ppq }
    }

    fn ticks_per_step(&self) -> f64 {
        (self.ppq.ticks_per_beat() / 4) as f64
    }

    /// Apply a step change coming in from the UI.
    pub fn set_step(&mut self, channel: usize, step: usize, on: bool, velocity: u8) {
        if let Some(ch) = self.pattern.channel_mut(channel) {
            if let Some(s) = ch.steps.get_mut(step) {
                s.on = on;
                s.velocity = velocity;
            }
        }
    }

    /// For each step boundary in `[start, end)`, schedule note-ons into
    /// `devices` for any active step. `ticks_per_sample` converts tick deltas
    /// to sample offsets inside the block.
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

        let tps = self.ticks_per_step();
        // Pattern length in ticks; wraps here.
        let pattern_steps = self.pattern.length_steps.max(1) as i64;
        let pattern_ticks = tps * pattern_steps as f64;

        // First step boundary at or after `start` (half-open: a boundary
        // exactly at `start` is ours; the previous block's `[., start)` did
        // not include it).
        let mut step_idx = (start_tick / tps).ceil() as i64;
        let mut step_tick = step_idx as f64 * tps;

        while step_tick < end_tick {
            let offset = ((step_tick - start_tick) / ticks_per_sample).round() as i64;
            if (0..frames as i64).contains(&offset) {
                let step_in_pattern = step_idx.rem_euclid(pattern_steps) as usize;
                for (ch_idx, ch) in self.pattern.channels.iter().enumerate() {
                    if let Some(step) = ch.steps.get(step_in_pattern) {
                        if step.on {
                            if let Some(dev) = devices.get_mut(ch_idx) {
                                // Root note 60 (C4); per-step pitch arrives
                                // with the piano roll.
                                dev.note_on(offset as usize, 60, step.velocity);
                            }
                        }
                    }
                }
            }
            step_idx += 1;
            step_tick = step_idx as f64 * tps;
        }

        // `pattern_ticks` is used implicitly via rem_euclid above; kept here
        // for future "song-position aware" scheduling (playlist).
        let _ = pattern_ticks;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::ticks_per_sample;
    use mooloop_dsp::{Device, ProcessContext, SampleData, Sampler};
    use std::sync::Arc;

    /// Drive Sequencer -> Sampler offline exactly the way `Graph::process`
    /// does, and verify audible output appears at the right density.
    #[test]
    fn sequencer_drives_sampler() {
        let sr = 48_000u32;
        let bpm = 120.0;
        let ppq = Ppq::DEFAULT;
        let tps = ticks_per_sample(bpm, sr, ppq);

        let mut pattern = Pattern::new(1);
        for s in [0, 4, 8, 12] {
            pattern.channel_mut(0).unwrap().steps[s].on = true;
            pattern.channel_mut(0).unwrap().steps[s].velocity = 100;
        }
        let seq = Sequencer::new(pattern, ppq);

        let slot: Arc<arc_swap::ArcSwapOption<SampleData>> =
            Arc::new(arc_swap::ArcSwapOption::from(Some(SampleData::default_kick(sr))));
        let sampler = Sampler::new(slot, mooloop_core::SamplerParams::default(), sr);
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
            let ctx = ProcessContext { sample_rate: sr, frames: block };
            devices[0].process(ctx, &mut l, &mut r);

            let peak = l.iter().fold(0.0f32, |a, x| a.max(x.abs()));
            if peak > 0.0 {
                nonzero_blocks += 1;
            }
            max_peak = max_peak.max(peak);
        }

        // Four-on-the-floor at 120 bpm = 2 hits/second = ~20 hits in 10 s.
        // Each kick lasts 0.25 s > block size, so nonzero blocks >= hits.
        assert!(nonzero_blocks >= 20, "too few nonzero blocks: {nonzero_blocks}");
        assert!(max_peak > 0.1, "output too quiet: {max_peak}");
    }
}
