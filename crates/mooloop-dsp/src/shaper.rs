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
    ((x + TAPE_BIAS).tanh() - TAPE_BIAS_DC) / (1.0 + TAPE_BIAS_DC)
}

/// The Tape curve's fixed bias.
const TAPE_BIAS: f32 = 0.12;
/// `TAPE_BIAS.tanh()`, precomputed: `tanh` is not a const fn.
const TAPE_BIAS_DC: f32 = 0.119_427_3;

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
/// Odd, so the kernel is a true half-band: cut at a quarter of the 2x rate,
/// every tap an even distance from the centre is zero, apart from the centre
/// itself (MOO-250). That is what lets both filters skip half their work.
const FIR_TAPS: usize = 31;
/// The centre tap, and the delay of either filter at the 2x rate.
const CENTRE: usize = FIR_TAPS / 2;
/// The nonzero taps an odd distance from the centre: `kernel[0]`,
/// `kernel[2]`, ... `kernel[30]`. Both filters use exactly these, plus the
/// centre.
const BRANCH_TAPS: usize = CENTRE + 1;
/// How many frames back the centre tap reaches, in each filter's branch
/// history: the upsampler's odd phase is the input `CENTRE / 2` frames ago,
/// and the decimator's centre tap is the odd 2x sample `CENTRE / 2 + 1`
/// frames ago.
const CENTRE_FRAMES: usize = CENTRE / 2;

/// Effective base-rate latency of the complete interpolate/process/decimate
/// path: each filter delays by `CENTRE` samples at the 2x rate, so the pair
/// delays by `CENTRE` frames at the base rate, exactly.
///
/// Re-exported from `mooloop_core::effect`, which owns it: the control thread
/// sizes compensation delays from it before any node exists to ask, so it is
/// part of the device's declared interface rather than a private property of
/// this file. `identity_path_has_the_declared_latency` below is what keeps the declaration
/// honest against the filter that produces it: it drives an impulse through
/// the real path and asserts where the peak lands, which no derivation from
/// the tap count could do once the kernel changed shape.
pub use mooloop_core::effect::OVERSAMPLER_LATENCY_FRAMES as OVERSAMPLER_LATENCY_U32;

/// The same figure as a `usize`, for the array lengths and index arithmetic
/// in this crate.
pub const OVERSAMPLER_LATENCY_FRAMES: usize = OVERSAMPLER_LATENCY_U32 as usize;

/// A 2x oversampler for one audio channel.
///
/// Zero-stuffs to twice the rate through a windowed-sinc half-band low-pass,
/// hands both 2x samples to a caller-supplied nonlinearity, then low-passes
/// and decimates back down. Both filters use the same kernel.
///
/// **Polyphase, and half-band** (MOO-250). Zero-stuffing means the
/// upsampler's even output only ever meets the kernel's even taps, and its
/// odd output only the odd taps, which in a half-band are all zero but the
/// centre: so the odd output is one delayed, scaled input sample. The
/// decimator keeps only the even 2x samples, and its kernel's nonzero taps
/// fall on the even samples plus the centre, which lands on one odd sample.
/// That is 34 multiplies a frame where the 32-tap full-rate version did 64.
/// **A block at a time** (MOO-253). [`Oversampler2x::process_block`] runs up
/// to [`OVERSAMPLE_CHUNK`] frames through each stage in turn, over the
/// history and the chunk laid end to end in one scratch buffer: the upsampler
/// for every frame, then the caller's nonlinearity over all the 2x samples
/// at once, then the decimator. Each stage is a loop over frames with no
/// ring index and no call per sample, which is what lets the shaper and the
/// dot products vectorise.
///
/// Construction allocates nothing beyond the struct itself and computes the
/// kernel once; processing is allocation-free (the scratch is on the stack)
/// and safe on the realtime thread.
pub struct Oversampler2x {
    /// `kernel[2t]` in the order a history slice runs, oldest first: entry
    /// `i` weighs the sample `BRANCH_TAPS - 1 - i` frames old.
    branch: [f32; BRANCH_TAPS],
    /// The centre tap.
    centre: f32,
    /// The last `BRANCH_TAPS - 1` inputs, oldest first.
    up_history: [f32; BRANCH_TAPS - 1],
    /// The last `BRANCH_TAPS - 1` even 2x samples after the nonlinearity,
    /// oldest first.
    even_history: [f32; BRANCH_TAPS - 1],
    /// The last `CENTRE_FRAMES + 1` odd 2x samples after the nonlinearity,
    /// oldest first: the decimator's centre tap reads the oldest.
    odd_history: [f32; CENTRE_FRAMES + 1],
}

