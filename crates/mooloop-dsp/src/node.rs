//! The `AudioNode` trait — the common interface every DSP unit implements.
//!
//! This is deliberately shaped like the modern plugin APIs (VST3's
//! `IAudioProcessor`, CLAP's `process`, LV2's `run`): each call hands the
//! node an audio buffer to work on, a list of sample-timed input events, and
//! an optional output event list. A future plugin-hosting layer can map CLAP
//! or LV2 plugins onto this trait one-to-one.
//!
//! ## Processing model
//!
//! Nodes process **in place** on a [`StereoBus`]:
//!
//! - **Instruments** assume the bus has been cleared for the block and add
//!   their rendered audio into it.
//! - **Effects** read the bus and modify it in place (input == output).
//!
//! The engine owns all buses and decides what each node's buffer means —
//! today every channel strip is `instrument -> [effects] -> gain/pan ->
//! master`. Future routing (send/return buses, sidechain inputs) hangs off
//! the engine's bus management, not a node retaining someone else's storage.
//! A future process-buffer view will provide auxiliary inputs such as a
//! sidechain for the duration of each call, exactly like plugin port groups.
//!
//! ## Realtime safety contract
//!
//! `process` runs on the JACK realtime thread. Implementations MUST NOT:
//! - allocate or free memory,
//! - take any lock that could be contended by a non-RT thread,
//! - perform I/O or syscalls,
//! - block or wait.
//!
//! Parameter changes arrive via the command queue / lock-free structures
//! owned by the node; the trait exposes only the realtime surface.

use crate::bus::StereoBus;
use crate::event::{Event, EventList, TimedEvent};
use crate::sampler::Sampler;
use crate::taps::AudioTaps;
use mooloop_core::modulation::MAX_GENERATOR_OUTLETS;
use mooloop_core::{DeviceKind, GeneratorParams, PluginSlotId};

/// A node built outside the engine and handed to it: a hosted plugin's
/// processor, on its way into or out of a slot.
pub type HostedNode = Box<dyn AudioNode + Send>;

/// Control subdivisions in the largest block the engine will ever hand a
/// node, so a per-destination curve buffer can be sized once and shared.
///
/// Derived rather than restated: [`crate::bus::MAX_BLOCK_SIZE`] is the
/// executor's block-size ceiling and [`crate::modulator::CONTROL_RATE_FRAMES`]
/// is the subdivision every control source already advances at
/// (`docs/MODULATION.md`'s "32 or 64 frames"). `mooloop-engine`'s own
/// per-block tables used to recompute this same division privately; it now
/// imports this constant instead; see
/// `docs/plans/automation-curves/00-status.md`.
pub const MAX_CONTROL_TICKS_PER_BLOCK: usize =
    crate::bus::MAX_BLOCK_SIZE / crate::modulator::CONTROL_RATE_FRAMES;

/// One driven destination's resolved value at every control subdivision of
/// the current block — the representation `docs/MODULATION.md` calls a
/// curve, handed to a node instead of a step of `Event::ParamValue` events.
///
/// `values[t]` is the value at control tick `t`, in the same natural units
/// [`Event::ParamValue`] always carried; the caller trims the slice to the
/// block's actual tick count, so `values.len()` is never more than
/// [`MAX_CONTROL_TICKS_PER_BLOCK`] and may be less on a short final block.
#[derive(Clone, Copy, Default)]
pub struct ControlCurve<'a> {
    /// The destination's stable descriptor id -- exactly what an equivalent
    /// `Event::ParamValue { id, .. }` would have carried.
    pub id: u32,
    pub values: &'a [f32],
    /// Whether `values` are the parameter's value or an offset over it.
    /// Native destinations only ever get [`CurveKind::Value`].
    pub kind: CurveKind,
}

/// What a [`ControlCurve`]'s values are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CurveKind {
    /// The destination's value, in its natural units: what
    /// `Event::ParamValue` carries.
    #[default]
    Value,
    /// An offset in the destination's plain units over the value its owner
    /// holds: what `Event::ParamMod` carries. Only a hosted plugin's
    /// parameter is driven this way, because only there does the value
    /// belong to someone else (MOO-82).
    Offset,
}

/// One of a hosted plugin's parameters as its processor describes it to the
/// engine's control pass, which has no `&'static` descriptor for it: enough
/// to turn a lane's or a route's normalized value into the plugin's plain
/// units, and to know which of the two it takes (MOO-82).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HostedParam {
    pub min: f64,
    pub max: f64,
    /// `Some(n)` for a parameter with `n` discrete positions.
    pub steps: Option<u16>,
    /// Whether a lane may drive it.
    pub automatable: bool,
    /// Whether a route may offset it.
    pub modulatable: bool,
}

impl HostedParam {
    /// The plain value at `normalized` (`0..=1`) of the range: linear, as
    /// the plan has every plugin parameter, and rounded to a whole position
    /// for a stepped one.
    pub fn plain(&self, normalized: f32) -> f32 {
        let value = self.min + f64::from(normalized.clamp(0.0, 1.0)) * (self.max - self.min);
        let value = if self.steps.is_some() { value.round() } else { value };
        value as f32
    }

    /// A normalized offset as a plain one: the same fraction of the range.
    pub fn plain_offset(&self, normalized: f32) -> f32 {
        (f64::from(normalized) * (self.max - self.min)) as f32
    }
}

/// Per-block context handed to every `AudioNode::process` call. Valid only
/// for the duration of the call; must not be retained.
#[derive(Debug, Clone, Copy)]
pub struct ProcessContext {
    /// Sample rate in Hz, fixed for the lifetime of the audio client.
    pub sample_rate: u32,
    /// Number of frames in this block (the active region of the bus).
    pub frames: usize,
    /// Transport state for this block. Nodes that sync to the host
    /// (tempo-synced delays, LFOs) should read these rather than guessing.
    pub playing: bool,
    pub bpm: f64,
    /// Transport position in ticks at the start of this block.
    pub position_ticks: f64,
    /// Absolute transport position in frames at the start of this block.
    /// Ground truth for tempo-synced nodes (delays, LFOs): unlike tick
    /// position it never accumulates float error.
    pub position_frames: u64,
}

