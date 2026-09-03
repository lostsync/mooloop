//! Step-grid editing.
//!
//! A rack cell summarizes every note starting inside one sixteenth. All six
//! edits below therefore share the same shape: resolve the cell, gather what
//! starts in it, change that, and report which cells stopped matching what is
//! drawn.

use crate::session::Session;
use mooloop_core::{
    EngineCommand, NoteEvent, NoteId, DEFAULT_NOTE_DURATION_TICKS, TICKS_PER_STEP,
};
use std::ops::RangeInclusive;

/// What a step edit did: the commands the engine needs, in order, and the
/// span of cells in that channel's row whose contents have changed.
pub struct StepEdit {
    pub commands: Vec<EngineCommand>,
    pub redraw: RangeInclusive<usize>,
}

impl StepEdit {
    fn one(step: usize, commands: Vec<EngineCommand>) -> Self {
        Self {
            commands,
            redraw: step..=step,
        }
    }
}

/// A resolved rack cell: which channel and pattern, and the half-open tick
/// window the cell covers.
struct Cell {
    channel: usize,
    pattern: usize,
    step: usize,
    start: u32,
    end: u32,
}

impl Session {
    /// Resolves a rack cell, or `None` when the grid asked about one that is
    /// not there -- a channel that has gone, or a step past the pattern's
    /// logical length.
    fn cell(&self, channel: i32, step: i32) -> Option<Cell> {
        let channel = usize::try_from(channel).ok()?;
        let step = usize::try_from(step).ok()?;
        let pattern = self.current_pattern;
        if channel >= self.channels.len() || step >= self.pattern_lengths[pattern] {
            return None;
        }
        let start = step as u32 * TICKS_PER_STEP;
        Some(Cell {
            channel,
            pattern,
            step,
            start,
            end: start + TICKS_PER_STEP,
        })
    }

    /// Every note starting inside `cell`, in stored order.
    fn ids_in(&self, cell: &Cell) -> Vec<NoteId> {
        self.channels[cell.channel].notes[cell.pattern]
            .iter()
            .filter(|note| note.start_tick >= cell.start && note.start_tick < cell.end)
            .map(|note| note.id)
            .collect()
    }

    /// Removes `ids` from the cell's pattern and returns the commands that
    /// tell the engine so.
    fn erase(&mut self, cell: &Cell, ids: Vec<NoteId>) -> Vec<EngineCommand> {
        self.channels[cell.channel].notes[cell.pattern].retain(|note| !ids.contains(&note.id));
        self.prune_note_selection(&ids);
        ids.into_iter()
            .map(|id| EngineCommand::RemoveNote {
                pattern: cell.pattern as u8,
                channel: cell.channel as u8,
                id,
            })
            .collect()
    }

    /// Adds a default note at the cell's start.
    fn strike(&mut self, cell: &Cell) -> NoteEvent {
        self.channels[cell.channel].create_note(
            cell.pattern,
            cell.start,
            DEFAULT_NOTE_DURATION_TICKS,
            60,
        )
    }

    fn upsert_in(cell: &Cell, note: NoteEvent) -> EngineCommand {
        EngineCommand::UpsertNote {
            pattern: cell.pattern as u8,
            channel: cell.channel as u8,
            note,
        }
    }

    /// Adds an anchor note to an empty cell, or clears a populated one.
    pub fn toggle_step(&mut self, channel: i32, step: i32) -> Option<StepEdit> {
        let cell = self.cell(channel, step)?;
        let ids = self.ids_in(&cell);
        let commands = if ids.is_empty() {
            let note = self.strike(&cell);
            if cell.channel == self.selected {
                self.select_note(Some(note.id));
            }
            vec![Self::upsert_in(&cell, note)]
        } else {
            self.erase(&cell, ids)
        };
        Some(StepEdit::one(cell.step, commands))
    }

    /// Clears a cell whatever is in it. Right-click always means this, where
    /// a left click toggles.
    pub fn clear_step(&mut self, channel: i32, step: i32) -> Option<StepEdit> {
        let cell = self.cell(channel, step)?;
        let ids = self.ids_in(&cell);
        let commands = self.erase(&cell, ids);
        Some(StepEdit::one(cell.step, commands))
    }

