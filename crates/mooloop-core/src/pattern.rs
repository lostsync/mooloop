//! Tick-addressed pattern data. Pure model; owns no audio or UI dependencies.
//!
//! Pattern length is still presented as sixteenth-note rack cells, while each
//! channel stores independent note events at PPQ tick precision. This keeps
//! the compact rack useful without making it the authoritative note model.

use crate::{AutomationLane, LanePool, ParamAddr, MAX_AUTOMATION_LANES_PER_CHANNEL};

/// Default number of sixteenth-note cells per pattern (one 4/4 bar).
pub const DEFAULT_STEPS: u16 = 16;

/// Maximum pattern length in sixteenth-note cells.
pub const MAX_PATTERN_STEPS: u16 = 256;

/// Sixteenth-note cells to a quarter-note beat. A pattern length is chosen in
/// beats and bars far more often than in cells, so the controls that change
/// one move by this rather than by one.
pub const STEPS_PER_BEAT: u16 = 4;

/// PPQ 96 has 24 ticks per sixteenth and 6 ticks per sixty-fourth.
pub const TICKS_PER_STEP: u32 = 24;
pub const TICKS_PER_64TH: u32 = 6;
pub const DEFAULT_NOTE_DURATION_TICKS: u32 = TICKS_PER_STEP;

/// Four sixty-fourth-note starts per rack cell. Storage is reserved up front
/// so note edits on the audio thread never allocate.
pub const MAX_NOTES_PER_CHANNEL_PATTERN: usize = MAX_PATTERN_STEPS as usize * 4;

pub type NoteId = u32;

/// Temporary rack/UI compatibility cell. It is a view/edit affordance, not
/// canonical pattern storage; new engine code should use [`NoteEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub on: bool,
    pub note: u8,
    pub velocity: u8,
}

impl Default for Step {
    fn default() -> Self {
        Self {
            on: false,
            note: 60,
            velocity: 100,
        }
    }
}

impl Step {
    pub fn toggled(self) -> Self {
        Self {
            on: !self.on,
            ..self
        }
    }
}

/// A pitched note on the pattern timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NoteEvent {
    pub id: NoteId,
    pub start_tick: u32,
    pub duration_ticks: u32,
    /// MIDI note number. The sampler interprets this relative to its root.
    pub note: u8,
    pub velocity: u8,
}

impl NoteEvent {
    pub fn new(id: NoteId, start_tick: u32, duration_ticks: u32, note: u8, velocity: u8) -> Self {
        Self {
            id,
            start_tick,
            duration_ticks: duration_ticks.max(1),
            note: note.min(127),
            velocity: velocity.clamp(1, 127),
        }
    }

    pub fn end_tick(self) -> u32 {
        self.start_tick.saturating_add(self.duration_ticks)
    }
}

/// One channel's notes and automation inside a pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelPattern {
    notes: Vec<NoteEvent>,
    /// `(end_tick, index into notes)`, sorted by end tick. `notes` is sorted
    /// by `(start_tick, id)` for the note-on walk; this is the same storage
    /// read the other way, for the note-off walk, so the scheduler can
    /// binary-search either edge instead of scanning every note for both
    /// (`reports/fable-2026-09-22.md`, finding 1). Kept in lockstep by every
    /// site that touches `notes`, and by the same rule: a sorted insert, no
    /// heap allocation on the hot path (the `binary_search_by_key` below is
    /// the same one `upsert_note` already used).
    ///
    /// A note's raw `end_tick` is `start_tick + duration_ticks`, which
    /// nothing here caps, so it is stored as the real value rather than one
    /// folded into the pattern's current length -- the same reason `notes`
    /// still holds a note whose `start_tick` has drifted past the pattern
    /// after a shorten. `index` is a `u16` because
    /// [`MAX_NOTES_PER_CHANNEL_PATTERN`] fits comfortably under 2^16.
    offs: Vec<(u32, u16)>,
    /// A fixed bank of [`MAX_AUTOMATION_LANES_PER_CHANNEL`] lane slots whose
    /// open ones are the prefix `..open_lanes`, at most one per destination.
    /// Order inside the prefix is the order they were opened, which is the
    /// order the lane picker lists them in, and closing one shifts the rest
    /// down rather than leaving a hole -- a move of eight words, against the
    /// 12 KB free that removing the lane outright used to cost the callback.
    lanes: Vec<AutomationLane>,
    open_lanes: usize,
    capacity_ticks: u32,
}

