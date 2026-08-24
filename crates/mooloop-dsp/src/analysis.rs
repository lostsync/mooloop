//! Allocation-free display analysis shared by device hosts.
//!
//! This intentionally produces compact, log-frequency level vectors rather
//! than handing PCM to the UI. The audio thread owns the rolling window; a
//! device host publishes its latest semantic display data through the engine.

use crate::bus::StereoBus;

/// Number of logarithmically spaced spectrum bands intended for a compact UI.
pub const SPECTRUM_BINS: usize = 48;
const SPECTRUM_WINDOW: usize = 512;
const SPECTRUM_HOP: usize = 2_048;
const SPECTRUM_FLOOR_DB: f32 = -84.0;

/// Rolling, low-rate spectrum analyzer. It is deliberately inexpensive:
/// values are calculated once per hop, with a fixed Goertzel bank over a
/// mono sum, and only when a device display subscribes to it.
pub struct SpectrumAnalyzer {
    samples: [f32; SPECTRUM_WINDOW],
    coefficients: [f32; SPECTRUM_BINS],
    write: usize,
    filled: usize,
    since_publish: usize,
    sample_rate: u32,
}

impl SpectrumAnalyzer {
    pub const fn new() -> Self {
        Self {
            samples: [0.0; SPECTRUM_WINDOW],
            coefficients: [0.0; SPECTRUM_BINS],
            write: 0,
            filled: 0,
            since_publish: SPECTRUM_HOP,
            sample_rate: 0,
        }
    }

    pub fn reset(&mut self) {
        self.samples.fill(0.0);
        self.write = 0;
        self.filled = 0;
        self.since_publish = SPECTRUM_HOP;
    }

    fn configure(&mut self, sample_rate: u32) {
        if self.sample_rate == sample_rate {
            return;
        }
        self.sample_rate = sample_rate;
        self.reset();
        let max_frequency = sample_rate as f32 * 0.45;
        for (index, coefficient) in self.coefficients.iter_mut().enumerate() {
            let position = index as f32 / (SPECTRUM_BINS - 1) as f32;
            let frequency = 20.0 * (max_frequency / 20.0).powf(position);
            *coefficient = 2.0 * (core::f32::consts::TAU * frequency / sample_rate as f32).cos();
        }
    }

    /// Ingest one device input block. Returns a fresh normalized display vector
    /// at most once every [`SPECTRUM_HOP`] samples after the window fills.
    pub fn push(
        &mut self,
        sample_rate: u32,
        bus: &StereoBus,
        frames: usize,
    ) -> Option<[f32; SPECTRUM_BINS]> {
        self.configure(sample_rate);
        let frames = frames.min(bus.capacity());
        for frame in 0..frames {
            self.samples[self.write] = (bus.l[frame] + bus.r[frame]) * 0.5;
            self.write = (self.write + 1) % SPECTRUM_WINDOW;
            self.filled = (self.filled + 1).min(SPECTRUM_WINDOW);
        }
        self.since_publish = self.since_publish.saturating_add(frames);
        if self.filled < SPECTRUM_WINDOW || self.since_publish < SPECTRUM_HOP {
            return None;
        }
        self.since_publish = 0;
        Some(self.analyze())
    }

    fn analyze(&self) -> [f32; SPECTRUM_BINS] {
        let mut levels = [0.0; SPECTRUM_BINS];
        for (bin, level) in levels.iter_mut().enumerate() {
            let coefficient = self.coefficients[bin];
            let mut q1 = 0.0;
            let mut q2 = 0.0;
            for offset in 0..SPECTRUM_WINDOW {
                let sample = self.samples[(self.write + offset) % SPECTRUM_WINDOW];
                let q0 = sample + coefficient * q1 - q2;
                q2 = q1;
                q1 = q0;
            }
            let power = (q1 * q1 + q2 * q2 - coefficient * q1 * q2)
                / (SPECTRUM_WINDOW * SPECTRUM_WINDOW) as f32;
            let db = 10.0 * power.max(1e-12).log10();
            *level = ((db - SPECTRUM_FLOOR_DB) / -SPECTRUM_FLOOR_DB).clamp(0.0, 1.0);
        }
        levels
    }
}

impl Default for SpectrumAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sine_raises_its_nearest_log_band() {
        let sample_rate = 48_000;
        let mut analyzer = SpectrumAnalyzer::new();
        let mut bus = StereoBus::with_capacity(256);
        let mut spectrum = None;
        for block in 0..16 {
            for frame in 0..256 {
                let t = (block * 256 + frame) as f32 / sample_rate as f32;
                let sample = (core::f32::consts::TAU * 1_000.0 * t).sin() * 0.5;
                bus.l[frame] = sample;
                bus.r[frame] = sample;
            }
            spectrum = analyzer.push(sample_rate, &bus, 256).or(spectrum);
        }
        let spectrum = spectrum.expect("analyzer should publish after a hop");
        let nearest = (0..SPECTRUM_BINS)
            .min_by(|a, b| {
                let frequency = |index: usize| {
                    20.0_f32 * (21_600.0_f32 / 20.0_f32).powf(index as f32 / 47.0_f32)
                };
                (frequency(*a) - 1_000.0)
                    .abs()
                    .total_cmp(&(frequency(*b) - 1_000.0).abs())
            })
            .unwrap();
        assert!(
            spectrum[nearest] > 0.45,
            "1 kHz level: {}",
            spectrum[nearest]
        );
    }
}
