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
//! See `docs/MODULATION.md` ("Anti-aliasing policy").

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

/// Signal level (linear) that drive compensation anchors to: the operating
/// level, `10^(REFERENCE_PEAK_DBFS/20)`. Written as a literal because it is
/// used per sample; `drive_reference_matches_the_operating_level` holds it to
/// `mooloop_core::gain::REFERENCE_PEAK_DBFS`.
pub(crate) const DRIVE_REFERENCE_LINEAR: f32 = 0.251;

/// Output scaling anchored at full scale: a full-scale input stays near full
/// scale as drive rises. DS-01's Hard, Fold and Crush characters are
/// calibrated against this; the Drive effect uses
/// [`reference_drive_compensation`] instead.
///
/// The asymptotic curves are normalized by their own response to the drive
/// amount. `Hard` already bounds its output, and `Fold` is non-monotonic in
/// drive so normalizing by its response would swing wildly — it gets a gentle
/// square-root law instead.
pub fn drive_compensation(curve: DriveCurve, drive: f32) -> f32 {
    let drive = drive.max(1.0);
    match curve {
        DriveCurve::Soft | DriveCurve::Tape => 1.0 / shape(curve, drive).max(0.1),
        DriveCurve::Hard => 1.0,
        DriveCurve::Fold => 1.0 / drive.sqrt(),
    }
}

/// Output scaling anchored at the operating level, so raising drive changes
/// character, not level: a sine peaking at [`DRIVE_REFERENCE_LINEAR`] comes out
/// peaking there at any drive, on any curve. Signals arrive near that level
/// (`docs/GAIN_STRUCTURE.md`), which is why this is the anchor rather than
/// full scale -- anchored at full scale, the Drive effect's default drive was
/// +5.6 dB of plain volume. The compromise is the same as `apply_drive`'s: a
/// static nonlinearity cannot be level-flat at every input, and a hotter one
/// is held down toward the reference instead.
///
/// It normalizes by the peak the curve gives a reference-level sine, which is
/// what keeps it monotonic where normalizing by a single point would not be.
/// `Hard` and `Fold` are both the identity up to 1 and never exceed it, so
/// that peak is `min(peak_in, 1)` for both; `Tape` is asymmetric, so it
/// takes the larger of its two half-waves.
pub fn reference_drive_compensation(curve: DriveCurve, drive: f32) -> f32 {
    let peak_in = DRIVE_REFERENCE_LINEAR * drive.max(1.0);
    let peak_out = match curve {
        DriveCurve::Soft => shape(curve, peak_in),
        DriveCurve::Tape => shape(curve, peak_in).max(-shape(curve, -peak_in)),
        DriveCurve::Hard | DriveCurve::Fold => peak_in.min(1.0),
    };
    DRIVE_REFERENCE_LINEAR / peak_out
}

/// Number of FIR taps in the oversampler's anti-imaging/anti-aliasing filter.
/// Even, so it splits into two equal polyphase branches.
const FIR_TAPS: usize = 32;
const HALF_TAPS: usize = FIR_TAPS / 2;

/// Effective base-rate latency of the complete interpolate/process/decimate
/// path. Both 32-tap filters contribute; the retained polyphase output has its
/// impulse peak at frame 15.
///
/// Re-exported from `mooloop_core::effect`, which owns it: the control thread
/// sizes compensation delays from it before any node exists to ask, so it is
/// part of the device's declared interface rather than a private property of
/// this file. `identity_path_has_the_declared_latency` below is what keeps the declaration
/// honest against the filter that produces it: it drives an impulse through
/// the real path and asserts where the peak lands, which no derivation from
/// `HALF_TAPS` could do once the kernel changed shape.
pub use mooloop_core::effect::OVERSAMPLER_LATENCY_FRAMES as OVERSAMPLER_LATENCY_U32;

/// The same figure as a `usize`, for the array lengths and index arithmetic
/// in this crate.
pub const OVERSAMPLER_LATENCY_FRAMES: usize = OVERSAMPLER_LATENCY_U32 as usize;

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
    use crate::testkit::{alias_db, frames_for, sine, Probe, RATES};

    /// The literal is a copy of a value `mooloop-core` owns; this is what
    /// reads the original.
    #[test]
    fn drive_reference_matches_the_operating_level() {
        let operating = 10.0_f32.powf(mooloop_core::gain::REFERENCE_PEAK_DBFS / 20.0);
        assert!(
            (DRIVE_REFERENCE_LINEAR - operating).abs() < 5.0e-4,
            "{DRIVE_REFERENCE_LINEAR} vs {operating}"
        );
    }

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
    /// identity, apart from its filter delay -- in the audible band, at every
    /// rate: its filters are fixed in proportion to the rate, so the band they
    /// have to pass is a smaller share of it the higher the rate runs.
    #[test]
    fn identity_shaping_preserves_a_sine() {
        for sr in RATES {
            for freq in [100.0_f32, 1_000.0, 10_000.0] {
                let mut os = Oversampler2x::new();
                let gain = Probe::new(sr).gain_db(|x| os.process(x, |v| v), freq);
                assert!(
                    gain.abs() < IDENTITY_PASSBAND_DB,
                    "{sr} Hz: identity oversampling moved {freq} Hz by {gain:.3} dB"
                );
            }
        }
    }

    /// How far the identity path may move a tone in the audible band.
    const IDENTITY_PASSBAND_DB: f32 = 0.5;

    /// The point of the whole module: hard-clipping a high sine at base rate
    /// folds harmonics down into the audible band. At 2x they are filtered
    /// before they can. The kit's alias measure finds the loudest component
    /// that is not a harmonic of the note, at every rate.
    #[test]
    fn oversampling_reduces_aliasing_below_the_fundamental() {
        let freq = 9_000.0_f32;
        for sr in RATES {
            let input = sine(freq, 4.0, sr, frames_for(0.5, sr));
            let mut os = Oversampler2x::new();
            let naive: Vec<f32> = input.iter().map(|&x| shape(DriveCurve::Hard, x)).collect();
            let over: Vec<f32> = input
                .iter()
                .map(|&x| os.process(x, |v| shape(DriveCurve::Hard, v)))
                .collect();
            let band = (20.0, 20_000.0);
            let naive_alias = alias_db(&naive, sr, freq, band);
            let over_alias = alias_db(&over, sr, freq, band);
            println!("{sr} Hz: hard clip aliases at {naive_alias:.1} dB, oversampled {over_alias:.1} dB");
            assert!(
                over_alias < naive_alias - OVERSAMPLED_ALIAS_MARGIN_DB,
                "{sr} Hz: oversampled alias {over_alias:.1} dB should be well under naive \
                 {naive_alias:.1} dB"
            );
        }
    }

    /// How much the 2x path has to take off the loudest alias of a
    /// hard-clipped 9 kHz sine.
    const OVERSAMPLED_ALIAS_MARGIN_DB: f32 = 6.0;

    #[test]
    fn identity_path_has_the_declared_latency() {
        let mut oversampler = Oversampler2x::new();
        let mut impulse = [0.0f32; 64];
        for (frame, output) in impulse.iter_mut().enumerate() {
            *output = oversampler.process((frame == 0) as u8 as f32, |sample| sample);
        }
        let peak = impulse
            .iter()
            .enumerate()
            .max_by(|(_, left), (_, right)| left.abs().total_cmp(&right.abs()))
            .map(|(index, _)| index)
            .unwrap();
        assert_eq!(peak, OVERSAMPLER_LATENCY_FRAMES);
    }
}
