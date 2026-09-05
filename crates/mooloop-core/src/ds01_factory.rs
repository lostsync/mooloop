//! The DS-01 factory bank.
//!
//! Seventeen patches, defined here as data rather than as files, for the same
//! two reasons `mlm1_factory` gives. The DSP tests need the same values the
//! preset seeder writes — a bank that only existed as TOML would have to be
//! parsed back to be tested, and the thing under test would be the parser.
//! And a patch is a set of parameters, so [`Ds01Params`] is its natural form;
//! the bundle on disk is a serialization of it, not the other way round.
//!
//! `docs/plans/drum-synth-v2/09-the-kit.md` names the list, and the claim it
//! is here to prove is DS-01's whole argument: **one universal percussion
//! voice reaches every drum type from range and factory patches rather than
//! from mode branches**. `mooloop_dsp::ds01`'s `one_architecture_reaches_a_kit`
//! asserts that mechanically against this bank, so the patches that are shipped
//! are the patches that are tested.
//!
//! These are generator presets, not channel presets. DS-01's modulation is
//! [`Ds01Params::matrix`] — inside the voice, where a drum needs it — so a
//! patch carries no [`crate::modulation::ModRack`] and needs no rescoping.
//! That is the difference from the ML-M1 bank, whose Sequence Bleep is
//! nothing without a channel rack.

use crate::ds01::{
    Ds01Character, Ds01EnvParams, Ds01ModSource, Ds01NoiseColor, Ds01PitchEnvParams, Ds01Route,
};
use crate::{ds01, Ds01Params};

/// One factory patch: presentation metadata plus the complete parameter set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ds01FactoryPatch {
    pub name: &'static str,
    pub category: &'static str,
    pub tags: &'static [&'static str],
    /// One line on what the patch is for, and what it demonstrates.
    pub description: &'static str,
    pub params: Ds01Params,
}

/// How many patches the bank ships. Named because the seeder's test asserts
/// the count and a bank that quietly shrank would otherwise still pass.
pub const BANK_SIZE: usize = 17;

/// The bank, in the order it should be presented: kicks, snares, hands,
/// toms, metal, and the two that are neither.
pub fn patches() -> [Ds01FactoryPatch; BANK_SIZE] {
    [
        sub_kick(),
        kit_kick(),
        dnb_kick(),
        tight_snare(),
        deep_snare(),
        ghost_snare(),
        rimshot(),
        clap(),
        tom_low(),
        tom_mid(),
        tom_high(),
        closed_hat(),
        open_hat(),
        ride(),
        cowbell(),
        clave(),
        zap(),
    ]
}

/// Every patch starts from the default and changes what it needs, so the diff
/// between this file and `Ds01Params::default()` *is* the "from the controls
/// rather than from hand-tuning" claim, checkable by reading.
fn base() -> Ds01Params {
    Ds01Params::default()
}

/// A one-shot amplitude or noise envelope of `decay` seconds. Every patch in
/// the bank shapes its layers with one, because a drum is a hit.
const fn env(decay: f32) -> Ds01EnvParams {
    Ds01EnvParams::one_shot(decay)
}

fn sub_kick() -> Ds01FactoryPatch {
    Ds01FactoryPatch {
        name: "Sub Kick",
        category: "DS-01",
        tags: &["kick", "sub", "clean"],
        description: "Tone alone, deep and long, with no noise at all.",
        params: Ds01Params {
            tone_pitch: 45.0,
            pitch: Ds01PitchEnvParams {
                attack: 0.0,
                decay: 0.06,
                curve: 0.0,
                depth: 24.0,
            },
            amp: env(0.6),
            ..base()
        },
    }
}

fn kit_kick() -> Ds01FactoryPatch {
    Ds01FactoryPatch {
        name: "Kit Kick",
        category: "DS-01",
        tags: &["kick", "acoustic"],
        description: "The default hit with a short noise click on the front.",
        params: Ds01Params {
            noise_level: 0.35,
            filter_cutoff: 3_000.0,
            noise_env: env(0.008),
            ..base()
        },
    }
}

