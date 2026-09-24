//! Retained-audio buffer device core.
//!
//! **A ring that is always recording, and four ways to hear it instead of the
//! input.** The device is deliberately independent of project and UI plumbing:
//! construction allocates the ring, [`process`] is allocation-free and works
//! in place on an existing [`StereoBus`].
//!
//! Rewritten 2026-09-16, the day it was first played. What it replaced was a
//! turntable: one read head that `Position`, `Rate` and `Loop` fought over
//! under an arbitration rule, `Position` aiming a *chase* whose closing speed
//! became the playback rate, and the face's buttons implemented as macros
//! writing those shared knobs. It needed a chase time constant, an arrival
//! test and a stillness test to work out when an edit had finished, and the
//! buttons could not be given settings of their own without taking them from
//! each other. Adam, having played it: *"i dont understand what is difficult.
//! its a buffer."*
//!
//! So: a gesture computes two sample indices when it fires and plays between
//! them. Nothing chases anything. The whole arbitration rule is
//! [`BufferDevice::head`]'s match, in priority order, and it is eleven lines.

use mooloop_core::{BufferDuration, BufferEvent, BufferParams};

use crate::{AudioNode, Discontinuity, Event, EventList, ProcessContext, StereoBus};

/// A parameter change placed at a sample offset within a process block. The
/// buffer takes these separately from [`TimedBufferEvent`] because the two are
/// different in kind: an event is a gesture that creates a head, a parameter
/// is a standing value the head reads.
#[derive(Clone, Copy)]
pub struct TimedBufferParam {
    pub offset: u32,
    pub id: u32,
    pub value: f32,
}

/// A [`BufferEvent`] placed at a sample offset within a process block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimedBufferEvent {
    pub offset: u32,
    pub event: BufferEvent,
}

/// How many peaks the face draws the retained history as.
///
/// The memory *is* the instrument here, which is why the Buffer gets a
/// waveform where a synth does not -- but the ring is megabytes and the face
/// is a few hundred pixels, so what crosses to the GUI is a downsample rather
/// than PCM.
pub const WAVEFORM_BINS: usize = 256;

/// How far either side of the midpoint a switch has to travel before it
/// changes anything. A modulator resting on the threshold would otherwise
/// chatter the writer once per control tick, and every one of those is a
/// crossfade.
const SWITCH_HYSTERESIS: f32 = 0.05;

/// How much `Position` has to move between control ticks to count as moving.
///
/// **This is the whole of what makes `Position` an instrument rather than a
/// setting**: a playhead that is being driven is what you hear, and one
/// sitting still is somebody who has stopped playing, so the device falls
/// through to live audio. Adam: *"if it is moving, thats what we should hear,
/// otherwise its just live audio."*
///
/// Sixteen ulps at the top of the range, which is comfortably under the
/// slowest thing anybody would drive it with -- a sixteen-bar ramp moves
/// twenty times this per control tick -- and comfortably over the noise a
/// normalized round-trip through a descriptor leaves behind.
const POSITION_MOVED: f32 = 1e-6;

/// How long `Position` may sit still before the device hands back to live.
///
/// Four control ticks at the engine's rate. Long enough that a lane's
/// resolution cannot stutter it, short enough to be inaudible as a tail.
const POSITION_STILL_FRAMES: u32 = 128;

/// How fast `Position` may sweep before the device treats the move as an edit
/// and cuts instead.
///
/// The head *is* the position, so a control tick that asks it to travel a
/// long way is asking for a very fast sweep, and past a few times speed a
/// sweep stops sounding like one -- it is a zip, and then a click. A saw's
/// wrap is exactly this case and wants to be a seam. Above the limit the head
/// cuts to the new position under the crossfade instead of racing to it.
const MAX_SWEEP_RATE: f64 = 4.0;

/// Which gesture is holding the head.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Gesture {
    Jump,
    Reverse,
    Stutter,
    /// A [`BufferEvent`] from the MIDI map, which carries its own geometry.
    Event,
}

/// What the head is following, in priority order. A later variant never
/// displaces an earlier one; that is the entire arbitration rule.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Source {
    /// A held gesture: JUMP, REVERSE, STUTTER, or a mapped event.
    Gesture(Gesture),
    /// `Position`, while it is moving.
    Position,
    /// Frozen with nothing else playing: the ring goes round.
    Frozen,
}

/// A read head, and the region it stays inside.
///
/// One shape for all four sources, because they differ only in where they
/// start, which way they go and how far they may wander -- which is what
/// "play the samples between two numbers" means once it is written down.
#[derive(Clone, Copy)]
struct Head {
    source: Source,
    /// Absolute ring position, in frames. Fractional: the interpolator reads
    /// between samples.
    position: f64,
    /// Frames travelled per frame. `±1` for a gesture; for `Position` it is
    /// whatever the playhead's own motion implies, which is why nothing on
    /// the face has to name a rate.
    step: f64,
    /// The region the head wraps inside, as absolute positions. A gesture
    /// that repeats sets this to its own length; everything else gets the
    /// whole ring, which is what makes a held JUMP lap rather than stop.
    region: (f64, f64),
}

impl Head {
    /// Wrap back inside the region. `rem_euclid` rather than a loop, because
    /// at four times speed over a four-frame ring one subtraction is not
    /// enough and a loop on the audio thread is not a thing to reason about
    /// later.
    fn wrapped(mut self) -> (Self, bool) {
        let (start, end) = self.region;
        let span = end - start;
        if span <= 0.0 {
            return (self, false);
        }
        let before = self.position;
        self.position = start + (self.position - start).rem_euclid(span);
        // A wrap of less than half a frame is rounding, not a seam.
        (self, (self.position - before).abs() > 0.5)
    }
}

#[derive(Clone, Copy)]
enum FadeSource {
    Live,
    Detached { position: f64, step: f64 },
}

#[derive(Clone, Copy)]
struct Fade {
    source: FadeSource,
    frame: u32,
    frames: u32,
}

/// A freeze or a thaw waiting for its boundary, when `Quantize` says it has
/// to wait.
///
/// **Its own slot, apart from the gestures' waits.** One shared slot meant a
/// gesture pressed while a freeze was waiting replaced it, and the freeze
/// never happened -- which a lane hid, by re-sending `Freeze` every control
/// tick and re-arming it, and a single press from the face did not.
#[derive(Clone, Copy)]
struct ArmedFreeze {
    freeze: bool,
    /// Frames still to wait. Counted down rather than compared against a
    /// transport position, because the position is only given at block start
    /// and the boundary can fall inside a block.
    frames_remaining: f64,
}

/// The three gates, in the order [`BufferDevice::gates`] holds them.
const GATES: [Gesture; 3] = [Gesture::Jump, Gesture::Reverse, Gesture::Stutter];

/// One opt-in stereo rolling history. `capacity_frames` is fixed after
/// construction; replacing it on a tempo/config change is a control-plane
/// operation, never something [`process`] attempts.
pub struct BufferDevice {
    left: Vec<f32>,
    right: Vec<f32>,
    write_head: u64,
    /// Whether the writer is running. A gesture stops it for as long as it is
    /// held, and `Freeze` stops it until it is turned off.
    ///
    /// A `bool` rather than an absent writer because the ring must keep its
    /// contents: freezing is latching the history, not discarding it.
    writing: bool,
    /// Whether the latching `Freeze` is on, as distinct from a gesture having
    /// stopped the writer for the moment. Releasing a gesture restarts the
    /// writer only if this is off.
    frozen: bool,
    head: Option<Head>,
    fade: Option<Fade>,
    armed_freeze: Option<ArmedFreeze>,
    /// Frames each pressed gate still has to wait for its boundary, in
    /// [`GATES`] order. A gate that is held and waiting is not yet held as far
    /// as [`Self::held_gesture`] is concerned.
    gate_waits: [Option<f64>; 3],

    // --- standing settings, in the units the descriptors publish ----------
    crossfade_ms: f32,
    /// Where `Position` last asked the playhead to be, normalized over its
    /// span.
    position: f32,
    /// The last `Position` the device *acted* on, so a repeat can be told
    /// from a change.
    position_was: f32,
    /// Frames since `Position` last changed. What tells a playhead somebody
    /// is driving from one somebody has left alone.
    position_still: u32,
    /// Frames since the last `Position` write, so a move can be turned into
    /// a speed without anybody declaring the control rate.
    position_age: u32,
    position_span_index: f32,
    jump_back_index: f32,
    stutter_length_index: f32,
    quantize: bool,
    quant_start_index: f32,
    /// Which gestures are currently held, in [`GATES`] order.
    gates: [bool; 3],

    /// Peak magnitude per bin over the retained history, for the face to
    /// draw. Accumulated as the writer goes, one `max` per frame into the bin
    /// the write head is in, and reset when the writer first enters a bin.
    peaks: [f32; WAVEFORM_BINS],
    /// Frames processed, whatever the writer is doing.
    frames_elapsed: u64,
    /// Seams: a head wrapping its region, or a `Position` that moved too far
    /// to sweep and had to cut. Cheap observable diagnostics for tests and
    /// the face without logging on the RT thread.
    seam_count: u64,
    /// Settings carried in from the saved parameter set and applied on the
    /// first block, because construction has no [`ProcessContext`] and cannot
    /// work out how long a crossfade is.
    pending_freeze: bool,
    pending_gates: [bool; 3],
}

impl BufferDevice {
    pub fn new(params: BufferParams, sample_rate: u32, bpm: f64) -> Self {
        let mut device = Self::with_bars(sample_rate, bpm, u32::from(params.bars.max(1)));
        device.crossfade_ms = params.crossfade_ms.clamp(0.0, 50.0);
        device.position = params.position.clamp(0.0, 1.0);
        device.position_was = device.position;
        device.position_span_index = params.position_span;
        device.jump_back_index = params.jump_back;
        device.stutter_length_index = params.stutter_length;
        device.quantize = params.quantize >= 0.5;
        device.quant_start_index = params.quant_start;
        device.pending_freeze = params.freeze >= 0.5;
        device.pending_gates = [
            params.jump >= 0.5,
            params.reverse >= 0.5,
            params.stutter >= 0.5,
        ];
        device
    }

