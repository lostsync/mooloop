//! Envelopes shared by the synth voices. The sampler keeps its own private
//! ADSR (parameterized directly from `SamplerParams`); these are the generic
//! versions new instruments use.
//!
//! The sampler's copy is not a performance decision (its `advance`/`configure`
//! are already scalar and cheap, character-identical to `Adsr`'s) — it's
//! `note_on` that genuinely differs. `Adsr::note_on` deliberately does *not*
//! reset `level` (see its doc comment: attacking from wherever the envelope
//! already is, so a legato retrigger over a synth voice's own release tail
//! doesn't click). The sampler's `trigger` always hard-resets — level,
//! filter state, and playback position together — because a triggered voice
//! restarts a *sample* from its start, including when `select_voice` steals
//! an already-sounding voice; that's a fresh onset by design, not a legato
//! retrigger. Swapping in `Adsr` as-is would silently change that: a stolen
//! voice would attack from its previous level instead of zero. Kept
//! separate for this reason, not measured cost — see `filter.rs`'s header
//! for the sampler decision that *was* about cost.
//!
//! All envelopes are sample-based, allocation-free, and cheap enough to run
//! per-sample on the realtime thread.

/// Minimum stage time, to avoid divide-by-zero and infinite rates.
const MIN_STAGE_S: f32 = 1.0e-4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AdsrStage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// A scalar linear-segment ADSR envelope. `advance` moves it one sample; the
/// caller reads `level` to shape amplitude or filter cutoff.
#[derive(Clone, Copy, Debug)]
pub struct Adsr {
    stage: AdsrStage,
    level: f32,
    attack_inc: f32,
    decay_dec: f32,
    sustain: f32,
    release_dec: f32,
    release_s: f32,
    sample_rate: u32,
}

impl Adsr {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            stage: AdsrStage::Idle,
            level: 0.0,
            attack_inc: 0.0,
            decay_dec: 0.0,
            sustain: 0.0,
            release_dec: 0.0,
            release_s: MIN_STAGE_S,
            sample_rate,
        }
    }

    /// Recompute segment rates from times in seconds. Call when parameters
    /// change; safe to call while the envelope runs.
    pub fn configure(&mut self, attack_s: f32, decay_s: f32, sustain: f32, release_s: f32) {
        let sr = self.sample_rate as f32;
        self.attack_inc = 1.0 / (attack_s.max(MIN_STAGE_S) * sr);
        self.decay_dec = (1.0 - sustain.clamp(0.0, 1.0)) / (decay_s.max(MIN_STAGE_S) * sr);
        self.sustain = sustain.clamp(0.0, 1.0);
        self.release_s = release_s.max(MIN_STAGE_S);
    }

    /// Start the attack stage. The level is deliberately *not* reset to zero:
    /// a note that arrives while the previous one is still sounding (a
    /// retrigger, or a new note over the tail of a release) would otherwise
    /// step the output straight to silence for one sample, which is an
    /// audible click. Attacking from wherever the envelope already is keeps
    /// the amplitude continuous; from idle the level is already zero, so a
    /// fresh note behaves exactly as before.
    pub fn note_on(&mut self) {
        self.stage = AdsrStage::Attack;
        self.level = self.level.clamp(0.0, 1.0);
    }

    /// Enter release from the current level.
    pub fn release(&mut self) {
        self.release_dec = self.level / (self.release_s * self.sample_rate as f32);
        self.stage = AdsrStage::Release;
    }

    /// Enter release with an explicit time (used for chokes).
    pub fn release_with(&mut self, seconds: f32) {
        self.release_dec = self.level / (seconds.max(MIN_STAGE_S) * self.sample_rate as f32);
        self.stage = AdsrStage::Release;
    }

    pub fn advance(&mut self) {
        match self.stage {
            AdsrStage::Idle => self.level = 0.0,
            AdsrStage::Attack => {
                self.level += self.attack_inc;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = AdsrStage::Decay;
                }
            }
            AdsrStage::Decay => {
                self.level -= self.decay_dec;
                if self.level <= self.sustain {
                    self.level = self.sustain;
                    self.stage = AdsrStage::Sustain;
                }
            }
            AdsrStage::Sustain => {}
            AdsrStage::Release => {
                self.level -= self.release_dec;
                if self.level <= 0.0 {
                    self.level = 0.0;
                    self.stage = AdsrStage::Idle;
                }
            }
        }
    }

    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn is_idle(&self) -> bool {
        self.stage == AdsrStage::Idle
    }

    pub fn is_releasing(&self) -> bool {
        self.stage == AdsrStage::Release
    }
}

