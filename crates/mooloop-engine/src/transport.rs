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

use mooloop_core::{ticks_per_sample, Ppq};

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

    /// Current beat index within the bar (0-based). Assumes 4/4 for now.
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
        if self.playing {
            self.frames_played += frames as u64;
        }
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
        self.frames_played = 0;
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
}
