//! Musical timekeeping.
//!
//! The internal clock is ticks in PPQ (pulses per quarter note), the same unit
//! MIDI Standard Files use, so note data maps cleanly to SMF export later.

use std::fmt;

/// Pulses per quarter note. 96 keeps things divisible by common rhythmic
/// denominators (4, 6, 8, 12, 16, 24, 32) without floating point.
pub const DEFAULT_PPQ: u32 = 96;

/// Beats to the bar. Four, everywhere, and this is the only place that says
/// so.
///
/// [`Project::beats_per_bar`] is persisted metadata and is **not** this
/// value: the audio thread is never given the project, so a signature it
/// could not read would be a signature it could not honour. Until that
/// changes, the format field is documentation and this constant is the
/// behaviour, and `integrity.rs` holds the two to the same number.
///
/// It was nine anonymous fours before `docs/plans/musical-time/`, in six
/// crates, three of which carried the comment "assumes 4/4 for now"
/// independently of the other two.
///
/// [`Project::beats_per_bar`]: crate::project::Project::beats_per_bar
pub const BEATS_PER_BAR: u32 = 4;

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

    /// Beat offset within the containing bar (0-based).
    pub fn beat_in_bar(self, ppq: Ppq) -> u8 {
        (self.beat(ppq) % u64::from(BEATS_PER_BAR)) as u8
    }
}

/// A transport position in bars, beats and ticks.
///
/// Bars and beats count from one, ticks from zero, because that is what a
/// musician reads off a transport: tick zero is `1:1:0`.
///
/// It is a separate type from [`BbtDuration`] on purpose. One type with an
/// `is_duration` flag would be one thing standing for two semantics, with
/// nothing able to report a caller that picked wrong -- and the two differ by
/// exactly the off-by-one that makes a one-bar length print as `2:1:0`. Two
/// types make choosing a deliberate act rather than a correct value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BbtPosition {
    pub bar: u32,
    pub beat: u32,
    pub tick: u32,
}

impl BbtPosition {
    /// Where `ticks` lands. Integer division and remainder only -- no
    /// allocation and no float, because `transport.rs` calls this on the
    /// audio thread.
    pub fn from_ticks(ticks: Ticks, ppq: Ppq) -> Self {
        Self {
            bar: saturate(ticks.beat(ppq) / u64::from(BEATS_PER_BAR)) + 1,
            beat: u32::from(ticks.beat_in_bar(ppq)) + 1,
            tick: ticks.within_beat(ppq),
        }
    }
}

impl fmt::Display for BbtPosition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.bar, self.beat, self.tick)
    }
}

/// A length in bars, beats and ticks.
///
/// Everything counts from zero: one bar is `1:0:0`. See [`BbtPosition`] for
/// why this is a second type rather than a flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct BbtDuration {
    pub bars: u32,
    pub beats: u32,
    pub ticks: u32,
}

impl BbtDuration {
    /// How long `ticks` is. Same arithmetic as [`BbtPosition::from_ticks`]
    /// without the two `+ 1`s, which is the whole difference between the
    /// types.
    pub fn from_ticks(ticks: Ticks, ppq: Ppq) -> Self {
        Self {
            bars: saturate(ticks.beat(ppq) / u64::from(BEATS_PER_BAR)),
            beats: u32::from(ticks.beat_in_bar(ppq)),
            ticks: ticks.within_beat(ppq),
        }
    }
}

impl fmt::Display for BbtDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.bars, self.beats, self.ticks)
    }
}

/// Bars past four billion are a broken clock, not a song. Saturating rather
/// than truncating so a wrong number stays obviously wrong.
fn saturate(bars: u64) -> u32 {
    u32::try_from(bars).unwrap_or(u32::MAX)
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

    /// The derivation, not the pair of numbers. `STEPS_PER_BAR` was a bare
    /// `16` one line above a `TICKS_PER_BAR` that was already derived, so
    /// moving `STEPS_PER_BEAT` would have parted them in silence.
    #[test]
    fn steps_per_bar_is_derived_from_the_one_constant() {
        assert_eq!(
            crate::STEPS_PER_BAR,
            u32::from(crate::STEPS_PER_BEAT) * BEATS_PER_BAR,
            "STEPS_PER_BAR stopped being STEPS_PER_BEAT * BEATS_PER_BAR"
        );
    }

    /// The relationship, not the remembered answer. `sampler.rs` asserts that
    /// a bar at 48 kHz and 120 bpm is 96_000 frames; that stays green if
    /// `BEATS_PER_BAR` moves and `frames_per_bar` does not follow it.
    #[test]
    fn a_bar_is_beats_per_bar_beats_long() {
        let seconds_per_beat = 60.0 / 120.0;
        let frames_per_beat = 48_000.0 * seconds_per_beat;
        assert_eq!(
            crate::frames_per_bar(48_000, 120.0),
            frames_per_beat * f64::from(BEATS_PER_BAR)
        );
    }

    /// Why there are two types. A position counts from one, so tick zero is
    /// `1:1:0`; a duration counts from zero, so one bar is `1:0:0`. Print a
    /// one-bar length through the position type and it reads `2:1:0`, which
    /// is the mistake a single type with a flag would have let through.
    #[test]
    fn a_position_counts_from_one_and_a_duration_counts_from_zero() {
        let ppq = Ppq::DEFAULT;
        let one_bar = Ticks(u64::from(ppq.ticks_per_beat()) * u64::from(BEATS_PER_BAR));

        let start = BbtPosition::from_ticks(Ticks::ZERO, ppq);
        assert_eq!(
            (start.bar, start.beat, start.tick),
            (1, 1, 0),
            "the transport opens at bar one, beat one"
        );
        assert_eq!(start.to_string(), "1:1:0");

        let length = BbtDuration::from_ticks(one_bar, ppq);
        assert_eq!(
            (length.bars, length.beats, length.ticks),
            (1, 0, 0),
            "one bar is one bar and no beats, not two bars and one beat"
        );
        assert_eq!(length.to_string(), "1:0:0");

        // The same tick through both types, side by side, because that is the
        // difference the two exist to keep apart.
        assert_eq!(BbtPosition::from_ticks(one_bar, ppq).to_string(), "2:1:0");
    }

    /// The invariant `engine::transport::beat_in_bar` relies on when it
    /// subtracts one from `BbtPosition::beat`.
    #[test]
    fn a_positions_beat_is_one_past_the_zero_based_beat_in_bar() {
        let ppq = Ppq::DEFAULT;
        let mut checked = 0;
        let mut tick = 0;
        while tick < u64::from(ppq.ticks_per_beat()) * u64::from(BEATS_PER_BAR) * 5 {
            let ticks = Ticks(tick);
            assert_eq!(
                BbtPosition::from_ticks(ticks, ppq).beat - 1,
                u32::from(ticks.beat_in_bar(ppq)),
                "beat {} of BbtPosition is not one past beat_in_bar at tick {tick}",
                BbtPosition::from_ticks(ticks, ppq).beat
            );
            checked += 1;
            tick += 7;
        }
        assert_eq!(checked, 275, "the sweep stopped covering five bars");
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
