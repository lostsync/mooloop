//! Retained-audio buffer device core.
//!
//! The device is deliberately independent of project and UI plumbing: its
//! public event tuple is the contract that the sequencer, parameter locks, and
//! debug triggers will share. Construction allocates the ring; [`process`] is
//! allocation-free and operates in place on an existing [`StereoBus`].

use mooloop_core::{BufferDuration, BufferEvent, BufferParams};

use crate::{AudioNode, Event, EventList, ProcessContext, StereoBus};

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

/// What decides the head's velocity on a given frame.
///
/// The controls that can move the head would otherwise fight over it, and
/// this is the arbitration rule from
/// `docs/plans/buffer-implementation/03-freeze-and-the-grid.md` written down
/// as a type: a chase outranks free-run while it is closing, and a gesture
/// outranks both for as long as it runs.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Drive {
    /// A [`BufferEvent`] set a fixed rate when it fired. JUMP, REV and STUT.
    /// The rate is part of the gesture and does not move under it.
    Event,
    /// The `Rate` parameter, re-read every frame so that automating it moves
    /// the head at once rather than at the next gesture.
    Rate,
}

#[derive(Clone, Copy)]
struct ReadHead {
    position: f64,
    rate: f32,
    drive: Drive,
    window_start: f64,
    window_end: Option<f64>,
    repeats_remaining: Option<u32>,
    expires_at: Option<u64>,
    crossfade_frames: u32,
    /// Set when the event's duration is `Gate`: the head holds until an
    /// explicit release rather than until it expires or collides.
    gated: bool,
}

#[derive(Clone, Copy)]
enum FadeSource {
    Live,
    Detached { position: f64, rate: f32 },
}

/// A platter under the hand. The read head chases `target`, and the speed it
/// closes the gap at *is* the playback rate — so spinning fast plays fast,
/// and letting go coasts to a stop. Position is the input; rate is derived.
/// Driving rate directly instead would be a jog shuttle, not a turntable.
#[derive(Clone, Copy)]
struct Scrub {
    target: f64,
    /// Time constant of the chase, in frames. Roughly one control-message
    /// interval: long enough that per-message steps read as continuous
    /// motion, short enough that the head does not lag the hand.
    chase_frames: f64,
    /// Set when the scrub is driven by the `offset_beats` parameter rather
    /// than by a hand. The target is then `write_head - offset_frames`
    /// recomputed every frame, not a value pushed in per control message:
    /// a held offset has to play forward at unity, and a target that only
    /// moved 32 frames at a time would sag between messages and warble.
    offset_frames: Option<f64>,
}

/// A stopped platter must go silent. Holding at rate zero would repeat one
/// sample forever, which is a DC step, not silence.
const SCRUB_MUTE_RATE: f32 = 0.02;
/// How far either side of the midpoint the `Freeze` parameter has to travel
/// before it changes anything.
const FREEZE_HYSTERESIS: f32 = 0.05;

/// Ceiling on how fast anything may drive the head, so a wild spin cannot
/// outrun the interpolator into noise. The `Rate` descriptor's range is this
/// same number, from the same place: a knob whose top travel the clamp threw
/// away would be a control lying about its own span.
const MAX_SCRUB_RATE: f32 = mooloop_core::MAX_BUFFER_RATE;

#[derive(Clone, Copy)]
struct Fade {
    source: FadeSource,
    frame: u32,
    frames: u32,
}

/// One opt-in stereo rolling history. `capacity_frames` is fixed after
/// construction; replacing it on a tempo/config change is a control-plane
/// operation, never something [`process`] attempts.
pub struct BufferDevice {
    left: Vec<f32>,
    right: Vec<f32>,
    write_head: u64,
    head: Option<ReadHead>,
    fade: Option<Fade>,
    scrub: Option<Scrub>,
    /// Amplitude the head is currently entitled to, from its speed. Held
    /// across frames so the mute fades rather than steps, and applied to any
    /// head whose velocity comes from a control rather than from a gesture --
    /// a stopped platter and a `Rate` of zero are the same DC step.
    head_gain: f32,
    /// Free-run velocity, from `BUFFER_PARAM_RATE`. What a detached head does
    /// when nothing else is talking to it.
    rate: f32,
    /// Whether the writer is running. Freeze is what turns it off.
    ///
    /// It is a `bool` rather than an absent writer because the ring must keep
    /// its contents: freezing is latching the history, not discarding it.
    writing: bool,
    /// Frames processed, whatever the writer is doing.
    ///
    /// `write_head` used to be both the writer's position and the clock, and
    /// Freeze separates them: a gesture with a `Steps` duration must still
    /// expire while the writer is stopped, or freezing during a STUT would
    /// leave it repeating forever.
    frames_elapsed: u64,
    /// A freeze carried in from the saved parameter set, applied on the first
    /// block for the same reason [`Self::pending_offset_beats`] is:
    /// construction has no [`ProcessContext`], so it cannot know how long the
    /// crossfade is.
    pending_freeze: bool,
    /// Incremented when the writer catches a detached head. This is a cheap
    /// observable diagnostic for the host/tests without logging on the RT
    /// thread.
    collision_count: u64,
    /// Standing crossfade length, set by `BUFFER_PARAM_CROSSFADE_MS`. Gesture
    /// events carry their own and are unaffected.
    crossfade_ms: f32,
    /// An offset carried in from the saved parameter set, applied on the first
    /// block. Construction has no `ProcessContext`, so it cannot know how many
    /// frames a beat is, and a loaded project must still come up with its head
    /// where the document says it was.
    pending_offset_beats: Option<f32>,
}

impl BufferDevice {
    pub fn new(params: BufferParams, sample_rate: u32, bpm: f64) -> Self {
        let mut device = Self::with_bars(sample_rate, bpm, u32::from(params.bars.max(1)));
        device.crossfade_ms = params.crossfade_ms.clamp(0.0, 50.0);
        device.rate = params.rate.clamp(-MAX_SCRUB_RATE, MAX_SCRUB_RATE);
        device.pending_freeze = params.freeze >= 0.5;
        device.pending_offset_beats = (params.offset_beats > 0.0).then_some(params.offset_beats);
        device
    }
    /// Allocate a ring for `bars` bars at the supplied tempo.
    ///
    /// The bar length is [`mooloop_core::frames_per_bar`], which the sampler
    /// uses too -- the comment there used to claim it followed this device
    /// while this device spelled its own copy. `ceil` and the floor of four
    /// frames stay: the ring must not shrink on a rounding change, and
    /// [`Self::with_capacity`] has a floor for a reason. Construction time,
    /// not `process`, so the float is free.
    pub fn with_bars(sample_rate: u32, bpm: f64, bars: u32) -> Self {
        let frames_per_bar = mooloop_core::frames_per_bar(sample_rate, bpm).ceil() as usize;
        Self::with_capacity((frames_per_bar * bars.max(1) as usize).max(4))
    }

