//! JACK-independent render state shared by realtime playback and file export.

use std::sync::Arc;

use arc_swap::ArcSwapOption;
use mooloop_core::{
    ChannelSource, EngineCommand, Project, SamplerParams, DEFAULT_STEPS, MAX_CHANNELS,
};
use mooloop_dsp::{
    pan_gains, AudioNode, Event, EventList, ProcessContext, SampleData, Sampler, StereoBus,
    TimedEvent, MAX_BLOCK_SIZE,
};

use crate::sequencer::Sequencer;
use crate::transport::Transport;

struct ChannelStrip {
    instrument: Sampler,
    effects: Vec<Box<dyn AudioNode + Send>>,
    bus: StereoBus,
    gain: f32,
    pan: f32,
    muted: bool,
}

impl ChannelStrip {
    fn new(instrument: Sampler) -> Self {
        Self {
            instrument,
            effects: Vec::new(),
            bus: StereoBus::with_capacity(MAX_BLOCK_SIZE),
            gain: 0.8,
            pan: 0.0,
            muted: false,
        }
    }

    fn set_volume(&mut self, volume: f32) {
        self.gain = volume.clamp(0.0, 1.0);
    }

    fn set_pan(&mut self, pan: f32) {
        self.pan = pan.clamp(-1.0, 1.0);
    }
}

