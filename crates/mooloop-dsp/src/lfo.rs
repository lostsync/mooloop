//! A low-frequency oscillator for modulating parameters.
//!
//! Separate from [`crate::osc`] because the trade-offs invert: nothing here
//! needs band limiting (the output drives parameters, not the speaker), but
//! the shapes are phase-aligned so that a retriggered LFO starts at zero
//! modulation instead of stepping the sound at every note.

use core::f32::consts::TAU;

use mooloop_core::LfoWave;

use crate::osc::Noise;

/// Seed for the sample-and-hold source. Fixed so renders are reproducible.
const SH_SEED: u32 = 0x5f37_1e21;

/// A bipolar LFO in `[-1, 1]`.
#[derive(Clone, Copy, Debug)]
pub struct Lfo {
    phase: f32,
    hold: f32,
    noise: Noise,
}

impl Lfo {
    pub fn new() -> Self {
        let mut noise = Noise::new(SH_SEED);
        let hold = noise.next_sample();
        Self {
            phase: 0.0,
            hold,
            noise,
        }
    }

    /// Restart the cycle. Called on note-on when the LFO is set to retrigger.
    pub fn retrigger(&mut self) {
        self.phase = 0.0;
        self.hold = self.noise.next_sample();
    }

    /// Advance one sample and return the current value.
    pub fn next_sample(&mut self, rate_hz: f32, wave: LfoWave, sample_rate: u32) -> f32 {
        let phase = self.phase;
        self.advance(1.0, rate_hz, sample_rate);
        match wave {
            LfoWave::Sine => (phase * TAU).sin(),
            // Shifted a quarter cycle so the shape leaves zero rising.
            LfoWave::Triangle => 1.0 - 4.0 * ((phase + 0.25).fract() - 0.5).abs(),
            // Likewise: the ramp's discontinuity sits mid-cycle, not at the
            // note boundary.
            LfoWave::Saw => 2.0 * (phase + 0.5).fract() - 1.0,
            LfoWave::Square => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            LfoWave::Random => self.hold,
        }
    }

    /// Advance without producing a value, for stretches where the voice is
    /// silent. Keeps a free-running LFO in phase with the transport.
    pub fn skip(&mut self, frames: usize, rate_hz: f32, sample_rate: u32) {
        if frames > 0 {
            self.advance(frames as f32, rate_hz, sample_rate);
        }
    }

    fn advance(&mut self, frames: f32, rate_hz: f32, sample_rate: u32) {
        let rate = rate_hz.clamp(0.0, sample_rate as f32 * 0.25);
        let next = self.phase + rate * frames / sample_rate as f32;
        if next >= 1.0 {
            // A new sample-and-hold value per cycle, however many cycles the
            // skip covered.
            self.hold = self.noise.next_sample();
        }
        self.phase = next.fract();
    }
}

impl Default for Lfo {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cycle(wave: LfoWave, sr: u32, rate: f32) -> Vec<f32> {
        let mut lfo = Lfo::new();
        let frames = (sr as f32 / rate) as usize;
        (0..frames)
            .map(|_| lfo.next_sample(rate, wave, sr))
            .collect()
    }

    #[test]
    fn continuous_shapes_start_at_zero_and_stay_bipolar() {
        let sr = 48_000;
        for wave in [LfoWave::Sine, LfoWave::Triangle, LfoWave::Saw] {
            let values = cycle(wave, sr, 5.0);
            assert!(values[0].abs() < 1.0e-3, "{wave:?} starts at {}", values[0]);
            let max = values.iter().cloned().fold(f32::MIN, f32::max);
            let min = values.iter().cloned().fold(f32::MAX, f32::min);
            assert!((max - 1.0).abs() < 0.01, "{wave:?} max {max}");
            assert!((min + 1.0).abs() < 0.01, "{wave:?} min {min}");
        }
    }

    #[test]
    fn square_is_two_valued_and_evenly_split() {
        let sr = 48_000;
        let values = cycle(LfoWave::Square, sr, 5.0);
        let high = values.iter().filter(|v| **v > 0.0).count();
        assert!(values.iter().all(|v| v.abs() == 1.0));
        assert!((high as f32 / values.len() as f32 - 0.5).abs() < 0.01);
    }

    #[test]
    fn sample_and_hold_changes_once_per_cycle() {
        let sr = 48_000;
        let values = cycle(LfoWave::Random, sr, 5.0);
        let changes = values.windows(2).filter(|w| w[0] != w[1]).count();
        assert!(changes <= 1, "{changes} changes within one cycle");
        assert!(values.iter().all(|v| (-1.0..1.0).contains(v)));
    }

    #[test]
    fn skip_keeps_a_free_running_lfo_in_phase() {
        let sr = 48_000;
        let rate = 3.0;
        let mut running = Lfo::new();
        let mut skipping = Lfo::new();
        for _ in 0..1000 {
            running.next_sample(rate, LfoWave::Sine, sr);
        }
        skipping.skip(1000, rate, sr);
        assert!((running.phase - skipping.phase).abs() < 1.0e-4);
    }

    #[test]
    fn retrigger_returns_to_the_start_of_the_cycle() {
        let sr = 48_000;
        let mut lfo = Lfo::new();
        for _ in 0..500 {
            lfo.next_sample(5.0, LfoWave::Sine, sr);
        }
        lfo.retrigger();
        assert_eq!(lfo.next_sample(5.0, LfoWave::Sine, sr), 0.0);
    }
}
