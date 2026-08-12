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
