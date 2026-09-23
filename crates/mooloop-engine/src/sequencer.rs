//! Realtime pattern scheduler.
//!
//! Notes live at PPQ tick positions and are converted to sample offsets for
//! each process block. Pattern storage and event lists are bounded up front;
//! scheduling and edits never allocate on the audio thread.

use std::ops::Range;

use mooloop_core::{
    AutomationLane, AutomationPoint, EffectTarget, LanePool, NoteEvent, NoteId, ParamAddr, Pattern,
    DeviceId, PatternPlacement, PlaybackMode, PointId, Ppq, Project,
    DEFAULT_NOTE_DURATION_TICKS, DEFAULT_STEPS, DEFAULT_SWING_PERCENT, MAX_CHANNELS,
    MAX_NOTES_PER_CHANNEL_PATTERN, MAX_PATTERN_STEPS, MAX_PLAYLIST_PLACEMENTS, MAX_PLAYLIST_TICKS,
    MAX_SWING_PERCENT, MIN_SWING_PERCENT, TICKS_PER_BAR, TICKS_PER_STEP,
};
use mooloop_dsp::{Event, EventList, TimedEvent};

/// The stretch of a process block one scheduling pass may write into.
///
/// A block is one window until a song loop cuts it in two, and the halves
/// carry different musical ranges into different frames of the same event
/// list, so the frame arithmetic cannot be left implicit at `0..frames`.
#[derive(Debug, Clone, Copy)]
struct FrameWindow {
    offset: usize,
    frames: usize,
}

impl FrameWindow {
    /// The block-absolute frame an event `tick_delta` ticks into this window
    /// lands on, clamped so a rounding error at either edge cannot escape the
    /// window it belongs to.
    fn offset_for(self, tick_delta: f64, ticks_per_sample: f64) -> u32 {
        let frame = (tick_delta / ticks_per_sample).round() as i64;
        let last = self.offset as i64 + self.frames as i64 - 1;
        frame.saturating_add(self.offset as i64).clamp(self.offset as i64, last) as u32
    }
}

/// [`MAX_PATTERN_STEPS`] converted to ticks: the largest span a single
/// pattern pass can occupy, and so how far before a reduced tick-range's
/// start a placement search must reach to be sure it has not missed one
/// that started earlier and still covers the range
/// (`reports/fable-2026-09-22.md`, finding 1, Plan A).
const MAX_PATTERN_TICKS: u32 = MAX_PATTERN_STEPS as u32 * TICKS_PER_STEP;

pub struct Sequencer {
    patterns: Vec<Pattern>,
    active_patterns: usize,
    current: usize,
    active_channels: usize,
    playback_mode: PlaybackMode,
    swing_percent: u8,
    playlist: Vec<PatternPlacement>,
    /// The same placements as `playlist`, sorted by `(start_tick, pattern)`
    /// instead of `(pattern, start_tick)` -- a timeline view kept beside the
    /// editing one rather than instead of it, since nothing about
    /// `playlist`'s own order is allowed to change (`set_playlist_placement`'s
    /// doc comment). Rebuilt incrementally at the same sites that touch
    /// `playlist`, so a block never re-sorts it.
    ///
    /// Only [`Self::automation_lane_at`]'s Song arm reads this -- it answers
    /// one destination and is asked once per automated destination per
    /// block, so avoiding a walk of every placement pays for itself there.
    /// `schedule_song` walks `playlist` itself instead, even though it is
    /// exactly the query this view answers: it emits events, and two
    /// placements can each contribute one at the very same offset, where
    /// `push_ordered` keeps ties in insertion order -- so the walk has to
    /// visit placements in `playlist`'s own order, not this one's. The
    /// secondary key here matters for the query this view *does* answer:
    /// two placements can share a start tick
    /// (`song_mode_layers_patterns_at_the_same_position`), and the winner is
    /// whichever comes last in `playlist`'s order, which is pattern
    /// ascending within a shared start tick -- sorting by
    /// `(start_tick, pattern)` reproduces that without having to remember
    /// insertion history.
    playlist_by_start: Vec<PatternPlacement>,
    /// Point storage for lanes opened on the audio thread. Filled here and
    /// topped up at every project install, both off the thread.
    lane_pool: LanePool,
}

impl Sequencer {
    pub fn new(
        initial_channels: usize,
        active_patterns: usize,
        num_steps: usize,
        ppq: Ppq,
    ) -> Self {
        assert_eq!(ppq, Ppq::DEFAULT, "pattern tick constants require PPQ 96");
        let patterns = (0..mooloop_core::MAX_PATTERNS)
            .map(|_| {
                let mut pattern = Pattern::with_steps(MAX_CHANNELS, MAX_PATTERN_STEPS as usize);
                pattern.set_length_steps(num_steps);
                pattern
            })
            .collect();
        Self {
            patterns,
            active_patterns: active_patterns.clamp(1, mooloop_core::MAX_PATTERNS),
            current: 0,
            active_channels: initial_channels.min(MAX_CHANNELS),
            playback_mode: PlaybackMode::Pattern,
            swing_percent: DEFAULT_SWING_PERCENT,
            playlist: Vec::with_capacity(MAX_PLAYLIST_PLACEMENTS),
            playlist_by_start: Vec::with_capacity(MAX_PLAYLIST_PLACEMENTS),
            lane_pool: LanePool::new(),
        }
    }

    /// Select a pattern, answering whether the selection actually moved.
    ///
    /// The answer is what [`crate::render::RenderState::apply_command`]
    /// charges against. A refused index and a re-selection of the pattern
    /// already current both change nothing that is scheduled, and both used
    /// to cost a full discontinuity -- so opening the jump menu on its own
    /// current entry, or bouncing the stepper off a bound, cut off every
    /// sounding voice. `docs/plans/transport-discontinuity/`.
    pub fn set_current_pattern(&mut self, pattern: usize) -> bool {
        if pattern >= self.active_patterns || pattern == self.current {
            return false;
        }
        self.current = pattern;
        true
    }

    pub fn add_pattern(&mut self) -> bool {
        if self.active_patterns >= self.patterns.len() {
            return false;
        }
        self.active_patterns += 1;
        true
    }

    #[cfg(test)]
    pub fn active_patterns(&self) -> usize {
        self.active_patterns
    }

    pub fn set_pattern_length(&mut self, pattern: usize, length_steps: usize) {
        if pattern < self.active_patterns {
            let pattern = &mut self.patterns[pattern];
            pattern.set_length_steps(length_steps);
        }
    }

    pub fn set_playback_mode(&mut self, mode: PlaybackMode) {
        self.playback_mode = mode;
    }

    pub fn set_swing(&mut self, percent: u8) {
        self.swing_percent = percent.clamp(MIN_SWING_PERCENT, MAX_SWING_PERCENT);
    }

    // Keep override resolution here when patterns gain their own swing value.
    fn swing_for_pattern(&self, _pattern: usize) -> u8 {
        self.swing_percent
    }

    /// Toggle one placement, answering whether the playlist changed.
    ///
    /// **The playlist is sorted and deduplicated, and this is the only thing
    /// that keeps it that way once a project is loaded** (`load_project`
    /// sorts and dedups what it takes from the document). That invariant used
    /// to be a consequence of the implementation -- push, then sort the whole
    /// vector -- and is now the thing the implementation depends on: a
    /// `partition_point` gives the insertion index and the duplicate check in
    /// one binary search, and both answers are wrong if the vector is ever
    /// unsorted. `playlist_stays_sorted_and_deduplicated_however_it_is_painted`
    /// is the guard.
    ///
    /// This runs on the audio thread, once per cell of a painted range, so a
    /// `sort_unstable` over up to `MAX_PLAYLIST_PLACEMENTS` per cell was
    /// `O(n log n)` work in the callback for a vector that was already in
    /// order except for the one element just pushed
    /// (`reports/fable-2026-09-21.md`, finding 5). The insert is one memmove
    /// and the capacity guard is unchanged, so it still never reallocates.
    ///
    /// Note that the order is `PatternPlacement`'s derived one -- pattern
    /// first, then start tick -- and not time order. Nothing here reads it as
    /// a timeline; every consumer walks the whole vector.
    pub fn set_playlist_placement(&mut self, pattern: usize, start_tick: u32, on: bool) -> bool {
        if pattern >= self.active_patterns {
            return false;
        }
        if start_tick >= MAX_PLAYLIST_TICKS {
            return false;
        }
        let placement = PatternPlacement::new(pattern as u8, start_tick);
        let index = self.playlist.partition_point(|item| *item < placement);
        let present = self.playlist.get(index) == Some(&placement);
        let changed = match (on, present) {
            (true, false) if self.playlist.len() < self.playlist.capacity() => {
                self.playlist.insert(index, placement);
                true
            }
            (false, true) => {
                self.playlist.remove(index);
                true
            }
            _ => false,
        };
        if changed {
            self.update_playlist_by_start(placement, on);
        }
        changed
    }

    /// Keep [`Self::playlist_by_start`] in step with one change already made
    /// to `playlist`. A sorted insert or removal by `(start_tick, pattern)`,
    /// the same shape `playlist`'s own maintenance uses -- no re-sort, no
    /// allocation, and the audio thread this runs on never sees either.
    fn update_playlist_by_start(&mut self, placement: PatternPlacement, on: bool) {
        let key = (placement.start_tick, placement.pattern);
        let index = self
            .playlist_by_start
            .partition_point(|item| (item.start_tick, item.pattern) < key);
        if on {
            self.playlist_by_start.insert(index, placement);
        } else {
            debug_assert_eq!(self.playlist_by_start.get(index), Some(&placement));
            self.playlist_by_start.remove(index);
        }
    }

    /// Rebuild [`Self::playlist_by_start`] from `playlist` after a bulk
    /// change (`load_project`) rather than one placement at a time. Off the
    /// audio thread, so a full sort is fine; `sort_unstable_by_key` does not
    /// need to be stable here because the key is already the whole ordering
    /// (`(start_tick, pattern)`, and `playlist` holds no two placements with
    /// the same `(pattern, start_tick)`, so no two share this key either).
    fn rebuild_playlist_by_start(&mut self) {
        self.playlist_by_start.clear();
        self.playlist_by_start.extend_from_slice(&self.playlist);
        self.playlist_by_start
            .sort_unstable_by_key(|item| (item.start_tick, item.pattern));
    }

