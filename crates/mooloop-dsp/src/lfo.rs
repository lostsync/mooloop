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
        Self::shape_at(phase, wave, self.hold)
    }

    /// Sample the current cycle's shape at an additional phase offset
    /// (fractional cycles), without advancing the accumulator. For a stereo
    /// effect that wants a second tap of the same cycle — e.g. a
    /// runtime-variable stereo spread — rather than a second, independently
    /// drifting oscillator. Call before `next_sample`/`skip` for the same
    /// sample so both reads see the same phase.
    pub fn peek_offset(&self, offset: f32, wave: LfoWave) -> f32 {
        Self::shape_at((self.phase + offset).rem_euclid(1.0), wave, self.hold)
    }

    fn shape_at(phase: f32, wave: LfoWave, hold: f32) -> f32 {
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
            LfoWave::Random => hold,
        }
    }

    /// Advance without producing a value, for stretches where the voice is
    /// silent. Keeps a free-running LFO in phase with the transport.
    ///
    /// One sample at a time, and not because the phase could not be added in
    /// one go. `advance` folds at every wrap and refreshes the held value
    /// there, so a stride is not the same state as the samples it stands in
    /// for -- and it does not have to span two cycles to differ, because the
    /// accumulation itself rounds differently. That is a render that depends
    /// on how the audio was cut into blocks, which is the one thing an export
    /// matching a take rules out. What a skip does save is `shape_at`, whose
    /// value nothing is going to read.
    pub fn skip(&mut self, frames: usize, rate_hz: f32, sample_rate: u32) {
        for _ in 0..frames {
            self.advance(1.0, rate_hz, sample_rate);
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
        // Exactly, not nearly: skipping is the same arithmetic with the shape
        // evaluation left out, so any difference at all would mean the two
        // paths had stopped agreeing about where the transport is.
        assert_eq!(running.phase, skipping.phase);
        assert_eq!(running.hold, skipping.hold);
    }

    /// The property the engine's block-size tests rest on, stated where it is
    /// actually decided. One skip of 1024 and eight of 128 have to leave the
    /// same state, or a device that slept renders differently depending on
    /// the host's buffer size.
    #[test]
    fn a_skip_does_not_depend_on_how_it_is_divided() {
        let sr = 48_000;
        for rate in [0.5f32, 3.0, 7.0, 19.0] {
            let mut whole = Lfo::new();
            let mut eighths = Lfo::new();
            whole.skip(1024, rate, sr);
            for _ in 0..8 {
                eighths.skip(128, rate, sr);
            }
            assert_eq!(whole.phase, eighths.phase, "phase diverged at {rate} Hz");
            assert_eq!(whole.hold, eighths.hold, "held value diverged at {rate} Hz");
        }
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

    #[test]
    fn peek_offset_matches_a_quarter_cycle_advance_without_moving_the_phase() {
        let sr = 48_000;
        let mut reference = Lfo::new();
        for _ in 0..1234 {
            reference.next_sample(5.0, LfoWave::Sine, sr);
        }
        let probe = reference;
        // A quarter cycle at this rate/sample-rate, expressed in fractional
        // cycles the same way the offset argument is.
        let offset = 0.25;
        let peeked = probe.peek_offset(offset, LfoWave::Sine);
        let phase_before = probe.phase;
        let direct = ((probe.phase + offset).rem_euclid(1.0) * TAU).sin();
        assert_eq!(peeked, direct);
        assert_eq!(probe.phase, phase_before, "peek must not advance the phase");
    }
}