    /// Allocate an explicit number of frames. Primarily useful for tests and
    /// prepared engine allocations.
    pub fn with_capacity(capacity_frames: usize) -> Self {
        let capacity_frames = capacity_frames.max(4);
        Self {
            left: vec![0.0; capacity_frames],
            right: vec![0.0; capacity_frames],
            write_head: 0,
            head: None,
            fade: None,
            scrub: None,
            head_gain: 0.0,
            rate: 1.0,
            writing: true,
            frames_elapsed: 0,
            pending_freeze: false,
            collision_count: 0,
            crossfade_ms: 2.5,
            pending_offset_beats: None,
        }
    }

    pub fn capacity_frames(&self) -> usize {
        self.left.len()
    }

    pub fn memory_bytes(&self) -> usize {
        self.capacity_frames() * 2 * std::mem::size_of::<f32>()
    }

    pub fn collision_count(&self) -> u64 {
        self.collision_count
    }

    pub fn is_following(&self) -> bool {
        self.head.is_none()
    }

    /// Process one block. Events must be in ascending sample-offset order.
    /// Follow is intentionally a direct assignment from the input sample,
    /// rather than a read from the ring: that makes it bit-identical and zero
    /// latency even at startup and across wraparound.
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
        if let Some(beats) = self.pending_offset_beats.take() {
            self.set_offset_beats(beats, context);
        }
        if std::mem::take(&mut self.pending_freeze) {
            self.freeze(context);
        }
        let mut event_index = 0;
        let mut param_index = 0;
        for frame in 0..context.frames {
            // Parameters first: a gesture arriving on the same frame as an
            // offset change should see the new crossfade, and a gesture owns
            // the head afterwards either way.
            while let Some(timed) = params.get(param_index) {
                if timed.offset as usize != frame {
                    break;
                }
                self.set_param(timed.id, timed.value, context);
                param_index += 1;
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
            // it whatever the read head is doing. `writing` is the one thing
            // that stops it, and Freeze is the only thing that sets it.
            if self.writing {
                let write_index = self.index(self.write_head as f64);
                self.left[write_index] = input_l;
                self.right[write_index] = input_r;
            }

            let (mut output_l, mut output_r) = match self.head {
                Some(head) => {
                    let (left, right) = self.read_stereo(head.position);
                    // A gesture's rate is part of the gesture and is left
                    // alone; a control-driven head follows its own speed down
                    // to silence, because holding at zero repeats one sample
                    // forever and that is a DC step rather than a hold.
                    if self.scrub.is_some() || head.drive == Drive::Rate {
                        (left * self.head_gain, right * self.head_gain)
                    } else {
                        (left, right)
                    }
                }
                None => (input_l, input_r),
            };

            if let Some(mut fade) = self.fade {
                let (from_l, from_r) = match fade.source {
                    FadeSource::Live => (input_l, input_r),
                    FadeSource::Detached { position, .. } => self.read_stereo(position),
                };
                let phase = fade.frame as f32 / fade.frames as f32;
                let from_gain = (phase * core::f32::consts::FRAC_PI_2).cos();
                let to_gain = (phase * core::f32::consts::FRAC_PI_2).sin();
                output_l = from_l * from_gain + output_l * to_gain;
                output_r = from_r * from_gain + output_r * to_gain;
                if let FadeSource::Detached { position, rate } = &mut fade.source {
                    *position += f64::from(*rate);
                }
                fade.frame += 1;
                self.fade = (fade.frame < fade.frames).then_some(fade);
            }

            bus.l[frame] = output_l;
            bus.r[frame] = output_r;
            self.frames_elapsed += 1;
            if self.writing {
                self.write_head += 1;
            }
            self.advance_head();
        }
    }

    /// Stop the writer and make the retained history a sample.
    ///
    /// The head is handed to `Rate` if nothing else owns it, so it keeps
    /// moving with the time base the writer was providing. A gesture running
    /// at this moment keeps running, against the frozen ring: its window and
    /// repeat count are about the material, and the material is still there.
    pub fn freeze(&mut self, context: &ProcessContext) {
        if !self.writing {
            return;
        }
        self.writing = false;
        if self.head.is_none() {
            self.detach_at_rate(context);
        }
    }

    /// Restart the writer and return to live.
    ///
    /// A free-running head goes back to following; a gesture is left alone,
    /// exactly as a gesture outranks the offset parameter, and returns by its
    /// own duration or collision.
    pub fn thaw(&mut self) {
        if self.writing {
            return;
        }
        self.writing = true;
        if let Some(head) = self.head.filter(|head| head.drive == Drive::Rate) {
            self.return_live(head, head.position);
        }
    }

    pub fn is_frozen(&self) -> bool {
        !self.writing
    }

    fn set_param(&mut self, id: u32, value: f32, context: &ProcessContext) {
        match id {
            mooloop_core::BUFFER_PARAM_OFFSET_BEATS => self.set_offset_beats(value, context),
            mooloop_core::BUFFER_PARAM_CROSSFADE_MS => {
                self.crossfade_ms = value.clamp(0.0, 50.0)
            }
            mooloop_core::BUFFER_PARAM_RATE => {
                self.rate = value.clamp(-MAX_SCRUB_RATE, MAX_SCRUB_RATE)
            }
            // Hysteresis, not a threshold. A modulator resting near the
            // midpoint would otherwise chatter the writer once per control
            // tick, and every one of those is a crossfade.
            mooloop_core::BUFFER_PARAM_FREEZE => {
                if value >= 0.5 + FREEZE_HYSTERESIS {
                    self.freeze(context);
                } else if value <= 0.5 - FREEZE_HYSTERESIS {
                    self.thaw();
                }
            }
            _ => {}
        }
    }

    /// Place the read head `beats` behind the writer, or return it to live at
    /// zero.
    ///
    /// This is position mode, the same as a hand scrub: the head chases the
    /// offset and the closing speed *is* the playback rate, so sweeping the
    /// offset is a scrub and holding it is delayed playback at unity. That is
    /// the whole reason the buffer is worth automating, and it is why there is
    /// no separate rate parameter — a rate would contradict the position.
    ///
    /// A gesture head outranks the parameter. Automation does not fight a
    /// JUMP/REV/STUT that is already running; the offset re-asserts itself on
    /// the next control tick after that gesture ends, which for a lane is
    /// within 32 frames.
    fn set_offset_beats(&mut self, beats: f32, context: &ProcessContext) {
        let frames_per_beat = context.sample_rate as f64 * 60.0 / context.bpm.max(1.0);
        let offset_frames = f64::from(beats.max(0.0)) * frames_per_beat;
        let ours = self
            .scrub
            .is_some_and(|scrub| scrub.offset_frames.is_some());
        // Below a frame there is no offset to speak of, and asking the head to
        // sit zero frames behind the writer is a collision by definition.
        if offset_frames < 1.0 {
            if ours {
                if let Some(head) = self.head {
                    self.return_live(head, head.position);
                }
            }
            return;
        }
        if self.head.is_some() && !ours {
            return;
        }
        if !ours {
            self.scrub_begin(context, self.crossfade_ms);
        }
        if let Some(scrub) = &mut self.scrub {
            scrub.offset_frames = Some(offset_frames);
        }
    }