    #[cfg(test)]
    pub fn playlist(&self) -> &[PatternPlacement] {
        &self.playlist
    }

    pub fn song_length_ticks(&self) -> u32 {
        let content_end = self
            .playlist
            .iter()
            .filter_map(|placement| {
                self.patterns
                    .get(placement.pattern as usize)
                    .filter(|_| (placement.pattern as usize) < self.active_patterns)
                    .map(|pattern| placement.start_tick.saturating_add(pattern.length_ticks()))
            })
            .max()
            .unwrap_or(TICKS_PER_BAR)
            .max(TICKS_PER_BAR);
        content_end.div_ceil(TICKS_PER_BAR) * TICKS_PER_BAR
    }

    pub fn active_channels(&self) -> usize {
        self.active_channels
    }

    pub fn set_active_channels(&mut self, n: usize) {
        self.active_channels = n.min(MAX_CHANNELS);
    }

    /// Clear one preallocated channel lane across the patterns the song
    /// actually holds.
    ///
    /// Bounded by `active_patterns` the way [`Self::forget_device`] is, and
    /// for the same reason: this runs on the realtime command drain, and the
    /// bank behind `active_patterns` is 256 patterns of nothing.
    pub fn clear_channel(&mut self, channel: usize) {
        if channel >= MAX_CHANNELS {
            return;
        }
        for pattern in self.patterns.iter_mut().take(self.active_patterns) {
            pattern.channels[channel].clear();
        }
    }

    /// Replace musical state without growing any realtime-owned allocation.
    ///
    /// Runs off the audio thread (the executor installs a `RenderState` that
    /// is already built), so it is also where [`LanePool`] is refilled for
    /// the lanes the user will draw before the next install.
    pub fn load_project(&mut self, project: &Project) {
        self.lane_pool.refill();
        self.active_patterns = project.pattern_lengths.len().clamp(1, self.patterns.len());
        self.active_channels = project.channels.len().min(MAX_CHANNELS);
        self.current = (project.current_pattern as usize).min(self.active_patterns - 1);
        self.playback_mode = project.playback_mode;
        self.set_swing(project.swing_percent);
        self.playlist.clear();
        self.playlist.extend(
            project
                .playlist
                .iter()
                .copied()
                .take(self.playlist.capacity()),
        );
        self.playlist.sort_unstable();
        // Two identical placements schedule the same pattern twice at the
        // same offset, and `instance_offset` is derived from the pattern and
        // the start tick -- so both NoteOns carry the *same* voice id and one
        // NoteOff half-releases them. `set_playlist_placement` refuses a
        // duplicate; the load path is the way one gets in.
        self.playlist.dedup();
        self.rebuild_playlist_by_start();

        for pattern in &mut self.patterns {
            pattern.set_length_steps(DEFAULT_STEPS as usize);
            for channel in &mut pattern.channels {
                channel.clear();
            }
        }
        // Both banks are preallocated and both counts were clamped above, so
        // take the same bound here: `load_bundle` can be driven without the
        // integrity pass that refuses an oversized file, and an unclamped
        // index into a realtime-owned array panics on the audio thread.
        for (pattern_index, length) in project
            .pattern_lengths
            .iter()
            .enumerate()
            .take(self.patterns.len())
        {
            self.patterns[pattern_index].set_length_steps(*length as usize);
        }
        for (channel_index, channel) in project.channels.iter().enumerate().take(MAX_CHANNELS) {
            for (pattern_index, notes) in
                channel.notes.iter().enumerate().take(self.active_patterns)
            {
                let lane = &mut self.patterns[pattern_index].channels[channel_index];
                for note in notes.iter().copied() {
                    let _ = lane.upsert_note(note);
                }
            }
            for (pattern_index, lanes) in channel
                .automation
                .iter()
                .enumerate()
                .take(self.active_patterns)
            {
                self.patterns[pattern_index].channels[channel_index].set_lanes(lanes.clone());
            }
        }
    }

    pub fn pattern_length_ticks(&self, pattern: usize) -> Option<u32> {
        (pattern < self.active_patterns).then(|| self.patterns[pattern].length_ticks())
    }

    pub fn upsert_note(&mut self, pattern: usize, channel: usize, note: NoteEvent) -> bool {
        (pattern < self.active_patterns)
            .then(|| &mut self.patterns[pattern])
            .and_then(|pattern| pattern.channel_mut(channel))
            .is_some_and(|channel| channel.upsert_note(note))
    }

    /// The note `id` as `pattern` stores it on `channel`, if it does.
    pub fn note(&self, pattern: usize, channel: usize, id: NoteId) -> Option<NoteEvent> {
        (pattern < self.active_patterns)
            .then(|| &self.patterns[pattern])
            .and_then(|pattern| pattern.channel(channel))
            .and_then(|channel| channel.note(id))
            .copied()
    }

    /// Which pattern, and in Song mode which placement, scheduled the voice
    /// `id` -- asked as the voice starts, so Pattern mode's answer is the
    /// pattern being scheduled now (MOO-99, `crate::voices`).
    ///
    /// Song mode decodes it from the id's instance half, which
    /// [`Self::schedule_song`] builds as `lap * stride + pattern *
    /// MAX_PLAYLIST_TICKS + placement start`. That half is 32 bits wide, so
    /// after about 680 laps of a song the lap count wraps into the placement
    /// and this answer is wrong; what a wrong answer costs is one targeted
    /// release that misses, which a fold, a stop or a seek still catches.
    pub fn voice_origin(&self, id: u64) -> crate::voices::VoiceOrigin {
        match self.playback_mode {
            PlaybackMode::Pattern => crate::voices::VoiceOrigin {
                pattern: self.current as u8,
                placement: None,
            },
            PlaybackMode::Song => {
                let stride = u64::from(MAX_PLAYLIST_TICKS) * self.patterns.len().max(1) as u64;
                let offset = (id >> 32) % stride;
                crate::voices::VoiceOrigin {
                    pattern: (offset / u64::from(MAX_PLAYLIST_TICKS)) as u8,
                    placement: Some((offset % u64::from(MAX_PLAYLIST_TICKS)) as u32),
                }
            }
        }
    }

    pub fn remove_note(&mut self, pattern: usize, channel: usize, id: NoteId) -> bool {
        (pattern < self.active_patterns)
            .then(|| &mut self.patterns[pattern])
            .and_then(|pattern| pattern.channel_mut(channel))
            .and_then(|channel| channel.remove_note(id))
            .is_some()
    }

    fn channel_pattern_mut(
        &mut self,
        pattern: usize,
        channel: usize,
    ) -> Option<&mut mooloop_core::pattern::ChannelPattern> {
        (pattern < self.active_patterns)
            .then(|| &mut self.patterns[pattern])
            .and_then(|pattern| pattern.channel_mut(channel))
    }

    /// The channel's clip together with the lane storage opening one needs.
    /// Two disjoint fields, handed out as a pair because `open_lane` takes
    /// both and neither can be reached through the other.
    fn channel_pattern_and_pool(
        &mut self,
        pattern: usize,
        channel: usize,
    ) -> Option<(&mut mooloop_core::pattern::ChannelPattern, &mut LanePool)> {
        if pattern >= self.active_patterns {
            return None;
        }
        let channel = self.patterns[pattern].channel_mut(channel)?;
        Some((channel, &mut self.lane_pool))
    }

    pub fn open_automation_lane(
        &mut self,
        pattern: usize,
        channel: usize,
        target: ParamAddr,
    ) -> bool {
        self.channel_pattern_and_pool(pattern, channel)
            .and_then(|(channel, pool)| channel.open_lane(target, pool))
            .is_some()
    }

    pub fn remove_automation_lane(
        &mut self,
        pattern: usize,
        channel: usize,
        target: ParamAddr,
    ) -> bool {
        self.channel_pattern_mut(pattern, channel)
            .is_some_and(|channel| channel.remove_lane(target))
    }

    pub fn clear_automation_lane(
        &mut self,
        pattern: usize,
        channel: usize,
        target: ParamAddr,
    ) -> bool {
        let Some(lane) = self
            .channel_pattern_mut(pattern, channel)
            .and_then(|channel| channel.lane_mut(target))
        else {
            return false;
        };
        lane.clear();
        true
    }

    /// Insert or replace a breakpoint, opening the lane if the editor has not
    /// already asked for it.
    pub fn upsert_automation_point(
        &mut self,
        pattern: usize,
        channel: usize,
        target: ParamAddr,
        point: AutomationPoint,
    ) -> bool {
        self.channel_pattern_and_pool(pattern, channel)
            .and_then(|(channel, pool)| channel.open_lane(target, pool))
            .is_some_and(|lane| lane.upsert(point))
    }

    pub fn remove_automation_point(
        &mut self,
        pattern: usize,
        channel: usize,
        target: ParamAddr,
        id: PointId,
    ) -> bool {
        self.channel_pattern_mut(pattern, channel)
            .and_then(|channel| channel.lane_mut(target))
            .and_then(|lane| lane.remove(id))
            .is_some()
    }

    /// Drop every lane driving `device` in `scope`, in every pattern, because
    /// that device has been removed. A channel's chain is only ever addressed
    /// from that channel's clips; a bus chain can be addressed from any of
    /// them.
    ///
    /// A reorder needs no equivalent any more. A lane names a device
    /// identity, and an identity does not move.
    pub fn forget_device(&mut self, scope: EffectTarget, device: DeviceId) {
        let channels = self.active_channels;
        // Bounded by what the song actually holds. The bank is preallocated
        // to its ceiling, and this runs on the realtime command drain.
        let patterns = self.active_patterns;
        for pattern in self.patterns.iter_mut().take(patterns) {
            match scope {
                EffectTarget::Channel(channel) => {
                    if let Some(channel) = pattern.channel_mut(channel as usize) {
                        channel.forget_device(scope, device);
                    }
                }
                EffectTarget::Bus(_) => {
                    for channel in 0..channels {
                        if let Some(channel) = pattern.channel_mut(channel) {
                            channel.forget_device(scope, device);
                        }
                    }
                }
            }
        }
    }

    /// Which pattern the pattern-mode playhead is in.
    pub fn current_pattern(&self) -> usize {
        self.current
    }

