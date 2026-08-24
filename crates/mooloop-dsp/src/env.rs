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
}
