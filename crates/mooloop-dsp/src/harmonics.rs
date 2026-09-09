//! Distortion authored as a **target**, not as an adjective.
//!
//! Adam, naming the channel strip's four voicings: *"note its not just curves
//! im hoping to capture...idk if we can do it but some THD that's on-target
//! would be really cool."* This is the answer, and it is yes, with two limits
//! stated below.
//!
//! # The trick
//!
//! Feed a unit sine into a Chebyshev polynomial and the harmonic it produces
//! falls out by construction. `T_n(cos θ) = cos(n θ)` is the defining
//! identity, and the same holds for a sine up to phase:
//!
//! ```text
//!   T₁(sin θ) =  sin θ        the fundamental
//!   T₂(sin θ) = -cos 2θ       pure 2nd,  and no DC
//!   T₃(sin θ) = -sin 3θ       pure 3rd
//!   T₄(sin θ) =  cos 4θ       pure 4th
//! ```
//!
//! So a shaper written as `x + Σ aₙ Tₙ(x)` produces, for a full-scale sine,
//! harmonic `n` at exactly `aₙ`. The harmonic profile is therefore something
//! you **write down** and solve for, rather than something you arrive at by
//! turning a knob until it sounds right — and, being a number, it can be
//! asserted in a test. That is what `harmonics_hit_their_stated_target` does.
//!
//! # The two limits, stated so the target is not quietly missed
//!
//! **1. The profile is exact at full scale, and below it the character both
//! thins *and changes shape*.** The orthogonality above is a property of the
//! full-scale sine, not of the polynomial. Feed it `a·sin θ` and every `Tₙ`
//! spills into harmonics `n`, `n-2`, `n-4`… — so `T₄` contributes to the 2nd
//! as well as the 4th, with the *opposite* sign to `T₂`, and the two
//! partially cancel:
//!
//! ```text
//!   2nd harmonic coefficient  =  -a₂·a²  +  a₄·(4a² - 4a⁴)
//!                a = 1        =  -a₂                      the stated target
//!                a = 0.5      =  -0.25·a₂ + 0.75·a₄
//! ```
//!
//! For `IRON` that is -28 dB at full scale and -37 dB at half — a 9 dB fall
//! where a single-term profile would have given 6. **There is no simple
//! `a^(n-1)` law once a profile has more than one term**, and a version of
//! this file that claimed one was wrong; `the_profile_thins_as_the_signal_
//! quietens` now asserts the property that is actually true and actually
//! wanted, which is that every harmonic recedes monotonically as the signal
//! quietens.
//!
//! None of that is a defect to be corrected. `reference/ADAM.md` asks for
//! colour that *"reacts to level"*, and a distortion whose spectrum is
//! identical at -30 and -6 dBFS is the one that sounds like a plugin. The
//! strip's `pre in / drive` control is what buys the character back: it is a
//! gain into this curve, so driving it harder walks the profile back up
//! toward its stated shape rather than fading in a wet/dry mix.
//!
//! **2. It is memoryless, so it cannot be frequency-dependent.** Real
//! transformer distortion rises steeply toward low frequencies -- most of why
//! an iron-sounding stage is obvious on a kick and nearly clean on a hat --
//! and it is hysteretic besides. A polynomial is neither. The cheap and
//! well-understood approximation is a tilt into the shaper and its inverse
//! after, which is worth building for the `IRON` voicing specifically rather
//! than for all four. It is not built here.
//!
//! What is *not* claimed anywhere is a match to a measured unit. The claim is
//! a stated harmonic target, hit at a stated level, and tested.

/// Harmonics a profile describes: the 2nd through the 5th. Beyond the 5th
/// there is little a listener can attribute to a stage rather than to the
/// source, and every extra term is another polynomial degree to alias.
pub const PROFILE_HARMONICS: usize = 4;

/// A voicing's harmonic signature, in **dB below the fundamental, at full
/// scale**, for harmonics 2, 3, 4 and 5.
///
/// `f32::NEG_INFINITY` — or anything below [`SILENT_HARMONIC_DB`] — means the
/// harmonic is absent, which is how [`HarmonicProfile::TRANSPARENT`] is
/// spelt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HarmonicProfile {
    pub harmonics_db: [f32; PROFILE_HARMONICS],
}

/// Below this, a harmonic is treated as absent rather than as very quiet.
/// -120 dB is under the floor of the 24-bit file an export is written to.
pub const SILENT_HARMONIC_DB: f32 = -120.0;

impl HarmonicProfile {
    /// No added harmonics at all. `Moo`'s, and the identity: a strip in this
    /// voicing is bit-identical to no strip.
    pub const TRANSPARENT: Self = Self {
        harmonics_db: [SILENT_HARMONIC_DB; PROFILE_HARMONICS],
    };

