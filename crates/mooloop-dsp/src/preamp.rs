//! The strip's input stage: what makes a voicing sound like a preamp rather
//! than like a waveshaper.
//!
//! [`crate::harmonics`] makes a harmonic profile authorable and testable, and
//! says in its own header what it cannot do: it is memoryless, so it has no
//! opinion about frequency. That is the gap this module closes, and closing
//! it is most of the difference between "a shaper" and "a preamp".
//!
//! # Why frequency comes into it at all
//!
//! A transformer's core flux goes as `V / f`. At a constant voltage, halve
//! the frequency and you double the flux — so a transformer **saturates from
//! the bottom up**, and that is the whole reason iron-sounding gear is
//! obvious on a kick and nearly clean on a hi-hat. It is also, in Adam's
//! description of a much cleaner desk, why *"the breakup would probably make
//! you want to back off rather than lean in, except maybe in the low end
//! where you get a nice fat round sound"* — same mechanism, less of it.
//!
//! # The filter sandwich
//!
//! ```text
//!   in ─▶ tilt (highs down) ─▶ shaper ─▶ untilt (highs up) ─▶ DC block ─▶ out
//! ```
//!
//! High frequencies arrive at the curve quieter and so saturate less; the
//! matching boost afterwards puts the frequency response back, so what
//! survives is frequency-dependent **distortion** rather than
//! frequency-dependent **level**. This is a Wiener–Hammerstein model —
//! linear, static nonlinear, linear — which is the standard block-oriented
//! structure for this class of device.
//!
//! **Highs down rather than lows up, and that is not a stylistic choice.**
//! The first version of this file boosted the low end into the curve, which
//! is the more obvious reading of "the bottom saturates first" and is wrong
//! in practice: a 14 dB boost on material at the operating level walks
//! straight past the shaper's `[-1, 1]` domain, the monotone clamp takes
//! over, and what comes out is a square wave — all odd harmonics, `Iron`'s
//! 3rd sitting 31 dB *above* its 2nd, and **more drive producing less** 2nd
//! harmonic. Attenuating the top instead is the same relative tilt with the
//! loudest part of the signal left where the curve can still shape it.
//!
//! # `tilt_db` is a control, not a prediction
//!
//! It is tempting to say the tilt in dB *is* the difference in distortion in
//! dB, because the post-filter restores fundamental and harmonics by the same
//! factor. That was claimed here, tested, and false: `Grip` states 10 dB and
//! delivers 31.
//!
//! The reason is the one [`crate::harmonics`] already records about its level
//! law — **a multi-term profile's terms interact**, and here they nearly
//! cancel. `Grip`'s 2nd-harmonic coefficient is `-a₂a² + a₄(4a² - 4a⁴)`, and
//! at the amplitude the tilt leaves at 8 kHz those two terms agree to within
//! a few percent and annihilate each other, dropping the 2nd by 30 dB rather
//! than 10.
//!
//! That is twice now that a clean closed-form claim about this scheme has
//! turned out to be wrong in the same way, which is itself the finding:
//! **derive nothing about a profile, measure it.** More tilt does reliably
//! mean more difference — `more_tilt_means_more_difference` holds that, and
//! it is monotone rather than linear — but the number is a control to be
//! turned, not a specification to be read off.
//!
//! **The two shelves are exact inverses**, not approximate ones. Substituting
//! `A → 1/A` in the RBJ low-shelf swaps its numerator and denominator (both
//! scaled by `1/A²`, and `alpha` depends on `A` only through `A + 1/A`, which
//! is invariant), so the cascade is exactly unity. `the_sandwich_is_flat_when_
//! the_shaper_is_out` is that as a test, and it is what lets `Moo` be
//! bit-identical to no preamp at all.
//!
//! # What is not here
//!
//! Supply sag, and hysteresis. `docs/plans/console/06-preamp-modelling.md`
//! prices both; the second is Jiles–Atherton and is a per-sample ODE solve,
//! which is not worth reaching for before this has been heard.
//!
//! # The numbers are provisional, and deliberately separable
//!
//! Adam, 2026-09-09: *"we can do some sweep measurements on UAD and Waves
//! plugs later to try and get some real numbers. if we need them before then
//! lets just pick some."* So [`PreampVoicing`] is a plain table of numbers
//! with no behaviour attached, and a measured fit replaces its rows without
//! touching anything else in this file.