/// Threshold below which an exponential decay is considered finished.
const EXP_IDLE_LEVEL: f32 = 1.0e-4;

/// Number of 1/e time constants for a level of 1.0 to fall below
/// `EXP_IDLE_LEVEL`: `ln(1 / EXP_IDLE_LEVEL)`. `ExpDecay::set_time` uses this
/// so its `seconds` argument means "time to become inaudible" (matching the
/// UI's decay-time knobs) rather than the raw 1/e time constant.
pub(crate) const DECAY_TAIL_CONSTANTS: f32 = 9.210_34;

/// A one-shot exponential decay envelope for percussive sounds. Triggers at
/// level 1 and falls toward zero with a musically natural curve.
#[derive(Clone, Copy, Debug)]
pub struct ExpDecay {
    level: f32,
    coeff: f32,
}

impl ExpDecay {
    pub fn new() -> Self {
        Self {
            level: 0.0,
            coeff: 1.0,
        }
    }

    /// Set the decay time in seconds (time for the level to fall below
    /// audibility, not the raw 1/e time constant).
    pub fn set_time(&mut self, seconds: f32, sample_rate: u32) {
        let tau = seconds.max(MIN_STAGE_S) / DECAY_TAIL_CONSTANTS;
        self.coeff = (-1.0 / (tau * sample_rate as f32)).exp();
    }

    /// Force an already-computed coefficient (used for fixed fast fades).
    pub fn set_coeff(&mut self, coeff: f32) {
        self.coeff = coeff.clamp(0.0, 1.0);
    }

    pub fn trigger(&mut self) {
        self.level = 1.0;
    }

    pub fn advance(&mut self) {
        self.level *= self.coeff;
        if self.level < EXP_IDLE_LEVEL {
            self.level = 0.0;
        }
    }

    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn is_idle(&self) -> bool {
        self.level <= 0.0
    }
}

impl Default for ExpDecay {
    fn default() -> Self {
        Self::new()
    }
}

/// `1 / (e^DECAY_TAIL_CONSTANTS - 1)`, which normalizes the exponential
/// shaping curve so it is exactly 0 at the bottom and exactly 1 at the top.
///
/// A literal because `exp` is not a `const fn`; `the_exponential_curve_is_normalized`
/// pins it to the constant it is derived from.
const EXP_SHAPE_NORM: f32 = 1.0 / 9_999.0;

/// Shape a normalized `0..1` envelope position by a bipolar curve control.
///
/// `-1` is logarithmic, `0` is exponential — the same law as [`ExpDecay`],
/// and therefore v1's — and `+1` is linear. `shape(0) == 0` and
/// `shape(1) == 1` at every curve, so it is a reparameterization of the
/// segment rather than a gain on it.
///
/// One shaping of the output rather than three different integrators: it is
/// then continuous across zero, and a hit latches one number instead of
/// choosing a code path.
pub fn shape(position: f32, curve: f32) -> f32 {
    let u = position.clamp(0.0, 1.0);
    // Exact at the ends rather than nearly exact. `EXP_SHAPE_NORM` is a
    // literal because `exp` is not a `const fn`, so it is a hair off, and an
    // envelope whose first sample was 0.9999992 would make "attack zero
    // costs the transient nothing" almost true instead of true.
    if u <= 0.0 {
        return 0.0;
    }
    if u >= 1.0 {
        return 1.0;
    }
    let exponential = ((DECAY_TAIL_CONSTANTS * u).exp() - 1.0) * EXP_SHAPE_NORM;
    let curve = curve.clamp(-1.0, 1.0);
    if curve >= 0.0 {
        exponential + (u - exponential) * curve
    } else {
        // The exponential curve reflected through the diagonal: a tail that
        // stays audible for most of its length and then stops, rather than
        // one that is 40 dB down at half time.
        let logarithmic =
            1.0 - ((DECAY_TAIL_CONSTANTS * (1.0 - u)).exp() - 1.0) * EXP_SHAPE_NORM;
        exponential + (logarithmic - exponential) * -curve
    }
}

