//! The command layer's state.
//!
//! Clipboard data, undo history, and the gesture tokens that collapse a drag
//! into one undo step live here rather than in a particular widget, so menu,
//! keyboard, and context-menu surfaces all dispatch the same command.

use crate::channel::ChannelClipboard;
use crate::history::History;
use crate::project::ProjectSnapshot;
use mooloop_core::NoteEvent;

/// The command layer's state. Clipboard data and history live here rather
/// than in a particular widget, so menu, keyboard, and context-menu surfaces
/// all dispatch the same command.
#[derive(Default)]
pub struct CommandState {
    pub channel_clipboard: Option<ChannelClipboard>,
    pub history: History<ProjectSnapshot>,
    pub project_edit_pending: bool,
    pub pane: Pane,
    /// Notes cut or copied from the roll, kept relative to the earliest one
    /// so a paste lands as a phrase rather than at absolute ticks.
    pub note_clipboard: Vec<NoteEvent>,
    /// Token identifying the pointer gesture currently in flight, if one is.
    /// A drag reports an edit on every move frame; stamping them all with the
    /// same token is what collapses them into one undo step.
    pub gesture: Option<u64>,
    /// Source of the next token. Monotonic rather than a bool so that two
    /// drags separated by a release never look like one continuous gesture.
    pub next_gesture: u64,
}

/// The work-surface/lower-dock combination a `view.pane-*` shortcut
/// targets. `mixer-visible` and `editor-page` are independent Slint
/// properties (the step grid or mixer sits above an always-visible
/// Source/Notes/Playlist dock), so there is no single UI property that
/// says "which pane is current" -- this is tracked here instead of derived,
/// so Next/Prev cycles predictably even though Steps and the dock tabs are
/// simultaneously visible.
#[derive(Clone, Copy, Default, PartialEq)]
pub enum Pane {
    Steps,
    Mixer,
    #[default]
    Source,
    Notes,
    Playlist,
}

const PANE_CYCLE: [Pane; 5] = [
    Pane::Steps,
    Pane::Mixer,
    Pane::Source,
    Pane::Notes,
    Pane::Playlist,
];

pub fn cycle_pane(current: Pane, forward: bool) -> Pane {
    let position = PANE_CYCLE
        .iter()
        .position(|pane| *pane == current)
        .unwrap_or(0);
    let len = PANE_CYCLE.len();
    let next = if forward {
        (position + 1) % len
    } else {
        (position + len - 1) % len
    };
    PANE_CYCLE[next]
}