use crate::biquad::Biquad;
use crate::harmonics::{DcBlocker, HarmonicProfile, HarmonicShaper, GRIP, IRON, MOO, PUNCH};

/// One voicing's input stage, as data.
///
/// Every field is a number somebody picked and can re-pick. Nothing here
/// decides *how* the stage works — that is [`Preamp`] — which is what keeps a
/// later measurement pass a matter of editing four rows.
#[derive(Debug, Clone, Copy)]
pub struct PreampVoicing {
    /// What the curve does, at full scale. See [`crate::harmonics`].
    pub harmonics: HarmonicProfile,
    /// How much *less* the top of the band distorts than the bottom, in dB.
    /// This is the transformer, and it is the difference between a voicing
    /// that thickens a kick and one that fizzes a hat.
    ///
    /// Realised as an attenuation of the highs into the curve rather than a
    /// boost of the lows — see this module's header for why that distinction
    /// is load-bearing. **Monotone but not linear**: more of it means more
    /// difference across the corner, and how much more depends on the profile
    /// it is applied to, for the reason the header gives.
    pub tilt_db: f32,
    /// Corner of the tilt. Below it the curve sees the signal as it arrives;
    /// above it, `tilt_db` quieter.
    pub tilt_hz: f32,
    /// Largest change per sample at 48 kHz, as a fraction of full scale, or
    /// `None` for no limit.
    ///
    /// This is the transient half, and it is a *different* nonlinearity from
    /// the curve: it depends on `dV/dt` rather than on level, which is what
    /// lets it tell a fast transient from a loud tone. A discrete op-amp
    /// known for being fast is modelled by **not** limiting it.
    pub slew: Option<f32>,
}

impl PreampVoicing {
    /// Whether this voicing does nothing at all, so the whole stage can be
    /// skipped. `Moo`'s answer, and the reason a track with the strip out is
    /// bit-identical to no track.
    pub fn is_transparent(&self) -> bool {
        self.harmonics.is_transparent() && self.slew.is_none()
    }
}

/// `Moo` — the house voicing. Unity, and the null case.
pub const MOO_PREAMP: PreampVoicing = PreampVoicing {
    harmonics: MOO,
    tilt_db: 0.0,
    tilt_hz: 200.0,
    slew: None,
};

/// `Grip` — *"cleanest per dB of gain … the breakup would probably make you
/// want to back off rather than lean in, except maybe in the low end where
/// you get a nice fat round sound."*
///
/// Low overall content and odd-dominant, so the top breaks up harshly enough
/// to be a warning rather than an invitation — and the same tilt as `Iron`,
/// so the bottom still rounds when it is pushed. That combination is Adam's
/// sentence, mechanically.
pub const GRIP_PREAMP: PreampVoicing = PreampVoicing {
    harmonics: GRIP,
    tilt_db: 10.0,
    tilt_hz: 180.0,
    slew: None,
};

/// `Punch` — *"needs to sound like rock and roll."*
///
/// Both orders and more of them, and **no slew limit at all**: transients
/// arrive intact, which is most of what "fast" and "forward" mean about the
/// discrete op-amp this is after. A milder tilt, because the character is
/// meant to be broadband rather than bottom-heavy.
pub const PUNCH_PREAMP: PreampVoicing = PreampVoicing {
    harmonics: PUNCH,
    tilt_db: 5.0,
    tilt_hz: 150.0,
    slew: None,
};

/// `Iron` — *"a little warm, mid forward."*
///
/// Even-order dominant and the deepest tilt of the four, so the warmth
/// arrives on a kick and is nearly absent on a hat. The slew limit is what
/// rounds a transient rather than passing it whole, which is the other half
/// of "warm".
pub const IRON_PREAMP: PreampVoicing = PreampVoicing {
    harmonics: IRON,
    tilt_db: 14.0,
    tilt_hz: 220.0,
    slew: Some(0.08),
};

/// A slew limiter: the transient half of a preamp's character.
#[derive(Debug, Clone, Copy)]
struct SlewLimiter {
    /// Largest change per sample, already scaled to the running rate.
    max_step: f32,
    last: f32,
}

