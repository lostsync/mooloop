//! Parameter smoothing.
//!
//! Parameters arrive from the UI once per block, so using them raw means a
//! knob turn steps the signal at every block boundary — zipper noise, or a
//! plain click when the parameter scales amplitude directly. `Smoothed` runs
//! a one-pole lag toward the incoming target so the audible value always
//! moves continuously.

/// Gap below which `Smoothed::advance` snaps to the target instead of
/// continuing to decay toward it. Far below any audible or musically
/// meaningful parameter step, and far above `f32`'s subnormal range.
const SNAP_EPSILON: f32 = 1.0e-9;

/// A one-pole smoothed scalar. `set_target` is cheap enough to call every
/// block; `advance` moves it one sample.
#[derive(Clone, Copy, Debug)]
pub struct Smoothed {
    current: f32,
    target: f32,
    coeff: f32,
}

impl Smoothed {
    /// `time_s` is the time constant: the lag settles to within ~2% of a new
    /// target after five of them.
    pub fn new(initial: f32, time_s: f32, sample_rate: u32) -> Self {
        let mut smoothed = Self {
            current: initial,
            target: initial,
            coeff: 0.0,
        };
        smoothed.set_time(time_s, sample_rate);
        smoothed
    }

    pub fn set_time(&mut self, time_s: f32, sample_rate: u32) {
        let samples = (time_s.max(1.0e-5) * sample_rate as f32).max(1.0);
        self.coeff = 1.0 - (-1.0 / samples).exp();
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Jump straight to a value, skipping the lag. Use when there is nothing
    /// to click — a voice starting from silence, or a reset.
    pub fn reset_to(&mut self, value: f32) {
        self.current = value;
        self.target = value;
    }

    /// Advance one sample and return the smoothed value.
    pub fn advance(&mut self) -> f32 {
        let delta = self.target - self.current;
        // Snap once the remaining gap is inaudibly small rather than let it
        // decay asymptotically forever: the tail would otherwise spend many
        // samples as a subnormal float, which is far slower to compute than
        // the snap it approximates.
        if delta.abs() < SNAP_EPSILON {
            self.current = self.target;
        } else {
            self.current += delta * self.coeff;
        }
        self.current
    }

    pub fn value(&self) -> f32 {
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approaches_target_without_jumping() {
        let sr = 48_000;
        let mut smoothed = Smoothed::new(0.0, 0.005, sr);
        smoothed.set_target(1.0);
        let first = smoothed.advance();
        assert!(first > 0.0 && first < 0.01, "{first}");
        for _ in 0..(0.05 * sr as f32) as usize {
            smoothed.advance();
        }
        assert!((smoothed.value() - 1.0).abs() < 1.0e-3);
    }

    #[test]
    fn reset_skips_the_lag() {
        let mut smoothed = Smoothed::new(0.0, 0.005, 48_000);
        smoothed.reset_to(0.5);
        assert_eq!(smoothed.advance(), 0.5);
    }
}
