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

/// A stream of edits that arrives from outside any control, and so has no
/// press and release to say where it starts and stops.
///
/// The pump applies two kinds of edit nobody brackets: a mapped hardware
/// control moving a parameter, and a note played into an armed pattern. Each
/// arrives as a message every few milliseconds, for as long as the knob turns
/// or the take runs. Recording one entry per message would spend the whole
/// history on one knob sweep; recording none is worse, because undo installs
/// a whole-project snapshot and an edit that never reached the history is
/// *destroyed* by the next Ctrl+Z (MOO-96, MOO-97).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    /// Parameter moves from mapped hardware controls. Closes when the
    /// controls go idle.
    Controller,
    /// Notes recorded from a MIDI keyboard over one armed run. Closes when
    /// the transport stops or recording is disarmed.
    Recording,
}

/// An entry whose edits are still arriving: its `before` is fixed, and its
/// `after` is whatever the document is when something closes it.
struct Open<T> {
    before: T,
    label: &'static str,
    stream: Stream,
}

pub struct History<T> {
    entries: Vec<Entry<T>>,
    /// What each entry in `entries` reported costing, measured once when it
    /// was recorded rather than re-walked on every edit. Parallel to
    /// `entries` and maintained only by the two methods that change it.
    sizes: Vec<usize>,
    /// Number of entries currently applied.  Entries after this cursor are
    /// the redo branch.
    cursor: usize,
    /// The stream entry still collecting edits, if one is. See [`Stream`] and
    /// [`History::open`].
    open: Option<Open<T>>,
}

impl<T> Default for History<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            sizes: Vec::new(),
            cursor: 0,
            open: None,
        }
    }
}

impl<T: Retained + Clone> History<T> {
    /// Start collecting a stream of edits into one entry, from `before`.
    ///
    /// One snapshot at the start and one at the end, however many messages
    /// arrive between -- the same bargain the `Gesture` bracket makes for a
    /// knob drag, for the edits that have no bracket. The entry is recorded
    /// when [`History::close`] is called, or as soon as anything else reaches
    /// the history, whichever is first.
    ///
    /// **A stream already open is closed at `before`, not dropped.** The
    /// snapshot that opens this one is the document as the last one left it,
    /// so it is exactly the `after` that one needs: a take that starts while
    /// a knob is still settling records the knob, then the take.
    ///
    /// Opening one is an edit, so it discards the redo branch now, as
    /// [`History::record`] would.
    pub fn open(&mut self, stream: Stream, before: T, label: &'static str) {
        if let Some(open) = self.open.take() {
            let after = before.clone();
            self.push(Entry {
                before: open.before,
                after,
                label: open.label,
                gesture: None,
            });
        }
        self.entries.truncate(self.cursor);
        self.sizes.truncate(self.cursor);
        self.open = Some(Open {
            before,
            label,
            stream,
        });
    }

    /// Record the open stream as one entry ending at `after`. Nothing open
    /// is not an error: a stream something else already closed has nothing
    /// left to record.
    pub fn close(&mut self, after: T) {
        if let Some(open) = self.open.take() {
            self.push(Entry {
                before: open.before,
                after,
                label: open.label,
                gesture: None,
            });
        }
    }

    /// Which stream is collecting, if any.
    pub fn open_stream(&self) -> Option<Stream> {
        self.open.as_ref().map(|open| open.stream)
    }

    /// The document as the open stream found it, for asking what the history
    /// still refers to: an undo will want it back as surely as any recorded
    /// entry's `before`.
    pub fn open_before(&self) -> Option<&T> {
        self.open.as_ref().map(|open| &open.before)
    }

    /// Record an edit that has already been successfully installed.
    ///
    /// Extends the top entry instead of pushing when both carry the same
    /// gesture token. Tokens are compared rather than labels so two separate
    /// drags of the same kind stay two undo steps.
    ///
    /// **An open stream is closed first, at this entry's `before`.** That is
    /// the document as the stream left it and as this edit found it, so the
    /// two entries meet exactly: undoing this edit leaves the stream's edits
    /// in place, and undoing again removes them. Every recorder reaches the
    /// history through here, so no edit can land on top of a stream that is
    /// still open.
    pub fn record(&mut self, entry: Entry<T>) {
        if self.open.is_some() {
            self.close(entry.before.clone());
        }
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
                    // `trim`'s own doc says dropping from the front "leaves
                    // the cursor at the end either way", which is only true
                    // if somebody puts it there. The push path below does;
                    // this one returned without it, so a trim here left
                    // `cursor > entries.len()` -- `can_undo` true, and
                    // `undo_target` `None`, which is an enabled Ctrl+Z that
                    // does nothing until the next non-coalescing record.
                    // Reachable because this branch re-measures a growing
                    // gesture against a budget already at its ceiling.
                    self.cursor = self.entries.len();
                    return;
                }
            }
        }
        self.push(entry);
    }

    /// Append `entry` at the cursor, dropping the redo branch, and keep both
    /// ceilings.
    fn push(&mut self, entry: Entry<T>) {
        self.entries.truncate(self.cursor);
        self.sizes.truncate(self.cursor);
        self.sizes
            .push(entry.before.retained_bytes() + entry.after.retained_bytes());
        self.entries.push(entry);
        self.trim();
        self.cursor = self.entries.len();
    }
}

