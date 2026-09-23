//! The master's last stage: what stands between the mix and the driver.
//!
//! Two jobs, in this order, on every frame the master hands to the output
//! ports or to an export:
//!
//! 1. **Scrub.** A sample that is NaN or infinite is replaced by zero and
//!    counted. A device that blows up must not reach a speaker, and a NaN in a
//!    file is a file every later tool chokes on. The count is the fault: the
//!    engine publishes it so the interface can say something went wrong, and
//!    an export reports it.
//! 2. **Limit.** Nothing leaves above [`OUTPUT_CEILING`], which is 0 dBFS. The
//!    limiter is a *safety* stage, not a musical one (`docs/SCOPE.md` §2.1
//!    keeps it apart from the master bus compressor, MOO-13): it exists so a
//!    runaway resonance or a stack of hot channels does not hit a DAC or a
//!    24-bit file as a wall of clipping, and it is built to be inaudible
//!    everywhere it is not needed.
//!
//! # Transparent below the ceiling
//!
//! **A signal that never exceeds the ceiling passes bit for bit.** The gain is
//! held as a *reduction* from unity, exactly zero at rest, and a frame is only
//! multiplied while that reduction is non-zero -- so a mix that stays under
//! 0 dBFS leaves exactly as it arrived, and the mixer's linear-summing
//! guarantees (`docs/GAIN_STRUCTURE.md`) still hold at the ports.
//! `a_signal_under_the_ceiling_passes_bit_identical` holds that.
//!
//! # How it limits
//!
//! Zero latency: instant attack, a hold, then an exponential release, with
//! the two sides linked so a limited stereo image does not lean.
//!
//! - **Instant attack.** A frame above the ceiling sets the reduction to at
//!   least what brings that frame down to it, on that frame. Without
//!   lookahead that is the only way to guarantee the ceiling; the first frame
//!   of an over is therefore shaped rather than ducked ahead of time.
//! - **Hold, [`HOLD_SECONDS`].** The reduction stays put for 20 ms after the
//!   last over, which spans the gap between the two peaks of any cycle above
//!   25 Hz, so a sustained over-level tone is turned down rather than
//!   re-shaped every half cycle.
//! - **Release, [`RELEASE_SECONDS`].** Then the reduction decays by a one-pole
//!   towards zero. It decays as a *reduction* rather than a gain rising
//!   towards one because a gain near unity in `f32` stalls: its increments
//!   fall below one ulp of 1.0 and it never arrives. A reduction near zero has
//!   all the precision it needs and reaches zero, at which point the stage is
//!   bit-transparent again.
//! - **A final clamp** at the ceiling, because `x * (ceiling / x)` in floating
//!   point may land an ulp above it.
//!
//! No lookahead is a decision with a cost -- an over's leading edge is
//! clipped for the one frame it takes the gain to arrive -- taken because a
//! lookahead limiter delays *everything* on the master, which moves monitoring
//! latency and every recording's alignment for a stage that should normally be
//! doing nothing at all.

/// The level nothing leaves the master above: 0 dBFS.
pub const OUTPUT_CEILING: f32 = 1.0;

/// How long the reduction holds after the last frame above the ceiling.
pub const HOLD_SECONDS: f32 = 0.020;

/// The release's time constant: the reduction falls to 1/e of itself in this
/// long once the hold runs out.
pub const RELEASE_SECONDS: f32 = 0.150;

/// A reduction this small is below one ulp of unity gain, so it is zero.
const AT_REST: f32 = 1.0e-9;

/// What one block through the guard found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GuardReport {
    /// Samples (left and right counted separately) that were NaN or
    /// infinite, and were replaced by zero.
    pub non_finite: u32,
    /// Samples that arrived above the ceiling and were limited to it.
    pub overs: u32,
}

impl GuardReport {
    /// Add another block's findings to these.
    pub fn add(&mut self, other: GuardReport) {
        self.non_finite = self.non_finite.saturating_add(other.non_finite);
        self.overs = self.overs.saturating_add(other.overs);
    }
}

/// The master output's safety stage. See the module documentation.
#[derive(Debug, Clone)]
pub struct OutputGuard {
    /// [`OUTPUT_CEILING`] everywhere but in a test that measures the mix
    /// above it; see [`Self::without_limit`].
    ceiling: f32,
    /// `1 - gain`: zero at rest, and the only thing that decides whether a
    /// frame is touched at all.
    reduction: f32,
    /// Frames left before the release starts.
    hold_left: u32,
    hold_frames: u32,
    /// What the reduction is multiplied by per frame of release.
    release: f32,
}

