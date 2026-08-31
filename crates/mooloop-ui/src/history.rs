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
        self.cursor = self.entries.len();
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
