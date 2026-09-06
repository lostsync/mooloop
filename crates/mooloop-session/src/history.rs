//! Small, UI-thread-only undo history.
//!
//! The caller owns applying snapshots.  That matters here because a project
//! edit is only committed once the audio engine has accepted its replacement
//! render state; moving the history cursor before then could make undo lie
//! about what is actually audible.

#[derive(Clone)]
pub struct Entry<T> {
    pub before: T,
    pub after: T,
    pub label: &'static str,
    /// Which continuous gesture produced this edit, if any.
    ///
    /// A pointer drag reports an edit on every move frame, so recording each
    /// one as its own entry made a single note drag cost twenty undos. Frames
    /// carrying the same token collapse into one entry that keeps the first
    /// frame's `before` and the last frame's `after`, which is what the user
    /// means by "undo that drag". `None` never coalesces.
    pub gesture: Option<u64>,
}

/// How many edits the history keeps before the oldest starts falling off.
///
/// A depth rather than a product cap: an entry is two whole project
/// snapshots, and a project's cost is mostly its channels' device parameters
/// rather than its notes — a sixteen-channel song measures about 200 KB an
/// entry with only sixteen notes to a pattern, and a thirty-two channel one
/// with a busy pattern measures 1.1 MB. Unbounded, an afternoon's five
/// hundred edits is 96 MB of a realistic song and 566 MB of a large one, and
/// it keeps going; every entry also holds an `Arc` to each sample loaded at
/// the time, so a replaced sample stayed resident for the rest of the
/// session.
///
/// Two hundred and fifty-six bounds that at roughly 50 MB for the realistic
/// case while being far past what anyone reaches for: it is a number to raise
/// in one edit if it ever proves short, not a limit the interface should ever
/// have to explain. `edit_cost::undo_entry_memory` is where the figures come
/// from.
pub const MAX_ENTRIES: usize = 256;

pub struct History<T> {
    entries: Vec<Entry<T>>,
    /// Number of entries currently applied.  Entries after this cursor are
    /// the redo branch.
    cursor: usize,
}

impl<T> Default for History<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            cursor: 0,
        }
    }
}