    /// Allocate a ring for `bars` bars at the supplied tempo.
    ///
    /// The bar length is [`mooloop_core::frames_per_bar`], which the sampler
    /// uses too. `ceil` and the floor of four frames stay: the ring must not
    /// shrink on a rounding change, and [`Self::with_capacity`] has a floor
    /// for a reason. Construction time, not `process`, so the float is free.
    pub fn with_bars(sample_rate: u32, bpm: f64, bars: u32) -> Self {
        let frames_per_bar = mooloop_core::frames_per_bar(sample_rate, bpm).ceil() as usize;
        Self::with_capacity((frames_per_bar * bars.max(1) as usize).max(4))
    }

    /// Allocate an explicit number of frames. Primarily useful for tests and
    /// prepared engine allocations.
    pub fn with_capacity(capacity_frames: usize) -> Self {
        let capacity_frames = capacity_frames.max(4);
        let defaults = BufferParams::default();
        Self {
            left: vec![0.0; capacity_frames],
            right: vec![0.0; capacity_frames],
            write_head: 0,
            writing: true,
            frozen: false,
            head: None,
            fade: None,
            armed_freeze: None,
            gate_waits: [None; 3],
            crossfade_ms: defaults.crossfade_ms,
            position: defaults.position,
            position_was: defaults.position,
            position_still: POSITION_STILL_FRAMES,
            position_age: 1,
            position_span_index: defaults.position_span,
            jump_back_index: defaults.jump_back,
            stutter_length_index: defaults.stutter_length,
            quantize: defaults.quantize >= 0.5,
            quant_start_index: defaults.quant_start,
            gates: [false; 3],
            peaks: [0.0; WAVEFORM_BINS],
            frames_elapsed: 0,
            seam_count: 0,
            pending_freeze: false,
            pending_gates: [false; 3],
        }
    }

    pub fn capacity_frames(&self) -> usize {
        self.left.len()
    }

    pub fn memory_bytes(&self) -> usize {
        self.capacity_frames() * 2 * std::mem::size_of::<f32>()
    }

    /// Wraps and cuts since construction. Not an error count -- a stutter
    /// makes one per repeat by design -- but it is the number that says
    /// whether the head is doing what the face is drawing.
    pub fn seam_count(&self) -> u64 {
        self.seam_count
    }

    /// Whether the device is passing its input through untouched. A direct
    /// assignment from the input sample, so it is bit-identical and zero
    /// latency even at startup and across wraparound.
    pub fn is_following(&self) -> bool {
        self.head.is_none()
    }

    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    pub fn is_writing(&self) -> bool {
        self.writing
    }

    /// Process one block. Events must be in ascending sample-offset order.
    pub fn process(
        &mut self,
        context: &ProcessContext,
        bus: &mut StereoBus,
        events: &[TimedBufferEvent],
    ) {
        self.process_with_params(context, bus, events, &[]);
    }

    /// As [`Self::process`], with parameter changes applied at their own
    /// sample offsets. Both slices must be in ascending offset order.
    pub fn process_with_params(
        &mut self,
        context: &ProcessContext,
        bus: &mut StereoBus,
        events: &[TimedBufferEvent],
        params: &[TimedBufferParam],
    ) {
        debug_assert!(context.frames <= bus.capacity());
        // A non-finite sample would stay in the ring for as long as it is
        // retained, and every gesture over it would replay it (MOO-176). One
        // pass over the block finds it, and it is stored, and passed, as
        // silence.
        let frames = context.frames.min(bus.capacity());
        let finite = |samples: &[f32]| samples.iter().all(|sample| sample.is_finite());
        if !(finite(&bus.l[..frames]) && finite(&bus.r[..frames])) {
            for sample in bus.l[..frames].iter_mut().chain(&mut bus.r[..frames]) {
                if !sample.is_finite() {
                    *sample = 0.0;
                }
            }
        }
        if std::mem::take(&mut self.pending_freeze) {
            // A saved freeze is a state the document was in, not a gesture
            // somebody just made, so it is restored rather than quantized --
            // **once there is something to hold** (MOO-137). The ring's audio
            // is not saved, so a device built with Freeze on (a reopened
            // song, a preset, a paste, an undo that rebuilt it) starts empty,
            // and latching that played silence under a face reading FROZEN.
            // It records instead, with the freeze armed to land the moment
            // the ring holds a full history. A ring that already does -- one
            // that adopted another's history -- latches at once. Whether the
            // frozen audio itself should be saved is MOO-196.
            let unfilled = (self.capacity_frames() as u64).saturating_sub(self.write_head);
            if unfilled == 0 {
                self.apply_freeze(true);
                self.reconsider(context);
            } else {
                self.armed_freeze = Some(ArmedFreeze {
                    freeze: true,
                    frames_remaining: unfilled as f64,
                });
            }
        }
        for (index, held) in std::mem::take(&mut self.pending_gates).into_iter().enumerate() {
            if held {
                self.gates[index] = true;
                self.start(index, 0, context);
            }
        }

        let mut event_index = 0;
        let mut param_index = 0;
        for frame in 0..context.frames {
            // Parameters first: a gesture arriving on the same frame as a
            // setting change should see the new setting, and a gesture owns
            // the head afterwards either way.
            //
            // **Settings before actions, whatever order they came in.** The
            // engine delivers one frame's values in descriptor-table order,
            // which puts the gates ahead of `Quant Start`, so a gate and a
            // new grid on the same tick waited on the old grid. The table
            // order is frozen -- the face addresses it by position -- so the
            // two passes are here instead.
            let due_from = param_index;
            while params
                .get(param_index)
                .is_some_and(|timed| timed.offset as usize == frame)
            {
                param_index += 1;
            }
            let due = &params[due_from..param_index];
            for acts in [false, true] {
                for timed in due.iter().filter(|timed| is_action(timed.id) == acts) {
                    self.set_param(timed.id, timed.value, frame, context);
                }
            }
            while let Some(timed) = events.get(event_index) {
                if timed.offset as usize != frame {
                    break;
                }
                self.fire(timed.event, context);
                event_index += 1;
            }

            let input_l = bus.l[frame];
            let input_r = bus.r[frame];
            // The writer stores the device input, never its output, and does
            // it whatever the read head is doing.
            if self.writing {
                let write_index = self.index(self.write_head as f64);
                self.left[write_index] = input_l;
                self.right[write_index] = input_r;
                self.accumulate_peak(write_index, input_l.abs().max(input_r.abs()));
            }

            let (mut output_l, mut output_r) = match self.head {
                Some(head) => self.read_stereo(head.position),
                None => (input_l, input_r),
            };

            if let Some(mut fade) = self.fade {
                let (from_l, from_r) = match fade.source {
                    FadeSource::Live => (input_l, input_r),
                    FadeSource::Detached { position, .. } => self.read_stereo(position),
                };
                let phase = fade.frame as f32 / fade.frames as f32;
                let (from_gain, to_gain) = crate::smooth::equal_power(phase);
                output_l = from_l * from_gain + output_l * to_gain;
                output_r = from_r * from_gain + output_r * to_gain;
                if let FadeSource::Detached { position, step } = &mut fade.source {
                    *position += *step;
                }
                fade.frame += 1;
                self.fade = (fade.frame < fade.frames).then_some(fade);
            }

            bus.l[frame] = output_l;
            bus.r[frame] = output_r;

            self.count_down(context);
            self.frames_elapsed += 1;
            if self.writing {
                self.write_head += 1;
            }
            self.advance(context);
        }
    }

    // --- the arbitration rule, entire --------------------------------------

    /// Install whichever head the device's state calls for, or none.
    ///
    /// **This is the whole arbitration rule.** A held gesture outranks a
    /// moving playhead, which outranks a frozen ring going round, which
    /// outranks the input. It is re-evaluated whenever one of those facts
    /// changes -- never per frame -- and each source, once installed, is left
    /// alone to advance.
    fn reconsider(&mut self, context: &ProcessContext) {
        let wanted = if let Some(gesture) = self.held_gesture() {
            Some(Source::Gesture(gesture))
        } else if self.position_still < POSITION_STILL_FRAMES {
            Some(Source::Position)
        } else if self.frozen {
            Some(Source::Frozen)
        } else {
            None
        };
        let current = self.head.map(|head| head.source);
        if current == wanted {
            return;
        }
        // An event head is a gesture the gates know nothing about, so it is
        // only ever displaced by its own release.
        if matches!(current, Some(Source::Gesture(Gesture::Event))) {
            return;
        }
        match wanted {
            Some(Source::Gesture(gesture)) => self.begin(gesture, context),
            Some(Source::Position) => self.begin_position(context),
            Some(Source::Frozen) => self.begin_frozen(context),
            None => self.return_live(),
        }
    }

    /// The gesture currently holding the head. Last one pressed wins, which
    /// is what a hand expects: reaching for STUTTER while JUMP is still down
    /// is a stutter, not a refusal.
    ///
    /// A gate still waiting for its boundary is not held yet: otherwise
    /// anything else that reconsidered in the meantime -- a freeze landing, a
    /// playhead let go of -- would start the gesture early.
    fn held_gesture(&self) -> Option<Gesture> {
        GATES
            .iter()
            .zip(self.gates)
            .zip(self.gate_waits)
            .rev()
            .find_map(|((gesture, held), wait)| (held && wait.is_none()).then_some(*gesture))
    }

