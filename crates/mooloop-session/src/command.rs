//! The command layer's state.
//!
//! Clipboard data, undo history, and the gesture tokens that collapse a drag
//! into one undo step live here rather than in a particular widget, so menu,
//! keyboard, and context-menu surfaces all dispatch the same command.

use crate::channel::ChannelClipboard;
use crate::history::History;
use crate::project::ProjectSnapshot;
use mooloop_core::{EffectTarget, NoteEvent};
use std::time::{Duration, Instant};

/// The command layer's state. Clipboard data and history live here rather
/// than in a particular widget, so menu, keyboard, and context-menu surfaces
/// all dispatch the same command.
#[derive(Default)]
pub struct CommandState {
    pub channel_clipboard: Option<ChannelClipboard>,
    pub history: History<ProjectSnapshot>,
    pub project_edit_pending: bool,
    pub pane: Pane,
    /// A device cut or copied from a rack, with its whole run when it is a
    /// container, and with every identity stripped -- so a paste is a new
    /// device that sounds the same, not the same device twice.
    ///
    /// It does **not** carry the modulation routes or automation lanes that
    /// drove the original: a route's source is a module in the *channel's*
    /// rack, so it cannot follow a device onto another channel. That is the
    /// question `docs/plans/containers/` reserved rather than answered, and
    /// this inherits its answer instead of making a second one.
    pub device_clipboard: Option<mooloop_core::EffectRun>,
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
    /// What the open value gesture will be called in the history, set by the
    /// first edit inside it.
    ///
    /// Here rather than on `Session`, because it is what the entry is named
    /// and not part of the document; `Session` holds the gesture's `before`
    /// because `set_modulator_param` has to ask whether one is open. Cleared
    /// when a gesture opens rather than when it closes, so a gesture thrown
    /// away by an install cannot name the next one.
    pub gesture_label: Option<&'static str>,
    /// The last value gesture recorded, so the next one on the same control
    /// can join it. See [`CommandState::value_run_token`].
    pub value_run: Option<ValueRun>,
}

/// How close together two value gestures on one control have to be to undo
/// as one step.
///
/// Every wheel notch, arrow press and double-click reset on a shared control
/// is its own `Gesture.begin()`/`end()`, because only the widget knows where
/// one stops -- so one trackpad sweep of a cutoff was forty whole-project
/// entries, most of a heavy song's history (MOO-95). Half a second is long
/// enough that a sweep, or a held arrow key, is one run, and short enough
/// that coming back to the knob a moment later is a new step.
pub const VALUE_RUN_GAP: Duration = Duration::from_millis(500);

/// One run of value gestures on one control: the entry they are joining.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ValueRun {
    /// What the entry is called, which is the parameter's own name.
    pub label: &'static str,
    /// Where the control is: the selected channel and the rack on screen.
    /// Two "Mix" knobs on two channels are two runs.
    pub scope: (usize, EffectTarget),
    /// The history token the run's entries share, which is what makes
    /// `History::record` fold them into one.
    pub token: u64,
    /// When the last gesture of the run was recorded. The window slides, so
    /// a long sweep is one run for as long as it keeps moving.
    pub at: Instant,
}

impl CommandState {
    /// The history token a value gesture recorded `now` should carry.
    ///
    /// The run's own token when this gesture continues it -- the same label
    /// in the same place, within [`VALUE_RUN_GAP`] of the last one -- so
    /// `History::record` folds it into the entry the run already has, keeping
    /// the first `before` and this `after`. A fresh token otherwise, which
    /// starts a new entry. Tokens come from the same counter as a pointer
    /// drag's, so the two can never collide.
    ///
    /// Only for value gestures. A step toggle or a menu pick twice in half a
    /// second is two things the user did, and stays two steps.
    pub fn value_run_token(
        &mut self,
        label: &'static str,
        scope: (usize, EffectTarget),
        now: Instant,
    ) -> u64 {
        let token = match self.value_run {
            Some(run)
                if run.label == label
                    && run.scope == scope
                    && now.saturating_duration_since(run.at) <= VALUE_RUN_GAP =>
            {
                run.token
            }
            _ => {
                self.next_gesture = self.next_gesture.wrapping_add(1);
                self.next_gesture
            }
        };
        self.value_run = Some(ValueRun {
            label,
            scope,
            token,
            at: now,
        });
        token
    }
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

#[cfg(test)]
mod tests {
    use super::{CommandState, VALUE_RUN_GAP};
    use mooloop_core::EffectTarget;
    use std::time::{Duration, Instant};

    const HERE: (usize, EffectTarget) = (0, EffectTarget::Channel(0));

    /// Wheel notches on one knob, each inside the gap of the last, are one
    /// run however long the sweep goes on -- the window slides.
    #[test]
    fn notches_on_one_control_inside_the_gap_share_a_token() {
        let mut commands = CommandState::default();
        let start = Instant::now();
        let step = VALUE_RUN_GAP / 2;
        let first = commands.value_run_token("Cutoff", HERE, start);
        for notch in 1..10 {
            assert_eq!(
                commands.value_run_token("Cutoff", HERE, start + step * notch),
                first,
                "notch {notch} is still the same sweep"
            );
        }
    }

    /// A pause longer than the gap, another parameter, or the same name in
    /// another place each start a run of their own.
    #[test]
    fn a_pause_another_parameter_or_another_place_starts_a_new_run() {
        let mut commands = CommandState::default();
        let start = Instant::now();
        let first = commands.value_run_token("Cutoff", HERE, start);
        let later = start + VALUE_RUN_GAP + Duration::from_millis(1);
        let paused = commands.value_run_token("Cutoff", HERE, later);
        assert_ne!(paused, first);
        let other = commands.value_run_token("Resonance", HERE, later);
        assert_ne!(other, paused);
        let elsewhere = commands.value_run_token("Resonance", (1, EffectTarget::Channel(1)), later);
        assert_ne!(elsewhere, other);
        // And a pointer drag's token can never be one of these: both come
        // from the same counter.
        assert_eq!(commands.next_gesture, elsewhere);
    }
}