impl<T> History<T> {
    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.cursor < self.entries.len()
    }

    pub fn undo_target(&self) -> Option<&Entry<T>> {
        self.cursor
            .checked_sub(1)
            .and_then(|index| self.entries.get(index))
    }

    pub fn redo_target(&self) -> Option<&Entry<T>> {
        self.entries.get(self.cursor)
    }

    /// Call only after `undo_target` was successfully installed.
    pub fn commit_undo(&mut self) {
        debug_assert!(self.can_undo());
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Call only after `redo_target` was successfully installed.
    pub fn commit_redo(&mut self) {
        debug_assert!(self.can_redo());
        self.cursor = (self.cursor + 1).min(self.entries.len());
    }

    /// Record an edit that has already been successfully installed.
    ///
    /// Extends the top entry instead of pushing when both carry the same
    /// gesture token. Tokens are compared rather than labels so two separate
    /// drags of the same kind stay two undo steps.
    pub fn record(&mut self, entry: Entry<T>) {
        self.entries.truncate(self.cursor);
        if let Some(gesture) = entry.gesture {
            if let Some(open) = self.entries.last_mut() {
                if open.gesture == Some(gesture) {
                    open.after = entry.after;
                    return;
                }
            }
        }
        self.entries.push(entry);
        // The oldest go, not the newest: undo reaches backwards, so the edits
        // furthest from the cursor are the ones nobody is going to ask for.
        // Dropping from the front leaves the cursor at the end either way.
        if self.entries.len() > MAX_ENTRIES {
            let excess = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..excess);
        }
        self.cursor = self.entries.len();
    }

    /// How many edits are currently retained. For the test that pins the
    /// bound; nothing in the interface asks.
    ///
    /// Not `len`: a history with nothing in it is an ordinary state and the
    /// question callers ask about it is `can_undo`, so an `is_empty` beside
    /// this would be a second way to ask something already answered.
    #[cfg(test)]
    pub fn retained(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{Entry, History};

    #[test]
    fn undo_redo_keeps_the_branch_until_a_new_edit_commits() {
        let mut history = History::default();
        history.record(Entry {
            before: 0,
            after: 1,
            label: "one",
            gesture: None,
        });
        history.record(Entry {
            before: 1,
            after: 2,
            label: "two",
            gesture: None,
        });

        assert_eq!(history.undo_target().map(|entry| entry.before), Some(1));
        history.commit_undo();
        assert_eq!(history.redo_target().map(|entry| entry.after), Some(2));
        history.commit_redo();
        assert!(!history.can_redo());

        history.commit_undo();
        history.record(Entry {
            before: 1,
            after: 3,
            label: "three",
            gesture: None,
        });
        assert_eq!(history.undo_target().map(|entry| entry.before), Some(1));
        assert!(!history.can_redo());
    }

    #[test]
    fn one_gesture_collapses_to_one_entry_that_spans_it() {
        let mut history = History::default();
        // What a pointer drag looks like: a frame per move, each reporting
        // the state it started from and the state it produced.
        for step in 0..5 {
            history.record(Entry {
                before: step,
                after: step + 1,
                label: "Note moved",
                gesture: Some(7),
            });
        }

        assert_eq!(history.undo_target().map(|entry| entry.before), Some(0));
        assert_eq!(history.redo_target().map(|entry| entry.after), None);
        history.commit_undo();
        // One undo, and it lands before the whole drag rather than one
        // frame back into the middle of it.
        assert!(!history.can_undo());
        assert_eq!(history.redo_target().map(|entry| entry.after), Some(5));
    }

    #[test]
    fn separate_gestures_stay_separate_undo_steps() {
        let mut history = History::default();
        history.record(Entry {
            before: 0,
            after: 1,
            label: "Note moved",
            gesture: Some(1),
        });
        // Same label, different gesture: releasing and dragging again is two
        // things the user did, so it has to be two things they can undo.
        history.record(Entry {
            before: 1,
            after: 2,
            label: "Note moved",
            gesture: Some(2),
        });

        history.commit_undo();
        assert_eq!(history.undo_target().map(|entry| entry.before), Some(0));
        assert_eq!(history.redo_target().map(|entry| entry.after), Some(2));
    }
}

#[cfg(test)]
mod bound_tests {
    use super::{Entry, History, MAX_ENTRIES};

    fn edit(value: usize) -> Entry<usize> {
        Entry {
            before: value,
            after: value + 1,
            label: "edit",
            gesture: None,
        }
    }

    /// An undo entry is two whole project snapshots, so a history that grows
    /// without limit is a session that grows without limit. It stops at the
    /// depth, and it stops by dropping the oldest.
    #[test]
    fn the_history_stops_at_its_depth_and_drops_the_oldest_first() {
        let mut history = History::default();
        for value in 0..MAX_ENTRIES + 50 {
            history.record(edit(value));
        }
        assert_eq!(history.retained(), MAX_ENTRIES);
        assert!(history.can_undo());
        assert!(!history.can_redo());

        // The newest edit is still the one undo reaches first, and the oldest
        // retained is the fiftieth -- the first fifty fell off the front.
        assert_eq!(
            history.undo_target().map(|entry| entry.after),
            Some(MAX_ENTRIES + 50)
        );
        for _ in 0..MAX_ENTRIES {
            history.commit_undo();
        }
        assert!(!history.can_undo());
        assert_eq!(history.redo_target().map(|entry| entry.before), Some(50));
    }

    /// Coalescing a drag must not count against the depth: a gesture that
    /// reports two hundred move frames is one edit, and it was already being
    /// folded into one entry.
    #[test]
    fn a_coalesced_drag_is_one_entry_however_many_frames_it_reports() {
        let mut history = History::default();
        for value in 0..500 {
            history.record(Entry {
                gesture: Some(7),
                ..edit(value)
            });
        }
        assert_eq!(history.retained(), 1);
    }
}
