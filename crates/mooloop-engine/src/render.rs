//! JACK-independent render state shared by realtime playback and file export.

use std::sync::Arc;

use arc_swap::ArcSwapOption;
use mooloop_core::{
    compile_render_order, default_render_order, sanitize_route, ChannelSource, DeviceKind,
    DrumSynthParams,
    EffectTarget, EngineCommand, MonoSynthParams, Project, RenderOrder, SamplerParams,
    DEFAULT_STEPS, MASTER_BUS, MAX_BUSES, MAX_CHANNELS, MAX_EFFECTS_PER_CHANNEL,
};
use mooloop_dsp::{
    build_effect, pan_gains, AudioNode, DrumSynth, Event, EventList, MonoSynth, ProcessContext,
    SampleData, Sampler, StereoBus, TimedEvent, MAX_BLOCK_SIZE,
};

use crate::meters::BusMeters;
use crate::sequencer::Sequencer;
use crate::transport::Transport;
use crate::StructuralCommand;

/// Nodes displaced from effect slots, handed back so the non-realtime side
/// can drop them. The realtime thread must never free a `Box` itself.
type Reclaim = Vec<Box<dyn AudioNode + Send>>;

/// A fixed-size chain of optional effect nodes plus the per-slot machinery
/// that feeds them. Channels and mixer buses both own one, which is the whole
/// reason effect commands address an `EffectTarget` rather than a channel.
struct EffectChain {
    /// Processed in order after whatever produced the audio. Slots are `None`
    /// until a node is installed structurally.
    nodes: [Option<Box<dyn AudioNode + Send>>; MAX_EFFECTS_PER_CHANNEL],
    /// Per-slot parameter events, queued between blocks by
    /// `EngineCommand::SetEffectParam` and consumed by the next block. Kept
    /// separate from the note-event lists so slot addressing is trivial and
    /// generators never see effect events.
    events: [EventList; MAX_EFFECTS_PER_CHANNEL],
    bypassed: [bool; MAX_EFFECTS_PER_CHANNEL],
}

impl EffectChain {
    fn new() -> Self {
        Self {
            nodes: std::array::from_fn(|_| None),
            events: std::array::from_fn(|_| EventList::empty()),
            bypassed: [false; MAX_EFFECTS_PER_CHANNEL],
        }
    }

    /// Remove every node, queuing the boxes for off-thread disposal.
    fn clear(&mut self, reclaim: &mut Reclaim) {
        for slot in &mut self.nodes {
            if let Some(node) = slot.take() {
                reclaim.push(node);
            }
        }
        for events in &mut self.events {
            events.clear();
        }
        self.bypassed = [false; MAX_EFFECTS_PER_CHANNEL];
    }

    fn install(&mut self, slot: usize, node: Box<dyn AudioNode + Send>, reclaim: &mut Reclaim) {
        if let Some(target) = self.nodes.get_mut(slot) {
            if let Some(old) = target.replace(node) {
                reclaim.push(old);
            }
        }
    }

    fn remove(&mut self, slot: usize, reclaim: &mut Reclaim) {
        if let Some(target) = self.nodes.get_mut(slot) {
            if let Some(old) = target.take() {
                reclaim.push(old);
            }
        }
        if let Some(events) = self.events.get_mut(slot) {
            events.clear();
        }
        if let Some(bypassed) = self.bypassed.get_mut(slot) {
            *bypassed = false;
        }
    }

    fn swap(&mut self, slot_a: usize, slot_b: usize) {
        if slot_a < MAX_EFFECTS_PER_CHANNEL && slot_b < MAX_EFFECTS_PER_CHANNEL {
            self.nodes.swap(slot_a, slot_b);
            self.events.swap(slot_a, slot_b);
            self.bypassed.swap(slot_a, slot_b);
        }
    }

    fn set_bypassed(&mut self, slot: usize, bypassed: bool) {
        if let Some(flag) = self.bypassed.get_mut(slot) {
            *flag = bypassed;
        }
    }

    fn queue_param(&mut self, slot: usize, id: u32, value: f32) {
        if let Some(events) = self.events.get_mut(slot) {
            // Queued between blocks, so it lands at the next block's first
            // frame. If the slot's list is full the update is dropped,
            // matching the command ring's overflow policy.
            let _ = events.push_ordered(TimedEvent {
                offset: 0,
                event: Event::ParamValue { id, value },
            });
        }
    }

