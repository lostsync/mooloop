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

/// What retaining one history entry costs, so the history can bound itself
/// by memory rather than by a count.
///
/// A count is the wrong unit here and measurably so: `edit_cost::undo_entry_memory`
/// puts a sixteen-channel song with sixteen notes to a pattern at 122 KB an
/// entry and a thirty-two channel one with a busy pattern at 1.14 MB, and a
/// song with twenty-four patterns and a full effect chain on every channel is
/// larger again. One ceiling of 256 entries therefore means 31 MB of one
/// song and most of a gigabyte of another, which is exactly backwards: the
/// heavy document is the one that can least afford it.
pub trait Retained {
    /// Roughly what this occupies, heap included. An estimate is the right
    /// shape of answer -- see [`mooloop_core::Project::heap_bytes`].
    fn retained_bytes(&self) -> usize;
}

/// How many edits the history keeps before the oldest starts falling off.
///
/// Still a depth as well as a budget, because a small document's entries are
/// cheap enough that [`MAX_RETAINED_BYTES`] would never bind and an
/// afternoon's editing would otherwise accumulate without limit. On the
/// sixteen-channel song this is what binds, at about 31 MB, which is the
/// behaviour this number has always had.
pub const MAX_ENTRIES: usize = 256;

/// How much memory the history may hold before the oldest entries fall off.
///
/// The ceiling that binds on a large song, where the count never would. 128
/// MB is chosen against the measured table: it leaves every document the
/// count already bounded completely unaffected, and it stops a heavy one at
/// a size that is a fraction of what it reached before.
///
/// Deliberately generous rather than tight. Undo depth is the thing being
/// traded away, and a musician noticing they cannot undo far enough is a
/// worse failure than a hundred megabytes on a machine that has them.
pub const MAX_RETAINED_BYTES: usize = 128 * 1024 * 1024;

/// The depth the byte budget may never trim below.
///
/// A budget alone is not safe: a document heavy enough would reduce the
/// history to one entry, which is a worse bug than the memory it saves. Below
/// this many entries the budget stops applying and the memory is simply
/// spent -- an undo that reaches sixteen edits back is the floor of useful.
pub const MIN_ENTRIES: usize = 16;

pub struct History<T> {
    entries: Vec<Entry<T>>,
    /// What each entry in `entries` reported costing, measured once when it
    /// was recorded rather than re-walked on every edit. Parallel to
    /// `entries` and maintained only by the two methods that change it.
    sizes: Vec<usize>,
    /// Number of entries currently applied.  Entries after this cursor are
    /// the redo branch.
    cursor: usize,
}

impl<T> Default for History<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            sizes: Vec::new(),
            cursor: 0,
        }
    }
}