    /// Which pattern a note played at `song_tick` belongs in -- the selected
    /// one -- and where in it, or `None` when the selected pattern is not
    /// what is playing there.
    ///
    /// The transport never folds in pattern mode -- scheduling wraps its own
    /// copy of the position -- so a recorder that reported the playhead as it
    /// stands would report tick 400 of a 384-tick pattern on the second pass.
    /// Pattern mode folds with the same [`wrap_tick`] scheduling uses. Song
    /// mode answers the offset into the placement of the selected pattern
    /// that covers the playhead, taking the latest-starting one where two
    /// overlap, the rule [`Self::automation_lane_at`] follows. Where no
    /// placement of it covers the playhead there is nowhere to record: the
    /// note would otherwise land in a pattern at a position that was never
    /// heard against it.
    pub fn recording_tick(&self, song_tick: f64) -> Option<(usize, u32)> {
        let length = self.pattern_length_ticks(self.current)?;
        let tick = match self.playback_mode {
            PlaybackMode::Pattern => Some(wrap_tick(song_tick, length) as u32),
            PlaybackMode::Song => {
                let position = wrap_tick(song_tick, self.song_length_ticks());
                self.playlist
                    .iter()
                    .rev()
                    .filter(|placement| placement.pattern as usize == self.current)
                    .map(|placement| position - f64::from(placement.start_tick))
                    .find(|offset| (0.0..f64::from(length)).contains(offset))
                    .map(|offset| offset as u32)
            }
        };
        tick.map(|tick| (self.current, tick))
    }

    /// The `ordinal`-th pattern covering a position, for a caller that has to
    /// ask about a position it has already left.
    ///
    /// Takes the mode, the pattern-mode selection and the tick rather than
    /// reading its own three, because the caller is
    /// `RenderState::restore_lanes_left_behind`, which asks *after* applying
    /// the command that moved the playhead: the outgoing coverage is what has
    /// to hand its destinations back.
    ///
    /// Indexed rather than returning an iterator for the same reason
    /// [`Self::pattern_lane_destination`] is: the caller holds `&mut
    /// RenderState` across each answer.
    pub fn covering_pattern_at(
        &self,
        mode: PlaybackMode,
        current: usize,
        song_tick: f64,
        ordinal: usize,
    ) -> Option<usize> {
        match mode {
            PlaybackMode::Pattern => {
                (ordinal == 0 && current < self.active_patterns).then_some(current)
            }
            PlaybackMode::Song => {
                let position = wrap_tick(song_tick, self.song_length_ticks());
                self.playlist
                    .iter()
                    .filter(|placement| {
                        let index = placement.pattern as usize;
                        if index >= self.active_patterns {
                            return false;
                        }
                        let start = placement.start_tick;
                        let length = self.patterns[index].length_ticks();
                        position >= start as f64
                            && position < start.saturating_add(length) as f64
                    })
                    .nth(ordinal)
                    .map(|placement| placement.pattern as usize)
            }
        }
    }

    /// The destination of one lane of one channel of `pattern`, by position.
    ///
    /// An empty lane answers `None`, matching the filter
    /// [`Self::automation_lane_at`] applies: a lane that was opened and then
    /// cleared drives nothing, so it has nothing to hand back either.
    pub fn pattern_lane_destination(
        &self,
        pattern: usize,
        channel: usize,
        lane: usize,
    ) -> Option<ParamAddr> {
        let lane = self
            .patterns
            .get(pattern)?
            .channel(channel)?
            .lanes()
            .get(lane)?;
        (!lane.is_empty()).then_some(lane.target)
    }

    /// Whether anything under the playhead could resolve a lane at all.
    ///
    /// [`Self::automation_lane_at`] answers one destination, and the engine
    /// asks it for every descriptor of every device on every live channel,
    /// once a block. On a song with no automation drawn -- which is most
    /// songs, and all of them until somebody draws one -- every one of those
    /// questions walks every active channel's lane list to arrive at `None`,
    /// so the cost of automation nobody authored is the descriptor count
    /// times the channel count, squared into the channel count again by the
    /// outer loop. This is the same walk done once, hoisted to the top of the
    /// block: exactly one of its answers can be `true`, and while it is
    /// `false` no destination needs asking.
    pub fn has_automation_at(&self, song_tick: f64) -> bool {
        match self.playback_mode {
            PlaybackMode::Pattern => self
                .patterns
                .get(self.current)
                .is_some_and(|pattern| self.pattern_has_lanes(pattern)),
            PlaybackMode::Song => {
                let position = wrap_tick(song_tick, self.song_length_ticks());
                self.playlist.iter().any(|placement| {
                    let pattern_index = placement.pattern as usize;
                    if pattern_index >= self.active_patterns {
                        return false;
                    }
                    let pattern = &self.patterns[pattern_index];
                    let start = placement.start_tick;
                    if position < start as f64
                        || position >= start.saturating_add(pattern.length_ticks()) as f64
                    {
                        return false;
                    }
                    self.pattern_has_lanes(pattern)
                })
            }
        }
    }

    /// Whether any active channel of `pattern` holds a lane with points in it.
    /// The emptiness filter matches [`Self::automation_lane_at`]'s: a lane
    /// that was opened and then cleared resolves to nothing there, so it must
    /// not count as automation here either.
    fn pattern_has_lanes(&self, pattern: &Pattern) -> bool {
        (0..self.active_channels)
            .filter_map(|channel| pattern.channel(channel))
            .any(|channel| channel.lanes().iter().any(|lane| !lane.is_empty()))
    }

    /// Resolve `target` to the lane driving it at `song_tick`, together with
    /// that lane's pattern-local tick and its pattern's length.
    ///
    /// The engine calls this once per automated destination per block and then
    /// walks the lane itself at the control rate, so the per-tick cost is one
    /// binary search rather than one lane search.
    ///
    /// A lane lives in the clip that drew it but may address a bus, so this
    /// searches every active channel rather than taking one. Two clips
    /// automating one destination is a UI-level mistake; the lowest channel
    /// wins here rather than the two summing into something neither drew.
    /// In song mode, layered placements resolve the same way notes do, except
    /// that only one can supply a value: the latest-starting cover wins.
    pub fn automation_lane_at(
        &self,
        target: ParamAddr,
        song_tick: f64,
    ) -> Option<(&AutomationLane, f64, u32)> {
        match self.playback_mode {
            PlaybackMode::Pattern => {
                let pattern = self.patterns.get(self.current)?;
                let length = pattern.length_ticks();
                let lane = (0..self.active_channels)
                    .filter_map(|channel| pattern.channel(channel))
                    .find_map(|channel| channel.lane(target))
                    .filter(|lane| !lane.is_empty())?;
                Some((lane, wrap_tick(song_tick, length), length))
            }
            PlaybackMode::Song => {
                let position = wrap_tick(song_tick, self.song_length_ticks());
                // Every placement covering `position` starts at or before
                // it and starts no more than one pattern pass earlier --
                // the same widened, index-bounded search `schedule_song`
                // runs for a span, here for a point.
                let hi = self
                    .playlist_by_start
                    .partition_point(|item| f64::from(item.start_tick) <= position);
                let lo_tick = (position - f64::from(MAX_PATTERN_TICKS)).max(0.0);
                let lo = self
                    .playlist_by_start
                    .partition_point(|item| f64::from(item.start_tick) < lo_tick);
                let mut best: Option<(&AutomationLane, f64, u32)> = None;
                let mut best_start = 0u32;
                for placement in &self.playlist_by_start[lo..hi] {
                    let pattern_index = placement.pattern as usize;
                    if pattern_index >= self.active_patterns {
                        continue;
                    }
                    let pattern = &self.patterns[pattern_index];
                    let length = pattern.length_ticks();
                    let start = placement.start_tick;
                    if position < start as f64
                        || position >= start.saturating_add(length) as f64
                    {
                        continue;
                    }
                    let Some(lane) = (0..self.active_channels)
                        .filter_map(|channel| pattern.channel(channel))
                        .find_map(|channel| channel.lane(target))
                        .filter(|lane| !lane.is_empty())
                    else {
                        continue;
                    };
                    if best.is_some() && start < best_start {
                        continue;
                    }
                    best = Some((lane, position - start as f64, length));
                    best_start = start;
                }
                best
            }
        }
    }

    /// Compatibility edit for the rack while it still addresses one anchor
    /// note per sixteenth. The canonical storage remains tick-addressed.
    pub fn set_step(
        &mut self,
        pattern: usize,
        channel: usize,
        step: usize,
        on: bool,
        note: u8,
        velocity: u8,
    ) {
        let id = step as NoteId + 1;
        if on {
            self.upsert_note(
                pattern,
                channel,
                NoteEvent::new(
                    id,
                    (step as u32).saturating_mul(TICKS_PER_STEP),
                    DEFAULT_NOTE_DURATION_TICKS,
                    note,
                    velocity,
                ),
            );
        } else {
            self.remove_note(pattern, channel, id);
        }
    }

    /// Schedule note starts and ends in `[start_tick, end_tick)`. Equal-time
    /// events are ordered NoteOff before NoteOn by `EventList::push_ordered`.
    ///
    /// `frame_offset` and `frames` describe the stretch of the process block
    /// this musical range occupies, which is the whole block except when a
    /// song loop cuts it: a block spanning the loop point is scheduled twice,
    /// each half against its own ticks and its own frames. Offsets are
    /// produced absolute within the block either way, since that is what the
    /// event list holds.
    pub fn schedule(
        &self,
        start_tick: f64,
        end_tick: f64,
        frame_offset: usize,
        frames: usize,
        ticks_per_sample: f64,
        events: &mut [Box<EventList>],
    ) {
        if frames == 0
            || !start_tick.is_finite()
            || !end_tick.is_finite()
            || !ticks_per_sample.is_finite()
            || ticks_per_sample <= 0.0
            || end_tick <= start_tick
        {
            return;
        }
        let window = FrameWindow {
            offset: frame_offset,
            frames,
        };
        match self.playback_mode {
            PlaybackMode::Pattern => {
                self.schedule_pattern(start_tick, end_tick, window, ticks_per_sample, events)
            }
            PlaybackMode::Song => {
                self.schedule_song(start_tick, end_tick, window, ticks_per_sample, events)
            }
        }
    }

    /// Which playback mode is installed. The render layer asks because a song
    /// loop is an arrangement feature and must not fold a pattern's own.
    pub fn playback_mode(&self) -> PlaybackMode {
        self.playback_mode
    }

