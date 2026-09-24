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
//! No lookahead is the default, and a decision with a cost -- an over's
//! leading edge is clipped for the one frame it takes the gain to arrive --
//! taken because a lookahead limiter delays *everything* on the master, which
//! moves monitoring latency and every recording's alignment for a stage that
//! should normally be doing nothing at all.
//!
//! # Lookahead, when asked for
//!
//! Adam, 2026-09-23 (MOO-169): *"make it a knob, defaults to 0.0"*.
//! [`OutputGuard::set_lookahead_ms`] takes up to [`MAX_LOOKAHEAD_MS`].
//!
//! - **At 0 the guard runs exactly the code above**, not a delay of length
//!   zero: the same branch, so every guarantee MOO-93 pinned stands as it
//!   was.
//! - **Above 0** the output is the input `L` frames late, and the detector
//!   reads the frame arriving. An over starts a *linear ramp* of the
//!   reduction that reaches what that frame needs by the time the frame
//!   leaves, so its leading edge is turned down rather than shaped. The ramp
//!   runs at the steeper of the slope it is on and the one the new over
//!   needs -- which is what keeps every earlier over in the window covered --
//!   and the hold counts from when the over *leaves*. The final clamp stays,
//!   as a backstop against rounding.
//! - Below the ceiling it is a pure delay, bit for bit.
//!
//! The delay costs `L` frames on everything leaving the master, so the
//! caller reports it: [`OutputGuard::latency_frames`]. Changing it while the
//! mix plays moves the output by up to 5 ms, once, as the knob moves.