/// The most frames [`Oversampler2x::process_block`] runs through its stages
/// at once; a longer slice is taken this many at a time. It sizes the
/// scratch buffers on the stack.
pub const OVERSAMPLE_CHUNK: usize = 64;

impl Default for Oversampler2x {
    fn default() -> Self {
        Self::new()
    }
}

impl Oversampler2x {
    pub fn new() -> Self {
        let kernel = blackman_sinc_kernel();
        let mut branch = [0.0; BRANCH_TAPS];
        for (i, tap) in branch.iter_mut().enumerate() {
            *tap = kernel[2 * (BRANCH_TAPS - 1 - i)];
        }
        Self {
            branch,
            centre: kernel[CENTRE],
            up_history: [0.0; BRANCH_TAPS - 1],
            even_history: [0.0; BRANCH_TAPS - 1],
            odd_history: [0.0; CENTRE_FRAMES + 1],
        }
    }

    /// Drop all filter state. Call when a chain is reset, not per block.
    pub fn reset(&mut self) {
        self.up_history = [0.0; BRANCH_TAPS - 1];
        self.even_history = [0.0; BRANCH_TAPS - 1];
        self.odd_history = [0.0; CENTRE_FRAMES + 1];
    }

    /// Run one input sample through `f` at twice the sample rate. The same
    /// arithmetic as [`Self::process_block`] on a one-frame slice, so the two
    /// agree to the bit.
    pub fn process<F: FnMut(f32) -> f32>(&mut self, input: f32, mut f: F) -> f32 {
        let mut io = [input];
        self.process_block(&mut io, |even, odd| {
            even[0] = f(even[0]);
            odd[0] = f(odd[0]);
        });
        io[0]
    }

    /// Run `io` through the oversampler in place. `shape` is handed each
    /// chunk's 2x samples as two equal slices, the even ones (on the input
    /// frames) and the odd ones (between them), already at the path's gain,
    /// and shapes both in place; frame `k` of the chunk is `even[k]` then
    /// `odd[k]` in time.
    pub fn process_block<F: FnMut(&mut [f32], &mut [f32])>(&mut self, io: &mut [f32], mut shape: F) {
        for chunk in io.chunks_mut(OVERSAMPLE_CHUNK) {
            self.process_chunk(chunk, &mut shape);
        }
    }

    fn process_chunk<F: FnMut(&mut [f32], &mut [f32])>(&mut self, io: &mut [f32], shape: &mut F) {
        const UP: usize = BRANCH_TAPS - 1;
        const ODD: usize = CENTRE_FRAMES + 1;
        let frames = io.len();

        // Upsample. Zero-stuffing halves the signal's energy, so the kernel's
        // gain is doubled here to compensate.
        let mut inputs = [0.0f32; UP + OVERSAMPLE_CHUNK];
        inputs[..UP].copy_from_slice(&self.up_history);
        inputs[UP..UP + frames].copy_from_slice(io);
        let mut even = [0.0f32; OVERSAMPLE_CHUNK];
        let mut odd = [0.0f32; OVERSAMPLE_CHUNK];
        for k in 0..frames {
            // Oldest first, ending with frame `k`'s input.
            let window = &inputs[k..k + BRANCH_TAPS];
            even[k] = dot(window, &self.branch) * 2.0;
            odd[k] = window[BRANCH_TAPS - 1 - CENTRE_FRAMES] * self.centre * 2.0;
        }
        self.up_history.copy_from_slice(&inputs[frames..frames + UP]);

        shape(&mut even[..frames], &mut odd[..frames]);

        // Decimate: keep the even 2x sample, low-passed. Its even taps read
        // the even samples, and its centre tap the odd sample
        // `CENTRE_FRAMES + 1` frames back.
        let mut evens = [0.0f32; UP + OVERSAMPLE_CHUNK];
        evens[..UP].copy_from_slice(&self.even_history);
        evens[UP..UP + frames].copy_from_slice(&even[..frames]);
        let mut odds = [0.0f32; ODD + OVERSAMPLE_CHUNK];
        odds[..ODD].copy_from_slice(&self.odd_history);
        odds[ODD..ODD + frames].copy_from_slice(&odd[..frames]);
        for (k, output) in io.iter_mut().enumerate() {
            *output = dot(&evens[k..k + BRANCH_TAPS], &self.branch) + odds[k] * self.centre;
        }
        self.even_history.copy_from_slice(&evens[frames..frames + UP]);
        self.odd_history.copy_from_slice(&odds[frames..frames + ODD]);
    }
}

