//! Normalized-knob-to-frequency mapping shared by every filter-cutoff-style
//! control: `20 * (max_hz / 20) ^ normalized` puts 0 at 20 Hz, 1 at
//! `max_hz`, and spaces perceptually even steps in between (each unit of
//! `normalized` covers the same number of octaves). Pull any new frequency
//! knob's mapping from here rather than re-deriving it.

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

    #[test]
    fn a_nan_parameter_lands_on_the_bottom_of_its_range() {
        assert_eq!(clamp_param(f32::NAN, 20.0, 100.0), 20.0);
        assert_eq!(clamp_param(f32::INFINITY, 20.0, 100.0), 100.0);
        assert_eq!(clamp_param(f32::NEG_INFINITY, 20.0, 100.0), 20.0);
        assert_eq!(clamp_param(50.0, 20.0, 100.0), 50.0);
    }
}