impl ChannelPattern {
    pub fn new(num_steps: usize) -> Self {
        let note_capacity = num_steps
            .saturating_mul(4)
            .min(MAX_NOTES_PER_CHANNEL_PATTERN);
        Self {
            notes: Vec::with_capacity(note_capacity),
            offs: Vec::with_capacity(note_capacity),
            // Eight vacant slots own no point storage, so this is the same
            // heap the empty `Vec::with_capacity(8)` here used to take.
            lanes: (0..MAX_AUTOMATION_LANES_PER_CHANNEL)
                .map(|_| AutomationLane::vacant())
                .collect(),
            open_lanes: 0,
            capacity_ticks: (num_steps as u32).saturating_mul(TICKS_PER_STEP),
        }
    }

    pub fn notes(&self) -> &[NoteEvent] {
        &self.notes
    }

    pub fn note(&self, id: NoteId) -> Option<&NoteEvent> {
        self.notes.iter().find(|note| note.id == id)
    }

    /// Notes whose `start_tick` falls in `[range.start, range.end)`, in
    /// storage (start-tick) order. A direct slice of `notes`, since that is
    /// exactly what it is already sorted by.
    pub fn notes_starting_in(&self, range: std::ops::Range<u32>) -> &[NoteEvent] {
        if range.start >= range.end {
            return &[];
        }
        let lo = self.notes.partition_point(|note| note.start_tick < range.start);
        let hi = self.notes.partition_point(|note| note.start_tick < range.end);
        &self.notes[lo..hi]
    }

