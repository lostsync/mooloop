//! Small filters shared by the synth voices.
//!
//! Until 2026-09-22 the sampler kept its own inline per-voice filter math
//! rather than calling `Svf` directly: `Svf::next_sample`/`tick` recomputed
//! `g`/`damping`/`a1`/`a2`/`a3` (including a `tan()`) from cutoff/resonance
//! on every call, and the sampler needed one shared coefficient set applied
//! to both channels of a stereo frame, which its inline version got by
//! computing the coefficients once and ticking L/R against the same values
//! — calling `Svf::next_sample` once per channel would have recomputed them
//! twice, and a synthetic benchmark isolating just this (32 voices, 4s of
//! audio at 48 kHz, coefficients varying every 1000 frames) measured
//! shared-coefficient-per-frame at ~154ms versus per-channel-recompute at
//! ~310ms — about 2x, dominated by the doubled `tan()`
//! (`docs/plans/archive/share-dsp-primitives/03-collapse-duplicate-implementations.md`).
//! `SvfCoeffs` (below) removes the reason to copy the math: `Svf::tick_with`
//! takes a coefficient set instead of deriving one, so the sampler now
//! computes it once (`SvfCoeffs::for_cutoff`) and ticks a plain `Svf` per
//! channel against it — one evaluation, no duplicated formula, no doubling.
//! See `reports/fable-2026-09-22.md` finding 2, Plan B.

use mooloop_core::DriveCurve;

use crate::scale::clamp_param;
use crate::shaper::DRIVE_REFERENCE_LINEAR;

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

    /// Whether the stage's stored energy has decayed far enough that silent
    /// input produces silent output. At high resonance this stays false for
    /// a long time, which is the honest answer: a nearly self-oscillating
    /// SVF is still ringing.
    pub fn is_at_rest(&self) -> bool {
        self.low.abs() <= crate::node::REST_EPSILON && self.band.abs() <= crate::node::REST_EPSILON
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

    /// Process one sample from a precomputed coefficient set: only the
    /// per-sample state update, none of [`SvfCoeffs::for_cutoff`]'s `tan()`
    /// or divide. `tick` (and the `next_sample*` wrappers built on it) stay
    /// the per-call convenience that derives coefficients fresh and then
    /// calls this; a caller rendering many samples at the same, or a
    /// lerped, coefficient set should call this directly instead — see
    /// `effects/filter.rs`'s `process_range` and the sampler's voice
    /// filter.
    pub fn tick_with(&mut self, input: f32, coeffs: &SvfCoeffs) -> (f32, f32, f32) {
        let v3 = input - self.low;
        let v1 = coeffs.a1 * self.band + coeffs.a2 * v3;
        let v2 = self.low + coeffs.a2 * self.band + coeffs.a3 * v3;
        let high = input - coeffs.damping * v1 - v2;
        self.band = 2.0 * v1 - self.band;
        self.low = 2.0 * v2 - self.low;
        (v2, v1, high)
    }

    fn tick(
        &mut self,
        input: f32,
        cutoff_hz: f32,
        resonance: f32,
        sample_rate: u32,
    ) -> (f32, f32, f32) {
        let coeffs = SvfCoeffs::for_cutoff(cutoff_hz, resonance, sample_rate);
        self.tick_with(input, &coeffs)
    }
}

/// The coefficient set [`Svf::tick`] used to re-derive from `cutoff_hz`,
/// `resonance` and `sample_rate` on every call: `tan()`, a divide, and three
/// products. Computing it once — per block, or per control tick with
/// [`SvfCoeffs::lerp`] carrying the ramp across it — and feeding it to
/// [`Svf::tick_with`] turns that per-sample cost into one evaluation for the
/// whole span it covers. See this module's header and
/// `reports/fable-2026-09-22.md` finding 2.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SvfCoeffs {
    pub g: f32,
    pub a1: f32,
    pub a2: f32,
    pub a3: f32,
    pub damping: f32,
}

impl SvfCoeffs {
    /// Derive the coefficient set for a cutoff/resonance/sample-rate triple.
    /// Exactly the math [`Svf::tick`] ran inline every sample before the
    /// split; a caller holding cutoff and resonance constant across many
    /// samples calls this once instead of paying the `tan()` per sample.
    pub fn for_cutoff(cutoff_hz: f32, resonance: f32, sample_rate: u32) -> Self {
        let sr = sample_rate as f32;
        let cutoff = clamp_param(cutoff_hz, 20.0, sr * 0.45);
        let g = (core::f32::consts::PI * cutoff / sr).tan();
        let damping = (2.0 - clamp_param(resonance, 0.0, 1.0) * 1.9).clamp(0.1, 2.0);
        let a1 = 1.0 / (1.0 + g * (g + damping));
        let a2 = g * a1;
        let a3 = g * a2;
        Self {
            g,
            a1,
            a2,
            a3,
            damping,
        }
    }

    /// Component-wise linear interpolation toward `other`, `t` clamped to
    /// `[0, 1]`. The SVF is stable under an interpolated coefficient set —
    /// that is what "topology-preserving" buys — so a caller sweeping
    /// across a block can ramp the four numbers linearly instead of
    /// re-deriving `tan()` every sample to stay well behaved.
    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        // `a * (1 - t) + b * t` rather than `a + (b - a) * t`: at `t = 0.0`
        // and `t = 1.0` this reduces to exactly `a` and exactly `b` (a `* 0.0`
        // term vanishes, a `* 1.0` term is untouched), which the interpolated
        // subtraction form is not guaranteed to under IEEE 754 rounding.
        let mix = |a: f32, b: f32| a * (1.0 - t) + b * t;
        Self {
            g: mix(self.g, other.g),
            a1: mix(self.a1, other.a1),
            a2: mix(self.a2, other.a2),
            a3: mix(self.a3, other.a3),
            damping: mix(self.damping, other.damping),
        }
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
        let cutoff = clamp_param(cutoff_hz, 10.0, sample_rate as f32 * 0.45);
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
        let cutoff = clamp_param(cutoff_hz, 10.0, sample_rate as f32 * 0.45);
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
        self.coeff = clamp_param(coeff, 0.0, 1.0);
    }

    pub fn reset(&mut self) {
        self.state = 0.0;
    }

    /// Whether the pole's stored sample has decayed below audibility.
    pub fn is_at_rest(&self) -> bool {
        self.state.abs() <= crate::node::REST_EPSILON
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
    apply_drive_compensated(input, drive, drive_compensation(drive))
}

