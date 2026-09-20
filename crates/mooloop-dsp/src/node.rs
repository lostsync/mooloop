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
use crate::event::EventList;
use crate::taps::AudioTaps;
use mooloop_core::modulation::MAX_GENERATOR_OUTLETS;

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
    /// The playhead moved somewhere it was not travelling towards: a seek, or
    /// a loop fold. Audio a node is holding belongs to the old position and
    /// is now wrong -- this is the one that invalidates a delay line.
    Seek,
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
    ///   program change does not, and flushing a reverb because the player
    ///   looked at another pattern would be a worse artefact than the one
    ///   this exists to fix.
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
}

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
}