    /// Notes whose `end_tick()` falls in `[range.start, range.end)`, in
    /// end-tick order (not `notes`' own timeline order -- a caller after a
    /// specific note-on/note-off ordering sorts what this returns itself).
    pub fn notes_ending_in(
        &self,
        range: std::ops::Range<u32>,
    ) -> impl Iterator<Item = &NoteEvent> + '_ {
        let (lo, hi) = if range.start >= range.end {
            (0, 0)
        } else {
            let lo = self.offs.partition_point(|&(end, _)| end < range.start);
            let hi = self.offs.partition_point(|&(end, _)| end < range.end);
            (lo, hi)
        };
        self.offs[lo..hi]
            .iter()
            .map(move |&(_, index)| &self.notes[index as usize])
    }

    /// Indices into `notes()` whose `end_tick()` falls in
    /// `[range.start, range.end)`. The same query as
    /// [`Self::notes_ending_in`], but the position rather than the note
    /// itself -- a caller that needs the notes back in `notes()`'s own
    /// `(start_tick, id)` order (not end-tick order) marks these positions
    /// and re-walks `notes()` instead of sorting what this returns.
    pub fn note_indices_ending_in(
        &self,
        range: std::ops::Range<u32>,
    ) -> impl Iterator<Item = usize> + '_ {
        let (lo, hi) = if range.start >= range.end {
            (0, 0)
        } else {
            let lo = self.offs.partition_point(|&(end, _)| end < range.start);
            let hi = self.offs.partition_point(|&(end, _)| end < range.end);
            (lo, hi)
        };
        self.offs[lo..hi].iter().map(|&(_, index)| index as usize)
    }

    /// The largest `end_tick()` currently stored, or `None` when empty.
    /// Cheap (the last element of a sorted store) and the scheduler's only
    /// way to notice a note that sustains across more than one pattern or
    /// song pass, which the `offs` shift-by-one-period search does not
    /// cover -- see `sequencer.rs`'s note-off fallback.
    pub fn max_end_tick(&self) -> Option<u32> {
        self.offs.last().map(|&(end, _)| end)
    }

    /// Insert or replace a note while preserving timeline order. Returns false
    /// when the start is outside storage or the preallocated capacity is full.
    pub fn upsert_note(&mut self, note: NoteEvent) -> bool {
        if note.start_tick >= self.capacity_ticks {
            return false;
        }

        if let Some(index) = self
            .notes
            .iter()
            .position(|existing| existing.id == note.id)
        {
            self.remove_off_entry(index);
            self.notes.remove(index);
            self.shift_offs_after_removal(index);
        } else if self.notes.len() == self.notes.capacity() {
            return false;
        }

        let index = self
            .notes
            .binary_search_by_key(&(note.start_tick, note.id), |existing| {
                (existing.start_tick, existing.id)
            })
            .unwrap_or_else(|index| index);
        self.notes.insert(index, note);
        self.shift_offs_after_insertion(index);
        self.insert_off_entry(note.end_tick(), index);
        true
    }

    pub fn remove_note(&mut self, id: NoteId) -> Option<NoteEvent> {
        let index = self.notes.iter().position(|note| note.id == id)?;
        self.remove_off_entry(index);
        let removed = self.notes.remove(index);
        self.shift_offs_after_removal(index);
        Some(removed)
    }

    /// Remove the `offs` entry for the note currently at `notes[index]`,
    /// before that note itself is removed. A linear scan among the ties at
    /// its end tick, matching `upsert_note`/`remove_note`'s own `position`
    /// lookup on `notes` -- O(n), no allocation, and not the per-block path.
    fn remove_off_entry(&mut self, index: usize) {
        let end = self.notes[index].end_tick();
        let start = self.offs.partition_point(|&(candidate, _)| candidate < end);
        let offset = self.offs[start..]
            .iter()
            .position(|&(candidate, at)| candidate == end && at as usize == index)
            .expect("every stored note has a matching offs entry");
        self.offs.remove(start + offset);
    }

    /// Every `offs` entry pointing past a just-removed `notes[index]` now
    /// points one too far; bring it back in line.
    fn shift_offs_after_removal(&mut self, index: usize) {
        for (_, at) in &mut self.offs {
            if *at as usize > index {
                *at -= 1;
            }
        }
    }

    /// Every `offs` entry pointing at or past a just-inserted `notes[index]`
    /// now points one short; move it up.
    fn shift_offs_after_insertion(&mut self, index: usize) {
        for (_, at) in &mut self.offs {
            if *at as usize >= index {
                *at += 1;
            }
        }
    }

    /// Insert the `(end_tick, index)` pair for a just-inserted note.
    fn insert_off_entry(&mut self, end_tick: u32, index: usize) {
        let at = self
            .offs
            .binary_search_by_key(&end_tick, |&(end, _)| end)
            .unwrap_or_else(|at| at);
        self.offs.insert(at, (end_tick, index as u16));
    }

    /// Empty the channel, keeping every slot's point storage: this runs on
    /// the audio thread (Add Channel clears the seat across the bank).
    pub fn clear(&mut self) {
        self.notes.clear();
        self.offs.clear();
        for lane in &mut self.lanes[..self.open_lanes] {
            lane.vacate();
        }
        self.open_lanes = 0;
    }

    pub fn lanes(&self) -> &[AutomationLane] {
        &self.lanes[..self.open_lanes]
    }

    pub fn lane(&self, target: ParamAddr) -> Option<&AutomationLane> {
        self.lanes().iter().find(|lane| lane.target == target)
    }

    pub fn lane_mut(&mut self, target: ParamAddr) -> Option<&mut AutomationLane> {
        self.lanes[..self.open_lanes]
            .iter_mut()
            .find(|lane| lane.target == target)
    }

    /// Open the lane for `target`, or return the existing one. `None` when all
    /// [`MAX_AUTOMATION_LANES_PER_CHANNEL`] slots are open.
    ///
    /// Runs on the audio thread and does not free. It allocates only when the
    /// slot it lands on has never held a lane *and* `pool` is empty; see
    /// [`LanePool`].
    pub fn open_lane(
        &mut self,
        target: ParamAddr,
        pool: &mut LanePool,
    ) -> Option<&mut AutomationLane> {
        if let Some(index) = self.lanes()
            .iter()
            .position(|lane| lane.target == target)
        {
            return self.lanes.get_mut(index);
        }
        if self.open_lanes == self.lanes.len() {
            return None;
        }
        let index = self.open_lanes;
        self.lanes[index].claim(target, pool);
        self.open_lanes += 1;
        self.lanes.get_mut(index)
    }

    /// Close the lane driving `target`, answering whether there was one.
    ///
    /// The slot keeps its point storage and moves to the end of the bank, so
    /// the open prefix stays in the order the picker lists it and nothing is
    /// freed on the audio thread.
    pub fn remove_lane(&mut self, target: ParamAddr) -> bool {
        let Some(index) = self.lanes().iter().position(|lane| lane.target == target) else {
            return false;
        };
        self.close_slot(index);
        true
    }

    /// Vacate slot `index` of the open prefix and shift the rest down.
    fn close_slot(&mut self, index: usize) {
        self.lanes[index].vacate();
        self.lanes[index..self.open_lanes].rotate_left(1);
        self.open_lanes -= 1;
    }

    /// Drop the lanes driving `device` in `scope`, because that device has
    /// been removed. In place and without allocating, so the engine can run
    /// it where the edit arrives.
    ///
    /// A reorder needs no equivalent: a lane names a device identity, and an
    /// identity does not move.
    pub fn forget_device(
        &mut self,
        scope: crate::EffectTarget,
        device: crate::DeviceId,
    ) -> bool {
        let before = self.open_lanes;
        let mut index = 0;
        while index < self.open_lanes {
            if crate::structure::lane_drives_device(&self.lanes[index], scope, device) {
                self.close_slot(index);
            } else {
                index += 1;
            }
        }
        self.open_lanes != before
    }

    /// Replace the whole lane set. Used by project load, which is the only
    /// caller allowed to allocate.
    pub fn set_lanes(&mut self, lanes: Vec<AutomationLane>) {
        self.open_lanes = 0;
        for (index, mut lane) in lanes
            .into_iter()
            .take(MAX_AUTOMATION_LANES_PER_CHANNEL)
            .enumerate()
        {
            // This is the caller allowed to allocate, and a lane arriving
            // from a decode or a clone has none of its preallocation left.
            lane.reserve_points();
            self.lanes[index] = lane;
            self.open_lanes = index + 1;
        }
        for lane in &mut self.lanes[self.open_lanes..] {
            lane.vacate();
        }
    }

    pub fn notes_in_step(&self, step: usize) -> impl Iterator<Item = &NoteEvent> {
        let start = (step as u32).saturating_mul(TICKS_PER_STEP);
        let end = start.saturating_add(TICKS_PER_STEP);
        self.notes
            .iter()
            .filter(move |note| note.start_tick >= start && note.start_tick < end)
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty() && self.lanes().iter().all(AutomationLane::is_empty)
    }
}

