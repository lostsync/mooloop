//! Bounded song arrangement data shared by the UI and realtime engine.

use crate::time::BEATS_PER_BAR;
use crate::{STEPS_PER_BEAT, TICKS_PER_STEP};

/// The playlist opens on a 64-bar canvas. Placement starts retain absolute PPQ
/// ticks; the active musical snap belongs to the editor, not stored data.
pub const MAX_PLAYLIST_BARS: u32 = 64;
pub const STEPS_PER_BAR: u32 = STEPS_PER_BEAT as u32 * BEATS_PER_BAR;
pub const TICKS_PER_BAR: u32 = STEPS_PER_BAR * TICKS_PER_STEP;
/// Exclusive end of the editable placement-start grid. Long clips may extend
/// past this point and still contribute to the derived song length.
pub const MAX_PLAYLIST_TICKS: u32 = MAX_PLAYLIST_BARS * TICKS_PER_BAR;

/// Fixed upper bound so the realtime sequencer never grows its placement store.
pub const MAX_PLAYLIST_PLACEMENTS: usize = 512;

/// How much of a new song is marked as its loop, in bars.
///
/// Marked, and *not* running -- see [`LoopRange::marked`] and
/// `Project::starter_kit`. A song used to open with no loop at all, which put
/// a disabled toggle and an empty strip in front of anyone who had not yet
/// found out that the strip above the bar numbers is draggable. Two bars is
/// the smallest section that reads as a repeat rather than as a stutter, and
/// it is where a drag would most likely have been aimed anyway.
pub const STARTER_LOOP_BARS: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackMode {
    #[default]
    Pattern,
    Song,
}

/// One pattern instance on the absolute song timeline.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct PatternPlacement {
    pub pattern: u8,
    pub start_tick: u32,
}

impl PatternPlacement {
    pub fn new(pattern: u8, start_tick: u32) -> Self {
        Self {
            pattern,
            start_tick,
        }
    }
}

/// The section of the arrangement the transport repeats.
///
/// Held as absolute PPQ ticks on the same grid as [`PatternPlacement`], so a
/// loop point means the same thing as a clip start and survives a tempo
/// change. The range is half-open: `end_tick` is the tick the transport jumps
/// away from, never one it plays.
///
/// `enabled` is separate from the points so switching looping off and on
/// again returns to the same section rather than to nothing, which is the
/// whole reason a loop toggle is worth having next to the points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct LoopRange {
    #[serde(default)]
    pub start_tick: u32,
    #[serde(default)]
    pub end_tick: u32,
    #[serde(default)]
    pub enabled: bool,
}

impl LoopRange {
    /// The range as the transport should use it, or `None` when nothing
    /// should repeat.
    ///
    /// `song_ticks` bounds the end because the arrangement past its own
    /// content is not a place the transport can be: the sequencer folds every
    /// position into the song's own period, so a loop reaching beyond it
    /// would ask for a position that does not exist. A range clamped down to
    /// nothing is inert rather than an error -- shortening a song under a
    /// loop should stop the loop, not refuse the edit.
    pub fn active(self, song_ticks: u32) -> Option<(u32, u32)> {
        if !self.enabled {
            return None;
        }
        let start = self.start_tick.min(song_ticks);
        let end = self.end_tick.min(song_ticks);
        (end > start).then_some((start, end))
    }

    /// A range from two ticks in either order, snapped by the caller and
    /// bounded by the editable canvas.
    ///
    /// A drag that ends where it started is a click, and a click has no
    /// section in it, so this returns `None` rather than a zero-width loop
    /// the transport would have to special-case.
    pub fn from_drag(a: u32, b: u32) -> Option<Self> {
        let start = a.min(b).min(MAX_PLAYLIST_TICKS);
        let end = a.max(b).min(MAX_PLAYLIST_TICKS);
        (end > start).then_some(Self {
            start_tick: start,
            end_tick: end,
            enabled: true,
        })
    }

