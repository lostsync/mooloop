//! The wire between the session and the audio engine.
//!
//! UI callbacks all run on one thread, but boxed structural edits and POD
//! commands used to enter separate relay queues and lose their relative
//! order. Everything below shares one queue so that ordering survives, and
//! the typed senders keep the convenient `.send(...)` shape the callers use.

use crate::channel::ChannelState;
use crate::project::ProjectEdit;
use mooloop_core::{
    chain_latency, compile_audio_graph, compile_bus_graph, compile_latency, CompiledAudioGraph,
    CompiledLatency, DeviceKind, EffectTarget, EngineCommand, OutletDescriptor, PublishesOutlets,
    SliceMap, MASTER_BUS, MAX_BUSES, MAX_CHANNELS,
};
use mooloop_dsp::{IntegerDelay, SampleData, StereoBus, MAX_BLOCK_SIZE};
use crate::session::Session;
use mooloop_engine::{AudioTapBank, EngineHandle, StructuralCommand};
use std::sync::Arc;

/// UI callbacks all run on one thread, but boxed structural edits and POD
/// commands used to enter separate relay queues and lose their relative
/// order. These typed senders share one queue while preserving the convenient
/// `.send(...)` call shape used by the callback wiring below.
///
/// The width is `StructuralCommand`'s, and `EngineCommand` (`bridge.rs`
/// documents what sets that) is the runner-up. This queue is drained on the
/// UI thread into the preallocated ring, so evening the variants out with a
/// `Box` would trade a fixed stack copy for an allocation per command and
/// cost the `Copy` the wiring relies on.
#[allow(clippy::large_enum_variant)]
pub enum PendingEngineMessage {
    Command(EngineCommand),
    ResizeBuffers {
        bpm: f64,
    },
    Structural(StructuralCommand),
    /// Adding a channel allocates its strip, event list and control-output
    /// buffer, so it is structural rather than POD. The pump expands it: the
    /// engine handle owns the sample slot the new strip needs.
    AddChannel {
        channel: usize,
        source: DeviceKind,
    },
    ProjectEdit(ProjectEdit),
    Audio(AudioAction),
    Telemetry(TelemetryAction),
    /// Linear preview gain. A plain value rather than a command because the
    /// engine reads it from a shared cell, live, while a preview plays.
    PreviewGain(f32),
}

/// Display subscriptions are handled by the pump, which exclusively owns the
/// engine handle. They observe a device's signal; they are not audio-thread
/// commands and never become modulation routes.
pub enum TelemetryAction {
    SetEffectSpectrumEnabled {
        target: EffectTarget,
        slot: u8,
        enabled: bool,
    },
}

/// One requested change from the Audio preferences page. These reach
/// `EngineHandle` directly rather than through `EngineCommand`: they are
/// non-realtime JACK API calls (port connect/disconnect, buffer resize), not
/// realtime-thread state, but `handle` still only lives inside the pump.
pub enum AudioAction {
    /// Apply settings loaded from disk at startup, before the user has
    /// touched the Audio page.
    ApplyPersisted(mooloop_engine::AudioConfig),
    /// Re-read the live JACK graph and driver status.
    RefreshTargets,
    SelectOutput {
        port_l: String,
        port_r: String,
    },
    SelectBufferSize(u32),
    SetAutoReconnect(bool),
}

#[derive(Clone)]
pub struct EngineCommandSender(pub std::sync::mpsc::Sender<PendingEngineMessage>);

impl EngineCommandSender {
    pub fn send(&self, command: EngineCommand) -> bool {
        self.0.send(PendingEngineMessage::Command(command)).is_ok()
    }

    pub fn resize_buffers(&self, bpm: f64) -> bool {
        self.0
            .send(PendingEngineMessage::ResizeBuffers { bpm })
            .is_ok()
    }
}

#[derive(Clone)]
pub struct StructuralCommandSender(pub std::sync::mpsc::Sender<PendingEngineMessage>);

impl StructuralCommandSender {
    pub fn send(&self, command: StructuralCommand) -> bool {
        self.0
            .send(PendingEngineMessage::Structural(command))
            .is_ok()
    }

