//! JACK-independent render state shared by realtime playback and file export.

use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwapOption;
use mooloop_core::{
    audio_tap_index, compile_bus_graph, AutomationLane, ChannelSource,
    CompiledAudioGraph, CompiledBusGraph, DeviceKind, OutletDescriptor, PublishesOutlets,
    Ds01Params, DrumSynthParams, EffectTarget, EngineCommand, GeneratorParams,
    LoopRange, ModDestinationDescriptor, MusicalEdge, PlaybackMode,
    ModRack, MonoSynthParams, MlM1Params, MlP8Params, ParamAddr, ParamOwner, PolySynthParams,
    Project,
    SamplerParams, SendTap,
    chain_latency, clamp_bus, compensable_send_edges, compile_latency,
    sends_are_compensable, DEFAULT_STEPS, MAX_CONTAINER_DEPTH, MAX_SAMPLER_VOICES, MASTER_BUS, MAX_BUSES, MAX_CHANNELS, MAX_EFFECTS_PER_CHANNEL, MAX_LINEAR_GAIN,
    MAX_AUTOMATION_LANES_PER_CHANNEL, MAX_MODULATORS_PER_CHANNEL, STRIP_DESCRIPTORS,
    STRIP_PARAM_VOLUME,
};
use mooloop_core::mixer::{StripPin, STRIP_PIN};
use mooloop_core::strip::StripParams;
use mooloop_core::modulation::{
    CONTROL_SOURCE_SLOTS, MAX_GENERATOR_OUTLETS, PERFORMANCE_AFTERTOUCH, PERFORMANCE_MOD_WHEEL,
    PERFORMANCE_SOURCES,
};
use mooloop_dsp::console;
use crate::voices::SequencedVoices;
#[cfg(test)]
use mooloop_dsp::build_effect;
use mooloop_dsp::{
    balance_gains, buffer_allocation_key, build_effect_at_tempo, pan_gains, AudioNode, Ds01,
    Discontinuity, DrumSynth,
    AudioTaps, AuxIn, IntegerDelay, Event, EventList, ModulatorRack, MonoSynth, MlM1, MlP8,
    NoteGateEvents, OutputGuard, PolySynth,
    ChannelAudioSnapshot,
    ProcessContext, SampleData, Sampler, SourceNode, SpectrumAnalyzer, StereoBus, StretchPool,
    TimedEvent,
    CONTROL_RATE_FRAMES, ControlCurve, MAX_BLOCK_SIZE, MAX_CONTROL_TICKS_PER_BLOCK, SILENCE_PEAK,
};
use mooloop_dsp::interpolate::{Region, SincTable};
use mooloop_dsp::smooth::Smoothed;
use mooloop_dsp::strip::Strip;

use crate::meters::{BusMeters, DeviceMeters, DeviceTelemetry, ModulatorMeters, PlayheadMeters};
use crate::sequencer::Sequencer;
use crate::transport::{BlockSpan, Transport};
use crate::{PreviewCommand, StructuralCommand, StructuralReclaim};

/// How many finished preview samples either side of the reclaim path will
/// hold while the ring is full.
///
/// One answer, shared by the renderer and the executor, because they are two
/// halves of one queue and two limits would be two things to reason about. A
/// preview is one auditioned sample and the ring drains every GUI frame, so
/// reaching this takes a backlog no interaction produces -- but the capacity
/// is reserved up front regardless, because the alternative is a `Vec`
/// growing on the realtime thread.
pub(crate) const MAX_RETIRED_PREVIEWS: usize = 64;

/// Finished preview samples waiting for the reclaim ring, with a ceiling.
///
/// A plain `Vec` was used on both sides of this. The renderer's started at
/// zero capacity, so its **first** push allocated -- audition a sample from
/// the browser, let it play to the end, and the callback allocated. The
/// executor's reserved its capacity but pushed past it unconditionally, so a
/// reclaim ring that stayed full while previews kept retiring grew it and
/// reallocated.
///
/// **A refused push hands the sample back rather than dropping it.** That is
/// the whole of the policy and it is the only option with no failure mode:
/// the caller holds the voice one more block and tries again, and the next
/// block almost certainly has room. Dropping the `Arc` here would be a free
/// on the audio thread, which is the thing the entire reclaim path exists to
/// avoid.
pub(crate) struct RetiredPreviews {
    samples: Vec<Arc<SampleData>>,
}

impl RetiredPreviews {
    pub(crate) fn new() -> Self {
        Self {
            samples: Vec::with_capacity(MAX_RETIRED_PREVIEWS),
        }
    }

    /// Take `sample`, or hand it straight back when full. Never allocates.
    #[must_use]
    pub(crate) fn push(&mut self, sample: Arc<SampleData>) -> Option<Arc<SampleData>> {
        if self.samples.len() == self.samples.capacity() {
            return Some(sample);
        }
        self.samples.push(sample);
        None
    }

    pub(crate) fn pop(&mut self) -> Option<Arc<SampleData>> {
        self.samples.pop()
    }

    pub(crate) fn len(&self) -> usize {
        self.samples.len()
    }

    pub(crate) fn is_full(&self) -> bool {
        self.samples.len() == self.samples.capacity()
    }
}

/// One channel's published audio, as the control thread writes it and the
/// channel's voices read it.
pub type ChannelAudioSlot = Arc<ArcSwapOption<ChannelAudioSnapshot>>;

/// Every addressable channel's audio slot.
///
/// **One bank per render generation, and that is the point.** A generation's
/// strips clone their slot out of the bank they were built from, so a bank
/// built alongside a prepared project is read only by that project's voices.
/// A single shared bank -- which is what this replaced -- meant the outgoing
/// project's graph read the incoming project's samples for the block or two
/// between queueing an install and the audio thread consuming it.
///
/// The slots stay atomic rather than becoming plain values, because they are
/// still written after the generation is live: loading a sample into an open
/// song publishes into the live generation's bank. What changed is *which*
/// bank that is, not how a channel is published.
pub type ChannelAudioBank = Arc<Vec<ChannelAudioSlot>>;

/// A bank with nothing published in any channel.
pub fn empty_channel_audio_bank() -> ChannelAudioBank {
    Arc::new(
        (0..MAX_CHANNELS)
            .map(|_| Arc::new(ArcSwapOption::empty()))
            .collect(),
    )
}

/// A **fresh** bank holding `audio`, one entry per addressable channel.
///
/// Fresh slots, never a clone of an existing bank, and that is the whole
/// contract: the generation built from this reads what the caller prepared
/// and nothing else can reach it until the caller publishes the bank as the
/// current one. `audio` is by value for the same reason -- there is no way to
/// hand this the live generation's slots, so the install path cannot
/// accidentally share them.
pub fn channel_audio_bank(audio: Vec<ChannelAudioSnapshot>) -> ChannelAudioBank {
    let mut audio = audio.into_iter();
    Arc::new(
        (0..MAX_CHANNELS)
            .map(|_| {
                let held = audio.next().unwrap_or_default();
                Arc::new(ArcSwapOption::from(
                    (!held.is_empty()).then(|| Arc::new(held)),
                ))
            })
            .collect(),
    )
}

/// One generation's audio edges and the buffers they carry.
///
/// Prepared whole on the control thread and installed as one value, so the
/// executor can never observe a schedule against another generation's
/// buffers -- the same reason `CompiledBusGraph` and its render order travel
/// together.
///
/// **A tap exists only when somebody is subscribed to it.** ML-P8 declares
/// seven stereo outlets; materializing them all would be 448 KB a channel and
/// 7 MB across a full bank for a feature that is off by default. `buffers` is
/// as long as the number of distinct (producer, outlet) pairs somebody reads,
/// which on every project that has never authored an edge is zero.
pub struct AudioTapBank {
    graph: CompiledAudioGraph,
    /// One buffer per tap index the graph handed out, deduplicated by the
    /// (producer, outlet) pair: two channels reading the same `Osc 3` share
    /// one buffer rather than getting two copies of the same samples.
    buffers: Vec<StereoBus>,
}

impl AudioTapBank {
    /// Allocate the storage `graph` asks for. Control thread only: this is
    /// where the megabytes would be, which is why they are conditional.
    pub fn new(graph: CompiledAudioGraph) -> Self {
        Self {
            buffers: (0..graph.tap_count())
                .map(|_| StereoBus::with_capacity(MAX_BLOCK_SIZE))
                .collect(),
            graph,
        }
    }

    /// Whether `producer` owes a tap this block.
    ///
    /// The question the mute check asks: a muted producer still fills its
    /// tap, because mute is an output-stage decision about what reaches the
    /// bus and a pre-level tap is exactly the signal a muted source still
    /// has. A muted channel nobody reads still skips, which is what keeps
    /// mute a way of not spending the work.
    fn produces(&self, producer: usize) -> bool {
        self.graph
            .taps()
            .any(|(_, tap)| usize::from(tap.channel) == producer)
    }

    /// The buffer `consumer`'s resolved edge reads, if it has one.
    fn source(&self, consumer: usize) -> Option<&StereoBus> {
        self.buffers.get(self.graph.edge(consumer).tap()? as usize)
    }

    /// The port group `producer` owes this block, indexed by its own tap
    /// numbers.
    ///
    /// Built by walking the buffers rather than the device's outlet table, so
    /// each buffer is borrowed exactly once and the whole group is one set of
    /// disjoint mutable references with no unsafe code.
    fn ports(&mut self, producer: usize, outlets: &'static [OutletDescriptor]) -> AudioTaps<'_> {
        let mut ports = AudioTaps::none();
        let Self { graph, buffers } = self;
        for (index, buffer) in buffers.iter_mut().enumerate() {
            let Some(subscription) = graph.tap(index) else {
                continue;
            };
            if usize::from(subscription.channel) != producer {
                continue;
            }
            let Some(tap) = audio_tap_index(outlets, subscription.outlet) else {
                continue;
            };
            ports.set(tap, buffer);
        }
        ports
    }

    /// Empty every tap for the block about to render.
    ///
    /// Once for the whole bank rather than per producer, so a producer that
    /// stops playing -- or stops existing -- publishes silence rather than
    /// the block before.
    fn clear(&mut self, frames: usize) {
        for buffer in &mut self.buffers {
            buffer.clear(frames);
        }
    }
}

/// Lag on every strip-level gain, in seconds: a send's level, a fader, a pan
/// or balance, the fade a mute or a solo makes, and a track's polarity.
///
/// The same figure every other gain that scales the signal directly uses
/// (`mooloop_dsp::synth_voice::PARAM_SMOOTH_S`), as a time constant for
/// [`Smoothed`]: a full-scale change moves 1/240 of the way in its first
/// sample at 48 kHz and is inaudibly close to its target within 25 ms.
///
/// Sends smoothed first and alone, and until MOO-107 the comment here called
/// the fader's zipper "a separate known gap": the output stage stamped its
/// value per block, or per control tick when something drove it, and a mute
/// was a branch. One constant for all of them, because a mute that faded
/// faster than the send leaving the same strip would be heard as the send
/// arriving late.
const STRIP_GAIN_SMOOTH_S: f32 = 0.005;

/// Where a strip's send reads from, and what it owes when it arrives.
///
/// One of these per authored send. It holds the two things a second outgoing
/// edge needs and a single one never did: its own gain, and its own delay --
/// the strip's `compensation` is what its *output* waits, and a send reaching
/// a different summing point is generally owed something else.
struct CompiledSend {
    /// Track this send sums into.
    target: u8,
    tap: SendTap,
    enabled: bool,
    /// The level the send was authored at. `level` is aimed at it while its
    /// producer is heard and at silence while its producer is muted or
    /// solo-silenced, so a mute fades the send with the strip's own output.
    authored: f32,
    /// Smoothed, because unlike a fader a send level has no automation path
    /// stepping it per control tick -- an unsmoothed one would zipper on
    /// every drag.
    level: Smoothed,
    /// What this send waits before summing into `target`, from
    /// `mooloop_core::compile_latency`'s per-send answer.
    compensation: Option<Box<IntegerDelay>>,
}

/// Scratch buffers a block's sends work in.
///
/// Three, and only when a project has a send at all. A strip's own buffer
/// cannot be used: it is still owed to the strip's output, and each send
/// applies its own level and its own delay, so each one needs a copy. The two
/// tap buffers are captured while the strip is mid-block; `work` is where one
/// send at a time is prepared.
struct SendScratch {
    pre: StereoBus,
    post: StereoBus,
    work: StereoBus,
}

impl SendScratch {
    fn new() -> Self {
        Self {
            pre: StereoBus::with_capacity(MAX_BLOCK_SIZE),
            post: StereoBus::with_capacity(MAX_BLOCK_SIZE),
            work: StereoBus::with_capacity(MAX_BLOCK_SIZE),
        }
    }
}

/// One generation's sends, prepared whole on the control thread.
///
/// The same shape and the same reason as [`AudioTapBank`]: rings and buffers
/// are allocated where allocation is allowed, and the whole generation
/// crosses as one value so the executor can never hold one generation's
/// delays against another's routing.
///
/// **A project with no sends allocates nothing**, which is the "free while it
/// is out" rule `docs/plans/archive/console/` is held to, and is exactly
/// the derived
/// `Default`: two empty `Vec`s and no scratch. `a_project_with_no_sends_
/// allocates_nothing` is the test that says so.
#[derive(Default)]
pub struct SendBank {
    /// Every send, grouped by producer and in bank order, which is the order
    /// `mooloop_core::send_edges` flattens them in.
    sends: Vec<CompiledSend>,
    /// Where each producer's run starts in `sends`, plus a final end marker.
    /// Empty when `sends` is, so an unused feature is not a kilobyte of
    /// zeroes either.
    starts: Vec<u32>,
    scratch: Option<Box<SendScratch>>,
}

/// One authored send, flattened for the crossing. POD: the rings and the
/// smoothing are built from it on the control thread.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SendSpec {
    pub producer: EffectTarget,
    pub target: u8,
    pub tap: SendTap,
    pub enabled: bool,
    pub level: f32,
    /// Frames this send waits before it sums, from `compile_latency`.
    pub delay: u32,
}

/// Index of `producer` in the flat producer space the bank is grouped by:
/// every channel, then every track.
fn producer_slot(producer: EffectTarget) -> usize {
    match producer {
        EffectTarget::Channel(channel) => channel as usize,
        EffectTarget::Bus(bus) => MAX_CHANNELS + bus as usize,
    }
}

const PRODUCER_SLOTS: usize = MAX_CHANNELS + MAX_BUSES;

/// [`producer_slot`] read backwards.
fn producer_at(slot: usize) -> EffectTarget {
    if slot < MAX_CHANNELS {
        EffectTarget::Channel(slot as u8)
    } else {
        EffectTarget::Bus((slot - MAX_CHANNELS) as u8)
    }
}

/// How many frames a compensation ring holds, with no ring holding none.
fn ring_frames(ring: &Option<Box<IntegerDelay>>) -> usize {
    ring.as_ref().map_or(0, |ring| ring.frames())
}

/// Put back whichever of two compensation rings is live, when both are the
/// same length.
///
/// **A ring the same length as the live one is the live one's job, and the
/// live one is already doing it with the right audio in it.** A fresh ring
/// starts empty, so installing it plays silence for the length of the delay
/// -- the gap a structural edit still left on any path with a latency after
/// `incremental-structure/02`. `incoming` is the ring about to be installed
/// and `live` the one it would displace; after this, `incoming` holds
/// whichever should be heard and `live` whichever should be reclaimed.
fn keep_live_ring(incoming: &mut Option<Box<IntegerDelay>>, live: &mut Option<Box<IntegerDelay>>) {
    if ring_frames(incoming) == ring_frames(live) {
        std::mem::swap(incoming, live);
    }
}

impl SendBank {
    /// Prepare `specs` for the audio thread. Control thread only: this is
    /// where the rings and the scratch are allocated.
    ///
    /// `specs` need not arrive grouped; they are sorted by producer here, and
    /// a producer's own sends keep the order they were authored in, which is
    /// what makes an index into that run a stable address for a level change.
    pub fn new(specs: &[SendSpec], sample_rate: u32) -> Self {
        if specs.is_empty() {
            return Self::default();
        }
        // A producer outside the address space is dropped rather than
        // indexed with. Nothing in the model can mint one -- a producer is a
        // bank position -- but this is a public constructor, and the
        // alternative to a filter here is a panic while preparing a plan.
        let mut ordered: Vec<&SendSpec> = specs
            .iter()
            .filter(|spec| producer_slot(spec.producer) < PRODUCER_SLOTS)
            .collect();
        if ordered.is_empty() {
            return Self::default();
        }
        ordered.sort_by_key(|spec| producer_slot(spec.producer));

        let mut starts = vec![0u32; PRODUCER_SLOTS + 1];
        for spec in &ordered {
            starts[producer_slot(spec.producer) + 1] += 1;
        }
        for slot in 1..starts.len() {
            starts[slot] += starts[slot - 1];
        }

        Self {
            sends: ordered
                .iter()
                .map(|spec| CompiledSend {
                    target: spec.target,
                    tap: spec.tap,
                    enabled: spec.enabled,
                    authored: spec.level,
                    // From silence: a send that did not exist a moment ago
                    // fades in rather than arriving at its level in one
                    // sample. A document arriving settles every send to its
                    // level (`RenderState::load_project`), and a bank
                    // replacing a live one takes each surviving edge's level
                    // over (`adopt_rings_from`), so only a send that is new
                    // in the middle of a song is heard to ramp.
                    level: Smoothed::new(0.0, STRIP_GAIN_SMOOTH_S, sample_rate),
                    compensation: IntegerDelay::new(spec.delay).map(Box::new),
                })
                .collect(),
            starts,
            scratch: Some(Box::new(SendScratch::new())),
        }
    }

    fn is_empty(&self) -> bool {
        self.sends.is_empty()
    }

    /// Take over `old`'s compensation rings for every send that is the same
    /// edge in both banks, so a bank arriving does not empty the delays of
    /// the sends it did not change.
    ///
    /// `seat` says where a producer or target of `old` sits in this bank, or
    /// `None` when it is gone: the identity for a reconciler's resend, the
    /// install's seat map for a structural edit. A producer's sends are
    /// matched in authored order by target and tap -- the *n*th send from a
    /// producer to a target on a tap is the *n*th in both -- and only a ring
    /// of the same length is taken; see [`keep_live_ring`].
    ///
    /// Audio thread. Swaps boxes and counts; allocates nothing.
    fn adopt_rings_from(
        &mut self,
        old: &mut SendBank,
        seat: impl Fn(EffectTarget) -> Option<EffectTarget>,
    ) {
        if self.is_empty() || old.is_empty() {
            return;
        }
        for slot in 0..PRODUCER_SLOTS {
            let producer = producer_at(slot);
            let was = old.range(producer);
            if was.is_empty() {
                continue;
            }
            let Some(now) = seat(producer).map(|producer| self.range(producer)) else {
                continue;
            };
            for index in was.clone() {
                let (target, tap) = (old.sends[index].target, old.sends[index].tap);
                let Some(EffectTarget::Bus(new_target)) = seat(EffectTarget::Bus(target)) else {
                    continue;
                };
                let nth = old.sends[was.start..index]
                    .iter()
                    .filter(|send| send.target == target && send.tap == tap)
                    .count();
                let Some(matched) = now
                    .clone()
                    .filter(|&at| self.sends[at].target == new_target && self.sends[at].tap == tap)
                    .nth(nth)
                else {
                    continue;
                };
                // The level where the live send has it, mid-ramp or mid-fade
                // included: `emit` aims it at this bank's authored level on
                // the next block, so a rebuild never steps a send that
                // survived it.
                self.sends[matched].level = old.sends[index].level;
                keep_live_ring(
                    &mut self.sends[matched].compensation,
                    &mut old.sends[index].compensation,
                );
            }
        }
    }

    fn range(&self, producer: EffectTarget) -> std::ops::Range<usize> {
        if self.starts.is_empty() {
            return 0..0;
        }
        let slot = producer_slot(producer);
        match (self.starts.get(slot), self.starts.get(slot + 1)) {
            (Some(&start), Some(&end)) => start as usize..end as usize,
            _ => 0..0,
        }
    }

    /// Whether `producer` has a send reading `tap` this block. What decides
    /// whether the capture below is worth the copy.
    fn taps(&self, producer: EffectTarget, tap: SendTap) -> bool {
        self.sends[self.range(producer)]
            .iter()
            .any(|send| send.tap == tap && send.enabled)
    }

    /// Keep a copy of `bus` as `producer`'s `tap` signal.
    ///
    /// Called from inside the block loop, where the strip that owns `bus` is
    /// still borrowed and the tracks its sends reach are not reachable. The
    /// copy is what lets the emission happen afterwards, and it is skipped
    /// entirely when nothing reads that tap.
    fn capture(&mut self, producer: EffectTarget, tap: SendTap, bus: &StereoBus, frames: usize) {
        if self.is_empty() || !self.taps(producer, tap) {
            return;
        }
        let Some(scratch) = self.scratch.as_mut() else {
            return;
        };
        match tap {
            SendTap::PreFader => scratch.pre.copy_from(bus, frames),
            SendTap::PostFader => scratch.post.copy_from(bus, frames),
        }
    }

    /// Empty `producer`'s send rings.
    ///
    /// For every path that skips [`Self::emit`] -- a muted or solo-silenced
    /// track, a sleeping one, a muted channel. The rings are advanced *only*
    /// inside `emit`, so without this they freeze rather than drain: the
    /// frames captured just before a mute sat in the ring and were the first
    /// thing the return heard on unmute, a fragment of the previous phrase
    /// arriving where nothing was played.
    ///
    /// It is the statement `emit` already makes for a *disabled* send, and
    /// the one the track's own ring already makes at `render.rs`'s mute
    /// check -- "a muted bus still advances its ring rather than holding
    /// stale audio to emit when it is unmuted".
    ///
    /// **Not covered by a test, and the reason is worth knowing before
    /// writing one.** A muted track also goes to sleep, so the frozen ring is
    /// unobservable at the destination until that track wakes -- which needs
    /// a second note after the unmute, and then a differential render against
    /// an unmuted control to separate the stale frames from the new ones. An
    /// attempt that muted, idled and unmuted measures zero either way. See
    /// `docs/LOOSE_ENDS.md`.
    fn reset(&mut self, producer: EffectTarget) {
        if self.is_empty() {
            return;
        }
        let range = self.range(producer);
        for send in &mut self.sends[range] {
            if let Some(delay) = send.compensation.as_mut() {
                delay.reset();
            }
        }
    }

    /// Whether every one of `producer`'s sends has faded all the way out, so
    /// that skipping [`Self::emit`] for it drops nothing anybody could hear.
    ///
    /// The send half of a mute's fade (MOO-107). A muted producer keeps
    /// emitting, aimed at silence, until this and its own output stage both
    /// say so -- and only then takes the path that skips the sends and empties
    /// their rings. Disabled sends count too, because `emit` holds them at
    /// silence: one enabled while its producer is muted must not arrive at
    /// its authored level and fade from there.
    fn is_silent(&self, producer: EffectTarget) -> bool {
        self.is_empty()
            || self.sends[self.range(producer)]
                .iter()
                .all(|send| send.level.is_settled() && send.level.value() == 0.0)
    }

    /// Jump `producer`'s send levels to where [`Self::emit`] would aim them:
    /// silence while `silenced` or switched off, the authored level
    /// otherwise. For a document arriving, where there is nothing sounding
    /// to be continuous with -- and where a muted track's send starting at
    /// its level would leak the first milliseconds of a muted part into its
    /// return.
    fn settle(&mut self, producer: EffectTarget, silenced: bool) {
        if self.is_empty() {
            return;
        }
        let range = self.range(producer);
        for send in &mut self.sends[range] {
            send.level.reset_to(if silenced || !send.enabled {
                0.0
            } else {
                send.authored
            });
        }
    }

    /// Sum `producer`'s captured sends into the tracks they feed.
    ///
    /// Called once the strip's own borrow has ended. A disabled send resets
    /// its ring rather than advancing it, so re-enabling one does not emit the
    /// audio it was holding when it was switched off.
    ///
    /// `silenced` is the producer's mute or solo verdict: while it holds,
    /// every send is aimed at silence rather than at its authored level, and
    /// fades there with the producer's own output. A disabled send is held at
    /// silence outright -- nothing of it is heard, so there is nothing to
    /// fade -- which also means switching one on ramps it in from nothing
    /// rather than stepping.
    fn emit(
        &mut self,
        producer: EffectTarget,
        buses: &mut [BusStrip],
        frames: usize,
        silenced: bool,
    ) {
        if self.is_empty() {
            return;
        }
        let range = self.range(producer);
        let Self { sends, scratch, .. } = self;
        let Some(scratch) = scratch.as_mut() else {
            return;
        };
        for send in &mut sends[range] {
            if !send.enabled {
                if let Some(delay) = send.compensation.as_mut() {
                    delay.reset();
                }
                send.level.reset_to(0.0);
                continue;
            }
            send.level
                .set_target(if silenced { 0.0 } else { send.authored });
            let Some(destination) = buses.get_mut(send.target as usize) else {
                continue;
            };
            let SendScratch { pre, post, work } = &mut **scratch;
            work.copy_from(
                match send.tap {
                    SendTap::PreFader => &*pre,
                    SendTap::PostFader => &*post,
                },
                frames,
            );
            apply_smoothed_gain(&mut send.level, work, frames);
            if let Some(delay) = send.compensation.as_mut() {
                delay.process(&mut work.l[..frames], &mut work.r[..frames]);
            }
            // Always linear, for the reason a channel reaching a track is:
            // analog sum is what a strip does to its *output*, and a send is
            // a feed into another strip's input, which encodes on its own
            // switch or not at all.
            destination.bus.add_from(work, frames);
            destination.dirty = true;
        }
    }

    /// Aim a send at a new level, which it reaches over
    /// [`STRIP_GAIN_SMOOTH_S`].
    ///
    /// `index` is the send's position in its own producer's run, which is the
    /// order it was authored in and survives a bank rebuild -- so a fader drag
    /// keeps addressing the same send while the plan around it changes.
    ///
    /// Only the authored level moves here. `emit` aims the smoother, because
    /// only `emit` knows whether the producer is muted: a send dragged on a
    /// muted track must not start fading up.
    fn set_level(&mut self, producer: EffectTarget, index: usize, level: f32) {
        let range = self.range(producer);
        if let Some(send) = self.sends[range].get_mut(index) {
            send.authored = level;
        }
    }

    fn set_enabled(&mut self, producer: EffectTarget, index: usize, enabled: bool) {
        let range = self.range(producer);
        if let Some(send) = self.sends[range].get_mut(index) {
            send.enabled = enabled;
        }
    }

    fn set_tap(&mut self, producer: EffectTarget, index: usize, tap: SendTap) {
        let range = self.range(producer);
        if let Some(send) = self.sends[range].get_mut(index) {
            send.tap = tap;
        }
    }
}

/// Apply a smoothed gain to both sides of a block: a send's level, or a
/// track's polarity.
///
/// A settled gain is one pass over the block, which is what it is on every
/// block but the few after a drag -- so the ordinary case costs exactly what
/// an unsmoothed gain would, and a settled unity costs nothing at all.
///
/// A moving one steps **per sample**, not per control tick: a `Smoothed` has
/// a finer answer to give than the 32-frame control rate, and reusing that
/// coarse subdivision would throw away the resolution that is the whole
/// reason to smooth. [`OutputStage`] does the same per side.
fn apply_smoothed_gain(level: &mut Smoothed, bus: &mut StereoBus, frames: usize) {
    if level.is_settled() {
        let gain = level.value();
        if gain != 1.0 {
            bus.apply_stereo_gain(gain, gain, frames);
        }
        return;
    }
    for frame in 0..frames {
        let gain = level.advance();
        bus.l[frame] *= gain;
        bus.r[frame] *= gain;
    }
}

/// A displaced effect-slot occupant: the node plus the dry-align delay the
/// container allocated alongside it. Both halves are heap objects built on
/// the non-realtime side, so they must also be dropped there — the realtime
/// thread never frees a `Box` itself.
pub(crate) struct ReclaimedEffect {
    pub node: Option<Box<dyn AudioNode + Send>>,
    pub align: Option<Box<IntegerDelay>>,
    pub analyzer: Option<Box<SpectrumAnalyzer>>,
    /// The slot's own state, which is a box like the rest and so must leave
    /// the realtime thread the same way rather than being dropped on it.
    pub state: Option<Box<EffectSlot>>,
    /// Channel storage the graph turned out not to need, handed back for the
    /// same reason as the rest: the audio thread never drops a box.
    pub channel: Option<Box<ChannelStorage>>,
}

impl ReclaimedEffect {
    fn is_empty(&self) -> bool {
        self.node.is_none()
            && self.align.is_none()
            && self.analyzer.is_none()
            && self.state.is_none()
            && self.channel.is_none()
    }
}

/// Occupants displaced from effect slots, handed back so the non-realtime
/// side can drop them.
type Reclaim = Vec<ReclaimedEffect>;

/// A compact, preallocated per-effect mailbox. Knob traffic is coalesced by
/// parameter ID; retaining every intermediate mouse position is neither
/// audible nor necessary, while allocating a full `EventList` for every one
/// of 256 possible slots would make empty chains prohibitively expensive.
const MAX_PENDING_EFFECT_PARAMS: usize = 8;

/// `MAX_BLOCK_SIZE` is the executor's explicit block-size boundary. Capturing
/// one value for each rack slot at every control-rate boundary lets every
/// effect in a channel read the exact same LFO timeline without allocating or
/// advancing the source more than once.
///
/// [`MAX_CONTROL_TICKS_PER_BLOCK`] itself now lives in `mooloop_dsp::node`,
/// next to [`ControlCurve`] -- a node's own curve buffer and the engine's
/// per-block pool that fills it have to agree on this bound exactly, and a
/// value derived independently in two crates from the same two inputs is
/// still the same value written twice in the sense `AGENTS.md`'s
/// duplication section means; importing it is what makes that impossible
/// rather than merely unlikely.
const _: () = assert!(MAX_CONTROL_TICKS_PER_BLOCK == MAX_BLOCK_SIZE / CONTROL_RATE_FRAMES);

/// The song position, in quarter-note beats, at the start of each control
/// tick of a block, or `None` everywhere while the transport is stopped.
type SongBeats = [Option<f64>; MAX_CONTROL_TICKS_PER_BLOCK];

/// Where each control tick of this block starts in the song, read off the
/// transport's spans -- so a loop wrap or a seek inside the block is where
/// the ticks after it say they are (MOO-127).
fn song_beats_for(
    playing: bool,
    spans: &[BlockSpan],
    frames: usize,
    ticks_per_beat: f64,
) -> SongBeats {
    let mut beats = [None; MAX_CONTROL_TICKS_PER_BLOCK];
    if !playing || spans.is_empty() {
        return beats;
    }
    for (tick, slot) in beats
        .iter_mut()
        .enumerate()
        .take(frames.div_ceil(CONTROL_RATE_FRAMES))
    {
        let frame = tick * CONTROL_RATE_FRAMES;
        let span = spans
            .iter()
            .rev()
            .find(|span| span.frame <= frame)
            .unwrap_or(&spans[0]);
        let along = if span.frames == 0 {
            0.0
        } else {
            (frame - span.frame).min(span.frames) as f64 / span.frames as f64
        };
        let position = span.start_tick + (span.end_tick - span.start_tick) * along;
        *slot = Some(position / ticks_per_beat.max(1.0));
    }
    beats
}

/// The most destinations any single [`mooloop_core::EffectKind`]'s parameter
/// table can drive on one effect slot at once -- `EqParams`'s fifty,
/// verified against every kind by `mooloop_dsp`'s own
/// `no_effect_kinds_descriptor_table_exceeds_the_curve_frames_capacity`,
/// since this constant and that crate's identically-derived one can never be
/// merged into a single definition across the crate boundary.
const MAX_EFFECT_CURVE_DESTINATIONS: usize = mooloop_core::effect::EQ_DESCRIPTOR_COUNT;

/// The most destinations any single [`mooloop_core::DeviceKind`]'s parameter
/// table can drive on one channel's source at once -- DS-01's ninety-two,
/// the largest generator table by a wide margin (`generators_never_drive_more_curve_ids_than_the_pool_holds`
/// checks every kind against it below).
const MAX_SOURCE_CURVE_DESTINATIONS: usize = mooloop_core::ds01::DESCRIPTORS.len();

/// One node's driven destinations for one block, captured from resolved
/// modulation/automation instead of pushed as `Event::ParamValue`s onto a
/// shared, capacity-256 `EventList` -- `docs/plans/automation-curves/00-status.md`,
/// written against `reports/fable-2026-09-22.md` finding 3: "one destination
/// emits 256 events and fills the list alone; a second is silently dropped."
///
/// `N` bounds how many *destinations* one node can drive at once, not how
/// many ticks: every accepted row gets the full block's
/// `MAX_CONTROL_TICKS_PER_BLOCK`, regardless of how many other destinations
/// are also driven, which is the whole difference from the list this
/// replaces.
struct CurvePool<const N: usize> {
    ids: [u32; N],
    ticks: [[f32; MAX_CONTROL_TICKS_PER_BLOCK]; N],
    count: usize,
    /// How many of each row's `MAX_CONTROL_TICKS_PER_BLOCK` slots this
    /// block actually resolved -- set once per block by [`Self::clear`],
    /// which is what lets [`Self::fill`] hand out the right-length slice
    /// without the caller re-deriving the same tick count a second time.
    active_ticks: usize,
}

impl<const N: usize> CurvePool<N> {
    fn empty() -> Self {
        Self {
            ids: [0; N],
            ticks: [[0.0; MAX_CONTROL_TICKS_PER_BLOCK]; N],
            count: 0,
            active_ticks: 0,
        }
    }

    /// Empty the pool for a new node/block, recording how many ticks are
    /// valid this time -- a short final block resolves fewer than
    /// `MAX_CONTROL_TICKS_PER_BLOCK`, and a row's unwritten tail must never
    /// be handed to a node as though it were this block's data.
    fn clear(&mut self, ticks: usize) {
        self.count = 0;
        self.active_ticks = ticks.min(MAX_CONTROL_TICKS_PER_BLOCK);
    }

    /// Reserve the next row for `id`, returning where to write its tick
    /// values. `None` past `N` destinations driven on this node at once --
    /// refused and left for the caller to count, rather than silently
    /// dropped, mirroring `RenderState::defer_command`'s refusal.
    fn begin(&mut self, id: u32) -> Option<&mut [f32; MAX_CONTROL_TICKS_PER_BLOCK]> {
        if self.count == N {
            return None;
        }
        let index = self.count;
        self.ids[index] = id;
        self.count += 1;
        Some(&mut self.ticks[index])
    }

    /// Write this pool's curves into `buf`, each trimmed to the ticks
    /// [`Self::clear`] recorded, and return how many rows were written.
    /// `buf` is caller-owned (a small on-stack array of `ControlCurve`,
    /// which borrows from `self`) because `AudioNode::apply_curves` takes a
    /// slice, not an iterator.
    fn fill<'a>(&'a self, buf: &mut [ControlCurve<'a>]) -> usize {
        let count = self.count.min(buf.len());
        let active_ticks = self.active_ticks;
        for (slot, (&id, ticks)) in buf[..count]
            .iter_mut()
            .zip(self.ids.iter().zip(self.ticks.iter()))
        {
            *slot = ControlCurve {
                id,
                values: &ticks[..active_ticks],
            };
        }
        count
    }
}

type EffectCurvePool = CurvePool<MAX_EFFECT_CURVE_DESTINATIONS>;
type SourceCurvePool = CurvePool<MAX_SOURCE_CURVE_DESTINATIONS>;

/// One channel's modulator outputs for one block, captured at each control
/// subdivision.
///
/// Only the rack: a generator's outlets are published once a block, so they
/// ride beside this table rather than in it. Copying eight block-constant
/// values into all 256 tick rows would be 8 KB a live channel for numbers
/// that do not change, and `ControlSources` is what puts the two halves back
/// into one flat address space at the point a route reads them.
type ControlOutputs = [[f32; MAX_MODULATORS_PER_CHANNEL]; MAX_CONTROL_TICKS_PER_BLOCK];

/// Every channel's note gates at every control subdivision of one block, for
/// the modulators that key off them.
///
/// Tick-major, because a rack is advanced one subdivision at a time and needs
/// every channel's gates at that subdivision -- an envelope can follow another
/// channel's notes. At full size this is 192 KB, so it is kept and cleared a
/// row at a time rather than built on the stack of the audio callback.
type GateTable = [[NoteGateEvents; MAX_CHANNELS]; MAX_CONTROL_TICKS_PER_BLOCK];

/// Control-side identity of an effect's asynchronously prepared resource, for
/// the kinds that have one. Reverb was the original case and no longer is:
/// the FDN hall reallocates nothing, so its parameters travel as ordinary
/// `SetEffectParam` commands. The mechanism stays for the retained-audio
/// buffer, whose ring size genuinely cannot change on the audio thread.
fn effect_resource_key(params: mooloop_core::EffectParams) -> Option<u64> {
    match params {
        // A hosted plugin's processor is swapped by `ReplaceEffect` when the
        // plugin restarts, keyed by its song slot: a replacement that
        // arrives after the device was removed, or for a different plugin,
        // finds a different key and does nothing
        // (`docs/plans/plugin-hosting/04-the-plugin-rack.md`).
        mooloop_core::EffectParams::Plugin(slot) => Some(u64::from(slot.0)),
        _ => params.buffer().copied().map(buffer_allocation_key),
    }
}

#[derive(Clone, Copy)]
struct PendingEffectParams {
    events: [Option<TimedEvent>; MAX_PENDING_EFFECT_PARAMS],
}

/// The already-ticked control signal for one channel's current block. It is
/// deliberately a read-only view: only `RenderState` advances modulators, so
/// graph order cannot accidentally change their phase.
struct ModulationBlock<'a> {
    rack: &'a ModRack,
    outputs: &'a ControlOutputs,
    /// What this channel's generator published at the end of the *previous*
    /// block, constant for the whole of this one. Held here rather than in
    /// `outputs` because it is per block, not per tick.
    outlets: &'a [f32; MAX_GENERATOR_OUTLETS],
    /// The keyboard's mod wheel and aftertouch on this channel, constant
    /// for the block: they change between blocks, in `apply_midi`.
    performance: &'a [f32; PERFORMANCE_SOURCES],
    ticks: usize,
}

impl ModulationBlock<'_> {
    /// The channel's control sources at one tick, as one flat address space.
    fn sources(&self, tick: usize) -> mooloop_core::modulation::ControlSources<'_> {
        mooloop_core::modulation::ControlSources {
            modulators: &self.outputs[tick],
            outlets: self.outlets,
            performance: self.performance,
        }
    }
}

/// Where the automation pass was reading, captured before a command moves it.
///
/// Three scalars rather than a list of covering patterns, because the point of
/// capturing it is to re-derive that coverage *after* the command has landed
/// -- and none of the three commands that move the playhead changes more than
/// one of them.
#[derive(Clone, Copy)]
struct AutomationPosition {
    mode: PlaybackMode,
    /// The pattern-mode selection, ignored in song mode.
    pattern: usize,
    tick: f64,
}

/// The clip automation covering this block. Unlike modulation this is not
/// pre-ticked: a lane is a sorted breakpoint list, so resolving it per control
/// tick is a binary search rather than state that must advance exactly once.
struct AutomationBlock<'a> {
    sequencer: &'a Sequencer,
    /// Transport position at frame 0, in song ticks.
    start_tick: f64,
    ticks_per_sample: f64,
    ticks: usize,
}

/// One destination's lane, already resolved to the pattern driving it.
struct AutomationCurve<'a> {
    lane: &'a AutomationLane,
    /// Pattern-local tick at frame 0.
    start_tick: f64,
    length_ticks: u32,
}

impl<'a> AutomationCurve<'a> {
    /// The lane driving `destination` at `song_tick`, if one is. This is the
    /// one answer to "has a lane taken this base over": the block's
    /// resolution and a knob edit's decision not to queue both ask here, so
    /// they cannot disagree about it.
    fn at(sequencer: &'a Sequencer, destination: ParamAddr, song_tick: f64) -> Option<Self> {
        let (lane, start_tick, length_ticks) =
            sequencer.automation_lane_at(destination, song_tick)?;
        Some(Self {
            lane,
            start_tick,
            length_ticks,
        })
    }
}

impl<'a> AutomationBlock<'a> {
    fn curve_for(&self, destination: ParamAddr) -> Option<AutomationCurve<'a>> {
        AutomationCurve::at(self.sequencer, destination, self.start_tick)
    }

    /// Normalized value at control tick `tick`. The pattern wraps underneath a
    /// block that straddles the loop point, which is why the position is
    /// recomputed per tick instead of advanced.
    fn value_at(&self, curve: &AutomationCurve<'_>, tick: usize) -> Option<f32> {
        let elapsed = (tick * CONTROL_RATE_FRAMES) as f64 * self.ticks_per_sample;
        let position = curve.start_tick + elapsed;
        let wrapped = if curve.length_ticks == 0 {
            position
        } else {
            position.rem_euclid(curve.length_ticks as f64)
        };
        curve.lane.value_at(wrapped)
    }
}

impl PendingEffectParams {
    const fn empty() -> Self {
        Self {
            events: [None; MAX_PENDING_EFFECT_PARAMS],
        }
    }

    fn clear(&mut self) {
        self.events.fill(None);
    }

    fn queue(&mut self, event: TimedEvent) {
        let Event::ParamValue { id, .. } = event.event else {
            if let Some(empty) = self.events.iter_mut().find(|entry| entry.is_none()) {
                *empty = Some(event);
            }
            return;
        };
        if let Some(existing) = self.events.iter_mut().find(|existing| {
            matches!(existing, Some(TimedEvent { event: Event::ParamValue { id: existing_id, .. }, .. }) if *existing_id == id)
        }) {
            *existing = Some(event);
            return;
        }
        if let Some(empty) = self.events.iter_mut().find(|entry| entry.is_none()) {
            *empty = Some(event);
        } else {
            // The command queue is already bounded. Under pathological
            // automation traffic, keep the newest value rather than retaining
            // a stale one indefinitely.
            self.events[0] = Some(event);
        }
    }

    /// Returns how many of the queued events `destination` had no room for.
    fn copy_to(&self, destination: &mut EventList) -> u64 {
        let mut refused = 0;
        for event in self.events.iter().flatten() {
            if !destination.push_ordered(*event) {
                refused += 1;
            }
        }
        refused
    }
}

/// A fixed-size chain of optional effect nodes plus the per-slot machinery
/// that feeds them. Channels and mixer buses both own one, which is the whole
/// reason effect commands address an `EffectTarget` rather than a channel.
/// Everything a populated effect slot carries besides its node, its dry-path
/// aligner, and its analyzer — all of which are already boxed.
///
/// Grouped and boxed so an addressable-but-empty slot costs a pointer rather
/// than its full state. A chain addresses `MAX_EFFECTS_PER_CHANNEL` slots
/// because that is the width of the index, and a project populates a handful;
/// holding a 320-byte event queue and a 140-byte parameter set for each of
/// the 256 was 140 KiB per chain, and a chain lives on every one of 256
/// channels (`docs/plans/archive/modulator-capacity/`).
///
/// Allocated on the control thread and installed, like the node beside it.
pub struct EffectSlot {
    /// Which device this is, as routes and lanes name it.
    ///
    /// The engine keeps indexing by position -- that is what makes the inner
    /// loop cheap -- and this is the one field that lets a position be turned
    /// back into an address without a table. `control_events_for_slot` reads
    /// it once per slot per block to build the address it looks a route up
    /// by; `EffectChain::slot_of` walks it the other way, on removal only.
    device: mooloop_core::DeviceId,
    /// The slot's persisted device kind, tracked independently of the
    /// trait object so prepared resource replacements can refuse stale work.
    kind: Option<mooloop_core::EffectKind>,
    /// The authoritative knob value. Nodes retain only the resolved value they
    /// were last sent; keeping the base here is what lets a knob move
    /// underneath an active modulator without fighting it.
    base_params: Option<mooloop_core::EffectParams>,
    /// Control-side identity of an asynchronously prepared device resource.
    resource_key: Option<u64>,
    /// Parameter events queued between blocks by `EngineCommand::SetEffectParam`
    /// and consumed by the next block.
    events: PendingEffectParams,
    /// Host controls. These belong to the slot rather than to the device in
    /// it, so replacing an effect keeps the wet/dry and trims dialled there.
    bypassed: bool,
    wet_dry: f32,
    input_trim: f32,
    output_trim: f32,
    /// The four host controls above, and a container's Mix, as the audio
    /// hears them. See [`HostRamps`].
    ramps: HostRamps,
    /// Frames this slot has spent fading out for a removal, or for a device
    /// about to be installed in its place, from the first time the executor
    /// asked. `None` when nobody has. While it is `Some` the slot is
    /// [`leaving`](Self::leaving): it fades out as a bypass would, without
    /// touching `bypassed`, which is the user's and has to survive a swap.
    removal_waited: Option<u32>,
    /// Whether this slot arrived with its own host controls and base
    /// parameters ([`EffectSlot::for_effect`]) rather than inheriting the
    /// occupant's ([`EffectSlot::for_device`]). Read once, by `install`.
    carries_host: bool,
    /// For a container: how many of the rows after this one are inside it,
    /// and the ring that delays its dry copy by that run's declared latency.
    ///
    /// Both arrive on [`StructuralCommand::SetContainerSpan`] rather than on
    /// the value ring, for the reason `SetCompensation` is structural: the
    /// ring is allocated on the control thread and the displaced one is
    /// reclaimed there, because the audio thread may do neither. Zero and
    /// `None` on every leaf, which costs a byte and a pointer in a struct
    /// that is only allocated for occupied slots anyway.
    container_children: u8,
    container_align: Option<Box<IntegerDelay>>,
    /// For the head of a layer's branch: the ring that holds this branch
    /// back until the layer's longest branch has caught up, sized by
    /// [`mooloop_core::branch_alignment`]. `None` on every other row, and on
    /// the longest branch itself -- so a layer whose branches declare no
    /// latency, which is nearly all of them, carries none.
    ///
    /// Distinct from `container_align`, which a branch head that is itself a
    /// container also has: that one delays the box's *dry* copy by its own
    /// run; this one delays the whole branch's *output* to meet its siblings.
    /// Arrives on [`StructuralCommand::SetBranchAlign`], for the reason the
    /// span does.
    ///
    /// A row that stops being a branch keeps whatever it last held, and
    /// nothing reads it: the loop only asks for this at a layer's branch
    /// boundary, and every edit that makes a row a branch republishes it.
    branch_align: Option<Box<IntegerDelay>>,
    /// Consecutive frames of silent input this slot has seen.
    ///
    /// Here rather than in a `[u32; MAX_EFFECTS_PER_CHANNEL]` beside the
    /// nodes for the reason this whole struct exists: an addressable-but-empty
    /// slot should cost a pointer, and there are 256 of them on each of 272
    /// chains. A slot that has never held a device never counts anything.
    silent_frames: u32,
}

impl EffectSlot {
    /// A fresh slot, allocated on the control thread to be installed.
    pub fn new() -> Self {
        Self {
            device: mooloop_core::DeviceId::UNASSIGNED,
            kind: None,
            base_params: None,
            resource_key: None,
            events: PendingEffectParams::empty(),
            bypassed: false,
            wet_dry: 1.0,
            input_trim: 1.0,
            output_trim: 1.0,
            ramps: HostRamps::new(),
            removal_waited: None,
            carries_host: false,
            container_children: 0,
            container_align: None,
            branch_align: None,
            silent_frames: 0,
        }
    }

    /// A container's Mix, from the parameters it was last sent. Unity on a
    /// leaf, where nothing reads it -- and where asking would be wrong:
    /// `CONTAINER_PARAM_MIX` is id 0, which on a leaf is one of its own knobs.
    fn container_mix(&self) -> f32 {
        if !self.kind.is_some_and(mooloop_core::EffectKind::is_container) {
            return 1.0;
        }
        self.base_params
            .and_then(|params| params.get(mooloop_core::CONTAINER_PARAM_MIX))
            .unwrap_or(1.0)
            .clamp(0.0, 1.0)
    }

    /// Where each ramp is headed: the controls as they stand.
    fn ramp_targets(&self) -> [f32; 5] {
        [
            self.wet_dry,
            self.input_trim,
            self.output_trim,
            if self.leaving() { 0.0 } else { 1.0 },
            self.container_mix(),
        ]
    }

    /// Aim every ramp at its control, at the block's sample rate.
    fn aim_ramps(&mut self, sample_rate: u32) {
        let targets = self.ramp_targets();
        self.ramps.aim(targets, sample_rate);
    }

    /// Jump every ramp to its control: for a document arriving, and for a
    /// row with no audio path of its own to ramp along.
    fn settle_ramps(&mut self) {
        let targets = self.ramp_targets();
        self.ramps.settle(targets);
    }

    /// Jump the leaf controls' ramps -- wet/dry and the trims -- to their
    /// controls, for a row that does not use them: a container, or a slot
    /// out of the path, where a move has nothing audible to ramp along and
    /// a ramp left travelling would keep the chain awake for nothing.
    fn settle_leaf_ramps(&mut self) {
        self.ramps.wet.reset_to(self.wet_dry);
        self.ramps.input.reset_to(self.input_trim);
        self.ramps.output.reset_to(self.output_trim);
    }

    /// Whether every ramp has arrived at its control -- judged against the
    /// controls rather than against where the ramps were last aimed, so a
    /// move made while the chain slept still counts as one to make.
    fn ramps_settled(&self) -> bool {
        self.ramps.settled_at(self.ramp_targets())
    }

    /// Whether this slot is on its way out of the path: bypassed, or being
    /// removed or replaced (MOO-108, MOO-172).
    fn leaving(&self) -> bool {
        self.bypassed || self.removal_waited.is_some()
    }

    /// Whether bypass has finished taking this slot out of the path. Until
    /// it has, the device keeps running under the crossfade.
    fn out_of_path(&self) -> bool {
        self.leaving() && self.ramps.active.value() == 0.0
    }

    /// Whether this slot is coming back from being wholly out of the path,
    /// which is when its device is told the audio it holds is stale. A
    /// device just installed is here too, and starts clean for the same
    /// reason: it has heard nothing yet.
    fn returning(&self) -> bool {
        !self.leaving() && self.ramps.active.value() == 0.0
    }

    /// Land a bypass fade that has reached [`BYPASS_FADE_FLOOR`]: the
    /// one-pole would otherwise spend a hundred milliseconds creeping the
    /// last sixty decibels, running a device nobody can hear.
    fn finish_bypass_fade(&mut self) {
        if self.leaving() && self.ramps.active.value() <= BYPASS_FADE_FLOOR {
            self.ramps.active.reset_to(0.0);
        }
    }
}

/// How far down a bypass or removal fade has to get before the device is
/// taken out of the path: -60 dB, about 35 ms into the 5 ms one-pole. A
/// step of a thousandth of the difference between the wet and dry paths is
/// a tenth of what the continuity family allows.
const BYPASS_FADE_FLOOR: f32 = 1.0e-3;

/// The longest a removal waits for its fade before going anyway (MOO-108).
///
/// The fade only runs while the chain is processed, and a chain can stop
/// being processed while it waits -- a muted strip that has settled is
/// skipped outright. It is inaudible then, so going without the fade costs
/// nothing; waiting forever would hold every edit queued behind it.
const REMOVAL_MAX_WAIT_S: f32 = 0.1;

/// A slot's host controls as the audio hears them (MOO-108).
///
/// Wet/dry, the two trims, a container's Mix, and whether the device is in
/// the path at all each follow their control through the mixer's one-pole
/// over [`STRIP_GAIN_SMOOTH_S`], per sample -- the primitive and the time
/// MOO-107 put on faders, pans and mutes, so every host move in the engine
/// ramps the same way. A settled ramp returns its target exactly, which is
/// what keeps a still chain bit-identical to the flat multiply it replaced.
#[derive(Clone, Copy)]
struct HostRamps {
    wet: Smoothed,
    input: Smoothed,
    output: Smoothed,
    /// One while the device is in the path, zero once bypass has taken it
    /// out; between the two, the crossfade between the device's output and
    /// the bypassed path.
    active: Smoothed,
    /// A container's Mix. Unused on a leaf.
    mix: Smoothed,
    /// The rate the times were last set for. A slot is built before it
    /// knows one, and learns it from the first block it runs in.
    sample_rate: u32,
}

impl HostRamps {
    /// Where every control of a fresh slot starts.
    const RATE_UNSET: u32 = 0;

    fn new() -> Self {
        let at = |value| Smoothed::new(value, STRIP_GAIN_SMOOTH_S, 48_000);
        Self {
            wet: at(1.0),
            input: at(1.0),
            output: at(1.0),
            active: at(1.0),
            mix: at(1.0),
            sample_rate: Self::RATE_UNSET,
        }
    }

    fn all_mut(&mut self) -> [&mut Smoothed; 5] {
        [
            &mut self.wet,
            &mut self.input,
            &mut self.output,
            &mut self.active,
            &mut self.mix,
        ]
    }

    fn aim(&mut self, targets: [f32; 5], sample_rate: u32) {
        let retime = sample_rate != self.sample_rate;
        self.sample_rate = sample_rate;
        for (ramp, target) in self.all_mut().into_iter().zip(targets) {
            if retime {
                ramp.set_time(STRIP_GAIN_SMOOTH_S, sample_rate);
            }
            ramp.set_target(target);
        }
    }

    fn settle(&mut self, targets: [f32; 5]) {
        for (ramp, target) in self.all_mut().into_iter().zip(targets) {
            ramp.reset_to(target);
        }
    }

    fn settled_at(&self, targets: [f32; 5]) -> bool {
        [self.wet, self.input, self.output, self.active, self.mix]
            .iter()
            .zip(targets)
            .all(|(ramp, target)| ramp.value() == target)
    }
}

/// The equal-power pair for a wet/dry or Mix position: `(dry, wet)` gains.
fn equal_power(wet: f32) -> (f32, f32) {
    let blend = wet * core::f32::consts::FRAC_PI_2;
    (blend.cos(), blend.sin())
}

impl EffectSlot {
    /// A fresh slot that already knows which device it is about to hold.
    ///
    /// Every install goes through this rather than `new`, because a slot that
    /// does not know its identity is a slot no route can find.
    pub fn for_device(device: mooloop_core::DeviceId) -> Self {
        Self {
            device,
            ..Self::new()
        }
    }

    /// A fresh slot for `effect` as the document holds it: its identity, its
    /// parameters as the base a modulator moves around, and its own bypass,
    /// wet/dry and trims.
    ///
    /// For an install that replaces what a slot *is*, such as a preset
    /// loaded into a row (MOO-172), where [`for_device`](Self::for_device)
    /// would keep the occupant's host controls and seed the kind's defaults
    /// as the base.
    pub fn for_effect(effect: &mooloop_core::EffectSlotState) -> Self {
        Self {
            device: effect.id,
            base_params: Some(effect.params),
            bypassed: effect.bypassed,
            wet_dry: effect.wet_dry.clamp(0.0, 1.0),
            input_trim: effect.input_trim.clamp(0.0, MAX_LINEAR_GAIN),
            output_trim: effect.output_trim.clamp(0.0, MAX_LINEAR_GAIN),
            carries_host: true,
            ..Self::new()
        }
    }
}

impl Default for EffectSlot {
    fn default() -> Self {
        Self::new()
    }
}

struct EffectChain {
    /// Processed in order after whatever produced the audio. Slots are `None`
    /// until a node is installed structurally.
    nodes: [Option<Box<dyn AudioNode + Send>>; MAX_EFFECTS_PER_CHANNEL],
    /// One past the highest occupied node slot. Keeps the realtime pass
    /// proportional to the populated chain instead of its addressable size.
    bound: usize,
    /// Per-slot host and control state, present only where a slot is (or was)
    /// populated. See [`EffectSlot`] for why this is one boxed struct rather
    /// than a parallel array per field.
    slots: [Option<Box<EffectSlot>>; MAX_EFFECTS_PER_CHANNEL],
    /// Reused while each sequential slot processes. See
    /// `PendingEffectParams` for why this is not stored per slot. Holds only
    /// the once-per-block queued knob/buffer commands now
    /// (`PendingEffectParams::copy_to`): a driven parameter's per-tick
    /// values go into `curve_scratch` instead of here -- see
    /// `control_events_for_slot`.
    event_scratch: EventList,
    /// This slot's driven destinations for the current block, resolved by
    /// `control_events_for_slot` and handed to the node through
    /// `AudioNode::apply_curves` in place of pushing a `ParamValue` event per
    /// tick per destination onto `event_scratch`. One scratch buffer is
    /// enough for the same reason `event_scratch` is: slots process
    /// sequentially. Boxed: `MAX_EFFECT_CURVE_DESTINATIONS` rows of
    /// `MAX_CONTROL_TICKS_PER_BLOCK` ticks is tens of kilobytes, the same
    /// reason `ControlOutputs` and `GateTable` are boxed elsewhere in this
    /// file.
    curve_scratch: Box<EffectCurvePool>,
    /// Destinations this chain could not fit into one slot's curve pool at
    /// once, this session -- `N` destinations already comfortably covers
    /// every shipped effect kind's whole parameter table
    /// (`no_effect_kinds_descriptor_table_exceeds_the_curve_frames_capacity`
    /// in `mooloop-dsp`), so a nonzero count here means a future kind's
    /// table grew past it. Counted rather than silently dropped, mirroring
    /// `defer_command`'s refusal.
    curve_refusals: u64,
    /// Parameter events `event_scratch` had no room for, since this chain was
    /// built. See [`RenderState::refused_events`].
    refused_events: u64,
    /// Slot inputs that arrived carrying a NaN or an infinity since the
    /// render loop last collected them (MOO-176). Each was silenced before
    /// the device saw it. The loop drains this into
    /// `BusMeters::effect_faults` after running the chain, so the count
    /// crosses to the interface with one relaxed add, and only on a block
    /// that had a fault.
    faults_unpublished: u32,
    /// Per-slot dry-path delay matching the installed node's reported
    /// latency, so the wet/dry blend never mixes time-misaligned signals.
    /// Allocated off the realtime thread, next to the node it belongs to.
    dry_align: [Option<Box<IntegerDelay>>; MAX_EFFECTS_PER_CHANNEL],
    /// Input analyzers follow effect slots during reorders. They are generic
    /// host instrumentation, not EQ-specific DSP state. The boxes are built
    /// with nodes so empty addressable slots stay compact.
    analyzers: [Option<Box<SpectrumAnalyzer>>; MAX_EFFECTS_PER_CHANNEL],
    /// One scratch buffer is enough: chain slots process sequentially, so no
    /// slot needs to retain its dry signal once its mix has been applied.
    /// Keeping this per-chain rather than per-slot makes the full 256-slot
    /// addressable chain practical.
    dry: StereoBus,
    /// One dry copy per *open* container, allocated the first time a
    /// container is installed on this chain and kept thereafter.
    ///
    /// Indexed by nesting depth rather than by slot, which is the whole
    /// saving: what a chain needs at once is one buffer per box it is
    /// currently inside, not one per box it holds. Ten sibling containers
    /// need one of these; four nested ones need four. A chain with no
    /// container at all pays a pointer.
    ///
    /// The graph only grows, in the same way and for the same reason channel
    /// storage does: freeing this would be a deallocation reached from a
    /// structural edit, and a chain that held a container once is likely to
    /// again.
    container_dry: Option<Box<ContainerScratch>>,
}

/// The dry copies a chain needs while it is inside containers.
///
/// Allocated on the control thread and installed, like every other piece of
/// chain state that owns heap.
pub struct ContainerScratch {
    dry: [StereoBus; MAX_CONTAINER_DEPTH],
    /// What a *layer* needs besides its dry copy, per open depth. Absent
    /// until the chain holds a layer, so a chain of plain boxes pays for
    /// none of it.
    branches: Option<Box<BranchScratch>>,
}

/// A layer's two working buffers at each nesting depth: the input every
/// branch starts from, and the sum the finished branches accumulate into.
///
/// Per depth rather than per layer for `ContainerScratch`'s reason -- ten
/// sibling layers are open one at a time. The layer's own mix blends against
/// `ContainerScratch::dry`, so there is no third buffer. Two buses of
/// `MAX_BLOCK_SIZE` stereo `f32` at each of `MAX_CONTAINER_DEPTH` depths is
/// 512 KiB, allocated once per chain that has ever held a layer.
struct BranchScratch {
    input: [StereoBus; MAX_CONTAINER_DEPTH],
    sum: [StereoBus; MAX_CONTAINER_DEPTH],
}

impl ContainerScratch {
    /// Dry copies only: enough for any number of chains, and no layer.
    pub fn new() -> Self {
        Self {
            dry: std::array::from_fn(|_| StereoBus::with_capacity(MAX_BLOCK_SIZE)),
            branches: None,
        }
    }

    /// Dry copies and a layer's branch buffers.
    pub fn with_layers() -> Self {
        Self {
            branches: Some(Box::new(BranchScratch {
                input: std::array::from_fn(|_| StereoBus::with_capacity(MAX_BLOCK_SIZE)),
                sum: std::array::from_fn(|_| StereoBus::with_capacity(MAX_BLOCK_SIZE)),
            })),
            ..Self::new()
        }
    }

    /// What `effects` needs, or `None` when it holds no container at all.
    ///
    /// The one rule for which scratch a chain gets, read by the loader and
    /// by the control thread's span publisher alike.
    pub fn for_chain(effects: &[mooloop_core::EffectSlotState]) -> Option<Self> {
        let mut flows = effects.iter().filter_map(|effect| effect.params.container_flow());
        let first = flows.next()?;
        let layered = first == mooloop_core::ContainerFlow::Parallel
            || flows.any(|flow| flow == mooloop_core::ContainerFlow::Parallel);
        Some(if layered { Self::with_layers() } else { Self::new() })
    }

    /// Whether this scratch can run a layer.
    pub fn holds_layers(&self) -> bool {
        self.branches.is_some()
    }
}

impl Default for ContainerScratch {
    fn default() -> Self {
        Self::new()
    }
}

/// One container the chain is currently inside.
#[derive(Clone, Copy)]
struct OpenRun {
    /// One past the last row of the run: the index at which it closes.
    end: usize,
    /// The container's own row, which owns the mix and the dry-path ring.
    slot: usize,
    /// Whether the run is a layer's: its direct children are branches, each
    /// started from the layer's input and summed at the end.
    parallel: bool,
    /// For a layer, the row heading the branch now running, whose
    /// `branch_align` delays it...
    branch: usize,
    /// ...and one past that branch's last row, where the next one starts.
    branch_end: usize,
}

impl EffectChain {
    fn new() -> Self {
        Self {
            nodes: std::array::from_fn(|_| None),
            bound: 0,
            slots: std::array::from_fn(|_| None),
            event_scratch: EventList::empty(),
            curve_scratch: Box::new(EffectCurvePool::empty()),
            curve_refusals: 0,
            refused_events: 0,
            faults_unpublished: 0,
            dry_align: std::array::from_fn(|_| None),
            analyzers: std::array::from_fn(|_| None),
            dry: StereoBus::with_capacity(MAX_BLOCK_SIZE),
            container_dry: None,
        }
    }

    /// Where `device` currently sits in this chain.
    ///
    /// Identity resolved to position, on the control-command drain rather
    /// than in a sample loop -- the same bargain `ModRack::slot_for` makes
    /// for modulator sources. Bounded by `bound` rather than by the 256
    /// addressable slots, so an empty chain answers in no time at all.
    fn slot_of(&self, device: mooloop_core::DeviceId) -> Option<usize> {
        if !device.is_assigned() {
            return None;
        }
        (0..self.bound).find(|slot| {
            self.slot(*slot)
                .is_some_and(|state| state.device == device)
        })
    }

    /// One slot's host and control state, if it has any.
    fn slot(&self, slot: usize) -> Option<&EffectSlot> {
        self.slots.get(slot)?.as_deref()
    }

    fn slot_mut(&mut self, slot: usize) -> Option<&mut EffectSlot> {
        self.slots.get_mut(slot)?.as_deref_mut()
    }

    /// Whether this slot is out of the path: bypassed, and the bypass fade
    /// has finished. A slot fading out still runs its device (MOO-108).
    fn bypassed(&self, slot: usize) -> bool {
        self.slot(slot).is_some_and(EffectSlot::out_of_path)
    }

    /// Jump every slot's host ramps to its controls. See
    /// [`RenderState::settle_mixer`].
    fn settle_ramps(&mut self) {
        for state in self.slots[..self.bound].iter_mut().flatten() {
            state.settle_ramps();
        }
    }

    /// Tell the device in `slot` -- every device in its run, for a
    /// container -- that what it holds is stale, because the slot is coming
    /// back from being wholly out of the path. A delay un-bypassed has to
    /// start empty rather than play the repeats it held when it went out
    /// (MOO-108); it was not called while it was out, so what it holds is
    /// audio from then.
    fn restart_returning(&mut self, slot: usize) {
        let last = slot + self.container_children(slot);
        for row in slot..=last.min(self.bound.saturating_sub(1)) {
            if let Some(node) = self.nodes[row].as_mut() {
                node.on_discontinuity(Discontinuity::Seek);
            }
        }
    }

    /// Whether this slot holds a container rather than a leaf device.
    ///
    /// A container has no signal path of its own: its children are rows of
    /// this same chain, which the loop is already about to run in order.
    fn is_container(&self, slot: usize) -> bool {
        self.slot(slot)
            .and_then(|state| state.kind)
            .is_some_and(mooloop_core::EffectKind::is_container)
    }

    /// Whether `slot` lies inside the run of a container that is out of the
    /// path, and so is not heard at all. Control thread and the executor's
    /// hold only; a scan of the rows before it.
    fn inside_silent_container(&self, slot: usize) -> bool {
        (0..slot.min(self.bound)).any(|head| {
            self.is_container(head)
                && head + self.container_children(head) >= slot
                && self.bypassed(head)
        })
    }

    /// The container whose run holds `slot` and which is on its way out of
    /// the path, if there is one. The outermost, if several are.
    fn enclosing_leaving_container(&self, slot: usize) -> Option<usize> {
        (0..slot.min(self.bound)).find(|&head| {
            self.is_container(head)
                && head + self.container_children(head) >= slot
                && self.slot(head).is_some_and(EffectSlot::leaving)
        })
    }

    /// How many rows the container in `slot` encloses. Zero for a leaf, and
    /// zero for an empty container, which is the same thing to this loop.
    fn container_children(&self, slot: usize) -> usize {
        self.slot(slot).map_or(0, |state| state.container_children as usize)
    }

    /// Remove every node, queuing the boxes for off-thread disposal.
    fn clear(&mut self, reclaim: &mut Reclaim) {
        for slot in 0..MAX_EFFECTS_PER_CHANNEL {
            let displaced = ReclaimedEffect {
                node: self.nodes[slot].take(),
                align: self.dry_align[slot].take(),
                analyzer: self.analyzers[slot].take(),
                state: self.slots[slot].take(),
                channel: None,
            };
            if !displaced.is_empty() {
                reclaim.push(displaced);
            }
        }
        self.bound = 0;
    }

    fn refresh_bound(&mut self) {
        self.bound = self
            .nodes
            .iter()
            .rposition(Option::is_some)
            .map_or(0, |slot| slot + 1);
    }

    /// Install a node together with its dry-path delay, returning whichever
    /// occupants must be reclaimed. An invalid slot returns the incoming
    /// pieces so they are never dropped by the realtime caller.
    #[allow(clippy::too_many_arguments)]
    fn install(
        &mut self,
        slot: usize,
        kind: mooloop_core::EffectKind,
        resource_key: Option<u64>,
        node: Box<dyn AudioNode + Send>,
        align: Option<Box<IntegerDelay>>,
        analyzer: Box<SpectrumAnalyzer>,
        mut state: Box<EffectSlot>,
    ) -> ReclaimedEffect {
        if slot < MAX_EFFECTS_PER_CHANNEL {
            if state.base_params.is_none() {
                state.base_params = Some(kind.default_params());
            }
            state.kind = Some(kind);
            // Host controls belong to the slot, not to the device in it, so
            // an effect swapped into an occupied slot keeps the wet/dry and
            // trims already dialled there -- unless it brought its own, as a
            // preset does. Either way the ramps continue from where the
            // occupant's were, so a changed wet/dry or trim travels.
            if let Some(previous) = self.slot(slot) {
                if !state.carries_host {
                    state.bypassed = previous.bypassed;
                    state.wet_dry = previous.wet_dry;
                    state.input_trim = previous.input_trim;
                    state.output_trim = previous.output_trim;
                }
                state.ramps = previous.ramps;
            } else {
                state.settle_ramps();
            }
            // **A device arrives faded out and fades in** (MOO-172), along
            // the bypass crossfade from the bypassed path, rather than
            // switching the chain's sound in one sample. An occupant it
            // displaces has already faded out: the executor holds an
            // install into an occupied slot until it has
            // (`RenderState::effect_slot_vacated`). A document load settles
            // every ramp after this, so a song opens at its own controls.
            //
            // Not a container: it has no sound of its own, and one arriving
            // around rows that are already playing (a wrap) must not take
            // them out of the path while it fades in. Rows arriving inside
            // it fade in themselves.
            if !kind.is_container() {
                state.ramps.active.reset_to(0.0);
            }
            state.resource_key = resource_key;
            self.bound = self.bound.max(slot + 1);
            ReclaimedEffect {
                node: self.nodes[slot].replace(node),
                align: std::mem::replace(&mut self.dry_align[slot], align),
                analyzer: self.analyzers[slot].replace(analyzer),
                state: self.slots[slot].replace(state),
                channel: None,
            }
        } else {
            ReclaimedEffect {
                node: Some(node),
                align,
                analyzer: Some(analyzer),
                state: Some(state),
                channel: None,
            }
        }
    }

    /// Replace only the realtime node and latency aligner. Host controls and
    /// display instrumentation deliberately remain attached to the slot.
    fn replace_if_kind(
        &mut self,
        slot: usize,
        expected_kind: mooloop_core::EffectKind,
        expected_resource_key: u64,
        resource_key: u64,
        node: Box<dyn AudioNode + Send>,
        align: Option<Box<IntegerDelay>>,
    ) -> ReclaimedEffect {
        let matches = self.slot(slot).is_some_and(|state| {
            state.kind == Some(expected_kind) && state.resource_key == Some(expected_resource_key)
        });
        // A frozen buffer's ring is a sample somebody is playing. Replacing
        // the node would take it away, and the commonest reason to replace one
        // is an ordinary tempo change -- so the swap is refused and the
        // replacement travels back down the reclaim ring unused, exactly as a
        // mismatched kind already does.
        let frozen = self.nodes[slot]
            .as_ref()
            .is_some_and(|node| node.holds_frozen_audio());
        if matches && !frozen {
            if let Some(state) = self.slot_mut(slot) {
                state.resource_key = Some(resource_key);
            }
            // **The history comes across** (MOO-137). A tempo change or a
            // HISTORY change builds a buffer at the new length, and until
            // this every unfrozen one arrived empty. Two copies bounded by
            // the smaller ring and no allocation, which is what makes it an
            // audio-thread job.
            let mut node = node;
            if let (Some(incoming), Some(outgoing)) = (
                node.as_buffer_device_mut(),
                self.nodes[slot].as_mut().and_then(|live| live.as_buffer_device_mut()),
            ) {
                incoming.adopt_history_from(outgoing);
            }
            ReclaimedEffect {
                node: self.nodes[slot].replace(node),
                align: std::mem::replace(&mut self.dry_align[slot], align),
                analyzer: None,
                state: None,
                channel: None,
            }
        } else {
            ReclaimedEffect {
                node: Some(node),
                align,
                analyzer: None,
                state: None,
                channel: None,
            }
        }
    }

    /// Move the devices `rows` names from `live` into this chain, and this
    /// chain's own nodes for those rows into `live` in their place (MOO-137).
    ///
    /// Boxes change places and nothing else happens: no allocation, and
    /// nothing is dropped here -- `live` leaves with the retired generation.
    /// What goes with a device is what it has been doing: its node, the dry
    /// ring that is aligned to it, its analyzer, its host ramps, its silence
    /// count and any knob moves queued for it. What stays is the structure
    /// the incoming project describes -- the slot's span, its containers'
    /// dry rings and its branch alignment. A row is only ever named here when
    /// its whole document state is identical on both sides, so the node that
    /// arrives is the one the row would have been built with.
    ///
    /// With `fade_in_the_rest`, every other device in this chain starts faded
    /// out of the path and fades in, as an installed one does.
    fn adopt_devices_from(
        &mut self,
        live: &mut EffectChain,
        rows: &[(u8, u8)],
        fade_in_the_rest: bool,
    ) {
        let mut carried = [false; MAX_EFFECTS_PER_CHANNEL];
        for &(from, to) in rows {
            let (from, to) = (usize::from(from), usize::from(to));
            if from >= MAX_EFFECTS_PER_CHANNEL || to >= MAX_EFFECTS_PER_CHANNEL {
                continue;
            }
            if self.nodes[to].is_none() || live.nodes[from].is_none() {
                continue;
            }
            carried[to] = true;
            std::mem::swap(&mut self.nodes[to], &mut live.nodes[from]);
            std::mem::swap(&mut self.dry_align[to], &mut live.dry_align[from]);
            std::mem::swap(&mut self.analyzers[to], &mut live.analyzers[from]);
            if let (Some(fresh), Some(held)) =
                (self.slots[to].as_deref_mut(), live.slots[from].as_deref_mut())
            {
                std::mem::swap(&mut fresh.ramps, &mut held.ramps);
                std::mem::swap(&mut fresh.silent_frames, &mut held.silent_frames);
                std::mem::swap(&mut fresh.events, &mut held.events);
            }
        }
        if fade_in_the_rest {
            for (slot, state) in self.slots[..self.bound].iter_mut().enumerate() {
                let Some(state) = state.as_deref_mut() else {
                    continue;
                };
                if !carried[slot] && !state.kind.is_some_and(mooloop_core::EffectKind::is_container) {
                    state.ramps.active.reset_to(0.0);
                }
            }
        }
    }

    fn remove(&mut self, slot: usize) -> ReclaimedEffect {
        let removed = if slot < MAX_EFFECTS_PER_CHANNEL {
            ReclaimedEffect {
                node: self.nodes[slot].take(),
                align: self.dry_align[slot].take(),
                analyzer: self.analyzers[slot].take(),
                state: self.slots[slot].take(),
                channel: None,
            }
        } else {
            ReclaimedEffect {
                node: None,
                align: None,
                analyzer: None,
                state: None,
                channel: None,
            }
        };
        self.refresh_bound();
        removed
    }

    /// Move the occupant of `from` to `to`, shifting everything between by
    /// one place: the same permutation `mooloop_core::structure::move_effect`
    /// performs on the model. A rotation of boxed pointers, so nothing is
    /// allocated or dropped here. Returns whether anything moved.
    fn move_slot(&mut self, from: usize, to: usize) -> bool {
        if from == to || from >= MAX_EFFECTS_PER_CHANNEL || to >= MAX_EFFECTS_PER_CHANNEL {
            return false;
        }
        let (low, high) = (from.min(to), from.max(to));
        if from < to {
            self.nodes[low..=high].rotate_left(1);
            self.slots[low..=high].rotate_left(1);
            self.dry_align[low..=high].rotate_left(1);
            self.analyzers[low..=high].rotate_left(1);
        } else {
            self.nodes[low..=high].rotate_right(1);
            self.slots[low..=high].rotate_right(1);
            self.dry_align[low..=high].rotate_right(1);
            self.analyzers[low..=high].rotate_right(1);
        }
        self.refresh_bound();
        true
    }

    fn set_bypassed(&mut self, slot: usize, bypassed: bool) {
        if let Some(state) = self.slot_mut(slot) {
            state.bypassed = bypassed;
        }
    }

    fn queue_param(&mut self, slot: usize, id: u32, value: f32) {
        if let Some(state) = self.slot_mut(slot) {
            // Queued between blocks, so it lands at the next block's first
            // frame. Repeated writes to a parameter coalesce to its newest
            // value, matching the command ring's latest-state semantics.
            state.events.queue(TimedEvent {
                offset: 0,
                event: Event::ParamValue { id, value },
            });
        }
    }

    /// Update a knob's base value through the descriptor table. This is the
    /// only path that writes base effect state after installation.
    fn set_base_param(&mut self, slot: usize, id: u32, value: f32) -> Option<f32> {
        self.slot_mut(slot)?.base_params.as_mut()?.set(id, value)
    }

    fn base_param(&self, slot: usize, id: u32) -> Option<f32> {
        self.slot(slot)?.base_params.as_ref()?.get(id)
    }

    /// Resolve every control signal aimed at this slot into a row of
    /// `curve_scratch`, one destination per row, one value per control tick.
    ///
    /// Until this plan, the per-tick loop below pushed an `Event::ParamValue`
    /// per tick per destination onto the slot's shared, 256-slot
    /// `event_scratch` (`event.rs::MAX_EVENTS`) -- fine for one destination
    /// at a typical block, but at `MAX_BLOCK_SIZE` one destination alone
    /// reaches 256 events and a second is silently dropped
    /// (`reports/fable-2026-09-22.md`, finding 3). `curve_scratch` gives
    /// each destination its own row instead, so nothing here competes with
    /// anything else driven on the same slot for room; only the slot's own
    /// *destination count* is bounded, at `MAX_EFFECT_CURVE_DESTINATIONS`,
    /// counted as a refusal rather than silently dropped past it.
    ///
    /// Automation and modulation compose rather than compete: a lane supplies
    /// the **base** the knob would otherwise supply, and the matrix adds its
    /// offsets on top. That ordering is what lets an LFO wobble around a drawn
    /// curve instead of one of them winning.
    ///
    /// # Write precedence
    ///
    /// Three writers reach an effect parameter, and this is the rule between
    /// them (`docs/MODULATION.md` states the same table):
    ///
    /// | Lane | Route | Base | Offset | Who writes the device |
    /// | --- | --- | --- | --- | --- |
    /// | yes | any | lane | routes, summed | this function, every control tick |
    /// | no | yes | knob | routes, summed | this function, every control tick |
    /// | no | no | knob | none | the knob's own `ParamValue`, queued once at offset 0 |
    ///
    /// "Lane" means [`AutomationCurve::at`] finds one; "route" means the
    /// rack `modulates` the destination under its descriptor's policy.
    /// `RenderState::effect_is_driven` asks the same two questions, so a knob
    /// edit queues a value exactly when this function will not write one. The
    /// knob always updates the stored base; under a lane that base is not
    /// heard until the lane goes, and clearing it hands the knob back.
    ///
    /// A recorded lane, when recording lands, is written from the knob and
    /// takes over as the base on the next control tick.
    ///
    /// Returns the tick count `curve_scratch`'s rows are valid for this
    /// block, `0` when nothing on this slot is driven at all. `clear`
    /// already records the same number on the pool itself, which is what
    /// lets `fill` trim its rows without being told again -- the return
    /// value is for a caller (a test, today) that wants the count without
    /// reaching into a private field.
    fn control_events_for_slot(
        &mut self,
        slot: usize,
        scope: EffectTarget,
        modulation: Option<&ModulationBlock<'_>>,
        automation: Option<&AutomationBlock<'_>>,
    ) -> usize {
        // Cleared unconditionally, before any early return: a block on
        // which this slot stops being driven altogether must not leave a
        // previous block's rows in the pool for `EffectChain::process` to
        // hand the node again. `curve_scratch.count > 0` is exactly what
        // that call site reads to decide whether to call `apply_curves` at
        // all, so a stale nonzero count there replays automation the
        // engine has already stopped resolving -- caught by
        // `a_slot_that_stops_being_driven_does_not_replay_a_stale_curve`.
        self.curve_scratch.clear(0);
        // No route in the channel's rack and no lane under the playhead means
        // every descriptor below can only reach its `continue`. Both are facts
        // about the channel, not the descriptor, and this runs once per effect
        // slot per block over everything the effect declares -- so asking here
        // is one question in place of a hundred and something.
        if !modulation.is_some_and(|modulation| modulation.rack.has_routes())
            && automation.is_none()
        {
            return 0;
        }
        let Some(state) = self.slot(slot) else {
            return 0;
        };
        let (Some(kind), Some(params)) = (state.kind, state.base_params) else {
            return 0;
        };
        // The address a route or lane could be naming this device by. Read
        // from the slot rather than derived from its position, which is the
        // whole of what durable identity costs the realtime path: one field
        // read per slot per block, in place of a cast.
        let device = state.device;
        let ticks = modulation
            .map(|modulation| modulation.ticks)
            .into_iter()
            .chain(automation.map(|automation| automation.ticks))
            .max()
            .unwrap_or(0);
        self.curve_scratch.clear(ticks);

        for descriptor in kind.descriptors() {
            let destination = ParamAddr::effect(scope, device, descriptor.id);
            // The destination's own declaration decides whether modulation is
            // legal here at all -- a stepped mode selector refuses it, so an
            // LFO cannot flap an algorithm switch. Automation is unaffected: a
            // lane is explicit authored intent, not a continuous signal.
            let policy = ModDestinationDescriptor::for_param(descriptor);
            let modulated = modulation
                .is_some_and(|modulation| modulation.rack.modulates(destination, &policy));
            let curve = automation.and_then(|automation| automation.curve_for(destination));
            if !modulated && curve.is_none() {
                continue;
            }
            let Some(base) = params.get(descriptor.id) else {
                continue;
            };
            let knob_normalized = descriptor.to_normalized(base);
            let Some(row) = self.curve_scratch.begin(descriptor.id) else {
                // Every shipped effect kind's whole table fits
                // `MAX_EFFECT_CURVE_DESTINATIONS` with room to spare
                // (`mooloop_dsp`'s own
                // `no_effect_kinds_descriptor_table_exceeds_the_curve_frames_capacity`);
                // reaching this arm means a future kind's table has grown
                // past it. Counted so it is a number in telemetry, not a
                // silently missing modulation route.
                self.curve_refusals += 1;
                continue;
            };
            for (tick, slot) in row.iter_mut().enumerate().take(ticks) {
                let base_normalized = curve
                    .as_ref()
                    .zip(automation)
                    .and_then(|(curve, automation)| automation.value_at(curve, tick))
                    .unwrap_or(knob_normalized);
                let offset_normalized = match (modulated, modulation) {
                    (true, Some(modulation)) => {
                        modulation
                            .rack
                            .offset_for(destination, modulation.sources(tick), &policy)
                    }
                    _ => 0.0,
                };
                *slot = descriptor
                    .from_normalized((base_normalized + offset_normalized).clamp(0.0, 1.0));
            }
        }
        ticks
    }

    fn queue_buffer(&mut self, slot: usize, event: mooloop_core::BufferEvent) {
        if let Some(state) = self.slot_mut(slot) {
            state.events.queue(TimedEvent {
                offset: 0,
                event: Event::Buffer(event),
            });
        }
    }

    fn queue_buffer_scrub(&mut self, slot: usize, delta_frames: f32) {
        if let Some(state) = self.slot_mut(slot) {
            state.events.queue(TimedEvent {
                offset: 0,
                event: Event::BufferScrub { delta_frames },
            });
        }
    }

    fn queue_buffer_release(&mut self, slot: usize) {
        if let Some(state) = self.slot_mut(slot) {
            state.events.queue(TimedEvent {
                offset: 0,
                event: Event::BufferRelease,
            });
        }
    }

    /// Load a project's saved chain. Construction allocates, so this is a
    /// load-time operation only, never a per-block one.
    fn load(
        &mut self,
        slots: &[mooloop_core::EffectSlotState],
        sample_rate: u32,
        bpm: f64,
        reclaim: &mut Reclaim,
    ) {
        self.clear(reclaim);
        // A chain arriving without identities is a project that skipped
        // `Project::assign_device_ids`, and the symptom would be every route
        // and lane on it quietly resolving to nothing. Caught here, on the
        // control thread, rather than found by ear.
        debug_assert!(
            slots.iter().all(|effect| effect.id.is_assigned()),
            "a chain reached the engine with unassigned device identities; \
             `Project::assign_device_ids` did not run over it"
        );
        for (slot, effect) in slots.iter().take(MAX_EFFECTS_PER_CHANNEL).enumerate() {
            let node = build_effect_at_tempo(effect.params, sample_rate, bpm);
            let align = IntegerDelay::new(node.dry_path_latency_frames()).map(Box::new);
            let displaced = self.install(
                slot,
                effect.kind(),
                effect_resource_key(effect.params),
                node,
                align,
                Box::new(SpectrumAnalyzer::new()),
                Box::new(EffectSlot::for_device(effect.id)),
            );
            if !displaced.is_empty() {
                reclaim.push(displaced);
            }
            // A container's span and the ring that delays its dry copy.
            // Allocated here rather than sent, because `load` already runs on
            // the control thread inside `install_project`.
            let children = effect.params.container_children().unwrap_or(0);
            let align = (children > 0)
                .then(|| IntegerDelay::new(mooloop_core::run_latency(slots, slot)))
                .flatten()
                .map(Box::new);
            // `install` seeds the kind's defaults; a saved chain overrides
            // them with what was persisted.
            if let Some(state) = self.slot_mut(slot) {
                state.base_params = Some(effect.params);
                state.bypassed = effect.bypassed;
                state.wet_dry = effect.wet_dry.clamp(0.0, 1.0);
                state.input_trim = effect.input_trim.clamp(0.0, MAX_LINEAR_GAIN);
                state.output_trim = effect.output_trim.clamp(0.0, MAX_LINEAR_GAIN);
                state.container_children = children;
                state.container_align = align;
                // A document arriving starts at its own controls.
                state.settle_ramps();
            }
        }
        // Each layer's shorter branches, held back to meet its longest.
        for layer in 0..slots.len().min(MAX_EFFECTS_PER_CHANNEL) {
            if slots[layer].params.container_flow() != Some(mooloop_core::ContainerFlow::Parallel) {
                continue;
            }
            for branch in mooloop_core::layer_branches(slots, layer) {
                let align = IntegerDelay::new(mooloop_core::branch_alignment(slots, layer, branch))
                    .map(Box::new);
                if let Some(state) = self.slot_mut(branch) {
                    state.branch_align = align;
                }
            }
        }
        // One allocation for the whole chain, and only for a chain that
        // actually holds a box -- with branch buffers only if one is a layer.
        let wanted = ContainerScratch::for_chain(slots);
        let upgrade = match (&self.container_dry, &wanted) {
            (None, Some(_)) => true,
            (Some(have), Some(want)) => want.holds_layers() && !have.holds_layers(),
            _ => false,
        };
        if upgrade {
            self.container_dry = wanted.map(Box::new);
        }
    }

    /// Record how loud this slot's input was, and return how many
    /// consecutive frames of silence it has now seen.
    ///
    /// Resets to zero the moment anything audible arrives, which is what
    /// makes waking up a property of the audio rather than of a timer.
    fn note_input_level(&mut self, slot: usize, peak: f32, frames: usize) -> u32 {
        let Some(state) = self.slot_mut(slot) else {
            return 0;
        };
        state.silent_frames = if peak <= SILENCE_PEAK {
            state.silent_frames.saturating_add(frames as u32)
        } else {
            0
        };
        state.silent_frames
    }

    /// Whether this slot's device can be left uncalled this block.
    ///
    /// Three conditions, and each one is load-bearing:
    ///
    /// - **Its input is silent.** `silent` is zero on any block that carried
    ///   audio, so this is the clause that makes waking instant.
    /// - **Its dry-path aligner has emptied.** The host stops feeding a
    ///   sleeping slot's ring, so a ring still holding pre-silence audio would
    ///   emit it on the wet/dry blend when the slot wakes. Only the
    ///   oversampled drive has one at all, and its tail is sixty times longer,
    ///   but this is stated rather than assumed.
    /// - **Its state has settled, or its tail has run out.** The two are
    ///   different answers to the same question and a device may give either:
    ///   a filter reads its own state exactly, a reverb declares how long its
    ///   decay takes. `>` rather than `>=` so a device that never converted
    ///   -- `u32::MAX`, which `silent` saturates at -- is never skipped.
    fn may_sleep(&self, slot: usize, silent: u32) -> bool {
        let Some(node) = self.nodes[slot].as_ref() else {
            return false;
        };
        silent > 0
            && self.slot(slot).is_none_or(EffectSlot::ramps_settled)
            && silent >= node.dry_path_latency_frames()
            && (node.is_at_rest() || silent > node.tail_frames())
    }

    /// Whether every occupied slot in this chain would be skipped this
    /// block: the condition a whole channel strip has to satisfy before it
    /// can be left uncalled.
    ///
    /// A bypassed slot runs no device, so the only thing it can still be
    /// holding is its dry-path aligner, and that is empty once the slot has
    /// seen silence for as long as the ring is. An empty slot is trivially
    /// at rest.
    fn is_at_rest(&self) -> bool {
        (0..self.bound).all(|slot| {
            let Some(node) = self.nodes[slot].as_ref() else {
                return true;
            };
            let silent = self.slot(slot).map_or(0, |state| state.silent_frames);
            if silent == 0 {
                return false;
            }
            if silent < node.dry_path_latency_frames() {
                return false;
            }
            // A host ramp still travelling keeps the chain awake, so a
            // sleeping strip and a running one leave it in the same place.
            if !self.slot(slot).is_none_or(EffectSlot::ramps_settled) {
                return false;
            }
            self.bypassed(slot) || node.is_at_rest() || silent > node.tail_frames()
        })
    }

    /// Sleep the whole chain for one block, because the strip around it is
    /// asleep and nothing will be handed to it.
    ///
    /// The bookkeeping `process` would have done, minus the audio: every
    /// occupied slot's silence counter grows so the chain stays at rest, and
    /// every device gets the chance to move whatever it runs on the clock. A
    /// chain is only ever slept while `is_at_rest` holds, so this cannot be
    /// reached with anything still decaying in it.
    /// Tell every device in the chain that time stopped being continuous.
    ///
    /// **Bypassed and sleeping slots included**, which is the opposite of
    /// what `sleep` does and is deliberate: a slot that is not being called
    /// is exactly the one whose ring is still holding audio from where the
    /// transport used to be, and it would emit it on waking. There is no
    /// running-versus-sleeping disagreement to create here, because this
    /// moves nothing on the clock -- it only invalidates what is stored.
    fn on_discontinuity(&mut self, kind: Discontinuity) {
        for slot in 0..self.bound {
            if let Some(node) = self.nodes[slot].as_mut() {
                node.on_discontinuity(kind);
            }
        }
    }

    fn sleep(&mut self, context: &ProcessContext) {
        for slot in 0..self.bound {
            // A bypassed slot's device is not called at all while the strip
            // is running, so bypass has already frozen whatever it runs on
            // the clock. Moving it here would make a sleeping strip and a
            // running one disagree, which is the one thing none of this may
            // do. Its counter still grows: what a bypassed slot holds is its
            // aligner, and that is empty once the silence outlasts the ring.
            let bypassed = self.bypassed(slot);
            let Some(node) = self.nodes[slot].as_mut() else {
                continue;
            };
            if !bypassed {
                node.skip_block(context);
            }
            if let Some(state) = self.slots[slot].as_deref_mut() {
                state.silent_frames = state.silent_frames.saturating_add(context.frames as u32);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn process(
        &mut self,
        context: &ProcessContext,
        bus: &mut StereoBus,
        scope: EffectTarget,
        device_display: Option<(&DeviceMeters, &DeviceTelemetry, usize)>,
        modulation: Option<&ModulationBlock<'_>>,
        automation: Option<&AutomationBlock<'_>>,
        skip_idle: bool,
    ) {
        // The containers this chain is currently inside, innermost last.
        // Fixed at the depth cap and never grown, so nothing here allocates.
        let mut open = [OpenRun {
            end: 0,
            slot: 0,
            parallel: false,
            branch: 0,
            branch_end: 0,
        }; MAX_CONTAINER_DEPTH];
        let mut depth = 0usize;
        // Set past the end of a bypassed container's run: a bypassed box
        // skips its whole run rather than its own row.
        let mut skip_until = 0usize;

        for slot in 0..self.bound {
            // Close every run that ends here, innermost first. A loop
            // rather than an `if` because several boxes can end on the same
            // row, and they nest, so the innermost is always on top.
            //
            // A layer's *branch* can end here too, and only once every run
            // inside that branch has closed -- a branch is a whole run -- so
            // it is asked of whichever run is on top after the closing.
            while depth > 0 {
                let run = open[depth - 1];
                if run.end == slot {
                    depth -= 1;
                    self.close_run(run, depth, bus, context, device_display);
                    continue;
                }
                if run.parallel && run.branch_end == slot {
                    self.finish_branch(run, depth - 1, bus, context);
                    self.start_branch(&mut open[depth - 1], slot, depth - 1, bus, context);
                }
                break;
            }
            if slot < skip_until {
                continue;
            }
            // Aim the row's host ramps at its controls. A slot coming back
            // from wholly out of the path starts its device clean first
            // (MOO-108): it was not called while it was out.
            let returning = self.slot(slot).is_some_and(EffectSlot::returning);
            if let Some(state) = self.slots[slot].as_deref_mut() {
                state.aim_ramps(context.sample_rate);
            }
            if returning {
                self.restart_returning(slot);
            }
            if let Some((_, telemetry, target)) = device_display {
                // A device that publishes its own display spectrum owns the
                // stage; feeding the generic analyzer as well would burn a
                // Goertzel bank writing cells the device is about to
                // overwrite.
                let host_analyzes = self.nodes[slot]
                    .as_ref()
                    .is_some_and(|node| !node.provides_display_spectrum());
                if host_analyzes && telemetry.spectrum_enabled(target, slot + 1) {
                    if let Some(analyzer) = &mut self.analyzers[slot] {
                        if let Some(levels) =
                            analyzer.push(context.sample_rate, bus, context.frames)
                        {
                            telemetry.publish_spectrum(target, slot + 1, &levels);
                        }
                    }
                }
            }
            if self.is_container(slot) {
                // **A container never processes audio.** Its children are
                // rows of this same chain and the loop is about to run them;
                // all it does here is keep a copy of what is going in, so the
                // crossfade at the end of its run has something to blend.
                //
                // It also does not take the *slot's* leaf wet/dry, which
                // would be meaningless (there is no node to be wet with) and,
                // through the equal-power crossfade, not even bit-exact -- a
                // transparent node blended at unity still leaks a cos(pi/2)
                // of the dry.
                let (left, right) = bus.peak(context.frames);
                self.note_input_level(slot, left.max(right), context.frames);
                if let Some((meters, _, target)) = device_display {
                    meters.publish_input(target, slot + 1, left, right);
                }
                if let Some(state) = self.slots[slot].as_mut() {
                    state.events.clear();
                    state.settle_leaf_ramps();
                }
                let children = self.container_children(slot);
                if children == 0 {
                    if let Some(state) = self.slots[slot].as_mut() {
                        state.settle_ramps();
                    }
                    // An empty box is a row that does nothing, so what comes
                    // out of it is what went in.
                    if let Some((meters, _, target)) = device_display {
                        meters.publish_output(target, slot + 1, left, right);
                    }
                    continue;
                }
                let end = (slot + 1 + children).min(self.bound);
                let mut open_run = OpenRun {
                    end,
                    slot,
                    parallel: false,
                    branch: slot + 1,
                    branch_end: end,
                };
                if self.bypassed(slot) {
                    // **Bypassing a box bypasses the run**, and costs exactly
                    // the frames that run declares. Same argument as a
                    // bypassed device: a bypass that shortened the path would
                    // move the channel in time against every other one, so
                    // A/B-ing the box would also A/B the timing.
                    if let Some(state) = self.slots[slot].as_mut() {
                        if let Some(align) = &mut state.container_align {
                            align.process(
                                &mut bus.l[..context.frames],
                                &mut bus.r[..context.frames],
                            );
                        }
                    }
                    // Past the delay its run declares, which is what the
                    // box costs while it is out.
                    if let Some((meters, _, target)) = device_display {
                        let (left, right) = bus.peak(context.frames);
                        meters.publish_output(target, slot + 1, left, right);
                    }
                    skip_until = open_run.end;
                    continue;
                }
                // Checked *below* the bypass branch, so a box nested past the
                // cap still bypasses its run: being too deep to blend is no
                // reason for its bypass button to do nothing too. Its Mix
                // still does nothing, which is the part that needs a decision
                // -- see `docs/LOOSE_ENDS.md`.
                if depth >= MAX_CONTAINER_DEPTH {
                    // Inert: no blend to ramp its Mix or its bypass along.
                    if let Some(state) = self.slots[slot].as_mut() {
                        state.settle_ramps();
                    }
                    // No OUT published. This branch cannot open a run --
                    // `scratch.dry` is `MAX_CONTAINER_DEPTH` long, which is
                    // what the cap is for -- so `close_run` never publishes
                    // the box's real output, and the peak in hand here is the
                    // one taken going *in*. Publishing that was the same
                    // defect `close_run`'s comment describes, introduced at
                    // this second site while the bypass ordering was being
                    // fixed. A box past the cap is inert and draws no chrome
                    // anyway; `docs/LOOSE_ENDS.md` carries the whole of it.
                    continue;
                }
                // **A layer splits here.** Its input is kept for every
                // branch to start from and its sum starts empty; the first
                // branch runs on the bus as it stands. A scratch without
                // branch buffers cannot happen once the span has arrived --
                // the same command carries the upgrade -- and would run the
                // layer in series rather than fail.
                let layered = self
                    .slot(slot)
                    .and_then(|state| state.kind)
                    .and_then(mooloop_core::EffectKind::container_flow)
                    == Some(mooloop_core::ContainerFlow::Parallel);
                let first_branch_end = self.run_end(slot + 1).min(end);
                if let Some(scratch) = self.container_dry.as_mut() {
                    scratch.dry[depth].l[..context.frames]
                        .copy_from_slice(&bus.l[..context.frames]);
                    scratch.dry[depth].r[..context.frames]
                        .copy_from_slice(&bus.r[..context.frames]);
                    if let (true, Some(branches)) = (layered, scratch.branches.as_mut()) {
                        branches.input[depth].l[..context.frames]
                            .copy_from_slice(&bus.l[..context.frames]);
                        branches.input[depth].r[..context.frames]
                            .copy_from_slice(&bus.r[..context.frames]);
                        branches.sum[depth].l[..context.frames].fill(0.0);
                        branches.sum[depth].r[..context.frames].fill(0.0);
                        open_run.parallel = true;
                        open_run.branch_end = first_branch_end;
                    }
                    open[depth] = open_run;
                    depth += 1;
                }
                continue;
            }
            if self.bypassed(slot) {
                if let Some(state) = self.slots[slot].as_mut() {
                    state.settle_leaf_ramps();
                }
                // A bypassed slot keeps its queued events until re-enabled, so
                // knob turns made while bypassed are not lost.
                if let Some(align) = &mut self.dry_align[slot] {
                    // **Bypass is time transparent, not latency free.** The
                    // signal goes through the slot's own ring, so a bypassed
                    // node still costs exactly the frames it declares.
                    //
                    // Two things depend on this. The latency plan sums a
                    // chain's *declared* latency including bypassed slots
                    // (`mooloop_core::chain_latency`, and every host works this
                    // way), so a bypass that shortened the path would leave
                    // every other channel over-compensated against it. And
                    // toggling one would move the channel in time, so A/B-ing
                    // an effect would also A/B the timing and neither answer
                    // would be about the effect.
                    //
                    // This also subsumes what the branch did before, which was
                    // to push a shadow copy through the ring so re-enabling
                    // never blended in pre-bypass audio: the ring sees the same
                    // samples either way.
                    align.process(
                        &mut bus.l[..context.frames],
                        &mut bus.r[..context.frames],
                    );
                }
                // Measured *after* the aligner on purpose. A bypassed slot
                // is its ring and nothing else, so the level coming out of
                // the ring is exactly the level the slot still has to offer,
                // and counting that is what lets `is_at_rest` above know the
                // ring has emptied without looking inside it.
                let (left, right) = bus.peak(context.frames);
                self.note_input_level(slot, left.max(right), context.frames);
                if let Some((meters, _, target)) = device_display {
                    meters.publish_input(target, slot + 1, left, right);
                    meters.publish_output(target, slot + 1, left, right);
                }
                continue;
            }
            if self.nodes[slot].is_some() {
                // Taken once, before the trim, and used twice: for the
                // decision below and for this slot's input meter. Peak is
                // linear in a non-negative gain, so scaling it by the trim is
                // exactly what a second scan after the trim would have found.
                // Silence detection therefore costs nothing the meters were
                // not already paying, and the chain does one scan a slot
                // rather than two.
                let (mut peak_l, mut peak_r) = bus.peak(context.frames);
                // **A non-finite input stops here** (MOO-176). `peak` reads a
                // NaN as an infinite peak, so the fold the meters and the
                // silence count already pay for is the check: a finite block
                // is not touched. One that is not has its NaNs and
                // infinities silenced before the device sees them -- a
                // feedback network or a detector would keep one for good --
                // and is counted where the interface can read it.
                if !(peak_l.is_finite() && peak_r.is_finite()) {
                    mooloop_dsp::effects::scrub_non_finite(bus, context.frames);
                    self.faults_unpublished = self.faults_unpublished.saturating_add(1);
                    (peak_l, peak_r) = bus.peak(context.frames);
                }
                let silent = self.note_input_level(slot, peak_l.max(peak_r), context.frames);
                if skip_idle && self.may_sleep(slot, silent) {
                    // The one thing a sleeping node is still owed: whatever
                    // it runs on the clock rather than on its input. A
                    // reverb's line modulation and a chorus's LFO have to
                    // arrive where the transport says, or the device would
                    // sound different after a gap and *how* different would
                    // depend on the host's buffer size.
                    if let Some(node) = self.nodes[slot].as_mut() {
                        node.skip_block(context);
                    }
                    // Nothing is published and nothing is cleared. The device
                    // meters are peak-hold cells the GUI empties as it reads
                    // them, so not writing one *is* publishing silence; and
                    // the queued parameter events stay queued exactly as they
                    // do under bypass, so a knob turned while a channel was
                    // quiet still lands when it wakes.
                    //
                    // Nothing else here touches the node, which is what makes
                    // waking instant: the first block with audio in it
                    // processes normally, from the state the node had when it
                    // stopped, with no ramp and no reset.
                    continue;
                }
                // The dry copy is taken *before* the input trim: it is also
                // the bypassed path, which the trim is not on, and the blend
                // below puts the trim back on it frame by frame. With the
                // trim still, that is the same product in the same order.
                self.dry.l[..context.frames].copy_from_slice(&bus.l[..context.frames]);
                self.dry.r[..context.frames].copy_from_slice(&bus.r[..context.frames]);
                if let Some(align) = &mut self.dry_align[slot] {
                    align.process(
                        &mut self.dry.l[..context.frames],
                        &mut self.dry.r[..context.frames],
                    );
                }
                let mut ramps = self.slot(slot).map_or_else(HostRamps::new, |state| state.ramps);
                // Walked twice, identically: once onto the device's input
                // here, once onto the dry copy in the blend.
                let mut dry_input = ramps.input;
                for frame in 0..context.frames {
                    let input_trim = ramps.input.advance();
                    bus.l[frame] *= input_trim;
                    bus.r[frame] *= input_trim;
                }
                let input_trim = ramps.input.value();
                if let Some((meters, _, target)) = device_display {
                    meters.publish_input(target, slot + 1, peak_l * input_trim, peak_r * input_trim);
                }
                self.event_scratch.clear();
                if let Some(state) = self.slots[slot].as_mut() {
                    self.refused_events += state.events.copy_to(&mut self.event_scratch);
                }
                self.control_events_for_slot(slot, scope, modulation, automation);
                // `control_events_for_slot` mutates the shared scratch
                // pools, so take the node borrow only after that work.
                let node = self.nodes[slot].as_mut().expect("checked above");
                // The common case -- nothing on this slot is modulated or
                // automated -- skips building the on-stack curve array
                // entirely, the same "one question in place of a hundred"
                // shortcut `control_events_for_slot` itself takes.
                if self.curve_scratch.count > 0 {
                    // A small on-stack array, not a `Vec`: `apply_curves`
                    // takes a slice, and this is the realtime thread.
                    let mut curve_buf: [ControlCurve<'_>; MAX_EFFECT_CURVE_DESTINATIONS] =
                        std::array::from_fn(|_| ControlCurve::default());
                    let curve_count = self.curve_scratch.fill(&mut curve_buf);
                    self.refused_events += node.apply_curves(
                        &curve_buf[..curve_count],
                        CONTROL_RATE_FRAMES,
                        &mut self.event_scratch,
                    );
                }
                node.process(context, bus, &self.event_scratch, None);
                // Equal-power crossfade. The wet paths people actually blend
                // (reverb, chorus, delay) are decorrelated from dry, where a
                // linear fade dips ~3 dB at the midpoint; correlated paths
                // (filters, EQ) now sum slightly hot at 50%. Trade-off noted
                // in docs/GAIN_STRUCTURE.md.
                //
                // Every control in it ramps per sample (MOO-108), and the
                // whole of it crossfades against the bypassed path -- the
                // untrimmed dry copy -- while bypass takes the device out or
                // brings it back. A still slot takes the branches that do
                // exactly what the flat version did.
                let wet_moving = !ramps.wet.is_settled();
                let (mut dry_gain, mut wet_gain) = equal_power(ramps.wet.value());
                for frame in 0..context.frames {
                    let input_trim = dry_input.advance();
                    if wet_moving {
                        (dry_gain, wet_gain) = equal_power(ramps.wet.advance());
                    }
                    let trim = ramps.output.advance();
                    let active = ramps.active.advance();
                    let (dry_l, dry_r) = (self.dry.l[frame], self.dry.r[frame]);
                    let left = (dry_l * input_trim * dry_gain + bus.l[frame] * wet_gain) * trim;
                    let right = (dry_r * input_trim * dry_gain + bus.r[frame] * wet_gain) * trim;
                    if active == 1.0 {
                        bus.l[frame] = left;
                        bus.r[frame] = right;
                    } else {
                        bus.l[frame] = dry_l + (left - dry_l) * active;
                        bus.r[frame] = dry_r + (right - dry_r) * active;
                    }
                }
                if let Some(state) = self.slots[slot].as_deref_mut() {
                    state.ramps = ramps;
                    state.finish_bypass_fade();
                }
                if let Some((meters, telemetry, target)) = device_display {
                    let (left, right) = bus.peak(context.frames);
                    meters.publish_output(target, slot + 1, left, right);
                    // Retained-audio forced returns are otherwise invisible:
                    // the device recovers silently and the only trace is this
                    // counter. Publishing it here keeps the audio thread free
                    // of logging.
                    telemetry.publish_buffer_collisions(target, slot + 1, node.buffer_collisions());
                    // Only the buffer device answers; everything else is one
                    // `None` and the cells stay where they were.
                    if let Some(display) = node.buffer_waveform() {
                        telemetry.publish_buffer_display(target, slot + 1, &display);
                    }
                    // Only the dynamics devices answer this; for everything
                    // else it is one `None` and the cells stay at rest.
                    if let Some(frame) = node.dynamics_frame() {
                        meters.publish_dynamics(target, slot + 1, frame);
                    }
                    // And the same shape again for a device that draws its
                    // own spectrum rather than its input's. `None` on every
                    // block but the one a hop comes due on.
                    if let Some(levels) = node.take_display_spectrum() {
                        telemetry.publish_spectrum(target, slot + 1, &levels);
                    }
                }
            }
            if self.nodes[slot].is_none() {
                if let Some(state) = self.slots[slot].as_mut() {
                    state.settle_ramps();
                }
            }
            if let Some(state) = self.slots[slot].as_mut() {
                state.events.clear();
            }
        }
        // A run whose span reaches the end of the populated chain closes
        // here. `bound` clamps `end` on the way in, so this is the same
        // arithmetic rather than a second rule.
        while depth > 0 {
            depth -= 1;
            let run = open[depth];
            self.close_run(run, depth, bus, context, device_display);
        }
    }

    /// Close a container's run: blend it back against its dry copy, then
    /// publish what came out as the box's OUT.
    fn close_run(
        &mut self,
        run: OpenRun,
        depth: usize,
        bus: &mut StereoBus,
        context: &ProcessContext,
        device_display: Option<(&DeviceMeters, &DeviceTelemetry, usize)>,
    ) {
        if run.parallel {
            // The last branch joins the others, and what the layer carries
            // on with is their sum.
            self.finish_branch(run, depth, bus, context);
            if let Some(branches) = self
                .container_dry
                .as_ref()
                .and_then(|scratch| scratch.branches.as_ref())
            {
                bus.l[..context.frames].copy_from_slice(&branches.sum[depth].l[..context.frames]);
                bus.r[..context.frames].copy_from_slice(&branches.sum[depth].r[..context.frames]);
            }
        }
        self.blend_run(run, depth, bus, context);
        // The box's OUT is what leaves the far end of its run, taken after
        // the blend. It used to be published at the container's own row from
        // the peak going *in*, which is the one number it certainly is not:
        // `DeviceOutputRail` is drawn past everything the box holds
        // precisely so it reads as the box's output. The meter cells are
        // `fetch_max` peak holds, so publishing the input there did not
        // merely report the wrong figure -- it survived a correct later
        // write for any run that attenuates.
        if let Some((meters, _, target)) = device_display {
            let (left, right) = bus.peak(context.frames);
            meters.publish_output(target, run.slot + 1, left, right);
        }
    }

    /// One past the last row of the run headed by `slot`: itself, and what
    /// it holds when it is a container.
    fn run_end(&self, slot: usize) -> usize {
        slot + 1 + self.container_children(slot)
    }

    /// A layer's branch is done: hold it back to meet the longest branch,
    /// then add it to the sum.
    ///
    /// **At unity.** Two branches carrying the same signal come out 6 dB
    /// hotter than one, and that is what a layer is; any other gain would be
    /// a decision about level, not a default an engine step gets to choose.
    ///
    /// A branch that has gone quiet still adds -- nothing here skips one --
    /// and adding silence is the same number as not adding it, so the idle
    /// skip inside a branch cannot change the sum.
    fn finish_branch(
        &mut self,
        run: OpenRun,
        depth: usize,
        bus: &mut StereoBus,
        context: &ProcessContext,
    ) {
        let Self {
            slots,
            container_dry,
            ..
        } = self;
        if let Some(align) = slots[run.branch]
            .as_mut()
            .and_then(|state| state.branch_align.as_mut())
        {
            align.process(&mut bus.l[..context.frames], &mut bus.r[..context.frames]);
        }
        let Some(branches) = container_dry
            .as_mut()
            .and_then(|scratch| scratch.branches.as_mut())
        else {
            return;
        };
        let sum = &mut branches.sum[depth];
        for frame in 0..context.frames {
            sum.l[frame] += bus.l[frame];
            sum.r[frame] += bus.r[frame];
        }
    }

    /// Start the layer's next branch at `slot`, from the layer's input.
    fn start_branch(
        &mut self,
        run: &mut OpenRun,
        slot: usize,
        depth: usize,
        bus: &mut StereoBus,
        context: &ProcessContext,
    ) {
        run.branch = slot;
        run.branch_end = self.run_end(slot).min(run.end);
        if let Some(branches) = self
            .container_dry
            .as_ref()
            .and_then(|scratch| scratch.branches.as_ref())
        {
            bus.l[..context.frames].copy_from_slice(&branches.input[depth].l[..context.frames]);
            bus.r[..context.frames].copy_from_slice(&branches.input[depth].r[..context.frames]);
        }
    }

    /// Crossfade a container's run back against the dry copy taken when it
    /// opened, delayed by the run's declared latency.
    ///
    /// The per-slot dry path generalised from one device to a span, and
    /// deliberately the same crossfade: equal-power, because the runs people
    /// will actually blend are the decorrelated ones a linear fade dips 3 dB
    /// in the middle of. `docs/GAIN_STRUCTURE.md` records the trade.
    ///
    /// The blend alone -- [`Self::close_run`] publishes what comes out,
    /// which has to happen on every path including the two this one returns
    /// early on.
    fn blend_run(
        &mut self,
        run: OpenRun,
        depth: usize,
        bus: &mut StereoBus,
        context: &ProcessContext,
    ) {
        // Two disjoint fields at once: the ring lives on the container's slot
        // and the copy it delays lives on the chain.
        let Self {
            slots,
            container_dry,
            ..
        } = self;
        let Some(scratch) = container_dry.as_mut() else {
            return;
        };
        // Aligned before the blend, not after: the wet path came out of the
        // run's devices `run_latency` frames late, so the copy has to wait
        // the same amount or the two comb. Run unconditionally, including at
        // full wet, because the ring has to keep advancing or the mix would
        // read stale audio the first time it moved off unity.
        if let Some(state) = slots[run.slot].as_mut() {
            if let Some(align) = &mut state.container_align {
                align.process(
                    &mut scratch.dry[depth].l[..context.frames],
                    &mut scratch.dry[depth].r[..context.frames],
                );
            }
        }
        // Nothing to add at full wet, and skipping it is correctness rather
        // than an optimisation: the equal-power crossfade leaves a
        // `cos(pi/2)` of the dry -- about 6e-8 -- so *running* it would leak
        // a fraction of the dry into a run the user asked to hear whole, and
        // step 02's bit-exact null is what would break.
        //
        // The Mix and the box's bypass both ramp (MOO-108): bypassing a box
        // is its Mix fading to dry while its run keeps playing, and only
        // once that has arrived does the run stop being called.
        let Some(state) = slots[run.slot].as_deref_mut() else {
            return;
        };
        let mut ramps = state.ramps;
        let still = ramps.mix.is_settled() && ramps.active.is_settled();
        let mix = ramps.mix.value() * ramps.active.value();
        if still && mix >= 1.0 {
            return;
        }
        let (mut dry_gain, mut wet_gain) = equal_power(mix);
        for frame in 0..context.frames {
            if !still {
                (dry_gain, wet_gain) = equal_power(ramps.mix.advance() * ramps.active.advance());
            }
            bus.l[frame] = scratch.dry[depth].l[frame] * dry_gain + bus.l[frame] * wet_gain;
            bus.r[frame] = scratch.dry[depth].r[frame] * dry_gain + bus.r[frame] * wet_gain;
        }
        state.ramps = ramps;
        state.finish_bypass_fade();
    }
}

/// The strip's resolved `(gain, pan)` for each control subdivision of one
/// block. Fixed-size and `Copy`: it is built on the audio thread.
#[derive(Debug, Clone, Copy)]
struct StripSegments {
    values: [(f32, f32); MAX_CONTROL_TICKS_PER_BLOCK],
    count: usize,
}

/// Resolve the strip's fader and pan for this block.
///
/// The strip is an ordinary destination -- `ParamOwner::Strip` with the
/// descriptor ids in `STRIP_DESCRIPTORS` -- but unlike a device it keeps no
/// parameter state between blocks: the output stage multiplies the bus by its
/// knob value from scratch every time. So there is no base/resolved split to
/// maintain here and nothing to restore when a route is removed; the knob is
/// already the base, and a control signal simply resolves into per-subdivision
/// gain segments on top of it.
///
/// Returns `None` when nothing drives either parameter, so the overwhelmingly
/// common still-fader case stays one pass over the block.
fn resolve_strip_segments(
    base_gain: f32,
    base_pan: f32,
    scope: EffectTarget,
    modulation: &ModulationBlock<'_>,
    automation: Option<&AutomationBlock<'_>>,
) -> Option<StripSegments> {
    let ticks = modulation
        .ticks
        .max(automation.map_or(0, |automation| automation.ticks))
        .min(MAX_CONTROL_TICKS_PER_BLOCK);
    if ticks == 0 {
        return None;
    }
    // Asked before the table exists rather than after. `StripSegments` is two
    // kilobytes, every byte of it written by the initializer below, and a
    // channel with an empty rack and no lane throws all of it away -- which
    // is every channel on a song nobody has automated or routed.
    if !modulation.rack.has_routes() && automation.is_none() {
        return None;
    }
    let mut segments = StripSegments {
        values: [(base_gain, base_pan); MAX_CONTROL_TICKS_PER_BLOCK],
        count: ticks,
    };
    let mut driven = false;
    for descriptor in STRIP_DESCRIPTORS.iter() {
        let destination = ParamAddr::strip(scope, descriptor.id);
        let policy = ModDestinationDescriptor::for_param(descriptor);
        let modulated = modulation.rack.modulates(destination, &policy);
        let curve = automation.and_then(|automation| automation.curve_for(destination));
        if !modulated && curve.is_none() {
            continue;
        }
        driven = true;
        let knob = if descriptor.id == STRIP_PARAM_VOLUME {
            base_gain
        } else {
            base_pan
        };
        let knob_normalized = descriptor.to_normalized(knob);
        for tick in 0..ticks {
            let base_normalized = curve
                .as_ref()
                .zip(automation)
                .and_then(|(curve, automation)| automation.value_at(curve, tick))
                .unwrap_or(knob_normalized);
            let offset_normalized = if modulated {
                modulation
                    .rack
                    .offset_for(destination, modulation.sources(tick), &policy)
            } else {
                0.0
            };
            let value =
                descriptor.from_normalized((base_normalized + offset_normalized).clamp(0.0, 1.0));
            if descriptor.id == STRIP_PARAM_VOLUME {
                segments.values[tick].0 = value;
            } else {
                segments.values[tick].1 = value;
            }
        }
    }
    driven.then_some(segments)
}

/// How a stage turns its pan knob into a gain per side: a channel's
/// constant-power [`pan_gains`], or a track's level-neutral [`balance_gains`].
type PanLaw = fn(f32) -> (f32, f32);

/// Shared output stage: linear gain, a source-pan or bus-balance application,
/// and a mute that stops the strip contributing without stopping it processing
/// (so effect tails on a muted strip still decay instead of freezing).
///
/// **Every change it makes is a ramp** (MOO-107). What reaches the bus is a
/// gain per side, `left` and `right`, each a [`Smoothed`] lagging the knob
/// over [`STRIP_GAIN_SMOOTH_S`]: the fader and the pan through the stage's
/// law while the strip is heard, and silence while it is muted or
/// solo-silenced. So a fader ride, a pan move, a mute and an unmute all move
/// the output continuously, and so does a lane or a modulator driving the
/// fader, whose control-rate staircase the lag rounds off.
///
/// Smoothing the two sides rather than the gain and the pan separately is
/// what keeps the ordinary case cheap: a pan swept through the law per sample
/// would be a sine and a cosine a frame, where this is two multiplies. The
/// price is that the few milliseconds of a pan move travel a straight line
/// between the two ends of the law rather than along its curve, which is a
/// dip in level nobody can hear in the time it takes.
///
/// A settled stage is one pass over the block with exactly the gains the
/// unsmoothed stage applied, so a still fader renders bit-identically to the
/// way it always did.
struct OutputStage {
    /// The fader, as the knob holds it. Resolved through `law` with the pan
    /// into what `left` and `right` aim at.
    gain: f32,
    pan: f32,
    muted: bool,
    law: PanLaw,
    /// The gain actually applied to each side, lagging what [`Self::aim`]
    /// last asked for.
    left: Smoothed,
    right: Smoothed,
}

impl OutputStage {
    fn new(gain: f32, law: PanLaw, sample_rate: u32) -> Self {
        let (left, right) = law(0.0);
        Self {
            gain,
            pan: 0.0,
            muted: false,
            law,
            left: Smoothed::new(gain * left, STRIP_GAIN_SMOOTH_S, sample_rate),
            right: Smoothed::new(gain * right, STRIP_GAIN_SMOOTH_S, sample_rate),
        }
    }

    /// Put the stage back to a fresh one at `gain`, keeping its law and its
    /// smoothing rate. Snaps rather than ramps: a reset is a strip being
    /// reused for something else, and there is nothing to be continuous with.
    fn reset(&mut self, gain: f32) {
        self.gain = gain;
        self.pan = 0.0;
        self.muted = false;
        self.aim(false);
        self.settle();
    }

    fn set_volume(&mut self, volume: f32) {
        // Channels and buses gain up to +12 dB, same headroom as the effect
        // container's trims.
        self.gain = volume.clamp(0.0, MAX_LINEAR_GAIN);
    }

    fn set_pan(&mut self, pan: f32) {
        self.pan = pan.clamp(-1.0, 1.0);
    }

    /// The gain per side a fader at `gain` and a pan at `pan` resolve to.
    fn sides(&self, gain: f32, pan: f32) -> (f32, f32) {
        let (left, right) = (self.law)(pan);
        (gain * left, gain * right)
    }

    /// Aim both sides at what the knobs say, or at silence while `silenced`
    /// -- the strip's own mute or a solo somewhere else. The stage is told
    /// the verdict rather than holding it, because solo is a property of the
    /// whole graph.
    ///
    /// Called at the top of the strip's block, before anything asks whether
    /// the stage is settled or silent, so that those answers are about this
    /// block's knobs and not the last one's.
    fn aim(&mut self, silenced: bool) {
        let (left, right) = if silenced {
            (0.0, 0.0)
        } else {
            self.sides(self.gain, self.pan)
        };
        self.left.set_target(left);
        self.right.set_target(right);
    }

    /// Jump to wherever the stage is aimed. For a document arriving, where
    /// there is nothing sounding to be continuous with -- and where a ramp
    /// would put the first milliseconds of every bounce at the wrong level.
    fn settle(&mut self) {
        self.left.reset_to(self.left.target());
        self.right.reset_to(self.right.target());
    }

    /// Whether both sides have arrived where they were aimed, so a block
    /// rendered or skipped would leave the stage in the same place.
    fn is_settled(&self) -> bool {
        self.left.is_settled() && self.right.is_settled()
    }

    /// Whether the stage has faded all the way out: settled, and at silence.
    /// What a muted strip waits for before it stops contributing.
    fn is_silent(&self) -> bool {
        self.is_settled() && self.left.value() == 0.0 && self.right.value() == 0.0
    }

    /// Apply the stage to the block, per sample while either side is moving.
    fn apply(&mut self, bus: &mut StereoBus, frames: usize) {
        if self.is_settled() {
            bus.apply_stereo_gain(self.left.value(), self.right.value(), frames);
            return;
        }
        self.apply_range(bus, 0, frames);
    }

    /// Advance both sides one sample per frame over `start..end`.
    fn apply_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        for frame in start..end {
            bus.l[frame] *= self.left.advance();
            bus.r[frame] *= self.right.advance();
        }
    }

    /// Apply gain and pan, re-aimed per control subdivision when a source or
    /// a lane is driving them. `segments` is `None` for the ordinary case of a
    /// still fader, which stays a single pass over the block once settled.
    ///
    /// A driven fader used to *step* per subdivision, which is a zipper at
    /// the control rate. Each subdivision now moves the target and the lag
    /// walks to it a sample at a time. `silenced` wins over any lane: a
    /// muted strip fades out whatever is driving its fader.
    fn apply_segments(
        &mut self,
        bus: &mut StereoBus,
        frames: usize,
        segments: Option<&StripSegments>,
        silenced: bool,
    ) {
        let Some(segments) = segments.filter(|_| !silenced) else {
            self.apply(bus, frames);
            return;
        };
        let mut done = 0;
        for (tick, &(gain, pan)) in segments.values.iter().take(segments.count).enumerate() {
            let start = tick * CONTROL_RATE_FRAMES;
            if start >= frames {
                break;
            }
            let end = (start + CONTROL_RATE_FRAMES).min(frames);
            let (left, right) = self.sides(gain, pan);
            self.left.set_target(left);
            self.right.set_target(right);
            self.apply_range(bus, start, end);
            done = end;
        }
        // A block longer than its segments -- which the resolver does not
        // produce, but nothing here can see that -- keeps walking toward the
        // last target rather than leaving its tail unscaled.
        self.apply_range(bus, done, frames);
    }
}

/// One mixer bus: an effect chain and an output stage.
///
/// Where it feeds is **not** a field here -- the destination lives in
/// `CompiledBusGraph`, and any track may feed any other. `process_block`
/// walks `bus_graph.render_order()`, a compiled topological permutation that
/// places every producer before the summing point it reaches, with scratch
/// buffers when sends exist. This used to say `output` was always lower than
/// the bus's own index and that the bank was rendered in one descending
/// pass; steps 04 and 05 replaced that model, and `compensation`,
/// `console_sum` and `solo_silenced` below only exist because they did.
struct BusStrip {
    effects: EffectChain,
    bus: StereoBus,
    output: OutputStage,
    /// This track's channel strip: the input stage, the EQ and the
    /// compressor, all out until something switches them in.
    ///
    /// Not boxed and not optional. A few hundred bytes against the 128 KB a
    /// track already costs (`docs/plans/archive/console/00-status.md`,
    /// step 04's
    /// measurement), and what it must not spend is *time*, which is what the
    /// three `in` switches see to. Where it runs in the block is
    /// `mooloop_core::mixer::STRIP_PIN`.
    strip: Strip,
    /// Whether this track's signal is inverted, applied at the top of its
    /// block so everything downstream sees the flip.
    polarity: bool,
    /// The sign actually applied: `1` or `-1` once settled, and passing
    /// through zero for the few milliseconds after the switch, so a flip is
    /// a crossfade through silence rather than a step of twice the signal
    /// (MOO-107).
    sign: Smoothed,
    /// Whether something *else* is soloed and this track is not part of it.
    ///
    /// Derived on the control thread by `mooloop_core::mixer::solo_silenced`
    /// -- whether a track is heard under a solo is a property of the whole
    /// graph -- and held apart from `output.muted` so that dropping the solo
    /// gives a track back whatever its own mute said.
    solo_silenced: bool,
    /// How long this bus waits before summing into the bus it feeds. Same
    /// contract as a channel's, and always `None` on the master, which feeds
    /// nothing.
    compensation: Option<Box<IntegerDelay>>,
    /// Whether this bus's *output* is console-encoded. Nesting needs no
    /// special case: whatever this bus feeds decodes it exactly as it decodes
    /// a channel. Meaningless and ignored on the master, which feeds nothing.
    console: bool,
    /// The second input accumulator: the sum of everything feeding this bus
    /// that opted into console summing, held apart from the linear sum in
    /// `bus` so it can be decoded before the two are added.
    ///
    /// **This is the whole mechanism.** Decoding one sum and adding the other
    /// is Adam's *"the decode stage is mixed with master to pick up any
    /// channels that don't have it switched on"*, generalised from the master
    /// to every summing point -- which is what makes the decoding bus
    /// invisible: there is no device to place and no bus to create, because
    /// every bus already is one.
    ///
    /// `None` unless something console-encoded actually feeds this bus. The
    /// buffer is 64 KB and is allocated on the control thread by
    /// `Session::sync_console_sums`, exactly as a compensation ring is; a
    /// strip whose switch is on but whose buffer has not arrived sums
    /// linearly for the tick it takes to converge, which is the same
    /// either-order tolerance `SetSamplerStretch` documents.
    console_sum: Option<Box<StereoBus>>,
    /// Whether anything was written into `console_sum` this block. Read
    /// instead of scanning the buffer, for the reason `dirty` exists.
    console_dirty: bool,
    /// Whether `bus` may hold anything but zeros: set when something sums
    /// into it and when the strip runs, since a chain with a tail writes into
    /// a buffer nothing fed. Read at the top of the next block to decide
    /// whether it needs emptying at all.
    dirty: bool,
    /// Frames since anything last reached this bus. The chain's tail is
    /// measured against it, and so is the compensation ring: once silence has
    /// been fed for the ring's whole length every slot in it is a zero.
    silent_frames: u32,
    /// Whether the strip has already done the once-only work of going to
    /// sleep, so that emptying the buffer is not repeated every idle block.
    sleeping: bool,
}

impl BusStrip {
    fn new(sample_rate: u32) -> Self {
        Self {
            effects: EffectChain::new(),
            bus: StereoBus::with_capacity(MAX_BLOCK_SIZE),
            // Unity, as a channel is: see `mooloop_core::MixerBus::new`.
            output: OutputStage::new(1.0, balance_gains, sample_rate),
            strip: Strip::new(StripParams::default(), sample_rate),
            polarity: false,
            sign: Smoothed::new(1.0, STRIP_GAIN_SMOOTH_S, sample_rate),
            solo_silenced: false,
            compensation: None,
            console: false,
            console_sum: None,
            console_dirty: false,
            // Nothing has been written yet, but the first block empties it
            // anyway rather than reasoning about a buffer it did not fill.
            dirty: true,
            silent_frames: 0,
            sleeping: false,
        }
    }

    /// Whether this bus can be left unrendered for a block in which nothing
    /// reached it.
    ///
    /// Two conditions, and the second is the one that is easy to miss. The
    /// chain has to have finished — a reverb on a bus must be allowed to
    /// decay after the last thing feeding it stops. And the compensation ring
    /// has to have been fed silence for at least its own length, because
    /// until then it is still holding audio it has not emitted yet, and
    /// freezing it would strand that audio until the bus woke.
    fn is_resting(&self) -> bool {
        self.effects.is_at_rest()
            // The strip is asked the same question the chain is, and for the
            // same reason: a detector frozen mid-release wakes up holding
            // reduction the music has stopped asking for. A strip whose
            // sections are all out answers `true` without reading any state.
            && self.strip.is_at_rest()
            && self.silent_frames
                >= self.compensation.as_ref().map_or(0, |delay| delay.frames() as u32)
            // And its ramps have to have arrived. A bus that slept through a
            // fader move or a polarity flip would make the move when it woke,
            // on whatever reached it first, where a bus that never slept made
            // it on silence -- and the two renders must agree.
            && self.output.is_settled()
            && self.sign.is_settled()
    }

    /// Aim the output stage and the polarity at this block's knobs. First
    /// thing in the track's block, so `is_resting` and every mute decision
    /// after it are answered about this block rather than the last.
    fn aim(&mut self) {
        let silenced = self.output.muted || self.solo_silenced;
        self.output.aim(silenced);
        self.sign.set_target(if self.polarity { -1.0 } else { 1.0 });
    }

    /// Jump every ramp to where it is aimed. For a document arriving.
    fn settle(&mut self) {
        self.aim();
        self.output.settle();
        self.sign.reset_to(self.sign.target());
    }

    /// Whether what this track holds after its output stage is what is
    /// heard from it: anything but a muted or solo-silenced track whose fade
    /// has finished. A take reading a track reads this.
    fn is_heard(&self) -> bool {
        !((self.output.muted || self.solo_silenced) && self.output.is_silent())
    }

    fn reset(&mut self, reclaim: &mut Reclaim) {
        self.effects.clear(reclaim);
        self.output.reset(1.0);
        // Cleared as well as defaulted, and in that order, for the reason
        // `load_project`'s `Some` arm gives: `set_params` moves the values
        // and deliberately leaves the filter state alone, so a strip put
        // back to factory would still be holding the audio of the song that
        // just went away. This is the arm a project with *fewer* tracks than
        // the last one takes, which is the case that keeps a spare strip
        // rather than freeing it.
        //
        // By construction rather than under a test, and unusually for this
        // file that is the honest arrangement: the default it installs is
        // every section **out**, and an out strip neither reads its state nor
        // writes a sample, so no render can tell the two apart. What would
        // make the difference visible is a later document switching a section
        // back in -- and that arrives through the `Some` arm, which resets.
        self.strip.reset();
        self.strip.set_params(StripParams::default());
        self.polarity = false;
        self.sign.reset_to(1.0);
        self.solo_silenced = false;
        // The displaced ring leaves on the same carrier a displaced dry-path
        // aligner does: it is the same type doing the same job one level out,
        // and inventing a second channel for it would only mean two things to
        // drain.
        if let Some(delay) = self.compensation.take() {
            reclaim.push(ReclaimedEffect {
                node: None,
                align: Some(delay),
                analyzer: None,
                state: None,
                channel: None,
            });
        }
    }
}

/// One channel's storage, moved as a unit between the control thread and the
/// graph. The graph unpacks it into its parallel vectors immediately: those
/// stay separate because the block loop borrows them with different
/// mutabilities at once, and bundling would make that a conflict.
pub struct ChannelStorage {
    strip: Box<ChannelStrip>,
    events: Box<EventList>,
    control_outputs: Box<ControlOutputs>,
    source_curves: Box<SourceCurvePool>,
}

pub struct ChannelStrip {
    /// The channel's instrument: one slot, whatever kind it is (MOO-56).
    ///
    /// It used to be eight concrete fields -- one of every generator, all
    /// resident on every live channel -- and a `DeviceKind` tag picking one,
    /// so a source change could flip the tag inline on the audio thread. Now
    /// a source change is an ownership move: the new node is built on the
    /// control thread, arrives through `StructuralCommand::InstallSource`,
    /// and the one it displaces leaves through the reclaim ring. Adam ruled
    /// on 2026-09-22 that the change may land a block or more after the
    /// click when the ring is full: *"totally fine. there's no reason to
    /// expect that this action should be instantaneous."*
    ///
    /// The node says which kind it is ([`SourceNode::kind`]); nothing beside
    /// it keeps a second copy of that answer.
    source: Box<dyn SourceNode + Send>,
    /// What this channel's generator published during the block just
    /// rendered, indexed by outlet id.
    ///
    /// Read at the *start* of the next block, which is where the one
    /// declared block of outlet latency physically lives: it is not a delay
    /// line or a scheduling rule anybody has to remember, it is the fact
    /// that the control table is filled before the strips run. That is also
    /// what makes an offline render agree with a live take and stops graph
    /// order deciding what a route hears.
    published_outlets: [f32; MAX_GENERATOR_OUTLETS],
    /// The knob value for the active generator's parameters. The device
    /// retains only the value it was last sent, so this is what lets a knob
    /// move underneath an active lane without the two fighting -- the same
    /// split `EffectChain::base_params` makes for effects.
    source_base: GeneratorParams,
    effects: EffectChain,
    bus: StereoBus,
    output: OutputStage,
    /// Whether some *other* channel is soloed and this one is not.
    ///
    /// Derived on the control thread by `mooloop_core::channel::solo_silenced`
    /// and held apart from `output.muted` for the reason a track's is: dropping
    /// the solo gives a channel back whatever its own mute said. From here down
    /// it is the same question as mute -- a silenced channel's generator still
    /// runs for anything reading its tap, and nothing it makes reaches its bus.
    solo_silenced: bool,
    /// Mixer bus this channel feeds.
    destination: u8,
    /// How long this channel waits before summing into its bus, so that
    /// everything arriving there comes from the same moment.
    ///
    /// `None` is the common case and means this channel *is* the longest path
    /// into its bus, so nothing is owed. The length is decided off-thread by
    /// `mooloop_core::compile_latency` and the ring arrives preallocated;
    /// see `docs/plans/latency-compensation/`.
    compensation: Option<Box<IntegerDelay>>,
    /// Consecutive frames of silence the generator has put on this strip's
    /// bus, counted where the source meter is already read.
    ///
    /// A generator has no input to go quiet, so this is its *output*: the one
    /// measurement that covers every reason a device might still be making
    /// sound, including the finishing stages that outlive its voices.
    source_silent_frames: u32,
    /// Whether the strip was left uncalled last block. Only used to do the
    /// once-off tidying that falling asleep needs -- emptying the bus and the
    /// compensation ring -- rather than repeating it every idle block.
    sleeping: bool,
    /// This channel's take, if its record button has armed one. On the strip
    /// so that an install carrying the strip carries the take with it; see
    /// `take.rs`.
    take: Option<Box<crate::take::Take>>,
    /// The voices the sequencer started on this channel and has not ended
    /// (MOO-99). On the strip so an install that carries the strip -- and
    /// with it the voices -- carries the record of them too.
    sequenced: SequencedVoices,
}

/// Build a channel source running `params`. Allocates, so it belongs on the
/// control thread: in a project install, in `EngineHandle`'s source change,
/// or in the storage an added channel arrives in.
///
/// **The one match over the kinds of source**, and the only place the engine
/// names a concrete generator. Everything it does to a running one goes
/// through [`SourceNode`].
///
/// Each is built at its defaults and then *sent* `params` -- the way the
/// eight resident generators were reset and then sent a project's patch --
/// rather than constructed with it, so a device whose constructor and
/// `set_params` settle their smoothing differently still starts where it
/// always did. Aux In is the exception and has always been: its level is
/// snapped to the patch rather than ramped to it, because a loaded song
/// starts at the level it was saved at with nothing to click, and building it
/// with the patch is exactly that snap.
///
/// `audio_slot` is the channel's sample slot. Only a sampler reads it, but
/// every channel has one whatever it is playing, so a channel switched to the
/// sampler later is built reading the slot the channel already publishes to.
pub(crate) fn build_source(
    params: &GeneratorParams,
    audio_slot: ChannelAudioSlot,
    sample_rate: u32,
) -> Box<dyn SourceNode + Send> {
    match *params {
        GeneratorParams::Sampler(params) => {
            let mut node = Sampler::new(audio_slot, SamplerParams::default(), sample_rate);
            node.set_params(params);
            Box::new(node)
        }
        GeneratorParams::DrumSynth(params) => {
            let mut node = DrumSynth::new(DrumSynthParams::default(), sample_rate);
            node.set_params(params);
            Box::new(node)
        }
        GeneratorParams::MonoSynth(params) => {
            let mut node = MonoSynth::new(MonoSynthParams::default(), sample_rate);
            node.set_params(params);
            Box::new(node)
        }
        GeneratorParams::PolySynth(params) => {
            let mut node = PolySynth::new(PolySynthParams::default(), sample_rate);
            node.set_params(params);
            Box::new(node)
        }
        GeneratorParams::MlM1(params) => {
            let mut node = MlM1::new(MlM1Params::default(), sample_rate);
            node.set_params(params);
            Box::new(node)
        }
        GeneratorParams::MlP8(params) => {
            let mut node = MlP8::new(MlP8Params::default(), sample_rate);
            node.set_params(params);
            Box::new(node)
        }
        GeneratorParams::Ds01(params) => {
            let mut node = Ds01::new(Ds01Params::default(), sample_rate);
            node.set_params(params);
            Box::new(node)
        }
        GeneratorParams::AuxIn(params) => Box::new(AuxIn::new(params, sample_rate)),
    }
}

/// Where a channel's sampler voices are, for the playhead meter: nowhere --
/// every slot `NaN` -- on a channel that is not running the sampler, which
/// is what an idle resident sampler used to report on one.
fn sampler_playheads(source: &dyn SourceNode) -> [f32; MAX_SAMPLER_VOICES as usize] {
    source
        .as_sampler()
        .map_or([f32::NAN; MAX_SAMPLER_VOICES as usize], Sampler::voice_positions)
}

/// The patch a saved source carries, as the typed block the engine keeps.
fn saved_source_params(source: &ChannelSource) -> GeneratorParams {
    match source {
        ChannelSource::Sampler(state) => GeneratorParams::Sampler(state.params),
        ChannelSource::DrumSynth(state) => GeneratorParams::DrumSynth(state.params),
        ChannelSource::MonoSynth(state) => GeneratorParams::MonoSynth(state.params),
        ChannelSource::PolySynth(state) => GeneratorParams::PolySynth(state.params),
        ChannelSource::MlM1(state) => GeneratorParams::MlM1(state.params),
        ChannelSource::MlP8(state) => GeneratorParams::MlP8(state.params),
        ChannelSource::Ds01(state) => GeneratorParams::Ds01(state.params),
        ChannelSource::AuxIn(state) => GeneratorParams::AuxIn(state.params),
    }
}

impl ChannelStrip {
    /// A strip running `source`, which arrives at its kind's defaults.
    fn new(source: Box<dyn SourceNode + Send>, sample_rate: u32) -> Self {
        Self {
            source_base: source.kind().default_generator_params(),
            source,
            published_outlets: [0.0; MAX_GENERATOR_OUTLETS],
            effects: EffectChain::new(),
            bus: StereoBus::with_capacity(MAX_BLOCK_SIZE),
            output: OutputStage::new(mooloop_core::DEFAULT_CHANNEL_VOLUME, pan_gains, sample_rate),
            solo_silenced: false,
            destination: MASTER_BUS,
            compensation: None,
            source_silent_frames: 0,
            sleeping: false,
            take: None,
            sequenced: SequencedVoices::new(),
        }
    }

    /// Put `source` in the slot and hand back the one it displaces, which
    /// the caller must not drop on the audio thread.
    ///
    /// Moves two boxes and writes the base; allocates and frees nothing,
    /// which is what makes it callable from `apply_structural`. `source`
    /// arrives at its kind's defaults (see [`build_source`]), so the base
    /// is those defaults too, and the patch follows as parameter commands
    /// the way it always has.
    fn install_source(
        &mut self,
        source: Box<dyn SourceNode + Send>,
    ) -> Box<dyn SourceNode + Send> {
        self.source_base = source.kind().default_generator_params();
        std::mem::replace(&mut self.source, source)
    }

    /// Put the mixer half of the strip back to a fresh channel's: no
    /// effects, the default fader, the master. The source is not touched --
    /// replacing it is an ownership move, and the caller has the node.
    fn reset_slot(&mut self, reclaim: &mut Reclaim) {
        self.effects.clear(reclaim);
        self.output.reset(mooloop_core::DEFAULT_CHANNEL_VOLUME);
        self.solo_silenced = false;
        self.destination = MASTER_BUS;
    }

    /// Move one internal route's depth on both the base and the running node.
    ///
    /// Not routed through `push_source_base` like a knob is, because a route
    /// amount is the one authored value that must not rebuild the compiled
    /// topology: the node retunes the row it already has.
    fn set_source_route_amount(&mut self, route: u16, amount: f32) {
        let moved = self
            .source_base
            .internal_routes_mut()
            .is_some_and(|routes| routes.set_amount(route, amount));
        if !moved {
            return;
        }
        // Only a device with internal routes has one to move; the default
        // does nothing, which is what every other kind always did here.
        self.source.set_route_amount(route, amount);
    }

    /// Hand the authored base to the generator this channel is running.
    ///
    /// Allocation-free for every kind, which is what makes it callable from
    /// the audio thread: a generator keeps no queue between blocks, so its
    /// base is applied straight through rather than staged.
    fn push_source_base(&mut self) {
        let _ = self.source.set_generator_params(&self.source_base);
    }

    /// Take a whole typed parameter block from a command, if it is for the
    /// device this channel is running.
    ///
    /// One aimed at another kind changes nothing, the base included. The
    /// eight resident generators used to take such a block into the idle one
    /// and point the base at it, leaving the base describing a device the
    /// channel was not playing -- so the next single-parameter edit was
    /// applied to the wrong table.
    fn set_source_params(&mut self, params: GeneratorParams) {
        if self.source.set_generator_params(&params) {
            self.source_base = params;
        }
    }

    /// Install a channel's saved source, returning the node it displaces.
    ///
    /// Control thread only, and the caller drops what comes back: this
    /// builds a node, and `load_project` only ever runs while a `RenderState`
    /// is being prepared on the control thread or for an offline render --
    /// never from the audio callback. The same reason makes this the one
    /// path that may allocate the sampler's stretch pool inline instead of
    /// installing it structurally.
    fn load_source(
        &mut self,
        source: &ChannelSource,
        audio_slot: ChannelAudioSlot,
        sample_rate: u32,
    ) -> Box<dyn SourceNode + Send> {
        let params = saved_source_params(source);
        let mut node = build_source(&params, audio_slot, sample_rate);
        // Reconcile intent with state, so a saved project plays stretched
        // from its first note rather than after a round trip through the
        // structural queue.
        if let (GeneratorParams::Sampler(sampler), Some(node)) = (params, node.as_sampler_mut()) {
            if sampler.stretch_enabled {
                node.install_stretch(Box::new(StretchPool::new(
                    sampler.stretch_mode,
                    sample_rate,
                    MAX_SAMPLER_VOICES as usize,
                )));
            }
        }
        let displaced = std::mem::replace(&mut self.source, node);
        self.source_base = params;
        displaced
    }

    /// The generator this channel is running, as the node it is.
    ///
    /// `SourceNode` rather than `AudioNode` because `SourceNode: AudioNode`:
    /// the rest contract is inherited, so this answers `is_at_rest` and
    /// `tail_frames`, and also answers the one call the strip makes into its
    /// generator.
    fn source_node(&self) -> &dyn SourceNode {
        &*self.source
    }

    /// The generator and this channel's bus, borrowed apart.
    ///
    /// It hands back the bus with the node because it has to. A
    /// `source_node_mut(&mut self)` borrows the *whole* strip, so
    /// `node.process_source(.., &mut self.bus, ..)` at a call site is two
    /// mutable borrows of `self` and does not compile. Splitting the two
    /// fields here is what lets the compiler see they are disjoint.
    fn source_and_bus(&mut self) -> (&mut dyn SourceNode, &mut StereoBus) {
        (&mut *self.source, &mut self.bus)
    }

    /// The generator alone, for the callers that touch nothing else.
    fn source_node_mut(&mut self) -> &mut dyn SourceNode {
        self.source_and_bus().0
    }

    /// Record how loud the generator was this block, and return how many
    /// consecutive frames of silence it has now produced.
    fn note_source_level(&mut self, peak: f32, frames: usize) -> u32 {
        self.source_silent_frames = if peak <= SILENCE_PEAK {
            self.source_silent_frames.saturating_add(frames as u32)
        } else {
            0
        };
        self.source_silent_frames
    }

    /// Whether this strip can be left unrendered for a block.
    ///
    /// Three questions, and the generator is asked two of them. `is_at_rest`
    /// is about its voices -- a device with one still releasing says no --
    /// and the silence count is about everything downstream of them inside
    /// the device, which is how a finishing stage that outlives its voices is
    /// covered without the host knowing one exists. Then the chain, which has
    /// been counting the same way slot by slot.
    ///
    /// Aux In answers the first question with a flat no, and that is the
    /// point: its sound is another channel's, and it can start without an
    /// event of its own.
    fn is_idle(&self) -> bool {
        let source = self.source_node();
        source.is_at_rest()
            && self.source_silent_frames > source.tail_frames()
            && self.effects.is_at_rest()
    }

    /// Spend a block asleep: move whatever runs on the clock, and the first
    /// time round, empty what would otherwise be emitted on waking.
    /// Tell this channel's generator and its whole chain that time moved.
    fn on_discontinuity(&mut self, kind: Discontinuity) {
        self.source_node_mut().on_discontinuity(kind);
        self.effects.on_discontinuity(kind);
    }

    fn sleep(&mut self, context: &ProcessContext) {
        self.source_node_mut().skip_block(context);
        self.effects.sleep(context);
        self.source_silent_frames = self.source_silent_frames.saturating_add(context.frames as u32);
        if self.sleeping {
            return;
        }
        self.sleeping = true;
        // Once, on the way down. Nothing writes either of these while the
        // strip is asleep, so emptying them again every block would be work
        // to reach a state they are already in.
        //
        // The bus so nothing downstream can read what the last audible block
        // left in it, and the compensation ring for the reason the mute path
        // empties it: a silent producer's pipeline is silent too, and a ring
        // still holding pre-silence audio would emit it on the first block
        // after the strip wakes.
        self.bus.clear(context.frames.min(self.bus.capacity()));
        if let Some(delay) = self.compensation.as_mut() {
            delay.reset();
        }
    }

    fn choke_group(&self) -> u8 {
        self.source.choke_group()
    }

    /// Render the generator, filling whatever audio outlets are subscribed
    /// and reading whatever edge resolved.
    ///
    /// `source` and `ports` are supplied for the duration of the call and not
    /// retained, which is `AUDIO_ARCHITECTURE.md`'s rule for auxiliary
    /// buffers. Both are empty on every project that has never authored an
    /// edge, and the devices that can use them branch once a render range on
    /// that fact.
    ///
    /// `events` is `&mut` now rather than `&`: `curves` (this block's driven
    /// source parameters, resolved by the caller into a per-destination row
    /// rather than pushed here) is handed to the generator through
    /// `AudioNode::apply_curves` before `process_source`, and the default
    /// implementation -- every generator's, today -- needs somewhere to put
    /// the equivalent `ParamValue` events it falls back to. That somewhere
    /// is this same list, which is why it has to be writable; a generator
    /// still receives exactly what it always did, either by the old event
    /// path directly or by the fallback reconstructing it.
    ///
    /// Returns how many of the fallback's events `events` had no room for.
    fn process(
        &mut self,
        context: &ProcessContext,
        events: &mut EventList,
        curves: &[ControlCurve<'_>],
        source: Option<&StereoBus>,
        ports: &mut AudioTaps,
    ) -> u64 {
        let (node, bus) = self.source_and_bus();
        let refused = if curves.is_empty() {
            0
        } else {
            node.apply_curves(curves, CONTROL_RATE_FRAMES, events)
        };
        node.process_source(context, bus, &*events, source, ports);
        self.publish_outlets();
        refused
    }

    /// Take the generator's published control outlets for this block.
    ///
    /// Only the active generator publishes, and the rest of the band is
    /// zeroed rather than left holding the last device's values: replacing a
    /// source must not leave a route reading a signal from an instrument that
    /// is no longer there. A generator with nothing to publish clears the
    /// band, which is the same thing said for a device that has not
    /// implemented outlets yet.
    fn publish_outlets(&mut self) {
        // Zeroed here, before the device is asked, which is the sequencing
        // the paragraph above is about: the band the generator does not fill
        // is cleared by the host and not by whoever happens to be installed.
        // The band is eight floats on the stack, so building it and storing
        // it is a move rather than anything the callback has to pay for, and
        // it keeps the field from being readable half-written.
        let mut published = [0.0; MAX_GENERATOR_OUTLETS];
        self.source_node_mut().publish_outlets_into(&mut published);
        self.published_outlets = published;
    }
}

/// Sum one bus into another. The two indices are unrelated now that routing
/// is arbitrary, so the disjoint borrow is taken by splitting at whichever is
/// higher rather than assuming the destination is lower.
fn mix_into(buses: &mut [BusStrip], from: usize, into: usize, frames: usize, console: bool) {
    if from == into || from >= buses.len() || into >= buses.len() {
        return;
    }
    let (left, right) = buses.split_at_mut(from.max(into));
    let (source, destination) = if from < into {
        (&left[from], &mut right[0])
    } else {
        (&right[0], &mut left[into])
    };
    // The same two-accumulator rule a channel follows, which is why nesting
    // needs no special case: a console-on bus is a producer like any other
    // and whatever it feeds decodes it.
    match (console, destination.console_sum.as_mut()) {
        (true, Some(sum)) => {
            add_encoded(sum, &source.bus, frames);
            destination.console_dirty = true;
        }
        _ => destination.bus.add_from(&source.bus, frames),
    }
    destination.dirty = true;
}

/// Sum `source` into `sum` through the console encode, in one pass.
///
/// One pass rather than encode-then-add because the alternative would have to
/// write the encoded signal somewhere, and the only buffer available is the
/// producer's own -- which the meters and, for the master, the caller still
/// read.
fn add_encoded(sum: &mut StereoBus, source: &StereoBus, frames: usize) {
    for index in 0..frames {
        sum.l[index] += console::encode(source.l[index]);
        sum.r[index] += console::encode(source.r[index]);
    }
}


/// The most auditions one block may carry. A block is a couple of
/// milliseconds; anything past this is a stuck key, not playing. Sized for
/// both hands on a MIDI keyboard landing a chord and releasing the last one
/// in the same block, because what falls past the cap can be a note-off.
const MAX_AUDITIONS_PER_BLOCK: usize = 64;

/// The most events one block hands back to the control layer. Control input is
/// forwarded, not acted on here, and a desk sending a fader stream must not be
/// able to make the audio thread grow a buffer -- so the surplus is dropped,
/// which for a stream of positions means the control layer sees a slightly
/// coarser sweep and nothing worse. A note-off is never in this list: notes
/// sound on the audio thread and their release does not depend on it.
const MAX_OUTGOING_EVENTS_PER_BLOCK: usize = 128;

/// The sustain pedal's controller number (MOO-128).
const SUSTAIN_PEDAL_CC: u8 = 64;

/// The mod wheel's controller number (MOO-128).
const MOD_WHEEL_CC: u8 = 1;

/// How far the bend wheel reaches either way, in semitones (MOO-128). One
/// range for every source, and the General MIDI default. Not a setting yet,
/// so it is neither persisted nor per channel.
const PITCH_BEND_RANGE_SEMITONES: f32 = 2.0;

/// A 14-bit bend, centred on zero, as semitones. The two halves are scaled
/// separately because the wire range is lopsided (`-8192..=8191`): the wheel
/// fully up is the whole range, not a hair short of it.
fn bend_semitones(value: i16) -> f32 {
    let travel = if value >= 0 { 8191.0 } else { 8192.0 };
    (f32::from(value) / travel).clamp(-1.0, 1.0) * PITCH_BEND_RANGE_SEMITONES
}

/// One channel's keyboard expression: what the wheels last said, and what
/// its source has still to hear (MOO-128).
///
/// Held here rather than queued as auditions, because a bend is a state and
/// not a gesture. Only the latest value matters, so a wheel swept through a
/// block costs one event rather than one per message, and a value that finds
/// the channel's event list full stays owed until a later block has room --
/// the wheel's return to centre is the message that must never be lost.
#[derive(Clone, Copy)]
struct ChannelExpression {
    /// Semitones, `0.0` at rest.
    bend: f32,
    /// The frame the latest bend is still owed at, or `None` once the
    /// source has it.
    bend_due: Option<u32>,
    /// The mod wheel and aftertouch as this channel's routes read them,
    /// `0..1`. Not events: a route reads them at the top of each block.
    performance: [f32; PERFORMANCE_SOURCES],
    /// The two things aftertouch is made of, kept apart so a keyboard that
    /// sends both is read as the harder of the two rather than as whichever
    /// spoke last.
    channel_pressure: f32,
    key_pressure: f32,
}

impl ChannelExpression {
    const REST: Self = Self {
        bend: 0.0,
        bend_due: None,
        performance: [0.0; PERFORMANCE_SOURCES],
        channel_pressure: 0.0,
        key_pressure: 0.0,
    };

    fn aftertouch(&mut self) {
        self.performance[usize::from(PERFORMANCE_AFTERTOUCH)] =
            self.channel_pressure.max(self.key_pressure);
    }

    fn bend(&mut self, offset: u32, semitones: f32) {
        self.bend = semitones;
        self.bend_due = Some(offset);
    }
}

/// Words of the held-key bitset: one bit per channel, so a note held on
/// several listening channels releases on all of them.
const HELD_KEY_WORDS: usize = MAX_CHANNELS / 64;

/// Which channels are holding each MIDI note down.
///
/// A bitset rather than [`Renderer::keyboard_channel`]'s single byte, because
/// a note can now go down on several channels at once: that is what a
/// multitimbral setup *is*. The release still goes exactly where the press
/// went, which is what the single byte was protecting, and now protects it for
/// every channel that took the note rather than for the last one to.
///
/// **The sustain pedal lives here too** (MOO-128), because a sustained note is
/// a held note whose key has come up: the one place that knows where a
/// release has to go is the one place that can defer it. While the pedal is
/// down, a released key moves from `channels` to `sustained` instead of
/// sounding its note-off, and lifting the pedal releases everything
/// `sustained` holds. No source has to know the pedal exists, which is why
/// all eight respond to it at once.
#[derive(Clone, Copy)]
struct HeldKeys {
    channels: [[u64; HELD_KEY_WORDS]; 128],
    /// Notes whose key is up but whose release the pedal is holding back,
    /// on the channels that played them.
    sustained: [[u64; HELD_KEY_WORDS]; 128],
    /// CC 64 at or above 64, from any input. One pedal, like one keyboard:
    /// a second controller's pedal is the same pedal.
    pedal: bool,
}

impl HeldKeys {
    const fn new() -> Self {
        Self {
            channels: [[0; HELD_KEY_WORDS]; 128],
            sustained: [[0; HELD_KEY_WORDS]; 128],
            pedal: false,
        }
    }

    /// A key came up. With the pedal down its channels are sustained and
    /// nothing is released; otherwise they are released now.
    fn key_up(&mut self, note: u8) -> HeldChannels {
        let released = self.release(note);
        if !self.pedal {
            return released;
        }
        let sustained = &mut self.sustained[usize::from(note & 0x7f)];
        for (word, held) in sustained.iter_mut().zip(released.words) {
            *word |= held;
        }
        HeldChannels {
            words: [0; HELD_KEY_WORDS],
            word: 0,
        }
    }

    /// The channels the pedal is holding `note` on, without clearing them:
    /// a release that finds no room this block has to be tried again next
    /// block, or the note would sound forever. [`Self::unsustain`] clears
    /// each one as its note-off is queued.
    fn sustained_on(&self, note: u8) -> HeldChannels {
        HeldChannels {
            words: self.sustained[usize::from(note & 0x7f)],
            word: 0,
        }
    }

    fn unsustain(&mut self, note: u8, channel: u8) {
        let channel = usize::from(channel);
        self.sustained[usize::from(note & 0x7f)][channel / 64] &= !(1 << (channel % 64));
    }

    /// Whether a lifted pedal left releases still to send.
    fn owes_releases(&self) -> bool {
        !self.pedal
            && self
                .sustained
                .iter()
                .any(|note| note.iter().any(|word| *word != 0))
    }

    /// Set the pedal. Returns whether it just came up, when the caller has to
    /// release every sustained note.
    fn set_pedal(&mut self, down: bool) -> bool {
        let lifted = self.pedal && !down;
        self.pedal = down;
        lifted
    }

    #[cfg(test)]
    fn is_sustained(&self, note: u8, channel: u8) -> bool {
        let channel = usize::from(channel);
        self.sustained[usize::from(note & 0x7f)][channel / 64] & (1 << (channel % 64)) != 0
    }

    fn hold(&mut self, note: u8, channel: u8) {
        let channel = usize::from(channel);
        self.channels[usize::from(note & 0x7f)][channel / 64] |= 1 << (channel % 64);
    }

    fn is_held(&self, note: u8, channel: u8) -> bool {
        let channel = usize::from(channel);
        self.channels[usize::from(note & 0x7f)][channel / 64] & (1 << (channel % 64)) != 0
    }

    /// Take every channel holding `note`, clearing them.
    fn release(&mut self, note: u8) -> HeldChannels {
        HeldChannels {
            words: std::mem::replace(&mut self.channels[usize::from(note & 0x7f)], [0; HELD_KEY_WORDS]),
            word: 0,
        }
    }

    #[cfg(test)]
    fn any_held(&self) -> bool {
        self.channels
            .iter()
            .any(|note| note.iter().any(|word| *word != 0))
    }
}

/// The channels a released note was held on.
struct HeldChannels {
    words: [u64; HELD_KEY_WORDS],
    word: usize,
}

impl Iterator for HeldChannels {
    type Item = u8;

    fn next(&mut self) -> Option<u8> {
        while self.word < HELD_KEY_WORDS {
            let word = &mut self.words[self.word];
            if *word == 0 {
                self.word += 1;
                continue;
            }
            let bit = word.trailing_zeros() as usize;
            *word &= *word - 1;
            return Some((self.word * 64 + bit) as u8);
        }
        None
    }
}

/// How each channel takes MIDI input, indexed by channel.
///
/// Built on the control thread from the project's stored input settings and
/// the driver's current port list, and swapped in whole. Shorter than the
/// channel bank is not an error: a channel past the end has no explicit route
/// and so follows the selection, which is the default and what every project
/// written before this existed does.
#[derive(Debug, Clone, Default)]
pub struct MidiRouting {
    pub routes: Vec<mooloop_core::MidiInputRoute>,
}

impl MidiRouting {
    fn route(&self, channel: usize) -> mooloop_core::MidiInputRoute {
        self.routes.get(channel).copied().unwrap_or_default()
    }
}

/// Which buffer each channel records from, indexed by channel.
///
/// `audio-recording/02`. Every channel's AUDIO row, resolved on the control
/// thread from the stored `AudioInputSource`s -- identities -- to seats in
/// this generation's bank, and swapped in whole like [`MidiRouting`]. It is
/// per generation for the same reason: a channel edit renumbers seats, so the
/// outgoing renderer must keep the routing it was built for. Shorter than the
/// bank is not an error: a channel past the end has no audio input.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioInputRouting {
    pub taps: Vec<Option<mooloop_core::AudioTap>>,
}

/// One note being recorded, from its press until its release.
#[derive(Clone, Copy)]
struct RecordingNote {
    channel: u8,
    /// The pattern `start_tick` is in: the one selected at the press, which
    /// is not necessarily the one selected at the release.
    pattern: u8,
    velocity: u8,
    /// Where it landed in the selected pattern, folded by
    /// `Sequencer::recording_tick`.
    start_tick: u32,
    /// Absolute frames at the press. Length is measured from this rather than
    /// from `start_tick`, so a note held across the loop point reports how
    /// long it was held instead of a negative number.
    start_frames: u64,
}

/// Note ids for auditioned notes, kept clear of the sequencer's.
///
/// Counted down from the top rather than up from zero because the sequencer
/// mints ids from its patterns upwards: a note-off has to find the note-on it
/// belongs to, and two id spaces that can meet would let an audition release
/// a sequenced note.
fn audition_note_id(note: u8) -> u64 {
    u64::MAX - u64::from(note)
}

/// Note ids for keys played on a MIDI keyboard: the block below the UI's
/// auditions, so releasing a key never releases a slice being auditioned on
/// the same pitch, or the other way round.
fn keyboard_note_id(note: u8) -> u64 {
    u64::MAX - 128 - u64::from(note)
}

/// The keyboard channel's value when no channel should hear the keyboard.
pub(crate) const NO_KEYBOARD_CHANNEL: u8 = u8::MAX;

/// One note the UI or the keyboard asked a channel to sound this block.
#[derive(Clone, Copy)]
struct Audition {
    channel: u8,
    /// Frames into the block. Zero for the UI, whose gesture has no position
    /// inside one; where the driver timestamps MIDI, a key keeps its own.
    offset: u32,
    event: Event,
}

fn inject_choke_events(choke_groups: &[u8], events: &mut [Box<EventList>]) {
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

/// Every how many control ticks `rows` destinations of `ticks` ticks each
/// can emit an event and still fit in `room`: one when they all fit, and at
/// most `ticks`, which is one event per destination per block. The internal
/// routes' twin of `mooloop_dsp::node`'s `fallback_stride` (MOO-73).
fn control_tick_stride(ticks: usize, rows: usize, room: usize) -> usize {
    let mut stride = 1;
    while stride < ticks && rows * ticks.div_ceil(stride) > room {
        stride += 1;
    }
    stride
}

/// Release every voice on every live channel at `offset`.
///
/// Sorted ahead of any note-on at the same offset by `push_ordered`, so the
/// notes a loop pass or a seek lands on start after the previous ones have
/// been let go rather than instead of them.
fn release_all_voices(offset: u32, channels: usize, events: &mut [Box<EventList>]) {
    for event_list in events.iter_mut().take(channels) {
        event_list.push_ordered(TimedEvent {
            offset,
            event: Event::Choke,
        });
    }
}

/// Tuple fields a control change has retuned, waiting for the next note to
/// fire. Held rather than applied immediately so turning a knob mid-gesture
/// bends the *next* edit instead of restarting the current one.
#[derive(Debug, Clone, Copy, Default)]
struct BufferCcState {
    window_beats: Option<f32>,
    offset_beats: Option<f32>,
    repeat: Option<u32>,
}

impl BufferCcState {
    fn apply(&self, mut event: mooloop_core::BufferEvent) -> mooloop_core::BufferEvent {
        if let Some(window) = self.window_beats {
            event.window_beats = Some(window);
        }
        if let Some(offset) = self.offset_beats {
            event.offset_beats = offset;
        }
        if let Some(repeat) = self.repeat {
            event.repeat = Some(repeat);
        }
        event
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

/// Deferred command slots. One per kind that can be in flight, with room to
/// spare: the count bounds the state without an overflow policy, and it is
/// sized so that reaching the end is a defect rather than a busy engine.
const MAX_DEFERRED: usize = 8;

/// A command and the absolute tick it is waiting for.
#[derive(Clone, Copy)]
struct PendingCommand {
    target_tick: f64,
    command: EngineCommand,
}

pub(crate) struct RenderState {
    transport: Transport,
    sequencer: Sequencer,
    /// One entry per channel the project actually has, not per addressable
    /// channel. Boxed so the vector reserves its full addressable length in
    /// pointers and grows without ever reallocating on the audio thread.
    strips: Vec<Box<ChannelStrip>>,
    /// Kept so a channel can be materialized after construction: a strip
    /// needs its channel's audio slot, and the control thread builds them.
    audio_slots: ChannelAudioBank,
    /// The full bus bank, master first. Always `MAX_BUSES` long, so assigning
    /// a channel to any bus is a bounded mutation rather than an allocation.
    buses: Vec<BusStrip>,
    /// Destinations and their matching render order, compiled together off the
    /// audio thread. The executor only installs or walks this value.
    bus_graph: CompiledBusGraph,
    /// This generation's sends. Installed with `bus_graph` as one value, so
    /// the executor never holds a send whose target the render order has not
    /// been told about.
    ///
    /// Boxed for the reason `audio` is: the swap that installs a new
    /// generation hands the old one back through the reclaim ring, and a
    /// box-for-box exchange is the only way to do that without the audio
    /// thread allocating.
    sends: Box<SendBank>,
    /// The channels' audio edges, the order that satisfies them, and the
    /// buffers they carry. Boxed so a whole generation crosses to the
    /// executor as one pointer swap and the displaced one is freed off the
    /// audio thread.
    audio: Box<AudioTapBank>,
    /// One block of the tap a consumer is reading, copied in before its
    /// generator runs.
    ///
    /// A copy rather than a borrow because the same call may write this
    /// channel's own taps: those are provably different buffers -- a device
    /// cannot subscribe to itself -- but nothing in the type system knows it,
    /// and one scratch buffer for the whole bank is a far smaller price than
    /// unsafe aliasing or one buffer a channel.
    aux_scratch: StereoBus,
    events: Vec<Box<EventList>>,
    /// One channel's driven source destinations for the current block,
    /// resolved beside `events` and handed to the generator through
    /// `AudioNode::apply_curves` instead of being pushed onto `events` as
    /// `ParamValue`s -- see the loop this replaced, once at
    /// `control_events_for_slot`'s doc comment for the effect-slot half of
    /// the same change. Boxed for the reason `control_outputs` below is:
    /// `MAX_SOURCE_CURVE_DESTINATIONS` rows of `MAX_CONTROL_TICKS_PER_BLOCK`
    /// ticks is real size, and one addressable channel should cost a
    /// pointer until it exists.
    source_curves: Vec<Box<SourceCurvePool>>,
    /// Destinations a channel's source could not fit into its curve pool at
    /// once, counted the way `EffectChain::curve_refusals` is.
    source_curve_refusals: u64,
    /// The saved matrix and the runnable sources are deliberately separate:
    /// the former is editable/persisted configuration; the latter contains
    /// LFO phase and other realtime-only state.
    modulation: Vec<ModRack>,
    modulators: Vec<ModulatorRack>,
    /// A full block of resolved control signal is 8 KiB; reserving one for
    /// every addressable channel cost 2 MiB before a project existed
    /// (`docs/plans/archive/modulator-capacity/`).
    control_outputs: Vec<Box<ControlOutputs>>,
    /// This block's note gates, kept rather than rebuilt. See [`GateTable`].
    gate_ticks: Box<GateTable>,
    sample_rate: u32,
    /// Effect-slot occupants displaced by a chain being cleared, awaiting
    /// disposal off the realtime thread.
    ///
    /// Filled by `EffectChain::clear` through `reset_slot` / `reset`: on the
    /// control thread when `load_project` reuses a state, and on the audio
    /// thread when `StructuralCommand::AddChannel` reuses a spare channel
    /// slot. Drained by the executor, one [`StructuralReclaim::Effect`] per
    /// free reclaim-ring slot, through [`Self::pop_displaced_effect`]; a
    /// state that never reaches an executor (an offline render, a retired
    /// generation) drops whatever is left with itself, off the audio thread.
    ///
    /// Reserved for one whole chain, which is the most a single structural
    /// edit can displace, and the executor admits no structural edit while
    /// anything is still waiting here -- so the audio thread only ever pushes
    /// into an empty vector with room for everything the push can add.
    reclaim: Reclaim,
    /// Where per-bus peaks are published for the mixer. Offline renders keep
    /// their own unread instance rather than paying for an `Option` check per
    /// bus per block.
    meters: Arc<BusMeters>,
    device_meters: Arc<DeviceMeters>,
    device_telemetry: Arc<DeviceTelemetry>,
    /// How MIDI input drives a buffer insert. `None` until the control layer
    /// configures one, so an unmapped project pays nothing for MIDI beyond
    /// decoding it.
    ///
    /// **This and the two routing tables below are plain boxes, replaced by
    /// structural commands** (`StructuralCommand::SetBufferMidi`,
    /// `SetMidiRouting`, `SetAudioInputRouting`), and the one they displace
    /// leaves through the reclaim ring. They were `ArcSwap` cells until
    /// 2026-09-19, and a guard held here could be the last owner of a table
    /// the control thread had just replaced, freeing it on this thread
    /// (`reports/fable-2026-09-19.md`, finding 2). Nothing the audio thread
    /// reads from the control side is reference-counted now: it arrives on
    /// the ordered stream or it is an atomic.
    buffer_midi: Option<Box<mooloop_core::midi::BufferMidiMap>>,
    buffer_cc: BufferCcState,
    /// The channel a MIDI keyboard plays, or [`NO_KEYBOARD_CHANNEL`]. Shared
    /// with the control layer, which follows the editor's selection with it.
    keyboard_channel: Arc<AtomicU8>,
    /// How each channel takes MIDI input. Rebuilt by the control layer when a
    /// channel's setting changes or a port appears, and swapped in whole.
    midi_routing: Box<MidiRouting>,
    /// The keys the control map takes from the instruments: a pad bound to
    /// something, or every key while a learn gesture waits. Asked before a
    /// note is played, and swapped in whole like `midi_routing` (MOO-129).
    claimed_notes: Box<mooloop_core::ClaimedNotes>,
    /// Which buffer each channel records from, read every block by
    /// [`Self::advance_takes`]. Swapped in whole like `midi_routing`.
    audio_input_routing: Box<AudioInputRouting>,
    /// The hardware input, filled from the driver at the top of each block by
    /// [`Self::load_input`] (`audio-recording/01`). Preallocated; a driver
    /// with no input, and every offline render, leaves it silent.
    input: StereoBus,
    /// Whether `input` may hold anything but zeros, so a block with no input
    /// empties it once rather than every time.
    input_dirty: bool,
    /// Which channels play the hardware input through their strip
    /// (`audio-recording/01`, monitoring). Only a channel whose AUDIO input is
    /// the hardware input hears it; the flag on any other does nothing.
    monitor: [bool; MAX_CHANNELS],
    /// Which channels are holding each MIDI note down. The release goes where
    /// the press went: a key held while the selection moves would otherwise
    /// send its note-off to a channel that never started it and leave the
    /// first one sounding.
    held_keys: HeldKeys,
    /// Each channel's bend wheel (MOO-128). Delivered to the channels a key
    /// from the same input would play, so a bend reaches the notes it is
    /// meant for.
    expression: [ChannelExpression; MAX_CHANNELS],
    /// Each key's own pressure, from a keyboard that sends it per key. The
    /// aftertouch a channel's routes read is the hardest-pressed key's: a
    /// channel has one aftertouch source, not one per voice.
    key_pressure: [u8; 128],
    /// Whether recording is armed. Capture also needs the transport running,
    /// which is checked at the note rather than here, so arming while stopped
    /// is the ordinary thing it looks like.
    record_armed: bool,
    /// Notes being recorded, by MIDI note. One per pitch: a second press of a
    /// pitch already down closes the first, which is the same rule
    /// [`Renderer::press_key`] applies to sounding it.
    recording: [Option<RecordingNote>; 128],
    /// Events for the control layer, collected this block and drained by the
    /// executor after the block renders.
    outgoing: [Option<mooloop_core::EngineEvent>; MAX_OUTGOING_EVENTS_PER_BLOCK],
    playhead_meters: Arc<PlayheadMeters>,
    modulator_meters: Arc<ModulatorMeters>,
    /// The sample browser's audition voice, if something is playing. One at
    /// a time: a new preview replaces the old, and the retired sample's
    /// ownership returns to the UI thread through the reclaim ring.
    preview: Option<PreviewVoice>,
    /// The voice a new preview or a Stop displaced, fading out over
    /// [`PREVIEW_FADE_S`] rather than cut mid-waveform. It retires through the
    /// same ring as a preview that played to its end.
    preview_fading: Option<PreviewVoice>,
    preview_retired: RetiredPreviews,
    /// Linear preview gain, shared with the GUI so the knob is heard live.
    /// It starts at the operating level rather than unity: an audition is
    /// usually a full-scale commercial file, and the browser should not be
    /// 12 dB louder than the project it plays over.
    ///
    /// `REFERENCE_PEAK_DBFS` here, not the generator output reference the
    /// sampler's trim uses, because a preview goes straight to the master
    /// with no channel strip -- so it never pays the pan law the sampler's
    /// 3 dB of extra trim exists to cancel. Both land at -12 dBFS.
    preview_gain: Arc<AtomicU32>,
    /// Notes the UI asked for since the last block, waiting to be dispatched.
    ///
    /// Held rather than applied on arrival because the command drain runs
    /// before `process_block_inner` clears the event lists, so a note pushed
    /// straight into one would be thrown away before anything read it. A
    /// fixed array, so filling it allocates nothing.
    auditions: [Option<Audition>; MAX_AUDITIONS_PER_BLOCK],
    /// The section of the arrangement the transport repeats.
    ///
    /// Held unclamped, as the project stores it, and resolved against the
    /// song's own length once per block: the length is derived from the clips
    /// on the playlist and so changes under the loop without the loop being
    /// edited at all.
    loop_range: LoopRange,
    /// Whether the transport moved discontinuously since the last block.
    ///
    /// Set by a seek and consumed by the next block, for the same reason
    /// `auditions` is: the command drain runs before the event lists are
    /// cleared, so the release this owes every sounding voice cannot be
    /// pushed at the moment the seek arrives.
    ///
    /// **Only a seek.** A Pattern-mode pattern switch owes the same release
    /// and none of the rest, and until 2026-09-22 it set this flag too -- so
    /// the block after it told every node `Seek`, straight after the switch
    /// had told them `ProgramChange`, and every delay, reverb and plate in
    /// the project flushed on a pattern switch (MOO-59). The switch releases
    /// the voices it stranded through each strip's table of sequenced voices
    /// instead (MOO-99), which also leaves a held key alone.
    seeked: bool,
    /// Whether [`EngineCommand::Panic`] arrived since the last block, which
    /// owes every voice on every channel the release a seek owes and none of
    /// the rest: the transport has not moved (MOO-99).
    panicked: bool,
    /// Commands waiting for a musical edge, one slot per kind.
    ///
    /// `docs/plans/transport-discontinuity/02-deferred-commands.md`.
    deferred: [Option<PendingCommand>; MAX_DEFERRED],
    /// How many channel-blocks have been skipped since this state was built.
    ///
    /// One `u64` for the whole engine and one add per skipped strip. It is
    /// here so the equivalence tests can say that the two renders they
    /// compared were not simply the same render twice: a skip mechanism that
    /// never fires would pass every one of them.
    slept_strip_blocks: u64,
    /// Deferred commands refused for want of a slot, since this state was
    /// built. The per-channel lists count their own refusals
    /// (`EventList::refused`), which is what reaches the sequencer's note
    /// and choke pushes too (MOO-73). See [`Self::refused_events`].
    refused_events: u64,
    /// Where each track's channel strip runs in its block.
    ///
    /// Initialized from `mooloop_core::mixer::STRIP_PIN`, which is the one
    /// statement of the policy; this field exists so a test can render the
    /// same project both ways and say what the difference is, the way
    /// `skip_idle` exists so one can render it with and without skipping. It
    /// is not a user setting and no command moves it.
    strip_pin: StripPin,
    /// Whether devices and strips with nothing to do may be left uncalled.
    ///
    /// On by default and not exposed as a user setting. It exists so the
    /// equivalence tests can render the same project both ways and compare
    /// the two sample for sample, which is the only way to hold a skip
    /// honest: a mechanism whose whole claim is that it changes nothing has
    /// to be checkable against the thing it claims not to change.
    skip_idle: bool,
    /// The master's last stage: the non-finite scrub and the 0 dBFS safety
    /// limiter every block passes through on its way to the ports or a file
    /// (MOO-93, `mooloop_dsp::output_guard`).
    output_guard: OutputGuard,
    /// Samples the guard found NaN or infinite and replaced with silence,
    /// since this state was built. See [`Self::output_non_finite`].
    output_non_finite: u64,
    /// Samples that reached the guard above 0 dBFS and were limited, since
    /// this state was built. See [`Self::output_overs`].
    output_overs: u64,
}

/// How long a preview that is stopped or replaced takes to fade out. Long
/// enough that the cut is not a click, short enough that auditioning down a
/// list of files still feels immediate.
const PREVIEW_FADE_S: f64 = 0.002;

/// One-shot straight to the master output: no envelope, no channel strip.
/// A browser preview should sound like the file, not like the project.
///
/// "Like the file" includes its sample rate. The read position counts the
/// file's own frames and advances by `file rate / engine rate` per output
/// frame, through the sampler's band-limited [`SincTable`] -- read at the
/// engine rate instead, a 44.1 kHz file auditioned 1.5 semitones sharp on a
/// 48 kHz engine and a 96 kHz file an octave low, while the same file
/// loaded into a sampler played at its true pitch.
struct PreviewVoice {
    sample: Arc<SampleData>,
    /// Position in the file's frames. Whole-numbered while the file's rate
    /// matches the engine's, which reads each frame untouched.
    position: f64,
    /// Output frames left in a fade-out, and its length; `None` while the
    /// voice is playing normally.
    fade: Option<(u32, u32)>,
}

impl PreviewVoice {
    fn new(sample: Arc<SampleData>) -> Self {
        Self {
            sample,
            position: 0.0,
            fade: None,
        }
    }

    fn finished(&self) -> bool {
        self.position >= self.sample.frames.len() as f64 || matches!(self.fade, Some((0, _)))
    }

    /// Sum up to `frames` output frames into `bus`. Returns whether the voice
    /// has nothing left to play.
    fn render(&mut self, bus: &mut StereoBus, frames: usize, gain: f32, engine_rate: u32) -> bool {
        let samples = &self.sample.frames;
        let len = samples.len();
        // A file with no rate recorded plays at the engine's rather than not
        // at all.
        let file_rate = if self.sample.sample_rate == 0 {
            engine_rate
        } else {
            self.sample.sample_rate
        };
        let rate = f64::from(file_rate) / f64::from(engine_rate.max(1));
        let table = SincTable::shared();
        let region = Region::whole(len);
        for index in 0..frames {
            if self.position >= len as f64 {
                return true;
            }
            let fade_gain = match &mut self.fade {
                None => 1.0,
                Some((0, _)) => return true,
                Some((left, length)) => {
                    let level = *left as f32 / *length as f32;
                    *left -= 1;
                    level
                }
            };
            let frame = if rate == 1.0 {
                samples[self.position as usize]
            } else {
                table.read(samples, self.position, rate, region)
            };
            bus.l[index] += frame[0] * gain * fade_gain;
            bus.r[index] += frame[1] * gain * fade_gain;
            self.position += rate;
        }
        self.finished()
    }
}

impl RenderState {
    pub fn new(sample_rate: u32, audio_slots: ChannelAudioBank) -> Self {
        #![allow(clippy::let_and_return)]
        // Deliberately empty. Channels are materialized from a project on
        // this thread, or pushed one at a time through the structural ring.
        let strips = Vec::with_capacity(MAX_CHANNELS);
        let slots_for_growth = audio_slots;
        let mut state = Self {
            transport: Transport::new(sample_rate),
            strip_pin: STRIP_PIN,
            skip_idle: true,
            slept_strip_blocks: 0,
            refused_events: 0,
            sequencer: Sequencer::new(1, 1, DEFAULT_STEPS as usize, mooloop_core::Ppq::DEFAULT),
            strips,
            audio_slots: slots_for_growth,
            // The master alone. Tracks arrive with the project through
            // `grow_buses`, rather than seventeen 64 KB strips being built
            // whether or not a song has them -- `docs/CAPACITY_POLICY.md`:
            // reserving an address space is free, dimensioning by it is not.
            // The master is not optional, because `master()` reads it and
            // every route ends there.
            buses: {
                let mut buses = Vec::with_capacity(MAX_BUSES);
                buses.push(BusStrip::new(sample_rate));
                buses
            },
            bus_graph: CompiledBusGraph::default(),
            sends: Box::new(SendBank::default()),
            // A project with no subscriptions holds no buffers at all, which
            // is the whole design: the identity order costs nothing and
            // renders exactly what the engine rendered before this existed.
            audio: Box::new(AudioTapBank::new(CompiledAudioGraph::default())),
            aux_scratch: StereoBus::with_capacity(MAX_BLOCK_SIZE),
            events: Vec::with_capacity(MAX_CHANNELS),
            source_curves: Vec::with_capacity(MAX_CHANNELS),
            source_curve_refusals: 0,
            // Small enough that reserving the addressable length outright
            // costs 433 KiB and saves boxing every control-path access.
            modulation: (0..MAX_CHANNELS).map(|_| ModRack::default()).collect(),
            modulators: (0..MAX_CHANNELS).map(|_| ModulatorRack::new()).collect(),
            control_outputs: Vec::with_capacity(MAX_CHANNELS),
            gate_ticks: Box::new([[NoteGateEvents::default(); MAX_CHANNELS];
                MAX_CONTROL_TICKS_PER_BLOCK]),
            sample_rate,
            reclaim: Vec::with_capacity(MAX_EFFECTS_PER_CHANNEL),
            meters: BusMeters::new(),
            device_meters: DeviceMeters::new(),
            device_telemetry: DeviceTelemetry::new(),
            buffer_midi: None,
            buffer_cc: BufferCcState::default(),
            keyboard_channel: Arc::new(AtomicU8::new(NO_KEYBOARD_CHANNEL)),
            midi_routing: Box::new(MidiRouting::default()),
            claimed_notes: Box::default(),
            audio_input_routing: Box::new(AudioInputRouting::default()),
            input: StereoBus::with_capacity(MAX_BLOCK_SIZE),
            input_dirty: false,
            monitor: [false; MAX_CHANNELS],
            held_keys: HeldKeys::new(),
            expression: [ChannelExpression::REST; MAX_CHANNELS],
            key_pressure: [0; 128],
            record_armed: false,
            recording: [None; 128],
            outgoing: [None; MAX_OUTGOING_EVENTS_PER_BLOCK],
            playhead_meters: PlayheadMeters::new(),
            modulator_meters: ModulatorMeters::new(),
            auditions: [None; MAX_AUDITIONS_PER_BLOCK],
            loop_range: LoopRange::default(),
            seeked: false,
            panicked: false,
            deferred: [None; MAX_DEFERRED],
            preview: None,
            preview_fading: None,
            preview_retired: RetiredPreviews::new(),
            preview_gain: Arc::new(AtomicU32::new(mooloop_core::gain::db_to_linear(mooloop_core::gain::REFERENCE_PEAK_DBFS).to_bits())),
            output_guard: OutputGuard::new(sample_rate),
            output_non_finite: 0,
            output_overs: 0,
        };
        // The sequencer starts with one channel, so the graph starts with
        // storage for one. `live_channels` tolerates the two disagreeing, but
        // they should not disagree at rest.
        let initial = state.sequencer.active_channels();
        state.grow_channels(initial);
        // The preview reads through the shared sinc table; build it here, on
        // the control thread, so no audition is the first to touch it.
        SincTable::shared();
        state
    }

    /// Point bus metering at the array the GUI reads. Called once at startup,
    /// before the realtime thread exists.
    pub(crate) fn attach_meters(&mut self, meters: Arc<BusMeters>) {
        self.meters = meters;
    }

    pub(crate) fn attach_device_meters(&mut self, meters: Arc<DeviceMeters>) {
        self.device_meters = meters;
    }

    /// Points the preview voice at the gain cell the GUI's volume knob
    /// writes. Read once per block, so knob turns are heard live.
    pub(crate) fn attach_preview_gain(&mut self, gain: Arc<AtomicU32>) {
        self.preview_gain = gain;
    }

    /// Starts, restarts, or stops the preview voice. The voice it displaces
    /// fades out rather than stopping dead, and retires through the ring when
    /// the fade ends. Returns the sample of a voice that was *already* fading
    /// -- a third audition inside two milliseconds cuts it the rest of the
    /// way -- for off-thread disposal.
    pub(crate) fn apply_preview(&mut self, command: PreviewCommand) -> Option<Arc<SampleData>> {
        let cut = self.preview_fading.take().map(|voice| voice.sample);
        if let Some(mut voice) = self.preview.take() {
            let length = (PREVIEW_FADE_S * f64::from(self.sample_rate)).round().max(1.0) as u32;
            voice.fade = Some((length, length));
            self.preview_fading = Some(voice);
        }
        match command {
            PreviewCommand::Play { sample } => {
                self.preview = Some(PreviewVoice::new(sample));
            }
            PreviewCommand::Stop => {}
        }
        cut
    }

    /// Hands back a sample whose preview finished, for disposal off the
    /// realtime thread.
    pub(crate) fn pop_retired_preview(&mut self) -> Option<Arc<SampleData>> {
        self.preview_retired.pop()
    }

    /// Hands back a sample a channel's sampler let go of on a note-on, for
    /// disposal off the realtime thread. Every materialised strip, not just
    /// the live ones: a strip the project stopped using can still be holding
    /// what it displaced before the ring had room.
    ///
    /// Only a strip running the sampler can be holding any. One whose sampler
    /// was replaced took what it held with it, through the reclaim ring.
    pub(crate) fn pop_retired_sampler_audio(&mut self) -> Option<mooloop_dsp::RetiredAudio> {
        self.strips
            .iter_mut()
            .find_map(|strip| strip.source.as_sampler_mut().and_then(Sampler::pop_retired))
    }


    /// Sums the preview voice into the master bus. Deliberately after the
    /// bus walk: the preview bypasses the project's chains, balance, and
    /// mute so the file is heard as the file.
    fn render_preview(&mut self, frames: usize) {
        if self.preview.is_none() && self.preview_fading.is_none() {
            return;
        }
        let gain = f32::from_bits(self.preview_gain.load(Ordering::Relaxed));
        let master = &mut self.buses[MASTER_BUS as usize];
        // The preview writes into the master after the bus walk has already
        // decided whether to empty it, so it has to say that it did: a master
        // left holding the last frames of a retired preview would keep
        // playing them for as long as nothing else routed to it.
        master.dirty = true;
        for slot in [&mut self.preview, &mut self.preview_fading] {
            let Some(voice) = slot.as_mut() else {
                continue;
            };
            if !voice.render(&mut master.bus, frames, gain, self.sample_rate) {
                continue;
            }
            // Finished. Retire it if there is room, and otherwise leave the
            // voice where it is and try again next block: it is past its end,
            // so it sums nothing further. Holding costs a comparison; the
            // alternatives are a `Vec` growing here or an `Arc` freed here,
            // and this is the realtime callback.
            let voice = slot.take().expect("voice checked above");
            if let Some(returned) = self.preview_retired.push(voice.sample) {
                *slot = Some(PreviewVoice {
                    sample: returned,
                    ..voice
                });
            }
        }
    }

    pub(crate) fn attach_device_telemetry(&mut self, telemetry: Arc<DeviceTelemetry>) {
        self.device_telemetry = telemetry;
    }

    pub(crate) fn attach_playhead_meters(&mut self, meters: Arc<PlayheadMeters>) {
        self.playhead_meters = meters;
    }

    pub(crate) fn attach_modulator_meters(&mut self, meters: Arc<ModulatorMeters>) {
        self.modulator_meters = meters;
    }

    pub fn from_project(
        sample_rate: u32,
        project: &Project,
        samples: &[Option<Arc<SampleData>>],
    ) -> Self {
        let fallback = SampleData::default_kick(sample_rate);
        let slots: ChannelAudioBank = Arc::new(
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
                    // A committed stretch is baked here too, from the same
                    // spec the editor uses. `samples` carries sources -- that
                    // is what a project's assets are -- so without this an
                    // export would play the unstretched original while the
                    // app plays the render.
                    let sample = sample.map(|sample| {
                        match project
                            .channels
                            .get(index)
                            .and_then(|channel| channel.setup.source.sampler_state())
                            .and_then(|state| state.commit.as_ref())
                            .and_then(|commit| {
                                mooloop_dsp::commit::rerender_commit(&sample, commit)
                            }) {
                            Some(rendered) => rendered,
                            None => sample,
                        }
                    });
                    // Slice maps travel with the project, not with `samples`.
                    // Omitting them made every note in a sliced channel
                    // resolve out of range, so an exported mix was silent
                    // exactly where the app was not. They are built in this
                    // same pass now, which is what makes that omission
                    // unrepresentable rather than merely fixed.
                    let slices = project
                        .channels
                        .get(index)
                        .and_then(|channel| channel.setup.source.sampler_state())
                        .map(|state| state.slices.clone())
                        .filter(|slices| !slices.is_empty())
                        .map(Arc::new);
                    let audio = ChannelAudioSnapshot { sample, slices };
                    Arc::new(ArcSwapOption::from(
                        (!audio.is_empty()).then(|| Arc::new(audio)),
                    ))
                })
                .collect(),
        );
        let mut state = Self::new(sample_rate, slots);
        state.load_project(project);
        state
    }

    /// Build one channel's storage. Allocates, so it belongs on the control
    /// thread — either here during a project install, or in the `AddChannel`
    /// structural command that carries the result across.
    ///
    /// The strip arrives running `source` at its defaults, built here too:
    /// an added channel's instrument is part of what has to be allocated off
    /// the audio thread.
    pub(crate) fn build_channel(
        audio_slot: ChannelAudioSlot,
        source: DeviceKind,
        sample_rate: u32,
    ) -> Box<ChannelStorage> {
        let source = build_source(&source.default_generator_params(), audio_slot, sample_rate);
        Box::new(ChannelStorage {
            strip: Box::new(ChannelStrip::new(source, sample_rate)),
            events: Box::new(EventList::empty()),
            control_outputs: Box::new(
                [[0.0; MAX_MODULATORS_PER_CHANNEL]; MAX_CONTROL_TICKS_PER_BLOCK],
            ),
            source_curves: Box::new(SourceCurvePool::empty()),
        })
    }

    /// Take one channel's storage into the graph's parallel vectors. The box
    /// is the transport, not an optimisation: it was allocated on the control
    /// thread and the audio thread only ever moves what is inside it.
    #[allow(clippy::boxed_local)]
    fn push_channel(&mut self, storage: Box<ChannelStorage>) {
        let ChannelStorage {
            strip,
            events,
            control_outputs,
            source_curves,
        } = *storage;
        self.strips.push(strip);
        self.events.push(events);
        self.control_outputs.push(control_outputs);
        self.source_curves.push(source_curves);
    }

    /// Channels that both exist to the sequencer and have storage behind
    /// them. Those two can disagree for one block — a project can declare
    /// more channels than the graph has been handed storage for — and a
    /// per-channel pass must render the ones it has rather than panic on the
    /// ones it does not.
    fn live_channels(&self) -> usize {
        self.sequencer.active_channels().min(self.strips.len())
    }

    /// Materialize channels up to `count`. Allocates; control thread only.
    /// Materialise track strips up to `count`, the way [`Self::grow_channels`]
    /// does for channels. The graph only grows: a project with fewer tracks
    /// than the last one keeps the spare strips rather than freeing them,
    /// since this may run while a state is being prepared for a swap and the
    /// displaced one is dropped on the control thread anyway.
    fn grow_buses(&mut self, count: usize) {
        while self.buses.len() < count.min(MAX_BUSES) {
            self.buses.push(BusStrip::new(self.sample_rate));
        }
    }

    fn grow_channels(&mut self, count: usize) {
        let sample_rate = self.sample_rate;
        while self.strips.len() < count.min(MAX_CHANNELS) {
            let slot = self.audio_slots[self.strips.len()].clone();
            self.push_channel(Self::build_channel(slot, DeviceKind::Sampler, sample_rate));
        }
    }

    /// Take the running transport of the renderer this one is replacing.
    ///
    /// Called by the executor at the moment of the swap, for a structural
    /// edit; see [`crate::PreparedProject::keep_transport`]. Not part of
    /// `load_project`, which runs on the control thread where the answer is
    /// not yet knowable.
    ///
    /// **Voices, tails and delay lines are still cut**, because the incoming
    /// renderer is a fresh graph. The song keeps its place and its clock; what
    /// was ringing at the moment of the edit is not carried across.
    /// `docs/plans/archive/channel-identity/05-strips-by-id.md` is where that goes.
    /// Move the strips listed in `carry` out of `outgoing` and into this
    /// state, so a chain that did not change keeps the node that is making
    /// its sound.
    ///
    /// **This is the whole of what stops the dropout.** A voice, a delay
    /// line, a reverb tail and a compressor's envelope all live inside a
    /// `ChannelStrip`, and until now every install built new ones and let the
    /// old ones leave with the retired generation -- so any structural edit
    /// silenced every channel in the song, including the ones it never
    /// touched (`every_install_silences_every_voice_until_strips_are_kept`).
    ///
    /// `carry` is `(outgoing index, incoming index)` and is decided on the
    /// **control thread**, which is the only place both projects are
    /// available. All this does is swap boxes: the live strip comes here, the
    /// freshly built one goes back in its place and leaves with the retired
    /// state to be freed off-thread. No allocation, no comparison, no
    /// reasoning on the audio thread.
    ///
    /// `carry` is borrowed here and freed nowhere near here: the executor
    /// sends the plan out on the reclaim ring alongside the generation it
    /// retired, so its four `Vec`s are destroyed on the control thread.
    ///
    /// **The compensation ring is decided by its length**, and that is not a
    /// detail. Every other setting on a strip comes from its own channel's
    /// setup, which is equal by construction or the pair would not be in
    /// `carry`. Compensation does not: `install_compensation` derives it from
    /// the *whole* project, so a channel that did not change can still be
    /// owed a different delay because some other channel altered the longest
    /// path into their shared bus. When it is owed a different one, the fresh
    /// ring goes onto the carried strip and the stale one leaves; when it is
    /// owed the same, the live ring stays, full, rather than restarting the
    /// path from silence -- [`keep_live_ring`]. The sends' rings follow the
    /// same rule, matched through `carry`'s seat maps.
    pub fn carry_strips_from(&mut self, outgoing: &mut Self, carry: &crate::CarryPlan) {
        for &(from, to) in carry.channels.iter().chain(&carry.rechained_channels) {
            let (from, to) = (usize::from(from), usize::from(to));
            let Some(live) = outgoing.strips.get_mut(from) else {
                continue;
            };
            let Some(fresh) = self.strips.get_mut(to) else {
                continue;
            };
            // **Only a strip running the instrument it was built for.** The
            // plan compares the incoming project with the last one
            // *installed*, and a source change is not an install: it moves
            // the live strip's node on and leaves that comparison behind. A
            // channel switched away and back between two installs compares
            // equal while the live strip may hold another kind, and carrying
            // it would play the wrong instrument -- a sampler bound to the
            // retired generation's audio slot, or no sampler to rebind.
            // Leaving the fresh one is a rebuild, which is the safe direction
            // every other stale comparison already fails in.
            if live.source.kind() != fresh.source.kind() {
                continue;
            }
            std::mem::swap(live, fresh);
            // Carried although its chain changed (MOO-137): the incoming
            // chain goes onto the carried strip, and the live one stays
            // behind for the devices below to be taken out of.
            if carry.rechained_channels.contains(&(from as u8, to as u8)) {
                std::mem::swap(&mut fresh.effects, &mut live.effects);
            }
            // **The audio slot has to come across too.** Every install builds
            // a fresh `ChannelAudioBank`, and a strip binds to its slot at
            // construction -- so a carried strip is still reading the retired
            // generation's slot, at the index it used to occupy. It sounds
            // right immediately, because the sample it is playing has not
            // changed; what breaks is everything published *afterwards*. The
            // handle addresses the new bank, so a sample loaded onto that
            // channel would land somewhere the strip never looks and the
            // channel would go on playing the old file with nothing to say
            // why. Both are samplers or neither is: the kinds matched above.
            if let (Some(carried), Some(built)) =
                (fresh.source.as_sampler_mut(), live.source.as_sampler_mut())
            {
                carried.swap_audio_slot_with(built);
            }
            // `fresh` now holds the carried strip and `live` the one built
            // for it. The carried strip has the live ring; give it the fresh
            // one only if the delay it is owed changed.
            if ring_frames(&fresh.compensation) != ring_frames(&live.compensation) {
                std::mem::swap(&mut fresh.compensation, &mut live.compensation);
            }
            // The same for the track it feeds: `carry_plan` ignores the bus
            // so that a track move can carry the channels routed to it, and
            // a carried strip would otherwise go on summing into the seat its
            // track used to have.
            std::mem::swap(&mut fresh.destination, &mut live.destination);
            // And the solo verdict, for the reason the compensation ring
            // above is decided by length: it is derived from the *whole*
            // bank, so a channel whose own setup did not change can still be
            // owed a different answer because somebody else pressed solo.
            // The fresh strip holds what `install_solo` just derived.
            std::mem::swap(&mut fresh.solo_silenced, &mut live.solo_silenced);
            // The modulator rack is the other half of "still sounding": a
            // free-running LFO that restarted its phase would step every
            // destination it drives at the moment of an unrelated edit. Its
            // *configuration* is equal -- the channel's setup matched -- so
            // only the phase travels.
            if let (Some(live_rack), Some(fresh_rack)) = (
                outgoing.modulators.get_mut(from),
                self.modulators.get_mut(to),
            ) {
                std::mem::swap(live_rack, fresh_rack);
            }
            // The voices came across with the strip, and so did the table
            // naming them. One whose note the incoming song no longer has,
            // or has at another pitch, will never see its note-off (MOO-99).
            let sequencer = &self.sequencer;
            fresh.sequenced.release_where(|voice| {
                sequencer
                    .note(usize::from(voice.origin.pattern), to, voice.note_id())
                    .is_none_or(|note| note.note != voice.note)
            });
        }
        // A track's live strip, for a track whose id and setup survived: its
        // chain, its channel strip's filter and envelope state, its fader's
        // smoothing and its tails. `incremental-structure/02`.
        //
        // What does not come across is everything `load_project` derives from
        // the *whole* graph rather than from this track's own setup -- the
        // compensation ring, the solo verdict and the console accumulator --
        // for the reason a channel's compensation stays behind above: an
        // untouched track can still be owed a different answer because the
        // graph around it moved.
        for &(from, to) in carry.tracks.iter().chain(&carry.rechained_tracks) {
            let (from, to) = (usize::from(from), usize::from(to));
            let (Some(live), Some(fresh)) = (outgoing.buses.get_mut(from), self.buses.get_mut(to))
            else {
                continue;
            };
            std::mem::swap(live, fresh);
            if carry.rechained_tracks.contains(&(from as u8, to as u8)) {
                std::mem::swap(&mut fresh.effects, &mut live.effects);
            }
            if ring_frames(&fresh.compensation) != ring_frames(&live.compensation) {
                std::mem::swap(&mut fresh.compensation, &mut live.compensation);
            }
            std::mem::swap(&mut fresh.solo_silenced, &mut live.solo_silenced);
            std::mem::swap(&mut fresh.console, &mut live.console);
            std::mem::swap(&mut fresh.console_sum, &mut live.console_sum);
            std::mem::swap(&mut fresh.console_dirty, &mut live.console_dirty);
        }
        // The sends are compiled whole from the incoming project, so their
        // rings are matched by edge rather than carried with a strip.
        self.sends.adopt_rings_from(&mut outgoing.sends, |seat| carry.seat(seat));
        // **The devices that did not change** (MOO-137). Rebuilt strip or
        // carried one -- which the loops above have already given the
        // incoming chain -- every device the edit left alone moves from the
        // live chain into its incoming row, and the node built for that row
        // leaves with the retired generation in its place.
        //
        // On a strip that kept sounding, every device that did *not* come
        // across is new to the sound, and fades in along the bypass
        // crossfade the way an installed one does (MOO-172) -- an undo that
        // puts a removed device back brings it in rather than switching it
        // in. A rebuilt strip starts from silence, so its devices need not.
        for chain in &carry.effects {
            let kept_sounding = match (chain.from, chain.to) {
                (EffectTarget::Channel(from), EffectTarget::Channel(to)) => carry
                    .rechained_channels
                    .contains(&(from, to))
                    && self
                        .strips
                        .get(usize::from(to))
                        .zip(outgoing.strips.get(usize::from(from)))
                        .is_some_and(|(carried, left)| carried.source.kind() == left.source.kind()),
                (EffectTarget::Bus(from), EffectTarget::Bus(to)) => {
                    carry.rechained_tracks.contains(&(from, to))
                }
                _ => false,
            };
            let (Some(incoming), Some(live)) = (
                Self::chain_for(&mut self.strips, &mut self.buses, chain.to),
                Self::chain_for(&mut outgoing.strips, &mut outgoing.buses, chain.from),
            ) else {
                continue;
            };
            incoming.adopt_devices_from(live, &chain.rows, kept_sounding);
        }
    }

    /// The identity of the audio slot in this generation's bank at `index`,
    /// and the one the strip there is actually reading. Two answers that must
    /// agree; see `a_carried_strip_reads_the_new_generations_audio_slot`.
    #[cfg(test)]
    pub(crate) fn audio_slot_ptr(&self, index: usize) -> usize {
        Arc::as_ptr(&self.audio_slots[index]) as usize
    }

    #[cfg(test)]
    pub(crate) fn strip_audio_slot_ptr(&self, index: usize) -> usize {
        self.strips[index]
            .source
            .as_sampler()
            .map_or(0, Sampler::audio_slot_ptr)
    }

    /// The node `channel` is running, for tests that ask what it holds.
    #[cfg(test)]
    pub(crate) fn channel_source(&self, channel: usize) -> &dyn SourceNode {
        self.strips[channel].source_node()
    }

    /// Build `params`' device for `channel`, reading this generation's audio
    /// slot for it -- what `EngineHandle::set_channel_source` builds, for the
    /// tests that drive a `RenderState` without a handle.
    #[cfg(test)]
    pub(crate) fn build_source_for(
        &self,
        channel: usize,
        params: &GeneratorParams,
    ) -> Box<dyn SourceNode + Send> {
        build_source(params, self.audio_slots[channel].clone(), self.sample_rate)
    }

    #[cfg(test)]
    pub(crate) fn strip_destination(&self, index: usize) -> u8 {
        self.strips[index].destination
    }

    #[cfg(test)]
    pub(crate) fn track_solo_silenced(&self, index: usize) -> bool {
        self.buses[index].solo_silenced
    }

    #[cfg(test)]
    pub(crate) fn channel_solo_silenced(&self, index: usize) -> bool {
        self.strips[index].solo_silenced
    }

    /// A channel's *own* mute, as opposed to the solo verdict above. Two
    /// answers because they are two fields, which is the whole point of
    /// holding them apart.
    #[cfg(test)]
    pub(crate) fn channel_muted(&self, index: usize) -> bool {
        self.strips[index].output.muted
    }

    /// Take the outgoing renderer's position-in-time state: where the song is,
    /// which keys are down, and which notes a take has open.
    ///
    /// **Everything whose value is only correct at the instant of the switch
    /// belongs here**, and that is the rule this function exists to hold. The
    /// alternative is `InputState`, a hand-maintained list prepared ahead of
    /// the swap on the control thread, which has needed patching three times
    /// in a month -- record arm, then routing twice. A field that only the
    /// swap can fill cannot be prepared early by definition, so putting it
    /// there is the wrong mechanism as well as the wrong moment.
    ///
    /// All three are fixed-size and `Copy`. Nothing allocates, so this is
    /// callable from the audio thread, which is where it runs.
    ///
    /// `held_keys` and `recording` were carried by nothing until 2026-09-18.
    /// The loss was invisible while a structural edit stopped the song -- the
    /// old voices went with the old renderer anyway -- and `04-keep-the-transport`
    /// is what made it audible: once the transport survives the install, a key
    /// held across a channel move has its note-off delivered to a renderer
    /// that never saw the press, so the note never lifts. A note captured but
    /// not yet closed was dropped from the take the same way.
    pub fn adopt_performance_state(&mut self, outgoing: &Self) {
        self.transport.adopt_running_state(outgoing.transport());
        self.held_keys = outgoing.held_keys;
        self.recording = outgoing.recording;
        // A wheel held across an install is still held. Re-sent rather than
        // assumed: a strip the install rebuilt starts at rest.
        self.expression = outgoing.expression;
        self.key_pressure = outgoing.key_pressure;
        for expression in &mut self.expression {
            if expression.bend != 0.0 {
                expression.bend_due = Some(0);
            }
        }
        // The limiter's envelope, so an install in the middle of an over does
        // not jump the output back to unity for the release it still owed.
        self.output_guard.adopt(&outgoing.output_guard);
        // Carried, not re-sent -- the lesson `incremental-structure/` paid
        // for. The control thread has no way to know an install happened
        // between its send and the edge, so a pending command left behind
        // here would simply never land.
        self.deferred = outgoing.deferred;
    }

    /// Whether `note` is held on `channel`, for the tests that assert what a
    /// swap did to the performance state.
    #[cfg(test)]
    pub(crate) fn key_is_held(&self, note: u8, channel: u8) -> bool {
        self.held_keys.is_held(note, channel)
    }

    #[cfg(test)]
    pub(crate) fn any_key_is_held(&self) -> bool {
        self.held_keys.any_held()
    }

    /// Whether a take has an open note at `note`, and on which channel.
    #[cfg(test)]
    pub(crate) fn capturing(&self, note: u8) -> Option<u8> {
        self.recording[usize::from(note & 0x7f)].map(|note| note.channel)
    }

    /// The transport this renderer is running, for the executor -- which owns
    /// this state and has to read the outgoing one at a swap -- and for the
    /// tests that assert what a swap did to it.
    pub(crate) fn transport(&self) -> &crate::transport::Transport {
        &self.transport
    }

    pub fn load_project(&mut self, project: &Project) {
        // A loaded project decides how many channels exist. This runs on the
        // control thread inside `install_project`, so allocating here is the
        // point rather than a hazard.
        self.grow_channels(project.channels.len());
        self.grow_buses(project.buses.len());
        self.transport.stop();
        self.transport.set_tempo(project.bpm.into());
        self.loop_range = project.loop_range;
        self.sequencer.load_project(project);
        for (index, strip) in self.strips.iter_mut().enumerate() {
            let audio_slot = self.audio_slots[index].clone();
            if let Some(channel) = project.channels.get(index) {
                // Dropped here, on the control thread, like everything else
                // this function displaces.
                drop(strip.load_source(&channel.setup.source, audio_slot, self.sample_rate));
                strip.output.muted = channel.setup.channel.muted;
                strip.output.set_volume(channel.setup.channel.volume);
                strip.output.set_pan(channel.setup.channel.pan);
                // A channel naming a track that is not in the bank feeds the
                // master rather than nothing: `clamp_bus` bounds by the
                // address space, which is not the same as the bank being that
                // long, and a silently unheard channel is the worst of the
                // available answers.
                let destination = clamp_bus(channel.setup.channel.bus);
                strip.destination = if (destination as usize) < project.buses.len().max(1) {
                    destination
                } else {
                    MASTER_BUS
                };
                // `load_project` runs while a complete RenderState is prepared
                // on the control thread (or for offline export), never from the
                // JACK callback, so constructing boxed nodes is acceptable.
                // Displaced nodes still collect in `reclaim` for callers that
                // deliberately reuse a state off-thread.
                strip.effects.load(
                    &channel.setup.effects,
                    self.sample_rate,
                    self.transport.bpm,
                    &mut self.reclaim,
                );
            } else {
                // A strip past the end of the project is a spare, and goes
                // back to a fresh sampler channel.
                strip.reset_slot(&mut self.reclaim);
                drop(strip.install_source(build_source(
                    &DeviceKind::Sampler.default_generator_params(),
                    audio_slot,
                    self.sample_rate,
                )));
            }
        }
        for index in 0..MAX_CHANNELS {
            let modulation = project
                .channels
                .get(index)
                .map(|channel| channel.setup.modulation)
                .unwrap_or_default();
            self.set_channel_modulation(index, modulation);
        }
        for (index, strip) in self.buses.iter_mut().enumerate() {
            match project.buses.get(index) {
                Some(setup) => {
                    strip.output.muted = setup.bus.muted;
                    strip.output.set_volume(setup.bus.volume);
                    strip.output.set_pan(setup.bus.pan);
                    strip.polarity = setup.bus.polarity;
                    // Reset before installing: a document arriving is the
                    // one moment there is nothing to be continuous with, and
                    // a state being reused -- an undo, or a project with
                    // fewer tracks than the last one -- would otherwise hand
                    // the new song a filter bank holding the old one's audio.
                    strip.strip.reset();
                    strip.strip.set_params(setup.bus.strip);
                    strip.effects.load(
                        &setup.effects,
                        self.sample_rate,
                        self.transport.bpm,
                        &mut self.reclaim,
                    );
                }
                None => strip.reset(&mut self.reclaim),
            }
        }
        // A file whose routing does not sort is repaired to everything-to-master
        // rather than rejected, so a hand-edited or future-format song still
        // opens and makes sound.
        self.bus_graph = compile_bus_graph(&project.buses).unwrap_or_default();
        self.install_compensation(project);
        self.install_console(project);
        self.install_solo(project);
        // Every output stage, polarity and send at the document's values from
        // the first sample, rather than ramping there from whatever the strip
        // held: a document arriving has nothing sounding to be continuous
        // with, and a ramp here would put the first milliseconds of every
        // bounce at the wrong level. After `install_solo`, because the solo
        // verdict is half of where a stage is aimed, and after
        // `install_compensation`, which builds the sends. A strip carried
        // across an install is swapped in after this and keeps its own
        // ramps, which is the point of carrying it.
        self.settle_mixer();
        // Here as well as through the session's incremental sync, and for the
        // same reason `install_compensation` is: an offline render builds its
        // own `RenderState` and never runs a pump, so without this an export
        // would be the one place the channels rendered in index order.
        *self.audio = AudioTapBank::new(project.audio_graph());
    }

    /// Jump every output stage, polarity and send level to where it is
    /// aimed, skipping the ramps MOO-107 put on them.
    ///
    /// For a document arriving, which is the one moment there is nothing
    /// sounding to be continuous with. Also what a test calls when it sets a
    /// mix up with commands and then measures it: the question there is the
    /// mix, not the few milliseconds of ramp between the project it loaded
    /// and the one it configured.
    pub(crate) fn settle_mixer(&mut self) {
        for (index, strip) in self.strips.iter_mut().enumerate() {
            let silenced = strip.output.muted || strip.solo_silenced;
            strip.output.aim(silenced);
            strip.output.settle();
            strip.effects.settle_ramps();
            self.sends
                .settle(EffectTarget::Channel(index as u8), silenced);
        }
        for (index, strip) in self.buses.iter_mut().enumerate() {
            strip.settle();
            strip.effects.settle_ramps();
            let silenced = strip.output.muted || strip.solo_silenced;
            self.sends.settle(EffectTarget::Bus(index as u8), silenced);
        }
    }

    /// Install the solo state from `project`.
    ///
    /// Here as well as through the session's incremental sync, for the
    /// reason [`Self::install_console`] gives: an **offline render** builds
    /// its own `RenderState` and never runs a pump, so without this a bounce
    /// would be the one place a solo was ignored -- and a bounce that does
    /// not match what was heard is the disagreement this engine is arranged
    /// to prevent.
    fn install_solo(&mut self, project: &Project) {
        let silenced = mooloop_core::mixer::solo_silenced(&project.buses);
        for (index, strip) in self.buses.iter_mut().enumerate() {
            strip.solo_silenced = silenced.get(index).copied().unwrap_or(false);
        }
        let silenced = mooloop_core::channel::solo_silenced(
            project.channels.iter().map(|channel| channel.setup.channel.solo),
        );
        for (index, strip) in self.strips.iter_mut().enumerate() {
            strip.solo_silenced = silenced.get(index).copied().unwrap_or(false);
        }
    }

    /// Install the console switches from `project`, and the second input
    /// accumulator for every bus something encoded actually reaches.
    ///
    /// Here as well as through the session's incremental sync, for the reason
    /// [`Self::install_compensation`] gives: an **offline render** builds its
    /// own `RenderState` and never runs a pump, so without this a bounce
    /// would be the one place console summing did not happen -- which is the
    /// export-versus-live disagreement everything in this engine is arranged
    /// to prevent, and it is one of step 02's acceptance cases.
    ///
    /// Allocates, and is allowed to: `load_project` runs on the control
    /// thread while a state is prepared, never from the callback.
    fn install_console(&mut self, project: &Project) {
        for (index, strip) in self.buses.iter_mut().enumerate() {
            // The master feeds nothing, so a switch on it would encode into a
            // sum that is never decoded. Refused here rather than hidden in
            // the UI, so a hand-edited file cannot make the master inaudible.
            strip.console = index != MASTER_BUS as usize
                && project.buses.get(index).is_some_and(|setup| setup.bus.console);
        }
        let mut wanted = [false; MAX_BUSES];
        for index in 1..self.buses.len().min(MAX_BUSES) {
            if self.buses[index].console {
                wanted[self.bus_graph.destination(index) as usize] = true;
            }
        }
        for (index, strip) in self.buses.iter_mut().enumerate() {
            match (wanted.get(index).copied().unwrap_or(false), strip.console_sum.is_some()) {
                (true, false) => {
                    strip.console_sum = Some(Box::new(StereoBus::with_capacity(MAX_BLOCK_SIZE)))
                }
                (false, true) => strip.console_sum = None,
                _ => {}
            }
            strip.console_dirty = false;
        }
    }

    /// Build and install the tree's latency compensation from `project`.
    ///
    /// Here as well as through the session's incremental sync, because this is
    /// the path an **offline render** takes: it builds its own `RenderState`
    /// and never runs a pump, so without this an export would be the one
    /// place the mixer was not time aligned — which is exactly the disagreement
    /// between offline and live that everything else in this engine is
    /// arranged to prevent.
    ///
    /// Allocates, and is allowed to: `load_project` runs on the control thread
    /// while a state is being prepared, never from the callback.
    fn install_compensation(&mut self, project: &Project) {
        let mut channel_latency = [0u32; MAX_CHANNELS];
        let mut channel_bus = [MASTER_BUS; MAX_CHANNELS];
        for (index, channel) in project.channels.iter().take(MAX_CHANNELS).enumerate() {
            channel_latency[index] = chain_latency(&channel.setup.effects);
            channel_bus[index] = channel.setup.channel.bus;
        }
        let mut bus_latency = [0u32; MAX_BUSES];
        for (index, bus) in project.buses.iter().take(MAX_BUSES).enumerate() {
            bus_latency[index] = chain_latency(&bus.effects);
        }
        // Why a non-sorting bank has no sends is written once, in
        // `mooloop_core::mixer`, because `Session::latency_plan` has to make
        // the same decision and for a while did not.
        let sorts = sends_are_compensable(&project.buses);
        let edges = compensable_send_edges(&project.buses);
        let plan = compile_latency(
            &self.bus_graph,
            &channel_latency,
            &channel_bus,
            &bus_latency,
            &edges,
        );
        for (index, strip) in self.strips.iter_mut().enumerate() {
            strip.compensation = IntegerDelay::new(plan.channel(index)).map(Box::new);
        }
        for (index, strip) in self.buses.iter_mut().enumerate() {
            strip.compensation = IntegerDelay::new(plan.bus(index)).map(Box::new);
        }
        // The sends belong to the same plan, so they are built from the same
        // pass rather than a second one that could disagree with it.
        let specs: Vec<SendSpec> = project
            .buses
            .iter()
            .take(if sorts { MAX_BUSES } else { 0 })
            .enumerate()
            .flat_map(|(index, setup)| {
                setup.sends.iter().map(move |send| (index, send))
            })
            .zip(0..)
            .map(|((index, send), edge)| SendSpec {
                producer: EffectTarget::Bus(index as u8),
                target: send.target,
                tap: send.tap,
                enabled: send.enabled,
                level: send.level,
                delay: plan.send(edge),
            })
            .collect();
        *self.sends = SendBank::new(&specs, self.sample_rate);
    }

    /// Resolve an effect address to the chain that owns it. Both arms are
    /// bounds-checked, so a stale index from the GUI is a no-op rather than a
    /// panic on the audio thread.
    fn chain_for<'a>(
        strips: &'a mut [Box<ChannelStrip>],
        buses: &'a mut [BusStrip],
        target: EffectTarget,
    ) -> Option<&'a mut EffectChain> {
        match target {
            EffectTarget::Channel(index) => strips.get_mut(index as usize).map(|s| &mut s.effects),
            EffectTarget::Bus(index) => buses.get_mut(index as usize).map(|b| &mut b.effects),
        }
    }

    fn chain_mut(&mut self, target: EffectTarget) -> Option<&mut EffectChain> {
        Self::chain_for(&mut self.strips, &mut self.buses, target)
    }

    /// Drop every route and every lane driving `device` in `target`, because
    /// that device has just been removed.
    ///
    /// All that is left of what used to run on every chain edit. A reorder is
    /// no longer an addressing event on either side: the devices move with
    /// their base values, event queues and host controls, and the addresses
    /// naming them were never positions to begin with.
    ///
    /// A channel's routes can only address that channel, so a channel edit
    /// touches one rack. A bus chain can be addressed from any channel's
    /// clip, so a bus edit walks them all -- a few thousand comparisons, on a
    /// gesture that happens by hand.
    fn forget_device(&mut self, target: EffectTarget, device: mooloop_core::DeviceId) {
        if !device.is_assigned() {
            return;
        }
        match target {
            EffectTarget::Channel(channel) => {
                if let Some(rack) = self.modulation.get_mut(channel as usize) {
                    rack.forget_device(target, device);
                }
            }
            EffectTarget::Bus(_) => {
                for rack in self.modulation.iter_mut() {
                    rack.forget_device(target, device);
                }
            }
        }
        self.sequencer.forget_device(target, device);
    }

    fn chain(&self, target: EffectTarget) -> Option<&EffectChain> {
        match target {
            EffectTarget::Channel(index) => self.strips.get(index as usize).map(|s| &s.effects),
            EffectTarget::Bus(index) => self.buses.get(index as usize).map(|b| &b.effects),
        }
    }

    /// Install a complete saved rack while retaining a same-kind LFO's phase.
    /// Copying the small matrix is realtime-safe; `ModulatorRack::set_slot`
    /// owns the phase-preserving detail.
    /// Replace one channel's whole rack. Used where a rack genuinely arrives
    /// entire — project load, and a channel added or removed — not for
    /// ordinary edits, which name one fact each through `edit_modulation`.
    fn set_channel_modulation(&mut self, channel: usize, modulation: ModRack) {
        self.edit_modulation(channel, |rack| {
            *rack = modulation;
            true
        });
    }

    /// Apply one edit to a channel's rack and put everything that depends on
    /// it back in step.
    ///
    /// Every modulation command is this shape: change one fact in the saved
    /// rack, mirror the slots whose behaviour actually moved into the DSP
    /// rack, and hand back any destination that just lost its last route.
    /// The diff lives here rather than at each call site because it is the
    /// only part a narrow command can get *wrong* rather than merely
    /// expensive: without it, a removed route leaves the device holding
    /// whatever the control signal last resolved, until someone happens to
    /// touch that knob again.
    ///
    /// `edit` reports whether it changed anything, so a command that names a
    /// slot or a route this rack does not hold costs a comparison and stops.
    /// Reorder one channel's modulator grid, carrying every module's running
    /// state to its new slot.
    ///
    /// Its own handler rather than an `edit_modulation` closure, because
    /// `edit_modulation` mirrors an edit as a **params diff by slot number**
    /// and a reorder is exactly what that cannot see: every moved position
    /// reads as "the params changed". Different kinds swapped were rebuilt
    /// from scratch -- an envelope dragged to the front restarted at level 0
    /// stage `Idle`, dropping a held note's contour to zero mid-sustain, and a
    /// Random module was reseeded, so a realtime take and an offline render of
    /// the same song stopped matching. Same kinds **cross-wired**, because
    /// `set_slot` retunes in place: two LFOs dragged past each other kept
    /// their own phase, smoothing and fade position and took the other's
    /// params, so both jumped and nothing said why.
    ///
    /// The diff is what makes every *other* narrow command cheap, so it stays;
    /// this is the one edit that owes it a permutation instead.
    ///
    /// The params pass afterwards is not belt and braces. `retarget` may
    /// rewrite a Math module's `input_slot`, which lives in its params, so the
    /// permuted runtime can be holding a module whose params moved underneath
    /// it. Comparing against the *permuted* previous params is what makes that
    /// the only thing it touches -- a `MathSource` is its params and rebuilds
    /// for nothing, where rebuilding an LFO is the defect above.
    fn move_modulator(&mut self, channel: usize, from: usize, to: usize) {
        let Some(saved) = self.modulation.get_mut(channel) else {
            return;
        };
        let before = saved.slots;
        let Some(remap) = saved.move_module_mapped(from, to) else {
            return;
        };
        let after = saved.slots;
        let Some(runtime) = self.modulators.get_mut(channel) else {
            return;
        };
        runtime.permute(&remap);

        // What the runtime now holds: the old params, in their new places.
        let mut carried = [None; MAX_MODULATORS_PER_CHANNEL];
        for (old, new) in remap.iter().enumerate() {
            let new = *new as usize;
            if new < MAX_MODULATORS_PER_CHANNEL {
                carried[new] = before[old].map(|entry| entry.params);
            }
        }
        for (slot, carried) in carried.into_iter().enumerate() {
            let params = after[slot].map(|entry| entry.params);
            if carried == params {
                continue;
            }
            runtime.set_slot(slot, params);
        }
    }

    fn edit_modulation(&mut self, channel: usize, edit: impl FnOnce(&mut ModRack) -> bool) {
        let Some(saved) = self.modulation.get_mut(channel) else {
            return;
        };
        let previous = *saved;
        if !edit(saved) {
            return;
        }
        let modulation = *saved;
        // The runtime rack holds behaviour, not identity: it is addressed by
        // slot, and the durable ids stay on the control side where routes are
        // resolved. Only a slot whose parameters moved is re-set, so turning
        // one knob does not touch the seven modules beside it -- and a module
        // that keeps its kind keeps its phase, cursor and envelope stage
        // because `set_slot` retunes in place.
        if let Some(runtime) = self.modulators.get_mut(channel) {
            for (slot, entry) in modulation.slots.into_iter().enumerate() {
                let params = entry.map(|entry| entry.params);
                if previous.slots[slot].map(|entry| entry.params) == params {
                    continue;
                }
                runtime.set_slot(slot, params);
            }
        }
        for destination in previous.destinations() {
            if modulation
                .destinations()
                .any(|current| current == destination)
            {
                continue;
            }
            self.restore_base_param(destination);
        }
    }

    /// Where the automation pass is reading right now.
    fn automation_position(&self) -> AutomationPosition {
        AutomationPosition {
            mode: self.sequencer.playback_mode(),
            pattern: self.sequencer.current_pattern(),
            tick: self.transport.position_ticks,
        }
    }

    /// Hand back every destination the outgoing playhead was driving and the
    /// incoming one is not.
    ///
    /// [`Self::restore_base_param`]'s own comment names this hazard, and it
    /// was called only when a lane was *deleted* or *cleared*. A lane that
    /// merely stops covering the playhead does exactly the same thing:
    /// pattern 1 sweeps a cutoff down to 200 Hz, pattern 2 has no such lane,
    /// and after the switch `has_automation_at` answers false,
    /// `control_events_for_slot` takes its early return, and the filter plays
    /// at 200 Hz while its knob and its face both read 1 kHz until somebody
    /// touches it or reloads the song. It bites only destinations that are
    /// automated and *not* modulated -- a modulated one takes
    /// `base_normalized = knob_normalized` when the curve is `None` and so
    /// restores the knob every block by accident.
    ///
    /// **Only the commands that move the playhead**, which is the half a
    /// user can reach: `SetCurrentPattern`, `SetPlaybackMode` and `Seek`. A
    /// song-mode clip boundary is not a command and is still open in
    /// `LOOSE_ENDS.md`; closing it needs the engine to carry which
    /// destinations had a curve last block and no longer do, which is
    /// per-channel state across blocks on the audio thread and which
    /// `AutomationBlock` is explicitly a read-only view to avoid.
    ///
    /// Bounded by the **outgoing coverage** and not by the bank: one pattern
    /// in pattern mode, the covering placements in song mode, each times the
    /// active channels times `MAX_AUTOMATION_LANES_PER_CHANNEL`. Walking
    /// every active pattern instead would be 256 x 256 x 8 index lookups on
    /// a full project, on the audio thread, for a command that is rare.
    fn restore_lanes_left_behind(&mut self, from: AutomationPosition) {
        let to = self.transport.position_ticks;
        let channels = self.sequencer.active_channels();
        let mut ordinal = 0;
        while let Some(pattern) =
            self.sequencer
                .covering_pattern_at(from.mode, from.pattern, from.tick, ordinal)
        {
            for channel in 0..channels {
                for lane in 0..MAX_AUTOMATION_LANES_PER_CHANNEL {
                    let Some(target) =
                        self.sequencer.pattern_lane_destination(pattern, channel, lane)
                    else {
                        continue;
                    };
                    // Still covered after the move is the common case, and it
                    // must not be disturbed: writing the knob here would undo
                    // one block of a curve that is still playing.
                    if self.sequencer.automation_lane_at(target, to).is_none() {
                        self.restore_base_param(target);
                    }
                }
            }
            ordinal += 1;
        }
    }

    /// Return one destination to its knob value at the next block. Removing a
    /// lane or a matrix route otherwise leaves the device holding whatever the
    /// control signal last resolved, until someone happens to touch that knob.
    fn restore_base_param(&mut self, destination: ParamAddr) {
        match destination.owner {
            ParamOwner::Effect { device } => {
                let Some(chain) = self.chain_mut(destination.scope) else {
                    return;
                };
                let Some(slot) = chain.slot_of(device) else {
                    return;
                };
                if let Some(base) = chain.base_param(slot, destination.param) {
                    chain.queue_param(slot, destination.param, base);
                }
            }
            // A generator has no queue between blocks; its base is applied
            // directly, which is safe because `set_params` allocates nothing.
            ParamOwner::Source => {
                let EffectTarget::Channel(channel) = destination.scope else {
                    return;
                };
                let Some(strip) = self.strips.get_mut(channel as usize) else {
                    return;
                };
                strip.push_source_base();
            }
            // A route amount lives in the same parameter block as the rest of
            // the patch, so the base push that restores a generator knob
            // restores this too.
            ParamOwner::SourceRoute { .. } => {
                let EffectTarget::Channel(channel) = destination.scope else {
                    return;
                };
                let Some(strip) = self.strips.get_mut(channel as usize) else {
                    return;
                };
                strip.push_source_base();
            }
            // Neither needs one. The strip's output stage keeps no parameter
            // state between blocks -- it re-reads its knob every block -- and
            // a modulator's own parameters are not modulation destinations
            // yet, so there is nothing left holding a stale resolved value.
            ParamOwner::Modulator { .. } | ParamOwner::Strip => {}
            // Nothing resolves a plugin parameter in the control pass yet
            // (`docs/plans/plugin-hosting/`, step 07, and MOO-195), so there
            // is no resolved value to go stale.
            ParamOwner::PluginParam { .. } => {}
        }
    }

    /// Whether a lane or a route will resolve this effect parameter in the
    /// next block, and so whether writing the knob's base straight through
    /// would put a second, stale value in front of it. These are the two
    /// questions of the precedence table on
    /// `EffectChain::control_events_for_slot`, asked the way it asks them:
    ///
    /// - a lane counts when [`AutomationCurve::at`] -- which
    ///   `AutomationBlock::curve_for` also calls -- finds one at the position
    ///   the next block starts from. Playing or stopped does not matter;
    ///   lanes resolve either way. Channel and bus chains both carry lanes.
    /// - a route counts only on a channel, and only when the destination
    ///   accepts modulation: a route aimed at one that refuses it resolves to
    ///   nothing, so the knob must still reach the device.
    fn effect_is_driven(&self, target: EffectTarget, slot: u8, id: u32) -> bool {
        let Some(state) = self.chain(target).and_then(|chain| chain.slot(slot as usize)) else {
            return false;
        };
        let destination = ParamAddr::effect(target, state.device, id);
        let Some(descriptor) = state.kind.and_then(|kind| kind.descriptor(id)) else {
            return false;
        };
        if AutomationCurve::at(&self.sequencer, destination, self.transport.position_ticks)
            .is_some()
        {
            return true;
        }
        let EffectTarget::Channel(channel) = target else {
            return false;
        };
        let policy = ModDestinationDescriptor::for_param(descriptor);
        self.modulation
            .get(channel as usize)
            .is_some_and(|rack| rack.modulates(destination, &policy))
    }

    /// Change the stored base, then queue it for the next block only if
    /// neither a lane nor a route is about to resolve that destination -- the
    /// last row of `control_events_for_slot`'s precedence table.
    fn set_effect_param(&mut self, target: EffectTarget, slot: u8, id: u32, value: f32) {
        let Some(value) = self
            .chain_mut(target)
            .and_then(|chain| chain.set_base_param(slot as usize, id, value))
        else {
            return;
        };
        if !self.effect_is_driven(target, slot, id) {
            if let Some(chain) = self.chain_mut(target) {
                chain.queue_param(slot as usize, id, value);
            }
        }
    }

    /// Tick one channel's source rack for every 32-frame subdivision and
    /// capture each output before advancing it. The final subdivision can be
    /// shorter; its event still starts at its exact frame offset.
    ///
    /// Takes the two tables it advances rather than `&mut self`, because the
    /// gate table it reads is now a field too and only field-level borrows
    /// can see that the three are disjoint.
    #[allow(clippy::too_many_arguments)]
    fn tick_channel_modulators(
        modulators: &mut [ModulatorRack],
        control_outputs: &mut [Box<ControlOutputs>],
        sample_rate: u32,
        bpm: f64,
        channel: usize,
        frames: usize,
        gate_ticks: &GateTable,
        song_beats: &SongBeats,
    ) -> usize {
        let Some(runtime) = modulators.get_mut(channel) else {
            return 0;
        };
        let Some(outputs) = control_outputs.get_mut(channel) else {
            return 0;
        };

        let mut tick = 0;
        for offset in (0..frames).step_by(CONTROL_RATE_FRAMES) {
            let span = (frames - offset).min(CONTROL_RATE_FRAMES);
            runtime.tick_with_note_gates(
                sample_rate,
                span,
                bpm,
                song_beats[tick],
                channel,
                &gate_ticks[tick],
            );
            outputs[tick] = *runtime.outputs();
            tick += 1;
        }
        tick
    }

    /// The block path's call to [`Self::tick_channel_modulators`], reading the
    /// gate table the tests set up on the state itself. Only the borrow
    /// splitting differs; the arguments are the ones `process_block_inner`
    /// passes.
    #[cfg(test)]
    fn tick_modulators_from_gate_table(&mut self, channel: usize, frames: usize) -> usize {
        Self::tick_channel_modulators(
            &mut self.modulators,
            &mut self.control_outputs,
            self.sample_rate,
            self.transport.bpm,
            channel,
            frames,
            &self.gate_ticks,
            &[None; MAX_CONTROL_TICKS_PER_BLOCK],
        )
    }

    /// Hands back one effect-slot occupant a cleared chain displaced, for the
    /// executor to send down the reclaim ring. See the `reclaim` field.
    pub(crate) fn pop_displaced_effect(&mut self) -> Option<ReclaimedEffect> {
        self.reclaim.pop()
    }

    /// Whether displaced effect occupants are still waiting for the reclaim
    /// ring. The executor holds structural edits back while they are, which
    /// is what keeps the `reclaim` vector within its reservation.
    pub(crate) fn has_displaced_effects(&self) -> bool {
        !self.reclaim.is_empty()
    }

    /// Whether the effect in `slot` has faded out of the path and can be
    /// removed, or replaced by an install, without a step (MOO-108,
    /// MOO-172). A slot with nothing in it is vacated already.
    ///
    /// The first call starts the fade by marking the slot as leaving -- it
    /// is going anyway -- and the executor holds the removal or install, and
    /// everything behind it, until this says yes: about 35 ms. `bypassed`
    /// is left alone, so a device installed in its place inherits the
    /// user's bypass rather than the fade's. `frames` is the length of the
    /// block the executor is about to render, taken as the length of the
    /// one since it last asked. A slot the chain has stopped processing
    /// (a settled mute) cannot finish its fade, and is inaudible besides,
    /// so it goes after [`REMOVAL_MAX_WAIT_S`] regardless; one that never
    /// rendered at all goes at once.
    pub(crate) fn effect_slot_vacated(
        &mut self,
        target: EffectTarget,
        slot: u8,
        frames: usize,
    ) -> bool {
        let Some(chain) = self.chain_mut(target) else {
            return true;
        };
        // A row inside a box that is out of the path is not heard, however
        // far its own fade has got: that is how a container preset clears
        // the run it replaces (MOO-172).
        if chain.inside_silent_container(slot as usize) {
            return true;
        }
        // A row inside a box already on its way out waits for the box
        // rather than fading on its own: two fades at once, the row's inside
        // the box's equal-power Mix, swell the level by up to 3 dB on the
        // way down. The wait is counted on the box, which is leaving anyway.
        let owner = chain
            .enclosing_leaving_container(slot as usize)
            .unwrap_or(slot as usize);
        let Some(state) = chain.slot_mut(owner) else {
            return true;
        };
        if state.out_of_path() {
            return true;
        }
        let waited = state
            .removal_waited
            .map_or(0, |waited| waited.saturating_add(frames as u32));
        state.removal_waited = Some(waited);
        // A slot that has never rendered has a rate of zero here, and goes
        // at once: nothing has heard it.
        let limit = (REMOVAL_MAX_WAIT_S * state.ramps.sample_rate as f32) as u32;
        waited >= limit
    }

    /// Apply a structural change (install/remove of a boxed node). Called on
    /// the realtime thread from the ordered control stream; the boxes
    /// themselves were allocated on the control thread. Returns whatever the
    /// edit displaced, so the caller can hand it to the reclaim ring.
    /// Apply a structural edit, returning whatever it displaced so the caller
    /// can send it back for off-thread disposal. Returns the reclaim variant
    /// rather than a bare effect, because not everything structural is an
    /// effect any more.
    pub(crate) fn apply_structural(
        &mut self,
        cmd: StructuralCommand,
    ) -> Option<StructuralReclaim> {
        match cmd {
            StructuralCommand::InstallEffect {
                target,
                slot,
                kind,
                resource_key,
                node,
                align,
                analyzer,
                state,
            } => {
                // `chain_for` borrows the two strip vectors rather than all of
                // `self`, so `reclaim` stays independently borrowable here.
                if let Some(chain) = Self::chain_for(&mut self.strips, &mut self.buses, target) {
                    Some(chain.install(
                        slot as usize,
                        kind,
                        resource_key,
                        node,
                        align,
                        analyzer,
                        state,
                    ))
                } else {
                    Some(ReclaimedEffect {
                        node: Some(node),
                        align,
                        analyzer: Some(analyzer),
                        state: Some(state),
                        channel: None,
                    })
                }
                .filter(|displaced| !displaced.is_empty())
                .map(StructuralReclaim::Effect)
            }
            StructuralCommand::ReplaceEffect {
                target,
                slot,
                expected_kind,
                expected_resource_key,
                resource_key,
                node,
                align,
            } => {
                if let Some(chain) = Self::chain_for(&mut self.strips, &mut self.buses, target) {
                    Some(chain.replace_if_kind(
                        slot as usize,
                        expected_kind,
                        expected_resource_key,
                        resource_key,
                        node,
                        align,
                    ))
                } else {
                    Some(ReclaimedEffect {
                        node: Some(node),
                        align,
                        analyzer: None,
                        state: None,
                        channel: None,
                    })
                }
                .filter(|displaced| !displaced.is_empty())
                .map(StructuralReclaim::Effect)
            }
            StructuralCommand::AddChannel { mut storage } => {
                let channel = self.sequencer.active_channels();
                if channel >= MAX_CHANNELS {
                    return Some(StructuralReclaim::Effect(ReclaimedEffect {
                        node: None,
                        align: None,
                        analyzer: None,
                        state: None,
                        channel: Some(storage),
                    }));
                }
                // The graph only grows. A channel removed earlier left its
                // storage behind, so this may already have somewhere to go —
                // in which case the storage that arrived goes straight back
                // rather than being dropped on this thread.
                let spare = channel < self.strips.len();
                // A spare slot takes the instrument that arrived and sends
                // its own back in the storage it came in, so the old node is
                // dropped off this thread with the rest of that storage.
                if let Some(strip) = spare.then(|| self.strips.get_mut(channel)).flatten() {
                    std::mem::swap(&mut strip.source, &mut storage.strip.source);
                    strip.source_base = strip.source.kind().default_generator_params();
                }
                let returned = if spare { Some(storage) } else { self.push_channel(storage); None };
                // A spare slot keeps whatever chain it had until now. Clearing
                // it pushes up to one chain's worth into `reclaim`, which the
                // executor guarantees is empty here and which was reserved
                // for exactly that much; the executor forwards the contents.
                debug_assert!(
                    self.reclaim.is_empty(),
                    "a structural edit ran while displaced effects were still queued"
                );
                if let Some(strip) = self.strips.get_mut(channel) {
                    strip.reset_slot(&mut self.reclaim);
                }
                self.set_channel_modulation(channel, ModRack::default());
                self.sequencer.clear_channel(channel);
                self.sequencer.set_active_channels(channel + 1);
                returned
                    .map(|storage| ReclaimedEffect {
                        node: None,
                        align: None,
                        analyzer: None,
                        state: None,
                        channel: Some(storage),
                    })
                    .map(StructuralReclaim::Effect)
            }
            StructuralCommand::SetContainerSpan {
                target,
                slot,
                children,
                align,
                scratch,
            } => {
                let Some(chain) = Self::chain_for(&mut self.strips, &mut self.buses, target)
                else {
                    return Some(StructuralReclaim::Container { align, scratch });
                };
                // A chain keeps the first scratch it is given and hands every
                // later one straight back, the way the graph keeps the first
                // storage a channel index is ever given.
                // ...unless the arrival can run a layer and what it has
                // cannot, which is the one time the chain trades up; the
                // smaller one goes back to be dropped like any other.
                let scratch = match (&chain.container_dry, scratch) {
                    (None, Some(scratch)) => {
                        chain.container_dry = Some(scratch);
                        None
                    }
                    (Some(have), Some(scratch)) if scratch.holds_layers() && !have.holds_layers() => {
                        chain.container_dry.replace(scratch)
                    }
                    (_, scratch) => scratch,
                };
                let displaced = chain.slot_mut(slot as usize).and_then(|state| {
                    state.container_children = children;
                    std::mem::replace(&mut state.container_align, align)
                });
                (displaced.is_some() || scratch.is_some()).then_some(
                    StructuralReclaim::Container {
                        align: displaced,
                        scratch,
                    },
                )
            }
            StructuralCommand::SetBranchAlign {
                target,
                slot,
                align,
            } => {
                let Some(chain) = Self::chain_for(&mut self.strips, &mut self.buses, target)
                else {
                    return Some(StructuralReclaim::Container {
                        align,
                        scratch: None,
                    });
                };
                let displaced = match chain.slot_mut(slot as usize) {
                    Some(state) => std::mem::replace(&mut state.branch_align, align),
                    None => align,
                };
                displaced.map(|align| StructuralReclaim::Container {
                    align: Some(align),
                    scratch: None,
                })
            }
            StructuralCommand::RemoveEffect { target, slot } => {
                let chain = Self::chain_for(&mut self.strips, &mut self.buses, target);
                // Read the departing device's identity before it goes, since
                // it is what the routes and lanes that drove it are named by.
                let device = chain
                    .as_ref()
                    .and_then(|chain| chain.slot(slot as usize))
                    .map_or(mooloop_core::DeviceId::UNASSIGNED, |state| state.device);
                let displaced = chain.map(|chain| chain.remove(slot as usize));
                // The routes and lanes that drove the departed device go with
                // it. Nothing else has to be told: what closed up behind it
                // was positions, and no address is one.
                self.forget_device(target, device);
                displaced
                    .filter(|displaced| !displaced.is_empty())
                    .map(StructuralReclaim::Effect)
            }
            StructuralCommand::SetSamplerStretch { channel, pool } => {
                let Some(sampler) = self
                    .strips
                    .get_mut(channel as usize)
                    .and_then(|strip| strip.source.as_sampler_mut())
                else {
                    // Nothing to install into -- no such channel, or it is
                    // not running the sampler any more. Hand the pool
                    // straight back rather than dropping it here: this is the
                    // realtime thread, and neither is a reason to free 1.6 MB
                    // on it.
                    return pool.map(StructuralReclaim::SamplerStretch);
                };
                match pool {
                    Some(pool) => sampler.install_stretch(pool),
                    None => sampler.take_stretch(),
                }
                .map(StructuralReclaim::SamplerStretch)
            }
            StructuralCommand::InstallSource { channel, node } => {
                let displaced = match self.strips.get_mut(usize::from(channel)) {
                    Some(strip) => strip.install_source(node),
                    // No such channel: the node goes straight back, like
                    // every other arrival with nowhere to go.
                    None => node,
                };
                Some(StructuralReclaim::Source(displaced))
            }
            StructuralCommand::SetCompensation { target, delay } => {
                let slot = match target {
                    EffectTarget::Channel(channel) => self
                        .strips
                        .get_mut(channel as usize)
                        .map(|strip| &mut strip.compensation),
                    EffectTarget::Bus(bus) => self
                        .buses
                        .get_mut(bus as usize)
                        .map(|strip| &mut strip.compensation),
                };
                let Some(slot) = slot else {
                    // Nothing to install into. Hand the ring straight back
                    // rather than dropping it here: this is the realtime
                    // thread, and an unaddressable producer is not a reason to
                    // free memory on it.
                    return delay.map(StructuralReclaim::Compensation);
                };
                // A resend of the length already installed -- which is what
                // the session's reconciler does after every install, having
                // forgotten what it sent -- keeps the live ring.
                let mut delay = delay;
                keep_live_ring(&mut delay, slot);
                std::mem::replace(slot, delay).map(StructuralReclaim::Compensation)
            }
            StructuralCommand::SetConsoleSum { bus, buffer } => {
                let Some(strip) = self.buses.get_mut(bus as usize) else {
                    // Hand it straight back rather than dropping it here:
                    // this is the realtime thread, and an unaddressable bus is
                    // not a reason to free 64 KB on it.
                    return buffer.map(StructuralReclaim::ConsoleSum);
                };
                // A fresh accumulator starts empty and nothing has fed it
                // yet, so the flag has to come back with it -- otherwise the
                // first block would decode whatever the arriving buffer
                // happened to contain.
                strip.console_dirty = false;
                std::mem::replace(&mut strip.console_sum, buffer)
                    .map(StructuralReclaim::ConsoleSum)
            }
            StructuralCommand::SetMidiRouting(routing) => {
                Some(StructuralReclaim::MidiRouting(self.set_midi_routing(routing)))
            }
            StructuralCommand::SetAudioInputRouting(routing) => Some(
                StructuralReclaim::AudioInputRouting(self.set_audio_input_routing(routing)),
            ),
            StructuralCommand::SetBufferMidi(map) => {
                self.set_buffer_midi(map).map(StructuralReclaim::BufferMidi)
            }
            StructuralCommand::SetClaimedNotes(claimed) => Some(StructuralReclaim::ClaimedNotes(
                self.set_claimed_notes(claimed),
            )),
            StructuralCommand::StartTake { channel, take } => {
                let Some(strip) = self.strips.get_mut(channel as usize) else {
                    // Nothing to record on. Handed straight back, so the
                    // drain sees its ring abandoned rather than waiting on it.
                    return Some(StructuralReclaim::Take(take));
                };
                strip.take.replace(take).map(StructuralReclaim::Take)
            }
            StructuralCommand::SetTrackGraph { graph, mut sends } => {
                // One swap, for the reason `SetAudioGraph` is one: a send
                // whose target the render order has not been told about would
                // arrive a block late, and the two must never be observed
                // from different generations.
                //
                // The seats did not move -- a reconciler resends against the
                // document it already installed -- so every edge is matched
                // where it stands.
                sends.adopt_rings_from(&mut self.sends, Some);
                self.bus_graph = graph;
                Some(StructuralReclaim::TrackGraph(std::mem::replace(
                    &mut self.sends,
                    sends,
                )))
            }
            StructuralCommand::SetAudioGraph { bank } => {
                // One swap: the executor never sees an edge without its
                // schedule or a schedule against another generation's
                // buffers.
                Some(StructuralReclaim::AudioGraph(std::mem::replace(
                    &mut self.audio,
                    bank,
                )))
            }
        }
    }

    /// Tell every node in the project that time stopped being continuous.
    ///
    /// Channels first, then buses, which is signal order -- it does not
    /// matter today, because a node may not produce audio from here, but a
    /// contract that is silent about order invites one that does.
    ///
    /// `docs/plans/transport-discontinuity/03-a-discontinuity-is-a-node-contract.md`.
    fn on_discontinuity(&mut self, kind: Discontinuity) {
        for index in 0..self.live_channels() {
            self.strips[index].on_discontinuity(kind);
        }
        for bus in &mut self.buses {
            bus.effects.on_discontinuity(kind);
        }
    }

    /// Hold `command` until the transport reaches `when`.
    ///
    /// The edge is resolved to an absolute tick here, once, rather than every
    /// block: a bar line is a position in the score, so a tempo change moves
    /// when it arrives but not where it is. Resolving repeatedly would also
    /// make the target chase the playhead, and it would never be reached.
    ///
    /// One slot per command kind. A second deferred command of the same kind
    /// **replaces** the first, which is what re-queueing means to a player and
    /// what keeps this bounded without an overflow policy. Different kinds do
    /// not displace each other.
    ///
    /// A stopped transport is not waiting for anything, so the command is
    /// applied immediately rather than parked where nothing would ever
    /// release it.
    pub fn defer_command(&mut self, when: MusicalEdge, command: EngineCommand) {
        if !self.transport.playing {
            self.apply_command(command);
            return;
        }
        let Some(target_tick) = self.resolve_edge(when) else {
            self.apply_command(command);
            return;
        };
        let pending = PendingCommand {
            target_tick,
            command,
        };
        let kind = std::mem::discriminant(&command);
        if let Some(slot) = self
            .deferred
            .iter_mut()
            .find(|slot| slot.is_some_and(|held| std::mem::discriminant(&held.command) == kind))
        {
            *slot = Some(pending);
            return;
        }
        if let Some(slot) = self.deferred.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(pending);
            return;
        }
        // Sized above the number of kinds that can be in flight rather than
        // against a load, so this is a defect and not a busy engine. Dropping
        // is still the release behaviour: a refused deferred command is a
        // gesture that does not happen, which is survivable, and applying it
        // early here would be a discontinuity at the worst possible moment.
        //
        // Counted rather than asserted (MOO-73): a `debug_assert!` here
        // formatted an `EngineCommand` and panicked on the audio thread,
        // which is a worse outcome than the drop it was reporting.
        self.refused_events = self.refused_events.saturating_add(1);
    }

    /// The absolute tick a [`MusicalEdge`] names, or `None` when the edge
    /// cannot be resolved and the caller should apply the command now.
    fn resolve_edge(&self, when: MusicalEdge) -> Option<f64> {
        let now = self.transport.position_ticks;
        let period = match when {
            MusicalEdge::Beat => f64::from(mooloop_core::TICKS_PER_BAR)
                / f64::from(mooloop_core::BEATS_PER_BAR),
            MusicalEdge::Bar => f64::from(mooloop_core::TICKS_PER_BAR),
            MusicalEdge::PatternEnd => {
                // Only Pattern mode schedules from the current pattern; in
                // Song mode the playlist decides what plays and a pattern end
                // is not a boundary the music crosses. The next bar is the
                // nearest edge that still means something there.
                let length = (self.sequencer.playback_mode() == PlaybackMode::Pattern)
                    .then(|| {
                        self.sequencer
                            .pattern_length_ticks(self.sequencer.current_pattern())
                    })
                    .flatten()
                    .filter(|length| *length > 0);
                match length {
                    Some(length) => f64::from(length),
                    None => f64::from(mooloop_core::TICKS_PER_BAR),
                }
            }
        };
        if !period.is_finite() || period <= 0.0 || !now.is_finite() {
            return None;
        }
        // Strictly after, the same rule a take's count-in uses: an edge
        // exactly under the playhead has already happened, and landing on it
        // would make a deferred command indistinguishable from an immediate
        // one at exactly the moments a player is most likely to be aiming
        // for it.
        Some((now / period).floor() * period + period)
    }

    /// Drop any pending deferred command. The transport stopping, the
    /// playback mode changing and a seek all invalidate the position an edge
    /// was resolved against: after one, the target either never arrives or
    /// arrives somewhere that no longer means what the caller meant.
    fn cancel_deferred(&mut self) {
        self.deferred = [None; MAX_DEFERRED];
    }

    /// Apply every deferred command whose edge `tick` has reached, in the
    /// order the block walks. Called between spans, so what runs after it
    /// schedules against the state the command left.
    fn apply_deferred_reached(&mut self, tick: f64) {
        for index in 0..MAX_DEFERRED {
            let Some(pending) = self.deferred[index] else {
                continue;
            };
            if tick + 1e-9 < pending.target_tick {
                continue;
            }
            self.deferred[index] = None;
            self.apply_command(pending.command);
        }
    }

    /// The earliest edge anything is waiting for, which is where the block
    /// has to be cut.
    fn next_deferred_tick(&self) -> Option<f64> {
        self.deferred
            .iter()
            .flatten()
            .map(|pending| pending.target_tick)
            .min_by(f64::total_cmp)
    }

    /// Owe every voice the sequencer is sounding, on every channel, a
    /// release at the start of the next block (MOO-99).
    fn release_all_sequenced(&mut self) {
        for strip in &mut self.strips {
            strip.sequenced.release_all();
        }
    }

    /// Owe the sequenced voices `owed` picks out a release, on every
    /// channel.
    fn release_sequenced_where(&mut self, owed: impl Fn(&crate::voices::SequencedVoice) -> bool) {
        for strip in &mut self.strips {
            strip.sequenced.release_where(&owed);
        }
    }

    /// Owe a release to the voices playing note `id` of `pattern` on
    /// `channel`, whose stored note an edit has just removed or changed.
    fn release_edited_note(&mut self, pattern: u8, channel: u8, id: mooloop_core::NoteId) {
        if let Some(strip) = self.strips.get_mut(usize::from(channel)) {
            strip
                .sequenced
                .release_where(|voice| voice.note_id() == id && voice.origin.pattern == pattern);
        }
    }

    pub fn apply_command(&mut self, cmd: EngineCommand) {
        match cmd {
            EngineCommand::Play => self.transport.play(),
            // A transport that is not running reaches no edge, so anything
            // held would wait forever. Cancelling is the honest end: the
            // gesture was aimed at a moment in the music that is not coming.
            EngineCommand::Pause => {
                self.transport.pause();
                self.cancel_deferred();
                // Through the table as well as each device's own watch on
                // the playing edge, which a muted, skipped generator never
                // sees (MOO-99).
                self.release_all_sequenced();
            }
            EngineCommand::Stop => {
                self.transport.stop();
                self.cancel_deferred();
                self.release_all_sequenced();
                // Stop returns the playhead to the start, so it is a seek as
                // well as a stop -- and what a node is holding must not come
                // back on the next play.
                self.on_discontinuity(Discontinuity::Stop);
            }
            EngineCommand::SetRecordArmed(armed) => self.set_record_armed(armed),
            EngineCommand::SetInputMonitor { channel, on } => {
                if let Some(flag) = self.monitor.get_mut(usize::from(channel)) {
                    *flag = on;
                }
            }
            EngineCommand::StopTake { channel } => {
                if let Some(take) = self
                    .strips
                    .get_mut(channel as usize)
                    .and_then(|strip| strip.take.as_mut())
                {
                    take.stop();
                }
            }
            EngineCommand::SetTempo(bpm) => self.transport.set_tempo(bpm),
            EngineCommand::SetSwing(percent) => self.sequencer.set_swing(percent),
            EngineCommand::SetCurrentPattern(pattern) => {
                let from = self.automation_position();
                let moved = self.sequencer.set_current_pattern(pattern as usize);
                // **A view change is not a seek**, and this command is a view
                // change unless the selection is what is being scheduled. It
                // reaches playback only through `PlaybackMode::Pattern` arms
                // -- `schedule_pattern`, `schedule_once`, `automation_lane_at`
                // and `has_automation_at` -- so in Song mode nothing that is
                // scheduled has changed and neither debt below is owed. (The
                // restore would be a no-op there in any case: song mode
                // answers both `covering_pattern_at` and `automation_lane_at`
                // from the placements under a playhead this command does not
                // move, so the coverage is identical either side of it.) A
                // refused or unchanged selection is the same argument again,
                // and `set_current_pattern` answers it.
                //
                // `docs/plans/transport-discontinuity/`.
                if moved && self.sequencer.playback_mode() == PlaybackMode::Pattern {
                    // The same debt a seek owes, for the same reason: the
                    // note-off that would have ended a sounding voice lives in
                    // the pattern we just stopped scheduling, so without this
                    // it is never emitted and the voice holds until Stop.
                    // Pattern mode has no loop fold to catch it either --
                    // `loop_range` is `None` outside Song mode.
                    //
                    // **Only under a running transport.** Stopped, nothing
                    // sequenced is sounding and so no note-off has been
                    // stranded; what may be sounding is an audition or a held
                    // key, which belongs to the player rather than to the
                    // pattern being left.
                    if self.transport.playing {
                        // Not `seeked`: the block owes the pattern's voices a
                        // release and must not say `Seek`. Only the
                        // pattern's voices, through the table on each strip,
                        // so a key the player is holding rings on (MOO-99).
                        self.release_all_sequenced();
                        // Named for what it is, and distinct from a seek.
                        // Time is still continuous -- what changed is which
                        // notes are being scheduled -- so a node that flushes
                        // a tail on this is wrong, and every one that opted
                        // in checks the kind and declines it. It is here so
                        // the engine stops having one word for two different
                        // facts.
                        self.on_discontinuity(Discontinuity::ProgramChange);
                    }
                    // A *lane* in that pattern owes the same debt, and it is
                    // the quieter one: a note that never ends is heard, and a
                    // knob parked where the last curve left it is not.
                    //
                    // This half is owed whether or not the transport is
                    // running, and the two must not share a condition: lanes
                    // resolve while stopped as well -- the playhead holds
                    // still and the destination sits at the value drawn under
                    // it -- so a stopped switch off an automated pattern
                    // strands the knob exactly as a running one does.
                    self.restore_lanes_left_behind(from);
                }
            }
            EngineCommand::AddPattern => {
                self.sequencer.add_pattern();
            }
            EngineCommand::SetPlaybackMode(mode) => {
                let from = self.automation_position();
                // Every note-off still to come belongs to the mode being
                // left (MOO-99).
                if self.sequencer.playback_mode() != mode {
                    self.release_all_sequenced();
                }
                self.sequencer.set_playback_mode(mode);
                // A pattern end resolved in Pattern mode is not an edge Song
                // mode ever crosses, and the reverse changes what the target
                // meant. Cancelling rather than re-resolving: the caller
                // aimed at an edge in a mode that is gone.
                self.cancel_deferred();
                self.restore_lanes_left_behind(from);
            }
            EngineCommand::Seek { tick } => {
                let from = self.automation_position();
                self.transport.seek(tick);
                self.seeked = true;
                // The block chokes every channel for a seek, so the table
                // has nothing left to name.
                for strip in &mut self.strips {
                    strip.sequenced.forget_all();
                }
                // The target was resolved against a position the transport is
                // no longer travelling from. Left alone it would either fire
                // instantly -- the seek having landed past it -- or wait a
                // whole lap for an edge the player did not aim at.
                self.cancel_deferred();
                self.restore_lanes_left_behind(from);
            }
            EngineCommand::SetLoopRange(range) => self.loop_range = range,
            EngineCommand::SetPatternLength {
                pattern,
                length_steps,
            } => {
                // A new length moves where every lap of the pattern falls,
                // so a note-off scheduled against the old one may never come
                // (MOO-99).
                if self.sequencer.pattern_length_ticks(pattern as usize)
                    != Some(length_steps as u32 * mooloop_core::TICKS_PER_STEP)
                {
                    self.release_sequenced_where(|voice| voice.origin.pattern == pattern);
                }
                self.sequencer
                    .set_pattern_length(pattern as usize, length_steps as usize);
            }
            EngineCommand::SetPlaylistPlacement {
                pattern,
                start_tick,
                on,
            } => {
                let changed = self
                    .sequencer
                    .set_playlist_placement(pattern as usize, start_tick, on);
                // A placement taken away takes its note-offs with it.
                if changed && !on {
                    self.release_sequenced_where(|voice| {
                        voice.origin.pattern == pattern && voice.origin.placement == Some(start_tick)
                    });
                }
            }
            EngineCommand::SetChannelMuted { channel, muted } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    // A mute ends what the pattern is sounding here: its
                    // note-offs would reach a generator the mute is about to
                    // stop calling, and on unmute the voice would resume
                    // mid-envelope (MOO-99).
                    if muted && !strip.output.muted {
                        strip.sequenced.release_all();
                    }
                    strip.output.muted = muted;
                }
            }
            EngineCommand::SetChannelSoloSilenced { channel, silenced } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    if silenced && !strip.solo_silenced {
                        strip.sequenced.release_all();
                    }
                    strip.solo_silenced = silenced;
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
            EngineCommand::SetSendLevel {
                producer,
                index,
                level,
            } => self
                .sends
                .set_level(producer, index as usize, level.clamp(0.0, MAX_LINEAR_GAIN)),
            EngineCommand::SetSendEnabled {
                producer,
                index,
                enabled,
            } => self.sends.set_enabled(producer, index as usize, enabled),
            EngineCommand::SetSendTap {
                producer,
                index,
                tap,
            } => self.sends.set_tap(producer, index as usize, tap),
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
                // A sounding note whose edit moves its note-off somewhere the
                // playhead will not reach -- shortened, moved, or re-pitched,
                // since a note-off names its pitch -- is released now. One
                // that only grows, or changes velocity, keeps its voice: its
                // note-off is still ahead (MOO-99).
                if let Some(old) = self.sequencer.note(pattern as usize, channel as usize, note.id) {
                    if old.note != note.note
                        || old.start_tick != note.start_tick
                        || note.duration_ticks < old.duration_ticks
                    {
                        self.release_edited_note(pattern, channel, note.id);
                    }
                }
                self.sequencer
                    .upsert_note(pattern as usize, channel as usize, note);
            }
            EngineCommand::RemoveNote {
                pattern,
                channel,
                id,
            } => {
                self.release_edited_note(pattern, channel, id);
                self.sequencer
                    .remove_note(pattern as usize, channel as usize, id);
            }
            EngineCommand::OpenAutomationLane {
                pattern,
                channel,
                target,
            } => {
                self.sequencer
                    .open_automation_lane(pattern as usize, channel as usize, target);
            }
            EngineCommand::RemoveAutomationLane {
                pattern,
                channel,
                target,
            } => {
                if self
                    .sequencer
                    .remove_automation_lane(pattern as usize, channel as usize, target)
                {
                    self.restore_base_param(target);
                }
            }
            EngineCommand::ClearAutomationLane {
                pattern,
                channel,
                target,
            } => {
                if self
                    .sequencer
                    .clear_automation_lane(pattern as usize, channel as usize, target)
                {
                    self.restore_base_param(target);
                }
            }
            EngineCommand::UpsertAutomationPoint {
                pattern,
                channel,
                target,
                point,
            } => {
                self.sequencer.upsert_automation_point(
                    pattern as usize,
                    channel as usize,
                    target,
                    point,
                );
            }
            EngineCommand::RemoveAutomationPoint {
                pattern,
                channel,
                target,
                id,
            } => {
                self.sequencer.remove_automation_point(
                    pattern as usize,
                    channel as usize,
                    target,
                    id,
                );
            }
            EngineCommand::TriggerChannelNote {
                channel,
                note,
                velocity,
            } => {
                self.queue_audition(
                    channel,
                    0,
                    Event::NoteOn {
                        id: audition_note_id(note),
                        note,
                        velocity,
                    },
                );
            }
            EngineCommand::Panic => {
                self.panicked = true;
                // Every voice is choked, so the tables have nothing left to
                // name; and every key is let go, pedal and all, so no
                // note-off still owed to one can land on a voice started
                // after this.
                for strip in &mut self.strips {
                    strip.sequenced.forget_all();
                }
                self.held_keys = HeldKeys::new();
                // Nor may a panic leave a note played afterwards bent by a
                // wheel nobody is holding any more.
                for expression in &mut self.expression {
                    if expression.bend != 0.0 {
                        expression.bend(0, 0.0);
                    }
                    // The wheel and the pressure go back to rest with it.
                    let bend_due = expression.bend_due;
                    *expression = ChannelExpression {
                        bend_due,
                        ..ChannelExpression::REST
                    };
                }
                self.key_pressure = [0; 128];
            }
            EngineCommand::ReleaseChannelNote { channel, note } => {
                self.queue_audition(
                    channel,
                    0,
                    Event::NoteOff {
                        id: audition_note_id(note),
                        note,
                    },
                );
            }
            EngineCommand::SetChannelSamplerParams { channel, params } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.set_source_params(GeneratorParams::Sampler(params));
                }
            }
            // **Never reaches a renderer.** A source change moves ownership of
            // a heap node, which this thread may neither build nor free, so
            // `EngineHandle` turns the command into
            // `StructuralCommand::InstallSource` with the node built. It stays
            // an `EngineCommand` because it is still the *edit* the session
            // records and dirties the document for; see its doc comment in
            // `mooloop_core::bridge`. Ignored if it arrives anyway, which only
            // a test pushing it by hand can do.
            EngineCommand::SetChannelSource { .. } => {}
            EngineCommand::SetChannelDrumSynthParams { channel, params } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.set_source_params(GeneratorParams::DrumSynth(params));
                }
            }
            EngineCommand::SetChannelMonoSynthParams { channel, params } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.set_source_params(GeneratorParams::MonoSynth(params));
                }
            }
            EngineCommand::SetChannelMlM1Params { channel, params } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.set_source_params(GeneratorParams::MlM1(params));
                }
            }
            EngineCommand::SetChannelGeneratorParam { channel, id, value } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    // The base is the authored value modulation offsets from,
                    // so this edits the base and lets the ordinary modulation
                    // pass re-derive what the node should hear. Writing the
                    // node directly here would be the same thing for an
                    // unmodulated parameter and would fight the rack for a
                    // modulated one.
                    if strip.source_base.set(id, value).is_some() {
                        strip.push_source_base();
                    }
                }
            }
            EngineCommand::SetSourceRoute { channel, route } => {
                // Structural, so it goes through the base and is pushed
                // whole: the node recompiles its flat table from the routes
                // it is handed.
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    let landed = strip
                        .source_base
                        .internal_routes_mut()
                        .is_some_and(|routes| routes.upsert(route));
                    if landed {
                        strip.push_source_base();
                    }
                }
            }
            EngineCommand::RemoveSourceRoute { channel, route } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    let removed = strip
                        .source_base
                        .internal_routes_mut()
                        .is_some_and(|routes| routes.remove(route));
                    if removed {
                        strip.push_source_base();
                    }
                }
            }
            EngineCommand::SetSourceRouteAmount {
                channel,
                route,
                amount,
            } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.set_source_route_amount(route, amount);
                }
            }
            EngineCommand::SetChannelPolySynthParams { channel, params } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.set_source_params(GeneratorParams::PolySynth(params));
                }
            }
            EngineCommand::MoveEffect { target, from, to } => {
                // The devices move; nothing else has to. This used to run a
                // permutation over every route in the rack and every lane in
                // every pattern.
                if let Some(chain) = self.chain_mut(target) {
                    chain.move_slot(from as usize, to as usize);
                }
            }
            EngineCommand::SetTrackConsole { bus, enabled } => {
                // The master feeds nothing, so encoding its output would put
                // the mix into a sum nothing decodes.
                if bus != MASTER_BUS {
                    if let Some(strip) = self.buses.get_mut(bus as usize) {
                        strip.console = enabled;
                    }
                }
            }
            EngineCommand::SetTrackSoloSilenced { bus, silenced } => {
                if let Some(strip) = self.buses.get_mut(bus as usize) {
                    strip.solo_silenced = silenced;
                }
            }
            EngineCommand::SetTrackPolarity { bus, on } => {
                if let Some(strip) = self.buses.get_mut(bus as usize) {
                    strip.polarity = on;
                }
            }
            EngineCommand::SetStripParam { bus, param, value } => {
                if let Some(strip) = self.buses.get_mut(bus as usize) {
                    strip.strip.apply_param(param, value);
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
            EngineCommand::SetEffectWetDry {
                target,
                slot,
                wet_dry,
            } => {
                if let Some(chain) = self.chain_mut(target) {
                    if let Some(value) = chain.slot_mut(slot as usize).map(|state| &mut state.wet_dry) {
                        *value = wet_dry.clamp(0.0, 1.0);
                    }
                }
            }
            EngineCommand::SetEffectInputTrim {
                target,
                slot,
                input_trim,
            } => {
                if let Some(chain) = self.chain_mut(target) {
                    if let Some(value) = chain.slot_mut(slot as usize).map(|state| &mut state.input_trim) {
                        *value = input_trim.clamp(0.0, MAX_LINEAR_GAIN);
                    }
                }
            }
            EngineCommand::SetEffectOutputTrim {
                target,
                slot,
                output_trim,
            } => {
                if let Some(chain) = self.chain_mut(target) {
                    if let Some(value) = chain.slot_mut(slot as usize).map(|state| &mut state.output_trim) {
                        *value = output_trim.clamp(0.0, MAX_LINEAR_GAIN);
                    }
                }
            }
            EngineCommand::SetEffectParam {
                target,
                slot,
                id,
                value,
            } => self.set_effect_param(target, slot, id, value),
            // Every modulation edit names one fact. The rack-wide diff still
            // runs behind each of them, so a route that disappears here
            // returns its destination to its knob value at the next block
            // exactly as it did when the whole rack travelled.
            EngineCommand::SetModulatorParam {
                channel,
                slot,
                id,
                value,
            } => self.edit_modulation(channel as usize, |rack| {
                let Some(params) = rack.params_mut(slot as usize) else {
                    return false;
                };
                params.set(id, value);
                true
            }),
            EngineCommand::InstallModulator {
                channel,
                slot,
                source,
                params,
            } => self.edit_modulation(channel as usize, |rack| {
                rack.install_with_id(slot as usize, source, params)
            }),
            EngineCommand::ClearModulator { channel, slot } => {
                self.edit_modulation(channel as usize, |rack| rack.clear(slot as usize))
            }
            EngineCommand::MoveModulator { channel, from, to } => {
                self.move_modulator(channel as usize, from as usize, to as usize)
            }
            // A route names its source by durable id, so one that arrives
            // before (or after) the module it names is refused rather than
            // aimed at whatever else occupies that slot.
            EngineCommand::SetModRoute { channel, route } => {
                self.edit_modulation(channel as usize, |rack| rack.apply_route(route).is_some())
            }
            EngineCommand::RemoveModRoute {
                channel,
                source,
                destination,
            } => self.edit_modulation(channel as usize, |rack| {
                rack.remove_route_by_source(source, destination)
            }),
            EngineCommand::TriggerBuffer {
                target,
                slot,
                event,
            } => {
                if let Some(chain) = self.chain_mut(target) {
                    chain.queue_buffer(slot as usize, event);
                }
            }
            EngineCommand::ReleaseBuffer { target, slot } => {
                if let Some(chain) = self.chain_mut(target) {
                    chain.queue_buffer_release(slot as usize);
                }
            }
        }
    }

    /// Install the buffer MIDI mapping, returning the one it displaces for the
    /// caller to reclaim. `apply_structural` on the audio thread, directly on
    /// the control thread while a renderer is being prepared.
    pub(crate) fn set_buffer_midi(
        &mut self,
        map: Option<Box<mooloop_core::midi::BufferMidiMap>>,
    ) -> Option<Box<mooloop_core::midi::BufferMidiMap>> {
        std::mem::replace(&mut self.buffer_midi, map)
    }

    /// Share the control layer's keyboard channel cell. An atomic rather than
    /// a command because it follows the selection, which changes from more
    /// places than any one command would be sent from.
    pub(crate) fn attach_keyboard_channel(&mut self, channel: Arc<AtomicU8>) {
        self.keyboard_channel = channel;
    }

    /// Install the MIDI routing, returning the table it displaces. Same
    /// transport as [`Self::set_buffer_midi`].
    pub(crate) fn set_midi_routing(&mut self, routing: Box<MidiRouting>) -> Box<MidiRouting> {
        std::mem::replace(&mut self.midi_routing, routing)
    }

    /// Install which keys the control map claims, returning the table it
    /// displaces. Same transport as [`Self::set_buffer_midi`].
    pub(crate) fn set_claimed_notes(
        &mut self,
        claimed: Box<mooloop_core::ClaimedNotes>,
    ) -> Box<mooloop_core::ClaimedNotes> {
        std::mem::replace(&mut self.claimed_notes, claimed)
    }

    /// Install the audio input routing, returning the table it displaces.
    pub(crate) fn set_audio_input_routing(
        &mut self,
        routing: Box<AudioInputRouting>,
    ) -> Box<AudioInputRouting> {
        std::mem::replace(&mut self.audio_input_routing, routing)
    }

    /// Copy the driver's input for the next block into the input bus. Empty
    /// slices -- a driver with no input, an offline render -- leave it
    /// silent. Audio thread: copies, never allocates.
    pub(crate) fn load_input(&mut self, left: &[f32], right: &[f32], frames: usize) {
        let frames = frames.min(MAX_BLOCK_SIZE);
        let copied = frames.min(left.len()).min(right.len());
        if copied == 0 {
            if self.input_dirty {
                self.input.clear(frames);
                self.input_dirty = false;
            }
            return;
        }
        self.input.l[..copied].copy_from_slice(&left[..copied]);
        self.input.r[..copied].copy_from_slice(&right[..copied]);
        self.input.l[copied..frames].fill(0.0);
        self.input.r[copied..frames].fill(0.0);
        self.input_dirty = true;
        let (peak_l, peak_r) = self.input.peak(frames);
        self.meters.publish_input(peak_l, peak_r);
    }

    /// Set which channels monitor the hardware input, by seat, for an
    /// install: the incoming project's own, for the reason the routings are.
    pub(crate) fn set_input_monitors(&mut self, monitors: &[bool]) {
        self.monitor = [false; MAX_CHANNELS];
        for (seat, on) in monitors.iter().take(MAX_CHANNELS).enumerate() {
            self.monitor[seat] = *on;
        }
    }

    /// Whether the channel at `index` is hearing the hardware input this
    /// block: its flag is on and its AUDIO input is the hardware input.
    fn monitors_input(&self, index: usize) -> bool {
        self.monitor.get(index).copied().unwrap_or(false)
            && self.audio_input_routing.taps.get(index).copied().flatten()
                == Some(mooloop_core::AudioTap::Input)
    }

    #[cfg(test)]
    pub(crate) fn input_bus(&self) -> &StereoBus {
        &self.input
    }

    #[cfg(test)]
    pub(crate) fn audio_input_taps(&self) -> Vec<Option<mooloop_core::AudioTap>> {
        self.audio_input_routing.taps.clone()
    }

    /// Arm or disarm recording.
    pub(crate) fn set_record_armed(&mut self, armed: bool) {
        self.record_armed = armed;
        if !armed {
            // A note still down when recording is disarmed is abandoned
            // rather than reported half-measured. Its *sound* is untouched:
            // the key is still held, and `held_keys` is what releases it.
            self.recording = [None; 128];
        }
    }

    /// Take one event for the control layer, oldest first. Called by the
    /// executor after the block has rendered, which is the only place that
    /// holds the event ring. What the executor does not take stays here and
    /// goes out next block, so a full ring delays control input rather than
    /// dropping it.
    pub(crate) fn pop_outgoing(&mut self) -> Option<mooloop_core::EngineEvent> {
        self.outgoing.iter_mut().find(|slot| slot.is_some())?.take()
    }

    /// Queue one event for the control layer. Dropped past the cap; see
    /// [`MAX_OUTGOING_EVENTS_PER_BLOCK`].
    fn emit(&mut self, event: mooloop_core::EngineEvent) {
        if let Some(slot) = self.outgoing.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(event);
        }
    }

    /// Translate one block's MIDI input into notes and buffer events. Runs
    /// before the block renders, so input acts on the audio it arrived with.
    ///
    /// The split this method makes is the one the whole MIDI design rests on:
    ///
    /// - **Notes are realtime.** A note a buffer mapping claims drives the
    ///   buffer; otherwise every channel whose input claims the message sounds
    ///   it, and if none does, the selected channel does. All of that happens
    ///   here, at the message's own frame offset. The exception is a key the
    ///   control map has claimed -- a bound pad, or any key while a learn
    ///   gesture waits -- which is forwarded like a control instead
    ///   ([`mooloop_core::ClaimedNotes`]).
    /// - **Control is not.** Control changes, pitch bends and transport
    ///   messages are *forwarded* to the control layer, which maps them
    ///   against the project's bindings and issues the same edits the
    ///   interface would. See the header of `mooloop_core::control` for why.
    ///
    /// For the buffer, note says what and how long, velocity says how hard,
    /// and a CC carries whatever else the tuple needs — the note table holds
    /// the shape of an edit and the controls bend it. A buffer mapping is
    /// resolved here rather than forwarded because it is a performance gesture
    /// on a block boundary, which is the one thing the round trip would spoil.
    pub(crate) fn apply_midi(&mut self, messages: &[mooloop_core::MidiMessage]) {
        use mooloop_core::midi::BufferCcTarget;
        use mooloop_core::MidiKind;

        if messages.is_empty() {
            return;
        }
        let map = self.buffer_midi.as_deref().copied();
        for message in messages {
            // A transport message is addressed to no channel and claimed by
            // no mapping; it goes straight up.
            if message.kind.is_transport() {
                self.emit(mooloop_core::EngineEvent::ControlInput(*message));
                continue;
            }
            let claimed = |note| {
                map.filter(|map| map.accepts(message))
                    .filter(|map| map.note_event(note, 1).is_some())
            };
            // A key the control map has claimed -- a bound pad, or any key
            // while a learn gesture waits -- is a control, not a note: it goes
            // up to be learned or to fire its binding, and is neither played,
            // recorded nor given to the buffer (MOO-129).
            //
            // Its release still lifts anything the same key started as a
            // note. A key held down when learn was armed, or when its pad was
            // bound, went down as a note, and a claimed release that only went
            // up would leave it sounding until Stop. `stop_note` releases only
            // what is held, so for a key that went down as a control it does
            // nothing.
            if self.claimed_notes.claims(message) {
                self.emit(mooloop_core::EngineEvent::ControlInput(*message));
                if let MidiKind::NoteOff { note } = message.kind {
                    self.stop_note(message.offset, note);
                }
                continue;
            }
            match message.kind {
                MidiKind::NoteOn { note, velocity } => match claimed(note) {
                    Some(map) => {
                        if let Some(event) = map.note_event(note, velocity) {
                            let event = self.buffer_cc.apply(event);
                            if let Some(chain) = self.chain_mut(map.target) {
                                chain.queue_buffer(map.slot as usize, event);
                            }
                        }
                    }
                    None => self.play_note(message, note, velocity),
                },
                MidiKind::NoteOff { note } => match claimed(note) {
                    // Only a note this map owns may release; an unmapped key
                    // must not cancel an edit it never started.
                    Some(map) => {
                        if let Some(chain) = self.chain_mut(map.target) {
                            chain.queue_buffer_release(map.slot as usize);
                        }
                    }
                    None => self.stop_note(message.offset, note),
                },
                MidiKind::ControlChange { controller, value } => {
                    // Forwarded whether or not a buffer mapping also claims
                    // it: the control layer needs to see every control that
                    // moves, both to map it and to learn it.
                    self.emit(mooloop_core::EngineEvent::ControlInput(*message));
                    if controller == SUSTAIN_PEDAL_CC {
                        self.set_sustain_pedal(message.offset, value >= 64);
                    }
                    if controller == MOD_WHEEL_CC {
                        let wheel = f32::from(value) / 127.0;
                        self.express(message, |expression| {
                            expression.performance[usize::from(PERFORMANCE_MOD_WHEEL)] = wheel;
                        });
                    }
                    let Some(map) = map.filter(|map| map.accepts(message)) else {
                        continue;
                    };
                    let slot = map.slot as usize;
                    let Some(target) = map.cc_target(controller) else {
                        continue;
                    };
                    match target {
                        BufferCcTarget::Scrub { encoding } => {
                            let ticks = encoding.delta(value);
                            if ticks != 0 {
                                let delta = f64::from(ticks) * self.scrub_frames_per_tick();
                                if let Some(chain) = self.chain_mut(map.target) {
                                    chain.queue_buffer_scrub(slot, delta as f32);
                                }
                            }
                        }
                        // Absolute assignments retune the *next* edit rather
                        // than re-firing one: turning a knob mid-gesture
                        // should not restart the gesture.
                        BufferCcTarget::WindowBars { bars } => {
                            let bucket = mooloop_core::cc_bucket(value, bars.max(1));
                            self.buffer_cc.window_beats =
                                Some(f32::from(bucket + 1) * mooloop_core::BEATS_PER_BAR as f32);
                        }
                        BufferCcTarget::OffsetBeats { beats } => {
                            let bucket = mooloop_core::cc_bucket(value, beats.max(1));
                            self.buffer_cc.offset_beats = Some(-f32::from(bucket + 1));
                        }
                        BufferCcTarget::Repeat { max } => {
                            let bucket = mooloop_core::cc_bucket(value, max.max(1));
                            self.buffer_cc.repeat = Some(u32::from(bucket) + 1);
                        }
                    }
                }
                MidiKind::PitchBend { value } => {
                    // Forwarded as well, so a bend can still be learned.
                    self.emit(mooloop_core::EngineEvent::ControlInput(*message));
                    let semitones = bend_semitones(value);
                    self.express(message, |expression| {
                        expression.bend(message.offset, semitones)
                    });
                }
                // Aftertouch is a modulation source and nothing else: it is
                // not learnable, so it is not forwarded either, which keeps a
                // pressure stream out of the control ring (MOO-128).
                MidiKind::ChannelPressure { value } => {
                    let pressure = f32::from(value) / 127.0;
                    self.express(message, |expression| {
                        expression.channel_pressure = pressure;
                        expression.aftertouch();
                    });
                }
                MidiKind::PolyPressure { note, value } => {
                    self.key_pressure[usize::from(note & 0x7f)] = value;
                    let hardest = self.key_pressure.iter().copied().max().unwrap_or(0);
                    let pressure = f32::from(hardest) / 127.0;
                    self.express(message, |expression| {
                        expression.key_pressure = pressure;
                        expression.aftertouch();
                    });
                }
                // Handled above, before any channel or mapping saw it.
                MidiKind::Start
                | MidiKind::Continue
                | MidiKind::Stop
                | MidiKind::SongPosition { .. } => {}
            }
        }
    }

    /// Frames the head travels per encoder tick. One tick is a 128th of a
    /// beat, so a 128-tick-per-revolution wheel turns one beat per turn —
    /// close enough to a platter's feel to be playable without calibration.
    fn scrub_frames_per_tick(&self) -> f64 {
        self.sample_rate as f64 * 60.0 / self.transport.bpm.max(1.0) / 128.0
    }

    /// A key went down: sound it on every channel listening to the input it
    /// arrived on, and remember which those were.
    ///
    /// **The selected channel is a fallback, not an addition.** A channel that
    /// claimed the message by its own input setting plays it; only if nothing
    /// claimed it does the selection sound it. Adding the selection to the
    /// claimants instead would double every note on a channel that was both
    /// selected and listening -- once at full velocity, once again, which
    /// reads as a stuck-sounding 6 dB rather than as a bug.
    fn play_note(&mut self, message: &mooloop_core::MidiMessage, note: u8, velocity: u8) {
        let offset = message.offset;
        // A key pressed again before its release arrived -- a dropped
        // note-off, or two controllers on one pitch -- lets the first go
        // rather than stranding it under an id the second is about to reuse.
        // So does one the pedal is still holding: the id is the key's.
        self.stop_note(offset, note);
        self.stop_sustained_note(offset, note);
        let id = keyboard_note_id(note);
        let mut claimed = false;
        for channel in 0..self.channel_count() {
            if !self.midi_routing.route(channel).claims(message) {
                continue;
            }
            claimed = true;
            let channel = channel as u8;
            if self.queue_audition(channel, offset, Event::NoteOn { id, note, velocity }) {
                self.held_keys.hold(note, channel);
            }
        }
        if !claimed {
            let channel = self.keyboard_channel.load(Ordering::Relaxed);
            if channel != NO_KEYBOARD_CHANNEL
                && usize::from(channel) < self.channel_count()
                && self.midi_routing.route(usize::from(channel)).follows_selection(message)
                && self.queue_audition(channel, offset, Event::NoteOn { id, note, velocity })
            {
                self.held_keys.hold(note, channel);
            }
        }
        self.capture_note_on(message, note, velocity);
    }

    /// A key came up: release it on every channel it went down on -- or,
    /// with the sustain pedal down, leave it sounding until the pedal lifts.
    ///
    /// A recorded note still ends here, with its key: the pedal is a
    /// performance gesture over the notes, not part of their length.
    fn stop_note(&mut self, offset: u32, note: u8) {
        let id = keyboard_note_id(note);
        for channel in self.held_keys.key_up(note) {
            self.queue_audition(channel, offset, Event::NoteOff { id, note });
        }
        self.capture_note_off(offset, note);
    }

    /// End a note the pedal is holding: its key went down again, or the
    /// pedal lifted. A channel whose note-off finds the block's auditions
    /// full stays sustained, and [`Self::release_lifted_sustain`] sends it
    /// next block -- a pedal lifted over a long glissando can owe more
    /// releases than one block carries.
    fn stop_sustained_note(&mut self, offset: u32, note: u8) {
        let id = keyboard_note_id(note);
        for channel in self.held_keys.sustained_on(note) {
            if self.queue_audition(channel, offset, Event::NoteOff { id, note }) {
                self.held_keys.unsustain(note, channel);
            }
        }
    }

    /// CC 64. Lifting the pedal releases every note it was holding, on the
    /// channels that played it, at the pedal's own frame (MOO-128).
    fn set_sustain_pedal(&mut self, offset: u32, down: bool) {
        if self.held_keys.set_pedal(down) {
            self.release_lifted_sustain(offset);
        }
    }

    /// Send what a lifted pedal still owes, as far as this block has room.
    fn release_lifted_sustain(&mut self, offset: u32) {
        if !self.held_keys.owes_releases() {
            return;
        }
        for note in 0..128u8 {
            self.stop_sustained_note(offset, note);
        }
    }

    /// Apply one expression message to the channels a key from the same
    /// input would sound on: every channel whose input claims it, or, if
    /// none does, the selected one -- [`Self::play_note`]'s rule, so a
    /// wheel bends the notes the keyboard is playing (MOO-128).
    ///
    /// The selected channel keeps what it was last sent when the selection
    /// moves on. A held bend stays on the channel it was bent on until the
    /// wheel moves again with that channel selected, or a panic.
    fn express(
        &mut self,
        message: &mooloop_core::MidiMessage,
        mut apply: impl FnMut(&mut ChannelExpression),
    ) {
        let mut claimed = false;
        for channel in 0..self.channel_count() {
            if self.midi_routing.route(channel).claims(message) {
                claimed = true;
                apply(&mut self.expression[channel]);
            }
        }
        if claimed {
            return;
        }
        let channel = self.keyboard_channel.load(Ordering::Relaxed);
        if channel != NO_KEYBOARD_CHANNEL
            && usize::from(channel) < self.channel_count()
            && self
                .midi_routing
                .route(usize::from(channel))
                .follows_selection(message)
        {
            apply(&mut self.expression[usize::from(channel)]);
        }
    }

    /// Hand each channel's source the bend it is still owed. One that finds
    /// its event list full stays owed and is tried again next block.
    fn dispatch_expression(&mut self, last_frame: u32) {
        for (channel, expression) in self.expression.iter_mut().enumerate() {
            let Some(offset) = expression.bend_due else {
                continue;
            };
            let Some(events) = self.events.get_mut(channel) else {
                continue;
            };
            let delivered = events.push_ordered(TimedEvent {
                offset: offset.min(last_frame),
                event: Event::PitchBend {
                    semitones: expression.bend,
                },
            });
            if delivered {
                expression.bend_due = None;
            }
        }
    }

    /// How many channels the renderer currently has. A route past the end
    /// names nothing, so routing never reaches beyond the bank.
    fn channel_count(&self) -> usize {
        self.events.len()
    }

    /// Note down while recording: remember where and how hard, on whichever
    /// channel took it. Nothing is reported yet -- a note's length is not
    /// known until its key comes up.
    ///
    /// Capture follows the *sound*: the channels in `held_keys` are the ones
    /// that played the note, so recording can never write to a channel that
    /// did not hear it. Only one of them is recorded, and it is the first,
    /// because a pattern note belongs to one channel: a layered input records
    /// where it is played, and the others sound without being written down.
    fn capture_note_on(&mut self, message: &mooloop_core::MidiMessage, note: u8, velocity: u8) {
        if !self.record_armed || !self.transport.playing {
            return;
        }
        // `min` and then the cast, not the cast and then the range: a full
        // bank is 256 channels, and `256 as u8` is 0, which would make an
        // empty range and stop recording entirely at exactly the capacity the
        // bank is sized for.
        let Some(channel) = (0..self.channel_count().min(MAX_CHANNELS))
            .map(|channel| channel as u8)
            .find(|&channel| self.held_keys.is_held(note, channel))
        else {
            return;
        };
        let Some((pattern, start_tick)) =
            self.sequencer.recording_tick(self.tick_at(message.offset))
        else {
            return;
        };
        self.recording[usize::from(note & 0x7f)] = Some(RecordingNote {
            channel,
            // In range: the sequencer's bank is `MAX_PATTERNS` long, which
            // `SetCurrentPattern`'s own `u8` already bounds.
            pattern: pattern as u8,
            velocity,
            start_tick,
            start_frames: self.transport.frames_played() + u64::from(message.offset),
        });
    }

    /// Note up while recording: report the whole note.
    fn capture_note_off(&mut self, offset: u32, note: u8) {
        let Some(held) = self.recording[usize::from(note & 0x7f)].take() else {
            return;
        };
        let frames = (self.transport.frames_played() + u64::from(offset))
            .saturating_sub(held.start_frames);
        // At least one tick: a note tapped inside a single block is still a
        // note, and a zero-length one would be invisible in the pattern.
        let length_ticks = ((frames as f64 * self.transport.ticks_per_sample()).round() as u32)
            .max(1);
        self.emit(mooloop_core::EngineEvent::RecordedNote {
            channel: held.channel,
            pattern: held.pattern,
            note,
            velocity: held.velocity,
            start_tick: held.start_tick,
            length_ticks,
        });
    }

    /// Where `offset` frames into this block falls on the playhead.
    ///
    /// The transport has not advanced yet when MIDI is applied, so this is the
    /// block's start plus the offset's own share -- the position the note was
    /// actually played at, not the position the block ends at. Unfolded: in
    /// pattern mode this runs past the pattern's end on every pass after the
    /// first, which is why recording asks the sequencer where it falls.
    fn tick_at(&self, offset: u32) -> f64 {
        (self.transport.position_ticks + f64::from(offset) * self.transport.ticks_per_sample())
            .max(0.0)
    }

    /// The channels this block's queued notes are addressed to, in order.
    #[cfg(test)]
    pub(crate) fn audition_channels(&self) -> Vec<u8> {
        self.auditions
            .iter()
            .flatten()
            .map(|audition| audition.channel)
            .collect()
    }

    /// Hold an auditioned note until the block's event lists exist.
    ///
    /// Silently dropped past the cap, which returns `false`: that many notes
    /// inside one block is a stuck key, and refusing the next is better than
    /// growing a buffer on the audio thread.
    fn queue_audition(&mut self, channel: u8, offset: u32, event: Event) -> bool {
        let Some(slot) = self.auditions.iter_mut().find(|slot| slot.is_none()) else {
            return false;
        };
        *slot = Some(Audition {
            channel,
            offset,
            event,
        });
        true
    }

    /// Dispatch this block's auditions into the channels' event lists.
    ///
    /// After the sequencer has scheduled, and at the offset each one carries
    /// -- the top of the block for the UI's, whose gesture already happened.
    /// They go in whether or not the transport is running, which is the point
    /// -- auditioning a slice or playing a keyboard must not require pressing
    /// play.
    fn dispatch_auditions(&mut self, frames: usize) {
        // Releases a lifted pedal could not fit into an earlier block.
        self.release_lifted_sustain(0);
        let last_frame = frames.saturating_sub(1) as u32;
        self.dispatch_expression(last_frame);
        for slot in self.auditions.iter_mut() {
            let Some(audition) = slot.take() else {
                continue;
            };
            if let Some(events) = self.events.get_mut(audition.channel as usize) {
                // A refusal is counted by the list itself.
                let _ = events.push_ordered(TimedEvent {
                    offset: audition.offset.min(last_frame),
                    event: audition.event,
                });
            }
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
        let skip_idle = self.skip_idle;
        let ticks_per_sample = self.transport.ticks_per_sample();
        let position_frames = self.transport.frames_played();
        // A song loop belongs to the realtime path only. An offline render
        // walks the arrangement once from the top -- `looping` is already the
        // flag that says so -- and a loop is a way of listening to a section
        // rather than a property of the song, so an export must not take one.
        // Pattern mode declines it for a different reason: it folds into the
        // pattern on screen already, and a second fold over the top of that
        // would be a loop inside a loop.
        let loop_range = (looping && self.sequencer.playback_mode() == PlaybackMode::Song)
            .then(|| self.loop_range.active(self.sequencer.song_length_ticks()))
            .flatten()
            .map(|(start, end)| (f64::from(start), f64::from(end)));
        // The block is cut at the earliest edge anything is waiting for, so
        // the command can be applied between the two halves and the second
        // half scheduled against what it changed.
        let deferred_tick = self.next_deferred_tick();
        let (spans, span_count) =
            self.transport
                .advance_looped(frames, loop_range, deferred_tick);
        let start_tick = spans[0].start_tick;
        let end_tick = spans[span_count - 1].end_tick;
        let seeked = std::mem::take(&mut self.seeked);
        let panicked = std::mem::take(&mut self.panicked);

        for events in &mut self.events {
            events.clear();
        }
        // The releases an edit, a mute, a pattern switch or a stop owed the
        // sequencer's voices since the last block (MOO-99). First, so a note
        // the block is about to start again at the same offset starts after
        // the old one has been let go.
        let live = self.live_channels();
        for (strip, events) in self.strips.iter_mut().zip(self.events.iter_mut()).take(live) {
            strip.sequenced.deliver(0, events);
        }
        if self.transport.playing {
            // A deferred command lands between spans, so the stretch after it
            // must be scheduled separately from the stretch before -- the
            // same per-span pass a loop fold needs, for the same reason.
            if looping || deferred_tick.is_some() {
                // One pass per stretch of musical time in the block. Without
                // a loop or an edge that is the single stretch this has
                // always been.
                for span in &spans[..span_count] {
                    // Before scheduling, not after: what this applies decides
                    // what the span about to be scheduled contains. A target
                    // at or behind the span's start produces no cut above, so
                    // an edge that has already arrived lands here too.
                    if deferred_tick.is_some() {
                        self.apply_deferred_reached(span.start_tick);
                    }
                    self.sequencer.schedule(
                        span.start_tick,
                        span.end_tick,
                        span.frame,
                        span.frames,
                        ticks_per_sample,
                        &mut self.events,
                    );
                }
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
                .take(self.live_channels())
            {
                if !strip.output.muted && !strip.solo_silenced {
                    choke_groups[index] = strip.choke_group();
                }
            }
            inject_choke_events(
                &choke_groups[..self.live_channels()],
                &mut self.events,
            );
        }
        // Everything the sequencer started that is sounding at a fold has to
        // be let go of. The note-off that would have ended it sits past the
        // loop end, which the transport is no longer travelling towards, so
        // without this a pad held across a loop point would be joined by
        // another one every pass, forever.
        //
        // **Only the sequencer's voices** (MOO-99). This used to choke every
        // voice on every channel, and so cut every key the player was
        // holding at each lap; the table on each strip knows which voices
        // the pattern started, and a `NoteOff` for each of those is also a
        // release rather than a cut, so a pad's own release tail rings over
        // the loop point.
        let mut jumped = false;
        let mut folds = [0u32; crate::transport::MAX_BLOCK_SPANS];
        let mut fold_count = 0;
        for span in spans[..span_count].iter().filter(|span| span.jumped) {
            jumped = true;
            folds[fold_count] = span.frame as u32;
            fold_count += 1;
            // A fold is a discontinuity like a seek, and invalidates a
            // pending edge for the same reason -- with one failure mode a
            // seek does not have. A target resolved past the loop end is
            // never reached at all: the fold turns the playhead back before
            // it, every lap, and the command waits for a tick the transport
            // has stopped travelling towards. Cancelling is what keeps a
            // deferred command from being silently immortal.
            self.cancel_deferred();
        }
        // Read what the sequencer just scheduled into each channel's table,
        // ending at each fold whatever was sounding there. Before the
        // auditions and the keyboard join the lists, which is what keeps
        // the table to the sequencer's own voices.
        {
            let sequencer = &self.sequencer;
            for (strip, events) in self.strips.iter_mut().zip(self.events.iter_mut()).take(live) {
                strip.sequenced.observe(events, &folds[..fold_count], |id| {
                    sequencer.voice_origin(id)
                });
            }
        }
        // A seek owes every voice a release -- the note-offs sit before the
        // tick that was seeked to -- and chokes rather than naming them,
        // because an audition or a held key is also somewhere the transport
        // has left. Outside the `playing` arm because a seek while stopped
        // still owes it, for auditioned notes if nothing else. The tables
        // were emptied when the seek was applied.
        if seeked || panicked {
            release_all_voices(0, self.live_channels(), &mut self.events);
        }
        // Before the block's events, as the contract promises, and once for
        // however many spans jumped: a node is being told that time is no
        // longer continuous, which is a fact about the block rather than a
        // count of folds.
        //
        // This is the half of a discontinuity that had no words before. The
        // release above is the *voices* being let go of; this is the delay
        // lines, the reverb tails and the splice positions being told that
        // what they are holding came from somewhere the transport has left.
        //
        // A fold says so, in its own word, and the tail devices decline it:
        // a delay repeat or a reverb tail wraps from the end of the loop into
        // its start (Adam, 2026-09-22, MOO-59). Under one name a node could
        // not keep a tail across a fold without keeping it across a seek too
        // (`reports/fable-2026-09-21.md`, finding 2). A block that both
        // seeked and folded is a seek: the stronger claim is the true one.
        //
        // A pattern switch is neither, and says nothing here: it said
        // `ProgramChange` when it was applied, and the release above is all
        // it owes the block.
        if seeked {
            self.on_discontinuity(Discontinuity::Seek);
        } else if jumped {
            self.on_discontinuity(Discontinuity::LoopFold);
        }
        self.dispatch_auditions(frames);

        let context = ProcessContext {
            sample_rate: self.sample_rate,
            frames,
            playing: self.transport.playing,
            bpm: self.transport.bpm,
            position_ticks: start_tick,
            position_frames,
        };
        // Modulators must all advance before anything borrows the sequencer for
        // automation, and every channel's rack advances even while muted so
        // unmuting does not restart its phase.
        let active_channels = self.live_channels();
        let mut modulator_ticks = [0usize; MAX_CHANNELS];
        // The gate table is sized for the largest block the engine accepts,
        // which is 8192 frames and so 256 control ticks of 256 channels. That
        // is 192 KB, and it used to be a local: every block began by zeroing
        // all of it and then writing four rows. Kept here instead, only the
        // rows this block will read are cleared, and the audio thread stops
        // wiping an L2's worth of cache before it renders anything.
        let control_ticks = frames.div_ceil(CONTROL_RATE_FRAMES);
        for row in self.gate_ticks.iter_mut().take(control_ticks) {
            *row = [NoteGateEvents::default(); MAX_CHANNELS];
        }
        // A zero-frame block has no subdivision to file a gate under, and no
        // cleared row to file it in either; nothing will read the table.
        let gated_channels = if control_ticks == 0 { 0 } else { active_channels };
        // Not an iterator loop: `gate_ticks` is indexed by control tick first
        // and by channel second, so the loop variable is not this array's
        // outer index and enumerating it would walk the wrong axis.
        #[allow(clippy::needless_range_loop)]
        for source_channel in 0..gated_channels {
            for event in self.events[source_channel].iter() {
                // Clamped into the cleared region rather than into the array:
                // an offset past the end of the block is already nonsense,
                // and the rows beyond this block's own are stale.
                let tick = (event.offset as usize / CONTROL_RATE_FRAMES)
                    .min(control_ticks.saturating_sub(1));
                let gate = &mut self.gate_ticks[tick][source_channel];
                match event.event {
                    Event::NoteOn { .. } => gate.note_ons = gate.note_ons.saturating_add(1),
                    Event::NoteOff { .. } => gate.note_offs = gate.note_offs.saturating_add(1),
                    Event::Choke => gate.choke = true,
                    _ => {}
                }
            }
        }
        // Where in the song each control tick starts, in beats, for the
        // tempo-synced LFOs that take their phase from it (MOO-127). None
        // while stopped: they free-run then.
        let song_beats = song_beats_for(
            self.transport.playing,
            &spans[..span_count],
            frames,
            f64::from(self.transport.ppq.ticks_per_beat()),
        );
        // Field-by-field rather than through `self`, so the gate table stays
        // borrowable while the racks it feeds are advanced.
        for (index, ticks) in modulator_ticks.iter_mut().enumerate().take(active_channels) {
            *ticks = Self::tick_channel_modulators(
                &mut self.modulators,
                &mut self.control_outputs,
                self.sample_rate,
                self.transport.bpm,
                index,
                frames,
                &self.gate_ticks,
                &song_beats,
            );
        }
        // Lanes resolve whether or not the transport is running: stopped, the
        // playhead simply holds still and the destination sits at the value
        // drawn under it. Making automation conditional on playback would mean
        // a knob that jumps the moment you press play.
        //
        // It *is* conditional on a lane existing, which is a different claim
        // and costs nothing to make: with none under the playhead every
        // `curve_for` below can only answer `None`, and each of those answers
        // is a walk over every active channel. Asking once here instead of
        // once per descriptor per channel is the whole of the saving.
        let automation = (frames > 0 && self.sequencer.has_automation_at(start_tick)).then(|| {
            AutomationBlock {
                sequencer: &self.sequencer,
                start_tick,
                ticks_per_sample,
                ticks: frames.div_ceil(CONTROL_RATE_FRAMES),
            }
        });
        // Only the buses that may hold something. A song uses one or two of
        // the seventeen every project carries, and emptying a buffer that is
        // already zero is the largest single line in an empty block.
        for strip in &mut self.buses {
            if strip.dirty {
                strip.bus.clear(frames);
                strip.dirty = false;
            }
            // The encoded accumulator empties on its own flag: it is written
            // by a different set of feeders than `bus` and is usually absent
            // altogether, so hanging it off `dirty` would clear a buffer that
            // does not exist on every block a bus is used at all.
            if strip.console_dirty {
                if let Some(sum) = strip.console_sum.as_mut() {
                    sum.clear(frames);
                }
                strip.console_dirty = false;
            }
        }
        // Emptied before anything renders, so a producer that stopped playing
        // -- or a channel that stopped existing -- publishes silence rather
        // than the block before.
        self.audio.clear(frames);
        // The compiled schedule, not index order: a producer has to render
        // before the consumer that reads it, in the same block, because a
        // block-sized delay is a delay whose length is the host's buffer
        // size. `order()` is the identity permutation on a project with no
        // subscriptions, which is what makes the edge inaudible until one is
        // authored.
        //
        // The modulator tick pass above stays in index order on purpose. A
        // modulator's phase must not depend on a subscription somebody made
        // on another channel, so the two passes are separate and only this
        // one is scheduled.
        // Which channels reached their output this block, for a take reading
        // one: a muted or sleeping channel's buffer holds stale or pre-fader
        // audio, and what a take of it should hear is silence.
        let mut heard = [false; MAX_CHANNELS];
        for slot in 0..MAX_CHANNELS {
            let index = self.audio.graph.order()[slot] as usize;
            if index >= active_channels {
                continue;
            }
            let ticks = modulator_ticks[index];
            // Published before the mute check: a muted channel's modulators
            // still run, so its knobs should still animate rather than freeze
            // on whatever the last audible block left behind.
            // The generator's outlets, as published at the end of the block
            // before this one. Taken before the strip renders, so nothing in
            // this block can read its own publication and the one declared
            // block of latency is a fact about the order rather than a rule.
            let outlets = self.strips[index].published_outlets;
            if ticks > 0 {
                let mut row = [0.0; CONTROL_SOURCE_SLOTS];
                let (modulators, rest) = row.split_at_mut(MAX_MODULATORS_PER_CHANNEL);
                let (published, performance) = rest.split_at_mut(MAX_GENERATOR_OUTLETS);
                modulators.copy_from_slice(&self.control_outputs[index][ticks - 1]);
                published.copy_from_slice(&outlets);
                performance.copy_from_slice(&self.expression[index].performance);
                // The whole row, so the view can resolve a knob driven by an
                // outlet as well as one driven by a module. One snapshot a
                // block, unlike the per-tick table this is taken from.
                self.modulator_meters.publish(index, &row);
            }
            // Mute and solo are one question here and two fields everywhere
            // else, the way the track loop below puts it: a channel silenced
            // by someone else's solo behaves exactly as a muted one, down to
            // still filling a tap that something reads.
            let muted =
                self.strips[index].output.muted || self.strips[index].solo_silenced;
            self.strips[index].output.aim(muted);
            // **A mute is a fade first** (MOO-107). A channel muted mid-note
            // goes on rendering in full, its output stage and its sends
            // ramping to silence, and only once both have arrived is it
            // `faded` and allowed to take the paths below that stop it
            // contributing. Before that it stopped in the block the button
            // was pressed, which on anything sustained is a click.
            let producer = EffectTarget::Channel(index as u8);
            let faded = muted
                && self.strips[index].output.is_silent()
                && self.sends.is_silent(producer);
            // A muted producer that nobody reads still skips, which is what
            // keeps mute a way of not spending the work. One that somebody
            // reads renders its generator and stops there: mute is an
            // output-stage decision about what reaches the bus, and a
            // pre-level tap is exactly the signal a source muted in its own
            // mix still has.
            // A release still has to reach a generator the mute has stopped
            // calling, or its voice is frozen mid-note and resumes where it
            // stopped on unmute -- and ML-M1 keeps the key in its held-note
            // stack, so its next release drones (MOO-99). Such a block is
            // rendered, silently, and idle-skip puts the channel back to
            // sleep once the generator is at rest.
            let owes_a_release = self.events[index]
                .iter()
                .any(|event| matches!(event.event, Event::NoteOff { .. } | Event::Choke))
                && !self.strips[index].source_node().is_at_rest();
            if faded && !self.audio.produces(index) && !owes_a_release {
                // A muted channel renders nothing, so its compensation ring
                // would still be holding the audio from before the mute and
                // would emit it on unmute. Emptying it is fifteen writes, and
                // it is the honest state: a silent producer's pipeline is
                // silent too.
                if let Some(delay) = self.strips[index].compensation.as_mut() {
                    delay.reset();
                }
                // Nothing measured this block, so nothing may be concluded
                // from it: unmuting always renders at least one block before
                // the channel is allowed to decide it is idle.
                self.strips[index].source_silent_frames = 0;
                continue;
            }
            let performance = self.expression[index].performance;
            let modulation = ModulationBlock {
                rack: &self.modulation[index],
                outputs: &self.control_outputs[index],
                outlets: &outlets,
                performance: &performance,
                ticks,
            };
            // The generator's driven parameters go into their own curve
            // pool now (`source_curves[index]`); its internal route amounts
            // (`SourceRouteAmount`, just below) still go into the channel's
            // own note list, the event stream it already splits its block
            // on. Written inline rather than as a method because the
            // automation block holds `&self.sequencer` for the whole loop,
            // and only the compiler's field-level borrow splitting can see
            // that `self.events`, `self.source_curves` and `self.strips`
            // are disjoint from it.
            // Cleared unconditionally, before the gate below: a channel
            // whose source stops being driven altogether (the last route
            // removed, the last lane cleared or switched off) must not
            // leave a previous block's rows in the pool for `ChannelStrip::process`
            // to hand the generator again through `apply_curves` -- the
            // gate's own early skip means the descriptor loop that would
            // otherwise refresh or empty this pool never runs, so an
            // un-gated clear is the only thing that empties it. Caught by
            // `two_simultaneously_modulated_source_params_survive_a_maximal_block`'s
            // sibling in spirit, though the regression itself was found by
            // the pre-existing `a_generator_parameter_reaches_the_device`
            // and the "hands the knob back" family.
            self.source_curves[index].clear(0);
            // Neither pass below can produce an event without either a route
            // in this channel's rack or a lane under the playhead, and both
            // questions are settled for the whole channel before either loop
            // starts. Asked per descriptor instead, a device the size of
            // ML-P8 pays two hundred route-table walks a block to be told
            // what one walk already said.
            if modulation.rack.has_routes() || automation.is_some() {
                let base = self.strips[index].source_base;
                let scope = EffectTarget::Channel(index as u8);
                // Shared by every descriptor below, and by `curve_scratch`'s
                // trim: hoisted once rather than recomputed per descriptor,
                // which the old per-event form did (harmlessly, since it
                // was cheap per event; kept as one place now that it also
                // decides how much of the pool's rows are valid).
                let resolved_ticks = ticks.max(automation.as_ref().map_or(0, |a| a.ticks));
                // This channel's driven source parameters go into their own
                // per-destination curve pool instead of the shared,
                // 256-slot `events[index]` list `push_ordered` used to fill
                // one `ParamValue` at a time -- the same change
                // `EffectChain::control_events_for_slot` makes, and for the
                // same reason (`reports/fable-2026-09-22.md`, finding 3):
                // this list also carries the channel's *notes*, so a
                // maximal block with even one automated parameter and a
                // played note could make the note lose the race.
                self.source_curves[index].clear(resolved_ticks);
                for descriptor in base.kind().descriptors() {
                    let destination = ParamAddr {
                        scope,
                        owner: ParamOwner::Source,
                        param: descriptor.id,
                    };
                    let policy = ModDestinationDescriptor::for_param(descriptor);
                    let modulated = modulation.rack.modulates(destination, &policy);
                    let curve = automation
                        .as_ref()
                        .and_then(|automation| automation.curve_for(destination));
                    if !modulated && curve.is_none() {
                        continue;
                    }
                    let Some(knob) = base.get(descriptor.id) else {
                        continue;
                    };
                    let knob_normalized = descriptor.to_normalized(knob);
                    let Some(row) = self.source_curves[index].begin(descriptor.id) else {
                        // Every shipped generator's whole table fits
                        // `MAX_SOURCE_CURVE_DESTINATIONS` (DS-01's own 92 is
                        // the bound) with room to spare; reaching this arm
                        // means a future kind's table has grown past it.
                        self.source_curve_refusals += 1;
                        continue;
                    };
                    for (tick, slot) in row.iter_mut().enumerate().take(resolved_ticks) {
                        let base_normalized = curve
                            .as_ref()
                            .zip(automation.as_ref())
                            .and_then(|(curve, automation)| automation.value_at(curve, tick))
                            .unwrap_or(knob_normalized);
                        let offset_normalized = if modulated {
                            modulation.rack.offset_for(
                                destination,
                                modulation.sources(tick),
                                &policy,
                            )
                        } else {
                            0.0
                        };
                        *slot = descriptor
                            .from_normalized((base_normalized + offset_normalized).clamp(0.0, 1.0));
                    }
                }

                // The generator's own internal routes, whose amounts are
                // automatable but are not entries in the table above: they
                // are addressed by the route's durable id, so they resolve
                // through the same base-plus-offset pass and leave through
                // their own event.
                //
                // **Deliberately still event-based, not curve-based.**
                // `Event::SourceRouteAmount` is a different wire shape from
                // `Event::ParamValue` (a route id, not a descriptor id), and
                // `ControlCurve`/`apply_curves`'s default only knows how to
                // reconstruct the latter -- folding this in too would mean
                // deciding what a curve *means* for a non-descriptor
                // destination, which this pass leaves for the next one.
                // `docs/plans/automation-curves/00-status.md` records it as
                // not done rather than silently unaddressed.
                let internal: Option<mooloop_core::MlP8Routes> =
                    base.internal_routes().copied();
                // Thinned the way `apply_curves`'s default fallback thins
                // (MOO-73), against half of what the channel's list has left:
                // the other half is for the source parameters' fallback,
                // which runs after this. Sixteen routes at 1024 frames would
                // otherwise fill the list alone.
                let route_rows = internal.iter().flat_map(|routes| routes.iter()).count()
                    * base.kind().route_descriptors().len();
                let route_stride = control_tick_stride(
                    resolved_ticks,
                    route_rows,
                    self.events[index].remaining() / 2,
                );
                for route in internal.iter().flat_map(|routes| routes.iter()) {
                    for descriptor in base.kind().route_descriptors() {
                        let destination = ParamAddr {
                            scope,
                            owner: ParamOwner::SourceRoute { route: route.id },
                            param: descriptor.id,
                        };
                        let policy = ModDestinationDescriptor::for_param(descriptor);
                        let modulated = modulation.rack.modulates(destination, &policy);
                        let curve = automation
                            .as_ref()
                            .and_then(|automation| automation.curve_for(destination));
                        if !modulated && curve.is_none() {
                            continue;
                        }
                        let knob_normalized = descriptor.to_normalized(route.amount);
                        for tick in 0..resolved_ticks {
                            // Counted back from the last tick, so the block
                            // still ends on the resolved value.
                            if (resolved_ticks - 1 - tick) % route_stride != 0 {
                                continue;
                            }
                            let base_normalized = curve
                                .as_ref()
                                .zip(automation.as_ref())
                                .and_then(|(curve, automation)| automation.value_at(curve, tick))
                                .unwrap_or(knob_normalized);
                            let offset_normalized = if modulated {
                                modulation.rack.offset_for(
                                    destination,
                                    modulation.sources(tick),
                                    &policy,
                                )
                            } else {
                                0.0
                            };
                            let amount = descriptor.from_normalized(
                                (base_normalized + offset_normalized).clamp(0.0, 1.0),
                            );
                            // A refusal is counted by the list itself.
                            let _ = self.events[index].push_ordered(TimedEvent {
                                offset: (tick * CONTROL_RATE_FRAMES) as u32,
                                event: Event::SourceRouteAmount {
                                    route: route.id,
                                    amount,
                                },
                            });
                        }
                    }
                }
            }
            let strip_segments = resolve_strip_segments(
                self.strips[index].output.gain,
                self.strips[index].output.pan,
                EffectTarget::Channel(index as u8),
                &modulation,
                automation.as_ref(),
            );
            // A channel with nothing to answer, nothing sounding, and nothing
            // still decaying in its chain is not rendered at all. On a
            // thirty-two channel arrangement with four things playing, that
            // is twenty-eight generators, twenty-eight effect chains and
            // twenty-eight pan stages that do not run.
            //
            // The event list has to be *empty*, not merely free of notes. A
            // modulated or automated source parameter resolves into
            // `ParamValue` events just above, and a generator splits its
            // block at every event it is given -- so a strip that slept
            // through them would advance its free-running state in one stride
            // where a running one took several, and the two would not agree
            // to the bit. A channel whose source is being driven therefore
            // keeps rendering, which is also the honest reading: something is
            // still moving in it.
            // A monitored channel never sleeps: its input can start at any
            // moment, and the generator's own silence says nothing about it.
            let monitored = self.monitors_input(index);
            // Nor while its output stage is still moving, for the reason a
            // driven source keeps it awake: a strip that slept through a
            // fader move would make it on the first note after waking, where
            // a strip that stayed awake made it on silence. A driven fader is
            // the exception -- it never settles, and a strip must not be kept
            // awake for ever by a lane on its fader.
            let stage_still =
                strip_segments.is_some() || self.strips[index].output.is_settled();
            if skip_idle
                && !monitored
                && stage_still
                && self.events[index].is_empty()
                && self.strips[index].is_idle()
            {
                self.strips[index].sleep(&context);
                self.slept_strip_blocks += 1;
                // Positions are stored rather than peak-held, so unlike the
                // meters they do have to be written: otherwise the last
                // sounding voice's playhead stays pinned in the UI.
                self.playhead_meters
                    .publish(index, &[0.0; MAX_SAMPLER_VOICES as usize]);
                continue;
            }
            self.strips[index].sleeping = false;
            // The edge this channel reads, taken before the port group so the
            // bank is borrowed once each way rather than both at once.
            let source = self.audio.source(index).is_some_and(|tap| {
                self.aux_scratch.l[..frames].copy_from_slice(&tap.l[..frames]);
                self.aux_scratch.r[..frames].copy_from_slice(&tap.r[..frames]);
                true
            });
            // Scoped, because the port group holds the bank borrowed and the
            // mute check below needs it back.
            {
                let published = self.strips[index].source.kind().outlets();
                let mut ports = self.audio.ports(index, published);
                // A small on-stack array of slice references, not a `Vec`:
                // `apply_curves` takes a slice, and this is the realtime
                // thread. Building it costs a `MAX_SOURCE_CURVE_DESTINATIONS`-
                // long array of (id, empty-slice) pairs even when nothing is
                // driven -- cheap (tens of bytes, no allocation), unlike the
                // curve pool's own storage this borrows from.
                let mut curve_buf: [ControlCurve<'_>; MAX_SOURCE_CURVE_DESTINATIONS] =
                    std::array::from_fn(|_| ControlCurve::default());
                let curve_count = self.source_curves[index].fill(&mut curve_buf);
                let strip = &mut self.strips[index];
                strip.bus.clear(frames);
                // The list counts what the fallback could not fit.
                let _ = strip.process(
                    &context,
                    &mut self.events[index],
                    &curve_buf[..curve_count],
                    source.then_some(&self.aux_scratch),
                    &mut ports,
                );
            }
            // Monitoring: the hardware input joins the generator's output
            // before the channel's devices, fader and pan, so it is heard
            // through the channel exactly as the channel is heard.
            if monitored {
                self.strips[index].bus.add_from(&self.input, frames);
            }
            if faded {
                // Its tap is filled and its bus is not read: a muted producer
                // publishes, and reaches nothing else -- once it has faded out.
                //
                // It also publishes its *control* outlets, where a muted
                // channel nobody reads freezes them at the last audible
                // block. That is a difference between two muted channels, and
                // the moving one is the honest answer: a muted channel's
                // modulators already keep running so its knobs keep animating,
                // and a device that has stopped sounding should publish a
                // decayed envelope rather than the one it had when it was
                // silenced.
                if let Some(delay) = self.strips[index].compensation.as_mut() {
                    delay.reset();
                }
                // And its sends', on the same argument. Dormant while nothing
                // authors a channel send, correct the day something does.
                self.sends.reset(EffectTarget::Channel(index as u8));
                self.strips[index].source_silent_frames = 0;
                continue;
            }
            let strip = &mut self.strips[index];
            let source_peak = strip.bus.peak(frames);
            strip.note_source_level(source_peak.0.max(source_peak.1), frames);
            self.device_meters
                .publish_output(index, 0, source_peak.0, source_peak.1);
            self.playhead_meters
                .publish(index, &sampler_playheads(&*strip.source));
            strip.effects.process(
                &context,
                &mut strip.bus,
                EffectTarget::Channel(index as u8),
                Some((&self.device_meters, &self.device_telemetry, index)),
                Some(&modulation),
                automation.as_ref(),
                skip_idle,
            );
            // Slot inputs the chain found non-finite (MOO-176). One branch a
            // chain a block, and a relaxed add only when there were any.
            let faults = std::mem::take(&mut strip.effects.faults_unpublished);
            if faults > 0 {
                self.meters.publish_effect_faults(faults);
            }
            // A pre-fader send leaves from here: after the chain, before the
            // fader and the pan. Copied rather than emitted, because the
            // tracks it reaches are not reachable while this strip is
            // borrowed -- and skipped entirely when nothing reads this tap,
            // which is every strip in a project that has no sends.
            self.sends
                .capture(producer, SendTap::PreFader, &strip.bus, frames);
            strip
                .output
                .apply_segments(&mut strip.bus, frames, strip_segments.as_ref(), muted);
            // And a post-fader one from here, which is the same signal the
            // strip's own output carries -- but *before* the compensation
            // below, because that is what this strip's output owes its own
            // summing point and a send generally owes a different one.
            self.sends
                .capture(producer, SendTap::PostFader, &strip.bus, frames);
            // Wait, if this channel is shorter than something else feeding the
            // same bus. Last, so what waits is the finished channel, and
            // immediately before the sum it is being aligned for.
            if let Some(delay) = strip.compensation.as_mut() {
                delay.process(&mut strip.bus.l[..frames], &mut strip.bus.r[..frames]);
            }
            heard[index] = true;
            if let Some(destination) = self.buses.get_mut(strip.destination as usize) {
                // Always linear. A channel is what reaches a track, not a
                // console strip of its own -- analog sum is a track's switch
                // and only a track's, so the encode happens at a track's
                // *output*, one level further on.
                destination.bus.add_from(&strip.bus, frames);
                destination.dirty = true;
            }
            self.sends.emit(producer, &mut self.buses, frames, muted);
        }

        // Walk the compiled schedule. Every bus is guaranteed to appear after
        // everything feeding it, so one pass suffices whatever the routing
        // looks like; the master sorts last and keeps its audio, since it is
        // what the caller reads.
        let mut master_peak = (0.0, 0.0);
        // The **whole** compiled order, not the first `buses.len()` slots of
        // it. `render_order` is a permutation over the entire address space,
        // so a track's position in it has nothing to do with how many tracks
        // exist -- walking a prefix would visit an arbitrary subset and drop
        // real tracks silently. Absent indices are skipped instead.
        for slot in 0..MAX_BUSES {
            let index = self.bus_graph.render_order()[slot] as usize;
            let Some(strip) = self.buses.get_mut(index) else {
                continue;
            };
            // Before `is_resting` asks whether its ramps have arrived.
            strip.aim();
            // Everything feeding this bus has already run -- that is what the
            // compiled order guarantees -- so `dirty` is the settled answer to
            // whether anything reached it this block, and costs no pass over
            // the buffer to ask.
            if strip.dirty {
                strip.silent_frames = 0;
            } else {
                strip.silent_frames = strip.silent_frames.saturating_add(frames as u32);
            }
            // A bus nobody routed to, whose chain has finished and whose
            // compensation ring holds only the silence it has been fed, has
            // nothing to contribute. Every project carries all seventeen and
            // a song uses one or two, so this is most of what an empty block
            // was spending: sixteen buffers emptied, peaked twice, balanced
            // and metered to say nothing.
            if skip_idle && !strip.dirty && strip.is_resting() {
                strip.effects.sleep(&context);
                if !strip.sleeping {
                    strip.sleeping = true;
                    // The whole buffer, once, rather than this block's worth.
                    // A later block may be longer than the one that emptied
                    // it, and would then read past what was cleared into
                    // audio from before the silence.
                    let capacity = strip.bus.capacity();
                    strip.bus.clear(capacity);
                    // Its send rings too: `is_resting` weighs the strip's own
                    // compensation against the silence and knows nothing
                    // about these, so a track that idles would truncate its
                    // send tail and freeze the remainder until it woke.
                    self.sends.reset(EffectTarget::Bus(index as u8));
                    if let Some(sum) = strip.console_sum.as_mut() {
                        sum.clear(capacity);
                    }
                    strip.console_dirty = false;
                }
                // Nothing is published here. Both `publish` and
                // `publish_input` are `fetch_max`, so writing zero cannot
                // lower a cell -- this used to be two such writes under a
                // comment claiming they cleared the meter, which they never
                // did. The mechanism that actually empties a peak cell is the
                // GUI's own read (`AUDIO_ARCHITECTURE.md`), so not writing one
                // *is* publishing silence for every cell somebody drains. The
                // bus peaks are drained every tick; a resting bus's head-stage
                // input is not, and that is recorded in `docs/LOOSE_ENDS.md`
                // rather than papered over here.
                if index == MASTER_BUS as usize {
                    master_peak = (0.0, 0.0);
                }
                continue;
            }
            // Anything below may leave audio in the buffer -- a chain with a
            // tail writes into one nothing fed -- so the next block empties it.
            strip.dirty = true;
            strip.sleeping = false;
            // **The decode, and the whole of console summing on this side.**
            //
            // Decode the encoded sum, then add the linear one -- which is
            // Adam's "the decode stage is mixed with master to pick up any
            // channels that don't have it switched on", generalised from the
            // master to every summing point. Doing it here rather than in a
            // device is what makes the decoding bus invisible: every bus
            // already is one, so nothing has to be placed or created.
            //
            // Before the input meter on purpose: the meter then reads what
            // the chain is actually handed, so the ceiling is visible as a
            // needle that stops climbing rather than as a number nothing
            // reports.
            if strip.console_dirty {
                if let Some(sum) = strip.console_sum.as_mut() {
                    console::decode_block(&mut sum.l[..frames], &mut sum.r[..frames]);
                    strip.bus.add_from(sum, frames);
                    sum.clear(frames);
                }
                strip.console_dirty = false;
            }
            // Polarity, at the top of the track's block: everything below
            // -- the strip, the chain, both send taps and the fader -- sees
            // the flipped signal, which is what a desk's input invert does.
            // It cannot move a meter, because it does not move a magnitude.
            //
            // A multiply by the smoothed sign: exactly -1 or nothing at all
            // once settled, and a pass through zero for the few milliseconds
            // after a flip, so the switch is a crossfade to the inverted
            // signal rather than a step of twice its level (MOO-107).
            apply_smoothed_gain(&mut strip.sign, &mut strip.bus, frames);
            // The bus head's input meter reads what the bus received this
            // block, before its own chain touches it.
            let (input_l, input_r) = strip.bus.peak(frames);
            self.device_meters
                .publish_input(MAX_CHANNELS + index, 0, input_l, input_r);
            // **The channel strip, at the pin.** One statement decides
            // whether a track's own devices run before or after its EQ and
            // compressor, and it is `mooloop_core::mixer::STRIP_PIN` rather
            // than the order of two lines here -- so moving the pin moves
            // the audio and the rack's drawing together.
            strip.strip.set_sample_rate(context.sample_rate);
            if self.strip_pin == StripPin::Head {
                strip.strip.process_block(&mut strip.bus, frames);
            }
            // The lamp beside the strip's COMP header. Published wherever
            // the strip ran, and not at all while its compressor is out --
            // `dynamics_frame` answers `None` there, the cell falls to zero
            // on the next read, and the lamp goes dark rather than holding
            // the last thing it saw.
            if let Some(frame) = strip.strip.dynamics_frame() {
                self.meters.publish_reduction(index, frame.reduction_db);
            }
            strip.effects.process(
                &context,
                &mut strip.bus,
                EffectTarget::Bus(index as u8),
                Some((
                    &self.device_meters,
                    &self.device_telemetry,
                    MAX_CHANNELS + index,
                )),
                None,
                automation.as_ref(),
                skip_idle,
            );
            let faults = std::mem::take(&mut strip.effects.faults_unpublished);
            if faults > 0 {
                self.meters.publish_effect_faults(faults);
            }
            if self.strip_pin == StripPin::Tail {
                strip.strip.process_block(&mut strip.bus, frames);
            }
            // **The master bus compressor**, on the master alone: after its
            // own devices, as a mix-bus compressor sits on the insert point,
            // and before the pre-fader send and the fader, so a fade-out on
            // the master does not ride the mix out of compression on its way
            // down (MOO-13). Every other track's section is out, and the
            // session refuses to switch one in.
            if index == MASTER_BUS as usize {
                strip.strip.process_master(&mut strip.bus, frames);
                if let Some(frame) = strip.strip.master_frame() {
                    self.meters.publish_master_comp(frame.reduction_db);
                }
            }
            let producer = EffectTarget::Bus(index as u8);
            // Mute and solo are one question here and two fields
            // everywhere else: a track silenced by someone else's solo
            // behaves exactly as a muted one -- it processes, so tails
            // decay, and contributes nothing anywhere, sends included.
            let muted = strip.output.muted || strip.solo_silenced;
            // Mute silences a track's sends, pre-fader ones included. That is
            // the reading a desk gives -- a muted strip contributes nothing
            // anywhere -- and it is what the channel loop already does by
            // skipping the whole tail of its body on a muted channel.
            //
            // **Once it has faded** (MOO-107). Until the output stage and the
            // sends have both ramped to silence, a muted track goes on
            // capturing, summing and emitting, all of it aimed at nothing;
            // only a `faded` one takes the branches below that stop it
            // contributing, and those are exactly the branches every muted
            // track took at once before.
            let faded = muted && strip.output.is_silent() && self.sends.is_silent(producer);
            if !faded {
                self.sends
                    .capture(producer, SendTap::PreFader, &strip.bus, frames);
            }
            strip.output.apply(&mut strip.bus, frames);
            if !faded {
                self.sends
                    .capture(producer, SendTap::PostFader, &strip.bus, frames);
            }
            // Before the meter and before the mute check on purpose: from here
            // the bus's audio genuinely *is* delayed, so metering the delayed
            // signal is honest, and a muted bus still advances its ring rather
            // than holding stale audio to emit when it is unmuted.
            if let Some(delay) = strip.compensation.as_mut() {
                delay.process(&mut strip.bus.l[..frames], &mut strip.bus.r[..frames]);
            }
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
                // A non-master track is silenced by `mix_into` not running.
                // The master has no `mix_into` -- it *is* the output, copied
                // straight to the ports and to an export -- so nothing was
                // applying its mute at all: the button lit, both of its
                // meters read silent because the peak above is zeroed, and
                // the audio carried on at full level. An export made in that
                // state was full level too. Buses are cleared at the top of
                // every block, so emptying it here is safe, and the preview
                // is summed after this walk on purpose and stays audible.
                //
                // The output stage fades the master like any other track now,
                // so this only makes the faded master exactly silent -- which
                // a gain settled at zero already is, for everything but a
                // non-finite sample.
                if faded {
                    strip.bus.clear(frames);
                }
                master_peak = (peak_l, peak_r);
            } else if !faded {
                let console = strip.console;
                let destination = self.bus_graph.destination(index) as usize;
                mix_into(&mut self.buses, index, destination, frames, console);
            }
            if !faded {
                self.sends.emit(producer, &mut self.buses, frames, muted);
            } else {
                self.sends.reset(producer);
            }
        }
        // After the walk on purpose: the preview bypasses every chain, so it
        // is heard raw and does not move the mixer's meters.
        // After every strip and track has rendered, so each source buffer
        // holds this block's audio; before the preview, so a resample of the
        // master never records a browser audition.
        // A resample of the master must not record a NaN either, and the
        // takes run before the limiter can -- they have to precede the
        // preview, and the limiter has to follow it. So the scrub runs twice
        // on the master: here, and inside the guard below, which is what
        // catches a preview's. Only the master, and a pass of `is_finite`.
        let scrubbed = {
            let master = &mut self.buses[MASTER_BUS as usize].bus;
            OutputGuard::scrub(&mut master.l[..frames], &mut master.r[..frames])
        };
        self.advance_takes(&spans[..span_count], ticks_per_sample, &heard);
        self.render_preview(frames);
        // **The output guard, last of all** (MOO-93). Everything that reaches
        // the ports or an export has passed it: the walk, the preview, all
        // of it. It is after the master's meter on purpose -- the meter reads
        // the mix, so a mix over 0 dBFS still lights the clip latch and says
        // that the limiter is working, rather than the limiter hiding it.
        let mut guarded = {
            let master = &mut self.buses[MASTER_BUS as usize];
            // The lookahead is the master section's (MOO-169): 0 is the
            // zero-latency guard exactly, and a change takes effect here, at
            // the top of a block.
            self.output_guard
                .set_lookahead_ms(master.strip.params().master.lookahead_ms);
            self.output_guard
                .process(&mut master.bus.l[..frames], &mut master.bus.r[..frames])
        };
        guarded.non_finite = guarded.non_finite.saturating_add(scrubbed);
        if guarded.non_finite > 0 {
            self.output_non_finite += u64::from(guarded.non_finite);
            // Published as a running count that only grows, so it *is* the
            // latched fault: the interface compares it with what it last
            // saw, and a NaN in one block between two of its reads is not
            // lost the way a flag set and cleared inside a block would be.
            self.meters.publish_output_faults(guarded.non_finite);
        }
        self.output_overs += u64::from(guarded.overs);
        let (peak_l, peak_r) = master_peak;
        RenderReport {
            position_tick: self.transport.position_ticks as u64,
            beat_in_bar: self.transport.beat_in_bar(),
            playing: self.transport.playing,
            peak_l,
            peak_r,
        }
    }

    /// Run every live take for this block (`audio-recording/03`).
    ///
    /// Each reads its channel's audio input where the block left it: a
    /// channel's output after its fader, pan and compensation (so it lines up
    /// with the track it feeds), a track's after its balance and
    /// compensation, or the master. A source that did not sound -- a muted
    /// or sleeping channel, a muted or solo-silenced track, a source that no
    /// longer exists -- records silence.
    fn advance_takes(
        &mut self,
        spans: &[crate::transport::BlockSpan],
        ticks_per_sample: f64,
        heard: &[bool; MAX_CHANNELS],
    ) {
        let live = self.live_channels();
        if !self.strips[..live]
            .iter()
            .any(|strip| strip.take.as_ref().is_some_and(|take| take.is_live()))
        {
            return;
        }
        let routing = &self.audio_input_routing;
        let playing = self.transport.playing;
        let ticks_per_bar = f64::from(mooloop_core::TICKS_PER_BAR);
        for index in 0..live {
            let Some(mut take) = self.strips[index].take.take() else {
                continue;
            };
            if take.is_live() {
                let source = match routing.taps.get(index).copied().flatten() {
                    Some(mooloop_core::AudioTap::Channel(channel)) => {
                        let channel = channel as usize;
                        (channel < live && heard[channel]).then(|| &self.strips[channel].bus)
                    }
                    Some(mooloop_core::AudioTap::Track(track)) => self
                        .buses
                        .get(track as usize)
                        .filter(|track| track.is_heard())
                        .map(|track| &track.bus),
                    Some(mooloop_core::AudioTap::Master) => {
                        Some(&self.buses[MASTER_BUS as usize].bus)
                    }
                    Some(mooloop_core::AudioTap::Input) => Some(&self.input),
                    None => None,
                };
                take.advance(spans, playing, ticks_per_sample, ticks_per_bar, source);
            }
            self.strips[index].take = Some(take);
        }
    }

    #[cfg(test)]
    pub(crate) fn take_status(&self, channel: usize) -> Option<Arc<crate::take::TakeStatus>> {
        self.strips[channel].take.as_ref().map(|take| take.status().clone())
    }

    pub fn master(&self) -> &StereoBus {
        &self.buses[MASTER_BUS as usize].bus
    }

    /// Move the strip's pinned position, for a test that renders the same
    /// project both ways. `mooloop_core::mixer::STRIP_PIN` is the policy;
    /// this only exists so the difference the policy makes can be asserted
    /// rather than argued.
    #[cfg(test)]
    pub fn set_strip_pin(&mut self, pin: StripPin) {
        self.strip_pin = pin;
    }

    /// Whether one track's strip is holding anything that could still come
    /// out of it.
    ///
    /// The only way to see the difference between `set_params` and
    /// `reset()` + `set_params` from outside: both leave the same
    /// *parameters*, and the whole of what the second one adds is state a
    /// rendered comparison can only reach through the one block it would
    /// corrupt. `a_document_arriving_clears_the_strip_it_lands_in` asks it
    /// directly instead.
    #[cfg(test)]
    pub fn strip_is_at_rest(&self, bus: usize) -> Option<bool> {
        Some(self.buses.get(bus)?.strip.is_at_rest())
    }

    /// Turn skipping idle devices and idle channels off, or back on.
    ///
    /// Not a user setting and not exposed as a command: it exists so a render
    /// can be run twice and the two compared sample for sample. A mechanism
    /// whose whole claim is that it changes nothing has to be checkable
    /// against the thing it claims not to change, and this is what makes that
    /// check a test rather than an argument.
    #[cfg(test)]
    pub fn set_idle_skipping(&mut self, enabled: bool) {
        self.skip_idle = enabled;
    }

    /// Take the output guard's limiter off, keeping its scrub, for a test
    /// that measures the *mix* above 0 dBFS. Not a user setting and not
    /// exposed as a command: `mooloop_dsp::OutputGuard::without_limit`
    /// says why it exists.
    #[cfg(test)]
    pub fn unlimit_output(&mut self) {
        self.output_guard = OutputGuard::without_limit(self.sample_rate);
    }

    /// Channel-blocks skipped since this state was built. See the field.
    #[cfg(test)]
    pub fn slept_strip_blocks(&self) -> u64 {
        self.slept_strip_blocks
    }

    /// Events refused for want of room since this state was built: an
    /// `EventList` holds `MAX_EVENTS`, and a block that resolves more
    /// automation or modulation ticks than that for one device loses the
    /// rest -- the second parameter, typically, since the first filled the
    /// list. Zero is the only right answer; a non-zero count means what was
    /// heard is not what the project says.
    ///
    /// A driven parameter reaches its device as a curve row now rather than
    /// as events, so this also counts the destinations a curve pool had no
    /// row for (`source_curve_refusals`, each chain's `curve_refusals`), and
    /// the events `AudioNode::apply_curves`'s default fallback could not fit
    /// -- which it now avoids by thinning, so a count there is a block that
    /// asked for more destinations than a list has events.
    ///
    /// Since MOO-73 every channel list counts its own refusals, so a note or
    /// choke the sequencer could not place is in here too, and so is a
    /// deferred command with no free slot.
    pub fn refused_events(&self) -> u64 {
        let chains = self
            .strips
            .iter()
            .map(|strip| &strip.effects)
            .chain(self.buses.iter().map(|bus| &bus.effects))
            .map(|chain| chain.refused_events + chain.curve_refusals);
        let lists = self.events.iter().map(|list| list.refused());
        self.refused_events + self.source_curve_refusals + chains.sum::<u64>() + lists.sum::<u64>()
    }

    /// Whether nothing in the project can sound again until something new
    /// is played: every live channel idle -- its voices at rest, its
    /// generator's tail and its chain's run out -- every track and the
    /// master resting, and no browser preview playing.
    ///
    /// The same questions idle-skipping asks, strip by strip, so a delay or
    /// a reverb whose tail is still running keeps the answer at no through
    /// the silent gaps between its echoes. What an export's tail waits for
    /// (MOO-125): an Aux In channel never answers yes, since its sound is
    /// another channel's, so a project with one tails to the cap.
    pub fn is_at_rest(&self) -> bool {
        self.preview.is_none()
            && self.preview_fading.is_none()
            && self.strips[..self.live_channels()]
                .iter()
                .all(|strip| strip.is_idle())
            && self.buses.iter().all(BusStrip::is_resting)
            // Frames still in flight in the safety limiter's lookahead are
            // part of the mix that has not left yet.
            && self.output_guard.is_at_rest()
    }

    /// How far behind the timeline everything leaving the master is, in
    /// frames: the safety limiter's lookahead (MOO-169), zero by default.
    /// What an export trims from its head so a file starts on the bar line.
    pub fn output_latency_frames(&self) -> u32 {
        let lookahead = self
            .buses
            .get(MASTER_BUS as usize)
            .map_or(0.0, |master| master.strip.params().master.lookahead_ms);
        mooloop_core::strip::lookahead_frames(lookahead, self.sample_rate) as u32
    }

    /// Samples the output guard found NaN or infinite, and sent as silence
    /// instead, since this state was built. Non-zero means a device blew up;
    /// an export reports it (`RenderSummary::non_finite_samples`).
    pub fn output_non_finite(&self) -> u64 {
        self.output_non_finite
    }

    /// Samples that reached the output guard above 0 dBFS, since this state
    /// was built. The limiter brought each of them down to the ceiling, so
    /// what left is not quite what was mixed; an export reports it
    /// (`RenderSummary::overs`).
    pub fn output_overs(&self) -> u64 {
        self.output_overs
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

/// A project with the whole track address space materialised.
///
/// `Project::default()` is the master alone since tracks stopped being a
/// fixed bank (`docs/CAPACITY_POLICY.md`), and the tests below are about the
/// *graph* rather than about how many tracks a song has, so they ask for the
/// bank they route through.
fn full_bank_project() -> Project {
    let mut project = Project::default();
    project.ensure_tracks(mooloop_core::MAX_BUSES);
    project

}

/// The same bank on its own, for the tests that build a routing graph
/// directly rather than through a project.
fn full_bank() -> Vec<mooloop_core::BusSetup> {
    full_bank_project().buses
}
    use super::*;
    use mooloop_core::{NoteEvent, ProjectChannel};

    fn test_strip() -> ChannelStrip {
        ChannelStrip::new(
            build_source(
                &DeviceKind::Sampler.default_generator_params(),
                Arc::new(ArcSwapOption::empty()),
                48_000,
            ),
            48_000,
        )
    }

    /// A strip starts, and resets, at the channel default the session and
    /// core start a channel at, not a quieter one of its own.
    #[test]
    fn a_strip_starts_and_resets_at_the_channel_default_volume() {
        let mut strip = test_strip();
        assert_eq!(strip.output.gain, mooloop_core::DEFAULT_CHANNEL_VOLUME);
        strip.output.set_volume(0.25);
        strip.reset_slot(&mut Reclaim::default());
        assert_eq!(strip.output.gain, mooloop_core::DEFAULT_CHANNEL_VOLUME);
    }

    #[test]
    fn channel_output_controls_are_bounded() {
        let mut strip = test_strip();
        strip.output.set_volume(MAX_LINEAR_GAIN + 1.0);
        strip.output.set_pan(-2.0);
        assert_eq!(strip.output.gain, MAX_LINEAR_GAIN);
        assert_eq!(strip.output.pan, -1.0);
        strip.output.set_volume(-1.0);
        strip.output.set_pan(2.0);
        assert_eq!(strip.output.gain, 0.0);
        assert_eq!(strip.output.pan, 1.0);
    }

    #[test]
    fn matching_choke_group_receives_sample_timed_choke() {
        let mut events = [
            Box::new(EventList::empty()),
            Box::new(EventList::empty()),
            Box::new(EventList::empty()),
        ];
        events[0].push(TimedEvent {
            offset: 37,
            event: Event::NoteOn {
                id: 1,
                note: 60,
                velocity: 100,
            },
        });
        inject_choke_events(&[2, 2, 3], &mut events[..]);
        assert_eq!(events[0].len(), 1);
        assert_eq!(events[1].iter().next().unwrap().event, Event::Choke);
        assert!(events[2].is_empty());
    }

    #[test]
    fn choke_is_ordered_before_a_simultaneous_note_on() {
        let mut events = [Box::new(EventList::empty()), Box::new(EventList::empty())];
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

        inject_choke_events(&[1, 1], &mut events[..]);

        for channel in &events {
            assert!(matches!(channel.iter().next().unwrap().event, Event::Choke));
            assert!(matches!(
                channel.iter().nth(1).unwrap().event,
                Event::NoteOn { .. }
            ));
        }
    }

    /// The sustain pedal, through `apply_midi` (MOO-128): with CC 64 down a
    /// released key queues no note-off and stays sustained on the channel
    /// that played it; lifting the pedal releases it there. A key pressed
    /// again under the pedal ends its sustained note before restarting, and
    /// a key released after the pedal is already up releases at once.
    #[test]
    fn the_sustain_pedal_holds_a_released_key_until_it_lifts() {
        use mooloop_core::{MidiKind, MidiMessage};

        let mut render = RenderState::new(48_000, empty_channel_audio_bank());
        render.load_project(&Project::default());
        let keyboard = Arc::new(AtomicU8::new(0));
        render.attach_keyboard_channel(keyboard);
        let message = |kind| MidiMessage {
            offset: 0,
            port: mooloop_core::MidiPortId::FIRST,
            channel: 0,
            kind,
        };
        let pedal = |value| message(MidiKind::ControlChange { controller: 64, value });
        let releases = |render: &mut RenderState| -> Vec<(u8, u8)> {
            let found = render
                .auditions
                .iter()
                .flatten()
                .filter_map(|audition| match audition.event {
                    Event::NoteOff { note, .. } => Some((audition.channel, note)),
                    _ => None,
                })
                .collect();
            render.auditions = [None; MAX_AUDITIONS_PER_BLOCK];
            found
        };

        render.apply_midi(&[message(MidiKind::NoteOn { note: 60, velocity: 100 })]);
        render.apply_midi(&[pedal(127)]);
        releases(&mut render);

        // Down edge: the key comes up and nothing is released.
        render.apply_midi(&[message(MidiKind::NoteOff { note: 60 })]);
        assert!(releases(&mut render).is_empty(), "the pedal let the note go");
        assert!(render.held_keys.is_sustained(60, 0));
        assert!(!render.held_keys.any_held(), "the key itself is up");

        // Pressed again under the pedal: the sustained note ends first.
        render.apply_midi(&[message(MidiKind::NoteOn { note: 60, velocity: 90 })]);
        assert_eq!(releases(&mut render), [(0, 60)]);
        assert!(!render.held_keys.is_sustained(60, 0));
        render.apply_midi(&[message(MidiKind::NoteOff { note: 60 })]);
        assert!(releases(&mut render).is_empty());

        // Up edge: everything the pedal held goes, on its own channel.
        render.apply_midi(&[pedal(0)]);
        assert_eq!(releases(&mut render), [(0, 60)]);
        assert!(!render.held_keys.is_sustained(60, 0));

        // With the pedal up a key releases at once again.
        render.apply_midi(&[message(MidiKind::NoteOn { note: 64, velocity: 100 })]);
        releases(&mut render);
        render.apply_midi(&[message(MidiKind::NoteOff { note: 64 })]);
        assert_eq!(releases(&mut render), [(0, 64)]);

        // A glissando under the pedal owes more releases than one block's
        // auditions hold. What does not fit is sent next block, not lost.
        render.apply_midi(&[pedal(127)]);
        for note in 0..100u8 {
            render.apply_midi(&[message(MidiKind::NoteOn { note, velocity: 100 })]);
            render.apply_midi(&[message(MidiKind::NoteOff { note })]);
            releases(&mut render);
        }
        render.apply_midi(&[pedal(0)]);
        let first = releases(&mut render).len();
        assert_eq!(first, MAX_AUDITIONS_PER_BLOCK);
        render.release_lifted_sustain(0);
        assert_eq!(first + releases(&mut render).len(), 100);
        assert!((0..100).all(|note| !render.held_keys.is_sustained(note, 0)));
    }

    /// A keyboard message on the first input, first MIDI channel.
    fn keyboard_message(offset: u32, kind: mooloop_core::MidiKind) -> mooloop_core::MidiMessage {
        mooloop_core::MidiMessage {
            offset,
            port: mooloop_core::MidiPortId::FIRST,
            channel: 0,
            kind,
        }
    }

    /// The bends waiting in `channel`'s event list, in order.
    fn bends_in(render: &RenderState, channel: usize) -> Vec<(u32, f32)> {
        render.events[channel]
            .iter()
            .filter_map(|event| match event.event {
                Event::PitchBend { semitones } => Some((event.offset, semitones)),
                _ => None,
            })
            .collect()
    }

    /// The bend wheel, through `apply_midi` (MOO-128): it reaches the
    /// selected channel as semitones at the shared ±2 range, a wheel swept
    /// through one block costs one event carrying where it ended, a value
    /// that finds the event list full is sent next block rather than lost,
    /// and a panic returns a held wheel to centre.
    #[test]
    fn the_bend_wheel_reaches_the_channel_the_keyboard_plays() {
        use mooloop_core::MidiKind;

        let mut render = RenderState::new(48_000, empty_channel_audio_bank());
        render.load_project(&Project::default());
        render.attach_keyboard_channel(Arc::new(AtomicU8::new(0)));

        // Full up is the whole range, not a hair short; full down likewise.
        assert_eq!(bend_semitones(8191), PITCH_BEND_RANGE_SEMITONES);
        assert_eq!(bend_semitones(-8192), -PITCH_BEND_RANGE_SEMITONES);
        assert_eq!(bend_semitones(0), 0.0);

        render.apply_midi(&[
            keyboard_message(3, MidiKind::PitchBend { value: 4096 }),
            keyboard_message(9, MidiKind::PitchBend { value: 8191 }),
        ]);
        render.process_block(64);
        assert_eq!(bends_in(&render, 0), [(9, 2.0)], "one event, at the last value");
        assert!(
            render.outgoing.iter().flatten().any(|event| matches!(
                event,
                mooloop_core::EngineEvent::ControlInput(message)
                    if matches!(message.kind, MidiKind::PitchBend { .. })
            )),
            "a bend is still forwarded, so it can be learned"
        );

        // Nothing moved, nothing sent: a bend is a state, not a stream.
        render.process_block(64);
        assert!(bends_in(&render, 0).is_empty());

        // A full list does not lose the wheel's return to centre.
        render.apply_midi(&[keyboard_message(0, MidiKind::PitchBend { value: 0 })]);
        render.events[0].clear();
        while render.events[0].remaining() > 0 {
            render.events[0].push(TimedEvent {
                offset: 0,
                event: Event::ParamValue { id: 0, value: 0.0 },
            });
        }
        render.dispatch_expression(63);
        assert!(bends_in(&render, 0).is_empty());
        render.process_block(64);
        assert_eq!(bends_in(&render, 0), [(0, 0.0)], "the owed bend went next block");

        // A panic lets go of a held wheel.
        render.apply_midi(&[keyboard_message(0, MidiKind::PitchBend { value: -8192 })]);
        render.process_block(64);
        assert_eq!(bends_in(&render, 0), [(0, -2.0)]);
        render.apply_command(EngineCommand::Panic);
        render.process_block(64);
        assert_eq!(bends_in(&render, 0), [(0, 0.0)]);
    }

    /// A song of one channel running `kind`.
    fn one_source_project(kind: mooloop_core::DeviceKind) -> Project {
        use mooloop_core::DeviceKind;
        let mut project = Project::default();
        project.channels.clear();
        project.channels.push(match kind {
            DeviceKind::Sampler => ProjectChannel::sampler(0, 1),
            DeviceKind::DrumSynth => ProjectChannel::drum_synth(0, 1),
            DeviceKind::MonoSynth => ProjectChannel::mono_synth(0, 1),
            DeviceKind::PolySynth => ProjectChannel::poly_synth(0, 1),
            DeviceKind::MlM1 => ProjectChannel::mlm1(0, 1),
            DeviceKind::MlP8 => ProjectChannel::mlp8(0, 1),
            DeviceKind::Ds01 => ProjectChannel::ds01(0, 1),
            DeviceKind::AuxIn => ProjectChannel::aux_in(0, 1),
        });
        project
    }

    /// The mod wheel and both kinds of aftertouch, through `apply_midi`, are
    /// modulation sources on every source (MOO-128). A route from each to
    /// one of the device's own parameters moves what the device is handed
    /// when the keyboard moves, the modulator meters show the band, and a
    /// panic puts it back to rest.
    #[test]
    fn the_mod_wheel_and_aftertouch_modulate_every_source() {
        use mooloop_core::modulation::performance_slot;
        use mooloop_core::{DeviceKind, MidiKind, ModPolarity, ModRoute, ParamOwner};

        let kinds = [
            DeviceKind::Sampler,
            DeviceKind::DrumSynth,
            DeviceKind::MonoSynth,
            DeviceKind::PolySynth,
            DeviceKind::MlM1,
            DeviceKind::MlP8,
            DeviceKind::Ds01,
            DeviceKind::AuxIn,
        ];
        let gestures = [
            (PERFORMANCE_MOD_WHEEL, MidiKind::ControlChange { controller: 1, value: 127 }),
            (PERFORMANCE_AFTERTOUCH, MidiKind::ChannelPressure { value: 127 }),
            (PERFORMANCE_AFTERTOUCH, MidiKind::PolyPressure { note: 60, value: 127 }),
        ];
        // What the source is handed for `id` this block, last tick.
        let driven = |render: &RenderState, id: u32| -> Option<f32> {
            let mut buf: [ControlCurve<'_>; MAX_SOURCE_CURVE_DESTINATIONS] =
                std::array::from_fn(|_| ControlCurve::default());
            let count = render.source_curves[0].fill(&mut buf);
            buf[..count]
                .iter()
                .find(|curve| curve.id == id)
                .and_then(|curve| curve.values.last().copied())
        };
        for kind in kinds {
            let project = one_source_project(kind);
            let descriptor = kind
                .descriptors()
                .iter()
                .find(|descriptor| ModDestinationDescriptor::for_param(descriptor).allowed)
                .unwrap_or_else(|| panic!("{kind:?} has no modulatable parameter"));
            for (source, gesture) in gestures {
                let mut render = RenderState::from_project(48_000, &project, &[]);
                render.attach_keyboard_channel(Arc::new(AtomicU8::new(0)));
                let knob = render.strips[0]
                    .source_base
                    .get(descriptor.id)
                    .map(|value| descriptor.to_normalized(value))
                    .expect("the device has the parameter");
                // Towards the far end of the knob, so the offset is not
                // clamped away.
                let depth = if knob > 0.5 { -0.5 } else { 0.5 };
                render.apply_command(EngineCommand::SetModRoute {
                    channel: 0,
                    route: ModRoute::from_performance(
                        source,
                        ParamAddr {
                            scope: EffectTarget::Channel(0),
                            owner: ParamOwner::Source,
                            param: descriptor.id,
                        },
                        depth,
                        ModPolarity::Bipolar,
                    ),
                });
                render.process_block(128);
                let at_rest = driven(&render, descriptor.id).expect("a routed parameter is driven");

                render.apply_midi(&[keyboard_message(0, gesture)]);
                render.process_block(128);
                let moved = driven(&render, descriptor.id).expect("a routed parameter is driven");
                assert!(
                    (descriptor.to_normalized(moved) - descriptor.to_normalized(at_rest)).abs() > 0.4,
                    "{kind:?} {gesture:?}: {} stayed at {at_rest} (moved to {moved})",
                    descriptor.name
                );
                assert_eq!(
                    render.modulator_meters.read(0)[usize::from(performance_slot(source))],
                    1.0,
                    "the shelf's meter shows the band"
                );

                render.apply_command(EngineCommand::Panic);
                render.process_block(128);
                assert_eq!(render.expression[0].performance, [0.0; PERFORMANCE_SOURCES]);
                let rested = driven(&render, descriptor.id).expect("a routed parameter is driven");
                assert!((rested - at_rest).abs() < 1e-6, "{kind:?}: a panic left the band up");
            }
        }
    }

    /// Every pitched source bends (MOO-128). A note played with the wheel
    /// fully up, through `apply_midi`, sounds where the key a whole tone
    /// higher sounds with the wheel at rest -- measured, on the master, for
    /// each of the seven sources that have a pitch. Aux In has none.
    #[test]
    fn every_pitched_source_plays_a_bent_note_a_whole_tone_up() {
        use mooloop_core::{DeviceKind, MidiKind};
        use mooloop_dsp::testkit::{cents, dominant_hz};

        let kinds = [
            DeviceKind::Sampler,
            DeviceKind::DrumSynth,
            DeviceKind::MonoSynth,
            DeviceKind::PolySynth,
            DeviceKind::MlM1,
            DeviceKind::MlP8,
            DeviceKind::Ds01,
        ];
        let played = |kind: DeviceKind, note: u8, bend: i16| -> f32 {
            let project = one_source_project(kind);
            // The sampler needs something to play: a second of a pure tone,
            // so the pitch it is played at is the one measured.
            let tone = Arc::new(SampleData {
                frames: (0..48_000)
                    .map(|frame| {
                        let s = (frame as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin();
                        [s * 0.5, s * 0.5]
                    })
                    .collect(),
                sample_rate: 48_000,
                root_note: 69,
            });
            let mut render = RenderState::from_project(48_000, &project, &[Some(tone)]);
            render.attach_keyboard_channel(Arc::new(AtomicU8::new(0)));
            render.apply_midi(&[
                keyboard_message(0, MidiKind::PitchBend { value: bend }),
                keyboard_message(0, MidiKind::NoteOn { note, velocity: 110 }),
            ]);
            let (left, _) = crate::render_test_support::render_frames(&mut render, 12_000);
            dominant_hz(&left, 48_000, 40.0)
        };
        for kind in kinds {
            // High enough that a drum's body is well clear of the lowest
            // bins, where a whole tone is a bin or two.
            let note = 72;
            let at_rest = played(kind, note, 0);
            let bent = played(kind, note, 8191);
            let a_tone_up = played(kind, note + 2, 0);
            assert!(
                cents(bent, a_tone_up).abs() < 20.0,
                "{kind:?}: bent to {bent} Hz, a whole tone up is {a_tone_up} Hz"
            );
            assert!(
                cents(bent, at_rest) > 180.0,
                "{kind:?}: bent to {bent} Hz from {at_rest} Hz"
            );
        }
    }

    /// A MIDI keyboard plays the selected channel with the transport stopped,
    /// and a key comes up on the channel it went down on. Holding a key while
    /// the selection moves is exactly how a performer changes sounds, and a
    /// release routed to the new channel would leave the old one sounding.
    #[test]
    fn a_keyboard_key_is_released_on_the_channel_it_went_down_on() {
        use mooloop_core::{MidiKind, MidiMessage};

        // A full second on both channels, so a note left held is still
        // sounding when the check looks.
        let sample = Arc::new(SampleData {
            frames: vec![[0.5, -0.5]; 48_000],
            sample_rate: 48_000,
            root_note: 60,
        });
        let slots = empty_channel_audio_bank();
        slots[0].store(Some(Arc::new(ChannelAudioSnapshot::sample(sample.clone()))));
        slots[1].store(Some(Arc::new(ChannelAudioSnapshot::sample(sample))));
        let mut render = RenderState::new(48_000, slots);
        let mut project = Project::default();
        project.channels.push(ProjectChannel::sampler(1, 1));
        render.load_project(&project);
        let keyboard = Arc::new(AtomicU8::new(NO_KEYBOARD_CHANNEL));
        render.attach_keyboard_channel(keyboard.clone());
        let sounding = |render: &RenderState, channel: usize| {
            !render.strips[channel].source.as_sampler().expect("a sampler channel").voice_positions()[0].is_nan()
        };
        let key = |kind| MidiMessage {
            offset: 0,
            port: mooloop_core::MidiPortId::FIRST,
            channel: 0,
            kind,
        };

        // With no channel to play, a key plays nothing and holds nothing.
        render.apply_midi(&[key(MidiKind::NoteOn {
            note: 60,
            velocity: 100,
        })]);
        render.process_block(128);
        assert!(!sounding(&render, 0) && !sounding(&render, 1));
        assert!(!render.held_keys.any_held());

        keyboard.store(0, Ordering::Relaxed);
        render.apply_midi(&[key(MidiKind::NoteOn {
            note: 62,
            velocity: 100,
        })]);
        render.process_block(128);
        assert!(sounding(&render, 0), "the key should play the selected channel");
        assert!(!sounding(&render, 1));

        // The selection moves to the second channel with the key still down,
        // and then the key comes up. Asserted on the queued release rather
        // than on the voice: the default sampler is one-shot and ignores a
        // note-off, so silence could not tell a right release from a lost one.
        keyboard.store(1, Ordering::Relaxed);
        render.apply_midi(&[key(MidiKind::NoteOff { note: 62 })]);
        let releases: Vec<_> = render
            .auditions
            .iter()
            .flatten()
            .map(|audition| (audition.channel, audition.event))
            .collect();
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].0, 0, "the release went to a channel the key never started");
        assert!(matches!(
            releases[0].1,
            Event::NoteOff { id, note: 62 } if id == keyboard_note_id(62)
        ));
        assert!(!render.held_keys.any_held());
        render.process_block(128);
        assert!(!sounding(&render, 1), "the second channel was never played");
    }

    /// A render state with two sampler channels, each holding a second of
    /// audio, for the MIDI routing tests. They need a channel bank and voices
    /// that outlast a block; none of them cares what the audio is.
    fn two_channel_render() -> RenderState {
        let sample = Arc::new(SampleData {
            frames: vec![[0.5, -0.5]; 48_000],
            sample_rate: 48_000,
            root_note: 60,
        });
        let slots = empty_channel_audio_bank();
        slots[0].store(Some(Arc::new(ChannelAudioSnapshot::sample(sample.clone()))));
        slots[1].store(Some(Arc::new(ChannelAudioSnapshot::sample(sample))));
        let mut render = RenderState::new(48_000, slots);
        let mut project = Project::default();
        project.channels.push(ProjectChannel::sampler(1, 1));
        render.load_project(&project);
        render
    }

    /// Two channels listening to different MIDI channels play their own notes
    /// and not each other's. This is what multitimbral means, and it is the
    /// point of the whole input setting.
    #[test]
    fn channels_listening_on_different_midi_channels_play_their_own_notes() {
        use mooloop_core::{
            MidiChannelFilter, MidiInputRoute, MidiKind, MidiMessage, MidiPortId, MidiRouteSource,
        };

        let mut render = two_channel_render();
        // Channel 0 takes MIDI channel 1, channel 1 takes MIDI channel 10.
        let routing = Box::new(MidiRouting {
            routes: vec![
                MidiInputRoute {
                    source: MidiRouteSource::AllPorts,
                    channel: MidiChannelFilter::One(0),
                },
                MidiInputRoute {
                    source: MidiRouteSource::AllPorts,
                    channel: MidiChannelFilter::One(9),
                },
            ],
        });
        drop(render.set_midi_routing(routing));
        let note = |channel, note| MidiMessage {
            offset: 0,
            port: MidiPortId::FIRST,
            channel,
            kind: MidiKind::NoteOn {
                note,
                velocity: 100,
            },
        };

        render.apply_midi(&[note(0, 60), note(9, 62)]);
        let played: Vec<_> = render
            .auditions
            .iter()
            .flatten()
            .map(|audition| audition.channel)
            .collect();
        assert_eq!(played, vec![0, 1]);

        // A note on a MIDI channel neither wants plays nothing -- not even
        // the selected channel, which has an explicit route of its own.
        render.process_block(128);
        render.keyboard_channel.store(0, Ordering::Relaxed);
        render.apply_midi(&[note(5, 64)]);
        assert_eq!(render.auditions.iter().flatten().count(), 0);
    }

    /// A channel that claims a note is not *also* played by being selected.
    /// Adding the selection to the claimants would sound the note twice on the
    /// same channel, which reads as a stuck 6 dB rather than as a bug.
    #[test]
    fn a_claiming_channel_is_not_played_twice_for_being_selected() {
        use mooloop_core::{
            MidiChannelFilter, MidiInputRoute, MidiKind, MidiMessage, MidiPortId, MidiRouteSource,
        };

        let mut render = two_channel_render();
        render.set_midi_routing(Box::new(MidiRouting {
            routes: vec![MidiInputRoute {
                source: MidiRouteSource::AllPorts,
                channel: MidiChannelFilter::Omni,
            }],
        }));
        render.keyboard_channel.store(0, Ordering::Relaxed);
        render.apply_midi(&[MidiMessage {
            offset: 0,
            port: MidiPortId::FIRST,
            channel: 0,
            kind: MidiKind::NoteOn {
                note: 60,
                velocity: 100,
            },
        }]);
        let played: Vec<_> = render
            .auditions
            .iter()
            .flatten()
            .map(|audition| audition.channel)
            .collect();
        assert_eq!(played, vec![0], "the selected channel claimed it once");
    }

    /// Two channels holding one note both let go of it. The single byte this
    /// bitset replaced could only remember one of them, so the second would
    /// have been left sounding.
    #[test]
    fn a_note_held_on_two_channels_is_released_on_both() {
        use mooloop_core::{
            MidiChannelFilter, MidiInputRoute, MidiKind, MidiMessage, MidiPortId, MidiRouteSource,
        };

        let mut render = two_channel_render();
        let both = MidiInputRoute {
            source: MidiRouteSource::AllPorts,
            channel: MidiChannelFilter::Omni,
        };
        render.set_midi_routing(Box::new(MidiRouting {
            routes: vec![both, both],
        }));
        let message = |kind| MidiMessage {
            offset: 0,
            port: MidiPortId::FIRST,
            channel: 0,
            kind,
        };

        render.apply_midi(&[message(MidiKind::NoteOn {
            note: 60,
            velocity: 100,
        })]);
        render.process_block(128);
        render.apply_midi(&[message(MidiKind::NoteOff { note: 60 })]);
        let released: Vec<_> = render
            .auditions
            .iter()
            .flatten()
            .map(|audition| audition.channel)
            .collect();
        assert_eq!(released, vec![0, 1]);
        assert!(!render.held_keys.any_held());
    }

    /// Control and transport messages leave for the control layer and play
    /// nothing. A Start message arriving while a channel listens Omni must not
    /// be heard as a note.
    #[test]
    fn control_and_transport_messages_are_forwarded_rather_than_played() {
        use mooloop_core::{
            EngineEvent, MidiChannelFilter, MidiInputRoute, MidiKind, MidiMessage, MidiPortId,
            MidiRouteSource, SYSTEM_CHANNEL,
        };

        let mut render = two_channel_render();
        render.set_midi_routing(Box::new(MidiRouting {
            routes: vec![MidiInputRoute {
                source: MidiRouteSource::AllPorts,
                channel: MidiChannelFilter::Omni,
            }],
        }));
        let cc = MidiMessage {
            offset: 0,
            port: MidiPortId::FIRST,
            channel: 2,
            kind: MidiKind::ControlChange {
                controller: 74,
                value: 100,
            },
        };
        let start = MidiMessage {
            offset: 0,
            port: MidiPortId::FIRST,
            channel: SYSTEM_CHANNEL,
            kind: MidiKind::Start,
        };
        let bend = MidiMessage {
            offset: 0,
            port: MidiPortId::FIRST,
            channel: 2,
            kind: MidiKind::PitchBend { value: 1000 },
        };

        render.apply_midi(&[cc, start, bend]);
        assert_eq!(
            render.auditions.iter().flatten().count(),
            0,
            "nothing about a control message is a note"
        );
        let forwarded: Vec<_> = std::iter::from_fn(|| render.pop_outgoing()).collect();
        assert_eq!(
            forwarded,
            vec![
                EngineEvent::ControlInput(cc),
                EngineEvent::ControlInput(start),
                EngineEvent::ControlInput(bend),
            ]
        );
    }

    /// A pad can be learned, and a learned pad fires rather than plays.
    ///
    /// MOO-129: `apply_midi` gave every note to an instrument, so a pad
    /// pressed during learn only ever sounded and a Note binding never heard
    /// anything -- while the session's own test, which hands the session the
    /// message directly, stayed green. This one starts where the key arrives:
    /// the engine is told a learn is waiting, the pad goes up instead of
    /// sounding, the learn that the *forwarded* message completes is what
    /// claims the pad, and the next press goes up again and fires its target.
    #[test]
    fn a_pad_is_learned_through_the_engine_and_then_fires_instead_of_playing() {
        use mooloop_core::{
            ClaimedNotes, ControlLearn, ControlMap, ControlMapState, ControlOutcome,
            ControlTarget, EngineEvent, MidiChannelFilter, MidiInputRoute, MidiKind, MidiMessage,
            MidiPortId, MidiPortInfo, MidiRouteSource, TransportControl,
        };

        let mut render = two_channel_render();
        render.set_midi_routing(Box::new(MidiRouting {
            routes: vec![MidiInputRoute {
                source: MidiRouteSource::AllPorts,
                channel: MidiChannelFilter::Omni,
            }],
        }));
        let ports = vec![MidiPortInfo {
            id: MidiPortId::FIRST,
            name: "Pads".to_owned(),
        }];
        let key = |kind| MidiMessage {
            offset: 0,
            port: MidiPortId::FIRST,
            channel: 9,
            kind,
        };
        let pad_down = key(MidiKind::NoteOn {
            note: 36,
            velocity: 110,
        });
        let pad_up = key(MidiKind::NoteOff { note: 36 });
        let forwarded = |render: &mut RenderState| -> Vec<EngineEvent> {
            std::iter::from_fn(|| render.pop_outgoing()).collect()
        };
        let sounding = |render: &RenderState| render.auditions.iter().flatten().count();

        // Nothing claimed: a key is a note, and nothing goes up.
        render.apply_midi(&[key(MidiKind::NoteOn {
            note: 38,
            velocity: 100,
        })]);
        assert!(render.key_is_held(38, 0));
        assert!(forwarded(&mut render).is_empty());

        // A learn is waiting. The pad goes up and sounds nothing.
        let learn = ControlLearn {
            target: ControlTarget::Transport(TransportControl::Play),
            bind_port: true,
            replaces: None,
        };
        let map = ControlMap::default();
        drop(render.set_claimed_notes(Box::new(map.claimed_notes(&ports, true))));
        let before = sounding(&render);
        render.apply_midi(&[pad_down]);
        assert_eq!(sounding(&render), before, "a pad pressed during learn is not played");
        assert!(!render.key_is_held(36, 0));
        let up = forwarded(&mut render);
        assert_eq!(up, vec![EngineEvent::ControlInput(pad_down)]);

        // A key that went down as a note before the learn still lifts: its
        // release goes up too, and the note it started stops.
        render.apply_midi(&[key(MidiKind::NoteOff { note: 38 })]);
        assert!(!render.key_is_held(38, 0), "a claimed release still lifts a held key");
        assert_eq!(forwarded(&mut render).len(), 1);

        // The control layer learns from what the engine sent, not from a
        // message the test made up.
        let EngineEvent::ControlInput(message) = up[0] else {
            unreachable!()
        };
        let mut map = map;
        map.bind(learn.resolve(&message, &ports).expect("a pad completes a learn"));
        let mut state = ControlMapState::default();
        state.resolve(&map, &ports);
        let claimed: ClaimedNotes = map.claimed_notes(&ports, false);
        drop(render.set_claimed_notes(Box::new(claimed)));

        // The bound pad fires its target and plays no note, press and release.
        let before = sounding(&render);
        render.apply_midi(&[pad_up, pad_down]);
        assert_eq!(sounding(&render), before);
        assert!(!render.key_is_held(36, 0));
        let up = forwarded(&mut render);
        assert_eq!(
            up,
            vec![
                EngineEvent::ControlInput(pad_up),
                EngineEvent::ControlInput(pad_down)
            ]
        );
        let EngineEvent::ControlInput(press) = up[1] else {
            unreachable!()
        };
        assert_eq!(
            state.apply(&map, &press, |_| 0.0),
            vec![(0, ControlOutcome::Fire)]
        );

        // Any other key is still a note.
        render.apply_midi(&[key(MidiKind::NoteOn {
            note: 37,
            velocity: 100,
        })]);
        assert!(render.key_is_held(37, 0));
        assert!(forwarded(&mut render).is_empty());
    }

    /// A swap takes the outgoing renderer's held keys and its open take notes,
    /// not just its transport.
    ///
    /// **Verified failing before the fix**, with `adopt_performance_state`
    /// copying only the transport: the incoming renderer held no key and had
    /// no open note, which is the tree as it stood on 2026-09-18.
    ///
    /// The cost of not carrying them was invisible for as long as a structural
    /// edit stopped the song -- the voices went with the old renderer anyway.
    /// `channel-identity/04` is what made it audible: once the transport
    /// survives, a key held across a channel move has its note-off delivered
    /// to a renderer that never saw the press, so `held_keys.release` finds
    /// nothing and the note never lifts.
    #[test]
    fn a_swap_carries_the_keys_that_are_down_and_the_notes_being_taken() {
        use mooloop_core::{
            MidiChannelFilter, MidiInputRoute, MidiKind, MidiMessage, MidiPortId, MidiRouteSource,
        };

        let routing = || {
            Box::new(MidiRouting {
                routes: vec![MidiInputRoute {
                    source: MidiRouteSource::AllPorts,
                    channel: MidiChannelFilter::Omni,
                }],
            })
        };
        let mut outgoing = two_channel_render();
        drop(outgoing.set_midi_routing(routing()));
        outgoing.set_record_armed(true);
        outgoing.play();
        outgoing.apply_midi(&[MidiMessage {
            offset: 0,
            port: MidiPortId::FIRST,
            channel: 0,
            kind: MidiKind::NoteOn {
                note: 60,
                velocity: 90,
            },
        }]);
        outgoing.process_block(256);
        assert!(outgoing.key_is_held(60, 0), "the premise: a key is down");
        assert_eq!(
            outgoing.capturing(60),
            Some(0),
            "the premise: a take has that note open"
        );

        // What an install builds: a fresh renderer that has seen nothing.
        let mut incoming = two_channel_render();
        drop(incoming.set_midi_routing(routing()));
        assert!(!incoming.any_key_is_held());
        assert_eq!(incoming.capturing(60), None);

        incoming.adopt_performance_state(&outgoing);

        assert!(
            incoming.key_is_held(60, 0),
            "the key went down on channel 0 and the swap lost it"
        );
        assert_eq!(
            incoming.capturing(60),
            Some(0),
            "the note was open in the take and the swap dropped it"
        );

        // And the release lands: it reaches the channel the press went to,
        // which is the whole point of carrying the set rather than clearing
        // it. A renderer that never saw the press has nothing to release.
        incoming.set_record_armed(true);
        incoming.play();
        incoming.apply_midi(&[MidiMessage {
            offset: 0,
            port: MidiPortId::FIRST,
            channel: 0,
            kind: MidiKind::NoteOff { note: 60 },
        }]);
        incoming.process_block(256);
        assert!(!incoming.any_key_is_held(), "the note-off did not lift the key");
        assert_eq!(incoming.capturing(60), None, "the take note was left open");
    }

    /// Recording reports a note when its key comes up, with the position it
    /// was played at and the length it was actually held.
    #[test]
    fn a_recorded_note_carries_its_position_and_its_length() {
        use mooloop_core::{
            EngineEvent, MidiChannelFilter, MidiInputRoute, MidiKind, MidiMessage, MidiPortId,
            MidiRouteSource,
        };

        let mut render = two_channel_render();
        render.set_midi_routing(Box::new(MidiRouting {
            routes: vec![MidiInputRoute {
                source: MidiRouteSource::AllPorts,
                channel: MidiChannelFilter::Omni,
            }],
        }));
        let message = |offset, kind| MidiMessage {
            offset,
            port: MidiPortId::FIRST,
            channel: 0,
            kind,
        };
        let down = |offset| {
            message(
                offset,
                MidiKind::NoteOn {
                    note: 60,
                    velocity: 90,
                },
            )
        };
        let up = |offset| message(offset, MidiKind::NoteOff { note: 60 });

        // Armed but stopped: nothing is captured, because a recorder that
        // wrote while the transport was parked would fill the pattern's first
        // tick with everything anybody played.
        render.set_record_armed(true);
        render.apply_midi(&[down(0)]);
        render.process_block(128);
        render.apply_midi(&[up(0)]);
        render.process_block(128);
        assert_eq!(std::iter::from_fn(|| render.pop_outgoing()).count(), 0);

        // Running: one note, at the position it was played and for as long as
        // it was held. At 120 bpm and 48 kHz a quarter note is 24 000 frames,
        // and the transport's own resolution is 96 ticks to the quarter.
        render.play();
        render.apply_midi(&[down(0)]);
        for _ in 0..93 {
            render.process_block(256);
        }
        // 93 blocks of 256 is 23 808 frames, a hair under a quarter note.
        render.apply_midi(&[up(0)]);
        render.process_block(256);
        let recorded: Vec<_> = std::iter::from_fn(|| render.pop_outgoing()).collect();
        let [EngineEvent::RecordedNote {
            channel,
            pattern,
            note,
            velocity,
            start_tick,
            length_ticks,
        }] = recorded[..]
        else {
            panic!("expected exactly one recorded note, got {recorded:?}");
        };
        assert_eq!((channel, pattern, note, velocity), (0, 0, 60, 90));
        assert_eq!(start_tick, 0, "it was played at the top of the pattern");
        assert!(
            (94..=96).contains(&length_ticks),
            "a note held just under a quarter should be just under 96 ticks, was {length_ticks}"
        );

        // Disarming abandons a note still down rather than reporting a
        // half-measured one.
        render.apply_midi(&[down(0)]);
        render.process_block(256);
        render.set_record_armed(false);
        render.apply_midi(&[up(0)]);
        render.process_block(256);
        assert_eq!(std::iter::from_fn(|| render.pop_outgoing()).count(), 0);

        // A note on the second pass lands where it was played *in the
        // pattern*. The pattern-mode transport never folds -- the sequencer
        // wraps its own copy of the position -- so the playhead's tick is 144
        // here, and the session, which takes `start_tick` as a pattern
        // position, used to clamp that onto the last tick of the pattern.
        render.apply_command(EngineCommand::Stop);
        render.apply_command(EngineCommand::SetPatternLength {
            pattern: 0,
            length_steps: 4,
        });
        render.set_record_armed(true);
        render.play();
        // 140 blocks of 256 and an offset of 160 is 36 000 frames: one and a
        // half quarter notes, which in a 96-tick pattern is tick 48.
        for _ in 0..140 {
            render.process_block(256);
        }
        render.apply_midi(&[down(160)]);
        render.process_block(256);
        render.apply_midi(&[up(0)]);
        render.process_block(256);
        let recorded: Vec<_> = std::iter::from_fn(|| render.pop_outgoing()).collect();
        let [EngineEvent::RecordedNote { start_tick, .. }] = recorded[..] else {
            panic!("expected exactly one recorded note, got {recorded:?}");
        };
        assert_eq!(start_tick, 48, "the second pass folds into the pattern");
    }

    /// In song mode a note records into the selected pattern at its offset
    /// inside the placement under the playhead, and is not recorded at all
    /// where no placement of that pattern is playing.
    #[test]
    fn a_note_recorded_in_song_mode_lands_in_the_placement_under_the_playhead() {
        use mooloop_core::{
            EngineEvent, MidiChannelFilter, MidiInputRoute, MidiKind, MidiMessage, MidiPortId,
            MidiRouteSource, PlaybackMode, TICKS_PER_BAR,
        };

        let mut render = two_channel_render();
        render.set_midi_routing(Box::new(MidiRouting {
            routes: vec![MidiInputRoute {
                source: MidiRouteSource::AllPorts,
                channel: MidiChannelFilter::Omni,
            }],
        }));
        // A one-quarter pattern placed on the second bar, and a second
        // pattern with no placement at all.
        render.apply_command(EngineCommand::AddPattern);
        render.apply_command(EngineCommand::SetPatternLength {
            pattern: 0,
            length_steps: 4,
        });
        render.apply_command(EngineCommand::SetPlaylistPlacement {
            pattern: 0,
            start_tick: TICKS_PER_BAR,
            on: true,
        });
        render.apply_command(EngineCommand::SetPlaybackMode(PlaybackMode::Song));
        render.set_record_armed(true);
        render.play();

        // Press `offset` frames into the next block, release a block later,
        // and return what was reported.
        let tap = |render: &mut RenderState, offset| {
            let message = |offset, kind| MidiMessage {
                offset,
                port: MidiPortId::FIRST,
                channel: 0,
                kind,
            };
            render.apply_midi(&[message(
                offset,
                MidiKind::NoteOn {
                    note: 60,
                    velocity: 90,
                },
            )]);
            render.process_block(256);
            render.apply_midi(&[message(0, MidiKind::NoteOff { note: 60 })]);
            render.process_block(256);
            std::iter::from_fn(|| render.pop_outgoing()).collect::<Vec<_>>()
        };

        // Tick 144 of the first bar: nothing of pattern 0 is playing.
        for _ in 0..140 {
            render.process_block(256);
        }
        assert_eq!(tap(&mut render, 160), vec![], "no placement covers the playhead");

        // Tick 432 is 48 ticks into the placement at 384: 108 000 frames, or
        // 421 blocks of 256 and an offset of 224. 142 blocks have run.
        for _ in 142..421 {
            render.process_block(256);
        }
        let recorded = tap(&mut render, 224);
        let [EngineEvent::RecordedNote { start_tick, .. }] = recorded[..] else {
            panic!("expected exactly one recorded note, got {recorded:?}");
        };
        assert_eq!(start_tick, 48, "the offset inside the placement");

        // The same position with pattern 1 selected: pattern 0 is what is
        // playing, and pattern 1 has no placement here to record into.
        render.apply_command(EngineCommand::SetCurrentPattern(1));
        render.apply_command(EngineCommand::Seek {
            tick: f64::from(TICKS_PER_BAR + 48),
        });
        assert_eq!(tap(&mut render, 0), vec![], "the selected pattern is not playing");
    }

    /// A recorded note names the pattern it was folded into, and that is the
    /// pattern selected when the key went down -- not the one selected when it
    /// came up.
    #[test]
    fn a_recorded_note_names_the_pattern_it_was_played_over() {
        use mooloop_core::{
            EngineEvent, MidiChannelFilter, MidiInputRoute, MidiKind, MidiMessage, MidiPortId,
            MidiRouteSource,
        };

        let mut render = two_channel_render();
        render.set_midi_routing(Box::new(MidiRouting {
            routes: vec![MidiInputRoute {
                source: MidiRouteSource::AllPorts,
                channel: MidiChannelFilter::Omni,
            }],
        }));
        render.apply_command(EngineCommand::AddPattern);
        render.set_record_armed(true);
        render.play();
        let message = |kind| MidiMessage {
            offset: 0,
            port: MidiPortId::FIRST,
            channel: 0,
            kind,
        };

        render.process_block(256);
        render.apply_midi(&[message(MidiKind::NoteOn {
            note: 60,
            velocity: 90,
        })]);
        render.process_block(256);
        // The selection moves with the key still down.
        render.apply_command(EngineCommand::SetCurrentPattern(1));
        render.apply_midi(&[message(MidiKind::NoteOff { note: 60 })]);
        render.process_block(256);
        let recorded: Vec<_> = std::iter::from_fn(|| render.pop_outgoing()).collect();
        let [EngineEvent::RecordedNote { pattern, .. }] = recorded[..] else {
            panic!("expected exactly one recorded note, got {recorded:?}");
        };
        assert_eq!(pattern, 0, "the pattern the press was folded into");
    }

    /// Auditioning has to work with the transport stopped -- that is the
    /// whole gesture -- and the note has to go through the channel's own
    /// device, not past it, or a slice would be auditioned without the
    /// envelopes, filter and drive it will actually play through.
    #[test]
    fn an_auditioned_note_sounds_a_stopped_channel_through_its_own_device() {
        // A full second, so the note outlasts the release window the check
        // below is looking through rather than simply running out of audio.
        let sample = Arc::new(SampleData {
            frames: vec![[0.5, -0.5]; 48_000],
            sample_rate: 48_000,
            root_note: 60,
        });
        let slots = empty_channel_audio_bank();
        slots[0].store(Some(Arc::new(ChannelAudioSnapshot::sample(sample))));
        let mut render = RenderState::new(48_000, slots);
        render.load_project(&Project::default());
        assert!(!render.transport.playing);

        // Stopped and untouched, the channel is silent.
        render.process_block(128);
        assert!(render.strips[0].source.as_sampler().expect("a sampler channel").voice_positions()[0].is_nan());

        render.apply_command(EngineCommand::TriggerChannelNote {
            channel: 0,
            note: 60,
            velocity: 127,
        });
        let opening = render.process_block(128);
        assert!(
            !render.strips[0].source.as_sampler().expect("a sampler channel").voice_positions()[0].is_nan(),
            "the audition should have started a voice on the channel's sampler"
        );
        assert!(opening.peak_l > 0.01, "the audition made no sound");

        // And it has to keep sounding at level. A stopped transport used to
        // release every voice on every block, which put an audition into
        // release one block after it began. Measured as level rather than as
        // playhead position on purpose: a releasing voice is still an active
        // voice and its head keeps advancing, so a position check passes
        // straight through the bug it is meant to catch.
        //
        // Fifty blocks is 133 ms, comfortably past the 50 ms default release.
        let mut peak = opening.peak_l;
        for block in 0..50 {
            let report = render.process_block(128);
            assert!(
                report.peak_l > opening.peak_l * 0.9,
                "the auditioned note decayed by block {block}: {} vs {}",
                report.peak_l,
                opening.peak_l
            );
            peak = report.peak_l;
        }
        assert!(peak > 0.01);

        // An audition is consumed by the block it arrives in, not replayed.
        assert!(render.auditions.iter().all(Option::is_none));

        // Pressing stop still cuts sounding voices: the release moved to the
        // transition, it did not go away. The default release is 50 ms, so
        // give it comfortably longer than that.
        render.transport.play();
        render.process_block(128);
        render.transport.stop();
        for _ in 0..40 {
            render.process_block(128);
        }
        assert!(
            render.strips[0].source.as_sampler().expect("a sampler channel").voice_positions()[0].is_nan(),
            "stopping the transport must still release what is sounding"
        );
    }

    /// The acceptance case for the song loop: a two-bar arrangement with a
    /// note in its first bar, looped over that bar, plays the note every bar
    /// instead of every two, and never leaves the section.
    ///
    /// Asserted on scheduled events rather than on the transport, because the
    /// transport folding correctly and the sequencer being handed the folded
    /// span are two different things and only the second one is audible.
    #[test]
    fn a_song_loop_replays_its_section_and_stays_inside_it() {
        use mooloop_core::{PatternPlacement, TICKS_PER_BAR, TICKS_PER_STEP};

        let mut project = Project::default();
        // Two one-bar patterns. The second is empty and exists to make the
        // song two bars long, so that a loop over the first bar has something
        // to be shorter than.
        project.pattern_lengths.push(DEFAULT_STEPS);
        project.channels[0].notes.push(Vec::new());
        project.channels[0].automation.push(Vec::new());
        project.channels[0].notes[0].push(NoteEvent::new(1, 0, TICKS_PER_STEP, 60, 100));
        project.playlist = vec![
            PatternPlacement::new(0, 0),
            PatternPlacement::new(1, TICKS_PER_BAR),
        ];
        project.playback_mode = PlaybackMode::Song;

        let slots = empty_channel_audio_bank();

        // Two bars of elapsed time, in blocks big enough to keep the count
        // down and to make a block that straddles the loop point likely.
        let two_bars = |render: &mut RenderState| {
            let mut hits = Vec::new();
            for _ in 0..48 {
                let before = render.transport.position_ticks;
                render.process_block(4_096);
                if render.events[0]
                    .iter()
                    .any(|event| matches!(event.event, Event::NoteOn { .. }))
                {
                    hits.push(before);
                }
            }
            hits
        };

        let mut render = RenderState::new(48_000, slots.clone());
        render.load_project(&project);
        render.transport.play();
        let straight = two_bars(&mut render);
        assert_eq!(
            straight.len(),
            2,
            "an unlooped two-bar song should sound its one note once a bar \
             pair, not {straight:?}"
        );

        project.loop_range = LoopRange {
            start_tick: 0,
            end_tick: TICKS_PER_BAR,
            enabled: true,
        };
        let mut render = RenderState::new(48_000, slots);
        render.load_project(&project);
        render.transport.play();
        let looped = two_bars(&mut render);

        assert_eq!(
            looped.len(),
            3,
            "a one-bar loop over the same span should sound the note once a \
             bar rather than once every two, not {looped:?}"
        );
        assert!(
            render.transport.position_ticks < f64::from(TICKS_PER_BAR),
            "the transport left the loop: {}",
            render.transport.position_ticks
        );
    }

    /// **No allocations in the callback**, measured rather than read.
    ///
    /// `control-plane-seams/04`, and `buffer-implementation/`'s Stage 1
    /// acceptance test 8, which `FOCUS.md` records as having been open since
    /// it was written because it "needs an allocation-tracking harness rather
    /// than a reading of the code". The harness turned out to be three lines
    /// on the counting allocator this crate already installs: a per-thread
    /// count of `alloc` and `realloc` calls that never decreases, because
    /// `live()` is a net byte figure and cannot see an allocation paired with
    /// a free in the same block — which is precisely what a `Vec` growing on
    /// the audio thread looks like.
    ///
    /// The block under the counter is the one that **retires a preview**,
    /// which is the path that allocated: `preview_retired` was a `Vec::new()`
    /// and its first push was its first allocation. Audition a sample from
    /// the browser, let it play to the end, and the callback allocated.
    ///
    /// This is a floor, not a ceiling: it proves these paths do not allocate,
    /// not that no path does. Test 8's full claim — no allocations *or locks*
    /// anywhere in the callback — is wider than one test, and the locks half
    /// is not measured here at all.
    #[test]
    fn a_block_that_retires_a_preview_does_not_allocate() {
        let mut render = RenderState::new(48_000, empty_channel_audio_bank());
        let sample = Arc::new(SampleData {
            frames: vec![[0.5, -0.5]; 64],
            sample_rate: 48_000,
            root_note: 60,
        });
        // Everything that can allocate happens before the counter is read:
        // the preview command is applied on the control thread in the real
        // engine, and `process_block` is warmed once so no lazy table or
        // first-touch buffer inside it is counted against the block under
        // test.
        render.process_block(512);
        assert!(render.apply_preview(PreviewCommand::Play { sample }).is_none());

        let before = crate::COUNTING.allocations();
        // 64 frames into a 512-frame block: the voice finishes and retires
        // inside this call.
        render.process_block(512);
        let allocations = crate::COUNTING.allocations() - before;

        assert!(
            render.pop_retired_preview().is_some(),
            "the block has to actually retire a preview, or this measures the \
             wrong thing"
        );
        assert_eq!(
            allocations, 0,
            "the callback allocated {allocations} times on the block that \
             retired a preview"
        );
    }

    /// **A note-on that replaces a voice's sample frees nothing on the
    /// callback.** Finding 2 of `reports/fable-2026-09-17.md`.
    ///
    /// Load sample A, play it, load sample B over it and let go of A the way
    /// the session does (no undo entry pins it), then play the same voice
    /// again. The voice was A's last holder, so assigning B to it used to free
    /// A's frame buffer inside `trigger` -- allocation-free, and therefore
    /// invisible to the allocation counter, which is why this counts frees
    /// too.
    #[test]
    fn a_note_on_that_replaces_a_voice_sample_does_not_free() {
        let sample = |frames: usize| {
            Arc::new(SampleData {
                frames: vec![[0.5, -0.5]; frames],
                sample_rate: 48_000,
                root_note: 60,
            })
        };
        let slots = empty_channel_audio_bank();
        let first = sample(64);
        let first_alive = Arc::downgrade(&first);
        slots[0].store(Some(Arc::new(ChannelAudioSnapshot::sample(first))));
        let mut render = RenderState::new(48_000, slots.clone());
        render.load_project(&Project::default());
        let trigger = |render: &mut RenderState| {
            render.apply_command(EngineCommand::TriggerChannelNote {
                channel: 0,
                note: 60,
                velocity: 127,
            });
        };

        // A plays and runs out, so the voice is free again and the next note
        // lands on the same one.
        trigger(&mut render);
        render.process_block(512);
        render.process_block(512);
        render.process_block(512);
        assert!(render.strips[0].source.as_sampler().expect("a sampler channel").voice_positions()[0].is_nan());

        // The control thread publishes B -- long enough to still be sounding
        // after the block under test -- and nothing but the voice still holds
        // A.
        slots[0].store(Some(Arc::new(ChannelAudioSnapshot::sample(sample(4_096)))));
        assert!(first_alive.upgrade().is_some(), "the voice should still hold A");
        trigger(&mut render);

        let allocations = crate::COUNTING.allocations();
        let frees = crate::COUNTING.frees();
        render.process_block(512);
        let allocations = crate::COUNTING.allocations() - allocations;
        let frees = crate::COUNTING.frees() - frees;

        assert!(
            !render.strips[0].source.as_sampler().expect("a sampler channel").voice_positions()[0].is_nan(),
            "the block has to actually play B, or this measures the wrong thing"
        );
        assert_eq!(
            allocations, 0,
            "the callback allocated {allocations} times on a note-on that \
             replaced a voice's sample"
        );
        assert_eq!(
            frees, 0,
            "the callback freed {frees} times on a note-on that replaced a \
             voice's sample"
        );

        // And A leaves through the reclaim path rather than being leaked:
        // the voice's handle and the snapshot the sampler last read.
        let retired: Vec<_> = std::iter::from_fn(|| render.pop_retired_sampler_audio()).collect();
        assert_eq!(retired.len(), 2, "expected A's sample and its snapshot");
        assert!(first_alive.upgrade().is_some());
        drop(retired);
        assert!(
            first_alive.upgrade().is_none(),
            "the reclaim path should have held A's last reference"
        );
    }

    /// Acceptance test 8 for the Buffer's own operations, from
    /// `docs/plans/archive/buffer-implementation/01-the-whole-thing.md`: **no
    /// allocations in the callback**, measured rather than reasoned.
    ///
    /// It lives in this crate because `CountingAllocator` does -- it is the
    /// global allocator only under `cfg(test)` here -- and it drives the
    /// device rather than a whole `RenderState`, because the operations under
    /// test are the device's. The block above it covers the surrounding
    /// callback.
    ///
    /// The list is every state the head can be in: following, a gesture with a
    /// window and a repeat count, a hand scrub, the `Offset` chase, free-run
    /// at `Rate`, frozen, and each transition between them. Freeze is the
    /// interesting one, because it changes whether the writer runs -- an
    /// implementation that copied the ring to latch it would be caught here
    /// and nowhere else.
    ///
    /// Still a floor rather than a ceiling, exactly as the preview test says:
    /// it proves these paths do not allocate. **The locks half of test 8 is
    /// not measured by anything**, and `LOOSE_ENDS.md` says so.
    #[test]
    fn no_buffer_operation_allocates_on_the_callback() {
        use mooloop_core::{BufferDuration, BufferEvent};
        use mooloop_dsp::{Event, EventList, StereoBus, TimedEvent};

        const FRAMES: usize = 256;
        let context = mooloop_dsp::ProcessContext {
            sample_rate: 48_000,
            frames: FRAMES,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        };
        // Everything that allocates happens before the counter is read: the
        // ring, the bus and the event list are all built here, and the device
        // is warmed with one block so no first-touch page is counted against
        // the blocks under test.
        let mut device = mooloop_dsp::BufferDevice::with_bars(48_000, 120.0, 2);
        let mut bus = StereoBus::with_capacity(FRAMES);
        let mut events = EventList::empty();
        device.process(&context, &mut bus, &[]);

        let param = |id: u32, value: f32| TimedEvent {
            offset: 0,
            event: Event::ParamValue { id, value },
        };
        let stutter = BufferEvent {
            offset_beats: -0.25,
            rate: 1.0,
            window_beats: Some(0.25),
            repeat: Some(4),
            duration: BufferDuration::UntilNextEvent,
            crossfade_ms: 2.5,
        };

        // Each entry is one block's worth of control input, named for the
        // state it puts the head into. Held in an array so the count below is
        // the number of states actually exercised rather than a number
        // somebody remembered.
        //
        // Rewritten with the device on 2026-09-16. The states it used to
        // sweep -- a chase closing, a free-run at `Rate`, a hand on the
        // platter -- were the turntable model's and no longer exist; what is
        // here is the four sources `reconsider` arbitrates between, and each
        // transition between them.
        let blocks: [(&str, &[TimedEvent]); 15] = [
            ("the playhead moving", &[param(mooloop_core::BUFFER_PARAM_POSITION, 0.5)]),
            ("the playhead moving again", &[param(mooloop_core::BUFFER_PARAM_POSITION, 0.6)]),
            ("the playhead let go of", &[]),
            ("a jump held", &[param(mooloop_core::BUFFER_PARAM_JUMP, 1.0)]),
            ("the jump still held", &[]),
            ("the jump released", &[param(mooloop_core::BUFFER_PARAM_JUMP, 0.0)]),
            ("a reverse held", &[param(mooloop_core::BUFFER_PARAM_REVERSE, 1.0)]),
            ("the reverse released", &[param(mooloop_core::BUFFER_PARAM_REVERSE, 0.0)]),
            (
                "a stutter held, at its own length",
                &[
                    param(mooloop_core::BUFFER_PARAM_STUTTER_LENGTH, 16.0),
                    param(mooloop_core::BUFFER_PARAM_STUTTER, 1.0),
                ],
            ),
            ("the stutter repeating", &[]),
            ("the stutter released", &[param(mooloop_core::BUFFER_PARAM_STUTTER, 0.0)]),
            (
                "a mapped event, which carries its own geometry",
                &[TimedEvent { offset: 0, event: Event::Buffer(stutter) }],
            ),
            ("the mapped event released", &[TimedEvent { offset: 0, event: Event::BufferRelease }]),
            (
                "a quantized freeze arming",
                &[
                    param(mooloop_core::BUFFER_PARAM_QUANT_START, 2.0),
                    param(mooloop_core::BUFFER_PARAM_FREEZE, 1.0),
                ],
            ),
            ("the armed freeze counting down", &[]),
        ];

        let mut exercised = 0;
        for (what, input) in blocks {
            events.clear();
            for event in input {
                events.push(*event);
            }
            for frame in 0..FRAMES {
                bus.l[frame] = (frame as f32 / FRAMES as f32) - 0.5;
                bus.r[frame] = 0.5 - (frame as f32 / FRAMES as f32);
            }

            let before = crate::COUNTING.allocations();
            // Through the trait, which is the path the host takes: the
            // inherent `process` is the narrower one the device's own tests
            // use and would skip the event splitting entirely.
            mooloop_dsp::AudioNode::process(&mut device, &context, &mut bus, &events, None);
            let allocations = crate::COUNTING.allocations() - before;

            assert_eq!(
                allocations, 0,
                "the callback allocated {allocations} times on the block that \
                 was {what}"
            );
            exercised += 1;
        }
        assert_eq!(
            exercised, 15,
            "the sweep stopped covering the head's states, so it proved nothing"
        );
    }

    /// The picture a Buffer face draws reaches the bank, and says the right
    /// things about a head that is somewhere.
    ///
    /// Display telemetry rather than a signal: it has no timing guarantee
    /// beyond "latest available", which is why nothing in the audio path may
    /// read it back. What it has to be is *truthful* -- a face that drew a
    /// head where there was none, or a still picture while the writer ran,
    /// would be worse than no face.
    #[test]
    fn a_buffer_publishes_a_picture_for_its_face_to_draw() {
        use mooloop_dsp::StereoBus;

        const FRAMES: usize = 512;
        let context = mooloop_dsp::ProcessContext {
            sample_rate: 48_000,
            frames: FRAMES,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        };
        let mut device = mooloop_dsp::BufferDevice::with_capacity(4_096);
        let mut bus = StereoBus::with_capacity(FRAMES);
        // Half a ring of signal, so some bins have peaks and some do not --
        // a picture that was all one value would not prove it was drawn.
        for block in 0..4 {
            for frame in 0..FRAMES {
                let loud = block % 2 == 0;
                bus.l[frame] = if loud { 0.8 } else { 0.0 };
                bus.r[frame] = bus.l[frame];
            }
            device.process(&context, &mut bus, &[]);
        }

        let display = device.buffer_waveform().expect("a buffer draws itself");
        assert!(
            display.peaks.iter().any(|peak| *peak > 0.5),
            "the loud blocks have to show up"
        );
        assert!(
            display.peaks.iter().any(|peak| *peak < 0.1),
            "and the quiet ones have to stay quiet: a picture that is all one \
             value is not a picture"
        );
        assert!(display.head.is_none(), "following, so there is no head to draw");
        assert!(!display.frozen);
        assert_eq!(display.armed_freeze, None);
        let write = display.write;
        assert!((0.0..1.0).contains(&write), "the writer is somewhere: {write}");

        // Freeze, and the picture has to follow: a frozen ring plays, so the
        // head becomes drawable and the writer's mark stops moving.
        //
        // Unquantized. The transport is running and `Quantize` defaults on,
        // so a plain freeze would correctly *arm* for the next bar line and
        // this block would end before it landed -- which is a test of the
        // wait, not of the picture.
        let mut events = mooloop_dsp::EventList::empty();
        for (id, value) in [
            (mooloop_core::BUFFER_PARAM_QUANTIZE, 0.0),
            (mooloop_core::BUFFER_PARAM_FREEZE, 1.0),
        ] {
            events.push(mooloop_dsp::TimedEvent {
                offset: 0,
                event: mooloop_dsp::Event::ParamValue { id, value },
            });
        }
        mooloop_dsp::AudioNode::process(&mut device, &context, &mut bus, &events, None);
        let display = device.buffer_waveform().expect("a buffer draws itself");
        assert!(display.head.is_some(), "a frozen ring plays, so it has a head");
        assert!(display.frozen, "and the face has to know the writer stopped");
        assert_eq!(
            display.write, write,
            "a frozen writer does not move, and neither does its mark"
        );
    }

    /// The bank is a pool, and an unsubscribed stage reads empty.
    ///
    /// `docs/CAPACITY_POLICY.md` is about the array this is not: one waveform
    /// per addressable stage would be allocated whether or not a Buffer
    /// existed. What is bounded here is how many are *drawn at once*.
    #[test]
    fn a_waveform_crosses_only_while_a_face_is_looking() {
        let telemetry = DeviceTelemetry::new();
        let peaks = [0.7_f32; mooloop_dsp::WAVEFORM_BINS];
        let display = mooloop_dsp::BufferDisplay {
            peaks: &peaks,
            head: Some(0.25),
            write: 0.5,
            region: Some((0.1, 0.2)),
            frozen: true,
            armed_freeze: Some(false),
            armed_gesture: true,
        };

        telemetry.publish_buffer_display(0, 1, &display);
        assert!(
            telemetry.read_waveform(0, 1).iter().all(|peak| *peak == 0.0),
            "nothing is subscribed, so no peaks crossed"
        );
        // The marks are unpooled and always published: they are five stores,
        // and a face needs them the moment it opens rather than a block later.
        let marks = telemetry.read_buffer_marks(0, 1);
        assert_eq!(marks.head, Some(0.25));
        assert_eq!(marks.write, 0.5);
        assert_eq!(marks.region, Some((0.1, 0.2)));
        assert!(marks.frozen);
        assert_eq!(marks.armed_freeze, Some(false));
        assert!(marks.armed_gesture, "a press waiting for its boundary crosses too");

        assert!(telemetry.set_waveform_enabled(0, 1, true));
        telemetry.publish_buffer_display(0, 1, &display);
        assert!(
            telemetry.read_waveform(0, 1).iter().all(|peak| *peak == 0.7),
            "subscribed, so the peaks crossed"
        );

        telemetry.set_waveform_enabled(0, 1, false);
        assert!(
            telemetry.read_waveform(0, 1).iter().all(|peak| *peak == 0.0),
            "and unsubscribing hands the slot back empty"
        );

        // A head that is not there reads as absent rather than as position
        // zero, which is a real place in the ring.
        let following = mooloop_dsp::BufferDisplay {
            head: None,
            region: None,
            frozen: false,
            armed_freeze: None,
            armed_gesture: false,
            ..display
        };
        telemetry.publish_buffer_display(0, 1, &following);
        let marks = telemetry.read_buffer_marks(0, 1);
        assert_eq!(marks.head, None);
        assert_eq!(marks.region, None);
        assert_eq!(marks.armed_freeze, None);
        assert!(!marks.armed_gesture);
    }

    /// A frozen buffer's ring is a sample being played, so the node that holds
    /// it is not replaceable.
    ///
    /// The prepared-resource guard above refuses a *stale* key. This refuses a
    /// current one, because the reason is different: the swap is correct and
    /// the timing is not. Its trigger in the wild is an ordinary tempo change,
    /// which calls `resize_buffers` and would otherwise take the frozen audio
    /// away mid-performance with nothing on screen to explain it.
    #[test]
    fn a_frozen_buffer_refuses_to_be_replaced() {
        let mut chain = EffectChain::new();
        let mut frozen = Box::new(mooloop_dsp::BufferDevice::with_bars(48_000, 120.0, 1));
        frozen.set_frozen(true);
        let displaced = chain.install(
            0,
            mooloop_core::EffectKind::Buffer,
            Some(1),
            frozen,
            None,
            Box::new(SpectrumAnalyzer::new()),
            Box::new(EffectSlot::new()),
        );
        assert!(displaced.is_empty());

        let replacement = Box::new(mooloop_dsp::BufferDevice::with_bars(48_000, 90.0, 1));
        let refused = chain.replace_if_kind(
            0,
            mooloop_core::EffectKind::Buffer,
            1,
            2,
            replacement,
            None,
        );
        assert!(
            refused.node.is_some(),
            "the replacement must come back for the reclaim ring"
        );
        assert_eq!(
            chain.slot(0).unwrap().resource_key,
            Some(1),
            "and the slot must still be describing the ring that is playing"
        );
    }

    /// **A tempo change keeps what an unfrozen Buffer holds** (MOO-137). The
    /// session answers a tempo change by building every Buffer again at the
    /// new length and swapping it in; the swap now hands the new ring the
    /// history the old one was holding, where it used to arrive empty.
    #[test]
    fn a_tempo_change_keeps_an_unfrozen_buffers_history() {
        let mut chain = EffectChain::new();
        let live = Box::new(mooloop_dsp::BufferDevice::with_bars(48_000, 120.0, 1));
        let displaced = chain.install(
            0,
            mooloop_core::EffectKind::Buffer,
            Some(1),
            live,
            None,
            Box::new(SpectrumAnalyzer::new()),
            Box::new(EffectSlot::new()),
        );
        assert!(displaced.is_empty());
        let context = ProcessContext {
            sample_rate: 48_000,
            frames: 4_800,
            playing: false,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        };
        let mut bus = StereoBus::with_capacity(4_800);
        for frame in 0..4_800 {
            bus.l[frame] = 0.5;
            bus.r[frame] = -0.5;
        }
        chain.nodes[0]
            .as_mut()
            .and_then(|node| node.as_buffer_device_mut())
            .expect("a buffer")
            .process(&context, &mut bus, &[]);

        let slower = Box::new(mooloop_dsp::BufferDevice::with_bars(48_000, 90.0, 1));
        let displaced = chain.replace_if_kind(
            0,
            mooloop_core::EffectKind::Buffer,
            1,
            1,
            slower,
            None,
        );
        assert!(
            displaced.node.is_some(),
            "the outgoing ring must leave through the reclaim ring"
        );
        let replaced = chain.nodes[0]
            .as_mut()
            .and_then(|node| node.as_buffer_device_mut())
            .expect("still a buffer");
        assert!(
            replaced.capacity_frames() > 96_000,
            "the premise: the replacement is the slower tempo's longer ring"
        );
        assert!(
            replaced.waveform_peaks().iter().any(|peak| *peak >= 0.5),
            "the tempo change emptied the ring"
        );
    }

    /// **An undo of an unrelated effect edit keeps what a Buffer holds**
    /// (MOO-137). Every undo is a whole-project install, and a channel whose
    /// chain differed from the live one in any device used to be rebuilt,
    /// Buffer included. Now the channel is carried, and the Buffer -- whose
    /// document state did not change -- moves into the incoming chain with
    /// its ring.
    #[test]
    fn an_install_that_changed_another_device_keeps_the_buffers_history() {
        let mut channel = ProjectChannel::poly_synth(0, 1);
        channel.notes[0].push(NoteEvent::new(1, 0, 8_000, 48, 110));
        channel
            .setup
            .push_effect(mooloop_core::EffectSlotState::new(mooloop_core::EffectParams::Buffer(
                mooloop_core::BufferParams {
                    bars: 1,
                    ..Default::default()
                },
            )))
            .expect("room");
        channel
            .setup
            .push_effect(mooloop_core::EffectSlotState::of_kind(mooloop_core::EffectKind::Filter))
            .expect("room");
        channel.setup.assign_device_ids();
        let mut project = Project {
            channels: vec![channel],
            ..Project::default()
        };
        // The carry matches channels by identity, as a loaded song has them.
        project.assign_channel_ids();
        let mut live = RenderState::from_project(48_000, &project, &[]);
        live.play();
        for _ in 0..(48_000 / 512) {
            live.process_once_block(512);
        }

        // The unrelated edit, as an undo would install it: the filter moved.
        let mut edited = project.clone();
        let mooloop_core::EffectParams::Filter(filter) = &mut edited.channels[0].setup.effects[1].params else {
            panic!("the second row is the filter");
        };
        filter.cutoff_hz = 800.0;
        let plan = crate::carry_plan(&project, &edited);
        assert_eq!(plan.channels, Vec::<(u8, u8)>::new(), "the premise: the chain changed");
        assert_eq!(plan.rechained_channels, vec![(0, 0)]);

        let mut incoming = RenderState::from_project(48_000, &edited, &[]);
        incoming.adopt_performance_state(&live);
        incoming.carry_strips_from(&mut live, &plan);
        let buffer = incoming.strips[0].effects.nodes[0]
            .as_mut()
            .and_then(|node| node.as_buffer_device_mut())
            .expect("the first row is still the Buffer");
        assert!(
            buffer.waveform_peaks().iter().any(|peak| *peak > 0.01),
            "the install emptied the Buffer"
        );
        // And the filter's row describes the incoming document, not the live one.
        assert_eq!(
            incoming.strips[0].effects.slot(1).and_then(|state| state.base_params),
            Some(edited.channels[0].setup.effects[1].params)
        );
    }

    #[test]
    fn preview_voice_plays_replaces_and_retires() {
        let mut render = RenderState::new(48_000, empty_channel_audio_bank());
        let first = Arc::new(SampleData {
            frames: vec![[0.5, -0.5]; 1_000],
            sample_rate: 48_000,
            root_note: 60,
        });
        assert!(
            render
                .apply_preview(PreviewCommand::Play {
                    sample: first.clone(),
                })
                .is_none()
        );
        render.process_block(512);
        let master = render.master();
        assert!(
            master.l[..512].iter().any(|sample| *sample != 0.0),
            "the preview must reach the master bus while the transport is stopped"
        );

        // Replacing a playing preview fades the old one out rather than
        // handing it straight back: it is still sounding. Nothing is returned
        // for disposal, because nothing has finished yet.
        let second = Arc::new(SampleData {
            frames: vec![[1.0, 1.0]; 10],
            sample_rate: 48_000,
            root_note: 60,
        });
        let replaced = render.apply_preview(PreviewCommand::Play { sample: second.clone() });
        assert!(replaced.is_none(), "the displaced voice is fading, not finished");

        // Ten frames and a two-millisecond fade are both over after one
        // block, and both samples retire through the ring -- never dropped on
        // the realtime thread.
        render.process_block(512);
        let retired = [
            render.pop_retired_preview().expect("a voice finished"),
            render.pop_retired_preview().expect("both voices finished"),
        ];
        assert!(retired.iter().any(|sample| Arc::ptr_eq(sample, &first)));
        assert!(retired.iter().any(|sample| Arc::ptr_eq(sample, &second)));
        assert!(render.pop_retired_preview().is_none());

        // And the preview is silent again.
        render.process_block(512);
        assert!(
            !render
                .master()
                .l[..512]
                .iter()
                .any(|sample| *sample != 0.0)
        );
    }

    /// A preview plays a file at the file's own rate, as the sampler does
    /// once the same file is loaded into it. Measured as duration: a tenth of
    /// a second of audio lasts a tenth of a second whatever rate it was
    /// written at.
    ///
    /// Shaped against the unfixed tree, which copied one file frame per
    /// output frame: the 44.1 kHz file ran 4,410 frames (1.5 semitones sharp)
    /// and the 96 kHz one 9,600 (an octave low).
    #[test]
    fn preview_plays_a_file_at_its_own_rate() {
        for file_rate in [44_100u32, 48_000, 96_000] {
            let mut render = RenderState::new(48_000, empty_channel_audio_bank());
            render.attach_preview_gain(Arc::new(AtomicU32::new(1.0f32.to_bits())));
            let tenth = file_rate as usize / 10;
            render.apply_preview(PreviewCommand::Play {
                sample: Arc::new(SampleData {
                    frames: vec![[0.5, 0.5]; tenth],
                    sample_rate: file_rate,
                    root_note: 60,
                }),
            });
            let mut sounding = 0usize;
            for _ in 0..24 {
                render.process_block(512);
                sounding += render.master().l[..512]
                    .iter()
                    .filter(|sample| **sample > 0.25)
                    .count();
            }
            assert!(
                sounding.abs_diff(4_800) <= 2,
                "a tenth of a second at {file_rate} Hz lasted {sounding} frames at 48 kHz"
            );
        }
    }

    /// Stopping a preview fades it rather than cutting it mid-waveform,
    /// which is a click on every audition that is interrupted -- and
    /// auditioning down a list interrupts every one of them.
    #[test]
    fn stopping_a_preview_fades_instead_of_cutting() {
        let mut render = RenderState::new(48_000, empty_channel_audio_bank());
        render.attach_preview_gain(Arc::new(AtomicU32::new(1.0f32.to_bits())));
        let sample = Arc::new(SampleData {
            frames: vec![[0.5, 0.5]; 48_000],
            sample_rate: 48_000,
            root_note: 60,
        });
        render.apply_preview(PreviewCommand::Play { sample: sample.clone() });
        render.process_block(512);
        assert!(render.apply_preview(PreviewCommand::Stop).is_none());
        render.process_block(512);
        let out = &render.master().l[..512];
        let fade = (PREVIEW_FADE_S * 48_000.0).round() as usize;
        assert!((out[0] - 0.5).abs() < 1.0e-6, "the fade starts from where it was");
        let largest_step = out[..=fade]
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            largest_step <= 0.5 / fade as f32 + 1.0e-6,
            "the stop stepped by {largest_step}"
        );
        assert!(out[fade..].iter().all(|sample| *sample == 0.0), "and then it is silent");
        let retired = render.pop_retired_preview().expect("the faded voice retires");
        assert!(Arc::ptr_eq(&retired, &sample));
    }

    #[test]
    fn preview_gain_cell_is_heard_live() {
        let mut render = RenderState::new(48_000, empty_channel_audio_bank());
        let loud = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        render.attach_preview_gain(loud.clone());
        render.apply_preview(PreviewCommand::Play {
            sample: Arc::new(SampleData {
                frames: vec![[0.5, 0.5]; 4_000],
                sample_rate: 48_000,
                root_note: 60,
            }),
        });
        loud.store(0.25f32.to_bits(), Ordering::Relaxed);
        render.process_block(512);
        for sample in &render.master().l[..512] {
            assert!(
                (*sample - 0.125).abs() < 1e-6,
                "the shared gain cell must gate the preview immediately"
            );
        }
    }

    #[test]
    fn project_load_replaces_preallocated_state() {
        let mut project = Project {
            bpm: 173,
            ..full_bank_project()
        };
        project.pattern_lengths[0] = 32;
        project.channels[0].notes[0].push(mooloop_core::NoteEvent::new(1, 24, 12, 60, 100));
        let render = RenderState::from_project(48_000, &project, &[]);
        assert_eq!(render.pattern_length_ticks(0), Some(32 * 24));
        assert!((render.ticks_per_sample() - (173.0 * 96.0 / 60.0 / 48_000.0)).abs() < 1e-12);
    }

    #[test]
    fn project_load_installs_the_last_addressable_effect_slot() {
        let mut project = Project::default();
        for _ in 0..MAX_EFFECTS_PER_CHANNEL {
            project.channels[0]
                .setup
                .push_effect(mooloop_core::EffectSlotState::of_kind(
                    mooloop_core::EffectKind::Filter,
                ));
        }

        let render = RenderState::from_project(48_000, &project, &[]);
        assert!(render.strips[0].effects.nodes[MAX_EFFECTS_PER_CHANNEL - 1].is_some());
        assert_eq!(render.strips[0].effects.bound, MAX_EFFECTS_PER_CHANNEL);
    }

    /// Builds a project around `channel` with one triggering note. A fresh
    /// sampler channel has no sample loaded (and would render silent), so
    /// these generic routing/mixing/metering tests — which need *some*
    /// audible source, not specifically sampler behavior — point a sampler
    /// channel at the legacy builtin kick the same way an old saved project
    /// would.
    /// A saved project that stretches must play stretched from its first
    /// note. `load_project` runs on the control thread, so it provisions the
    /// pool inline rather than waiting for a round trip through the
    /// structural queue.
    #[test]
    fn loading_a_project_provisions_the_stretch_it_asks_for() {
        let mut project = synth_project(ProjectChannel::sampler(0, 1));
        if let Some(state) = project.channels[0].setup.sampler_state_mut() {
            state.params.stretch_enabled = true;
            state.params.stretch_ratio = 2.0;
        }
        let render = RenderState::from_project(48_000, &project, &[]);
        assert!(render.strips[0].source.as_sampler().expect("a sampler channel").has_stretch());
        assert!(render.strips[0].source.as_sampler().expect("a sampler channel").wants_stretch());
        // Channels the project does not describe are not provisioned because
        // they are not built at all -- a stronger statement than "built and
        // left empty", and the one the graph actually makes now.
        assert_eq!(render.strips.len(), 1);
    }

    /// The inverse, which is the part that actually saves the memory: a
    /// project that does not stretch must not carry the state for it.
    #[test]
    fn loading_a_project_without_stretch_provisions_nothing() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let render = RenderState::from_project(48_000, &project, &[]);
        assert!(!render.strips[0].source.as_sampler().expect("a sampler channel").has_stretch());
    }

    /// Installing and removing through the structural path, with the displaced
    /// state coming back rather than being freed on the audio thread.
    #[test]
    fn the_structural_path_installs_and_reclaims_stretch_state() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let mut render = RenderState::from_project(48_000, &project, &[]);

        let installed = render.apply_structural(StructuralCommand::SetSamplerStretch {
            channel: 0,
            pool: Some(Box::new(StretchPool::new(
                mooloop_core::StretchMode::Music,
                48_000,
                MAX_SAMPLER_VOICES as usize,
            ))),
        });
        assert!(installed.is_none(), "nothing was displaced by the first install");
        assert!(render.strips[0].source.as_sampler().expect("a sampler channel").has_stretch());

        let replaced = render.apply_structural(StructuralCommand::SetSamplerStretch {
            channel: 0,
            pool: Some(Box::new(StretchPool::new(
                mooloop_core::StretchMode::Grain,
                48_000,
                MAX_SAMPLER_VOICES as usize,
            ))),
        });
        assert!(
            matches!(replaced, Some(StructuralReclaim::SamplerStretch(_))),
            "the displaced pool must be handed back, not dropped here"
        );

        let removed = render.apply_structural(StructuralCommand::SetSamplerStretch {
            channel: 0,
            pool: None,
        });
        assert!(matches!(
            removed,
            Some(StructuralReclaim::SamplerStretch(_))
        ));
        assert!(!render.strips[0].source.as_sampler().expect("a sampler channel").has_stretch());
    }

    /// `apply_structural` hands the pool back when the channel does not
    /// exist, because dropping it would free megabytes on the realtime
    /// thread.
    ///
    /// That guard was written when it could not fire: `MAX_CHANNELS` is 256
    /// and the command addresses channels with a `u8`, so every address was
    /// backed by a strip that had been reserved up front. Reserving them
    /// stopped (`docs/plans/archive/modulator-capacity/`) -- the graph now builds
    /// only the channels a project describes -- so an in-range `u8` can
    /// address a strip that is simply not there, and the guard became load
    /// bearing rather than defensive. This is the case that reaches it.
    #[test]
    fn a_pool_aimed_at_a_channel_that_does_not_exist_comes_back() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        // Addressable, and beyond what this project materialized.
        assert!(usize::from(u8::MAX) < MAX_CHANNELS);
        assert!(render.strips.len() <= usize::from(u8::MAX));

        let turned_away = render.apply_structural(StructuralCommand::SetSamplerStretch {
            channel: u8::MAX,
            pool: Some(Box::new(StretchPool::new(
                mooloop_core::StretchMode::Music,
                48_000,
                1,
            ))),
        });
        assert!(
            matches!(turned_away, Some(StructuralReclaim::SamplerStretch(_))),
            "a pool with nowhere to go must be handed back, not freed here"
        );
    }

    fn synth_project(mut channel: ProjectChannel) -> Project {
        if let Some(sampler) = channel.setup.sampler_state_mut() {
            sampler.sample = mooloop_core::SampleReference::Builtin {
                id: "default_kick".into(),
            };
        }
        let mut project = Project {
            channels: vec![channel],
            ..full_bank_project()
        };
        project.channels[0].notes[0].push(NoteEvent::new(1, 0, 96, 60, 127));
        project
    }

    /// A prepared generation reads the bank it was prepared with, and nothing
    /// a later generation is given can reach it.
    ///
    /// This is `control-plane-seams/03`. `RenderState::new` used to be handed
    /// a **clone of the engine handle's** bank, so there was one set of slots
    /// shared by every generation that had ever existed. Opening a song then
    /// queued the new graph through the command ring and overwrote that
    /// shared bank immediately, out of band -- so for the block or two before
    /// the audio thread consumed the install, the outgoing project's graph
    /// played the incoming project's samples, and mid-loop, a half-replaced
    /// set of them.
    ///
    /// The test that would have caught it directly cannot be written here: it
    /// wants `EngineHandle::install_project`, and an `EngineHandle` cannot be
    /// built without opening an audio driver. What is checked instead is the
    /// property that install now relies on, and the signature carries the
    /// rest -- `install_project` takes its snapshots **by value** and builds
    /// its own slots, so handing it the live generation's bank is not
    /// something a caller can express.
    #[test]
    fn a_generation_plays_the_bank_it_was_prepared_with() {
        use crate::render_test_support::{peak_of, BLOCK, SAMPLE_RATE};

        let quiet = Arc::new(SampleData {
            frames: (0..8_000).map(|_| [0.1, -0.1]).collect(),
            sample_rate: 48_000,
            root_note: 60,
        });
        let loud = Arc::new(SampleData {
            frames: (0..8_000).map(|_| [0.9, -0.9]).collect(),
            sample_rate: 48_000,
            root_note: 60,
        });
        let project = synth_project(ProjectChannel::sampler(0, 1));

        let mut a = RenderState::new(
            SAMPLE_RATE,
            channel_audio_bank(vec![ChannelAudioSnapshot::sample(quiet.clone())]),
        );
        a.load_project(&project);

        // Generation B, prepared exactly as `install_project` prepares one:
        // its own bank, from values rather than from A's slots.
        let mut b = RenderState::new(
            SAMPLE_RATE,
            channel_audio_bank(vec![ChannelAudioSnapshot::sample(loud.clone())]),
        );
        b.load_project(&project);

        // B exists and is not yet installed. A is still the live generation
        // and must still be playing A's sample.
        a.transport.play();
        b.transport.play();
        for _ in 0..4 {
            a.process_once_block(BLOCK);
            b.process_once_block(BLOCK);
        }
        let heard = peak_of(&a.master().l[..BLOCK]);
        let prepared = peak_of(&b.master().l[..BLOCK]);

        // The ratio, not the absolute levels: what reaches the master has
        // been through the sampler's output trim and the pan law, so the
        // figure to hold is the 9:1 the two buffers differ by. Both must be
        // sounding, or this passes on two silences.
        assert!(
            heard > 0.0 && prepared > 0.0,
            "both generations must sound for this to test anything; A was \
             {heard} and B was {prepared}"
        );
        let ratio = prepared / heard;
        assert!(
            (7.0..=11.0).contains(&ratio),
            "the two generations played a ratio of {ratio} (A {heard}, B \
             {prepared}); their buffers differ by 9, so anything else means \
             they are not reading separate banks"
        );
    }

    /// The offline renderer builds its own slots from the project, so it is a
    /// second place a channel's audio is assembled. Slice maps travel with the
    /// project rather than with `samples`, and leaving them out made every
    /// note in a sliced channel resolve out of range -- an exported mix silent
    /// exactly where the app was not. A committed stretch has the mirror
    /// problem: `samples` carries sources, so the export would play the
    /// unstretched original.
    #[test]
    fn an_offline_render_gets_the_slice_map_and_the_committed_buffer() {
        let sample = Arc::new(SampleData {
            frames: (0..8_000).map(|_| [0.5, -0.5]).collect(),
            sample_rate: 48_000,
            root_note: 60,
        });
        let mut channel = ProjectChannel::sampler(0, 1);
        {
            let sampler = channel.setup.sampler_state_mut().unwrap();
            sampler.params.play_mode = mooloop_core::PlayMode::Slice;
            sampler.params.attack = 0.0;
            sampler.slices.divide_evenly(4, 0, 8_000);
        }
        // The third slice, so a map that failed to arrive cannot be mistaken
        // for one that did.
        let note = mooloop_core::DEFAULT_SLICE_BASE_NOTE + 2;
        channel.notes[0].push(NoteEvent::new(1, 0, 96, note, 127));
        let project = Project {
            channels: vec![channel],
            ..full_bank_project()
        };

        let samples = vec![Some(sample.clone())];
        let mut render = RenderState::from_project(48_000, &project, &samples);
        render.play();
        let report = render.process_block(512);
        assert!(
            report.peak_l > 0.001,
            "a sliced channel rendered silent offline: the map never arrived"
        );

        // And a commit is baked here too, so the exported length is the
        // stretched one rather than the source's.
        let mut committed = project.clone();
        {
            let sampler = committed.channels[0].setup.sampler_state_mut().unwrap();
            sampler.params.play_mode = mooloop_core::PlayMode::Pitched;
            sampler.commit = Some(Box::new(mooloop_core::SampleCommit {
                mode: mooloop_core::StretchMode::Music,
                ratio: 2.0,
                grain: 1024,
                source_markers: Vec::new(),
                source_start: 0.0,
                source_end: 1.0,
                source_loop_start: 0.0,
                source_loop_end: 1.0,
            }));
            sampler.slices = mooloop_core::SliceMap::default();
        }
        let render = RenderState::from_project(48_000, &committed, &samples);
        let published = render.audio_slots[0]
            .load_full()
            .expect("the channel should have audio")
            .sample
            .clone()
            .expect("the channel should have a buffer");
        assert_eq!(
            published.frames.len(),
            16_000,
            "the export played the source rather than the committed render"
        );
    }

    #[test]
    fn synth_sources_render_without_sample_data() {
        for channel in [
            ProjectChannel::drum_synth(0, 1),
            ProjectChannel::mono_synth(0, 1),
            ProjectChannel::poly_synth(0, 1),
        ] {
            let project = synth_project(channel);
            let mut render = RenderState::from_project(48_000, &project, &[]);
            render.play();
            let report = render.process_block(512);
            assert!(report.peak_l > 0.001, "synth source was silent");
        }
    }

    /// Install `kind`'s device on `channel` the way a source change does,
    /// and drop the displaced one here, off the "audio thread".
    fn switch_source(render: &mut RenderState, channel: u8, kind: DeviceKind) {
        let node = render.build_source_for(channel as usize, &kind.default_generator_params());
        let displaced = render.apply_structural(StructuralCommand::InstallSource { channel, node });
        assert!(matches!(displaced, Some(StructuralReclaim::Source(_))));
    }

    #[test]
    fn source_switch_resets_inactive_voice_state() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        assert!(render.process_block(256).peak_l > 0.001);

        switch_source(&mut render, 0, DeviceKind::MonoSynth);
        render.process_block(256);
        switch_source(&mut render, 0, DeviceKind::Sampler);
        assert_eq!(render.process_block(256).peak_l, 0.0);
    }

    /// Adding a channel allocates, so it goes through the structural ring
    /// with storage built off-thread — the same route an effect node takes.
    fn add_channel(render: &mut RenderState, source: DeviceKind) {
        let storage =
            RenderState::build_channel(Arc::new(ArcSwapOption::from(None)), source, 48_000);
        let returned = render.apply_structural(StructuralCommand::AddChannel { storage });
        // Reused storage comes straight back rather than being dropped here.
        drop(returned);
    }

    #[test]
    fn readding_a_channel_resets_its_preallocated_slot() {
        let mut render = RenderState::from_project(48_000, &Project::default(), &[]);
        add_channel(&mut render, DeviceKind::DrumSynth);
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

        // What incremental removal would do: shrink the active region and
        // leave the slot's storage and contents behind for the re-add to
        // reset. Nothing sends that today (`EngineCommand::RemoveChannel`
        // was deleted unused), so the test shrinks the region directly.
        render.sequencer.set_active_channels(1);
        add_channel(&mut render, DeviceKind::DrumSynth);
        render.apply_command(EngineCommand::Stop);
        render.apply_command(EngineCommand::Play);
        assert_eq!(render.process_block(256).peak_l, 0.0);
    }

    /// A spare channel slot that still holds effects gives them back through
    /// the reclaim ring when `AddChannel` reuses it, and the block that does
    /// so does not allocate.
    ///
    /// Nothing in production leaves a populated spare today -- removal
    /// rebuilds the whole state -- so the test lowers the active count by
    /// hand, which is what incremental removal will do when it arrives
    /// (`reports/fable-2026-09-17.md`, finding 3). `reclaim` was a
    /// `Vec::new()` that nothing drained: the first displaced node allocated
    /// on the audio thread and then stayed in the live generation. With that
    /// fixed, the block still made two allocations: resetting the slot's
    /// sources rebuilt ML-P8's chorus delay line, which `SetChannelSource`
    /// does in production too.
    #[test]
    fn reusing_a_populated_spare_channel_reclaims_its_effects_without_allocating() {
        use crate::executor::{Executor, ExecutorIo};
        use crate::RealtimeCommand;

        let mut project = full_bank_project();
        project.channels = (0..3).map(|index| ProjectChannel::sampler(index, 1)).collect();
        project.channels[2]
            .setup
            .push_effect(mooloop_core::EffectSlotState::of_kind(
                mooloop_core::EffectKind::Eq,
            ))
            .expect("pushed");
        let mut render = RenderState::from_project(48_000, &project, &[]);
        assert!(render.strips[2].effects.nodes[0].is_some());
        render.sequencer.set_active_channels(2);

        let (mut cmd_tx, cmd_rx) = rtrb::RingBuffer::new(8);
        let (evt_tx, _evt_rx) = rtrb::RingBuffer::new(64);
        let (reclaim_tx, mut reclaim_rx) = rtrb::RingBuffer::new(8);
        let mut executor = Executor::new(
            ExecutorIo {
                cmd_rx,
                evt_tx,
                reclaim_tx,
            },
            Box::new(render),
            Arc::new(std::sync::atomic::AtomicU64::new(0)),
            48_000,
            crate::load::LoadMeters::new(),
        );
        let (mut left, mut right) = ([0.0; 256], [0.0; 256]);
        let no_midi = || std::iter::empty::<(mooloop_core::MidiPortId, u32, &[u8])>();
        // Warm: the first block asks the scheduler about its thread and
        // touches whatever is lazily initialised.
        executor.process(no_midi(), &mut left, &mut right);

        let storage = RenderState::build_channel(
            Arc::new(ArcSwapOption::from(None)),
            DeviceKind::Sampler,
            48_000,
        );
        assert!(cmd_tx
            .push(RealtimeCommand::Structural(StructuralCommand::AddChannel { storage }))
            .is_ok());
        let before = crate::COUNTING.allocations();
        executor.process(no_midi(), &mut left, &mut right);
        let allocations = crate::COUNTING.allocations() - before;

        let (mut nodes, mut storages) = (0, 0);
        while let Ok(reclaimed) = reclaim_rx.pop() {
            if let StructuralReclaim::Effect(effect) = reclaimed {
                nodes += usize::from(effect.node.is_some());
                storages += usize::from(effect.channel.is_some());
            }
        }
        assert_eq!(
            (allocations, nodes, storages),
            (0, 1, 1),
            "(allocations in the block, effect nodes reclaimed, channel \
             storages reclaimed)"
        );
    }

    fn strip_route(param: u32, depth: f32) -> ModRack {
        let mut rack = ModRack::default();
        rack.install(
            0,
            mooloop_core::ModulatorParams::Lfo(mooloop_core::ModLfoParams::default()),
        );
        rack.add_route(mooloop_core::ModRoute::to_slot(
            0,
            ParamAddr::strip(EffectTarget::Channel(0), param),
            depth,
            mooloop_core::ModPolarity::Bipolar,
        ))
        .expect("route fits the matrix");
        rack
    }

    /// The strip's fader is an ordinary destination: a source resolves it into
    /// one gain per control subdivision, centred on the knob value, and leaves
    /// pan untouched. The offset sums in normalized space and clamps there, so
    /// a swing that would drive the fader below zero lands on silence rather
    /// than a negative gain.
    #[test]
    fn a_source_resolves_the_strip_fader_into_control_rate_segments() {
        let rack = strip_route(mooloop_core::STRIP_PARAM_VOLUME, 0.25);
        let mut outputs: ControlOutputs =
            [[0.0; MAX_MODULATORS_PER_CHANNEL]; MAX_CONTROL_TICKS_PER_BLOCK];
        outputs[0][0] = 1.0;
        outputs[1][0] = -1.0;
        let modulation = ModulationBlock {
            rack: &rack,
            outputs: &outputs,
            outlets: &[0.0; MAX_GENERATOR_OUTLETS],
            performance: &[0.0; PERFORMANCE_SOURCES],
            ticks: 2,
        };

        let segments =
            resolve_strip_segments(0.8, 0.0, EffectTarget::Channel(0), &modulation, None)
                .expect("a routed fader resolves");
        assert_eq!(segments.count, 2);

        // Depth is a fraction of fader travel. A gain of 0.8 sits about 0.71
        // of the throw; +0.25 lands near the top and -0.25 about 0.46.
        let volume = mooloop_core::strip_descriptor(mooloop_core::STRIP_PARAM_VOLUME).unwrap();
        let knob = volume.to_normalized(0.8);
        assert!((segments.values[0].0 - volume.from_normalized(knob + 0.25)).abs() < 1e-6);
        assert!((segments.values[1].0 - volume.from_normalized(knob - 0.25)).abs() < 1e-6);
        assert!(segments.values[0].0 > 0.8 && segments.values[1].0 < 0.8);
        assert_eq!(segments.values[0].1, 0.0);
        assert_eq!(segments.values[1].1, 0.0);
    }

    /// A still fader resolves to no segments at all, so the ordinary block
    /// stays a single pass over the bus rather than a per-subdivision walk.
    #[test]
    fn an_undriven_strip_resolves_to_no_segments() {
        let outputs: ControlOutputs =
            [[0.0; MAX_MODULATORS_PER_CHANNEL]; MAX_CONTROL_TICKS_PER_BLOCK];
        let modulation = ModulationBlock {
            rack: &ModRack::default(),
            outputs: &outputs,
            outlets: &[0.0; MAX_GENERATOR_OUTLETS],
            performance: &[0.0; PERFORMANCE_SOURCES],
            ticks: 2,
        };
        assert!(
            resolve_strip_segments(0.8, 0.0, EffectTarget::Channel(0), &modulation, None).is_none()
        );
    }

    /// A modulated fader must actually reach the audio. Two subdivisions with
    /// opposite source outputs scale the same block by different gains, which
    /// an unmodulated render does not do.
    #[test]
    fn strip_modulation_reaches_the_rendered_block() {
        let mut channel = ProjectChannel::sampler(0, 1);
        channel.setup.modulation.install(0, mooloop_core::ModulatorParams::Lfo(
            mooloop_core::ModLfoParams {
                // A quarter-cycle per 32-frame subdivision at 48 kHz.
                rate_hz: 375.0,
                waveform: mooloop_core::ModLfoWaveform::Square,
                ..mooloop_core::ModLfoParams::default()
            },
        ));
        channel.setup.modulation = {
            let mut rack = strip_route(mooloop_core::STRIP_PARAM_VOLUME, 0.5);
            rack.slots = channel.setup.modulation.slots;
            rack
        };
        let project = synth_project(channel);

        let mut flat_project = project.clone();
        flat_project.channels[0].setup.modulation.routes =
            [None; mooloop_core::MAX_MOD_ROUTES_PER_CHANNEL];
        let mut flat = RenderState::from_project(48_000, &flat_project, &[]);
        flat.play();
        flat.process_block(256);
        let flat_master: Vec<f32> = flat.master().l[..256].to_vec();

        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        render.process_block(256);
        let modulated: Vec<f32> = render.master().l[..256].to_vec();

        // Compare the two renders subdivision by subdivision. A square LFO
        // alternates the fader between two gains, so the ratio to the
        // unmodulated render must not be the same in every subdivision.
        let ratio_at = |tick: usize| -> Option<f32> {
            (tick * CONTROL_RATE_FRAMES..(tick + 1) * CONTROL_RATE_FRAMES)
                .filter(|&i| flat_master[i].abs() > 1e-4)
                .map(|i| modulated[i] / flat_master[i])
                .next()
        };
        let first = ratio_at(0).expect("the first subdivision must carry audio");
        let differs = (1..256 / CONTROL_RATE_FRAMES)
            .filter_map(ratio_at)
            .any(|ratio| (ratio - first).abs() > 1e-3);
        assert!(
            differs,
            "a source on the fader must change gain across subdivisions"
        );
    }

    /// A test-only variant of `install_effect_as` that records the real
    /// `EffectKind` rather than that helper's hardcoded `Filter` -- needed
    /// here because the destination this test drives is addressed by an EQ
    /// descriptor id, and `control_events_for_slot` reads a slot's
    /// descriptor table from `state.kind`, not from the node it happens to
    /// hold.
    fn install_effect_of_kind(
        target: EffectTarget,
        slot: u8,
        device: mooloop_core::DeviceId,
        kind: mooloop_core::EffectKind,
        node: Box<dyn AudioNode + Send>,
    ) -> StructuralCommand {
        let align = IntegerDelay::new(node.dry_path_latency_frames()).map(Box::new);
        StructuralCommand::InstallEffect {
            target,
            slot,
            kind,
            resource_key: None,
            node,
            align,
            analyzer: Box::new(SpectrumAnalyzer::new()),
            state: Box::new(EffectSlot::for_device(device)),
        }
    }

    /// The point of the whole curve pool (Plan D, `reports/fable-2026-09-22.md`
    /// finding 3), exercised end to end rather than through
    /// `mooloop_dsp::effects`'s own unit tests: two of an EQ's own bands
    /// modulated at once, on a real channel, for the largest block the
    /// engine ever hands a node.
    ///
    /// Under the event path this replaced, one fully-automated destination
    /// alone reaches `event.rs`'s `MAX_EVENTS` (256) at `MAX_BLOCK_SIZE`
    /// (8192 frames / 32 = 256 ticks) -- so a *second* simultaneously
    /// modulated destination on the same slot had nowhere left to go and
    /// was silently dropped by `push_ordered`'s own `if self.len ==
    /// MAX_EVENTS { return false; }`. Two rows, not two-on-one-list, is
    /// what the curve pool buys: `curve_refusals` -- the counted refusal
    /// this plan added in the same place that silent drop used to be --
    /// must still read zero after both bands rode a full block.
    #[test]
    fn two_simultaneously_modulated_eq_bands_survive_a_maximal_block() {
        let mut channel = ProjectChannel::sampler(0, 1);
        let device = mooloop_core::DeviceId(901);
        let gain0 = mooloop_core::eq_band_param(0, mooloop_core::EQ_BAND_GAIN);
        let gain1 = mooloop_core::eq_band_param(1, mooloop_core::EQ_BAND_GAIN);

        let mut rack = ModRack::default();
        rack.install(
            0,
            mooloop_core::ModulatorParams::Lfo(mooloop_core::ModLfoParams {
                rate_hz: 4.0,
                ..mooloop_core::ModLfoParams::default()
            }),
        );
        rack.add_route(mooloop_core::ModRoute::to_slot(
            0,
            ParamAddr::effect(EffectTarget::Channel(0), device, gain0),
            0.5,
            mooloop_core::ModPolarity::Bipolar,
        ))
        .expect("the first route fits the matrix");
        rack.add_route(mooloop_core::ModRoute::to_slot(
            0,
            ParamAddr::effect(EffectTarget::Channel(0), device, gain1),
            0.5,
            mooloop_core::ModPolarity::Bipolar,
        ))
        .expect("the second route fits the matrix");
        channel.setup.modulation = rack;

        let project = synth_project(channel);
        let mut render = RenderState::from_project(48_000, &project, &[]);
        let _ = render.apply_structural(install_effect_of_kind(
            EffectTarget::Channel(0),
            0,
            device,
            mooloop_core::EffectKind::Eq,
            mooloop_dsp::build_effect_at_tempo(
                mooloop_core::EffectParams::Eq(mooloop_core::EqParams::default()),
                48_000,
                120.0,
            ),
        ));
        render.play();
        render.process_block(MAX_BLOCK_SIZE);

        assert_eq!(
            render.strips[0].effects.curve_refusals, 0,
            "a driven EQ destination was refused though the pool has room \
             for many more than two at once"
        );
    }

    /// The source-side half of the same change: a channel's own generator
    /// parameters, resolved at
    /// `RenderState::process_block_inner`'s descriptor loop, used to share
    /// `events[index]` -- the same list the channel's *notes* travel on --
    /// with every other driven source parameter. Two destinations driven at
    /// once across a maximal block now cost nothing from that shared list
    /// at all: `source_curve_refusals` reads zero, and the note this
    /// channel plays (`synth_project` always schedules one) still reaches
    /// the render.
    #[test]
    fn two_simultaneously_modulated_source_params_survive_a_maximal_block() {
        let mut channel = ProjectChannel::sampler(0, 1);
        let cutoff = mooloop_core::SAMPLER_PARAM_FILTER_CUTOFF;
        // Not re-exported at the crate root like most sampler param ids;
        // reached through its module directly.
        let output_gain = mooloop_core::generator::SAMPLER_PARAM_OUTPUT_GAIN;

        let mut rack = ModRack::default();
        rack.install(
            0,
            mooloop_core::ModulatorParams::Lfo(mooloop_core::ModLfoParams {
                rate_hz: 4.0,
                ..mooloop_core::ModLfoParams::default()
            }),
        );
        rack.add_route(mooloop_core::ModRoute::to_slot(
            0,
            ParamAddr {
                scope: EffectTarget::Channel(0),
                owner: ParamOwner::Source,
                param: cutoff,
            },
            0.5,
            mooloop_core::ModPolarity::Bipolar,
        ))
        .expect("the first route fits the matrix");
        rack.add_route(mooloop_core::ModRoute::to_slot(
            0,
            ParamAddr {
                scope: EffectTarget::Channel(0),
                owner: ParamOwner::Source,
                param: output_gain,
            },
            0.5,
            mooloop_core::ModPolarity::Bipolar,
        ))
        .expect("the second route fits the matrix");
        channel.setup.modulation = rack;

        let project = synth_project(channel);
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        let report = render.process_block(MAX_BLOCK_SIZE);

        assert_eq!(
            render.source_curve_refusals, 0,
            "a driven source destination was refused though the pool has \
             room for many more than two at once"
        );
        assert!(
            report.peak_l.max(report.peak_r) > 0.0,
            "the channel's own note should still have reached the render \
             alongside its two modulated parameters"
        );
        // And nothing was dropped on the way in. No generator reads curves
        // natively yet, so both rows went through `apply_curves`'s default
        // fallback into the channel's 256-event list: 2 x 256 ticks, which
        // refused the whole second destination until MOO-73 thinned it.
        assert_eq!(render.refused_events(), 0);
    }

    /// The internal routes' stride: every tick while they fit, coarser only
    /// when sixteen routes would fill the list alone, never coarser than
    /// one event per route per block.
    #[test]
    fn internal_route_amounts_thin_only_when_they_would_not_fit() {
        assert_eq!(control_tick_stride(16, 16, 128), 2);
        assert_eq!(control_tick_stride(16, 4, 128), 1);
        assert_eq!(control_tick_stride(32, 16, 128), 4);
        assert_eq!(control_tick_stride(256, 16, 8), 256);
        assert_eq!(control_tick_stride(0, 16, 0), 1);
    }

    /// MOO-73, the threshold the issue was filed on: sixteen routes, a full
    /// modulation rack, on one generator at 1024 frames is 16 x 32 = 512
    /// control events into a list of 256 that also holds the channel's
    /// notes. Every one of them has to arrive, thinned, rather than the
    /// first eight filling the list and the rest freezing.
    #[test]
    fn a_full_modulation_rack_on_a_generator_refuses_nothing_at_1024_frames() {
        let mut channel = ProjectChannel::sampler(0, 1);
        let mut rack = ModRack::default();
        rack.install(
            0,
            mooloop_core::ModulatorParams::Lfo(mooloop_core::ModLfoParams {
                rate_hz: 4.0,
                ..mooloop_core::ModLfoParams::default()
            }),
        );
        let targets: Vec<u32> = channel
            .setup
            .source
            .kind()
            .descriptors()
            .iter()
            .filter(|descriptor| {
                mooloop_core::ModDestinationDescriptor::for_param(descriptor).allowed
            })
            .map(|descriptor| descriptor.id)
            .take(mooloop_core::MAX_MOD_ROUTES_PER_CHANNEL)
            .collect();
        assert_eq!(targets.len(), mooloop_core::MAX_MOD_ROUTES_PER_CHANNEL);
        for param in targets {
            rack.add_route(mooloop_core::ModRoute::to_slot(
                0,
                ParamAddr {
                    scope: EffectTarget::Channel(0),
                    owner: ParamOwner::Source,
                    param,
                },
                0.3,
                mooloop_core::ModPolarity::Bipolar,
            ))
            .expect("sixteen routes fit the matrix");
        }
        channel.setup.modulation = rack;

        let project = synth_project(channel);
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        for _ in 0..4 {
            render.process_block(1024);
        }
        assert_eq!(render.source_curve_refusals, 0);
        assert_eq!(
            render.refused_events(),
            0,
            "a full rack's control events did not fit the channel's list"
        );
    }

    #[test]
    fn a_channel_note_trigger_restarts_its_played_lfo_on_the_control_tick() {
        let mut channel = ProjectChannel::sampler(0, 1);
        channel.setup.modulation.install(0, mooloop_core::ModulatorParams::Lfo(
            mooloop_core::ModLfoParams {
                rate_hz: 375.0,
                waveform: mooloop_core::ModLfoWaveform::Saw,
                retrigger: true,
                ..mooloop_core::ModLfoParams::default()
            },
        ));
        let project = synth_project(channel);
        let mut render = RenderState::from_project(48_000, &project, &[]);
        assert_eq!(render.tick_modulators_from_gate_table(0, 64), 2);
        assert_eq!(render.control_outputs[0][0][0], -1.0);
        assert_eq!(render.control_outputs[0][1][0], -0.5);

        // `process_block_inner` builds this fixed bitmap from scheduled
        // NoteOn offsets. The tick method applies it before sampling, so the
        // destination sees the reset phase on that subdivision rather than
        // one control tick later.
        render.gate_ticks[0][0].note_ons = 1;
        render.tick_modulators_from_gate_table(0, 32);
        assert_eq!(render.control_outputs[0][0][0], -1.0);
    }

    /// **A synced LFO follows the song position** (MOO-127): the downbeat
    /// reads the same after a stop and a wait, the same as a fresh render --
    /// which is what an export builds -- and a seek lands it where playing
    /// through would have.
    ///
    /// Shaped against the unfixed tree, where the rack ran on elapsed frames:
    /// the second downbeat read wherever the free run had got to.
    #[test]
    fn a_synced_lfo_follows_the_song_position_through_stop_and_seek() {
        const BLOCK: usize = 480;
        let mut channel = ProjectChannel::sampler(0, 1);
        channel.setup.modulation.install(0, mooloop_core::ModulatorParams::Lfo(
            mooloop_core::ModLfoParams {
                tempo_sync: true,
                // Three beats: a cycle the bar does not divide, so no
                // position lands on the same phase by accident.
                rate_division: mooloop_core::ModTimeDivision::DottedHalf,
                waveform: mooloop_core::ModLfoWaveform::Saw,
                ..mooloop_core::ModLfoParams::default()
            },
        ));
        let project = synth_project(channel);
        let first_tick = |render: &RenderState| render.control_outputs[0][0][0];

        // What an export hears on the downbeat: a fresh state, played.
        let mut fresh = RenderState::from_project(48_000, &project, &[]);
        fresh.play();
        fresh.process_block(BLOCK);
        let downbeat = first_tick(&fresh);

        // A live state whose rack has been running free, stopped, first.
        let mut live = RenderState::from_project(48_000, &project, &[]);
        for _ in 0..137 {
            live.process_block(BLOCK);
        }
        live.play();
        live.process_block(BLOCK);
        assert_eq!(first_tick(&live), downbeat, "play from the top");

        // Played through to beat 2 (one second at 120 BPM)...
        for _ in 1..100 {
            live.process_block(BLOCK);
        }
        live.process_block(BLOCK);
        let played_through = first_tick(&live);

        // ...stopped, left, and played from the top again...
        live.apply_command(EngineCommand::Stop);
        for _ in 0..61 {
            live.process_block(BLOCK);
        }
        live.play();
        live.process_block(BLOCK);
        assert_eq!(first_tick(&live), downbeat, "play from the top after a stop");

        // ...and a seek to beat 2 lands where playing through did.
        live.apply_command(EngineCommand::Seek { tick: 192.0 });
        live.process_block(BLOCK);
        assert!(
            (first_tick(&live) - played_through).abs() < 1e-4,
            "seek read {}, playing through read {played_through}",
            first_tick(&live)
        );
    }

    #[test]
    fn an_envelope_can_subscribe_to_another_channels_note_gate() {
        let mut target = ProjectChannel::sampler(0, 1);
        target.setup.modulation.install(0, mooloop_core::ModulatorParams::Envelope(
            mooloop_core::ModEnvelopeParams {
                input_channel: 1,
                attack_seconds: 0.0,
                decay_seconds: 0.0,
                sustain: 1.0,
                ..mooloop_core::ModEnvelopeParams::default()
            },
        ));
        let mut project = synth_project(target);
        project.channels.push(ProjectChannel::sampler(1, 1));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.gate_ticks[0][1].note_ons = 1;

        render.tick_modulators_from_gate_table(0, 32);
        assert_eq!(render.control_outputs[0][0][0], 1.0);
    }

    /// A stepped parameter refuses modulation, so a route aimed at one is
    /// inert -- and, just as importantly, does not suppress the knob. Without
    /// the policy check the engine would treat the destination as modulated,
    /// withhold the base write, and leave the mode stuck.
    #[test]
    fn a_route_on_a_stepped_parameter_neither_moves_nor_blocks_its_knob() {
        let mut channel = ProjectChannel::sampler(0, 1);
        let eq = channel
            .setup
            .push_effect(mooloop_core::EffectSlotState::of_kind(
                mooloop_core::EffectKind::Eq,
            ))
            .expect("pushed");
        channel.setup.modulation.install(0, mooloop_core::ModulatorParams::Lfo(
            mooloop_core::ModLfoParams {
                rate_hz: 375.0,
                ..mooloop_core::ModLfoParams::default()
            },
        ));
        // A band's *type*, which is stepped. The EQ's band selector was the
        // stepped parameter this test used until `eq-v2/01` stopped it being
        // a parameter at all; what is under test is the policy, not the EQ.
        let stepped_id = mooloop_core::eq_band_param(0, mooloop_core::EQ_BAND_KIND);
        let stepped = ParamAddr::effect(EffectTarget::Channel(0), eq, stepped_id);
        assert!(channel
            .setup
            .modulation
            .add_route(mooloop_core::ModRoute::to_slot(
                0,
                stepped,
                1.0,
                mooloop_core::ModPolarity::Bipolar,
            ))
            .is_some());

        let project = synth_project(channel);
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        render.process_block(128);

        // No control events are emitted for a destination that refuses
        // modulation.
        assert!(!render.strips[0]
            .effects
            .event_scratch
            .iter()
            .any(|event| matches!(
                event.event,
                Event::ParamValue { id, .. } if id == stepped_id
            )));

        // And the knob still reaches the device, because the parked route does
        // not count as modulating it.
        assert!(!render.effect_is_driven(EffectTarget::Channel(0), 0, stepped_id));
    }

    #[test]
    fn lfo_resolves_filter_cutoff_at_the_control_rate_from_its_base_value() {
        let mut channel = ProjectChannel::sampler(0, 1);
        channel
            .setup
            .push_effect(mooloop_core::EffectSlotState::filter(
                mooloop_core::FilterParams {
                    cutoff_hz: 1_000.0,
                    resonance: 0.0,
                    mode: mooloop_core::FilterMode::LowPass,
                    ..mooloop_core::FilterParams::default()
                },
            ));
        channel.setup.modulation.install(0, mooloop_core::ModulatorParams::Lfo(
            mooloop_core::ModLfoParams {
                // 32 frames advance the LFO by a quarter-cycle at 48 kHz,
                // giving this block four clear control-rate landmarks.
                rate_hz: 375.0,
                ..mooloop_core::ModLfoParams::default()
            },
        ));
        assert!(channel
            .setup
            .modulation
            .add_route(mooloop_core::ModRoute::to_slot(
                0,
                ParamAddr::effect(
                    EffectTarget::Channel(0),
                    mooloop_core::DeviceId(0),
                    mooloop_core::FILTER_PARAM_CUTOFF_HZ,
                ),
                0.25,
                mooloop_core::ModPolarity::Bipolar,
            ))
            .is_some());

        let project = synth_project(channel);
        let mut fixed_project = project.clone();
        fixed_project.channels[0].setup.modulation = ModRack::default();
        let mut fixed = RenderState::from_project(48_000, &fixed_project, &[]);
        fixed.play();
        fixed.process_block(128);
        let mut render = RenderState::from_project(48_000, &project, &[]);
        // This command changes the base, not an absolute value that the LFO
        // will overwrite. It is deliberately issued before the block whose
        // event list we inspect.
        render.apply_command(EngineCommand::SetEffectParam {
            target: EffectTarget::Channel(0),
            slot: 0,
            id: mooloop_core::FILTER_PARAM_CUTOFF_HZ,
            value: 1_000.0,
        });
        render.play();
        render.process_block(128);

        let cutoff_events: Vec<_> = render.strips[0]
            .effects
            .event_scratch
            .iter()
            .filter_map(|event| match event.event {
                Event::ParamValue {
                    id: mooloop_core::FILTER_PARAM_CUTOFF_HZ,
                    value,
                } => Some((event.offset, value)),
                _ => None,
            })
            .collect();
        assert_eq!(
            cutoff_events
                .iter()
                .map(|(offset, _)| *offset)
                .collect::<Vec<_>>(),
            vec![0, 32, 64, 96],
        );
        let values: Vec<_> = cutoff_events.iter().map(|(_, value)| *value).collect();
        assert!(
            (values[0] - 1_000.0).abs() < 1.0,
            "base event was {values:?}"
        );
        assert!(
            values[1] > values[0] * 3.0,
            "LFO did not open cutoff: {values:?}"
        );
        assert!(
            values[3] < values[0] * 0.4,
            "LFO did not close cutoff: {values:?}"
        );
        assert_eq!(
            render.strips[0].effects.slot(0).and_then(|state| state.base_params)
                .unwrap()
                .get(mooloop_core::FILTER_PARAM_CUTOFF_HZ),
            Some(1_000.0),
        );
        let audible_difference: f32 = render.master().l[..128]
            .iter()
            .zip(&fixed.master().l[..128])
            .map(|(modulated, fixed)| (modulated - fixed).abs())
            .sum();
        assert!(
            audible_difference > 0.01,
            "LFO modulation did not change the rendered signal"
        );
    }

    fn filter_channel(cutoff_hz: f32) -> ProjectChannel {
        let mut channel = ProjectChannel::sampler(0, 1);
        channel
            .setup
            .push_effect(mooloop_core::EffectSlotState::filter(
                mooloop_core::FilterParams {
                    cutoff_hz,
                    resonance: 0.0,
                    mode: mooloop_core::FilterMode::LowPass,
                    ..mooloop_core::FilterParams::default()
                },
            ));
        channel
    }

    /// The filter added first by `filter_channel`, so its identity is the
    /// first one minted on that chain.
    const CUTOFF: ParamAddr = ParamAddr::effect(
        EffectTarget::Channel(0),
        mooloop_core::DeviceId(0),
        mooloop_core::FILTER_PARAM_CUTOFF_HZ,
    );

    fn cutoff_events(render: &RenderState) -> Vec<(u32, f32)> {
        render.strips[0]
            .effects
            .event_scratch
            .iter()
            .filter_map(|event| match event.event {
                Event::ParamValue {
                    id: mooloop_core::FILTER_PARAM_CUTOFF_HZ,
                    value,
                } => Some((event.offset, value)),
                _ => None,
            })
            .collect()
    }

    /// One LFO on the filter cutoff, and the durable id it was installed
    /// under. Narrow commands name that id rather than the slot it landed in,
    /// so the tests below can address the module the way the UI does.
    fn lfo_on_cutoff(depth: f32) -> (mooloop_core::Project, mooloop_core::ModSourceId) {
        let mut channel = filter_channel(1_000.0);
        let source = channel
            .setup
            .modulation
            .install(
                0,
                mooloop_core::ModulatorParams::Lfo(mooloop_core::ModLfoParams {
                    // A quarter-cycle every 32 frames at 48 kHz, so a
                    // 128-frame block reads four clearly different points.
                    rate_hz: 375.0,
                    ..mooloop_core::ModLfoParams::default()
                }),
            )
            .expect("slot 0 accepts a module");
        assert!(channel
            .setup
            .modulation
            .add_route(mooloop_core::ModRoute::to_slot(
                0,
                CUTOFF,
                depth,
                mooloop_core::ModPolarity::Bipolar,
            ))
            .is_some());
        (synth_project(channel), source)
    }

    /// A generator outlet reaching another device's parameter: the whole
    /// point of step 06, and the thing that could not be authored at all
    /// before a route could name something that is not a rack module.
    ///
    /// `Gate` is used rather than an envelope because it is a step, which is
    /// what makes the *timing* legible: the block the note lands in must show
    /// the cutoff still at its base, and the block after it must show the
    /// gate. That one-block gap is not a scheduling accident to be tolerated
    /// — it is the declared contract from `MODULATION.md`, and it
    /// is what makes an offline render agree with a live take.
    #[test]
    fn a_generator_outlet_drives_another_device_one_block_later() {
        use mooloop_core::mlp8::OUTLET_GATE;

        let mut channel = filter_channel(1_000.0);
        channel.setup.source = mooloop_core::ChannelSource::MlP8(mooloop_core::MlP8State::default());
        assert!(channel
            .setup
            .modulation
            .add_route(mooloop_core::ModRoute::from_outlet(
                OUTLET_GATE,
                CUTOFF,
                0.4,
                mooloop_core::ModPolarity::Bipolar,
            ))
            .is_some());

        let mut project = mooloop_core::Project {
            channels: vec![channel],
            ..mooloop_core::Project::default()
        };
        // One long note from the top of the pattern, so the gate goes high on
        // the first block and stays there.
        project.channels[0].notes[0].push(NoteEvent::new(1, 0, 384, 60, 127));

        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();

        // Block one: the note starts here, so the gate the control table is
        // holding is still the silence from before it.
        render.process_block(128);
        let first: Vec<f32> = cutoff_events(&render).iter().map(|(_, v)| *v).collect();
        // Not vacuous: a routed destination resolves every control tick, so
        // an empty list would mean the route was not running at all rather
        // than that it was running and reading silence.
        assert_eq!(first.len(), 4, "the route was not resolving: {first:?}");
        assert!(
            first.iter().all(|value| (value - 1_000.0).abs() < 1.0),
            "the outlet arrived in its own block: {first:?}"
        );

        // Block two: the gate published at the end of block one is what this
        // block's routes read.
        render.process_block(128);
        let second: Vec<f32> = cutoff_events(&render).iter().map(|(_, v)| *v).collect();
        assert_eq!(second.len(), 4, "the route stopped resolving: {second:?}");
        assert!(
            second.iter().all(|value| *value > 1_100.0),
            "the gate never reached the cutoff: {second:?}"
        );
    }

    /// The same contract from the other instrument, and the one DS-01's plan
    /// says is worth wanting soonest: a kick's `Trigger` reaching a later
    /// device without a sidechain graph.
    ///
    /// `Trigger` is the sharper timing test of the two. It is one publication
    /// wide, so three blocks tell the whole story -- base in the block the
    /// hit lands in, moved in the block after it, and back to base in the one
    /// after that. A trigger that leaked into a second block would be a
    /// device inventing a pulse width the table does not declare.
    #[test]
    fn a_ds01_trigger_drives_another_device_for_exactly_one_block() {
        use mooloop_core::ds01::DS01_OUTLET_TRIGGER;

        let mut channel = filter_channel(1_000.0);
        channel.setup.source = mooloop_core::ChannelSource::Ds01(mooloop_core::Ds01State::default());
        assert!(channel
            .setup
            .modulation
            .add_route(mooloop_core::ModRoute::from_outlet(
                DS01_OUTLET_TRIGGER,
                CUTOFF,
                0.4,
                // Bipolar is what passes a `0..1` outlet through unchanged;
                // `Session::arm_modulation_route` is where the two source
                // conventions are written down.
                mooloop_core::ModPolarity::Bipolar,
            ))
            .is_some());

        let mut project = mooloop_core::Project {
            channels: vec![channel],
            ..mooloop_core::Project::default()
        };
        // One hit at the top of the pattern. Its length does not matter: a
        // DS-01 one-shot ignores the note-off, and `Trigger` is about the
        // start either way.
        project.channels[0].notes[0].push(NoteEvent::new(1, 0, 24, 60, 127));

        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();

        let block = |render: &mut RenderState| {
            render.process_block(128);
            cutoff_events(render)
                .iter()
                .map(|(_, value)| *value)
                .collect::<Vec<f32>>()
        };

        let first = block(&mut render);
        assert_eq!(first.len(), 4, "the route was not resolving: {first:?}");
        assert!(
            first.iter().all(|value| (value - 1_000.0).abs() < 1.0),
            "the trigger arrived in its own block: {first:?}"
        );

        let second = block(&mut render);
        assert_eq!(second.len(), 4, "the route stopped resolving: {second:?}");
        assert!(
            second.iter().all(|value| *value > 1_100.0),
            "the trigger never reached the cutoff: {second:?}"
        );

        let third = block(&mut render);
        assert_eq!(third.len(), 4, "the route stopped resolving: {third:?}");
        assert!(
            third.iter().all(|value| (value - 1_000.0).abs() < 1.0),
            "the trigger stayed high for a second block: {third:?}"
        );
    }

    /// **A lane that stops covering the playhead hands its destination
    /// back**, which it did not until 2026-09-14.
    ///
    /// Pattern 0 sweeps the filter cutoff and pattern 1 has no such lane.
    /// After the switch `has_automation_at` answers false,
    /// `control_events_for_slot` takes its early return, and the filter goes
    /// on playing at whatever the curve last resolved while its knob and its
    /// face both read 1 kHz -- until somebody touches that knob or reloads
    /// the song. `restore_base_param` existed for exactly this and was
    /// reached only by a lane being *deleted*.
    #[test]
    fn switching_off_an_automated_pattern_hands_the_knob_back() {
        let mut project = synth_project(filter_channel(1_000.0));
        // A second pattern with nothing drawn on it, which is the whole
        // setup: one pattern cannot stop covering the playhead.
        project.pattern_lengths.push(DEFAULT_STEPS);
        project.channels[0].notes.push(Vec::new());
        project.channels[0].automation.push(Vec::new());

        let mut render = RenderState::from_project(48_000, &project, &[]);
        for (id, tick, value) in [(1u32, 0u32, 1.0f32), (2, 96, 0.0)] {
            render.apply_command(EngineCommand::UpsertAutomationPoint {
                pattern: 0,
                channel: 0,
                target: CUTOFF,
                point: mooloop_core::AutomationPoint::new(id, tick, value),
            });
        }
        render.play();
        render.process_block(128);

        let driven = cutoff_events(&render);
        assert_eq!(driven.len(), 4, "the lane was not resolving: {driven:?}");
        assert!(
            driven.iter().any(|(_, value)| (value - 1_000.0).abs() > 1.0),
            "the cutoff never left its knob value: {driven:?}"
        );

        render.apply_command(EngineCommand::SetCurrentPattern(1));
        render.process_block(128);

        // One event, at the top of the block, carrying the knob value back --
        // the same shape a route removal produces, and for the same reason.
        let restored = cutoff_events(&render);
        assert_eq!(
            restored.iter().map(|(offset, _)| *offset).collect::<Vec<_>>(),
            vec![0],
            "expected exactly one restoring event: {restored:?}"
        );
        assert!(
            (restored[0].1 - 1_000.0).abs() < 1.0,
            "the knob was not handed back: {restored:?}"
        );
    }

    /// And it hands it back with the transport stopped, which is the half
    /// that nearly went missing on 2026-09-20.
    ///
    /// Step 01 of `docs/plans/transport-discontinuity/` stopped charging a
    /// view change for a discontinuity, and the first draft put both debts
    /// this command owes behind one condition that included
    /// `transport.playing`. The voice release is genuinely conditional on it
    /// -- stopped, no sequenced note-off has been stranded -- but the lane
    /// restore is not: `process` resolves lanes whether or not the transport
    /// is running, deliberately, so that a knob does not jump the moment you
    /// press play -- `effect_is_driven`'s own doc says it: *"playing or
    /// stopped does not matter; lanes resolve either way"*. A paused switch
    /// off an automated pattern therefore parks the destination exactly as a
    /// running one does, and the two conditions have to stay apart.
    #[test]
    fn switching_off_an_automated_pattern_hands_the_knob_back_while_stopped() {
        let mut project = synth_project(filter_channel(1_000.0));
        project.pattern_lengths.push(DEFAULT_STEPS);
        project.channels[0].notes.push(Vec::new());
        project.channels[0].automation.push(Vec::new());

        let mut render = RenderState::from_project(48_000, &project, &[]);
        for (id, tick, value) in [(1u32, 0u32, 1.0f32), (2, 96, 0.0)] {
            render.apply_command(EngineCommand::UpsertAutomationPoint {
                pattern: 0,
                channel: 0,
                target: CUTOFF,
                point: mooloop_core::AutomationPoint::new(id, tick, value),
            });
        }
        // Play far enough for the curve to take the cutoff off its knob, then
        // pause. An idle strip is skipped, so a transport that never ran
        // would leave the device holding its knob value and there would be
        // nothing stranded to hand back -- the hazard needs a lane that has
        // actually resolved.
        render.play();
        render.process_block(128);
        let driven = cutoff_events(&render);
        assert!(
            driven.iter().any(|(_, value)| (value - 1_000.0).abs() > 1.0),
            "the cutoff never left its knob value: {driven:?}"
        );

        render.apply_command(EngineCommand::Pause);
        render.process_block(128);
        assert!(!render.transport.playing);

        render.apply_command(EngineCommand::SetCurrentPattern(1));
        render.process_block(128);

        let restored = cutoff_events(&render);
        assert_eq!(
            restored.iter().map(|(offset, _)| *offset).collect::<Vec<_>>(),
            vec![0],
            "expected exactly one restoring event: {restored:?}"
        );
        assert!(
            (restored[0].1 - 1_000.0).abs() < 1.0,
            "the knob was not handed back: {restored:?}"
        );
    }

    /// The other half of the same rule: a destination that is **still**
    /// automated after the switch must not be written at all.
    ///
    /// The cheap version of this fix -- restore everything the outgoing
    /// position drove and let the automation pass re-assert it -- passes the
    /// test above and fails this one. Measured rather than assumed: dropping
    /// the still-covered guard puts `(0, 1000.0)` in front of the curve's own
    /// `(0, 19276.6)`, so the two do *not* coalesce and the knob write is not
    /// silently dropped. It is inaudible, because both land on frame 0 and
    /// the later one wins -- and it is still a `ParamValue` that says
    /// something untrue, at an offset every effect in the chain splits its
    /// block on.
    #[test]
    fn switching_between_two_automated_patterns_disturbs_nothing() {
        let mut project = synth_project(filter_channel(1_000.0));
        project.pattern_lengths.push(DEFAULT_STEPS);
        project.channels[0].notes.push(Vec::new());
        project.channels[0].automation.push(Vec::new());

        let mut render = RenderState::from_project(48_000, &project, &[]);
        for pattern in 0..2u8 {
            for (id, tick, value) in [(1u32, 0u32, 1.0f32), (2, 96, 0.0)] {
                render.apply_command(EngineCommand::UpsertAutomationPoint {
                    pattern,
                    channel: 0,
                    target: CUTOFF,
                    point: mooloop_core::AutomationPoint::new(id, tick, value),
                });
            }
        }
        render.play();
        render.process_block(128);

        render.apply_command(EngineCommand::SetCurrentPattern(1));
        render.process_block(128);

        let driven = cutoff_events(&render);
        assert_eq!(
            driven.len(),
            4,
            "the incoming lane should still resolve once per control tick: {driven:?}"
        );
        assert!(
            driven.iter().all(|(_, value)| (value - 1_000.0).abs() > 1.0),
            "a restoring write landed on a destination that is still \
             automated: {driven:?}"
        );
    }

    /// The property a narrow command could get wrong rather than merely
    /// cheap: dropping one route has to hand the device back its knob value.
    /// Without it the filter would hold whatever the LFO last resolved, until
    /// someone happened to touch that knob again.
    #[test]
    fn a_narrow_route_removal_restores_the_destinations_base() {
        let (project, source) = lfo_on_cutoff(0.25);
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        render.process_block(128);

        let modulated: Vec<_> = cutoff_events(&render)
            .iter()
            .map(|(_, value)| *value)
            .collect();
        assert_eq!(modulated.len(), 4, "LFO was not resolving: {modulated:?}");
        assert!(
            modulated.iter().any(|value| (value - 1_000.0).abs() > 100.0),
            "cutoff never left its base: {modulated:?}"
        );

        render.apply_command(EngineCommand::RemoveModRoute {
            channel: 0,
            source: mooloop_core::ModSourceRef::Id(source),
            destination: CUTOFF,
        });
        render.process_block(128);

        // One event, at the top of the block, carrying the knob value back.
        let restored = cutoff_events(&render);
        assert_eq!(
            restored.iter().map(|(offset, _)| *offset).collect::<Vec<_>>(),
            vec![0],
            "expected exactly one restoring event: {restored:?}"
        );
        assert!(
            (restored[0].1 - 1_000.0).abs() < 1.0,
            "the base was not restored: {restored:?}"
        );
        assert!(!render.effect_is_driven(
            EffectTarget::Channel(0),
            0,
            mooloop_core::FILTER_PARAM_CUTOFF_HZ
        ));
    }

    /// The defect this guards used to be that a route and a lane named their
    /// destination by *slot*, so reordering the chain left both pointing at
    /// the old number and the LFO on the filter's cutoff started driving
    /// whatever slid into that slot.
    ///
    /// It is now guarding something stronger and simpler: **the addresses do
    /// not change at all.** `CUTOFF` is built once, before the reorder, and
    /// still resolves afterwards -- there is no `moved` address, because
    /// there is nothing for the reorder to move.
    #[test]
    fn a_route_and_a_lane_follow_their_device_through_a_reorder_and_die_with_it() {
        let (project, _source) = lfo_on_cutoff(0.25);
        let mut render = RenderState::from_project(48_000, &project, &[]);
        let channel = EffectTarget::Channel(0);
        let _ = render.apply_structural(install_effect(
            channel,
            1,
            default_effect(mooloop_core::EffectKind::Drive),
        ));
        render.apply_command(EngineCommand::UpsertAutomationPoint {
            pattern: 0,
            channel: 0,
            target: CUTOFF,
            point: mooloop_core::AutomationPoint::new(1, 0, 0.5),
        });
        assert!(render.effect_is_driven(channel, 0, mooloop_core::FILTER_PARAM_CUTOFF_HZ));
        assert!(render.sequencer.automation_lane_at(CUTOFF, 0.0).is_some());

        // Filter to the end of the chain: drive first, filter second.
        render.apply_command(EngineCommand::MoveEffect {
            target: channel,
            from: 0,
            to: 1,
        });
        assert!(
            !render.effect_is_driven(channel, 0, mooloop_core::FILTER_PARAM_CUTOFF_HZ),
            "the drive inherited the filter's route or lane"
        );
        assert!(
            render.effect_is_driven(channel, 1, mooloop_core::FILTER_PARAM_CUTOFF_HZ),
            "the route and lane did not follow the filter"
        );
        // The address built before the reorder is the address after it. This
        // is the assertion the slot scheme could not make: there, `CUTOFF`
        // would now name the drive and the lane would have been rewritten.
        assert!(
            render.sequencer.automation_lane_at(CUTOFF, 0.0).is_some(),
            "the lane stopped resolving even though its device is still there"
        );
        assert_eq!(
            render.strips[0].effects.slot(1).and_then(|slot| slot.kind),
            Some(mooloop_core::EffectKind::Filter)
        );

        // And the filter in its new slot is actually being driven: the scratch
        // list holds the last slot's events after a block, which is now the
        // filter's.
        render.play();
        render.process_block(128);
        let events = cutoff_events(&render);
        assert!(
            events.iter().any(|(_, value)| (value - 1_000.0).abs() > 100.0),
            "the filter in slot 1 never left its base: {events:?}"
        );

        // Removing the filter takes its route and its lane with it rather than
        // leaving either parked on an empty slot for the next device to inherit.
        let _ = render.apply_structural(StructuralCommand::RemoveEffect {
            target: channel,
            slot: 1,
        });
        assert_eq!(render.modulation[0].routes.iter().flatten().count(), 0);
        assert!(render.sequencer.automation_lane_at(CUTOFF, 0.0).is_none());
    }

    /// Emptying a slot is the same fact stated once for every route it drove.
    #[test]
    fn clearing_a_module_restores_what_it_was_driving() {
        let (project, _) = lfo_on_cutoff(0.25);
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        render.process_block(128);

        render.apply_command(EngineCommand::ClearModulator {
            channel: 0,
            slot: 0,
        });
        render.process_block(128);

        let restored = cutoff_events(&render);
        assert_eq!(
            restored.iter().map(|(offset, _)| *offset).collect::<Vec<_>>(),
            vec![0],
            "expected exactly one restoring event: {restored:?}"
        );
        assert!((restored[0].1 - 1_000.0).abs() < 1.0, "{restored:?}");
    }

    /// The ordinary knob turn: one small command, and the module it names
    /// keeps running rather than being rebuilt around the new value.
    #[test]
    fn a_narrow_parameter_command_retunes_the_running_module() {
        let (project, _) = lfo_on_cutoff(0.25);
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        render.process_block(128);

        render.apply_command(EngineCommand::SetModulatorParam {
            channel: 0,
            slot: 0,
            id: mooloop_core::LFO_PARAM_DEPTH,
            value: 0.0,
        });
        render.process_block(128);

        // Still resolving four times a block, because the route is intact --
        // but at zero depth every tick lands on the base.
        let values: Vec<_> = cutoff_events(&render)
            .iter()
            .map(|(_, value)| *value)
            .collect();
        assert_eq!(values.len(), 4, "the route stopped resolving: {values:?}");
        assert!(
            values.iter().all(|value| (value - 1_000.0).abs() < 1.0),
            "depth 0 still moved the cutoff: {values:?}"
        );
    }

    /// The ordering trap this step had to avoid. Slot edits and route edits
    /// arrive as separate ring entries, so a route may name a module the
    /// engine does not hold. Because a route names a durable id and not a
    /// slot number, that route is refused outright rather than aimed at
    /// whatever else happens to occupy the slot.
    #[test]
    fn a_route_naming_an_absent_module_is_inert() {
        let project = synth_project(filter_channel(1_000.0));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.apply_command(EngineCommand::SetModRoute {
            channel: 0,
            route: mooloop_core::ModRoute {
                source: mooloop_core::ModSourceRef::Id(mooloop_core::ModSourceId(7)),
                source_slot: 0,
                destination: CUTOFF,
                depth: 1.0,
                polarity: mooloop_core::ModPolarity::Bipolar,
            },
        });
        render.play();
        render.process_block(128);

        assert!(!render.effect_is_driven(
            EffectTarget::Channel(0),
            0,
            mooloop_core::FILTER_PARAM_CUTOFF_HZ
        ));
        assert!(
            cutoff_events(&render).is_empty(),
            "an unresolvable route reached the device"
        );
    }

    #[test]
    fn an_automation_lane_resolves_a_param_at_the_control_rate() {
        let project = synth_project(filter_channel(1_000.0));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        let descriptor = mooloop_core::EffectKind::Filter
            .descriptor(mooloop_core::FILTER_PARAM_CUTOFF_HZ)
            .expect("cutoff is a described parameter");

        // A ramp across the first sixteenth, so a 128-frame block at 120 BPM
        // sits entirely inside the rising segment.
        for (id, tick, value) in [(1u32, 0u32, 0.0f32), (2, 24, 1.0)] {
            render.apply_command(EngineCommand::UpsertAutomationPoint {
                pattern: 0,
                channel: 0,
                target: CUTOFF,
                point: mooloop_core::AutomationPoint::new(id, tick, value),
            });
        }
        render.play();
        render.process_block(128);

        let events = cutoff_events(&render);
        assert_eq!(
            events.iter().map(|(offset, _)| *offset).collect::<Vec<_>>(),
            vec![0, 32, 64, 96],
        );
        let values: Vec<_> = events.iter().map(|(_, value)| *value).collect();
        assert!(
            (values[0] - descriptor.min).abs() < 1.0,
            "the lane did not start at its first point: {values:?}"
        );
        assert!(
            values.windows(2).all(|pair| pair[1] > pair[0]),
            "the ramp did not rise across the block: {values:?}"
        );
        // The knob is untouched: a lane supplies the base, it does not
        // overwrite what the user set.
        assert_eq!(
            render.strips[0].effects.slot(0).and_then(|state| state.base_params)
                .unwrap()
                .get(mooloop_core::FILTER_PARAM_CUTOFF_HZ),
            Some(1_000.0),
        );
    }

    #[test]
    fn a_lane_supplies_the_base_that_modulation_then_offsets() {
        let mut channel = filter_channel(1_000.0);
        channel.setup.modulation.install(0, mooloop_core::ModulatorParams::Lfo(
            mooloop_core::ModLfoParams {
                rate_hz: 375.0,
                ..mooloop_core::ModLfoParams::default()
            },
        ));
        assert!(channel
            .setup
            .modulation
            .add_route(mooloop_core::ModRoute::to_slot(
                0,
                CUTOFF,
                0.25,
                mooloop_core::ModPolarity::Bipolar,
            ))
            .is_some());
        let project = synth_project(channel);

        // A flat lane at half scale. With no modulation every control tick
        // would read the same value; the LFO is the only thing that can make
        // them differ, and it must differ *around the lane*, not the knob.
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.apply_command(EngineCommand::UpsertAutomationPoint {
            pattern: 0,
            channel: 0,
            target: CUTOFF,
            point: mooloop_core::AutomationPoint::new(1, 0, 0.5),
        });
        render.play();
        render.process_block(128);
        let values: Vec<_> = cutoff_events(&render)
            .iter()
            .map(|(_, value)| *value)
            .collect();

        let flat = mooloop_core::EffectKind::Filter
            .descriptor(mooloop_core::FILTER_PARAM_CUTOFF_HZ)
            .expect("cutoff is a described parameter")
            .from_normalized(0.5);
        assert!(
            (values[0] - flat).abs() < flat * 0.02,
            "the first tick should sit on the lane, not the 1 kHz knob: {values:?}"
        );
        assert!(
            values[1] > values[0] * 1.5 && values[3] < values[0] * 0.7,
            "the LFO did not swing around the lane value: {values:?}"
        );
    }

    #[test]
    fn clearing_a_lane_returns_the_destination_to_its_knob() {
        let project = synth_project(filter_channel(1_000.0));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.apply_command(EngineCommand::UpsertAutomationPoint {
            pattern: 0,
            channel: 0,
            target: CUTOFF,
            point: mooloop_core::AutomationPoint::new(1, 0, 0.1),
        });
        render.play();
        render.process_block(128);
        let automated = cutoff_events(&render)[0].1;
        assert!(automated < 900.0, "lane did not take the base: {automated}");

        render.apply_command(EngineCommand::ClearAutomationLane {
            pattern: 0,
            channel: 0,
            target: CUTOFF,
        });
        render.process_block(128);
        let restored = cutoff_events(&render);
        assert_eq!(
            restored.len(),
            1,
            "an empty lane should stop resolving per control tick: {restored:?}"
        );
        assert_eq!(restored[0], (0, 1_000.0));
    }

    /// The cutoff values waiting in slot 0's between-block queue: what a knob
    /// edit sends straight to the device, ahead of any control signal.
    fn queued_cutoff(render: &RenderState) -> Vec<(u32, f32)> {
        render.strips[0]
            .effects
            .slot(0)
            .expect("slot 0 holds the filter")
            .events
            .events
            .iter()
            .flatten()
            .filter_map(|event| match event.event {
                Event::ParamValue {
                    id: mooloop_core::FILTER_PARAM_CUTOFF_HZ,
                    value,
                } => Some((event.offset, value)),
                _ => None,
            })
            .collect()
    }

    fn turn_cutoff_knob(render: &mut RenderState, value: f32) {
        render.apply_command(EngineCommand::SetEffectParam {
            target: EffectTarget::Channel(0),
            slot: 0,
            id: mooloop_core::FILTER_PARAM_CUTOFF_HZ,
            value,
        });
    }

    /// Precedence, lane over knob: a lane is the base, so a knob edit under
    /// one changes only the stored knob and sends the device nothing. Queuing
    /// it put a second, stray value at offset 0 ahead of the lane's.
    #[test]
    fn a_knob_edit_under_a_lane_queues_no_value() {
        let project = synth_project(filter_channel(1_000.0));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.apply_command(EngineCommand::UpsertAutomationPoint {
            pattern: 0,
            channel: 0,
            target: CUTOFF,
            point: mooloop_core::AutomationPoint::new(1, 0, 0.1),
        });
        render.play();
        render.process_block(128);

        turn_cutoff_knob(&mut render, 5_000.0);
        assert_eq!(
            queued_cutoff(&render),
            vec![],
            "the knob reached the device under a lane"
        );
        render.process_block(128);
        let events = cutoff_events(&render);
        assert_eq!(
            events.iter().map(|(offset, _)| *offset).collect::<Vec<_>>(),
            vec![0, 32, 64, 96],
            "only the lane should resolve the cutoff: {events:?}"
        );
        assert!(
            events.iter().all(|(_, value)| *value < 900.0),
            "the knob value sounded under the lane: {events:?}"
        );
        // The knob is still stored, so clearing the lane hands it back.
        assert_eq!(
            render.strips[0].effects.slot(0).and_then(|state| state.base_params)
                .unwrap()
                .get(mooloop_core::FILTER_PARAM_CUTOFF_HZ),
            Some(5_000.0),
        );
    }

    /// Precedence, route over knob: a route resolves the destination every
    /// control tick from the knob's base, so the edit is heard through it.
    #[test]
    fn a_knob_edit_under_a_route_queues_no_value() {
        let (project, _) = lfo_on_cutoff(0.25);
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        render.process_block(128);

        turn_cutoff_knob(&mut render, 5_000.0);
        assert_eq!(
            queued_cutoff(&render),
            vec![],
            "the knob reached the device under a route"
        );
    }

    /// Precedence, knob alone: nothing else will write the destination, so
    /// the knob's value is queued for the next block's first frame.
    #[test]
    fn a_knob_edit_with_nothing_driving_it_queues_one_value() {
        let project = synth_project(filter_channel(1_000.0));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        render.process_block(128);

        turn_cutoff_knob(&mut render, 5_000.0);
        assert_eq!(queued_cutoff(&render), vec![(0, 5_000.0)]);
    }

    #[test]
    fn a_lane_survives_a_project_round_trip_through_the_sequencer() {
        let mut project = synth_project(filter_channel(1_000.0));
        let mut lane = mooloop_core::AutomationLane::new(CUTOFF);
        lane.upsert(mooloop_core::AutomationPoint::new(1, 0, 0.25));
        project.channels[0].automation[0].push(lane);

        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        render.process_block(128);
        let loaded = cutoff_events(&render)[0].1;
        let expected = mooloop_core::EffectKind::Filter
            .descriptor(mooloop_core::FILTER_PARAM_CUTOFF_HZ)
            .expect("cutoff is a described parameter")
            .from_normalized(0.25);
        assert!(
            (loaded - expected).abs() < expected * 0.02,
            "a loaded lane did not drive the destination: {loaded} vs {expected}"
        );
    }

    #[test]
    fn a_lane_drives_the_buffer_read_head() {
        // The point of the whole exercise: a curve drawn in a clip moves a
        // retained-audio read head, with no gesture and no MIDI involved.
        let mut channel = ProjectChannel::sampler(0, 1);
        channel
            .setup
            .push_effect(mooloop_core::EffectSlotState::new(
                mooloop_core::EffectParams::Buffer(mooloop_core::BufferParams {
                    bars: 1,
                    ..mooloop_core::BufferParams::default()
                }),
            ));
        let project = synth_project(channel);
        let target = ParamAddr::effect(
            EffectTarget::Channel(0),
            mooloop_core::DeviceId(0),
            mooloop_core::BUFFER_PARAM_POSITION,
        );

        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        // Fill the ring before asking the head to look backward into it.
        render.process_block(2048);

        // `Position` counts from the old end, so the curve starts at the
        // write head and travels back into the history. The old spelling of
        // this lane ran 0.0 to 0.25 and meant the same journey the other way
        // round.
        for (id, tick, value) in [(1u32, 0u32, 1.0f32), (2, 96, 0.75)] {
            render.apply_command(EngineCommand::UpsertAutomationPoint {
                pattern: 0,
                channel: 0,
                target,
                point: mooloop_core::AutomationPoint::new(id, tick, value),
            });
        }
        for _ in 0..8 {
            render.process_block(2048);
        }

        let events: Vec<f32> = render.strips[0]
            .effects
            .event_scratch
            .iter()
            .filter_map(|event| match event.event {
                Event::ParamValue {
                    id: mooloop_core::BUFFER_PARAM_POSITION,
                    value,
                } => Some(value),
                _ => None,
            })
            .collect();
        assert!(
            events.len() > 1,
            "the lane did not resolve at the control rate: {events:?}"
        );
        assert!(
            events.iter().any(|value| *value < 1.0),
            "the lane never moved the head off the write position: {events:?}"
        );
    }

    // --- ML-P8 internal routes ------------------------------------------

    /// An ML-P8 channel with one internal route already authored, and the
    /// route's durable id.
    fn mlp8_project_with_route() -> (Project, u16) {
        let mut channel = ProjectChannel::mlp8(0, 1);
        let state = channel
            .setup
            .mlp8_state_mut()
            .expect("an ML-P8 channel has ML-P8 state");
        let id = state
            .params
            .routes
            .add(
                mooloop_core::MlP8ModSource::Lfo,
                mooloop_core::MlP8ModDest::Param {
                    id: mooloop_core::mlp8::PARAM_FILTER_CUTOFF,
                },
            )
            .expect("the route should be accepted");
        assert!(state.params.routes.set_amount(id, 40.0));
        (synth_project(channel), id)
    }

    fn route_amounts(render: &RenderState, route: u16) -> Vec<f32> {
        render.events[0]
            .iter()
            .filter_map(|event| match event.event {
                Event::SourceRouteAmount { route: id, amount } if id == route => Some(amount),
                _ => None,
            })
            .collect()
    }

    fn authored_amount(render: &RenderState, route: u16) -> f32 {
        render.strips[0]
            .source_base
            .internal_routes()
            .and_then(|routes| routes.get(route))
            .expect("the route should still be authored")
            .amount
    }

    /// A lane drawn on a route's amount resolves at the control rate through
    /// the ordinary event path, exactly like a lane on a knob -- but through
    /// the route's durable id rather than a parameter of the device.
    #[test]
    fn a_lane_drives_an_internal_route_amount() {
        let (project, route) = mlp8_project_with_route();
        let target = ParamAddr::source_route(
            EffectTarget::Channel(0),
            route,
            mooloop_core::MLP8_ROUTE_PARAM_AMOUNT,
        );
        let mut render = RenderState::from_project(48_000, &project, &[]);
        for (id, tick, value) in [(1u32, 0u32, 1.0f32), (2, 96, 0.0)] {
            render.apply_command(EngineCommand::UpsertAutomationPoint {
                pattern: 0,
                channel: 0,
                target,
                point: mooloop_core::AutomationPoint::new(id, tick, value),
            });
        }
        render.play();
        render.process_block(128);

        let values = route_amounts(&render, route);
        assert_eq!(
            values.len(),
            4,
            "the lane should resolve once per control tick: {values:?}"
        );
        assert!(
            values.windows(2).all(|pair| pair[1] < pair[0]),
            "the ramp did not fall across the block: {values:?}"
        );
        // Full scale at the top of the lane and centre at the bottom, which
        // is the route amount's own -100..100 range rather than a unit one.
        assert!((values[0] - 100.0).abs() < 1.0, "{values:?}");
        // The authored depth is untouched: a lane supplies the base.
        assert_eq!(authored_amount(&render, route), 40.0);
    }

    /// A route with no lane and no rack route on it emits nothing at all, so
    /// an ordinary ML-P8 patch does not pay for the machinery.
    #[test]
    fn an_unautomated_route_amount_emits_no_events() {
        let (project, route) = mlp8_project_with_route();
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.play();
        render.process_block(128);
        assert!(route_amounts(&render, route).is_empty());
    }

    /// Moving a depth is not a structural edit. The engine writes it to the
    /// authored base and to the running node without pushing the whole
    /// parameter block, which is what would rebuild the compiled topology.
    #[test]
    fn setting_a_route_amount_keeps_the_authored_topology() {
        let (project, route) = mlp8_project_with_route();
        let mut render = RenderState::from_project(48_000, &project, &[]);
        let before = *render.strips[0]
            .source_base
            .internal_routes()
            .expect("ML-P8 has internal routes");

        render.apply_command(EngineCommand::SetSourceRouteAmount {
            channel: 0,
            route,
            amount: -80.0,
        });
        let after = render.strips[0]
            .source_base
            .internal_routes()
            .expect("ML-P8 has internal routes");
        assert!(
            before.same_topology(after),
            "moving a depth changed the topology"
        );
        assert_eq!(authored_amount(&render, route), -80.0);
    }

    /// Adding and removing a route is structural, and both directions land on
    /// the authored base so a save records what is being heard.
    #[test]
    fn a_route_can_be_added_and_removed_through_the_command_ring() {
        let project = synth_project(ProjectChannel::mlp8(0, 1));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        let route = mooloop_core::MlP8Route {
            id: 7,
            source: mooloop_core::MlP8ModSource::FilterEnv,
            dest: mooloop_core::MlP8ModDest::Param {
                id: mooloop_core::mlp8::PARAM_DRIVE,
            },
            amount: 55.0,
        };

        render.apply_command(EngineCommand::SetSourceRoute { channel: 0, route });
        assert_eq!(authored_amount(&render, 7), 55.0);

        // Repointing under the same id replaces rather than adds.
        render.apply_command(EngineCommand::SetSourceRoute {
            channel: 0,
            route: mooloop_core::MlP8Route {
                dest: mooloop_core::MlP8ModDest::VcaLevel,
                ..route
            },
        });
        let routes = render.strips[0]
            .source_base
            .internal_routes()
            .expect("ML-P8 has internal routes");
        assert_eq!(routes.len(), 1);
        assert_eq!(routes.get(7).unwrap().dest, mooloop_core::MlP8ModDest::VcaLevel);

        render.apply_command(EngineCommand::RemoveSourceRoute { channel: 0, route: 7 });
        assert!(render.strips[0]
            .source_base
            .internal_routes()
            .expect("ML-P8 has internal routes")
            .is_empty());
    }

    /// **A reorder moves each module's running state with it**, which it did
    /// not until 2026-09-14.
    ///
    /// `edit_modulation` mirrors a rack edit into the DSP rack as a params
    /// diff by slot number, and a reorder is exactly the edit a diff by slot
    /// number cannot see. Two LFOs dragged past each other cross-wired: each
    /// kept its own phase, smoothing and fade position and took the *other's*
    /// params, because `set_slot` retunes in place. Both jumped and nothing
    /// said why. Different kinds swapped were worse in a different direction
    /// -- both rebuilt from scratch, so an envelope restarted at level 0
    /// mid-sustain and a Random module was reseeded, which also broke the
    /// promise that an offline render matches a realtime take.
    ///
    /// Driven through `apply_command` rather than through the rack directly,
    /// because the defect was in the mirroring and not in either rack.
    #[test]
    fn reordering_the_grid_carries_each_modules_running_state() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        for (slot, rate, phase) in [(0u8, 1.0f32, 0.0f32), (1, 7.0, 0.25)] {
            render.apply_command(EngineCommand::InstallModulator {
                channel: 0,
                slot,
                source: mooloop_core::ModSourceId(u32::from(slot)),
                params: mooloop_core::ModulatorParams::Lfo(mooloop_core::ModLfoParams {
                    rate_hz: rate,
                    phase,
                    ..mooloop_core::ModLfoParams::default()
                }),
            });
        }
        render.play();
        for _ in 0..8 {
            render.process_block(256);
        }

        let before = *render.modulators[0].outputs();
        assert!(
            (before[0] - before[1]).abs() > 0.05,
            "the two LFOs were indistinguishable to begin with: {before:?}"
        );

        render.apply_command(EngineCommand::MoveModulator {
            channel: 0,
            from: 0,
            to: 1,
        });

        let after = *render.modulators[0].outputs();
        assert_eq!(
            (after[0], after[1]),
            (before[1], before[0]),
            "the swap rebuilt or cross-wired the modules rather than moving them"
        );
        // The control rack agrees about which is which, so a route that
        // followed the move reads the module it named.
        let rates: Vec<f32> = render.modulation[0]
            .slots
            .iter()
            .flatten()
            .map(|slot| match slot.params {
                mooloop_core::ModulatorParams::Lfo(lfo) => lfo.rate_hz,
                _ => f32::NAN,
            })
            .collect();
        assert_eq!(rates, vec![7.0, 1.0], "the control rack did not permute");
    }

    /// A generator that has no internal routes ignores the commands entirely
    /// rather than misapplying them.
    #[test]
    fn a_device_without_internal_routes_ignores_route_commands() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.apply_command(EngineCommand::SetSourceRoute {
            channel: 0,
            route: mooloop_core::MlP8Route {
                id: 1,
                source: mooloop_core::MlP8ModSource::Lfo,
                dest: mooloop_core::MlP8ModDest::VcaLevel,
                amount: 100.0,
            },
        });
        render.apply_command(EngineCommand::SetSourceRouteAmount {
            channel: 0,
            route: 1,
            amount: 100.0,
        });
        assert!(render.strips[0].source_base.internal_routes().is_none());
    }

    /// Removing a lane returns the route to its authored depth, which is the
    /// same promise a knob gets. Without it the device would keep whatever
    /// the lane last resolved.
    #[test]
    fn removing_a_route_lane_restores_the_authored_depth() {
        let (project, route) = mlp8_project_with_route();
        let target = ParamAddr::source_route(
            EffectTarget::Channel(0),
            route,
            mooloop_core::MLP8_ROUTE_PARAM_AMOUNT,
        );
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.apply_command(EngineCommand::UpsertAutomationPoint {
            pattern: 0,
            channel: 0,
            target,
            point: mooloop_core::AutomationPoint::new(1, 0, 1.0),
        });
        render.play();
        render.process_block(128);
        assert!(!route_amounts(&render, route).is_empty());

        render.apply_command(EngineCommand::RemoveAutomationLane {
            pattern: 0,
            channel: 0,
            target,
        });
        render.process_block(128);
        assert!(
            route_amounts(&render, route).is_empty(),
            "a removed lane kept driving the route"
        );
        assert_eq!(authored_amount(&render, route), 40.0);
    }

    #[test]
    fn a_lane_drives_a_generator_parameter() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let target = ParamAddr {
            scope: EffectTarget::Channel(0),
            owner: ParamOwner::Source,
            param: mooloop_core::SAMPLER_PARAM_FILTER_CUTOFF,
        };
        let mut render = RenderState::from_project(48_000, &project, &[]);
        for (id, tick, value) in [(1u32, 0u32, 1.0f32), (2, 96, 0.0)] {
            render.apply_command(EngineCommand::UpsertAutomationPoint {
                pattern: 0,
                channel: 0,
                target,
                point: mooloop_core::AutomationPoint::new(id, tick, value),
            });
        }
        render.play();
        render.process_block(128);

        let values: Vec<f32> = render.events[0]
            .iter()
            .filter_map(|event| match event.event {
                Event::ParamValue {
                    id: mooloop_core::SAMPLER_PARAM_FILTER_CUTOFF,
                    value,
                } => Some(value),
                _ => None,
            })
            .collect();
        assert_eq!(
            values.len(),
            4,
            "the lane should resolve once per control tick: {values:?}"
        );
        assert!(
            values.windows(2).all(|pair| pair[1] < pair[0]),
            "the ramp did not fall across the block: {values:?}"
        );
        // The knob is untouched: a lane supplies the base, it does not
        // overwrite what the user set.
        assert_eq!(
            render.strips[0]
                .source_base
                .get(mooloop_core::SAMPLER_PARAM_FILTER_CUTOFF),
            Some(1.0),
        );
    }

    // --- Latency compensation ------------------------------------------

    /// A Drive that is a pure fifteen-frame delay and nothing else.
    ///
    /// At `mix: 0` the effect outputs its own aligned dry path, so the shaper
    /// contributes nothing audible and what is left is exactly the
    /// oversampler's latency. That makes it the cleanest possible probe: any
    /// difference these tests find is timing rather than timbre.
    fn transparent_drive() -> mooloop_core::EffectSlotState {
        mooloop_core::EffectSlotState::drive(mooloop_core::DriveParams {
            drive: 1.0,
            curve: mooloop_core::DriveCurve::Soft,
            tone: 0.0,
            mix: 0.0,
            output: 1.0,
        })
    }

    /// One drum channel that hits on the downbeat, optionally through the
    /// transparent Drive.
    fn hit_channel(index: usize, latent: bool) -> ProjectChannel {
        let mut channel = ProjectChannel::drum_synth(index, 1);
        if latent {
            channel.setup.push_effect(transparent_drive());
        }
        channel.notes[0].push(NoteEvent::new(index as u32 + 1, 0, 24, 60, 127));
        channel
    }

    fn render_master(project: &Project, frames: usize) -> Vec<f32> {
        let mut render = RenderState::from_project(48_000, project, &[]);
        render.play();
        render.process_block(frames);
        render.master().l[..frames].to_vec()
    }

    /// The headline case, and the one that is silently wrong without this
    /// plan: two channels hitting on the same tick, one of them through a
    /// device that costs fifteen frames. They must land in the same frame.
    ///
    /// Both assertions fail on `main`. The plain channel would start at frame
    /// zero while its neighbour started at fifteen, and the master would carry
    /// one copy of the hit followed by a second — which is comb filtering,
    /// worst exactly when the two channels are most alike.
    #[test]
    fn two_channels_of_different_depths_land_in_the_same_frame() {
        const FRAMES: usize = 512;
        let latency = mooloop_core::effect::OVERSAMPLER_LATENCY_FRAMES as usize;

        let together = render_master(
            &Project {
                channels: vec![hit_channel(0, true), hit_channel(1, false)],
                ..full_bank_project()
            },
            FRAMES,
        );
        // Nothing arrives early: the shorter channel waited for the longer.
        assert!(
            together[..latency].iter().all(|sample| *sample == 0.0),
            "the plain channel arrived {latency} frames early"
        );

        // And they are aligned *exactly*, not merely both late. One channel
        // alone is the longest path and is compensated by nothing, so it is
        // the reference; two identical channels summed on top of each other
        // must be it, doubled, sample for sample.
        let alone = render_master(
            &Project {
                channels: vec![hit_channel(0, true)],
                ..full_bank_project()
            },
            FRAMES,
        );
        for (frame, (summed, single)) in together.iter().zip(alone.iter()).enumerate() {
            assert!(
                (summed - single * 2.0).abs() < 1.0e-6,
                "frame {frame}: two aligned copies gave {summed}, one copy doubled is {}",
                single * 2.0
            );
        }
    }

    /// The same through a bus, which is the case a per-channel-only scheme
    /// gets wrong: the latency is on the *bus*, so what has to wait is the
    /// channel that does not go through it.
    #[test]
    fn a_channel_waits_for_a_latent_bus_beside_it() {
        const FRAMES: usize = 512;
        let latency = mooloop_core::effect::OVERSAMPLER_LATENCY_FRAMES as usize;

        let mut project = Project {
            channels: vec![hit_channel(0, false), hit_channel(1, false)],
            ..full_bank_project()
        };
        // Channel 0 through bus 1, which carries the cost; channel 1 straight
        // to the master with nothing.
        project.channels[0].setup.channel.bus = 1;
        project.buses[1].push_effect(transparent_drive());

        let master = render_master(&project, FRAMES);
        assert!(
            master[..latency].iter().all(|sample| *sample == 0.0),
            "the direct channel did not wait for the bus"
        );
    }

    /// Bypass keeps its latency, which is the convention every host follows
    /// and the reason is audible: a bypass that shortened the chain would move
    /// the channel in time, so A/B-ing an effect would also A/B the timing.
    ///
    /// This is also what makes the plan safe to compute from the *declared*
    /// chain — it sums bypassed slots too, so the two would disagree if the
    /// container let a bypassed node pass audio through untouched.
    #[test]
    fn bypassing_a_device_does_not_move_the_channel_in_time() {
        const FRAMES: usize = 512;
        let live = Project {
            channels: vec![hit_channel(0, true), hit_channel(1, false)],
            ..full_bank_project()
        };
        let mut bypassed = live.clone();
        bypassed.channels[0].setup.effects[0].bypassed = true;

        let live_master = render_master(&live, FRAMES);
        let bypassed_master = render_master(&bypassed, FRAMES);

        let onset = |samples: &[f32]| samples.iter().position(|sample| sample.abs() > 1.0e-9);
        assert_eq!(
            onset(&live_master),
            onset(&bypassed_master),
            "bypassing the device moved the channel in time"
        );
    }

    /// Compensation must not make the output depend on where the block
    /// boundaries fall. A ring advanced per block rather than per frame would
    /// pass every alignment test above and fail this one.
    #[test]
    fn compensation_renders_the_same_at_any_block_size() {
        const FRAMES: usize = 1_024;
        let project = Project {
            channels: vec![hit_channel(0, true), hit_channel(1, false)],
            ..full_bank_project()
        };
        let render_in_blocks = |block: usize| {
            let mut render = RenderState::from_project(48_000, &project, &[]);
            render.play();
            let mut out = Vec::with_capacity(FRAMES);
            while out.len() < FRAMES {
                render.process_block(block);
                out.extend_from_slice(&render.master().l[..block]);
            }
            out.truncate(FRAMES);
            out
        };
        assert_eq!(render_in_blocks(128), render_in_blocks(256));
        assert_eq!(render_in_blocks(128), render_in_blocks(64));
    }

    /// The offline renderer builds its own `RenderState`, so it is a second
    /// place the plan is compiled -- and the only one a listener never hears
    /// until the file is finished. An export that skipped `install_compensation`
    /// would sound right in the room and arrive misaligned on disk.
    ///
    /// So this renders the aligned pair both ways and compares the file
    /// against the live block path sample for sample. Float32 WAV, because
    /// the comparison has to be exact: PCM24 would quantize the difference
    /// this test exists to find.
    #[test]
    fn an_offline_render_compiles_the_same_compensation_as_a_live_one() {
        const FRAMES: usize = 16_384;
        let latency = mooloop_core::effect::OVERSAMPLER_LATENCY_FRAMES as usize;
        let project = Project {
            channels: vec![hit_channel(0, true), hit_channel(1, false)],
            ..full_bank_project()
        };

        let temp = tempfile::tempdir().expect("a temporary directory");
        let path = temp.path().join("compensated.wav");
        crate::offline::OfflineRenderer::render(
            &project,
            &[],
            48_000,
            &crate::offline::ExportSpec {
                path: path.clone(),
                scope: crate::offline::RenderScope::Pattern { index: 0 },
                tail_seconds: 0.0,
                format: crate::offline::ExportFormat::Wav(
                    crate::offline::WavEncoding::Float32,
                ),
            },
        )
        .expect("the project renders offline");

        // Interleaved stereo; the left channel is what the live tests read.
        let offline: Vec<f32> = hound::WavReader::open(&path)
            .expect("the export is readable")
            .samples::<f32>()
            .map(|sample| sample.expect("a decoded sample"))
            .step_by(2)
            .take(FRAMES)
            .collect();
        assert_eq!(offline.len(), FRAMES, "the export was shorter than expected");

        let mut live_state = RenderState::from_project(48_000, &project, &[]);
        live_state.play();
        let mut live = Vec::with_capacity(FRAMES);
        while live.len() < FRAMES {
            live_state.process_block(512);
            live.extend_from_slice(&live_state.master().l[..512]);
        }
        live.truncate(FRAMES);

        // The comparison is only worth anything against audio, and both hits
        // are in the window: a silent pair of buffers would agree perfectly.
        assert!(
            offline.iter().any(|sample| sample.abs() > 0.01),
            "the offline render is silent, so the comparison proves nothing"
        );
        // And the file itself waited: without the plan the plain channel
        // would arrive `latency` frames before its neighbour, which is the
        // defect the export could carry on its own.
        assert!(
            offline[..latency].iter().all(|sample| *sample == 0.0),
            "the offline render arrived {latency} frames early"
        );

        for (frame, (exported, played)) in offline.iter().zip(live.iter()).enumerate() {
            assert_eq!(
                exported, played,
                "frame {frame}: the export gave {exported}, the live render {played}"
            );
        }
    }

    /// The parameters channel 0's drum synth is running.
    fn drum_params(render: &RenderState) -> mooloop_core::DrumSynthParams {
        match render.strips[0].source.generator_params() {
            GeneratorParams::DrumSynth(params) => params,
            other => panic!("channel 0 is not a drum synth: {:?}", other.kind()),
        }
    }

    /// The whole acceptance case of the v1 drum synth's descriptor table
    /// (`docs/FOCUS.md`, 2026-09-05, closed): a modulation route and an
    /// automation lane both reach the v1 drum synth, which until then was the
    /// one source nothing could move.
    ///
    /// Both halves in one test because they share the resolve pass and the
    /// interesting question is whether a *generator* that had no table until
    /// today is now indistinguishable from one that always had one — nothing
    /// here is drum-specific, which is the point.
    #[test]
    fn a_lane_and_a_route_both_reach_the_v1_drum_synth() {
        let mut channel = ProjectChannel::drum_synth(0, 1);
        // Snare mode on purpose. The kick controls are inert here, so this is
        // also the "audibility gate" case: the route below still resolves,
        // still writes the parameter, and simply is not heard until the
        // device is switched back. That is documented behaviour rather than a
        // special case, and it is what a route onto a bypassed effect does.
        channel
            .setup
            .drum_synth_state_mut()
            .expect("a drum channel has drum state")
            .params
            .mode = mooloop_core::DrumMode::Snare;
        let source = channel
            .setup
            .modulation
            .install(
                0,
                mooloop_core::ModulatorParams::Lfo(mooloop_core::ModLfoParams {
                    rate_hz: 375.0,
                    ..mooloop_core::ModLfoParams::default()
                }),
            )
            .expect("slot 0 accepts a module");
        let punch = ParamAddr {
            scope: EffectTarget::Channel(0),
            owner: ParamOwner::Source,
            param: mooloop_core::DRUM_PARAM_PUNCH,
        };
        assert!(channel
            .setup
            .modulation
            .add_route(mooloop_core::ModRoute::to_slot(
                0,
                punch,
                0.4,
                mooloop_core::ModPolarity::Bipolar,
            ))
            .is_some());
        let _ = source;

        let project = synth_project(channel);
        let mut render = RenderState::from_project(48_000, &project, &[]);

        // The lane drives a different control, so the two are visible apart.
        let start = ParamAddr {
            scope: EffectTarget::Channel(0),
            owner: ParamOwner::Source,
            param: mooloop_core::DRUM_PARAM_KICK_START_HZ,
        };
        render.apply_command(EngineCommand::UpsertAutomationPoint {
            pattern: 0,
            channel: 0,
            target: start,
            point: mooloop_core::AutomationPoint::new(1, 0, 1.0),
        });
        render.play();
        render.process_block(128);

        // The lane reached the device: full-scale on a 20..1000 Hz control.
        assert!(
            (drum_params(&render).kick_start_hz - 1_000.0).abs() < 1.0,
            "the lane did not reach the drum synth: {}",
            drum_params(&render).kick_start_hz
        );

        // The route resolved every control tick and actually moved Punch off
        // the knob it was authored at. Four ticks a block, so an empty list
        // would mean the route never ran rather than that it ran flat.
        let punched: Vec<f32> = render.events[0]
            .iter()
            .filter_map(|event| match event.event {
                Event::ParamValue {
                    id: mooloop_core::DRUM_PARAM_PUNCH,
                    value,
                } => Some(value),
                _ => None,
            })
            .collect();
        assert_eq!(punched.len(), 4, "the route was not resolving: {punched:?}");
        let authored = mooloop_core::DrumSynthParams::default().punch;
        assert!(
            punched.iter().any(|value| (value - authored).abs() > 0.05),
            "the route never moved Punch off {authored}: {punched:?}"
        );

        // Mode is untouched by either, which is what makes the inert-kick
        // case an audibility gate rather than an addressing accident.
        assert_eq!(
            drum_params(&render).mode,
            mooloop_core::DrumMode::Snare
        );

        // Clearing the lane hands the device back its knob rather than
        // leaving it holding the last resolved value.
        render.apply_command(EngineCommand::ClearAutomationLane {
            pattern: 0,
            channel: 0,
            target: start,
        });
        render.process_block(128);
        assert_eq!(
            drum_params(&render).kick_start_hz,
            mooloop_core::DrumSynthParams::default().kick_start_hz
        );
    }

    #[test]
    fn a_generator_parameter_reaches_the_device() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let mut render = RenderState::from_project(48_000, &project, &[]);
        let target = ParamAddr {
            scope: EffectTarget::Channel(0),
            owner: ParamOwner::Source,
            param: mooloop_core::SAMPLER_PARAM_DRIVE,
        };
        render.apply_command(EngineCommand::UpsertAutomationPoint {
            pattern: 0,
            channel: 0,
            target,
            point: mooloop_core::AutomationPoint::new(1, 0, 1.0),
        });
        render.play();
        render.process_block(128);
        assert!(
            (render.strips[0].source.as_sampler().expect("a sampler channel").params().drive - 1.0).abs() < 1e-3,
            "the lane did not reach the sampler: {}",
            render.strips[0].source.as_sampler().expect("a sampler channel").params().drive
        );

        // Clearing it returns the device to the knob rather than leaving it
        // holding the last resolved value.
        render.apply_command(EngineCommand::ClearAutomationLane {
            pattern: 0,
            channel: 0,
            target,
        });
        render.process_block(128);
        assert_eq!(render.strips[0].source.as_sampler().expect("a sampler channel").params().drive, 0.0);
    }

    #[test]
    fn mixed_source_project_renders_all_preallocated_nodes() {
        let mut project = Project {
            channels: vec![
                ProjectChannel::sampler(0, 1),
                ProjectChannel::drum_synth(1, 1),
                ProjectChannel::mono_synth(2, 1),
                ProjectChannel::poly_synth(3, 1),
            ],
            ..full_bank_project()
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
    fn route(render: &mut RenderState, buses: &mut [mooloop_core::BusSetup], bus: u8, output: u8) {
        buses[bus as usize].bus.output = output;
        install_track_graph(render, buses);
    }

    /// Compile a bank and install it the way the pump does: the graph and the
    /// sends that ride on it, as one value.
    fn install_track_graph(render: &mut RenderState, buses: &[mooloop_core::BusSetup]) {
        let graph = compile_bus_graph(buses).expect("test graph should be acyclic");
        //  rather than , because that is
        // what the pump calls. Equivalent here -- the line above already
        // requires the bank to sort -- but a helper that says it installs
        // things the way the pump does should go through what the pump goes
        // through.
        let edges = compensable_send_edges(buses);
        let mut bus_latency = [0u32; MAX_BUSES];
        for (index, setup) in buses.iter().take(MAX_BUSES).enumerate() {
            bus_latency[index] = chain_latency(&setup.effects);
        }
        let plan = compile_latency(&graph, &[], &[], &bus_latency, &edges);
        let specs: Vec<SendSpec> = buses
            .iter()
            .take(MAX_BUSES)
            .enumerate()
            .flat_map(|(index, setup)| setup.sends.iter().map(move |send| (index, send)))
            .zip(0..)
            .map(|((index, send), edge)| SendSpec {
                producer: EffectTarget::Bus(index as u8),
                target: send.target,
                tap: send.tap,
                enabled: send.enabled,
                level: send.level,
                delay: plan.send(edge),
            })
            .collect();
        let _ = render.apply_structural(StructuralCommand::SetTrackGraph {
            graph,
            sends: Box::new(SendBank::new(&specs, 48_000)),
        });
    }

    fn rendered_energy(project: &Project, configure: impl FnOnce(&mut RenderState)) -> f32 {
        let mut render = RenderState::from_project(48_000, project, &[]);
        configure(&mut render);
        // The mix as configured, not the ramp into it: see `settle_mixer`.
        render.settle_mixer();
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
                ..mooloop_core::FilterParams::default()
            }),
            48_000,
        )
    }

    fn default_effect(kind: mooloop_core::EffectKind) -> Box<dyn AudioNode + Send> {
        build_effect(kind.default_params(), 48_000)
    }

    fn install_effect(
        target: EffectTarget,
        slot: u8,
        node: Box<dyn AudioNode + Send>,
    ) -> StructuralCommand {
        // A slot number the model would never mint, so a test device cannot
        // be mistaken for one the project loaded.
        install_effect_as(target, slot, mooloop_core::DeviceId(900 + slot as u32), node)
    }

    fn install_effect_as(
        target: EffectTarget,
        slot: u8,
        device: mooloop_core::DeviceId,
        node: Box<dyn AudioNode + Send>,
    ) -> StructuralCommand {
        let align = IntegerDelay::new(node.dry_path_latency_frames()).map(Box::new);
        StructuralCommand::InstallEffect {
            target,
            slot,
            kind: mooloop_core::EffectKind::Filter,
            resource_key: None,
            node,
            align,
            analyzer: Box::new(SpectrumAnalyzer::new()),
            state: Box::new(EffectSlot::for_device(device)),
        }
    }

    // --- Sends -------------------------------------------------------------

    /// A kick on channel 0 feeding track 1, with the bank routed
    /// everything-to-master. What every send test below is measured against.
    fn send_project() -> Project {
        let mut project = synth_project(ProjectChannel::sampler(0, 1));
        project.channels[0].setup.channel.bus = 1;
        project
    }

    /// Energy in a track's own buffer after a block. A track's buffer still
    /// holds its output when the block ends -- the next one empties it -- so
    /// this reads what the track produced without having to unpick it from
    /// the master's sum.
    fn track_energy(render: &RenderState, track: usize, frames: usize) -> f32 {
        render.buses[track].bus.l[..frames]
            .iter()
            .map(|s| s * s)
            .sum()
    }

    /// Render `project` for one block and report what reached `track`.
    fn wet_energy(project: &Project, track: usize, configure: impl FnOnce(&mut RenderState)) -> f32 {
        let mut render = RenderState::from_project(48_000, project, &[]);
        configure(&mut render);
        // The mix as configured, not the ramp into it: see `settle_mixer`.
        render.settle_mixer();
        render.play();
        render.process_block(1024);
        track_energy(&render, track, 1024)
    }

    /// The ordinary case, and the one the default song is: a track sends to
    /// another and pulling its fader down takes the send with it.
    #[test]
    fn a_post_fader_send_follows_the_fader() {
        let mut project = send_project();
        project.buses[1].sends.push(mooloop_core::AuxSend::new(2));

        let unity = wet_energy(&project, 2, |_| {});
        let halved = wet_energy(&project, 2, |render| {
            render.apply_command(EngineCommand::SetBusVolume { bus: 1, volume: 0.5 });
        });

        assert!(unity > 0.0, "the send carried nothing at all");
        let ratio = halved / unity;
        assert!(
            (0.2..0.3).contains(&ratio),
            "half the fader should be a quarter of the energy, got {ratio}"
        );
    }

    /// The documented different result. A pre-fader send is after the chain
    /// and before the fader, so the fader moves the main output and leaves
    /// the send where it was.
    #[test]
    fn a_pre_fader_send_holds_its_level_while_the_fader_moves() {
        let mut project = send_project();
        let mut send = mooloop_core::AuxSend::new(2);
        send.tap = mooloop_core::SendTap::PreFader;
        project.buses[1].sends.push(send);

        let unity = wet_energy(&project, 2, |_| {});
        let faded = wet_energy(&project, 2, |render| {
            render.apply_command(EngineCommand::SetBusVolume { bus: 1, volume: 0.25 });
        });

        assert!(unity > 0.0, "the send carried nothing at all");
        let ratio = faded / unity;
        assert!(
            (0.99..1.01).contains(&ratio),
            "a pre-fader send should not have moved, got {ratio}"
        );
    }

    /// Mute silences a track's sends, pre-fader ones included. A desk's mute
    /// is "this strip contributes nothing anywhere", not "its main output is
    /// off and its sends carry on".
    #[test]
    fn mute_silences_a_tracks_sends() {
        let mut project = send_project();
        let mut send = mooloop_core::AuxSend::new(2);
        send.tap = mooloop_core::SendTap::PreFader;
        project.buses[1].sends.push(send);

        let heard = wet_energy(&project, 2, |_| {});
        let muted = wet_energy(&project, 2, |render| {
            render.apply_command(EngineCommand::SetBusMuted {
                bus: 1,
                muted: true,
            });
        });

        assert!(heard > 0.0);
        assert_eq!(muted, 0.0, "a muted track still fed its send");
    }

    /// A send at zero and a send switched off are both silent, and they are
    /// not the same statement: the disabled one keeps its level, so turning
    /// it back on returns it to where it was.
    #[test]
    fn a_disabled_send_is_silent_without_forgetting_its_level() {
        let mut project = send_project();
        let mut send = mooloop_core::AuxSend::new(2);
        send.enabled = false;
        send.level = 0.75;
        project.buses[1].sends.push(send);

        assert_eq!(wet_energy(&project, 2, |_| {}), 0.0);
        let reenabled = wet_energy(&project, 2, |render| {
            render.apply_command(EngineCommand::SetSendEnabled {
                producer: EffectTarget::Bus(1),
                index: 0,
                enabled: true,
            });
        });
        assert!(reenabled > 0.0, "the send did not come back");
    }

    /// A send is linear even when the track sending it is analog-summed.
    ///
    /// The two are about different things and it is easy to conflate them:
    /// analog sum is what a strip does to its own *output*, on its way into
    /// the summing point it feeds. A send is a feed into a *different*
    /// strip's input, which encodes on its own switch or not at all. So
    /// switching the source's analog sum on must not change a byte of what
    /// its send carries.
    #[test]
    fn a_send_is_linear_whatever_the_track_sending_it_does() {
        let mut project = send_project();
        project.buses[1].sends.push(mooloop_core::AuxSend::new(2));

        let linear = wet_energy(&project, 2, |_| {});
        let summed = wet_energy(&project, 2, |render| {
            render.apply_command(EngineCommand::SetTrackConsole {
                bus: 1,
                enabled: true,
            });
        });

        assert!(linear > 0.0);
        assert_eq!(
            linear, summed,
            "analog sum reached the send, which is a strip's output decision"
        );
    }

    /// **The alignment null test.** A latency-bearing device on the track a
    /// send feeds moves the summing point both paths meet at, so the dry path
    /// owes the difference.
    ///
    /// The send is *disabled*, which isolates timing from level: it is still
    /// in the graph and still moves the arrival, but contributes no audio. So
    /// the master's output must be the un-sent render delayed by exactly the
    /// return's latency, sample for sample -- not merely "about right".
    ///
    /// At three block sizes, because a compensation that happened to be a
    /// whole number of blocks would pass at one and fail at the others.
    #[test]
    fn a_dry_path_waits_for_a_latency_bearing_return() {
        let plain = send_project();
        let mut sent = plain.clone();
        let mut send = mooloop_core::AuxSend::new(2);
        send.enabled = false;
        sent.buses[1].sends.push(send);
        // The one device in the catalogue that declares a latency.
        sent.buses[2].push_effect(mooloop_core::EffectSlotState {
            id: mooloop_core::DeviceId::default(),
            params: mooloop_core::EffectParams::Drive(mooloop_core::DriveParams::default()),
            bypassed: false,
            wet_dry: 1.0,
            input_trim: 1.0,
            output_trim: 1.0,
        });
        let latency = mooloop_core::EffectKind::Drive.latency_frames() as usize;
        assert!(latency > 0, "the alignment case needs a device that declares one");

        for frames in [128, 256, 1024] {
            let reference = master_run(&plain, frames);
            let delayed = master_run(&sent, frames);
            for frame in latency..frames {
                let expected = reference[frame - latency];
                let actual = delayed[frame];
                assert!(
                    (expected - actual).abs() < 1e-6,
                    "at {frames} frames, sample {frame}: expected {expected}, got {actual}"
                );
            }
        }
    }

    /// One block of the master, for the alignment test above.
    fn master_run(project: &Project, frames: usize) -> Vec<f32> {
        let mut render = RenderState::from_project(48_000, project, &[]);
        render.play();
        render.process_block(frames);
        render.master().l[..frames].to_vec()
    }

    /// A level change ramps rather than steps. A send was the first place at
    /// strip level this was true; the faders, pans, mutes and polarity
    /// followed with MOO-107, and `continuity_tests` holds them to it through
    /// a whole render.
    #[test]
    fn a_send_level_is_smoothed() {
        let mut level = Smoothed::new(0.0, STRIP_GAIN_SMOOTH_S, 48_000);
        level.set_target(1.0);
        let mut bus = StereoBus::with_capacity(512);
        for frame in 0..512 {
            bus.l[frame] = 1.0;
            bus.r[frame] = 1.0;
        }
        apply_smoothed_gain(&mut level, &mut bus, 512);

        let biggest_step = bus.l[..512]
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            biggest_step < 0.01,
            "a level change stepped by {biggest_step}, which is a click"
        );
        assert!(bus.l[0] < 0.02, "it did not start from where it was");
        assert!(bus.l[511] > 0.85, "it never arrived");
    }

    /// The rule `docs/CAPACITY_POLICY.md` asks for, stated as a test: an
    /// unused feature reserves nothing. No sends means no bank, no rings, and
    /// none of the three scratch buffers.
    #[test]
    fn a_project_with_no_sends_allocates_nothing() {
        let bank = SendBank::new(&[], 48_000);
        assert!(bank.is_empty());
        assert!(bank.starts.is_empty());
        assert!(bank.scratch.is_none());
        assert_eq!(bank.range(EffectTarget::Bus(1)), 0..0);
    }

    /// A bank groups its sends by producer, and a producer's own run keeps
    /// the order it was authored in -- which is what makes an index into that
    /// run a stable address for a level change.
    #[test]
    fn a_bank_groups_sends_by_producer() {
        let spec = |producer, target| SendSpec {
            producer,
            target,
            tap: SendTap::PostFader,
            enabled: true,
            level: 1.0,
            delay: 0,
        };
        let bank = SendBank::new(
            &[
                spec(EffectTarget::Bus(3), 5),
                spec(EffectTarget::Channel(1), 4),
                spec(EffectTarget::Bus(3), 6),
            ],
            48_000,
        );
        let bus_three = bank.range(EffectTarget::Bus(3));
        assert_eq!(bus_three.len(), 2);
        assert_eq!(bank.sends[bus_three.clone()][0].target, 5);
        assert_eq!(bank.sends[bus_three][1].target, 6);
        assert_eq!(bank.range(EffectTarget::Channel(1)).len(), 1);
        // A producer with no sends gets an empty run where its run would
        // start, which is what a prefix-sum table gives and is what the
        // emission loop wants: a zero-length slice, not a special case.
        assert!(bank.range(EffectTarget::Bus(4)).is_empty());
    }

    #[test]
    fn installed_filter_changes_channel_output() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let dry = rendered_energy(&project, |_| {});
        let filtered = rendered_energy(&project, |render| {
            let _ = render.apply_structural(install_effect(
                EffectTarget::Channel(0),
                0,
                muffling_filter(),
            ));
        });
        assert!(dry > 0.0, "reference render was silent");
        assert!(
            filtered < dry * 0.5,
            "100 Hz low-pass should eat most of a kick: dry {dry}, filtered {filtered}"
        );
    }

    /// A plugin device whose plugin is not running -- not yet swapped in, or
    /// missing -- holds `build_effect`'s placeholder, and the song plays
    /// through it unchanged (`docs/plans/plugin-hosting/00-status.md`,
    /// "Failure"). Parameter events aimed at it are dropped, not applied to
    /// anything else.
    #[test]
    fn a_plugin_device_with_no_plugin_passes_the_signal_through() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let dry = rendered_energy(&project, |_| {});
        assert!(dry > 0.0, "reference render was silent");
        let params = mooloop_core::EffectParams::Plugin(mooloop_core::PluginSlotId(0));
        assert_eq!(params.kind().latency_frames(), 0);
        let wet = rendered_energy(&project, |render| {
            let _ = render.apply_structural(install_effect(
                EffectTarget::Channel(0),
                0,
                build_effect(params, 48_000),
            ));
        });
        assert!(
            (wet - dry).abs() < dry * 1.0e-5,
            "a placeholder must be transparent: dry {dry}, wet {wet}"
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
                mooloop_core::EffectKind::Eq => {
                    params.set(
                        mooloop_core::eq_band_param(1, mooloop_core::EQ_BAND_GAIN),
                        18.0,
                    );
                }
                mooloop_core::EffectKind::Modulation => {
                    params.set(mooloop_core::MODULATION_PARAM_MODE, 2.0);
                    params.set(mooloop_core::MODULATION_PARAM_DEPTH, 0.85);
                }
                mooloop_core::EffectKind::Filter => {
                    params.set(mooloop_core::FILTER_PARAM_CUTOFF_HZ, 100.0);
                }
                mooloop_core::EffectKind::Drive => {
                    params.set(mooloop_core::DRIVE_PARAM_DRIVE, 64.0);
                }
                mooloop_core::EffectKind::Preamp => {
                    // `Moo` is the default and is the identity by design, so
                    // a voicing has to be chosen or this kind would be the
                    // one that proves nothing.
                    params.set(
                        mooloop_core::PREAMP_PARAM_VOICING,
                        mooloop_core::PreampVoicing::Iron.to_index() as f32,
                    );
                    params.set(mooloop_core::PREAMP_PARAM_DRIVE_DB, 24.0);
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
                mooloop_core::EffectKind::Reverb => {
                    // Entirely wet, and its shortest delay line plus the
                    // default pre-delay outlast this render window, so the
                    // output here is silence — clearly different from the
                    // nonzero dry reference.
                }
                mooloop_core::EffectKind::Plate => {
                    // Also entirely wet, and its shortest comb tap is longer
                    // than this render window, so the output is silence here
                    // — clearly different from the nonzero dry reference.
                }
                mooloop_core::EffectKind::Buffer => {
                    // Follow is deliberately transparent until an atomic
                    // buffer event arrives.
                }
                mooloop_core::EffectKind::Chain | mooloop_core::EffectKind::Layer => {
                    // A container is transparent by construction and stays
                    // that way: its mix belongs to the chain host, not to the
                    // node in the slot. `docs/plans/containers/03` gives the
                    // host that dry path, and this arm is where a container
                    // that started processing audio itself would be caught.
                }
                mooloop_core::EffectKind::Plugin => {
                    unreachable!("a plugin is not in EffectKind::ALL; see the test below")
                }
            }

            let wet = rendered_energy(&project, |render| {
                let _ = render.apply_structural(install_effect(
                    EffectTarget::Channel(0),
                    0,
                    build_effect(params, 48_000),
                ));
            });
            // Two shapes are transparent on purpose, for two different
            // reasons: Follow passes audio through until an atomic buffer
            // event arrives, and a container of either kind has no signal
            // path of its own. Equal-power leaks a cos(pi/2) ~ 6e-8 of the
            // aligned dry alongside either, inaudible but not bit-exact.
            //
            // A *layer* here is empty -- nothing is wrapped -- so it has no
            // branch to split into and is its input, the same as an empty
            // chain.
            if kind == mooloop_core::EffectKind::Buffer || kind.is_container() {
                assert!(
                    (wet - dry).abs() < dry * 1.0e-5,
                    "{} must be transparent: dry {dry}, wet {wet}",
                    kind.label()
                );
            } else {
                assert!(
                    (wet - dry).abs() > dry * 0.01,
                    "{} left the signal unchanged: dry {dry}, wet {wet}",
                    kind.label()
                );
            }
        }
    }

    /// A channel routed to a bus must reach the master *through* that bus, so
    /// an effect inserted on the bus shapes everything feeding it.
    #[test]
    fn a_bus_effect_processes_every_channel_feeding_it() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let dry = rendered_energy(&project, |render| {
            render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 3 });
        });
        assert!(dry > 0.0, "routing through a bus must not lose the signal");

        let filtered = rendered_energy(&project, |render| {
            render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 3 });
            let _ =
                render.apply_structural(install_effect(EffectTarget::Bus(3), 0, muffling_filter()));
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
            render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 2 });
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
            render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 1 });
            render.apply_command(EngineCommand::SetBusVolume {
                bus: 1,
                volume: 1.0,
            });
        });
        let halved = rendered_energy(&project, |render| {
            render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 1 });
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
            let mut buses = full_bank();
            render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 5 });
            route(render, &mut buses, 5, 2);
        });
        assert!(chained > 0.0, "chained buses must still reach the master");

        let filtered = rendered_energy(&project, |render| {
            let mut buses = full_bank();
            render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 5 });
            route(render, &mut buses, 5, 2);
            let _ =
                render.apply_structural(install_effect(EffectTarget::Bus(2), 0, muffling_filter()));
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
            let mut buses = full_bank();
            render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 2 });
            route(render, &mut buses, 2, 9);
        });
        assert!(chained > 0.0, "an uphill route must still reach the master");

        let filtered = rendered_energy(&project, |render| {
            let mut buses = full_bank();
            render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 2 });
            route(render, &mut buses, 2, 9);
            let _ =
                render.apply_structural(install_effect(EffectTarget::Bus(9), 0, muffling_filter()));
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
            let mut buses = full_bank();
            render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 1 });
            buses[1].bus.output = 4;
            buses[4].bus.output = 11;
            buses[11].bus.output = MASTER_BUS;
            install_track_graph(render, &buses);
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
            clean.buses = full_bank();
            rendered_energy(&clean, |_| {})
        };
        let repaired = rendered_energy(&project, |_| {});
        let ratio = repaired / dry;
        assert!(
            (0.9..1.1).contains(&ratio),
            "a cyclic file should still play: {repaired} vs {dry}"
        );
    }

    #[test]
    fn malformed_and_short_bus_banks_still_reach_the_master() {
        let dry = rendered_energy(&synth_project(ProjectChannel::sampler(0, 1)), |_| {});

        let mut malformed = synth_project(ProjectChannel::sampler(0, 1));
        malformed.channels[0].setup.channel.bus = 3;
        malformed.buses[3].bus.output = MAX_BUSES as u8;
        let repaired = rendered_energy(&malformed, |_| {});
        assert!(
            (0.9..1.1).contains(&(repaired / dry)),
            "an invalid destination must be repaired to master"
        );

        let mut short = synth_project(ProjectChannel::sampler(0, 1));
        short.channels[0].setup.channel.bus = 9;
        short.buses.truncate(2);
        let padded = rendered_energy(&short, |_| {});
        assert!(
            (0.9..1.1).contains(&(padded / dry)),
            "missing bus definitions must compile as default buses"
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
        render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 6 });
        render.play();
        render.process_block(1024);

        assert!(meters.take(6).0 > 0.001, "the routed bus should meter");
        assert!(meters.take(MASTER_BUS as usize).0 > 0.001);
        assert_eq!(meters.take(5), (0.0, 0.0), "an unused bus must read silent");
    }

    #[test]
    fn device_meters_follow_the_host_signal_flow() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let meters = DeviceMeters::new();
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.attach_device_meters(meters.clone());
        let _ = render.apply_structural(install_effect(
            EffectTarget::Channel(0),
            0,
            muffling_filter(),
        ));
        render.play();
        render.process_block(1024);

        let (source_in, source_out) = meters.take(0, 0);
        assert_eq!(source_in, (0.0, 0.0), "sources have no device input");
        assert!(source_out.0 > 0.001, "source output should meter");
        let (effect_in, effect_out) = meters.take(0, 1);
        assert!(effect_in.0 > 0.001, "effect sees the source output");
        assert!(
            effect_out.0 < effect_in.0,
            "filter output should differ from its input"
        );
    }

    #[test]
    fn bus_effect_slots_meter_like_channel_slots() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let meters = DeviceMeters::new();
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.attach_device_meters(meters.clone());
        render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 3 });
        let _ = render.apply_structural(install_effect(EffectTarget::Bus(3), 0, muffling_filter()));
        render.play();
        render.process_block(1024);

        let (effect_in, effect_out) = meters.take(MAX_CHANNELS + 3, 1);
        assert!(
            effect_in.0 > 0.001,
            "a bus effect sees what its bus received"
        );
        assert!(
            effect_out.0 < effect_in.0,
            "filter output should differ from its input"
        );
        let (channel_in, _) = meters.take(0, 1);
        assert_eq!(
            channel_in,
            (0.0, 0.0),
            "the channel has no effect in slot 1"
        );
    }

    /// Test node: an honest pure delay. What the container's dry path must be
    /// aligned against whenever a node reports latency.
    struct LatentDelay {
        left: std::collections::VecDeque<f32>,
        right: std::collections::VecDeque<f32>,
    }

    impl LatentDelay {
        fn new(frames: usize) -> Self {
            Self {
                left: std::iter::repeat_n(0.0, frames).collect(),
                right: std::iter::repeat_n(0.0, frames).collect(),
            }
        }
    }

    impl AudioNode for LatentDelay {
        fn latency_frames(&self) -> u32 {
            self.left.len() as u32
        }

        fn process(
            &mut self,
            ctx: &ProcessContext,
            bus: &mut StereoBus,
            _events_in: &EventList,
            _events_out: Option<&mut EventList>,
        ) {
            for frame in 0..ctx.frames {
                self.left.push_back(bus.l[frame]);
                self.right.push_back(bus.r[frame]);
                bus.l[frame] = self.left.pop_front().unwrap_or(0.0);
                bus.r[frame] = self.right.pop_front().unwrap_or(0.0);
            }
        }
    }

    #[test]
    fn effect_chain_bound_tracks_sparse_slots() {
        let mut chain = EffectChain::new();
        assert_eq!(chain.bound, 0);

        for slot in [2, 5] {
            let displaced = chain.install(
                slot,
                mooloop_core::EffectKind::Delay,
                None,
                Box::new(LatentDelay::new(1)),
                None,
                Box::new(SpectrumAnalyzer::new()),
                Box::new(EffectSlot::new()),
            );
            assert!(displaced.is_empty());
        }
        assert_eq!(chain.bound, 6);

        // Moving 5 to 1 rotates 1..=5 right: the delay in 5 lands on 1 and
        // the one in 2 shifts to 3.
        assert!(chain.move_slot(5, 1));
        assert_eq!(chain.bound, 4);
        assert!(chain.nodes[1].is_some());
        assert!(chain.nodes[2].is_none());
        assert!(chain.nodes[3].is_some());

        assert!(chain.remove(3).node.is_some());
        assert_eq!(chain.bound, 2);
        assert!(chain.remove(1).node.is_some());
        assert_eq!(chain.bound, 0);
    }

    #[test]
    fn the_container_aligns_its_dry_path_to_node_latency() {
        const LATENCY: usize = 15;
        let mut chain = EffectChain::new();
        let node = Box::new(LatentDelay::new(LATENCY));
        let align = IntegerDelay::new(node.latency_frames()).map(Box::new);
        let displaced = chain.install(
            0,
            mooloop_core::EffectKind::Delay,
            None,
            node,
            align,
            Box::new(SpectrumAnalyzer::new()),
            Box::new(EffectSlot::new()),
        );
        assert!(displaced.is_empty());
        chain.slot_mut(0).unwrap().wet_dry = 0.5;
        // The blend as configured, not the ramp into it (MOO-108).
        chain.slot_mut(0).unwrap().settle_ramps();

        let context = ProcessContext {
            sample_rate: 48_000,
            frames: 64,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        };
        let mut bus = StereoBus::with_capacity(MAX_BLOCK_SIZE);
        bus.l[0] = 1.0;
        bus.r[0] = 1.0;
        chain.process(&context, &mut bus, EffectTarget::Channel(0), None, None, None, true);

        assert!(
            bus.l[..LATENCY].iter().all(|s| *s == 0.0),
            "a latent node must not pass dry audio early: {:?}",
            &bus.l[..LATENCY]
        );
        assert!(
            (bus.l[LATENCY] - core::f32::consts::SQRT_2).abs() < 1e-5,
            "aligned dry + wet recombine to equal-power unity (sqrt(2) for a \
             correlated path at 50%), got {}",
            bus.l[LATENCY]
        );
        assert!(
            bus.l[LATENCY + 1..].iter().all(|s| *s == 0.0),
            "no second, misaligned copy of the impulse may follow"
        );
    }

    /// The prepared-resource guard is generic, but Buffer is now its only
    /// user: reverb used to key on an IR fingerprint and no longer allocates
    /// anything a parameter change could invalidate.
    #[test]
    fn prepared_resource_replacement_refuses_a_stale_slot_key() {
        let mut chain = EffectChain::new();
        let initial = Box::new(LatentDelay::new(1));
        let initial_align = IntegerDelay::new(initial.latency_frames()).map(Box::new);
        let displaced = chain.install(
            0,
            mooloop_core::EffectKind::Buffer,
            Some(10),
            initial,
            initial_align,
            Box::new(SpectrumAnalyzer::new()),
            Box::new(EffectSlot::new()),
        );
        assert!(displaced.is_empty());

        let stale = Box::new(LatentDelay::new(2));
        let stale_align = IntegerDelay::new(stale.latency_frames()).map(Box::new);
        let rejected = chain.replace_if_kind(
            0,
            mooloop_core::EffectKind::Buffer,
            9,
            11,
            stale,
            stale_align,
        );
        assert!(rejected.node.is_some());
        assert_eq!(chain.slot(0).unwrap().resource_key, Some(10));

        let current = Box::new(LatentDelay::new(2));
        let current_align = IntegerDelay::new(current.latency_frames()).map(Box::new);
        let replaced = chain.replace_if_kind(
            0,
            mooloop_core::EffectKind::Buffer,
            10,
            11,
            current,
            current_align,
        );
        assert!(replaced.node.is_some());
        assert_eq!(chain.slot(0).unwrap().resource_key, Some(11));
    }

    /// **Muting the master silences the master**, and not only its meters.
    ///
    /// A non-master track is silenced by `mix_into` not running. The master
    /// has no `mix_into` -- it *is* the output, copied straight to the ports
    /// and to an export -- so nothing applied its mute: the button lit, both
    /// of its meters read silent (the published peak is zeroed a few lines
    /// up), and the audio carried on at full level. An export made in that
    /// state was full level too. `CURRENT.md` already said a muted bus
    /// "contributes no audio and meters as silent"; only the second half was
    /// true.
    ///
    /// Measured through the master buffer rather than the meter, because the
    /// meter was the half that already worked.
    #[test]
    fn muting_the_master_silences_it_and_not_just_its_meter() {
        let project = synth_project(ProjectChannel::sampler(0, 1));

        let peak_of = |muted: bool| {
            let mut render = RenderState::from_project(48_000, &project, &[]);
            if muted {
                render.apply_command(EngineCommand::SetBusMuted {
                    bus: mooloop_core::MASTER_BUS,
                    muted: true,
                });
                // Muted from the start rather than faded into: the fade is
                // `continuity_tests`' question, this one is whether a muted
                // master is silent at all.
                render.settle_mixer();
            }
            render.play();
            render.process_block(1024);
            let master = render.master();
            master.l[..1024]
                .iter()
                .fold(0.0f32, |peak, s| peak.max(s.abs()))
        };

        let heard = peak_of(false);
        assert!(heard > 0.001, "the song has to be making sound: {heard}");
        assert_eq!(peak_of(true), 0.0, "a muted master is silent");
    }

    #[test]
    fn a_muted_bus_meters_silent() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let meters = BusMeters::new();
        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.attach_meters(meters.clone());
        render.apply_command(EngineCommand::SetChannelBus { channel: 0, bus: 6 });
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
            let _ = render.apply_structural(install_effect(
                EffectTarget::Bus(mooloop_core::MASTER_BUS),
                0,
                muffling_filter(),
            ));
        });
        assert!(filtered < dry * 0.5, "dry {dry}, filtered {filtered}");
    }

    #[test]
    fn generic_host_mix_and_input_output_trims_wrap_every_effect() {
        let project = synth_project(ProjectChannel::sampler(0, 1));
        let dry = rendered_energy(&project, |_| {});
        let host_dry = rendered_energy(&project, |render| {
            let _ = render.apply_structural(install_effect(
                EffectTarget::Channel(0),
                0,
                muffling_filter(),
            ));
            render.apply_command(EngineCommand::SetEffectWetDry {
                target: EffectTarget::Channel(0),
                slot: 0,
                wet_dry: 0.0,
            });
        });
        assert!(
            (host_dry / dry - 1.0).abs() < 0.1,
            "wet=0 must pass dry signal"
        );
        let trimmed = rendered_energy(&project, |render| {
            let _ = render.apply_structural(install_effect(
                EffectTarget::Channel(0),
                0,
                muffling_filter(),
            ));
            render.apply_command(EngineCommand::SetEffectWetDry {
                target: EffectTarget::Channel(0),
                slot: 0,
                wet_dry: 0.0,
            });
            render.apply_command(EngineCommand::SetEffectOutputTrim {
                target: EffectTarget::Channel(0),
                slot: 0,
                output_trim: 0.5,
            });
        });
        assert!(
            (trimmed / dry - 0.25).abs() < 0.08,
            "trim is amplitude, energy scales squared"
        );
        let input_trimmed = rendered_energy(&project, |render| {
            let _ = render.apply_structural(install_effect(
                EffectTarget::Channel(0),
                0,
                muffling_filter(),
            ));
            render.apply_command(EngineCommand::SetEffectWetDry {
                target: EffectTarget::Channel(0),
                slot: 0,
                wet_dry: 0.0,
            });
            render.apply_command(EngineCommand::SetEffectInputTrim {
                target: EffectTarget::Channel(0),
                slot: 0,
                input_trim: 0.5,
            });
        });
        assert!(
            (input_trimmed / dry - 0.25).abs() < 0.08,
            "input trim must feed the hosted effect at reduced amplitude"
        );
    }

    #[test]
    fn effect_param_bypass_and_reorder_plumbing() {
        let project = synth_project(ProjectChannel::sampler(0, 1));

        // A wide-open filter passes (nearly) everything; closing it via a
        // queued ParamValue event must change the output.
        let open = rendered_energy(&project, |render| {
            let _ = render.apply_structural(install_effect(
                EffectTarget::Channel(0),
                0,
                default_effect(mooloop_core::EffectKind::Filter),
            ));
        });
        // The cutoff now ramps (see
        // docs/plans/archive/share-dsp-primitives/01-smooth-effect-parameters.md)
        // rather than snapping, so a queued change doesn't fully close the
        // filter within the same 1024-frame block it's queued in. Render
        // one block to let the ramp settle, discard it, then measure the
        // next — this still asserts the param change lands, just not
        // instantaneously.
        let mut render = RenderState::from_project(48_000, &project, &[]);
        let _ = render.apply_structural(install_effect(
            EffectTarget::Channel(0),
            0,
            default_effect(mooloop_core::EffectKind::Filter),
        ));
        render.apply_command(EngineCommand::SetEffectParam {
            target: EffectTarget::Channel(0),
            slot: 0,
            id: mooloop_core::FILTER_PARAM_CUTOFF_HZ,
            value: 100.0,
        });
        render.play();
        render.process_block(1024);
        render.process_block(1024);
        let closed: f32 = render.master().l[..1024].iter().map(|s| s * s).sum();
        assert!(closed < open * 0.5, "open {open}, closed {closed}");

        // Bypass restores the dry sound.
        let bypassed = rendered_energy(&project, |render| {
            let _ = render.apply_structural(install_effect(
                EffectTarget::Channel(0),
                0,
                muffling_filter(),
            ));
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

        // Moving the filter into an empty slot keeps it in the chain.
        let mut render = RenderState::from_project(48_000, &project, &[]);
        let _ = render.apply_structural(install_effect(
            EffectTarget::Channel(0),
            0,
            muffling_filter(),
        ));
        render.apply_command(EngineCommand::MoveEffect {
            target: EffectTarget::Channel(0),
            from: 0,
            to: 3,
        });
        render.play();
        render.process_block(1024);
        let moved = render.master().l[..1024].iter().map(|s| s * s).sum::<f32>();
        assert!(moved < dry * 0.5, "filter should still muffle after the move");

        // Removing the slot reclaims the node instead of dropping it here.
        let reclaimed = render.apply_structural(StructuralCommand::RemoveEffect {
            target: EffectTarget::Channel(0),
            slot: 3,
        });
        assert!(reclaimed.is_some());
    }

    /// A four-device chain with a long tail in it, built the way the engine
    /// builds one so the aligners and slot state match a real project's.
    fn tailed_chain() -> EffectChain {
        let mut chain = EffectChain::new();
        let kinds = [
            mooloop_core::EffectKind::Reverb,
            mooloop_core::EffectKind::Delay,
            mooloop_core::EffectKind::Drive,
            mooloop_core::EffectKind::Eq,
        ];
        for (slot, kind) in kinds.into_iter().enumerate() {
            let node = build_effect(kind.default_params(), 48_000);
            let align = IntegerDelay::new(node.dry_path_latency_frames()).map(Box::new);
            let displaced = chain.install(
                slot,
                kind,
                None,
                node,
                align,
                Box::new(SpectrumAnalyzer::new()),
                Box::new(EffectSlot::new()),
            );
            assert!(displaced.is_empty());
        }
        chain
    }

    fn chain_context(frames: usize) -> ProcessContext {
        ProcessContext {
            sample_rate: 48_000,
            frames,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        }
    }

    /// Play `blocks` blocks through a fresh chain, with a burst of noise at
    /// the start and another after `wake_at`, and return everything it
    /// produced. `skip_idle` is the only thing that differs between the two
    /// runs the tests below compare.
    fn render_chain(blocks: usize, wake_at: usize, skip_idle: bool) -> Vec<f32> {
        const FRAMES: usize = 512;
        let mut chain = tailed_chain();
        let mut bus = StereoBus::with_capacity(MAX_BLOCK_SIZE);
        let mut out = Vec::with_capacity(blocks * FRAMES);
        let mut noise = 0x1234_5678u32;
        for block in 0..blocks {
            bus.clear(FRAMES);
            if block == 0 || block == wake_at {
                for index in 0..FRAMES {
                    noise ^= noise << 13;
                    noise ^= noise >> 17;
                    noise ^= noise << 5;
                    let sample = ((noise >> 8) as f32 / 8_388_608.0 - 1.0) * 0.5;
                    bus.l[index] = sample;
                    bus.r[index] = sample;
                }
            }
            chain.process(
                &chain_context(FRAMES),
                &mut bus,
                EffectTarget::Channel(0),
                None,
                None,
                None,
                skip_idle,
            );
            out.extend_from_slice(&bus.l[..FRAMES]);
        }
        out
    }

    /// The plainest form of the claim: a chain that is handed nothing renders
    /// nothing, sample for sample, whether or not it is allowed to sleep.
    #[test]
    fn a_chain_fed_silence_renders_identically_with_and_without_skipping() {
        let mut chain = tailed_chain();
        let mut bus = StereoBus::with_capacity(MAX_BLOCK_SIZE);
        // Long enough for the reverb in the chain to run out its declared
        // tail, which is what the chain waits on before any of it sleeps.
        for _ in 0..(10 * 48_000 / 512) {
            bus.clear(512);
            chain.process(
                &chain_context(512),
                &mut bus,
                EffectTarget::Channel(0),
                None,
                None,
                None,
                true,
            );
            assert!(
                bus.l[..512].iter().all(|sample| *sample == 0.0),
                "a sleeping chain must pass exact zeros, not nearly-zeros"
            );
        }
        assert!(
            chain.is_at_rest(),
            "a chain handed nothing for ten seconds should have gone to sleep;              if it has not, this test proves nothing"
        );
    }

    /// The one way this mechanism can be *heard*: a device that under-reports
    /// its tail gets cut off mid-decay. So render the same reverb-and-delay
    /// chain twice — once allowed to sleep and once forced to run every block
    /// — and hold the two against each other for the whole tail, across the
    /// sleep, and through the note that wakes it again.
    ///
    /// The tolerance is `SILENCE_PEAK` with room for the gain of whatever
    /// stands after the device that fell asleep. A slot going to sleep with
    /// its input sitting on the threshold hands the rest of the chain a
    /// signal up to `SILENCE_PEAK` different from what it would have had, and
    /// an EQ band may put 24 dB on that. Sixteen times the threshold is
    /// -116 dBFS, still under one step of a 20-bit render. A truncated tail
    /// would miss by five orders of magnitude, which is the distance this
    /// test is really measuring.
    #[test]
    fn skipping_never_changes_what_a_tail_sounds_like() {
        const BLOCKS: usize = 12 * 48_000 / 512;
        const WAKE_AT: usize = 10 * 48_000 / 512;
        let slept = render_chain(BLOCKS, WAKE_AT, true);
        let ran = render_chain(BLOCKS, WAKE_AT, false);
        assert_eq!(slept.len(), ran.len());

        let mut worst = 0.0f32;
        let mut worst_at = 0;
        for (index, (a, b)) in slept.iter().zip(&ran).enumerate() {
            let difference = (a - b).abs();
            if difference > worst {
                worst = difference;
                worst_at = index;
            }
        }
        assert!(
            worst <= SILENCE_PEAK * 16.0,
            "letting the chain sleep changed its output by {worst} at frame              {worst_at} ({:.3} s in)",
            worst_at as f32 / 48_000.0
        );

        // The premise: there has to be a tail there to truncate, and the
        // chain has to actually wake up for the second burst.
        let tail: f32 = ran[48_000..2 * 48_000]
            .iter()
            .fold(0.0f32, |peak, s| peak.max(s.abs()));
        assert!(tail > 1.0e-3, "no audible tail a second in: {tail}");
        // Half a second after the second burst rather than the block it
        // landed in: the reverb runs at full wet with a 12 ms pre-delay, so
        // the first block after a burst is genuinely almost empty.
        let woken: f32 = slept[WAKE_AT * 512..(WAKE_AT * 512 + 24_000).min(slept.len())]
            .iter()
            .fold(0.0f32, |peak, s| peak.max(s.abs()));
        assert!(
            woken > 1.0e-2,
            "the chain did not wake for the second burst: {woken}"
        );
    }

    /// Render `blocks` blocks of a one-buffer-insert project, optionally
    /// firing one event as a command before block `trigger_block`, and return
    /// the concatenated master output.
    fn render_with_buffer(
        project: &Project,
        blocks: usize,
        frames: usize,
        trigger_block: usize,
        trigger: Option<mooloop_core::BufferEvent>,
        telemetry: Option<&Arc<DeviceTelemetry>>,
        device: Box<dyn AudioNode + Send>,
    ) -> Vec<f32> {
        let mut render = RenderState::from_project(48_000, project, &[]);
        if let Some(telemetry) = telemetry {
            render.attach_device_telemetry(telemetry.clone());
        }
        let _ = render.apply_structural(install_effect(EffectTarget::Channel(0), 0, device));
        render.play();
        let mut out = Vec::with_capacity(blocks * frames);
        for block in 0..blocks {
            if block == trigger_block {
                if let Some(event) = trigger {
                    render.apply_command(EngineCommand::TriggerBuffer {
                        target: EffectTarget::Channel(0),
                        slot: 0,
                        event,
                    });
                }
            }
            render.process_block(frames);
            out.extend_from_slice(&render.master().l[..frames]);
        }
        out
    }

    fn jump_event(offset_beats: f32, rate: f32) -> mooloop_core::BufferEvent {
        mooloop_core::BufferEvent {
            offset_beats,
            rate,
            window_beats: None,
            repeat: None,
            duration: mooloop_core::BufferDuration::UntilNextEvent,
            // Zero, so the divergence a test observes is the edit itself and
            // not a fade that would blur the first frames after it.
            crossfade_ms: 0.0,
        }
    }

    /// The whole command path a debug trigger takes: an `EngineCommand`
    /// carrying one event tuple, reaching an inserted buffer device, and
    /// changing what the master renders. Follow is deliberately transparent,
    /// so nothing short of a fired event proves this plumbing works.
    #[test]
    fn triggered_buffer_event_alters_rendered_output() {
        const BLOCKS: usize = 8;
        const FRAMES: usize = 1024;
        const TRIGGER_BLOCK: usize = 4;
        let project = synth_project(ProjectChannel::sampler(0, 1));

        let follow = render_with_buffer(
            &project,
            BLOCKS,
            FRAMES,
            TRIGGER_BLOCK,
            None,
            None,
            default_effect(mooloop_core::EffectKind::Buffer),
        );
        let jumped = render_with_buffer(
            &project,
            BLOCKS,
            FRAMES,
            TRIGGER_BLOCK,
            Some(jump_event(-0.05, 1.0)),
            None,
            default_effect(mooloop_core::EffectKind::Buffer),
        );

        let split = TRIGGER_BLOCK * FRAMES;
        assert_eq!(
            follow[..split],
            jumped[..split],
            "audio before the trigger must be untouched"
        );
        assert!(
            follow[split..].iter().any(|sample| *sample != 0.0),
            "reference tail was silent, so divergence would prove nothing"
        );
        assert!(
            follow[split..] != jumped[split..],
            "TriggerBuffer never reached the inserted device"
        );
    }

    /// A head wrapping the ring surfaces as device telemetry -- the only trace
    /// a seam leaves, since the audio thread cannot log.
    ///
    /// The number is the same one this test asserted when it was about a
    /// *collision*: a reverse head overtaken by its writer and forced back to
    /// live. That failure is gone with the turntable model -- a mapped event
    /// holds the ring still and wraps instead -- and the count now reports
    /// laps. The arithmetic happens to land on one either way: the head
    /// starts 480 frames behind now on a 4 096-frame ring, reaches the oldest
    /// sample 3 616 frames later, and wraps once before the render ends. The
    /// ring is deliberately tiny so that happens inside a short render.
    #[test]
    fn a_buffer_seam_surfaces_as_device_telemetry() {
        const BLOCKS: usize = 8;
        const FRAMES: usize = 1024;
        const TRIGGER_BLOCK: usize = 4;
        const RING: usize = 4_096;
        let project = synth_project(ProjectChannel::sampler(0, 1));

        let telemetry = DeviceTelemetry::new();
        let _ = render_with_buffer(
            &project,
            BLOCKS,
            FRAMES,
            TRIGGER_BLOCK,
            Some(jump_event(-0.02, -1.0)),
            Some(&telemetry),
            Box::new(mooloop_dsp::BufferDevice::with_capacity(RING)),
        );
        // Stage 0 is the source; the insert in slot 0 publishes as stage 1.
        assert_eq!(telemetry.read_buffer_collisions(0, 1), 1);

        // Following has no head, so it has nothing to wrap.
        let quiet = DeviceTelemetry::new();
        let _ = render_with_buffer(
            &project,
            BLOCKS,
            FRAMES,
            TRIGGER_BLOCK,
            None,
            Some(&quiet),
            Box::new(mooloop_dsp::BufferDevice::with_capacity(RING)),
        );
        assert_eq!(quiet.read_buffer_collisions(0, 1), 0);
    }

    /// The whole control path: a note-on carrying a mapped tuple reaches the
    /// insert and changes the render, and the matching note-off ends it.
    /// Note says what and how long; velocity says how hard.
    #[test]
    fn mapped_midi_notes_drive_a_buffer_insert() {
        use mooloop_core::midi::{BufferMidiMap, BufferNoteMapping};
        use mooloop_core::{MidiKind, MidiMessage};

        const FRAMES: usize = 1024;
        let project = synth_project(ProjectChannel::sampler(0, 1));

        let render_with_midi = |messages_at: Option<usize>| -> Vec<f32> {
            let mut render = RenderState::from_project(48_000, &project, &[]);
            let _ = render.apply_structural(install_effect(
                EffectTarget::Channel(0),
                0,
                default_effect(mooloop_core::EffectKind::Buffer),
            ));
            let mut map = BufferMidiMap::new(EffectTarget::Channel(0), 0);
            map.notes[0] = Some(BufferNoteMapping {
                note: 60,
                event: mooloop_core::BufferEvent {
                    offset_beats: -0.05,
                    ..mooloop_core::BufferEvent::live()
                },
            });
            render.set_buffer_midi(Some(Box::new(map)));
            render.play();

            let mut out = Vec::new();
            for block in 0..8 {
                if Some(block) == messages_at {
                    render.apply_midi(&[MidiMessage {
                        port: mooloop_core::MidiPortId::FIRST,
                        offset: 0,
                        channel: 0,
                        kind: MidiKind::NoteOn {
                            note: 60,
                            velocity: 127,
                        },
                    }]);
                }
                render.process_block(FRAMES);
                out.extend_from_slice(&render.master().l[..FRAMES]);
            }
            out
        };

        let quiet = render_with_midi(None);
        let played = render_with_midi(Some(4));
        let split = 4 * FRAMES;
        assert_eq!(quiet[..split], played[..split]);
        assert!(
            quiet[split..].iter().any(|sample| *sample != 0.0),
            "reference tail was silent"
        );
        assert!(
            quiet[split..] != played[split..],
            "a mapped note never reached the insert"
        );
    }

    /// An unmapped key must not release an edit it never started, and a
    /// mapped one must. Both halves matter: a keyboard is full of notes this
    /// map does not own.
    #[test]
    fn only_a_mapped_note_releases_the_edit() {
        use mooloop_core::midi::{BufferMidiMap, BufferNoteMapping};
        use mooloop_core::{MidiKind, MidiMessage};

        let project = synth_project(ProjectChannel::sampler(0, 1));
        let note_off = |note| MidiMessage {
            port: mooloop_core::MidiPortId::FIRST,
            offset: 0,
            channel: 0,
            kind: MidiKind::NoteOff { note },
        };

        let mut map = BufferMidiMap::new(EffectTarget::Channel(0), 0);
        map.notes[0] = Some(BufferNoteMapping {
            note: 60,
            event: mooloop_core::BufferEvent::live(),
        });
        assert!(map.note_event(60, 100).is_some());
        assert!(map.note_event(61, 100).is_none());

        // Velocity drives the crossfade: hard is abrupt, soft is declicked.
        let hard = map.note_event(60, 127).unwrap();
        let soft = map.note_event(60, 1).unwrap();
        assert_eq!(hard.crossfade_ms, 0.0);
        assert!(soft.crossfade_ms > hard.crossfade_ms);
        assert_eq!(hard.duration, mooloop_core::BufferDuration::Gate);

        let mut render = RenderState::from_project(48_000, &project, &[]);
        render.set_buffer_midi(Some(Box::new(map)));
        render.play();
        // Neither of these should panic or route anywhere unexpected; the
        // unmapped note is simply ignored.
        render.apply_midi(&[note_off(61), note_off(60)]);
        render.process_block(256);
    }

    /// A relative CC drives the platter rather than re-firing an event, and
    /// a centred message means no movement at all.
    #[test]
    fn a_relative_cc_scrubs_without_refiring() {
        use mooloop_core::midi::{BufferCcMapping, BufferCcTarget, BufferMidiMap};
        use mooloop_core::{MidiKind, MidiMessage, RelativeEncoding};

        let project = synth_project(ProjectChannel::sampler(0, 1));
        let mut map = BufferMidiMap::new(EffectTarget::Channel(0), 0);
        map.controls[0] = Some(BufferCcMapping {
            controller: 21,
            target: BufferCcTarget::Scrub {
                encoding: RelativeEncoding::BinaryOffset,
            },
        });

        let cc = |value| MidiMessage {
            port: mooloop_core::MidiPortId::FIRST,
            offset: 0,
            channel: 0,
            kind: MidiKind::ControlChange {
                controller: 21,
                value,
            },
        };

        let mut render = RenderState::from_project(48_000, &project, &[]);
        let _ = render.apply_structural(install_effect(
            EffectTarget::Channel(0),
            0,
            default_effect(mooloop_core::EffectKind::Buffer),
        ));
        render.set_buffer_midi(Some(Box::new(map)));
        render.play();
        for _ in 0..4 {
            render.process_block(1024);
        }

        // 64 is the rest position of a binary-offset encoder.
        render.apply_midi(&[cc(64)]);
        render.process_block(1024);

        // A real turn detaches the head and moves it back in time.
        render.apply_midi(&[cc(32)]);
        render.process_block(1024);
        assert!(
            render.scrub_frames_per_tick() > 0.0,
            "scrub must resolve against tempo"
        );
    }

    /// Play `blocks` blocks and answer the master's peak over the last one.
    fn peak_after(render: &mut RenderState, blocks: usize) -> f32 {
        use crate::render_test_support::peak_of;

        for _ in 0..blocks {
            render.process_once_block(1_024);
        }
        peak_of(&render.master().l[..1_024])
    }


    /// The point of the whole class: a deferred command does **not** land at
    /// the top of the block that drains it. It is held until the transport
    /// reaches the edge, which is somewhere inside a later block, and the
    /// block it lands in is cut so that what runs after it is scheduled
    /// against what it changed.
    ///
    /// Asserted against the block *before* the bar as well as the one that
    /// crosses it. Only checking that the tempo eventually changes would pass
    /// just as well if the command had been applied immediately, which is the
    /// behaviour this exists to rule out.
    #[test]
    fn a_deferred_command_waits_for_its_edge() {
        use crate::render_test_support::SAMPLE_RATE;

        let project = held_note_project();
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        render.play();
        let bar = f64::from(mooloop_core::TICKS_PER_BAR);

        render.defer_command(MusicalEdge::Bar, EngineCommand::SetTempo(140.0));
        assert_eq!(
            render.transport.bpm, 120.0,
            "deferring must not apply the command"
        );

        let mut blocks = 0;
        while render.transport.position_ticks + 1e-9 < bar {
            assert_eq!(
                render.transport.bpm, 120.0,
                "the tempo changed at block {blocks}, before the bar line at \
                 {bar}; the command was applied early"
            );
            render.process_once_block(256);
            blocks += 1;
            assert!(blocks < 10_000, "the bar line was never reached");
        }
        // One further block, because an edge that falls exactly on a block
        // boundary produces no cut: there is nothing to divide, and the next
        // block's first span *opens* on the edge. That is the same instant,
        // reached by the other route through `apply_deferred_reached`.
        if render.transport.bpm == 120.0 {
            render.process_once_block(256);
        }
        assert_eq!(
            render.transport.bpm, 140.0,
            "the command did not land at the bar line it was waiting for"
        );
    }

    /// A stopped transport reaches no edge, so a command deferred against one
    /// would wait forever. It is applied immediately instead -- the gesture
    /// still happens, it simply has nothing to wait for.
    #[test]
    fn a_deferred_command_sent_while_stopped_lands_immediately() {
        use crate::render_test_support::SAMPLE_RATE;

        let project = held_note_project();
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        assert!(!render.transport.playing);

        render.defer_command(MusicalEdge::Bar, EngineCommand::SetTempo(140.0));
        assert_eq!(
            render.transport.bpm, 140.0,
            "a stopped transport has no edge to wait for"
        );
        assert!(
            render.deferred.iter().all(Option::is_none),
            "nothing should have been parked"
        );
    }

    /// One slot per kind: a second deferred command of the same kind replaces
    /// the first rather than queueing behind it. That is what re-aiming a
    /// gesture means to a player, and it bounds the state without an
    /// overflow policy. Different kinds do not displace each other.
    #[test]
    fn a_second_deferred_command_of_one_kind_replaces_the_first() {
        use crate::render_test_support::SAMPLE_RATE;

        let project = held_note_project();
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        render.play();

        render.defer_command(MusicalEdge::Bar, EngineCommand::SetTempo(140.0));
        render.defer_command(MusicalEdge::Bar, EngineCommand::SetTempo(90.0));
        assert_eq!(
            render.deferred.iter().flatten().count(),
            1,
            "the second tempo should have replaced the first, not queued"
        );

        render.defer_command(MusicalEdge::Bar, EngineCommand::SetSwing(60));
        assert_eq!(
            render.deferred.iter().flatten().count(),
            2,
            "a different kind must take its own slot"
        );

        let bar = f64::from(mooloop_core::TICKS_PER_BAR);
        while render.transport.position_ticks + 1e-9 < bar {
            render.process_once_block(256);
        }
        if render.transport.bpm == 120.0 {
            render.process_once_block(256);
        }
        assert_eq!(
            render.transport.bpm, 90.0,
            "the surviving tempo is the one sent last"
        );
    }

    /// Stopping, seeking and changing playback mode each invalidate the
    /// position an edge was resolved against. Left parked, the target either
    /// never arrives or arrives somewhere the player never aimed at, so it is
    /// cancelled rather than re-resolved.
    #[test]
    fn a_pending_command_is_cancelled_by_a_transport_discontinuity() {
        use crate::render_test_support::SAMPLE_RATE;

        let project = held_note_project();
        for (label, interruption) in [
            ("stop", EngineCommand::Stop),
            ("pause", EngineCommand::Pause),
            ("seek", EngineCommand::Seek { tick: 4_096.0 }),
            (
                "mode",
                EngineCommand::SetPlaybackMode(mooloop_core::PlaybackMode::Song),
            ),
        ] {
            let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
            render.play();
            render.defer_command(MusicalEdge::Bar, EngineCommand::SetTempo(140.0));
            assert_eq!(render.deferred.iter().flatten().count(), 1, "{label}: setup");

            render.apply_command(interruption);
            assert!(
                render.deferred.iter().all(Option::is_none),
                "{label} left a command waiting for an edge that no longer means \
                 what it meant when it was resolved"
            );
        }
    }

    /// Carried across an install, not re-sent. The control thread cannot know
    /// a project was installed between its send and the edge, so a pending
    /// command left behind in the outgoing renderer would simply never land.
    /// The lesson `incremental-structure/` paid for, applied to a new piece
    /// of performance state.
    #[test]
    fn a_pending_command_survives_a_project_install() {
        use crate::render_test_support::SAMPLE_RATE;

        let project = held_note_project();
        let mut outgoing = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        outgoing.play();
        outgoing.defer_command(MusicalEdge::Bar, EngineCommand::SetTempo(140.0));

        let mut incoming = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        incoming.adopt_performance_state(&outgoing);

        assert_eq!(
            incoming.deferred.iter().flatten().count(),
            1,
            "the install dropped a command that was waiting for an edge"
        );
        let bar = f64::from(mooloop_core::TICKS_PER_BAR);
        while incoming.transport.position_ticks + 1e-9 < bar {
            incoming.process_once_block(256);
        }
        if incoming.transport.bpm == 120.0 {
            incoming.process_once_block(256);
        }
        assert_eq!(
            incoming.transport.bpm, 140.0,
            "the carried command still has to land at its edge"
        );
    }

    /// `PatternEnd` in Song mode resolves to the next bar rather than to
    /// nothing. Song mode schedules from placements and never crosses a
    /// pattern end, so the edge as named does not exist there; falling back
    /// to the nearest edge that does keeps a deferred command from being
    /// stranded by the mode it was sent in.
    #[test]
    fn a_pattern_end_deferred_in_song_mode_falls_back_to_the_bar() {
        use crate::render_test_support::SAMPLE_RATE;

        let project = held_note_project();
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        render.apply_command(EngineCommand::SetPlaybackMode(
            mooloop_core::PlaybackMode::Song,
        ));
        render.play();

        render.defer_command(MusicalEdge::PatternEnd, EngineCommand::SetTempo(140.0));
        let pending = render
            .deferred
            .iter()
            .flatten()
            .next()
            .expect("the command should be waiting");
        assert!(
            (pending.target_tick - f64::from(mooloop_core::TICKS_PER_BAR)).abs() < 1e-9,
            "expected the next bar, got {}",
            pending.target_tick
        );
    }


    /// A target past the loop end is never reached: the fold turns the
    /// playhead back before it, every lap, and the command would wait for a
    /// tick the transport has stopped travelling towards. A fold is a
    /// discontinuity like a seek and cancels for the same reason -- this is
    /// the case that makes it a hang rather than a surprise if it does not.
    #[test]
    fn a_fold_cancels_a_command_waiting_past_the_loop_end() {
        use crate::render_test_support::SAMPLE_RATE;

        let project = held_note_project();
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        render.apply_command(EngineCommand::SetPlaybackMode(
            mooloop_core::PlaybackMode::Song,
        ));
        // A loop shorter than a bar, so the bar line the command waits for
        // sits beyond the end of it.
        let bar = mooloop_core::TICKS_PER_BAR;
        render.apply_command(EngineCommand::SetLoopRange(mooloop_core::LoopRange {
            start_tick: 0,
            end_tick: bar / 2,
            enabled: true,
        }));
        render.play();
        render.defer_command(MusicalEdge::Bar, EngineCommand::SetTempo(140.0));
        assert_eq!(
            render.deferred.iter().flatten().count(),
            1,
            "the command should be waiting for the bar line"
        );

        let mut blocks = 0;
        while render.deferred.iter().flatten().count() > 0 {
            render.process_block(256);
            blocks += 1;
            assert!(
                blocks < 10_000,
                "the fold never cancelled it; the command is waiting for a tick \
                 inside a loop that never reaches it"
            );
        }
        assert_eq!(
            render.transport.bpm, 120.0,
            "cancelling must not apply the command on the way out"
        );
    }


    /// The wiring and the vocabulary, which is what this layer owns: that a
    /// discontinuity reaches an installed node at all, and that it arrives
    /// **named**. The `mooloop-dsp` tests prove what each device does when
    /// told; this proves it is told, and told which.
    ///
    /// A spy rather than a measurement of the audio. A seek chokes the voices
    /// as well as reaching the nodes, so an output-level assertion would be
    /// measuring two mechanisms at once and could not say which one moved.
    #[test]
    fn a_discontinuity_reaches_installed_nodes_and_says_which_it_is() {
        use crate::render_test_support::SAMPLE_RATE;
        use std::sync::{Arc, Mutex};

        #[derive(Default)]
        struct Spy {
            heard: Arc<Mutex<Vec<Discontinuity>>>,
        }

        impl AudioNode for Spy {
            fn on_discontinuity(&mut self, kind: Discontinuity) {
                self.heard.lock().expect("spy lock").push(kind);
            }

            fn process(
                &mut self,
                _context: &ProcessContext,
                _bus: &mut StereoBus,
                _events_in: &EventList,
                _events_out: Option<&mut EventList>,
            ) {
            }
        }

        fn spied(render: &mut RenderState) -> Arc<Mutex<Vec<Discontinuity>>> {
            let heard = Arc::<Mutex<Vec<Discontinuity>>>::default();
            render.strips[0].effects.nodes[0] = Some(Box::new(Spy {
                heard: Arc::clone(&heard),
            }));
            heard
        }

        fn render_with_a_slot() -> RenderState {
            let mut project = held_note_project();
            project.channels[0]
                .setup
                .push_effect(mooloop_core::EffectSlotState::of_kind(
                    mooloop_core::EffectKind::Eq,
                ))
                .expect("pushed");
            let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
            assert!(
                render.strips[0].effects.nodes[0].is_some(),
                "the slot has to exist for the spy to take it"
            );
            render.play();
            render
        }

        let mut render = render_with_a_slot();
        let heard = spied(&mut render);
        render.apply_command(EngineCommand::Seek { tick: 4_096.0 });
        render.process_once_block(1_024);
        assert_eq!(
            heard.lock().expect("spy lock").as_slice(),
            [Discontinuity::Seek],
            "a seek has to reach the node, exactly once, named as a seek"
        );

        let mut render = render_with_a_slot();
        let heard = spied(&mut render);
        render.apply_command(EngineCommand::SetCurrentPattern(1));
        render.process_once_block(1_024);
        // Exactly one, and not followed by a seek. Until 2026-09-22 a switch
        // set `seeked` as well -- the note-off is stranded either way -- so
        // the block after it said `Seek` too, and every delay, reverb and
        // plate flushed on a pattern switch (MOO-59). This asserted with
        // `contains` then, and a comment that "the node hears both".
        assert_eq!(
            heard.lock().expect("spy lock").as_slice(),
            [Discontinuity::ProgramChange],
            "a pattern switch has to reach the node named as a program change, \
             and as nothing else"
        );

        let mut render = render_with_a_slot();
        let heard = spied(&mut render);
        render.apply_command(EngineCommand::Stop);
        assert_eq!(
            heard.lock().expect("spy lock").as_slice(),
            [Discontinuity::Stop],
            "stopping has to reach the node, named as a stop"
        );

        let mut render = render_with_a_slot();
        let heard = spied(&mut render);
        render.process_once_block(1_024);
        assert!(
            heard.lock().expect("spy lock").is_empty(),
            "an ordinary block is not a discontinuity and must say nothing"
        );
    }

    /// **A loop fold reaches the nodes as a fold, not as a seek.** Both are
    /// discontinuous in time; only one of them is discontinuous in the
    /// music, and a device that wants to keep its tail across the fold could
    /// not say so while the two arrived under one name
    /// (`reports/fable-2026-09-21.md`, finding 2). Nothing about the sound
    /// changes here: every node that cleared on a fold still clears on one.
    #[test]
    fn a_fold_reaches_installed_nodes_as_a_fold() {
        use crate::render_test_support::SAMPLE_RATE;
        use std::sync::{Arc, Mutex};

        #[derive(Default)]
        struct Spy {
            heard: Arc<Mutex<Vec<Discontinuity>>>,
        }

        impl AudioNode for Spy {
            fn on_discontinuity(&mut self, kind: Discontinuity) {
                self.heard.lock().expect("spy lock").push(kind);
            }

            fn process(
                &mut self,
                _context: &ProcessContext,
                _bus: &mut StereoBus,
                _events_in: &EventList,
                _events_out: Option<&mut EventList>,
            ) {
            }
        }

        let mut project = held_note_project();
        project.channels[0]
            .setup
            .push_effect(mooloop_core::EffectSlotState::of_kind(
                mooloop_core::EffectKind::Eq,
            ))
            .expect("pushed");
        // A fold is a Song-mode loop range turning the playhead back.
        // Pattern mode wraps inside the sequencer without the transport
        // jumping, so it never reaches here at all.
        project.playback_mode = PlaybackMode::Song;
        project.playlist = vec![mooloop_core::PatternPlacement {
            pattern: 0,
            start_tick: 0,
        }];
        let end = mooloop_core::TICKS_PER_BAR;
        project.loop_range = mooloop_core::LoopRange {
            start_tick: 0,
            end_tick: end,
            enabled: true,
        };
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        let heard = Arc::<Mutex<Vec<Discontinuity>>>::default();
        render.strips[0].effects.nodes[0] = Some(Box::new(Spy {
            heard: Arc::clone(&heard),
        }));
        render.play();

        let length = end;
        render.apply_command(EngineCommand::Seek {
            tick: length as f64 - 1.0,
        });
        // `process_block`, not `process_once_block`: the loop belongs to the
        // realtime path, and the offline one walks the arrangement once.
        render.process_block(1_024);
        assert_eq!(
            heard.lock().expect("spy lock").as_slice(),
            [Discontinuity::Seek],
            "the seek that positions this test has to be the seek"
        );
        heard.lock().expect("spy lock").clear();

        // Blocks from a tick before the loop end: the transport turns back
        // inside one of them, which is the only way a fold happens.
        // The seek landed a tick from the loop end, so the block above
        // already folded -- and said `Seek`, which is the right word for a
        // block that did both. Play round the loop once more for a fold with
        // nothing else in it: 384 ticks at 120 bpm is a hundred-odd blocks.
        for _ in 0..200 {
            render.process_block(1_024);
            if !heard.lock().expect("spy lock").is_empty() {
                break;
            }
        }
        assert_eq!(
            heard.lock().expect("spy lock").as_slice(),
            [Discontinuity::LoopFold],
            "the fold reached the node as something other than a fold"
        );
    }

    /// One short, dry-sounding note into a fully wet delay whose echo lands
    /// half a second later, with no feedback: whatever is heard in the gap
    /// after the note is the delay line and nothing else. Two patterns, and
    /// only pattern 0 carries the note, as in [`held_note_project`].
    fn echo_project(note_start_tick: u32) -> Project {
        let params = mooloop_core::MlP8Params {
            attack: 0.0,
            decay: 0.0,
            sustain: 1.0,
            release: 0.0,
            ..mooloop_core::MlP8Params::default()
        };
        let mut channel = ProjectChannel::mlp8_with_params(0, 2, params);
        channel.setup.channel.volume = 1.0;
        channel.notes[0].push(NoteEvent::new(1, note_start_tick, 6, 60, 127));
        let mut delay = mooloop_core::EffectSlotState::of_kind(mooloop_core::EffectKind::Delay);
        delay.params = mooloop_core::EffectParams::Delay(mooloop_core::DelayParams {
            time_ms: ECHO_MS,
            feedback: 0.0,
            mix: 1.0,
            tone: 1.0,
            ..mooloop_core::DelayParams::default()
        });
        channel.setup.push_effect(delay).expect("pushed");

        Project {
            channels: vec![channel],
            pattern_lengths: vec![DEFAULT_STEPS, DEFAULT_STEPS],
            ..Project::default()
        }
    }

    const ECHO_MS: f32 = 500.0;

    /// **A delay tail crosses the loop point** -- Adam's ruling on MOO-59,
    /// heard rather than spied on. The note sits a quarter of a second before
    /// the loop end and its echo is due a quarter of a second after it, so
    /// the only way to hear the echo is for the delay line to survive the
    /// fold. Until 2026-09-22 a fold told every node `LoopFold`, the delay
    /// cleared on it as it does on a seek, and the gap after the fold was
    /// silent.
    #[test]
    fn a_delay_tail_crosses_the_loop_point() {
        use crate::render_test_support::{peak_of, SAMPLE_RATE};

        const BLOCK: usize = 512;
        let end = mooloop_core::TICKS_PER_BAR;
        // 48 ticks is a quarter of a second at 120 bpm.
        let mut project = echo_project(end - 48);
        project.playback_mode = PlaybackMode::Song;
        project.playlist = vec![mooloop_core::PatternPlacement {
            pattern: 0,
            start_tick: 0,
        }];
        project.loop_range = mooloop_core::LoopRange {
            start_tick: 0,
            end_tick: end,
            enabled: true,
        };
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        render.play();

        // Up to the block the transport turns back in.
        let mut folded = false;
        for _ in 0..(4 * SAMPLE_RATE as usize / BLOCK) {
            let before = render.transport.position_ticks;
            render.process_block(BLOCK);
            if render.transport.position_ticks < before {
                folded = true;
                break;
            }
        }
        assert!(folded, "the transport never reached the loop end");

        // Half a second after the fold: the echo is due at a quarter, and the
        // next lap's note is more than a second away.
        let mut after = 0.0f32;
        for _ in 0..(SAMPLE_RATE as usize / 2 / BLOCK) {
            render.process_block(BLOCK);
            after = after.max(peak_of(&render.master().l[..BLOCK]));
        }
        assert!(
            after > 0.01,
            "the echo of a note played before the loop end never arrived after \
             it: the fold emptied the delay line ({after})"
        );
    }

    /// **A pattern switch is only a program change**, heard. The note's echo
    /// is in flight in the delay line when the player switches to an empty
    /// pattern; the switch releases the voice, and must leave the echo to
    /// arrive. Until 2026-09-22 the switch set the seek flag as well, the
    /// block after it told every node `Seek`, and the delay flushed (MOO-59).
    #[test]
    fn a_pattern_switch_is_only_a_program_change() {
        use crate::render_test_support::{peak_of, SAMPLE_RATE};

        const BLOCK: usize = 512;
        let project = echo_project(0);
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        render.play();

        // A tenth of a second: the note has played and ended, and its echo is
        // four tenths away.
        for _ in 0..(SAMPLE_RATE as usize / 10 / BLOCK) {
            render.process_block(BLOCK);
        }
        render.apply_command(EngineCommand::SetCurrentPattern(1));

        let mut after = 0.0f32;
        for _ in 0..(SAMPLE_RATE as usize / 2 / BLOCK) {
            render.process_block(BLOCK);
            after = after.max(peak_of(&render.master().l[..BLOCK]));
        }
        assert!(
            after > 0.01,
            "the echo of a note played before the switch never arrived: the \
             switch flushed the delay line as though it were a seek ({after})"
        );
    }

    /// A channel sounding one note for far longer than any of the tests
    /// below run, on a flat, fully sustaining ML-P8. Nothing about the
    /// material can end it, so anything that ends it is the engine's doing,
    /// which is what every test here is measuring.
    ///
    /// Two patterns, and only pattern 0 carries the note: a switch to
    /// pattern 1 therefore schedules nothing, and the note's own note-off
    /// goes with it.
    fn held_note_project() -> Project {
        let params = mooloop_core::MlP8Params {
            attack: 0.0,
            decay: 0.0,
            sustain: 1.0,
            release: 0.0,
            ..mooloop_core::MlP8Params::default()
        };
        let mut channel = ProjectChannel::mlp8_with_params(0, 2, params);
        channel.setup.channel.volume = 1.0;
        channel.notes[0].push(NoteEvent::new(1, 0, 4608, 60, 127));

        Project {
            channels: vec![channel],
            pattern_lengths: vec![DEFAULT_STEPS, DEFAULT_STEPS],
            ..Project::default()
        }
    }

    /// Switching the current pattern in pattern mode, with the transport
    /// running, is a discontinuity in the *note source* rather than in the
    /// position, and it owes the same release a seek owes. `schedule_pattern`
    /// reads
    /// `patterns[current]` fresh every block, so the moment `current` moves,
    /// the note-off of anything still sounding is in a list nobody schedules
    /// any more. Nothing else let it go: pattern mode takes no loop fold
    /// (`loop_range` is `None` outside song mode, so no span is ever
    /// `jumped`), and the generators only release on `!ctx.playing`, which is
    /// false here -- the transport is still running. The voice held at full
    /// sustain until Stop.
    #[test]
    fn switching_pattern_while_playing_releases_the_sounding_voices() {
        use crate::render_test_support::SAMPLE_RATE;

        let project = held_note_project();
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        render.play();

        let sounding = peak_after(&mut render, 8);
        assert!(
            sounding > 0.01,
            "the note has to be sounding before the switch, got {sounding}"
        );

        render.apply_command(EngineCommand::SetCurrentPattern(1));
        let after = peak_after(&mut render, 24);
        assert!(
            after < sounding * 0.01,
            "the voice must be released by the switch; it was {after} against \
             {sounding} before, which is the note still held"
        );
    }

    /// The bug Adam reported on 2026-09-20: *"when i change what pattern im
    /// looking at, the audio glitches... its really only an issue in song
    /// mode."*
    ///
    /// In song mode the selection is not what is playing. Every read of
    /// `Sequencer::current` that reaches playback is inside a
    /// `PlaybackMode::Pattern` arm -- `schedule_pattern`, `schedule_once`,
    /// `automation_lane_at`, `has_automation_at` -- and song mode schedules
    /// from playlist placements instead. What `current` still reaches is
    /// `recording_tick`, so the command has to keep arriving; what it must not
    /// do is cost a voice.
    ///
    /// Held to the *same* level rather than merely to audible: a choke on an
    /// ML-P8 is `release_all`, and at `release: 0.0` that is a fast fade
    /// rather than an instant one, so "still making a sound one block later"
    /// would pass straight through the bug.
    #[test]
    fn switching_the_viewed_pattern_in_song_mode_releases_nothing() {
        use crate::render_test_support::SAMPLE_RATE;

        let project = held_note_project();
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        render.apply_command(EngineCommand::SetPlaylistPlacement {
            pattern: 0,
            start_tick: 0,
            on: true,
        });
        render.apply_command(EngineCommand::SetPlaybackMode(PlaybackMode::Song));
        render.play();

        let sounding = peak_after(&mut render, 8);
        assert!(
            sounding > 0.01,
            "the placement has to be sounding before the switch, got {sounding}"
        );

        render.apply_command(EngineCommand::SetCurrentPattern(1));
        let after = peak_after(&mut render, 24);
        assert!(
            after > sounding * 0.9,
            "looking at another pattern released a voice song mode is still \
             playing: {after} against {sounding} before"
        );
    }

    /// With the transport stopped there is no sequenced voice whose note-off
    /// could have been stranded. What may be sounding is an audition or a held
    /// key, and that belongs to the user rather than to the pattern being left.
    ///
    /// This contradicts the reasoning `release_all_voices` is called under for
    /// a *seek* -- "a seek while stopped still owes the release, for auditioned
    /// notes if nothing else" -- and deliberately so. After a seek the playhead
    /// has moved and a note heard at the old position is stale; after this the
    /// playhead has not moved and the note is a key still held down.
    #[test]
    fn switching_the_viewed_pattern_while_stopped_leaves_an_audition_alone() {
        use crate::render_test_support::SAMPLE_RATE;

        let project = held_note_project();
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        assert!(!render.transport.playing);

        render.apply_command(EngineCommand::TriggerChannelNote {
            channel: 0,
            note: 60,
            velocity: 127,
        });
        let sounding = peak_after(&mut render, 4);
        assert!(
            sounding > 0.01,
            "the audition has to be sounding before the switch, got {sounding}"
        );

        render.apply_command(EngineCommand::SetCurrentPattern(1));
        let after = peak_after(&mut render, 24);
        assert!(
            after > sounding * 0.9,
            "looking at another pattern released an audition the transport \
             was not playing: {after} against {sounding} before"
        );
    }

    /// Re-selecting the pattern already current changes nothing that is
    /// scheduled, so it owes nothing. It is not a rare gesture: the jump menu
    /// carries the current pattern as an entry, and the stepper bounces off
    /// its bounds onto the same index.
    #[test]
    fn reselecting_the_pattern_already_current_releases_nothing() {
        use crate::render_test_support::SAMPLE_RATE;

        let project = held_note_project();
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        render.play();

        let sounding = peak_after(&mut render, 8);
        assert!(sounding > 0.01, "nothing was sounding, got {sounding}");

        render.apply_command(EngineCommand::SetCurrentPattern(0));
        let after = peak_after(&mut render, 24);
        assert!(
            after > sounding * 0.9,
            "re-selecting the current pattern released a voice: {after} \
             against {sounding} before"
        );
    }

    /// A selection the sequencer refuses must not cost anything either.
    /// `set_current_pattern` ignores an index past the active prefix, so the
    /// pattern being scheduled is the one that was already scheduled.
    #[test]
    fn an_out_of_range_pattern_selection_releases_nothing() {
        use crate::render_test_support::SAMPLE_RATE;

        let project = held_note_project();
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        render.play();

        let sounding = peak_after(&mut render, 8);
        assert!(sounding > 0.01, "nothing was sounding, got {sounding}");

        // The project has two patterns.
        render.apply_command(EngineCommand::SetCurrentPattern(9));
        let after = peak_after(&mut render, 24);
        assert!(
            after > sounding * 0.9,
            "a refused selection released a voice: {after} against \
             {sounding} before"
        );
    }

    /// Removing or shortening a note while it sounds releases it (MOO-99).
    /// Before this the `UpsertNote` and `RemoveNote` arms released nothing,
    /// and the note-off that would have ended the voice was gone with the
    /// note, so it held at full sustain until Stop.
    #[test]
    fn removing_or_shortening_a_sounding_note_releases_it() {
        use crate::render_test_support::SAMPLE_RATE;

        let edits = [
            EngineCommand::RemoveNote {
                pattern: 0,
                channel: 0,
                id: 1,
            },
            EngineCommand::UpsertNote {
                pattern: 0,
                channel: 0,
                note: NoteEvent::new(1, 0, 24, 60, 127),
            },
            // Moved: the voice playing the old start has no note-off coming.
            EngineCommand::UpsertNote {
                pattern: 0,
                channel: 0,
                note: NoteEvent::new(1, 96, 4608, 60, 127),
            },
        ];
        for edit in edits {
            let project = held_note_project();
            let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
            render.play();
            let sounding = peak_after(&mut render, 4);
            assert!(sounding > 0.01, "nothing was sounding, got {sounding}");

            render.apply_command(edit);
            let after = peak_after(&mut render, 8);
            assert!(
                after < sounding * 0.01,
                "{edit:?} left the voice sounding: {after} against {sounding}"
            );
        }
    }

    /// Lengthening a sounding note, or changing only its velocity, leaves the
    /// voice alone: its note-off is still ahead of the playhead.
    #[test]
    fn lengthening_a_sounding_note_keeps_its_voice() {
        use crate::render_test_support::SAMPLE_RATE;

        let project = held_note_project();
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        render.play();
        let sounding = peak_after(&mut render, 4);
        render.apply_command(EngineCommand::UpsertNote {
            pattern: 0,
            channel: 0,
            note: NoteEvent::new(1, 0, 9216, 60, 100),
        });
        let after = peak_after(&mut render, 8);
        assert!(
            after > sounding * 0.5,
            "lengthening the note released it: {after} against {sounding}"
        );
    }

    /// **A note that ends while its channel is muted is ended** (MOO-99). A
    /// muted channel that has faded out is not called, so the note-off used
    /// to be dropped and the voice sat frozen mid-note; unmuting resumed it,
    /// at full sustain, long after the note it belonged to had finished.
    #[test]
    fn a_note_that_ends_while_muted_is_silent_on_unmute() {
        use crate::render_test_support::{peak_of, SAMPLE_RATE};

        const BLOCK: usize = 512;
        let mut project = held_note_project();
        // A beat long, in a one-bar pattern: over by half a second, and the
        // next lap two seconds in.
        project.channels[0].notes[0][0] = NoteEvent::new(1, 0, 96, 60, 127);
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        render.play();
        let blocks = |seconds: f32| (seconds * SAMPLE_RATE as f32) as usize / BLOCK;

        let mut sounding = 0.0f32;
        for _ in 0..blocks(0.1) {
            render.process_block(BLOCK);
            sounding = sounding.max(peak_of(&render.master().l[..BLOCK]));
        }
        assert!(sounding > 0.01, "nothing was sounding, got {sounding}");

        render.apply_command(EngineCommand::SetChannelMuted {
            channel: 0,
            muted: true,
        });
        for _ in 0..blocks(0.9) {
            render.process_block(BLOCK);
        }
        render.apply_command(EngineCommand::SetChannelMuted {
            channel: 0,
            muted: false,
        });
        // Leave the unmute ramp a moment, then listen until just before the
        // next lap.
        for _ in 0..blocks(0.1) {
            render.process_block(BLOCK);
        }
        let mut after = 0.0f32;
        for _ in 0..blocks(0.7) {
            render.process_block(BLOCK);
            after = after.max(peak_of(&render.master().l[..BLOCK]));
        }
        assert!(
            after < 1.0e-3,
            "a note that ended while its channel was muted sounded again on \
             unmute: {after} against {sounding}"
        );
    }

    /// **A chord the player holds across a Song-mode loop fold keeps
    /// sounding** (MOO-99). The fold used to choke every voice on every
    /// channel, so each lap cut the keys still down. It ends the sequencer's
    /// own voices now, and nothing else.
    #[test]
    fn a_held_chord_survives_a_song_mode_loop_fold() {
        use crate::render_test_support::{peak_of, SAMPLE_RATE};

        const BLOCK: usize = 512;
        let mut project = held_note_project();
        project.channels[0].notes[0].clear();
        project.playback_mode = PlaybackMode::Song;
        project.playlist = vec![mooloop_core::PatternPlacement {
            pattern: 0,
            start_tick: 0,
        }];
        project.loop_range = mooloop_core::LoopRange {
            start_tick: 0,
            end_tick: mooloop_core::TICKS_PER_BAR,
            enabled: true,
        };
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        for note in [60, 64, 67] {
            render.apply_command(EngineCommand::TriggerChannelNote {
                channel: 0,
                note,
                velocity: 127,
            });
        }
        render.play();

        let mut before = 0.0f32;
        let mut folded = false;
        for _ in 0..(4 * SAMPLE_RATE as usize / BLOCK) {
            let position = render.transport.position_ticks;
            render.process_block(BLOCK);
            if render.transport.position_ticks < position {
                folded = true;
                break;
            }
            before = peak_of(&render.master().l[..BLOCK]);
        }
        assert!(folded, "the transport never reached the loop end");
        assert!(before > 0.01, "the chord was not sounding, got {before}");

        // Past the first few blocks, which a choke would still be fading
        // through.
        let mut after = 0.0f32;
        for block in 0..10 {
            render.process_block(BLOCK);
            if block >= 3 {
                after = after.max(peak_of(&render.master().l[..BLOCK]));
            }
        }
        assert!(
            after > before * 0.5,
            "the fold cut a chord the player was holding: {after} against {before}"
        );
    }

    /// **Panic ends everything and keeps the song playing** (MOO-99): the
    /// pattern's voice, a held audition, and a key held down, all at once.
    #[test]
    fn panic_ends_every_voice_without_stopping_the_transport() {
        use crate::render_test_support::SAMPLE_RATE;

        let project = held_note_project();
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        render.play();
        render.apply_command(EngineCommand::TriggerChannelNote {
            channel: 0,
            note: 72,
            velocity: 127,
        });
        let sounding = peak_after(&mut render, 4);
        assert!(sounding > 0.01, "nothing was sounding, got {sounding}");

        render.apply_command(EngineCommand::Panic);
        let after = peak_after(&mut render, 8);
        assert!(
            after < sounding * 0.01,
            "Panic left a voice sounding: {after} against {sounding}"
        );
        assert!(render.transport.playing, "Panic stopped the song");
    }

    /// **Every note the sequencer starts, it ends** -- under the edits a
    /// player makes while the song runs (MOO-99; `reports/teams-2026-09-22.md`
    /// §4.2 item 6). Random note, placement, pattern-length, mode, pattern
    /// and mute edits, a few a second, and after each block every `NoteOn`
    /// the channels have been sent must have been answered by a `NoteOff` for
    /// its id, or a `Choke`, within the longest a note here can legitimately
    /// last. What fails it is a drone: a voice whose note-off an edit put
    /// somewhere the playhead will not go.
    #[test]
    fn every_sequenced_note_is_ended_under_random_edits_while_playing() {
        use crate::render_test_support::SAMPLE_RATE;
        use std::collections::HashMap;

        const BLOCK: usize = 256;
        let params = mooloop_core::MlP8Params {
            attack: 0.0,
            decay: 0.0,
            sustain: 1.0,
            release: 0.0,
            ..mooloop_core::MlP8Params::default()
        };
        let mut project = Project {
            channels: vec![
                ProjectChannel::mlp8_with_params(0, 2, params),
                ProjectChannel::mlp8_with_params(1, 2, params),
            ],
            pattern_lengths: vec![DEFAULT_STEPS, DEFAULT_STEPS],
            playlist: vec![
                mooloop_core::PatternPlacement {
                    pattern: 0,
                    start_tick: 0,
                },
                mooloop_core::PatternPlacement {
                    pattern: 1,
                    start_tick: mooloop_core::TICKS_PER_BAR,
                },
            ],
            loop_range: mooloop_core::LoopRange {
                start_tick: 0,
                end_tick: 2 * mooloop_core::TICKS_PER_BAR,
                enabled: true,
            },
            ..Project::default()
        };
        for channel in &mut project.channels {
            for (pattern, notes) in channel.notes.iter_mut().enumerate() {
                for id in 1..=4u32 {
                    notes.push(NoteEvent::new(
                        id,
                        (id - 1) * 96,
                        96 + pattern as u32 * 200,
                        48 + id as u8,
                        100,
                    ));
                }
            }
        }
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        render.play();

        // The longest a voice may honestly sound: the longest note here is
        // two bars, and a lap of the longest pattern is two more. At 120 bpm
        // a bar is two seconds.
        let longest = 5 * SAMPLE_RATE as usize / BLOCK;
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut random = move |below: u32| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % u64::from(below)) as u32
        };
        let mut sounding: HashMap<(usize, u64), usize> = HashMap::new();
        let mut muted = [false; 2];
        let mut song = false;
        let blocks = 60 * SAMPLE_RATE as usize / BLOCK;
        for block in 0..blocks {
            if random(16) == 0 {
                let pattern = random(2) as u8;
                let channel = random(2) as u8;
                let edit = match random(7) {
                    0 | 1 => EngineCommand::UpsertNote {
                        pattern,
                        channel,
                        note: NoteEvent::new(
                            1 + random(6),
                            random(4 * 96),
                            24 + random(768),
                            48 + random(24) as u8,
                            100,
                        ),
                    },
                    2 => EngineCommand::RemoveNote {
                        pattern,
                        channel,
                        id: 1 + random(6),
                    },
                    3 => EngineCommand::SetPatternLength {
                        pattern,
                        length_steps: [8, 16, 32][random(3) as usize],
                    },
                    4 => {
                        song = !song;
                        EngineCommand::SetPlaybackMode(if song {
                            PlaybackMode::Song
                        } else {
                            PlaybackMode::Pattern
                        })
                    }
                    5 => EngineCommand::SetPlaylistPlacement {
                        pattern,
                        start_tick: random(2) * mooloop_core::TICKS_PER_BAR,
                        on: random(2) == 0,
                    },
                    _ if random(2) == 0 => EngineCommand::SetCurrentPattern(pattern),
                    _ => {
                        let index = usize::from(channel);
                        muted[index] = !muted[index];
                        EngineCommand::SetChannelMuted {
                            channel,
                            muted: muted[index],
                        }
                    }
                };
                render.apply_command(edit);
            }
            render.process_block(BLOCK);
            for channel in 0..2 {
                for event in render.events[channel].iter() {
                    match event.event {
                        Event::NoteOn { id, .. } => {
                            sounding.insert((channel, id), block);
                        }
                        Event::NoteOff { id, .. } => {
                            sounding.remove(&(channel, id));
                        }
                        Event::Choke => sounding.retain(|(owner, _), _| *owner != channel),
                        _ => {}
                    }
                }
            }
            if let Some(((channel, id), started)) = sounding
                .iter()
                .find(|(_, started)| block - **started > longest)
            {
                panic!(
                    "channel {channel}'s voice {id:#x} has sounded since block \
                     {started} and is still sounding at block {block}: an edit \
                     stranded its note-off"
                );
            }
        }
    }
}


#[cfg(test)]
mod footprint {
    use super::*;

    /// What the render graph costs, and why. Both `MAX_CHANNELS` and
    /// `MAX_EFFECTS_PER_CHANNEL` are `u8::MAX + 1`, so the graph *addresses*
    /// the product of two full index spaces — 65,536 effect slots. It used to
    /// reserve all of them: 42.8 MiB before a project existed.
    ///
    /// Now it allocates neither ceiling up front. An empty effect slot costs a
    /// pointer, and a channel that does not exist costs nothing at all, so the
    /// price tracks the project rather than the limits.
    ///
    /// This is pinned rather than described because it is invisible at every
    /// individual definition — no single array looks unreasonable, and the
    /// number only appeared when they were multiplied. If this test fails,
    /// something changed a ceiling or widened a per-slot or per-channel
    /// struct; check the new figure is one worth paying before updating it.
    ///
    /// `docs/plans/archive/modulator-capacity/`.
    #[test]
    fn the_render_graph_costs_what_the_project_uses() {
        use core::mem::size_of;

        assert_eq!(MAX_CHANNELS, 256);
        assert_eq!(MAX_EFFECTS_PER_CHANNEL, 256);

        // An occupied effect slot's state is half a kilobyte; an empty one is
        // the pointer that would otherwise have reserved it.
        //
        // It grew by eight when the slot started carrying its device's
        // durable `DeviceId` (`docs/plans/containers/01`) -- four for the id
        // and four of padding -- and by eight more for a container's span and
        // the pointer to the ring that delays its dry copy
        // (`docs/plans/containers/03`). The span is a byte and falls in
        // padding; the eight is the `Option<Box<IntegerDelay>>`, and it is
        // `None` on every leaf.
        //
        // Both are paid per *occupied* slot only, so a project with twenty
        // devices on it pays 320 bytes for the pair. A per-container box
        // instead of a per-slot pointer was the alternative and it is worse:
        // it would need a second lookup on the realtime path to find the
        // container's ring from its slot, which is the table the identity
        // work exists to avoid.
        //
        // Eight more for the ring that holds a layer's shorter branch back to
        // meet its longest (`docs/plans/containers/08`), on the branch head's
        // slot for the same reason: `None` everywhere but a branch that
        // declares less than its siblings.
        //
        // And by seventy-two for MOO-108: sixty-four for the host ramps that
        // make wet/dry, the trims, Mix and bypass move per sample instead of
        // switching (five `Smoothed` and the rate they were timed for), and
        // eight for the frames a removal has waited on its fade. Again per
        // occupied slot only.
        assert_eq!(size_of::<EffectSlot>(), 592);
        assert_eq!(size_of::<Option<Box<EffectSlot>>>(), 8);
        // Eight of this is the pointer to the per-depth dry buffers a chain
        // needs while it is *inside* containers. One pointer, not four
        // buffers: what a chain needs at once is one copy per box it is
        // currently in, not one per box it holds, so ten sibling containers
        // share what four nested ones would need. A chain with no container
        // pays only the pointer. Eight more is the count of events its lists
        // had no room for, which is what lets an export say it lost some.
        //
        // `docs/plans/automation-curves/` added sixteen: a pointer to
        // `curve_scratch` (`Box<CurvePool<50>>`, the boxed reason next to
        // `EffectSlot`'s own applies here too -- fifty rows of two hundred
        // and fifty-six ticks is real size, and a chain with nothing driven
        // pays only the pointer) and the `u64` refusal counter beside it.
        //
        // MOO-176 added eight: the count of slot inputs found non-finite
        // since the render loop last published it (a `u32` and padding).
        assert_eq!(size_of::<EffectChain>(), 20_584);
        // A strip used to hold one node of every generator kind, so a new
        // device was paid for on every live channel whether or not anything
        // used it; since MOO-56 it holds only the one it plays, boxed, and
        // `per_live` below pays for the widest. The history of these two
        // nodes is kept because it is still what a channel running one pays.
        // The ML-P8 is 5,776 bytes of it. Its eight voices are the bulk -- a
        // voice carries three oscillators, their modulation taps and sync
        // carries, a sub, coloured noise, two envelopes, two filter stages, a
        // drive follower and the feedback loop's delay.
        //
        // Step 04 of `docs/plans/archive/poly-synth-v2/` added 1,536 of it, and where
        // it went is the point. Just over a thousand is per *voice*: the
        // thirty-one destination offsets a voice resolves each sample, which
        // is the price of the modulation landing per voice rather than per
        // device, and is not reducible without giving that up. The rest is
        // held once per node -- the LFO, the parameter block's LFO controls
        // and route list, and the compiled route table with the destination
        // ranges it clamps through. The compiled rows carry byte indices
        // rather than words, and the voice clears its offsets by walking the
        // routes rather than a second list of the destinations they touch;
        // both were measured here, and together they were 576 bytes.
        // Sampler stretch adds another 160 bytes for its
        // pool pointer, tempo, and per-voice struck pitches; the ~1.6 MB pool
        // itself is allocated only when a sampler asks for stretching.
        // Slicing adds 392: the channel's slice-map slot pointer, plus 24
        // bytes on each of the sixteen voices for the span it was struck
        // with. The map itself lives in the slot, off the strip. The last 8
        // are the sampler's transport-edge flag, which is what lets a note
        // auditioned while stopped keep sounding.
        //
        // Step 05 added 984, and it divides cleanly. Seventy-two of it is per
        // *voice*, and fifty-nine of that is the slot's fixed drift table --
        // eleven offsets that exist so "how far is this voice off" is a
        // property of the slot rather than of runtime entropy, which is what
        // makes an offline render reproduce a live take. The rest of the
        // voice's share is the three multipliers Drift, Detune and Spread
        // resolve to once a render range instead of once a sample. Off the
        // voices: 296 for the finishing chorus, of which 280 is the shared
        // `ModulationEffect` it reuses rather than a second chorus; 96 for the
        // two scratch bus *headers*; and 16 for the five new parameters.
        //
        // The scratch buses are the one figure this test cannot see, because
        // `size_of` a `Vec` is its header. They are deliberately one 512-frame
        // chunk each rather than `MAX_BLOCK_SIZE`, which is 8 KB of heap a
        // materialized channel instead of 128 KB; the device renders in chunks
        // to afford that, and its bit-identity tests are what say the chunk
        // boundary is not audible.
        //
        // Step 06's published control outlets added 72 on top, and they are
        // the cheapest thing in this test because publication is a reduction
        // rather than a buffer: eight bytes on the node for the focus group's
        // age and its trigger latch, and eight on each voice for the velocity
        // its note was played at. The seven outlet *values* are computed on
        // demand from state the voices already carry, so nothing here stores
        // them.
        //
        // The typed audio edges added 64: eight bytes a voice for the sub,
        // pre-filter and filter samples it publishes, recorded as it runs
        // them. Nothing on the node at all, and nothing per outlet -- a tap's
        // *buffer* is 64 KB and is allocated only when somebody subscribes,
        // which is the whole shape of that plan.
        // Grew by 64 since this figure was last written, from
        // `docs/plans/filter-coeffs/` (`reports/fable-2026-09-22.md`
        // finding 2 / Plan B, landed concurrently with
        // `automation-curves/` and not this plan's doing): each voice
        // keeps a per-sample cutoff equality cache so its filter can still
        // recompute `SvfCoeffs` bit-identically under live route
        // modulation while the other generators hoist the cutoff off the
        // per-sample path outright. Recorded here rather than left stale
        // because this assertion is the only thing that would otherwise
        // have called the drift out.
        //
        // Grew by 48 more from `reports/fable-2026-09-22.md` finding 2,
        // Plan C step 1: the finishing chorus's shared `ModulationEffect`
        // now keeps a twelve-entry `tilt` table (`f32 * 12`) for the
        // phaser's per-stage spread, rebuilt once when `stages` changes
        // instead of recomputed inline on every stage of every sample.
        //
        // Grew by 192 with MOO-145: each of the eight voices carries a
        // `Glide` (24 bytes: the slide's two ends in octaves, its length and
        // progress in samples, the pitch now and the exact target), which is
        // what makes a glide arrive in its Glide time and slide evenly in
        // pitch rather than approaching in Hz.
        //
        // Grew by 8 with MOO-128: the device's bend ratio (an `f32`, padded),
        // which it multiplies into every oscillator's pitch.
        assert_eq!(size_of::<MlP8>(), 6_152);
        // DS-01 is 6,832, and almost all of it is the eight-voice pool: a
        // voice carries six tone oscillators for its partial bank, an FM
        // modulator, four noise generators' worth of state, a state-variable
        // filter, the rate reducer's hold, four envelopes, the body's three
        // resonators, the burst's schedule, and the eight source values it
        // presents to its own matrix. Its envelopes are the largest share --
        // an `Ahd` is 48 bytes against `ExpDecay`'s 8, four of them where v1
        // has two -- and they are the largest single reason DS-01's snare and
        // its hat do not sound like the same instrument. The shape stage costs
        // almost nothing on top, being a function of the sample rather than a
        // stage with state; step 07's matrix costs a parameter block its eight
        // rows dominate, plus one resolved control set per voice, because the
        // matrix is per voice and two hits sounding at once have to be able to
        // disagree about where the filter is.
        //
        // `mooloop_dsp::ds01`'s own size test splits that between the pool and
        // the parameter block. It does not widen `source_base`: `Ds01Params`
        // is smaller than `MlP8Params`, which is still the widest
        // `GeneratorParams` variant and therefore still what every channel
        // pays for.
        //
        // Step 07's publication is the last sixteen: the focus age and the
        // trigger flag, both node state rather than voice state, because the
        // focus is a fact about this channel's run of hits and a per-voice
        // copy would be eight numbers agreeing about one.
        // The typed audio edges added 128 on top: sixteen bytes a voice for
        // the three pre-Level layers and the layer mix it publishes.
        assert_eq!(size_of::<Ds01>(), 6_960);
        // The strip pays the parameter block twice: once inside the node
        // above, and once for `source_base`, whose `GeneratorParams` is as
        // wide as its widest variant and the ML-P8 is that variant. Step 05's
        // sixteen bytes of new parameters are therefore paid twice, which is
        // why the strip moved by 1,000 where the node moved by 984. Step
        // 06's routable outlets added 32 more, and none of it is in the node:
        // it is the eight `f32` the strip holds of what its generator
        // published last block, which is where the one block of declared
        // outlet latency physically lives.
        // Latency compensation adds eight: a nullable pointer to the ring, and
        // nothing else. The ring itself is heap and exists only on a channel
        // that actually owes a delay — `4 * 2 * frames`, so fifteen frames is
        // 120 bytes — which is why the common project, where every path is the
        // same length, pays exactly this pointer and no buffer at all.
        // The typed audio edges added 208. Sixty-four of it is ML-P8's
        // published samples and 128 is DS-01's, both paid inside the nodes
        // above; the Aux In node itself is 20 bytes -- a subscription, a
        // level and its smoother -- and `GeneratorParams` did not widen,
        // because the ML-P8's parameter block is still much the largest
        // variant. A channel that is not an Aux In and publishes nothing pays
        // 20 bytes for a node it never runs, which is what every generator
        // kind already costs every channel.
        // Letting an idle channel stop rendering added eight: a count of the
        // frames its generator has been silent for, and a flag saying whether
        // it is currently asleep. The chain's half of the same bookkeeping is
        // free -- a `u32` fits in `EffectSlot`'s existing padding, which is
        // why the assertion above did not move, and an addressable-but-empty
        // slot still costs a pointer.
        // Containers added the eight above and nothing else: the dry buffers
        // themselves are behind the pointer and only allocated for a chain
        // that holds a box.
        // Playing a key with the transport stopped added 24: a `was_playing`
        // flag in every generator, so a stop releases once at the transition
        // instead of on every stopped block. DS-01's fits its padding, which
        // is why its assertion above did not move; the others round up.
        // The v1 poly's mono mode added 264, and **none of it is the three new
        // parameters** -- a bool and two one-byte enums land in padding. It is
        // the held-note stack: sixteen entries of an id, a note and a
        // velocity, the same `HeldNotes` the ML-M1 already carries, and a
        // fixed array because it is touched from `process()`. That is the
        // price of the v1 poly being a monosynth rather than a pool of one
        // voice, and it is worth paying twice over, because it is the only
        // thing keeping `DeviceKind::MonoSynth` alive and that deletion takes
        // a whole generator's state back out of this struct.
        //
        // `control-plane-seams/02` took eight back off, and it is the only
        // entry in this list that subtracts. The sampler held two slot
        // pointers -- one for the channel's buffer, one for its slice map --
        // and holds one now, because the two are published as a single
        // `ChannelAudioSnapshot`. The saving is incidental; the reason was
        // that two slots meant a note-on could land between the two stores
        // and play a new buffer against old markers. It is recorded here
        // because a figure that only ever grows stops being read.
        //
        // Finding 2 of `reports/fable-2026-09-17.md` added 152, all in the
        // sampler, and all so a note-on never frees a sample buffer: a fixed
        // ring of sixteen displaced voice samples (128), its count (8), one
        // displaced snapshot (8), and the snapshot the last note-on read,
        // held rather than dropped (8). The buffers themselves are not new
        // memory -- they were already alive until the voice let go -- they
        // just leave on the control thread now.
        //
        // `audio-recording/03` added 8: the channel's take, one pointer, on
        // the strip so that an install carrying the strip carries the take.
        // The ring and the status it points to are allocated only while a
        // take is armed.
        //
        // `teams-2026-09-22` C4 added 8, in the strip's `EffectChain`: the
        // count of events its lists refused, so an export can say it lost
        // some rather than quietly dropping a parameter.
        //
        // `docs/plans/automation-curves/` moved this by eighty: a strip
        // holds one `EffectChain` by value, not by pointer, so
        // `curve_scratch`'s pointer and refusal counter (sixteen of it, the
        // same sixteen `EffectChain` moved by above) are paid here too; the
        // rest is alignment padding the compiler adds around them.
        //
        // Grew by 48 more with `MlP8`'s own figure above
        // (`reports/fable-2026-09-22.md` finding 2, Plan C step 1): the
        // strip holds one `MlP8` by value, so its finishing chorus's new
        // `tilt` table is paid here too.
        //
        // MOO-107 added 40, in the output stage: the pan law it applies, as
        // a function pointer (8), and the gain actually reaching each side,
        // two `Smoothed` of twelve bytes each, so a fader, a pan and a mute
        // ramp instead of stepping. The rest is alignment.
        //
        // MOO-99 added 1,552: the table of voices the sequencer started
        // (`crate::voices`), sixty-four entries of twenty-four bytes and a
        // count, so a release can name the pattern's voices instead of
        // choking the channel. On the strip, so it travels with the voices
        // through an install.
        //
        // MOO-145 added 624, one 24-byte `Glide` per synth voice the strip
        // holds by value: the ML-P8's eight (192, above), the poly synth's
        // sixteen (384), and one each for the v1 mono and the ML-M1 (48).
        //
        // MOO-56 took 21,928 back off, the most any entry here has moved it
        // in either direction. The eight generators the strip held by value
        // -- 21,944 bytes, of which a live channel ever played one -- became
        // one `Box<dyn SourceNode + Send>`, sixteen bytes, and the
        // `active_source` tag went with them into padding. The device the
        // channel plays is still paid, once, on the heap: see `per_live`.
        // What it bought besides the bytes is that a ninth kind of source,
        // a hosted plugin among them, costs a channel nothing until it is
        // played. Much of the history above is now the history of the nodes
        // rather than of the strip; it stays, because it is still what a
        // channel running that node pays.
        //
        // MOO-176 added eight, its chain's unpublished fault count.
        assert_eq!(size_of::<ChannelStrip>(), 22_808);

        // Reserved whatever the project holds: the two small modulation
        // vectors, plus three vectors of pointers to per-channel storage.
        // Sixteen KiB of the rise is `ParamAddr` growing four bytes to carry
        // a durable internal-route id, paid once per stored route across the
        // reserved channel count. Another sixteen is `ModRoute` growing four
        // the same way and for the same kind of reason: its source became a
        // `ModSourceRef`, because a generator outlet is not a rack module and
        // has no identity the rack could mint for it.
        //
        // And another sixteen for the third instance of that same trade
        // (`docs/plans/containers/01`): `ParamOwner::Effect` stopped naming a
        // `u8` slot and started naming a `u32` `DeviceId`, so a route's
        // destination no longer moves when the rack is reordered. Four bytes
        // an address, 64 a channel's rack, 16 KiB across the reserved count.
        // The same price and the same argument as the two above, and it buys
        // the thing they bought one level out.
        let fixed = (size_of::<ModRack>() + size_of::<ModulatorRack>()) * MAX_CHANNELS
            + MAX_CHANNELS * size_of::<usize>() * 3;
        assert_eq!(fixed / 1024, 487);

        // Paid per channel the project actually has. The source is boxed
        // since MOO-56, so it is not in `ChannelStrip`'s own size, and the
        // figure pays for the widest one a live channel can hold: DS-01's
        // eight-voice pool, just ahead of the ML-P8's.
        let widest_source = [
            size_of::<Sampler>(),
            size_of::<DrumSynth>(),
            size_of::<MonoSynth>(),
            size_of::<PolySynth>(),
            size_of::<MlM1>(),
            size_of::<MlP8>(),
            size_of::<Ds01>(),
            size_of::<AuxIn>(),
        ]
        .into_iter()
        .max()
        .unwrap_or(0);
        assert_eq!(widest_source, size_of::<Ds01>());
        let per_live = size_of::<ChannelStrip>()
            + widest_source
            + size_of::<EventList>()
            + size_of::<ControlOutputs>()
            + size_of::<SourceCurvePool>();
        // The sampler's retired-sample ring is 152 of this, a take's pointer
        // 8, and the effect chain's refused-event count 8.
        //
        // `docs/plans/automation-curves/` added `SourceCurvePool` itself,
        // 94,592 of this: ninety-two rows (`DS_01`'s own descriptor count,
        // the widest generator table, so it is the one bound has to cover)
        // of two hundred fifty-six ticks each, the same per-destination
        // shape `EffectChain::curve_scratch` pays per *slot* rather than
        // per *channel*. It replaces per-tick `push_ordered`s into the
        // channel's shared note list with rows that do not compete with
        // each other or with notes for room -- see finding 3,
        // `reports/fable-2026-09-22.md`. Boxed like everything else in this
        // list, so an addressable-but-idle channel never pays it; this
        // number is what a *live* one does.
        //
        // Grew by 48 with `ChannelStrip` above (Plan C step 1, same
        // finding): the phaser's tilt table again, once per live channel.
        //
        // And by 40 with it for MOO-107: the output stage's ramps.
        //
        // And by 1,552 with it for MOO-99: the sequenced-voice table.
        //
        // And by 624 with it for MOO-145: a `Glide` per synth voice.
        //
        // MOO-56 took 14,968 off: the strip's 21,928 above, less the 6,960
        // of the one source now counted on its own. A channel running a
        // smaller source than DS-01 pays less than this; a v1 mono pays
        // 6,624 less again.
        //
        // And by 8 with it for MOO-176: the chain's unpublished fault count.
        assert_eq!(per_live, 142_800);

        // 42.8 MiB reserved at startup became 1.1 MiB for a sixteen-channel
        // project, with both ceilings untouched. A sixth generator kind moved
        // it by 41 KiB, which is what a device costs now: linear in the
        // channels a project has rather than in the channels it could address.
        // Slice mode moved it by 6 KiB across sixteen channels, the ML-P8's
        // native modulation another 40 -- 24 of it per-voice offset tables on
        // sixteen channels, which is what buys modulation that lands per
        // voice, and 16 of it the wider parameter address, paid once per
        // reserved channel whether or not anything routes -- and DS-01, the
        // seventh kind, another 106. The ML-P8's voice pool moved it 15 more:
        // nine of that is the eight slots' fixed drift tables, and the rest is
        // the finishing chorus and its two scratch headers. The chorus's
        // *buffers* are 8 KiB of heap a live channel on top of this, which is
        // the figure the 512-frame chunk exists to keep small. Step 06's
        // published outlets moved it by one more KiB across sixteen channels,
        // which is what a device's whole published interface costs when
        // publication is a reduction of state that already exists. Making
        // them routable moved it by half of one more: 32 bytes a live channel
        // for the published row, and 16 KiB of the reserved figure above for
        // the wider route. DS-01 publishing its own six added sixteen bytes a
        // live channel -- a focus age and a trigger flag on the node, and
        // nothing on any voice. Latency compensation added eight more, which
        // is a pointer: the ring is allocated only for a producer that is
        // genuinely shorter than its neighbours, so an aligned project pays
        // nothing beyond the pointer.
        //
        // It nearly cost 8 KiB a live channel instead. Putting the outlet
        // band in the per-tick control table would have stored eight
        // block-constant values in all 256 tick rows; `ControlSources` keeps
        // the flat *address space* routes and projects depend on while
        // storing each half at the rate it is actually captured. This test
        // is what asked the question.
        // The typed audio edges moved it by 4 KiB across sixteen channels,
        // and that is the entire cost of the feature on a project that never
        // uses it: no buffers, no plan storage beyond one boxed value for the
        // whole engine, and an identity render order. Materialising ML-P8's
        // seven stereo outlets unconditionally would have been 448 KB a
        // channel instead -- 7 MB across a full bank for something switched
        // off -- which is the design `03-materialized-taps.md` exists to
        // avoid, and this is the measurement that says it was avoided.
        // Durable device identity added 16 KiB of the reserved figure and
        // nothing per live channel: an effect slot is heap-allocated, so its
        // own growth is per *occupied* slot rather than per channel.
        // Containers added eight bytes a live channel -- one pointer to the
        // dry buffers a chain needs while it is inside a box -- and the
        // buffers themselves only exist on a chain that holds one. 128 bytes
        // across sixteen channels, which does not move the figure below.
        // Releasing on the transport's stop edge rather than on every stopped
        // block added 24 bytes a live channel, the generators' `was_playing`
        // flags: 384 bytes across sixteen.
        // The v1 poly's mono mode added 264 a live channel and nothing to the
        // reserved figure -- a held-note stack is per sounding voice, not per
        // addressable channel -- so four KiB across sixteen.
        // The sampler's retired-sample ring added 152 a live channel, so
        // that a note-on never frees a buffer: 2.4 KiB across sixteen.
        //
        // `docs/plans/automation-curves/` is the one that moved this
        // figure by more than a rounding error: `SourceCurvePool` alone is
        // 92.4 KiB a live channel, 1,478 KiB across sixteen -- almost all
        // of the difference between 1,437 and 2,916. `N` is sized to
        // DS-01's own ninety-two-descriptor table on the stated principle
        // ("derive it, don't guess" -- a project with a smaller generator
        // never has more destinations to drive than that table can name),
        // and every row of it is boxed so an idle or addressable-but-empty
        // channel never pays it, the same shape `EffectChain::curve_scratch`
        // pays. This is the cost of removing the per-slot 256-event cap
        // (`reports/fable-2026-09-22.md` finding 3) rather than a leak --
        // whether it is a cost worth paying for every live channel, or
        // whether the bound belongs somewhere narrower than "the widest
        // generator's whole table", is a product question this run did not
        // have standing to answer and is recorded rather than decided in
        // `docs/plans/automation-curves/00-status.md`.
        //
        // Crossed one more KiB boundary with `per_live` above: the phaser's
        // tilt table is 768 bytes across sixteen live channels (Plan C
        // step 1, `reports/fable-2026-09-22.md` finding 2).
        //
        // And one more with MOO-107's output-stage ramps: 40 bytes a live
        // channel, 640 across sixteen.
        //
        // MOO-99's sequenced-voice table: 1,552 bytes a live channel, about
        // 24 KiB across sixteen.
        //
        // MOO-145's per-voice `Glide`: 624 bytes a live channel, about
        // 10 KiB across sixteen.
        //
        // MOO-56's one boxed source: 14,968 bytes less a live channel,
        // 234 KiB across sixteen, and that at the widest source. Sixteen
        // v1 monos are 104 KiB lighter again.
        assert_eq!((fixed + per_live * 16) / 1024, 2_718);
    }

}


/// A compensation ring the same length as the live one keeps the live one --
/// across an install, a reconciler's resend, and a send bank arriving.
/// `incremental-structure/`'s option 1: what a structural edit still emptied
/// after step 02 was exactly these rings.
///
/// Asserted on the ring's address rather than on audio. A kept ring and a
/// fresh one are indistinguishable until the delay's length has played out,
/// and the address is the property the fix actually holds.
#[cfg(test)]
mod kept_rings {
    use super::*;
    use mooloop_core::{EffectKind, EffectSlotState, ProjectChannel};

    fn ring(ring: &Option<Box<IntegerDelay>>) -> Option<usize> {
        ring.as_ref().map(|ring| &**ring as *const IntegerDelay as usize)
    }

    /// Two channels on two tracks, and a Drive on the first: its oversampler
    /// is a latency, so the *other* track is owed a ring into the master.
    fn one_late_path() -> Project {
        let mut project = Project::default();
        project.channels.push(ProjectChannel::mono_synth(1, 1));
        project.assign_channel_ids();
        project.ensure_tracks(3);
        project.channels[0].setup.channel.bus = 1;
        project.channels[1].setup.channel.bus = 2;
        project.channels[0]
            .setup
            .push_effect(EffectSlotState::of_kind(EffectKind::Drive))
            .expect("room in the chain");
        project
    }

    #[test]
    fn a_track_move_keeps_a_ring_it_is_still_owed() {
        let project = one_late_path();
        let live = RenderState::from_project(48_000, &project, &[]);
        let held = ring(&live.buses[2].compensation);
        assert!(held.is_some(), "the premise: track 2 is owed a delay");

        let mut moved = project.clone();
        moved.move_track(1, 2).expect("a real move");
        let mut live = live;
        let mut incoming = RenderState::from_project(48_000, &moved, &[]);
        incoming.carry_strips_from(&mut live, &crate::carry_plan(&project, &moved));

        assert_eq!(
            ring(&incoming.buses[1].compensation),
            held,
            "the carried track restarted its delay from silence"
        );
    }

    /// The channel side of the same rule: two channels into one track, the
    /// Drive on the first, so the second is owed a ring -- and keeps it
    /// through a channel move.
    #[test]
    fn a_channel_move_keeps_a_ring_it_is_still_owed() {
        let mut project = one_late_path();
        project.channels[1].setup.channel.bus = 1;
        let mut live = RenderState::from_project(48_000, &project, &[]);
        let held = ring(&live.strips[1].compensation);
        assert!(held.is_some(), "the premise: channel 1 is owed a delay");

        let mut moved = project.clone();
        moved.move_channel(1, 0).expect("a real move");
        let mut incoming = RenderState::from_project(48_000, &moved, &[]);
        incoming.carry_strips_from(&mut live, &crate::carry_plan(&project, &moved));

        assert_eq!(
            ring(&incoming.strips[0].compensation),
            held,
            "the carried channel restarted its delay from silence"
        );
    }

    /// The other side of the rule: a track owed a *different* delay gets the
    /// ring built for it, full of nothing, because the one it had is the
    /// wrong length and its contents are aligned to a path that has changed.
    #[test]
    fn a_ring_of_the_wrong_length_is_replaced() {
        let project = one_late_path();
        let mut live = RenderState::from_project(48_000, &project, &[]);
        let held = ring(&live.buses[2].compensation);

        let mut longer = project.clone();
        longer.channels[0]
            .setup
            .push_effect(EffectSlotState::of_kind(EffectKind::Drive))
            .expect("room in the chain");
        let mut incoming = RenderState::from_project(48_000, &longer, &[]);
        let built = ring(&incoming.buses[2].compensation);
        incoming.carry_strips_from(&mut live, &crate::carry_plan(&project, &longer));

        assert_ne!(held, built);
        assert_eq!(ring(&incoming.buses[2].compensation), built);
    }

    /// The session forgets what it sent on every install and resends the
    /// whole plan a tick later, each ring freshly built. The same length is a
    /// no-op; the arriving ring goes back for reclaim.
    #[test]
    fn a_resent_ring_of_the_same_length_is_handed_back() {
        let project = one_late_path();
        let mut render = RenderState::from_project(48_000, &project, &[]);
        let held = ring(&render.buses[2].compensation);
        let frames = ring_frames(&render.buses[2].compensation) as u32;

        let returned = render.apply_structural(StructuralCommand::SetCompensation {
            target: EffectTarget::Bus(2),
            delay: IntegerDelay::new(frames).map(Box::new),
        });
        assert!(
            matches!(returned, Some(StructuralReclaim::Compensation(_))),
            "the arriving ring was not handed back"
        );
        assert_eq!(ring(&render.buses[2].compensation), held);

        let returned = render.apply_structural(StructuralCommand::SetCompensation {
            target: EffectTarget::Bus(2),
            delay: IntegerDelay::new(frames + 1).map(Box::new),
        });
        assert!(matches!(returned, Some(StructuralReclaim::Compensation(_))));
        assert_ne!(ring(&render.buses[2].compensation), held, "a new length was refused");
    }

    fn send(producer: u8, target: u8, delay: u32) -> SendSpec {
        SendSpec {
            producer: EffectTarget::Bus(producer),
            target,
            tap: SendTap::PostFader,
            enabled: true,
            level: 1.0,
            delay,
        }
    }

    /// A send bank arriving keeps the rings of the edges it did not change,
    /// found where their two ends went -- here both ends swapped seats.
    #[test]
    fn a_send_keeps_its_ring_through_a_move_of_both_ends() {
        let mut old = SendBank::new(&[send(1, 2, 64), send(1, 3, 32)], 48_000);
        let held = ring(&old.sends[0].compensation);
        let mut new = SendBank::new(&[send(2, 1, 64), send(2, 3, 32)], 48_000);
        let other = ring(&old.sends[1].compensation);

        let swap = |seat: EffectTarget| match seat {
            EffectTarget::Bus(1) => Some(EffectTarget::Bus(2)),
            EffectTarget::Bus(2) => Some(EffectTarget::Bus(1)),
            other => Some(other),
        };
        new.adopt_rings_from(&mut old, swap);

        let range = new.range(EffectTarget::Bus(2));
        let to_one = range.clone().find(|&at| new.sends[at].target == 1).expect("the edge");
        let to_three = range.clone().find(|&at| new.sends[at].target == 3).expect("the edge");
        assert_eq!(ring(&new.sends[to_one].compensation), held);
        assert_eq!(ring(&new.sends[to_three].compensation), other);
    }

    /// And through the reconciler's own resend, where nothing moved.
    #[test]
    fn a_resent_send_bank_keeps_its_rings() {
        let project = one_late_path();
        let mut render = RenderState::from_project(48_000, &project, &[]);
        *render.sends = SendBank::new(&[send(1, 2, 64)], 48_000);
        let held = ring(&render.sends.sends[0].compensation);

        let returned = render.apply_structural(StructuralCommand::SetTrackGraph {
            graph: render.bus_graph,
            sends: Box::new(SendBank::new(&[send(1, 2, 64)], 48_000)),
        });
        assert!(matches!(returned, Some(StructuralReclaim::TrackGraph(_))));
        assert_eq!(ring(&render.sends.sends[0].compensation), held);
    }
}