/// What a gain-reducing device did over one block, for its display.
///
/// Both values are referred to the node's *input*, so a display can plot
/// them straight onto the same input-level axis as its transfer curve, and
/// both are block extremes rather than end-of-block samples: an attack
/// faster than the GUI's frame interval is the one thing a dynamics display
/// most needs to show.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DynamicsFrame {
    /// The loudest level the sidechain detector reached, in dB. This is the
    /// post-ballistics envelope, not a raw sample peak: it is the signal the
    /// gain computer actually saw, so a dot drawn at it moves with the
    /// device's attack and release rather than with the block boundaries.
    pub detector_db: f32,
    /// The deepest gain reduction applied, in dB. Always <= 0.
    pub reduction_db: f32,
}

impl DynamicsFrame {
    /// A block in which nothing was heard and nothing was reduced.
    pub const SILENT: Self = Self {
        detector_db: f32::NEG_INFINITY,
        reduction_db: 0.0,
    };
}

/// Peak below which a block of audio counts as silence for the purpose of
/// letting a node sleep.
///
/// This is a statement about audibility, not about float performance —
/// `enable_flush_to_zero()` in `mooloop-engine`'s graph already deals with
/// denormals, and this sits twenty-odd orders of magnitude above them. The
/// line is drawn one bit below a 24-bit render's least significant step
/// (`2^-23`, about `1.2e-7`): anything quieter than this cannot survive an
/// export at the deepest integer depth the application writes, so discarding
/// it cannot change a rendered file. Played back at a level that puts full
/// scale at 100 dB SPL it is -40 dB SPL, which is below the noise floor of a
/// room, let alone of the converter.
///
/// Deliberately not looser. A reverb tail crossing -100 dBFS is still a
/// reverb tail, and the whole risk in this mechanism is cutting one off.
pub const SILENCE_PEAK: f32 = 1.0e-7;

/// Magnitude below which a node's own internal state counts as settled.
///
/// Two orders of magnitude under [`SILENCE_PEAK`], because state is not
/// output: a filter's stored sample passes through the rest of its own
/// recurrence before it is heard, and the free response of a resonant stage
/// can grow slightly for a sample or two before it decays. The margin is what
/// makes "the state is this small" imply "nothing audible can come out of
/// it".
pub const REST_EPSILON: f32 = 1.0e-9;

/// Frames for a feedback loop of per-trip gain `gain` and trip length
/// `trip_frames` to decay below [`SILENCE_PEAK`], as a [`AudioNode::tail_frames`]
/// answer.
///
/// One shared derivation because two devices already need the same one — the
/// delay's repeats and the modulation line's regeneration — and anything else
/// built on a ring that feeds itself will need it too. A gain of one or more
/// never decays and returns `u32::MAX`, which is the trait's "never skip me".
pub fn feedback_tail_frames(gain: f32, trip_frames: f32) -> u32 {
    let trip = trip_frames.max(1.0);
    let gain = gain.abs();
    if !gain.is_finite() || gain >= 1.0 {
        return u32::MAX;
    }
    // One pass through the line even with no feedback at all, then however
    // many further trips the gain needs to fall below audibility. Rounded up
    // twice over: the extra trip, and the ceiling.
    let trips = if gain <= 0.0 {
        1.0
    } else {
        SILENCE_PEAK.ln() / gain.ln() + 1.0
    };
    let frames = trips * trip;
    if !frames.is_finite() || frames >= u32::MAX as f32 {
        u32::MAX
    } else {
        frames.ceil() as u32
    }
}

/// A realtime audio node (instrument or effect).
/// What kind of discontinuity the host is reporting.
///
/// The kind is the whole point of [`AudioNode::on_discontinuity`] carrying an
/// argument: the three mean different things to a device, and before this
/// they all arrived as one synthesised choke with no way to tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Discontinuity {
    /// The playhead moved somewhere it was not travelling towards. Audio a
    /// node is holding belongs to the old position and is now wrong -- this
    /// is the one that invalidates a delay line.
    Seek,
    /// The transport turned back at a loop end. **Time is discontinuous and
    /// the music usually is not**: the same bar is about to play again, so a
    /// tail that crosses the fold is the sound a groove box is expected to
    /// make, where a tail that crosses a seek is not.
    ///
    /// **Tails survive it** -- Adam, 2026-09-22 (MOO-59): a delay repeat or a
    /// reverb tail wraps from the end of the loop into its start. So it does
    /// not [invalidate tails](Self::invalidates_tails), and the four devices
    /// that clear on a seek decline it. Until then it arrived *as* a `Seek`
    /// (`reports/fable-2026-09-21.md`, finding 2), and every lap of every
    /// loop zeroed every delay line, reverb and plate in the project.
    LoopFold,
    /// The transport stopped. Everything a node holds is about to be
    /// inaudible anyway; the reason to say so is that it must not come back
    /// on the next play.
    Stop,
    /// What is being scheduled changed underneath a running transport -- the
    /// current pattern switched in Pattern mode. **Time is still
    /// continuous**, so audio in flight is still correct and a node that
    /// flushes on this is wrong. What is lost is a note-off, not a position.
    ProgramChange,
}

impl Discontinuity {
    /// Whether audio a node is holding -- a delay's repeats, a reverb's
    /// tail, a chorus line -- belongs to somewhere the transport has left,
    /// and so must not be heard over where it is now.
    ///
    /// The one copy of that rule: the delay, the modulation effect (and
    /// through it ML-P8's chorus), the reverb and the plate all ask it
    /// rather than each spelling its own list of kinds, because four lists
    /// are how one of them comes to disagree. A `match` with no wildcard, so
    /// a fifth kind cannot arrive without somebody deciding this for it.
    ///
    /// - [`Self::Seek`]: yes. The tail is the sound of a bar the player has
    ///   left.
    /// - [`Self::Stop`]: yes, today -- the playhead returns to the start and
    ///   the tail must not come back on the next play. Whether a tail should
    ///   ring out *past* the Stop instead is a question for Adam (MOO-171),
    ///   and this arm is where its answer lands.
    /// - [`Self::LoopFold`]: no. Adam's ruling, 2026-09-22 (MOO-59): tails
    ///   survive a loop fold.
    /// - [`Self::ProgramChange`]: no. Time is still continuous.
    pub fn invalidates_tails(self) -> bool {
        match self {
            Self::Seek | Self::Stop => true,
            Self::LoopFold | Self::ProgramChange => false,
        }
    }
}