fn dnb_kick() -> Ds01FactoryPatch {
    Ds01FactoryPatch {
        name: "DnB Kick",
        category: "DS-01",
        tags: &["kick", "drive", "fold"],
        description: "Fold drive over a fast pitch sweep: dense without being louder.",
        params: Ds01Params {
            drive: 0.8,
            character: Ds01Character::Fold,
            pitch: Ds01PitchEnvParams {
                attack: 0.0,
                decay: 0.03,
                curve: 0.0,
                depth: 30.0,
            },
            amp: env(0.35),
            ..base()
        },
    }
}

fn tight_snare() -> Ds01FactoryPatch {
    Ds01FactoryPatch {
        name: "Tight Snare",
        category: "DS-01",
        tags: &["snare", "short"],
        description: "Tone body under band-passed noise, both short.",
        params: tight_snare_params(),
    }
}

/// Shared, because Ghost Snare is this patch plus two matrix rows and the
/// step's acceptance case is that the *difference* is the two rows.
fn tight_snare_params() -> Ds01Params {
    Ds01Params {
        tone_pitch: 190.0,
        noise_level: 0.8,
        filter_morph: 0.5,
        filter_cutoff: 2_500.0,
        filter_res: 0.3,
        amp: env(0.12),
        noise_env: env(0.09),
        pitch: Ds01PitchEnvParams {
            depth: 8.0,
            ..base().pitch
        },
        ..base()
    }
}

fn deep_snare() -> Ds01FactoryPatch {
    Ds01FactoryPatch {
        name: "Deep Snare",
        category: "DS-01",
        tags: &["snare", "body"],
        description: "The same, longer, with the body layer at a low Ratio underneath.",
        params: Ds01Params {
            tone_pitch: 160.0,
            noise_level: 0.7,
            filter_morph: 0.5,
            filter_cutoff: 1_800.0,
            body_level: 0.4,
            body_pitch: 180.0,
            amp: env(0.25),
            noise_env: env(0.2),
            ..base()
        },
    }
}

/// The acceptance case for the whole instrument.
///
/// A quiet hit is a *different* sound — shorter and duller — rather than the
/// same sound turned down, and it is built from two ordinary matrix rows
/// rather than from anything the device does specially. The plain Velocity
/// Amount steps back to 0.4 because the routes carry velocity now; leaving it
/// at 1.0 would apply velocity twice.
fn ghost_snare() -> Ds01FactoryPatch {
    let mut params = Ds01Params {
        velocity_amount: 0.4,
        amp: env(0.05),
        filter_cutoff: 900.0,
        ..tight_snare_params()
    };
    params.matrix[0] = Ds01Route {
        source: Ds01ModSource::Velocity,
        dest: ds01::PARAM_AMP_DECAY,
        amount: 0.35,
        curve: 0.0,
    };
    params.matrix[1] = Ds01Route {
        source: Ds01ModSource::Velocity,
        dest: ds01::PARAM_FILTER_CUTOFF,
        amount: 0.35,
        curve: 0.0,
    };
    Ds01FactoryPatch {
        name: "Ghost Snare",
        category: "DS-01",
        tags: &["snare", "velocity", "matrix"],
        description: "Tight Snare with velocity on decay and cutoff: a quiet hit is a different sound.",
        params,
    }
}

fn rimshot() -> Ds01FactoryPatch {
    Ds01FactoryPatch {
        name: "Rimshot",
        category: "DS-01",
        tags: &["rim", "body", "short"],
        description: "Body at a high Ratio struck by the impulse, and over almost at once.",
        params: Ds01Params {
            tone_level: 0.2,
            noise_level: 0.2,
            body_level: 1.0,
            body_ratio: 0.9,
            body_pitch: 420.0,
            body_decay: 0.08,
            amp: env(0.06),
            ..base()
        },
    }
}

fn clap() -> Ds01FactoryPatch {
    Ds01FactoryPatch {
        name: "Clap",
        category: "DS-01",
        tags: &["clap", "burst"],
        description: "Four accelerating noise impulses inside one envelope — the burst's reason to exist.",
        params: Ds01Params {
            tone_level: 0.0,
            noise_level: 1.0,
            filter_morph: 0.5,
            filter_cutoff: 1_200.0,
            burst_repeats: 4,
            burst_spacing: 0.011,
            burst_spread: -0.6,
            burst_level_step: -0.3,
            amp: env(0.25),
            noise_env: env(0.02),
            ..base()
        },
    }
}