/// A pattern whose channels are indexed in registration order.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub length_steps: u16,
    pub channels: Vec<ChannelPattern>,
}

impl Pattern {
    pub fn new(num_channels: usize) -> Self {
        Self::with_steps(num_channels, DEFAULT_STEPS as usize)
    }

    pub fn with_steps(num_channels: usize, num_steps: usize) -> Self {
        let stored_steps = num_steps.min(MAX_PATTERN_STEPS as usize).max(1);
        Self {
            length_steps: stored_steps as u16,
            channels: (0..num_channels)
                .map(|_| ChannelPattern::new(stored_steps))
                .collect(),
        }
    }

    pub fn length_ticks(&self) -> u32 {
        u32::from(self.length_steps).saturating_mul(TICKS_PER_STEP)
    }

    pub fn channel(&self, index: usize) -> Option<&ChannelPattern> {
        self.channels.get(index)
    }

    pub fn channel_mut(&mut self, index: usize) -> Option<&mut ChannelPattern> {
        self.channels.get_mut(index)
    }

    /// Change logical playback length without discarding notes past the end.
    pub fn set_length_steps(&mut self, length_steps: usize) {
        let capacity = self
            .channels
            .iter()
            .map(|channel| channel.capacity_ticks / TICKS_PER_STEP)
            .min()
            .unwrap_or(1)
            .min(u16::MAX as u32)
            .max(1);
        self.length_steps = length_steps.clamp(1, capacity as usize) as u16;
    }

