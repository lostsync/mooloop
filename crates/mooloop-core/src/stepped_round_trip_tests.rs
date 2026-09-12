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

/// `EQ_PARAM_CHARACTER` is the one id this check cannot pass, and the reason
/// is a defect rather than a quirk of the check. See
/// `one_id_decodes_to_two_enums_of_different_arity` below, which pins what it
/// actually does; `docs/LOOSE_ENDS.md` carries the decision it is waiting on.
const KNOWN_BAD: &[(EffectKind, u32)] = &[(EffectKind::Eq, crate::effect::EQ_PARAM_CHARACTER)];

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
            if KNOWN_BAD.contains(&(kind, descriptor.id)) {
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

/// **The defect the exclusion above names.** `EQ_PARAM_CHARACTER` is one id
/// that decodes to two different enums, chosen by which target the EQ has
/// selected, and the two do not have the same number of variants:
/// `EqSlope` has five (Db6..Db36) and `EqQProfile` has two.
///
/// The descriptor is sized for the slope, `Stepped(5)`. So when a *band* is
/// selected, positions 1 through 4 all mean `Proportional`, and all four read
/// back as position 1. The face never sends the other three -- `eq-device.slint`
/// drives the band's control as a two-state toggle sending 0 or 0.25 -- so this
/// is not reachable by hand. An automation lane is not the face: a lane drawn
/// at half travel writes position 2, and the value that comes back is position
/// 1, so the lane and the parameter disagree about what the lane says.
///
/// Fixing it properly means a second descriptor id, and ids are frozen once
/// shipped -- "never renumber a shipped id, append instead" -- so it is a
/// decision about the project format rather than a correction. This test
/// records exactly what happens in the meantime, so the day somebody changes
/// it, this fails and says so.
#[test]
fn one_id_decodes_to_two_enums_of_different_arity() {
    use crate::effect::{EqQProfile, EqSlope};

    // The two decodes this id can reach, and the count each one has.
    assert_eq!(EqSlope::from_index(4).to_index(), 4, "five slopes, all reachable");
    assert_eq!(
        EqQProfile::from_index(4).to_index(),
        1,
        "two profiles: position 4 collapses onto position 1"
    );

    // And what that does through the parameter path, on a band.
    let descriptor = EffectKind::Eq
        .descriptor(crate::effect::EQ_PARAM_CHARACTER)
        .expect("the EQ has a Shape parameter");
    assert!(
        matches!(descriptor.curve, ParamCurve::Stepped(5)),
        "sized for the slope, which is the half of this that is correct"
    );

    let collapsed: Vec<f32> = positions(descriptor)
        .into_iter()
        .map(|position| {
            let mut params = EffectKind::Eq.default_params();
            params.set(descriptor.id, position);
            params.get(descriptor.id).expect("Shape reads back")
        })
        .collect();
    assert_eq!(
        collapsed,
        vec![0.0, 1.0, 1.0, 1.0, 1.0],
        "four of the five positions mean Proportional and read back as one"
    );
}
