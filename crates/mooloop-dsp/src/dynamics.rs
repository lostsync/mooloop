//! Level detection and gain computation shared by the dynamics effects.
//!
//! Gate, compressor, and limiter differ only in how they turn a measured
//! level into a gain; the measuring, the smoothing, and the decibel plumbing
//! are the same in all three, so they live here.
//!
//! ## Stereo linking
//!
//! Every dynamics effect in this crate detects on the **maximum** of the two
//! channels and applies one gain to both. Detecting per channel would let a
//! loud left channel duck only itself, which walks the stereo image around
//! under compression. Linking is the default people expect and the only mode
//! offered for now.

/// Smallest level fed to the log converter, about -180 dB. Keeps silence from
/// producing negative infinity and poisoning the gain computers.
const MIN_LEVEL: f32 = 1e-9;

/// Gap below which `EnvelopeFollower::process` snaps to the rectified input
/// instead of continuing to decay toward it. Far below any audible or
/// musically meaningful level, and far above `f32`'s subnormal range.
const SNAP_EPSILON: f32 = 1.0e-9;

pub fn lin_to_db(level: f32) -> f32 {
    20.0 * level.abs().max(MIN_LEVEL).log10()
}

pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// One-pole coefficient for a time constant in milliseconds. This is the
/// fraction of the old value *retained* per sample, so 0 is instant.
pub fn time_coeff(ms: f32, sample_rate: u32) -> f32 {
    let sr = sample_rate.max(1) as f32;
    let samples = ms.max(0.0) * 0.001 * sr;
    if samples <= f32::EPSILON {
        0.0
    } else {
        (-1.0 / samples).exp()
    }
}

/// A peak envelope follower with separate attack and release times.
///
/// Peak rather than RMS: these effects exist to catch transients in
/// percussive material, and an RMS window would let exactly the hits that
/// matter through before responding.
#[derive(Debug, Clone, Copy, Default)]
pub struct EnvelopeFollower {
    envelope: f32,
    attack: f32,
    release: f32,
}

impl EnvelopeFollower {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set attack and release in milliseconds.
    pub fn set_times(&mut self, attack_ms: f32, release_ms: f32, sample_rate: u32) {
        self.attack = time_coeff(attack_ms, sample_rate);
        self.release = time_coeff(release_ms, sample_rate);
    }

    pub fn envelope(&self) -> f32 {
        self.envelope
    }

    pub fn reset(&mut self) {
        self.envelope = 0.0;
    }

    /// Feed one rectified sample and return the smoothed envelope. Rising
    /// input uses the attack time, falling input the release time.
    pub fn process(&mut self, input: f32) -> f32 {
        let rectified = input.abs();
        let coeff = if rectified > self.envelope {
            self.attack
        } else {
            self.release
        };
        let delta = self.envelope - rectified;
        // Snap once the remaining gap is inaudibly small rather than let it
        // decay asymptotically forever: the tail would otherwise spend many
        // samples as a subnormal float, which is far slower to compute than
        // the snap it approximates.
        self.envelope = if delta.abs() < SNAP_EPSILON {
            rectified
        } else {
            rectified + coeff * delta
        };
        self.envelope
    }
}

/// Gain reduction in dB (always <= 0) for a compressor's static curve.
///
/// `knee_db` spreads the transition symmetrically around the threshold with a
/// quadratic interpolation, so the onset of compression is gradual rather
/// than a corner. A knee of 0 gives the hard corner.
pub fn compressor_gain_db(input_db: f32, threshold_db: f32, ratio: f32, knee_db: f32) -> f32 {
    let ratio = ratio.max(1.0);
    let slope = 1.0 - 1.0 / ratio;
    let over = input_db - threshold_db;
    let half_knee = knee_db.max(0.0) * 0.5;

    if over <= -half_knee {
        0.0
    } else if over >= half_knee && half_knee > 0.0 {
        -slope * over
    } else if half_knee <= 0.0 {
        // Hard knee: everything above the threshold is on the ratio line.
        if over > 0.0 {
            -slope * over
        } else {
            0.0
        }
    } else {
        let x = over + half_knee;
        -slope * x * x / (4.0 * half_knee)
    }
}

/// Gain reduction in dB (always <= 0) for a gate's static curve.
///
/// Below the threshold the gate closes to `range_db`. `range_db` is the floor
/// rather than a slope, because a gate that merely expands is a different
/// effect from one that shuts — this one shuts.
pub fn gate_gain_db(input_db: f32, threshold_db: f32, range_db: f32) -> f32 {
    if input_db >= threshold_db {
        0.0
    } else {
        range_db.min(0.0)
    }
}

