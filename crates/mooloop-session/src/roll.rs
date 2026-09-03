//! Piano-roll editing.
//!
//! Every edit here acts on the selected channel's notes in the current
//! pattern. They were callback bodies; as methods they can be reached from a
//! menu, a shortcut and a test, and the group-clamping rules that three of the
//! drag gestures share are now stated where they can be read side by side.

use crate::notes::ScaleBase;
use crate::session::Session;
use mooloop_core::{EngineCommand, NoteEvent, NoteId, TICKS_PER_STEP};
use std::collections::{BTreeMap, HashSet};

/// What a roll edit did.
pub struct NoteEdit {
    /// Engine commands, in the order the engine must see them.
    pub commands: Vec<EngineCommand>,
    /// Rack cells in the edited channel's row whose summary changed, or `None`
    /// when enough moved that the whole row is redrawn.
    pub cells: Option<Vec<usize>>,
    /// How many notes the edit touched. The status bar reports it.
    pub notes: usize,
}

/// A pointer drag reports where the *grabbed* note should land. Every other
/// selected note moves by the same delta, so a chord keeps its shape -- and
/// the delta is clamped by the group rather than per note, because letting
/// members clip individually would silently flatten the chord at the edge of
/// the range.
struct GroupBounds {
    min_tick: i64,
    max_tick: i64,
    min_note: i32,
    max_note: i32,
}

impl Session {
    /// The channel and pattern the roll is showing.
    fn roll(&self) -> (usize, usize) {
        (self.selected, self.current_pattern)
    }

    /// The current pattern's logical length in ticks.
    fn pattern_ticks(&self) -> u32 {
        self.pattern_lengths[self.current_pattern] as u32 * TICKS_PER_STEP
    }

    fn upsert(&self, note: NoteEvent) -> EngineCommand {
        let (channel, pattern) = self.roll();
        EngineCommand::UpsertNote {
            pattern: pattern as u8,
            channel: channel as u8,
            note,
        }
    }

    fn remove(&self, id: NoteId) -> EngineCommand {
        let (channel, pattern) = self.roll();
        EngineCommand::RemoveNote {
            pattern: pattern as u8,
            channel: channel as u8,
            id,
        }
    }

    /// Hands out the next note id on the selected channel.
    fn next_id(&mut self) -> NoteId {
        let channel = self.selected;
        let id = self.channels[channel].next_note_id;
        self.channels[channel].next_note_id = id.wrapping_add(1).max(1);
        id
    }

    fn sort_notes(&mut self) {
        let (channel, pattern) = self.roll();
        self.channels[channel].notes[pattern].sort_by_key(|note| (note.start_tick, note.id));
    }

    /// Which notes a gesture on `anchor` acts on.
    ///
    /// Grabbing a note that is not selected acts on that note alone; grabbing
    /// one that is acts on the whole selection. Stale ids -- selected notes
    /// that have since gone -- are dropped.
    pub fn selection_including(&self, anchor: NoteId) -> HashSet<NoteId> {
        let (channel, pattern) = self.roll();
        if !self.selected_note_ids.contains(&anchor) {
            return HashSet::from([anchor]);
        }
        let live: HashSet<NoteId> = self.channels[channel].notes[pattern]
            .iter()
            .map(|note| note.id)
            .collect();
        let mut acting: HashSet<NoteId> = self
            .selected_note_ids
            .intersection(&live)
            .copied()
            .collect();
        acting.insert(anchor);
        acting
    }

    /// The tick and pitch extent of `moving`, or `None` when none of them are
    /// in the pattern.
    fn group_bounds(&self, moving: &HashSet<NoteId>) -> Option<GroupBounds> {
        let (channel, pattern) = self.roll();
        let mut bounds = GroupBounds {
            min_tick: i64::MAX,
            max_tick: i64::MIN,
            min_note: i32::MAX,
            max_note: i32::MIN,
        };
        for note in self.channels[channel].notes[pattern]
            .iter()
            .filter(|note| moving.contains(&note.id))
        {
            bounds.min_tick = bounds.min_tick.min(note.start_tick as i64);
            bounds.max_tick = bounds.max_tick.max(note.start_tick as i64);
            bounds.min_note = bounds.min_note.min(note.note as i32);
            bounds.max_note = bounds.max_note.max(note.note as i32);
        }
        (bounds.min_tick != i64::MAX).then_some(bounds)
    }

    /// Clamps a (tick, pitch) delta so the whole group stays in the pattern.
    ///
    /// Bound by note *starts*, not by their tails: a note is allowed to
    /// overhang the pattern's logical end -- that is how a shortened pattern
    /// keeps its notes -- so measuring the tail would refuse to move a
    /// selection right the moment any member overhung.
    fn clamp_group_delta(&self, bounds: &GroupBounds, tick: i64, note: i32) -> (i64, i32) {
        let last_start = self.pattern_ticks().saturating_sub(1) as i64;
        (
            tick.clamp(
                -bounds.min_tick,
                (last_start - bounds.max_tick).max(-bounds.min_tick),
            ),
            note.clamp(-bounds.min_note, (127 - bounds.max_note).max(-bounds.min_note)),
        )
    }