impl<T: Retained> History<T> {
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
        self.sizes.truncate(self.cursor);
        if let Some(gesture) = entry.gesture {
            if let Some(open) = self.entries.last_mut() {
                if open.gesture == Some(gesture) {
                    open.after = entry.after;
                    // The entry grew or shrank with its new `after`, and the
                    // budget below is only honest if this is re-measured
                    // rather than left at what the first frame of the drag
                    // cost.
                    if let Some(size) = self.sizes.last_mut() {
                        *size = open.before.retained_bytes() + open.after.retained_bytes();
                    }
                    self.trim();
                    return;
                }
            }
        }
        self.sizes
            .push(entry.before.retained_bytes() + entry.after.retained_bytes());
        self.entries.push(entry);
        self.trim();
        self.cursor = self.entries.len();
    }

    /// Drop the oldest entries until both ceilings are satisfied.
    ///
    /// The oldest go, not the newest: undo reaches backwards, so the edits
    /// furthest from the cursor are the ones nobody is going to ask for.
    /// Dropping from the front leaves the cursor at the end either way.
    ///
    /// [`MIN_ENTRIES`] outranks the byte budget. A document heavy enough to
    /// blow the budget in a handful of edits still gets a usable history;
    /// the alternative is an undo that reaches back one step on exactly the
    /// songs where it matters most.
    fn trim(&mut self) {
        let mut retained: usize = self.sizes.iter().sum();
        let mut excess = 0;
        while self.entries.len() - excess > MIN_ENTRIES
            && (self.entries.len() - excess > MAX_ENTRIES || retained > MAX_RETAINED_BYTES)
        {
            retained = retained.saturating_sub(self.sizes[excess]);
            excess += 1;
        }
        if excess > 0 {
            self.entries.drain(..excess);
            self.sizes.drain(..excess);
        }
    }

    /// What the retained entries are estimated to occupy. For the tests that
    /// pin the budget; nothing in the interface asks.
    #[cfg(test)]
    pub fn retained_bytes(&self) -> usize {
        self.sizes.iter().sum()
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
    use super::{Entry, History, Retained, MAX_ENTRIES, MAX_RETAINED_BYTES, MIN_ENTRIES};

    /// The existing tests care about ordering and coalescing rather than
    /// memory, and an integer entry costs what an integer costs.
    impl Retained for i32 {
        fn retained_bytes(&self) -> usize {
            std::mem::size_of::<i32>()
        }
    }

    impl Retained for usize {
        fn retained_bytes(&self) -> usize {
            std::mem::size_of::<usize>()
        }
    }

    /// An entry that reports whatever size a test needs it to, so the budget
    /// can be exercised without building projects big enough to reach it.
    #[derive(Clone, Copy)]
    struct Heavy(usize);

    impl Retained for Heavy {
        fn retained_bytes(&self) -> usize {
            self.0
        }
    }

    fn heavy(bytes: usize) -> Entry<Heavy> {
        Entry {
            before: Heavy(bytes / 2),
            after: Heavy(bytes / 2),
            label: "heavy",
            gesture: None,
        }
    }

    /// The ceiling that binds on a small document is still the count, and it
    /// still binds where it always did.
    #[test]
    fn a_cheap_history_is_bounded_by_the_count_as_before() {
        let mut history: History<Heavy> = History::default();
        for _ in 0..MAX_ENTRIES + 50 {
            history.record(heavy(1024));
        }
        assert_eq!(history.retained(), MAX_ENTRIES);
    }

    /// The ceiling that binds on a large one is the budget, which the count
    /// alone would never have reached.
    #[test]
    fn an_expensive_history_is_bounded_by_memory_not_by_the_count() {
        // Eight megabytes an entry: 256 of them would be two gigabytes.
        let mut history: History<Heavy> = History::default();
        for _ in 0..MAX_ENTRIES {
            history.record(heavy(8 * 1024 * 1024));
        }
        assert!(history.retained() < MAX_ENTRIES, "{}", history.retained());
        assert!(
            history.retained_bytes() <= MAX_RETAINED_BYTES,
            "{} bytes retained",
            history.retained_bytes()
        );
    }

    /// A document heavy enough to blow the budget in a few edits still gets a
    /// history worth having. The floor outranks the budget.
    #[test]
    fn the_floor_outranks_the_budget_however_heavy_the_document() {
        let mut history: History<Heavy> = History::default();
        // One entry is already half the whole budget.
        for _ in 0..MIN_ENTRIES + 20 {
            history.record(heavy(MAX_RETAINED_BYTES / 2));
        }
        assert_eq!(history.retained(), MIN_ENTRIES);
        assert!(history.retained_bytes() > MAX_RETAINED_BYTES);
    }

    /// Coalescing a drag re-measures the entry rather than keeping the first
    /// frame's figure, so a gesture that grows the project is charged for it.
    #[test]
    fn coalescing_a_gesture_remeasures_what_it_now_costs() {
        let mut history: History<Heavy> = History::default();
        history.record(Entry {
            before: Heavy(16),
            after: Heavy(16),
            label: "drag",
            gesture: Some(1),
        });
        assert_eq!(history.retained_bytes(), 32);
        history.record(Entry {
            before: Heavy(16),
            after: Heavy(4096),
            label: "drag",
            gesture: Some(1),
        });
        assert_eq!(history.retained(), 1);
        assert_eq!(history.retained_bytes(), 16 + 4096);
    }

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