pub trait AudioNode {
    /// Frames of audible output this node can still produce after its input
    /// goes silent.
    ///
    /// `u32::MAX` — the default — means "unbounded or unknown" and tells the
    /// host it may never let this node sleep on the strength of a tail alone.
    /// Any other value is a promise the node has to be able to defend: once
    /// the host has fed it this many frames of silence, continuing to call
    /// `process` would produce nothing above [`SILENCE_PEAK`]. Over-report
    /// rather than under-report. A tail that is too long costs a little CPU;
    /// a tail that is too short is an audibly truncated reverb.
    ///
    /// **A ring counts even when nothing is reading it yet.** The host stops
    /// calling `process` when it sleeps a node, so the node's delay lines
    /// stop advancing and their contents freeze. If a parameter can later
    /// move a read head further back than the reported tail — a delay time
    /// swept up, a reverb's pre-delay lengthened — it would read audio from
    /// before the silence. So a tail must also cover the capacity of every
    /// ring whose read offset a parameter can move, not just the time the
    /// device takes to go quiet at its current settings.
    ///
    /// Queried once per block, never from a sample loop.
    fn tail_frames(&self) -> u32 {
        u32::MAX
    }

    /// True when the node's internal state has settled, so that silent input
    /// from here on produces silent output *immediately*.
    ///
    /// This is a statement about the node's own state, not about the audio it
    /// was last handed: the host tracks how long its input has been silent
    /// and combines the two. A memoryless effect is therefore always at rest,
    /// and it is still only skipped once its input goes quiet. The default is
    /// `false`, so a node that has not opted in is never skipped.
    ///
    /// **Nothing driven by the input may still be moving.** Silence is not
    /// only about what can be heard: a parameter lag halfway to its target or
    /// a delay head halfway through a glide would be frozen where it was and
    /// come back describing settings nobody dialled. `Smoothed::is_settled`
    /// is the usual answer, and it is exact because the lag snaps rather than
    /// asymptoting. What moves whether or not the node is called belongs to
    /// [`Self::skip_block`] instead, not here.
    ///
    /// Must be cheap — a field read or a handful of comparisons. It is called
    /// once per node per block on the audio thread, and it must never scan a
    /// buffer.
    fn is_at_rest(&self) -> bool {
        false
    }

    /// Processing latency of the active path in base-rate frames. The value is
    /// queried while a graph is prepared, never from a hot inner sample loop.
    /// Nodes with internal parallel paths must align them before reporting the
    /// resulting external latency.
    fn latency_frames(&self) -> u32 {
        0
    }

    /// Frames by which the generic host should delay its dry branch before it
    /// mixes it with this node. Most latency-producing effects return their
    /// processing latency here. A wet-only device such as convolution reverb
    /// can retain an intentional delayed return without moving the channel's
    /// dry signal and neighbouring tracks with it.
    fn dry_path_latency_frames(&self) -> u32 {
        self.latency_frames()
    }

    /// A hosted plugin's parameter `id`, for the control pass to drive it
    /// by, or `None` when this node has no such parameter. Every native node
    /// answers `None`: their ranges are in their kind's descriptor table.
    /// Called on the audio thread, so it must not allocate or lock.
    fn hosted_param(&self, id: u32) -> Option<HostedParam> {
        let _ = id;
        None
    }

    /// Number of times a retained-audio read head has been overtaken by its
    /// writer and force-returned to live. Only the buffer device reports a
    /// nonzero value; the host publishes it as display telemetry so forced
    /// returns are observable without logging from the audio thread.
    fn buffer_collisions(&self) -> u64 {
        0
    }