impl OutputGuard {
    pub fn new(sample_rate: u32) -> Self {
        let rate = sample_rate.max(1) as f32;
        Self {
            ceiling: OUTPUT_CEILING,
            reduction: 0.0,
            hold_left: 0,
            hold_frames: (HOLD_SECONDS * rate).round() as u32,
            release: (-1.0 / (RELEASE_SECONDS * rate)).exp(),
        }
    }

    /// A guard that scrubs and never limits.
    ///
    /// Not something the engine ever runs. The mixer's own guarantees --
    /// summing is linear up to the +12 dB fader ceiling, N channels are
    /// 20·log10(N) dB louder than one -- are claims about the *mix*, and a
    /// test that measures them above 0 dBFS at the master would otherwise be
    /// measuring this limiter instead.
    pub fn without_limit(sample_rate: u32) -> Self {
        Self {
            ceiling: f32::INFINITY,
            ..Self::new(sample_rate)
        }
    }

    /// Replace every non-finite sample with zero, and say how many there
    /// were. The scrub alone, for a reader of the master that must not see a
    /// NaN but runs before the limiter does.
    pub fn scrub(left: &mut [f32], right: &mut [f32]) -> u32 {
        let mut found = 0u32;
        for sample in left.iter_mut().chain(right.iter_mut()) {
            if !sample.is_finite() {
                *sample = 0.0;
                found = found.saturating_add(1);
            }
        }
        found
    }

