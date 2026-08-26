//! Small filters shared by the synth voices. The sampler keeps its own
//! inline per-voice filter math rather than calling `Svf` directly, and this
//! is a measured decision, not inertia: `Svf::next_sample`/`tick` recompute
//! `g`/`damping`/`a1`/`a2`/`a3` (including a `tan()`) from cutoff/resonance
//! on every call, and the sampler needs one shared coefficient set applied
//! to both channels of a stereo frame, which is exactly what its inline
//! version does — computing them once and ticking L/R against the same
//! coefficients — while calling `Svf::next_sample` once per channel would
//! recompute them twice. A synthetic benchmark isolating just this
//! (32 voices, 4s of audio at 48 kHz, coefficients varying every 1000
//! frames) measured shared-coefficient-per-frame at ~154ms versus
//! per-channel-recompute at ~310ms — about 2x, dominated by the doubled
//! `tan()`. `docs/plans/share-dsp-primitives/03-collapse-duplicate-implementations.md`
//! asked for exactly this measurement before converting; this is the
//! result. New instruments (mono, not stereo-per-voice) use `Svf` directly,
//! where the doubling doesn't apply.

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
    pub fn next_sample(
        &mut self,
        input: f32,
        cutoff_hz: f32,
        resonance: f32,
        sample_rate: u32,
    ) -> f32 {
        self.tick(input, cutoff_hz, resonance, sample_rate).0
    }

    /// Process one sample, returning `(low_pass, high_pass)`. The high-pass
    /// output is the SVF's exact complementary output
    /// (`input - damping * band - low`), not the leaky `input - low`
    /// approximation.
    pub fn next_sample_lp_hp(
        &mut self,
        input: f32,
        cutoff_hz: f32,
        resonance: f32,
        sample_rate: u32,
    ) -> (f32, f32) {
        let (low, _, high) = self.tick(input, cutoff_hz, resonance, sample_rate);
        (low, high)
    }

    /// Process one sample and return the low-pass, band-pass, and high-pass
    /// outputs from the same state-variable stage.
    pub fn next_sample_lp_bp_hp(
        &mut self,
        input: f32,
        cutoff_hz: f32,
        resonance: f32,
        sample_rate: u32,
    ) -> (f32, f32, f32) {
        self.tick(input, cutoff_hz, resonance, sample_rate)
    }

    fn tick(
        &mut self,
        input: f32,
        cutoff_hz: f32,
        resonance: f32,
        sample_rate: u32,
    ) -> (f32, f32, f32) {
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
        let high = input - damping * v1 - v2;
        self.band = 2.0 * v1 - self.band;
        self.low = 2.0 * v2 - self.low;
        (v2, v1, high)
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

    pub fn reset(&mut self) {
        self.prev_in = 0.0;
        self.prev_out = 0.0;
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

/// A one-pole low-pass filter: the tone/damping stage several effects
/// reimplemented inline (drive's tilt, delay's feedback damping,
/// modulation's tone control). Not for envelope-modulated cutoff sweeps —
/// reach for [`Svf`] there, this is for smoothing a spectral tilt or damping
/// a feedback path where the cutoff itself changes slowly if at all.
#[derive(Clone, Copy, Debug)]
pub struct OnePoleLp {
    state: f32,
    coeff: f32,
}

impl OnePoleLp {
    pub fn new() -> Self {
        Self {
            state: 0.0,
            coeff: 0.0,
        }
    }

    pub fn set_cutoff(&mut self, cutoff_hz: f32, sample_rate: u32) {
        let cutoff = cutoff_hz.clamp(10.0, sample_rate as f32 * 0.45);
        self.coeff = 1.0 - (-core::f32::consts::TAU * cutoff / sample_rate as f32).exp();
    }

    /// Set the leak coefficient directly, bypassing the Hz mapping. For a
    /// caller that already smooths the coefficient itself rather than a
    /// cutoff control (see `DelayEffect`'s damping, which smooths this value
    /// directly to skip a `powf` per sample — the coefficient is bounded in
    /// `[0, 1]` at both ends of that ramp, so interpolating it directly
    /// can't destabilize the filter the way interpolating a biquad's
    /// coefficients can).
    pub fn set_coeff(&mut self, coeff: f32) {
        self.coeff = coeff.clamp(0.0, 1.0);
    }

    pub fn reset(&mut self) {
        self.state = 0.0;
    }

    pub fn next_sample(&mut self, input: f32) -> f32 {
        self.state += (input - self.state) * self.coeff;
        self.state
    }
}

impl Default for OnePoleLp {
    fn default() -> Self {
        Self::new()
    }
}

/// A first-order all-pass stage: unity gain at every frequency, phase shift
/// only. The building block of phaser stages, reverb diffusers, and
/// fractional-delay interpolation. `coefficient` is supplied per call rather
/// than stored, since callers like a phaser cascade recompute it every
/// sample from a swept frequency.
#[derive(Clone, Copy, Debug, Default)]
pub struct AllPass {
    z: f32,
}

impl AllPass {
    pub fn new() -> Self {
        Self { z: 0.0 }
    }

    pub fn reset(&mut self) {
        self.z = 0.0;
    }

    pub fn next(&mut self, input: f32, coefficient: f32) -> f32 {
        let output = -coefficient * input + self.z;
        self.z = input + coefficient * output;
        output
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

    #[test]
    fn one_pole_lp_attenuates_high_frequencies() {
        let sr = 48_000;
        let mut filter = OnePoleLp::new();
        filter.set_cutoff(200.0, sr);
        let mut peak = 0.0f32;
        for i in 0..sr as usize {
            let t = i as f32 / sr as f32;
            let input = (t * 8_000.0 * core::f32::consts::TAU).sin();
            let out = filter.next_sample(input);
            if i > sr as usize / 2 {
                peak = peak.max(out.abs());
            }
        }
        assert!(peak < 0.05, "peak {peak}");
    }

    #[test]
    fn one_pole_lp_passes_dc() {
        let sr = 48_000;
        let mut filter = OnePoleLp::new();
        filter.set_cutoff(1_000.0, sr);
        let mut last = 0.0;
        for _ in 0..sr as usize {
            last = filter.next_sample(1.0);
        }
        assert!((last - 1.0).abs() < 0.01, "dc settled at {last}");
    }

    #[test]
    fn set_coeff_matches_the_equivalent_set_cutoff() {
        let sr = 48_000;
        let mut via_cutoff = OnePoleLp::new();
        via_cutoff.set_cutoff(1_000.0, sr);
        let mut via_coeff = OnePoleLp::new();
        via_coeff.set_coeff(1.0 - (-core::f32::consts::TAU * 1_000.0 / sr as f32).exp());
        for i in 0..64 {
            let input = (i as f32 * 0.1).sin();
            assert_eq!(via_cutoff.next_sample(input), via_coeff.next_sample(input));
        }
    }

    #[test]
    fn all_pass_preserves_energy_but_shifts_phase() {
        let mut filter = AllPass::new();
        let frames = 4_096;
        let coefficient = 0.5;
        let mut energy_in = 0.0f32;
        let mut energy_out = 0.0f32;
        let mut differs = false;
        for i in 0..frames {
            let input = (i as f32 * 0.05).sin();
            let output = filter.next(input, coefficient);
            if i > 64 {
                energy_in += input * input;
                energy_out += output * output;
                if (input - output).abs() > 1e-3 {
                    differs = true;
                }
            }
        }
        assert!(
            (energy_in - energy_out).abs() < energy_in * 0.05,
            "all-pass should preserve energy: in {energy_in}, out {energy_out}"
        );
        assert!(
            differs,
            "all-pass should shift phase, not pass through unchanged"
        );
    }
}
