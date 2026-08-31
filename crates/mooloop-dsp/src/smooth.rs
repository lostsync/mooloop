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

    /// Advance `frames` samples in one step, landing where that many calls to
    /// `advance` would have left it (within `SNAP_EPSILON`).
    ///
    /// This exists for a device that fans one smoothed value out over several
    /// voices: each voice walks its own copy sample by sample, so the shared
    /// original has to be caught up once for the whole block rather than once
    /// per voice.
    pub fn advance_by(&mut self, frames: usize) -> f32 {
        let delta = self.target - self.current;
        if frames == 0 {
            return self.current;
        }
        // `(1 - coeff)^frames` is the closed form of the per-sample recurrence
        // `current += (target - current) * coeff`.
        let remaining = delta * (1.0 - self.coeff).powi(frames.min(i32::MAX as usize) as i32);
        if remaining.abs() < SNAP_EPSILON {
            self.current = self.target;
        } else {
            self.current = self.target - remaining;
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

    /// The whole point of `advance_by`: a caller that walks copies per voice
    /// and catches the original up in one step must not drift away from a
    /// caller that walked it sample by sample. The tolerance is loose enough
    /// for the `f32` rounding the per-sample walk accumulates over thousands
    /// of steps and the closed form does not -- where they differ, the
    /// closed form is the more accurate of the two.
    #[test]
    fn advancing_a_block_matches_walking_it_sample_by_sample() {
        for frames in [1usize, 7, 64, 512, 4096] {
            let mut walked = Smoothed::new(0.25, 0.005, 48_000);
            walked.set_target(1.0);
            let mut stepped = walked;
            for _ in 0..frames {
                walked.advance();
            }
            stepped.advance_by(frames);
            assert!(
                (walked.value() - stepped.value()).abs() <= 3.0e-5,
                "{frames} frames: walked to {}, stepped to {}",
                walked.value(),
                stepped.value()
            );
        }
    }

    #[test]
    fn advancing_no_frames_holds_still() {
        let mut smoothed = Smoothed::new(0.25, 0.005, 48_000);
        smoothed.set_target(1.0);
        assert_eq!(smoothed.advance_by(0), 0.25);
    }

    #[test]
    fn reset_skips_the_lag() {
        let mut smoothed = Smoothed::new(0.0, 0.005, 48_000);
        smoothed.reset_to(0.5);
        assert_eq!(smoothed.advance(), 0.5);
    }
}
