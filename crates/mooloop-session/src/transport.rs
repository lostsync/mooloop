//! Transport, pattern and playlist edits.
//!
//! Each of these was the body of a Slint callback. They are named methods now
//! so a menu, a shortcut and a test can all reach the same edit, and so the
//! commands they produce are returned in the order the engine has to see them
//! rather than being sent from wherever the closure happened to be.

use crate::roll::NoteEdit;
use crate::session::Session;
use mooloop_core::{
    EngineCommand, LoopRange, MidiInputSource, NoteId, PatternMeta, PatternPlacement, PlaybackMode, ProjectColor, DEFAULT_STEPS,
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
    /// the tempo first, then every synced effect receives its resolved value
    /// -- a delay its milliseconds, a modulation its hertz -- so no
    /// beat-relative buffer is replaced against the old tempo.
    pub fn set_tempo(&mut self, bpm: f64) -> Vec<EngineCommand> {
        let mut commands = vec![EngineCommand::SetTempo(bpm)];
        commands.extend(self.update_tempo_synced_effects(bpm).into_iter().map(
            |(target, slot, id, value)| EngineCommand::SetEffectParam {
                target,
                slot,
                id,
                value,
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
        self.pattern_meta.push(PatternMeta::default());
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
        let Some(slot) = self.pattern_meta.get_mut(index) else {
            return false;
        };
        slot.name = name.trim().to_string();
        true
    }

    /// Gives a pattern a colour, or takes its colour away with `None`.
    ///
    /// Returns whether anything changed, so a caller does not mark a document
    /// dirty for re-choosing the colour it already had. The same shape as
    /// `rename_pattern`, and for the same reason: this is content, and the
    /// only thing it can be wrong about is which pattern it addresses.
    pub fn set_pattern_color(&mut self, index: usize, color: Option<ProjectColor>) -> bool {
        let Some(slot) = self.pattern_meta.get_mut(index) else {
            return false;
        };
        if slot.color == color {
            return false;
        }
        slot.color = color;
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

    /// Moves the transport to `tick` along the arrangement.
    ///
    /// Clamped to the drawn timeline rather than refused: this is the
    /// playhead being dragged, and a drag that runs off the end of the
    /// timeline means the end of the timeline. Not a document edit -- where
    /// the transport is playing from is not something a song should have to
    /// be saved to keep.
    ///
    /// The bound is the *song*, not `MAX_PLAYLIST_TICKS`. That constant is
    /// the end of the grid a placement may start on, and a long clip placed
    /// near the end legally overhangs it -- so clamping to it would have
    /// stopped the playhead at bar 64 of an eighty-bar song the transport
    /// plays to the end of on its own. `.max` keeps the empty-song case at
    /// the full canvas, which is what the view draws.
    pub fn seek_playlist(&mut self, tick: i32) -> EngineCommand {
        let limit = self.song_length_ticks().max(MAX_PLAYLIST_TICKS) as i32;
        let tick = tick.clamp(0, limit - 1);
        // A locate, not a pass over what it skips (MOO-234).
        self.recording.locate(tick as u64);
        EngineCommand::Seek { tick: tick.into() }
    }

    /// Sets the section the transport repeats, enabling the loop.
    ///
    /// `None` clears the loop and leaves the points where they were, so the
    /// toggle has something to switch back on. Ticks arrive snapped by the
    /// editor, and a drag with no width in it is a click rather than a loop.
    pub fn set_loop_range(&mut self, from_tick: i32, to_tick: i32) -> Option<EngineCommand> {
        self.apply_loop_range(LoopRange::from_drag(
            from_tick.max(0) as u32,
            to_tick.max(0) as u32,
        )?)
    }

    /// Moves the section without deciding whether it runs.
    ///
    /// This is the edge-handle drag, and it is a different gesture from
    /// [`set_loop_range`](Self::set_loop_range) above in exactly one respect:
    /// dragging a loop *out* asks for one and gets it switched on, while
    /// dragging an end of a loop that is already there says nothing about
    /// whether it should be live. A new song opens with two bars marked and
    /// looping off, so an edge drag that enabled it would overturn that
    /// answer on the way past -- and there would be no way to nudge a marked
    /// section without starting the repeat.
    pub fn adjust_loop_range(&mut self, from_tick: i32, to_tick: i32) -> Option<EngineCommand> {
        let range = LoopRange::from_drag(from_tick.max(0) as u32, to_tick.max(0) as u32)?;
        self.apply_loop_range(LoopRange {
            enabled: self.loop_range.enabled,
            ..range
        })
    }

    /// Stores a range and reports the command for it, or nothing when it is
    /// the range already held. The one writer of `loop_range`, so "a changed
    /// loop marks the song dirty and reaches the engine" is stated once.
    fn apply_loop_range(&mut self, range: LoopRange) -> Option<EngineCommand> {
        if self.loop_range == range {
            return None;
        }
        self.loop_range = range;
        self.mark_dirty();
        Some(EngineCommand::SetLoopRange(range))
    }

    /// Turns looping on or off without disturbing the points.
    ///
    /// Refuses to enable a loop that has no section in it: the button would
    /// otherwise light up and change nothing, which reads as a broken button
    /// rather than as an empty loop.
    pub fn set_loop_enabled(&mut self, enabled: bool) -> Option<EngineCommand> {
        if self.loop_range.enabled == enabled {
            return None;
        }
        if enabled && self.loop_range.end_tick <= self.loop_range.start_tick {
            return None;
        }
        self.apply_loop_range(LoopRange {
            enabled,
            ..self.loop_range
        })
    }

    /// Removes the loop entirely, points and all.
    pub fn clear_loop_range(&mut self) -> Option<EngineCommand> {
        self.apply_loop_range(LoopRange::default())
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

/// What a MIDI take does to the notes already under the playhead (MOO-234).
///
/// A session setting beside the record arm, and not saved for the arm's
/// reason: a song does not reopen armed, and what the next take will do to
/// the pattern is part of the same gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecordMode {
    /// Every pass adds to what is there. The default, and all there was
    /// before MOO-234.
    #[default]
    Overdub,
    /// Each pass replaces what it passes over: the recording channel's notes
    /// whose start the playhead crosses are removed as it crosses them.
    Replace,
}

/// The furthest a playhead moves between two pump ticks and still counts as
/// playing through the ticks in between, rather than as a locate.
///
/// A bar is several hundred milliseconds at any tempo the transport offers,
/// where the pump drains every 8 ms. Every locate the session makes itself
/// says so ([`RecordTake::locate`]); this is the backstop for one it did not
/// make, such as a song-position message from a controller.
const MAX_CROSSING_TICKS: u64 = mooloop_core::TICKS_PER_BAR as u64;

/// How far past a locate the session made the first position after it may
/// land and still be taken as the playhead arriving there, rather than as
/// one reported before the engine took the locate. A beat is many blocks.
const LOCATE_WINDOW_TICKS: u64 = mooloop_core::TICKS_PER_STEP as u64 * 4;

/// The notes a Replace take's playhead has just crossed, by channel, in the
/// pattern being recorded into: [`Session::record_position`] finds them and
/// [`Session::apply_replace`] removes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplacePlan {
    pub pattern: usize,
    pub removals: Vec<(usize, Vec<NoteId>)>,
}

/// A note this take recorded in the pass the playhead is in: spared by that
/// pass, and by no other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TakeNote {
    channel: usize,
    pattern: usize,
    id: NoteId,
    pass: u64,
}

/// A MIDI take as Replace sees it: where the playhead has been, and which
/// notes this take wrote. **Not** document state, like the arm it serves.
///
/// Only the pump knows where the playhead is, from the engine's once-a-block
/// `Position`, so the crossing is done here from those positions rather than
/// on the audio thread. The cost is that an old note still sounds if the
/// playhead reaches it before the pump does -- at most one pump tick.
#[derive(Debug, Clone, Default)]
pub struct RecordTake {
    pub mode: RecordMode,
    /// Where the transport was last seen parked, so a take started from a
    /// stop crosses the ticks from there, the downbeat included.
    parked: Option<u64>,
    /// The last song tick already crossed. `i64` so a take can start by
    /// crossing tick 0.
    cursor: Option<i64>,
    /// A locate the session made itself: the next position starts from it
    /// and does not cross the ticks the jump skipped.
    located: Option<u64>,
    /// The pattern and mode the cursor was measured in. A change of either
    /// is a new place to record, not a crossing.
    context: Option<(usize, bool)>,
    /// How many times the crossed offset has wrapped, and the last offset
    /// crossed.
    pass: u64,
    last_offset: Option<u32>,
    /// The notes this take wrote in the current pass. Emptied of earlier
    /// passes' as the crossing wraps, so it stays a pass's worth.
    notes: Vec<TakeNote>,
    /// The channels the MIDI input was routed to when the take started:
    /// the ones whose own input setting takes MIDI, or else the selected
    /// one. Fixed for the take, so a routing change mid-take does not move
    /// what it replaces. `None` between takes.
    channels: Option<Vec<usize>>,
}

impl RecordTake {
    /// The playhead was sent somewhere by the session itself: whatever it
    /// jumped over was not played.
    pub fn locate(&mut self, tick: u64) {
        self.located = Some(tick);
    }

    /// A take is over: forget what it wrote, so the next one replaces it.
    fn end(&mut self) {
        self.notes.clear();
        self.channels = None;
    }

}

impl Session {
    pub fn record_mode(&self) -> RecordMode {
        self.recording.mode
    }

    /// Switch Overdub and Replace. Not an edit: nothing in the song changes
    /// until a take plays over it.
    pub fn set_record_mode(&mut self, mode: RecordMode) {
        self.recording.mode = mode;
    }

    /// The playhead's position, from the engine's `Position`, in the order
    /// the engine reported it; and in Replace, while recording, the notes it
    /// has just crossed, for [`Session::apply_replace`] to remove (MOO-234).
    /// Nothing in the song changes here, so the caller can snapshot for
    /// undo only when there is something to remove.
    ///
    /// Only continuous advance crosses anything. A stop ends the take; a
    /// locate the session made ([`RecordTake::locate`]), a jump backwards
    /// other than the song loop's own, a jump forwards of more than a bar,
    /// and a change of pattern or mode each restart the crossing where the
    /// playhead now is. A tick belongs to the pattern the way a recorded
    /// note's start does, by [`recording_offset`], so the span removed is
    /// the span recording would have written into.
    ///
    /// Only the recording channel's notes in the pattern being recorded into
    /// are touched: the channel the MIDI input was routed to when the take
    /// started (`Session::input_channels`), whether or not anything has
    /// been played yet, so a silent pass clears what it crosses. A note
    /// this take recorded is removed only by a later pass over its start,
    /// never by the pass that played it.
    ///
    /// [`recording_offset`]: mooloop_core::playlist::recording_offset
    pub fn record_position(&mut self, tick: u64, playing: bool) -> Option<ReplacePlan> {
        if !playing {
            self.recording.parked = Some(tick);
            self.recording.cursor = None;
            self.recording.located = None;
            self.recording.context = None;
            self.recording.end();
            return None;
        }
        if !self.record_armed {
            self.recording.end();
        }
        let pattern = self.current_pattern;
        let length = self.recorded_pattern_length(pattern);
        let context = (pattern, self.song_mode);
        let take = &mut self.recording;
        let now = tick as i64;
        let cursor = if let Some(located) = take.located {
            // A position from before the engine took the locate may still
            // be in flight: the locate is taken by the first position at or
            // just past it, and one elsewhere crosses nothing.
            if tick >= located && tick - located <= LOCATE_WINDOW_TICKS {
                take.located = None;
                located as i64 - 1
            } else {
                now
            }
        } else if take.context.is_some_and(|previous| previous != context) {
            now
        } else {
            take.cursor
                .or_else(|| take.parked.map(|parked| parked as i64 - 1))
                .unwrap_or(now)
        };
        take.context = Some(context);
        take.cursor = Some(now);
        if !self.record_armed {
            return None;
        }
        if self.recording.channels.is_none() {
            let channels = self.input_channels();
            self.recording.channels = Some(channels);
        }
        let mut spans: [(i64, i64); 2] = [(0, 0); 2];
        if now > cursor {
            if (now - cursor) as u64 <= MAX_CROSSING_TICKS {
                spans[0] = (cursor, now);
            }
        } else if now < cursor {
            // The song loop's jump back is playing on, not a locate.
            let song_ticks = self.song_length_ticks();
            if let Some((start, end)) = self
                .song_mode
                .then(|| self.loop_range.active(song_ticks))
                .flatten()
            {
                let (start, end) = (i64::from(start), i64::from(end));
                let jumped = (end - 1 - cursor).max(0) + (now - start + 1).max(0);
                if cursor < end && now >= start && jumped as u64 <= MAX_CROSSING_TICKS {
                    spans = [(cursor, end - 1), (start - 1, now)];
                }
            }
        }
        let mode = if self.song_mode {
            PlaybackMode::Song
        } else {
            PlaybackMode::Pattern
        };
        let song_ticks = self.song_length_ticks();
        // Each offset crossed, and the pass it was crossed in.
        let mut crossed: Vec<(u32, u64)> = Vec::new();
        for (from, to) in spans {
            for song_tick in (from + 1).max(0)..=to {
                let Some(offset) = mooloop_core::playlist::recording_offset(
                    mode,
                    song_tick as f64,
                    pattern,
                    length,
                    song_ticks,
                    &self.playlist,
                ) else {
                    continue;
                };
                let take = &mut self.recording;
                if take.last_offset.is_some_and(|last| offset < last) {
                    take.pass += 1;
                }
                take.last_offset = Some(offset);
                crossed.push((offset, take.pass));
            }
        }
        if crossed.is_empty()
            || !self.record_armed
            || self.recording.mode != RecordMode::Replace
        {
            return None;
        }
        self.plan_replace(pattern, &crossed)
    }

    /// Which of the recording channels' notes have their start in
    /// `crossed`. Planned and applied separately so the caller can take an
    /// undo snapshot only when something is about to go.
    fn plan_replace(&self, pattern: usize, crossed: &[(u32, u64)]) -> Option<ReplacePlan> {
        let channels = self.recording.channels.clone().unwrap_or_default();
        let take = &self.recording;
        let removals: Vec<(usize, Vec<NoteId>)> = channels
            .into_iter()
            .filter_map(|channel| {
                let notes = self.channels.get(channel)?.notes.get(pattern)?;
                let removed: Vec<NoteId> = notes
                    .iter()
                    .filter(|note| {
                        crossed.iter().any(|&(offset, pass)| {
                            offset == note.start_tick
                                && !take.notes.contains(&TakeNote {
                                    channel,
                                    pattern,
                                    id: note.id,
                                    pass,
                                })
                        })
                    })
                    .map(|note| note.id)
                    .collect();
                (!removed.is_empty()).then_some((channel, removed))
            })
            .collect();
        (!removals.is_empty()).then_some(ReplacePlan { pattern, removals })
    }

    /// Where the MIDI input goes now: every channel whose own input setting
    /// takes MIDI, or, when none does, the selected channel if it follows
    /// the selection. The engine's rule (`RenderState::play_note`) at the
    /// grain the session can see: a port that is not plugged in still
    /// counts as routed, because the session holds names, not ports.
    fn input_channels(&self) -> Vec<usize> {
        let routed: Vec<usize> = self
            .channels
            .iter()
            .enumerate()
            .filter(|(_, channel)| {
                matches!(
                    channel.midi_input.source,
                    MidiInputSource::AllPorts | MidiInputSource::Port(_)
                )
            })
            .map(|(index, _)| index)
            .collect();
        if !routed.is_empty() {
            return routed;
        }
        self.channels
            .get(self.selected)
            .filter(|channel| channel.midi_input.source == MidiInputSource::FollowSelection)
            .map_or_else(Vec::new, |_| vec![self.selected])
    }

    /// Remove what [`Session::record_position`] found the playhead crossed.
    pub fn apply_replace(&mut self, plan: ReplacePlan) -> NoteEdit {
        let pattern = plan.pattern;
        let mut commands = Vec::new();
        for (channel, removed) in plan.removals {
            let Some(notes) = self
                .channels
                .get_mut(channel)
                .and_then(|state| state.notes.get_mut(pattern))
            else {
                continue;
            };
            notes.retain(|note| !removed.contains(&note.id));
            self.recording.notes.retain(|recorded| {
                !(recorded.channel == channel
                    && recorded.pattern == pattern
                    && removed.contains(&recorded.id))
            });
            if channel == self.selected && pattern == self.current_pattern {
                self.prune_note_selection(&removed);
            }
            commands.extend(removed.iter().map(|&id| EngineCommand::RemoveNote {
                pattern: pattern as u8,
                channel: channel as u8,
                id,
            }));
        }
        let notes = commands.len();
        NoteEdit {
            commands,
            cells: None,
            notes,
        }
    }

    /// Note a recorded note as this take's, so the pass that played it does
    /// not replace it. Called by [`Session::record_note`].
    pub(crate) fn remember_take_note(&mut self, channel: usize, pattern: usize, id: NoteId) {
        let Some(start) = self
            .channels
            .get(channel)
            .and_then(|state| state.notes.get(pattern))
            .and_then(|notes| notes.iter().find(|note| note.id == id))
            .map(|note| note.start_tick)
        else {
            return;
        };
        // The pump hands the session a block's position before the notes
        // that block reported, and the position is the block's end, so a
        // note played in this pass starts at or before the last offset
        // crossed. One that starts after it was pressed in an earlier pass
        // and held over the loop point: this pass replaces it like any old
        // note. (If the event ring ever drops a position, a note of this
        // pass can look like that and is replaced when crossed; the ring
        // keeps two slots for positions so that does not happen.)
        let take = &mut self.recording;
        if take.last_offset.is_some_and(|offset| start > offset) {
            return;
        }
        let pass = take.pass;
        take.notes.retain(|note| note.pass == pass);
        take.notes.push(TakeNote {
            channel,
            pattern,
            id,
            pass,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{MAX_PLAYLIST_BARS, TICKS_PER_BAR};

    /// **A clip buried by a pattern-length change can still be reached.**
    ///
    /// `add_playlist_placement` refuses an overlap and is the only place that
    /// invariant exists. `set_pattern_length` rewrites the length with no
    /// revalidation, so two clips that abut exactly -- which is accepted --
    /// swallow each other the moment the pattern grows. Both go on playing,
    /// which is what layering means and is not this; what could not be done
    /// was *removing* the later one, because `placement_covering` took the
    /// first match on a list sorted by `(pattern, start_tick)` and so always
    /// answered with the earlier clip. The buried one could not be reached at
    /// all, by any gesture.
    #[test]
    fn a_clip_buried_by_a_length_change_can_still_be_removed() {
        let mut session = Session::default();
        session
            .add_playlist_placement(0, 0)
            .expect("the first clip");
        session
            .add_playlist_placement(0, 16 * TICKS_PER_STEP as i32)
            .expect("abutting exactly is accepted");
        assert_eq!(session.playlist.len(), 2);

        // Double the pattern: the first clip now covers the second.
        assert_eq!(session.current_pattern, 0);
        session.set_pattern_length(32);

        let buried = 16 * TICKS_PER_STEP;
        assert_eq!(
            session
                .placement_covering(0, buried)
                .map(|placement| placement.start_tick),
            Some(buried),
            "a click in the overlap resolved to the clip underneath"
        );
        let removed = session
            .remove_playlist_placement(0, buried as i32)
            .expect("the buried clip is removable");
        assert_eq!(removed.start_tick, buried);
        assert_eq!(
            session.playlist.iter().map(|p| p.start_tick).collect::<Vec<_>>(),
            vec![0],
            "and removing it left the one that was on top of it"
        );
    }

    /// Outside the overlap nothing changed: a tick covered by one clip
    /// resolves to that clip, and a tick covered by none resolves to nothing.
    #[test]
    fn a_tick_under_one_clip_or_no_clip_answers_the_same_as_it_always_did() {
        let mut session = Session::default();
        session.add_playlist_placement(0, 0).expect("a clip");
        let length = 16 * TICKS_PER_STEP;

        assert_eq!(
            session.placement_covering(0, 0).map(|p| p.start_tick),
            Some(0)
        );
        assert_eq!(
            session.placement_covering(0, length - 1).map(|p| p.start_tick),
            Some(0)
        );
        assert_eq!(session.placement_covering(0, length), None, "half-open");
        assert_eq!(session.placement_covering(1, 0), None, "another pattern");
        assert_eq!(
            session.placement_covering(usize::MAX, 0),
            None,
            "a pattern that is not there must not index the length table"
        );
    }

    /// A pattern may go nameless -- the playlist gutter and the pattern menu
    /// both fall back to its number -- so blanking one is an edit, not a
    /// refusal. That is the deliberate difference from `rename_channel` and
    /// `rename_track`, which reject a blank.
    #[test]
    fn a_pattern_takes_a_name_and_may_give_it_back() {
        let mut session = Session::default();

        assert!(session.rename_pattern(0, "  Chorus  "), "a real name was refused");
        assert_eq!(session.pattern_meta[0].name, "Chorus", "the name was not trimmed");

        assert!(session.rename_pattern(0, "   "), "blanking a pattern name was refused");
        assert_eq!(session.pattern_meta[0].name, "", "the name did not clear");

        assert!(!session.rename_pattern(session.pattern_meta.len(), "Nope"));
    }

    /// A colour is the other half of the same entry, and the difference from
    /// the name is that nothing is derived when it is absent: no colour is the
    /// ordinary case rather than a fallback.
    #[test]
    fn a_pattern_takes_a_colour_and_may_give_it_back() {
        let mut session = Session::default();
        let green = ProjectColor::new(0x84, 0xCC, 0x16);

        assert!(session.set_pattern_color(0, Some(green)));
        assert_eq!(session.pattern_meta[0].color, Some(green));

        // Choosing the colour it already has is not an edit, so a caller does
        // not mark the document dirty for it.
        assert!(!session.set_pattern_color(0, Some(green)));

        assert!(session.set_pattern_color(0, None), "clearing a colour was refused");
        assert_eq!(session.pattern_meta[0].color, None);

        assert!(!session.set_pattern_color(session.pattern_meta.len(), Some(green)));
    }

    /// Every pattern has an entry, so adding one cannot leave the name and
    /// colour lists short of the bank -- which is what an index into them
    /// would panic on.
    #[test]
    fn a_new_pattern_arrives_with_an_entry_of_its_own() {
        let mut session = Session::default();
        session.add_pattern().expect("the bank was full");
        assert_eq!(session.pattern_meta.len(), session.pattern_lengths.len());
        assert!(session.pattern_meta.last().expect("no entry").is_empty());
    }

    /// **The round trip the whole pattern-metadata field exists for.**
    ///
    /// A name and a colour set on a pattern, written into a project snapshot,
    /// and loaded back. This is the test that would have failed every day
    /// between 2026-09-07 and 2026-09-13, when `rename_pattern` wrote to a
    /// session field the document had no room for: the name went in, the
    /// snapshot dropped it, and the load blanked what was left.
    #[test]
    fn a_patterns_name_and_colour_survive_a_snapshot_and_a_load() {
        let mut session = Session::default();
        let amber = ProjectColor::new(0xEA, 0xB3, 0x08);
        session.add_pattern().expect("the bank was full");
        assert!(session.rename_pattern(1, "Chorus"));
        assert!(session.set_pattern_color(1, Some(amber)));
        // Pattern 0 is left untouched on purpose: the trimming on the way out
        // drops trailing empties, and an empty entry *before* a full one has
        // to keep its place or every colour after it shifts by one.
        let project = session.project_snapshot(120, 50);
        assert_eq!(project.pattern_meta.len(), 2, "a leading empty entry was dropped");

        let mut reopened = Session::default();
        reopened.replace_project(&project, &[None]);
        assert_eq!(reopened.pattern_meta[1].name, "Chorus");
        assert_eq!(reopened.pattern_meta[1].color, Some(amber));
        assert!(reopened.pattern_meta[0].is_empty(), "pattern 1 gained something");
        assert_eq!(
            reopened.pattern_meta.len(),
            reopened.pattern_lengths.len(),
            "the loaded bank and its metadata disagree about how many patterns there are"
        );
    }

    /// A song where nobody named or coloured anything writes no entries at
    /// all, which is what keeps an untouched song byte-identical to one saved
    /// before the field existed.
    #[test]
    fn a_song_nobody_has_named_writes_no_pattern_metadata() {
        let mut session = Session::default();
        session.add_pattern().expect("the bank was full");
        assert!(session.project_snapshot(120, 50).pattern_meta.is_empty());
    }

    /// Shortening a pattern must not leave the roll highlighting notes it has
    /// stopped drawing.
    #[test]
    fn shortening_a_pattern_drops_the_selection_past_its_end() {
        let mut session = Session::default();
        let inside = session.channels[0].create_note(0, 0, TICKS_PER_STEP, 60).expect("room");
        let outside = session.channels[0].create_note(0, 8 * TICKS_PER_STEP, TICKS_PER_STEP, 62).expect("room");
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
        let note = session.channels[0].create_note(0, 0, TICKS_PER_STEP, 60).expect("room");
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

    /// A loop is a section, so a drag with no width in it is not one, and the
    /// points survive being switched off so the toggle has something to
    /// switch back on.
    #[test]
    fn a_loop_needs_a_section_and_keeps_its_points_when_off() {
        let mut session = Session::default();
        assert!(session.set_loop_range(0, 0).is_none(), "a click is not a loop");
        assert!(
            !session.loop_range.enabled,
            "a refused drag must not enable an empty loop"
        );
        assert!(session.set_loop_enabled(true).is_none(), "nothing to enable yet");

        // Dragged right to left, which is the same section.
        let command = session.set_loop_range(4 * TICKS_PER_BAR as i32, TICKS_PER_BAR as i32);
        assert!(matches!(
            command,
            Some(EngineCommand::SetLoopRange(range))
                if range.start_tick == TICKS_PER_BAR
                    && range.end_tick == 4 * TICKS_PER_BAR
                    && range.enabled
        ));
        assert!(session.dirty);
        assert!(session.set_loop_range(TICKS_PER_BAR as i32, 4 * TICKS_PER_BAR as i32).is_none());

        assert!(session.set_loop_enabled(false).is_some());
        assert_eq!(
            (session.loop_range.start_tick, session.loop_range.end_tick),
            (TICKS_PER_BAR, 4 * TICKS_PER_BAR),
            "switching a loop off threw its points away"
        );
        assert!(session.set_loop_enabled(false).is_none());
        assert!(session.set_loop_enabled(true).is_some());

        assert!(session.clear_loop_range().is_some());
        assert_eq!(session.loop_range, LoopRange::default());
        assert!(session.clear_loop_range().is_none());
    }

    /// A loop past the end of the song is inert rather than a position the
    /// transport can be asked for, and shortening a song under a loop stops
    /// it without editing it.
    #[test]
    fn a_loop_is_clamped_to_the_song_it_sits_in() {
        let range = LoopRange {
            start_tick: 2 * TICKS_PER_BAR,
            end_tick: 6 * TICKS_PER_BAR,
            enabled: true,
        };
        assert_eq!(
            range.active(8 * TICKS_PER_BAR),
            Some((2 * TICKS_PER_BAR, 6 * TICKS_PER_BAR))
        );
        assert_eq!(
            range.active(4 * TICKS_PER_BAR),
            Some((2 * TICKS_PER_BAR, 4 * TICKS_PER_BAR)),
            "a loop overhanging the song end plays the part of it that exists"
        );
        assert_eq!(
            range.active(TICKS_PER_BAR),
            None,
            "a loop entirely past the song end is inert"
        );
        assert_eq!(
            LoopRange { enabled: false, ..range }.active(8 * TICKS_PER_BAR),
            None
        );
    }

    /// The playhead can be dragged past the end of the timeline; the timeline
    /// end is where it lands.
    #[test]
    fn a_seek_lands_inside_the_canvas() {
        let mut session = Session::default();
        assert!(matches!(
            session.seek_playlist(-10),
            EngineCommand::Seek { tick } if tick == 0.0
        ));
        assert!(matches!(
            session.seek_playlist(i32::MAX),
            EngineCommand::Seek { tick } if tick == f64::from(MAX_PLAYLIST_TICKS - 1)
        ));
        assert!(!session.dirty, "moving the playhead is not a document edit");
    }

    /// `MAX_PLAYLIST_TICKS` bounds where a clip may *start*, and
    /// `playlist.rs` says so: "Long clips may extend past this point and
    /// still contribute to the derived song length." The seek borrowed that
    /// bound for a different question, so the playhead stopped at bar 64 of
    /// a song the view draws eighty bars of and the transport plays to the
    /// end of on its own.
    #[test]
    fn a_seek_reaches_the_end_of_a_song_that_overhangs_the_canvas() {
        let mut session = Session::default();
        session.set_pattern_length(MAX_PATTERN_STEPS as i32);
        let start = (MAX_PLAYLIST_BARS - 1) * TICKS_PER_BAR;
        session
            .add_playlist_placement(0, start as i32)
            .expect("a clip may start on the last bar of the canvas");

        let length = session.song_length_ticks();
        assert!(
            length > MAX_PLAYLIST_TICKS,
            "the clip must overhang for this to test anything, got {length}"
        );
        assert!(matches!(
            session.seek_playlist(i32::MAX),
            EngineCommand::Seek { tick } if tick == f64::from(length - 1)
        ));
    }

    /// MOO-234's Replace, driven the way the pump drives it: one position
    /// at a time, notes arriving when their keys come up.
    mod replace {
        use super::*;
        use crate::channel::ChannelState;

        const STEP: u64 = 8;

        /// A session armed in Replace on a 384-tick pattern, with a second
        /// channel and a second pattern for the notes it must not touch.
        fn armed() -> Session {
            let mut session = Session::default();
            session.channels.push(ChannelState::new(1));
            session.add_pattern().expect("a second pattern");
            session.current_pattern = 0;
            session.selected = 0;
            session.record_armed = true;
            session.set_record_mode(RecordMode::Replace);
            // Parked at the top.
            assert!(session.record_position(0, false).is_none());
            session
        }

        fn plant(session: &mut Session, channel: usize, pattern: usize, start: u32) -> NoteId {
            session.channels[channel]
                .create_note(pattern, start, TICKS_PER_STEP, 60)
                .expect("room")
                .id
        }

        fn has(session: &Session, channel: usize, pattern: usize, id: NoteId) -> bool {
            session.channels[channel].notes[pattern]
                .iter()
                .any(|note| note.id == id)
        }

        /// Play from `from` to `to` in pump-sized steps, returning every
        /// note id removed.
        fn play(session: &mut Session, from: u64, to: u64) -> Vec<NoteId> {
            let mut removed = Vec::new();
            let mut tick = from;
            while tick <= to {
                if let Some(plan) = session.record_position(tick, true) {
                    let edit = session.apply_replace(plan);
                    removed.extend(edit.commands.iter().filter_map(|command| match command {
                        EngineCommand::RemoveNote { id, .. } => Some(*id),
                        _ => None,
                    }));
                }
                tick += STEP;
            }
            removed
        }

        /// (1) and (4): a note goes when the playhead crosses its start,
        /// only on the recording channel and in the pattern being recorded
        /// into, and a note this take played survives the pass that played
        /// it -- even one reported before the position that passed it.
        #[test]
        fn a_pass_replaces_what_it_crosses_and_keeps_what_it_played() {
            let mut session = armed();
            let downbeat = plant(&mut session, 0, 0, 0);
            let early = plant(&mut session, 0, 0, 100);
            let late = plant(&mut session, 0, 0, 200);
            let other_channel = plant(&mut session, 1, 0, 100);
            let other_pattern = plant(&mut session, 0, 1, 100);

            let removed = play(&mut session, 8, 152);
            assert_eq!(removed, vec![downbeat, early], "the downbeat is crossed too");
            assert!(has(&session, 0, 0, late));

            // Played at 144, reported with the position at 152 that the
            // pump hands over first.
            session
                .record_note(0, 0, 64, 100, 144, TICKS_PER_STEP)
                .expect("recorded");
            let played = session.channels[0].notes[0]
                .iter()
                .find(|note| note.note == 64)
                .expect("the take's note")
                .id;
            let removed = play(&mut session, 160, 384 + 96);
            assert_eq!(removed, vec![late], "the rest of the pass, and not its own note");
            assert!(has(&session, 0, 0, played), "the pass that played it keeps it");

            // The next pass over 144 replaces it.
            let removed = play(&mut session, 384 + 104, 384 + 200);
            assert_eq!(removed, vec![played]);
            assert!(has(&session, 1, 0, other_channel), "another channel is untouched");
            assert!(has(&session, 0, 1, other_pattern), "another pattern is untouched");
        }

        /// A note held over the loop point was played in the pass before,
        /// so the pass it is reported in replaces it when it gets there.
        #[test]
        fn a_note_held_over_the_loop_belongs_to_the_pass_it_was_played_in() {
            let mut session = armed();
            play(&mut session, 8, 384 + 40);
            session
                .record_note(0, 0, 64, 100, 360, TICKS_PER_STEP * 3)
                .expect("recorded");
            let removed = play(&mut session, 384 + 48, 384 + 368);
            assert_eq!(removed.len(), 1, "replaced by the pass after the one that played it");
        }

        /// On a four-bar pattern, a note one beat ahead of the playhead is
        /// still replaced when the playhead gets there: an old note that
        /// was there before the take, and one this take played in the pass
        /// before and held over the loop point. No tolerance spares either.
        #[test]
        fn a_long_pattern_spares_nothing_ahead_of_the_playhead() {
            let mut session = armed();
            session.pattern_lengths[0] = 64;
            let bar = u64::from(mooloop_core::TICKS_PER_BAR);
            let pattern = 4 * bar;
            let beat = u64::from(TICKS_PER_STEP) * 4;
            play(&mut session, 8, pattern + 104);
            let old = plant(&mut session, 0, 0, (104 + beat) as u32);
            // Pressed on 200 in the first pass and released on 104 of the
            // second, a pass and a half later.
            session
                .record_note(0, 0, 64, 100, (104 + beat) as u32, TICKS_PER_STEP)
                .expect("recorded");
            let held = session.channels[0].notes[0]
                .iter()
                .find(|note| note.note == 64)
                .expect("the held note")
                .id;
            let mut removed = play(&mut session, pattern + 112, pattern + 104 + beat);
            removed.sort_unstable();
            let mut expected = vec![old, held];
            expected.sort_unstable();
            assert_eq!(removed, expected);
        }

        /// (2): a locate, a jump, a stop or a pattern switch crosses
        /// nothing.
        #[test]
        fn only_continuous_advance_crosses_anything() {
            let mut session = armed();
            let skipped = plant(&mut session, 0, 0, 150);
            let reached = plant(&mut session, 0, 0, 300);
            play(&mut session, 8, 48);

            // The session's own locate. A position from before the engine
            // took it is still in flight and crosses nothing either.
            let _ = session.seek_playlist(250);
            assert!(session.record_position(56, true).is_none());
            let removed = play(&mut session, 256, 320);
            assert_eq!(removed, vec![reached]);
            assert!(has(&session, 0, 0, skipped), "a locate skips, it does not cross");

            // A jump of more than a bar that nobody announced.
            let far = plant(&mut session, 0, 0, 40);
            assert!(session.record_position(320 + 384 + 8, true).is_none());
            assert!(has(&session, 0, 0, far));

            // A stop, and play again from somewhere else.
            assert!(session.record_position(2000, false).is_none());
            let parked_over = plant(&mut session, 0, 0, 16);
            let removed = play(&mut session, 3000, 3008);
            assert!(removed.is_empty(), "a new start is not a pass over the old one");
            assert!(has(&session, 0, 0, parked_over));

            // A pattern switch while playing.
            let in_new = plant(&mut session, 0, 1, 250);
            session.select_pattern(1).expect("pattern 1");
            assert!(session.record_position(3016 + 384, true).is_none());
            assert!(has(&session, 0, 1, in_new), "the switch itself crosses nothing");
        }

        /// The song loop jumping back is a pass wrapping, not a locate, and
        /// crosses the ticks either side of the jump.
        #[test]
        fn the_song_loop_wraps_a_pass() {
            let mut session = armed();
            session.song_mode = true;
            session.add_playlist_placement(0, 0).expect("a clip");
            session.loop_range = LoopRange {
                start_tick: 0,
                end_tick: 384,
                enabled: true,
            };
            let before_end = plant(&mut session, 0, 0, 380);
            let after_start = plant(&mut session, 0, 0, 4);
            assert!(session.record_position(360, false).is_none());
            play(&mut session, 368, 376);
            assert!(has(&session, 0, 0, before_end));
            let plan = session.record_position(8, true).expect("a crossing");
            let removed = session.apply_replace(plan).notes;
            assert_eq!(removed, 2, "both sides of the jump are crossed");
            assert!(!has(&session, 0, 0, before_end));
            assert!(!has(&session, 0, 0, after_start));
        }

        /// (3): nothing is removed while merely armed and stopped, while
        /// playing disarmed, or in Overdub.
        #[test]
        fn replace_waits_for_a_running_take() {
            let mut session = armed();
            let note = plant(&mut session, 0, 0, 16);
            for tick in (0..64).step_by(8) {
                assert!(session.record_position(tick, false).is_none(), "armed and stopped");
            }
            session.record_armed = false;
            assert!(play(&mut session, 0, 64).is_empty(), "playing, not armed");
            session.record_armed = true;
            session.set_record_mode(RecordMode::Overdub);
            assert!(session.record_position(0, false).is_none());
            assert!(play(&mut session, 0, 64).is_empty(), "overdub");
            assert!(has(&session, 0, 0, note));
        }

        /// A pass where nothing is played still replaces: the recording
        /// channel is where the input goes when the take starts, not where
        /// the first note lands.
        #[test]
        fn a_silent_pass_clears_what_it_crosses() {
            let mut session = armed();
            let crossed: Vec<NoteId> = [0, 96, 200, 383]
                .into_iter()
                .map(|start| plant(&mut session, 0, 0, start))
                .collect();
            let removed = play(&mut session, 8, 384);
            assert_eq!(removed, crossed, "the whole pass, with nothing played");
            assert!(session.channels[0].notes[0].is_empty());
        }

        /// A channel that takes MIDI by its own input is the recording
        /// channel from the start of the take, and the selected one is not;
        /// a routing change mid-take does not move it.
        #[test]
        fn the_channel_routed_when_the_take_starts_is_the_one_replaced() {
            let mut session = armed();
            session.channels[1].midi_input.source = MidiInputSource::AllPorts;
            let selected = plant(&mut session, 0, 0, 200);
            let routed = plant(&mut session, 1, 0, 100);
            let routed_late = plant(&mut session, 1, 0, 300);
            play(&mut session, 8, 104);
            assert!(!has(&session, 1, 0, routed), "replaced before anything is played");

            // The routing moves back to the selection mid-take.
            session.channels[1].midi_input.source = MidiInputSource::FollowSelection;
            play(&mut session, 112, 320);
            assert!(has(&session, 0, 0, selected), "the take started on channel 1");
            assert!(!has(&session, 1, 0, routed_late), "and stays on it");

            // The next take starts where the input goes now.
            assert!(session.record_position(320, false).is_none());
            play(&mut session, 328, 384 + 208);
            assert!(!has(&session, 0, 0, selected));
        }
    }
}
