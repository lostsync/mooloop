//! Every stepped parameter's positions survive a write and a read back.
//!
//! A `ParamCurve::Stepped(n)` descriptor states a parameter's range twice: as
//! `n` positions, and as `min`/`max`. Most of them then decode the value into
//! an enum, which states it a third time as a variant count -- and
//! `ParamDescriptor`'s own documentation says "a range written a second time
//! anywhere else is a bug". Nothing checked that the three agreed.
//!
//! The check is a round trip rather than a comparison of counts, because a
//! count is not reachable from a descriptor: 45 enums spell `from_index` by
//! hand, none of them in a form a test can enumerate. What *is* reachable is
//! the behaviour that matters. For every position a knob can produce, write it
//! through the params' own `set` and read it back with `get`. If the
//! descriptor offers more positions than the decode has answers, two positions
//! collapse onto one variant and the read back lands somewhere the write did
//! not.
//!
//! That makes this a test of the whole path rather than of a table, so it
//! covers the stepped parameters that are *not* enums as well -- voice counts,
//! partial counts, the phaser's stage count -- for free.
//!
//! Params are rebuilt for every descriptor rather than reused across the loop.
//! Some stepped parameters *select what the next id addresses*: the EQ's
//! `EQ_PARAM_TARGET` decides whether `EQ_PARAM_Q` means a band's Q or the
//! high-pass filter's, so a loop that left the target on its last position
//! would go on to test a different parameter than the one it named.

use crate::channel::DeviceKind;
use crate::effect::{EffectKind, ParamCurve, ParamDescriptor};
use crate::modulation::ModulatorKind;
use crate::strip::StripParams;

// Nothing is excluded any more. `EQ_PARAM_CHARACTER` was, and it was the one
// defect this check found: one id decoding to two enums of different arity, so
// four of its five positions collapsed onto one. It was split into
// `EQ_PARAM_PASS_SLOPE` and `EQ_PARAM_Q_PROFILE`, each with its own arity, and
// the exclusion went with it.

/// Every position a stepped descriptor declares, in natural units, as the
/// values a knob at each detent actually produces.
fn positions(descriptor: &ParamDescriptor) -> Vec<f32> {
    let ParamCurve::Stepped(steps) = descriptor.curve else {
        return Vec::new();
    };
    if steps <= 1 {
        return vec![descriptor.min];
    }
    (0..steps)
        .map(|index| descriptor.from_normalized(f32::from(index) / f32::from(steps - 1)))
        .collect()
}

/// A hundredth of the gap between two positions. Wide enough for the rounding
/// `from_normalized` does on a 21-position divider list, far narrower than the
/// distance to the neighbouring position, which is what a collapse looks like.
fn tolerance(descriptor: &ParamDescriptor) -> f32 {
    let ParamCurve::Stepped(steps) = descriptor.curve else {
        return 0.0;
    };
    if steps <= 1 {
        return 0.0;
    }
    ((descriptor.max - descriptor.min) / f32::from(steps - 1)).abs() * 0.01
}

#[test]
fn every_effect_stepped_position_reads_back_as_itself() {
    for kind in EffectKind::ALL {
        for descriptor in kind.descriptors() {
            if !matches!(descriptor.curve, ParamCurve::Stepped(_)) {
                continue;
            }
            for (index, position) in positions(descriptor).into_iter().enumerate() {
                let mut params = kind.default_params();
                params.set(descriptor.id, position);
                let read = params
                    .get(descriptor.id)
                    .unwrap_or_else(|| panic!("{kind:?} {} (id {}) has no get", descriptor.name, descriptor.id));
                assert!(
                    (read - position).abs() <= tolerance(descriptor),
                    "{kind:?} {} (id {}): position {index} is {position}, read back as {read}. \
                     The descriptor offers more positions than the value behind it has answers.",
                    descriptor.name,
                    descriptor.id,
                );
            }
        }
    }
}

/// The generators, which is where most of the stepped enums are: the four
/// sampler modes, the three drum characters, ML-M1's four switches, ML-P8's
/// eight and DS-01's four, plus every oscillator's wave and every LFO's.
#[test]
fn every_generator_stepped_position_reads_back_as_itself() {
    for kind in [
        DeviceKind::Sampler,
        DeviceKind::DrumSynth,
        DeviceKind::MonoSynth,
        DeviceKind::PolySynth,
        DeviceKind::MlM1,
        DeviceKind::MlP8,
        DeviceKind::Ds01,
        DeviceKind::AuxIn,
    ] {
        for descriptor in kind.descriptors() {
            if !matches!(descriptor.curve, ParamCurve::Stepped(_)) {
                continue;
            }
            for (index, position) in positions(descriptor).into_iter().enumerate() {
                let mut params = kind.default_generator_params();
                params.set(descriptor.id, position);
                let read = params.get(descriptor.id).unwrap_or_else(|| {
                    panic!("{kind:?} {} (id {}) has no get", descriptor.name, descriptor.id)
                });
                assert!(
                    (read - position).abs() <= tolerance(descriptor),
                    "{kind:?} {} (id {}): position {index} is {position}, read back as {read}. \
                     The descriptor offers more positions than the value behind it has answers.",
                    descriptor.name,
                    descriptor.id,
                );
            }
        }
    }
}