    pub const fn new(second: f32, third: f32, fourth: f32, fifth: f32) -> Self {
        Self {
            harmonics_db: [second, third, fourth, fifth],
        }
    }

    /// Whether this profile adds nothing, so the shaper can be skipped
    /// outright. The strip is on every channel and every bus, so "free while
    /// it is out" has to mean *free*.
    pub fn is_transparent(&self) -> bool {
        self.harmonics_db
            .iter()
            .all(|db| *db <= SILENT_HARMONIC_DB)
    }
}

// --- The four voicings ------------------------------------------------------
//
// Named for the sound rather than for any hardware, per `UI_DESIGN.md`.
//
// **These numbers are provisional, and there is a reason beyond taste.** A
// profile is stated at full scale, and the level law above means the same
// curve is far quieter on material at mooloop's -12 dBFS operating level --
// `the_profile_thins_as_the_signal_quietens` prints the figures, and at
// -18 dBFS `GRIP`'s harmonics have receded past -110 dB, which is nothing at
// all. So **what these numbers should be cannot be settled until the strip's
// input stage is**: "-28 dB of 2nd harmonic" says nothing without saying what
// level arrives at the curve.
//
// The stage that decides it is `pre in / drive`, which normalizes into the
// shaper the way `shaper::drive_compensation` already does for the Drive
// device -- +12 dB brings operating-level material to where these profiles
// are stated. Authoring the two together, with ears, is step 03's job; this
// module exists to prove that the target *can* be stated and hit.

/// `Moo` — the house voicing. Calibrated and uncoloured.
pub const MOO: HarmonicProfile = HarmonicProfile::TRANSPARENT;

/// `Grip` — fast, tight, controlled. Low distortion and mostly odd-order,
/// which is what a well-behaved VCA-ish stage measures like.
pub const GRIP: HarmonicProfile = HarmonicProfile::new(-58.0, -44.0, -70.0, -60.0);

/// `Punch` — forward and thick. Both orders present and enough of them to
/// hear on a snare.
pub const PUNCH: HarmonicProfile = HarmonicProfile::new(-36.0, -40.0, -54.0, -56.0);

/// `Iron` — transformer warmth: even-order dominant, the one whose
/// distortion is the point. At the operating level its 2nd harmonic lands
/// near -40 dB, which is about a percent and is audible as warmth rather than
/// as an effect.
pub const IRON: HarmonicProfile = HarmonicProfile::new(-28.0, -46.0, -48.0, -60.0);

// --- The shaper -------------------------------------------------------------

/// A memoryless curve that realises a [`HarmonicProfile`].
///
/// Cheap enough to sit on every strip: five multiply-accumulates on the
/// polynomial, evaluated by the Chebyshev recurrence so no `powi` is needed,
/// and skipped entirely when the profile is transparent.
#[derive(Debug, Clone, Copy)]
pub struct HarmonicShaper {
    /// `a₂..a₅` in linear amplitude. `a₁` is 1 and is not stored.
    coefficients: [f32; PROFILE_HARMONICS],
    transparent: bool,
}

impl HarmonicShaper {
    pub fn new(profile: HarmonicProfile) -> Self {
        let mut coefficients = [0.0; PROFILE_HARMONICS];
        for (coefficient, db) in coefficients.iter_mut().zip(profile.harmonics_db) {
            *coefficient = if db <= SILENT_HARMONIC_DB {
                0.0
            } else {
                10.0f32.powf(db / 20.0)
            };
        }
        Self {
            coefficients,
            transparent: profile.is_transparent(),
        }
    }

    /// Whether this shaper may be skipped outright.
    pub fn is_transparent(&self) -> bool {
        self.transparent
    }

    /// Shape one sample.
    ///
    /// The input is clamped to the polynomial's domain, which is a *monotone*
    /// extension rather than a bound for its own sake: a Chebyshev polynomial
    /// grows without limit outside `[-1, 1]` and would turn a hot sample into
    /// a much hotter one. Nothing reaches here above full scale in ordinary
    /// use, because the drive stage in front of it is what pushes the level.
    #[inline]
    pub fn shape(&self, sample: f32) -> f32 {
        if self.transparent {
            return sample;
        }
        let x = sample.clamp(-1.0, 1.0);
        // Chebyshev by recurrence: T₀ = 1, T₁ = x, Tₙ₊₁ = 2x·Tₙ - Tₙ₋₁.
        let mut previous = 1.0f32;
        let mut current = x;
        let mut out = x;
        for coefficient in self.coefficients {
            let next = 2.0 * x * current - previous;
            previous = current;
            current = next;
            out += coefficient * current;
        }
        out
    }
}