    pub fn count_active(&self, index: usize) -> usize {
        let length_ticks = self.length_ticks();
        self.channel(index)
            .map(|channel| {
                channel
                    .notes()
                    .iter()
                    .filter(|note| note.start_tick < length_ticks)
                    .count()
            })
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_defaults() {
        let pattern = Pattern::new(2);
        assert_eq!(pattern.length_steps, 16);
        assert_eq!(pattern.length_ticks(), 384);
        assert_eq!(pattern.channels.len(), 2);
        assert!(pattern.channel(0).unwrap().is_empty());
    }

    #[test]
    fn notes_are_sorted_and_replaced_by_stable_id() {
        let mut channel = ChannelPattern::new(16);
        assert!(channel.upsert_note(NoteEvent::new(8, 24, 12, 62, 80)));
        assert!(channel.upsert_note(NoteEvent::new(3, 6, 6, 60, 90)));
        assert_eq!(
            channel
                .notes()
                .iter()
                .map(|note| note.id)
                .collect::<Vec<_>>(),
            [3, 8]
        );

        assert!(channel.upsert_note(NoteEvent::new(8, 0, 48, 64, 100)));
        assert_eq!(
            channel
                .notes()
                .iter()
                .map(|note| note.id)
                .collect::<Vec<_>>(),
            [8, 3]
        );
        assert_eq!(channel.note(8).unwrap().duration_ticks, 48);
    }

    /// `offs` (the end-tick index) has to track `notes` exactly through
    /// every kind of edit `upsert_note`/`remove_note` do: a fresh insert, a
    /// same-id replace (which moves other notes' positions in `notes` when
    /// its start tick changes), and a removal from the middle -- each a way
    /// the positional indices `offs` stores can go stale if the shift is
    /// wrong. Checked by brute force: sort `notes` by end tick and compare.
    #[test]
    fn offs_index_tracks_notes_through_inserts_replaces_and_removals() {
        fn assert_offs_matches(channel: &ChannelPattern) {
            let mut expected: Vec<_> = channel.notes().to_vec();
            expected.sort_by_key(|note| (note.end_tick(), note.id));
            let actual: Vec<_> = channel
                .notes_ending_in(0..u32::MAX)
                .copied()
                .collect();
            let mut actual_sorted = actual.clone();
            actual_sorted.sort_by_key(|note| (note.end_tick(), note.id));
            assert_eq!(actual_sorted, expected, "offs disagrees with notes");
            // notes_ending_in must itself already be in end-tick order.
            assert!(actual
                .windows(2)
                .all(|pair| pair[0].end_tick() <= pair[1].end_tick()));
        }

        let mut channel = ChannelPattern::new(64);
        for (id, (start, dur)) in [(1, (10, 5)), (2, (0, 100)), (3, (50, 1)), (4, (20, 20))] {
            assert!(channel.upsert_note(NoteEvent::new(id, start, dur, 60, 100)));
        }
        assert_offs_matches(&channel);

        // Replace id 2 with a start tick that moves it later in `notes`,
        // shifting every note between its old and new position.
        assert!(channel.upsert_note(NoteEvent::new(2, 45, 3, 61, 90)));
        assert_offs_matches(&channel);

        // Remove from the middle.
        assert!(channel.remove_note(4).is_some());
        assert_offs_matches(&channel);

        // Remove everything, then rebuild.
        for id in [1, 2, 3] {
            channel.remove_note(id);
        }
        assert!(channel.notes().is_empty());
        assert_offs_matches(&channel);
        assert!(channel.upsert_note(NoteEvent::new(9, 5, 5, 60, 100)));
        assert_offs_matches(&channel);

        channel.clear();
        assert_offs_matches(&channel);
    }

    #[test]
    fn notes_starting_and_ending_in_bound_correctly() {
        let mut channel = ChannelPattern::new(64);
        // start=10 end=15, start=20 end=100, start=30 end=35
        assert!(channel.upsert_note(NoteEvent::new(1, 10, 5, 60, 100)));
        assert!(channel.upsert_note(NoteEvent::new(2, 20, 80, 60, 100)));
        assert!(channel.upsert_note(NoteEvent::new(3, 30, 5, 60, 100)));

        assert_eq!(
            channel
                .notes_starting_in(0..25)
                .iter()
                .map(|n| n.id)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(
            channel
                .notes_ending_in(0..40)
                .map(|n| n.id)
                .collect::<Vec<_>>(),
            [1, 3]
        );
        assert!(channel.notes_starting_in(25..25).is_empty());
        assert_eq!(channel.notes_ending_in(1000..2000).count(), 0);
        assert_eq!(channel.max_end_tick(), Some(100));
    }

    #[test]
    fn step_summary_includes_all_four_sixty_fourths() {
        let mut channel = ChannelPattern::new(16);
        for (id, tick) in [0, 6, 12, 18].into_iter().enumerate() {
            assert!(channel.upsert_note(NoteEvent::new(id as u32, tick, 6, 60, 100)));
        }
        assert_eq!(channel.notes_in_step(0).count(), 4);
        assert_eq!(channel.notes_in_step(1).count(), 0);
    }

    #[test]
    fn length_changes_are_bounded_and_non_destructive() {
        let mut pattern = Pattern::with_steps(1, 32);
        assert!(pattern.channel_mut(0).unwrap().upsert_note(NoteEvent::new(
            1,
            23 * TICKS_PER_STEP,
            TICKS_PER_STEP,
            60,
            100,
        )));

        pattern.set_length_steps(12);
        assert_eq!(pattern.length_steps, 12);
        assert_eq!(pattern.count_active(0), 0);

        pattern.set_length_steps(24);
        assert_eq!(pattern.count_active(0), 1);
        assert!(pattern.channel(0).unwrap().note(1).is_some());

        pattern.set_length_steps(0);
        assert_eq!(pattern.length_steps, 1);
        pattern.set_length_steps(usize::MAX);
        assert_eq!(pattern.length_steps, 32);
    }
}
