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
    pub fn record(&mut self, entry: Entry<T>) {
        self.entries.truncate(self.cursor);
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
        });
        history.record(Entry {
            before: 1,
            after: 2,
            label: "two",
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
        });
        assert_eq!(history.undo_target().map(|entry| entry.before), Some(1));
        assert!(!history.can_redo());
    }
}
