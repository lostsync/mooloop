//! Realtime pattern scheduler.
//!
//! Notes live at PPQ tick positions and are converted to sample offsets for
//! each process block. Pattern storage and event lists are bounded up front;
//! scheduling and edits never allocate on the audio thread.

use mooloop_core::{
    AutomationLane, AutomationPoint, NoteEvent, NoteId, ParamAddr, Pattern, PatternPlacement,
    PlaybackMode, PointId, Ppq, Project,
    DEFAULT_NOTE_DURATION_TICKS, DEFAULT_STEPS, DEFAULT_SWING_PERCENT, MAX_CHANNELS,
    MAX_PATTERN_STEPS, MAX_PLAYLIST_PLACEMENTS, MAX_PLAYLIST_TICKS, MAX_SWING_PERCENT,
    MIN_SWING_PERCENT, TICKS_PER_BAR, TICKS_PER_STEP,
};
use mooloop_dsp::{Event, EventList, TimedEvent};

pub struct Sequencer {
    patterns: Vec<Pattern>,
    active_patterns: usize,
    current: usize,
    active_channels: usize,
    playback_mode: PlaybackMode,
    swing_percent: u8,
    playlist: Vec<PatternPlacement>,
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
        }
    }

    pub fn set_current_pattern(&mut self, pattern: usize) {
        if pattern < self.active_patterns {
            self.current = pattern;
        }
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

    pub fn set_playlist_placement(&mut self, pattern: usize, start_tick: u32, on: bool) -> bool {
        if pattern >= self.active_patterns {
            return false;
        }
        if start_tick >= MAX_PLAYLIST_TICKS {
            return false;
        }
        let placement = PatternPlacement::new(pattern as u8, start_tick);
        let position = self.playlist.iter().position(|item| *item == placement);
        match (on, position) {
            (true, None) if self.playlist.len() < self.playlist.capacity() => {
                self.playlist.push(placement);
                self.playlist.sort_unstable();
                true
            }
            (false, Some(index)) => {
                self.playlist.remove(index);
                true
            }
            _ => false,
        }
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

    /// Clear one preallocated channel lane across the full pattern bank.
    pub fn clear_channel(&mut self, channel: usize) {
        if channel >= MAX_CHANNELS {
            return;
        }
        for pattern in &mut self.patterns {
            pattern.channels[channel].clear();
        }
    }

    /// Replace musical state without growing any realtime-owned allocation.
    pub fn load_project(&mut self, project: &Project) {
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

        for pattern in &mut self.patterns {
            pattern.set_length_steps(DEFAULT_STEPS as usize);
            for channel in &mut pattern.channels {
                channel.clear();
            }
        }
        for (pattern_index, length) in project.pattern_lengths.iter().enumerate() {
            self.patterns[pattern_index].set_length_steps(*length as usize);
        }
        for (channel_index, channel) in project.channels.iter().enumerate() {
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

    pub fn open_automation_lane(
        &mut self,
        pattern: usize,
        channel: usize,
        target: ParamAddr,
    ) -> bool {
        self.channel_pattern_mut(pattern, channel)
            .and_then(|channel| channel.open_lane(target))
            .is_some()
    }

    pub fn remove_automation_lane(
        &mut self,
        pattern: usize,
        channel: usize,
        target: ParamAddr,
    ) -> bool {
        self.channel_pattern_mut(pattern, channel)
            .and_then(|channel| channel.remove_lane(target))
            .is_some()
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
        self.channel_pattern_mut(pattern, channel)
            .and_then(|channel| channel.open_lane(target))
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
                let mut best: Option<(&AutomationLane, f64, u32)> = None;
                let mut best_start = 0u32;
                for placement in &self.playlist {
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
    pub fn schedule(
        &self,
        start_tick: f64,
        end_tick: f64,
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
        match self.playback_mode {
            PlaybackMode::Pattern => {
                self.schedule_pattern(start_tick, end_tick, frames, ticks_per_sample, events)
            }
            PlaybackMode::Song => {
                self.schedule_song(start_tick, end_tick, frames, ticks_per_sample, events)
            }
        }
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
                    for note in channel
                        .notes()
                        .iter()
                        .copied()
                        .filter(|note| note.start_tick < pattern_ticks)
                    {
                        let swing = swing_offset_ticks(note.start_tick, swing_percent);
                        Self::schedule_edge_once(
                            note,
                            note.start_tick.saturating_add(swing),
                            false,
                            0,
                            start_tick,
                            end_tick,
                            frames,
                            ticks_per_sample,
                            event_list,
                        );
                        Self::schedule_edge_once(
                            note,
                            note.end_tick().saturating_add(swing),
                            true,
                            0,
                            start_tick,
                            end_tick,
                            frames,
                            ticks_per_sample,
                            event_list,
                        );
                    }
                }
            }
            PlaybackMode::Song => {
                for placement in &self.playlist {
                    let pattern_index = placement.pattern as usize;
                    if pattern_index >= self.active_patterns {
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
                        for note in channel
                            .notes()
                            .iter()
                            .copied()
                            .filter(|note| note.start_tick < pattern_ticks)
                        {
                            let swing = swing_offset_ticks(note.start_tick, swing_percent);
                            Self::schedule_edge_once(
                                note,
                                placement
                                    .start_tick
                                    .saturating_add(note.start_tick)
                                    .saturating_add(swing),
                                false,
                                instance,
                                start_tick,
                                end_tick,
                                frames,
                                ticks_per_sample,
                                event_list,
                            );
                            Self::schedule_edge_once(
                                note,
                                placement
                                    .start_tick
                                    .saturating_add(note.end_tick())
                                    .saturating_add(swing),
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

    fn schedule_pattern(
        &self,
        start_tick: f64,
        end_tick: f64,
        frames: usize,
        ticks_per_sample: f64,
        events: &mut [Box<EventList>],
    ) {
        let Some(pattern) = self.patterns.get(self.current) else {
            return;
        };
        let pattern_ticks = pattern.length_ticks();
        let swing_percent = self.swing_for_pattern(self.current);

        for (channel_index, event_list) in events.iter_mut().enumerate().take(self.active_channels)
        {
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
                Self::schedule_note_edge(
                    note,
                    note.start_tick.saturating_add(swing),
                    false,
                    pattern_ticks,
                    1,
                    0,
                    start_tick,
                    end_tick,
                    frames,
                    ticks_per_sample,
                    event_list,
                );
                Self::schedule_note_edge(
                    note,
                    note.end_tick().saturating_add(swing),
                    true,
                    pattern_ticks,
                    1,
                    0,
                    start_tick,
                    end_tick,
                    frames,
                    ticks_per_sample,
                    event_list,
                );
            }
        }
    }

    fn schedule_song(
        &self,
        start_tick: f64,
        end_tick: f64,
        frames: usize,
        ticks_per_sample: f64,
        events: &mut [Box<EventList>],
    ) {
        let song_ticks = self.song_length_ticks();
        let instance_stride = MAX_PLAYLIST_TICKS as u64 * self.patterns.len() as u64;
        for placement in &self.playlist {
            let pattern_index = placement.pattern as usize;
            if pattern_index >= self.active_patterns {
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
                for note in channel
                    .notes()
                    .iter()
                    .copied()
                    .filter(|note| note.start_tick < pattern_ticks)
                {
                    let swing = swing_offset_ticks(note.start_tick, swing_percent);
                    Self::schedule_note_edge(
                        note,
                        placement
                            .start_tick
                            .saturating_add(note.start_tick)
                            .saturating_add(swing),
                        false,
                        song_ticks,
                        instance_stride,
                        instance_offset,
                        start_tick,
                        end_tick,
                        frames,
                        ticks_per_sample,
                        event_list,
                    );
                    Self::schedule_note_edge(
                        note,
                        placement
                            .start_tick
                            .saturating_add(note.end_tick())
                            .saturating_add(swing),
                        true,
                        song_ticks,
                        instance_stride,
                        instance_offset,
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
        frames: usize,
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
                let offset = ((absolute_tick - start_tick) / ticks_per_sample).round() as i64;
                let offset = offset.clamp(0, frames as i64 - 1) as u32;
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

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{ticks_per_sample, TICKS_PER_64TH};

    const TEST_PLACEMENT_TICKS: u32 = TICKS_PER_BAR / 2;

    fn schedule_range(sequencer: &Sequencer, start_tick: f64, end_tick: f64) -> Vec<TimedEvent> {
        let ticks_per_sample = ticks_per_sample(120.0, 48_000, Ppq::DEFAULT);
        let frames = ((end_tick - start_tick) / ticks_per_sample).ceil() as usize;
        let mut events = [Box::new(EventList::empty())];
        sequencer.schedule(start_tick, end_tick, frames, ticks_per_sample, &mut events);
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
}