impl<T: Retained> History<T> {
    /// Every entry still retained, undo branch and redo branch alike.
    ///
    /// For asking what the history as a whole still refers to --
    /// `recordings::referenced_paths` walks these so a take an undo could
    /// reach is not offered for deletion. Deliberately not cursor-aware: a
    /// redo reaches its snapshot just as an undo does.
    pub fn entries(&self) -> &[Entry<T>] {
        &self.entries
    }

    /// An open stream counts: it is an edit, and Undo has to be enabled the
    /// moment a knob on a desk has moved something.
    pub fn can_undo(&self) -> bool {
        self.cursor > 0 || self.open.is_some()
    }

    /// Opening a stream discarded the redo branch, as any edit does.
    pub fn can_redo(&self) -> bool {
        self.open.is_none() && self.cursor < self.entries.len()
    }

    /// What an undo would install. **`None` while a stream is open**: the
    /// open entry has no `after` yet, and the entry below it is not what an
    /// undo should reach. The caller closes the stream first, with the
    /// document as it is now -- `close_edit_stream` in `mooloop-ui` -- and a
    /// caller that forgot gets a Ctrl+Z that does nothing, which is visible,
    /// rather than one that skips the stream, which destroys it.
    pub fn undo_target(&self) -> Option<&Entry<T>> {
        if self.open.is_some() {
            return None;
        }
        self.cursor
            .checked_sub(1)
            .and_then(|index| self.entries.get(index))
    }