impl SlewLimiter {
    fn new(per_sample_at_48k: f32, sample_rate: u32) -> Self {
        // A slew *rate* is per unit time, not per sample, so the same stage
        // has to allow a bigger step at a lower rate to stay the same device.
        Self {
            max_step: per_sample_at_48k * 48_000.0 / sample_rate.max(1) as f32,
            last: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let delta = (input - self.last).clamp(-self.max_step, self.max_step);
        self.last += delta;
        self.last
    }
}

/// The strip's input stage for one voicing, at one sample rate.
pub struct Preamp {
    shaper: HarmonicShaper,
    tilt: Biquad,
    untilt: Biquad,
    dc: DcBlocker,
    slew: Option<SlewLimiter>,
    transparent: bool,
}

impl Preamp {
    pub fn new(voicing: PreampVoicing, sample_rate: u32) -> Self {
        let mut tilt = Biquad::identity();
        let mut untilt = Biquad::identity();
        if voicing.tilt_db != 0.0 {
            // High shelf, cut then restore: the top of the band is what gets
            // held back from the curve. The pair is exactly reciprocal, so
            // the stage colours without equalising.
            tilt.shelf(voicing.tilt_hz, -voicing.tilt_db, false, sample_rate);
            untilt.shelf(voicing.tilt_hz, voicing.tilt_db, false, sample_rate);
        }
        Self {
            shaper: HarmonicShaper::new(voicing.harmonics),
            tilt,
            untilt,
            dc: DcBlocker::new(sample_rate, 5.0),
            slew: voicing
                .slew
                .map(|rate| SlewLimiter::new(rate, sample_rate)),
            transparent: voicing.is_transparent(),
        }
    }

    /// Whether this stage may be skipped outright.
    pub fn is_transparent(&self) -> bool {
        self.transparent
    }

    pub fn reset(&mut self) {
        self.tilt.reset();
        self.untilt.reset();
        self.dc.reset();
        if let Some(slew) = self.slew.as_mut() {
            slew.last = 0.0;
        }
    }