    /// Applies a (tick, pitch) delta to every note in `moving`.
    fn shift_notes(&mut self, moving: &HashSet<NoteId>, tick: i64, pitch: i32) -> NoteEdit {
        let (channel, pattern) = self.roll();
        let length_ticks = self.pattern_ticks();
        let mut edited = Vec::new();
        let mut cells = Vec::new();
        for note in self.channels[channel].notes[pattern]
            .iter_mut()
            .filter(|note| moving.contains(&note.id))
        {
            cells.push((note.start_tick / TICKS_PER_STEP) as usize);
            note.start_tick = ((note.start_tick as i64 + tick).max(0) as u32)
                .min(length_ticks.saturating_sub(1));
            note.duration_ticks = note
                .duration_ticks
                .min(length_ticks.saturating_sub(note.start_tick).max(1));
            note.note = (note.note as i32 + pitch).clamp(0, 127) as u8;
            cells.push((note.start_tick / TICKS_PER_STEP) as usize);
            edited.push(*note);
        }
        self.sort_notes();
        NoteEdit {
            notes: edited.len(),
            commands: edited.iter().map(|note| self.upsert(*note)).collect(),
            cells: Some(cells),
        }
    }

    /// Arrow-key editing: the same group clamp the pointer drag uses, so a
    /// selection that hits the edge stops as one rather than flattening onto
    /// it note by note.
    pub fn nudge_selection(&mut self, tick_delta: i32, note_delta: i32) -> Option<NoteEdit> {
        let moving = self.selected_note_ids.clone();
        let bounds = self.group_bounds(&moving)?;
        let (tick, pitch) = self.clamp_group_delta(&bounds, tick_delta as i64, note_delta);
        if tick == 0 && pitch == 0 {
            return None;
        }
        Some(self.shift_notes(&moving, tick, pitch))
    }

    /// Moves the selection so the grabbed note lands where the drag says.
    pub fn move_selection(
        &mut self,
        anchor_id: NoteId,
        start_tick: i32,
        midi_note: i32,
    ) -> Option<NoteEdit> {
        let (channel, pattern) = self.roll();
        let length_ticks = self.pattern_ticks();
        let anchor = self.channels[channel].notes[pattern]
            .iter()
            .copied()
            .find(|note| note.id == anchor_id)?;
        let moving = self.selection_including(anchor.id);
        let wanted_tick = (start_tick.max(0) as u32).min(length_ticks.saturating_sub(1));
        let wanted_note = midi_note.clamp(0, 127) as u8;
        let bounds = self.group_bounds(&moving)?;
        let (tick, pitch) = self.clamp_group_delta(
            &bounds,
            wanted_tick as i64 - anchor.start_tick as i64,
            wanted_note as i32 - anchor.note as i32,
        );
        let edit = self.shift_notes(&moving, tick, pitch);
        if edit.notes == 1 {
            self.select_note(Some(anchor.id));
        }
        Some(edit)
    }

    /// Creates a note, clamped into the pattern, and selects it.
    pub fn create_roll_note(
        &mut self,
        start_tick: i32,
        midi_note: i32,
        duration_ticks: i32,
    ) -> (NoteId, NoteEdit) {
        let note = self.strike_roll_note(start_tick, midi_note, duration_ticks);
        self.select_note(Some(note.id));
        (
            note.id,
            NoteEdit {
                commands: vec![self.upsert(note)],
                cells: Some(vec![(note.start_tick / TICKS_PER_STEP) as usize]),
                notes: 1,
            },
        )
    }

    /// Paint-stroke note creation.
    ///
    /// A stroke adds what it lays down to the selection, so the run can be
    /// moved or lengthened without re-selecting it by hand.
    pub fn paint_roll_note(
        &mut self,
        start_tick: i32,
        midi_note: i32,
        duration_ticks: i32,
    ) -> NoteEdit {
        let note = self.strike_roll_note(start_tick, midi_note, duration_ticks);
        self.selected_note_ids.insert(note.id);
        self.selected_note_id = (self.selected_note_ids.len() == 1).then_some(note.id);
        NoteEdit {
            commands: vec![self.upsert(note)],
            cells: Some(vec![(note.start_tick / TICKS_PER_STEP) as usize]),
            notes: 1,
        }
    }

