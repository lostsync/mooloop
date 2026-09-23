//! Where a synth voice's filter sits, sample by sample: the Cutoff knob, swept
//! by the filter envelope in octaves, tracking the keyboard, clamped to what
//! the filter can run at.
//!
//! ```text
//! VoiceCutoff
//! in:  Cutoff knob (0..1), envelope level and amount, keytrack amount and
//!      the voice's pitch, any other modulation in octaves
//! out: cutoff (Hz)
//! ```
//!
//! Until 2026-09-22 this was written out five times -- in the v1 mono and
//! poly synths, the sampler, the ML-M1 and the ML-P8 -- with the envelope's
//! six octaves a bare literal in four of them and `KEYTRACK_REFERENCE_HZ`
//! defined twice (`reports/teams-2026-09-22.md`, F7; MOO-144). Five copies of
//! one law is five places for it to drift, and the knob-to-Hz ceiling had
//! already drifted into all five as `0.45 * sample_rate` (MOO-119).
//!
//! What is shared is the cutoff, not the filter. The five run four different
//! filters -- a plain [`crate::filter::Svf`], a stereo pair of them sharing one
//! coefficient set, the ML-M1's three models and the ML-P8's multimode -- and
//! each keeps its own. They all ask this where to put it.

use crate::scale::{cutoff_hz_from_normalized, CUTOFF_FLOOR_HZ};

/// How far a full envelope at full amount moves the cutoff, in octaves.
pub const FILTER_ENV_OCTAVES: f32 = 6.0;

/// Middle C (MIDI 60). Keytracking is referenced here, so a patch voiced
/// around the middle of the keyboard keeps its cutoff where it was set.
pub const KEYTRACK_REFERENCE_HZ: f32 = 261.625_58;

/// The per-block half of a voice's cutoff: the knob's frequency and the
/// ceiling the filter can run at. Build it once per block (or whenever the
/// knob moves) and ask it for [`Self::hz`] every sample.
#[derive(Clone, Copy, Debug)]
pub struct VoiceCutoff {
    base_hz: f32,
    max_hz: f32,
}

impl VoiceCutoff {
    /// The cutoff for a knob position, by the one knob-to-Hz law
    /// ([`cutoff_hz_from_normalized`]): 20 Hz to 20 kHz at every sample rate.
    pub fn from_knob(knob: f32, sample_rate: u32) -> Self {
        Self::from_hz(cutoff_hz_from_normalized(knob.clamp(0.0, 1.0)), sample_rate)
    }

    /// The same from a frequency already resolved -- for a caller that caches
    /// the knob's `powf`, or scales it (ML-P8's drift) before modulation.
    pub fn from_hz(base_hz: f32, sample_rate: u32) -> Self {
        Self {
            base_hz,
            max_hz: max_cutoff_hz(sample_rate),
        }
    }

    /// The knob's frequency, before anything moves it.
    pub fn base_hz(&self) -> f32 {
        self.base_hz
    }

    /// The cutoff with `octaves` of modulation applied, clamped to the range
    /// the filters are correct over. `octaves` is the sum of everything that
    /// moves it this sample: [`env_octaves`], [`keytrack_octaves`], an LFO.
    pub fn hz(&self, octaves: f32) -> f32 {
        (self.base_hz * octaves.exp2()).clamp(CUTOFF_FLOOR_HZ, self.max_hz)
    }
}

/// The highest cutoff a voice filter is asked for: `0.45 * sample_rate`, the
/// same clamp the filter primitives apply. Below 44.4 kHz this sits under
/// the knob's 20 kHz top, so the last sliver of the knob is flat there.
pub fn max_cutoff_hz(sample_rate: u32) -> f32 {
    sample_rate as f32 * 0.45
}

/// The envelope's contribution, in octaves: its level times the amount knob
/// (`-1..1`) times [`FILTER_ENV_OCTAVES`].
pub fn env_octaves(env_level: f32, amount: f32) -> f32 {
    env_level * amount * FILTER_ENV_OCTAVES
}

/// Keytracking's contribution, in octaves: `amount` (0..1) of the voice's
/// distance from [`KEYTRACK_REFERENCE_HZ`]. Read the *gliding* pitch, so a
/// slide sweeps the filter with it.
pub fn keytrack_octaves(note_hz: f32, amount: f32) -> f32 {
    if amount <= 0.0 {
        return 0.0;
    }
    amount * (note_hz.max(1.0) / KEYTRACK_REFERENCE_HZ).log2()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{cents, RATES};

    /// The same knob and the same modulation put the cutoff at the same
    /// frequency at every rate, until the filter's own ceiling.
    #[test]
    fn a_voice_cutoff_is_one_frequency_at_every_rate() {
        for knob in [0.0_f32, 0.25, 0.5, 0.75, 0.9] {
            for octaves in [-2.0_f32, 0.0, 1.5] {
                let at_48 = VoiceCutoff::from_knob(knob, 48_000).hz(octaves);
                for sr in RATES {
                    let here = VoiceCutoff::from_knob(knob, sr).hz(octaves);
                    if at_48 < max_cutoff_hz(44_100) {
                        assert!(
                            cents(here, at_48).abs() < 0.01,
                            "knob {knob}, {octaves} oct: {here} Hz at {sr} vs {at_48} Hz at 48k"
                        );
                    }
                    assert!(here <= max_cutoff_hz(sr));
                }
            }
        }
    }

    #[test]
    fn the_envelope_and_keytracking_are_measured_in_octaves() {
        assert_eq!(env_octaves(1.0, 1.0), FILTER_ENV_OCTAVES);
        assert_eq!(env_octaves(0.5, -1.0), -3.0);
        assert_eq!(keytrack_octaves(KEYTRACK_REFERENCE_HZ * 4.0, 1.0), 2.0);
        assert_eq!(keytrack_octaves(KEYTRACK_REFERENCE_HZ * 4.0, 0.5), 1.0);
        assert_eq!(keytrack_octaves(1_000.0, 0.0), 0.0);
        let cutoff = VoiceCutoff::from_hz(1_000.0, 48_000);
        assert!((cutoff.hz(1.0) - 2_000.0).abs() < 1.0e-3);
        assert_eq!(cutoff.hz(-20.0), CUTOFF_FLOOR_HZ);
        assert_eq!(cutoff.hz(20.0), max_cutoff_hz(48_000));
    }
}
