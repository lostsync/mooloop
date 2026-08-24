//! Normalized-knob-to-frequency mapping shared by every filter-cutoff-style
//! control: `20 * (max_hz / 20) ^ normalized` puts 0 at 20 Hz, 1 at
//! `max_hz`, and spaces perceptually even steps in between (each unit of
//! `normalized` covers the same number of octaves). Used identically by
//! `MonoSynth`, `PolySynth`, `Sampler`, and `SpectrumAnalyzer`'s bin
//! spacing — pull any new frequency knob's mapping from here rather than
//! re-deriving it.

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
}