    pub fn redo_target(&self) -> Option<&Entry<T>> {
        if self.open.is_some() {
            return None;
        }
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

    /// Forget everything.
    ///
    /// An entry holds a whole-document snapshot, so it is only meaningful
    /// against the document it was taken from: undoing into a snapshot of a
    /// song that is no longer open installs *that song* over this one, and
    /// the save path would then write it to this one's path. Opening or
    /// starting a document calls this for the same reason it clears the
    /// preset-label maps beside it.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.sizes.clear();
        self.cursor = 0;
        self.open = None;
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

    /// A coalescing record that trims must leave the cursor where the
    /// pushing one does.
    ///
    /// The push path re-syncs the cursor after `trim`; the coalescing path
    /// returned without it, so a trim there left `cursor > entries.len()` --
    /// `can_undo()` true (the cursor is non-zero) while `undo_target()`
    /// returns `None` (the index is past the end). An enabled Ctrl+Z that
    /// does nothing, until the next non-coalescing record puts the cursor
    /// back.
    ///
    /// It is reachable because this is the branch that *re-measures* a
    /// growing gesture against a budget already at its ceiling -- which is
    /// the steady state for a heavy song, since `trim` leaves the history at
    /// or just under the budget after every record.
    #[test]
    fn a_coalescing_record_that_trims_leaves_the_cursor_at_the_end() {
        let mut history: History<Heavy> = History::default();
        // Small enough that the *count* is not what binds -- `MIN_ENTRIES`
        // outranks the budget, so a history sitting at sixteen huge entries
        // cannot trim at all and the branch never runs. Forty cheap ones
        // leave room to evict.
        let each = MAX_RETAINED_BYTES / 64;
        for _ in 0..40 {
            history.record(heavy(each));
        }
        assert!(
            history.entries.len() > MIN_ENTRIES,
            "the count must not be the binding cap, or nothing can be evicted"
        );

        // Open a gesture, then grow it enough to breach the budget. The
        // second record coalesces, re-measures the entry upward, and trims.
        history.record(Entry {
            gesture: Some(1),
            ..heavy(each)
        });
        let before = history.entries.len();
        // Only `after` is replaced when an entry coalesces -- `before` is
        // the one the gesture opened with -- so the growth has to be spelled
        // on `after` alone to be sure it breaches the budget.
        history.record(Entry {
            before: Heavy(each / 2),
            after: Heavy(MAX_RETAINED_BYTES),
            label: "heavy",
            gesture: Some(1),
        });

        assert!(
            history.entries.len() < before,
            "the grown gesture has to have evicted something, or this test \
             proves nothing: {} entries before, {} after",
            before,
            history.entries.len()
        );
        assert_eq!(
            history.cursor,
            history.entries.len(),
            "the cursor has to land at the end, as it does on the push path"
        );
        assert_eq!(
            history.can_undo(),
            history.undo_target().is_some(),
            "an enabled Undo must have something to undo"
        );
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
mod stream_tests {
    use super::{Entry, History, Stream};

    fn edit(before: i32, after: i32, label: &'static str) -> Entry<i32> {
        Entry {
            before,
            after,
            label,
            gesture: None,
        }
    }

    /// A knob sweep from a desk is one entry, from the document the first
    /// move found to the one the last move left, however many messages came
    /// between -- and Undo is live the moment the first one lands.
    #[test]
    fn a_stream_is_one_entry_from_where_it_opened_to_where_it_closed() {
        let mut history = History::default();
        history.open(Stream::Controller, 0, "Cutoff");
        assert!(history.can_undo(), "a moved knob is something to undo");
        assert!(!history.can_redo());
        assert_eq!(history.open_stream(), Some(Stream::Controller));
        assert!(
            history.undo_target().is_none(),
            "an open stream has no after yet, and the entry under it is not the \
             one an undo should reach"
        );

        history.close(5);
        assert_eq!(history.open_stream(), None);
        let entry = history.undo_target().expect("the stream is an entry now");
        assert_eq!((entry.before, entry.after, entry.label), (0, 5, "Cutoff"));
        assert_eq!(history.retained(), 1);
    }

    /// The failure this exists for, in miniature: an edit lands while a take
    /// is still collecting notes. Nothing may be lost and the two must meet
    /// exactly, so undoing the edit leaves the notes and undoing again takes
    /// them.
    #[test]
    fn an_edit_on_top_of_an_open_stream_closes_it_where_the_edit_began() {
        let mut history = History::default();
        history.record(edit(0, 1, "Toggle step"));
        history.open(Stream::Recording, 1, "Record notes");
        // Notes landed (1 -> 4), then a knob on screen was turned (4 -> 5).
        history.record(edit(4, 5, "Cutoff"));

        assert_eq!(history.open_stream(), None);
        assert_eq!(history.retained(), 3);
        let top = history.undo_target().unwrap();
        assert_eq!((top.before, top.after, top.label), (4, 5, "Cutoff"));
        history.commit_undo();
        let pass = history.undo_target().unwrap();
        assert_eq!((pass.before, pass.after, pass.label), (1, 4, "Record notes"));
        history.commit_undo();
        let first = history.undo_target().unwrap();
        assert_eq!((first.before, first.label), (0, "Toggle step"));
    }

    /// Two streams in a row meet at the snapshot the second one opened with.
    #[test]
    fn opening_a_stream_closes_the_one_already_open() {
        let mut history = History::default();
        history.open(Stream::Controller, 0, "Cutoff");
        history.open(Stream::Recording, 3, "Record notes");
        assert_eq!(history.open_stream(), Some(Stream::Recording));
        history.close(7);

        let pass = history.undo_target().unwrap();
        assert_eq!((pass.before, pass.after), (3, 7));
        history.commit_undo();
        let knob = history.undo_target().unwrap();
        assert_eq!((knob.before, knob.after, knob.label), (0, 3, "Cutoff"));
    }

    /// A stream is an edit, so it ends the redo branch when it starts rather
    /// than when it is recorded -- otherwise Redo would be offered over
    /// knob moves it would destroy.
    #[test]
    fn opening_a_stream_discards_the_redo_branch() {
        let mut history = History::default();
        history.record(edit(0, 1, "one"));
        history.record(edit(1, 2, "two"));
        history.commit_undo();
        assert!(history.can_redo());

        history.open(Stream::Controller, 1, "Cutoff");
        assert!(!history.can_redo());
        history.close(9);
        assert!(!history.can_redo());
        assert_eq!(history.retained(), 2);
        assert_eq!(history.undo_target().map(|entry| entry.after), Some(9));
    }

    /// A new document forgets the stream with the rest: its `before` is a
    /// snapshot of the song that was open.
    #[test]
    fn clearing_the_history_drops_an_open_stream() {
        let mut history = History::default();
        history.open(Stream::Recording, 0, "Record notes");
        history.clear();
        assert_eq!(history.open_stream(), None);
        assert!(!history.can_undo());
        history.close(3);
        assert!(!history.can_undo(), "nothing was open to record");
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
