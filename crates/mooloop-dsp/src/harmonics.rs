//! Distortion authored as a **target**, not as an adjective.
//!
//! Adam, naming the channel strip's four voicings: *"note its not just curves
//! im hoping to capture...idk if we can do it but some THD that's on-target
//! would be really cool."* This is the answer, and it is yes, at the level
//! the music is actually at.
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
//! ```
//!
//! So a shaper written as `x + Σ aₙ Tₙ(x)` produces, for a **full-scale**
//! sine, harmonic `n` at exactly `aₙ`. The harmonic profile is therefore
//! something you **write down** and solve for, rather than something you
//! arrive at by turning a knob until it sounds right — and, being a number,
//! it can be asserted in a test.
//!
//! # The profile is stated at the operating level, not at full scale
//!
//! That identity is exact at full scale and nowhere else, and full scale is
//! not where music sits. mooloop's unity operating level is
//! [`PROFILE_REFERENCE_DBFS`] — `-12` dBFS, from
//! `mooloop_core::gain::REFERENCE_PEAK_DBFS` — and a profile stated 12 dB
//! above the signal describes a stage nobody hears. The version of this file
//! that authored at full scale delivered `Iron`'s 2nd harmonic at **-44 dB**
//! where its own doc comment claimed "near -40 dB at the operating level",
//! and its 4th and 5th at -84 and -108 dB, which is nothing at all.
//!
//! So [`HarmonicShaper::new`] **solves** for the coefficients that hit the
//! profile at [`PROFILE_REFERENCE_DBFS`], instead of using the targets as
//! coefficients directly. Feed `a·sin θ` to `Tₙ` and it spills into harmonics
//! `n`, `n-2`, `n-4`…, so the map from coefficients to harmonics is a small
//! linear system rather than the identity:
//!
//! ```text
//!   F  = a + a₃(3a³ - 3a)      the fundamental, which T₃ moves
//!   H₂ = -a₂a²
//!   H₃ = -a₃a³
//! ```
//!
//! `solve_at` inverts that in closed form. At `a = 1` the spill terms vanish
//! and it reduces to the old direct mapping exactly, which
//! `full_scale_solves_to_the_coefficients_themselves` pins.
//!
//! # Why only the 2nd and the 3rd
//!
//! Because the 4th and the 5th **cannot be authored down here**, and that is
//! arithmetic rather than taste. `Tₙ`'s own harmonic output scales as `aⁿ`
//! while its spill into the fundamental scales as `a`. At `a = 0.251` that
//! is `a⁵ = 0.001`: `T₅` is a thousand times better at disturbing the
//! fundamental than at making a 5th harmonic. Solving all four of a measured
//! unit's harmonics at -12 dBFS needs coefficients that drop the stage's own
//! fundamental by **15 dB** for `Grip` and 7 dB for `Iron`. The four-target
//! form is only sane above about -4 dBFS.
//!
//! Nothing audible is lost. A profile that carried the 4th and 5th delivered
//! them at -84 and -108 dB at the operating level, which is under the floor
//! of the file an export is written to. Two harmonics stated where the music
//! is beats four stated where it is not.
//!
//! # The two limits, stated so the target is not quietly missed
//!
//! **1. The profile is exact at the operating level, and the character both
//! grows above it and thins below it.** `the_profile_thins_as_the_signal_
//! quietens` asserts the direction, which is the property that is actually
//! wanted and the one a well-meaning normalization upstream would destroy.
//!
//! Above the reference the curve breaks up fast: these voicings reach 5-11%
//! THD at 0 dBFS. That is not a defect — it is what the references do. The
//! units these profiles are fitted to go from 1% THD at -12 dBFS to **20-35%
//! at 0 dBFS**, a knee they cross between -12 and -9. See
//! `spikes/preamp-measure/DATA.md`.
//!
//! `reference/ADAM.md` asks for colour that *"reacts to level"*, and a
//! distortion whose spectrum is identical at -30 and -6 dBFS is the one that
//! sounds like a plugin.
//!
//! **2. It is memoryless, so it cannot be frequency-dependent.** A
//! transformer's core flux goes as `V/f`, so for a constant voltage it
//! saturates *from the bottom up* -- which is most of why an iron-sounding
//! stage is obvious on a kick and nearly clean on a hat. A polynomial has no
//! opinion about frequency at all, and real cores are hysteretic besides.
//!
//! The cheap and standard fix is a **filter sandwich**: hold the highs back
//! from the shaper and restore them after, so what survives is
//! frequency-dependent *distortion* rather than frequency-dependent *level*.
//! That is a Wiener-Hammerstein model -- linear, static nonlinear, linear --
//! and it is [`crate::preamp`], which is where `tilt_db` lives.
//!
//! What *is* claimed here is a stated harmonic target, hit at a stated level,
//! tested — and, since 2026-09-10, cited to a measured unit.
/// Harmonics a profile describes: the 2nd and the 3rd.
///
/// Two, not four, and this module's header says why: at the operating level
/// the 4th and 5th cannot be authored without wrecking the fundamental, and
/// a profile that carried them delivered them 80 dB down regardless.
pub const PROFILE_HARMONICS: usize = 2;