/// One tom, three tunings. Tune is which note this drum is, latched with the
/// note, and the resonator's decay is a time in seconds at every pitch — so
/// the three are the same patch with `tune` moved, not three authored sounds.
/// `one_tom_patch_tunes_across_a_range` is the assertion that they stay one
/// drum, and it would fail if that stopped being true.
fn tom(name: &'static str, tune: f32, description: &'static str) -> Ds01FactoryPatch {
    Ds01FactoryPatch {
        name,
        category: "DS-01",
        tags: &["tom", "body"],
        description,
        params: Ds01Params {
            tune,
            tone_level: 0.5,
            tone_pitch: 150.0,
            body_level: 0.9,
            body_pitch: 150.0,
            body_decay: 0.5,
            pitch: Ds01PitchEnvParams {
                depth: 8.0,
                ..base().pitch
            },
            amp: env(0.5),
            ..base()
        },
    }
}

fn tom_low() -> Ds01FactoryPatch {
    tom("Tom Low", -12.0, "One tom patch, an octave down.")
}

fn tom_mid() -> Ds01FactoryPatch {
    tom("Tom Mid", 0.0, "The tom at its written pitch.")
}

fn tom_high() -> Ds01FactoryPatch {
    tom("Tom High", 12.0, "The same tom an octave up, and still the same drum.")
}

/// The hats share a choke group, which is what makes a closed hat cut an open
/// one the way a real pair does. They are one patch at two decays otherwise.
fn hat(
    name: &'static str,
    amp_decay: f32,
    noise_decay: f32,
    description: &'static str,
) -> Ds01FactoryPatch {
    Ds01FactoryPatch {
        name,
        category: "DS-01",
        tags: &["hat", "metal", "choke"],
        description,
        params: Ds01Params {
            tone_pitch: 320.0,
            tone_wave: 1.0,
            tone_partials: 6,
            tone_spread: 1.0,
            noise_level: 0.6,
            noise_color: Ds01NoiseColor::Metal,
            filter_cutoff: 8_000.0,
            choke_group: 1,
            amp: env(amp_decay),
            noise_env: env(noise_decay),
            pitch: Ds01PitchEnvParams {
                depth: 0.0,
                ..base().pitch
            },
            ..base()
        },
    }
}

fn closed_hat() -> Ds01FactoryPatch {
    hat(
        "Closed Hat",
        0.05,
        0.04,
        "Six partials and metal noise, cut short — and in choke group 1.",
    )
}

fn open_hat() -> Ds01FactoryPatch {
    hat(
        "Open Hat",
        0.5,
        0.45,
        "The same hat left to ring, chased off by the closed one.",
    )
}

/// The gate is the point: a ride rings for as long as it is written, which is
/// a sound v1 cannot make at all. Noise excites a long, lightly damped body
/// rather than the body being struck.
fn ride() -> Ds01FactoryPatch {
    Ds01FactoryPatch {
        name: "Ride",
        category: "DS-01",
        tags: &["ride", "cymbal", "gate", "body"],
        description: "Noise driving a long, lightly damped body, held for the length of the note.",
        params: Ds01Params {
            tone_level: 0.15,
            tone_pitch: 520.0,
            tone_wave: 1.0,
            tone_partials: 6,
            tone_spread: 1.0,
            noise_level: 0.5,
            noise_color: Ds01NoiseColor::Metal,
            filter_cutoff: 9_000.0,
            body_level: 0.8,
            body_pitch: 1_050.0,
            body_ratio: 1.0,
            body_decay: 2.5,
            body_damping: 0.12,
            body_excite: 1.0,
            amp: Ds01EnvParams {
                attack: 0.0,
                hold: 0.0,
                decay: 2.2,
                curve: -0.3,
                sustain: 0.35,
                release: 0.6,
                gate: true,
            },
            noise_env: Ds01EnvParams {
                attack: 0.0,
                hold: 0.0,
                decay: 0.6,
                curve: 0.0,
                sustain: 0.2,
                release: 0.4,
                gate: true,
            },
            pitch: Ds01PitchEnvParams {
                depth: 0.0,
                ..base().pitch
            },
            ..base()
        },
    }
}