    /// Sets the velocity of everything in a cell, striking a note first if the
    /// cell is empty. `value` is the control's 0..1 travel.
    pub fn set_step_velocity(&mut self, channel: i32, step: i32, value: f32) -> Option<StepEdit> {
        let cell = self.cell(channel, step)?;
        let velocity = (1.0 + value.clamp(0.0, 1.0) * 126.0).round() as u8;
        let mut edited: Vec<NoteEvent> = self.channels[cell.channel].notes[cell.pattern]
            .iter_mut()
            .filter(|note| note.start_tick >= cell.start && note.start_tick < cell.end)
            .map(|note| {
                note.velocity = velocity;
                *note
            })
            .collect();
        if edited.is_empty() {
            let mut note = self.strike(&cell);
            note.velocity = velocity;
            *self.channels[cell.channel].notes[cell.pattern]
                .iter_mut()
                .find(|stored| stored.id == note.id)
                .expect("the note just created is in the pattern") = note;
            edited.push(note);
        }
        let primary = (cell.channel == self.selected).then_some(edited[0].id);
        self.select_note(primary);
        Some(StepEdit::one(
            cell.step,
            edited
                .into_iter()
                .map(|note| Self::upsert_in(&cell, note))
                .collect(),
        ))
    }

    /// Paint-drag editing. Idempotent per call, so a drag may cross the same
    /// cell repeatedly without toggling it back off.
    pub fn paint_step(&mut self, channel: i32, step: i32, on: bool) -> Option<StepEdit> {
        let cell = self.cell(channel, step)?;
        let ids = self.ids_in(&cell);
        if on == !ids.is_empty() {
            return None;
        }
        let commands = if on {
            let note = self.strike(&cell);
            if cell.channel == self.selected {
                self.select_note(Some(note.id));
            }
            vec![Self::upsert_in(&cell, note)]
        } else {
            self.erase(&cell, ids)
        };
        Some(StepEdit::one(cell.step, commands))
    }

    /// Replaces a cell's contents with `divisions` evenly spaced notes.
    pub fn slice_step(&mut self, channel: i32, step: i32, divisions: i32) -> Option<StepEdit> {
        let cell = self.cell(channel, step)?;
        let divisions = divisions.clamp(2, 4) as u32;
        let ids = self.ids_in(&cell);
        let mut commands = self.erase(&cell, ids);
        let slice_ticks = TICKS_PER_STEP / divisions;
        for k in 0..divisions {
            let note = self.channels[cell.channel].create_note(
                cell.pattern,
                cell.start + k * slice_ticks,
                slice_ticks,
                60,
            );
            commands.push(Self::upsert_in(&cell, note));
        }
        Some(StepEdit::one(cell.step, commands))
    }

