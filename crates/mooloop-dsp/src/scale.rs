//! Normalized-knob-to-frequency mapping shared by every filter-cutoff-style
//! control: `20 * (max_hz / 20) ^ normalized` puts 0 at 20 Hz, 1 at
//! `max_hz`, and spaces perceptually even steps in between (each unit of
//! `normalized` covers the same number of octaves). Pull any new frequency
//! knob's mapping from here rather than re-deriving it.
//!
//! **A cutoff knob maps through [`cutoff_hz_from_normalized`]**, whose top is
//! the fixed [`CUTOFF_CEILING_HZ`]. Until 2026-09-22 every instrument passed
//! `0.45 * sample_rate` as `max_hz` instead, so one knob position was 3.8 kHz
//! at 48 kHz, 6.3 kHz at 96 and 10.7 kHz at 192, while every face read
//! 3.56 kHz (MOO-112). The sample rate belongs in one place only: the filter
//! primitive's own clamp, which keeps a cutoff below `0.45 * sample_rate`
//! where its coefficients are correct.

/// The bottom of every cutoff knob.
pub const CUTOFF_FLOOR_HZ: f32 = 20.0;

/// The top of every cutoff knob: 20 kHz at every sample rate. At 44.1 kHz the
/// filter primitives clamp just under it (`0.45 * 44_100` is 19.8 kHz), so the
/// last sliver of the knob is flat there and nowhere else.
pub const CUTOFF_CEILING_HZ: f32 = 20_000.0;

/// Map a normalized `0..=1` knob position to a frequency in Hz.
pub fn hz_from_normalized(normalized: f32, max_hz: f32) -> f32 {
    20.0 * (max_hz / 20.0).powf(normalized)
}

/// Invert [`hz_from_normalized`]: recover the knob position for a frequency.
/// Needed anywhere a stored or measured Hz value has to be displayed back on
/// the same normalized control.
pub fn normalized_from_hz(hz: f32, max_hz: f32) -> f32 {
    (hz.max(20.0) / 20.0).ln() / (max_hz / 20.0).ln()
}

/// The cutoff knob's law: [`CUTOFF_FLOOR_HZ`] at 0, [`CUTOFF_CEILING_HZ`] at 1,
/// the same frequency at every sample rate. Every instrument's Cutoff knob and
/// every face's cutoff readout mean this.
pub fn cutoff_hz_from_normalized(normalized: f32) -> f32 {
    hz_from_normalized(normalized, CUTOFF_CEILING_HZ)
}

/// Invert [`cutoff_hz_from_normalized`].
pub fn normalized_from_cutoff_hz(hz: f32) -> f32 {
    normalized_from_hz(hz, CUTOFF_CEILING_HZ)
}

/// Clamp a parameter into `low..=high`, sending NaN to `low`.
///
/// `f32::clamp` passes NaN straight through, and a NaN cutoff or resonance
/// that reaches a filter's coefficients poisons its state for good -- every
/// later sample is NaN, whatever the input (`reports/teams-2026-09-22.md`,
/// F5). `max` and `min` each drop a NaN operand, so this lands a NaN on the
/// bottom of the range and infinities on the nearer end. Same cost as a
/// `clamp`: two compares.
pub fn clamp_param(value: f32, low: f32, high: f32) -> f32 {
    value.max(low).min(high)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{cents, RATES};

    #[test]
    fn endpoints_land_on_20hz_and_max_hz() {
        let max_hz = 20_000.0;
        assert!((hz_from_normalized(0.0, max_hz) - 20.0).abs() < 1e-3);
        assert!((hz_from_normalized(1.0, max_hz) - max_hz).abs() < 1e-2);
    }

    #[test]
    fn normalized_from_hz_inverts_the_forward_map() {
        let max_hz = 18_000.0;
        for normalized in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let hz = hz_from_normalized(normalized, max_hz);
            let recovered = normalized_from_hz(hz, max_hz);
            assert!(
                (recovered - normalized).abs() < 1e-3,
                "{normalized} -> {hz} Hz -> {recovered}"
            );
        }
    }

    /// **The sample-rate contract (MOO-116).** The knob law takes no sample
    /// rate, so a knob position is one frequency at every rate; what a filter
    /// runs at is that frequency under its own `0.45 * sample_rate` clamp. So
    /// the only place the rates may disagree is above 19.8 kHz, the 44.1 kHz
    /// clamp -- the knob's last 0.3%.
    #[test]
    fn the_cutoff_law_is_one_frequency_at_every_rate() {
        assert!((cutoff_hz_from_normalized(0.0) - CUTOFF_FLOOR_HZ).abs() < 1e-3);
        assert!((cutoff_hz_from_normalized(1.0) - CUTOFF_CEILING_HZ).abs() < 0.1);
        // Knob 0.75 is 3.56 kHz, which is what every face has always read.
        assert!((cutoff_hz_from_normalized(0.75) - 3_556.6).abs() < 1.0);
        for step in 0..=100 {
            let knob = step as f32 / 100.0;
            let law = cutoff_hz_from_normalized(knob);
            assert!((normalized_from_cutoff_hz(law) - knob).abs() < 1e-4);
            let runs_at = |sample_rate: u32| law.min(sample_rate as f32 * 0.45);
            for sr in RATES {
                if law < 44_100.0 * 0.45 {
                    assert!(
                        cents(runs_at(sr), runs_at(48_000)).abs() < 1e-3,
                        "knob {knob}: {} Hz at {sr} vs {} Hz at 48 kHz",
                        runs_at(sr),
                        runs_at(48_000)
                    );
                }
            }
        }
    }

    #[test]
    fn a_nan_parameter_lands_on_the_bottom_of_its_range() {
        assert_eq!(clamp_param(f32::NAN, 20.0, 100.0), 20.0);
        assert_eq!(clamp_param(f32::INFINITY, 20.0, 100.0), 100.0);
        assert_eq!(clamp_param(f32::NEG_INFINITY, 20.0, 100.0), 20.0);
        assert_eq!(clamp_param(50.0, 20.0, 100.0), 50.0);
    }
}