    /// Scrub and limit one block in place.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) -> GuardReport {
        let mut report = GuardReport::default();
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let mut a = *l;
            let mut b = *r;
            if !a.is_finite() {
                a = 0.0;
                report.non_finite = report.non_finite.saturating_add(1);
            }
            if !b.is_finite() {
                b = 0.0;
                report.non_finite = report.non_finite.saturating_add(1);
            }
            let ceiling = self.ceiling;
            let peak = a.abs().max(b.abs());
            if peak > ceiling {
                let over = u32::from(a.abs() > ceiling) + u32::from(b.abs() > ceiling);
                report.overs = report.overs.saturating_add(over);
                let needed = 1.0 - ceiling / peak;
                if needed > self.reduction {
                    self.reduction = needed;
                }
                self.hold_left = self.hold_frames;
            } else if self.hold_left > 0 {
                self.hold_left -= 1;
            } else if self.reduction > 0.0 {
                self.reduction *= self.release;
                if self.reduction < AT_REST {
                    self.reduction = 0.0;
                }
            }
            if self.reduction > 0.0 {
                let gain = 1.0 - self.reduction;
                a = (a * gain).clamp(-ceiling, ceiling);
                b = (b * gain).clamp(-ceiling, ceiling);
            }
            *l = a;
            *r = b;
        }
        report
    }

    /// The gain the limiter is applying now: exactly 1.0 at rest.
    pub fn gain(&self) -> f32 {
        1.0 - self.reduction
    }

    /// Take over another guard's envelope -- how far down it is and how long
    /// it still holds -- but keep this one's timing, which belongs to this
    /// one's sample rate. What a renderer replacing a running one calls, so a
    /// swap in the middle of an over does not jump the level back to unity.
    pub fn adopt(&mut self, other: &OutputGuard) {
        self.reduction = other.reduction;
        self.hold_left = other.hold_left.min(self.hold_frames);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn sine(amplitude: f32, hz: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|n| amplitude * (2.0 * core::f32::consts::PI * hz * n as f32 / RATE as f32).sin())
            .collect()
    }

    fn run(guard: &mut OutputGuard, left: &mut [f32], right: &mut [f32]) -> GuardReport {
        let mut report = GuardReport::default();
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            report.add(guard.process(l, r));
        }
        report
    }

    /// The limiter's transparency, pinned: a mix that never exceeds the
    /// ceiling leaves bit for bit, including frames sitting exactly on it.
    #[test]
    fn a_signal_under_the_ceiling_passes_bit_identical() {
        let mut guard = OutputGuard::new(RATE);
        let mut left = sine(0.999, 110.0, RATE as usize);
        let mut right = sine(0.5, 3_000.0, RATE as usize);
        left[100] = 1.0;
        right[200] = -1.0;
        left[300] = 1.0e-30;
        let (want_l, want_r) = (left.clone(), right.clone());
        let report = run(&mut guard, &mut left, &mut right);
        assert_eq!(report, GuardReport::default());
        for (frame, (got, want)) in left.iter().zip(&want_l).enumerate() {
            assert_eq!(got.to_bits(), want.to_bits(), "left moved at frame {frame}");
        }
        for (frame, (got, want)) in right.iter().zip(&want_r).enumerate() {
            assert_eq!(got.to_bits(), want.to_bits(), "right moved at frame {frame}");
        }
        assert_eq!(guard.gain(), 1.0);
    }

    /// Forty decibels over, held for a second: nothing leaves above the
    /// ceiling, and the output sits *at* it rather than somewhere below.
    #[test]
    fn an_input_far_above_the_ceiling_is_held_at_the_ceiling() {
        let mut guard = OutputGuard::new(RATE);
        let mut left = sine(100.0, 110.0, RATE as usize);
        let mut right = sine(100.0, 110.0, RATE as usize);
        let report = run(&mut guard, &mut left, &mut right);
        assert!(report.overs > 0);
        let peak = left.iter().chain(&right).fold(0.0f32, |p, s| p.max(s.abs()));
        assert!(peak <= OUTPUT_CEILING, "{peak} left the master");
        // The last quarter-second, once the reduction has settled: its peaks
        // are the ceiling, not a level the limiter overshot to.
        let tail = &left[RATE as usize * 3 / 4..];
        let tail_peak = tail.iter().fold(0.0f32, |p, s| p.max(s.abs()));
        assert!(tail_peak > 0.99, "settled at {tail_peak}, not at the ceiling");
    }

    /// Linked: an over on one side turns the other side down by the same
    /// amount, so a limited image does not lean.
    #[test]
    fn both_sides_are_turned_down_together() {
        let mut guard = OutputGuard::new(RATE);
        let mut left = vec![4.0f32; 64];
        let mut right = vec![0.5f32; 64];
        guard.process(&mut left, &mut right);
        assert!((left[63] - 1.0).abs() < 1e-6);
        assert!((right[63] - 0.125).abs() < 1e-6, "right is {}", right[63]);
    }

    #[test]
    fn non_finite_samples_are_zeroed_and_counted() {
        let mut guard = OutputGuard::new(RATE);
        let mut left = vec![0.25, f32::NAN, f32::INFINITY, 0.25];
        let mut right = vec![f32::NEG_INFINITY, 0.25, 0.25, 0.25];
        let report = guard.process(&mut left, &mut right);
        assert_eq!(report.non_finite, 3);
        assert_eq!(report.overs, 0, "an infinity is a fault, not an over");
        assert_eq!(left, vec![0.25, 0.0, 0.0, 0.25]);
        assert_eq!(right, vec![0.0, 0.25, 0.25, 0.25]);
        assert_eq!(guard.gain(), 1.0, "a scrubbed sample moved the limiter");

        let mut left = vec![f32::NAN, 0.5];
        let mut right = vec![0.5, f32::INFINITY];
        assert_eq!(OutputGuard::scrub(&mut left, &mut right), 2);
        assert_eq!((left, right), (vec![0.0, 0.5], vec![0.5, 0.0]));
    }

    /// An over is not a permanent change of level: once the hold and the
    /// release have run, the stage is bit-transparent again.
    #[test]
    fn after_an_over_it_releases_back_to_bit_identical() {
        let mut guard = OutputGuard::new(RATE);
        let mut left = vec![2.0f32; 256];
        let mut right = vec![2.0f32; 256];
        guard.process(&mut left, &mut right);
        assert!(guard.gain() < 0.51);

        // Mid-release, a quiet signal is still turned down.
        let mut left = sine(0.5, 440.0, RATE as usize / 10);
        let mut right = left.clone();
        let quiet = left.clone();
        run(&mut guard, &mut left, &mut right);
        assert!(left.iter().zip(&quiet).any(|(a, b)| a != b));

        // Four seconds on, the same signal passes untouched.
        let mut left = sine(0.5, 440.0, RATE as usize * 4);
        let mut right = left.clone();
        run(&mut guard, &mut left, &mut right);
        assert_eq!(guard.gain(), 1.0);
        let mut left = sine(0.5, 440.0, 4_096);
        let mut right = left.clone();
        let quiet = left.clone();
        run(&mut guard, &mut left, &mut right);
        assert_eq!(left, quiet);
    }

    #[test]
    fn a_guard_without_a_limit_still_scrubs_and_passes_overs_untouched() {
        let mut guard = OutputGuard::without_limit(RATE);
        let mut left = vec![8.0, f32::NAN, -3.0];
        let mut right = vec![0.5, 0.5, 0.5];
        let report = guard.process(&mut left, &mut right);
        assert_eq!(report, GuardReport { non_finite: 1, overs: 0 });
        assert_eq!((left, right), (vec![8.0, 0.0, -3.0], vec![0.5, 0.5, 0.5]));
    }

    #[test]
    fn a_replacement_guard_keeps_the_envelope_it_takes_over() {
        let mut running = OutputGuard::new(RATE);
        running.process(&mut [3.0], &mut [0.0]);
        let mut fresh = OutputGuard::new(RATE);
        fresh.adopt(&running);
        assert_eq!(fresh.gain(), running.gain());
        let (mut l, mut r) = ([0.9f32], [0.0f32]);
        fresh.process(&mut l, &mut r);
        assert!(l[0] < 0.9, "the adopted hold did not hold");
    }
}
