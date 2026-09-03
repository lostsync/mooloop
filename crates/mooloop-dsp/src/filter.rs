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
//! `tan()`. `docs/plans/archive/share-dsp-primitives/03-collapse-duplicate-implementations.md`
//! asked for exactly this measurement before converting; this is the
//! result. New instruments (mono, not stereo-per-voice) use `Svf` directly,
//! where the doubling doesn't apply.

use mooloop_core::DriveCurve;

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

/// Signal level (linear) the drive compensation anchors to: the operating
/// level, `10^(REFERENCE_PEAK_DBFS/20)`. Written as a literal because this
/// runs per sample.
const DRIVE_REFERENCE_LINEAR: f32 = 0.251;

/// Compensated soft saturation shared by the sampler, drum synth, both
/// synths, and the filter effect: pre-gain into `tanh`, normalized by the
/// shaper's own response to a reference-level signal, so raising drive
/// changes character, not level. A static nonlinearity cannot be level-flat
/// at every input; anchoring at the operating level
/// (`mooloop_core::gain::REFERENCE_PEAK_DBFS`) is the compromise, and it
/// also caps a full-scale peak at the reference rather than at clipping.
pub fn apply_drive(input: f32, drive: f32) -> f32 {
    let drive = drive.clamp(0.0, 1.0);
    if drive <= f32::EPSILON {
        return input;
    }
    let input_gain = 1.0 + drive * 15.0;
    let compensation =
        DRIVE_REFERENCE_LINEAR / (DRIVE_REFERENCE_LINEAR * input_gain).tanh();
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

/// Four cascaded one-poles reach -3 dB well below a single pole's corner --
/// at `sqrt(sqrt(2) - 1)` of it -- so the per-stage corner is pushed up by the
/// inverse to make the Cutoff knob mean the same frequency it does on `Svf`.
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
        let cutoff = (cutoff_hz * LADDER_POLE_COMPENSATION).clamp(20.0, sr * 0.45);
        let g = 1.0 - (-core::f32::consts::TAU * cutoff / sr).exp();
        resonance.clamp(0.0, 1.0) * LADDER_MAX_FEEDBACK * (1.0 + LADDER_FEEDBACK_TRACKING * g)
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
        let cutoff = (cutoff_hz * LADDER_POLE_COMPENSATION).clamp(20.0, sr * 0.45);
        let g = 1.0 - (-core::f32::consts::TAU * cutoff / sr).exp();
        let k = resonance.clamp(0.0, 1.0)
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
/// `docs/plans/mono-synth-v2/00-status.md`.
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
        let cutoff = (cutoff_hz * ACID_POLE_COMPENSATION).clamp(20.0, sr * 0.45);
        let g = 1.0 - (-core::f32::consts::TAU * cutoff / sr).exp();
        resonance.clamp(0.0, 1.0) * ACID_MAX_FEEDBACK * (1.0 + ACID_FEEDBACK_TRACKING * g)
    }

    pub fn next_sample(
        &mut self,
        input: f32,
        cutoff_hz: f32,
        resonance: f32,
        sample_rate: u32,
    ) -> f32 {
        let sr = sample_rate as f32;
        let cutoff = (cutoff_hz * ACID_POLE_COMPENSATION).clamp(20.0, sr * 0.45);
        let g = 1.0 - (-core::f32::consts::TAU * cutoff / sr).exp();
        let k = resonance.clamp(0.0, 1.0) * ACID_MAX_FEEDBACK * (1.0 + ACID_FEEDBACK_TRACKING * g);

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
        let drive = drive.clamp(0.0, 1.0);
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

    const SR: u32 = 48_000;

    /// Steady-state output peak for a sine of `amplitude` at `freq_hz`, in
    /// dBFS. Absolute, not normalized: the callers that want a transfer
    /// function take differences, and the one that wants a level reads it
    /// directly.
    fn response_db(filter: &mut dyn FnMut(f32) -> f32, freq_hz: f32, amplitude: f32) -> f32 {
        let mut peak = 0.0_f32;
        let total = SR as usize / 2;
        for i in 0..total {
            let t = i as f32 / SR as f32;
            let out = filter((t * freq_hz * core::f32::consts::TAU).sin() * amplitude);
            // Skip the transient and any follower's settling time.
            if i > total / 2 {
                peak = peak.max(out.abs());
            }
        }
        20.0 * peak.max(1.0e-9).log10()
    }

    /// Slope and corner frequency are small-signal properties. Measuring them
    /// at full scale would measure the `tanh` in the feedback path instead --
    /// which is real character, but not what "24 dB/oct" describes.
    const SMALL_SIGNAL: f32 = 0.02;

    fn ladder_response_db(cutoff: f32, resonance: f32, freq: f32) -> f32 {
        let mut ladder = Ladder::new();
        response_db(
            &mut |x| ladder.next_sample(x, cutoff, resonance, SR),
            freq,
            SMALL_SIGNAL,
        )
    }

    fn acid_response_db(cutoff: f32, resonance: f32, freq: f32) -> f32 {
        let mut acid = Acid::new();
        response_db(
            &mut |x| acid.next_sample(x, cutoff, resonance, SR),
            freq,
            SMALL_SIGNAL,
        )
    }

    fn svf_response_db(cutoff: f32, resonance: f32, freq: f32) -> f32 {
        let mut svf = Svf::new();
        response_db(
            &mut |x| svf.next_sample(x, cutoff, resonance, SR),
            freq,
            SMALL_SIGNAL,
        )
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

    /// Steady-state output RMS for a sine of `amplitude` at `freq_hz`.
    fn response_rms(filter: &mut dyn FnMut(f32) -> f32, freq_hz: f32, amplitude: f32) -> f32 {
        let total = SR as usize / 2;
        let mut sum = 0.0_f32;
        let mut counted = 0usize;
        for i in 0..total {
            let t = i as f32 / SR as f32;
            let out = filter((t * freq_hz * core::f32::consts::TAU).sin() * amplitude);
            if i > total / 2 {
                sum += out * out;
                counted += 1;
            }
        }
        (sum / counted.max(1) as f32).sqrt()
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
                    response_rms(&mut |x| stage.next_sample(x, *drive, SR), 220.0, level)
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
        const SR: f32 = 48_000.0;
        const FREQ: f32 = 100.0;
        const FRAMES: usize = 4_800;
        const REFERENCE: f32 = DRIVE_REFERENCE_LINEAR;

        let input: Vec<f32> = (0..FRAMES)
            .map(|i| (core::f32::consts::TAU * FREQ * i as f32 / SR).sin() * REFERENCE)
            .collect();

        let mut previous_harmonic_share = 0.0f32;
        for drive in [0.2f32, 0.5, 0.9] {
            let out: Vec<f32> = input.iter().map(|&x| apply_drive(x, drive)).collect();

            let peak = out.iter().fold(0.0f32, |p, s| p.max(s.abs()));
            assert!(
                (peak - REFERENCE).abs() < REFERENCE * 0.02,
                "drive {drive} moved the peak to {peak}"
            );

            // DFT bins report A/2 for a sine of amplitude A; true RMS is
            // amplitude/√2. Put both in amplitude terms before comparing.
            let total_rms = (out.iter().map(|s| s * s).sum::<f32>() / FRAMES as f32).sqrt();
            let fundamental = 2.0 * tone_energy(&out, SR, FREQ);
            let total_amplitude = core::f32::consts::SQRT_2 * total_rms;
            let harmonic = (total_amplitude * total_amplitude - fundamental * fundamental)
                .max(0.0)
                .sqrt();
            let harmonic_share = harmonic / fundamental;
            assert!(
                harmonic_share > previous_harmonic_share,
                "harmonic content did not grow with drive: {drive} -> {harmonic_share}"
            );
            previous_harmonic_share = harmonic_share;
        }
    }

    /// Single-bin DFT magnitude, normalized by length.
    fn tone_energy(samples: &[f32], sample_rate: f32, freq: f32) -> f32 {
        use core::f32::consts::TAU;
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (index, sample) in samples.iter().enumerate() {
            let phase = TAU * freq * index as f32 / sample_rate;
            re += sample * phase.cos();
            im -= sample * phase.sin();
        }
        (re * re + im * im).sqrt() / samples.len() as f32
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