/// Per-sample rate for a segment of `seconds`. `1.0` means "one sample or
/// less", which callers read as instant.
fn segment_rate(seconds: f32, sample_rate: u32) -> f32 {
    let samples = seconds.max(0.0) * sample_rate as f32;
    if samples <= 1.0 {
        1.0
    } else {
        1.0 / samples
    }
}

/// One [`Ahd`] envelope's shape, latched when it is triggered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AhdShape {
    /// Seconds to full level. `0` means the first sample is the peak, with no
    /// ramp and no smoothing.
    pub attack_s: f32,
    /// Seconds flat at the peak.
    pub hold_s: f32,
    pub decay_s: f32,
    /// See [`shape`].
    pub curve: f32,
    /// Level the decay falls to while the note is held. Only reached when
    /// [`Self::gate`] is set.
    pub sustain: f32,
    pub release_s: f32,
    /// Off is one-shot: [`Ahd::release`] does nothing and the envelope runs
    /// to silence on its own.
    pub gate: bool,
}

impl Default for AhdShape {
    fn default() -> Self {
        Self {
            attack_s: 0.0,
            hold_s: 0.0,
            decay_s: 0.25,
            curve: 0.0,
            sustain: 0.0,
            release_s: 0.1,
            gate: false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AhdStage {
    Idle,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
}

/// Attack-hold-decay with a curve, and an optional gate.
///
/// ```text
///           +---- hold ----+
///          /|              |\
///         / |              | \___ decay (curve)
///   attack                       \_____
/// ```
///
/// Built for percussion, where the interesting part of a sound is its first
/// few milliseconds, and where two properties an ADSR does not have both
/// matter: **a zero-length attack that costs the transient nothing**, and a
/// **hold**, which is the 909 clap tail and the gated snare. A segment of
/// zero length is skipped at the transition rather than costing a sample, so
/// an envelope with no attack and no hold is at its peak on sample zero.
///
/// [`Self::next`] returns this sample's level and then steps, so the first
/// value a caller reads is the one the trigger produced.
#[derive(Clone, Copy, Debug)]
pub struct Ahd {
    stage: AhdStage,
    level: f32,
    /// Position within the running segment, in `0..1`.
    phase: f32,
    held: u32,
    attack_rate: f32,
    hold_samples: u32,
    decay_rate: f32,
    release_rate: f32,
    curve: f32,
    sustain: f32,
    gate: bool,
    /// Level the release started from, so a release is a fade of whatever was
    /// sounding rather than a jump to a stage's own shape.
    release_from: f32,
}

impl Ahd {
    pub fn new() -> Self {
        Self {
            stage: AhdStage::Idle,
            level: 0.0,
            phase: 0.0,
            held: 0,
            attack_rate: 1.0,
            hold_samples: 0,
            decay_rate: 1.0,
            release_rate: 1.0,
            curve: 0.0,
            sustain: 0.0,
            gate: false,
            release_from: 0.0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Start the envelope, latching `shape` for the life of this run.
    pub fn trigger(&mut self, shape: AhdShape, sample_rate: u32) {
        self.curve = shape.curve.clamp(-1.0, 1.0);
        self.sustain = shape.sustain.clamp(0.0, 1.0);
        self.gate = shape.gate;
        self.attack_rate = segment_rate(shape.attack_s, sample_rate);
        self.hold_samples = (shape.hold_s.max(0.0) * sample_rate as f32) as u32;
        self.decay_rate = segment_rate(shape.decay_s, sample_rate);
        self.release_rate = segment_rate(shape.release_s, sample_rate);
        self.phase = 0.0;
        self.held = 0;
        if self.attack_rate >= 1.0 {
            self.start_hold();
        } else {
            self.stage = AhdStage::Attack;
            self.level = 0.0;
        }
    }

    /// This sample's level, then one step forward.
    ///
    /// Named `tick` rather than `next` because it is not an iterator: it
    /// never ends, and reading it while idle is the legitimate way to ask an
    /// envelope for silence.
    pub fn tick(&mut self) -> f32 {
        let out = self.level;
        self.step();
        out
    }

    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn is_idle(&self) -> bool {
        self.stage == AhdStage::Idle
    }

    /// Whether this envelope is waiting on a note-off. The only stage with no
    /// end of its own, and reachable only with the gate on.
    pub fn is_sustaining(&self) -> bool {
        self.stage == AhdStage::Sustain
    }

    pub fn is_gated(&self) -> bool {
        self.gate
    }

    /// Note-off. Ignored by a one-shot envelope, which is what makes a
    /// drum's note-off end nothing.
    pub fn release(&mut self) {
        if self.gate {
            self.begin_release();
        }
    }

    /// Fade out over `seconds` whatever the gate says. This is what a choke
    /// is: a release applied to the envelope, rather than a coefficient
    /// stamped over it, so the envelope's own shape does not have to
    /// special-case being cut short.
    pub fn release_over(&mut self, seconds: f32, sample_rate: u32) {
        if self.stage == AhdStage::Idle {
            return;
        }
        self.release_rate = segment_rate(seconds, sample_rate);
        self.begin_release();
    }

    fn begin_release(&mut self) {
        if self.stage == AhdStage::Idle {
            return;
        }
        self.release_from = self.level;
        self.phase = 0.0;
        self.stage = AhdStage::Release;
    }

    fn start_hold(&mut self) {
        self.held = 0;
        if self.hold_samples == 0 {
            self.start_decay();
        } else {
            self.stage = AhdStage::Hold;
            self.level = 1.0;
        }
    }

    fn start_decay(&mut self) {
        self.phase = 0.0;
        self.stage = AhdStage::Decay;
        self.level = self.decay_level(0.0);
    }

    /// The decay's output at `phase`. With the gate on it falls to
    /// [`Self::sustain`] instead of to zero, so one shaping serves both.
    fn decay_level(&self, phase: f32) -> f32 {
        let shaped = shape(1.0 - phase, self.curve);
        if self.gate {
            self.sustain + (1.0 - self.sustain) * shaped
        } else {
            shaped
        }
    }

    fn step(&mut self) {
        match self.stage {
            AhdStage::Idle => {}
            AhdStage::Attack => {
                self.phase += self.attack_rate;
                if self.phase >= 1.0 {
                    self.start_hold();
                } else {
                    self.level = shape(self.phase, self.curve);
                }
            }
            AhdStage::Hold => {
                self.held += 1;
                if self.held >= self.hold_samples {
                    self.start_decay();
                } else {
                    self.level = 1.0;
                }
            }
            AhdStage::Decay => {
                self.phase += self.decay_rate;
                if self.phase >= 1.0 {
                    // Sustain is the only stage without an end of its own,
                    // and it is reachable only with the gate on and a
                    // sustain worth holding. Everything else terminates.
                    if self.gate && self.sustain > EXP_IDLE_LEVEL {
                        self.stage = AhdStage::Sustain;
                        self.level = self.sustain;
                    } else {
                        self.stage = AhdStage::Idle;
                        self.level = 0.0;
                    }
                } else {
                    self.level = self.decay_level(self.phase);
                }
            }
            AhdStage::Sustain => self.level = self.sustain,
            AhdStage::Release => {
                self.phase += self.release_rate;
                if self.phase >= 1.0 {
                    self.stage = AhdStage::Idle;
                    self.level = 0.0;
                } else {
                    self.level = self.release_from * shape(1.0 - self.phase, self.curve);
                }
            }
        }
    }
}

impl Default for Ahd {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adsr_runs_full_cycle() {
        let sr = 48_000;
        let mut env = Adsr::new(sr);
        env.configure(0.001, 0.01, 0.5, 0.01);
        env.note_on();
        for _ in 0..(0.001 * sr as f32) as usize + 1 {
            env.advance();
        }
        assert!((env.level() - 1.0).abs() < 1e-3);
        // Decay to sustain.
        for _ in 0..(0.01 * sr as f32) as usize + 1 {
            env.advance();
        }
        assert!((env.level() - 0.5).abs() < 1e-3);
        let held = env.level();
        for _ in 0..100 {
            env.advance();
        }
        assert_eq!(held, env.level());
        // Release to idle.
        env.release();
        for _ in 0..(0.01 * sr as f32) as usize + 1 {
            env.advance();
        }
        assert!(env.is_idle());
        assert_eq!(env.level(), 0.0);
    }

    #[test]
    fn release_with_overrides_release_time() {
        let mut env = Adsr::new(48_000);
        env.configure(0.001, 0.01, 1.0, 10.0);
        env.note_on();
        for _ in 0..100 {
            env.advance();
        }
        env.release_with(0.001);
        for _ in 0..48 + 1 {
            env.advance();
        }
        assert!(env.is_idle());
    }

    #[test]
    fn exp_decay_falls_and_terminates() {
        let sr = 48_000;
        let mut env = ExpDecay::new();
        env.set_time(0.01, sr);
        env.trigger();
        assert_eq!(env.level(), 1.0);
        // `set_time`'s seconds argument is the time to become inaudible, so
        // running for slightly longer than that should reach idle.
        for _ in 0..(0.012 * sr as f32) as usize {
            env.advance();
        }
        assert!(env.is_idle());
    }

    #[test]
    fn exp_decay_is_exponential() {
        let sr = 48_000;
        let mut env = ExpDecay::new();
        env.set_time(0.1, sr);
        env.trigger();
        // One 1/e time constant is `seconds / DECAY_TAIL_CONSTANTS`.
        let tau_samples = (0.1 / DECAY_TAIL_CONSTANTS) * sr as f32;
        for _ in 0..tau_samples as usize {
            env.advance();
        }
        // After exactly one time constant the level should be ~1/e.
        assert!((env.level() - (-1.0_f32).exp()).abs() < 0.02);
    }

    const SR: u32 = 48_000;

    /// The literal is `1 / (e^DECAY_TAIL_CONSTANTS - 1)`, written out because
    /// `exp` is not a `const fn`. Pinned to what it is derived from so the
    /// two cannot drift apart silently.
    #[test]
    fn the_exponential_curve_is_normalized() {
        let derived = 1.0 / (DECAY_TAIL_CONSTANTS.exp() - 1.0);
        assert!((EXP_SHAPE_NORM - derived).abs() < 1.0e-9);
        for curve in [-1.0, -0.5, 0.0, 0.5, 1.0] {
            assert!(shape(0.0, curve).abs() < 1.0e-6, "curve {curve} is not 0 at 0");
            assert!(
                (shape(1.0, curve) - 1.0).abs() < 1.0e-6,
                "curve {curve} is not 1 at 1"
            );
        }
    }

    /// Curve 0 is v1's law, which is what makes an old-sounding patch still
    /// reachable: the shaped decay and an `ExpDecay` of the same length agree
    /// sample for sample.
    #[test]
    fn curve_zero_is_the_exponential_decay_law() {
        let decay_s = 0.25;
        let mut shaped = Ahd::new();
        shaped.trigger(
            AhdShape {
                decay_s,
                ..AhdShape::default()
            },
            SR,
        );
        let mut v1 = ExpDecay::new();
        v1.set_time(decay_s, SR);
        v1.trigger();

        let mut worst = 0.0_f32;
        for _ in 0..(decay_s * SR as f32) as usize {
            worst = worst.max((shaped.tick() - v1.level()).abs());
            v1.advance();
        }
        assert!(worst < 2.0e-3, "the shaped decay is {worst} away from v1's");
    }

    /// The three ends of the curve control are audibly different decays, not
    /// three names for one: at half the decay time, a logarithmic tail is
    /// still near the top, an exponential one is 40 dB down, and a linear one
    /// is halfway between.
    #[test]
    fn the_curve_control_reaches_three_different_decays() {
        let at_half = |curve: f32| {
            let mut env = Ahd::new();
            env.trigger(
                AhdShape {
                    decay_s: 0.25,
                    curve,
                    ..AhdShape::default()
                },
                SR,
            );
            for _ in 0..(0.125 * SR as f32) as usize {
                env.tick();
            }
            env.level()
        };
        let logarithmic = at_half(-1.0);
        let exponential = at_half(0.0);
        let linear = at_half(1.0);
        assert!(logarithmic > 0.9, "logarithmic fell to {logarithmic}");
        assert!(
            (exponential - 0.01).abs() < 0.005,
            "exponential is {exponential}, want v1's ~0.01"
        );
        assert!((linear - 0.5).abs() < 0.01, "linear is {linear}");
    }

    /// A segment of zero length costs no samples, so an envelope with neither
    /// attack nor hold is at its peak on sample zero. A drum synth whose
    /// attack cannot be zero is broken.
    #[test]
    fn a_zero_length_segment_costs_no_samples() {
        let mut env = Ahd::new();
        env.trigger(AhdShape::default(), SR);
        assert_eq!(env.tick(), 1.0);

        // And an attack that *is* asked for still rises rather than jumping.
        let mut ramped = Ahd::new();
        ramped.trigger(
            AhdShape {
                attack_s: 0.01,
                ..AhdShape::default()
            },
            SR,
        );
        assert_eq!(ramped.tick(), 0.0);
        let mut peak = 0.0_f32;
        for _ in 0..(0.01 * SR as f32) as usize {
            peak = peak.max(ramped.tick());
        }
        assert!((peak - 1.0).abs() < 1.0e-3, "the attack reached {peak}");
    }

    /// The flat top is the hold's own length plus one: the decay's first
    /// sample is the peak too, which is the same property that makes a
    /// zero-attack envelope start at full level.
    #[test]
    fn hold_is_flat_at_the_peak_for_its_stated_length() {
        let flat_samples = |hold_s: f32| {
            let mut env = Ahd::new();
            env.trigger(
                AhdShape {
                    hold_s,
                    decay_s: 0.25,
                    ..AhdShape::default()
                },
                SR,
            );
            let mut flat = 0;
            while env.tick() == 1.0 {
                flat += 1;
                assert!(flat < SR as usize, "the hold never ended");
            }
            flat
        };
        assert_eq!(flat_samples(0.0), 1);
        assert_eq!(flat_samples(0.02), (0.02 * SR as f32) as usize + 1);
    }

    #[test]
    fn a_gated_envelope_holds_at_sustain_and_releases() {
        let mut env = Ahd::new();
        env.trigger(
            AhdShape {
                decay_s: 0.05,
                sustain: 0.5,
                release_s: 0.05,
                gate: true,
                ..AhdShape::default()
            },
            SR,
        );
        for _ in 0..(0.2 * SR as f32) as usize {
            env.tick();
        }
        assert!(env.is_sustaining());
        assert!((env.level() - 0.5).abs() < 1.0e-4);

        env.release();
        for _ in 0..(0.1 * SR as f32) as usize {
            env.tick();
        }
        assert!(env.is_idle(), "a released envelope kept going");
    }

    #[test]
    fn a_one_shot_envelope_ignores_release() {
        let mut env = Ahd::new();
        env.trigger(
            AhdShape {
                decay_s: 0.5,
                sustain: 0.8,
                ..AhdShape::default()
            },
            SR,
        );
        for _ in 0..1_000 {
            env.tick();
        }
        let before = env.level();
        env.release();
        assert_eq!(env.level(), before);
        assert!(!env.is_idle());

        // A choke is not a note-off, and reaches it anyway.
        env.release_over(0.005, SR);
        for _ in 0..(0.01 * SR as f32) as usize {
            env.tick();
        }
        assert!(env.is_idle());
    }

    /// Sustain is the only stage without an end of its own, and it needs both
    /// a gate and a sustain worth holding. Every other combination of every
    /// segment terminates, which is what stops a voice from being stranded.
    #[test]
    fn every_shape_without_a_held_gate_terminates() {
        for attack_s in [0.0, 0.01, 0.5] {
            for hold_s in [0.0, 0.02] {
                for decay_s in [0.002, 0.25] {
                    for curve in [-1.0, 0.0, 1.0] {
                        for (gate, sustain) in [(false, 0.0), (false, 0.9), (true, 0.0)] {
                            let mut env = Ahd::new();
                            let shape = AhdShape {
                                attack_s,
                                hold_s,
                                decay_s,
                                curve,
                                sustain,
                                release_s: 0.05,
                                gate,
                            };
                            env.trigger(shape, SR);
                            for _ in 0..(2.0 * SR as f32) as usize {
                                let level = env.tick();
                                assert!(level.is_finite() && (0.0..=1.0).contains(&level));
                            }
                            assert!(env.is_idle(), "{shape:?} never ended");
                        }
                    }
                }
            }
        }
    }
}