    /// Run one sample through the stage.
    ///
    /// `drive` is a linear gain into the curve, and it is the control that
    /// makes the character playable: the harmonic profile is stated at full
    /// scale, so material at the -12 dBFS operating level needs pushing
    /// toward it before the curve does much. See `harmonics.rs` on the level
    /// law, which is not the simple one it looks like.
    #[inline]
    pub fn process(&mut self, sample: f32, drive: f32) -> f32 {
        if self.transparent {
            return sample * drive;
        }
        let tilted = self.tilt.process(sample * drive);
        let shaped = self.shaper.shape(tilted);
        let flat = self.untilt.process(shaped);
        let blocked = self.dc.process(flat);
        match self.slew.as_mut() {
            Some(slew) => slew.process(blocked),
            None => blocked,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE: u32 = 48_000;

    /// Amplitude of harmonic `n` of `fundamental` in `samples`, by direct
    /// correlation. Mirrors `harmonics.rs`'s measurement for the same reason:
    /// five bins are wanted and a whole FFT would be more code and less
    /// obviously correct.
    fn harmonic_amplitude(samples: &[f32], fundamental: f32, n: usize) -> f32 {
        let step = std::f32::consts::TAU * fundamental * n as f32 / SAMPLE_RATE as f32;
        let (mut sin_sum, mut cos_sum) = (0.0f64, 0.0f64);
        for (index, sample) in samples.iter().enumerate() {
            let phase = step * index as f32;
            sin_sum += (*sample * phase.sin()) as f64;
            cos_sum += (*sample * phase.cos()) as f64;
        }
        let scale = 2.0 / samples.len() as f64;
        (((sin_sum * scale).powi(2) + (cos_sum * scale).powi(2)).sqrt()) as f32
    }

    fn db(ratio: f32) -> f32 {
        20.0 * ratio.max(1e-12).log10()
    }

    /// Render a sine of `hz` at `amplitude`, discarding the first tenth so
    /// the filters have settled before anything is measured.
    fn run(voicing: PreampVoicing, hz: f32, amplitude: f32, drive: f32) -> Vec<f32> {
        let mut preamp = Preamp::new(voicing, SAMPLE_RATE);
        // An integer number of cycles, so every harmonic lands on a bin.
        let cycles = 40.0;
        let frames = (cycles * SAMPLE_RATE as f32 / hz) as usize;
        let step = std::f32::consts::TAU * hz / SAMPLE_RATE as f32;
        let warmup = frames / 2;
        (0..frames + warmup)
            .map(|index| preamp.process(amplitude * (step * index as f32).sin(), drive))
            .skip(warmup)
            .collect()
    }

    /// Second-harmonic distortion relative to the fundamental, in dB.
    fn thd2(voicing: PreampVoicing, hz: f32, amplitude: f32, drive: f32) -> f32 {
        let samples = run(voicing, hz, amplitude, drive);
        db(harmonic_amplitude(&samples, hz, 2) / harmonic_amplitude(&samples, hz, 1))
    }

    /// **The claim this module exists for**: the same signal level distorts
    /// more at the bottom of the band than at the top, which is "warm on a
    /// kick, clean on a hat" as a number.
    ///
    /// A memoryless shaper cannot do this at all — `harmonics.rs` says so in
    /// its header — so a failure here means the sandwich has been flattened,
    /// which is exactly the kind of simplification that looks harmless.
    #[test]
    fn the_low_end_distorts_more_than_the_top() {
        for (name, voicing) in [
            ("Grip", GRIP_PREAMP),
            ("Punch", PUNCH_PREAMP),
            ("Iron", IRON_PREAMP),
        ] {
            let low = thd2(voicing, 80.0, 0.5, 1.6);
            let high = thd2(voicing, 5000.0, 0.5, 1.6);
            println!("{name}: 2nd harmonic {low:.1} dB at 80 Hz, {high:.1} dB at 5 kHz");
            assert!(
                low > high + 4.0,
                "{name} distorted the bottom by only {:.1} dB more than the top; \
                 the tilt is not doing anything",
                low - high
            );
        }
    }

    /// Turning the tilt up widens the gap between bottom and top, which is
    /// the whole of what the control is for.
    ///
    /// **Monotone, not linear, and deliberately not a prediction.** The first
    /// version of this test asserted that the difference *equals* `tilt_db`,
    /// which reads beautifully and is false — `Grip` states 10 dB and
    /// delivers 31, because at the amplitude the tilt leaves at 8 kHz its
    /// `a₂` and `a₄` terms very nearly cancel. That is the second time a
    /// closed-form claim about a multi-term profile has been wrong in the
    /// same way; the module header records it as a rule.
    #[test]
    fn more_tilt_means_more_difference() {
        let mut previous = f32::NEG_INFINITY;
        for tilt_db in [0.0, 4.0, 8.0, 12.0, 16.0] {
            let voicing = PreampVoicing {
                tilt_db,
                slew: None,
                ..IRON_PREAMP
            };
            let difference = thd2(voicing, 60.0, 0.5, 1.6) - thd2(voicing, 8000.0, 0.5, 1.6);
            println!("tilt {tilt_db:>4.0} dB -> {difference:6.1} dB of difference");
            assert!(
                difference > previous,
                "{tilt_db} dB of tilt gave {difference:.1} dB, no more than the step below"
            );
            previous = difference;
        }
        // And with no tilt at all there is nothing to find, which is what
        // says the effect comes from the sandwich rather than from the curve.
        let flat = PreampVoicing {
            tilt_db: 0.0,
            slew: None,
            ..IRON_PREAMP
        };
        let difference = thd2(flat, 60.0, 0.5, 1.6) - thd2(flat, 8000.0, 0.5, 1.6);
        assert!(
            difference.abs() < 1.0,
            "an untilted stage still favoured the bottom by {difference:.1} dB"
        );
    }

    /// And the shelves really are inverses, so the stage colours without
    /// equalising. Without this the tilt would be an audible bass boost that
    /// happened to also distort.
    #[test]
    fn the_sandwich_is_flat_when_the_shaper_is_out() {
        // `Iron`'s filters with no curve between them: the deepest tilt in
        // the table, so if any pair fails to cancel it is this one.
        let flat = PreampVoicing {
            harmonics: HarmonicProfile::TRANSPARENT,
            slew: None,
            ..IRON_PREAMP
        };
        let mut tilt = Biquad::identity();
        let mut untilt = Biquad::identity();
        tilt.shelf(flat.tilt_hz, -flat.tilt_db, false, SAMPLE_RATE);
        untilt.shelf(flat.tilt_hz, flat.tilt_db, false, SAMPLE_RATE);
        for hz in [30.0, 80.0, 200.0, 1000.0, 5000.0, 12000.0] {
            let step = std::f32::consts::TAU * hz / SAMPLE_RATE as f32;
            let frames = (40.0 * SAMPLE_RATE as f32 / hz) as usize;
            let mut out = Vec::with_capacity(frames);
            for index in 0..frames * 2 {
                let input = (step * index as f32).sin();
                let processed = untilt.process(tilt.process(input));
                if index >= frames {
                    out.push(processed);
                }
            }
            let level = db(harmonic_amplitude(&out, hz, 1));
            assert!(
                level.abs() < 0.01,
                "the sandwich was {level:.4} dB off unity at {hz} Hz"
            );
        }
    }

    /// `Moo` is the identity, drive included, and says so cheaply enough that
    /// the whole stage can be skipped.
    #[test]
    fn the_house_voicing_passes_the_signal_through() {
        assert!(MOO_PREAMP.is_transparent());
        let mut preamp = Preamp::new(MOO_PREAMP, SAMPLE_RATE);
        assert!(preamp.is_transparent());
        for step in -100..=100 {
            let x = step as f32 / 100.0;
            assert_eq!(preamp.process(x, 1.0), x);
        }
    }

    /// Driving harder buys more character, which is the control that makes
    /// the level law in `harmonics.rs` playable rather than a limitation.
    #[test]
    fn drive_buys_character() {
        let quiet = thd2(IRON_PREAMP, 80.0, 0.25, 1.0);
        let pushed = thd2(IRON_PREAMP, 80.0, 0.25, 3.5);
        println!("Iron at 80 Hz: {quiet:.1} dB at unity drive, {pushed:.1} dB driven");
        assert!(
            pushed > quiet + 6.0,
            "drive moved the 2nd harmonic by only {:.1} dB",
            pushed - quiet
        );
    }

    /// The harmonic balance is what separates the voicings, and it is the one
    /// thing the table must not lose: `Iron` is even-dominant and `Grip` is
    /// odd-dominant, which is the whole warm-versus-hard axis.
    #[test]
    fn iron_is_even_dominant_and_grip_is_odd_dominant() {
        let balance = |voicing: PreampVoicing| {
            let samples = run(voicing, 80.0, 0.5, 1.9);
            let second = harmonic_amplitude(&samples, 80.0, 2);
            let third = harmonic_amplitude(&samples, 80.0, 3);
            db(second / third)
        };
        let iron = balance(IRON_PREAMP);
        let grip = balance(GRIP_PREAMP);
        println!("2nd-over-3rd: Iron {iron:+.1} dB, Grip {grip:+.1} dB");
        assert!(iron > 6.0, "Iron's 2nd was only {iron:+.1} dB over its 3rd");
        assert!(grip < -6.0, "Grip's 3rd was only {:+.1} dB over its 2nd", -grip);
    }

    /// A slew limit is a nonlinearity in `dV/dt`, so it acts on a fast
    /// transient and leaves a slow tone of the same amplitude alone. That
    /// distinction is the point: it is why one voicing can be "fast" and
    /// another "rounded" at identical harmonic content.
    #[test]
    fn a_slew_limit_catches_a_transient_and_not_a_slow_tone() {
        let mut limiter = SlewLimiter::new(0.02, SAMPLE_RATE);
        let step: Vec<f32> = (0..64).map(|i| if i < 8 { 0.0 } else { 1.0 }).collect();
        let limited: Vec<f32> = step.iter().map(|s| limiter.process(*s)).collect();
        assert!(
            limited[9] < 0.05,
            "a step arrived at {} in one sample",
            limited[9]
        );

        // The same peak, reached slowly: untouched.
        let mut limiter = SlewLimiter::new(0.02, SAMPLE_RATE);
        let hz = 100.0;
        let step_phase = std::f32::consts::TAU * hz / SAMPLE_RATE as f32;
        let frames = (SAMPLE_RATE as f32 / hz) as usize;
        let worst = (0..frames)
            .map(|i| {
                let input = (step_phase * i as f32).sin();
                (limiter.process(input) - input).abs()
            })
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-3, "a 100 Hz tone was slewed by {worst}");
    }
}