    /// Adds one note, clamped so neither its start nor its tail leaves the
    /// pattern.
    fn strike_roll_note(
        &mut self,
        start_tick: i32,
        midi_note: i32,
        duration_ticks: i32,
    ) -> NoteEvent {
        let (channel, pattern) = self.roll();
        let length_ticks = self.pattern_ticks();
        let start_tick = (start_tick.max(0) as u32).min(length_ticks.saturating_sub(1));
        let mut note = self.channels[channel].create_note(
            pattern,
            start_tick,
            duration_ticks.max(1) as u32,
            midi_note.clamp(0, 127) as u8,
        );
        note.duration_ticks = note
            .duration_ticks
            .min(length_ticks.saturating_sub(start_tick).max(1));
        if let Some(stored) = self.channels[channel].notes[pattern]
            .iter_mut()
            .find(|stored| stored.id == note.id)
        {
            *stored = note;
        }
        note
    }

    /// Applies a click on note `id`. `mode` is which gesture role the held
    /// modifiers satisfied, resolved by the grid: 1 toggles, 2 removes,
    /// anything else collapses the selection to this note alone.
    pub fn select_roll_note(&mut self, id: NoteId, mode: i32) -> bool {
        let (channel, pattern) = self.roll();
        if !self.channels[channel].notes[pattern]
            .iter()
            .any(|note| note.id == id)
        {
            return false;
        }
        match mode {
            1 => self.toggle_note_selection(id),
            2 => self.remove_note_from_selection(id),
            _ => self.select_note(Some(id)),
        }
        true
    }

    /// Copies the selection over itself and moves the selection to the copies.
    ///
    /// They land exactly on the originals, so the drag that triggered this
    /// continues on the duplicate with no visible jump. The first half of the
    /// answer is the copy of the note that was grabbed, so the drag knows what
    /// it is now holding -- `None` when the grabbed note was not among the
    /// originals, which is not a reason to withhold the copies that were made.
    pub fn duplicate_selection(
        &mut self,
        anchor_id: NoteId,
    ) -> Option<(Option<NoteId>, NoteEdit)> {
        let (channel, pattern) = self.roll();
        let originals: Vec<NoteEvent> = self.channels[channel].notes[pattern]
            .iter()
            .copied()
            .filter(|note| note.id == anchor_id || self.selected_note_ids.contains(&note.id))
            .collect();
        if originals.is_empty() {
            return None;
        }
        let mut copies = Vec::with_capacity(originals.len());
        let mut anchor_copy = None;
        for original in originals {
            let id = self.next_id();
            let copy = NoteEvent { id, ..original };
            if original.id == anchor_id {
                anchor_copy = Some(id);
            }
            self.channels[channel].notes[pattern].push(copy);
            copies.push(copy);
        }
        self.sort_notes();
        self.selected_note_ids = copies.iter().map(|note| note.id).collect();
        self.selected_note_id = (copies.len() == 1).then(|| copies[0].id);
        let edit = NoteEdit {
            notes: copies.len(),
            commands: copies.iter().map(|note| self.upsert(*note)).collect(),
            cells: None,
        };
        Some((anchor_copy, edit))
    }

    /// Resizes the selection so the grabbed note ends up `duration` long.
    ///
    /// Every other selected note changes by the same amount, so a chord keeps
    /// its rhythm; clamped by the group for the same reason a move is.
    pub fn resize_selection(&mut self, anchor_id: NoteId, duration: i32) -> Option<NoteEdit> {
        let (channel, pattern) = self.roll();
        let length_ticks = self.pattern_ticks();
        let anchor = self.channels[channel].notes[pattern]
            .iter()
            .copied()
            .find(|note| note.id == anchor_id)?;
        let resizing = self.selection_including(anchor.id);
        let wanted = duration.max(1) as i64 - anchor.duration_ticks as i64;

        let mut floor = i64::MIN;
        let mut ceiling = i64::MAX;
        for note in self.channels[channel].notes[pattern]
            .iter()
            .filter(|note| resizing.contains(&note.id))
        {
            floor = floor.max(1 - note.duration_ticks as i64);
            ceiling = ceiling.min(
                length_ticks.saturating_sub(note.start_tick).max(1) as i64
                    - note.duration_ticks as i64,
            );
        }
        if floor == i64::MIN {
            return None;
        }
        let delta = wanted.clamp(floor, ceiling.max(floor));

        let mut edited = Vec::with_capacity(resizing.len());
        for note in self.channels[channel].notes[pattern]
            .iter_mut()
            .filter(|note| resizing.contains(&note.id))
        {
            note.duration_ticks = (note.duration_ticks as i64 + delta).max(1) as u32;
            edited.push(*note);
        }
        if edited.len() == 1 {
            self.select_note(Some(edited[0].id));
        }
        Some(NoteEdit {
            notes: edited.len(),
            cells: Some(
                edited
                    .iter()
                    .map(|note| (note.start_tick / TICKS_PER_STEP) as usize)
                    .collect(),
            ),
            commands: edited.iter().map(|note| self.upsert(*note)).collect(),
        })
    }

