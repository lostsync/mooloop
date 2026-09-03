//! Transport, pattern and playlist edits.
//!
//! Each of these was the body of a Slint callback. They are named methods now
//! so a menu, a shortcut and a test can all reach the same edit, and so the
//! commands they produce are returned in the order the engine has to see them
//! rather than being sent from wherever the closure happened to be.

use crate::session::Session;
use mooloop_core::{
    EngineCommand, PatternPlacement, PlaybackMode, DEFAULT_STEPS, DELAY_PARAM_TIME_MS,
    MAX_PATTERNS, MAX_PATTERN_STEPS, MAX_PLAYLIST_PLACEMENTS, MAX_PLAYLIST_TICKS,
    MAX_SWING_PERCENT, MIN_SWING_PERCENT, TICKS_PER_STEP,
};

/// A pattern length that was actually applied, and the pattern it applied to.
pub struct PatternLength {
    pub pattern: usize,
    pub length: usize,
}

impl Session {
    /// Records that the document has been edited.
    ///
    /// Every mutation goes through here rather than setting the two fields
    /// itself, because the title, the close prompt and the engine's notion of
    /// which revision it is playing all read them and all have to agree.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
        self.revision = self.revision.wrapping_add(1);
    }

    /// Switches between pattern and song playback.
    pub fn set_playback_mode(&mut self, song_mode: bool) -> EngineCommand {
        self.song_mode = song_mode;
        EngineCommand::SetPlaybackMode(if song_mode {
            PlaybackMode::Song
        } else {
            PlaybackMode::Pattern
        })
    }

    /// Adopts a new tempo.
    ///
    /// The order of the returned commands is load-bearing: the transport takes
    /// the tempo first, then every synced delay receives its resolved
    /// millisecond value, so no beat-relative buffer is replaced against the
    /// old tempo.
    pub fn set_tempo(&mut self, bpm: f64) -> Vec<EngineCommand> {
        let mut commands = vec![EngineCommand::SetTempo(bpm)];
        commands.extend(self.update_tempo_synced_delay_times(bpm).into_iter().map(
            |(target, slot, time_ms)| EngineCommand::SetEffectParam {
                target,
                slot,
                id: DELAY_PARAM_TIME_MS,
                value: time_ms,
            },
        ));
        self.mark_dirty();
        commands
    }

    /// Sets the swing amount, clamped to what the sequencer accepts.
    pub fn set_swing(&mut self, percent: i32) -> EngineCommand {
        let percent = percent.clamp(MIN_SWING_PERCENT.into(), MAX_SWING_PERCENT.into()) as u8;
        self.mark_dirty();
        EngineCommand::SetSwing(percent)
    }

    /// Makes `pattern` the one being edited, returning it when it exists.
    ///
    /// The note selection goes with it: a selection is per-pattern, and
    /// carrying it across would highlight notes the roll is no longer showing.
    pub fn select_pattern(&mut self, pattern: i32) -> Option<usize> {
        let pattern = usize::try_from(pattern).ok()?;
        if pattern >= self.pattern_lengths.len() {
            return None;
        }
        self.current_pattern = pattern;
        self.select_note(None);
        Some(pattern)
    }

    /// Appends an empty pattern and selects it, or `None` if the bank is full.
    ///
    /// Patterns are created explicitly: the engine owns a fully preallocated
    /// pool and this is the active prefix of it.
    pub fn add_pattern(&mut self) -> Option<usize> {
        if self.pattern_lengths.len() >= MAX_PATTERNS {
            return None;
        }
        let pattern = self.pattern_lengths.len();
        self.pattern_lengths.push(DEFAULT_STEPS as usize);
        self.pattern_names.push(String::new());
        for channel in &mut self.channels {
            channel.notes.push(Vec::new());
            channel.automation.push(Vec::new());
        }
        self.current_pattern = pattern;
        self.select_note(None);
        Some(pattern)
    }

    /// Renames a pattern. An empty name is legal and reads as "Pattern N".
    pub fn rename_pattern(&mut self, index: usize, name: &str) -> bool {
        let Some(slot) = self.pattern_names.get_mut(index) else {
            return false;
        };
        *slot = name.trim().to_string();
        true
    }

    /// Sets the current pattern's logical length, or `None` if unchanged.
    ///
    /// Channel storage stays at the maximum, so shortening and re-extending
    /// does not discard hidden steps. What does not survive is a selection
    /// reaching past the new end, which the roll could not show.
    pub fn set_pattern_length(&mut self, length: i32) -> Option<PatternLength> {
        let length = length.clamp(1, MAX_PATTERN_STEPS as i32) as usize;
        let pattern = self.current_pattern;
        if self.pattern_lengths[pattern] == length {
            return None;
        }
        self.pattern_lengths[pattern] = length;
        let length_ticks = length as u32 * TICKS_PER_STEP;
        let notes = &self.channels[self.selected].notes[pattern];
        let out_of_range: Vec<_> = self
            .selected_note_ids
            .iter()
            .copied()
            .filter(|id| {
                notes
                    .iter()
                    .find(|note| note.id == *id)
                    .is_none_or(|note| note.start_tick >= length_ticks)
            })
            .collect();
        self.prune_note_selection(&out_of_range);
        Some(PatternLength { pattern, length })
    }

    /// Places `pattern` on the playlist at `start_tick`.
    ///
    /// Refuses silently when the tick is past the end of the arrangement, the
    /// playlist is full, or the clip would overlap another of the same
    /// pattern. Callers already snap the tick to the musical grid.
    pub fn add_playlist_placement(
        &mut self,
        pattern: i32,
        start_tick: i32,
    ) -> Option<PatternPlacement> {
        let pattern = usize::try_from(pattern).ok()?;
        if pattern >= self.pattern_lengths.len() {
            return None;
        }
        let start_tick = start_tick.max(0) as u32;
        if start_tick >= MAX_PLAYLIST_TICKS || self.playlist.len() >= MAX_PLAYLIST_PLACEMENTS {
            return None;
        }
        let span = self.pattern_lengths[pattern] as u32 * TICKS_PER_STEP;
        let end_tick = start_tick.saturating_add(span);
        let overlaps = self.playlist.iter().any(|placement| {
            placement.pattern as usize == pattern
                && start_tick < placement.start_tick.saturating_add(span)
                && placement.start_tick < end_tick
        });
        if overlaps {
            return None;
        }
        let placement = PatternPlacement::new(pattern as u8, start_tick);
        self.playlist.push(placement);
        self.playlist.sort_unstable();
        Some(placement)
    }

    /// Removes whichever clip of `pattern` covers `tick`.
    pub fn remove_playlist_placement(
        &mut self,
        pattern: i32,
        tick: i32,
    ) -> Option<PatternPlacement> {
        let pattern = usize::try_from(pattern).ok()?;
        if pattern >= self.pattern_lengths.len() {
            return None;
        }
        let placement = self.placement_covering(pattern, tick.max(0) as u32)?;
        let index = self.playlist.iter().position(|item| *item == placement)?;
        self.playlist.remove(index);
        Some(placement)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shortening a pattern must not leave the roll highlighting notes it has
    /// stopped drawing.
    #[test]
    fn shortening_a_pattern_drops_the_selection_past_its_end() {
        let mut session = Session::default();
        let inside = session.channels[0].create_note(0, 0, TICKS_PER_STEP, 60);
        let outside = session.channels[0].create_note(0, 8 * TICKS_PER_STEP, TICKS_PER_STEP, 62);
        session.selected_note_ids = [inside.id, outside.id].into_iter().collect();

        let applied = session.set_pattern_length(4).expect("length changed");
        assert_eq!((applied.pattern, applied.length), (0, 4));
        assert_eq!(
            session.selected_note_ids,
            [inside.id].into_iter().collect(),
            "the note past the new end kept its selection"
        );

        assert!(
            session.set_pattern_length(4).is_none(),
            "re-applying the same length reported a change"
        );
    }

    /// A clip may not sit on top of another of the same pattern, and the
    /// playlist stays sorted so the sequencer can walk it in order.
    #[test]
    fn playlist_placements_refuse_to_overlap_and_stay_ordered() {
        let mut session = Session::default();
        let span = session.pattern_lengths[0] as u32 * TICKS_PER_STEP;

        assert!(session.add_playlist_placement(0, span as i32).is_some());
        assert!(session.add_playlist_placement(0, 0).is_some());
        assert!(
            session.add_playlist_placement(0, (span / 2) as i32).is_none(),
            "a clip landing inside an existing one was accepted"
        );
        assert_eq!(
            session
                .playlist
                .iter()
                .map(|placement| placement.start_tick)
                .collect::<Vec<_>>(),
            vec![0, span]
        );

        // A pattern that does not exist is not a placement.
        assert!(session.add_playlist_placement(9, 0).is_none());
        assert!(session.add_playlist_placement(-1, 0).is_none());

        let removed = session
            .remove_playlist_placement(0, (span + 1) as i32)
            .expect("the second clip covers that tick");
        assert_eq!(removed.start_tick, span);
        assert_eq!(session.playlist.len(), 1);
    }

    /// The tempo command has to reach the engine before any delay time
    /// derived from it.
    #[test]
    fn the_tempo_leads_the_delay_times_it_resolves() {
        let mut session = Session::default();
        let commands = session.set_tempo(140.0);
        assert!(matches!(commands.first(), Some(EngineCommand::SetTempo(bpm)) if *bpm == 140.0));
        assert!(session.dirty);
        assert_eq!(session.revision, 1);
    }

    /// Swing is clamped rather than refused: an automated or typed value can
    /// arrive at anything.
    #[test]
    fn swing_is_clamped_to_what_the_sequencer_accepts() {
        let mut session = Session::default();
        assert!(matches!(
            session.set_swing(1_000),
            EngineCommand::SetSwing(p) if p == MAX_SWING_PERCENT
        ));
        assert!(matches!(
            session.set_swing(-1_000),
            EngineCommand::SetSwing(p) if p == MIN_SWING_PERCENT
        ));
    }

    /// Selecting a pattern drops the note selection, which belongs to the
    /// pattern that was on screen.
    #[test]
    fn selecting_a_pattern_clears_the_note_selection() {
        let mut session = Session::default();
        let note = session.channels[0].create_note(0, 0, TICKS_PER_STEP, 60);
        session.selected_note_ids = [note.id].into_iter().collect();
        session.selected_note_id = Some(note.id);

        assert_eq!(session.add_pattern(), Some(1));
        assert!(session.selected_note_ids.is_empty());
        assert_eq!(session.selected_note_id, None);
        assert_eq!(session.channels[0].notes.len(), 2);

        assert_eq!(session.select_pattern(0), Some(0));
        assert_eq!(session.select_pattern(2), None);
        assert_eq!(session.select_pattern(-1), None);
    }
}