    fn fire(&mut self, event: BufferEvent, context: &ProcessContext) {
        let frames_per_beat = context.sample_rate as f64 * 60.0 / context.bpm.max(1.0);
        let position = self.write_head as f64 + f64::from(event.offset_beats) * frames_per_beat;
        // A window always covers material the head is about to play, so it
        // extends backward from the entry point for a reverse head and
        // forward for a forward one. Extending forward in both cases would
        // point a reverse window at samples the writer has not reached.
        let window = event
            .window_beats
            .filter(|beats| *beats > 0.0)
            .map(|beats| f64::from(beats) * frames_per_beat);
        let (window_start, window_end) = match window {
            Some(length) if event.rate < 0.0 => (position - length, Some(position)),
            Some(length) => (position, Some(position + length)),
            None => (position, None),
        };
        let duration_frames = match event.duration {
            // Current patterns are a sixteenth-note grid: four steps/beat.
            BufferDuration::Steps(steps) => {
                Some((f64::from(steps) * frames_per_beat / 4.0).round() as u64)
            }
            BufferDuration::UntilNextEvent | BufferDuration::Gate => None,
        };
        let head = ReadHead {
            position,
            rate: event.rate,
            drive: Drive::Event,
            window_start,
            window_end,
            repeats_remaining: event.repeat,
            expires_at: duration_frames.map(|frames| self.frames_elapsed + frames),
            crossfade_frames: ms_to_frames(event.crossfade_ms, context.sample_rate),
            gated: matches!(event.duration, BufferDuration::Gate),
        };
        self.fade = (head.crossfade_frames > 0).then_some(Fade {
            source: FadeSource::Live,
            frame: 0,
            frames: head.crossfade_frames,
        });
        self.head = Some(head);
    }

    /// Put the head on the platter, holding at the live position. Idempotent:
    /// a scrub already under way keeps its target rather than snapping back
    /// to live, since control messages arrive as a stream with no press.
    pub fn scrub_begin(&mut self, context: &ProcessContext, crossfade_ms: f32) {
        if self.scrub.is_some() {
            return;
        }
        let position = self.write_head as f64;
        let crossfade_frames = ms_to_frames(crossfade_ms, context.sample_rate);
        self.fade = (crossfade_frames > 0).then_some(Fade {
            source: FadeSource::Live,
            frame: 0,
            frames: crossfade_frames,
        });
        self.head = Some(ReadHead {
            position,
            rate: 0.0,
            drive: Drive::Rate,
            window_start: position,
            window_end: None,
            repeats_remaining: None,
            expires_at: None,
            crossfade_frames,
            // Gated, so the same release that ends a held note ends a scrub.
            gated: true,
        });
        self.scrub = Some(Scrub {
            target: position,
            chase_frames: (context.sample_rate as f64 / 200.0).max(1.0),
            offset_frames: None,
        });
        self.head_gain = 0.0;
    }

    /// Move the platter. `delta_frames` is signed; negative is back in time.
    pub fn scrub_move(&mut self, delta_frames: f64) {
        if let Some(scrub) = &mut self.scrub {
            scrub.target += delta_frames;
        }
    }

    pub fn is_scrubbing(&self) -> bool {
        self.scrub.is_some()
    }

    /// Free-run velocity the head takes when nothing else is driving it.
    pub fn rate(&self) -> f32 {
        self.rate
    }

    /// Stop or restart the writer without the crossfade [`Self::freeze`]
    /// arranges. Tests, and [`Self::freeze`] itself; nothing else.
    pub fn set_writing(&mut self, writing: bool) {
        self.writing = writing;
    }

    pub fn is_writing(&self) -> bool {
        self.writing
    }

    /// Detach the head at the live position and let it free-run at `Rate`.
    ///
    /// No chase and no window: the head is handed over to the `Rate`
    /// parameter and stays there until a gesture takes it, a chase is armed,
    /// or it collides with the writer. This is the head Freeze creates.
    ///
    /// Note what freezing does *not* do on its own. With the writer stopped
    /// the target `offset_beats` computes goes static, the chase closes on it,
    /// the speed falls to zero and [`SCRUB_MUTE_RATE`] mutes the result -- a
    /// tape stop into silence. That is why `Rate` exists and why the two are
    /// one decision: take the writer away and something has to supply the time
    /// base it was providing.
    pub fn detach_at_rate(&mut self, context: &ProcessContext) {
        let position = self.write_head as f64;
        let crossfade_frames = ms_to_frames(self.crossfade_ms, context.sample_rate);
        self.fade = (crossfade_frames > 0).then_some(Fade {
            source: FadeSource::Live,
            frame: 0,
            frames: crossfade_frames,
        });
        self.scrub = None;
        self.head_gain = 0.0;
        self.head = Some(ReadHead {
            position,
            rate: self.rate,
            drive: Drive::Rate,
            window_start: position,
            window_end: None,
            repeats_remaining: None,
            expires_at: None,
            crossfade_frames,
            // Latching: a free-running head is not a held gesture, so a stray
            // release from some other control must not cancel it.
            gated: false,
        });
    }

    /// End a gated edit. A latching head is left alone: a held control sends
    /// this on release without knowing whether its own event is still the
    /// one running, and it must not cancel whatever superseded it.
    pub fn release(&mut self) {
        if let Some(head) = self.head.filter(|head| head.gated) {
            self.return_live(head, head.position);
        }
    }

    fn advance_head(&mut self) {
        let Some(mut head) = self.head else { return };
        if let Some(scrub) = self.scrub {
            // Speed is whatever it takes to close the remaining gap, so the
            // hand's motion sets the pitch without ever being asked for a
            // rate.
            let target = match scrub.offset_frames {
                Some(offset) => self.write_head as f64 - offset,
                None => scrub.target,
            };
            let rate = ((target - head.position) / scrub.chase_frames) as f32;
            head.rate = rate.clamp(-MAX_SCRUB_RATE, MAX_SCRUB_RATE);
            self.head_gain = (head.rate.abs() / SCRUB_MUTE_RATE).min(1.0);
        } else if head.drive == Drive::Rate {
            // Re-read every frame, so a lane drawn on `Rate` moves the head
            // within a control tick rather than at the next gesture.
            head.rate = self.rate.clamp(-MAX_SCRUB_RATE, MAX_SCRUB_RATE);
            self.head_gain = (head.rate.abs() / SCRUB_MUTE_RATE).min(1.0);
        }
        let old_position = head.position;
        head.position += f64::from(head.rate);

        if let Some(end) = head.window_end {
            let passed_end = head.rate >= 0.0 && head.position >= end;
            let passed_start = head.rate < 0.0 && head.position < head.window_start;
            if passed_end || passed_start {
                match head.repeats_remaining {
                    Some(1) => {
                        self.return_live(head, old_position);
                        return;
                    }
                    Some(remaining) => head.repeats_remaining = Some(remaining - 1),
                    None => {}
                }
                let destination = if head.rate < 0.0 {
                    end
                } else {
                    head.window_start
                };
                if head.crossfade_frames > 0 {
                    self.fade = Some(Fade {
                        source: FadeSource::Detached {
                            position: old_position,
                            rate: head.rate,
                        },
                        frame: 0,
                        frames: head.crossfade_frames,
                    });
                }
                head.position = destination;
            }
        }

        let writer = self.write_head as f64;
        let span = self.capacity_frames() as f64;
        let oldest = writer - span;
        let expired = head
            .expires_at
            .is_some_and(|frame| self.frames_elapsed >= frame);
        if !self.writing {
            // Frozen, so there is no writer to collide with: the ring is a
            // sample and the head wraps inside it. Running off the end into
            // the oldest retained sample is *why* quantized freeze matters --
            // it is seamless exactly when the material is periodic at the
            // buffer length, which is what freezing on a loop's bar line
            // gives you.
            //
            // `rem_euclid` rather than a loop, because at four times speed
            // over a four-frame ring a single subtraction is not enough, and a
            // loop on the audio thread is not a thing to reason about later.
            head.position = oldest + (head.position - oldest).rem_euclid(span);
            if expired {
                self.return_live(head, old_position);
            } else {
                self.head = Some(head);
            }
            return;
        }
        // A forward head entering unwritten future samples or any head falling
        // out of retained history has collided with the writer. Return rather
        // than wrapping into unrelated audio or silently clamping.
        let collision = head.position >= writer || head.position <= oldest;
        if expired || collision {
            if collision {
                self.collision_count += 1;
            }
            self.return_live(head, old_position);
        } else {
            self.head = Some(head);
        }
    }