/// The level a profile's harmonics are stated at.
///
/// mooloop's unity operating level, so a voicing is authored where the
/// material is. Taken from `mooloop_core::gain::REFERENCE_PEAK_DBFS` rather
/// than spelt again here: a number written down twice is this codebase's
/// characteristic fault, and this one is load-bearing in both places.
pub const PROFILE_REFERENCE_DBFS: f32 = mooloop_core::gain::REFERENCE_PEAK_DBFS;

/// [`PROFILE_REFERENCE_DBFS`] as an amplitude. About `0.2512`.
pub(crate) fn reference_amplitude() -> f32 {
    10.0f32.powf(PROFILE_REFERENCE_DBFS / 20.0)
}

/// A voicing's harmonic signature, in **dB below the fundamental, at
/// [`PROFILE_REFERENCE_DBFS`]**, for harmonics 2 and 3.
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

    pub const fn new(second: f32, third: f32) -> Self {
        Self {
            harmonics_db: [second, third],
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
// **Each is now a measured unit rather than a guess**, read at 1 kHz and
// -12 dBFS from the 106-unit survey in `spikes/preamp-measure/` — see
// `DATA.md` there for the method and `out/tidy/colour_thd_vs_level.csv` for
// the rows. Each is quoted at the drive setting where its reference makes
// **1% THD at the operating level**, which is the one comparison that means
// the same thing across makers: "Drive 6" does not, and 1% does.
//
// The previous numbers were picked by ear against a guess about level, and
// the guess was the part that was wrong — see the header on what `Iron`
// actually delivered at -12 dBFS versus what its doc comment claimed.

/// `Moo` — the house voicing. Calibrated and uncoloured.
pub const MOO: HarmonicProfile = HarmonicProfile::TRANSPARENT;

/// `Grip` — fast, tight, controlled. Low distortion and mostly odd-order,
/// which is what a well-behaved VCA-ish stage measures like.
///
/// Waves NLS Channel "Mike" (SSL 4000 G+) at 1% THD: the 3rd sits 16 dB
/// **above** the 2nd, which is the push-pull signature and the reason this
/// voicing reads as control rather than as warmth.
pub const GRIP: HarmonicProfile = HarmonicProfile::new(-56.4, -40.3);

/// `Punch` — forward and thick. Both orders present and enough of them to
/// hear on a snare.
///
/// UA 610-A at 1% THD, with UAD's Century Tube Channel agreeing within
/// 1.5 dB on both harmonics — two units of the same kind reading the same
/// way, which is worth more than either alone. The two orders sit within
/// 5 dB of each other, which is what "both present" means as a number.
pub const PUNCH: HarmonicProfile = HarmonicProfile::new(-41.5, -46.9);

/// `Iron` — transformer warmth: even-order dominant, the one whose
/// distortion is the point.
///
/// Waves NLS Channel "Spike" (EMI TG12345) at 1% THD, the 2nd 11 dB above
/// the 3rd. **Not a Neve**, and that is a finding rather than a
/// substitution: every Neve channel in the survey measures *odd*-dominant —
/// NLS "Nevo" by 10.5 dB, Brainworx bx_console N by 49 dB — so the
/// even-order transformer character this voicing is named for belongs to the
/// EMI desk. `spikes/preamp-measure/RESULTS.md` §5 item 4 asked which one
/// `Iron` was; this is the answer the wider pass gives.
pub const IRON: HarmonicProfile = HarmonicProfile::new(-40.4, -51.7);

// --- The shaper -------------------------------------------------------------

/// A memoryless curve that realises a [`HarmonicProfile`].
///
/// Cheap enough to sit on every strip: three multiply-accumulates on the
/// polynomial, evaluated by the Chebyshev recurrence so no `powi` is needed,
/// and skipped entirely when the profile is transparent. The solve happens
/// once, in [`Self::new`].
#[derive(Debug, Clone, Copy)]
pub struct HarmonicShaper {
    /// `a₂..a₃` in linear amplitude. `a₁` is 1 and is not stored.
    coefficients: [f32; PROFILE_HARMONICS],
    /// Restores the fundamental the `T₃` term takes away.
    ///
    /// `T₃`'s linear part is `-3x`, so a curve that makes a 3rd harmonic also
    /// attenuates: `Grip` loses 3.1 dB without this, which is a *level*
    /// change, and `GAIN_STRUCTURE.md` is explicit that raising drive changes
    /// character and not level. Set so the fundamental is exactly unity at
    /// [`PROFILE_REFERENCE_DBFS`]; below that it drifts by under 0.3 dB,
    /// because the small-signal gain `1 - 3a₃` and the reference-level gain
    /// are nearly the same number.
    makeup: f32,
    transparent: bool,
}

/// The coefficients that make `profile` measure true at amplitude `a`, and
/// the fundamental they leave behind.
///
/// Closed form, from the spill relations in this module's header:
///
/// ```text
///   H₃ = -a₃a³        = -t₃·F     with  F = a + a₃(3a³ - 3a)
///   H₂ = -a₂a²        = -t₂·F
/// ```
///
/// The first is one linear equation in `a₃` once `F` is substituted; the
/// second then falls out. Done in `f64` because `a³` is `1.6e-2` at the
/// reference and the subtraction below it is where the precision goes.
fn solve_at(profile: HarmonicProfile, a: f32) -> ([f32; PROFILE_HARMONICS], f32) {
    let linear = |db: f32| -> f64 {
        if db <= SILENT_HARMONIC_DB {
            0.0
        } else {
            10.0f64.powf(db as f64 / 20.0)
        }
    };
    let (t2, t3) = (
        linear(profile.harmonics_db[0]),
        linear(profile.harmonics_db[1]),
    );
    let a = a as f64;
    let (a2p, a3p) = (a * a, a * a * a);

    // How much of the fundamental `T₃` carries at this amplitude. Zero at
    // full scale, which is what makes this reduce to the identity there.
    let spill = 3.0 * a3p - 3.0 * a;

    // -a₃a³ = -t₃(a + a₃·spill)  =>  a₃(-a³ + t₃·spill) = -t₃·a
    let denominator = -a3p + t3 * spill;
    let a3 = if denominator.abs() < f64::EPSILON {
        0.0
    } else {
        -t3 * a / denominator
    };
    let fundamental = a + a3 * spill;
    let a2 = if a2p > 0.0 { t2 * fundamental / a2p } else { 0.0 };

    ([a2 as f32, a3 as f32], (fundamental / a) as f32)
}

impl HarmonicShaper {
    /// The shaper that hits `profile` at [`PROFILE_REFERENCE_DBFS`].
    pub fn new(profile: HarmonicProfile) -> Self {
        Self::at_amplitude(profile, reference_amplitude())
    }

    /// As [`Self::new`], for an arbitrary reference amplitude.
    ///
    /// Exists for the tests, which need to ask what the curve does at a level
    /// it was not authored at, and to pin the `a = 1` reduction.
    fn at_amplitude(profile: HarmonicProfile, a: f32) -> Self {
        let (coefficients, fundamental) = solve_at(profile, a);
        Self {
            coefficients,
            makeup: if fundamental.abs() > 1e-6 {
                1.0 / fundamental
            } else {
                1.0
            },
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
        out * self.makeup
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
    /// what a sine *at the operating level* actually measures coming out of
    /// it — which is the whole of the 2026-09-10 change, since the previous
    /// version of this test asserted it at full scale and was true there and
    /// wrong everywhere music is.
    ///
    /// The tolerance is 0.5 dB, which is far tighter than any of these
    /// numbers will ever be authored to; it is that tight because the solve
    /// is closed-form and anything looser would stop noticing if it broke.
    #[test]
    fn harmonics_hit_their_stated_target_at_the_operating_level() {
        let amplitude = reference_amplitude();
        for (name, profile) in [("Grip", GRIP), ("Punch", PUNCH), ("Iron", IRON)] {
            let samples = shaped_sine(profile, amplitude);
            let fundamental = harmonic_amplitude(&samples, 1);
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

    /// The stage does not change the level at the level it is authored at.
    ///
    /// `T₃`'s linear part is `-3x`, so a curve that makes a 3rd harmonic
    /// attenuates unless something puts the fundamental back;
    /// `HarmonicShaper::makeup` is that. Without it `Grip` runs 3.1 dB quiet,
    /// and `GAIN_STRUCTURE.md` says raising drive changes character and not
    /// level. This is that sentence as a number.
    #[test]
    fn the_stage_is_unity_at_the_operating_level() {
        let amplitude = reference_amplitude();
        for (name, profile) in [("Grip", GRIP), ("Punch", PUNCH), ("Iron", IRON)] {
            let samples = shaped_sine(profile, amplitude);
            let gain = db(harmonic_amplitude(&samples, 1) / amplitude);
            println!("{name} fundamental at the operating level: {gain:+.3} dB");
            assert!(gain.abs() < 0.1, "{name} moved the fundamental by {gain:+.2} dB");
        }

        // And stays near unity below it, where the makeup is no longer exact.
        for (name, profile) in [("Grip", GRIP), ("Punch", PUNCH), ("Iron", IRON)] {
            for quieter in [amplitude * 0.5, amplitude * 0.1, amplitude * 0.01] {
                let samples = shaped_sine(profile, quieter);
                let gain = db(harmonic_amplitude(&samples, 1) / quieter);
                assert!(
                    gain.abs() < 0.3,
                    "{name} at amplitude {quieter:.4} has gain {gain:+.2} dB"
                );
            }
        }
    }

    /// The colour recedes as the signal quietens, which is limit 1 in this
    /// module's own header and the property that makes it playable.
    ///
    /// **Monotonic, not a fixed dB law.** Measured downward from the
    /// operating level, where the profile is stated, rather than from full
    /// scale. What is worth holding is that every harmonic goes *down* and
    /// keeps going down, because that is what a well-meaning normalization
    /// somewhere upstream would destroy.
    #[test]
    fn the_profile_thins_as_the_signal_quietens() {
        let reference = reference_amplitude();
        for (name, profile) in [("Grip", GRIP), ("Punch", PUNCH), ("Iron", IRON)] {
            for n in 2..=3 {
                let mut previous = f32::INFINITY;
                for step in [1.0, 0.5, 0.25, 0.125] {
                    let amplitude = reference * step;
                    let samples = shaped_sine(profile, amplitude);
                    let ratio =
                        db(harmonic_amplitude(&samples, n) / harmonic_amplitude(&samples, 1));
                    assert!(
                        ratio < previous - 1.0,
                        "{name} harmonic {n} did not recede at amplitude {amplitude}: \
                         {ratio:.2} dB against {previous:.2} dB"
                    );
                    previous = ratio;
                }
                println!("{name} harmonic {n}: recedes to {previous:.1} dB by -36 dBFS");
            }
        }
    }

    /// The colour also *grows* above the operating level, and how fast is the
    /// thing the survey says a polynomial gets wrong.
    ///
    /// A Chebyshev term forces its harmonic to rise `n-1` dB per dB of level:
    /// 1 for the 2nd, 2 for the 3rd. Measured units are flatter than that —
    /// a median of +0.79 and +1.28 dB/dB over 186 unit-settings in
    /// `spikes/preamp-measure/out/tidy/colour_thd_vs_level.csv`. This test
    /// does not assert the references' law, because the shaper cannot hold
    /// it; it pins the shaper's own so the discrepancy stays visible and a
    /// future level-dependent scheme has something to beat.
    #[test]
    fn the_level_law_is_the_polynomials_and_not_the_references() {
        let reference = reference_amplitude();
        let slope = |profile: HarmonicProfile, n: usize| {
            let at = |amplitude: f32| {
                let samples = shaped_sine(profile, amplitude);
                db(harmonic_amplitude(&samples, n) / harmonic_amplitude(&samples, 1))
            };
            // Six dB of level, taken below the reference so the clamp is
            // nowhere near it.
            (at(reference) - at(reference * 0.5)) / 6.0
        };
        for (name, profile) in [("Grip", GRIP), ("Punch", PUNCH), ("Iron", IRON)] {
            for (n, expected) in [(2usize, 1.0f32), (3, 2.0)] {
                let measured = slope(profile, n);
                println!("{name} harmonic {n}: {measured:.3} dB per dB (polynomial says {expected})");
                assert!(
                    (measured - expected).abs() < 0.1,
                    "{name} harmonic {n} rises {measured:.3} dB/dB, not {expected}"
                );
            }
        }
    }

    /// At full scale the solve has nothing to do, and must reduce to the
    /// direct mapping the Chebyshev identity gives.
    ///
    /// This is what makes the change safe to reason about: the new scheme is
    /// a strict generalization of the old one, and `a = 1` is the old one.
    #[test]
    fn full_scale_solves_to_the_coefficients_themselves() {
        for (name, profile) in [("Grip", GRIP), ("Punch", PUNCH), ("Iron", IRON)] {
            let (coefficients, fundamental) = solve_at(profile, 1.0);
            assert!(
                (fundamental - 1.0).abs() < 1e-6,
                "{name} needs makeup at full scale: {fundamental}"
            );
            for (coefficient, db_target) in coefficients.iter().zip(profile.harmonics_db) {
                let expected = 10.0f32.powf(db_target / 20.0);
                assert!(
                    (coefficient - expected).abs() < 1e-6,
                    "{name}: solved {coefficient} against the identity's {expected}"
                );
            }
        }
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
    ///
    /// The bound is 1.7 rather than the old 1.5 because the coefficients are
    /// larger now — a profile stated at -12 dBFS needs more curve than one
    /// stated at full scale, and `Tₙ(1) = 1` for every `n`, so the peak is
    /// `makeup·(1 + Σaₙ)`. What matters is that it is finite and that a
    /// hotter sample cannot make it worse.
    #[test]
    fn a_hot_sample_saturates_instead_of_exploding() {
        for (name, profile) in [("Grip", GRIP), ("Punch", PUNCH), ("Iron", IRON)] {
            let shaper = HarmonicShaper::new(profile);
            assert_eq!(shaper.shape(4.0), shaper.shape(1.0), "{name}");
            assert_eq!(shaper.shape(-4.0), shaper.shape(-1.0), "{name}");
            let peak = shaper.shape(1.0).abs();
            println!("{name} peaks at {peak:.3} for a full-scale sample");
            assert!(peak < 1.7, "{name} peaks at {peak}");
        }
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
