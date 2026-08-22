//! JACK-independent render state shared by realtime playback and file export.

use std::sync::Arc;

use arc_swap::ArcSwapOption;
use mooloop_core::{
    ChannelSource, DeviceKind, DrumSynthParams, EngineCommand, MonoSynthParams, Project,
    SamplerParams, DEFAULT_STEPS, MAX_CHANNELS, MAX_EFFECTS_PER_CHANNEL,
};
use mooloop_dsp::{
    build_effect, pan_gains, AudioNode, DrumSynth, Event, EventList, MonoSynth, ProcessContext,
    SampleData, Sampler, StereoBus, TimedEvent, MAX_BLOCK_SIZE,
};

use crate::sequencer::Sequencer;
use crate::transport::Transport;
use crate::StructuralCommand;

/// Nodes displaced from effect slots, handed back so the non-realtime side
/// can drop them. The realtime thread must never free a `Box` itself.
type Reclaim = Vec<Box<dyn AudioNode + Send>>;

struct ChannelStrip {
    sampler: Sampler,
    drum_synth: DrumSynth,
    mono_synth: MonoSynth,
    active_source: DeviceKind,
    /// Fixed array of optional effect slots, processed in order after the
    /// generator. Slots are `None` until a node is installed structurally.
    effect_chain: [Option<Box<dyn AudioNode + Send>>; MAX_EFFECTS_PER_CHANNEL],
    /// Per-slot parameter events, queued between blocks by
    /// `EngineCommand::SetEffectParam` and consumed by the next block. Kept
    /// separate from the note-event lists so slot addressing is trivial and
    /// generators never see effect events.
    effect_events: [EventList; MAX_EFFECTS_PER_CHANNEL],
    effect_bypassed: [bool; MAX_EFFECTS_PER_CHANNEL],
    bus: StereoBus,
    gain: f32,
    pan: f32,
    muted: bool,
}