    fn return_live(&mut self, head: ReadHead, position: f64) {
        self.scrub = None;
        self.head_gain = 0.0;
        if head.crossfade_frames > 0 {
            self.fade = Some(Fade {
                source: FadeSource::Detached {
                    position,
                    rate: head.rate,
                },
                frame: 0,
                frames: head.crossfade_frames,
            });
        }
        self.head = None;
    }

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

/// Stable identity for a buffer allocation configuration. The tempo is
/// intentionally absent: a tempo resize replaces the same logical device;
/// only a bars change makes an older prepared resize stale.
pub fn buffer_allocation_key(params: BufferParams) -> u64 {
    u64::from(params.bars.max(1))
}

fn ms_to_frames(ms: f32, sample_rate: u32) -> u32 {
    (ms.max(0.0) * sample_rate as f32 / 1_000.0).round() as u32
}

/// Deliberately no `is_at_rest` or `tail_frames`: a retained-audio buffer
/// plays back what it captured, so it is the one device in the rack that
/// makes sound out of a silent input by design. It keeps the default — never
/// skipped — and that is a decision rather than an omission.
impl AudioNode for BufferDevice {
    fn buffer_collisions(&self) -> u64 {
        self.collision_count
    }

    fn holds_frozen_audio(&self) -> bool {
        self.is_frozen()
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
                Event::BufferScrub { delta_frames } => {
                    self.scrub_begin(context, self.crossfade_ms);
                    self.scrub_move(f64::from(delta_frames));
                }
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

    fn context(frames: usize) -> ProcessContext {
        ProcessContext {
            sample_rate: 48_000,
            frames,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        }
    }

    fn fill_ramp(bus: &mut StereoBus, first: usize, frames: usize) {
        for frame in 0..frames {
            bus.l[frame] = (first + frame) as f32;
            bus.r[frame] = -(first as f32 + frame as f32);
        }
    }

    fn offset_param(offset: u32, beats: f32) -> TimedBufferParam {
        TimedBufferParam {
            offset,
            id: mooloop_core::BUFFER_PARAM_OFFSET_BEATS,
            value: beats,
        }
    }

    /// Fill the ring with a ramp so a read position can be identified from the
    /// sample value alone.
    fn primed(frames: usize) -> (BufferDevice, StereoBus) {
        let mut device = BufferDevice::with_capacity(frames * 2);
        let mut bus = StereoBus::with_capacity(frames);
        fill_ramp(&mut bus, 0, frames);
        device.process(&context(frames), &mut bus, &[]);
        (device, bus)
    }

    #[test]
    fn a_held_offset_settles_into_playback_at_unity() {
        // One beat at 120 BPM and 48 kHz is 24 000 frames.
        let (mut device, mut bus) = primed(48_000);
        fill_ramp(&mut bus, 48_000, 48_000);
        device.process_with_params(
            &context(48_000),
            &mut bus,
            &[],
            &[offset_param(0, 1.0)],
        );
        assert!(!device.is_following());

        // After the chase has converged, consecutive output samples must
        // advance by one, which is unity rate rather than a sagging chase.
        fill_ramp(&mut bus, 96_000, 48_000);
        device.process_with_params(&context(48_000), &mut bus, &[], &[offset_param(0, 1.0)]);
        let tail = &bus.l[40_000..40_010];
        for pair in tail.windows(2) {
            assert!(
                (pair[1] - pair[0] - 1.0).abs() < 0.05,
                "offset playback is not running at unity: {tail:?}"
            );
        }
        // ...and it must be reading roughly a beat behind the writer.
        let lag = (96_000 + 40_000) as f32 - tail[0];
        assert!(
            (lag - 24_000.0).abs() < 1_500.0,
            "expected about one beat of lag, got {lag}"
        );
    }

    #[test]
    fn returning_the_offset_to_zero_returns_the_head_to_live() {
        let (mut device, mut bus) = primed(48_000);
        fill_ramp(&mut bus, 48_000, 48_000);
        device.process_with_params(&context(48_000), &mut bus, &[], &[offset_param(0, 1.0)]);
        assert!(!device.is_following());

        fill_ramp(&mut bus, 96_000, 48_000);
        device.process_with_params(&context(48_000), &mut bus, &[], &[offset_param(0, 0.0)]);
        assert!(device.is_following());
        assert!(!device.is_scrubbing());
    }

    #[test]
    fn sweeping_the_offset_moves_the_head_rather_than_jumping_it() {
        let (mut device, mut bus) = primed(48_000);
        // Ramp the offset across the block the way a lane would, one message
        // per 32 frames.
        let params: Vec<TimedBufferParam> = (0..48_000 / 32)
            .map(|tick| {
                offset_param(tick * 32, tick as f32 / (48_000.0 / 32.0))
            })
            .collect();
        fill_ramp(&mut bus, 48_000, 48_000);
        device.process_with_params(&context(48_000), &mut bus, &[], &params);

        // `fill_ramp` writes the absolute frame number, so an output sample
        // *is* the position it was read from. Read the second half, past the
        // scrub's fade-in from silence — a head at rate zero is deliberately
        // muted, so early samples report gain, not position.
        let travel = &bus.l[24_000..48_000];
        for pair in travel.windows(2) {
            let step = pair[1] - pair[0];
            assert!(
                (0.0..1.0).contains(&step),
                "the head should crawl forward, slower than the writer: {step}"
            );
        }
        // Half a beat of offset opens across the second half of the block, so
        // the head must fall behind by about that much and no more.
        let fell_behind = (travel[0] - travel[travel.len() - 1]) + travel.len() as f32;
        assert!(
            (fell_behind - 12_000.0).abs() < 1_000.0,
            "expected to lose about half a beat over the sweep, lost {fell_behind}"
        );
    }

    #[test]
    fn a_gesture_outranks_the_offset_parameter_while_it_runs() {
        let (mut device, mut bus) = primed(48_000);
        fill_ramp(&mut bus, 48_000, 48_000);
        let jump = TimedBufferEvent {
            offset: 0,
            event: BufferEvent {
                offset_beats: -2.0,
                duration: BufferDuration::Gate,
                ..BufferEvent::live()
            },
        };
        device.process_with_params(
            &context(48_000),
            &mut bus,
            &[jump],
            &[offset_param(24_000, 1.0)],
        );
        // The parameter arrived mid-block while the gesture owned the head; it
        // must not have converted that head into an offset scrub.
        assert!(!device.is_scrubbing());
        assert!(!device.is_following());

        // Once the gesture releases, the next control tick takes the offset.
        device.release();
        fill_ramp(&mut bus, 96_000, 48_000);
        device.process_with_params(&context(48_000), &mut bus, &[], &[offset_param(0, 1.0)]);
        assert!(device.is_scrubbing());
    }

    #[test]
    fn a_saved_offset_is_applied_on_the_first_block() {
        let mut device = BufferDevice::new(
            mooloop_core::BufferParams {
                bars: 1,
                offset_beats: 1.0,
                crossfade_ms: 2.5,
                ..Default::default()
            },
            48_000,
            120.0,
        );
        let mut bus = StereoBus::with_capacity(1_000);
        fill_ramp(&mut bus, 0, 1_000);
        device.process(&context(1_000), &mut bus, &[]);
        assert!(device.is_scrubbing());
    }

    #[test]
    fn follow_is_bit_transparent_and_zero_latency() {
        let mut device = BufferDevice::with_capacity(256);
        let mut bus = StereoBus::with_capacity(64);
        fill_ramp(&mut bus, 0, 64);
        let expected_l = bus.l.clone();
        let expected_r = bus.r.clone();
        device.process(&context(64), &mut bus, &[]);
        assert_eq!(bus.l, expected_l);
        assert_eq!(bus.r, expected_r);
        assert!(device.is_following());
    }

    #[test]
    fn jump_one_beat_is_an_exact_constant_delay() {
        let mut device = BufferDevice::with_capacity(100_000);
        let mut bus = StereoBus::with_capacity(48_000);
        fill_ramp(&mut bus, 0, 48_000);
        device.process(&context(48_000), &mut bus, &[]);
        fill_ramp(&mut bus, 48_000, 48_000);
        let event = TimedBufferEvent {
            offset: 0,
            event: BufferEvent {
                offset_beats: -1.0,
                rate: 1.0,
                window_beats: None,
                repeat: None,
                duration: BufferDuration::UntilNextEvent,
                crossfade_ms: 0.0,
            },
        };
        device.process(&context(48_000), &mut bus, &[event]);
        assert_eq!(bus.l[0], 24_000.0);
        assert_eq!(bus.l[12_345], 36_345.0);
        assert_eq!(bus.r[12_345], -36_345.0);
    }

    #[test]
    fn reverse_collides_then_returns_to_live() {
        let mut device = BufferDevice::with_capacity(1_000);
        let mut bus = StereoBus::with_capacity(100);
        for block in 0..10 {
            fill_ramp(&mut bus, block * 100, 100);
            device.process(&context(100), &mut bus, &[]);
        }
        fill_ramp(&mut bus, 1_000, 100);
        let event = TimedBufferEvent {
            offset: 0,
            event: BufferEvent {
                offset_beats: -0.01,
                rate: -1.0,
                window_beats: None,
                repeat: None,
                duration: BufferDuration::UntilNextEvent,
                crossfade_ms: 0.0,
            },
        };
        device.process(&context(100), &mut bus, &[event]);
        fill_ramp(&mut bus, 1_100, 100);
        device.process(&context(100), &mut bus, &[]);
        fill_ramp(&mut bus, 1_200, 100);
        device.process(&context(100), &mut bus, &[]);
        fill_ramp(&mut bus, 1_300, 100);
        device.process(&context(100), &mut bus, &[]);
        assert!(device.collision_count() > 0);
        assert!(device.is_following());
        assert_eq!(bus.l[99], 1_399.0);
    }

    #[test]
    fn stutter_repeats_a_beat_relative_window_then_returns_live() {
        let mut device = BufferDevice::with_capacity(100_000);
        let mut bus = StereoBus::with_capacity(48_000);
        fill_ramp(&mut bus, 0, 48_000);
        device.process(&context(48_000), &mut bus, &[]);
        fill_ramp(&mut bus, 48_000, 48_000);
        let event = TimedBufferEvent {
            offset: 0,
            event: BufferEvent {
                offset_beats: -0.0625,
                rate: 1.0,
                window_beats: Some(0.0625),
                repeat: Some(8),
                duration: BufferDuration::UntilNextEvent,
                crossfade_ms: 0.0,
            },
        };
        device.process(&context(48_000), &mut bus, &[event]);
        for repeat in 0..8 {
            let start = repeat * 1_500;
            assert_eq!(bus.l[start], 46_500.0);
            assert_eq!(bus.l[start + 1_499], 47_999.0);
        }
        assert_eq!(bus.l[12_000], 60_000.0);
        assert!(device.is_following());
    }

    fn rate_param(offset: u32, rate: f32) -> TimedBufferParam {
        TimedBufferParam {
            offset,
            id: mooloop_core::BUFFER_PARAM_RATE,
            value: rate,
        }
    }

    fn freeze_param(offset: u32, value: f32) -> TimedBufferParam {
        TimedBufferParam {
            offset,
            id: mooloop_core::BUFFER_PARAM_FREEZE,
            value,
        }
    }

    /// The claim `Rate` exists to make: with the writer stopped, the head
    /// still moves, and it moves at the commanded speed.
    ///
    /// Before this, the writer *was* the time base -- a held offset played
    /// forward at unity because the target was recomputed against a moving
    /// write head. Stop the writer and that head coasts to a stop and mutes,
    /// which is what makes Freeze and Rate one decision rather than two.
    #[test]
    fn a_detached_head_plays_at_the_commanded_rate_with_the_writer_stopped() {
        let (mut device, mut bus) = primed(48_000);
        device.detach_at_rate(&context(48_000));
        device.set_writing(false);
        let frozen_write_head = 48_000.0_f32;

        // Half speed, backwards. The ring holds a ramp whose value is its own
        // frame number, so consecutive output samples say what the head did.
        fill_ramp(&mut bus, 48_000, 4_000);
        device.process_with_params(&context(4_000), &mut bus, &[], &[rate_param(0, -0.5)]);
        // Past the crossfade and past the gain ramp, where the read is the
        // whole output.
        let tail = &bus.l[2_000..2_010];
        for pair in tail.windows(2) {
            assert!(
                (pair[1] - pair[0] + 0.5).abs() < 0.01,
                "the head is not running at -0.5x: {tail:?}"
            );
        }
        assert!(
            tail[0] < frozen_write_head && tail[0] > frozen_write_head - 4_000.0,
            "the head should be a little way back into the frozen history, not \
             at {}",
            tail[0]
        );
        assert!(
            !device.is_writing(),
            "the writer must still be stopped: nothing here restarts it"
        );
        assert_eq!(
            device.collision_count(),
            0,
            "a head running away from a stopped writer cannot collide with it"
        );
    }

    /// `Rate = 0` is a hold, and a hold is silence.
    ///
    /// Repeating one sample forever is a DC step, not a held note, so the gain
    /// follows the speed down -- the same law a stopped platter already runs.
    #[test]
    fn rate_zero_holds_silently_rather_than_repeating_one_sample() {
        let (mut device, mut bus) = primed(48_000);
        device.detach_at_rate(&context(48_000));
        device.set_writing(false);

        fill_ramp(&mut bus, 48_000, 4_000);
        device.process_with_params(&context(4_000), &mut bus, &[], &[rate_param(0, 0.0)]);

        let tail = &bus.l[2_000..4_000];
        let peak = tail.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
        assert!(
            peak < 1.0e-3,
            "a held head should be silent, and its peak is {peak}"
        );
    }

    /// Free-run is *alongside* the chase, not instead of it: a rate written
    /// while `Offset` owns the head changes nothing, because the chase
    /// outranks it.
    #[test]
    fn a_chase_outranks_the_rate_parameter_while_it_is_closing() {
        let (mut device, mut bus) = primed(48_000);
        fill_ramp(&mut bus, 48_000, 48_000);
        device.process_with_params(
            &context(48_000),
            &mut bus,
            &[],
            &[offset_param(0, 1.0), rate_param(0, -4.0)],
        );
        assert!(!device.is_following());

        fill_ramp(&mut bus, 96_000, 48_000);
        device.process_with_params(
            &context(48_000),
            &mut bus,
            &[],
            &[offset_param(0, 1.0), rate_param(0, -4.0)],
        );
        let tail = &bus.l[40_000..40_010];
        for pair in tail.windows(2) {
            assert!(
                (pair[1] - pair[0] - 1.0).abs() < 0.05,
                "a reverse Rate should not reach a head the chase owns: {tail:?}"
            );
        }
    }

    /// The `Rate` descriptor's range and the head's clamp are one number.
    ///
    /// A descriptor whose range ran past the clamp would draw a knob whose
    /// top travel did nothing, which is the defect `eq-v2` found on a shelf's
    /// Q and `LOOSE_ENDS.md` still carries.
    #[test]
    fn the_rate_descriptor_stops_where_the_head_clamp_does() {
        let descriptor = mooloop_core::EffectKind::Buffer
            .descriptors()
            .iter()
            .find(|d| d.id == mooloop_core::BUFFER_PARAM_RATE)
            .expect("Buffer publishes a Rate descriptor");
        assert_eq!(descriptor.max, MAX_SCRUB_RATE);
        assert_eq!(descriptor.min, -MAX_SCRUB_RATE);

        // And the clamp is real: asking for more than the descriptor allows
        // does not produce more than the descriptor allows.
        let (mut device, mut bus) = primed(1_000);
        device.process_with_params(&context(1_000), &mut bus, &[], &[rate_param(0, 99.0)]);
        assert_eq!(device.rate(), MAX_SCRUB_RATE);
    }

    /// The headline behaviour: freezing a periodic loop and doing nothing
    /// else is inaudible.
    ///
    /// It is only true when it is true, which is the whole reason quantized
    /// freeze is a step of its own. At the freeze instant the head sits at the
    /// write position, so continuing forward wraps immediately into the
    /// *oldest* retained sample -- you hear N bars ago, not now. That is
    /// seamless exactly when the material repeats at the buffer length, so
    /// this test gives it material that does.
    #[test]
    fn freezing_a_periodic_loop_is_inaudible() {
        // 1 200 frames of a 40-frame cycle: 30 whole cycles in the ring.
        //
        // A sine rather than a ramp, and that is the test's own trap: a
        // sawtooth's largest step is its cycle wrap, so it would report the
        // material's discontinuity as the device's and fail for the wrong
        // reason. A periodic signal has to be continuous across its own period
        // for "seamless" to mean anything. The first draft used a ramp and
        // failed at 1.95 against 0.05, which is the sawtooth's own tooth.
        const CYCLE: usize = 40;
        const RING: usize = 1_200;
        let cycle = |frame: usize| (frame as f32 / CYCLE as f32 * core::f32::consts::TAU).sin();

        let mut device = BufferDevice::with_capacity(RING);
        let mut bus = StereoBus::with_capacity(RING);
        for frame in 0..RING {
            bus.l[frame] = cycle(frame);
            bus.r[frame] = cycle(frame);
        }
        device.process(&context(RING), &mut bus, &[]);

        // Freeze with no crossfade, so nothing is hidden by a fade.
        device.crossfade_ms = 0.0;
        for frame in 0..RING {
            bus.l[frame] = 0.0;
            bus.r[frame] = 0.0;
        }
        device.process_with_params(&context(RING), &mut bus, &[], &[freeze_param(0, 1.0)]);
        assert!(device.is_frozen(), "the writer must have stopped");

        let step = max_step(&bus.l[1..RING]);
        // The sine's own largest step between adjacent frames.
        let cycle_step = core::f32::consts::TAU / CYCLE as f32;
        assert!(
            step < cycle_step * 1.5,
            "the wrap should be no bigger a step than any other sample of the \
             waveform, and it is {step} against {cycle_step}"
        );
        assert_eq!(
            device.collision_count(),
            0,
            "a frozen ring has no writer to collide with"
        );
    }

    /// Unfreezing returns to live, and the writer picks up where it stopped.
    #[test]
    fn unfreezing_returns_to_live_and_restarts_the_writer() {
        let (mut device, mut bus) = primed(4_000);
        fill_ramp(&mut bus, 4_000, 4_000);
        device.process_with_params(&context(4_000), &mut bus, &[], &[freeze_param(0, 1.0)]);
        assert!(device.is_frozen());
        assert!(!device.is_following(), "freezing detaches the head");

        fill_ramp(&mut bus, 8_000, 4_000);
        device.process_with_params(&context(4_000), &mut bus, &[], &[freeze_param(0, 0.0)]);
        assert!(!device.is_frozen(), "the writer must be running again");
        assert!(device.is_following(), "and the head must be back on live");

        // Live means live: the tail of the block is the input, sample for
        // sample, not a delayed stream catching up.
        for frame in 2_000..2_010 {
            assert!(
                (bus.l[frame] - (8_000 + frame) as f32).abs() < 0.001,
                "frame {frame} is {} and the input was {}",
                bus.l[frame],
                (8_000 + frame) as f32
            );
        }
    }

    /// Hysteresis, not a threshold. A modulator resting on the midpoint is
    /// the case this exists for: without it, every control tick would toggle
    /// the writer and every toggle is a crossfade.
    #[test]
    fn freeze_ignores_a_modulator_resting_on_the_threshold() {
        let (mut device, mut bus) = primed(1_000);
        let mut nudges = Vec::new();
        for tick in 0..8u32 {
            // Either side of 0.5 by less than the hysteresis band.
            let value = if tick % 2 == 0 { 0.52 } else { 0.48 };
            nudges.push(freeze_param(tick * 32, value));
        }
        fill_ramp(&mut bus, 1_000, 1_000);
        device.process_with_params(&context(1_000), &mut bus, &[], &nudges);
        assert!(
            !device.is_frozen(),
            "nothing inside the hysteresis band may move the writer"
        );

        // And the band is not a refusal: past it, it freezes.
        device.process_with_params(&context(1_000), &mut bus, &[], &[freeze_param(0, 0.56)]);
        assert!(device.is_frozen());
    }

    /// A gesture running at the moment of freeze keeps running, against the
    /// frozen ring -- and its `Steps` duration still expires, because the
    /// clock is no longer the writer.
    #[test]
    fn a_gesture_survives_a_freeze_and_still_expires() {
        let (mut device, mut bus) = primed(48_000);
        let stutter = BufferEvent {
            offset_beats: -0.25,
            rate: 1.0,
            window_beats: Some(0.25),
            repeat: None,
            duration: BufferDuration::Steps(2),
            crossfade_ms: 0.0,
        };
        // A `Steps(2)` duration at 120 BPM is two sixteenths: 12 000 frames.
        // The blocks below are short enough to land either side of that.
        fill_ramp(&mut bus, 48_000, 4_000);
        device.process(
            &context(4_000),
            &mut bus,
            &[TimedBufferEvent { offset: 0, event: stutter }],
        );
        assert!(!device.is_following(), "the stutter is running");

        fill_ramp(&mut bus, 52_000, 4_000);
        device.process_with_params(&context(4_000), &mut bus, &[], &[freeze_param(0, 1.0)]);
        assert!(device.is_frozen());
        assert!(
            !device.is_following(),
            "the gesture keeps running against the frozen ring"
        );
        assert_eq!(
            device.collision_count(),
            0,
            "and it does not collide with a writer that is not moving"
        );

        // Past 12 000 frames from the event. The writer has not moved since
        // the freeze, so an expiry measured against the write head would never
        // arrive.
        fill_ramp(&mut bus, 56_000, 8_000);
        device.process(&context(8_000), &mut bus, &[]);
        assert!(
            device.is_following(),
            "the gesture's duration must still expire with the writer stopped: \
             the clock is frames elapsed, not the write head"
        );
    }

    /// Freeze is persisted, and a project saved frozen reopens frozen.
    ///
    /// Over an empty ring, because the frozen audio itself is not saved yet.
    /// That is a scoped gap rather than an oversight -- see `BufferParams`.
    #[test]
    fn a_saved_freeze_is_applied_on_the_first_block() {
        let mut device = BufferDevice::new(
            mooloop_core::BufferParams {
                bars: 1,
                freeze: 1.0,
                ..Default::default()
            },
            48_000,
            120.0,
        );
        assert!(
            device.is_writing(),
            "construction has no context, so the freeze waits for a block"
        );
        let mut bus = StereoBus::with_capacity(1_000);
        fill_ramp(&mut bus, 0, 1_000);
        device.process(&context(1_000), &mut bus, &[]);
        assert!(device.is_frozen());
        assert!(!device.is_following());
    }

    #[test]
    fn ring_size_and_memory_are_explicit() {
        let device = BufferDevice::with_bars(48_000, 120.0, 8);
        assert_eq!(device.capacity_frames(), 768_000);
        assert_eq!(device.memory_bytes(), 6_144_000);
    }

    /// A 100 Hz tone as a pure function of the absolute frame. Unlike the
    /// ramp, it is centred on zero, so an equal-power crossfade behaves the
    /// way it does on real audio rather than on a large DC offset.
    fn tone(frame: usize) -> f32 {
        (frame as f32 * core::f32::consts::TAU * 100.0 / 48_000.0).sin()
    }

    /// Drive `total_frames` of `tone` through a device in `block` sized
    /// chunks, firing `event` at absolute frame `event_frame`, and return the
    /// left output. Blocking is the only thing that varies between calls.
    fn render_blocked(
        block: usize,
        total_frames: usize,
        event_frame: usize,
        event: BufferEvent,
    ) -> Vec<f32> {
        let mut device = BufferDevice::with_capacity(100_000);
        let mut bus = StereoBus::with_capacity(block);
        let mut out = Vec::with_capacity(total_frames);
        let mut first = 0;
        while first < total_frames {
            let frames = block.min(total_frames - first);
            for frame in 0..frames {
                bus.l[frame] = tone(first + frame);
                bus.r[frame] = -tone(first + frame);
            }
            // The event carries an in-block offset, so the same absolute
            // frame is addressable no matter where the block boundaries fall.
            let events: &[TimedBufferEvent] = if (first..first + frames).contains(&event_frame) {
                &[TimedBufferEvent {
                    offset: (event_frame - first) as u32,
                    event,
                }]
            } else {
                &[]
            };
            device.process(&context(frames), &mut bus, events);
            out.extend_from_slice(&bus.l[..frames]);
            first += frames;
        }
        out
    }

    fn max_step(samples: &[f32]) -> f32 {
        samples
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0, f32::max)
    }