    /// The latest picture of a retained-audio buffer, for a face to draw.
    ///
    /// Only the buffer device answers. It is display telemetry with no timing
    /// guarantee beyond "latest available", and it must never be read by an
    /// audio node as input -- a musical control signal belongs in the
    /// modulator path, where it has a declared rate and latency.
    fn buffer_waveform(&self) -> Option<crate::buffer_device::BufferDisplay<'_>> {
        None
    }

    /// Whether this node is holding audio that replacing it would destroy.
    ///
    /// Only the buffer device answers yes, and only while frozen: its ring is
    /// then a sample somebody is playing rather than a moving window nobody
    /// can hear the contents of. A ring resize builds a replacement
    /// off-thread and swaps it in, which is right for every other reason a
    /// buffer is rebuilt and wrong for this one -- an ordinary tempo change
    /// would silently take the frozen audio away mid-performance. The chain
    /// refuses the swap instead, and the new node goes back down the reclaim
    /// ring unused.
    fn holds_frozen_audio(&self) -> bool {
        false
    }

    /// This node as a retained-audio buffer, if it is one.
    ///
    /// For the one thing the host does with a buffer that no other node
    /// needs: when a ring resize swaps a replacement in, the replacement takes
    /// over the history the outgoing ring holds (MOO-137,
    /// `BufferDevice::adopt_history_from`). The same shape as
    /// `SourceNode::as_sampler_mut`.
    fn as_buffer_device_mut(&mut self) -> Option<&mut crate::buffer_device::BufferDevice> {
        None
    }

    /// What this node's gain computer did over the block just processed, if
    /// it has one. Only the dynamics devices report a value; the host
    /// publishes it as display telemetry, the same way `buffer_collisions`
    /// is published, so a transfer-curve display can show the detector and
    /// the gain reduction without the audio thread knowing about the GUI.
    fn dynamics_frame(&self) -> Option<DynamicsFrame> {
        None
    }

    /// Whether this node publishes its own display spectrum, in place of the
    /// generic analyzer the host otherwise runs on its input.
    ///
    /// Almost nothing should: an input spectrum is what a display usually
    /// wants and the host already has the bus. The exception is a device
    /// whose display is about *what it did* rather than what arrived --
    /// the preamp shows where on the spectrum it is distorting, which needs
    /// its dry and wet signals compared and only the device has both.
    ///
    /// A node that answers true owns the stage: the host stops feeding its
    /// generic analyzer, so the two never write the same cells.
    fn provides_display_spectrum(&self) -> bool {
        false
    }

    /// This node's own display spectrum, if a fresh one is ready.
    ///
    /// Taken rather than read, so a frame is published once. Answered at most
    /// once per analysis hop; `None` on every other block, which is most of
    /// them.
    fn take_display_spectrum(&mut self) -> Option<[f32; crate::analysis::SPECTRUM_BINS]> {
        None
    }

    /// Whether anyone is drawing this node's display right now.
    ///
    /// **Called from the callback, on every node the effect host holds,
    /// every block, before `process`**, with whether the engine's telemetry
    /// has this stage subscribed -- and with `false` where there is no
    /// telemetry at all, as in an export. So an implementation must be a
    /// store and nothing more: no allocation, no work.
    ///
    /// It gates display work only. A saved "display on" setting is what the
    /// window *asks* for; this is whether the window is asking now. A node
    /// that runs analysis for its own display (the preamp, MOO-233) runs it
    /// only while both are true -- and nothing audible may depend on it,
    /// since an export is never subscribed.
    fn set_display_subscribed(&mut self, _subscribed: bool) {}

    /// Tell this node that time stopped being continuous.
    ///
    /// Called once per discontinuity, before the node is handed the block's
    /// events and before its `process` -- so a node may reset here and then
    /// receive a note-on at offset 0 in the same block, deterministically, in
    /// that order. Every node the host holds is told, **sleeping and
    /// bypassed ones included**: a node that is not being called is exactly
    /// the one whose rings still hold audio from where the transport was, and
    /// it would emit it on waking.
    ///
    /// The default does nothing, which is right for most devices and is the
    /// only correct default: a node that has not opted in behaves exactly as
    /// it did before this existed.
    ///
    /// Before it, the host had one way of saying anything to a node --
    /// synthesising `Event::Choke` into its event list -- so *let go of these
    /// notes* and *time moved* arrived as the same sentence. Voices heard it
    /// and guessed, differently (`release_all` in the synths, a hard fade in
    /// the samplers), and **everything that is not a voice heard nothing at
    /// all**: a delay line's contents, a reverb's tail and a splice position
    /// carried across a seek as though the audio in them still belonged where
    /// the transport now is.
    ///
    /// What a node may do here:
    ///
    /// - **No allocation and no lock.** This is the callback.
    /// - **Free-running state keeps running.** The rest-and-tail rule applies
    ///   unchanged: an LFO that holds its phase across silence holds it
    ///   across a seek too, or a bounce stops matching a take. A node wanting
    ///   that gets it by not implementing this.
    /// - **Read the kind.** A seek invalidates the audio a node is holding; a
    ///   program change and a loop fold do not, and flushing a reverb because
    ///   the player looked at another pattern, or because the loop came round
    ///   again, would be a worse artefact than the one this exists to fix.
    ///   [`Discontinuity::invalidates_tails`] is the rule, written once.
    /// - **A node that cannot honour it declines in writing**, the way Aux In
    ///   and the retained-audio buffer decline rest-and-tail in their own
    ///   comments rather than by omission.
    ///
    /// `docs/plans/transport-discontinuity/03-a-discontinuity-is-a-node-contract.md`.
    fn on_discontinuity(&mut self, kind: Discontinuity) {
        let _ = kind;
    }

    /// Move whatever runs whether or not this node is called, over a block
    /// of `ctx.frames` the host decided not to hand over.
    ///
    /// The host calls this exactly once, in place of `process`, for every
    /// block it skips. The default does nothing, which is right for almost
    /// every device: by the time a node reports itself at rest, everything
    /// driven by its input has stopped moving and freezing it is exact.
    ///
    /// What is left is state that advances on the clock rather than on the
    /// audio — a free-running LFO that keeps its phase across silence, a
    /// sample-and-hold's counter, a reverb line's modulation. That state has
    /// to arrive at the same place whether or not the node slept, or the
    /// device would sound different after a gap *and* how different would
    /// depend on the host's buffer size. Mirror the sample loop rather than
    /// closed-forming it, so the two paths agree to the bit.
    fn skip_block(&mut self, ctx: &ProcessContext) {
        let _ = ctx;
    }

    /// Process one block in place on `bus`. `events_in` is sorted by sample
    /// offset; nodes that respond to events must split rendering at those
    /// offsets. `events_out` is provided when the engine wants events back
    /// (metronomes, arpeggiators, MIDI-generating effects); most nodes
    /// ignore it.
    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        events_out: Option<&mut EventList>,
    );

    /// Deliver this block's driven parameters as curves rather than a step
    /// of `Event::ParamValue` events, once per block, before `process`.
    ///
    /// `curves` holds every destination the engine resolved this block --
    /// modulated and/or automated (`docs/MODULATION.md`'s base-plus-offset
    /// rule; the carrier is now a curve) -- and `tick_frames` is the control
    /// subdivision in frames the values were sampled at
    /// ([`crate::modulator::CONTROL_RATE_FRAMES`] today), handed over rather
    /// than assumed so a node's own tick math can never silently disagree
    /// with the engine's.
    ///
    /// **The default turns each curve into the same `Event::ParamValue`
    /// events the engine used to push directly onto this node's event list,
    /// timed at each tick's frame offset, so `process`'s ordinary
    /// event-driven path hears exactly what it always did.** A node that
    /// does not override this keeps working unchanged: this is the safe
    /// default the curve path is an addition on top of, never a removal of
    /// the working event path.
    ///
    /// A node with a native curve path overrides this instead: it reads
    /// `curves` directly -- typically driving a curve-aware split that
    /// calls its own `apply_param` once per tick per destination rather
    /// than once per event -- and leaves `fallback` untouched for the
    /// destinations it is handling itself, so `process` never
    /// double-applies them. [`crate::effects::CurveFrame`] is the shared
    /// helper an effect uses to keep its own copy of `curves` alive between
    /// this call and its next `process`, since the borrow here does not
    /// outlive the call.
    ///
    /// # A deliberate, documented deviation
    ///
    /// The design note that named this method wrote its signature as
    /// `apply_curves(&mut self, curves: &[ControlCurve], tick_frames:
    /// usize)`, with no way for a *default* implementation to deliver the
    /// events it promises: a default method sees only `&mut self`, and
    /// `process`'s `events_in` belongs to the caller, not to the node. The
    /// `fallback: &mut EventList` parameter is the minimal addition that
    /// makes the default above implementable at all; the destinations, the
    /// timing, and the "safe default" behaviour are exactly as specified.
    /// See `docs/plans/automation-curves/00-status.md`.
    ///
    /// Returns how many events `fallback` had no room for, so the engine can
    /// count them in `RenderState::refused_events` the way it counts its own
    /// pushes. An override that does not use `fallback` returns zero.
    fn apply_curves(
        &mut self,
        curves: &[ControlCurve<'_>],
        tick_frames: usize,
        fallback: &mut EventList,
    ) -> u64 {
        let stride = fallback_stride(curves, fallback.remaining());
        let mut refused = 0;
        for curve in curves {
            let last = curve.values.len().saturating_sub(1);
            for (tick, &value) in curve.values.iter().enumerate() {
                // Counted back from the last tick, so a thinned curve still
                // ends the block exactly where the lane or route put it.
                if (last - tick) % stride != 0 {
                    continue;
                }
                let offset = (tick * tick_frames) as u32;
                let event = match curve.kind {
                    CurveKind::Value => Event::ParamValue { id: curve.id, value },
                    CurveKind::Offset => Event::ParamMod {
                        id: curve.id,
                        amount: value,
                    },
                };
                if !fallback.push_ordered(TimedEvent { offset, event }) {
                    refused += 1;
                }
            }
        }
        refused
    }
}