    /// A section marked out but not running.
    ///
    /// [`from_drag`](Self::from_drag) cannot express this and should not: it
    /// is the gesture that *creates* a loop, and someone who drags one out is
    /// asking for it. A song that opens with its first two bars marked is
    /// making a suggestion instead, and a suggestion that started the
    /// transport repeating would not be one.
    pub fn marked(a: u32, b: u32) -> Option<Self> {
        Self::from_drag(a, b).map(|range| Self {
            enabled: false,
            ..range
        })
    }
}

/// Fold a transport position into `[0, period)`. The transport is monotonic
/// across loops, so every pattern-local read needs this. A zero period folds
/// everything to 0.
pub fn wrap_tick(tick: f64, period_ticks: u32) -> f64 {
    if period_ticks == 0 {
        return 0.0;
    }
    let period = period_ticks as f64;
    let wrapped = tick % period;
    if wrapped < 0.0 {
        wrapped + period
    } else {
        wrapped
    }
}

/// Where a note played at `song_tick` lands in the selected pattern, or
/// `None` when the selected pattern is not what is playing there.
///
/// The one copy of the rule. The engine asks it for a recorded note's start
/// (`Sequencer::recording_tick`) and the session asks it for the span a
/// Replace take has crossed (MOO-234), so the two cannot come to disagree
/// about which pattern tick the playhead is over.
///
/// The transport never folds in pattern mode -- scheduling wraps its own
/// copy of the position -- so a recorder that reported the playhead as it
/// stands would report tick 400 of a 384-tick pattern on the second pass.
/// Pattern mode folds with [`wrap_tick`]. Song mode answers the offset into
/// the placement of the selected pattern that covers the playhead, taking
/// the latest-starting one where two overlap. Where no placement of it
/// covers the playhead there is nowhere to record: the note would otherwise
/// land in a pattern at a position that was never heard against it.
///
/// `placements` must be in playlist order; `pattern_ticks` is the selected
/// pattern's length, and a zero length records nothing.
pub fn recording_offset(
    mode: PlaybackMode,
    song_tick: f64,
    selected: usize,
    pattern_ticks: u32,
    song_ticks: u32,
    placements: &[PatternPlacement],
) -> Option<u32> {
    if pattern_ticks == 0 {
        return None;
    }
    match mode {
        PlaybackMode::Pattern => Some(wrap_tick(song_tick, pattern_ticks) as u32),
        PlaybackMode::Song => {
            let position = wrap_tick(song_tick, song_ticks);
            placements
                .iter()
                .rev()
                .filter(|placement| placement.pattern as usize == selected)
                .map(|placement| position - f64::from(placement.start_tick))
                .find(|offset| (0.0..f64::from(pattern_ticks)).contains(offset))
                .map(|offset| offset as u32)
        }
    }
}

#[cfg(test)]
mod recording_offset_tests {
    use super::*;

    #[test]
    fn pattern_mode_folds_every_pass_into_the_pattern() {
        for pass in 0..3u32 {
            let tick = f64::from(pass * 96 + 48);
            assert_eq!(
                recording_offset(PlaybackMode::Pattern, tick, 0, 96, TICKS_PER_BAR, &[]),
                Some(48)
            );
        }
    }

    #[test]
    fn song_mode_takes_the_latest_placement_of_the_selected_pattern() {
        let placements = [
            PatternPlacement::new(0, 0),
            PatternPlacement::new(1, 0),
            PatternPlacement::new(0, 48),
        ];
        let at = |tick: u32, selected| {
            recording_offset(
                PlaybackMode::Song,
                f64::from(tick),
                selected,
                96,
                TICKS_PER_BAR,
                &placements,
            )
        };
        assert_eq!(at(24, 0), Some(24));
        assert_eq!(at(60, 0), Some(12), "the later placement wins the overlap");
        assert_eq!(at(200, 0), None, "no placement covers it");
        assert_eq!(at(TICKS_PER_BAR + 24, 0), Some(24), "the song wraps");
        assert_eq!(at(24, 2), None, "a pattern with no placement");
    }
}
