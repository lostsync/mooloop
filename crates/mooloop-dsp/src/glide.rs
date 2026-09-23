//! Portamento: a pitch that slides to its target in log frequency and arrives
//! in the time it was given.
//!
//! ```text
//! Glide
//! in:  target pitch (Hz), glide time (s)
//! out: pitch (Hz), one value per sample
//! ```
//!
//! Until 2026-09-22 each of the four synths carried its own copy of a
//! one-pole on frequency in Hz, `current += (target - current) * (1 - coeff)`
//! with `coeff = exp(-1 / (glide * sr))` (MOO-144, MOO-145). That had two
//! faults, both audible:
//!
//! - **It slid in Hz.** An octave up and an octave down traced different
//!   curves in pitch: the Hz gap is twice as wide on the way up, so the ear
//!   heard the upward slide linger at the bottom and the downward one drop
//!   away first. Portamento sounded lopsided.
//! - **The Glide time was the lag's time constant**, so a 100 ms glide came
//!   within 1% of its target only after about 4.6 of them -- roughly 460 ms.
//!
//! This slides in octaves, linearly, for exactly the glide time: an octave up
//! and an octave down are mirror images, and a 100 ms glide arrives in
//! 100 ms. Linear in log frequency is constant-time portamento -- every
//! interval takes the Glide time, which is what the knob is labelled as.
//!
//! The position is computed from the elapsed sample count rather than
//! accumulated, so a long slide carries no rounding drift, and it lands on the
//! target exactly.

/// A pitch that slides.
#[derive(Clone, Copy, Debug)]
pub struct Glide {
    /// Where the current slide started, in octaves (`log2` of Hz).
    from: f32,
    /// Where it ends, in octaves.
    to: f32,
    /// Samples the slide lasts, and how many of them have passed. `elapsed ==
    /// length` means arrived.
    length: u32,
    elapsed: u32,
    /// The pitch this sample, in Hz. Held so an arrived glide costs nothing
    /// per sample.
    hz: f32,
    /// The target exactly as it was asked for, so [`Self::follow`] can tell a
    /// new note from the one it is already heading for without a round trip
    /// through `log2`.
    target_hz: f32,
}

impl Glide {
    /// A glide resting at `hz`.
    pub fn new(hz: f32) -> Self {
        let octave = to_octave(hz);
        Self {
            from: octave,
            to: octave,
            length: 0,
            elapsed: 0,
            hz: hz.max(MIN_HZ),
            target_hz: hz,
        }
    }

    /// Move to `hz` at once, with no slide.
    pub fn jump_to(&mut self, hz: f32) {
        *self = Self::new(hz);
    }

    /// Slide from wherever the pitch is now to `hz`, arriving `time_s` from
    /// now. A slide shorter than one sample is a jump.
    ///
    /// A new slide starting mid-slide starts from the pitch the old one had
    /// reached, and takes the whole of its own time: every note change takes
    /// the Glide time, however it interrupts the last.
    pub fn slide_to(&mut self, hz: f32, time_s: f32, sample_rate: u32) {
        let length = (time_s.max(0.0) * sample_rate as f32).round();
        if length.is_nan() || length < 1.0 {
            self.jump_to(hz);
            return;
        }
        self.from = to_octave(self.hz);
        self.to = to_octave(hz);
        self.target_hz = hz;
        self.length = length.min(u32::MAX as f32) as u32;
        self.elapsed = 0;
    }

    /// Advance one sample and return the pitch for it, in Hz.
    ///
    /// The first sample of a slide is already one step along it, so a slide
    /// of `n` samples reaches its target on the `n`th call and holds it
    /// after.
    pub fn advance(&mut self) -> f32 {
        if self.elapsed < self.length {
            self.elapsed += 1;
            if self.elapsed == self.length {
                self.from = self.to;
                self.hz = self.to.exp2();
            } else {
                let t = self.elapsed as f32 / self.length as f32;
                self.hz = (self.from + (self.to - self.from) * t).exp2();
            }
        }
        self.hz
    }

    /// Advance `frames` samples without reading each one. Where the pitch is
    /// afterwards is exactly where `frames` calls to [`Self::advance`] leave it.
    pub fn skip(&mut self, frames: usize) {
        if self.elapsed >= self.length {
            return;
        }
        let remaining = (self.length - self.elapsed) as usize;
        if frames >= remaining {
            self.elapsed = self.length;
            self.from = self.to;
            self.hz = self.to.exp2();
        } else if frames > 0 {
            self.elapsed += frames as u32;
            let t = self.elapsed as f32 / self.length as f32;
            self.hz = (self.from + (self.to - self.from) * t).exp2();
        }
    }

    /// The pitch now, in Hz.
    pub fn hz(&self) -> f32 {
        self.hz
    }

    /// The pitch the glide is heading for, in Hz, exactly as it was given.
    pub fn target_hz(&self) -> f32 {
        self.target_hz
    }

    /// Head for `target_hz` if that is a new target, then advance one sample:
    /// the per-sample call for a voice that keeps its note as a plain target
    /// frequency. A changed target starts a slide of `time_s` from wherever
    /// the pitch is now (or jumps, for a glide shorter than a sample); an
    /// unchanged one carries on. A voice that must *not* glide to its new
    /// note -- a fresh start, a note landing on silence -- calls
    /// [`Self::jump_to`] first.
    pub fn follow(&mut self, target_hz: f32, time_s: f32, sample_rate: u32) -> f32 {
        if target_hz != self.target_hz {
            self.slide_to(target_hz, time_s, sample_rate);
        }
        self.advance()
    }