pub use mooloop_core::strip::MAX_LOOKAHEAD_MS;

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
    sample_rate: u32,
    /// The lookahead in frames. Zero runs the zero-latency path.
    lookahead: usize,
    /// The frames in flight, `lookahead` of them in use. Allocated for
    /// [`MAX_LOOKAHEAD_MS`] when the guard is built, so setting a lookahead
    /// never allocates.
    ring: Vec<[f32; 2]>,
    /// The slot the next frame is written to, and the oldest one read.
    head: usize,
    /// Where the lookahead's ramp is heading, and how far it moves a frame.
    target: f32,
    step: f32,
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
            sample_rate: sample_rate.max(1),
            lookahead: 0,
            ring: vec![[0.0; 2]; lookahead_frames(MAX_LOOKAHEAD_MS, sample_rate).max(1)],
            head: 0,
            target: 0.0,
            step: 0.0,
        }
    }

    /// Look ahead by `ms` milliseconds, `0..=MAX_LOOKAHEAD_MS`. Zero is the
    /// zero-latency guard exactly. A change empties the frames in flight, so
    /// the output moves by the difference, once.
    pub fn set_lookahead_ms(&mut self, ms: f32) {
        let frames = lookahead_frames(ms, self.sample_rate).min(self.ring.len());
        if frames == self.lookahead {
            return;
        }
        self.lookahead = frames;
        self.ring.fill([0.0; 2]);
        self.head = 0;
        self.target = self.reduction;
        self.step = 0.0;
    }

    /// How far behind its input the guard's output is, in frames: what the
    /// master's lookahead adds to everything leaving it.
    pub fn latency_frames(&self) -> u32 {
        self.lookahead as u32
    }

    /// Whether the guard holds nothing: no reduction, and no frame in flight
    /// that is not silence. What an export's tail waits for, so a lookahead
    /// does not cut the last few milliseconds off a file.
    pub fn is_at_rest(&self) -> bool {
        self.reduction == 0.0
            && self.ring[..self.lookahead]
                .iter()
                .all(|frame| frame[0] == 0.0 && frame[1] == 0.0)
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
        if self.lookahead > 0 {
            return self.process_ahead(left, right);
        }
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

    /// The lookahead path: detect on the frame arriving, emit the one
    /// `lookahead` frames behind it. See the module documentation.
    fn process_ahead(&mut self, left: &mut [f32], right: &mut [f32]) -> GuardReport {
        let mut report = GuardReport::default();
        let window = self.lookahead;
        let span = window as f32;
        let ceiling = self.ceiling;
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
            let peak = a.abs().max(b.abs());
            if peak > ceiling {
                let over = u32::from(a.abs() > ceiling) + u32::from(b.abs() > ceiling);
                report.overs = report.overs.saturating_add(over);
                let needed = 1.0 - ceiling / peak;
                if needed > self.target {
                    // Arrive by the time this frame leaves, and never slower
                    // than the ramp already running, which is covering an
                    // earlier over that leaves sooner.
                    self.step = self.step.max((needed - self.reduction) / span);
                    self.target = needed;
                }
                // The hold starts when the over leaves, `window` from now.
                self.hold_left = self.hold_frames.saturating_add(window as u32);
            }
            if self.reduction < self.target {
                self.reduction = (self.reduction + self.step).min(self.target);
                if self.reduction >= self.target {
                    self.step = 0.0;
                }
            }
            if self.hold_left > 0 {
                self.hold_left -= 1;
            } else if self.reduction > 0.0 && self.reduction >= self.target {
                self.reduction *= self.release;
                if self.reduction < AT_REST {
                    self.reduction = 0.0;
                }
                self.target = self.reduction;
            }
            let [mut x, mut y] = self.ring[self.head];
            self.ring[self.head] = [a, b];
            self.head += 1;
            if self.head == window {
                self.head = 0;
            }
            if self.reduction > 0.0 {
                let gain = 1.0 - self.reduction;
                x = (x * gain).clamp(-ceiling, ceiling);
                y = (y * gain).clamp(-ceiling, ceiling);
            }
            *l = x;
            *r = y;
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
    ///
    /// The frames in flight come too when both look ahead by the same
    /// amount, so a swap neither drops nor repeats a few milliseconds of the
    /// mix.
    pub fn adopt(&mut self, other: &OutputGuard) {
        self.reduction = other.reduction;
        self.hold_left = other.hold_left.min(self.hold_frames);
        self.target = other.target;
        self.step = other.step;
        if self.lookahead == other.lookahead && self.ring.len() == other.ring.len() {
            self.ring.copy_from_slice(&other.ring);
            self.head = other.head;
        }
    }
}

/// `ms` of lookahead at `sample_rate`, in whole frames, clamped to
/// `0..=MAX_LOOKAHEAD_MS`.
pub fn lookahead_frames(ms: f32, sample_rate: u32) -> usize {
    let ms = if ms.is_finite() { ms.clamp(0.0, MAX_LOOKAHEAD_MS) } else { 0.0 };
    (ms * sample_rate as f32 / 1000.0).round() as usize
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

    /// MOO-169's promise: a lookahead of 0 is today's guard, bit for bit,
    /// including one that was set to look ahead and set back.
    #[test]
    fn a_lookahead_of_zero_is_the_zero_latency_guard_exactly() {
        let mut fresh = OutputGuard::new(RATE);
        let mut round_trip = OutputGuard::new(RATE);
        round_trip.set_lookahead_ms(3.0);
        assert_eq!(round_trip.latency_frames(), 144);
        round_trip.set_lookahead_ms(0.0);
        assert_eq!(round_trip.latency_frames(), 0);
        let mut a = (sine(3.0, 110.0, RATE as usize), sine(0.7, 440.0, RATE as usize));
        let mut b = a.clone();
        let report_a = run(&mut fresh, &mut a.0, &mut a.1);
        let report_b = run(&mut round_trip, &mut b.0, &mut b.1);
        assert_eq!(report_a, report_b);
        assert!(report_a.overs > 0, "the signal never went over, so this proves nothing");
        for (frame, (x, y)) in a.0.iter().zip(&b.0).enumerate() {
            assert_eq!(x.to_bits(), y.to_bits(), "left differs at {frame}");
        }
        for (frame, (x, y)) in a.1.iter().zip(&b.1).enumerate() {
            assert_eq!(x.to_bits(), y.to_bits(), "right differs at {frame}");
        }
    }

    /// Under the ceiling, a lookahead is a delay and nothing else.
    #[test]
    fn under_the_ceiling_a_lookahead_is_a_pure_delay() {
        let mut guard = OutputGuard::new(RATE);
        guard.set_lookahead_ms(1.5);
        let delay = guard.latency_frames() as usize;
        assert_eq!(delay, 72);
        let input_l = sine(0.99, 110.0, 8_192);
        let input_r = sine(0.5, 3_000.0, 8_192);
        let (mut left, mut right) = (input_l.clone(), input_r.clone());
        let report = run(&mut guard, &mut left, &mut right);
        assert_eq!(report, GuardReport::default());
        assert!(left[..delay].iter().chain(&right[..delay]).all(|&s| s == 0.0));
        for frame in delay..8_192 {
            assert_eq!(left[frame].to_bits(), input_l[frame - delay].to_bits(), "left {frame}");
            assert_eq!(right[frame].to_bits(), input_r[frame - delay].to_bits(), "right {frame}");
        }
        assert!(!guard.is_at_rest(), "frames still in flight");
        let (mut l, mut r) = (vec![0.0f32; delay], vec![0.0f32; delay]);
        guard.process(&mut l, &mut r);
        assert!(guard.is_at_rest());
    }

    /// The point of looking ahead: the ramp arrives before the over does, so
    /// nothing leaves above the ceiling and the final clamp does not have to
    /// do the limiting. Bursts of every height, including one far over.
    #[test]
    fn a_lookahead_turns_an_over_down_before_it_arrives() {
        let mut guard = OutputGuard::new(RATE);
        guard.set_lookahead_ms(1.5);
        let delay = guard.latency_frames() as usize;
        let frames = RATE as usize;
        let mut input = vec![0.0f32; frames];
        for (index, sample) in input.iter_mut().enumerate() {
            // A quiet tone, with a hard transient every 100 ms growing from
            // 1 dB to 30 dB over.
            let burst = index % 4_800;
            let height = 1.12 * (1.0 + (index / 4_800) as f32 * 1.6);
            let t = index as f32 / RATE as f32;
            *sample = 0.3 * (2.0 * core::f32::consts::PI * 220.0 * t).sin();
            if burst < 48 {
                *sample = height * if burst % 2 == 0 { 1.0 } else { -1.0 };
            }
        }
        // A frame at a time, so the gain each frame left under can be read
        // and the clamp's help ruled out: the source times that gain must
        // already be at or under the ceiling.
        let mut overs = 0;
        for frame in 0..frames {
            let (mut l, mut r) = ([input[frame]], [input[frame]]);
            overs += guard.process(&mut l, &mut r).overs;
            assert!(l[0].abs() <= OUTPUT_CEILING, "{} left the master at {frame}", l[0]);
            if frame >= delay {
                let source = input[frame - delay];
                let ducked = (source * guard.gain()).abs();
                assert!(
                    ducked <= OUTPUT_CEILING * (1.0 + 1e-5),
                    "frame {frame} needed the clamp: {source} under a gain of {}",
                    guard.gain()
                );
            }
        }
        assert!(overs > 0);
    }

    /// With no lookahead the first frame of an over is shaped by the clamp;
    /// with one, the frame before it is already turned down.
    #[test]
    fn a_lookahead_ducks_the_frames_before_an_over() {
        let mut ahead = OutputGuard::new(RATE);
        ahead.set_lookahead_ms(1.0);
        let delay = ahead.latency_frames() as usize;
        let mut left = vec![0.5f32; 400];
        left[200] = 4.0;
        let mut right = left.clone();
        ahead.process(&mut left, &mut right);
        // The over leaves at 200 + delay; the frame just before it is
        // already quieter than 0.5, which zero latency cannot do.
        assert!(left[199 + delay] < 0.5, "nothing was turned down ahead of the over");
        assert!((left[200 + delay] - 1.0).abs() < 1e-5, "the over left at {}", left[200 + delay]);
    }

    #[test]
    fn a_lookahead_still_scrubs_and_links() {
        let mut guard = OutputGuard::new(RATE);
        guard.set_lookahead_ms(0.5);
        let delay = guard.latency_frames() as usize;
        let mut left = vec![f32::NAN; 1];
        left.extend(vec![4.0f32; 200]);
        let mut right = vec![0.5f32; 201];
        let report = guard.process(&mut left, &mut right);
        assert_eq!(report.non_finite, 1);
        assert_eq!(left[delay], 0.0, "the NaN reached the ring");
        let last = 200;
        assert!((left[last] - 1.0).abs() < 1e-5);
        assert!((right[last] - 0.125).abs() < 1e-5, "right is {}", right[last]);
    }

    #[test]
    fn a_lookahead_is_clamped_to_its_range_and_counted_in_frames() {
        assert_eq!(lookahead_frames(0.0, 48_000), 0);
        assert_eq!(lookahead_frames(1.5, 48_000), 72);
        assert_eq!(lookahead_frames(5.0, 192_000), 960);
        assert_eq!(lookahead_frames(50.0, 48_000), 240);
        assert_eq!(lookahead_frames(f32::NAN, 48_000), 0);
        let mut guard = OutputGuard::new(44_100);
        guard.set_lookahead_ms(5.0);
        assert_eq!(guard.latency_frames(), 221, "5 ms at 44.1 kHz rounds to 220.5 -> 221");
    }

    #[test]
    fn a_replacement_guard_with_the_same_lookahead_keeps_the_frames_in_flight() {
        let mut running = OutputGuard::new(RATE);
        running.set_lookahead_ms(1.0);
        let mut left: Vec<f32> = (0..30).map(|n| n as f32 / 100.0).collect();
        let mut right = left.clone();
        running.process(&mut left, &mut right);
        let mut fresh = OutputGuard::new(RATE);
        fresh.set_lookahead_ms(1.0);
        fresh.adopt(&running);
        let (mut a, mut b) = (vec![0.0f32; 48], vec![0.0f32; 48]);
        let (mut c, mut d) = (vec![0.0f32; 48], vec![0.0f32; 48]);
        running.process(&mut a, &mut b);
        fresh.process(&mut c, &mut d);
        assert_eq!(a, c);
        assert!(a.iter().any(|&s| s != 0.0), "nothing was in flight");
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
