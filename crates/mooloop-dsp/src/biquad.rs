//! RBJ-cookbook biquad: peaking, shelving, and pass filters with normalized
//! coefficients.
//!
//! Reach for this over [`crate::filter::Svf`] when the shape needed is one
//! the cookbook already defines exactly (a parametric peak, a shelf, a
//! textbook Butterworth-Q pass stage) and coefficients only change at
//! sample-timed parameter boundaries — `EqEffect` is the reference caller,
//! recomputing a whole bank once per event rather than every sample. Reach
//! for `Svf` instead when cutoff needs to move continuously (envelope- or
//! LFO-modulated filters): a biquad's coefficients are only valid for the
//! frequency they were designed at, while `Svf` stays stable through a
//! sweep.

use crate::node::REST_EPSILON;

/// One RBJ-cookbook biquad section in Direct Form I, normalized so `a0` is
/// always 1.
#[derive(Clone, Copy)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    /// A pass-through stage: useful as the resting state for a bank of
    /// biquads where not every stage is active.
    pub const fn identity() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// Drop the filter's memory without disturbing its coefficients. For a
    /// chain reset, not a per-block call: zeroing state mid-signal is a
    /// discontinuity.
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    /// Whether this stage's stored samples are too small for anything audible
    /// to come out of it.
    ///
    /// With no input, [`Self::process`] returns `z1`, so a stage whose two
    /// stored samples are both under [`REST_EPSILON`] can only emit values
    /// under it. That is what lets a host stop calling a filter bank without
    /// changing where it comes back — see [`crate::node::AudioNode::is_at_rest`].
    pub fn is_at_rest(&self) -> bool {
        self.z1.abs() <= REST_EPSILON && self.z2.abs() <= REST_EPSILON
    }

    pub fn process(&mut self, input: f32) -> f32 {
        let out = self.b0 * input + self.z1;
        self.z1 = self.b1 * input - self.a1 * out + self.z2;
        self.z2 = self.b2 * input - self.a2 * out;
        out
    }

    /// Store cookbook coefficients normalized by `a0`.
    pub fn set_normalized(&mut self, b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) {
        let inv = a0.max(1e-12).recip();
        self.b0 = b0 * inv;
        self.b1 = b1 * inv;
        self.b2 = b2 * inv;
        self.a1 = a1 * inv;
        self.a2 = a2 * inv;
    }

    /// RBJ peaking EQ: boost or cut a band around `frequency` by `gain_db`.
    pub fn peak(&mut self, frequency: f32, q: f32, gain_db: f32, sample_rate: u32) {
        let w = core::f32::consts::TAU * frequency.clamp(20.0, sample_rate as f32 * 0.45)
            / sample_rate as f32;
        let alpha = w.sin() / (2.0 * q.clamp(0.15, 30.0));
        let a = 10.0_f32.powf(gain_db.clamp(-24.0, 24.0) / 40.0);
        self.set_normalized(
            1.0 + alpha * a,
            -2.0 * w.cos(),
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * w.cos(),
            1.0 - alpha / a,
        );
    }

    /// RBJ low- or high-shelf, boosting or cutting everything above/below
    /// `frequency` by `gain_db`.
    pub fn shelf(&mut self, frequency: f32, gain_db: f32, low: bool, sample_rate: u32) {
        let w = core::f32::consts::TAU * frequency.clamp(20.0, sample_rate as f32 * 0.45)
            / sample_rate as f32;
        let a = 10.0_f32.powf(gain_db.clamp(-24.0, 24.0) / 40.0);
        let alpha = w.sin() * 0.5 * (a + a.recip()).sqrt();
        let beta = 2.0 * a.sqrt() * alpha;
        let c = w.cos();
        if low {
            self.set_normalized(
                a * ((a + 1.0) - (a - 1.0) * c + beta),
                2.0 * a * ((a - 1.0) - (a + 1.0) * c),
                a * ((a + 1.0) - (a - 1.0) * c - beta),
                (a + 1.0) + (a - 1.0) * c + beta,
                -2.0 * ((a - 1.0) + (a + 1.0) * c),
                (a + 1.0) + (a - 1.0) * c - beta,
            );
        } else {
            self.set_normalized(
                a * ((a + 1.0) + (a - 1.0) * c + beta),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
                a * ((a + 1.0) + (a - 1.0) * c - beta),
                (a + 1.0) - (a - 1.0) * c + beta,
                2.0 * ((a - 1.0) - (a + 1.0) * c),
                (a + 1.0) - (a - 1.0) * c - beta,
            );
        }
    }

    /// RBJ low- or high-shelf with an explicit slope `s`, where 1 is the
    /// cookbook's own maximally-flat shelf, below 1 is gentler and wider, and
    /// above 1 steepens toward a resonant corner.
    ///
    /// **Why this is a second function rather than a parameter on
    /// [`Self::shelf`].** That one's `alpha` is `sin(w)/2 * sqrt(A + 1/A)`,
    /// which is the cookbook form at a slope that *varies with gain* — S = 1
    /// at 0 dB and about 0.83 at 12 dB, so its shelves widen as they are
    /// pushed. The seven-band EQ and `crate::preamp`'s tilt pair were both
    /// designed against that curve and both have tests on its numbers, so it
    /// is left exactly as it was; this is the form a face can put a slope
    /// knob in front of.
    ///
    /// It keeps the property `crate::preamp` depends on: `alpha` sees `A`
    /// only through `A + 1/A`, which is invariant under `A -> 1/A`, so a
    /// cut and a matching boost at the same slope are **exact** inverses.
    pub fn shelf_slope(
        &mut self,
        frequency: f32,
        gain_db: f32,
        s: f32,
        low: bool,
        sample_rate: u32,
    ) {
        let w = core::f32::consts::TAU * frequency.clamp(20.0, sample_rate as f32 * 0.45)
            / sample_rate as f32;
        let a = 10.0_f32.powf(gain_db.clamp(-24.0, 24.0) / 40.0);
        // Clamped low as well as high: the cookbook's radicand goes negative
        // for large S at high gain, which would produce NaN coefficients and
        // silence the stage rather than steepen it.
        let s = s.clamp(0.1, 2.0);
        let radicand = (a + a.recip()) * (s.recip() - 1.0) + 2.0;
        let alpha = w.sin() * 0.5 * radicand.max(0.0).sqrt();
        let beta = 2.0 * a.sqrt() * alpha;
        let c = w.cos();
        if low {
            self.set_normalized(
                a * ((a + 1.0) - (a - 1.0) * c + beta),
                2.0 * a * ((a - 1.0) - (a + 1.0) * c),
                a * ((a + 1.0) - (a - 1.0) * c - beta),
                (a + 1.0) + (a - 1.0) * c + beta,
                -2.0 * ((a - 1.0) + (a + 1.0) * c),
                (a + 1.0) + (a - 1.0) * c - beta,
            );
        } else {
            self.set_normalized(
                a * ((a + 1.0) + (a - 1.0) * c + beta),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
                a * ((a + 1.0) + (a - 1.0) * c - beta),
                (a + 1.0) - (a - 1.0) * c + beta,
                2.0 * ((a - 1.0) - (a + 1.0) * c),
                (a + 1.0) - (a - 1.0) * c - beta,
            );
        }
    }

    /// RBJ high- or low-pass, one Butterworth-Q stage.
    pub fn pass(&mut self, frequency: f32, q: f32, high: bool, sample_rate: u32) {
        let w = core::f32::consts::TAU * frequency.clamp(20.0, sample_rate as f32 * 0.45)
            / sample_rate as f32;
        let alpha = w.sin() / (2.0 * q.clamp(0.15, 30.0));
        let c = w.cos();
        if high {
            self.set_normalized(
                (1.0 + c) * 0.5,
                -(1.0 + c),
                (1.0 + c) * 0.5,
                1.0 + alpha,
                -2.0 * c,
                1.0 - alpha,
            );
        } else {
            self.set_normalized(
                (1.0 - c) * 0.5,
                1.0 - c,
                (1.0 - c) * 0.5,
                1.0 + alpha,
                -2.0 * c,
                1.0 - alpha,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    fn respond(mut filter: Biquad, freq_hz: f32, sample_rate: u32) -> f32 {
        let frames = sample_rate as usize;
        let mut samples = Vec::with_capacity(frames);
        for i in 0..frames {
            let t = i as f32 / sample_rate as f32;
            let input = (t * freq_hz * core::f32::consts::TAU).sin();
            let out = filter.process(input);
            if i > frames / 2 {
                samples.push(out);
            }
        }
        rms(&samples)
    }

    #[test]
    fn identity_passes_a_signal_unchanged() {
        let mut filter = Biquad::identity();
        for i in 0..64 {
            let input = (i as f32 * 0.37).sin();
            assert!((filter.process(input) - input).abs() < 1e-6);
        }
    }

    #[test]
    fn peak_boosts_the_target_frequency() {
        let sr = 48_000;
        let mut boosted = Biquad::identity();
        boosted.peak(1_000.0, 1.0, 12.0, sr);
        let mut flat = Biquad::identity();
        flat.peak(1_000.0, 1.0, 0.0, sr);
        let boosted_rms = respond(boosted, 1_000.0, sr);
        let flat_rms = respond(flat, 1_000.0, sr);
        assert!(
            boosted_rms > flat_rms * 1.5,
            "boosted {boosted_rms} should exceed flat {flat_rms}"
        );
    }

    #[test]
    fn low_pass_attenuates_high_frequencies() {
        let sr = 48_000;
        let mut filter = Biquad::identity();
        filter.pass(1_000.0, 0.707, false, sr);
        let low = respond(filter, 200.0, sr);
        let mut filter = Biquad::identity();
        filter.pass(1_000.0, 0.707, false, sr);
        let high = respond(filter, 8_000.0, sr);
        assert!(
            low > high * 4.0,
            "low {low} should pass far more than high {high}"
        );
    }

    /// The slope knob has to do something, and in the direction it says: a
    /// gentler shelf has already given away less of its boost an octave
    /// below the corner than a steep one has.
    #[test]
    fn a_gentler_shelf_slope_reaches_further_past_its_corner() {
        let sr = 48_000;
        let mut steep = Biquad::identity();
        steep.shelf_slope(1_000.0, 12.0, 2.0, false, sr);
        let mut gentle = Biquad::identity();
        gentle.shelf_slope(1_000.0, 12.0, 0.15, false, sr);
        let steep_below = respond(steep, 250.0, sr);
        let gentle_below = respond(gentle, 250.0, sr);
        assert!(
            gentle_below > steep_below * 1.3,
            "gentle {gentle_below} should still be lifting 250 Hz where steep {steep_below} has let go"
        );
    }

    /// The property `crate::preamp`'s filter sandwich rests on, stated for
    /// the sloped form as well: a cut and a matching boost at the same slope
    /// cancel, because `alpha` sees the gain only through `A + 1/A`.
    #[test]
    fn a_sloped_shelf_and_its_inverse_cancel() {
        let sr = 48_000;
        for slope in [0.3f32, 1.0, 1.7] {
            let mut down = Biquad::identity();
            let mut up = Biquad::identity();
            down.shelf_slope(300.0, -9.0, slope, false, sr);
            up.shelf_slope(300.0, 9.0, slope, false, sr);
            for i in 0..2_000 {
                let input = (i as f32 * 0.11).sin() * 0.7;
                let out = up.process(down.process(input));
                assert!(
                    (out - input).abs() < 1e-4,
                    "slope {slope} frame {i}: {out} should be {input}"
                );
            }
        }
    }

    #[test]
    fn shelf_boosts_its_side_and_leaves_the_other_alone() {
        let sr = 48_000;
        let mut low_shelf = Biquad::identity();
        low_shelf.shelf(1_000.0, 12.0, true, sr);
        let low = respond(low_shelf, 100.0, sr);
        let flat = Biquad::identity();
        let flat_low = respond(flat, 100.0, sr);
        assert!(
            low > flat_low * 1.5,
            "low-shelf should raise low-side energy: {low} vs {flat_low}"
        );
    }
}
