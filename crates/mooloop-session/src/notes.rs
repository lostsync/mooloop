//! Note-editing gesture state.

use mooloop_core::NoteId;

/// The selected notes a group gesture should act on, always including the
/// note that was actually grabbed.
///
/// Grabbing an unselected note has to act on that note alone, not on some
/// selection elsewhere in the pattern; grabbing a selected one acts on the
/// whole selection. Both fall out of "the selection, plus the anchor".
/// A scale drag's starting state.
pub struct ScaleBase {
    /// The tick the selection scales about: the edge the drag is not moving.
    pub anchor: u32,
    /// Each selected note's id with its pre-drag start and duration.
    pub notes: Vec<(NoteId, u32, u32)>,
}