    /// The head is driven in absolute ring frames, never in per-block ones, so
    /// the period the host happens to run at must not be audible. A stutter
    /// with a crossfade exercises the event, the window wrap, and the return
    /// in a single render.
    #[test]
    fn output_is_identical_across_block_sizes() {
        const TOTAL: usize = 24_000;
        const EVENT_FRAME: usize = 8_192;
        let event = BufferEvent {
            offset_beats: -0.0625,
            rate: 1.0,
            window_beats: Some(0.0625),
            repeat: Some(8),
            duration: BufferDuration::UntilNextEvent,
            crossfade_ms: 2.5,
        };

        let reference = render_blocked(64, TOTAL, EVENT_FRAME, event);
        assert!(
            reference.iter().any(|sample| *sample != 0.0),
            "reference render was silent"
        );
        for block in [128, 256, 1024] {
            let rendered = render_blocked(block, TOTAL, EVENT_FRAME, event);
            assert_eq!(
                reference, rendered,
                "block size {block} changed the rendered output"
            );
        }
    }

    /// Both halves of the crossfade contract: 2 ms must smooth the jump's
    /// discontinuity, and zero must leave it intact. A click is a legitimate
    /// result to ask for, so the second half matters as much as the first.
    #[test]
    fn crossfade_declicks_a_jump_and_zero_leaves_the_click() {
        const TOTAL: usize = 24_000;
        // Fire on a peak of the tone, and jump back to a trough: half a beat
        // is 25 whole periods, so the extra hundredth of a beat lands the read
        // head half a period out of phase — the worst-case step.
        const EVENT_FRAME: usize = 18_120;
        let jump = |crossfade_ms| BufferEvent {
            offset_beats: -0.51,
            rate: 1.0,
            window_beats: None,
            repeat: None,
            duration: BufferDuration::UntilNextEvent,
            crossfade_ms,
        };

        let abrupt = render_blocked(256, TOTAL, EVENT_FRAME, jump(0.0));
        let declicked = render_blocked(256, TOTAL, EVENT_FRAME, jump(2.0));

        // The tone's own slope is under 0.02 per frame, so anything near the
        // full 2.0 peak-to-peak step is the discontinuity itself.
        assert!(
            max_step(&abrupt) > 1.5,
            "zero crossfade must leave the discontinuity: step {}",
            max_step(&abrupt)
        );
        assert!(
            max_step(&declicked) < 0.2,
            "2 ms crossfade must smooth the discontinuity: step {}",
            max_step(&declicked)
        );
    }

