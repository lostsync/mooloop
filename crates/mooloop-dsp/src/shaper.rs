//! Waveshaping curves and the 2x oversampler that keeps them usable.
//!
//! ## Why oversampling is here and not everywhere
//!
//! A memoryless nonlinearity generates harmonics above the input's own
//! spectrum. Run at base rate, everything above Nyquist folds back down as
//! inharmonic content that does not move with pitch — the difference between
//! a saturator and a fizz generator. Distortion and saturation therefore run
//! through [`Oversampler2x`].
//!
//! Bitcrush deliberately does *not* oversample: its aliasing is the effect.
//! See `docs/MODULATION_PLAN.md` ("Anti-aliasing policy").

use mooloop_core::DriveCurve;

/// Apply one shaping curve to a single sample. Input is expected pre-gained;
/// output is roughly bounded to [-1, 1] except for `Fold`, which is exactly
/// bounded by construction.
pub fn shape(curve: DriveCurve, x: f32) -> f32 {
    match curve {
        DriveCurve::Soft => x.tanh(),
        DriveCurve::Hard => x.clamp(-1.0, 1.0),
        DriveCurve::Fold => fold(x),
        DriveCurve::Tape => tape(x),
    }
}

/// Triangle wavefolder: linear through |x| <= 1, then reflects. Period 4,
/// so `fold(0) == 0`, `fold(1) == 1`, `fold(2) == 0`, `fold(3) == -1`.
fn fold(x: f32) -> f32 {
    let p = x * 0.25 + 0.25;
    let p = p - p.floor();
    1.0 - 4.0 * (p - 0.5).abs()
}

/// Asymmetric soft saturation. The fixed bias pushes the signal onto an
/// uneven part of the curve, which is what produces the even harmonics; the
/// bias's own DC contribution is subtracted back out.
///
/// The result is normalized by the negative extreme so the curve still fits
/// in [-1, 1]. It compresses harder on the positive side than the negative
/// one, which is the asymmetry being asked for rather than a defect.
fn tape(x: f32) -> f32 {
    const BIAS: f32 = 0.12;
    /// `BIAS.tanh()`, precomputed: `tanh` is not a const fn.
    const BIAS_DC: f32 = 0.119_427_3;
    ((x + BIAS).tanh() - BIAS_DC) / (1.0 + BIAS_DC)
}

/// Output scaling that keeps perceived level roughly steady as drive rises.
///
/// The asymptotic curves are normalized by their own response to the drive
/// amount, so a full-scale input stays near full scale. `Hard` already bounds
/// its output, and `Fold` is non-monotonic in drive so normalizing by its
/// response would swing wildly — it gets a gentle square-root law instead.
pub fn drive_compensation(curve: DriveCurve, drive: f32) -> f32 {
    let drive = drive.max(1.0);
    match curve {
        DriveCurve::Soft | DriveCurve::Tape => 1.0 / shape(curve, drive).max(0.1),
        DriveCurve::Hard => 1.0,
        DriveCurve::Fold => 1.0 / drive.sqrt(),
    }
}

/// Number of FIR taps in the oversampler's anti-imaging/anti-aliasing filter.
/// Even, so it splits into two equal polyphase branches.
const FIR_TAPS: usize = 32;
const HALF_TAPS: usize = FIR_TAPS / 2;

/// A 2x oversampler for one audio channel.
///
/// Zero-stuffs to twice the rate through a windowed-sinc low-pass, hands both
/// half-rate samples to a caller-supplied nonlinearity, then low-passes and
/// decimates back down. Both filters use the same kernel.
///
/// Construction allocates nothing beyond the struct itself and computes the
/// kernel once; [`Oversampler2x::process`] is allocation-free and safe on the
/// realtime thread.
pub struct Oversampler2x {
    kernel: [f32; FIR_TAPS],
    /// Input history for the interpolating (upsampling) filter.
    up_history: [f32; HALF_TAPS],
    /// 2x-rate history for the decimating filter.
    down_history: [f32; FIR_TAPS],
    up_pos: usize,
    down_pos: usize,
}