/// One-pole DC blocker.
///
/// The Chebyshev basis is DC-free for a full-scale sine — `T₂(sin θ)` is
/// exactly `-cos 2θ` — but that is the *only* case it is free for. At any
/// other amplitude an even-order term carries a constant, and because that
/// constant moves with level it would arrive as a thump on every fader ride
/// rather than as a steady offset. Real stages have the same problem and the
/// same answer, which is a coupling capacitor.
#[derive(Debug, Clone, Copy)]
pub struct DcBlocker {
    coefficient: f32,
    last_input: f32,
    last_output: f32,
}

impl DcBlocker {
    /// A blocker cornered at `cutoff_hz`. 5 Hz is below anything musical and
    /// well above the offsets this exists to remove.
    pub fn new(sample_rate: u32, cutoff_hz: f32) -> Self {
        let coefficient =
            1.0 - (std::f32::consts::TAU * cutoff_hz / sample_rate.max(1) as f32).min(1.0);
        Self {
            coefficient,
            last_input: 0.0,
            last_output: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.last_input = 0.0;
        self.last_output = 0.0;
    }

    #[inline]
    pub fn process(&mut self, sample: f32) -> f32 {
        let out = sample - self.last_input + self.coefficient * self.last_output;
        self.last_input = sample;
        self.last_output = out;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE: f32 = 48_000.0;
    /// 4800 samples at 100 Hz is exactly ten cycles, so every harmonic lands
    /// on an integer bin and the measurement needs no window and leaks
    /// nothing. That is what makes the assertion below a tolerance on the
    /// *design* rather than on the analysis.
    const FRAMES: usize = 4800;
    const FUNDAMENTAL_HZ: f32 = 100.0;

    /// Amplitude of harmonic `n` in `samples`, by direct correlation at the
    /// exact frequency. Five bins are wanted, so a whole FFT would be more
    /// code and less obviously correct.
    fn harmonic_amplitude(samples: &[f32], n: usize) -> f32 {
        let step = std::f32::consts::TAU * FUNDAMENTAL_HZ * n as f32 / SAMPLE_RATE;
        let (mut sin_sum, mut cos_sum) = (0.0f64, 0.0f64);
        for (index, sample) in samples.iter().enumerate() {
            let phase = step * index as f32;
            sin_sum += (*sample * phase.sin()) as f64;
            cos_sum += (*sample * phase.cos()) as f64;
        }
        let scale = 2.0 / samples.len() as f64;
        (((sin_sum * scale).powi(2) + (cos_sum * scale).powi(2)).sqrt()) as f32
    }

    fn shaped_sine(profile: HarmonicProfile, amplitude: f32) -> Vec<f32> {
        let shaper = HarmonicShaper::new(profile);
        let step = std::f32::consts::TAU * FUNDAMENTAL_HZ / SAMPLE_RATE;
        (0..FRAMES)
            .map(|index| shaper.shape(amplitude * (step * index as f32).sin()))
            .collect()
    }

    fn db(ratio: f32) -> f32 {
        20.0 * ratio.max(1e-12).log10()
    }

    /// **The claim, as a test.** Every voicing's stated harmonic profile is
    /// what a full-scale sine actually measures coming out of it.
    ///
    /// The tolerance is 0.5 dB, which is far tighter than any of these
    /// numbers will ever be authored to; it is that tight because the
    /// Chebyshev construction is exact and anything looser would stop
    /// noticing if it broke.
    #[test]
    fn harmonics_hit_their_stated_target() {
        for (name, profile) in [("Grip", GRIP), ("Punch", PUNCH), ("Iron", IRON)] {
            let samples = shaped_sine(profile, 1.0);
            let fundamental = harmonic_amplitude(&samples, 1);
            assert!(
                (db(fundamental)).abs() < 0.1,
                "{name} moved the fundamental to {:.2} dB",
                db(fundamental)
            );
            for (index, target_db) in profile.harmonics_db.iter().enumerate() {
                let n = index + 2;
                let measured = db(harmonic_amplitude(&samples, n) / fundamental);
                println!("{name} harmonic {n}: target {target_db:.1} dB, measured {measured:.2} dB");
                assert!(
                    (measured - target_db).abs() < 0.5,
                    "{name} harmonic {n} measured {measured:.2} dB against a target of \
                     {target_db:.1} dB"
                );
            }
        }
    }

    /// The colour recedes as the signal quietens, which is limit 1 in this
    /// module's own header and the property that makes it playable.
    ///
    /// **Monotonic, not a fixed dB law.** The first version of this test
    /// asserted `a^(n-1)` -- 6 dB off the 2nd per halving -- and it failed,
    /// which is how the interaction in the header was found: below full scale
    /// `T₄` feeds the 2nd harmonic against `T₂`, so `Iron`'s 2nd falls 9 dB
    /// per halving rather than 6. What is worth holding is that every
    /// harmonic goes *down* and keeps going down, because that is what a
    /// well-meaning normalization somewhere upstream would destroy.
    #[test]
    fn the_profile_thins_as_the_signal_quietens() {
        for (name, profile) in [("Grip", GRIP), ("Punch", PUNCH), ("Iron", IRON)] {
            for n in 2..=5 {
                let mut previous = f32::INFINITY;
                for amplitude in [1.0, 0.5, 0.25, 0.125] {
                    let samples = shaped_sine(profile, amplitude);
                    let ratio =
                        db(harmonic_amplitude(&samples, n) / harmonic_amplitude(&samples, 1));
                    assert!(
                        ratio < previous - 1.0,
                        "{name} harmonic {n} did not recede at amplitude {amplitude}:                          {ratio:.2} dB against {previous:.2} dB"
                    );
                    previous = ratio;
                }
                println!("{name} harmonic {n}: recedes to {previous:.1} dB by -18 dBFS");
            }
        }
    }

    /// The interaction the test above found, pinned as a number so the
    /// header's worked example cannot drift away from the code.
    #[test]
    fn a_profiles_terms_interact_below_full_scale() {
        let ratio_at = |amplitude: f32| {
            let samples = shaped_sine(IRON, amplitude);
            db(harmonic_amplitude(&samples, 2) / harmonic_amplitude(&samples, 1))
        };
        let full = ratio_at(1.0);
        let half = ratio_at(0.5);
        assert!((full - -28.0).abs() < 0.1, "full scale {full:.2}");
        // -37.1 by hand from `-a2*a^2 + a4*(4a^2 - 4a^4)`, not -34 as a
        // single-term `a^(n-1)` law would predict.
        assert!(
            (half - -37.1).abs() < 0.3,
            "half scale measured {half:.2} dB, expected -37.1"
        );
    }

    /// `Moo` adds nothing, and says so cheaply enough that the strip can skip
    /// the whole stage. This is the "free while it is out" rule at the level
    /// of one sample.
    #[test]
    fn the_house_voicing_is_the_identity() {
        let shaper = HarmonicShaper::new(MOO);
        assert!(shaper.is_transparent());
        for step in -1000..=1000 {
            let x = step as f32 / 500.0;
            assert_eq!(shaper.shape(x), x, "Moo altered {x}");
        }
    }

    /// A polynomial grows without limit outside its domain, so the clamp is
    /// load-bearing: without it a sample at 2.0 would come back at 30-odd and
    /// the strip would be a fuzz box above full scale.
    #[test]
    fn a_hot_sample_saturates_instead_of_exploding() {
        let shaper = HarmonicShaper::new(IRON);
        assert_eq!(shaper.shape(4.0), shaper.shape(1.0));
        assert_eq!(shaper.shape(-4.0), shaper.shape(-1.0));
        assert!(shaper.shape(1.0).abs() < 1.5, "{}", shaper.shape(1.0));
    }

    /// The DC an even-order term carries at anything but full scale is real,
    /// and moves with level -- which is a thump on a fader ride, not an
    /// offset. The blocker removes it and leaves the audio alone.
    #[test]
    fn the_blocker_removes_the_offset_that_moves_with_level() {
        let samples = shaped_sine(IRON, 0.5);
        let mean = samples.iter().sum::<f32>() / samples.len() as f32;
        assert!(
            mean.abs() > 1e-4,
            "the fixture has no offset to remove ({mean:.2e}); this test proves nothing"
        );

        let mut blocker = DcBlocker::new(SAMPLE_RATE as u32, 5.0);
        // Settle the one-pole before measuring, so the mean is the steady
        // state rather than the transient. A 5 Hz corner takes a few hundred
        // milliseconds to get there, which is several passes of this fixture.
        let blocked: Vec<f32> = std::iter::repeat_n(samples.iter(), 4)
            .flatten()
            .map(|s| blocker.process(*s))
            .skip(samples.len() * 3)
            .collect();
        let blocked_mean = blocked.iter().sum::<f32>() / blocked.len() as f32;
        println!(
            "offset {mean:.3e} -> {blocked_mean:.3e} ({:.1} dB)",
            db(blocked_mean.abs() / mean.abs())
        );
        assert!(
            blocked_mean.abs() < mean.abs() * 0.02,
            "offset {mean:.3e} was only reduced to {blocked_mean:.3e}"
        );
        // And the fundamental survived it, which is the other half: a DC
        // blocker cornered too high would take the bass with it.
        let before = harmonic_amplitude(&samples, 1);
        let after = harmonic_amplitude(&blocked, 1);
        assert!(
            (db(after / before)).abs() < 0.05,
            "the blocker moved 100 Hz by {:.3} dB",
            db(after / before)
        );
    }
}
