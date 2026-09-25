//! The notes a hosted instrument is holding, by the ids each side uses
//! (`docs/plans/plugin-hosting/10-clap-instruments.md`, MOO-85).
//!
//! mooloop names a note with a `u64` that the sequencer, a MIDI keyboard or
//! an audition mints; CLAP names one with a non-negative `i32`. The table
//! maps the one to the other for every note the plugin is holding, so that a
//! note-off reaches the voice its note-on started, and a note-off whose
//! note-on the plugin never heard -- it arrived after a choke, a stop or a
//! full table let the note go -- is dropped rather than sent as a wildcard
//! that would release every voice on the key.
//!
//! **Fixed capacity, allocated with the processor.** Nothing here allocates
//! after [`NoteTable::new`]. When the table is full, the oldest note is
//! released to make room, which is what a synth out of voices does anyway.
//! A plugin's `note_end` removes its note early, so a voice that finished on
//! its own (a one-shot, a release that ran out) frees its row.

/// Notes a hosted instrument can be holding at once, as far as the host
/// tracks them. Past this the oldest is released.
pub const NOTE_ROWS: usize = 128;

/// One held note: mooloop's id, the plugin's id, and the key it is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldNote {
    pub id: u64,
    pub note_id: u32,
    pub key: u8,
}

#[derive(Clone, Copy)]
struct Row {
    note: HeldNote,
    /// When it started, in note-ons: the smallest is the oldest.
    serial: u64,
}

/// See the module documentation.
pub struct NoteTable {
    rows: Box<[Option<Row>]>,
    held: usize,
    serial: u64,
    next_note_id: u32,
}

impl NoteTable {
    pub fn new() -> Self {
        Self {
            rows: vec![None; NOTE_ROWS].into_boxed_slice(),
            held: 0,
            serial: 0,
            next_note_id: 0,
        }
    }

    /// How many notes are held.
    pub fn len(&self) -> usize {
        self.held
    }

    pub fn is_empty(&self) -> bool {
        self.held == 0
    }

    /// Start note `id` on `key`, returning the plugin's id for it, and the
    /// note released to make room when the table was full. The caller sends
    /// the released note's note-off first.
    pub fn start(&mut self, id: u64, key: u8) -> (HeldNote, Option<HeldNote>) {
        let mut evicted = None;
        let row = match self.rows.iter().position(Option::is_none) {
            Some(free) => free,
            None => {
                let oldest = self
                    .rows
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, row)| row.map_or(u64::MAX, |row| row.serial))
                    .map_or(0, |(index, _)| index);
                evicted = self.rows[oldest].take().map(|row| row.note);
                self.held -= 1;
                oldest
            }
        };
        let note = HeldNote {
            id,
            note_id: self.next_note_id,
            key,
        };
        // CLAP's note ids are `0..i32::MAX`; wrapping there is fine, since
        // no note held from two billion notes ago is still in the table.
        self.next_note_id = (self.next_note_id + 1) % (i32::MAX as u32);
        self.rows[row] = Some(Row {
            note,
            serial: self.serial,
        });
        self.serial += 1;
        self.held += 1;
        (note, evicted)
    }

    /// Stop note `id`: the held note to send the note-off for, or `None`
    /// when the plugin is not holding it and nothing should be sent.
    pub fn stop(&mut self, id: u64) -> Option<HeldNote> {
        self.take_where(|note| note.id == id)
    }

    /// The plugin says its note `note_id` has ended: forget it.
    pub fn ended(&mut self, note_id: u32) {
        let _ = self.take_where(|note| note.note_id == note_id);
    }

    /// Every held note, released: for a choke, each one's note-off.
    pub fn release_all(&mut self, mut each: impl FnMut(HeldNote)) {
        let mut released: [Option<Row>; NOTE_ROWS] = [None; NOTE_ROWS];
        let mut count = 0;
        for row in self.rows.iter_mut() {
            if let Some(held) = row.take() {
                released[count] = Some(held);
                count += 1;
            }
        }
        self.held = 0;
        // Oldest first, so a plugin sees them in the order they started.
        released[..count].sort_unstable_by_key(|row| row.map_or(u64::MAX, |row| row.serial));
        for row in released[..count].iter().flatten() {
            each(row.note);
        }
    }

    /// Forget every note without releasing it: the plugin was reset and is
    /// holding none.
    pub fn clear(&mut self) {
        self.rows.iter_mut().for_each(|row| *row = None);
        self.held = 0;
    }

    fn take_where(&mut self, wanted: impl Fn(&HeldNote) -> bool) -> Option<HeldNote> {
        let row = self
            .rows
            .iter_mut()
            .find(|row| row.is_some_and(|row| wanted(&row.note)))?;
        self.held -= 1;
        row.take().map(|row| row.note)
    }
}

impl Default for NoteTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_off_reaches_its_own_note_and_an_unknown_one_reaches_nothing() {
        let mut table = NoteTable::new();
        let (a, _) = table.start(10, 60);
        let (b, _) = table.start(11, 60);
        assert_ne!(a.note_id, b.note_id, "two notes on one key are two voices");
        assert_eq!(table.stop(11), Some(b));
        assert_eq!(table.stop(11), None, "a second note-off for it is stale");
        assert_eq!(table.stop(99), None);
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn a_full_table_releases_its_oldest_note() {
        let mut table = NoteTable::new();
        let (first, _) = table.start(0, 36);
        for id in 1..NOTE_ROWS as u64 {
            assert_eq!(table.start(id, 40).1, None);
        }
        let (_, evicted) = table.start(1_000, 50);
        assert_eq!(evicted, Some(first));
        assert_eq!(table.len(), NOTE_ROWS);
        assert_eq!(table.stop(0), None, "the released note is no longer held");
    }

    #[test]
    fn a_note_end_frees_its_row_and_a_choke_releases_the_rest_in_order() {
        let mut table = NoteTable::new();
        let (a, _) = table.start(1, 60);
        let (b, _) = table.start(2, 62);
        let (c, _) = table.start(3, 64);
        table.ended(b.note_id);
        let mut released = Vec::new();
        table.release_all(|note| released.push(note));
        assert_eq!(released, [a, c]);
        assert!(table.is_empty());
        assert_eq!(table.stop(1), None, "a note-off after the choke is stale");
    }
}
