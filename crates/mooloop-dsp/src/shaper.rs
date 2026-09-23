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

use crate::scale::clamp_param;

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

// ---------------------------------------------------------------------------
// Saturation stages for voices and filters. Moved here from `filter.rs`
// (MOO-144) so every saturation stage lives in one module under one
// anti-aliasing policy: these are smooth `tanh`-family curves run at the base
// rate, whose harmonics fall away fast enough that a voice's own filter and
// the 0.45 x sample rate ceiling keep folded content inaudible; the hard and
// folding curves above, whose harmonics do not fall away, run through
// [`Oversampler2x`].
// ---------------------------------------------------------------------------

/// Compensated soft saturation shared by the sampler, drum synth, both
/// synths, and the filter effect: pre-gain into `tanh`, normalized by the
/// shaper's own response to a reference-level signal, so raising drive
/// changes character, not level. A static nonlinearity cannot be level-flat
/// at every input; anchoring at the operating level
/// (`mooloop_core::gain::REFERENCE_PEAK_DBFS`) is the compromise, and it
/// also caps a full-scale peak at the reference rather than at clipping.
pub fn apply_drive(input: f32, drive: f32) -> f32 {
    let drive = clamp_param(drive, 0.0, 1.0);
    if drive <= f32::EPSILON {
        return input;
    }
    apply_drive_compensated(input, drive, voice_drive_compensation(drive))
}

/// The part of [`apply_drive`]'s response that depends on `drive` alone, not
/// on the sample it shapes: a `tanh` of the driven reference level.
///
/// A caller shaping many samples at one `drive` value -- a whole block, a
/// whole voice between parameter events -- computes this once and reuses it
/// through [`apply_drive_compensated`] rather than paying the `tanh` again
/// for every sample, exactly as `apply_drive` already does internally for a
/// single call.
pub fn voice_drive_compensation(drive: f32) -> f32 {
    let drive = clamp_param(drive, 0.0, 1.0);
    if drive <= f32::EPSILON {
        // Unused by `apply_drive_compensated`'s own bypass at this drive, but
        // a finite, well-defined value rather than one that only happens to
        // never be read.
        return 1.0;
    }
    let input_gain = 1.0 + drive * 15.0;
    DRIVE_REFERENCE_LINEAR / (DRIVE_REFERENCE_LINEAR * input_gain).tanh()
}

/// [`apply_drive`] with [`voice_drive_compensation`] already computed. The per-call
/// `tanh` of the *sample* still has to happen here -- that one genuinely
/// varies every call -- so this only removes the one `tanh` that does not.
pub fn apply_drive_compensated(input: f32, drive: f32, compensation: f32) -> f32 {
    let drive = clamp_param(drive, 0.0, 1.0);
    if drive <= f32::EPSILON {
        return input;
    }
    let input_gain = 1.0 + drive * 15.0;
    (input * input_gain).tanh() * compensation
}

/// A safety ceiling for a voice's output: exactly transparent below the knee,
/// asymptotic to [`VOICE_CEILING`] above it.
///
/// This is a bound, not a tone stage. [`Ladder`](crate::filter::Ladder) and [`Acid`](crate::filter::Acid) cannot exceed 1
/// by construction, since their stages only ever integrate a shaper output, so
/// for them this never engages at all. [`Svf`](crate::filter::Svf) is linear and has no such
/// guarantee: at full resonance with three oscillators pushed into it, it will
/// happily hand back four times full scale. The knee sits well above the
/// nominal voice level (one oscillator at its 0 dB top lands near 0.7), so a
/// patch that is merely loud passes through untouched.
pub fn soft_ceiling(input: f32) -> f32 {
    let magnitude = input.abs();
    if magnitude <= VOICE_CEILING_KNEE {
        return input;
    }
    let headroom = VOICE_CEILING - VOICE_CEILING_KNEE;
    let over = (magnitude - VOICE_CEILING_KNEE) / headroom;
    input.signum() * (VOICE_CEILING_KNEE + headroom * over.tanh())
}

/// Where the ceiling starts to bend. Above the loudest a sane patch reaches,
/// below where a resonant linear filter runs away.
const VOICE_CEILING_KNEE: f32 = 1.5;

/// Asymptote. Chosen against `VOICE_OUTPUT_REFERENCE` so a voice at the bound,
/// at full envelope and full velocity, still lands under full scale.
pub(crate) const VOICE_CEILING: f32 = 2.5;

/// Saturation that runs *ahead* of a filter rather than after it.
///
/// ```text
/// PreDrive
/// in:  audio, drive (0..1)
/// out: audio
/// ```
///
/// [`apply_drive`] anchors its makeup gain at the fixed operating level, which
/// is right for a stage at the end of a chain where the level is known. Ahead
/// of the filter the level is not known: it is the oscillator mix, and three
/// oscillators at full level sum to roughly three times one. Anchoring at a
/// constant there would make the Drive knob a volume control that happens to
/// distort.
///
/// So the makeup gain follows a peak estimate of the input instead. Two
/// consequences, and both are the point:
///
/// - Sweeping Drive on a fixed patch changes harmonic content and leaves the
///   level where it was, whatever that level happens to be.
/// - Raising an oscillator's level pushes harder into the shaper, so it
///   changes the timbre and not merely the gain. That is what makes the mixer
///   a tone control.
#[derive(Clone, Copy, Debug)]
pub struct PreDrive {
    /// Running mean square of the input and of the shaped signal. The ratio of
    /// their roots is the makeup gain.
    mean_input: f32,
    mean_shaped: f32,
}

/// Pre-gain at full drive. Much gentler than [`apply_drive`]'s, and
/// deliberately: that stage is anchored at the operating level, roughly a
/// quarter of full scale, while this one sees a raw oscillator mix at around
/// unity. At `apply_drive`'s range every patch would be a square wave by a
/// third of the way up the knob, and level would stop changing the timbre --
/// which is the one thing this stage exists to make it do.
const PRE_DRIVE_GAIN_RANGE: f32 = 4.0;

/// Level-follower time constant. Long enough not to follow the waveform
/// itself, short enough to keep up with an envelope.
const PRE_DRIVE_FOLLOW_S: f32 = 0.05;

impl PreDrive {
    pub fn new() -> Self {
        Self {
            mean_input: 0.0,
            mean_shaped: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.mean_input = 0.0;
        self.mean_shaped = 0.0;
    }

    pub fn next_sample(&mut self, input: f32, drive: f32, sample_rate: u32) -> f32 {
        let drive = clamp_param(drive, 0.0, 1.0);
        if drive <= f32::EPSILON {
            return input;
        }
        let gain = 1.0 + drive * PRE_DRIVE_GAIN_RANGE;
        let shaped = (input * gain).tanh();

        let follow = 1.0 - (-1.0 / (PRE_DRIVE_FOLLOW_S * sample_rate as f32)).exp();
        self.mean_input += follow * (input * input - self.mean_input);
        self.mean_shaped += follow * (shaped * shaped - self.mean_shaped);

        // Matching RMS rather than peak is what makes the knob a character
        // control: a saturated wave carries more energy for the same peak, so
        // peak-matching would still let Drive raise the loudness. Before the
        // followers have anything in them the ratio tends to `1 / gain`, which
        // cancels the pre-gain exactly, so the stage starts as a pass-through
        // instead of a burst.
        let compensation = if self.mean_shaped > 1.0e-12 {
            (self.mean_input / self.mean_shaped).sqrt()
        } else {
            1.0 / gain
        };
        shaped * compensation
    }
}

impl Default for PreDrive {
    fn default() -> Self {
        Self::new()
    }
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