    fn gated_reverse(window_beats: Option<f32>) -> BufferEvent {
        BufferEvent {
            offset_beats: 0.0,
            rate: -1.0,
            window_beats,
            repeat: None,
            duration: BufferDuration::Gate,
            crossfade_ms: 0.0,
        }
    }

    /// A held reverse is the gate case: it runs for exactly as long as the
    /// control is down and returns to live the moment it comes up, rather
    /// than latching until it runs out of retained history.
    #[test]
    fn a_gated_head_runs_until_released() {
        let mut device = BufferDevice::with_capacity(100_000);
        let mut bus = StereoBus::with_capacity(1_000);
        for block in 0..10 {
            fill_ramp(&mut bus, block * 1_000, 1_000);
            device.process(&context(1_000), &mut bus, &[]);
        }

        fill_ramp(&mut bus, 10_000, 1_000);
        let event = TimedBufferEvent {
            offset: 0,
            event: gated_reverse(None),
        };
        device.process(&context(1_000), &mut bus, &[event]);
        assert!(!device.is_following(), "a gated head must hold while down");

        fill_ramp(&mut bus, 11_000, 1_000);
        device.process(&context(1_000), &mut bus, &[]);
        assert!(!device.is_following(), "still down, so still detached");

        device.release();
        assert!(device.is_following(), "release must return to live");

        fill_ramp(&mut bus, 12_000, 1_000);
        device.process(&context(1_000), &mut bus, &[]);
        // Genuinely live, not a delayed stream catching up.
        assert_eq!(bus.l[999], 12_999.0);
    }