fn cowbell() -> Ds01FactoryPatch {
    Ds01FactoryPatch {
        name: "Cowbell",
        category: "DS-01",
        tags: &["cowbell", "metal"],
        description: "Two partials over a medium body — the 808 bell reached as a patch.",
        params: Ds01Params {
            tone_pitch: 540.0,
            tone_wave: 1.0,
            tone_partials: 2,
            tone_spread: 0.55,
            body_level: 0.5,
            body_ratio: 0.6,
            body_pitch: 540.0,
            body_decay: 0.25,
            amp: env(0.28),
            pitch: Ds01PitchEnvParams {
                depth: 0.0,
                ..base().pitch
            },
            ..base()
        },
    }
}

fn clave() -> Ds01FactoryPatch {
    Ds01FactoryPatch {
        name: "Clave",
        category: "DS-01",
        tags: &["clave", "woodblock", "body"],
        description: "Body alone, struck hard at a high Ratio and gone in fifty milliseconds.",
        params: Ds01Params {
            tone_level: 0.0,
            body_level: 1.0,
            body_ratio: 0.85,
            body_pitch: 1_200.0,
            body_decay: 0.05,
            amp: env(0.05),
            ..base()
        },
    }
}

fn zap() -> Ds01FactoryPatch {
    Ds01FactoryPatch {
        name: "Zap",
        category: "DS-01",
        tags: &["zap", "fx", "pitch"],
        description: "A large negative pitch depth: the sweep goes up into the hit instead of down.",
        params: Ds01Params {
            tone_pitch: 200.0,
            tone_wave: 0.6,
            pitch: Ds01PitchEnvParams {
                attack: 0.0,
                decay: 0.08,
                curve: 0.0,
                depth: -48.0,
            },
            amp: env(0.12),
            ..base()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Names are what a browser shows and what a file is called, so a
    /// duplicate would silently collapse two patches into one bundle.
    #[test]
    fn every_patch_has_its_own_name() {
        let bank = patches();
        for (index, patch) in bank.iter().enumerate() {
            assert!(!patch.name.is_empty());
            assert!(!patch.description.is_empty(), "{} has no line", patch.name);
            for other in &bank[index + 1..] {
                assert_ne!(patch.name, other.name, "two patches called {}", patch.name);
            }
        }
    }

    /// Every value has to be inside the range its own descriptor states, or
    /// the doctor would repair a shipped patch on the way in — which is the
    /// bank telling the user their factory content was broken.
    #[test]
    fn every_value_is_inside_its_descriptor() {
        for patch in patches() {
            for descriptor in ds01::DESCRIPTORS.iter() {
                let value = ds01::get(&patch.params, descriptor.id)
                    .unwrap_or_else(|| panic!("{} has no value for {}", patch.name, descriptor.id));
                assert!(
                    value >= descriptor.min && value <= descriptor.max,
                    "{}: {} is {value}, outside {}..{}",
                    patch.name,
                    descriptor.name,
                    descriptor.min,
                    descriptor.max
                );
            }
        }
    }

    /// The three toms are one patch at three tunings. If they ever stop being
    /// that, `09-the-kit.md`'s structural claim has quietly become false.
    #[test]
    fn the_toms_differ_only_in_tune() {
        let mid = tom_mid().params;
        for tuned in [tom_low().params, tom_high().params] {
            assert_eq!(
                Ds01Params {
                    tune: mid.tune,
                    ..tuned
                },
                mid
            );
        }
        assert_ne!(tom_low().params.tune, tom_high().params.tune);
    }

    /// And the ghost is the tight snare plus two matrix rows and the three
    /// controls those rows displace — not a separately authored sound.
    #[test]
    fn the_ghost_is_the_tight_snare_plus_two_rows() {
        let ghost = ghost_snare().params;
        assert_eq!(ghost.matrix[0].source, Ds01ModSource::Velocity);
        assert_eq!(ghost.matrix[1].source, Ds01ModSource::Velocity);
        let stripped = Ds01Params {
            matrix: tight_snare_params().matrix,
            velocity_amount: tight_snare_params().velocity_amount,
            amp: tight_snare_params().amp,
            filter_cutoff: tight_snare_params().filter_cutoff,
            ..ghost
        };
        assert_eq!(stripped, tight_snare_params());
    }
}