    pub fn add_channel(&self, channel: usize, source: DeviceKind) -> bool {
        self.0
            .send(PendingEngineMessage::AddChannel { channel, source })
            .is_ok()
    }
}

#[derive(Clone)]
pub struct ProjectEditSender(pub std::sync::mpsc::Sender<PendingEngineMessage>);

impl ProjectEditSender {
    pub fn send(&self, edit: ProjectEdit) -> bool {
        self.0.send(PendingEngineMessage::ProjectEdit(edit)).is_ok()
    }
}

#[derive(Clone)]
pub struct AudioActionSender(pub std::sync::mpsc::Sender<PendingEngineMessage>);

impl AudioActionSender {
    pub fn send(&self, action: AudioAction) -> bool {
        self.0.send(PendingEngineMessage::Audio(action)).is_ok()
    }
}

#[derive(Clone)]
pub struct TelemetryActionSender(pub std::sync::mpsc::Sender<PendingEngineMessage>);

impl TelemetryActionSender {
    pub fn send(&self, action: TelemetryAction) -> bool {
        self.0.send(PendingEngineMessage::Telemetry(action)).is_ok()
    }
}

#[derive(Clone)]
pub struct PreviewSender(pub std::sync::mpsc::Sender<PendingEngineMessage>);

impl PreviewSender {
    pub fn send_gain(&self, gain: f32) -> bool {
        self.0.send(PendingEngineMessage::PreviewGain(gain)).is_ok()
    }
}

/// A channel's audio, on its way to the pump.
///
/// Neither half can ride the command ring: `EngineCommand` is `Copy` and
/// unboxed by design, and both of these live in `ArcSwap` slots the pump
/// exclusively owns. Same route the built-in sample reset already takes.
///
/// Both are always sent together because they are one fact: after a commit
/// the published buffer and the map that indexes it change at the same
/// instant, and delivering one without the other would leave the voice
/// reading markers that name frames in a buffer it no longer holds.
pub struct ChannelAudio {
    pub channel: usize,
    pub sample: Option<Arc<SampleData>>,
    pub slices: Option<Arc<SliceMap>>,
}

#[derive(Clone)]
pub struct ChannelAudioSender(pub std::sync::mpsc::Sender<ChannelAudio>);

pub fn publish_channel_audio_to(tx: &ChannelAudioSender, channel: usize, state: &ChannelState) {
    let _ = tx.0.send(ChannelAudio {
        channel,
        sample: state.published_sample().cloned(),
        slices: (!state.slices.is_empty()).then(|| Arc::new(state.slices.clone())),
    });
}

/// Where the transport is, in the terms the position readout shows.
pub struct TransportPosition {
    /// Step under the playhead, wrapped into the current pattern.
    pub step: i32,
    /// Position along the arrangement, or `None` in pattern mode.
    pub playlist_ticks: Option<i32>,
    pub bar: i32,
    pub beat: i32,
    pub tick: i32,
}

impl Session {
    /// What each producer must wait, from the project as it stands.
    ///
    /// Derived rather than tracked, because latency is a consequence of five
    /// different edits (installing, replacing or removing an effect; adding a
    /// channel; changing a channel's bus; changing a bus's output; loading a
    /// project) and a flag each of those had to remember to set is a list that
    /// grows silently. Deriving it costs a walk of the chains, which is a few
    /// hundred additions.
    pub fn latency_plan(&self) -> CompiledLatency {
        let graph = compile_bus_graph(&self.buses).unwrap_or_default();
        let mut channel_latency = [0u32; MAX_CHANNELS];
        let mut channel_bus = [MASTER_BUS; MAX_CHANNELS];
        for (index, channel) in self.channels.iter().take(MAX_CHANNELS).enumerate() {
            channel_latency[index] = chain_latency(&channel.effects);
            channel_bus[index] = channel.bus;
        }
        let mut bus_latency = [0u32; MAX_BUSES];
        for (index, bus) in self.buses.iter().take(MAX_BUSES).enumerate() {
            bus_latency[index] = chain_latency(&bus.effects);
        }
        compile_latency(&graph, &channel_latency, &channel_bus, &bus_latency)
    }