impl ChannelStrip {
    fn new(sample_slot: Arc<ArcSwapOption<SampleData>>, sample_rate: u32) -> Self {
        Self {
            sampler: Sampler::new(sample_slot, SamplerParams::default(), sample_rate),
            drum_synth: DrumSynth::new(DrumSynthParams::default(), sample_rate),
            mono_synth: MonoSynth::new(MonoSynthParams::default(), sample_rate),
            active_source: DeviceKind::Sampler,
            effect_chain: std::array::from_fn(|_| None),
            effect_events: std::array::from_fn(|_| EventList::empty()),
            effect_bypassed: [false; MAX_EFFECTS_PER_CHANNEL],
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

    fn reset_sources_to_defaults(&mut self, source: DeviceKind) {
        self.sampler.reset();
        self.drum_synth.reset();
        self.mono_synth.reset();
        self.sampler.set_params(SamplerParams::default());
        self.drum_synth.set_params(DrumSynthParams::default());
        self.mono_synth.set_params(MonoSynthParams::default());
        self.active_source = source;
    }

    fn reset_slot(&mut self, source: DeviceKind, reclaim: &mut Reclaim) {
        self.reset_sources_to_defaults(source);
        self.clear_effects(reclaim);
        self.gain = 0.8;
        self.pan = 0.0;
        self.muted = false;
    }

    /// Remove every effect node, queuing the boxes for off-thread disposal.
    fn clear_effects(&mut self, reclaim: &mut Reclaim) {
        for slot in &mut self.effect_chain {
            if let Some(node) = slot.take() {
                reclaim.push(node);
            }
        }
        for events in &mut self.effect_events {
            events.clear();
        }
        self.effect_bypassed = [false; MAX_EFFECTS_PER_CHANNEL];
    }

    fn install_effect(
        &mut self,
        slot: usize,
        node: Box<dyn AudioNode + Send>,
        reclaim: &mut Reclaim,
    ) {
        if let Some(target) = self.effect_chain.get_mut(slot) {
            if let Some(old) = target.replace(node) {
                reclaim.push(old);
            }
        }
    }

    fn remove_effect(&mut self, slot: usize, reclaim: &mut Reclaim) {
        if let Some(target) = self.effect_chain.get_mut(slot) {
            if let Some(old) = target.take() {
                reclaim.push(old);
            }
        }
        if let Some(events) = self.effect_events.get_mut(slot) {
            events.clear();
        }
        if let Some(bypassed) = self.effect_bypassed.get_mut(slot) {
            *bypassed = false;
        }
    }

    fn swap_effects(&mut self, slot_a: usize, slot_b: usize) {
        if slot_a < MAX_EFFECTS_PER_CHANNEL && slot_b < MAX_EFFECTS_PER_CHANNEL {
            self.effect_chain.swap(slot_a, slot_b);
            self.effect_events.swap(slot_a, slot_b);
            self.effect_bypassed.swap(slot_a, slot_b);
        }
    }

    fn load_source(&mut self, source: &ChannelSource) {
        self.reset_sources_to_defaults(source.kind());
        match source {
            ChannelSource::Sampler(state) => self.sampler.set_params(state.params),
            ChannelSource::DrumSynth(state) => self.drum_synth.set_params(state.params),
            ChannelSource::MonoSynth(state) => self.mono_synth.set_params(state.params),
        }
    }

    fn choke_group(&self) -> u8 {
        match self.active_source {
            DeviceKind::Sampler => self.sampler.choke_group(),
            DeviceKind::DrumSynth => self.drum_synth.choke_group(),
            DeviceKind::MonoSynth => 0,
        }
    }

    fn process(&mut self, context: &ProcessContext, events: &EventList) {
        match self.active_source {
            DeviceKind::Sampler => self.sampler.process(context, &mut self.bus, events, None),
            DeviceKind::DrumSynth => self
                .drum_synth
                .process(context, &mut self.bus, events, None),
            DeviceKind::MonoSynth => self
                .mono_synth
                .process(context, &mut self.bus, events, None),
        }
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
    master: StereoBus,
    sample_rate: u32,
    /// Nodes displaced from effect slots this block, awaiting handoff to the
    /// reclaim ring (realtime playback) or plain drop (offline render).
    reclaim: Reclaim,
}

impl RenderState {
    pub fn new(sample_rate: u32, sample_slots: Arc<Vec<Arc<ArcSwapOption<SampleData>>>>) -> Self {
        let strips = sample_slots
            .iter()
            .map(|slot| ChannelStrip::new(slot.clone(), sample_rate))
            .collect();
        Self {
            transport: Transport::new(sample_rate),
            sequencer: Sequencer::new(1, 1, DEFAULT_STEPS as usize, mooloop_core::Ppq::DEFAULT),
            strips,
            events: (0..MAX_CHANNELS).map(|_| EventList::empty()).collect(),
            master: StereoBus::with_capacity(MAX_BLOCK_SIZE),
            sample_rate,
            reclaim: Vec::new(),
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
                            match &channel.setup.source {
                                ChannelSource::Sampler(state)
                                    if matches!(
                                        state.sample,
                                        mooloop_core::SampleReference::Builtin { .. }
                                    ) =>
                                {
                                    Some(fallback.clone())
                                }
                                _ => None,
                            }
                        })
                    });
                    Arc::new(ArcSwapOption::from(sample))
                })
                .collect(),
        );
        let mut state = Self::new(sample_rate, slots);
        state.load_project(project);
        state
    }

    pub fn load_project(&mut self, project: &Project) {
        self.transport.stop();
        self.transport.set_tempo(project.bpm.into());
        self.sequencer.load_project(project);
        for (index, strip) in self.strips.iter_mut().enumerate() {
            if let Some(channel) = project.channels.get(index) {
                strip.load_source(&channel.setup.source);
                strip.muted = channel.setup.channel.muted;
                strip.set_volume(channel.setup.channel.volume);
                strip.set_pan(channel.setup.channel.pan);
                // Project loading is a transport-stop/load-time operation,
                // not a hot per-block path, so constructing the boxed nodes
                // here is acceptable (see docs/EFFECTS_PLAN.md). Displaced
                // nodes still go through reclaim rather than being dropped.
                strip.clear_effects(&mut self.reclaim);
                for (slot, effect) in channel
                    .setup
                    .effects
                    .iter()
                    .take(MAX_EFFECTS_PER_CHANNEL)
                    .enumerate()
                {
                    let node = build_effect(effect.params, self.sample_rate);
                    strip.install_effect(slot, node, &mut self.reclaim);
                    strip.effect_bypassed[slot] = effect.bypassed;
                }
            } else {
                strip.reset_slot(DeviceKind::Sampler, &mut self.reclaim);
            }
        }
    }

    /// Apply a structural change (install/remove of a boxed node). Called on
    /// the realtime thread from the structural ring drain; the box itself was
    /// allocated on the GUI thread.
    pub(crate) fn apply_structural(&mut self, cmd: StructuralCommand) {
        match cmd {
            StructuralCommand::InstallEffect {
                channel,
                slot,
                node,
            } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.install_effect(slot as usize, node, &mut self.reclaim);
                }
            }
            StructuralCommand::RemoveEffect { channel, slot } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.remove_effect(slot as usize, &mut self.reclaim);
                }
            }
        }
    }

    /// Take the displaced nodes accumulated since the last call. Realtime
    /// playback pushes these into the reclaim ring; offline rendering can
    /// simply drop the returned vec.
    pub(crate) fn take_reclaim(&mut self) -> Reclaim {
        std::mem::take(&mut self.reclaim)
    }

    pub fn apply_command(&mut self, cmd: EngineCommand) {
        match cmd {
            EngineCommand::Play => self.transport.play(),
            EngineCommand::Pause => self.transport.pause(),
            EngineCommand::Stop => self.transport.stop(),
            EngineCommand::SetTempo(bpm) => self.transport.set_tempo(bpm),
            EngineCommand::SetSwing(percent) => self.sequencer.set_swing(percent),
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
            EngineCommand::AddChannel { source } => {
                let channel = self.sequencer.active_channels();
                if let Some(strip) = self.strips.get_mut(channel) {
                    strip.reset_slot(source, &mut self.reclaim);
                    self.sequencer.clear_channel(channel);
                    self.sequencer.set_active_channels(channel + 1);
                }
            }
            EngineCommand::RemoveChannel => {
                let active = self.sequencer.active_channels();
                if let Some(channel) = active.checked_sub(1) {
                    self.strips[channel].reset_slot(DeviceKind::Sampler, &mut self.reclaim);
                    self.sequencer.set_active_channels(channel);
                }
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
                    strip.sampler.set_params(params);
                }
            }
            EngineCommand::SetChannelSource { channel, source } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.reset_sources_to_defaults(source);
                }
            }
            EngineCommand::SetChannelDrumSynthParams { channel, params } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.drum_synth.set_params(params);
                }
            }
            EngineCommand::SetChannelMonoSynthParams { channel, params } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.mono_synth.set_params(params);
                }
            }
            EngineCommand::SwapEffectSlots {
                channel,
                slot_a,
                slot_b,
            } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.swap_effects(slot_a as usize, slot_b as usize);
                }
            }
            EngineCommand::SetEffectBypassed {
                channel,
                slot,
                bypassed,
            } => {
                if let Some(flag) = self
                    .strips
                    .get_mut(channel as usize)
                    .and_then(|strip| strip.effect_bypassed.get_mut(slot as usize))
                {
                    *flag = bypassed;
                }
            }
            EngineCommand::SetEffectParam {
                channel,
                slot,
                id,
                value,
            } => {
                if let Some(events) = self
                    .strips
                    .get_mut(channel as usize)
                    .and_then(|strip| strip.effect_events.get_mut(slot as usize))
                {
                    // Queued between blocks, so it lands at the next block's
                    // first frame. If the slot's list is full the update is
                    // dropped, matching the command ring's overflow policy.
                    let _ = events.push_ordered(TimedEvent {
                        offset: 0,
                        event: Event::ParamValue { id, value },
                    });
                }
            }
            EngineCommand::InstallProject { .. } => {}
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
                    choke_groups[index] = strip.choke_group();
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
            strip.process(&context, &self.events[index]);
            for slot in 0..MAX_EFFECTS_PER_CHANNEL {
                if !strip.effect_bypassed[slot] {
                    if let Some(node) = &mut strip.effect_chain[slot] {
                        node.process(&context, &mut strip.bus, &strip.effect_events[slot], None);
                    }
                }
                // Bypassed slots keep their queued events until re-enabled,
                // so knob turns made while bypassed are not lost.
                if !strip.effect_bypassed[slot] {
                    strip.effect_events[slot].clear();
                }
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
    use mooloop_core::{NoteEvent, ProjectChannel};

    fn test_strip() -> ChannelStrip {
        let slot = Arc::new(ArcSwapOption::empty());
        ChannelStrip::new(slot, 48_000)
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
    fn choke_is_ordered_before_a_simultaneous_note_on() {
        let mut events = [EventList::empty(), EventList::empty()];
        for (channel, id) in events.iter_mut().zip([1, 2]) {
            channel.push(TimedEvent {
                offset: 0,
                event: Event::NoteOn {
                    id,
                    note: 60,
                    velocity: 100,
                },
            });
        }

        inject_choke_events(&[1, 1], &mut events);

        for channel in &events {
            assert!(matches!(channel.iter().next().unwrap().event, Event::Choke));
            assert!(matches!(
                channel.iter().nth(1).unwrap().event,
                Event::NoteOn { .. }
            ));
        }
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

    fn synth_project(channel: ProjectChannel) -> Project {
        let mut project = Project {
            channels: vec![channel],
            ..Project::default()
        };
        project.channels[0].notes[0].push(NoteEvent::new(1, 0, 96, 60, 127));
        project
    }

    #[test]
    fn synth_sources_render_without_sample_data() {
        for channel in [
            ProjectChannel::drum_synth(0, 1),
            ProjectChannel::mono_synth(0, 1),
        ] {
            let project = synth_project(channel);
            let mut render = RenderState::from_project(48_000, &project, &[]);
            render.play();
            let report = render.process_block(512);
            assert!(report.peak_l > 0.001, "synth source was silent");
        }
    }

    #[test]
    fn source_switch_resets_inactive_voice_state() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        assert!(render.process_block(256).peak_l > 0.001);

        render.apply_command(EngineCommand::SetChannelSource {
            channel: 0,
            source: DeviceKind::MonoSynth,
        });
        render.process_block(256);
        render.apply_command(EngineCommand::SetChannelSource {
            channel: 0,
            source: DeviceKind::Sampler,
        });
        assert_eq!(render.process_block(256).peak_l, 0.0);
    }

    #[test]
    fn readding_a_channel_resets_its_preallocated_slot() {
        let mut render = RenderState::from_project(48_000, &Project::default(), &[]);
        render.apply_command(EngineCommand::AddChannel {
            source: DeviceKind::DrumSynth,
        });
        render.apply_command(EngineCommand::SetStep {
            pattern: 0,
            channel: 1,
            step: 0,
            on: true,
            note: 60,
            velocity: 127,
        });
        render.play();
        assert!(render.process_block(256).peak_l > 0.001);

        render.apply_command(EngineCommand::RemoveChannel);
        render.apply_command(EngineCommand::AddChannel {
            source: DeviceKind::DrumSynth,
        });
        render.apply_command(EngineCommand::Stop);
        render.apply_command(EngineCommand::Play);
        assert_eq!(render.process_block(256).peak_l, 0.0);
    }

    #[test]
    fn mixed_source_project_renders_all_preallocated_nodes() {        let mut project = Project {
            channels: vec![
                ProjectChannel::sampler(0, 1),
                ProjectChannel::drum_synth(1, 1),
                ProjectChannel::mono_synth(2, 1),
            ],
            ..Project::default()
        };
        for (index, channel) in project.channels.iter_mut().enumerate() {
            channel.notes[0].push(NoteEvent::new(index as u32 + 1, 0, 96, 60, 127));
        }
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        assert!(render.process_block(512).peak_l > 0.01);
    }

    fn rendered_energy(project: &Project, configure: impl FnOnce(&mut RenderState)) -> f32 {
        let mut render = RenderState::from_project(48_000, project, &[]);
        configure(&mut render);
        render.play();
        render.process_block(1024);
        let master = render.master();
        master.l[..1024].iter().map(|s| s * s).sum::<f32>()
    }

    fn muffling_filter() -> Box<dyn AudioNode + Send> {
        build_effect(
            mooloop_core::EffectParams::Filter(mooloop_core::FilterParams {
                cutoff_hz: 100.0,
                resonance: 0.0,
                mode: mooloop_core::FilterMode::LowPass,
            }),
            48_000,
        )
    }

    fn default_effect(kind: mooloop_core::EffectKind) -> Box<dyn AudioNode + Send> {
        build_effect(kind.default_params(), 48_000)
    }

    #[test]
    fn installed_filter_changes_channel_output() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let dry = rendered_energy(&project, |_| {});
        let filtered = rendered_energy(&project, |render| {
            render.apply_structural(StructuralCommand::InstallEffect {
                channel: 0,
                slot: 0,
                node: muffling_filter(),
            });
        });
        assert!(dry > 0.0, "reference render was silent");
        assert!(
            filtered < dry * 0.5,
            "100 Hz low-pass should eat most of a kick: dry {dry}, filtered {filtered}"
        );
    }

    /// Every effect kind must be constructible through the shared builder and
    /// audibly change the signal at a setting that is obviously not neutral.
    /// This is the test a new kind trips if it is added to `EffectKind` but
    /// never wired into `build_effect`.
    #[test]
    fn every_effect_kind_installs_and_alters_the_signal() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let dry = rendered_energy(&project, |_| {});
        assert!(dry > 0.0, "reference render was silent");

        for kind in mooloop_core::EffectKind::ALL {
            // Push each kind well away from neutral; defaults are chosen to
            // be transparent, so they would prove nothing here.
            let mut params = kind.default_params();
            match kind {
                mooloop_core::EffectKind::Filter => {
                    params.set(mooloop_core::FILTER_PARAM_CUTOFF_HZ, 100.0);
                }
                mooloop_core::EffectKind::Drive => {
                    params.set(mooloop_core::DRIVE_PARAM_DRIVE, 64.0);
                }
                mooloop_core::EffectKind::Bitcrush => {
                    params.set(mooloop_core::BITCRUSH_PARAM_BITS, 1.0);
                    params.set(mooloop_core::BITCRUSH_PARAM_DOWNSAMPLE, 32.0);
                }
                mooloop_core::EffectKind::Gate => {
                    // Threshold at the top of its range shuts on anything.
                    params.set(mooloop_core::GATE_PARAM_THRESHOLD_DB, 0.0);
                    params.set(mooloop_core::GATE_PARAM_ATTACK_MS, 0.05);
                }
                mooloop_core::EffectKind::Compressor => {
                    params.set(mooloop_core::COMP_PARAM_THRESHOLD_DB, -40.0);
                    params.set(mooloop_core::COMP_PARAM_RATIO, 20.0);
                    params.set(mooloop_core::COMP_PARAM_ATTACK_MS, 0.05);
                }
                mooloop_core::EffectKind::Limiter => {
                    params.set(mooloop_core::LIMITER_PARAM_CEILING_DB, -24.0);
                }
                mooloop_core::EffectKind::Delay => {
                    // Short enough that a repeat lands inside the rendered
                    // block, fully wet so the dry signal cannot mask it.
                    params.set(mooloop_core::DELAY_PARAM_TIME_MS, 5.0);
                    params.set(mooloop_core::DELAY_PARAM_FEEDBACK, 0.6);
                    params.set(mooloop_core::DELAY_PARAM_MIX, 1.0);
                }
            }

            let wet = rendered_energy(&project, |render| {
                render.apply_structural(StructuralCommand::InstallEffect {
                    channel: 0,
                    slot: 0,
                    node: build_effect(params, 48_000),
                });
            });
            assert!(
                (wet - dry).abs() > dry * 0.01,
                "{} left the signal unchanged: dry {dry}, wet {wet}",
                kind.label()
            );
        }
    }

    #[test]
    fn effect_param_bypass_and_reorder_plumbing() {
        let project = synth_project(ProjectChannel::sampler(0, 1));

        // A wide-open filter passes (nearly) everything; closing it via a
        // queued ParamValue event must change the output.
        let open = rendered_energy(&project, |render| {
            render.apply_structural(StructuralCommand::InstallEffect {
                channel: 0,
                slot: 0,
                node: default_effect(mooloop_core::EffectKind::Filter),
            });
        });
        let closed = rendered_energy(&project, |render| {
            render.apply_structural(StructuralCommand::InstallEffect {
                channel: 0,
                slot: 0,
                node: default_effect(mooloop_core::EffectKind::Filter),
            });
            render.apply_command(EngineCommand::SetEffectParam {
                channel: 0,
                slot: 0,
                id: mooloop_core::FILTER_PARAM_CUTOFF_HZ,
                value: 100.0,
            });
        });
        assert!(closed < open * 0.5, "open {open}, closed {closed}");

        // Bypass restores the dry sound.
        let bypassed = rendered_energy(&project, |render| {
            render.apply_structural(StructuralCommand::InstallEffect {
                channel: 0,
                slot: 0,
                node: muffling_filter(),
            });
            render.apply_command(EngineCommand::SetEffectBypassed {
                channel: 0,
                slot: 0,
                bypassed: true,
            });
        });
        let dry = rendered_energy(&project, |_| {});
        let ratio = bypassed / dry;
        assert!(
            (0.9..1.1).contains(&ratio),
            "bypassed should match dry: {bypassed} vs {dry}"
        );

        // Swapping an occupied slot with an empty one moves the filter.
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.apply_structural(StructuralCommand::InstallEffect {
            channel: 0,
            slot: 0,
            node: muffling_filter(),
        });
        render.apply_command(EngineCommand::SwapEffectSlots {
            channel: 0,
            slot_a: 0,
            slot_b: 3,
        });
        render.play();
        render.process_block(1024);
        let moved = render.master().l[..1024].iter().map(|s| s * s).sum::<f32>();
        assert!(moved < dry * 0.5, "filter should still muffle after swap");

        // Removing the slot reclaims the node instead of dropping it here.
        render.apply_structural(StructuralCommand::RemoveEffect {
            channel: 0,
            slot: 3,
        });
        assert_eq!(render.take_reclaim().len(), 1);
        assert!(render.take_reclaim().is_empty());
    }
}