fn inject_choke_events(choke_groups: &[u8], events: &mut [EventList]) {
    let active = choke_groups.len().min(events.len());
    for source in 0..active {
        let group = choke_groups[source];
        if group == 0 {
            continue;
        }
        for target in 0..active {
            if source == target || choke_groups[target] != group {
                continue;
            }
            let (source_events, target_events) = if source < target {
                let (left, right) = events.split_at_mut(target);
                (&left[source], &mut right[0])
            } else {
                let (left, right) = events.split_at_mut(source);
                (&right[0], &mut left[target])
            };
            for event in source_events.iter() {
                if matches!(event.event, Event::NoteOn { .. }) {
                    target_events.push_ordered(TimedEvent {
                        offset: event.offset,
                        event: Event::Choke,
                    });
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RenderReport {
    pub position_tick: u64,
    pub beat_in_bar: u8,
    pub playing: bool,
    pub peak_l: f32,
    pub peak_r: f32,
}

pub(crate) struct RenderState {
    transport: Transport,
    sequencer: Sequencer,
    strips: Vec<ChannelStrip>,
    events: Vec<EventList>,
    empty_events: EventList,
    master: StereoBus,
    sample_rate: u32,
}

impl RenderState {
    pub fn new(
        sample_rate: u32,
        sample_slots: Arc<Vec<Arc<ArcSwapOption<SampleData>>>>,
        initial_params: SamplerParams,
    ) -> Self {
        let strips = sample_slots
            .iter()
            .map(|slot| ChannelStrip::new(Sampler::new(slot.clone(), initial_params, sample_rate)))
            .collect();
        Self {
            transport: Transport::new(sample_rate),
            sequencer: Sequencer::new(1, 1, DEFAULT_STEPS as usize, mooloop_core::Ppq::DEFAULT),
            strips,
            events: (0..MAX_CHANNELS).map(|_| EventList::empty()).collect(),
            empty_events: EventList::empty(),
            master: StereoBus::with_capacity(MAX_BLOCK_SIZE),
            sample_rate,
        }
    }

    pub fn from_project(
        sample_rate: u32,
        project: &Project,
        samples: &[Option<Arc<SampleData>>],
    ) -> Self {
        let fallback = SampleData::default_kick(sample_rate);
        let slots = Arc::new(
            (0..MAX_CHANNELS)
                .map(|index| {
                    let sample = samples.get(index).cloned().flatten().or_else(|| {
                        project.channels.get(index).and_then(|channel| {
                            matches!(
                                channel.setup.sampler_state().sample,
                                mooloop_core::SampleReference::Builtin { .. }
                            )
                            .then(|| fallback.clone())
                        })
                    });
                    Arc::new(ArcSwapOption::from(sample))
                })
                .collect(),
        );
        let mut state = Self::new(sample_rate, slots, SamplerParams::default());
        state.load_project(project);
        state
    }

    pub fn load_project(&mut self, project: &Project) {
        self.transport.stop();
        self.transport.set_tempo(project.bpm.into());
        self.sequencer.load_project(project);
        for (index, strip) in self.strips.iter_mut().enumerate() {
            if let Some(channel) = project.channels.get(index) {
                strip.muted = channel.setup.channel.muted;
                strip.set_volume(channel.setup.channel.volume);
                strip.set_pan(channel.setup.channel.pan);
                match &channel.setup.source {
                    ChannelSource::Sampler(sampler) => strip.instrument.set_params(sampler.params),
                }
            } else {
                strip.muted = false;
                strip.set_volume(0.8);
                strip.set_pan(0.0);
                strip.instrument.set_params(SamplerParams::default());
            }
        }
    }

    pub fn apply_command(&mut self, cmd: EngineCommand) {
        match cmd {
            EngineCommand::Play => self.transport.play(),
            EngineCommand::Pause => self.transport.pause(),
            EngineCommand::Stop => self.transport.stop(),
            EngineCommand::SetTempo(bpm) => self.transport.set_tempo(bpm),
            EngineCommand::SetCurrentPattern(pattern) => {
                self.sequencer.set_current_pattern(pattern as usize)
            }
            EngineCommand::AddPattern => {
                self.sequencer.add_pattern();
            }
            EngineCommand::SetPlaybackMode(mode) => self.sequencer.set_playback_mode(mode),
            EngineCommand::SetPatternLength {
                pattern,
                length_steps,
            } => self
                .sequencer
                .set_pattern_length(pattern as usize, length_steps as usize),
            EngineCommand::SetPlaylistPlacement {
                pattern,
                start_tick,
                on,
            } => {
                self.sequencer
                    .set_playlist_placement(pattern as usize, start_tick, on);
            }
            EngineCommand::AddChannel => {
                self.sequencer
                    .set_active_channels(self.sequencer.active_channels() + 1);
            }
            EngineCommand::RemoveChannel => {
                self.sequencer
                    .set_active_channels(self.sequencer.active_channels().saturating_sub(1));
            }
            EngineCommand::SetChannelMuted { channel, muted } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.muted = muted;
                }
            }
            EngineCommand::SetChannelVolume { channel, volume } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.set_volume(volume);
                }
            }
            EngineCommand::SetChannelPan { channel, pan } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.set_pan(pan);
                }
            }
            EngineCommand::SetStep {
                pattern,
                channel,
                step,
                on,
                note,
                velocity,
            } => self.sequencer.set_step(
                pattern as usize,
                channel as usize,
                step as usize,
                on,
                note,
                velocity,
            ),
            EngineCommand::UpsertNote {
                pattern,
                channel,
                note,
            } => {
                self.sequencer
                    .upsert_note(pattern as usize, channel as usize, note);
            }
            EngineCommand::RemoveNote {
                pattern,
                channel,
                id,
            } => {
                self.sequencer
                    .remove_note(pattern as usize, channel as usize, id);
            }
            EngineCommand::SetChannelSamplerParams { channel, params } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.instrument.set_params(params);
                }
            }
            EngineCommand::InstallProject => {}
        }
    }

    pub fn process_block(&mut self, frames: usize) -> RenderReport {
        self.process_block_inner(frames, true)
    }

    pub fn process_once_block(&mut self, frames: usize) -> RenderReport {
        self.process_block_inner(frames, false)
    }

    fn process_block_inner(&mut self, frames: usize, looping: bool) -> RenderReport {
        let frames = frames.min(MAX_BLOCK_SIZE);
        let ticks_per_sample = self.transport.ticks_per_sample();
        let position_frames = self.transport.frames_played();
        let (start_tick, end_tick) = self.transport.advance(frames);

        for events in &mut self.events {
            events.clear();
        }
        if self.transport.playing {
            if looping {
                self.sequencer.schedule(
                    start_tick,
                    end_tick,
                    frames,
                    ticks_per_sample,
                    &mut self.events,
                );
            } else {
                self.sequencer.schedule_once(
                    start_tick,
                    end_tick,
                    frames,
                    ticks_per_sample,
                    &mut self.events,
                );
            }
            let mut choke_groups = [0; MAX_CHANNELS];
            for (index, strip) in self
                .strips
                .iter()
                .enumerate()
                .take(self.sequencer.active_channels())
            {
                if !strip.muted {
                    choke_groups[index] = strip.instrument.choke_group();
                }
            }
            inject_choke_events(
                &choke_groups[..self.sequencer.active_channels()],
                &mut self.events,
            );
        }

        let context = ProcessContext {
            sample_rate: self.sample_rate,
            frames,
            playing: self.transport.playing,
            bpm: self.transport.bpm,
            position_ticks: start_tick,
            position_frames,
        };
        self.master.clear(frames);
        for (index, strip) in self
            .strips
            .iter_mut()
            .enumerate()
            .take(self.sequencer.active_channels())
        {
            if strip.muted {
                continue;
            }
            strip.bus.clear(frames);
            strip
                .instrument
                .process(&context, &mut strip.bus, &self.events[index], None);
            for effect in &mut strip.effects {
                effect.process(&context, &mut strip.bus, &self.empty_events, None);
            }
            let (pan_l, pan_r) = pan_gains(strip.pan);
            strip
                .bus
                .apply_stereo_gain(strip.gain * pan_l, strip.gain * pan_r, frames);
            self.master.add_from(&strip.bus, frames);
        }
        let (peak_l, peak_r) = self.master.peak(frames);
        RenderReport {
            position_tick: self.transport.position_ticks as u64,
            beat_in_bar: self.transport.beat_in_bar(),
            playing: self.transport.playing,
            peak_l,
            peak_r,
        }
    }

    pub fn master(&self) -> &StereoBus {
        &self.master
    }

    pub fn play(&mut self) {
        self.transport.play();
    }

    pub fn pause(&mut self) {
        self.transport.pause();
    }

    pub fn ticks_per_sample(&self) -> f64 {
        self.transport.ticks_per_sample()
    }

    pub fn song_length_ticks(&self) -> u32 {
        self.sequencer.song_length_ticks()
    }

    pub fn pattern_length_ticks(&self, pattern: usize) -> Option<u32> {
        self.sequencer.pattern_length_ticks(pattern)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_strip() -> ChannelStrip {
        let slot = Arc::new(ArcSwapOption::empty());
        ChannelStrip::new(Sampler::new(slot, SamplerParams::default(), 48_000))
    }

    #[test]
    fn channel_output_controls_are_bounded() {
        let mut strip = test_strip();
        strip.set_volume(2.0);
        strip.set_pan(-2.0);
        assert_eq!(strip.gain, 1.0);
        assert_eq!(strip.pan, -1.0);
        strip.set_volume(-1.0);
        strip.set_pan(2.0);
        assert_eq!(strip.gain, 0.0);
        assert_eq!(strip.pan, 1.0);
    }

    #[test]
    fn matching_choke_group_receives_sample_timed_choke() {
        let mut events = [EventList::empty(), EventList::empty(), EventList::empty()];
        events[0].push(TimedEvent {
            offset: 37,
            event: Event::NoteOn {
                id: 1,
                note: 60,
                velocity: 100,
            },
        });
        inject_choke_events(&[2, 2, 3], &mut events);
        assert_eq!(events[0].len(), 1);
        assert_eq!(events[1].iter().next().unwrap().event, Event::Choke);
        assert!(events[2].is_empty());
    }

    #[test]
    fn project_load_replaces_preallocated_state() {
        let mut project = Project {
            bpm: 173,
            ..Project::default()
        };
        project.pattern_lengths[0] = 32;
        project.channels[0].notes[0].push(mooloop_core::NoteEvent::new(1, 24, 12, 60, 100));
        let render = RenderState::from_project(48_000, &project, &[]);
        assert_eq!(render.pattern_length_ticks(0), Some(32 * 24));
        assert!((render.ticks_per_sample() - (173.0 * 96.0 / 60.0 / 48_000.0)).abs() < 1e-12);
    }
}