    /// Reconcile the engine's compensation delays with the plan.
    ///
    /// Called from the pump rather than from each edit, and that is the whole
    /// design: the plan is **global** — installing a Drive on channel 3 changes
    /// what channels 1, 2 and 4 owe, because it moves the master's arrival —
    /// so a per-edit call site would have to be added to every path that can
    /// move a chain, and forgetting one produces a misalignment nothing
    /// reports. Deriving and diffing once a tick cannot be forgotten, costs a
    /// comparison when nothing changed, and converges within one frame of any
    /// edit. A structural edit already interrupts the thing being edited, so
    /// that frame is not a cost anyone hears.
    ///
    /// Sends nothing when the plan is unchanged, which is every tick but the
    /// few after an edit. Deliberately does **not** mark the document dirty:
    /// this is derived state, not something the user did.
    pub fn sync_compensation(&mut self, handle: &mut EngineHandle) {
        let plan = self.latency_plan();
        if plan == self.compensation_sent {
            return;
        }
        let channels = self.channels.len().min(MAX_CHANNELS);
        for channel in 0..channels {
            let frames = plan.channel(channel);
            if frames != self.compensation_sent.channel(channel) {
                handle.send_structural(StructuralCommand::SetCompensation {
                    target: EffectTarget::Channel(channel as u8),
                    delay: IntegerDelay::new(frames).map(Box::new),
                });
            }
        }
        for bus in 0..MAX_BUSES {
            let frames = plan.bus(bus);
            if frames != self.compensation_sent.bus(bus) {
                handle.send_structural(StructuralCommand::SetCompensation {
                    target: EffectTarget::Bus(bus as u8),
                    delay: IntegerDelay::new(frames).map(Box::new),
                });
            }
        }
        self.compensation_sent = plan;
    }

    /// Which tracks need a second input accumulator: the ones something
    /// analog-summed actually reaches.
    ///
    /// Derived rather than tracked, for the reason [`Self::latency_plan`]
    /// gives. Three different edits change the answer -- switching a track's
    /// analog sum on or off, re-routing a track, and loading a project -- so
    /// a flag each of them had to remember to set is a list that grows
    /// silently.
    ///
    /// The master is included like any other track: it is the summing point a
    /// default project already has, which is what lets two tracks glue with
    /// nothing created and nothing placed in a chain.
    pub fn console_plan(&self) -> [bool; MAX_BUSES] {
        let mut wanted = [false; MAX_BUSES];
        let graph = compile_bus_graph(&self.buses).unwrap_or_default();
        for (index, setup) in self.buses.iter().enumerate().take(MAX_BUSES).skip(1) {
            if setup.bus.console {
                wanted[graph.destination(index) as usize] = true;
            }
        }
        wanted
    }

    /// Reconcile the engine's console accumulators with the plan.
    ///
    /// Called from the pump beside [`Self::sync_compensation`] and for the
    /// same reasons: deriving and diffing once a tick cannot be forgotten,
    /// costs a comparison when nothing changed, and converges within one
    /// frame of any edit.
    ///
    /// The buffers are allocated here, on the pump thread, and only for the
    /// buses the plan names: a project that has never switched console on
    /// allocates nothing and this sends nothing, which is the "free while it
    /// is out" rule `docs/plans/console/` is held to. Deliberately does not
    /// mark the document dirty -- this is derived state, not something the
    /// user did.
    pub fn sync_console_sums(&mut self, handle: &mut EngineHandle) {
        let plan = self.console_plan();
        if plan == self.console_sums_sent {
            return;
        }
        for (bus, (&wanted, &sent)) in
            plan.iter().zip(self.console_sums_sent.iter()).enumerate()
        {
            if wanted == sent {
                continue;
            }
            handle.send_structural(StructuralCommand::SetConsoleSum {
                bus: bus as u8,
                buffer: wanted.then(|| Box::new(StereoBus::with_capacity(MAX_BLOCK_SIZE))),
            });
        }
        self.console_sums_sent = plan;
    }