/// A dot product over one branch, in four lanes so the compiler can
/// vectorise it: a single running sum is a chain of dependent float adds it
/// may not reorder.
fn dot(samples: &[f32], taps: &[f32; BRANCH_TAPS]) -> f32 {
    let samples: &[f32; BRANCH_TAPS] = samples.try_into().expect("a branch-long slice");
    let mut lanes = [0.0f32; 4];
    for (samples, taps) in samples.as_chunks::<4>().0.iter().zip(taps.as_chunks::<4>().0) {
        for lane in 0..4 {
            lanes[lane] += samples[lane] * taps[lane];
        }
    }
    (lanes[0] + lanes[1]) + (lanes[2] + lanes[3])
}

/// Blackman-windowed sinc low-pass at a quarter of the 2x-rate sample rate,
/// i.e. the base rate's Nyquist. Normalized to unity DC gain.
///
/// A half-band: the sinc is zero at every even distance from the centre but
/// the centre itself. Those taps are set to exactly zero rather than left at
/// the few parts in 1e8 `sin` rounds them to, so that skipping them in
/// [`Oversampler2x`] is not an approximation.
fn blackman_sinc_kernel() -> [f32; FIR_TAPS] {
    use core::f32::consts::PI;
    let mut kernel = [0.0f32; FIR_TAPS];
    let cutoff = 0.25; // cycles/sample at the oversampled rate
    let mut sum = 0.0;
    for (i, tap) in kernel.iter_mut().enumerate() {
        let offset = i as i32 - CENTRE as i32;
        let n = offset as f32;
        let sinc = if offset == 0 {
            2.0 * cutoff
        } else if offset % 2 == 0 {
            0.0
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

/// `tanh`, as a [7/6] Padé approximant clamped to ±1, for the curves
/// [`shape_oversampled`] runs twice a frame per channel (MOO-250).
///
/// Within 1.0e-4 of `f32::tanh` everywhere, the worst at |x| near 4.97 where
/// the rational function reaches 1 and the clamp takes over
/// (`fast_tanh_is_within_its_stated_error`). Odd, bounded, monotone to within
/// float rounding, and one division where libm's is several times the work. Only the oversampled path uses
/// it: what a test pins to the bit, and every curve outside the 2x path,
/// keeps `f32::tanh`.
pub(crate) fn fast_tanh(x: f32) -> f32 {
    // Past 5 the rational function is over 1 anyway, and the input is held
    // there so its seventh power cannot overflow into inf / inf.
    let x = x.clamp(-5.0, 5.0);
    let x2 = x * x;
    let numerator = x * (135_135.0 + x2 * (17_325.0 + x2 * (378.0 + x2)));
    let denominator = 135_135.0 + x2 * (62_370.0 + x2 * (3_150.0 + x2 * 28.0));
    (numerator / denominator).clamp(-1.0, 1.0)
}

/// [`shape`] for the inside of [`Oversampler2x`]: the same curves, with
/// [`fast_tanh`] for `Soft` and `Tape`. The difference is at most 1e-4 of full
/// scale before the decimator, and the decimator's low-pass only removes from
/// it.
pub fn shape_oversampled(curve: DriveCurve, x: f32) -> f32 {
    match curve {
        DriveCurve::Soft => fast_tanh(x),
        DriveCurve::Tape => (fast_tanh(x + TAPE_BIAS) - TAPE_BIAS_DC) / (1.0 + TAPE_BIAS_DC),
        DriveCurve::Hard | DriveCurve::Fold => shape(curve, x),
    }
}

/// [`shape_oversampled`] over a slice, each sample pre-gained by its own
/// `gain`: `samples[k] = shape_oversampled(curve, samples[k] * gains[k])`,
/// with the curve chosen once for the slice rather than per sample, so the
/// loop vectorises (MOO-253).
pub fn shape_oversampled_slice(curve: DriveCurve, samples: &mut [f32], gains: &[f32]) {
    let pairs = samples.iter_mut().zip(gains);
    match curve {
        DriveCurve::Soft => pairs.for_each(|(x, g)| *x = fast_tanh(*x * g)),
        DriveCurve::Tape => pairs.for_each(|(x, g)| {
            *x = (fast_tanh(*x * g + TAPE_BIAS) - TAPE_BIAS_DC) / (1.0 + TAPE_BIAS_DC)
        }),
        DriveCurve::Hard => pairs.for_each(|(x, g)| *x = (*x * g).clamp(-1.0, 1.0)),
        DriveCurve::Fold => pairs.for_each(|(x, g)| *x = fold(*x * g)),
    }
}

/// The oversampler as it was before MOO-250: a 32-tap full-rate kernel, both
/// filters convolving every tap through a `%` ring. Kept for the tests that
/// measure the new one against it, for its sound and its cost.
#[cfg(test)]
pub(crate) mod before_moo250 {
    const FIR_TAPS: usize = 32;
    const HALF_TAPS: usize = FIR_TAPS / 2;

    pub(crate) struct Oversampler2x {
        pub(crate) kernel: [f32; FIR_TAPS],
        up_history: [f32; HALF_TAPS],
        down_history: [f32; FIR_TAPS],
        up_pos: usize,
        down_pos: usize,
    }

    impl Oversampler2x {
        pub(crate) fn new() -> Self {
            Self {
                kernel: kernel(),
                up_history: [0.0; HALF_TAPS],
                down_history: [0.0; FIR_TAPS],
                up_pos: 0,
                down_pos: 0,
            }
        }

        pub(crate) fn process<F: FnMut(f32) -> f32>(&mut self, input: f32, mut f: F) -> f32 {
            self.up_history[self.up_pos] = input;
            self.up_pos = (self.up_pos + 1) % HALF_TAPS;
            let mut even = 0.0;
            let mut odd = 0.0;
            for tap in 0..HALF_TAPS {
                let idx = (self.up_pos + HALF_TAPS - 1 - tap) % HALF_TAPS;
                let sample = self.up_history[idx];
                even += sample * self.kernel[tap * 2];
                odd += sample * self.kernel[tap * 2 + 1];
            }
            let shaped_even = f(even * 2.0);
            let shaped_odd = f(odd * 2.0);
            self.push_down(shaped_even);
            self.push_down(shaped_odd);
            let mut acc = 0.0;
            for tap in 0..FIR_TAPS {
                let idx = (self.down_pos + FIR_TAPS - 1 - tap) % FIR_TAPS;
                acc += self.down_history[idx] * self.kernel[tap];
            }
            acc
        }

        fn push_down(&mut self, sample: f32) {
            self.down_history[self.down_pos] = sample;
            self.down_pos = (self.down_pos + 1) % FIR_TAPS;
        }
    }

    fn kernel() -> [f32; FIR_TAPS] {
        use core::f32::consts::PI;
        let mut kernel = [0.0f32; FIR_TAPS];
        let center = (FIR_TAPS - 1) as f32 / 2.0;
        let cutoff = 0.25;
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
    /// The followers' coefficient, a constant of the sample rate, and the
    /// rate it was made for (0 before the first sample). It was an `exp` on
    /// every sample until MOO-249.
    follow: f32,
    follow_rate: u32,
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
            follow: 0.0,
            follow_rate: 0,
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

        if sample_rate != self.follow_rate {
            self.follow = 1.0 - (-1.0 / (PRE_DRIVE_FOLLOW_S * sample_rate as f32)).exp();
            self.follow_rate = sample_rate;
        }
        let follow = self.follow;
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

    /// MOO-249: the followers' coefficient is made once per sample rate
    /// instead of every sample, by the same expression, so the output is
    /// the per-sample stage's to the bit, across a drive sweep and a change
    /// of sample rate mid-stream.
    #[test]
    fn pre_drive_is_bit_identical_with_its_coefficient_cached() {
        struct PerSample {
            mean_input: f32,
            mean_shaped: f32,
        }
        impl PerSample {
            fn next_sample(&mut self, input: f32, drive: f32, sample_rate: u32) -> f32 {
                let drive = clamp_param(drive, 0.0, 1.0);
                if drive <= f32::EPSILON {
                    return input;
                }
                let gain = 1.0 + drive * PRE_DRIVE_GAIN_RANGE;
                let shaped = (input * gain).tanh();
                let follow = 1.0 - (-1.0 / (PRE_DRIVE_FOLLOW_S * sample_rate as f32)).exp();
                self.mean_input += follow * (input * input - self.mean_input);
                self.mean_shaped += follow * (shaped * shaped - self.mean_shaped);
                let compensation = if self.mean_shaped > 1.0e-12 {
                    (self.mean_input / self.mean_shaped).sqrt()
                } else {
                    1.0 / gain
                };
                shaped * compensation
            }
        }
        let mut cached = PreDrive::new();
        let mut reference = PerSample {
            mean_input: 0.0,
            mean_shaped: 0.0,
        };
        for index in 0..96_000u32 {
            let rate = if index < 48_000 { 48_000 } else { 44_100 };
            let input = (index as f32 * 0.013).sin() * 0.8 + (index as f32 * 0.0007).sin() * 0.3;
            let drive = (index % 20_000) as f32 / 20_000.0;
            let a = cached.next_sample(input, drive, rate);
            let b = reference.next_sample(input, drive, rate);
            assert_eq!(a.to_bits(), b.to_bits(), "sample {index}");
        }
    }

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
            let mut before = before_moo250::Oversampler2x::new();
            let over_before: Vec<f32> = input
                .iter()
                .map(|&x| before.process(x, |v| shape(DriveCurve::Hard, v)))
                .collect();
            let band = (20.0, 20_000.0);
            let naive_alias = alias_db(&naive, sr, freq, band);
            let over_alias = alias_db(&over, sr, freq, band);
            let before_alias = alias_db(&over_before, sr, freq, band);
            println!(
                "{sr} Hz: hard clip aliases at {naive_alias:.1} dB, oversampled {over_alias:.1} dB \
                 (the 32-tap path before MOO-250: {before_alias:.1} dB)"
            );
            // Within 3 dB of the 32-tap path at every rate: measured
            // 2026-09-25 as 0.0, +0.9, -5.4 and +2.6 dB at 44.1, 48, 96 and
            // 192 kHz. The loudest single alias line moves with the stopband's
            // exact ripple, which one tap changes.
            assert!(
                over_alias <= before_alias + 3.0,
                "{sr} Hz: the half-band path aliases at {over_alias:.1} dB, the 32-tap one at \
                 {before_alias:.1} dB"
            );
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

    /// [`fast_tanh`]'s stated error, over the whole range a driven sample
    /// can reach, and its shape: odd, bounded, never decreasing.
    #[test]
    fn fast_tanh_is_within_its_stated_error() {
        let mut worst = 0.0f32;
        let mut previous = -1.0f32;
        for step in -200_000..=200_000 {
            let x = step as f32 * 1.0e-4;
            let fast = fast_tanh(x);
            worst = worst.max((fast - x.tanh()).abs());
            // Monotone to within float rounding: near |x| = 5, where the
            // curve flattens into 1, the rational function's terms are large
            // enough that one step can round a few ulps below the last.
            assert!(fast >= previous - 5.0e-7, "fast_tanh decreases at {x}");
            assert_eq!(fast, -fast_tanh(-x), "fast_tanh is not odd at {x}");
            assert!(fast.abs() <= 1.0);
            previous = fast;
        }
        for x in [100.0f32, 1.0e6, f32::MAX] {
            assert_eq!(fast_tanh(x), 1.0);
        }
        println!("fast_tanh is within {worst:.2e} of tanh over -20..20");
        assert!(worst <= 1.0e-4, "fast_tanh is {worst} from tanh");
    }

    /// The kernel really is a half-band: the taps the filters skip are zero,
    /// and the two phases each carry half the gain.
    #[test]
    fn the_kernel_is_a_half_band() {
        let kernel = blackman_sinc_kernel();
        for (i, tap) in kernel.iter().enumerate() {
            let offset = i as i32 - CENTRE as i32;
            if offset != 0 && offset % 2 == 0 {
                assert_eq!(*tap, 0.0, "tap {i}");
            }
        }
        let branch: f32 = kernel.iter().step_by(2).sum();
        assert!((branch - 0.5).abs() < 2.0e-3, "the even taps sum to {branch}");
        assert!((kernel[CENTRE] - 0.5).abs() < 2.0e-3, "the centre is {}", kernel[CENTRE]);
    }

    /// The kernel's response in the band the decimator has to throw away,
    /// from 0.35 of the 2x rate to its Nyquist (above 1.4 times the base
    /// rate's Nyquist): no worse than the 32-tap kernel it replaced (MOO-250).
    #[test]
    fn the_half_band_rejects_its_stopband_as_well_as_the_kernel_before_it() {
        fn worst_db(kernel: &[f32]) -> f32 {
            let mut worst = 0.0f32;
            for step in 0..=300 {
                let f = 0.35 + 0.15 * step as f32 / 300.0;
                let (mut re, mut im) = (0.0f32, 0.0f32);
                for (n, tap) in kernel.iter().enumerate() {
                    let w = core::f32::consts::TAU * f * n as f32;
                    re += tap * w.cos();
                    im -= tap * w.sin();
                }
                worst = worst.max((re * re + im * im).sqrt());
            }
            20.0 * worst.max(1.0e-12).log10()
        }
        let now = worst_db(&blackman_sinc_kernel());
        let before = worst_db(&before_moo250::Oversampler2x::new().kernel);
        println!("stopband: {now:.1} dB now, {before:.1} dB before");
        assert!(now <= before + 1.0, "the stopband rose from {before:.1} to {now:.1} dB");
        assert!(now < -60.0, "the stopband is only {now:.1} dB down");
    }

    /// **The half-band oversampler sounds like the one it replaced**
    /// (MOO-250). A two-tone signal hard-clipped and soft-clipped through
    /// both, at every rate: the new one against the old, over the same
    /// latency. The two kernels differ by one tap, so this is the size of that
    /// difference, and it is held under 50 dB below the signal.
    #[test]
    fn the_half_band_path_matches_the_32_tap_one() {
        for sr in RATES {
            for curve in [DriveCurve::Soft, DriveCurve::Hard] {
                let frames = frames_for(0.25, sr);
                let input: Vec<f32> = (0..frames)
                    .map(|i| {
                        let t = i as f32 / sr as f32;
                        0.5 * (t * 220.0 * core::f32::consts::TAU).sin()
                            + 0.3 * (t * 3_100.0 * core::f32::consts::TAU).sin()
                    })
                    .collect();
                let mut now = Oversampler2x::new();
                let mut before = before_moo250::Oversampler2x::new();
                let (mut diff, mut power) = (0.0f64, 0.0f64);
                for &x in &input {
                    let a = now.process(x, |v| shape(curve, v * 3.0));
                    let b = before.process(x, |v| shape(curve, v * 3.0));
                    diff += f64::from((a - b) * (a - b));
                    power += f64::from(b * b);
                }
                let null_db = 10.0 * (diff / power).log10();
                println!("{sr} Hz {curve:?}: the two paths differ by {null_db:.1} dB");
                assert!(null_db < -50.0, "{sr} Hz {curve:?}: only {null_db:.1} dB apart");
            }
        }
    }

    /// A block at a time and a sample at a time are the same arithmetic,
    /// so they agree to the bit, however the input is cut (MOO-253).
    #[test]
    fn a_block_at_a_time_matches_a_sample_at_a_time() {
        let input = sine(1_234.0, 0.9, 48_000, 1_000);
        let shaped = |x: f32| shape_oversampled(DriveCurve::Tape, x * 3.0);
        let mut one = Oversampler2x::new();
        let expected: Vec<f32> = input.iter().map(|&x| one.process(x, shaped)).collect();
        for cut in [1usize, 7, 64, 65, 200] {
            let mut blocks = Oversampler2x::new();
            let mut output = input.clone();
            for part in output.chunks_mut(cut) {
                blocks.process_block(part, |even, odd| {
                    for x in even.iter_mut().chain(odd.iter_mut()) {
                        *x = shaped(*x);
                    }
                });
            }
            for (k, (got, want)) in output.iter().zip(&expected).enumerate() {
                assert_eq!(got.to_bits(), want.to_bits(), "cut {cut}, frame {k}");
            }
        }
    }

    /// The slice shaper is `shape_oversampled` per sample, to the bit.
    #[test]
    fn the_slice_shaper_matches_the_sample_shaper() {
        let xs: Vec<f32> = (0..400).map(|k| (k as f32 - 200.0) * 0.013).collect();
        let gains: Vec<f32> = (0..400).map(|k| 1.0 + (k % 9) as f32).collect();
        for curve in [DriveCurve::Soft, DriveCurve::Hard, DriveCurve::Fold, DriveCurve::Tape] {
            let mut slice = xs.clone();
            shape_oversampled_slice(curve, &mut slice, &gains);
            for k in 0..xs.len() {
                let want = shape_oversampled(curve, xs[k] * gains[k]);
                assert_eq!(slice[k].to_bits(), want.to_bits(), "{curve:?} at {k}");
            }
        }
    }

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