    /// Releasing is unconditional at the call site, so it must be inert
    /// against a latching head: a held control coming up cannot be allowed to
    /// cancel an unrelated event that superseded its own.
    #[test]
    fn release_leaves_a_latching_head_alone() {
        let mut device = BufferDevice::with_capacity(100_000);
        let mut bus = StereoBus::with_capacity(1_000);
        for block in 0..10 {
            fill_ramp(&mut bus, block * 1_000, 1_000);
            device.process(&context(1_000), &mut bus, &[]);
        }
        fill_ramp(&mut bus, 10_000, 1_000);
        let latching = TimedBufferEvent {
            offset: 0,
            event: BufferEvent {
                offset_beats: -0.05,
                ..BufferEvent::live()
            },
        };
        device.process(&context(1_000), &mut bus, &[latching]);
        assert!(!device.is_following());

        device.release();
        assert!(
            !device.is_following(),
            "a latching head must ignore a gate release"
        );
    }

    /// A reverse window has to cover material behind the entry point. Pointed
    /// forward it would loop over samples the writer has not reached yet, so
    /// a held reverse would play silence instead of repeating the last bars.
    #[test]
    fn a_reverse_window_loops_backward_over_written_history() {
        let mut device = BufferDevice::with_capacity(200_000);
        let mut bus = StereoBus::with_capacity(1_000);
        for block in 0..100 {
            fill_ramp(&mut bus, block * 1_000, 1_000);
            device.process(&context(1_000), &mut bus, &[]);
        }

        // One beat is 24_000 frames at 120 bpm, so the window covers
        // [76_000, 100_000) — all of it long since written.
        fill_ramp(&mut bus, 100_000, 1_000);
        let event = TimedBufferEvent {
            offset: 0,
            event: gated_reverse(Some(1.0)),
        };
        device.process(&context(1_000), &mut bus, &[event]);
        // Reverse from the entry point: the first frame reads the entry
        // sample, and it walks backward from there.
        assert_eq!(bus.l[0], 100_000.0);
        assert_eq!(bus.l[500], 99_500.0);

        // Run past the window's far edge and confirm it wrapped forward to
        // the entry point rather than running off into unwritten samples.
        for block in 101..126 {
            fill_ramp(&mut bus, block * 1_000, 1_000);
            device.process(&context(1_000), &mut bus, &[]);
        }
        assert!(!device.is_following(), "a gated window must keep looping");
        assert!(
            bus.l[..1_000]
                .iter()
                .all(|sample| *sample >= 76_000.0 && *sample <= 100_000.0),
            "reverse window left its retained range"
        );
    }