    /// The audio edges this project's channels compile to, from the model as
    /// it stands.
    ///
    /// Derived rather than tracked, for the reason [`Self::latency_plan`]
    /// gives: an edge's fate is a property of every channel at once -- one
    /// channel changing generator can refuse another channel's subscription,
    /// and breaking a ring can give a third one back -- so a flag each edit
    /// had to set is a list that grows silently.
    pub fn audio_graph_plan(&self) -> CompiledAudioGraph {
        let count = self.channels.len().min(MAX_CHANNELS);
        let mut subscriptions = [None; MAX_CHANNELS];
        let mut published: [&'static [OutletDescriptor]; MAX_CHANNELS] = [&[]; MAX_CHANNELS];
        for (index, channel) in self.channels.iter().take(count).enumerate() {
            subscriptions[index] = channel.generator_params().audio_subscription();
            published[index] = channel.kind.outlets();
        }
        compile_audio_graph(&subscriptions[..count], &published[..count])
    }

    /// Reconcile the engine's audio edges with the plan.
    ///
    /// Called from the pump beside [`Self::sync_compensation`] and for the
    /// same reasons: deriving and diffing once a tick cannot be forgotten,
    /// costs a comparison when nothing changed, and converges within one
    /// frame of any edit.
    ///
    /// The buffers are allocated here, on the pump thread, and only when the
    /// plan says somebody is listening: a project that has never authored an
    /// edge allocates nothing and this sends nothing. Deliberately does not
    /// mark the document dirty -- this is derived state, not something the
    /// user did.
    pub fn sync_audio_graph(&mut self, handle: &mut EngineHandle) {
        let plan = self.audio_graph_plan();
        if plan == self.audio_graph_sent {
            return;
        }
        handle.send_structural(StructuralCommand::SetAudioGraph {
            bank: Box::new(AudioTapBank::new(plan)),
        });
        self.audio_graph_sent = plan;
    }

    /// Applies one queued message that needs nothing but the engine handle.
    ///
    /// Returns whether the document just became dirty, which the caller turns
    /// into a title refresh -- once per drain rather than once per message.
    ///
    /// `ProjectEdit` and `Audio` are not handled here: both end in something
    /// the user sees, so both stay with the layer that can show it.
    pub fn apply_engine_message(
        &mut self,
        handle: &mut EngineHandle,
        message: PendingEngineMessage,
    ) -> bool {
        match message {
            PendingEngineMessage::Command(command) => {
                // Transport is not an edit: starting playback must not make
                // an untouched document look unsaved.
                let edits = !matches!(
                    command,
                    EngineCommand::Play | EngineCommand::Pause | EngineCommand::Stop
                );
                handle.send(command);
                edits && self.became_dirty()
            }
            PendingEngineMessage::PreviewGain(gain) => {
                handle.set_preview_gain(gain);
                false
            }
            PendingEngineMessage::ResizeBuffers { bpm } => {
                // Each replacement allocates its ring on the pump thread; the
                // ordered realtime queue then swaps the ready node at a block
                // boundary.
                for (target, slot, params) in self.buffer_effects() {
                    let _ = handle.replace_buffer(target, slot, params, params, bpm);
                }
                false
            }
            PendingEngineMessage::AddChannel { channel, source } => {
                handle.add_channel(channel, source);
                self.became_dirty()
            }
            PendingEngineMessage::Structural(command) => {
                handle.send_structural(command);
                // Any structural change is an unsaved edit.
                self.became_dirty()
            }
            PendingEngineMessage::Telemetry(TelemetryAction::SetEffectSpectrumEnabled {
                target,
                slot,
                enabled,
            }) => {
                handle.set_effect_spectrum_enabled(target, slot, enabled);
                false
            }
            // Both of these end in something the user sees; the view keeps
            // them. `apply_engine_message` is only reached for the rest.
            PendingEngineMessage::ProjectEdit(_) | PendingEngineMessage::Audio(_) => false,
        }
    }

    /// Marks the document edited, reporting whether that was news.
    fn became_dirty(&mut self) -> bool {
        let was_clean = !self.dirty;
        self.mark_dirty();
        was_clean
    }

    /// Every buffer device in the document, channel chains then bus chains.
    fn buffer_effects(&self) -> Vec<(EffectTarget, u8, mooloop_core::BufferParams)> {
        let channels = self.channels.iter().enumerate().flat_map(|(channel, state)| {
            state.effects.iter().enumerate().filter_map(move |(slot, effect)| {
                effect
                    .params
                    .buffer()
                    .copied()
                    .map(|params| (EffectTarget::Channel(channel as u8), slot as u8, params))
            })
        });
        let buses = self.buses.iter().enumerate().flat_map(|(bus, state)| {
            state.effects.iter().enumerate().filter_map(move |(slot, effect)| {
                effect
                    .params
                    .buffer()
                    .copied()
                    .map(|params| (EffectTarget::Bus(bus as u8), slot as u8, params))
            })
        });
        channels.chain(buses).collect()
    }

    /// Where a transport tick lands, in the readout's terms.
    ///
    /// In song mode the position is along the arrangement; in pattern mode it
    /// wraps inside the pattern on screen, which is why the two cannot share
    /// one modulus.
    pub fn transport_position(&self, tick: u64) -> TransportPosition {
        let length = self.pattern_lengths[self.current_pattern] as u64;
        let ticks_per_step = (mooloop_core::Ppq::DEFAULT.ticks_per_beat() / 4) as u64;
        let ticks_per_beat = mooloop_core::Ppq::DEFAULT.ticks_per_beat() as u64;
        let (position_ticks, playlist_ticks) = if self.song_mode {
            let position = tick % u64::from(self.song_length_ticks());
            (position, Some(position as i32))
        } else {
            (tick % (length * ticks_per_step), None)
        };
        let ticks_per_bar = u64::from(mooloop_core::TICKS_PER_BAR);
        let tick_in_bar = position_ticks % ticks_per_bar;
        TransportPosition {
            step: ((tick / ticks_per_step) % length) as i32,
            playlist_ticks,
            bar: (position_ticks / ticks_per_bar) as i32 + 1,
            beat: (tick_in_bar / ticks_per_beat) as i32 + 1,
            tick: (tick_in_bar % ticks_per_beat) as i32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{PatternPlacement, TICKS_PER_BAR, TICKS_PER_STEP};

    /// Pattern mode wraps inside the pattern on screen; song mode runs along
    /// the arrangement. The two cannot share one modulus, which is the whole
    /// reason this is not a single expression.
    #[test]
    fn the_readout_wraps_by_pattern_or_by_song_depending_on_the_transport() {
        let mut session = Session::default();
        let pattern_ticks = session.pattern_lengths[0] as u32 * TICKS_PER_STEP;

        // One tick past the end of the pattern is the top of it again.
        let wrapped = session.transport_position(u64::from(pattern_ticks));
        assert_eq!(wrapped.step, 0);
        assert_eq!(wrapped.bar, 1);
        assert_eq!(wrapped.playlist_ticks, None);

        // Two clips back to back, so the song is twice the pattern.
        session.song_mode = true;
        session.playlist = vec![
            PatternPlacement::new(0, 0),
            PatternPlacement::new(0, pattern_ticks),
        ];
        let along = session.transport_position(u64::from(pattern_ticks));
        assert_eq!(
            along.playlist_ticks,
            Some(pattern_ticks as i32),
            "song mode wrapped inside the pattern instead of along the song"
        );
        assert_eq!(along.bar, (pattern_ticks / TICKS_PER_BAR) as i32 + 1);
    }

    /// Bar, beat and tick are one-based where the readout shows them and
    /// zero-based where it does not, which is easy to get backwards.
    #[test]
    fn the_position_readout_counts_bars_and_beats_from_one() {
        let session = Session::default();
        let at_start = session.transport_position(0);
        assert_eq!((at_start.bar, at_start.beat, at_start.tick), (1, 1, 0));

        let ticks_per_beat = u64::from(TICKS_PER_BAR) / 4;
        let second_beat = session.transport_position(ticks_per_beat + 3);
        assert_eq!(
            (second_beat.bar, second_beat.beat, second_beat.tick),
            (1, 2, 3)
        );
    }

    /// The audio-edge plan is derived from the model the same way the
    /// compensation plan is, and this is the derivation the pump reconciles
    /// against. What matters is that it follows the *session's* channels --
    /// an edit path that changed `aux_in_params` without telling anybody is
    /// the failure this design exists to make impossible.
    #[test]
    fn the_audio_graph_plan_follows_the_session_rather_than_being_tracked() {
        use mooloop_core::{AudioSubscription, DeviceKind, EdgeRefusal};

        let mut session = Session::default();
        while session.channels.len() < 2 {
            let index = session.channels.len();
            session.channels.push(ChannelState::new(index));
        }
        session.reset_channel_source(0, DeviceKind::MlP8);
        session.reset_channel_source(1, DeviceKind::AuxIn);
        // Nothing subscribed: no edges, and nothing for the engine to hold.
        assert!(session.audio_graph_plan().is_empty());

        // The consumer's own parameters are the only thing that changes.
        session.channels[1]
            .aux_in_params
            .set_subscription(Some(AudioSubscription::new(
                0,
                mooloop_core::mlp8::OUTLET_OSC3,
            )));
        let plan = session.audio_graph_plan();
        assert_eq!(plan.tap_count(), 1);
        assert_eq!(
            plan.edge(1).resolved(),
            Some(AudioSubscription::new(0, mooloop_core::mlp8::OUTLET_OSC3))
        );

        // And a change on the *producer* refuses it, without the consumer
        // being touched: that is why the plan cannot be a flag each edit sets.
        session.reset_channel_source(0, DeviceKind::Sampler);
        let plan = session.audio_graph_plan();
        assert_eq!(plan.edge(1).refusal(), Some(EdgeRefusal::NotAProducer));
        assert_eq!(plan.tap_count(), 0, "a refused edge kept its buffer");
    }

    fn transparent_drive() -> mooloop_core::EffectSlotState {
        mooloop_core::EffectSlotState::drive(mooloop_core::DriveParams::default())
    }

    /// The plan is derived from the model rather than tracked alongside it,
    /// which is what makes it impossible for an edit path to forget. This is
    /// that derivation: what the session holds in, what each producer owes
    /// out.
    #[test]
    fn the_plan_is_derived_from_the_chains_as_they_stand() {
        let latency = mooloop_core::effect::OVERSAMPLER_LATENCY_FRAMES;
        let mut session = Session::default();
        session.add_channel(mooloop_core::DeviceKind::Sampler);
        assert_eq!(session.channels.len(), 2);

        // Nothing installed: nothing owed, by anybody.
        assert_eq!(session.latency_plan(), mooloop_core::CompiledLatency::default());

        // One latent device on channel 0 and its neighbour has to wait.
        session.channels[0].effects.push(transparent_drive().with_id(mooloop_core::DeviceId(0)));
        let plan = session.latency_plan();
        assert_eq!(plan.channel(0), 0, "the longest path must not move");
        assert_eq!(plan.channel(1), latency);
        assert_eq!(plan.total(), latency);

        // A bypassed slot still counts, which is what keeps toggling one from
        // moving the channel in time.
        session.channels[0].effects[0].bypassed = true;
        assert_eq!(session.latency_plan().channel(1), latency);

        // Removing it is what gives the latency back.
        session.channels[0].effects.clear();
        assert_eq!(session.latency_plan(), mooloop_core::CompiledLatency::default());
    }

    /// A bus's own chain is part of the tree, so a channel that does not go
    /// through it still waits for it. This is the case a plan derived from
    /// channels alone would miss.
    #[test]
    fn a_channel_waits_for_a_latent_bus_it_does_not_use() {
        let latency = mooloop_core::effect::OVERSAMPLER_LATENCY_FRAMES;
        let mut session = Session::default();
        session.add_channel(mooloop_core::DeviceKind::Sampler);
        session.channels[0].bus = 1;
        session.buses[1].effects.push(transparent_drive().with_id(mooloop_core::DeviceId(0)));

        let plan = session.latency_plan();
        assert_eq!(plan.channel(0), 0, "nothing else feeds bus 1");
        assert_eq!(plan.channel(1), latency, "the direct channel waits for the bus");
        assert_eq!(plan.total(), latency);
    }
}