    /// Load a project's saved chain. Construction allocates, so this is a
    /// load-time operation only, never a per-block one.
    fn load(&mut self, slots: &[mooloop_core::EffectSlotState], sample_rate: u32, reclaim: &mut Reclaim) {
        self.clear(reclaim);
        for (slot, effect) in slots.iter().take(MAX_EFFECTS_PER_CHANNEL).enumerate() {
            self.install(slot, build_effect(effect.params, sample_rate), reclaim);
            self.bypassed[slot] = effect.bypassed;
        }
    }

    fn process(&mut self, context: &ProcessContext, bus: &mut StereoBus) {
        for slot in 0..MAX_EFFECTS_PER_CHANNEL {
            if self.bypassed[slot] {
                // A bypassed slot keeps its queued events until re-enabled, so
                // knob turns made while bypassed are not lost.
                continue;
            }
            if let Some(node) = &mut self.nodes[slot] {
                node.process(context, bus, &self.events[slot], None);
            }
            self.events[slot].clear();
        }
    }
}

/// Shared output stage: linear gain, constant-power pan, and a mute that stops
/// the strip contributing without stopping it processing (so effect tails on a
/// muted strip still decay instead of freezing).
struct OutputStage {
    gain: f32,
    pan: f32,
    muted: bool,
}

