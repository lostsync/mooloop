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

#[derive(Clone, Copy)]
struct ReadHead {
    position: f64,
    rate: f32,
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
/// Ceiling on how fast a scrub may drive the head, so a wild spin cannot
/// outrun the interpolator into noise.
const MAX_SCRUB_RATE: f32 = 4.0;

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
    /// Amplitude the scrub is currently entitled to, from its speed. Held
    /// across frames so the mute fades rather than steps.
    scrub_gain: f32,
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
        device.pending_offset_beats = (params.offset_beats > 0.0).then_some(params.offset_beats);
        device
    }
    /// Allocate a ring for `bars` 4/4 bars at the supplied tempo.
    pub fn with_bars(sample_rate: u32, bpm: f64, bars: u32) -> Self {
        let frames_per_bar = (sample_rate as f64 * 240.0 / bpm.max(1.0)).ceil() as usize;
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
            scrub_gain: 0.0,
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
            let write_index = self.index(self.write_head as f64);
            // Writer is unconditional and stores the device input, never its
            // output. This must happen even when the read head is detached.
            self.left[write_index] = input_l;
            self.right[write_index] = input_r;

            let (mut output_l, mut output_r) = match self.head {
                Some(head) => {
                    let (left, right) = self.read_stereo(head.position);
                    if self.scrub.is_some() {
                        (left * self.scrub_gain, right * self.scrub_gain)
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
            self.write_head += 1;
            self.advance_head();
        }
    }

    fn set_param(&mut self, id: u32, value: f32, context: &ProcessContext) {
        match id {
            mooloop_core::BUFFER_PARAM_OFFSET_BEATS => self.set_offset_beats(value, context),
            mooloop_core::BUFFER_PARAM_CROSSFADE_MS => {
                self.crossfade_ms = value.clamp(0.0, 50.0)
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
            window_start,
            window_end,
            repeats_remaining: event.repeat,
            expires_at: duration_frames.map(|frames| self.write_head + frames),
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
        self.scrub_gain = 0.0;
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
            self.scrub_gain = (head.rate.abs() / SCRUB_MUTE_RATE).min(1.0);
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
        let oldest = writer - self.capacity_frames() as f64;
        let expired = head
            .expires_at
            .is_some_and(|frame| self.write_head >= frame);
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
        self.scrub_gain = 0.0;
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

impl AudioNode for BufferDevice {
    fn buffer_collisions(&self) -> u64 {
        self.collision_count
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