    /// Start the gesture in `GATES[index]`, pressed `frame` frames into this
    /// block, waiting for the boundary if `Quantize` says so.
    fn start(&mut self, index: usize, frame: usize, context: &ProcessContext) {
        match self.boundary_wait(context, frame) {
            Some(frames) => self.gate_waits[index] = Some(frames),
            None => {
                self.gate_waits[index] = None;
                self.reconsider(context);
            }
        }
    }

    /// Count every waiting press down by a frame, and land the ones that are
    /// due.
    ///
    /// **A freeze lands before a gesture due on the same frame**, and the
    /// device reconsiders once, after both. `reconsider` ranks a held gesture
    /// above a frozen ring, so the result is the gesture either way; freezing
    /// first is what makes it a gesture *over the frozen ring*, so that
    /// letting go of it hands the head to the ring rather than to the input
    /// for a frame, and the writer never restarts in between.
    fn count_down(&mut self, context: &ProcessContext) {
        let mut landed = false;
        if let Some(mut armed) = self.armed_freeze {
            armed.frames_remaining -= 1.0;
            if armed.frames_remaining <= 0.0 {
                self.armed_freeze = None;
                self.apply_freeze(armed.freeze);
                landed = true;
            } else {
                self.armed_freeze = Some(armed);
            }
        }
        for wait in &mut self.gate_waits {
            let Some(frames) = wait else { continue };
            *frames -= 1.0;
            if *frames <= 0.0 {
                *wait = None;
                landed = true;
            }
        }
        if landed {
            self.reconsider(context);
        }
    }

    /// Frames from `frame` in this block to the next `Quant Start` boundary,
    /// or `None` when there is nothing to wait for -- quantize off, the
    /// transport stopped, a grid of no length, or a boundary already reached.
    fn boundary_wait(&self, context: &ProcessContext, frame: usize) -> Option<f64> {
        if !self.quantize || !context.playing {
            return None;
        }
        let division = mooloop_core::ModTimeDivision::from_index(self.quant_start_index as i32);
        let grid_beats = f64::from(division.beats());
        if !grid_beats.is_finite() || grid_beats <= 0.0 || !context.position_ticks.is_finite() {
            return None;
        }
        let ticks_per_beat = f64::from(mooloop_core::Ppq::DEFAULT.ticks_per_beat());
        let beats_now = context.position_ticks / ticks_per_beat;
        let frames_per_beat = frames_per_beat(context);
        // `position_ticks` is the block's *start*, and the request arrived
        // `frame` frames into it -- possibly past a boundary inside the block,
        // so the next boundary is found from the press, not the block start.
        let beats_at_press = beats_now + frame as f64 / frames_per_beat;
        let next = (beats_at_press / grid_beats).floor() * grid_beats + grid_beats;
        // Measured from the block start and then shortened, rather than from
        // `beats_at_press`, so a press on frame 0 waits exactly what it did.
        let frames = (next - beats_now) * frames_per_beat - frame as f64;
        (frames > 0.0).then_some(frames)
    }

    // --- the four heads ----------------------------------------------------

    /// JUMP, REVERSE and STUTTER, as two indices and a direction.
    ///
    /// Each owns its own settings and borrows nothing. JUMP lands
    /// `Jump Back` behind the write position and runs forward over the whole
    /// ring; REVERSE starts at the write position and runs backward over the
    /// whole ring; STUTTER is JUMP with its region closed to one
    /// `Stutter Length`, which is the only difference between the two.
    fn begin(&mut self, gesture: Gesture, context: &ProcessContext) {
        let now = self.write_head as f64;
        let span = self.capacity_frames() as f64;
        let whole_ring = (now - span, now);
        let (position, step, region) = match gesture {
            Gesture::Jump => {
                let back = self.division_frames(self.jump_back_index, context);
                (now - back, 1.0, whole_ring)
            }
            // `now` is where the writer is *about* to put a frame, so the
            // newest one it has actually written is one behind it. Starting
            // on `now` reads the oldest sample in the ring instead of the
            // newest, which is a whole buffer's worth of wrong.
            Gesture::Reverse => (now - 1.0, -1.0, whole_ring),
            Gesture::Stutter => {
                let length = self.division_frames(self.stutter_length_index, context);
                (now - length, 1.0, (now - length, now))
            }
            // Only ever reached through `fire`, which builds its own head.
            Gesture::Event => return,
        };
        // A gesture holds the ring still for as long as it is down: the press
        // is a freeze, which is what makes what you hear a fixed sample
        // rather than a moving window.
        self.writing = false;
        self.install(
            Head {
                source: Source::Gesture(gesture),
                position,
                step,
                region,
            },
            context,
        );
    }

    /// The playhead, while somebody is driving it. It starts where `Position`
    /// points and is then moved by `Position` rather than by a rate.
    fn begin_position(&mut self, context: &ProcessContext) {
        self.install(
            Head {
                source: Source::Position,
                position: self.position_target(self.position, context),
                step: 0.0,
                // Its own span, so a saw over a shortened one loops that much
                // rather than running out into the rest of the ring.
                region: self.position_region(context),
            },
            context,
        );
    }

    /// Frozen and left alone: the ring goes round, forward, at unity.
    ///
    /// That is what makes a freeze leave you holding a loop rather than a
    /// still, and it is why the freeze is quantized -- the chunk only joins
    /// up when its ends are on the same grid the music is.
    fn begin_frozen(&mut self, context: &ProcessContext) {
        // `now - span`, not `now`. They are the same ring index -- the frame
        // after the newest is the oldest -- but the region is half-open and
        // excludes `now`, so installing there wraps on the very first frame
        // and reports a seam that nothing audible happened at.
        let (start, _) = self.whole_ring();
        self.install(
            Head {
                source: Source::Frozen,
                position: start,
                step: 1.0,
                region: self.whole_ring(),
            },
            context,
        );
    }

    /// Put a head in, crossfading from whatever was playing.
    fn install(&mut self, head: Head, context: &ProcessContext) {
        let frames = ms_to_frames(self.crossfade_ms, context.sample_rate);
        self.fade = (frames > 0).then_some(Fade {
            source: match self.head {
                Some(old) => FadeSource::Detached {
                    position: old.position,
                    step: old.step,
                },
                None => FadeSource::Live,
            },
            frame: 0,
            frames,
        });
        self.head = Some(head);
    }

    /// Hand the output back to the input, crossfading out of whatever was
    /// playing. The writer restarts unless the latching `Freeze` is on.
    fn return_live(&mut self) {
        if let Some(head) = self.head {
            let frames = self.fade.map_or(0, |fade| fade.frames);
            let frames = frames.max(self.crossfade_frames_hint());
            if frames > 0 {
                self.fade = Some(Fade {
                    source: FadeSource::Detached {
                        position: head.position,
                        step: head.step,
                    },
                    frame: 0,
                    frames,
                });
            }
        }
        self.head = None;
        if !self.frozen {
            self.writing = true;
        }
    }

    /// The crossfade length in frames at the sample rate the last head was
    /// built with. Held rather than recomputed because `return_live` is
    /// reachable from [`Self::release`], which has no `ProcessContext`.
    fn crossfade_frames_hint(&self) -> u32 {
        // 48 kHz is the only rate the engine runs at; a wrong guess here
        // costs a millisecond of fade length, never a click.
        ms_to_frames(self.crossfade_ms, 48_000)
    }

    // --- advancing ---------------------------------------------------------

    fn advance(&mut self, context: &ProcessContext) {
        // `Position`'s stillness is counted in frames rather than in writes,
        // so a lane that has stopped changing is still even while it keeps
        // arriving.
        self.position_still = self.position_still.saturating_add(1);
        self.position_age = self.position_age.saturating_add(1);
        if self.position_still == POSITION_STILL_FRAMES
            && matches!(self.head.map(|head| head.source), Some(Source::Position))
        {
            // The playhead has been let go of. Whatever is underneath it --
            // a frozen ring, or the input -- takes over.
            self.reconsider(context);
        }
        let Some(head) = &mut self.head else { return };
        head.position += head.step;
        let (wrapped, seam) = head.wrapped();
        *head = wrapped;
        if seam {
            let old = head.position - head.step;
            let step = head.step;
            self.seam_count += 1;
            self.crossfade_from(old, step);
        }
    }

    /// Crossfade from a position the head has just left. A stutter's repeat
    /// and a reverse head lapping the ring are both this.
    fn crossfade_from(&mut self, position: f64, step: f64) {
        let frames = self.crossfade_frames_hint();
        if frames == 0 {
            return;
        }
        self.fade = Some(Fade {
            source: FadeSource::Detached { position, step },
            frame: 0,
            frames,
        });
    }

    // --- settings ----------------------------------------------------------

    fn set_param(&mut self, id: u32, value: f32, frame: usize, context: &ProcessContext) {
        match id {
            mooloop_core::BUFFER_PARAM_POSITION => self.set_position(value, context),
            mooloop_core::BUFFER_PARAM_POSITION_SPAN => self.position_span_index = value,
            mooloop_core::BUFFER_PARAM_CROSSFADE_MS => {
                self.crossfade_ms = value.clamp(0.0, 50.0)
            }
            mooloop_core::BUFFER_PARAM_JUMP_BACK => self.jump_back_index = value,
            mooloop_core::BUFFER_PARAM_STUTTER_LENGTH => self.stutter_length_index = value,
            mooloop_core::BUFFER_PARAM_QUANTIZE => self.quantize = value >= 0.5,
            mooloop_core::BUFFER_PARAM_QUANT_START => self.quant_start_index = value,
            mooloop_core::BUFFER_PARAM_FREEZE => {
                if value >= 0.5 + SWITCH_HYSTERESIS {
                    self.request_freeze(true, frame, context);
                } else if value <= 0.5 - SWITCH_HYSTERESIS {
                    self.request_freeze(false, frame, context);
                }
            }
            mooloop_core::BUFFER_PARAM_JUMP => self.set_gate(0, value, frame, context),
            mooloop_core::BUFFER_PARAM_REVERSE => self.set_gate(1, value, frame, context),
            mooloop_core::BUFFER_PARAM_STUTTER => self.set_gate(2, value, frame, context),
            _ => {}
        }
    }

