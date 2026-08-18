//! Small filters shared by the synth voices. The sampler keeps its own
//! inline per-voice filter math; new instruments use these.

/// A topology-preserving state-variable low-pass filter (Chamberlin/Zavalishin
/// form). Unlike a biquad it stays well behaved while cutoff moves every
/// sample, which is what envelope-modulated synth filters need.
#[derive(Clone, Copy, Debug)]
pub struct Svf {
    low: f32,
    band: f32,
}

impl Svf {
    pub fn new() -> Self {
        Self {
            low: 0.0,
            band: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.low = 0.0;
        self.band = 0.0;
    }

    /// Process one sample. `cutoff_hz` is clamped to a safe range;
    /// `resonance` in `[0, 1]` approaches self-oscillation at the top.
    pub fn next_sample(&mut self, input: f32, cutoff_hz: f32, resonance: f32, sample_rate: u32) -> f32 {
        let sr = sample_rate as f32;
        let cutoff = cutoff_hz.clamp(20.0, sr * 0.45);
        let g = (core::f32::consts::PI * cutoff / sr).tan();
        let damping = (2.0 - resonance.clamp(0.0, 1.0) * 1.9).clamp(0.1, 2.0);
        let a1 = 1.0 / (1.0 + g * (g + damping));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = input - self.low;
        let v1 = a1 * self.band + a2 * v3;
        let v2 = self.low + a2 * self.band + a3 * v3;
        self.band = 2.0 * v1 - self.band;
        self.low = 2.0 * v2 - self.low;
        v2
    }
}

impl Default for Svf {
    fn default() -> Self {
        Self::new()
    }
}

/// A one-pole high-pass filter for noise shaping (hats, snare snap).
#[derive(Clone, Copy, Debug)]
pub struct OnePoleHp {
    prev_in: f32,
    prev_out: f32,
    coeff: f32,
}

impl OnePoleHp {
    pub fn new() -> Self {
        Self {
            prev_in: 0.0,
            prev_out: 0.0,
            coeff: 0.0,
        }
    }

    pub fn set_cutoff(&mut self, cutoff_hz: f32, sample_rate: u32) {
        let cutoff = cutoff_hz.clamp(10.0, sample_rate as f32 * 0.45);
        self.coeff = (-core::f32::consts::TAU * cutoff / sample_rate as f32).exp();
    }

    pub fn next_sample(&mut self, input: f32) -> f32 {
        let out = input - self.prev_in + self.coeff * self.prev_out;
        self.prev_in = input;
        self.prev_out = out;
        out
    }
}

impl Default for OnePoleHp {
    fn default() -> Self {
        Self::new()
    }
}

/// Soft saturation matching the sampler's drive stage: pre-gain into `tanh`
/// with output compensation so low drive settings stay near unity gain.
pub fn apply_drive(input: f32, drive: f32) -> f32 {
    let drive = drive.clamp(0.0, 1.0);
    if drive <= f32::EPSILON {
        return input;
    }
    let input_gain = 1.0 + drive * 15.0;
    let compensation = input_gain.tanh().recip();
    (input * input_gain).tanh() * compensation
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_cutoff_attenuates_high_frequencies() {
        let sr = 48_000;
        let mut filter = Svf::new();
        // Feed a 10 kHz sine through a 100 Hz filter.
        let mut peak = 0.0_f32;
        for i in 0..sr as usize {
            let t = i as f32 / sr as f32;
            let input = (t * 10_000.0 * core::f32::consts::TAU).sin();
            let out = filter.next_sample(input, 100.0, 0.0, sr);
            // Skip the transient.
            if i > sr as usize / 2 {
                peak = peak.max(out.abs());
            }
        }
        assert!(peak < 0.02, "peak {peak}");
    }

    #[test]
    fn resonant_filter_remains_finite() {
        let sr = 48_000;
        let mut filter = Svf::new();
        for i in 0..20_000 {
            let input = if i == 0 { 1.0 } else { 0.0 };
            let out = filter.next_sample(input, 5_000.0, 1.0, sr);
            assert!(out.is_finite());
        }
    }

    #[test]
    fn high_pass_removes_dc() {
        let sr = 48_000;
        let mut filter = OnePoleHp::new();
        filter.set_cutoff(1_000.0, sr);
        let mut last = 0.0;
        for _ in 0..sr as usize {
            last = filter.next_sample(1.0);
        }
        assert!(last.abs() < 0.01, "dc residue {last}");
    }

    #[test]
    fn drive_bypasses_at_zero_and_saturates_at_max() {
        assert_eq!(apply_drive(0.25, 0.0), 0.25);
        let driven = apply_drive(0.25, 1.0);
        assert!(driven > 0.9);
        assert!(driven <= 1.0);
    }
}