#[test]
fn every_modulator_stepped_position_reads_back_as_itself() {
    for kind in ModulatorKind::ALL {
        for descriptor in kind.descriptors() {
            if !matches!(descriptor.curve, ParamCurve::Stepped(_)) {
                continue;
            }
            for (index, position) in positions(descriptor).into_iter().enumerate() {
                let mut params = kind.default_params();
                params.set(descriptor.id, position);
                let read = params
                    .get(descriptor.id)
                    .unwrap_or_else(|| panic!("{kind:?} {} has no get", descriptor.name));
                assert!(
                    (read - position).abs() <= tolerance(descriptor),
                    "{kind:?} {} (id {}): position {index} is {position}, read back as {read}",
                    descriptor.name,
                    descriptor.id,
                );
            }
        }
    }
}

#[test]
fn every_strip_stepped_position_reads_back_as_itself() {
    for descriptor in StripParams::descriptors() {
        if !matches!(descriptor.curve, ParamCurve::Stepped(_)) {
            continue;
        }
        for (index, position) in positions(descriptor).into_iter().enumerate() {
            let mut params = StripParams::default();
            params.set(descriptor.id, position);
            let read = params
                .get(descriptor.id)
                .unwrap_or_else(|| panic!("strip {} has no get", descriptor.name));
            assert!(
                (read - position).abs() <= tolerance(descriptor),
                "strip {} (id {}): position {index} is {position}, read back as {read}",
                descriptor.name,
                descriptor.id,
            );
        }
    }
}

/// **The defect that exclusion named, and what replaced it.**
///
/// `EQ_PARAM_CHARACTER` was one id decoding to two enums: `EqSlope` when a pass
/// filter was the EQ's selected target, `EqQProfile` when a band was. Five
/// variants against two, with the descriptor sized for the slope, so on a band
/// positions 1 through 4 all meant `Proportional` and every one of them read
/// back as position 1. The face was safe, driving that control as a two-state
/// toggle; an automation lane is not the face, and a lane at half travel wrote
/// one setting and reported another, with nothing to say what to draw instead.
///
/// Two ids now, each with its own arity, and this checks the property the old
/// arrangement could not have: every declared position means a distinct setting
/// and reads back as itself. `every_effect_stepped_position_reads_back_as_itself`
/// above covers that generically -- what is pinned here is the *split*, so that
/// folding them back together fails on purpose.
#[test]
fn the_eq_shape_parameters_are_two_ids_with_their_own_arities() {
    use crate::effect::{EQ_PARAM_PASS_SLOPE, EQ_PARAM_Q_PROFILE, EQ_PARAM_TARGET};

    let slope = EffectKind::Eq
        .descriptor(EQ_PARAM_PASS_SLOPE)
        .expect("the EQ describes the pass slope");
    let profile = EffectKind::Eq
        .descriptor(EQ_PARAM_Q_PROFILE)
        .expect("the EQ describes the Q profile");
    assert!(
        matches!(slope.curve, ParamCurve::Stepped(5)),
        "five slopes: Db6, Db12, Db18, Db24, Db36"
    );
    assert!(
        matches!(profile.curve, ParamCurve::Stepped(2)),
        "two profiles: Constant and Proportional"
    );

    // The whole point, on a band: four positions used to mean Proportional and
    // read back as one. Now there are two positions and both survive.
    let mut params = EffectKind::Eq.default_params();
    params.set(EQ_PARAM_TARGET, 0.0);
    for position in positions(profile) {
        params.set(EQ_PARAM_Q_PROFILE, position);
        assert_eq!(
            params.get(EQ_PARAM_Q_PROFILE),
            Some(position),
            "a band's Q profile lost position {position}"
        );
    }

    // And on a pass filter the slope keeps all five, which is what it always
    // had -- the range at this id did not move, only its meaning narrowed.
    params.set(EQ_PARAM_TARGET, 1.0);
    for position in positions(slope) {
        params.set(EQ_PARAM_PASS_SLOPE, position);
        assert_eq!(
            params.get(EQ_PARAM_PASS_SLOPE),
            Some(position),
            "a pass filter's slope lost position {position}"
        );
    }
}