    /// A gesture gate, with hysteresis for the same reason `Freeze` has it.
    fn set_gate(&mut self, index: usize, value: f32, frame: usize, context: &ProcessContext) {
        let held = if value >= 0.5 + SWITCH_HYSTERESIS {
            true
        } else if value <= 0.5 - SWITCH_HYSTERESIS {
            false
        } else {
            return;
        };
        if self.gates[index] == held {
            return;
        }
        self.gates[index] = held;
        if held {
            self.start(index, frame, context);
            return;
        }
        // Releases are never quantized: a gesture ends when the hand says so,
        // and a press still waiting for its boundary is taken back rather
        // than fired late.
        self.gate_waits[index] = None;
        self.reconsider(context);
    }

    /// Move the playhead.
    ///
    /// The head **is** the position: the step is whatever it takes to get
    /// there by the time the next write arrives, which makes the playback
    /// rate the position's own speed and means nothing on the face has to
    /// name one. A one-bar ramp over a one-bar span plays at unity.
    fn set_position(&mut self, value: f32, context: &ProcessContext) {
        let value = value.clamp(0.0, 1.0);
        let moved = (value - self.position_was).abs() > POSITION_MOVED;
        self.position = value;
        let age = self.position_age.max(1);
        self.position_age = 0;
        if !moved {
            return;
        }
        self.position_was = value;
        let was_still = self.position_still >= POSITION_STILL_FRAMES;
        self.position_still = 0;
        if self.held_gesture().is_some()
            || matches!(self.head.map(|head| head.source), Some(Source::Gesture(_)))
        {
            // A gesture outranks the playhead. The position re-asserts itself
            // on the next write after the gesture ends.
            return;
        }
        if was_still {
            self.reconsider(context);
            return;
        }
        let Some(head) = self.head.filter(|head| head.source == Source::Position) else {
            return;
        };
        let target = self.position_target(value, context);
        let step = (target - head.position) / f64::from(age);
        if step.abs() > MAX_SWEEP_RATE {
            // Too far to sweep, so it is an edit: cut, and let the crossfade
            // make it a seam. A saw's wrap is exactly this.
            self.seam_count += 1;
            self.crossfade_from(head.position, head.step);
            self.head = Some(Head {
                position: target,
                // Carry the speed across the seam rather than restarting from
                // nothing, so a saw's playback rate is continuous through its
                // own wrap instead of stalling for one control tick.
                step: head.step,
                ..head
            });
            return;
        }
        self.head = Some(Head {
            position: head.position,
            step,
            ..head
        });
    }

    /// Ask to freeze or return to live, on the boundary if `Quantize` says
    /// so.
    ///
    /// **Asking for the opposite of what is waiting takes the request back.**
    /// A musician who changed their mind should not have to wait out a bar to
    /// find out. **Asking again for what is already waiting is not a second
    /// press**: `Freeze` is a latching value rather than a trigger, so a lane
    /// holding it writes the same 1.0 every control tick, and treating each
    /// of those as a press would arm, cancel, arm, cancel and never land.
    fn request_freeze(&mut self, freeze: bool, frame: usize, context: &ProcessContext) {
        if let Some(armed) = self.armed_freeze {
            if armed.freeze != freeze {
                self.armed_freeze = None;
            }
            return;
        }
        if freeze == self.frozen {
            return;
        }
        match self.boundary_wait(context, frame) {
            Some(frames) => {
                self.armed_freeze = Some(ArmedFreeze {
                    freeze,
                    frames_remaining: frames,
                })
            }
            None => {
                self.apply_freeze(freeze);
                self.reconsider(context);
            }
        }
    }

    fn apply_freeze(&mut self, freeze: bool) {
        self.frozen = freeze;
        // A gesture is holding the writer down on its own account; freezing
        // and thawing under one must not restart it.
        if self.held_gesture().is_none() {
            self.writing = !freeze;
        }
    }

    /// Latch the ring, or let it go, without the boundary a parameter write
    /// waits for. The control plane and tests; nothing in the audio path,
    /// which goes through `Freeze` like everything else.
    pub fn set_frozen(&mut self, frozen: bool) {
        self.apply_freeze(frozen);
    }

    /// Whether a freeze or a thaw is waiting for its boundary, and which.
    ///
    /// The face needs it: while this is `Some` the device is still in the
    /// state it was, and a control that looked as though nothing had happened
    /// would be the whole gesture failing silently.
    pub fn armed_freeze(&self) -> Option<bool> {
        self.armed_freeze.map(|armed| armed.freeze)
    }

    /// Whether a gesture press is waiting for its boundary.
    pub fn armed_gesture(&self) -> bool {
        self.gate_waits.iter().any(Option::is_some)
    }

    // --- geometry ----------------------------------------------------------

    fn whole_ring(&self) -> (f64, f64) {
        let now = self.write_head as f64;
        (now - self.capacity_frames() as f64, now)
    }

    /// How long a `ModTimeDivision` index is in frames, never longer than the
    /// ring: a window longer than the ring is not a window, it is the ring.
    fn division_frames(&self, index: f32, context: &ProcessContext) -> f64 {
        let division = mooloop_core::ModTimeDivision::from_index(index as i32);
        let frames = f64::from(division.beats()) * frames_per_beat(context);
        frames.clamp(1.0, self.capacity_frames() as f64)
    }

    /// The region `Position` addresses.
    ///
    /// At the full span -- the default -- this is **the ring itself, in the
    /// ring's own coordinates**, which is the same map the waveform is drawn
    /// in: bin zero is ring index zero, so pointing at a peak on the face and
    /// hearing that peak are the same act.
    ///
    /// It matters more than it looks. A region anchored to the write head
    /// travels at a frame per frame, so a playhead sweeping it is moving at
    /// its own speed *plus* the writer's -- a one-bar ramp over a one-bar ring
    /// would play at double speed, and there would be no setting that gave
    /// unity. Addressing the ring, a one-bar ramp over a one-bar ring is
    /// unity exactly, which is what makes "put a one-measure saw on it"
    /// behave the way anybody would expect.
    ///
    /// A *shortened* span is the other case and wants the other anchor: "the
    /// last quarter" has to follow the writer or it stops being the last
    /// quarter within a bar.
    fn position_region(&self, context: &ProcessContext) -> (f64, f64) {
        if self.position_span_index >= mooloop_core::BUFFER_SPAN_FULL - 0.5 {
            return (0.0, self.capacity_frames() as f64);
        }
        let now = self.write_head as f64;
        (now - self.division_frames(self.position_span_index, context), now)
    }

    /// Where a normalized playhead points, in absolute frames.
    ///
    /// The top of the span is the newest frame in it, which is one behind the
    /// write head -- `now` itself is where the next frame goes, and reading it
    /// gets whatever was there a whole ring ago.
    fn position_target(&self, position: f32, context: &ProcessContext) -> f64 {
        let (start, end) = self.position_region(context);
        start + f64::from(position.clamp(0.0, 1.0)) * (end - 1.0 - start)
    }

    // --- the mapped-event path --------------------------------------------

    /// One [`BufferEvent`] from the MIDI map.
    ///
    /// The map is a parallel control surface that predates the gestures and
    /// still carries its own geometry, so an event builds its own head rather
    /// than borrowing a gate's settings. It is the one path that can ask for
    /// a playback speed other than `±1`, because `BufferEvent::rate` is a
    /// field it has always had.
    fn fire(&mut self, event: BufferEvent, context: &ProcessContext) {
        let frames_per_beat = frames_per_beat(context);
        let position = self.write_head as f64 + f64::from(event.offset_beats) * frames_per_beat;
        // A window always covers material the head is about to play, so it
        // extends backward from the entry point for a reverse head and
        // forward for a forward one.
        let region = match event
            .window_beats
            .filter(|beats| *beats > 0.0)
            .map(|beats| f64::from(beats) * frames_per_beat)
        {
            Some(length) if event.rate < 0.0 => (position - length, position),
            Some(length) => (position, position + length),
            None => self.whole_ring(),
        };
        self.writing = false;
        self.crossfade_ms = event.crossfade_ms.clamp(0.0, 50.0);
        self.install(
            Head {
                source: Source::Gesture(Gesture::Event),
                position,
                step: f64::from(event.rate),
                region,
            },
            context,
        );
        let _ = BufferDuration::Gate;
    }

    /// End a mapped event's gesture. A gate the device is holding on its own
    /// account is left alone: a held control sends this on release without
    /// knowing whether its own event is still the one running.
    pub fn release(&mut self) {
        if matches!(
            self.head.map(|head| head.source),
            Some(Source::Gesture(Gesture::Event))
        ) {
            self.return_live();
        }
    }

    /// Take over what `previous` has retained: its most recent history, as
    /// much of it as this ring holds, and where its writer is (MOO-137).
    ///
    /// For a replacement ring -- a tempo change or a HISTORY change builds a
    /// new device at the new length -- so the audio the player has been
    /// capturing is still there to be played. Until this, a tempo change
    /// gave every unfrozen Buffer an empty ring.
    ///
    /// Realtime-safe: two copies per channel, bounded by the smaller ring,
    /// and a pass over this ring's picture. No allocation.
    ///
    /// What is taken is the *history*. A gesture or a playhead the previous
    /// device was running is not: the replacement starts following, with
    /// whatever its own parameters say is held, which is what it was built
    /// from.
    pub fn adopt_history_from(&mut self, previous: &BufferDevice) {
        let frames = self
            .capacity_frames()
            .min(previous.capacity_frames())
            .min(usize::try_from(previous.write_head).unwrap_or(usize::MAX));
        let end = previous.write_head;
        let mut copied = 0usize;
        while copied < frames {
            let position = end - (frames - copied) as u64;
            let from = (position % previous.capacity_frames() as u64) as usize;
            let to = (position % self.capacity_frames() as u64) as usize;
            // The longest run that wraps neither ring.
            let run = (frames - copied)
                .min(previous.capacity_frames() - from)
                .min(self.capacity_frames() - to);
            self.left[to..to + run].copy_from_slice(&previous.left[from..from + run]);
            self.right[to..to + run].copy_from_slice(&previous.right[from..from + run]);
            copied += run;
        }
        self.write_head = previous.write_head;
        self.frames_elapsed = previous.frames_elapsed;
        self.seam_count = previous.seam_count;
        self.rebuild_peaks();
    }

