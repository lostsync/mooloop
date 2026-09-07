//! Bounded song arrangement data shared by the UI and realtime engine.

use crate::TICKS_PER_STEP;

/// The playlist opens on a 64-bar canvas. Placement starts retain absolute PPQ
/// ticks; the active musical snap belongs to the editor, not stored data.
pub const MAX_PLAYLIST_BARS: u32 = 64;
pub const STEPS_PER_BAR: u32 = 16;
pub const TICKS_PER_BAR: u32 = STEPS_PER_BAR * TICKS_PER_STEP;
/// Exclusive end of the editable placement-start grid. Long clips may extend
/// past this point and still contribute to the derived song length.
pub const MAX_PLAYLIST_TICKS: u32 = MAX_PLAYLIST_BARS * TICKS_PER_BAR;

/// Fixed upper bound so the realtime sequencer never grows its placement store.
pub const MAX_PLAYLIST_PLACEMENTS: usize = 512;

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
}
