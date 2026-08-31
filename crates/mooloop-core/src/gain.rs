//! The shared definition of decibels.
//!
//! One module for every dB/linear conversion, the +12 dB gain ceiling, and
//! the mixer fader taper, so a control's readout and its audio can never
//! disagree. Reference document: `docs/plans/gain-structure/01-the-gain-contract.md`.
//!
//! These run per control change, not per sample: clarity beats speed.

/// Floor of every dB readout and scale: -inf collapses here.
pub const MIN_DB: f32 = -60.0;

/// Ceiling of the trim range: +12 dB.
pub const MAX_DB: f32 = 12.0;

/// Where a peak meter turns from green to yellow. The conventional digital
/// peak warning point, and the one the reference level gives headroom
/// against: the operating level sits 2 dB below it.
pub const METER_WARNING_DB: f32 = -10.0;

/// Where a peak meter turns red: 3 dB of headroom left before full scale.
pub const METER_HOT_DB: f32 = -3.0;

/// Largest persisted linear gain for a channel/device output, shared by the
/// UI trim controls and the engine's clamps. Slightly above 10^(12/20) so a
/// value produced from that dB conversion always remains representable.
pub const MAX_LINEAR_GAIN: f32 = 4.0;

/// Peak level a generator's default patch produces at default velocity with
/// its channel at unity. The headroom between this and 0 dBFS is what lets
/// sources sum without pulling the master down first. Device calibration
/// against this constant lives in each generator; `gain_structure_tests.rs`
/// pins the measurements.
pub const REFERENCE_PEAK_DBFS: f32 = -12.0;

/// A centred channel's per-side gain, `pan_gains(0.0)`: the equal-power pan
/// law spends 3.01 dB on both sides at centre. Every channel pays it, which
/// is why `REFERENCE_PEAK_DBFS` -- measured at the master -- sits that far
/// below where a generator's own output actually peaks.
pub const CENTRE_PAN_DB: f32 = -3.0103;

/// Where a calibrated generator's *device output* peaks, as opposed to
/// `REFERENCE_PEAK_DBFS`, which is the same patch measured at the master
/// after the pan law. This is the level a source has to hit to sit level
/// with the others in the rack, at any pan position, since pan attenuates
/// every channel identically.
pub const GENERATOR_OUTPUT_REFERENCE_DBFS: f32 = REFERENCE_PEAK_DBFS - CENTRE_PAN_DB;

/// The generator output reference as a linear gain. A stage that has to put
/// an uncalibrated full-scale source level with the calibrated generators
/// spends exactly this much: the sampler's default output trim and the
/// browser's audition monitor both start here, which is what makes them
/// agree with each other and with the rest of the rack.
pub fn reference_level_gain() -> f32 {
    db_to_linear(GENERATOR_OUTPUT_REFERENCE_DBFS)
}

/// At or below `MIN_DB` is silence (0.0), not residual gain.
pub fn linear_to_db(linear: f32) -> f32 {
    if !linear.is_finite() || linear <= 0.0 {
        return MIN_DB;
    }
    (20.0 * linear.log10()).max(MIN_DB)
}

/// Inverse of `linear_to_db` for the trim knobs, which work in dB. The
/// knob's floor means silence, not `-60 dB of residual gain`.
pub fn db_to_linear(db: f32) -> f32 {
    if !db.is_finite() || db <= MIN_DB {
        return 0.0;
    }
    10.0f32.powf(db / 20.0)
}

/// The mixer fader taper: linear in dB over travel, piecewise between these
/// `(travel, dB)` breakpoints, interpolated in dB. Unity sits at
/// three-quarter travel; the top of the throw is +6 dB. Rust and Slint must
/// mirror this table exactly — `gain.slint` carries a copy and a test in
/// `mooloop-ui` fails if the two lists diverge.
pub const FADER_BREAKPOINTS: [(f32, f32); 7] = [
    (1.00, 6.0),
    (0.75, 0.0),
    (0.50, -12.0),
    (0.30, -24.0),
    (0.15, -40.0),
    (0.05, -60.0),
    (0.00, f32::NEG_INFINITY),
];

/// Fader travel (0..1) to gain in dB, per the breakpoint table. Travel 0 is
/// silence, represented as negative infinity; travel between 0 and the -60 dB
/// breakpoint holds the floor (`MIN_DB`), because interpolating towards an
/// infinite dB is meaningless — everything there reads as off anyway.
pub fn fader_position_to_db(position: f32) -> f32 {
    let position = position.clamp(0.0, 1.0);
    if position <= 0.0 {
        return f32::NEG_INFINITY;
    }
    for window in FADER_BREAKPOINTS.windows(2) {
        let (top_travel, top_db) = window[0];
        let (bottom_travel, bottom_db) = window[1];
        if position > bottom_travel {
            if !bottom_db.is_finite() {
                return MIN_DB;
            }
            let fraction = (position - bottom_travel) / (top_travel - bottom_travel);
            return bottom_db + fraction * (top_db - bottom_db);
        }
    }
    f32::NEG_INFINITY
}