    /// The picture, redrawn from the ring as it stands.
    fn rebuild_peaks(&mut self) {
        let capacity = self.capacity_frames();
        for (bin, peak) in self.peaks.iter_mut().enumerate() {
            let start = bin * capacity / WAVEFORM_BINS;
            let end = ((bin + 1) * capacity / WAVEFORM_BINS).max(start + 1).min(capacity);
            *peak = self.left[start..end]
                .iter()
                .zip(&self.right[start..end])
                .fold(0.0f32, |peak, (l, r)| peak.max(l.abs()).max(r.abs()));
        }
    }

    // --- telemetry ---------------------------------------------------------

    /// Fold one frame's magnitude into the bin it belongs to.
    ///
    /// The bin is reset on the writer's *first* frame in it, which is what
    /// makes the picture a rolling history rather than an ever-rising
    /// envelope. Integer arithmetic and one branch: this runs once a frame.
    fn accumulate_peak(&mut self, write_index: usize, magnitude: f32) {
        let capacity = self.capacity_frames();
        let bin = (write_index * WAVEFORM_BINS / capacity.max(1)).min(WAVEFORM_BINS - 1);
        // Entering the bin resets it, and inside it the peak only rises --
        // one condition rather than two, because "start again" and "this is
        // louder" both mean "take this sample".
        let first_frame_in_bin = write_index * WAVEFORM_BINS % capacity.max(1) < WAVEFORM_BINS;
        if first_frame_in_bin || magnitude > self.peaks[bin] {
            self.peaks[bin] = magnitude;
        }
    }

    /// Peak magnitude per bin over the retained history, oldest bin first.
    ///
    /// Bin zero is ring index zero rather than the oldest *sample*: the ring
    /// is a fixed array and the writer walks it, so the picture is stable and
    /// the head marker moves over it. A picture that rotated under a
    /// stationary head would be unreadable.
    pub fn waveform_peaks(&self) -> &[f32; WAVEFORM_BINS] {
        &self.peaks
    }

    /// Where the read head is, as a fraction of the ring. `None` when the
    /// device is following, because there is no head to draw.
    pub fn head_fraction(&self) -> Option<f32> {
        let capacity = self.capacity_frames() as f64;
        self.head
            .map(|head| (self.index(head.position) as f64 / capacity) as f32)
    }

    /// Where the writer is, as a fraction of the ring. This is "now", and it
    /// stops moving whenever the writer does.
    pub fn write_fraction(&self) -> f32 {
        let capacity = self.capacity_frames() as f64;
        (self.index(self.write_head as f64) as f64 / capacity) as f32
    }

    /// The region the head is confined to, as fractions of the ring, or
    /// `None` when that is the whole of it -- a band drawn round everything
    /// says nothing.
    pub fn region_fractions(&self) -> Option<(f32, f32)> {
        let capacity = self.capacity_frames() as f64;
        let head = self.head?;
        let (start, end) = head.region;
        if end - start >= capacity - 1.0 {
            return None;
        }
        Some((
            (self.index(start) as f64 / capacity) as f32,
            (self.index(end) as f64 / capacity) as f32,
        ))
    }

    // --- reading -----------------------------------------------------------

    fn index(&self, position: f64) -> usize {
        position.floor().rem_euclid(self.capacity_frames() as f64) as usize
    }

    /// Four-point, third-order Hermite interpolation. The history ring is
    /// planar, so the same fractional position is used for both channels.
    fn read_stereo(&self, position: f64) -> (f32, f32) {
        (
            self.read_channel(&self.left, position),
            self.read_channel(&self.right, position),
        )
    }

    fn read_channel(&self, channel: &[f32], position: f64) -> f32 {
        let base = position.floor();
        let t = (position - base) as f32;
        let ym1 = channel[self.index(base - 1.0)];
        let y0 = channel[self.index(base)];
        let y1 = channel[self.index(base + 1.0)];
        let y2 = channel[self.index(base + 2.0)];
        let c0 = y0;
        let c1 = 0.5 * (y1 - ym1);
        let c2 = ym1 - 2.5 * y0 + 2.0 * y1 - 0.5 * y2;
        let c3 = 0.5 * (y2 - ym1) + 1.5 * (y0 - y1);
        ((c3 * t + c2) * t + c1) * t + c0
    }
}

/// What a Buffer face draws, gathered in one borrow.
///
/// Display telemetry, and therefore **never** an input to an audio node --
/// `02-control-and-modulation.md` is explicit that a musical control signal
/// belongs in the modulator path where it gets a declared rate and latency,
/// and this has neither. It is the latest available picture and nothing more.
pub struct BufferDisplay<'a> {
    pub peaks: &'a [f32; WAVEFORM_BINS],
    /// Where the read head is, as a fraction of the ring. `None` while
    /// following, because there is no head to draw.
    pub head: Option<f32>,
    pub write: f32,
    /// The region a gesture is confined to, when that is less than the ring.
    pub region: Option<(f32, f32)>,
    pub frozen: bool,
    /// `Some(true)` when a freeze is waiting for its boundary, `Some(false)`
    /// for a thaw.
    pub armed_freeze: Option<bool>,
    /// Whether a gesture press is waiting for its boundary. The face has to
    /// show this: until it lands nothing has happened, and a button that
    /// looked inert would be the gesture failing silently.
    pub armed_gesture: bool,
}

/// Stable identity for a buffer allocation configuration. The tempo is
/// intentionally absent: a tempo resize replaces the same logical device;
/// only a bars change makes an older prepared resize stale.
pub fn buffer_allocation_key(params: BufferParams) -> u64 {
    u64::from(params.bars.max(1))
}

/// Whether a parameter *does* something when it arrives -- moves the head,
/// latches the ring, presses a gate -- rather than configuring what the next
/// such thing will do. See the two passes in
/// [`BufferDevice::process_with_params`].
const fn is_action(id: u32) -> bool {
    matches!(
        id,
        mooloop_core::BUFFER_PARAM_POSITION
            | mooloop_core::BUFFER_PARAM_FREEZE
            | mooloop_core::BUFFER_PARAM_JUMP
            | mooloop_core::BUFFER_PARAM_REVERSE
            | mooloop_core::BUFFER_PARAM_STUTTER
    )
}

fn frames_per_beat(context: &ProcessContext) -> f64 {
    context.sample_rate as f64 * 60.0 / context.bpm.max(1.0)
}

fn ms_to_frames(ms: f32, sample_rate: u32) -> u32 {
    (ms.max(0.0) * sample_rate as f32 / 1_000.0).round() as u32
}

/// Deliberately no `is_at_rest` or `tail_frames`: a retained-audio buffer
/// plays back what it captured, so it is the one device in the rack that
/// makes sound out of a silent input by design. It keeps the default — never
/// skipped — and that is a decision rather than an omission.
impl AudioNode for BufferDevice {
    /// **Declined, deliberately.** The ring is not audio in flight from the
    /// old position -- it is audio somebody is playing, and while frozen it
    /// is a sample rather than a moving window. Clearing it on a seek would
    /// take a performance away mid-gesture, which is the same thing
    /// `holds_frozen_audio` refuses a ring resize to prevent.
    ///
    /// The heads are left alone for the same reason: they are where the
    /// player put them, not where the transport was.
    fn on_discontinuity(&mut self, kind: Discontinuity) {
        let _ = kind;
    }

    /// Seams rather than collisions. The old number counted a detached head
    /// being overtaken by its writer, which was a failure the turntable model
    /// could have; every head here wraps instead, so what is worth reporting
    /// is how often it did.
    fn buffer_collisions(&self) -> u64 {
        self.seam_count
    }

    fn holds_frozen_audio(&self) -> bool {
        self.is_frozen()
    }

    fn as_buffer_device_mut(&mut self) -> Option<&mut BufferDevice> {
        Some(self)
    }

