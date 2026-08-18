//! Tick-addressed pattern data. Pure model; owns no audio or UI dependencies.
//!
//! Pattern length is still presented as sixteenth-note rack cells, while each
//! channel stores independent note events at PPQ tick precision. This keeps
//! the compact rack useful without making it the authoritative note model.

/// Default number of sixteenth-note cells per pattern (one 4/4 bar).
pub const DEFAULT_STEPS: u16 = 16;

/// Maximum pattern length in sixteenth-note cells.
pub const MAX_PATTERN_STEPS: u16 = 256;

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

/// One channel's notes inside a pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelPattern {
    notes: Vec<NoteEvent>,
    capacity_ticks: u32,
}

impl ChannelPattern {
    pub fn new(num_steps: usize) -> Self {
        let note_capacity = num_steps
            .saturating_mul(4)
            .min(MAX_NOTES_PER_CHANNEL_PATTERN);
        Self {
            notes: Vec::with_capacity(note_capacity),
            capacity_ticks: (num_steps as u32).saturating_mul(TICKS_PER_STEP),
        }
    }

    pub fn notes(&self) -> &[NoteEvent] {
        &self.notes
    }

    pub fn note(&self, id: NoteId) -> Option<&NoteEvent> {
        self.notes.iter().find(|note| note.id == id)
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
            self.notes.remove(index);
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
        true
    }

    pub fn remove_note(&mut self, id: NoteId) -> Option<NoteEvent> {
        let index = self.notes.iter().position(|note| note.id == id)?;
        Some(self.notes.remove(index))
    }

    pub fn clear(&mut self) {
        self.notes.clear();
    }

    pub fn notes_in_step(&self, step: usize) -> impl Iterator<Item = &NoteEvent> {
        let start = (step as u32).saturating_mul(TICKS_PER_STEP);
        let end = start.saturating_add(TICKS_PER_STEP);
        self.notes
            .iter()
            .filter(move |note| note.start_tick >= start && note.start_tick < end)
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }
}

/// A pattern whose channels are indexed in registration order.
#[derive(Debug, Clone, PartialEq, Eq)]
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