/// The part of [`apply_drive`]'s response that depends on `drive` alone, not
/// on the sample it shapes: a `tanh` of the driven reference level.
///
/// A caller shaping many samples at one `drive` value -- a whole block, a
/// whole voice between parameter events -- computes this once and reuses it
/// through [`apply_drive_compensated`] rather than paying the `tanh` again
/// for every sample, exactly as `apply_drive` already does internally for a
/// single call.
pub fn drive_compensation(drive: f32) -> f32 {
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

/// [`apply_drive`] with [`drive_compensation`] already computed. The per-call
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
/// This is a bound, not a tone stage. [`Ladder`] and [`Acid`] cannot exceed 1
/// by construction, since their stages only ever integrate a shaper output, so
/// for them this never engages at all. [`Svf`] is linear and has no such
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
const VOICE_CEILING: f32 = 2.5;

/// A cascade of identical one-pole stages `y += g * (x - y)`: how many, and
/// where its -3 dB point sits relative to one stage's.
#[derive(Clone, Copy, Debug)]
struct Cascade {
    /// `sqrt(2^(1/n) - 1)`: an analog cascade of `n` one-poles at one corner
    /// is -3 dB at this fraction of it.
    spread: f32,
    /// `r / (1 - r)` with `r = 2^(-1/n)`, the power each stage passes at the
    /// cascade's -3 dB point.
    share: f32,
}

const FOUR_POLES: Cascade = Cascade {
    spread: 0.434_979_44,
    share: 5.285_213_5,
};

const THREE_POLES: Cascade = Cascade {
    spread: 0.509_824_5,
    share: 3.847_322,
};

impl Cascade {
    /// The per-stage coefficient that puts the whole cascade's -3 dB point
    /// exactly on `corner_hz`, at any sample rate.
    ///
    /// The ladders used to take `g = 1 - exp(-2 pi fc / sr)`, the
    /// impulse-invariant pole, which only approximates a one-pole's corner
    /// and does so worse the larger `fc / sr` is -- so a Ladder or Acid patch
    /// was a different sound at every rate (MOO-116). This solves the
    /// stages' own magnitude response instead: each stage has
    /// `|H(w)|^2 = g^2 / (1 - 2a cos w + a^2)` with `a = 1 - g`, and setting
    /// that to `r = 2^(-1/n)` at the corner gives `a^2 - 2ab + 1 = 0` with
    /// `b = 1 + e`, `e = r (1 - cos w) / (1 - r)`. The root inside the unit
    /// circle is the pole, so `g = sqrt(e (2 + e)) - e`. `1 - cos w` is
    /// computed as `2 sin^2(w / 2)`, so a 20 Hz corner at 192 kHz, where
    /// `cos w` rounds to one in `f32`, is still exact.
    ///
    /// For small `w` this is `w - w^2 / 2 + ...`, the same as the old form to
    /// second order, so below a few kHz at 48 kHz -- where both ladders are
    /// voiced -- the sound is unchanged; the difference is at the top of the
    /// knob and at the other rates. `g` stays below one for any corner below
    /// Nyquist, so every stage is stable.
    fn coeff(self, corner_hz: f32, sample_rate: f32) -> f32 {
        let half = (core::f32::consts::PI * corner_hz / sample_rate).sin();
        let e = self.share * 2.0 * half * half;
        (e * (2.0 + e)).sqrt() - e
    }

    /// The coefficient for a cutoff knob in Hz and the model's stage
    /// compensation: the stage corner `cutoff * compensation` is clamped to
    /// the primitives' `0.45 * sr` exactly as it always was, and the cascade
    /// is cornered at its [`Self::spread`] of that.
    fn coeff_for(self, cutoff_hz: f32, compensation: f32, sample_rate: f32) -> f32 {
        let stage = clamp_param(cutoff_hz * compensation, 20.0, sample_rate * 0.45);
        self.coeff(stage * self.spread, sample_rate)
    }
}

/// A nonlinear four-pole ladder low-pass: four cascaded one-pole stages with a
/// resonance feedback path from the last stage back to the input, saturated
/// inside the loop.
///
/// As a composable unit (`docs/COMPOSABLE_DEVICE_UNITS.md`):
///
/// ```text
/// Ladder
/// in:  audio, cutoff (Hz, 20..sr*0.45), resonance (0..1)
/// out: audio
/// ```
///
/// The audible difference from [`Svf`] is the slope — roughly 24 dB/oct
/// against the SVF's 12 — and that the resonance path is nonlinear, so
/// pushing level into it changes character rather than only gain. Circuit
/// accuracy is explicitly not a goal.
///
/// Stability comes for free from the shape rather than from a limiter: the
/// only thing the stages ever integrate is a `tanh` output, so every stage is
/// bounded by 1 no matter what resonance, cutoff, or input level do.
#[derive(Clone, Copy, Debug)]
pub struct Ladder {
    stage: [f32; 4],
    /// Last output, delayed one sample, which is what the feedback path reads.
    feedback: f32,
}

/// Where each of the four stages is cornered, as a multiple of the cutoff.
///
/// Four one-poles at one corner are -3 dB at `sqrt(2^(1/4) - 1)`, 0.435, of it,
/// so this puts the ladder's own -3 dB point at 0.676x the cutoff. That is
/// deliberate company rather than an error: [`Svf`] at zero resonance (Q of a
/// half) is -3 dB at 0.644x, so the two sit within a tenth of an octave and the
/// Cutoff knob means much the same on either. (The constant is
/// `1 / sqrt(sqrt(2) - 1)`, the two-pole figure; until 2026-09-23 this comment
/// claimed it lined the ladder's corner up with the cutoff itself.) Since
/// MOO-116 the cascade is cornered exactly (see `Cascade::coeff`), so the
/// ratio holds at every sample rate.
const LADDER_POLE_COMPENSATION: f32 = 1.5538;

/// Feedback gain at full resonance, at a low corner frequency.
const LADDER_MAX_FEEDBACK: f32 = 4.3;

/// The feedback path is delayed a sample, and that delay costs more loop phase
/// the higher the corner sits -- so without this the filter self-oscillates
/// at 200 Hz and merely peaks at 2 kHz, and the Resonance knob means something
/// different at each end of the Cutoff knob. Scaling the feedback with the
/// stage coefficient puts the self-oscillation threshold at roughly the same
/// knob position across the range.
const LADDER_FEEDBACK_TRACKING: f32 = 1.5;

/// Classic ladders thin out as resonance rises, because the feedback path
/// cancels the bass along with everything else. Some of that is the character;
/// total bass loss is not, so a fraction of the input bypasses the
/// subtraction. Voiced by ear against the factory bank.
const LADDER_BASS_COMPENSATION: f32 = 0.5;

impl Ladder {
    pub fn new() -> Self {
        Self {
            stage: [0.0; 4],
            feedback: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.stage = [0.0; 4];
        self.feedback = 0.0;
    }

    /// The resonance feedback gain this filter would use at these settings.
    ///
    /// Published because how much level resonance costs is a function of it,
    /// and a host that wants to compensate should read the number rather than
    /// re-derive it from [`LADDER_MAX_FEEDBACK`] and get it wrong later.
    pub fn feedback_at(cutoff_hz: f32, resonance: f32, sample_rate: u32) -> f32 {
        let sr = sample_rate as f32;
        let g = FOUR_POLES.coeff_for(cutoff_hz, LADDER_POLE_COMPENSATION, sr);
        clamp_param(resonance, 0.0, 1.0) * LADDER_MAX_FEEDBACK * (1.0 + LADDER_FEEDBACK_TRACKING * g)
    }

    /// Process one sample. Safe to call with `cutoff_hz` moving every sample,
    /// which is what an envelope-swept filter does.
    pub fn next_sample(
        &mut self,
        input: f32,
        cutoff_hz: f32,
        resonance: f32,
        sample_rate: u32,
    ) -> f32 {
        let sr = sample_rate as f32;
        let g = FOUR_POLES.coeff_for(cutoff_hz, LADDER_POLE_COMPENSATION, sr);
        let k = clamp_param(resonance, 0.0, 1.0)
            * LADDER_MAX_FEEDBACK
            * (1.0 + LADDER_FEEDBACK_TRACKING * g);

        let driven = input * (1.0 + k * LADDER_BASS_COMPENSATION) - k * self.feedback;
        let shaped = driven.tanh();

        self.stage[0] += g * (shaped - self.stage[0]);
        self.stage[1] += g * (self.stage[0] - self.stage[1]);
        self.stage[2] += g * (self.stage[1] - self.stage[2]);
        self.stage[3] += g * (self.stage[2] - self.stage[3]);
        self.feedback = self.stage[3];
        self.stage[3]
    }
}

impl Default for Ladder {
    fn default() -> Self {
        Self::new()
    }
}

/// A nonlinear three-pole ladder with an asymmetric resonance path: the other
/// half of the ML-M1's filter, and a genuinely different circuit rather than
/// the same one with different constants.
///
/// ```text
/// Acid
/// in:  audio, cutoff (Hz, 20..sr*0.45), resonance (0..1)
/// out: audio
/// ```
///
/// Three things separate it from [`Ladder`], and they are the three things
/// that separate the instruments it is named after:
///
/// - **Three poles, not four.** Roughly 18 dB/oct. Less of the spectrum is
///   removed above the corner, so it reads as forward and nasal where the
///   ladder reads as heavy.
/// - **The saturation is asymmetric** ([`DriveCurve::Tape`]), so it generates
///   even harmonics as well as odd. That is most of why it sounds brighter at
///   the same settings rather than merely thinner.
/// - **Half the ladder's bass compensation.** The ladder feeds a generous part
///   of the input past the feedback subtraction to keep its low end; this one
///   feeds half as much ([`ACID_BASS_COMPENSATION`]), which is what lets
///   resonance squeeze the body out of a note and squelch without the
///   Resonance knob spending its first half as a volume control.
///
/// Bounded for the same structural reason as the ladder: the stages only ever
/// integrate a shaper output, and every shaper in `shaper::shape` is bounded.
#[derive(Clone, Copy, Debug)]
pub struct Acid {
    stage: [f32; 3],
    feedback: f32,
}

/// Three cascaded one-poles reach -3 dB at `1 / sqrt(2^(1/3) - 1)` below a
/// single pole's corner. Same job as [`LADDER_POLE_COMPENSATION`]: make the
/// Cutoff knob mean one frequency across every model.
///
/// It does not currently achieve that, and the gap is deliberate for now.
/// Measured, this puts Acid's corner at 0.41x the knob's value while [`Ladder`]
/// sits at 0.68x and [`Svf`] at 0.65x -- about three quarters of an octave
/// darker at the same setting. Correcting it to 1.307 lines all three up, but
/// Acid's feedback, bass compensation and Tape shaper are all voiced against
/// this low corner: with the corner moved, the resonance taper goes
/// non-monotonic and its range collapses from 12 dB to under 3, and no value
/// of [`ACID_MAX_FEEDBACK`] recovers it. Lining the corners up means
/// re-deriving the filter, not retuning a constant. Recorded in
/// `docs/plans/archive/mono-synth-v2/00-status.md`.
const ACID_POLE_COMPENSATION: f32 = 0.8;

/// Half the ladder's, so the low end still thins as resonance rises -- that is
/// the squelch -- without the Resonance knob spending its first half just
/// making the patch quieter.
const ACID_BASS_COMPENSATION: f32 = 0.25;

/// Feedback gain at full resonance. Higher than the ladder's because three
/// poles reach 180 degrees of phase further above the corner, where each pole
/// has already taken more out of the loop.
const ACID_MAX_FEEDBACK: f32 = 12.0;

/// Same correction as [`LADDER_FEEDBACK_TRACKING`], for the same reason.
const ACID_FEEDBACK_TRACKING: f32 = 1.5;

impl Acid {
    pub fn new() -> Self {
        Self {
            stage: [0.0; 3],
            feedback: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.stage = [0.0; 3];
        self.feedback = 0.0;
    }

    /// The resonance feedback gain this filter would use at these settings.
    /// See [`Ladder::feedback_at`].
    pub fn feedback_at(cutoff_hz: f32, resonance: f32, sample_rate: u32) -> f32 {
        let sr = sample_rate as f32;
        let g = THREE_POLES.coeff_for(cutoff_hz, ACID_POLE_COMPENSATION, sr);
        clamp_param(resonance, 0.0, 1.0) * ACID_MAX_FEEDBACK * (1.0 + ACID_FEEDBACK_TRACKING * g)
    }

    pub fn next_sample(
        &mut self,
        input: f32,
        cutoff_hz: f32,
        resonance: f32,
        sample_rate: u32,
    ) -> f32 {
        let sr = sample_rate as f32;
        let g = THREE_POLES.coeff_for(cutoff_hz, ACID_POLE_COMPENSATION, sr);
        let k = clamp_param(resonance, 0.0, 1.0) * ACID_MAX_FEEDBACK * (1.0 + ACID_FEEDBACK_TRACKING * g);

        let driven = input * (1.0 + k * ACID_BASS_COMPENSATION) - k * self.feedback;
        let shaped = crate::shaper::shape(DriveCurve::Tape, driven);

        self.stage[0] += g * (shaped - self.stage[0]);
        self.stage[1] += g * (self.stage[0] - self.stage[1]);
        self.stage[2] += g * (self.stage[1] - self.stage[2]);
        self.feedback = self.stage[2];
        self.stage[2]
    }
}

impl Default for Acid {
    fn default() -> Self {
        Self::new()
    }
}

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
    use crate::testkit::{all_finite, db, rms, sine, Probe, RATES};

    const SR: u32 = 48_000;

    /// Steady-state output peak over the input, in dB, through the kit's
    /// small-signal probe. Peak rather than the tone at the probe frequency,
    /// because a ladder near self-oscillation rings at its own frequency and
    /// the peak is what that sounds like. Small-signal because slope and
    /// corner are small-signal properties: at full scale this would measure
    /// the `tanh` in the feedback path instead -- which is real character,
    /// but not what "24 dB/oct" describes.
    fn ladder_response_db(cutoff: f32, resonance: f32, freq: f32) -> f32 {
        let mut ladder = Ladder::new();
        Probe::new(SR).peak_gain_db(|x| ladder.next_sample(x, cutoff, resonance, SR), freq)
    }

    fn acid_response_db(cutoff: f32, resonance: f32, freq: f32) -> f32 {
        let mut acid = Acid::new();
        Probe::new(SR).peak_gain_db(|x| acid.next_sample(x, cutoff, resonance, SR), freq)
    }

    fn svf_response_db(cutoff: f32, resonance: f32, freq: f32) -> f32 {
        let mut svf = Svf::new();
        Probe::new(SR).peak_gain_db(|x| svf.next_sample(x, cutoff, resonance, SR), freq)
    }

    /// **The SVF against its own math, at every rate.** The TPT state-variable
    /// filter is the bilinear transform of the analog prototype
    /// `1 / (s^2 + d s + 1)`, prewarped so the corner lands on the cutoff: at a
    /// probe frequency `f` the prototype sees `w = tan(pi f / sr) / g`, with
    /// `g` and `d` the coefficient set's own. So the low-, band- and high-pass
    /// magnitudes are `1`, `w` and `w^2` over `sqrt((1 - w^2)^2 + (d w)^2)`, and
    /// a sine through the filter has to measure exactly that -- at every
    /// cutoff, every resonance, and every rate. Until 2026-09-22 nothing
    /// checked the SVF's absolute response at all (MOO-117).
    #[test]
    fn the_svf_is_its_transfer_function_at_every_rate() {
        for sr in RATES {
            let probe = Probe::new(sr);
            for cutoff in [100.0_f32, 1_000.0, 5_000.0, 15_000.0] {
                for resonance in [0.0_f32, 0.5, 0.9, 1.0] {
                    let coeffs = SvfCoeffs::for_cutoff(cutoff, resonance, sr);
                    for ratio in [0.25_f32, 0.5, 0.9, 1.0, 1.1, 2.0, 4.0] {
                        let freq = cutoff * ratio;
                        if freq > sr as f32 * 0.45 {
                            continue;
                        }
                        let w = ((core::f64::consts::PI * freq as f64 / sr as f64).tan()
                            / coeffs.g as f64) as f32;
                        let denominator =
                            ((1.0 - w * w).powi(2) + (coeffs.damping * w).powi(2)).sqrt();
                        let math = [1.0 / denominator, w / denominator, w * w / denominator];
                        for (output, expected) in math.iter().enumerate() {
                            let expected = db(*expected);
                            if expected < -60.0 {
                                continue;
                            }
                            let mut svf = Svf::new();
                            let measured = probe.gain_db(
                                |x| {
                                    let (low, band, high) =
                                        svf.next_sample_lp_bp_hp(x, cutoff, resonance, sr);
                                    [low, band, high][output]
                                },
                                freq,
                            );
                            assert!(
                                (measured - expected).abs() < 0.05,
                                "{sr} Hz, cutoff {cutoff}, resonance {resonance}, output \
                                 {output} at {freq} Hz: measured {measured:.3} dB, math \
                                 {expected:.3} dB"
                            );
                        }
                    }
                }
            }
        }
    }

    /// **The corners hold still across sample rates (MOO-116).** One cutoff
    /// in Hz puts the -3 dB point of `Svf`, `Ladder` and `Acid` in the same
    /// place at 44.1, 48, 96 and 192 kHz, to within five cents of where it
    /// sits at 48 kHz, and within ten of where it is stated to be. Until MOO-116 the ladders took their stage coefficient
    /// from the impulse-invariant `1 - exp(-w)`, and a 5 kHz Ladder cornered
    /// in a different place at every rate.
    ///
    /// Each filter's corner is also held to where it is stated to be. The
    /// SVF's is the cutoff itself, where the bilinear prewarp puts its
    /// prototype's corner and where at zero resonance (Q of a half) it is
    /// -6.02 dB; its -3 dB point is *not* rate-independent near the top,
    /// because the bilinear transform compresses everything but the prewarp
    /// frequency. The ladders' corner is their -3 dB point, at 0.676x the
    /// cutoff for the Ladder (`LADDER_POLE_COMPENSATION`) and 0.510x of 0.8x
    /// for Acid (`ACID_POLE_COMPENSATION`, deliberately dark). Zero resonance,
    /// small signal: a corner is a property of the linear filter.
    #[test]
    fn corners_are_one_frequency_at_every_rate() {
        const TOLERANCE_CENTS: f32 = 5.0;
        // Where each corner is stated to be, a little looser: Acid runs its
        // input through the asymmetric Tape curve even at zero resonance, and
        // its curvature at the probe level moves the measured corner by about
        // six cents -- the same at every rate, which is the check that matters.
        const STATED_TOLERANCE_CENTS: f32 = 10.0;
        // (model, dB below the passband at the corner, corner over cutoff)
        let models: [(&str, f32, f32); 3] = [
            ("Svf", 6.0206, 1.0),
            ("Ladder", 3.0103, LADDER_POLE_COMPENSATION * FOUR_POLES.spread),
            ("Acid", 3.0103, ACID_POLE_COMPENSATION * THREE_POLES.spread),
        ];
        // A fresh filter for every measurement, so none hears the last one's
        // state.
        fn fresh(model: &str, cutoff: f32, sr: u32) -> Box<dyn FnMut(f32) -> f32> {
            match model {
                "Svf" => {
                    let mut f = Svf::new();
                    Box::new(move |x| f.next_sample(x, cutoff, 0.0, sr))
                }
                "Ladder" => {
                    let mut f = Ladder::new();
                    Box::new(move |x| f.next_sample(x, cutoff, 0.0, sr))
                }
                _ => {
                    let mut f = Acid::new();
                    Box::new(move |x| f.next_sample(x, cutoff, 0.0, sr))
                }
            }
        }
        let gain_db = |model: &str, cutoff: f32, sr: u32, freq: f32| -> f32 {
            Probe::new(sr).gain_db(fresh(model, cutoff, sr), freq)
        };
        // The passband is the DC gain: a probe tone any distance below a
        // low-pass corner is already a fraction of a dB down, which moves a
        // crossing measured from it by tens of cents.
        let dc_gain_db = |model: &str, cutoff: f32, sr: u32| -> f32 {
            let mut filter = fresh(model, cutoff, sr);
            let level = Probe::new(sr).amplitude;
            let mut out = 0.0;
            for _ in 0..sr {
                out = filter(level);
            }
            crate::testkit::db(out / level)
        };
        for (model, depth, ratio) in models {
            for cutoff in [200.0_f32, 1_000.0, 5_000.0] {
                let low = cutoff * ratio / 8.0;
                let mut at_48k = None;
                for sr in [48_000].into_iter().chain(RATES.into_iter().filter(|&r| r != 48_000)) {
                    let passband = dc_gain_db(model, cutoff, sr);
                    let corner = crate::testkit::crossing_hz(
                        |f| gain_db(model, cutoff, sr, f),
                        passband - depth,
                        low,
                        cutoff * ratio * 4.0,
                    );
                    let reference = *at_48k.get_or_insert(corner);
                    assert!(
                        crate::testkit::cents(corner, reference).abs() < TOLERANCE_CENTS,
                        "{model}, cutoff {cutoff} Hz: -3 dB at {corner:.1} Hz at {sr} Hz, \
                         {reference:.1} Hz at 48 kHz"
                    );
                    assert!(
                        crate::testkit::cents(corner, cutoff * ratio).abs() < STATED_TOLERANCE_CENTS,
                        "{model}, cutoff {cutoff} Hz at {sr} Hz: -3 dB at {corner:.1} Hz, \
                         stated {:.1} Hz",
                        cutoff * ratio
                    );
                }
            }
        }
    }

    /// The cascade constants are the formulas their doc comments give.
    #[test]
    fn cascade_constants_are_their_formulas() {
        for (cascade, stages) in [(FOUR_POLES, 4.0_f64), (THREE_POLES, 3.0)] {
            let r = 2.0_f64.powf(-1.0 / stages);
            let spread = (2.0_f64.powf(1.0 / stages) - 1.0).sqrt();
            assert!((cascade.spread as f64 - spread).abs() < 1.0e-6);
            assert!((cascade.share as f64 - r / (1.0 - r)).abs() < 1.0e-5);
        }
    }

    /// **NaN hygiene (MOO-117; `reports/teams-2026-09-22.md` F5).** A NaN or
    /// infinite cutoff, resonance or drive -- from a broken automation lane, a
    /// bad modulation sum -- must not reach a filter's coefficients:
    /// `f32::clamp` passes NaN through, and one NaN in a filter's state makes
    /// every later sample NaN. Every filter here is held to finite output on
    /// finite input with its parameters non-finite, at every rate.
    #[test]
    fn a_non_finite_parameter_never_poisons_a_filter() {
        for sr in RATES {
            let input = sine(440.0, 0.5, sr, 2_048);
            for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let mut out = Vec::new();
                let (mut svf, mut ladder, mut acid) = (Svf::new(), Ladder::new(), Acid::new());
                let (mut lp, mut hp, mut pre) = (OnePoleLp::new(), OnePoleHp::new(), PreDrive::new());
                lp.set_cutoff(value, sr);
                hp.set_cutoff(value, sr);
                for &x in &input {
                    out.push(svf.next_sample(x, value, 0.5, sr));
                    out.push(svf.next_sample(x, 1_000.0, value, sr));
                    out.push(ladder.next_sample(x, value, value, sr));
                    out.push(acid.next_sample(x, value, value, sr));
                    out.push(lp.next_sample(x));
                    out.push(hp.next_sample(x));
                    out.push(apply_drive(x, value));
                    out.push(pre.next_sample(x, value, sr));
                }
                assert!(all_finite(&out), "{sr} Hz: a parameter of {value} poisoned a filter");
            }
        }
    }

    /// A NaN that does get into a filter's *audio* path is not something a
    /// filter can repair -- the state is the signal -- but it must not be put
    /// to sleep broken: a poisoned stage is never at rest (a NaN fails every
    /// comparison, which is the honest answer here), and `reset` clears it.
    #[test]
    fn a_poisoned_filter_is_never_at_rest_and_reset_clears_it() {
        for sr in RATES {
            let mut svf = Svf::new();
            let mut lp = OnePoleLp::new();
            lp.set_cutoff(1_000.0, sr);
            svf.next_sample(f32::NAN, 1_000.0, 0.5, sr);
            lp.next_sample(f32::NAN);
            assert!(!svf.is_at_rest() && !lp.is_at_rest());
            svf.reset();
            lp.reset();
            assert!(svf.next_sample(0.5, 1_000.0, 0.5, sr).is_finite());
            assert!(lp.next_sample(0.5).is_finite());
        }
    }

    /// The whole reason the ladder exists: it is a four-pole where `Svf` is a
    /// two-pole. Measured two octaves above the corner, where the asymptote
    /// has been reached but the naive one-pole's flattening near Nyquist has
    /// not yet set in, and with a loose tolerance because this is a nonlinear
    /// filter and the number will not be exact.
    #[test]
    fn the_ladder_rolls_off_twice_as_steeply_as_the_svf() {
        let cutoff = 500.0;
        let fall = |at: &dyn Fn(f32, f32, f32) -> f32| {
            at(cutoff, 0.0, cutoff * 2.0) - at(cutoff, 0.0, cutoff * 8.0)
        };
        let ladder = fall(&ladder_response_db);
        let svf = fall(&svf_response_db);

        // Two octaves, so an ideal 24 dB/oct is 48 dB and 12 dB/oct is 24.
        assert!(
            (34.0..50.0).contains(&ladder),
            "ladder fell {ladder:.1} dB over two octaves, wanted roughly 48"
        );
        assert!(
            ladder > svf * 1.6,
            "ladder {ladder:.1} dB vs svf {svf:.1} dB is not a slope difference"
        );
    }

    /// The Cutoff knob has to mean the same frequency on both filters, or a
    /// patch would change pitch-of-tone when the filter behind it changes.
    /// Both read about -6 dB at the knob's frequency at zero resonance.
    #[test]
    fn the_ladder_corner_matches_the_svf_corner() {
        for cutoff in [200.0_f32, 1000.0, 4000.0] {
            let ladder = ladder_response_db(cutoff, 0.0, cutoff / 8.0)
                - ladder_response_db(cutoff, 0.0, cutoff);
            let svf =
                svf_response_db(cutoff, 0.0, cutoff / 8.0) - svf_response_db(cutoff, 0.0, cutoff);
            assert!(
                (ladder - svf).abs() < 1.5,
                "at {cutoff} Hz the ladder is {ladder:.1} dB down and the svf {svf:.1} dB"
            );
        }
    }

    /// The tallest point of the response, wherever it sits -- the resonant
    /// peak moves with resonance, so pinning the probe to the corner would
    /// measure the peak sliding off it rather than the peak growing.
    fn resonant_peak_db(
        at: &dyn Fn(f32, f32, f32) -> f32,
        cutoff: f32,
        resonance: f32,
    ) -> f32 {
        let passband = at(cutoff, 0.0, cutoff / 16.0);
        let mut best = f32::MIN;
        for step in 0..24 {
            let mult = 2.0_f32.powf(-1.0 + step as f32 * 2.0 / 23.0);
            best = best.max(at(cutoff, resonance, cutoff * mult) - passband);
        }
        best
    }

    /// Both character models have to climb smoothly to self-oscillation and
    /// mean the same thing wherever the Cutoff knob is, so that Resonance is
    /// one control rather than two that share a label.
    fn assert_resonance_taper(name: &str, at: &dyn Fn(f32, f32, f32) -> f32) {
        for cutoff in [100.0_f32, 500.0, 2000.0] {
            let peaks: Vec<f32> = [0.0, 0.3, 0.5, 0.7, 0.85, 1.0]
                .iter()
                .map(|reso| resonant_peak_db(at, cutoff, *reso))
                .collect();
            assert!(
                peaks.windows(2).all(|pair| pair[1] > pair[0]),
                "{name} at {cutoff} Hz has a non-monotonic resonance taper: {peaks:?}"
            );
            assert!(
                peaks[5] - peaks[0] > 14.0,
                "{name} at {cutoff} Hz covers only {:.1} dB across the knob: {peaks:?}",
                peaks[5] - peaks[0]
            );
            assert!(peaks.iter().all(|peak| peak.is_finite()));
        }
    }

    /// Resonance has to climb smoothly to self-oscillation and mean the same
    /// thing wherever the Cutoff knob is -- the second half is what
    /// `LADDER_FEEDBACK_TRACKING` exists for.
    #[test]
    fn ladder_resonance_climbs_smoothly_across_the_cutoff_range() {
        assert_resonance_taper("ladder", &ladder_response_db);
    }

    #[test]
    fn acid_resonance_climbs_smoothly_across_the_cutoff_range() {
        assert_resonance_taper("acid", &acid_response_db);
    }

    /// Three poles, so it sits between the SVF's two and the ladder's four.
    /// That is most of why it reads forward and nasal where the ladder reads
    /// heavy: it simply removes less of the spectrum above the corner.
    #[test]
    fn the_acid_slope_sits_between_the_svf_and_the_ladder() {
        let cutoff = 500.0;
        let fall = |at: &dyn Fn(f32, f32, f32) -> f32| {
            at(cutoff, 0.0, cutoff * 2.0) - at(cutoff, 0.0, cutoff * 8.0)
        };
        let svf = fall(&svf_response_db);
        let acid = fall(&acid_response_db);
        let ladder = fall(&ladder_response_db);
        assert!(
            svf < acid && acid < ladder,
            "wanted svf < acid < ladder, got {svf:.1} / {acid:.1} / {ladder:.1} dB"
        );
    }

    /// The two character models have to be different instruments' worth of
    /// filter at identical settings, not the same one with different numbers.
    #[test]
    fn the_ladder_and_the_acid_are_audibly_distinct() {
        let cutoff = 800.0;
        let mut widest = 0.0_f32;
        for reso in [0.2_f32, 0.6, 0.9] {
            for mult in [0.5_f32, 1.0, 2.0, 4.0] {
                let ladder = ladder_response_db(cutoff, reso, cutoff * mult);
                let acid = acid_response_db(cutoff, reso, cutoff * mult);
                widest = widest.max((ladder - acid).abs());
            }
        }
        // "Not bit-identical" is not the bar; this wants a difference a
        // listener would call a different filter.
        assert!(
            widest > 8.0,
            "the two models never differ by more than {widest:.1} dB"
        );
    }

    #[test]
    fn the_acid_stays_bounded_under_a_swept_resonant_sweep() {
        let mut acid = Acid::new();
        let mut peak = 0.0_f32;
        for i in 0..SR as usize {
            let t = i as f32 / SR as f32;
            let input = (t * 110.0 * core::f32::consts::TAU).sin() * 4.0;
            let cutoff = 200.0 + 6000.0 * (t * 40.0).sin().abs();
            let out = acid.next_sample(input, cutoff, 1.0, SR);
            peak = peak.max(out.abs());
        }
        assert!(peak.is_finite(), "acid went non-finite");
        assert!(peak <= 1.0, "acid peaked at {peak}");
    }

    /// The stages only ever integrate a `tanh` output, so this holds for any
    /// input at any setting -- including a cutoff swept every sample.
    #[test]
    fn the_ladder_stays_bounded_under_a_swept_resonant_sweep() {
        let mut ladder = Ladder::new();
        let mut peak = 0.0_f32;
        for i in 0..SR as usize {
            let t = i as f32 / SR as f32;
            let input = (t * 110.0 * core::f32::consts::TAU).sin() * 4.0;
            let cutoff = 200.0 + 6000.0 * (t * 40.0).sin().abs();
            let out = ladder.next_sample(input, cutoff, 1.0, SR);
            peak = peak.max(out.abs());
        }
        assert!(peak.is_finite(), "ladder went non-finite");
        assert!(peak <= 1.0, "ladder peaked at {peak}");
    }

    /// The pre-drive's contract, and the reason it does not reuse
    /// `apply_drive`'s fixed anchor: the knob is a character control at
    /// whatever level the oscillator mix happens to sit at. Measured as RMS,
    /// because that is what the stage matches and what loudness follows.
    #[test]
    fn pre_drive_holds_its_level_across_the_knob_at_any_input_level() {
        for level in [0.05_f32, 0.25, 1.0, 2.5] {
            let levels: Vec<f32> = [0.0_f32, 0.5, 1.0]
                .iter()
                .map(|drive| {
                    let mut stage = PreDrive::new();
                    let probe = Probe::new(SR).amplitude(level);
                    rms(&probe.run(|x| stage.next_sample(x, *drive, SR), 220.0))
                })
                .collect();
            let quietest = levels.iter().cloned().fold(f32::MAX, f32::min);
            let loudest = levels.iter().cloned().fold(0.0_f32, f32::max);
            let spread_db = 20.0 * (loudest / quietest).log10();
            assert!(
                spread_db < 1.0,
                "at input level {level} the drive knob moved the level {spread_db:.1} dB"
            );
        }
    }

    /// The other half: pushing more signal in changes the shape, which is what
    /// makes the oscillator mixer a tone control.
    #[test]
    fn pre_drive_gets_dirtier_as_the_input_grows() {
        fn harmonic_ratio(level: f32) -> f32 {
            let mut stage = PreDrive::new();
            let mut fundamental = 0.0_f32;
            let mut total = 0.0_f32;
            let frames = SR as usize / 4;
            for i in 0..frames {
                let phase = core::f32::consts::TAU * 220.0 * i as f32 / SR as f32;
                let out = stage.next_sample(phase.sin() * level, 1.0, SR);
                if i > frames / 2 {
                    fundamental += out * phase.sin();
                    total += out * out;
                }
            }
            // Energy not explained by the fundamental, relative to the whole.
            let frames = (frames / 2) as f32;
            let correlated = 2.0 * fundamental / frames;
            (total / frames - correlated * correlated / 2.0).max(0.0) / (total / frames)
        }

        let quiet = harmonic_ratio(0.1);
        let loud = harmonic_ratio(2.0);
        assert!(
            loud > quiet * 1.5,
            "input level did not change the harmonic content: {quiet:.4} -> {loud:.4}"
        );
    }

    /// The ceiling is a bound, not a tone stage: it has to be exactly
    /// transparent everywhere a real patch lives, and it has to hold whatever
    /// a linear filter at self-resonance throws at it.
    #[test]
    fn the_voice_ceiling_is_transparent_until_it_is_needed() {
        for sample in [0.0_f32, 0.25, -0.7, 1.0, -1.4999] {
            assert_eq!(soft_ceiling(sample), sample, "bent a normal level");
        }
        for sample in [2.0_f32, -4.5, 40.0, -1000.0] {
            let bounded = soft_ceiling(sample);
            assert!(bounded.abs() <= VOICE_CEILING, "{sample} escaped to {bounded}");
            assert_eq!(bounded.signum(), sample.signum());
        }
        // That the bound is low enough for a voice to stay under full scale
        // once the output reference is applied is asserted where the reference
        // lives, by `mlm1::tests::resonant_filter_and_drive_stay_bounded`.
    }

    #[test]
    fn pre_drive_at_zero_is_a_pass_through() {
        let mut stage = PreDrive::new();
        for sample in [0.0_f32, 0.3, -0.9, 2.0] {
            assert_eq!(stage.next_sample(sample, 0.0, SR), sample);
        }
    }

    /// `Svf::tick`'s wrapper (via `next_sample_lp_bp_hp`) has to produce the
    /// same numbers as calling `SvfCoeffs::for_cutoff` and `tick_with`
    /// directly -- the whole point of keeping it a thin wrapper is that the
    /// 86 existing call sites see no change at all.
    #[test]
    fn tick_with_matches_the_wrapper_bit_for_bit() {
        let sr = 48_000;
        let cutoff = 1_234.5_f32;
        let resonance = 0.6_f32;

        let mut via_wrapper = Svf::new();
        let mut via_coeffs = Svf::new();
        let coeffs = SvfCoeffs::for_cutoff(cutoff, resonance, sr);

        for i in 0..256 {
            let input = (i as f32 * 0.037).sin();
            let wrapped = via_wrapper.next_sample_lp_bp_hp(input, cutoff, resonance, sr);
            let direct = via_coeffs.tick_with(input, &coeffs);
            assert_eq!(wrapped, direct, "sample {i} diverged");
        }
    }

    /// `t = 0` and `t = 1` are the endpoints exactly; the midpoint is the
    /// arithmetic mean of each field.
    #[test]
    fn svf_coeffs_lerp_hits_its_endpoints_and_midpoint() {
        let sr = 48_000;
        let a = SvfCoeffs::for_cutoff(200.0, 0.0, sr);
        let b = SvfCoeffs::for_cutoff(8_000.0, 0.9, sr);

        assert_eq!(a.lerp(&b, 0.0), a);
        assert_eq!(a.lerp(&b, 1.0), b);

        let mid = a.lerp(&b, 0.5);
        assert!((mid.g - (a.g + b.g) / 2.0).abs() < 1.0e-6);
        assert!((mid.a1 - (a.a1 + b.a1) / 2.0).abs() < 1.0e-6);
        assert!((mid.a2 - (a.a2 + b.a2) / 2.0).abs() < 1.0e-6);
        assert!((mid.a3 - (a.a3 + b.a3) / 2.0).abs() < 1.0e-6);
        assert!((mid.damping - (a.damping + b.damping) / 2.0).abs() < 1.0e-6);
    }

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
    fn drive_bypasses_at_zero() {
        assert_eq!(apply_drive(0.25, 0.0), 0.25);
    }

    /// The whole point of the compensation: sweeping drive at a
    /// reference-level input keeps the peak where it was while harmonic
    /// content grows. A drive control changes character, not level.
    #[test]
    fn drive_changes_character_not_level_at_the_reference() {
        const FREQ: f32 = 100.0;
        const REFERENCE: f32 = DRIVE_REFERENCE_LINEAR;

        for sr in RATES {
            let input = sine(FREQ, REFERENCE, sr, sr as usize / 10);
            let mut previous_thd = 0.0f32;
            for drive in [0.2f32, 0.5, 0.9] {
                let out: Vec<f32> = input.iter().map(|&x| apply_drive(x, drive)).collect();

                let peak = crate::testkit::peak(&out);
                assert!(
                    (peak - REFERENCE).abs() < REFERENCE * 0.02,
                    "{sr} Hz: drive {drive} moved the peak to {peak}"
                );
                let thd = crate::testkit::thd(&out, sr, FREQ);
                assert!(
                    thd > previous_thd,
                    "{sr} Hz: harmonic content did not grow with drive: {drive} -> {thd}"
                );
                previous_thd = thd;
            }
        }
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