    fn buffer_waveform(&self) -> Option<BufferDisplay<'_>> {
        Some(BufferDisplay {
            peaks: self.waveform_peaks(),
            head: self.head_fraction(),
            write: self.write_fraction(),
            region: self.region_fractions(),
            frozen: self.is_frozen(),
            armed_freeze: self.armed_freeze(),
            armed_gesture: self.armed_gesture(),
        })
    }

    fn process(
        &mut self,
        context: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        const EMPTY: TimedBufferEvent = TimedBufferEvent {
            offset: 0,
            event: BufferEvent::live(),
        };
        const EMPTY_PARAM: TimedBufferParam = TimedBufferParam {
            offset: 0,
            id: 0,
            value: 0.0,
        };
        let mut buffer_events = [EMPTY; 256];
        let mut len = 0;
        let mut param_events = [EMPTY_PARAM; 256];
        let mut param_len = 0;
        for timed in events_in.iter() {
            match timed.event {
                Event::ParamValue { id, value } => {
                    if param_len < param_events.len() {
                        param_events[param_len] = TimedBufferParam {
                            offset: timed.offset,
                            id,
                            value,
                        };
                        param_len += 1;
                    }
                }
                Event::Buffer(event) => {
                    if len == buffer_events.len() {
                        continue;
                    }
                    buffer_events[len] = TimedBufferEvent {
                        offset: timed.offset,
                        event,
                    };
                    len += 1;
                }
                // Releases are rare and land between blocks, so they are
                // applied at the block edge rather than threaded through the
                // sample loop as a second timed stream.
                Event::BufferRelease => self.release(),
                _ => {}
            }
        }
        self.process_with_params(
            context,
            bus,
            &buffer_events[..len],
            &param_events[..param_len],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{
        BUFFER_PARAM_CROSSFADE_MS, BUFFER_PARAM_FREEZE, BUFFER_PARAM_JUMP,
        BUFFER_PARAM_POSITION, BUFFER_PARAM_POSITION_SPAN, BUFFER_PARAM_QUANTIZE,
        BUFFER_PARAM_QUANT_START, BUFFER_PARAM_REVERSE, BUFFER_PARAM_STUTTER,
        BUFFER_PARAM_STUTTER_LENGTH,
    };

    /// At 120 BPM and 48 kHz: a beat is 24 000 frames, a bar is 96 000, and a
    /// sixteenth is 6 000. Every number below is one of those.
    const BEAT: usize = 24_000;
    const BAR: usize = 96_000;
    const SIXTEENTH: usize = 6_000;

    /// The transport is **stopped**, so there is no grid for a press to wait
    /// for and it lands at once. The quantize tests use [`playing`] and say
    /// so; every other test here is about geometry, not timing.
    fn context(frames: usize) -> ProcessContext {
        ProcessContext {
            sample_rate: 48_000,
            frames,
            playing: false,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        }
    }

    fn playing(frames: usize, ticks: f64) -> ProcessContext {
        ProcessContext {
            playing: true,
            position_ticks: ticks,
            ..context(frames)
        }
    }

    fn param(offset: u32, id: u32, value: f32) -> TimedBufferParam {
        TimedBufferParam { offset, id, value }
    }

    /// No crossfade, so a test that asserts *where* the head is reads the
    /// sample it is on rather than a blend of two. `fill_ramp` writes each
    /// frame's own number, which makes the "audio" enormous DC -- an
    /// equal-power fade between 30 000 and 24 000 peaks at 38 000, higher
    /// than either, and the first loop test this device ever had read that as
    /// the window escaping.
    fn hard(id: u32, value: f32) -> [TimedBufferParam; 2] {
        [
            param(0, BUFFER_PARAM_CROSSFADE_MS, 0.0),
            param(0, id, value),
        ]
    }

    fn fill_ramp(bus: &mut StereoBus, first: usize, frames: usize) {
        for frame in 0..frames {
            bus.l[frame] = (first + frame) as f32;
            bus.r[frame] = -((first + frame) as f32);
        }
    }

    /// A one-bar ring, filled with a ramp so a read position can be named
    /// from the sample value alone. The writer ends on frame 96 000.
    fn primed() -> (BufferDevice, StereoBus) {
        let mut device = BufferDevice::with_capacity(BAR);
        // A bus a whole ring wide, so a test can render a full lap in one
        // block and see the wrap.
        let mut bus = StereoBus::with_capacity(BAR);
        for block in 0..2 {
            fill_ramp(&mut bus, block * BAR / 2, BAR / 2);
            device.process(&context(BAR / 2), &mut bus, &[]);
        }
        (device, bus)
    }

    #[test]
    fn a_fresh_device_passes_its_input_through() {
        let (mut device, mut bus) = primed();
        assert!(device.is_following());
        fill_ramp(&mut bus, BAR, 1_000);
        device.process(&context(1_000), &mut bus, &[]);
        assert_eq!(bus.l[999], (BAR + 999) as f32, "following is the input itself");
    }

    /// JUMP lands its own distance back and plays forward from there.
    #[test]
    fn jump_plays_forward_from_its_own_distance_back() {
        let (mut device, mut bus) = primed();
        fill_ramp(&mut bus, BAR, BEAT);
        // The default Jump Back is a quarter: one beat, 24 000 frames.
        device.process_with_params(
            &context(BEAT),
            &mut bus,
            &[],
            &hard(BUFFER_PARAM_JUMP, 1.0),
        );
        assert_eq!(bus.l[0], (BAR - BEAT) as f32, "a beat back, exactly");
        assert_eq!(bus.l[1_000], (BAR - BEAT + 1_000) as f32, "and forward at unity");
        assert!(!device.is_writing(), "a held gesture holds the ring still");
    }

    /// REVERSE runs backward from the newest frame and **keeps going**: it
    /// wraps the ring rather than ending, so a held button does not let go on
    /// its own a few seconds later.
    #[test]
    fn reverse_runs_backward_from_now_and_wraps_rather_than_stopping() {
        let (mut device, mut bus) = primed();
        fill_ramp(&mut bus, BAR, BAR);
        device.process_with_params(
            &context(BAR),
            &mut bus,
            &[],
            &hard(BUFFER_PARAM_REVERSE, 1.0),
        );
        assert_eq!(bus.l[0], (BAR - 1) as f32, "backward from the newest frame");
        assert_eq!(bus.l[1_000], (BAR - 1 - 1_000) as f32);
        // A whole ring later it has lapped and is still going.
        assert!(device.seam_count() > 0, "the test never reached the end of the ring");
        assert!(!device.is_following(), "a held REV has to keep reversing");
    }

    /// STUTTER repeats **its own** length.
    ///
    /// This is the question that started the rebuild -- Adam, of the version
    /// where the gesture overrode a shared `Length` knob and put it back on
    /// release: *"how does one set the stutter length?"*
    #[test]
    fn stutter_repeats_its_own_length() {
        let (mut device, mut bus) = primed();
        fill_ramp(&mut bus, BAR, BAR / 2);
        device.process_with_params(
            &context(BAR / 2),
            &mut bus,
            &[],
            &hard(BUFFER_PARAM_STUTTER, 1.0),
        );
        // The default is a sixteenth: the last 6 000 frames, over and over.
        assert_eq!(bus.l[0], (BAR - SIXTEENTH) as f32);
        assert_eq!(bus.l[SIXTEENTH - 1], (BAR - 1) as f32, "to the end of it");
        assert_eq!(bus.l[SIXTEENTH], (BAR - SIXTEENTH) as f32, "and round again");
        assert_eq!(bus.l[SIXTEENTH * 3], (BAR - SIXTEENTH) as f32);
        let _ = device;
    }

    /// And the two knobs are independent: moving the stutter's length does
    /// not move the jump's distance, which is the whole of what "they dont
    /// share any length settings" asks for.
    #[test]
    fn the_stutter_length_and_the_jump_distance_are_separate_settings() {
        let (mut device, mut bus) = primed();
        fill_ramp(&mut bus, BAR, BAR / 2);
        device.process_with_params(
            &context(BAR / 2),
            &mut bus,
            &[],
            &[
                param(0, BUFFER_PARAM_CROSSFADE_MS, 0.0),
                // An eighth, 12 000 frames -- not the sixteenth it defaults to.
                param(0, BUFFER_PARAM_STUTTER_LENGTH, 10.0),
                param(0, BUFFER_PARAM_STUTTER, 1.0),
            ],
        );
        assert_eq!(bus.l[0], (BAR - 2 * SIXTEENTH) as f32, "an eighth back");
        assert_eq!(bus.l[2 * SIXTEENTH], (BAR - 2 * SIXTEENTH) as f32, "repeating an eighth");
        assert_ne!(bus.l[SIXTEENTH], bus.l[0], "and not a sixteenth");

        // The jump is where its own knob left it, untouched by any of that.
        // The release gets a block of its own: it restarts the writer, and a
        // press in the same block would measure its distance from a write
        // head that had moved on by however many frames were between them.
        fill_ramp(&mut bus, BAR, 1_000);
        device.process_with_params(
            &context(1_000),
            &mut bus,
            &[],
            &[param(0, BUFFER_PARAM_STUTTER, 0.0)],
        );
        let now = BAR + 1_000;
        fill_ramp(&mut bus, now, 1_000);
        device.process_with_params(
            &context(1_000),
            &mut bus,
            &[],
            &hard(BUFFER_PARAM_JUMP, 1.0),
        );
        assert_eq!(bus.l[0], (now - BEAT) as f32, "still a quarter back");
    }

    /// Releasing hands the output back to the input and restarts the writer.
    #[test]
    fn releasing_a_gesture_returns_to_live_and_restarts_the_writer() {
        let (mut device, mut bus) = primed();
        fill_ramp(&mut bus, BAR, 1_000);
        device.process_with_params(
            &context(1_000),
            &mut bus,
            &[],
            &hard(BUFFER_PARAM_JUMP, 1.0),
        );
        assert!(!device.is_following() && !device.is_writing());
        fill_ramp(&mut bus, BAR, 1_000);
        device.process_with_params(
            &context(1_000),
            &mut bus,
            &[],
            &[param(0, BUFFER_PARAM_JUMP, 0.0)],
        );
        assert!(device.is_following(), "the button is up; the audio is live");
        assert!(device.is_writing(), "and the ring is recording again");
        assert_eq!(bus.l[999], (BAR + 999) as f32);
    }

    /// **A playhead nobody is moving is not a performance.** A held Position
    /// is a setting, and the device falls through to live audio -- Adam: *"if
    /// it is moving, thats what we should hear, otherwise its just live
    /// audio."*
    #[test]
    fn a_still_position_is_live_audio() {
        let (mut device, mut bus) = primed();
        fill_ramp(&mut bus, BAR, BAR / 2);
        // What a lane holding one value does: one write per control tick.
        let held: Vec<_> = (0..(BAR / 2) as u32 / 32)
            .map(|tick| param(tick * 32, BUFFER_PARAM_POSITION, 0.5))
            .collect();
        device.process_with_params(&context(BAR / 2), &mut bus, &[], &held);
        assert!(device.is_following(), "a position that stopped moving lets go");
        assert_eq!(
            bus.l[BAR / 2 - 1],
            (BAR + BAR / 2 - 1) as f32,
            "and what comes out is what went in"
        );
    }

    /// A moving one is the buffer, at whatever speed it is being moved.
    ///
    /// **A one-bar ramp over a one-bar ring plays at unity**, which is why
    /// nothing on the face names a rate: the head *is* the position, so the
    /// playback speed is the position's own speed. Half the period would be
    /// an octave up, and a descending ramp is reverse.
    #[test]
    fn a_one_bar_ramp_over_a_one_bar_ring_plays_at_unity() {
        let (mut device, mut bus) = primed();
        fill_ramp(&mut bus, BAR, BAR / 2);
        let swept: Vec<_> = (0..(BAR / 2) as u32 / 32)
            .map(|tick| {
                param(
                    tick * 32,
                    BUFFER_PARAM_POSITION,
                    (tick * 32) as f32 / BAR as f32,
                )
            })
            .collect();
        device.process_with_params(&context(BAR / 2), &mut bus, &[], &swept);
        assert!(!device.is_following(), "somebody is driving it");
        let span = &bus.l[20_000..20_010];
        for pair in span.windows(2) {
            assert!(
                (pair[1] - pair[0] - 1.0).abs() < 0.05,
                "a one-bar ramp over a one-bar ring is not unity: {span:?}"
            );
        }
    }

    /// The playhead addresses the ring, so pointing at a place on the drawn
    /// waveform and hearing that place are the same act.
    #[test]
    fn the_playhead_addresses_the_ring_the_waveform_is_drawn_in() {
        let (mut device, mut bus) = primed();
        fill_ramp(&mut bus, BAR, 8_000);
        // Driven throughout: a playhead has to keep moving to be heard, so a
        // couple of writes and then silence would correctly let go. A slow
        // creep across a quarter of the way along is the shape of the claim.
        let mut params = vec![param(0, BUFFER_PARAM_CROSSFADE_MS, 0.0)];
        params.extend((0..8_000u32 / 32).map(|tick| {
            param(
                tick * 32,
                BUFFER_PARAM_POSITION,
                0.24 + 0.02 * (tick * 32) as f32 / 8_000.0,
            )
        }));
        params.sort_by_key(|entry| entry.offset);
        device.process_with_params(&context(8_000), &mut bus, &[], &params);
        let heard = bus.l[4_000];
        let want = 0.25 * BAR as f32;
        assert!(
            (heard - want).abs() < 2_000.0,
            "a quarter of the way along a ring holding its own frame numbers \
             should read about {want}, and it read {heard}"
        );
    }

    /// A gesture outranks the playhead for as long as it is held, and the
    /// playhead takes over again on its next move.
    #[test]
    fn a_gesture_outranks_a_moving_position() {
        let (mut device, mut bus) = primed();
        fill_ramp(&mut bus, BAR, BAR / 2);
        let mut params = vec![param(0, BUFFER_PARAM_CROSSFADE_MS, 0.0)];
        params.extend((0..(BAR / 2) as u32 / 32).map(|tick| {
            param(tick * 32, BUFFER_PARAM_POSITION, (tick * 32) as f32 / BAR as f32)
        }));
        params.push(param(10_000, BUFFER_PARAM_STUTTER, 1.0));
        params.sort_by_key(|p| p.offset);
        device.process_with_params(&context(BAR / 2), &mut bus, &[], &params);
        // Past the press it is the stutter's region, not the sweep's.
        assert_eq!(bus.l[20_000], bus.l[20_000 + SIXTEENTH], "the stutter has it");
    }

    /// Frozen and left alone, the ring **plays** -- round and round, forward,
    /// at unity. Adam: *"the frozen audio should be a musically loopable
    /// chunk."* A freeze that left you holding a still would be a different
    /// device.
    #[test]
    fn freezing_leaves_a_loop_running() {
        let (mut device, mut bus) = primed();
        fill_ramp(&mut bus, BAR, BAR);
        device.process_with_params(
            &context(BAR),
            &mut bus,
            &[],
            &hard(BUFFER_PARAM_FREEZE, 1.0),
        );
        assert!(device.is_frozen() && !device.is_writing());
        assert!(!device.is_following(), "a frozen buffer plays");
        // Forward at unity from the freeze point, which is the oldest sample.
        assert_eq!(bus.l[1_000], 1_000.0);
        // And it comes round: a whole ring later it is back where it started.
        assert_eq!(bus.l[BAR - 1], (BAR - 1) as f32);
        // A ring's worth of frames is a ring's worth of advances, so it wraps
        // exactly once -- on the step after the last sample, which is the
        // seam the loop is made of.
        assert_eq!(device.seam_count(), 1, "one ring rendered is one lap");
    }

    /// A quantized press waits for the boundary, and the face is told so.
    #[test]
    fn a_quantized_press_waits_for_its_boundary() {
        let (mut device, mut bus) = primed();
        fill_ramp(&mut bus, BAR, BAR / 2);
        // Half a bar in, with a one-bar Quant Start: half a bar to wait.
        let half_bar_ticks = f64::from(mooloop_core::Ppq::DEFAULT.ticks_per_beat()) * 2.0;
        device.process_with_params(
            &playing(BAR / 2, half_bar_ticks),
            &mut bus,
            &[],
            &[
                param(0, BUFFER_PARAM_CROSSFADE_MS, 0.0),
                param(0, BUFFER_PARAM_QUANTIZE, 1.0),
                param(0, BUFFER_PARAM_JUMP, 1.0),
            ],
        );
        // It lands at the bar line, which is 48 000 frames from the block's
        // start -- the last frame of this block.
        assert_eq!(bus.l[1_000], (BAR + 1_000) as f32, "still live before the line");
        assert!(!device.is_following(), "and playing after it");
    }

    /// A press that arrives inside a block waits from **its own frame**, not
    /// from the block's start. It used to wait as though every press came on
    /// frame 0, so it landed late by its offset -- and a lane's press moved
    /// with the block size, which made an offline render depend on it.
    #[test]
    fn a_press_inside_a_block_waits_from_its_own_frame() {
        let (mut device, mut bus) = primed();
        fill_ramp(&mut bus, BAR, BAR / 2);
        // Half a bar in, with a one-bar Quant Start, pressed 10 000 frames
        // into the block: the line is 38 000 frames after the press, which is
        // the first frame of the next block.
        let half_bar_ticks = f64::from(mooloop_core::Ppq::DEFAULT.ticks_per_beat()) * 2.0;
        device.process_with_params(
            &playing(BAR / 2, half_bar_ticks),
            &mut bus,
            &[],
            &[
                param(0, BUFFER_PARAM_CROSSFADE_MS, 0.0),
                param(0, BUFFER_PARAM_QUANTIZE, 1.0),
                param(10_000, BUFFER_PARAM_JUMP, 1.0),
            ],
        );
        assert_eq!(
            bus.l[BAR / 2 - 1],
            (BAR + BAR / 2 - 1) as f32,
            "live up to the line"
        );
        assert!(!device.armed_gesture(), "the press landed on the line");
        assert!(!device.is_following(), "and the jump is playing");

        let live = BAR + BAR / 2;
        fill_ramp(&mut bus, live, 1_000);
        let bar_ticks = f64::from(mooloop_core::Ppq::DEFAULT.ticks_per_beat()) * 4.0;
        device.process(&playing(1_000, bar_ticks), &mut bus, &[]);
        // One beat back from the line, playing forward.
        assert_eq!(bus.l[0], (live - BEAT) as f32);
        assert_eq!(bus.l[999], (live - BEAT + 999) as f32);
    }

    /// A gesture pressed while a freeze waits for the same boundary does not
    /// take the freeze's place: both land, and the gesture plays over the
    /// frozen ring. One press each, the way the face sends them -- a lane
    /// would re-send `Freeze` every tick and hide the failure.
    #[test]
    fn a_gesture_pressed_while_a_freeze_waits_keeps_the_freeze() {
        let (mut device, mut bus) = primed();
        fill_ramp(&mut bus, BAR, BAR / 2);
        let half_bar_ticks = f64::from(mooloop_core::Ppq::DEFAULT.ticks_per_beat()) * 2.0;
        device.process_with_params(
            &playing(BAR / 2, half_bar_ticks),
            &mut bus,
            &[],
            &[
                param(0, BUFFER_PARAM_CROSSFADE_MS, 0.0),
                param(0, BUFFER_PARAM_QUANTIZE, 1.0),
                param(0, BUFFER_PARAM_FREEZE, 1.0),
                param(100, BUFFER_PARAM_JUMP, 1.0),
            ],
        );
        assert!(device.is_frozen(), "the freeze landed");
        assert_eq!(device.armed_freeze(), None);
        assert!(!device.armed_gesture(), "and so did the jump");
        assert!(!device.is_writing());

        // Let go of the jump: the frozen ring takes the head, not the input.
        let bar_ticks = f64::from(mooloop_core::Ppq::DEFAULT.ticks_per_beat()) * 4.0;
        fill_ramp(&mut bus, BAR + BAR / 2, 1_000);
        device.process_with_params(
            &playing(1_000, bar_ticks),
            &mut bus,
            &[],
            &[param(0, BUFFER_PARAM_JUMP, 0.0)],
        );
        assert!(device.is_frozen() && !device.is_writing());
        assert!(!device.is_following(), "the frozen ring plays");
    }

    /// A freeze that lands while a gesture is still waiting for a later
    /// boundary does not start the gesture early.
    #[test]
    fn a_landing_freeze_does_not_start_a_waiting_gesture() {
        let (mut device, mut bus) = primed();
        fill_ramp(&mut bus, BAR, BEAT);
        device.process_with_params(
            &playing(BEAT, 0.0),
            &mut bus,
            &[],
            &[
                param(0, BUFFER_PARAM_CROSSFADE_MS, 0.0),
                // On the bar line with a one-bar grid: the jump waits a bar.
                param(0, BUFFER_PARAM_QUANTIZE, 1.0),
                param(0, BUFFER_PARAM_JUMP, 1.0),
                // Quantize off before the freeze, so it lands at once.
                param(10, BUFFER_PARAM_QUANTIZE, 0.0),
                param(10, BUFFER_PARAM_FREEZE, 1.0),
            ],
        );
        assert!(device.is_frozen(), "the unquantized freeze landed at once");
        assert!(
            device.armed_gesture(),
            "the jump pressed under a one-bar grid is still waiting for it"
        );
        // The frozen ring is playing, not the jump. It starts at ring index
        // 10, the oldest frame, and from there the ring still holds the
        // primer's own frame numbers; a jump one beat back would be reading
        // this block's ramp instead.
        assert_eq!(bus.l[BEAT - 1], (BEAT - 1) as f32);
    }

    /// A setting and a gate on the same frame: the gate sees the setting,
    /// whatever order the two arrived in. The engine delivers them in
    /// descriptor-table order, which puts the gates first.
    #[test]
    fn a_gate_waits_on_a_grid_set_on_its_own_frame() {
        let (mut device, mut bus) = primed();
        fill_ramp(&mut bus, BAR, BAR / 2);
        // Half a bar in. The default grid is a bar, so the old order would
        // wait 48 000 frames; a quarter waits 24 000.
        let half_bar_ticks = f64::from(mooloop_core::Ppq::DEFAULT.ticks_per_beat()) * 2.0;
        device.process_with_params(
            &playing(BAR / 2, half_bar_ticks),
            &mut bus,
            &[],
            &[
                param(0, BUFFER_PARAM_CROSSFADE_MS, 0.0),
                param(0, BUFFER_PARAM_JUMP, 1.0),
                param(
                    0,
                    BUFFER_PARAM_QUANT_START,
                    mooloop_core::ModTimeDivision::Quarter.to_index() as f32,
                ),
            ],
        );
        assert_eq!(bus.l[BEAT - 1], (BAR + BEAT - 1) as f32, "live up to the beat");
        assert_ne!(
            bus.l[BEAT + 1_000],
            (BAR + BEAT + 1_000) as f32,
            "and jumping after it, not a bar later"
        );
    }

    /// Letting go before the boundary takes the press back. A musician who
    /// changed their mind should not have to wait out a bar to find out.
    #[test]
    fn a_release_before_the_boundary_takes_the_press_back() {
        let (mut device, mut bus) = primed();
        fill_ramp(&mut bus, BAR, 4_000);
        device.process_with_params(
            &playing(4_000, 0.0),
            &mut bus,
            &[],
            &[
                param(0, BUFFER_PARAM_QUANTIZE, 1.0),
                param(0, BUFFER_PARAM_JUMP, 1.0),
                param(2_000, BUFFER_PARAM_JUMP, 0.0),
            ],
        );
        assert!(!device.armed_gesture(), "the press was taken back");
        assert!(device.is_following(), "and nothing ever fired");
        assert_eq!(bus.l[3_999], (BAR + 3_999) as f32);
    }

    /// A shortened span is the most recent that much, and a playhead loops
    /// inside it rather than running out into the rest of the ring.
    #[test]
    fn a_shortened_span_is_the_most_recent_that_much() {
        let (mut device, mut bus) = primed();
        fill_ramp(&mut bus, BAR, 8_000);
        device.process_with_params(
            &context(8_000),
            &mut bus,
            &[],
            &[
                param(0, BUFFER_PARAM_CROSSFADE_MS, 0.0),
                // A sixteenth of span: the last 6 000 frames.
                param(0, BUFFER_PARAM_POSITION_SPAN, 13.0),
                param(0, BUFFER_PARAM_POSITION, 0.0),
                param(32, BUFFER_PARAM_POSITION, 0.02),
            ],
        );
        let heard = bus.l[100];
        assert!(
            heard >= (BAR - SIXTEENTH) as f32 - 100.0 && heard <= BAR as f32,
            "a sixteenth of span has to sit in the last sixteenth of history, \
             and it read {heard}"
        );
    }

    /// Output is identical however the block is cut up, which is the property
    /// every timed-parameter path has to have and the one a sample loop is
    /// easiest to get wrong.
    #[test]
    fn output_is_identical_across_block_sizes() {
        let render = |block: usize| {
            let mut device = BufferDevice::with_capacity(BAR);
            let mut bus = StereoBus::with_capacity(block);
            let mut out = Vec::new();
            for start in (0..BAR * 2).step_by(block) {
                fill_ramp(&mut bus, start, block);
                let params: Vec<_> = (0..block as u32 / 32)
                    .map(|tick| {
                        param(
                            tick * 32,
                            BUFFER_PARAM_POSITION,
                            ((start as u32 + tick * 32) % BAR as u32) as f32 / BAR as f32,
                        )
                    })
                    .collect();
                device.process_with_params(&context(block), &mut bus, &[], &params);
                out.extend_from_slice(&bus.l[..block]);
            }
            out
        };
        // Both divide the run exactly. A block size that did not would leave
        // a short final block, and the test would be comparing two different
        // amounts of audio rather than two cuts of the same audio.
        let small = render(64);
        let large = render(1_600);
        assert_eq!(small.len(), large.len());
        for (index, (a, b)) in small.iter().zip(&large).enumerate() {
            assert!(
                (a - b).abs() < 1e-3,
                "frame {index} differs by block size: {a} vs {b}"
            );
        }
    }

    /// **A replacement ring keeps the history** (MOO-137). A tempo change
    /// builds a buffer at the new length; the replacement takes the most
    /// recent frames the outgoing ring held, as many as it has room for, at
    /// the same absolute positions, and carries on writing from there.
    #[test]
    fn a_replacement_ring_takes_over_the_history() {
        let mut live = BufferDevice::with_capacity(BAR);
        let mut bus = StereoBus::with_capacity(BAR + BEAT);
        fill_ramp(&mut bus, 1, BAR + BEAT);
        live.process(&context(BAR + BEAT), &mut bus, &[]);
        assert_eq!(live.write_head, (BAR + BEAT) as u64);

        for capacity in [BAR / 2, BAR, BAR * 2] {
            let mut next = BufferDevice::with_capacity(capacity);
            next.adopt_history_from(&live);
            assert_eq!(next.write_head, live.write_head);
            let kept = capacity.min(BAR);
            for back in 1..=kept {
                let position = live.write_head - back as u64;
                let index = (position % capacity as u64) as usize;
                // `fill_ramp` wrote frame `n` as `n + 1`.
                assert_eq!(
                    next.left[index],
                    (position + 1) as f32,
                    "{capacity}: {back} frames back was lost"
                );
                assert_eq!(next.right[index], -((position + 1) as f32));
            }
            assert!(
                next.waveform_peaks().iter().any(|peak| *peak > 0.0),
                "{capacity}: the picture was not redrawn"
            );
        }
    }

    /// A ring that has been written for less than its length hands over
    /// only what it has, and the rest of the replacement stays silent.
    #[test]
    fn a_young_ring_hands_over_only_what_it_wrote() {
        let mut live = BufferDevice::with_capacity(BAR);
        let mut bus = StereoBus::with_capacity(BEAT);
        fill_ramp(&mut bus, 1, BEAT);
        live.process(&context(BEAT), &mut bus, &[]);
        let mut next = BufferDevice::with_capacity(BAR * 2);
        next.adopt_history_from(&live);
        assert_eq!(&next.left[..BEAT], &live.left[..BEAT]);
        assert!(next.left[BEAT..].iter().all(|sample| *sample == 0.0));
    }

    /// **A restored freeze waits for something to hold** (MOO-137). The
    /// ring's audio is not saved, so a device built with Freeze on used to
    /// latch an empty ring and play silence under a face reading FROZEN.
    /// It records until the ring is full, and then the freeze lands.
    #[test]
    fn a_restored_freeze_lands_once_the_ring_is_full() {
        let params = BufferParams {
            bars: 1,
            freeze: 1.0,
            ..BufferParams::default()
        };
        let mut device = BufferDevice::new(params, 48_000, 120.0);
        assert_eq!(device.capacity_frames(), BAR);

        let mut bus = StereoBus::with_capacity(BAR / 2);
        fill_ramp(&mut bus, 1, BAR / 2);
        device.process(&context(BAR / 2), &mut bus, &[]);
        assert!(!device.is_frozen(), "froze over a half-empty ring");
        assert!(device.is_writing());
        assert_eq!(device.armed_freeze(), Some(true), "the face has to show it coming");

        let mut bus = StereoBus::with_capacity(BAR / 2);
        fill_ramp(&mut bus, BAR / 2 + 1, BAR / 2);
        device.process(&context(BAR / 2), &mut bus, &[]);
        assert!(device.is_frozen(), "the freeze never landed on a full ring");
        assert_eq!(device.armed_freeze(), None);
        // And what it holds is the bar it recorded, not silence.
        assert!(device.left.iter().all(|sample| *sample != 0.0));
    }

    /// A NaN reaching the writer is stored as silence rather than kept for
    /// every later gesture to replay (MOO-176).
    #[test]
    fn a_nan_is_never_written_into_the_ring() {
        let mut device = BufferDevice::with_capacity(BEAT);
        let mut bus = StereoBus::with_capacity(BEAT);
        fill_ramp(&mut bus, 1, BEAT);
        bus.l[100] = f32::NAN;
        bus.r[200] = f32::INFINITY;
        device.process(&context(BEAT), &mut bus, &[]);
        assert!(device.left.iter().chain(&device.right).all(|sample| sample.is_finite()));
        assert!(bus.l[..BEAT].iter().all(|sample| sample.is_finite()));
    }
}