    /// Drags the selection's leading edge.
    ///
    /// Each note's end tick is what stays put, so the start may travel until
    /// it would reach it.
    pub fn resize_selection_start(
        &mut self,
        anchor_id: NoteId,
        start_tick: i32,
    ) -> Option<NoteEdit> {
        let (channel, pattern) = self.roll();
        let anchor = self.channels[channel].notes[pattern]
            .iter()
            .copied()
            .find(|note| note.id == anchor_id)?;
        let resizing = self.selection_including(anchor.id);
        let wanted = start_tick.max(0) as i64 - anchor.start_tick as i64;

        let mut floor = i64::MIN;
        let mut ceiling = i64::MAX;
        for note in self.channels[channel].notes[pattern]
            .iter()
            .filter(|note| resizing.contains(&note.id))
        {
            floor = floor.max(-(note.start_tick as i64));
            ceiling = ceiling.min(note.duration_ticks as i64 - 1);
        }
        if floor == i64::MIN {
            return None;
        }
        let delta = wanted.clamp(floor, ceiling.max(floor));

        let mut edited = Vec::with_capacity(resizing.len());
        let mut cells = Vec::with_capacity(resizing.len() * 2);
        for note in self.channels[channel].notes[pattern]
            .iter_mut()
            .filter(|note| resizing.contains(&note.id))
        {
            cells.push((note.start_tick / TICKS_PER_STEP) as usize);
            note.start_tick = (note.start_tick as i64 + delta).max(0) as u32;
            note.duration_ticks = (note.duration_ticks as i64 - delta).max(1) as u32;
            cells.push((note.start_tick / TICKS_PER_STEP) as usize);
            edited.push(*note);
        }
        self.sort_notes();
        if edited.len() == 1 {
            self.select_note(Some(edited[0].id));
        }
        Some(NoteEdit {
            notes: edited.len(),
            commands: edited.iter().map(|note| self.upsert(*note)).collect(),
            cells: Some(cells),
        })
    }

    /// Opens a marquee band in `mode`.
    ///
    /// The band updates live, so "add to the selection" has to mean "add to
    /// what was selected when the drag started". Recomputing from the live
    /// selection each frame would make the band's own previous frame part of
    /// its base, and the selection would only ever grow.
    pub fn begin_marquee(&mut self, mode: i32) {
        self.marquee_base = Some((mode, self.selected_note_ids.clone()));
    }

    /// Applies the band's current rectangle to the selection.
    pub fn update_marquee(
        &mut self,
        start_tick: i32,
        end_tick: i32,
        low_note: i32,
        high_note: i32,
    ) -> bool {
        let Some((mode, base)) = self.marquee_base.clone() else {
            return false;
        };
        let (channel, pattern) = self.roll();
        let (start_tick, end_tick) = (start_tick.min(end_tick), start_tick.max(end_tick));
        let (low_note, high_note) = (low_note.min(high_note), low_note.max(high_note));
        let caught: HashSet<NoteId> = self.channels[channel].notes[pattern]
            .iter()
            .filter(|note| {
                // Overlap, not containment: clipping a long note's tail
                // catches it, which is what every other editor does and what
                // a band drawn across a bar of held chords has to do to be
                // useful.
                note.start_tick as i32 <= end_tick
                    && note.end_tick() as i32 > start_tick
                    && note.note as i32 >= low_note
                    && note.note as i32 <= high_note
            })
            .map(|note| note.id)
            .collect();
        self.selected_note_ids = match mode {
            1 => base.union(&caught).copied().collect(),
            2 => base.difference(&caught).copied().collect(),
            _ => caught,
        };
        self.selected_note_id = (self.selected_note_ids.len() == 1)
            .then(|| *self.selected_note_ids.iter().next().expect("length is one"));
        true
    }

    /// Cuts one note in two at `tick`.
    ///
    /// Both halves end up selected, so the next gesture can act on the whole
    /// of what used to be one note. A cut at either end is refused: the grid
    /// guards this too, but it would silently delete half a note, which is
    /// worth not trusting one caller with.
    pub fn slice_note(&mut self, id: NoteId, tick: i32) -> Option<NoteEdit> {
        let (channel, pattern) = self.roll();
        let original = self.channels[channel].notes[pattern]
            .iter()
            .copied()
            .find(|note| note.id == id)?;
        let cut = tick.max(0) as u32;
        if cut <= original.start_tick || cut >= original.end_tick() {
            return None;
        }
        let tail = NoteEvent {
            id: self.next_id(),
            start_tick: cut,
            duration_ticks: original.end_tick() - cut,
            ..original
        };
        let mut head = original;
        head.duration_ticks = cut - original.start_tick;
        for note in self.channels[channel].notes[pattern].iter_mut() {
            if note.id == head.id {
                *note = head;
            }
        }
        self.channels[channel].notes[pattern].push(tail);
        self.sort_notes();
        self.selected_note_ids = HashSet::from([head.id, tail.id]);
        self.selected_note_id = None;
        Some(NoteEdit {
            notes: 2,
            commands: [head, tail].map(|note| self.upsert(note)).into(),
            cells: None,
        })
    }

