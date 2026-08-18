//! Oscillators and noise sources for the synth voices.
//!
//! Saw and pulse use PolyBLEP correction so they stay band-limited enough for
//! musical use without per-sample oversampling. Sine and triangle are naive
//! (they alias negligibly at these frequencies). Everything is allocation-free
//! state advanced one sample at a time.

use core::f32::consts::TAU;

use mooloop_core::OscWave;

/// A single band-limited oscillator.
#[derive(Clone, Copy, Debug)]
pub struct Osc {
    phase: f32,
}

impl Osc {
    pub fn new() -> Self {
        Self { phase: 0.0 }
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
    }

    /// Advance one sample at `freq_hz` and return the waveform value in
    /// `[-1, 1]`. `pulse_width` only applies to [`OscWave::Pulse`].
    pub fn next_sample(&mut self, freq_hz: f32, wave: OscWave, pulse_width: f32, sample_rate: u32) -> f32 {
        let sr = sample_rate as f32;
        let freq = freq_hz.clamp(0.01, sr * 0.45);
        let dt = freq / sr;
        let phase = self.phase;
        self.phase = (phase + dt).fract();
        match wave {
            OscWave::Sine => (phase * TAU).sin(),
            OscWave::Triangle => 4.0 * (phase - 0.5).abs() - 1.0,
            OscWave::Saw => 2.0 * phase - 1.0 - polyblep(phase, dt),
            OscWave::Pulse => {
                let width = pulse_width.clamp(0.05, 0.95);
                let mut value = if phase < width { 1.0 } else { -1.0 };
                value += polyblep(phase, dt);
                value -= polyblep((phase - width).rem_euclid(1.0), dt);
                value
            }
        }
    }
}

impl Default for Osc {
    fn default() -> Self {
        Self::new()
    }
}

/// PolyBLEP residual to subtract/add at discontinuities.
fn polyblep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let t = t / dt;
        2.0 * t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + 2.0 * t + 1.0
    } else {
        0.0
    }
}

/// A tiny xorshift32 white-noise source. Deterministic, allocation-free, and
/// good enough for percussive noise bursts.
#[derive(Clone, Copy, Debug)]
pub struct Noise {
    state: u32,
}

impl Noise {
    pub fn new(seed: u32) -> Self {
        Self {
            state: seed.max(1),
        }
    }

    /// Next white-noise sample in `[-1, 1]`.
    pub fn next_sample(&mut self) -> f32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        // Map to [-1, 1) using the top 24 bits.
        (x >> 8) as f32 / 8_388_608.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_waves_stay_bounded() {
        let sr = 48_000;
        for wave in OscWave::all() {
            let mut osc = Osc::new();
            for i in 0..sr as usize {
                let freq = 20.0 + (i % 5000) as f32;
                let value = osc.next_sample(freq, wave, 0.5, sr);
                assert!(
                    (-1.05..=1.05).contains(&value),
                    "{wave:?} out of range: {value}"
                );
            }
        }
    }

    #[test]
    fn sine_has_expected_frequency() {
        let sr = 48_000;
        let mut osc = Osc::new();
        let mut zero_crossings = 0u32;
        let mut prev = 0.0_f32;
        for _ in 0..sr as usize {
            let value = osc.next_sample(100.0, OscWave::Sine, 0.5, sr);
            if prev <= 0.0 && value > 0.0 {
                zero_crossings += 1;
            }
            prev = value;
        }
        assert_eq!(zero_crossings, 100);
    }

    #[test]
    fn noise_is_deterministic_and_bounded() {
        let mut a = Noise::new(42);
        let mut b = Noise::new(42);
        for _ in 0..1000 {
            let value = a.next_sample();
            assert!((-1.0..1.0).contains(&value));
            assert_eq!(value, b.next_sample());
        }
    }
}