    /// Drag-resizes every note starting in a cell.
    ///
    /// Called on every frame of a drag, so a length that changes nothing
    /// reports nothing. The redraw span reaches wherever the notes used to end
    /// as well as wherever they now do, since shortening one leaves a stale
    /// cell behind it.
    pub fn drag_step_length(
        &mut self,
        channel: i32,
        step: i32,
        length_in_steps: i32,
    ) -> Option<StepEdit> {
        let cell = self.cell(channel, step)?;
        let pattern_length = self.pattern_lengths[cell.pattern];
        let max_length = (pattern_length - cell.step) as i32;
        let duration_ticks = length_in_steps.clamp(1, max_length) as u32 * TICKS_PER_STEP;
        let mut edited = Vec::new();
        let mut last_step = cell.step;
        for note in self.channels[cell.channel].notes[cell.pattern].iter_mut() {
            if note.start_tick < cell.start || note.start_tick >= cell.end {
                continue;
            }
            let was =
                ((note.start_tick + note.duration_ticks.max(1) - 1) / TICKS_PER_STEP) as usize;
            last_step = last_step.max(was);
            if note.duration_ticks == duration_ticks {
                continue;
            }
            note.duration_ticks = duration_ticks;
            let now = ((note.start_tick + duration_ticks - 1) / TICKS_PER_STEP) as usize;
            last_step = last_step.max(now);
            edited.push(*note);
        }
        if edited.is_empty() {
            return None;
        }
        Some(StepEdit {
            commands: edited
                .into_iter()
                .map(|note| Self::upsert_in(&cell, note))
                .collect(),
            redraw: cell.step..=last_step.min(pattern_length - 1),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note_count(session: &Session) -> usize {
        session.channels[0].notes[0].len()
    }

    #[test]
    fn clicking_a_cell_strikes_it_and_clicking_again_clears_it() {
        let mut session = Session::default();

        let edit = session.toggle_step(0, 2).expect("cell 2 exists");
        assert_eq!(note_count(&session), 1);
        assert!(matches!(
            edit.commands.as_slice(),
            [EngineCommand::UpsertNote { .. }]
        ));
        assert_eq!(edit.redraw, 2..=2);
        assert_eq!(
            session.channels[0].notes[0][0].start_tick,
            2 * TICKS_PER_STEP
        );

        let edit = session.toggle_step(0, 2).expect("cell 2 still exists");
        assert_eq!(note_count(&session), 0);
        assert!(matches!(
            edit.commands.as_slice(),
            [EngineCommand::RemoveNote { .. }]
        ));
    }

    /// A drag crosses the same cell many times; painting has to be the same
    /// answer every time rather than a toggle.
    #[test]
    fn painting_the_same_cell_twice_changes_nothing_the_second_time() {
        let mut session = Session::default();

        assert!(session.paint_step(0, 0, true).is_some());
        assert!(
            session.paint_step(0, 0, true).is_none(),
            "painting an already-struck cell reported an edit"
        );
        assert_eq!(note_count(&session), 1);

        assert!(session.paint_step(0, 0, false).is_some());
        assert!(session.paint_step(0, 0, false).is_none());
        assert_eq!(note_count(&session), 0);
    }

    /// Cells the grid asks about but the document does not have are refused
    /// rather than panicking on the index.
    #[test]
    fn cells_outside_the_pattern_are_refused() {
        let mut session = Session::default();
        let length = session.pattern_lengths[0] as i32;
        assert!(session.toggle_step(0, length).is_none());
        assert!(session.toggle_step(0, -1).is_none());
        assert!(session.toggle_step(1, 0).is_none());
        assert!(session.toggle_step(-1, 0).is_none());
        assert_eq!(note_count(&session), 0);
    }

    #[test]
    fn slicing_replaces_a_cell_with_evenly_spaced_notes() {
        let mut session = Session::default();
        session.toggle_step(0, 1);

        session.slice_step(0, 1, 3).expect("cell 1 exists");

        let starts: Vec<u32> = session.channels[0].notes[0]
            .iter()
            .map(|note| note.start_tick)
            .collect();
        let slice = TICKS_PER_STEP / 3;
        assert_eq!(
            starts,
            vec![
                TICKS_PER_STEP,
                TICKS_PER_STEP + slice,
                TICKS_PER_STEP + 2 * slice
            ]
        );
        // Out-of-range division counts clamp rather than producing nonsense.
        session.slice_step(0, 1, 99).expect("cell 1 exists");
        assert_eq!(session.channels[0].notes[0].len(), 4);
    }

    /// Shortening a note leaves the cells it used to reach stale, so the
    /// redraw span has to cover where it was as well as where it is.
    #[test]
    fn a_length_drag_redraws_where_the_note_was_as_well_as_where_it_is() {
        let mut session = Session::default();
        session.toggle_step(0, 0);

        let grew = session.drag_step_length(0, 0, 4).expect("notes in cell 0");
        assert_eq!(grew.redraw, 0..=3);

        let shrank = session.drag_step_length(0, 0, 1).expect("notes in cell 0");
        assert_eq!(shrank.redraw, 0..=3, "the cells it vacated were not redrawn");

        assert!(
            session.drag_step_length(0, 0, 1).is_none(),
            "a drag frame that changes no duration reported an edit"
        );

        // The length cannot reach past the end of the pattern.
        let length = session.pattern_lengths[0];
        let clamped = session.drag_step_length(0, 0, 999).expect("notes in cell 0");
        assert_eq!(*clamped.redraw.end(), length - 1);
    }

    #[test]
    fn velocity_strikes_an_empty_cell_rather_than_doing_nothing() {
        let mut session = Session::default();

        session.set_step_velocity(0, 0, 1.0).expect("cell 0 exists");
        assert_eq!(session.channels[0].notes[0][0].velocity, 127);

        session.set_step_velocity(0, 0, 0.0).expect("cell 0 exists");
        assert_eq!(session.channels[0].notes[0][0].velocity, 1);
        assert_eq!(note_count(&session), 1, "a second note was struck");
    }
}