    /// The defining property of position mode: the platter's speed sets the
    /// pitch, and letting go goes silent rather than holding a sample as DC.
    #[test]
    fn a_stopped_scrub_goes_silent_rather_than_holding_dc() {
        let mut device = BufferDevice::with_capacity(100_000);
        let mut bus = StereoBus::with_capacity(1_000);
        for block in 0..20 {
            fill_ramp(&mut bus, block * 1_000, 1_000);
            device.process(&context(1_000), &mut bus, &[]);
        }

        // Spin: a large target displacement, so the head has ground to cover.
        device.scrub_begin(&context(1_000), 0.0);
        device.scrub_move(-4_000.0);
        assert!(device.is_scrubbing());
        fill_ramp(&mut bus, 20_000, 1_000);
        device.process(&context(1_000), &mut bus, &[]);
        let moving = bus.l[..1_000].iter().map(|s| s.abs()).fold(0.0, f32::max);
        assert!(moving > 0.0, "a moving platter must make sound");

        // Let go: no further movement, so the head coasts to a stop and the
        // gain follows it down.
        for _ in 0..20 {
            fill_ramp(&mut bus, 21_000, 1_000);
            device.process(&context(1_000), &mut bus, &[]);
        }
        let resting = bus.l[..1_000].iter().map(|s| s.abs()).fold(0.0, f32::max);
        assert!(
            resting < 1e-3,
            "a stopped platter must be silent, got {resting}"
        );
    }

    /// Scrub is driven by a stream of deltas with no press, so the first one
    /// has to detach the head and later ones must not snap it back to live.
    #[test]
    fn scrub_begin_is_idempotent_and_release_returns_live() {
        let mut device = BufferDevice::with_capacity(100_000);
        let mut bus = StereoBus::with_capacity(1_000);
        for block in 0..20 {
            fill_ramp(&mut bus, block * 1_000, 1_000);
            device.process(&context(1_000), &mut bus, &[]);
        }

        device.scrub_begin(&context(1_000), 0.0);
        device.scrub_move(-2_000.0);
        fill_ramp(&mut bus, 20_000, 1_000);
        device.process(&context(1_000), &mut bus, &[]);
        let drifted = device.head.expect("scrub head").position;

        // A second begin must be inert rather than re-detaching at live.
        device.scrub_begin(&context(1_000), 0.0);
        assert_eq!(device.head.expect("scrub head").position, drifted);

        device.release();
        assert!(device.is_following(), "release must end a scrub");
        assert!(!device.is_scrubbing());
        fill_ramp(&mut bus, 21_000, 1_000);
        device.process(&context(1_000), &mut bus, &[]);
        assert_eq!(bus.l[999], 21_999.0, "must land on genuinely live audio");
    }
}
