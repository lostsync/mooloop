//! Transport clock. Lives on the realtime thread.
//!
//! Position is tracked as a fractional tick count (`f64`) so tempo changes
//! don't cause phase quantisation noise. The integer tick reported to the UI
//! is `position_ticks as u64`.
//!
//! The transport only moves the clock; scheduling note events for the step
//! grid is the job of [`crate::sequencer::Sequencer`], which reads the
//! before/after tick each block.

use mooloop_core::{ticks_per_sample, Ppq};

pub struct Transport {
    pub playing: bool,
    pub bpm: f64,
    pub sample_rate: u32,
    pub ppq: Ppq,
    pub position_ticks: f64,
}

impl Transport {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            playing: false,
            bpm: 120.0,
            sample_rate,
            ppq: Ppq::DEFAULT,
            position_ticks: 0.0,
        }
    }

    pub fn ticks_per_sample(&self) -> f64 {
        ticks_per_sample(self.bpm, self.sample_rate, self.ppq)
    }

    /// Current beat index within the bar (0-based). Assumes 4/4 for Phase 1.
    pub fn beat_in_bar(&self) -> u8 {
        let tpb = self.ppq.ticks_per_beat() as f64;
        let beat = (self.position_ticks / tpb) as i64;
        beat.rem_euclid(4) as u8
    }

    /// Advance the clock by `frames` samples. Returns the `(start, end)` tick
    /// interval covered by this block, for the sequencer's use.
    pub fn advance(&mut self, frames: usize) -> (f64, f64) {
        let start = self.position_ticks;
        let end = if self.playing {
            start + frames as f64 * self.ticks_per_sample()
        } else {
            start
        };
        self.position_ticks = end;
        (start, end)
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
    }

    pub fn set_tempo(&mut self, bpm: f64) {
        self.bpm = bpm.clamp(1.0, 999.0);
    }
}