    /// Schedule a finite pass without wrapping at the pattern/song boundary.
    pub fn schedule_once(
        &self,
        start_tick: f64,
        end_tick: f64,
        frames: usize,
        ticks_per_sample: f64,
        events: &mut [Box<EventList>],
    ) {
        if frames == 0 || end_tick <= start_tick || ticks_per_sample <= 0.0 {
            return;
        }
        let swing_max = swing_offset_ticks(TICKS_PER_STEP, MAX_SWING_PERCENT);
        match self.playback_mode {
            PlaybackMode::Pattern => {
                let Some(pattern) = self.patterns.get(self.current) else {
                    return;
                };
                let pattern_ticks = pattern.length_ticks();
                let swing_percent = self.swing_for_pattern(self.current);
                for (channel_index, event_list) in
                    events.iter_mut().enumerate().take(self.active_channels)
                {
                    let Some(channel) = pattern.channel(channel_index) else {
                        continue;
                    };
                    Self::schedule_channel_once(
                        channel,
                        pattern_ticks,
                        0,
                        0,
                        swing_percent,
                        swing_max,
                        start_tick,
                        end_tick,
                        frames,
                        ticks_per_sample,
                        event_list,
                    );
                }
            }
            PlaybackMode::Song => {
                // No wraparound here (`schedule_once` is the no-loop, no-cut
                // fast path -- see its callers in `render.rs`), so a
                // placement is a candidate exactly when its own reach
                // overlaps the (swing-widened) window once, not per repeat.
                //
                // Walked in `playlist`'s own order, not the start-sorted
                // view: two placements can each contribute an event at the
                // very same offset, and `push_ordered` keeps ties in
                // insertion order, so this has to visit them in the order
                // the original full walk did. The skip below is what keeps
                // that walk cheap -- `playlist_by_start` stays for
                // `automation_lane_at`, whose per-destination search has no
                // insertion order to preserve.
                let reach = MAX_PATTERN_TICKS.saturating_add(self.max_note_overshoot());
                for placement in &self.playlist {
                    let pattern_index = placement.pattern as usize;
                    if pattern_index >= self.active_patterns {
                        continue;
                    }
                    let anchor = f64::from(placement.start_tick);
                    if anchor >= end_tick
                        || anchor + f64::from(reach) <= start_tick - f64::from(swing_max)
                    {
                        continue;
                    }
                    let pattern = &self.patterns[pattern_index];
                    let swing_percent = self.swing_for_pattern(pattern_index);
                    let pattern_ticks = pattern.length_ticks();
                    let instance = u64::from(placement.pattern) * u64::from(MAX_PLAYLIST_TICKS)
                        + u64::from(placement.start_tick);
                    for (channel_index, event_list) in
                        events.iter_mut().enumerate().take(self.active_channels)
                    {
                        let Some(channel) = pattern.channel(channel_index) else {
                            continue;
                        };
                        Self::schedule_channel_once(
                            channel,
                            pattern_ticks,
                            placement.start_tick,
                            instance,
                            swing_percent,
                            swing_max,
                            start_tick,
                            end_tick,
                            frames,
                            ticks_per_sample,
                            event_list,
                        );
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn schedule_edge_once(
        note: NoteEvent,
        edge_tick: u32,
        is_note_off: bool,
        instance: u64,
        start_tick: f64,
        end_tick: f64,
        frames: usize,
        ticks_per_sample: f64,
        event_list: &mut EventList,
    ) {
        let tick = f64::from(edge_tick);
        if tick < start_tick || tick >= end_tick {
            return;
        }
        let offset = ((tick - start_tick) / ticks_per_sample).round() as i64;
        let offset = offset.clamp(0, frames as i64 - 1) as u32;
        let id = (instance << 32) | u64::from(note.id);
        event_list.push_ordered(TimedEvent {
            offset,
            event: if is_note_off {
                Event::NoteOff {
                    id,
                    note: note.note,
                }
            } else {
                Event::NoteOn {
                    id,
                    note: note.note,
                    velocity: note.velocity,
                }
            },
        });
    }

    /// [`Self::schedule_once`]'s per-channel body: candidates from the
    /// note-on/note-off stores instead of a full walk, bounded to
    /// `[start_tick, end_tick)` (widened for swing on the low side, since
    /// swing only ever delays an edge -- `swing_offset_ticks` never
    /// subtracts) with no wraparound to account for, because a finite pass
    /// has none. `anchor` and `instance` carry a song placement's offset and
    /// voice-id contribution; both are `0` for the pattern-mode playhead.
    #[allow(clippy::too_many_arguments)]
    fn schedule_channel_once(
        channel: &mooloop_core::pattern::ChannelPattern,
        pattern_ticks: u32,
        anchor: u32,
        instance: u64,
        swing_percent: u8,
        swing_max: u32,
        start_tick: f64,
        end_tick: f64,
        frames: usize,
        ticks_per_sample: f64,
        event_list: &mut EventList,
    ) {
        let anchor_f = f64::from(anchor);
        let lo = (start_tick - anchor_f - f64::from(swing_max))
            .floor()
            .max(0.0) as u32;
        let hi = (end_tick - anchor_f).ceil().max(0.0) as u32;

        for note in channel.notes_starting_in(lo..hi.min(pattern_ticks)) {
            let swing = swing_offset_ticks(note.start_tick, swing_percent);
            Self::schedule_edge_once(
                *note,
                anchor.saturating_add(note.start_tick).saturating_add(swing),
                false,
                instance,
                start_tick,
                end_tick,
                frames,
                ticks_per_sample,
                event_list,
            );
        }

        // `notes_ending_in` answers in end-tick order, not `notes()`'s own
        // `(start_tick, id)` order -- so its candidates are marked here and
        // then walked back through `notes()` below, in the order the walk
        // this replaces visited every note in, since `push_ordered` keeps
        // same-offset ties in the order they were pushed.
        let mut off_candidate = [false; MAX_NOTES_PER_CHANNEL_PATTERN];
        for index in channel.note_indices_ending_in(lo..hi) {
            off_candidate[index] = true;
        }
        for (index, note) in channel.notes().iter().enumerate() {
            if off_candidate[index] && note.start_tick < pattern_ticks {
                let swing = swing_offset_ticks(note.start_tick, swing_percent);
                Self::schedule_edge_once(
                    *note,
                    anchor.saturating_add(note.end_tick()).saturating_add(swing),
                    true,
                    instance,
                    start_tick,
                    end_tick,
                    frames,
                    ticks_per_sample,
                    event_list,
                );
            }
        }
    }

    fn schedule_pattern(
        &self,
        start_tick: f64,
        end_tick: f64,
        window: FrameWindow,
        ticks_per_sample: f64,
        events: &mut [Box<EventList>],
    ) {
        let Some(pattern) = self.patterns.get(self.current) else {
            return;
        };
        let pattern_ticks = pattern.length_ticks();
        let swing_percent = self.swing_for_pattern(self.current);
        let swing_max = swing_offset_ticks(TICKS_PER_STEP, MAX_SWING_PERCENT);

        for (channel_index, event_list) in events.iter_mut().enumerate().take(self.active_channels)
        {
            let Some(channel) = pattern.channel(channel_index) else {
                continue;
            };
            Self::schedule_channel_pass(
                channel,
                pattern_ticks,
                pattern_ticks,
                0,
                1,
                0,
                swing_percent,
                swing_max,
                start_tick,
                end_tick,
                window,
                ticks_per_sample,
                event_list,
            );
        }
    }

    fn schedule_song(
        &self,
        start_tick: f64,
        end_tick: f64,
        window: FrameWindow,
        ticks_per_sample: f64,
        events: &mut [Box<EventList>],
    ) {
        let song_ticks = self.song_length_ticks();
        let instance_stride = MAX_PLAYLIST_TICKS as u64 * self.patterns.len() as u64;
        let swing_max = swing_offset_ticks(TICKS_PER_STEP, MAX_SWING_PERCENT);

        // A placement is a candidate when its reach -- its pattern's
        // length, plus however far any note in it sustains past that
        // length, since nothing caps a note's duration to its pattern's --
        // overlaps the (swing-widened) window on any repeat of the song.
        //
        // Walked in `playlist`'s own order, not the start-sorted view: two
        // placements can each contribute an event at the very same offset,
        // and `push_ordered` keeps ties in insertion order, so this has to
        // visit them in the order the original full walk did (Plan A's
        // first version used `playlist_by_start` here and the property test
        // caught the reordering it caused). The `window_reaches` check
        // below is what keeps that walk cheap -- `playlist_by_start` stays
        // for `automation_lane_at`, whose per-destination search has no
        // insertion order to preserve.
        let reach = MAX_PATTERN_TICKS.saturating_add(self.max_note_overshoot());
        for placement in &self.playlist {
            let pattern_index = placement.pattern as usize;
            if pattern_index >= self.active_patterns {
                continue;
            }
            let anchor = f64::from(placement.start_tick);
            let q_lo = (start_tick - anchor - f64::from(swing_max)).floor() as i64;
            let q_hi = (end_tick - anchor).ceil() as i64;
            if !window_reaches(q_lo, q_hi, song_ticks, reach) {
                continue;
            }
            let pattern = &self.patterns[pattern_index];
            let swing_percent = self.swing_for_pattern(pattern_index);
            let instance_offset = placement.pattern as u64 * MAX_PLAYLIST_TICKS as u64
                + u64::from(placement.start_tick);
            let pattern_ticks = pattern.length_ticks();
            for (channel_index, event_list) in
                events.iter_mut().enumerate().take(self.active_channels)
            {
                let Some(channel) = pattern.channel(channel_index) else {
                    continue;
                };
                Self::schedule_channel_pass(
                    channel,
                    pattern_ticks,
                    song_ticks,
                    placement.start_tick,
                    instance_stride,
                    instance_offset,
                    swing_percent,
                    swing_max,
                    start_tick,
                    end_tick,
                    window,
                    ticks_per_sample,
                    event_list,
                );
            }
        }
    }

    /// The most any note's raw end tick currently reaches past its own
    /// pattern's declared length, across every pattern and channel the
    /// song can place. Nothing caps a note's duration to its pattern's
    /// length, so a placement's own reach past `start + pattern_ticks` is
    /// exactly this -- `schedule_song` and `schedule_once`'s Song arm widen
    /// their placement search by it, on top of [`MAX_PATTERN_TICKS`], so a
    /// placement holding a long-sustaining note is never missed as a
    /// candidate (`reports/fable-2026-09-22.md`, finding 1, Plan A: the bug
    /// the property test caught before this existed).
    ///
    /// `O(active_patterns * active_channels)`, not the note count -- cheap
    /// next to the per-block walk this whole scheme replaces, and
    /// independent of it.
    fn max_note_overshoot(&self) -> u32 {
        (0..self.active_patterns)
            .filter_map(|pattern_index| self.patterns.get(pattern_index))
            .flat_map(|pattern| {
                let pattern_ticks = pattern.length_ticks();
                let channels = self.active_channels;
                (0..channels)
                    .filter_map(move |channel_index| pattern.channel(channel_index))
                    .filter_map(move |channel| channel.max_end_tick())
                    .map(move |max_end| max_end.saturating_sub(pattern_ticks))
            })
            .max()
            .unwrap_or(0)
    }

    /// One channel's notes for one pattern pass -- the pattern-mode
    /// playhead itself, or one song placement -- scheduled from the
    /// note-on/note-off stores instead of a full walk
    /// (`reports/fable-2026-09-22.md`, finding 1, Plan A).
    ///
    /// `pattern_ticks` bounds which notes are in play this pass: a note
    /// whose own start has drifted past a shortened pattern is stored but
    /// silent, the filter the walk this replaces applied with
    /// `.filter(|note| note.start_tick < pattern_ticks)`. `period_ticks` is
    /// what the *edges* repeat on -- the pattern's own length in Pattern
    /// mode, the whole song's in Song mode, where a placement plays once
    /// per song pass rather than once per pattern pass. `anchor` is the
    /// absolute tick this pass's local tick `0` sits at: `0` for the
    /// pattern-mode playhead, a placement's `start_tick` in Song mode.
    #[allow(clippy::too_many_arguments)]
    fn schedule_channel_pass(
        channel: &mooloop_core::pattern::ChannelPattern,
        pattern_ticks: u32,
        period_ticks: u32,
        anchor: u32,
        instance_stride: u64,
        instance_offset: u64,
        swing_percent: u8,
        swing_max: u32,
        start_tick: f64,
        end_tick: f64,
        window: FrameWindow,
        ticks_per_sample: f64,
        event_list: &mut EventList,
    ) {
        let anchor_f = f64::from(anchor);
        let q_lo = (start_tick - anchor_f - f64::from(swing_max)).floor() as i64;
        let q_hi = (end_tick - anchor_f).ceil() as i64;
        let reduced = reduce_window(q_lo, q_hi, period_ticks);

        let Some((r1, r2)) = reduced else {
            // The span covers a whole pass; every note is a candidate,
            // exactly the walk this replaces.
            for note in channel
                .notes()
                .iter()
                .copied()
                .filter(|note| note.start_tick < pattern_ticks)
            {
                Self::schedule_local_edge(
                    note, false, anchor, swing_percent, period_ticks, instance_stride,
                    instance_offset, start_tick, end_tick, window, ticks_per_sample, event_list,
                );
                Self::schedule_local_edge(
                    note, true, anchor, swing_percent, period_ticks, instance_stride,
                    instance_offset, start_tick, end_tick, window, ticks_per_sample, event_list,
                );
            }
            return;
        };

        // Ascending `(start_tick, id)` order -- the order the walk this
        // replaces visited every note in, which `push_ordered` needs
        // reproduced exactly for two notes that tie at the same offset.
        // `r2`, when present, is always the *lower* tick range (`reduce_window`
        // only ever splits as `lo..period` then `0..overflow`, and
        // `overflow <= lo` is exactly what makes the split correct), so it
        // has to be walked before `r1` to stay in that order.
        for range in [r2.clone(), Some(r1.clone())].into_iter().flatten() {
            for note in channel.notes_starting_in(range) {
                if note.start_tick < pattern_ticks {
                    Self::schedule_local_edge(
                        *note, false, anchor, swing_percent, period_ticks, instance_stride,
                        instance_offset, start_tick, end_tick, window, ticks_per_sample,
                        event_list,
                    );
                }
            }
        }

        // A note-off's raw end tick is unbounded (nothing caps duration to
        // the pattern's length), so the end-tick index cannot be trusted to
        // a single reduced range the way the start-tick one can: a note
        // that sustains less than one whole pass has its end tick in
        // `[0, 2 * period_ticks)`, covered by the reduced range and that
        // same range shifted forward by one period. A note that sustains
        // *more* than that falls back to the full walk, same as the
        // whole-pass case above -- rare (nothing authored inside one
        // pattern ordinarily lasts a whole extra pass), always correct, and
        // cheap even when it triggers since it is still one channel's worth
        // of notes, not the song's.
        let needs_fallback = channel
            .max_end_tick()
            .is_some_and(|max_end| max_end >= period_ticks.saturating_mul(2));
        if needs_fallback {
            for note in channel
                .notes()
                .iter()
                .copied()
                .filter(|note| note.start_tick < pattern_ticks)
            {
                Self::schedule_local_edge(
                    note, true, anchor, swing_percent, period_ticks, instance_stride,
                    instance_offset, start_tick, end_tick, window, ticks_per_sample, event_list,
                );
            }
        } else {
            // `notes_ending_in` answers in end-tick order, which is *not*
            // `notes()`'s own `(start_tick, id)` order (a note that starts
            // earlier can easily end later), so candidates are marked here
            // and the emission below walks `notes()` itself -- the same
            // fix `schedule_channel_once` needs, for the same reason.
            let mut off_candidate = [false; MAX_NOTES_PER_CHANNEL_PATTERN];
            for range in [Some(r1), r2].into_iter().flatten() {
                for shift in [0u32, period_ticks] {
                    let shifted =
                        range.start.saturating_add(shift)..range.end.saturating_add(shift);
                    for index in channel.note_indices_ending_in(shifted) {
                        off_candidate[index] = true;
                    }
                }
            }
            for (index, note) in channel.notes().iter().enumerate() {
                if off_candidate[index] && note.start_tick < pattern_ticks {
                    Self::schedule_local_edge(
                        *note, true, anchor, swing_percent, period_ticks, instance_stride,
                        instance_offset, start_tick, end_tick, window, ticks_per_sample,
                        event_list,
                    );
                }
            }
        }
    }

    /// One note-on or note-off edge, anchored and swung, via
    /// [`Self::schedule_note_edge`]'s unchanged cycle math. The shared tail
    /// of every branch in [`Self::schedule_channel_pass`].
    #[allow(clippy::too_many_arguments)]
    fn schedule_local_edge(
        note: NoteEvent,
        is_note_off: bool,
        anchor: u32,
        swing_percent: u8,
        period_ticks: u32,
        instance_stride: u64,
        instance_offset: u64,
        start_tick: f64,
        end_tick: f64,
        window: FrameWindow,
        ticks_per_sample: f64,
        event_list: &mut EventList,
    ) {
        let swing = swing_offset_ticks(note.start_tick, swing_percent);
        let local = if is_note_off {
            note.end_tick()
        } else {
            note.start_tick
        };
        Self::schedule_note_edge(
            note,
            anchor.saturating_add(local).saturating_add(swing),
            is_note_off,
            period_ticks,
            instance_stride,
            instance_offset,
            start_tick,
            end_tick,
            window,
            ticks_per_sample,
            event_list,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn schedule_note_edge(
        note: NoteEvent,
        edge_tick: u32,
        is_note_off: bool,
        period_ticks: u32,
        instance_stride: u64,
        instance_offset: u64,
        start_tick: f64,
        end_tick: f64,
        window: FrameWindow,
        ticks_per_sample: f64,
        event_list: &mut EventList,
    ) {
        let period = f64::from(period_ticks);
        let edge = f64::from(edge_tick);
        let mut cycle = ((start_tick - edge) / period).ceil() as i64;
        cycle = cycle.max(0);
        let mut absolute_tick = edge + cycle as f64 * period;

        while absolute_tick < end_tick {
            if absolute_tick >= start_tick {
                let offset = window.offset_for(absolute_tick - start_tick, ticks_per_sample);
                let instance = (cycle as u64)
                    .wrapping_mul(instance_stride)
                    .wrapping_add(instance_offset);
                let voice_id = (instance << 32) | u64::from(note.id);
                let event = if is_note_off {
                    Event::NoteOff {
                        id: voice_id,
                        note: note.note,
                    }
                } else {
                    Event::NoteOn {
                        id: voice_id,
                        note: note.note,
                        velocity: note.velocity,
                    }
                };
                event_list.push_ordered(TimedEvent { offset, event });
            }
            cycle += 1;
            absolute_tick += period;
        }
    }
}

/// Fold a transport position into `[0, period)`. The transport is monotonic
/// across loops, so every pattern-local read needs this.
fn wrap_tick(tick: f64, period_ticks: u32) -> f64 {
    if period_ticks == 0 {
        return 0.0;
    }
    let period = period_ticks as f64;
    let wrapped = tick % period;
    if wrapped < 0.0 {
        wrapped + period
    } else {
        wrapped
    }
}

fn swing_offset_ticks(note_start_tick: u32, percent: u8) -> u32 {
    if (note_start_tick / TICKS_PER_STEP).is_multiple_of(2) {
        return 0;
    }
    let amount = u32::from(percent.clamp(MIN_SWING_PERCENT, MAX_SWING_PERCENT))
        - u32::from(MIN_SWING_PERCENT);
    (TICKS_PER_STEP * 2 * amount + 50) / 100
}

/// Reduce an absolute tick window to up to two ranges inside `[0, period)`
/// -- the positions a value repeating on `period` could land on to
/// intersect it. `None` means the window's own span already covers a whole
/// period, so every position in `[0, period)` is a candidate: the fallback
/// the full walk this whole scheme replaces still is, for the pattern
/// lengths short enough (or blocks large enough) to reach it.
///
/// `q_lo`/`q_hi` are integers so that a window already widened for swing
/// cannot have its edges rounded past an in-range integer tick; the caller
/// floors the low edge and ceils the high one before calling this. Being
/// generous here is always safe -- every note this returns as a candidate
/// is still checked exactly by [`Sequencer::schedule_note_edge`], so an
/// extra one costs a rejected comparison and a missing one is the bug this
/// whole indexing scheme cannot afford
/// (`reports/fable-2026-09-22.md`, finding 1, Plan A).
fn reduce_window(q_lo: i64, q_hi: i64, period: u32) -> Option<(Range<u32>, Option<Range<u32>>)> {
    let period_i = i64::from(period);
    if period_i <= 0 || q_hi <= q_lo {
        return Some((0..0, None));
    }
    if q_hi - q_lo >= period_i {
        return None;
    }
    let lo = q_lo.rem_euclid(period_i);
    let hi = lo + (q_hi - q_lo);
    if hi <= period_i {
        Some((lo as u32..hi as u32, None))
    } else {
        Some((lo as u32..period, Some(0..(hi - period_i) as u32)))
    }
}

/// Whether a placement starting at local tick `0` and reaching `reach`
/// ticks -- `[0, reach)` -- could contribute an event to the window
/// `[q_lo, q_hi)` on any repeat of `period`. `schedule_song` and
/// `schedule_once`'s Song arm use this as a cheap per-placement skip
/// (walking `playlist` in its own order rather than pruning it with
/// `playlist_by_start`, which would reorder the ties `push_ordered` needs
/// kept in `playlist`'s order -- see `schedule_song`'s doc comment).
fn window_reaches(q_lo: i64, q_hi: i64, period: u32, reach: u32) -> bool {
    match reduce_window(q_lo, q_hi, period) {
        None => true,
        Some((r1, r2)) => {
            (r1.start < reach && r1.end > 0) || r2.is_some_and(|r2| r2.start < reach && r2.end > 0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{ticks_per_sample, TICKS_PER_64TH};

    const TEST_PLACEMENT_TICKS: u32 = TICKS_PER_BAR / 2;

    fn schedule_range(sequencer: &Sequencer, start_tick: f64, end_tick: f64) -> Vec<TimedEvent> {
        let ticks_per_sample = ticks_per_sample(120.0, 48_000, Ppq::DEFAULT);
        let frames = ((end_tick - start_tick) / ticks_per_sample).ceil() as usize;
        let mut events = [Box::new(EventList::empty())];
        sequencer.schedule(start_tick, end_tick, 0, frames, ticks_per_sample, &mut events);
        events[0].iter().copied().collect()
    }

    fn schedule_once_range(
        sequencer: &Sequencer,
        start_tick: f64,
        end_tick: f64,
    ) -> Vec<TimedEvent> {
        let ticks_per_sample = ticks_per_sample(120.0, 48_000, Ppq::DEFAULT);
        let frames = ((end_tick - start_tick) / ticks_per_sample).ceil() as usize;
        let mut events = [Box::new(EventList::empty())];
        sequencer.schedule_once(start_tick, end_tick, frames, ticks_per_sample, &mut events);
        events[0].iter().copied().collect()
    }

    #[test]
    fn patterns_become_addressable_only_after_creation() {
        let mut sequencer = Sequencer::new(1, 1, 16, Ppq::DEFAULT);
        assert_eq!(sequencer.active_patterns(), 1);
        assert!(!sequencer.upsert_note(1, 0, NoteEvent::new(1, 0, 24, 60, 100)));

        assert!(sequencer.add_pattern());
        assert_eq!(sequencer.active_patterns(), 2);
        assert!(sequencer.upsert_note(1, 0, NoteEvent::new(1, 0, 24, 60, 100)));
    }

    #[test]
    fn schedules_four_sixty_fourths_inside_one_rack_cell() {
        let mut sequencer = Sequencer::new(1, 1, 16, Ppq::DEFAULT);
        for substep in 0..4 {
            assert!(sequencer.upsert_note(
                0,
                0,
                NoteEvent::new(
                    substep + 1,
                    substep * TICKS_PER_64TH,
                    TICKS_PER_64TH,
                    60,
                    100,
                ),
            ));
        }

        let events = schedule_range(&sequencer, 0.0, f64::from(TICKS_PER_STEP));
        assert_eq!(
            events.len(),
            7,
            "the final note-off belongs to the next range"
        );
        let note_ons = events
            .iter()
            .filter(|event| matches!(event.event, Event::NoteOn { .. }))
            .count();
        assert_eq!(note_ons, 4);
        assert!(events
            .windows(2)
            .all(|pair| pair[0].offset <= pair[1].offset));
    }

    #[test]
    fn duration_schedules_a_sample_accurate_note_off() {
        let mut sequencer = Sequencer::new(1, 1, 16, Ppq::DEFAULT);
        assert!(sequencer.upsert_note(0, 0, NoteEvent::new(9, 6, 12, 64, 91)));
        let events = schedule_range(&sequencer, 0.0, 24.0);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0].event,
            Event::NoteOn {
                id: 9,
                note: 64,
                velocity: 91
            }
        ));
        assert!(matches!(
            events[1].event,
            Event::NoteOff { id: 9, note: 64 }
        ));
        assert!(events[1].offset > events[0].offset);
    }

    #[test]
    fn swing_delays_alternate_sixteenths_without_changing_duration() {
        let mut sequencer = Sequencer::new(1, 1, 16, Ppq::DEFAULT);
        sequencer.set_swing(66);
        assert!(sequencer.upsert_note(0, 0, NoteEvent::new(1, 0, 12, 60, 100)));
        assert!(sequencer.upsert_note(0, 0, NoteEvent::new(2, 24, 12, 60, 100)));

        for events in [
            schedule_range(&sequencer, 0.0, 48.0),
            schedule_once_range(&sequencer, 0.0, 48.0),
        ] {
            let swung_on = events
                .iter()
                .find(|event| matches!(event.event, Event::NoteOn { id: 2, .. }))
                .unwrap();
            let swung_off = events
                .iter()
                .find(|event| matches!(event.event, Event::NoteOff { id: 2, .. }))
                .unwrap();
            assert_eq!(swung_on.offset, 8_000);
            assert_eq!(swung_off.offset - swung_on.offset, 3_000);
        }
    }

    #[test]
    fn song_swing_uses_pattern_phase_not_playlist_position() {
        let mut sequencer = Sequencer::new(1, 1, 16, Ppq::DEFAULT);
        sequencer.set_swing(66);
        assert!(sequencer.upsert_note(0, 0, NoteEvent::new(1, 0, 6, 60, 100)));
        assert!(sequencer.upsert_note(0, 0, NoteEvent::new(2, 24, 6, 62, 100)));
        assert!(sequencer.set_playlist_placement(0, 24, true));
        sequencer.set_playback_mode(PlaybackMode::Song);

        let events = schedule_range(&sequencer, 0.0, 64.0);
        let note_on_offsets: Vec<_> = events
            .iter()
            .filter_map(|event| match event.event {
                Event::NoteOn { note, .. } => Some((note, event.offset)),
                _ => None,
            })
            .collect();
        assert_eq!(note_on_offsets, vec![(60, 6_000), (62, 14_000)]);
    }

    #[test]
    fn note_off_precedes_note_on_at_a_retrigger_boundary() {
        let mut sequencer = Sequencer::new(1, 1, 16, Ppq::DEFAULT);
        assert!(sequencer.upsert_note(0, 0, NoteEvent::new(1, 0, 6, 60, 100)));
        assert!(sequencer.upsert_note(0, 0, NoteEvent::new(2, 6, 6, 60, 100)));
        let events = schedule_range(&sequencer, 0.0, 12.0);
        assert!(matches!(events[1].event, Event::NoteOff { id: 1, .. }));
        assert!(matches!(events[2].event, Event::NoteOn { id: 2, .. }));
        assert_eq!(events[1].offset, events[2].offset);
    }

    #[test]
    fn boundary_at_block_start_fires_at_zero() {
        let mut sequencer = Sequencer::new(1, 1, 16, Ppq::DEFAULT);
        sequencer.set_step(0, 0, 0, true, 60, 100);
        let pattern_ticks = 16.0 * f64::from(TICKS_PER_STEP);
        for drift in [-0.5e-6, 0.0] {
            let events = schedule_range(&sequencer, pattern_ticks + drift, pattern_ticks + 2.0);
            assert_eq!(events[0].offset, 0, "drift {drift}");
            assert!(matches!(events[0].event, Event::NoteOn { .. }));
        }
    }

    #[test]
    fn loop_wrap_event_belongs_to_only_one_adjacent_block() {
        let mut sequencer = Sequencer::new(1, 1, 16, Ppq::DEFAULT);
        sequencer.set_step(0, 0, 0, true, 60, 100);
        let wrap = 16.0 * f64::from(TICKS_PER_STEP);
        let drift = 0.5e-6;

        let before = schedule_range(&sequencer, wrap - 2.0, wrap + drift);
        let after = schedule_range(&sequencer, wrap + drift, wrap + 2.0);
        let note_ons = before
            .iter()
            .chain(&after)
            .filter(|event| matches!(event.event, Event::NoteOn { .. }))
            .count();

        assert_eq!(
            note_ons, 1,
            "a loop-wrap NoteOn must not cross block ownership"
        );
    }

    #[test]
    fn song_loop_wrap_event_belongs_to_only_one_adjacent_block() {
        let mut sequencer = Sequencer::new(1, 1, 16, Ppq::DEFAULT);
        sequencer.set_step(0, 0, 0, true, 60, 100);
        assert!(sequencer.set_playlist_placement(0, 0, true));
        sequencer.set_playback_mode(PlaybackMode::Song);
        let wrap = f64::from(sequencer.song_length_ticks());
        let drift = 0.5e-6;

        let before = schedule_range(&sequencer, wrap - 2.0, wrap + drift);
        let after = schedule_range(&sequencer, wrap + drift, wrap + 2.0);
        let note_ons = before
            .iter()
            .chain(&after)
            .filter(|event| matches!(event.event, Event::NoteOn { .. }))
            .count();

        assert_eq!(
            note_ons, 1,
            "a song-loop NoteOn must not cross block ownership"
        );
    }

    #[test]
    fn pattern_bank_and_independent_lengths_are_respected() {
        let mut sequencer = Sequencer::new(1, 2, 16, Ppq::DEFAULT);
        sequencer.set_step(1, 0, 0, true, 48, 91);
        assert!(schedule_range(&sequencer, 0.0, 24.0).is_empty());

        sequencer.set_current_pattern(1);
        sequencer.set_pattern_length(1, 12);
        let wrap = 12.0 * f64::from(TICKS_PER_STEP);
        let events = schedule_range(&sequencer, wrap, wrap + 2.0);
        assert!(matches!(
            events[0].event,
            Event::NoteOn {
                note: 48,
                velocity: 91,
                ..
            }
        ));
    }

    #[test]
    fn playlist_placements_are_bounded_and_idempotent() {
        let mut sequencer = Sequencer::new(1, 2, 16, Ppq::DEFAULT);
        assert!(sequencer.set_playlist_placement(1, TEST_PLACEMENT_TICKS, true));
        assert!(!sequencer.set_playlist_placement(1, TEST_PLACEMENT_TICKS, true));
        assert!(!sequencer.set_playlist_placement(2, 0, true));
        assert!(!sequencer.set_playlist_placement(0, MAX_PLAYLIST_TICKS, true));
        assert!(sequencer.set_playlist_placement(1, TEST_PLACEMENT_TICKS, false));
        assert!(!sequencer.set_playlist_placement(1, TEST_PLACEMENT_TICKS, false));
    }

    /// A guard on the invariant `set_playlist_placement` now depends on
    /// rather than establishes, not a check for a defect: it passes on the
    /// tree before the `partition_point` insert as well as after, because
    /// nothing about the playlist a caller can see was meant to change. What
    /// it would catch is the insert landing at the wrong index, which a
    /// `sort_unstable` over the whole vector could not do and a hand-written
    /// insertion can.
    ///
    /// Painted deliberately out of order in both fields, since the order is
    /// pattern first and then tick: a range painted along the timeline on one
    /// pattern arrives in tick order, and a column painted across patterns at
    /// one tick arrives in pattern order, and the insert has to be right for
    /// both.
    #[test]
    fn playlist_stays_sorted_and_deduplicated_however_it_is_painted() {
        let mut sequencer = Sequencer::new(1, 4, 16, Ppq::DEFAULT);
        let painted = [
            (2u8, TICKS_PER_BAR * 3),
            (0, TICKS_PER_BAR),
            (3, 0),
            (0, 0),
            (2, TICKS_PER_BAR),
            (1, TICKS_PER_BAR * 2),
            (0, TICKS_PER_BAR * 3),
            (3, TICKS_PER_BAR * 2),
        ];
        for (pattern, tick) in painted {
            assert!(
                sequencer.set_playlist_placement(pattern as usize, tick, true),
                "painting {pattern} at {tick} changed nothing"
            );
            assert!(
                !sequencer.set_playlist_placement(pattern as usize, tick, true),
                "painting {pattern} at {tick} twice made a second placement"
            );
        }

        let expected = {
            let mut placements: Vec<_> = painted
                .iter()
                .map(|(pattern, tick)| PatternPlacement::new(*pattern, *tick))
                .collect();
            placements.sort_unstable();
            placements
        };
        assert_eq!(sequencer.playlist(), expected.as_slice());

        // Erasing from the middle, where a wrong index is a placement that
        // silently stops playing and another that silently keeps playing.
        assert!(sequencer.set_playlist_placement(2, TICKS_PER_BAR, false));
        assert!(!sequencer.set_playlist_placement(2, TICKS_PER_BAR, false));
        let expected: Vec<_> = expected
            .into_iter()
            .filter(|item| *item != PatternPlacement::new(2, TICKS_PER_BAR))
            .collect();
        assert_eq!(sequencer.playlist(), expected.as_slice());
        assert!(
            sequencer.playlist().windows(2).all(|pair| pair[0] < pair[1]),
            "the playlist is no longer strictly ordered: {:?}",
            sequencer.playlist()
        );
    }

    #[test]
    fn song_mode_layers_patterns_at_the_same_position() {
        let mut sequencer = Sequencer::new(1, 2, 16, Ppq::DEFAULT);
        assert!(sequencer.upsert_note(0, 0, NoteEvent::new(1, 0, 12, 60, 100)));
        assert!(sequencer.upsert_note(1, 0, NoteEvent::new(1, 0, 12, 72, 90)));
        assert!(sequencer.set_playlist_placement(0, 0, true));
        assert!(sequencer.set_playlist_placement(1, 0, true));
        sequencer.set_playback_mode(PlaybackMode::Song);

        let events = schedule_range(&sequencer, 0.0, 2.0);
        assert_eq!(events.len(), 2);
        assert!(events
            .iter()
            .any(|event| matches!(event.event, Event::NoteOn { note: 60, .. })));
        assert!(events
            .iter()
            .any(|event| matches!(event.event, Event::NoteOn { note: 72, .. })));
        let ids: Vec<_> = events
            .iter()
            .filter_map(|event| match event.event {
                Event::NoteOn { id, .. } => Some(id),
                _ => None,
            })
            .collect();
        assert_ne!(ids[0], ids[1], "layered placements need distinct voice IDs");
    }

    #[test]
    fn song_placement_can_start_on_a_half_bar() {
        let mut sequencer = Sequencer::new(1, 1, 16, Ppq::DEFAULT);
        sequencer.set_step(0, 0, 0, true, 60, 100);
        assert!(sequencer.set_playlist_placement(0, TEST_PLACEMENT_TICKS, true));
        sequencer.set_playback_mode(PlaybackMode::Song);

        assert!(schedule_range(&sequencer, 0.0, 2.0).is_empty());
        let events = schedule_range(
            &sequencer,
            f64::from(TEST_PLACEMENT_TICKS),
            f64::from(TEST_PLACEMENT_TICKS + 2),
        );
        assert_eq!(events[0].offset, 0);
        assert!(matches!(events[0].event, Event::NoteOn { .. }));
    }

    #[test]
    fn song_loop_follows_the_longest_pattern_placement() {
        let mut sequencer = Sequencer::new(1, 1, 16, Ppq::DEFAULT);
        sequencer.set_pattern_length(0, 32);
        sequencer.set_step(0, 0, 0, true, 60, 100);
        assert!(sequencer.set_playlist_placement(0, 0, true));
        sequencer.set_playback_mode(PlaybackMode::Song);
        assert_eq!(sequencer.song_length_ticks(), 32 * TICKS_PER_STEP);

        let wrap = f64::from(sequencer.song_length_ticks());
        let events = schedule_range(&sequencer, wrap, wrap + 2.0);
        assert_eq!(events[0].offset, 0);
        assert!(matches!(events[0].event, Event::NoteOn { .. }));
    }

    // -- Plan A property test -------------------------------------------
    //
    // `schedule`/`schedule_once` now answer from `notes_starting_in`/
    // `notes_ending_in` and the start-sorted playlist view instead of a
    // full walk. Both reference functions below call the *same*
    // `schedule_note_edge`/`schedule_edge_once` the fast path calls --
    // they just call it for every note (`.filter(|note| note.start_tick <
    // pattern_ticks)`, precisely the walk `reports/fable-2026-09-22.md`
    // describes), so a mismatch here can only be the candidate-selection
    // layer missing or duplicating a note, never a divergence in the edge
    // math itself.

    /// Tiny deterministic PRNG (xorshift64*) so the property test is
    /// reproducible without a `rand` dependency.
    struct Rng(u64);

    impl Rng {
        fn new(seed: u64) -> Self {
            Self(seed | 1)
        }

        fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        fn range_u32(&mut self, lo: u32, hi: u32) -> u32 {
            if hi <= lo {
                return lo;
            }
            lo + (self.next_u64() as u32) % (hi - lo)
        }

        fn range_f64(&mut self, lo: f64, hi: f64) -> f64 {
            lo + (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64) * (hi - lo)
        }

        fn chance(&mut self, percent: u32) -> bool {
            self.range_u32(0, 100) < percent
        }
    }

    fn dump(sequencer: &Sequencer) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        let _ = writeln!(out, "active_channels={} active_patterns={} current={} swing={}",
            sequencer.active_channels, sequencer.active_patterns, sequencer.current, sequencer.swing_percent);
        for p in 0..sequencer.active_patterns {
            let pattern = &sequencer.patterns[p];
            let _ = writeln!(out, "pattern {p}: length_ticks={}", pattern.length_ticks());
            for c in 0..sequencer.active_channels {
                if let Some(channel) = pattern.channel(c) {
                    let _ = writeln!(out, "  channel {c}: {:?}", channel.notes());
                }
            }
        }
        let _ = writeln!(out, "playlist={:?}", sequencer.playlist);
        out
    }

    fn empty_event_lists(channels: usize) -> Vec<Box<EventList>> {
        (0..channels).map(|_| Box::new(EventList::empty())).collect()
    }

    fn collect_lists(events: &[Box<EventList>]) -> Vec<Vec<TimedEvent>> {
        events
            .iter()
            .map(|list| list.iter().copied().collect())
            .collect()
    }

    fn real_schedule(
        sequencer: &Sequencer,
        start_tick: f64,
        end_tick: f64,
        frames: usize,
        ticks_per_sample: f64,
    ) -> Vec<Vec<TimedEvent>> {
        let mut events = empty_event_lists(sequencer.active_channels);
        sequencer.schedule(start_tick, end_tick, 0, frames, ticks_per_sample, &mut events);
        collect_lists(&events)
    }

    fn real_schedule_once(
        sequencer: &Sequencer,
        start_tick: f64,
        end_tick: f64,
        frames: usize,
        ticks_per_sample: f64,
    ) -> Vec<Vec<TimedEvent>> {
        let mut events = empty_event_lists(sequencer.active_channels);
        sequencer.schedule_once(start_tick, end_tick, frames, ticks_per_sample, &mut events);
        collect_lists(&events)
    }

    /// The full walk Plan A replaces, calling the same edge math the fast
    /// path does for every note rather than only the candidates the indexed
    /// stores narrow it to.
    fn brute_force_schedule(
        sequencer: &Sequencer,
        start_tick: f64,
        end_tick: f64,
        frames: usize,
        ticks_per_sample: f64,
    ) -> Vec<Vec<TimedEvent>> {
        let mut events = empty_event_lists(sequencer.active_channels);
        if frames == 0
            || !start_tick.is_finite()
            || !end_tick.is_finite()
            || !ticks_per_sample.is_finite()
            || ticks_per_sample <= 0.0
            || end_tick <= start_tick
        {
            return collect_lists(&events);
        }
        let window = FrameWindow { offset: 0, frames };
        match sequencer.playback_mode {
            PlaybackMode::Pattern => {
                if let Some(pattern) = sequencer.patterns.get(sequencer.current) {
                    let pattern_ticks = pattern.length_ticks();
                    let swing_percent = sequencer.swing_for_pattern(sequencer.current);
                    for (channel_index, event_list) in events.iter_mut().enumerate() {
                        let Some(channel) = pattern.channel(channel_index) else {
                            continue;
                        };
                        for note in channel
                            .notes()
                            .iter()
                            .copied()
                            .filter(|note| note.start_tick < pattern_ticks)
                        {
                            let swing = swing_offset_ticks(note.start_tick, swing_percent);
                            Sequencer::schedule_note_edge(
                                note, note.start_tick.saturating_add(swing), false,
                                pattern_ticks, 1, 0, start_tick, end_tick, window,
                                ticks_per_sample, event_list,
                            );
                            Sequencer::schedule_note_edge(
                                note, note.end_tick().saturating_add(swing), true,
                                pattern_ticks, 1, 0, start_tick, end_tick, window,
                                ticks_per_sample, event_list,
                            );
                        }
                    }
                }
            }
            PlaybackMode::Song => {
                let song_ticks = sequencer.song_length_ticks();
                let instance_stride = MAX_PLAYLIST_TICKS as u64 * sequencer.patterns.len() as u64;
                for placement in &sequencer.playlist {
                    let pattern_index = placement.pattern as usize;
                    if pattern_index >= sequencer.active_patterns {
                        continue;
                    }
                    let pattern = &sequencer.patterns[pattern_index];
                    let swing_percent = sequencer.swing_for_pattern(pattern_index);
                    let instance_offset = placement.pattern as u64 * MAX_PLAYLIST_TICKS as u64
                        + u64::from(placement.start_tick);
                    let pattern_ticks = pattern.length_ticks();
                    for (channel_index, event_list) in events.iter_mut().enumerate() {
                        let Some(channel) = pattern.channel(channel_index) else {
                            continue;
                        };
                        for note in channel
                            .notes()
                            .iter()
                            .copied()
                            .filter(|note| note.start_tick < pattern_ticks)
                        {
                            let swing = swing_offset_ticks(note.start_tick, swing_percent);
                            Sequencer::schedule_note_edge(
                                note,
                                placement
                                    .start_tick
                                    .saturating_add(note.start_tick)
                                    .saturating_add(swing),
                                false, song_ticks, instance_stride, instance_offset,
                                start_tick, end_tick, window, ticks_per_sample, event_list,
                            );
                            Sequencer::schedule_note_edge(
                                note,
                                placement
                                    .start_tick
                                    .saturating_add(note.end_tick())
                                    .saturating_add(swing),
                                true, song_ticks, instance_stride, instance_offset,
                                start_tick, end_tick, window, ticks_per_sample, event_list,
                            );
                        }
                    }
                }
            }
        }
        collect_lists(&events)
    }

    /// The full walk `schedule_once` replaced, same shape as
    /// [`brute_force_schedule`] but through `schedule_edge_once`'s
    /// no-wraparound math.
    fn brute_force_schedule_once(
        sequencer: &Sequencer,
        start_tick: f64,
        end_tick: f64,
        frames: usize,
        ticks_per_sample: f64,
    ) -> Vec<Vec<TimedEvent>> {
        let mut events = empty_event_lists(sequencer.active_channels);
        if frames == 0 || end_tick <= start_tick || ticks_per_sample <= 0.0 {
            return collect_lists(&events);
        }
        match sequencer.playback_mode {
            PlaybackMode::Pattern => {
                if let Some(pattern) = sequencer.patterns.get(sequencer.current) {
                    let pattern_ticks = pattern.length_ticks();
                    let swing_percent = sequencer.swing_for_pattern(sequencer.current);
                    for (channel_index, event_list) in events.iter_mut().enumerate() {
                        let Some(channel) = pattern.channel(channel_index) else {
                            continue;
                        };
                        for note in channel
                            .notes()
                            .iter()
                            .copied()
                            .filter(|note| note.start_tick < pattern_ticks)
                        {
                            let swing = swing_offset_ticks(note.start_tick, swing_percent);
                            Sequencer::schedule_edge_once(
                                note, note.start_tick.saturating_add(swing), false, 0,
                                start_tick, end_tick, frames, ticks_per_sample, event_list,
                            );
                            Sequencer::schedule_edge_once(
                                note, note.end_tick().saturating_add(swing), true, 0,
                                start_tick, end_tick, frames, ticks_per_sample, event_list,
                            );
                        }
                    }
                }
            }
            PlaybackMode::Song => {
                for placement in &sequencer.playlist {
                    let pattern_index = placement.pattern as usize;
                    if pattern_index >= sequencer.active_patterns {
                        continue;
                    }
                    let pattern = &sequencer.patterns[pattern_index];
                    let swing_percent = sequencer.swing_for_pattern(pattern_index);
                    let pattern_ticks = pattern.length_ticks();
                    let instance = u64::from(placement.pattern) * u64::from(MAX_PLAYLIST_TICKS)
                        + u64::from(placement.start_tick);
                    for (channel_index, event_list) in events.iter_mut().enumerate() {
                        let Some(channel) = pattern.channel(channel_index) else {
                            continue;
                        };
                        for note in channel
                            .notes()
                            .iter()
                            .copied()
                            .filter(|note| note.start_tick < pattern_ticks)
                        {
                            let swing = swing_offset_ticks(note.start_tick, swing_percent);
                            Sequencer::schedule_edge_once(
                                note,
                                placement
                                    .start_tick
                                    .saturating_add(note.start_tick)
                                    .saturating_add(swing),
                                false, instance, start_tick, end_tick, frames,
                                ticks_per_sample, event_list,
                            );
                            Sequencer::schedule_edge_once(
                                note,
                                placement
                                    .start_tick
                                    .saturating_add(note.end_tick())
                                    .saturating_add(swing),
                                true, instance, start_tick, end_tick, frames,
                                ticks_per_sample, event_list,
                            );
                        }
                    }
                }
            }
        }
        collect_lists(&events)
    }

    /// The indexed path (`notes_starting_in`/`notes_ending_in`, the
    /// start-sorted playlist view) must emit exactly the events the old
    /// full walk did, in the same order, for every note it touches -- over
    /// random patterns, random swing, random playlists, and random query
    /// windows, in both playback modes, through both `schedule` and
    /// `schedule_once`. Deliberately biased toward the edge cases Plan A's
    /// candidate selection has to get right rather than reject as a
    /// false positive: patterns short enough that one block spans a whole
    /// pass, notes whose duration sustains past one pass (the note-off
    /// fallback), and playlist placements sharing a start tick (the
    /// layering tie-break).
    #[test]
    fn indexed_scheduling_matches_brute_force_over_random_patterns() {
        let mut rng = Rng::new(0xC0FFEE_D15EA5E);
        for trial in 0..60u32 {
            let channels = rng.range_u32(1, 5) as usize;
            let active_patterns = rng.range_u32(1, 4) as usize;
            let mut sequencer =
                Sequencer::new(channels, active_patterns, DEFAULT_STEPS as usize, Ppq::DEFAULT);
            for _ in 1..active_patterns {
                sequencer.add_pattern();
            }
            sequencer.set_active_channels(channels);
            sequencer.set_swing(rng.range_u32(50, 76) as u8);

            for pattern in 0..active_patterns {
                // Occasionally a very short pattern, to exercise the
                // "span already covers a whole pass" fallback.
                let steps = if rng.chance(15) {
                    rng.range_u32(1, 3)
                } else {
                    rng.range_u32(1, 64)
                };
                sequencer.set_pattern_length(pattern, steps as usize);
                let pattern_ticks = sequencer.pattern_length_ticks(pattern).unwrap_or(1);
                for channel in 0..channels {
                    let note_count = rng.range_u32(0, 14);
                    for id in 0..note_count {
                        // Sometimes past the pattern's current length, so
                        // the `start_tick < pattern_ticks` filter has
                        // something to actually exclude.
                        let start = rng.range_u32(0, pattern_ticks + 60);
                        // Occasionally a duration spanning a whole pass or
                        // more, to exercise the note-off fallback.
                        let duration = if rng.chance(12) {
                            rng.range_u32(pattern_ticks, pattern_ticks.saturating_mul(3) + 5)
                        } else {
                            rng.range_u32(1, 60)
                        };
                        let note = NoteEvent::new(id + 1, start, duration.max(1), 60, 100);
                        sequencer.upsert_note(pattern, channel, note);
                    }
                }
            }

            // A handful of placements, sometimes sharing a start tick
            // (the layering tie-break) and spread widely enough to make a
            // short song's own wraparound reachable.
            let placement_count = rng.range_u32(0, 7);
            let mut shared_start = None;
            for _ in 0..placement_count {
                let pattern = rng.range_u32(0, active_patterns as u32) as usize;
                let start = if rng.chance(30) {
                    *shared_start.get_or_insert_with(|| rng.range_u32(0, TICKS_PER_BAR * 3))
                } else {
                    rng.range_u32(0, TICKS_PER_BAR * 3)
                };
                sequencer.set_playlist_placement(pattern, start, true);
            }

            for mode in [PlaybackMode::Pattern, PlaybackMode::Song] {
                sequencer.set_playback_mode(mode);
                let bpm = rng.range_f64(20.0, 999.0);
                let sample_rate = [22_050u32, 44_100, 48_000, 96_000]
                    [rng.range_u32(0, 4) as usize];
                let ticks_per_sample =
                    mooloop_core::ticks_per_sample(bpm, sample_rate, Ppq::DEFAULT);

                for query in 0..12u32 {
                    let start_tick = rng.range_f64(0.0, f64::from(TICKS_PER_BAR) * 4.0);
                    let frames = rng.range_u32(1, 700) as usize;
                    let end_tick = start_tick + frames as f64 * ticks_per_sample;

                    let expected =
                        brute_force_schedule(&sequencer, start_tick, end_tick, frames, ticks_per_sample);
                    let actual = real_schedule(&sequencer, start_tick, end_tick, frames, ticks_per_sample);
                    assert_eq!(
                        actual, expected,
                        "schedule mismatch: trial {trial} query {query} mode {mode:?} \
                         start {start_tick} frames {frames} ticks_per_sample {ticks_per_sample}\n{}",
                        dump(&sequencer)
                    );

                    let expected_once =
                        brute_force_schedule_once(&sequencer, start_tick, end_tick, frames, ticks_per_sample);
                    let actual_once =
                        real_schedule_once(&sequencer, start_tick, end_tick, frames, ticks_per_sample);
                    assert_eq!(
                        actual_once, expected_once,
                        "schedule_once mismatch: trial {trial} query {query} mode {mode:?} \
                         start {start_tick} frames {frames} ticks_per_sample {ticks_per_sample}\n{}",
                        dump(&sequencer)
                    );
                }
            }
        }
    }
}