impl OutputStage {
    fn new(gain: f32) -> Self {
        Self {
            gain,
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

    fn apply(&self, bus: &mut StereoBus, frames: usize) {
        let (pan_l, pan_r) = pan_gains(self.pan);
        bus.apply_stereo_gain(self.gain * pan_l, self.gain * pan_r, frames);
    }
}

/// One mixer bus: an effect chain, an output stage, and the index of the bus
/// it feeds. `output` is always lower than the bus's own index (see
/// `mooloop_core::mixer`), which is what lets `process_block` render the whole
/// bank in one descending pass with no sorting or scratch buffers.
struct BusStrip {
    effects: EffectChain,
    bus: StereoBus,
    output: OutputStage,
    destination: u8,
}

impl BusStrip {
    fn new() -> Self {
        Self {
            effects: EffectChain::new(),
            bus: StereoBus::with_capacity(MAX_BLOCK_SIZE),
            // Unity, not a channel's 0.8: see `mooloop_core::MixerBus::new`.
            output: OutputStage::new(1.0),
            destination: MASTER_BUS,
        }
    }

    fn reset(&mut self, reclaim: &mut Reclaim) {
        self.effects.clear(reclaim);
        self.output = OutputStage::new(1.0);
        self.destination = MASTER_BUS;
    }
}

struct ChannelStrip {
    sampler: Sampler,
    drum_synth: DrumSynth,
    mono_synth: MonoSynth,
    active_source: DeviceKind,
    effects: EffectChain,
    bus: StereoBus,
    output: OutputStage,
    /// Mixer bus this channel feeds.
    destination: u8,
}

impl ChannelStrip {
    fn new(sample_slot: Arc<ArcSwapOption<SampleData>>, sample_rate: u32) -> Self {
        Self {
            sampler: Sampler::new(sample_slot, SamplerParams::default(), sample_rate),
            drum_synth: DrumSynth::new(DrumSynthParams::default(), sample_rate),
            mono_synth: MonoSynth::new(MonoSynthParams::default(), sample_rate),
            active_source: DeviceKind::Sampler,
            effects: EffectChain::new(),
            bus: StereoBus::with_capacity(MAX_BLOCK_SIZE),
            output: OutputStage::new(0.8),
            destination: MASTER_BUS,
        }
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
        self.effects.clear(reclaim);
        self.output = OutputStage::new(0.8);
        self.destination = MASTER_BUS;
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

/// Sum one bus into another. The two indices are unrelated now that routing
/// is arbitrary, so the disjoint borrow is taken by splitting at whichever is
/// higher rather than assuming the destination is lower.
fn mix_into(buses: &mut [BusStrip], from: usize, into: usize, frames: usize) {
    if from == into || from >= buses.len() || into >= buses.len() {
        return;
    }
    let (left, right) = buses.split_at_mut(from.max(into));
    if from < into {
        let source = &left[from];
        right[0].bus.add_from(&source.bus, frames);
    } else {
        let source = &right[0];
        left[into].bus.add_from(&source.bus, frames);
    }
}

/// Keep a channel's bus assignment inside the bank. A stale index from the GUI
/// lands on the master rather than silently muting the channel.
fn clamp_bus(bus: u8) -> u8 {
    if (bus as usize) < MAX_BUSES {
        bus
    } else {
        MASTER_BUS
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
    /// The full bus bank, master first. Always `MAX_BUSES` long, so assigning
    /// a channel to any bus is a bounded mutation rather than an allocation.
    buses: Vec<BusStrip>,
    /// Order the bank is rendered in, sources before destinations. Compiled
    /// off the audio thread by `mooloop_core::compile_render_order`; this side
    /// only ever walks it.
    render_order: RenderOrder,
    events: Vec<EventList>,
    sample_rate: u32,
    /// Nodes displaced from effect slots this block, awaiting handoff to the
    /// reclaim ring (realtime playback) or plain drop (offline render).
    reclaim: Reclaim,
    /// Where per-bus peaks are published for the mixer. Offline renders keep
    /// their own unread instance rather than paying for an `Option` check per
    /// bus per block.
    meters: Arc<BusMeters>,
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
            buses: (0..MAX_BUSES).map(|_| BusStrip::new()).collect(),
            render_order: default_render_order(),
            events: (0..MAX_CHANNELS).map(|_| EventList::empty()).collect(),
            sample_rate,
            reclaim: Vec::new(),
            meters: BusMeters::new(),
        }
    }

    /// Point bus metering at the array the GUI reads. Called once at startup,
    /// before the realtime thread exists.
    pub(crate) fn attach_meters(&mut self, meters: Arc<BusMeters>) {
        self.meters = meters;
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
                strip.output.muted = channel.setup.channel.muted;
                strip.output.set_volume(channel.setup.channel.volume);
                strip.output.set_pan(channel.setup.channel.pan);
                strip.destination = clamp_bus(channel.setup.channel.bus);
                // Project loading is a transport-stop/load-time operation,
                // not a hot per-block path, so constructing the boxed nodes
                // here is acceptable (see docs/EFFECTS_PLAN.md). Displaced
                // nodes still go through reclaim rather than being dropped.
                strip.effects
                    .load(&channel.setup.effects, self.sample_rate, &mut self.reclaim);
            } else {
                strip.reset_slot(DeviceKind::Sampler, &mut self.reclaim);
            }
        }
        for (index, strip) in self.buses.iter_mut().enumerate() {
            match project.buses.get(index) {
                Some(setup) => {
                    strip.output.muted = setup.bus.muted;
                    strip.output.set_volume(setup.bus.volume);
                    strip.output.set_pan(setup.bus.pan);
                    strip.destination = setup.bus.output;
                    strip
                        .effects
                        .load(&setup.effects, self.sample_rate, &mut self.reclaim);
                }
                None => strip.reset(&mut self.reclaim),
            }
        }
        // A file whose routing does not sort is repaired to everything-to-master
        // rather than rejected, so a hand-edited or future-format song still
        // opens and makes sound.
        self.render_order = match compile_render_order(&project.buses) {
            Some(order) => order,
            None => {
                for strip in &mut self.buses {
                    strip.destination = MASTER_BUS;
                }
                default_render_order()
            }
        };
    }

    /// Resolve an effect address to the chain that owns it. Both arms are
    /// bounds-checked, so a stale index from the GUI is a no-op rather than a
    /// panic on the audio thread.
    fn chain_for<'a>(
        strips: &'a mut [ChannelStrip],
        buses: &'a mut [BusStrip],
        target: EffectTarget,
    ) -> Option<&'a mut EffectChain> {
        match target {
            EffectTarget::Channel(index) => {
                strips.get_mut(index as usize).map(|s| &mut s.effects)
            }
            EffectTarget::Bus(index) => buses.get_mut(index as usize).map(|b| &mut b.effects),
        }
    }

    fn chain_mut(&mut self, target: EffectTarget) -> Option<&mut EffectChain> {
        Self::chain_for(&mut self.strips, &mut self.buses, target)
    }

    /// Apply a structural change (install/remove of a boxed node). Called on
    /// the realtime thread from the structural ring drain; the box itself was
    /// allocated on the GUI thread.
    pub(crate) fn apply_structural(&mut self, cmd: StructuralCommand) {
        match cmd {
            StructuralCommand::InstallEffect { target, slot, node } => {
                // `chain_for` borrows the two strip vectors rather than all of
                // `self`, so `reclaim` stays independently borrowable here.
                if let Some(chain) = Self::chain_for(&mut self.strips, &mut self.buses, target) {
                    chain.install(slot as usize, node, &mut self.reclaim);
                }
            }
            StructuralCommand::RemoveEffect { target, slot } => {
                if let Some(chain) = Self::chain_for(&mut self.strips, &mut self.buses, target) {
                    chain.remove(slot as usize, &mut self.reclaim);
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
                    strip.output.muted = muted;
                }
            }
            EngineCommand::SetChannelVolume { channel, volume } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.output.set_volume(volume);
                }
            }
            EngineCommand::SetChannelPan { channel, pan } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.output.set_pan(pan);
                }
            }
            EngineCommand::SetChannelBus { channel, bus } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.destination = clamp_bus(bus);
                }
            }
            EngineCommand::SetBusMuted { bus, muted } => {
                if let Some(strip) = self.buses.get_mut(bus as usize) {
                    strip.output.muted = muted;
                }
            }
            EngineCommand::SetBusVolume { bus, volume } => {
                if let Some(strip) = self.buses.get_mut(bus as usize) {
                    strip.output.set_volume(volume);
                }
            }
            EngineCommand::SetBusPan { bus, pan } => {
                if let Some(strip) = self.buses.get_mut(bus as usize) {
                    strip.output.set_pan(pan);
                }
            }
            EngineCommand::SetBusOutput { bus, output, order } => {
                if let Some(strip) = self.buses.get_mut(bus as usize) {
                    // Sanitized again here rather than trusted from the
                    // sender: an out-of-range destination would fall through
                    // `mix_into`'s bounds check and silently drop this bus's
                    // audio, which is a worse failure than mis-routing it.
                    strip.destination = sanitize_route(bus, output);
                }
                // The edge and the schedule that accounts for it travel
                // together, so no block can ever render a route with a stale
                // order.
                self.render_order = order;
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
                target,
                slot_a,
                slot_b,
            } => {
                if let Some(chain) = self.chain_mut(target) {
                    chain.swap(slot_a as usize, slot_b as usize);
                }
            }
            EngineCommand::SetEffectBypassed {
                target,
                slot,
                bypassed,
            } => {
                if let Some(chain) = self.chain_mut(target) {
                    chain.set_bypassed(slot as usize, bypassed);
                }
            }
            EngineCommand::SetEffectParam {
                target,
                slot,
                id,
                value,
            } => {
                if let Some(chain) = self.chain_mut(target) {
                    chain.queue_param(slot as usize, id, value);
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
                if !strip.output.muted {
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
        for strip in &mut self.buses {
            strip.bus.clear(frames);
        }
        for (index, strip) in self
            .strips
            .iter_mut()
            .enumerate()
            .take(self.sequencer.active_channels())
        {
            if strip.output.muted {
                continue;
            }
            strip.bus.clear(frames);
            strip.process(&context, &self.events[index]);
            strip.effects.process(&context, &mut strip.bus);
            strip.output.apply(&mut strip.bus, frames);
            if let Some(destination) = self.buses.get_mut(strip.destination as usize) {
                destination.bus.add_from(&strip.bus, frames);
            }
        }

        // Walk the compiled schedule. Every bus is guaranteed to appear after
        // everything feeding it, so one pass suffices whatever the routing
        // looks like; the master sorts last and keeps its audio, since it is
        // what the caller reads.
        let mut master_peak = (0.0, 0.0);
        for slot in 0..self.buses.len() {
            let index = self.render_order[slot] as usize;
            let Some(strip) = self.buses.get_mut(index) else {
                continue;
            };
            strip.effects.process(&context, &mut strip.bus);
            strip.output.apply(&mut strip.bus, frames);
            // A muted bus still processes, so a delay or reverb tail on it
            // decays instead of freezing, but contributes nothing — and meters
            // as silent, matching what is heard rather than what is running.
            let (peak_l, peak_r) = if strip.output.muted {
                (0.0, 0.0)
            } else {
                strip.bus.peak(frames)
            };
            self.meters.publish(index, peak_l, peak_r);

            if index == MASTER_BUS as usize {
                master_peak = (peak_l, peak_r);
            } else if !strip.output.muted {
                let destination = strip.destination as usize;
                mix_into(&mut self.buses, index, destination, frames);
            }
        }
        let (peak_l, peak_r) = master_peak;
        RenderReport {
            position_tick: self.transport.position_ticks as u64,
            beat_in_bar: self.transport.beat_in_bar(),
            playing: self.transport.playing,
            peak_l,
            peak_r,
        }
    }

    pub fn master(&self) -> &StereoBus {
        &self.buses[MASTER_BUS as usize].bus
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
        strip.output.set_volume(2.0);
        strip.output.set_pan(-2.0);
        assert_eq!(strip.output.gain, 1.0);
        assert_eq!(strip.output.pan, -1.0);
        strip.output.set_volume(-1.0);
        strip.output.set_pan(2.0);
        assert_eq!(strip.output.gain, 0.0);
        assert_eq!(strip.output.pan, 1.0);
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

    /// Route `bus` into `output` the way the interface does: compile the
    /// schedule for the resulting graph and send both together.
    fn route(
        render: &mut RenderState,
        buses: &mut [mooloop_core::BusSetup],
        bus: u8,
        output: u8,
    ) {
        buses[bus as usize].bus.output = output;
        let order = compile_render_order(buses).expect("test graph should be acyclic");
        render.apply_command(EngineCommand::SetBusOutput { bus, output, order });
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
                target: EffectTarget::Channel(0),
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
                    target: EffectTarget::Channel(0),
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

    /// A channel routed to a bus must reach the master *through* that bus, so
    /// an effect inserted on the bus shapes everything feeding it.
    #[test]
    fn a_bus_effect_processes_every_channel_feeding_it() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let dry = rendered_energy(&project, |render| {
            render.apply_command(EngineCommand::SetChannelBus {
                channel: 0,
                bus: 3,
            });
        });
        assert!(dry > 0.0, "routing through a bus must not lose the signal");

        let filtered = rendered_energy(&project, |render| {
            render.apply_command(EngineCommand::SetChannelBus {
                channel: 0,
                bus: 3,
            });
            render.apply_structural(StructuralCommand::InstallEffect {
                target: EffectTarget::Bus(3),
                slot: 0,
                node: muffling_filter(),
            });
        });
        assert!(
            filtered < dry * 0.5,
            "bus filter should muffle the channel: dry {dry}, filtered {filtered}"
        );
    }

    #[test]
    fn muting_a_bus_silences_what_feeds_it() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let muted = rendered_energy(&project, |render| {
            render.apply_command(EngineCommand::SetChannelBus {
                channel: 0,
                bus: 2,
            });
            render.apply_command(EngineCommand::SetBusMuted {
                bus: 2,
                muted: true,
            });
        });
        assert_eq!(muted, 0.0);
    }

    #[test]
    fn bus_volume_scales_its_contribution() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let unity = rendered_energy(&project, |render| {
            render.apply_command(EngineCommand::SetChannelBus {
                channel: 0,
                bus: 1,
            });
            render.apply_command(EngineCommand::SetBusVolume {
                bus: 1,
                volume: 1.0,
            });
        });
        let halved = rendered_energy(&project, |render| {
            render.apply_command(EngineCommand::SetChannelBus {
                channel: 0,
                bus: 1,
            });
            render.apply_command(EngineCommand::SetBusVolume {
                bus: 1,
                volume: 0.5,
            });
        });
        // Energy is the square of amplitude, so halving the gain quarters it.
        let ratio = halved / unity;
        assert!((0.2..0.3).contains(&ratio), "expected ~0.25, got {ratio}");
    }

    /// A bus's input must be complete before it runs. Chain two and put the
    /// filter on the *second* hop: it can only be heard if the schedule
    /// rendered bus 5 first.
    #[test]
    fn a_bus_can_feed_another_bus() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let chained = rendered_energy(&project, |render| {
            let mut buses = mooloop_core::default_buses();
            render.apply_command(EngineCommand::SetChannelBus {
                channel: 0,
                bus: 5,
            });
            route(render, &mut buses, 5, 2);
        });
        assert!(chained > 0.0, "chained buses must still reach the master");

        let filtered = rendered_energy(&project, |render| {
            let mut buses = mooloop_core::default_buses();
            render.apply_command(EngineCommand::SetChannelBus {
                channel: 0,
                bus: 5,
            });
            route(render, &mut buses, 5, 2);
            render.apply_structural(StructuralCommand::InstallEffect {
                target: EffectTarget::Bus(2),
                slot: 0,
                node: muffling_filter(),
            });
        });
        assert!(
            filtered < chained * 0.5,
            "bus 5 must be rendered before bus 2: {chained} -> {filtered}"
        );
    }

    /// The whole point of compiling a schedule: routing a low-numbered bus
    /// into a high-numbered one is ordinary now. The old descending pass could
    /// not express this at all, and rewrote the edge to the master.
    #[test]
    fn a_bus_can_feed_a_higher_numbered_bus() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let chained = rendered_energy(&project, |render| {
            let mut buses = mooloop_core::default_buses();
            render.apply_command(EngineCommand::SetChannelBus {
                channel: 0,
                bus: 2,
            });
            route(render, &mut buses, 2, 9);
        });
        assert!(chained > 0.0, "an uphill route must still reach the master");

        let filtered = rendered_energy(&project, |render| {
            let mut buses = mooloop_core::default_buses();
            render.apply_command(EngineCommand::SetChannelBus {
                channel: 0,
                bus: 2,
            });
            route(render, &mut buses, 2, 9);
            render.apply_structural(StructuralCommand::InstallEffect {
                target: EffectTarget::Bus(9),
                slot: 0,
                node: muffling_filter(),
            });
        });
        assert!(
            filtered < chained * 0.5,
            "bus 9's filter must be in the path: {chained} -> {filtered}"
        );
    }

    /// A three-hop chain that runs against index order end to end, to prove
    /// the schedule is genuinely driving the pass rather than index order
    /// happening to agree with it.
    #[test]
    fn a_chain_that_runs_entirely_against_index_order_still_sums() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let dry = rendered_energy(&project, |_| {});
        let routed = rendered_energy(&project, |render| {
            let mut buses = mooloop_core::default_buses();
            render.apply_command(EngineCommand::SetChannelBus {
                channel: 0,
                bus: 1,
            });
            buses[1].bus.output = 4;
            buses[4].bus.output = 11;
            buses[11].bus.output = MASTER_BUS;
            let order = compile_render_order(&buses).expect("acyclic");
            for (bus, output) in [(1u8, 4u8), (4, 11), (11, MASTER_BUS)] {
                render.apply_command(EngineCommand::SetBusOutput { bus, output, order });
            }
        });
        // Three unity-gain buses in series should not change the level.
        let ratio = routed / dry;
        assert!(
            (0.9..1.1).contains(&ratio),
            "1 -> 4 -> 11 -> master should be level-neutral: {routed} vs {dry}"
        );
    }

    /// A cyclic project cannot be scheduled, so loading one must fall back to
    /// everything-to-master rather than dropping the audio or looping.
    #[test]
    fn a_cyclic_project_loads_as_everything_to_master() {
        let mut project = synth_project(ProjectChannel::sampler(0, 1));
        project.channels[0].setup.channel.bus = 3;
        project.buses[3].bus.output = 6;
        project.buses[6].bus.output = 3;

        let dry = {
            let mut clean = synth_project(ProjectChannel::sampler(0, 1));
            clean.buses = mooloop_core::default_buses();
            rendered_energy(&clean, |_| {})
        };
        let repaired = rendered_energy(&project, |_| {});
        let ratio = repaired / dry;
        assert!(
            (0.9..1.1).contains(&ratio),
            "a cyclic file should still play: {repaired} vs {dry}"
        );
    }

    /// An out-of-range bus index must not mute the channel; it lands on the
    /// master, which is the same thing a freshly-defaulted project does.
    #[test]
    fn an_out_of_range_bus_lands_on_the_master() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let dry = rendered_energy(&project, |_| {});
        let clamped = rendered_energy(&project, |render| {
            render.apply_command(EngineCommand::SetChannelBus {
                channel: 0,
                bus: 200,
            });
        });
        assert_eq!(clamped, dry);
    }

    /// The mixer's strips are only useful if the bus they name is the one
    /// being metered, so check that the audio shows up on the routed bus and
    /// the master and nowhere else.
    #[test]
    fn peaks_are_published_for_the_bus_that_carries_the_audio() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let meters = BusMeters::new();
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.attach_meters(meters.clone());
        render.apply_command(EngineCommand::SetChannelBus {
            channel: 0,
            bus: 6,
        });
        render.play();
        render.process_block(1024);

        assert!(meters.take(6).0 > 0.001, "the routed bus should meter");
        assert!(meters.take(MASTER_BUS as usize).0 > 0.001);
        assert_eq!(meters.take(5), (0.0, 0.0), "an unused bus must read silent");
    }

    #[test]
    fn a_muted_bus_meters_silent() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let meters = BusMeters::new();
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.attach_meters(meters.clone());
        render.apply_command(EngineCommand::SetChannelBus {
            channel: 0,
            bus: 6,
        });
        render.apply_command(EngineCommand::SetBusMuted {
            bus: 6,
            muted: true,
        });
        render.play();
        render.process_block(1024);
        assert_eq!(meters.take(6), (0.0, 0.0));
    }

    #[test]
    fn a_master_effect_chain_processes_the_whole_mix() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let dry = rendered_energy(&project, |_| {});
        let filtered = rendered_energy(&project, |render| {
            render.apply_structural(StructuralCommand::InstallEffect {
                target: EffectTarget::Bus(mooloop_core::MASTER_BUS),
                slot: 0,
                node: muffling_filter(),
            });
        });
        assert!(filtered < dry * 0.5, "dry {dry}, filtered {filtered}");
    }

    #[test]
    fn effect_param_bypass_and_reorder_plumbing() {
        let project = synth_project(ProjectChannel::sampler(0, 1));

        // A wide-open filter passes (nearly) everything; closing it via a
        // queued ParamValue event must change the output.
        let open = rendered_energy(&project, |render| {
            render.apply_structural(StructuralCommand::InstallEffect {
                target: EffectTarget::Channel(0),
                slot: 0,
                node: default_effect(mooloop_core::EffectKind::Filter),
            });
        });
        let closed = rendered_energy(&project, |render| {
            render.apply_structural(StructuralCommand::InstallEffect {
                target: EffectTarget::Channel(0),
                slot: 0,
                node: default_effect(mooloop_core::EffectKind::Filter),
            });
            render.apply_command(EngineCommand::SetEffectParam {
                target: EffectTarget::Channel(0),
                slot: 0,
                id: mooloop_core::FILTER_PARAM_CUTOFF_HZ,
                value: 100.0,
            });
        });
        assert!(closed < open * 0.5, "open {open}, closed {closed}");

        // Bypass restores the dry sound.
        let bypassed = rendered_energy(&project, |render| {
            render.apply_structural(StructuralCommand::InstallEffect {
                target: EffectTarget::Channel(0),
                slot: 0,
                node: muffling_filter(),
            });
            render.apply_command(EngineCommand::SetEffectBypassed {
                target: EffectTarget::Channel(0),
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
            target: EffectTarget::Channel(0),
            slot: 0,
            node: muffling_filter(),
        });
        render.apply_command(EngineCommand::SwapEffectSlots {
            target: EffectTarget::Channel(0),
            slot_a: 0,
            slot_b: 3,
        });
        render.play();
        render.process_block(1024);
        let moved = render.master().l[..1024].iter().map(|s| s * s).sum::<f32>();
        assert!(moved < dry * 0.5, "filter should still muffle after swap");

        // Removing the slot reclaims the node instead of dropping it here.
        render.apply_structural(StructuralCommand::RemoveEffect {
            target: EffectTarget::Channel(0),
            slot: 3,
        });
        assert_eq!(render.take_reclaim().len(), 1);
        assert!(render.take_reclaim().is_empty());
    }
}
