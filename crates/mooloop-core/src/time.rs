//! Musical timekeeping.
//!
//! The internal clock is ticks in PPQ (pulses per quarter note), the same unit
//! MIDI Standard Files use, so note data maps cleanly to SMF export later.

/// Pulses per quarter note. 96 keeps things divisible by common rhythmic
/// denominators (4, 6, 8, 12, 16, 24, 32) without floating point.
pub const DEFAULT_PPQ: u32 = 96;

/// A PPQ setting. Stored as a value type to discourage ad-hoc mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ppq(pub u32);

impl Ppq {
    pub const DEFAULT: Self = Self(DEFAULT_PPQ);
    pub fn ticks_per_beat(self) -> u32 {
        self.0
    }
}

impl Default for Ppq {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Absolute position in ticks since the start of the song. Monotonic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ticks(pub u64);

impl Ticks {
    pub const ZERO: Self = Self(0);

    pub fn saturating_add(self, other: u64) -> Self {
        Self(self.0.saturating_add(other))
    }

    /// Tick offset within the containing beat.
    pub fn within_beat(self, ppq: Ppq) -> u32 {
        (self.0 % u64::from(ppq.ticks_per_beat())) as u32
    }

    /// Beat index since song start (0-based).
    pub fn beat(self, ppq: Ppq) -> u64 {
        self.0 / u64::from(ppq.ticks_per_beat())
    }

    /// Beat offset within the containing bar (assumes 4/4 for now).
    pub fn beat_in_bar(self, ppq: Ppq) -> u8 {
        (self.beat(ppq) % 4) as u8
    }
}

/// A duration or position measured in audio samples at a given sample rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Samples(pub u64);

impl Samples {
    pub const ZERO: Self = Self(0);
}

/// Convert a tempo + sample rate into ticks-per-sample. Used by the engine to
/// advance the transport clock inside the audio callback.
pub fn ticks_per_sample(bpm: f64, sample_rate: u32, ppq: Ppq) -> f64 {
    // samples per quarter note = sample_rate / (bpm / 60) = sample_rate * 60 / bpm
    // ticks per sample = ppq / samples_per_quarter
    f64::from(ppq.ticks_per_beat()) * bpm / (60.0 * f64::from(sample_rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_arithmetic() {
        let ppq = Ppq::DEFAULT;
        let t = Ticks(0).saturating_add(ppq.ticks_per_beat() as u64 * 5 + 10);
        assert_eq!(t.beat(ppq), 5);
        assert_eq!(t.within_beat(ppq), 10);
        assert_eq!(t.beat_in_bar(ppq), 1); // beat 5 -> beat 1 of bar 1
    }

    #[test]
    fn ticks_per_sample_round_trip() {
        let bpm = 120.0;
        let sr = 48_000;
        let ppq = Ppq::DEFAULT;
        let tps = ticks_per_sample(bpm, sr, ppq);
        // one beat in samples:
        let samples_per_beat = ppq.ticks_per_beat() as f64 / tps;
        // at 120bpm a beat is 0.5s -> 24000 samples @48k
        assert!((samples_per_beat - 24_000.0).abs() < 0.5);
    }
}
