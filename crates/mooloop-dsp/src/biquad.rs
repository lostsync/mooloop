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

use mooloop_core::{eq_effective_q, EqBandKind, EqQProfile};

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

    /// This stage's gain at `frequency_hz`, in decibels.
    ///
    /// The **exact** response of the coefficients that are loaded --
    /// `20 log10 |H(e^{jw})|` for the transfer function this stage is
    /// running -- rather than an approximation of the shape it was designed
    /// for. That distinction is the whole reason this exists: a response plot
    /// that evaluates its own idea of what a bell looks like is a second law
    /// drawn next to the first, and `mooloop_core::eq_effective_q` already
    /// carries this codebase's sentence about which copy is the one deciding
    /// what is heard. A plot fed from here cannot disagree with the filter,
    /// because it is reading the filter.
    ///
    /// It follows `process`'s own normalization -- `a0` is 1 -- so
    /// [`Self::identity`] answers 0 dB and a bank can sum these.
    pub fn magnitude_db(&self, frequency_hz: f32, sample_rate: u32) -> f32 {
        let w = core::f32::consts::TAU * frequency_hz / sample_rate as f32;
        let (sin1, cos1) = w.sin_cos();
        let (sin2, cos2) = (2.0 * w).sin_cos();
        let num_re = self.b0 + self.b1 * cos1 + self.b2 * cos2;
        let num_im = -(self.b1 * sin1 + self.b2 * sin2);
        let den_re = 1.0 + self.a1 * cos1 + self.a2 * cos2;
        let den_im = -(self.a1 * sin1 + self.a2 * sin2);
        let num = num_re * num_re + num_im * num_im;
        let den = den_re * den_re + den_im * den_im;
        // Halved because these are squared magnitudes: 20 log10 |H| is
        // 10 log10 |H|^2, and taking the two square roots to say it the other
        // way would be arithmetic for nothing.
        10.0 * (num.max(1e-24) / den.max(1e-24)).log10()
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
        self.set_shelf(a, 2.0 * a.sqrt() * alpha, w.cos(), low);
    }

    /// The cookbook's shelf coefficients, given the gain, the `beta` the
    /// caller's slope law produced, and `cos(w)`.
    ///
    /// Shared by [`Self::shelf`] and [`Self::shelf_slope`] because the two
    /// differ **only** in how they reach `alpha` — the twelve coefficient
    /// expressions below were byte-identical in both, which is copied
    /// arithmetic doing nothing but waiting to be edited once.
    fn set_shelf(&mut self, a: f32, beta: f32, c: f32, low: bool) {
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
        self.set_shelf(a, 2.0 * a.sqrt() * alpha, w.cos(), low);
    }

    /// Design one EQ band's stage: a bell at its effective Q, or a shelf
    /// whose `q` is its slope.
    ///
    /// **Two banks run these laws and they must not be two copies of them.**
    /// The channel strip's four bands and the seven-band effect EQ design the
    /// same three kinds from the same fields, and on 2026-09-14 they were the
    /// same `match` written twice in two files -- with the effect's arm
    /// calling `shelf`, which takes no Q, so a shelf's Q knob did nothing
    /// there and did something on the strip. `eq-v2/03` asked for a test
    /// holding the two together; one function is the stronger form of the
    /// same answer, because there is nothing left to hold apart.
    ///
    /// What stays with the callers is what genuinely differs: the strip
    /// resolves a band's frequency from a *position* through its voicing, and
    /// takes the Q profile from that voicing, while the effect EQ has a
    /// frequency and a per-band profile. Those are the two banks' own
    /// vocabulary. The law is this.
    pub fn eq_band(
        &mut self,
        kind: EqBandKind,
        frequency_hz: f32,
        gain_db: f32,
        q: f32,
        q_profile: EqQProfile,
        sample_rate: u32,
    ) {
        match kind {
            EqBandKind::Bell => self.peak(
                frequency_hz,
                eq_effective_q(q, gain_db, q_profile),
                gain_db,
                sample_rate,
            ),
            EqBandKind::LowShelf => self.shelf_slope(frequency_hz, gain_db, q, true, sample_rate),
            EqBandKind::HighShelf => self.shelf_slope(frequency_hz, gain_db, q, false, sample_rate),
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

    /// The gain a sine actually came out at, in dB. A unit sine has an RMS of
    /// `1/sqrt(2)`, so this is the measured amplitude referred to the input's.
    fn measured_db(filter: Biquad, freq_hz: f32, sample_rate: u32) -> f32 {
        20.0 * (respond(filter, freq_hz, sample_rate) * core::f32::consts::SQRT_2).log10()
    }

    /// **`magnitude_db` is held to a sine going through the filter**, not to a
    /// second derivation of the same formula -- which is the only way to check
    /// a claim of the form "this is what the stage does". Every shape the
    /// cookbook gives us, across the audible decades, at the tenth of a
    /// decibel a plot could show.
    #[test]
    fn the_computed_magnitude_is_what_a_sine_measures() {
        let sr = 48_000;
        let mut cases: Vec<(&str, Biquad)> = Vec::new();
        for (name, build) in [
            ("bell +12", &(|f: &mut Biquad, sr| f.peak(1_000.0, 1.0, 12.0, sr)) as &dyn Fn(&mut Biquad, u32)),
            ("bell -9 narrow", &|f: &mut Biquad, sr| f.peak(2_500.0, 6.0, -9.0, sr)),
            ("low shelf +6", &|f: &mut Biquad, sr| f.shelf_slope(200.0, 6.0, 0.8, true, sr)),
            ("high shelf -6 steep", &|f: &mut Biquad, sr| f.shelf_slope(6_000.0, -6.0, 1.6, false, sr)),
            ("high pass", &|f: &mut Biquad, sr| f.pass(300.0, 0.707, true, sr)),
            ("low pass resonant", &|f: &mut Biquad, sr| f.pass(4_000.0, 4.0, false, sr)),
        ] {
            let mut filter = Biquad::identity();
            build(&mut filter, sr);
            cases.push((name, filter));
        }

        for (name, filter) in cases {
            for freq in [50.0, 120.0, 400.0, 1_000.0, 2_500.0, 6_000.0, 12_000.0] {
                let computed = filter.magnitude_db(freq, sr);
                let measured = measured_db(filter, freq, sr);
                assert!(
                    (computed - measured).abs() < 0.1,
                    "{name} at {freq} Hz: computed {computed} dB, measured {measured} dB"
                );
            }
        }
    }

    /// **What `eq-v2`'s step 03 did to a shelf that was already boosted**, in
    /// decibels, so the listening pass it asks for knows what to listen for.
    ///
    /// The seven-band EQ called [`Biquad::shelf`] until 2026-09-14 and calls
    /// [`Biquad::shelf_slope`] now. Both are the cookbook's shelf and they
    /// differ only in how they reach `alpha` -- `shelf` uses
    /// `sqrt(A + 1/A)`, which is a slope that *varies with gain*, and
    /// `shelf_slope` uses the knob. So the two agree exactly at 0 dB and
    /// nowhere else, and "it sounds different" is true but useless without a
    /// number.
    ///
    /// Run it deliberately; it measures rather than asserts. The bound that
    /// *is* asserted is in `the_shelf_law_change_pivots_about_the_corner`
    /// below, which is the claim this table supports.
    #[test]
    #[ignore = "prints a listening brief; run deliberately"]
    fn shelf_law_change_in_decibels() {
        let sr = 48_000;
        let fc = 200.0;
        println!();
        println!("  a {fc} Hz low shelf, old law (`shelf`) against new (`shelf_slope`)");
        println!("  at the 0.707 both shelves rest at, in dB, new minus old");
        println!();
        print!("  gain  ");
        for octave in -3..=3 {
            print!("{:>9}", format!("{:.0}Hz", fc * 2.0_f32.powi(octave)));
        }
        println!();
        for gain in [-12.0, -6.0, -3.0, 3.0, 6.0, 12.0] {
            print!("  {gain:>4}  ");
            for octave in -3..=3 {
                let hz = fc * 2.0_f32.powi(octave);
                let mut old = Biquad::identity();
                old.shelf(fc, gain, true, sr);
                let mut new = Biquad::identity();
                new.shelf_slope(fc, gain, 0.707, true, sr);
                print!(
                    "{:>9.2}",
                    new.magnitude_db(hz, sr) - old.magnitude_db(hz, sr)
                );
            }
            println!();
        }
        println!();

        // The other axis, and the one that matters: the knob was *ignored*
        // before, so a patch that moved it was rendered at the old law
        // whatever it said. Now it is the slope.
        println!("  the same shelf at +6 dB, across the Q knob's travel");
        println!("  (the knob did nothing at all before 2026-09-14)");
        println!();
        print!("  Q     ");
        for octave in -3..=3 {
            print!("{:>9}", format!("{:.0}Hz", fc * 2.0_f32.powi(octave)));
        }
        println!();
        for q in [0.15_f32, 0.35, 0.707, 1.4, 4.0, 18.0] {
            print!("  {q:>4}  ");
            for octave in -3..=3 {
                let hz = fc * 2.0_f32.powi(octave);
                let mut old = Biquad::identity();
                old.shelf(fc, 6.0, true, sr);
                let mut new = Biquad::identity();
                new.shelf_slope(fc, 6.0, q, true, sr);
                print!(
                    "{:>9.2}",
                    new.magnitude_db(hz, sr) - old.magnitude_db(hz, sr)
                );
            }
            println!();
        }
        println!();
    }

    /// **The shelf law change pivots a shelf about its corner; it does not
    /// move the shelf.**
    ///
    /// Three things have to hold for that sentence, and the third is the one
    /// the measurement taught rather than confirmed: the two laws agree
    /// *exactly* at the corner frequency. A cookbook shelf passes through half
    /// its gain at `w0` whatever `alpha` is, so the only thing a slope can
    /// change is how fast it gets there. The first version of this test
    /// asserted the opposite, and the table above is what corrected it.
    ///
    /// Far out on either side they agree too -- a shelf is its gain and unity
    /// there -- so the whole of the difference lives in the octave or two
    /// around the corner. That is what lets `eq-v2/00-status.md` tell a
    /// listener where to listen instead of "it sounds different".
    #[test]
    fn the_shelf_law_change_pivots_about_the_corner() {
        let sr = 48_000;
        let fc = 200.0;
        for gain in [-12.0_f32, -6.0, -3.0, 3.0, 6.0, 12.0] {
            let mut previous = Biquad::identity();
            previous.shelf(fc, gain, true, sr);
            let mut current = Biquad::identity();
            current.shelf_slope(fc, gain, 0.707, true, sr);
            let delta = |hz| current.magnitude_db(hz, sr) - previous.magnitude_db(hz, sr);

            // The plateau and the untouched side.
            for hz in [fc / 16.0, fc * 16.0] {
                assert!(
                    delta(hz).abs() < 0.1,
                    "a {gain} dB shelf moved by {} dB four octaves from its corner",
                    delta(hz)
                );
            }
            // The corner itself: both laws pass through half the gain.
            assert!(
                delta(fc).abs() < 1.0e-3,
                "the two laws disagree at the corner by {} dB",
                delta(fc)
            );
            // An octave out they differ, or step 03 changed nothing and its
            // status file is wrong to ask for a listen.
            assert!(
                delta(fc / 2.0).abs() > 0.05 && delta(fc * 2.0).abs() > 0.05,
                "a {gain} dB shelf is unchanged an octave from its corner"
            );
            // Antisymmetric: what one side loses the other gains.
            assert!(
                (delta(fc / 2.0) + delta(fc * 2.0)).abs() < 0.02,
                "the pivot is lopsided: {} below, {} above",
                delta(fc / 2.0),
                delta(fc * 2.0)
            );
        }
    }

    /// **Where the shelf Q knob stops doing anything, as a fraction of its own
    /// travel.**
    ///
    /// `shelf_slope` clamps the slope at 2.0 and the seven-band EQ's Q
    /// descriptor runs 0.15..18 exponentially, because the same id has to
    /// serve that band as a bell. So the knob saturates part way along and the
    /// rest of it is one filter. `LOOSE_ENDS.md` carried that as "the top
    /// four-fifths", which was estimated rather than measured and is nearly
    /// twice the truth.
    ///
    /// Pinned because the fraction is a *product* of two numbers that live in
    /// different crates -- the clamp here and the descriptor range in
    /// `mooloop_core` -- so either one moving silently changes what a face is
    /// promising, and neither one looks like it has anything to do with a
    /// knob's travel.
    #[test]
    fn the_shelf_q_knob_saturates_a_little_past_half_its_travel() {
        let sr = 48_000;
        // The descriptor's range, from the one table that states it.
        let descriptor = mooloop_core::EffectKind::Eq
            .descriptor(mooloop_core::eq_band_param(0, mooloop_core::EQ_BAND_Q))
            .expect("a band has a Q descriptor");
        let (min, max) = (descriptor.min, descriptor.max);
        assert_eq!((min, max), (0.15, 18.0));

        // Everything from the clamp upward is the same filter.
        const SLOPE_CEILING: f32 = 2.0;
        let mut at_ceiling = Biquad::identity();
        at_ceiling.shelf_slope(200.0, 6.0, SLOPE_CEILING, true, sr);
        let mut at_top = Biquad::identity();
        at_top.shelf_slope(200.0, 6.0, max, true, sr);
        for hz in [50.0, 200.0, 800.0] {
            assert!(
                (at_ceiling.magnitude_db(hz, sr) - at_top.magnitude_db(hz, sr)).abs() < 1.0e-4,
                "the knob is still moving the filter above its clamp"
            );
        }
        // And just below it, it is not.
        let mut below = Biquad::identity();
        below.shelf_slope(200.0, 6.0, SLOPE_CEILING * 0.7, true, sr);
        assert!((below.magnitude_db(100.0, sr) - at_ceiling.magnitude_db(100.0, sr)).abs() > 0.05);

        // Which is 46% of the travel, not the 80% that was written down.
        let dead = 1.0 - (SLOPE_CEILING / min).ln() / (max / min).ln();
        assert!(
            (0.45..0.47).contains(&dead),
            "the dead fraction of the shelf Q knob is {dead}, and \
             `LOOSE_ENDS.md` states a figure that has to move with it"
        );
    }

    /// A pass-through stage is 0 dB everywhere, which is what lets a bank sum
    /// the stages it is not running without a special case.
    #[test]
    fn identity_is_flat_at_zero_db() {
        for freq in [20.0, 1_000.0, 20_000.0] {
            assert!(Biquad::identity().magnitude_db(freq, 48_000).abs() < 1e-5);
        }
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

    /// A shelf's `q` reaches its curve.
    ///
    /// The thing this pins is not the arithmetic -- `shelf_slope` has its own
    /// tests -- but that `eq_band` *passes the Q through* on the shelf arms.
    /// The seven-band EQ called `shelf`, which takes no Q, until 2026-09-14,
    /// so its Q knob moved nothing at all while a band was a shelf, and no
    /// test in six hundred noticed.
    #[test]
    fn a_shelfs_q_changes_its_curve() {
        let sr = 48_000;
        let mut gentle = Biquad::identity();
        let mut steep = Biquad::identity();
        gentle.eq_band(EqBandKind::LowShelf, 1_000.0, 12.0, 0.4, EqQProfile::Constant, sr);
        steep.eq_band(EqBandKind::LowShelf, 1_000.0, 12.0, 1.8, EqQProfile::Constant, sr);
        // Deliberately *not* at the corner: a shelf passes through the
        // geometric mean of its two asymptotes there whatever its slope, so
        // 1 kHz is the one frequency at which these two agree. The slope is
        // the shape of the transition, so the probe goes into it.
        let probe = 2_000.0;
        let gentle_gain = respond(gentle, probe, sr);
        let steep_gain = respond(steep, probe, sr);
        assert!(
            (gentle_gain - steep_gain).abs() > 1e-3,
            "the shelf's slope did not reach its curve: {gentle_gain} vs {steep_gain}"
        );
    }

    /// A flat shelf is flat at every slope, which is what makes calling
    /// `shelf_slope` instead of `shelf` safe for a *default* EQ: both of its
    /// shelves rest at 0 dB. A song that boosted one does change.
    #[test]
    fn a_shelf_at_unity_is_flat_whatever_its_slope() {
        let sr = 48_000;
        for slope in [0.2, 0.707, 1.0, 2.0] {
            let mut stage = Biquad::identity();
            stage.eq_band(EqBandKind::HighShelf, 4_000.0, 0.0, slope, EqQProfile::Constant, sr);
            for probe in [100.0, 1_000.0, 8_000.0] {
                // Against the identity's own answer, because `respond` is an
                // RMS and a unit sine's is 0.707 rather than 1.
                let gain = respond(stage, probe, sr);
                let flat = respond(Biquad::identity(), probe, sr);
                assert!(
                    (gain - flat).abs() < 1e-4,
                    "slope {slope} at {probe} Hz is {gain}, not the flat {flat}"
                );
            }
        }
    }

    /// A proportional bell narrows as it is pushed, by the core law rather
    /// than by a copy of it. There was a private copy in `effects::eq` until
    /// 2026-09-14 -- byte-identical, therefore green, and exactly the shape
    /// `AGENTS.md` names as the one that diverges silently.
    #[test]
    fn a_proportional_bell_narrows_as_it_is_pushed() {
        let sr = 48_000;
        let mut gentle = Biquad::identity();
        let mut hard = Biquad::identity();
        gentle.eq_band(EqBandKind::Bell, 1_000.0, 3.0, 1.0, EqQProfile::Proportional, sr);
        hard.eq_band(EqBandKind::Bell, 1_000.0, 12.0, 1.0, EqQProfile::Proportional, sr);
        // An octave out, the harder-pushed band has narrowed, so it is doing
        // proportionally *less* there than its gain alone would suggest.
        let skirt_gentle = respond(gentle, 2_000.0, sr) / respond(gentle, 1_000.0, sr);
        let skirt_hard = respond(hard, 2_000.0, sr) / respond(hard, 1_000.0, sr);
        assert!(
            skirt_hard < skirt_gentle,
            "a boosted proportional bell did not narrow: {skirt_hard} vs {skirt_gentle}"
        );

        // And a constant-Q band does not narrow, which is the other half of
        // the law being real rather than always-on.
        let mut constant_gentle = Biquad::identity();
        let mut constant_hard = Biquad::identity();
        constant_gentle.eq_band(EqBandKind::Bell, 1_000.0, 3.0, 1.0, EqQProfile::Constant, sr);
        constant_hard.eq_band(EqBandKind::Bell, 1_000.0, 12.0, 1.0, EqQProfile::Constant, sr);
        let flat_gentle =
            respond(constant_gentle, 2_000.0, sr) / respond(constant_gentle, 1_000.0, sr);
        let flat_hard = respond(constant_hard, 2_000.0, sr) / respond(constant_hard, 1_000.0, sr);
        assert!(flat_hard > skirt_hard);
        let _ = flat_gentle;
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