/// Gain reduction in dB (always <= 0) that brings `input_db` down to
/// `ceiling_db`.
pub fn limiter_gain_db(input_db: f32, ceiling_db: f32) -> f32 {
    (ceiling_db - input_db).min(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decibel_conversions_round_trip() {
        for db in [-60.0, -24.0, -6.0, 0.0, 6.0] {
            let back = lin_to_db(db_to_lin(db));
            assert!((back - db).abs() < 1e-3, "{db} round-tripped to {back}");
        }
        assert!((lin_to_db(1.0)).abs() < 1e-4);
        assert!((db_to_lin(0.0) - 1.0).abs() < 1e-6);
        assert!(lin_to_db(0.0).is_finite(), "silence must not produce -inf");
    }

    #[test]
    fn a_zero_time_constant_is_instant() {
        assert_eq!(time_coeff(0.0, 48_000), 0.0);
        let mut env = EnvelopeFollower::new();
        env.set_times(0.0, 0.0, 48_000);
        assert!((env.process(0.7) - 0.7).abs() < 1e-6);
    }

    #[test]
    fn the_follower_rises_fast_and_falls_slow() {
        let mut env = EnvelopeFollower::new();
        env.set_times(1.0, 500.0, 48_000);
        // Attack: reaches most of the way within a few time constants.
        for _ in 0..(48 * 5) {
            env.process(1.0);
        }
        let peaked = env.envelope();
        assert!(peaked > 0.9, "attack too slow: {peaked}");

        // Release: still well above zero after the same number of samples.
        for _ in 0..(48 * 5) {
            env.process(0.0);
        }
        let held = env.envelope();
        assert!(
            held > 0.9 * peaked,
            "release too fast: {held} from {peaked}"
        );
    }

    #[test]
    fn the_follower_tracks_a_rectified_peak() {
        let mut env = EnvelopeFollower::new();
        env.set_times(0.0, 1_000.0, 48_000);
        // Negative peaks must register: detection is on magnitude.
        env.process(-0.8);
        assert!((env.envelope() - 0.8).abs() < 1e-6);
    }

    #[test]
    fn compression_is_inactive_below_the_threshold() {
        for input in [-60.0, -40.0, -25.0] {
            let gain = compressor_gain_db(input, -20.0, 4.0, 0.0);
            assert_eq!(gain, 0.0, "input {input} should be untouched");
        }
    }

    #[test]
    fn compression_follows_the_ratio_above_the_threshold() {
        // 12 dB over a -20 dB threshold at 4:1 leaves 3 dB over: 9 dB down.
        let gain = compressor_gain_db(-8.0, -20.0, 4.0, 0.0);
        assert!((gain + 9.0).abs() < 1e-3, "expected -9 dB, got {gain}");

        // Infinite-ish ratio pins the output at the threshold.
        let gain = compressor_gain_db(-8.0, -20.0, 1_000.0, 0.0);
        assert!((gain + 12.0).abs() < 0.1, "expected about -12 dB, got {gain}");

        // Unity ratio never reduces.
        assert_eq!(compressor_gain_db(0.0, -20.0, 1.0, 0.0), 0.0);
    }

    #[test]
    fn the_knee_is_continuous_and_sits_between_the_hard_curves() {
        let (threshold, ratio, knee) = (-20.0f32, 4.0f32, 12.0f32);
        // Continuity: no jumps anywhere across the knee region. The bound has
        // to clear the curve's own steepest legitimate slope over one step
        // (the ratio line, 1 - 1/ratio) or it flags the curve for being a
        // curve; a real discontinuity is several dB.
        let step_db = 0.1f32;
        let steepest = (1.0 - 1.0 / ratio) * step_db;
        let mut previous = compressor_gain_db(-45.0, threshold, ratio, knee);
        for step in 0..500 {
            let input = -45.0 + step as f32 * step_db;
            let gain = compressor_gain_db(input, threshold, ratio, knee);
            assert!(
                (gain - previous).abs() <= steepest + 1e-3,
                "discontinuity at {input}: {previous} -> {gain}"
            );
            assert!(gain <= 1e-6, "gain must never boost: {gain} at {input}");
            previous = gain;
        }
        // At the threshold itself a soft knee is already reducing a little,
        // where a hard knee would still be inactive.
        let soft = compressor_gain_db(threshold, threshold, ratio, knee);
        let hard = compressor_gain_db(threshold, threshold, ratio, 0.0);
        assert!(soft < hard, "soft knee {soft} should lead hard knee {hard}");
    }

    #[test]
    fn the_knee_rejoins_the_hard_curve_past_its_edge() {
        let (threshold, ratio, knee) = (-20.0f32, 4.0f32, 12.0f32);
        let past = threshold + knee; // clear of the knee's upper edge
        let soft = compressor_gain_db(past, threshold, ratio, knee);
        let hard = compressor_gain_db(past, threshold, ratio, 0.0);
        assert!(
            (soft - hard).abs() < 1e-3,
            "curves should meet past the knee: {soft} vs {hard}"
        );
    }

    #[test]
    fn the_gate_shuts_to_its_range_below_the_threshold() {
        assert_eq!(gate_gain_db(-10.0, -30.0, -60.0), 0.0);
        assert_eq!(gate_gain_db(-40.0, -30.0, -60.0), -60.0);
        // A range of 0 makes the gate a no-op rather than an inversion.
        assert_eq!(gate_gain_db(-90.0, -30.0, 0.0), 0.0);
    }

    #[test]
    fn the_limiter_only_ever_pulls_down_to_the_ceiling() {
        assert_eq!(limiter_gain_db(-12.0, -3.0), 0.0);
        assert!((limiter_gain_db(0.0, -3.0) + 3.0).abs() < 1e-6);
        assert!((limiter_gain_db(6.0, 0.0) + 6.0).abs() < 1e-6);
    }
}
