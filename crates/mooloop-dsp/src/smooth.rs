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
///
/// It only decides the end of the lag near zero. Everywhere else `f32` runs
/// out of resolution first: once `gap * coeff` is under half a unit in the
/// last place of the current value, the step rounds to nothing and the lag
/// stalls short of its target for good. For a 10 ms lag landing on 0.7 that
/// is about 1.4e-5 short; for the reverb's 120 ms predelay lag, which counts
/// in samples, it is more than a whole sample. `advance` finishes those by
/// creeping one unit in the last place per sample instead.
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
            return self.current;
        }
        let next = self.current + delta * self.coeff;
        self.current = if next != self.current {
            next
        } else if delta > 0.0 {
            // The step rounded to nothing: the lag has stalled (see
            // `SNAP_EPSILON`). One unit in the last place is the smallest move
            // there is, and the target is itself an `f32`, so this lands on
            // it exactly without ever stepping by more than the lag would.
            self.current.next_up()
        } else {
            self.current.next_down()
        };
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

    /// Whether the lag has reached its target and stopped moving.
    ///
    /// Exact rather than approximate: `advance` snaps once the remaining gap
    /// is under `SNAP_EPSILON` and creeps onto the target once `f32` can no
    /// longer represent its step, and `advance_by`'s closed form rounds onto
    /// the target once the gap is under half a unit in its last place, so a
    /// settled smoother really does hold still,
    /// and a device can tell a host it has nothing left to do without the
    /// answer depending on how many samples it is asked about.
    pub fn is_settled(&self) -> bool {
        self.current == self.target
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

    /// A lag that never arrives is never at rest, so a device reporting
    /// `is_settled` to the idle-skip host would run forever. `f32` makes that
    /// the default outcome: once `gap * coeff` is under half a unit in the
    /// last place of the current value the step rounds to nothing, and for a
    /// 10 ms lag landing on 0.7 that happens about 1.4e-5 short -- four orders
    /// of magnitude above an absolute snap threshold. Covers slow lags, where
    /// the stall is widest, and targets far from 1.0 in both directions.
    #[test]
    fn every_lag_reaches_its_target_and_stops() {
        let sr = 48_000;
        for (from, to, time_s) in [
            (0.2_f32, 0.7_f32, 0.010_f32),
            (0.7, 0.2, 0.010),
            (0.0, 1.0, 0.005),
            (1.0, 0.0, 0.005),
            (0.0, 1.0, 1.0),
            (-0.3, 0.9, 0.050),
            (20_000.0, 440.0, 0.010),
            (1.0e-4, 3.0e-4, 0.020),
            // The reverb's predelay lag, which is counted in samples.
            (0.0, 4_800.0, 0.120),
        ] {
            let mut smoothed = Smoothed::new(from, time_s, sr);
            smoothed.set_target(to);
            // Forty time constants is far past any audible movement.
            let budget = (40.0 * time_s * sr as f32) as usize;
            let mut previous = smoothed.value();
            let mut largest_final_step = 0.0_f32;
            for _ in 0..budget {
                let value = smoothed.advance();
                if smoothed.is_settled() {
                    largest_final_step = (value - previous).abs();
                    break;
                }
                previous = value;
            }
            assert!(
                smoothed.is_settled(),
                "{from} -> {to} over {time_s} s stalled at {} ({:e} short)",
                smoothed.value(),
                to - smoothed.value()
            );
            assert_eq!(smoothed.value(), to);
            // Arriving must not be a step of its own: the last move is no
            // larger than one unit in the last place of the target.
            let ulp = to.abs().next_up() - to.abs();
            assert!(
                largest_final_step <= ulp.max(SNAP_EPSILON),
                "{from} -> {to} over {time_s} s arrived with a step of {largest_final_step:e}"
            );
        }
    }

    #[test]
    fn advancing_a_block_reaches_the_target_too() {
        let mut smoothed = Smoothed::new(0.2, 0.010, 48_000);
        smoothed.set_target(0.7);
        for _ in 0..40 {
            smoothed.advance_by(512);
        }
        assert!(smoothed.is_settled(), "stalled at {}", smoothed.value());
    }

    #[test]
    fn reset_skips_the_lag() {
        let mut smoothed = Smoothed::new(0.0, 0.005, 48_000);
        smoothed.reset_to(0.5);
        assert_eq!(smoothed.advance(), 0.5);
    }
}
