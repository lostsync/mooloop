//! The wire between the session and the audio engine.
//!
//! UI callbacks all run on one thread, but boxed structural edits and POD
//! commands used to enter separate relay queues and lose their relative
//! order. Everything below shares one queue so that ordering survives, and
//! the typed senders keep the convenient `.send(...)` shape the callers use.

use crate::channel::ChannelState;
use crate::project::ProjectEdit;
use mooloop_core::{
    chain_latency, compensable_send_edges, compile_audio_graph, compile_bus_graph,
    compile_latency, log_error, sends_are_compensable,
    CompiledAudioGraph,
    BbtPosition, CompiledLatency, DeviceKind, EffectTarget, EngineCommand, OutletDescriptor,
    PublishesOutlets, Ticks,
    MASTER_BUS, MAX_BUSES, MAX_CHANNELS,
};
use mooloop_dsp::{ChannelAudioSnapshot, IntegerDelay, StereoBus, MAX_BLOCK_SIZE};
use crate::session::Session;
use mooloop_engine::{AudioTapBank, CommandSink, EngineHandle, SendBank, SendSpec, StructuralCommand};
use std::sync::Arc;

/// What is *structural* about one send: where it goes and what it waits.
///
/// The key `Session::sync_track_graph` diffs on. Level, tap and enable are
/// deliberately absent -- they travel as POD commands, so including them here
/// would rebuild the whole plan, and every compensation ring in it, on every
/// frame of a send-fader drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendRoute {
    producer: EffectTarget,
    target: u8,
    delay: u32,
}

impl SendRoute {
    fn of(spec: &SendSpec) -> Self {
        Self {
            producer: spec.producer,
            target: spec.target,
            delay: spec.delay,
        }
    }
}

/// The compensation the engine has *acknowledged*, per target.
///
/// Not a [`CompiledLatency`], although it is diffed against one. A plan is
/// compiled as a whole and is true as a whole; this is a record of sixteen
/// plus eight independent deliveries, any subset of which the command ring
/// may have refused. Storing a plan here would force the mirror to advance
/// all-or-nothing, and `sync_compensation` sends per channel and per bus
/// inside a loop -- so one refusal in the middle would leave the mirror
/// claiming a plan that was only partly delivered, and the next tick's diff
/// would find nothing to resend.
///
/// The send entries of a plan are deliberately absent: they travel with the
/// track graph in [`Session::sync_track_graph`], not here, and a mirror that
/// held them would diff on a value this reconciler never sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompensationSent {
    channels: [u32; MAX_CHANNELS],
    buses: [u32; MAX_BUSES],
}

impl CompensationSent {
    /// What `plan` asks of each target, as this mirror records it.
    fn of(plan: &CompiledLatency) -> Self {
        let mut mirror = Self::default();
        for (channel, frames) in mirror.channels.iter_mut().enumerate() {
            *frames = plan.channel(channel);
        }
        for (bus, frames) in mirror.buses.iter_mut().enumerate() {
            *frames = plan.bus(bus);
        }
        mirror
    }

    pub fn channel(&self, channel: usize) -> u32 {
        self.channels.get(channel).copied().unwrap_or(0)
    }

    pub fn bus(&self, bus: usize) -> u32 {
        self.buses.get(bus).copied().unwrap_or(0)
    }
}

impl Default for CompensationSent {
    fn default() -> Self {
        Self {
            channels: [0; MAX_CHANNELS],
            buses: [0; MAX_BUSES],
        }
    }
}

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
    /// One Buffer's ring replaced because its `bars` changed.
    ///
    /// Distinct from [`Self::ResizeBuffers`], which rebuilds every ring at a
    /// new tempo and passes one params value as both the expectation and the
    /// replacement -- correct there, because a tempo change leaves `bars`
    /// alone and the allocation key with it. Here the key is precisely what
    /// moved, and the document already says the new number by the time this
    /// is drained, so the expectation has to travel with the message or the
    /// realtime side refuses the swap it was asked for.
    ResizeBuffer(BufferResize, f64),
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
    /// How each channel takes MIDI input, in channel order. A whole table
    /// rather than one channel's entry: it is one small struct per channel,
    /// it is rebuilt on a menu pick rather than in a loop, and a patching
    /// verb would be a second path to the same state.
    MidiRouting(Vec<mooloop_core::MidiInputRoute>),
    /// Every channel's audio input resolved to a seat, in channel order, by
    /// `Session::audio_input_taps`. A whole table for the reason
    /// [`Self::MidiRouting`] is one.
    AudioInputRouting(Vec<Option<mooloop_core::AudioTap>>),
}