    /// Whether a slide is still under way.
    pub fn is_sliding(&self) -> bool {
        self.elapsed < self.length
    }
}

impl Default for Glide {
    fn default() -> Self {
        Self::new(440.0)
    }
}

/// The lowest pitch a glide will hold. `log2` of zero is `-inf`, and a slide
/// from `-inf` would be a slide from nowhere.
const MIN_HZ: f32 = 1.0e-3;

fn to_octave(hz: f32) -> f32 {
    hz.max(MIN_HZ).log2()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{cents, RATES};

    fn slide(from_hz: f32, to_hz: f32, time_s: f32, sample_rate: u32, frames: usize) -> Vec<f32> {
        let mut glide = Glide::new(from_hz);
        glide.slide_to(to_hz, time_s, sample_rate);
        (0..frames).map(|_| glide.advance()).collect()
    }

    /// **The asymmetry MOO-145 was filed for.** An octave up and an octave down
    /// take the same time, and at every sample the upward slide has covered
    /// exactly the share of the octave the downward one has -- mirror images
    /// in pitch, at every rate.
    #[test]
    fn an_octave_up_and_an_octave_down_are_mirror_images() {
        for sr in RATES {
            let frames = (0.1 * sr as f32) as usize + 10;
            let up = slide(220.0, 440.0, 0.1, sr, frames);
            let down = slide(440.0, 220.0, 0.1, sr, frames);
            for (index, (u, d)) in up.iter().zip(&down).enumerate() {
                let risen = cents(*u, 220.0);
                let fallen = cents(440.0, *d);
                assert!(
                    (risen - fallen).abs() < 0.01,
                    "{sr} Hz, sample {index}: up {risen} cents, down {fallen} cents"
                );
            }
        }
    }

    /// **The other half of MOO-145.** A 100 ms glide is at its target, to
    /// within a hundredth of a cent, after 100 ms -- not after the 460 ms the
    /// one-pole took -- and has not yet arrived one sample before.
    #[test]
    fn a_100_ms_glide_arrives_in_100_ms() {
        for sr in RATES {
            let arrive = (0.1 * sr as f32).round() as usize;
            let pitches = slide(110.0, 880.0, 0.1, sr, arrive + 100);
            // Three octaves over `arrive` samples is 3600 / arrive cents a
            // sample: 0.19 cents at 192 kHz, so two samples out it is still
            // measurably short.
            let late = cents(pitches[arrive - 2], 880.0);
            assert!(late.abs() > 0.1, "{sr} Hz: already arrived a sample early ({late} cents)");
            for (index, pitch) in pitches.iter().enumerate().skip(arrive - 1) {
                assert!(
                    cents(*pitch, 880.0).abs() < 0.01,
                    "{sr} Hz: {pitch} Hz at sample {index}, after the glide time"
                );
            }
        }
    }

    /// Linear in pitch: halfway through the time is halfway through the
    /// interval in cents, whatever the interval.
    #[test]
    fn the_slide_is_linear_in_pitch() {
        let sr = 48_000;
        for (from, to) in [(100.0_f32, 1_600.0_f32), (1_000.0, 950.0), (300.0, 75.0)] {
            let pitches = slide(from, to, 0.2, sr, 9_600);
            let halfway = pitches[4_799];
            let expected = cents(to, from) / 2.0;
            assert!(
                (cents(halfway, from) - expected).abs() < 0.05,
                "{from} -> {to}: halfway at {} cents, wanted {expected}",
                cents(halfway, from)
            );
        }
    }

    /// Retargeting mid-slide continues from where the pitch had got to: no
    /// step, whichever way the new note lies.
    #[test]
    fn a_new_slide_mid_slide_starts_where_the_last_one_had_got_to() {
        let sr = 48_000;
        let mut glide = Glide::new(220.0);
        glide.slide_to(880.0, 0.1, sr);
        let mut last = 0.0;
        for _ in 0..2_000 {
            last = glide.advance();
        }
        glide.slide_to(110.0, 0.1, sr);
        let first = glide.advance();
        assert!(cents(first, last).abs() < 5.0, "stepped {last} -> {first} Hz");
        assert!(first < last);
    }

    /// `follow` slides only when the target changes, and a jump beforehand
    /// means no slide at all.
    #[test]
    fn follow_slides_on_a_new_target_only() {
        let sr = 48_000;
        let mut glide = Glide::new(220.0);
        assert_eq!(glide.follow(220.0, 0.1, sr), 220.0);
        let first = glide.follow(440.0, 0.1, sr);
        assert!(first > 220.0 && first < 230.0, "{first}");
        for _ in 0..4_799 {
            glide.follow(440.0, 0.1, sr);
        }
        assert!(cents(glide.hz(), 440.0).abs() < 0.01);
        glide.jump_to(110.0);
        assert_eq!(glide.follow(110.0, 0.1, sr), 110.0);
    }

    #[test]
    fn a_zero_glide_is_a_jump_and_skip_matches_walking() {
        let sr = 48_000;
        let mut glide = Glide::new(220.0);
        glide.slide_to(330.0, 0.0, sr);
        assert_eq!(glide.hz(), 330.0);
        assert!(!glide.is_sliding());

        let mut walked = Glide::new(220.0);
        walked.slide_to(660.0, 0.05, sr);
        let mut skipped = walked;
        for _ in 0..777 {
            walked.advance();
        }
        skipped.skip(777);
        assert_eq!(walked.hz(), skipped.hz());
        skipped.skip(10_000);
        assert_eq!(skipped.hz(), (660.0_f32).log2().exp2());
        assert!(!skipped.is_sliding());
    }
}