    /// Merges runs of selected notes that share a pitch.
    ///
    /// Per pitch row, not across the whole selection: joining a chord into one
    /// note would throw away every pitch but one. The earliest note of each
    /// run survives, so the join keeps a stable id and the velocity the phrase
    /// started on.
    pub fn join_selection(&mut self) -> Option<NoteEdit> {
        let (channel, pattern) = self.roll();
        let selected: Vec<NoteEvent> = self.channels[channel].notes[pattern]
            .iter()
            .copied()
            .filter(|note| self.selected_note_ids.contains(&note.id))
            .collect();
        if selected.len() < 2 {
            return None;
        }
        let mut rows: BTreeMap<u8, Vec<NoteEvent>> = BTreeMap::new();
        for note in selected {
            rows.entry(note.note).or_default().push(note);
        }
        let mut kept = Vec::new();
        let mut removed = Vec::new();
        for (_, mut row) in rows {
            if row.len() < 2 {
                kept.extend(row.iter().map(|note| note.id));
                continue;
            }
            row.sort_by_key(|note| (note.start_tick, note.id));
            let end = row.iter().map(|note| note.end_tick()).max().unwrap_or(0);
            let mut merged = row[0];
            merged.duration_ticks = end.saturating_sub(merged.start_tick).max(1);
            for note in self.channels[channel].notes[pattern].iter_mut() {
                if note.id == merged.id {
                    *note = merged;
                }
            }
            kept.push(merged.id);
            removed.extend(row[1..].iter().map(|note| note.id));
        }
        if removed.is_empty() {
            return None;
        }
        self.channels[channel].notes[pattern].retain(|note| !removed.contains(&note.id));
        let edited: Vec<NoteEvent> = self.channels[channel].notes[pattern]
            .iter()
            .copied()
            .filter(|note| kept.contains(&note.id))
            .collect();
        self.prune_note_selection(&removed);
        let mut commands: Vec<EngineCommand> =
            removed.iter().map(|id| self.remove(*id)).collect();
        commands.extend(edited.iter().map(|note| self.upsert(*note)));
        Some(NoteEdit {
            notes: edited.len(),
            commands,
            cells: None,
        })
    }

    /// Opens a scale drag from `from_left`, or clears any base when there is
    /// not enough selected to scale.
    ///
    /// The anchor is the edge the drag is *not* moving, so that edge stays put
    /// and only the span changes.
    pub fn begin_scale(&mut self, from_left: bool) {
        let (channel, pattern) = self.roll();
        let notes: Vec<(NoteId, u32, u32)> = self.channels[channel].notes[pattern]
            .iter()
            .filter(|note| self.selected_note_ids.contains(&note.id))
            .map(|note| (note.id, note.start_tick, note.duration_ticks))
            .collect();
        if notes.len() < 2 {
            self.scale_base = None;
            return;
        }
        let anchor = if from_left {
            notes
                .iter()
                .map(|(_, start, duration)| start + duration)
                .max()
                .unwrap_or(0)
        } else {
            notes.iter().map(|(_, start, _)| *start).min().unwrap_or(0)
        };
        self.scale_base = Some(ScaleBase { anchor, notes });
    }

    /// Scales the selection about the drag's anchor.
    ///
    /// Applied to the geometry the drag started from rather than to the live
    /// notes, so repeated frames do not compound their own rounding. Lengths
    /// scale with the span, which is the point: double a selection's width and
    /// an eighth becomes a quarter.
    pub fn scale_selection(&mut self, factor: f32) -> Option<NoteEdit> {
        let base = self.scale_base.take()?;
        let (channel, pattern) = self.roll();
        let length_ticks = self.pattern_ticks();
        let last_start = length_ticks.saturating_sub(1);
        let factor = factor.clamp(0.02, 64.0) as f64;
        let anchor = base.anchor as f64;

        let mut edited = Vec::with_capacity(base.notes.len());
        let mut cells = Vec::with_capacity(base.notes.len() * 2);
        for (id, start, duration) in &base.notes {
            let Some(note) = self.channels[channel].notes[pattern]
                .iter_mut()
                .find(|note| note.id == *id)
            else {
                continue;
            };
            cells.push((note.start_tick / TICKS_PER_STEP) as usize);
            let scaled_start = anchor + (*start as f64 - anchor) * factor;
            let scaled_duration = *duration as f64 * factor;
            note.start_tick = (scaled_start.round().max(0.0) as u32).min(last_start);
            note.duration_ticks = (scaled_duration.round().max(1.0) as u32)
                .min(length_ticks.saturating_sub(note.start_tick).max(1));
            cells.push((note.start_tick / TICKS_PER_STEP) as usize);
            edited.push(*note);
        }
        self.sort_notes();
        self.scale_base = Some(base);
        Some(NoteEdit {
            notes: edited.len(),
            commands: edited.iter().map(|note| self.upsert(*note)).collect(),
            cells: Some(cells),
        })
    }