impl PendingEngineMessage {
    /// Whether this message still means something after a load replaces the
    /// whole project.
    ///
    /// A load discards what is queued, because the prepared project already
    /// contains every edit (or deliberately supersedes it). That is true of
    /// anything addressed to the document, and not of the two kinds addressed
    /// to the machine: an Audio preferences action and the preview volume are
    /// in no project, so dropping one lost a buffer-size pick or a knob turn
    /// for good.
    ///
    /// Everything else is dropped, and some of it *must* be. A routing table
    /// or a spectrum subscription queued before the load is indexed by the
    /// outgoing project's channels and devices, and applied afterwards it
    /// would overwrite what the install carried for the incoming one. Record
    /// arm travels with the install itself (`mooloop_engine::InputState`).
    pub fn survives_project_load(&self) -> bool {
        matches!(self, Self::Audio(_) | Self::PreviewGain(_))
    }
}

/// Drain everything queued for the engine and put back only what a project
/// install must not discard.
///
/// Installing a project is the point where a queue stops meaning anything:
/// every message in it was addressed to the *outgoing* document, so anything
/// still waiting is either already contained in the prepared project or
/// deliberately superseded by it. Applied after the install it would write the
/// outgoing song's intent over the incoming one -- a `MidiRouting` table
/// indexed by the old channel order, a `sample_reset` naming a channel that is
/// now somebody else. [`PendingEngineMessage::survives_project_load`] says
/// which two kinds are addressed to the machine instead and have to live.
///
/// This exists as one function because there are two install paths -- Open and
/// New Song -- and for a while only Open had the filter, which is how New Song
/// came to install the starter kit and then take the previous song's queued
/// routing on top of it.
///
/// `tx` must be a sender on the same channel as `rx`: the survivors go back on
/// the queue for the caller's own drain to forward. That is also why the kept
/// messages are **collected before any are sent** -- requeueing inside a live
/// `try_iter()` would re-observe its own sends and spin forever.
pub fn discard_document_messages(
    rx: &std::sync::mpsc::Receiver<PendingEngineMessage>,
    tx: &std::sync::mpsc::Sender<PendingEngineMessage>,
) {
    // Collected whole before anything goes back, not filtered in a streaming
    // pass: `tx` is a sender on the channel `rx` reads, so a send inside a
    // live `try_iter()` is a message the same iterator then yields again.
    let kept: Vec<_> = rx
        .try_iter()
        .filter(PendingEngineMessage::survives_project_load)
        .collect();
    for message in kept {
        let _ = tx.send(message);
    }
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

/// A Buffer's ring about to be rebuilt at a new length: which device, what it
/// was built as, and what it is to become.
///
/// `bars` is the one Buffer setting that is not a descriptor parameter,
/// because changing it allocates and the audio callback may not. So it takes
/// this path instead -- the ring is built on the pump thread and swapped at a
/// block boundary, down the same road a tempo change already travels.
#[derive(Clone, Copy)]
pub struct BufferResize {
    pub target: EffectTarget,
    pub slot: u8,
    pub expected: mooloop_core::BufferParams,
    pub next: mooloop_core::BufferParams,
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

    /// Rebuild one Buffer's ring at the length its document now says.
    pub fn resize_buffer(&self, resize: BufferResize, bpm: f64) -> bool {
        self.0
            .send(PendingEngineMessage::ResizeBuffer(resize, bpm))
            .is_ok()
    }

    /// Install how each channel takes MIDI input. See
    /// [`PendingEngineMessage::MidiRouting`].
    pub fn send_routing(&self, routes: Vec<mooloop_core::MidiInputRoute>) -> bool {
        self.0
            .send(PendingEngineMessage::MidiRouting(routes))
            .is_ok()
    }

    /// Install which channel records audio. See
    /// [`PendingEngineMessage::AudioInputRouting`].
    pub fn send_audio_input_routing(&self, taps: Vec<Option<mooloop_core::AudioTap>>) -> bool {
        self.0
            .send(PendingEngineMessage::AudioInputRouting(taps))
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
/// It cannot ride the command ring: `EngineCommand` is `Copy` and unboxed by
/// design, and this lives in an `ArcSwap` slot the pump exclusively owns.
/// Same route the built-in sample reset already takes.
///
/// The buffer and the map that indexes it travel as one
/// [`ChannelAudioSnapshot`] because they are one fact: after a commit they
/// change at the same instant, and delivering one without the other would
/// leave the voice reading markers that name frames in a buffer it no longer
/// holds. This message always carried both; what changed in
/// `control-plane-seams/02` is that the pump can no longer take them apart on
/// the way in, because the engine has no API that accepts one alone.
pub struct ChannelAudio {
    pub channel: usize,
    pub audio: ChannelAudioSnapshot,
}

#[derive(Clone)]
pub struct ChannelAudioSender(pub std::sync::mpsc::Sender<ChannelAudio>);

pub fn publish_channel_audio_to(tx: &ChannelAudioSender, channel: usize, state: &ChannelState) {
    let _ = tx.0.send(ChannelAudio {
        channel,
        audio: ChannelAudioSnapshot {
            sample: state.published_sample().cloned(),
            slices: (!state.slices.is_empty()).then(|| Arc::new(state.slices.clone())),
        },
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
        compile_latency(
            &graph,
            &channel_latency,
            &channel_bus,
            &bus_latency,
            // Not `send_edges`: a bank that does not sort has no compensable
            // sends, and the reason is in `mooloop_core::mixer` because the
            // engine's own `install_compensation` has to agree with it. This
            // call site spelled `send_edges` and the engine's did not, so on
            // such a bank the session would have handed over a full
            // `SendBank` compiled against a default order's arrival numbers.
            &compensable_send_edges(&self.buses),
        )
    }

    /// Every send in the document, prepared for the engine's bank.
    ///
    /// `plan` is [`Self::latency_plan`]'s answer, whose per-send entries are
    /// in the same order `send_edges` flattens them -- that shared order is
    /// the contract, and reading both from one pass is what keeps it true.
    fn send_specs(&self, plan: &CompiledLatency) -> Vec<SendSpec> {
        let mut specs = Vec::new();
        // The plan has no send entries for such a bank, so building specs
        // against it would be reading arrival numbers that were never
        // compiled. Same policy, same place, as the plan above.
        if !sends_are_compensable(&self.buses) {
            return specs;
        }
        for (index, setup) in self.buses.iter().take(MAX_BUSES).enumerate() {
            for send in &setup.sends {
                let edge = specs.len();
                specs.push(SendSpec {
                    producer: EffectTarget::Bus(index as u8),
                    target: send.target,
                    tap: send.tap,
                    enabled: send.enabled,
                    level: send.level,
                    delay: plan.send(edge),
                });
            }
        }
        specs
    }

    /// Reconcile the engine's track graph and sends with the document.
    ///
    /// Called from the pump beside [`Self::sync_compensation`] and for the
    /// same reasons. It replaces `EngineCommand::InstallBusGraph`, which the
    /// routing edit used to hand back for the caller to send: routing is no
    /// longer one `u8` per track, because a send is a second outgoing edge
    /// that carries a compensation ring, and a ring is a heap object that has
    /// to be built here and reclaimed here.
    ///
    /// The graph and its sends go as **one** command. A send whose target the
    /// render order has not been told about would arrive a block late, and
    /// two commands leave exactly that window open.
    pub fn sync_track_graph(&mut self, handle: &mut impl CommandSink) {
        let graph = compile_bus_graph(&self.buses).unwrap_or_default();
        let specs = self.send_specs(&self.latency_plan());
        let routes: Vec<SendRoute> = specs.iter().map(SendRoute::of).collect();
        if (graph, &routes) == (self.track_graph_sent.0, &self.track_graph_sent.1) {
            return;
        }
        let delivered = handle.send_structural(StructuralCommand::SetTrackGraph {
            graph,
            sends: Box::new(SendBank::new(&specs, handle.sample_rate())),
        });
        if !delivered {
            self.report_refused_command("track graph and sends");
            return;
        }
        self.track_graph_sent = (graph, routes);
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
    pub fn sync_compensation(&mut self, handle: &mut impl CommandSink) {
        let wanted = CompensationSent::of(&self.latency_plan());
        if wanted == self.compensation_sent {
            return;
        }
        // Per entry, not one assignment at the bottom. Each target is its own
        // command and its own refusal, so the mirror advances exactly as far
        // as the ring accepted and the next tick's diff resends the rest.
        let mut refused = false;
        let channels = self.channels.len().min(MAX_CHANNELS);
        for channel in 0..channels {
            let frames = wanted.channel(channel);
            if frames == self.compensation_sent.channel(channel) {
                continue;
            }
            if handle.send_structural(StructuralCommand::SetCompensation {
                target: EffectTarget::Channel(channel as u8),
                delay: IntegerDelay::new(frames).map(Box::new),
            }) {
                self.compensation_sent.channels[channel] = frames;
            } else {
                refused = true;
            }
        }
        for bus in 0..MAX_BUSES {
            let frames = wanted.bus(bus);
            if frames == self.compensation_sent.bus(bus) {
                continue;
            }
            if handle.send_structural(StructuralCommand::SetCompensation {
                target: EffectTarget::Bus(bus as u8),
                delay: IntegerDelay::new(frames).map(Box::new),
            }) {
                self.compensation_sent.buses[bus] = frames;
            } else {
                refused = true;
            }
        }
        if refused {
            self.report_refused_command("delay compensation");
        }
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
    /// is out" rule `docs/plans/archive/console/` is held to. Deliberately
    /// does not
    /// mark the document dirty -- this is derived state, not something the
    /// user did.
    pub fn sync_console_sums(&mut self, handle: &mut impl CommandSink) {
        let plan = self.console_plan();
        if plan == self.console_sums_sent {
            return;
        }
        let mut refused = false;
        for (bus, &wanted) in plan.iter().enumerate() {
            if wanted == self.console_sums_sent[bus] {
                continue;
            }
            if handle.send_structural(StructuralCommand::SetConsoleSum {
                bus: bus as u8,
                buffer: wanted.then(|| Box::new(StereoBus::with_capacity(MAX_BLOCK_SIZE))),
            }) {
                self.console_sums_sent[bus] = wanted;
            } else {
                refused = true;
            }
        }
        if refused {
            self.report_refused_command("console accumulators");
        }
    }

    /// Reconcile which tracks the engine is silencing for a solo.
    ///
    /// Derived and diffed once a tick, beside [`Self::sync_console_sums`]
    /// and for the same reasons. A bank with nothing soloed derives all
    /// false, matches what was sent, and returns without a command -- so
    /// solo costs one array comparison a tick until somebody presses a
    /// button. Deliberately does not mark the document dirty: what a solo
    /// silences is derived state, and `MixerBus::solo` is the thing the user
    /// did.
    pub fn sync_solo(&mut self, handle: &mut impl CommandSink) {
        let plan = mooloop_core::mixer::solo_silenced(&self.buses);
        if plan == self.solo_silenced_sent {
            return;
        }
        let mut refused = false;
        for (bus, &silenced) in plan.iter().enumerate() {
            if silenced == self.solo_silenced_sent[bus] {
                continue;
            }
            if handle.send(EngineCommand::SetTrackSoloSilenced {
                bus: bus as u8,
                silenced,
            }) {
                self.solo_silenced_sent[bus] = silenced;
            } else {
                refused = true;
            }
        }
        if refused {
            self.report_refused_command("solo silencing");
        }
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
    pub fn sync_audio_graph(&mut self, handle: &mut impl CommandSink) {
        let plan = self.audio_graph_plan();
        if plan == self.audio_graph_sent {
            return;
        }
        if !handle.send_structural(StructuralCommand::SetAudioGraph {
            bank: Box::new(AudioTapBank::new(plan)),
        }) {
            self.report_refused_command("audio edges");
            return;
        }
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
                // A one-shot edit has no mirror to retry from, so a refusal
                // here is divergence the next tick cannot repair -- which is
                // precisely why it gets said out loud.
                if !handle.send(command) {
                    self.report_refused_command("a parameter change");
                }
                edits && self.became_dirty()
            }
            PendingEngineMessage::PreviewGain(gain) => {
                handle.set_preview_gain(gain);
                false
            }
            PendingEngineMessage::MidiRouting(routes) => {
                handle.set_midi_routing(routes);
                // The *document* was already dirtied by the edit that changed
                // a channel's input; installing the resolved table is not a
                // second edit, and a port appearing must not dirty anything.
                false
            }
            PendingEngineMessage::AudioInputRouting(taps) => {
                handle.set_audio_input_routing(taps);
                // Not an edit, for the reason the MIDI routing is not.
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
            PendingEngineMessage::ResizeBuffer(resize, bpm) => {
                if !handle.replace_buffer(
                    resize.target,
                    resize.slot,
                    resize.expected,
                    resize.next,
                    bpm,
                ) {
                    self.report_refused_command("a buffer resize");
                }
                // The document recorded the new length when the control was
                // moved; this is the engine catching up, not a second edit.
                false
            }
            PendingEngineMessage::AddChannel { channel, source } => {
                if !handle.add_channel(channel, source) {
                    self.report_refused_command("adding a channel");
                }
                self.became_dirty()
            }
            PendingEngineMessage::Structural(command) => {
                if !handle.send_structural(command) {
                    self.report_refused_command("a structural edit");
                }
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

    /// Says, once for the life of this session, that the command ring refused
    /// something.
    ///
    /// Once and not per occurrence, because the condition that produces it --
    /// a burst of edits against a full ring -- produces it many times in a
    /// row, and a line per refusal would bury the first one. Not reset by a
    /// project load either: `replace_project` runs on every undo, and
    /// re-arming there would make the quiet version of this the noisy one.
    ///
    /// The sentence has to cover both kinds of caller, which is why it does
    /// not promise a retry. A **reconciler's** refusal is recoverable by
    /// construction: its mirror did not advance, so the next tick's diff
    /// finds the same difference and sends it again. A **one-shot** edit
    /// arriving through `apply_engine_message` has no mirror behind it, so
    /// its refusal is divergence that nothing will repair.
    fn report_refused_command(&mut self, what: &str) {
        if self.engine_queue_refused {
            return;
        }
        self.engine_queue_refused = true;
        log_error!(
            "engine",
            "the command queue refused {what}. Reconciled state -- routing, \
             compensation, console sums, solo, audio edges -- is re-derived \
             and resent on the next pump tick; a one-shot edit is not, and \
             has diverged from the visible project."
        );
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
        // The constant, not a derivation from PPQ. `TICKS_PER_STEP` is a
        // hard 24 and would not follow a PPQ change, so this line would have
        // drifted away from the scheduler while the readout looked right --
        // and being a derivation rather than a literal, no text search would
        // have found it. `BbtPosition` below takes the bar whole, for the
        // same reason.
        let ticks_per_step = u64::from(mooloop_core::TICKS_PER_STEP);
        let (position_ticks, playlist_ticks) = if self.song_mode {
            let position = tick % u64::from(self.song_length_ticks());
            (position, Some(position as i32))
        } else {
            (tick % (length * ticks_per_step), None)
        };
        // Bar, beat and tick come from `BbtPosition` rather than from three
        // lines of modulus here. This crate and `engine::transport` used to
        // derive the same window independently and agreed only because both
        // had hardcoded four.
        let bbt = BbtPosition::from_ticks(Ticks(position_ticks), mooloop_core::Ppq::DEFAULT);
        TransportPosition {
            step: ((tick / ticks_per_step) % length) as i32,
            playlist_ticks,
            bar: bbt.bar as i32,
            beat: bbt.beat as i32,
            tick: bbt.tick as i32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{PatternPlacement, BEATS_PER_BAR, TICKS_PER_BAR, TICKS_PER_STEP};

    /// A load keeps what is addressed to the machine and drops what is
    /// addressed to the outgoing project -- including a routing table, which
    /// is in that project's channel order.
    #[test]
    fn a_load_keeps_machine_settings_and_drops_project_addressed_messages() {
        assert!(PendingEngineMessage::Audio(AudioAction::SelectBufferSize(256))
            .survives_project_load());
        assert!(PendingEngineMessage::PreviewGain(0.5).survives_project_load());

        assert!(!PendingEngineMessage::MidiRouting(Vec::new()).survives_project_load());
        assert!(!PendingEngineMessage::Command(EngineCommand::SetRecordArmed(true))
            .survives_project_load());
        assert!(!PendingEngineMessage::Telemetry(TelemetryAction::SetEffectSpectrumEnabled {
            target: EffectTarget::Channel(0),
            slot: 0,
            enabled: true,
        })
        .survives_project_load());
    }

    /// The drain both install paths run: the two machine-addressed kinds come
    /// back, in the order they were queued, and everything addressed to the
    /// outgoing document is gone.
    ///
    /// The ordering assertion is not decoration. The survivors go back on the
    /// same channel the caller is about to drain, so a helper that requeued as
    /// it iterated would either spin or reorder a buffer-size pick behind a
    /// preview-gain knob turn.
    #[test]
    fn discarding_document_messages_keeps_the_machine_ones_in_order() {
        let (tx, rx) = std::sync::mpsc::channel::<PendingEngineMessage>();

        // Interleaved deliberately: the two survivors are neither first nor
        // adjacent, so a filter that kept a prefix would pass.
        tx.send(PendingEngineMessage::MidiRouting(Vec::new())).unwrap();
        tx.send(PendingEngineMessage::Audio(AudioAction::SelectBufferSize(256)))
            .unwrap();
        tx.send(PendingEngineMessage::Command(EngineCommand::SetRecordArmed(true)))
            .unwrap();
        tx.send(PendingEngineMessage::PreviewGain(0.5)).unwrap();
        tx.send(PendingEngineMessage::AudioInputRouting(Vec::new()))
            .unwrap();

        discard_document_messages(&rx, &tx);

        let survivors: Vec<_> = rx.try_iter().collect();
        assert_eq!(
            survivors.len(),
            2,
            "expected the two machine-addressed messages to survive, got {}",
            survivors.len()
        );
        assert!(
            matches!(
                survivors[0],
                PendingEngineMessage::Audio(AudioAction::SelectBufferSize(256))
            ),
            "the buffer-size pick was dropped or reordered"
        );
        assert!(
            matches!(survivors[1], PendingEngineMessage::PreviewGain(gain) if gain == 0.5),
            "the preview gain was dropped or reordered"
        );
    }

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

        let ticks_per_beat = u64::from(TICKS_PER_BAR) / u64::from(BEATS_PER_BAR);
        let second_beat = session.transport_position(ticks_per_beat + 3);
        assert_eq!(
            (second_beat.bar, second_beat.beat, second_beat.tick),
            (1, 2, 3)
        );
    }

    /// The session's compensation plan drops its sends on a bank that does
    /// not sort, the way the engine's always has.
    ///
    /// `RenderState::install_compensation` guards its send half on whether
    /// the bank sorts, with a reason: a send compiled against an order that is
    /// not the one being walked arrives a block late. This derivation called
    /// `send_edges` unconditionally and `send_specs` iterated every bus
    /// unconditionally, so on such a bank the session would have handed the
    /// engine a full `SendBank` compiled against a default order's arrival
    /// numbers.
    ///
    /// It was unreachable -- the bank is sanitized on load, and `set_bus_output`
    /// and `add_send` both refuse cycles -- which is exactly what let it sit
    /// there: two copies of one policy, one corrected and one not, and nothing
    /// able to notice. The cycle here is therefore written straight into
    /// `buses`, past the two edit paths that would refuse it, because that is
    /// the only way a hand-edited or foreign-build file reaches this code.
    #[test]
    fn the_session_drops_its_sends_from_the_plan_when_the_bank_does_not_sort() {
        use mooloop_core::{AuxSend, BusSetup};

        let mut session = Session::default();
        while session.buses.len() < 4 {
            session.buses.push(BusSetup::new(session.buses.len()));
        }
        session.buses[1].sends.push(AuxSend::new(2));

        let plan = session.latency_plan();
        assert_eq!(
            session.send_specs(&plan).len(),
            1,
            "a sorting bank should carry its send into the engine's spec list"
        );

        session.buses[2].bus.output = 3;
        session.buses[3].bus.output = 2;
        let looped = session.latency_plan();
        assert!(
            session.send_specs(&looped).is_empty(),
            "a bank that does not sort handed the engine sends compiled against \
             an order nothing is walking"
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
        session.ensure_tracks(2);
        session.add_channel(mooloop_core::DeviceKind::Sampler);
        session.channels[0].bus = 1;
        session.buses[1].effects.push(transparent_drive().with_id(mooloop_core::DeviceId(0)));

        let plan = session.latency_plan();
        assert_eq!(plan.channel(0), 0, "nothing else feeds bus 1");
        assert_eq!(plan.channel(1), latency, "the direct channel waits for the bus");
        assert_eq!(plan.total(), latency);
    }
}