impl Default for Oversampler2x {
    fn default() -> Self {
        Self::new()
    }
}

impl Oversampler2x {
    pub fn new() -> Self {
        Self {
            kernel: blackman_sinc_kernel(),
            up_history: [0.0; HALF_TAPS],
            down_history: [0.0; FIR_TAPS],
            up_pos: 0,
            down_pos: 0,
        }
    }

    /// Drop all filter state. Call when a chain is reset, not per block.
    pub fn reset(&mut self) {
        self.up_history = [0.0; HALF_TAPS];
        self.down_history = [0.0; FIR_TAPS];
        self.up_pos = 0;
        self.down_pos = 0;
    }

    /// Run one input sample through `f` at twice the sample rate.
    pub fn process<F: FnMut(f32) -> f32>(&mut self, input: f32, mut f: F) -> f32 {
        // Upsample. Zero-stuffing halves the signal's energy, so the kernel's
        // gain is doubled here to compensate.
        self.up_history[self.up_pos] = input;
        self.up_pos = (self.up_pos + 1) % HALF_TAPS;

        let mut even = 0.0;
        let mut odd = 0.0;
        for tap in 0..HALF_TAPS {
            // Most recent sample first.
            let idx = (self.up_pos + HALF_TAPS - 1 - tap) % HALF_TAPS;
            let sample = self.up_history[idx];
            even += sample * self.kernel[tap * 2];
            odd += sample * self.kernel[tap * 2 + 1];
        }

        let shaped_even = f(even * 2.0);
        let shaped_odd = f(odd * 2.0);

        // Decimate: low-pass the 2x stream, keep every second sample. Only
        // the retained phase's convolution is evaluated.
        self.push_down(shaped_even);
        self.push_down(shaped_odd);
        self.decimate()
    }

    fn push_down(&mut self, sample: f32) {
        self.down_history[self.down_pos] = sample;
        self.down_pos = (self.down_pos + 1) % FIR_TAPS;
    }

    fn decimate(&self) -> f32 {
        let mut acc = 0.0;
        for tap in 0..FIR_TAPS {
            let idx = (self.down_pos + FIR_TAPS - 1 - tap) % FIR_TAPS;
            acc += self.down_history[idx] * self.kernel[tap];
        }
        acc
    }
}