    /// Deletes one note.
    pub fn remove_roll_note(&mut self, id: NoteId) -> Option<NoteEdit> {
        let (channel, pattern) = self.roll();
        let index = self.channels[channel].notes[pattern]
            .iter()
            .position(|note| note.id == id)?;
        let removed = self.channels[channel].notes[pattern].remove(index);
        self.prune_note_selection(&[removed.id]);
        Some(NoteEdit {
            notes: 1,
            commands: vec![self.remove(removed.id)],
            cells: Some(vec![(removed.start_tick / TICKS_PER_STEP) as usize]),
        })
    }

    /// Sets one note's velocity, from the control's 0..1 travel.
    pub fn set_note_velocity(&mut self, id: NoteId, value: f32) -> Option<NoteEdit> {
        let (channel, pattern) = self.roll();
        let velocity = (1.0 + value.clamp(0.0, 1.0) * 126.0).round() as u8;
        let note = self.channels[channel].notes[pattern]
            .iter_mut()
            .find(|note| note.id == id)?;
        note.velocity = velocity;
        let edited = *note;
        self.select_note(Some(edited.id));
        Some(NoteEdit {
            notes: 1,
            commands: vec![self.upsert(edited)],
            cells: Some(vec![(edited.start_tick / TICKS_PER_STEP) as usize]),
        })
    }

    /// Gives every selected note the same length, trimmed so none overhangs
    /// what the pattern can hold.
    pub fn set_selection_duration(&mut self, ticks: u32) -> Option<NoteEdit> {
        let (channel, pattern) = self.roll();
        let length_ticks = self.pattern_ticks();
        let selected = self.selected_note_ids.clone();
        let mut edited = Vec::new();
        for note in self.channels[channel].notes[pattern]
            .iter_mut()
            .filter(|note| selected.contains(&note.id))
        {
            note.duration_ticks = ticks.min(length_ticks.saturating_sub(note.start_tick).max(1));
            edited.push(*note);
        }
        if edited.is_empty() {
            return None;
        }
        Some(NoteEdit {
            notes: edited.len(),
            cells: Some(
                edited
                    .iter()
                    .map(|note| (note.start_tick / TICKS_PER_STEP) as usize)
                    .collect(),
            ),
            commands: edited.iter().map(|note| self.upsert(*note)).collect(),
        })
    }

    /// The selection as a clipboard phrase.
    ///
    /// Stored relative to the earliest note, so a paste is a phrase that can
    /// land anywhere rather than a set of absolute positions that only fit
    /// where they came from.
    pub fn selection_phrase(&self) -> Vec<NoteEvent> {
        let (channel, pattern) = self.roll();
        let mut copied: Vec<NoteEvent> = self.channels[channel].notes[pattern]
            .iter()
            .copied()
            .filter(|note| self.selected_note_ids.contains(&note.id))
            .collect();
        let origin = copied.iter().map(|note| note.start_tick).min().unwrap_or(0);
        for note in &mut copied {
            note.start_tick -= origin;
        }
        copied.sort_by_key(|note| (note.start_tick, note.note));
        copied
    }

    /// Deletes every selected note.
    pub fn delete_selection(&mut self) -> Option<NoteEdit> {
        let (channel, pattern) = self.roll();
        let ids: Vec<NoteId> = self.selected_note_ids.iter().copied().collect();
        if ids.is_empty() {
            return None;
        }
        self.channels[channel].notes[pattern].retain(|note| !ids.contains(&note.id));
        self.prune_note_selection(&ids);
        Some(NoteEdit {
            notes: ids.len(),
            commands: ids.iter().map(|id| self.remove(*id)).collect(),
            cells: None,
        })
    }

