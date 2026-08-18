//! Bounded song arrangement data shared by the UI and realtime engine.

use crate::TICKS_PER_STEP;

/// The first playlist UI shows 64 bars and snaps placement starts to half bars.
pub const MAX_PLAYLIST_BARS: u32 = 64;
pub const STEPS_PER_BAR: u32 = 16;
pub const TICKS_PER_BAR: u32 = STEPS_PER_BAR * TICKS_PER_STEP;
pub const PLAYLIST_SNAP_TICKS: u32 = TICKS_PER_BAR / 2;
/// Exclusive end of the editable placement-start grid. Long clips may extend
/// past this point and still contribute to the derived song length.
pub const MAX_PLAYLIST_TICKS: u32 = MAX_PLAYLIST_BARS * TICKS_PER_BAR;

/// Fixed upper bound so the realtime sequencer never grows its placement store.
pub const MAX_PLAYLIST_PLACEMENTS: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackMode {
    #[default]
    Pattern,
    Song,
}

/// One pattern instance on the absolute song timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
