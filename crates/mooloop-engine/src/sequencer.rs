//! Realtime pattern scheduler.
//!
//! Notes live at PPQ tick positions and are converted to sample offsets for
//! each process block. Pattern storage and event lists are bounded up front;
//! scheduling and edits never allocate on the audio thread.

use mooloop_core::{
    NoteEvent, NoteId, Pattern, Ppq, DEFAULT_NOTE_DURATION_TICKS, MAX_CHANNELS, MAX_PATTERN_STEPS,
    TICKS_PER_STEP,
};
use mooloop_dsp::{Event, EventList, TimedEvent};

const BOUNDARY_EPS: f64 = 1.0e-6;

pub struct Sequencer {
    patterns: Vec<Pattern>,
    current: usize,
    active_channels: usize,
}

impl Sequencer {
    pub fn new(initial_channels: usize, num_patterns: usize, num_steps: usize, ppq: Ppq) -> Self {
        assert_eq!(ppq, Ppq::DEFAULT, "pattern tick constants require PPQ 96");
        let patterns = (0..num_patterns)
            .map(|_| {
                let mut pattern = Pattern::with_steps(MAX_CHANNELS, MAX_PATTERN_STEPS as usize);
                pattern.set_length_steps(num_steps);
                pattern
            })
            .collect();
        Self {
            patterns,
            current: 0,
            active_channels: initial_channels.min(MAX_CHANNELS),
        }
    }

    pub fn set_current_pattern(&mut self, pattern: usize) {
        if pattern < self.patterns.len() {
            self.current = pattern;
        }
    }

    pub fn set_pattern_length(&mut self, pattern: usize, length_steps: usize) {
        if let Some(pattern) = self.patterns.get_mut(pattern) {
            pattern.set_length_steps(length_steps);
        }
    }

    pub fn active_channels(&self) -> usize {
        self.active_channels
    }

    pub fn set_active_channels(&mut self, n: usize) {
        self.active_channels = n.min(MAX_CHANNELS);
    }

    pub fn upsert_note(&mut self, pattern: usize, channel: usize, note: NoteEvent) -> bool {
        self.patterns
            .get_mut(pattern)
            .and_then(|pattern| pattern.channel_mut(channel))
            .is_some_and(|channel| channel.upsert_note(note))
    }

    pub fn remove_note(&mut self, pattern: usize, channel: usize, id: NoteId) -> bool {
        self.patterns
            .get_mut(pattern)
            .and_then(|pattern| pattern.channel_mut(channel))
            .and_then(|channel| channel.remove_note(id))
            .is_some()
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
        events: &mut [EventList],
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
        let Some(pattern) = self.patterns.get(self.current) else {
            return;
        };
        let pattern_ticks = pattern.length_ticks();
        if pattern_ticks == 0 {
            return;
        }

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
                self.schedule_note_edge(
                    note,
                    note.start_tick,
                    false,
                    pattern_ticks,
                    start_tick,
                    end_tick,
                    frames,
                    ticks_per_sample,
                    event_list,
                );
                self.schedule_note_edge(
                    note,
                    note.end_tick(),
                    true,
                    pattern_ticks,
                    start_tick,
                    end_tick,
                    frames,
                    ticks_per_sample,
                    event_list,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn schedule_note_edge(
        &self,
        note: NoteEvent,
        edge_tick: u32,
        is_note_off: bool,
        pattern_ticks: u32,
        start_tick: f64,
        end_tick: f64,
        frames: usize,
        ticks_per_sample: f64,
        event_list: &mut EventList,
    ) {
        let period = f64::from(pattern_ticks);
        let edge = f64::from(edge_tick);
        let mut cycle = ((start_tick - edge - BOUNDARY_EPS) / period).ceil() as i64;
        cycle = cycle.max(0);
        let mut absolute_tick = edge + cycle as f64 * period;

        while absolute_tick < end_tick {
            if absolute_tick + BOUNDARY_EPS >= start_tick {
                let offset = ((absolute_tick - start_tick) / ticks_per_sample).round() as i64;
                let offset = offset.clamp(0, frames as i64 - 1) as u32;
                let voice_id = ((cycle as u64) << 32) | u64::from(note.id);
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

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{ticks_per_sample, TICKS_PER_64TH};

    fn schedule_range(sequencer: &Sequencer, start_tick: f64, end_tick: f64) -> Vec<TimedEvent> {
        let ticks_per_sample = ticks_per_sample(120.0, 48_000, Ppq::DEFAULT);
        let frames = ((end_tick - start_tick) / ticks_per_sample).ceil() as usize;
        let mut events = [EventList::empty()];
        sequencer.schedule(start_tick, end_tick, frames, ticks_per_sample, &mut events);
        events[0].iter().copied().collect()
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
        for drift in [-BOUNDARY_EPS * 0.5, 0.0, BOUNDARY_EPS * 0.5] {
            let events = schedule_range(&sequencer, pattern_ticks + drift, pattern_ticks + 2.0);
            assert_eq!(events[0].offset, 0, "drift {drift}");
            assert!(matches!(events[0].event, Event::NoteOn { .. }));
        }
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
}