/// Blackman-windowed sinc low-pass at a quarter of the 2x-rate sample rate,
/// i.e. the base rate's Nyquist. Normalized to unity DC gain.
fn blackman_sinc_kernel() -> [f32; FIR_TAPS] {
    use core::f32::consts::PI;
    let mut kernel = [0.0f32; FIR_TAPS];
    let center = (FIR_TAPS - 1) as f32 / 2.0;
    let cutoff = 0.25; // cycles/sample at the oversampled rate
    let mut sum = 0.0;
    for (i, tap) in kernel.iter_mut().enumerate() {
        let n = i as f32 - center;
        let sinc = if n.abs() < 1e-6 {
            2.0 * cutoff
        } else {
            (2.0 * PI * cutoff * n).sin() / (PI * n)
        };
        let phase = 2.0 * PI * i as f32 / (FIR_TAPS - 1) as f32;
        let window = 0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos();
        *tap = sinc * window;
        sum += *tap;
    }
    for tap in kernel.iter_mut() {
        *tap /= sum;
    }
    kernel
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_is_linear_inside_unity_and_reflects_outside() {
        for (input, expected) in [
            (0.0, 0.0),
            (0.5, 0.5),
            (1.0, 1.0),
            (2.0, 0.0),
            (3.0, -1.0),
            (-1.0, -1.0),
            (-2.0, 0.0),
        ] {
            let actual = fold(input);
            assert!(
                (actual - expected).abs() < 1e-5,
                "fold({input}) = {actual}, expected {expected}"
            );
        }
    }

    #[test]
    fn every_curve_is_bounded_and_passes_through_zero() {
        for curve in [
            DriveCurve::Soft,
            DriveCurve::Hard,
            DriveCurve::Fold,
            DriveCurve::Tape,
        ] {
            assert!(
                shape(curve, 0.0).abs() < 1e-5,
                "{curve:?} does not pass through zero"
            );
            for step in -400..=400 {
                let x = step as f32 * 0.25;
                let y = shape(curve, x);
                assert!(
                    y.abs() <= 1.0 + 1e-4,
                    "{curve:?} produced {y} for input {x}, outside [-1, 1]"
                );
            }
        }
    }

    #[test]
    fn tape_is_asymmetric_and_soft_is_not() {
        // Even harmonics come from asymmetry: |f(x)| != |f(-x)|.
        let x = 0.7;
        assert!((shape(DriveCurve::Soft, x) + shape(DriveCurve::Soft, -x)).abs() < 1e-5);
        assert!((shape(DriveCurve::Tape, x) + shape(DriveCurve::Tape, -x)).abs() > 1e-3);
    }

    /// The oversampler must be near-transparent when the nonlinearity is the
    /// identity, apart from its filter delay.
    #[test]
    fn identity_shaping_preserves_a_sine() {
        let sr = 48_000.0f32;
        let freq = 1_000.0f32;
        let frames = 4_096;
        let mut os = Oversampler2x::new();
        let mut out = vec![0.0f32; frames];
        for (i, slot) in out.iter_mut().enumerate() {
            let x = (i as f32 / sr * freq * core::f32::consts::TAU).sin();
            *slot = os.process(x, |v| v);
        }
        // Skip the filter's group delay and startup transient.
        let settled = &out[512..];
        let rms = (settled.iter().map(|s| s * s).sum::<f32>() / settled.len() as f32).sqrt();
        let expected = (0.5f32).sqrt();
        assert!(
            (rms / expected - 1.0).abs() < 0.05,
            "identity oversampling changed level: {rms} vs {expected}"
        );
    }

    /// The point of the whole module: hard-clipping a high sine at base rate
    /// folds harmonics down into the audible band. At 2x they are filtered
    /// before they can. Compare energy well below the fundamental, where
    /// only aliases can land.
    #[test]
    fn oversampling_reduces_aliasing_below_the_fundamental() {
        let sr = 48_000.0f32;
        let freq = 9_000.0f32;
        let frames = 8_192;

        let mut naive = Vec::with_capacity(frames);
        let mut over = Vec::with_capacity(frames);
        let mut os = Oversampler2x::new();
        for i in 0..frames {
            let x = (i as f32 / sr * freq * core::f32::consts::TAU).sin() * 4.0;
            naive.push(shape(DriveCurve::Hard, x));
            over.push(os.process(x, |v| shape(DriveCurve::Hard, v)));
        }

        // Goertzel energy at 3 kHz — not a harmonic of 9 kHz, so anything
        // there is an alias.
        let probe = 3_000.0;
        let naive_alias = tone_energy(&naive[1_024..], sr, probe);
        let over_alias = tone_energy(&over[1_024..], sr, probe);
        assert!(
            over_alias < naive_alias * 0.5,
            "oversampled alias energy {over_alias} should be well under naive {naive_alias}"
        );
    }

    /// Single-bin DFT magnitude, normalized by length.
    fn tone_energy(samples: &[f32], sample_rate: f32, freq: f32) -> f32 {
        use core::f32::consts::TAU;
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (i, s) in samples.iter().enumerate() {
            let phase = TAU * freq * i as f32 / sample_rate;
            re += s * phase.cos();
            im -= s * phase.sin();
        }
        (re * re + im * im).sqrt() / samples.len() as f32
    }
}