/// Every how many ticks [`AudioNode::apply_curves`]'s default emits an event,
/// so that every curve fits in the `room` its fallback list has left.
///
/// One, which is every tick, whenever that fits -- the ordinary case, and the
/// only one before MOO-73. When it does not, a list that gave each destination
/// in turn every tick it asked for filled up part-way through one of them:
/// that destination froze mid-block and every one after it got nothing, which
/// at 512 frames took sixteen routes and eight lanes on one generator (the
/// fallback shares the channel's list with its notes). Thinning every curve
/// evenly instead keeps them all moving, at a coarser step, for the rare
/// block that asks for more than a list holds. A stride of one tick is 32
/// frames; the coarsest possible is one value per destination per block.
///
/// Leaves [`FALLBACK_HEADROOM`] free for whatever the engine pushes after the
/// control pass.
fn fallback_stride(curves: &[ControlCurve<'_>], room: usize) -> usize {
    let room = room.saturating_sub(FALLBACK_HEADROOM);
    let events_at = |stride: usize| -> usize {
        curves
            .iter()
            .map(|curve| curve.values.len().div_ceil(stride))
            .sum()
    };
    let longest = curves.iter().map(|curve| curve.values.len()).max().unwrap_or(0);
    let mut stride = 1;
    while stride < longest && events_at(stride) > room {
        stride += 1;
    }
    stride.max(1)
}

/// Events the default [`AudioNode::apply_curves`] leaves free in a list it
/// has to thin: a note-off or choke the engine adds after the control pass
/// still finds room.
const FALLBACK_HEADROOM: usize = 8;

/// A device that can be the source of a channel — the one call a channel
/// strip makes into whatever generator it is running.
///
/// [`AudioNode::process`] is the node vocabulary and stays exactly what it
/// is; this is the *host* vocabulary, and it exists because the strip needs
/// one signature rather than three. Before it, delivering a block to a
/// channel's generator was a closed `match` over the eight device kinds
/// whose arms called three different methods with three different argument
/// lists — which is fine while every source is native and in this repo, and
/// is precisely what stops the strip from ever holding a boxed one.
///
/// A supertrait of `AudioNode` rather than a restatement of it: the rest
/// contract (`tail_frames`, `is_at_rest`, `skip_block`) is inherited, so a
/// `&mut dyn SourceNode` answers those too and nothing has two places to say
/// when a device has finished.
///
/// The two extras are the union of what the eight need, and no more.
/// `events_out` is deliberately absent: no generator emits events through a
/// strip today, and a parameter for it would be inventing a capability
/// rather than preserving one. It comes back the day something needs it.
///
/// **The strip holds one of these, boxed, and nothing else** (MOO-56). It
/// used to hold all eight generators as fields and pick one with a tag, so
/// everything it did to a generator outside this trait was a `match` over the
/// eight. The methods below `publish_outlets_into` are those matches, moved
/// onto the device: which kind it is, its typed parameter block in and out,
/// its choke group, ML-P8's route depth, and the one device-specific handle
/// the host still needs, the sampler's. Each has the default a device that
/// lacks the feature would give, so a new source implements what it has.
pub trait SourceNode: AudioNode {
    /// Render one block as a channel's source.
    ///
    /// `source` is the auxiliary input an Aux In reads and the other seven
    /// ignore; `ports` is the tap group the two publishing devices fill.
    /// Both are supplied **for the duration of the call and not retained**,
    /// which is `AUDIO_ARCHITECTURE.md`'s rule for auxiliary buffers, and it
    /// is the reason they are parameters rather than fields.
    fn process_source(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        source: Option<&StereoBus>,
        ports: &mut AudioTaps<'_>,
    );

    /// Copy this block's control outlets into `out`, which the caller has
    /// already zeroed.
    ///
    /// Into a caller-owned band rather than returned, because the two
    /// devices that implement it publish runs of different widths — seven
    /// for ML-P8, six for DS-01 — so there is no one array type to return.
    /// Writing into the full band also keeps the zeroing at the call site,
    /// where the rule that the rest of the band is cleared belongs: replacing
    /// a source must not leave a route reading a signal from an instrument
    /// that is no longer there.
    ///
    /// The default publishes nothing, which is the same thing said for a
    /// device that has not implemented outlets yet.
    fn publish_outlets_into(&mut self, out: &mut [f32; MAX_GENERATOR_OUTLETS]) {
        let _ = out;
    }

    /// Which device this is.
    ///
    /// The strip asks the node rather than keeping a tag beside it: a tag
    /// and the box it describes are one fact written twice, and the first
    /// install that updated one and not the other would play one device
    /// while routing, choking and publishing as another.
    fn kind(&self) -> DeviceKind;

    /// Take a whole authored parameter block, as the concrete type's
    /// `set_params` does. Realtime-safe for every native kind.
    ///
    /// Returns whether the block was this device's. A block for another kind
    /// is refused and changes nothing -- which is what a command addressed
    /// to the channel's *previous* device is, once a source change has moved
    /// the slot on.
    fn set_generator_params(&mut self, params: &GeneratorParams) -> bool;

    /// The parameter block this device is running: what it was last sent,
    /// which after a control tick is base plus modulation, not the knob.
    fn generator_params(&self) -> GeneratorParams;

    /// The choke group a note on this device belongs to, or 0 for none.
    fn choke_group(&self) -> u8 {
        0
    }

    /// Move one internal modulation route's depth without recompiling the
    /// route table. Only a device with internal routes has anything to move.
    fn set_route_amount(&mut self, route: u16, amount: f32) {
        let _ = (route, amount);
    }

    /// This device as the sampler, for the host's five sampler-only calls:
    /// the channel's audio slot, its retired buffers, its stretch pool and
    /// its playheads. A typed handle rather than five trait methods, because
    /// all five are about the sample a *channel* owns, which no other kind
    /// has.
    fn as_sampler(&self) -> Option<&Sampler> {
        None
    }

    /// [`Self::as_sampler`], mutably.
    fn as_sampler_mut(&mut self) -> Option<&mut Sampler> {
        None
    }

    /// Put `node` -- a hosted plugin's processor, or `None` to pull the
    /// running one back -- into this source, if it is the hosted source for
    /// plugin `slot` (MOO-84). `Ok` carries the processor it displaced, which
    /// the caller must not drop on the audio thread; `Err` hands `node`
    /// back untouched, which is every native source's answer and a hosted
    /// source's for another slot. Moves one box; allocates nothing.
    fn host_processor(
        &mut self,
        slot: PluginSlotId,
        node: Option<HostedNode>,
    ) -> Result<Option<HostedNode>, Option<HostedNode>> {
        let _ = slot;
        Err(node)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arc_swap::ArcSwapOption;
    use mooloop_core::{
        DrumSynthParams, Ds01Params, MlM1Params, MlP8Chorus, MlP8Params, MonoSynthParams,
        PolySynthParams, SamplerParams,
    };

    use super::*;
    use crate::bus::StereoBus;
    use crate::drumsynth::DrumSynth;
    use crate::ds01::Ds01;
    use crate::event::{Event, EventList, TimedEvent};
    use crate::mlm1::MlM1;
    use crate::mlp8::MlP8;
    use crate::monosynth::MonoSynth;
    use crate::polysynth::PolySynth;
    use crate::sampler::{SampleData, Sampler};

    const SAMPLE_RATE: u32 = 48_000;
    const BLOCK: usize = 512;

    fn context(frames: usize) -> ProcessContext {
        ProcessContext {
            sample_rate: SAMPLE_RATE,
            frames,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        }
    }

    fn one(event: Event) -> EventList {
        let mut list = EventList::empty();
        list.push(TimedEvent { offset: 0, event });
        list
    }

    fn sampler() -> Sampler {
        let audio = Arc::new(ArcSwapOption::from(Some(Arc::new(
            crate::sampler::ChannelAudioSnapshot::sample(SampleData::default_kick(SAMPLE_RATE)),
        ))));
        Sampler::new(audio, SamplerParams::default(), SAMPLE_RATE)
    }

    fn generators() -> Vec<(&'static str, Box<dyn AudioNode + Send>)> {
        vec![
            ("Sampler", Box::new(sampler())),
            (
                "DrumSynth",
                Box::new(DrumSynth::new(DrumSynthParams::default(), SAMPLE_RATE)),
            ),
            (
                "MonoSynth",
                Box::new(MonoSynth::new(MonoSynthParams::default(), SAMPLE_RATE)),
            ),
            (
                "PolySynth",
                Box::new(PolySynth::new(PolySynthParams::default(), SAMPLE_RATE)),
            ),
            (
                "MlM1",
                Box::new(MlM1::new(MlM1Params::default(), SAMPLE_RATE)),
            ),
            (
                "MlP8",
                Box::new(MlP8::new(MlP8Params::default(), SAMPLE_RATE)),
            ),
            (
                "Ds01",
                Box::new(Ds01::new(Ds01Params::default(), SAMPLE_RATE)),
            ),
        ]
    }

    /// ML-P8's chorus is a delay line and an LFO on the output of the voice
    /// sum, both running on the clock, so a patch with it switched on stays
    /// awake between notes on purpose. Asserted rather than left implied,
    /// because the difference between "declines to sleep" and "forgot to say
    /// it could" is the whole of this contract.
    #[test]
    fn a_chorused_mlp8_declines_to_rest_between_notes() {
        let mut node = MlP8::new(
            MlP8Params {
                chorus: MlP8Chorus::Ensemble,
                ..MlP8Params::default()
            },
            SAMPLE_RATE,
        );
        let mut bus = StereoBus::with_capacity(BLOCK);
        let silence = EventList::empty();
        for _ in 0..(2 * SAMPLE_RATE as usize / BLOCK) {
            bus.clear(BLOCK);
            node.process(&context(BLOCK), &mut bus, &silence, None);
            assert!(!node.is_at_rest(), "a chorused ML-P8 reported itself at rest");
        }

        // And with it off, the same instrument sleeps: the difference is the
        // finisher, not the device.
        let mut node = MlP8::new(MlP8Params::default(), SAMPLE_RATE);
        bus.clear(BLOCK);
        node.process(&context(BLOCK), &mut bus, &silence, None);
        assert!(node.is_at_rest());
    }

    /// **A seek empties ML-P8's chorus line.** The hook reaches the outer
    /// device and stopped there: the chorus is a `ModulationEffect`, which
    /// clears its own line on a seek when it is a standalone effect, and
    /// nothing forwarded the call to the one inside ML-P8. Because a chorused
    /// ML-P8 deliberately keeps running between notes, whatever was in the
    /// line before the seek rang out across it
    /// (`reports/fable-2026-09-21.md`, finding 4).
    ///
    /// On the unfixed tree the post-seek block is the chorus playing the note
    /// that is no longer being held, and this fails.
    #[test]
    fn a_seek_empties_a_chorused_mlp8s_line() {
        let mut node = MlP8::new(
            MlP8Params {
                chorus: MlP8Chorus::Ensemble,
                ..MlP8Params::default()
            },
            SAMPLE_RATE,
        );
        let mut bus = StereoBus::with_capacity(BLOCK);

        let mut events = EventList::empty();
        events.push_ordered(TimedEvent {
            offset: 0,
            event: Event::NoteOn {
                id: 1,
                note: 60,
                velocity: 100,
            },
        });
        bus.clear(BLOCK);
        node.process(&context(BLOCK), &mut bus, &events, None);

        let silence = EventList::empty();
        for _ in 0..8 {
            bus.clear(BLOCK);
            node.process(&context(BLOCK), &mut bus, &silence, None);
        }
        let mut choke = EventList::empty();
        choke.push_ordered(TimedEvent {
            offset: 0,
            event: Event::Choke,
        });
        bus.clear(BLOCK);
        node.process(&context(BLOCK), &mut bus, &choke, None);
        let sounding = bus.l.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
        assert!(
            sounding > 1e-3,
            "the chorus line held nothing to clear, so this test proves \
             nothing: {sounding}"
        );

        node.on_discontinuity(Discontinuity::Seek);
        bus.clear(BLOCK);
        node.process(&context(BLOCK), &mut bus, &silence, None);
        let peak = bus.l.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
        assert!(
            peak <= 1e-6,
            "the chorus line kept audio from before the seek: {peak}"
        );
    }

    /// Play a note, let go of it, and keep rendering until the device says it
    /// has nothing left; then keep rendering anyway.
    ///
    /// The engine skips a whole channel strip on this answer, so a generator
    /// that says it is at rest while a voice is still fading takes its own
    /// release off the end of the note. The cap is twenty seconds -- long
    /// enough for any release the default patches can ask for, short enough
    /// that a device which never rests fails rather than hangs.
    #[test]
    fn every_generator_is_silent_once_it_says_it_is_at_rest() {
        const CAP_BLOCKS: usize = 20 * SAMPLE_RATE as usize / BLOCK;

        for (name, mut node) in generators() {
            let mut bus = StereoBus::with_capacity(BLOCK);
            let silence = EventList::empty();

            bus.clear(BLOCK);
            node.process(
                &context(BLOCK),
                &mut bus,
                &one(Event::NoteOn {
                    id: 1,
                    note: 60,
                    velocity: 110,
                }),
                None,
            );
            let (left, right) = bus.peak(BLOCK);
            assert!(
                left.max(right) > SILENCE_PEAK,
                "{name} made no sound at all, so this test proves nothing about it"
            );
            for _ in 0..4 {
                bus.clear(BLOCK);
                node.process(&context(BLOCK), &mut bus, &silence, None);
            }
            bus.clear(BLOCK);
            node.process(
                &context(BLOCK),
                &mut bus,
                &one(Event::NoteOff { id: 1, note: 60 }),
                None,
            );

            // The engine's own rule, in miniature: no events to answer, the
            // device's state settled, and its output quiet for longer than
            // the tail it declared.
            let mut silent_frames = 0u32;
            let mut rested_at = None;
            for block in 0..CAP_BLOCKS {
                if node.is_at_rest() && silent_frames > node.tail_frames() {
                    rested_at = Some(block);
                    break;
                }
                bus.clear(BLOCK);
                node.process(&context(BLOCK), &mut bus, &silence, None);
                let (left, right) = bus.peak(BLOCK);
                if left.max(right) <= SILENCE_PEAK {
                    silent_frames = silent_frames.saturating_add(BLOCK as u32);
                } else {
                    silent_frames = 0;
                }
            }
            let rested_at =
                rested_at.unwrap_or_else(|| panic!("{name} never came to rest in twenty seconds"));

            for block in 0..64 {
                bus.clear(BLOCK);
                node.process(&context(BLOCK), &mut bus, &silence, None);
                let (left, right) = bus.peak(BLOCK);
                assert!(
                    left.max(right) <= SILENCE_PEAK,
                    "{name} came to rest after {rested_at} blocks, then produced {} \
                     on block {block} after that",
                    left.max(right)
                );
            }
        }
    }

    /// A feedback loop that does not lose anything never goes quiet, and
    /// saying so is what stops a host from cutting one off. Anything below
    /// unity has to answer with a finite number of frames.
    #[test]
    fn a_lossless_feedback_loop_reports_an_unbounded_tail() {
        assert_eq!(feedback_tail_frames(1.0, 480.0), u32::MAX);
        assert_eq!(feedback_tail_frames(1.5, 480.0), u32::MAX);
        assert_eq!(feedback_tail_frames(f32::NAN, 480.0), u32::MAX);
        // No feedback is still one trip through the line.
        assert_eq!(feedback_tail_frames(0.0, 480.0), 480);
        // Half per trip is 23.3 trips to fall below `SILENCE_PEAK`; the
        // helper adds one and rounds up, so the answer is never short.
        assert_eq!(feedback_tail_frames(0.5, 100.0), 2426);
        // Sign is irrelevant: a phase-inverting loop decays at the same rate.
        assert_eq!(
            feedback_tail_frames(-0.5, 100.0),
            feedback_tail_frames(0.5, 100.0)
        );
    }

    /// `AudioNode::apply_curves`'s default: for every generator here, none
    /// of which override it, a curve must turn into exactly the
    /// `Event::ParamValue` step a caller could have pushed by hand -- "a
    /// node that does not override this keeps working unchanged" is the
    /// whole of the safe-default contract, checked against the fallback
    /// list directly rather than trusted from the doc comment.
    #[test]
    fn the_default_apply_curves_reconstructs_the_events_a_caller_would_have_pushed() {
        for (name, mut node) in generators() {
            let values = [1.0f32, -1.0, 0.5];
            let curves = [ControlCurve { id: 7, values: &values, kind: CurveKind::Value }];
            let mut fallback = EventList::empty();
            node.apply_curves(&curves, 32, &mut fallback);

            let got: Vec<(u32, TimedEvent)> = fallback
                .iter()
                .enumerate()
                .map(|(i, ev)| (i as u32, *ev))
                .collect();
            assert_eq!(got.len(), values.len(), "{name}: wrong number of fallback events");
            for (index, (_, ev)) in got.iter().enumerate() {
                assert_eq!(ev.offset, (index * 32) as u32, "{name}: tick {index}'s offset");
                assert_eq!(
                    ev.event,
                    Event::ParamValue { id: 7, value: values[index] },
                    "{name}: tick {index}'s value"
                );
            }
        }
    }

    /// An offset curve -- a route on a hosted plugin's parameter (MOO-82) --
    /// comes out of the default as `Event::ParamMod` at the same ticks a
    /// value curve would, and a value curve beside it is untouched.
    #[test]
    fn the_default_apply_curves_turns_an_offset_curve_into_param_mods() {
        let (_, mut node) = generators().into_iter().next().expect("a generator");
        let (lane, offsets) = ([0.25f32, 0.5], [-3.0f32, 3.0]);
        let curves = [
            ControlCurve { id: 4_000_000_000, values: &lane, kind: CurveKind::Value },
            ControlCurve { id: 4_000_000_000, values: &offsets, kind: CurveKind::Offset },
        ];
        let mut fallback = EventList::empty();
        assert_eq!(node.apply_curves(&curves, 32, &mut fallback), 0);
        let events: Vec<TimedEvent> = fallback.iter().copied().collect();
        assert_eq!(
            events,
            [
                TimedEvent { offset: 0, event: Event::ParamValue { id: 4_000_000_000, value: 0.25 } },
                TimedEvent { offset: 0, event: Event::ParamMod { id: 4_000_000_000, amount: -3.0 } },
                TimedEvent { offset: 32, event: Event::ParamValue { id: 4_000_000_000, value: 0.5 } },
                TimedEvent { offset: 32, event: Event::ParamMod { id: 4_000_000_000, amount: 3.0 } },
            ]
        );
    }

    /// A plugin parameter's normalized value is linear across its range,
    /// a stepped one lands on a whole position, and an offset is the same
    /// fraction of the range.
    #[test]
    fn a_hosted_parameter_converts_normalized_values_to_its_plain_units() {
        let cutoff = HostedParam {
            min: 10.0,
            max: 20_010.0,
            steps: None,
            automatable: true,
            modulatable: true,
        };
        assert_eq!(cutoff.plain(0.0), 10.0);
        assert_eq!(cutoff.plain(0.5), 10_010.0);
        assert_eq!(cutoff.plain(2.0), 20_010.0, "clamped into the range");
        assert_eq!(cutoff.plain_offset(-0.25), -5_000.0);
        let mode = HostedParam { min: 0.0, max: 3.0, steps: Some(4), ..cutoff };
        assert_eq!(mode.plain(0.4), 1.0);
        assert_eq!(mode.plain(0.6), 2.0);
    }

    /// MOO-73: sixteen routes and eight lanes on one generator at 512
    /// frames ask the default fallback for 24 x 16 = 384 events, into a list
    /// that holds 256 and already carries the channel's notes. Every
    /// destination has to keep moving and end the block where its curve
    /// does. Before the fix the list filled part-way through the sixteenth
    /// destination, which froze there, and the eight after it got nothing.
    #[test]
    fn the_default_fallback_thins_rather_than_starving_later_destinations() {
        const DESTINATIONS: usize = 24;
        const TICKS: usize = 512 / 32;
        let rows: Vec<Vec<f32>> = (0..DESTINATIONS)
            .map(|d| (0..TICKS).map(|t| (d * 100 + t) as f32).collect())
            .collect();
        let curves: Vec<ControlCurve<'_>> = rows
            .iter()
            .enumerate()
            .map(|(d, values)| ControlCurve {
                id: d as u32,
                values,
                kind: CurveKind::Value,
            })
            .collect();

        for (name, mut node) in generators() {
            let mut fallback = EventList::empty();
            // A handful of notes already scheduled, as the engine has them.
            for note in 0..10u8 {
                assert!(fallback.push_ordered(TimedEvent {
                    offset: u32::from(note) * 40,
                    event: Event::NoteOn { id: u64::from(note), note: 60 + note, velocity: 100 },
                }));
            }
            let refused = node.apply_curves(&curves, 32, &mut fallback);
            assert_eq!(refused, 0, "{name}: the fallback refused events");
            assert_eq!(fallback.refused(), 0, "{name}");
            assert!(fallback.remaining() >= FALLBACK_HEADROOM, "{name}: no headroom left");
            assert_eq!(
                fallback
                    .iter()
                    .filter(|event| matches!(event.event, Event::NoteOn { .. }))
                    .count(),
                10,
                "{name}: a note lost its place"
            );

            for (d, values) in rows.iter().enumerate() {
                let mine: Vec<&TimedEvent> = fallback
                    .iter()
                    .filter(|event| {
                        matches!(event.event, Event::ParamValue { id, .. } if id == d as u32)
                    })
                    .collect();
                assert!(mine.len() >= 2, "{name}: destination {d} got {} events", mine.len());
                let last = mine.last().expect("checked above");
                assert_eq!(last.offset, ((TICKS - 1) * 32) as u32, "{name}: destination {d}");
                assert_eq!(
                    last.event,
                    Event::ParamValue { id: d as u32, value: values[TICKS - 1] },
                    "{name}: destination {d} did not end where its curve does"
                );
            }
        }
    }

    /// A node that overrides `apply_curves` must not also have the default
    /// fallback run -- there is exactly one trait method here, so an
    /// override replaces the default rather than composing with it, but
    /// this pins the observable half of that: nothing an override chooses
    /// to leave `fallback` untouched about should appear in it.
    #[test]
    fn a_curve_consuming_override_leaves_the_fallback_list_untouched() {
        struct Records {
            captured: Vec<(u32, Vec<f32>)>,
        }
        impl AudioNode for Records {
            fn process(
                &mut self,
                _ctx: &ProcessContext,
                _bus: &mut StereoBus,
                _events_in: &EventList,
                _events_out: Option<&mut EventList>,
            ) {
            }
            fn apply_curves(
                &mut self,
                curves: &[ControlCurve<'_>],
                _tick_frames: usize,
                _fallback: &mut EventList,
            ) -> u64 {
                self.captured = curves
                    .iter()
                    .map(|c| (c.id, c.values.to_vec()))
                    .collect();
                0
            }
        }
        let mut node = Records { captured: Vec::new() };
        let values = [2.0f32, 3.0];
        let curves = [ControlCurve { id: 11, values: &values, kind: CurveKind::Value }];
        let mut fallback = EventList::empty();
        node.apply_curves(&curves, 32, &mut fallback);
        assert!(fallback.is_empty(), "an override must not also get the default's events");
        assert_eq!(node.captured, vec![(11, vec![2.0, 3.0])]);
    }
}
