//! Small voice conventions genuinely shared by the three oscillator synths.
//!
//! These are mechanics, not an invitation to share the instruments' voice
//! implementations. The ML-1, v1 mono synth, and poly synth deliberately own
//! their envelopes, filters, note handling, and output calibration.

/// Minimum glide time; at or below this, pitch changes are instant.
pub(crate) const MIN_GLIDE_S: f32 = 1.0e-3;

/// Fast release used when the transport stops (seconds).
pub(crate) const STOP_RELEASE_S: f32 = 0.005;

/// Lag applied to parameters that scale the signal directly.
pub(crate) const PARAM_SMOOTH_S: f32 = 0.005;

/// MIDI note number to frequency in Hz (A4 = 69 = 440 Hz).
pub(crate) fn note_to_freq(note: u8) -> f32 {
    440.0 * 2.0_f32.powf((f32::from(note.min(127)) - 69.0) / 12.0)
}

#[cfg(test)]
mod tests {
    use super::note_to_freq;

    #[test]
    fn midi_note_frequency_is_anchored_at_a4_and_clamped() {
        assert_eq!(note_to_freq(69), 440.0);
        assert!((note_to_freq(60) - 261.625_58).abs() < 1.0e-4);
        assert_eq!(note_to_freq(u8::MAX), note_to_freq(127));
    }
}
