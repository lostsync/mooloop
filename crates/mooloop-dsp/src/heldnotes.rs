//! The held-note stack a monophonic voice needs to behave like a monosynth
//! rather than like a synth with one voice.
//!
//! Kept separate from any one instrument because it is not one instrument's
//! problem: the v2 mono synth uses it now, and the poly synth needs the same
//! behaviour the moment it grows a mono mode.
//!
//! Two rules the rest of the engine depends on:
//!
//! - **Fixed size, no allocation.** This is touched from `process()`, so the
//!   storage is an array. On overflow the *oldest* entry is dropped rather
//!   than the new note rejected: a seventeen-note-deep mono chord is not a
//!   real performance, and refusing the newest note is the audible failure.
//! - **Event ids are the identity of a note.** Removal matches on `event_id`
//!   and never on pitch, so a stale `NoteOff` cannot evict a newer entry that
//!   happens to share a note number.

use mooloop_core::NotePriority;

/// How many simultaneously held notes are tracked.
pub const MAX_HELD_NOTES: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldNote {
    pub event_id: u64,
    pub note: u8,
    /// Carried so that falling back to this note restores *its* velocity, not
    /// the velocity of the note that was just released.
    pub velocity: u8,
}

/// Oldest-first, so the last entry is the most recently pressed note.
pub struct HeldNotes {
    notes: [HeldNote; MAX_HELD_NOTES],
    len: usize,
}

impl Default for HeldNotes {
    fn default() -> Self {
        Self::new()
    }
}

impl HeldNotes {
    pub fn new() -> Self {
        Self {
            notes: [HeldNote {
                event_id: 0,
                note: 0,
                velocity: 0,
            }; MAX_HELD_NOTES],
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Record a newly pressed note. Dropping the oldest entry when full keeps
    /// the note the player just played.
    pub fn push(&mut self, note: HeldNote) {
        if self.len == MAX_HELD_NOTES {
            self.notes.copy_within(1.., 0);
            self.len -= 1;
        }
        self.notes[self.len] = note;
        self.len += 1;
    }

    /// Release the entry with this event id, preserving press order. Returns
    /// whether anything was actually held under that id — a `false` here is a
    /// stale `NoteOff` and the caller should do nothing at all.
    pub fn remove(&mut self, event_id: u64) -> bool {
        let Some(index) = self.notes[..self.len]
            .iter()
            .position(|held| held.event_id == event_id)
        else {
            return false;
        };
        self.notes.copy_within(index + 1..self.len, index);
        self.len -= 1;
        true
    }

    /// The note the voice should be playing, or `None` when nothing is held.
    /// `Low` and `High` break ties towards the most recently pressed note, so
    /// re-pressing a pitch that is already down still takes the voice.
    pub fn winner(&self, priority: NotePriority) -> Option<HeldNote> {
        let held = &self.notes[..self.len];
        match priority {
            NotePriority::Last => held.last().copied(),
            NotePriority::Low => held
                .iter()
                .copied()
                .reduce(|best, next| if next.note <= best.note { next } else { best }),
            NotePriority::High => held
                .iter()
                .copied()
                .reduce(|best, next| if next.note >= best.note { next } else { best }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(event_id: u64, note: u8) -> HeldNote {
        HeldNote {
            event_id,
            note,
            velocity: 100,
        }
    }

    fn stack(notes: &[(u64, u8)]) -> HeldNotes {
        let mut stack = HeldNotes::new();
        for (event_id, note) in notes {
            stack.push(held(*event_id, *note));
        }
        stack
    }

    #[test]
    fn each_priority_picks_its_documented_winner() {
        let stack = stack(&[(1, 60), (2, 48), (3, 72)]);
        assert_eq!(stack.winner(NotePriority::Last).unwrap().note, 72);
        assert_eq!(stack.winner(NotePriority::Low).unwrap().note, 48);
        assert_eq!(stack.winner(NotePriority::High).unwrap().note, 72);
    }

    #[test]
    fn ties_go_to_the_most_recent_press() {
        let stack = stack(&[(1, 60), (2, 60)]);
        assert_eq!(stack.winner(NotePriority::Low).unwrap().event_id, 2);
        assert_eq!(stack.winner(NotePriority::High).unwrap().event_id, 2);
    }

    #[test]
    fn releasing_the_winner_falls_back_to_the_next_held_note() {
        let mut stack = stack(&[(1, 60), (2, 48), (3, 72)]);
        assert!(stack.remove(3));
        assert_eq!(stack.winner(NotePriority::Last).unwrap().note, 48);
        assert_eq!(stack.winner(NotePriority::High).unwrap().note, 60);
    }

    #[test]
    fn releasing_a_non_winner_leaves_the_winner_alone() {
        let mut stack = stack(&[(1, 60), (2, 48)]);
        assert!(stack.remove(1));
        assert_eq!(stack.winner(NotePriority::Last).unwrap().event_id, 2);
    }

    /// The guard for the whole design: ids are the identity, so a late
    /// `NoteOff` for a pitch that has since been re-pressed must not evict the
    /// live entry.
    #[test]
    fn a_stale_note_off_does_not_evict_a_re_pressed_pitch() {
        let mut stack = stack(&[(1, 60)]);
        assert!(stack.remove(1));
        stack.push(held(2, 60));
        assert!(!stack.remove(1));
        assert_eq!(stack.len(), 1);
        assert_eq!(stack.winner(NotePriority::Last).unwrap().event_id, 2);
    }

    #[test]
    fn overflow_drops_the_oldest_note_not_the_newest() {
        let mut stack = HeldNotes::new();
        for index in 0..MAX_HELD_NOTES as u64 + 2 {
            stack.push(held(index, 60 + index as u8));
        }
        assert_eq!(stack.len(), MAX_HELD_NOTES);
        assert_eq!(
            stack.winner(NotePriority::Last).unwrap().event_id,
            MAX_HELD_NOTES as u64 + 1
        );
        // The two oldest are gone; the third-oldest survived.
        assert!(!stack.remove(0));
        assert!(!stack.remove(1));
        assert!(stack.remove(2));
    }

    #[test]
    fn an_empty_stack_has_no_winner() {
        let mut stack = stack(&[(1, 60)]);
        stack.clear();
        assert!(stack.is_empty());
        assert!(stack.winner(NotePriority::Last).is_none());
    }
}
