//! Transport clock. Lives on the realtime thread.
//!
//! Position is tracked as a fractional tick count (`f64`) so that tempo
//! changes don't cause phase quantisation noise. The integer tick reported to
//! the UI is just `position_ticks as u64`.

use mooloop_core::{ticks_per_sample, Ppq};
use mooloop_dsp::Metronome;

/// Beats per bar. Hard-coded to 4/4 for Phase 0; lands in the project model
/// once time signatures become user-editable.
const BEATS_PER_BAR: i64 = 4;

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

    fn ticks_per_sample(&self) -> f64 {
        ticks_per_sample(self.bpm, self.sample_rate, self.ppq)
    }

    /// Current beat index within the bar (0-based).
    pub fn beat_in_bar(&self) -> u8 {
        let tpb = self.ppq.ticks_per_beat() as f64;
        let beat = (self.position_ticks / tpb) as i64;
        beat.rem_euclid(BEATS_PER_BAR) as u8
    }

    /// Advance the clock by `frames` samples, scheduling metronome clicks for
    /// every beat boundary crossed in `[position, position + frames)` (half
    /// open on the right so beats don't double-trigger across blocks).
    pub fn advance(&mut self, frames: usize, metronome: &mut Metronome) {
        if !self.playing {
            return;
        }
        let tps = self.ticks_per_sample();
        let tpb = self.ppq.ticks_per_beat() as f64;
        let start = self.position_ticks;
        let end = start + frames as f64 * tps;

        // First beat at or after `start` (inclusive) — a beat exactly at
        // `start` is ours to fire, the previous block's half-open end excluded
        // it.
        let mut b = (start / tpb).ceil() as i64;
        let mut beat_tick = b as f64 * tpb;
        while beat_tick < end {
            let offset = ((beat_tick - start) / tps).round() as i64;
            if (0..frames as i64).contains(&offset) {
                let accent = b.rem_euclid(BEATS_PER_BAR) == 0;
                metronome.trigger(offset as usize, accent);
            }
            b += 1;
            beat_tick = b as f64 * tpb;
        }

        self.position_ticks = end;
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