/// Inverse of `fader_position_to_db`. Only true silence (negative infinity,
/// or anything at/below the -60 dB breakpoint's floor end) maps below the
/// bottom breakpoint's travel; +6 dB and above is full throw.
pub fn fader_db_to_position(db: f32) -> f32 {
    if !db.is_finite() {
        return 0.0;
    }
    if db >= FADER_BREAKPOINTS[0].1 {
        return 1.0;
    }
    for window in FADER_BREAKPOINTS.windows(2) {
        let (top_travel, top_db) = window[0];
        let (bottom_travel, bottom_db) = window[1];
        if db > bottom_db {
            if !bottom_db.is_finite() {
                return top_travel;
            }
            let fraction = (db - bottom_db) / (top_db - bottom_db);
            return (bottom_travel + fraction * (top_travel - bottom_travel)).clamp(0.0, 1.0);
        }
    }
    1.0
}

/// The one dB readout format, matching `TrimKnob`'s strings: `-inf`,
/// `±0.0 dB`, `+3.0 dB`, `-12.4 dB`. Slint mirrors this in `GainMath`.
pub fn format_db(db: f32) -> String {
    if !db.is_finite() || db <= MIN_DB + 0.05 {
        return "-inf".to_string();
    }
    if db.abs() < 0.05 {
        return "±0.0 dB".to_string();
    }
    format!(
        "{}{:.1} dB",
        if db > 0.0 { "+" } else { "" },
        (db * 10.0).round() / 10.0
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_linear_amplitude_to_db() {
        assert_eq!(linear_to_db(0.0), MIN_DB);
        assert!((linear_to_db(0.5) - -6.0206).abs() < 0.001);
        assert_eq!(linear_to_db(1.0), 0.0);
    }

    #[test]
    fn trim_db_round_trips_through_linear() {
        assert_eq!(db_to_linear(0.0), 1.0);
        assert!((db_to_linear(-6.0206) - 0.5).abs() < 0.001);
        assert_eq!(db_to_linear(MIN_DB), 0.0, "the knob floor is silence");
        assert_eq!(linear_to_db(db_to_linear(-12.0)), -12.0);
    }

    #[test]
    fn fader_matches_the_contract_breakpoints() {
        for (travel, db) in FADER_BREAKPOINTS {
            if !db.is_finite() {
                assert_eq!(fader_position_to_db(travel), f32::NEG_INFINITY);
            } else {
                assert!((fader_position_to_db(travel) - db).abs() < 0.001);
            }
        }
        assert_eq!(fader_position_to_db(0.0), f32::NEG_INFINITY);
    }

    #[test]
    fn fader_round_trips_across_the_whole_throw() {
        // Everything at/below the -60 dB breakpoint is the off floor: it
        // reads -inf at travel 0 and holds MIN_DB down to it, so exact
        // round-tripping starts at the bottom breakpoint.
        assert_eq!(fader_db_to_position(fader_position_to_db(0.0)), 0.0);
        assert_eq!(fader_db_to_position(fader_position_to_db(0.05)), 0.05,);
        assert_eq!(fader_position_to_db(0.02), MIN_DB);
        let mut position = 0.05;
        while position <= 1.0 {
            let round = fader_db_to_position(fader_position_to_db(position));
            assert!(
                (round - position).abs() < 1e-4,
                "travel {position} round-tripped to {round}"
            );
            position += 0.005;
        }
    }

    #[test]
    fn fader_interpolates_in_db_between_breakpoints() {
        // Between 0.75 (0 dB) and 1.0 (+6 dB), mid-travel interpolation is in
        // dB, so travel 0.875 reads +3 dB, not the dB of +3 dB-linear.
        assert!((fader_position_to_db(0.875) - 3.0).abs() < 0.001);
    }

    #[test]
    fn formats_like_the_trim_knob() {
        assert_eq!(format_db(f32::NEG_INFINITY), "-inf");
        assert_eq!(format_db(MIN_DB), "-inf");
        assert_eq!(format_db(0.0), "±0.0 dB");
        assert_eq!(format_db(0.04), "±0.0 dB");
        assert_eq!(format_db(3.0), "+3.0 dB");
        assert_eq!(format_db(-12.44), "-12.4 dB");
    }
}