    /// Pastes a phrase after whatever is selected.
    ///
    /// Or at the top of the pattern when nothing is: pasting on top of the
    /// originals looks like nothing happened. Notes that would land past the
    /// end are dropped, and `None` means none of them fit.
    pub fn paste_phrase(&mut self, phrase: &[NoteEvent]) -> Option<NoteEdit> {
        let (channel, pattern) = self.roll();
        let length_ticks = self.pattern_ticks();
        let origin = self.channels[channel].notes[pattern]
            .iter()
            .filter(|note| self.selected_note_ids.contains(&note.id))
            .map(|note| note.end_tick())
            .max()
            .unwrap_or(0);
        let mut pasted = Vec::with_capacity(phrase.len());
        for note in phrase {
            let start = origin.saturating_add(note.start_tick);
            if start >= length_ticks {
                continue;
            }
            let mut copy = NoteEvent {
                id: self.next_id(),
                ..*note
            };
            copy.start_tick = start;
            copy.duration_ticks = copy
                .duration_ticks
                .min(length_ticks.saturating_sub(start).max(1));
            self.channels[channel].notes[pattern].push(copy);
            pasted.push(copy);
        }
        if pasted.is_empty() {
            return None;
        }
        self.sort_notes();
        // Select what was pasted, so it can be moved straight away.
        self.selected_note_ids = pasted.iter().map(|note| note.id).collect();
        self.selected_note_id = (pasted.len() == 1).then(|| pasted[0].id);
        Some(NoteEdit {
            notes: pasted.len(),
            commands: pasted.iter().map(|note| self.upsert(*note)).collect(),
            cells: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pattern with a C major triad at tick 0 and a single note a bar in.
    fn chord_session() -> (Session, Vec<NoteId>) {
        let mut session = Session::default();
        let ids = [60u8, 64, 67]
            .into_iter()
            .map(|pitch| {
                session.channels[0]
                    .create_note(0, 0, TICKS_PER_STEP, pitch)
                    .id
            })
            .collect::<Vec<_>>();
        session.selected_note_ids = ids.iter().copied().collect();
        (session, ids)
    }

    /// The whole reason the deltas are group-clamped: a chord dragged into the
    /// edge of the pitch range has to stop as a chord, not pile onto one note.
    #[test]
    fn a_chord_dragged_past_the_top_keeps_its_shape() {
        let (mut session, ids) = chord_session();

        session
            .move_selection(ids[0], 0, 127)
            .expect("the anchor is in the pattern");

        let pitches: Vec<u8> = session.channels[0].notes[0]
            .iter()
            .map(|note| note.note)
            .collect();
        assert_eq!(
            pitches,
            vec![120, 124, 127],
            "the chord flattened onto the top note instead of stopping as one"
        );
    }

    /// The copies are made whether or not the grabbed note was among them.
    /// Withholding them because the anchor was stale would leave the session
    /// holding notes the engine had never been told about.
    #[test]
    fn duplicating_reports_the_copies_even_when_the_anchor_is_stale() {
        let (mut session, ids) = chord_session();

        let (anchor_copy, edit) = session
            .duplicate_selection(ids[0])
            .expect("the selection is not empty");
        assert!(anchor_copy.is_some());
        assert_eq!(edit.notes, 3);
        assert_eq!(session.channels[0].notes[0].len(), 6);

        // An id that is not in the pattern at all, with a live selection.
        let (anchor_copy, edit) = session
            .duplicate_selection(9_999)
            .expect("the selection is still not empty");
        assert_eq!(anchor_copy, None);
        assert_eq!(
            edit.notes, 3,
            "the copies were withheld because the anchor was stale"
        );
        assert_eq!(session.channels[0].notes[0].len(), 9);
        assert_eq!(edit.commands.len(), 3);
    }

    /// Grabbing a note outside the selection acts on that note alone.
    #[test]
    fn dragging_an_unselected_note_leaves_the_selection_alone() {
        let (mut session, ids) = chord_session();
        let loner = session.channels[0]
            .create_note(0, 4 * TICKS_PER_STEP, TICKS_PER_STEP, 72)
            .id;

        assert_eq!(session.selection_including(loner), HashSet::from([loner]));
        assert_eq!(session.selection_including(ids[0]).len(), 3);
    }

    /// A cut at either end would silently delete half a note.
    #[test]
    fn a_slice_at_either_end_is_refused() {
        let mut session = Session::default();
        let note = session.channels[0].create_note(0, 0, TICKS_PER_STEP, 60);

        assert!(session.slice_note(note.id, 0).is_none());
        assert!(session
            .slice_note(note.id, TICKS_PER_STEP as i32)
            .is_none());
        assert_eq!(session.channels[0].notes[0].len(), 1);

        let edit = session
            .slice_note(note.id, (TICKS_PER_STEP / 2) as i32)
            .expect("a cut in the middle is legal");
        assert_eq!(edit.notes, 2);
        let durations: Vec<u32> = session.channels[0].notes[0]
            .iter()
            .map(|note| note.duration_ticks)
            .collect();
        assert_eq!(durations, vec![TICKS_PER_STEP / 2, TICKS_PER_STEP / 2]);
        assert_eq!(session.selected_note_ids.len(), 2);
    }

    /// Joining a chord into one note would throw away every pitch but one.
    #[test]
    fn joining_merges_per_pitch_row_rather_than_across_the_chord() {
        let (mut session, _) = chord_session();
        // A second note on the middle pitch, so exactly one row has a run.
        let tail = session.channels[0]
            .create_note(0, TICKS_PER_STEP, TICKS_PER_STEP, 64)
            .id;
        session.selected_note_ids.insert(tail);

        session.join_selection().expect("one row has a run");

        let mut rows: Vec<(u8, u32)> = session.channels[0].notes[0]
            .iter()
            .map(|note| (note.note, note.duration_ticks))
            .collect();
        rows.sort();
        assert_eq!(
            rows,
            vec![
                (60, TICKS_PER_STEP),
                (64, 2 * TICKS_PER_STEP),
                (67, TICKS_PER_STEP)
            ],
            "the chord lost pitches, or the run did not merge"
        );
    }

    /// The band's base is the selection the drag started from, not the live
    /// one -- otherwise a subtractive band could never shrink anything.
    #[test]
    fn a_subtractive_marquee_measures_against_where_it_started() {
        let (mut session, ids) = chord_session();

        session.begin_marquee(2);
        assert!(session.update_marquee(0, TICKS_PER_STEP as i32, 0, 127));
        assert!(
            session.selected_note_ids.is_empty(),
            "a subtractive band over everything left something selected"
        );

        // Re-running the same frame must give the same answer, not compound.
        assert!(session.update_marquee(0, TICKS_PER_STEP as i32, 0, 127));
        assert!(session.selected_note_ids.is_empty());

        session.begin_marquee(1);
        assert!(session.update_marquee(0, TICKS_PER_STEP as i32, 0, 65));
        assert_eq!(
            session.selected_note_ids,
            [ids[0], ids[1]].into_iter().collect()
        );
    }

    /// Copy stores the phrase relative to its earliest note, and paste lands
    /// it after the selection rather than on top of it.
    #[test]
    fn a_phrase_pastes_after_the_selection_rather_than_over_it() {
        let mut session = Session::default();
        let first = session.channels[0]
            .create_note(0, 2 * TICKS_PER_STEP, TICKS_PER_STEP, 60)
            .id;
        let second = session.channels[0]
            .create_note(0, 3 * TICKS_PER_STEP, TICKS_PER_STEP, 62)
            .id;
        session.selected_note_ids = [first, second].into_iter().collect();

        let phrase = session.selection_phrase();
        assert_eq!(
            phrase.iter().map(|n| n.start_tick).collect::<Vec<_>>(),
            vec![0, TICKS_PER_STEP],
            "the phrase kept absolute positions instead of relative ones"
        );

        let edit = session.paste_phrase(&phrase).expect("the phrase fits");
        assert_eq!(edit.notes, 2);
        let starts: Vec<u32> = session.channels[0].notes[0]
            .iter()
            .map(|note| note.start_tick)
            .collect();
        assert_eq!(
            starts,
            vec![
                2 * TICKS_PER_STEP,
                3 * TICKS_PER_STEP,
                4 * TICKS_PER_STEP,
                5 * TICKS_PER_STEP
            ]
        );
        // The paste is selected, so a following drag moves what just landed.
        assert_eq!(session.selected_note_ids.len(), 2);
        assert!(!session.selected_note_ids.contains(&first));
    }

    /// Nothing fits past the end of the pattern, and saying so is not the same
    /// as pasting nothing.
    #[test]
    fn a_phrase_that_does_not_fit_pastes_nothing() {
        let mut session = Session::default();
        let last = session.pattern_lengths[0] as u32 - 1;
        let note = session.channels[0]
            .create_note(0, last * TICKS_PER_STEP, TICKS_PER_STEP, 60)
            .id;
        session.selected_note_ids = [note].into_iter().collect();

        let phrase = session.selection_phrase();
        assert!(session.paste_phrase(&phrase).is_none());
        assert_eq!(session.channels[0].notes[0].len(), 1);
    }

    /// Repeated scale frames apply to the geometry the drag started from, so
    /// scaling out and back returns the notes to where they were.
    #[test]
    fn scaling_applies_to_the_drags_own_starting_geometry() {
        let (mut session, _) = chord_session();
        session.channels[0].create_note(0, 2 * TICKS_PER_STEP, TICKS_PER_STEP, 60);
        session.selected_note_ids = session.channels[0].notes[0]
            .iter()
            .map(|note| note.id)
            .collect();

        session.begin_scale(false);
        let before: Vec<u32> = session.channels[0].notes[0]
            .iter()
            .map(|note| note.start_tick)
            .collect();

        session.scale_selection(2.0).expect("a scale is in flight");
        session.scale_selection(1.0).expect("still in flight");

        let after: Vec<u32> = session.channels[0].notes[0]
            .iter()
            .map(|note| note.start_tick)
            .collect();
        assert_eq!(after, before, "the scale compounded its own rounding");
    }

    /// Fewer than two notes is not a span, so there is nothing to scale.
    #[test]
    fn a_scale_needs_something_to_scale() {
        let mut session = Session::default();
        let note = session.channels[0].create_note(0, 0, TICKS_PER_STEP, 60);
        session.select_note(Some(note.id));

        session.begin_scale(true);
        assert!(session.scale_base.is_none());
        assert!(session.scale_selection(2.0).is_none());
    }
}
