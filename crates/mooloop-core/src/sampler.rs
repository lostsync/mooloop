//! Sampler device parameters. Pure data so the bridge can carry them.

/// How the sampler treats the loop region once the play head reaches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopMode {
    /// No looping. Play from `start` to `loop_end` (or sample end), then stop.
    #[default]
    Off,
    /// Loop forward: wrap from `loop_end` back to `loop_start`.
    Forward,
    /// Loop ping-pong: reverse direction at both loop points.
    Pingpong,
}

impl LoopMode {
    pub fn all() -> [LoopMode; 3] {
        [LoopMode::Off, LoopMode::Forward, LoopMode::Pingpong]
    }

    pub fn label(self) -> &'static str {
        match self {
            LoopMode::Off => "Off",
            LoopMode::Forward => "Fwd",
            LoopMode::Pingpong => "Pong",
        }
    }
}

/// All sampler parameters, in the units the DSP and UI share. All points are
/// fractions of the sample length in `[0, 1]`; times are seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SamplerParams {
    /// Play start point as a fraction of the sample length.
    pub start: f32,
    /// Loop start point as a fraction.
    pub loop_start: f32,
    /// Loop end point as a fraction.
    pub loop_end: f32,
    pub loop_mode: LoopMode,
    /// Attack time (seconds).
    pub attack: f32,
    /// Decay time (seconds).
    pub decay: f32,
    /// Sustain level in `[0, 1]`.
    pub sustain: f32,
    /// Release time (seconds).
    pub release: f32,
}

impl Default for SamplerParams {
    fn default() -> Self {
        Self {
            start: 0.0,
            loop_start: 0.0,
            loop_end: 1.0,
            loop_mode: LoopMode::Off,
            attack: 0.001,
            decay: 0.25,
            sustain: 0.0,
            release: 0.05,
        }
    }
}

/// Clamp helper used by both DSP (defensive) and UI (input validation).
pub fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}
