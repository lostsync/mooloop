//! Retained-audio buffer device core.
//!
//! The device is deliberately independent of project and UI plumbing: its
//! public event tuple is the contract that the sequencer, parameter locks, and
//! debug triggers will share. Construction allocates the ring; [`process`] is
//! allocation-free and operates in place on an existing [`StereoBus`].

use mooloop_core::{BufferDuration, BufferEvent, BufferParams};

use crate::{AudioNode, Event, EventList, ProcessContext, StereoBus};

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
}

#[derive(Clone, Copy)]
enum FadeSource {
    Live,
    Detached { position: f64, rate: f32 },
}

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
    /// Incremented when the writer catches a detached head. This is a cheap
    /// observable diagnostic for the host/tests without logging on the RT
    /// thread.
    collision_count: u64,
}

impl BufferDevice {
    pub fn new(params: BufferParams, sample_rate: u32, bpm: f64) -> Self {
        Self::with_bars(sample_rate, bpm, u32::from(params.bars.max(1)))
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
            collision_count: 0,
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
        debug_assert!(context.frames <= bus.capacity());
        let mut event_index = 0;
        for frame in 0..context.frames {
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
                Some(head) => self.read_stereo(head.position),
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

    fn fire(&mut self, event: BufferEvent, context: &ProcessContext) {
        let frames_per_beat = context.sample_rate as f64 * 60.0 / context.bpm.max(1.0);
        let position = self.write_head as f64 + f64::from(event.offset_beats) * frames_per_beat;
        let window_end = event.window_beats.and_then(|beats| {
            (beats > 0.0).then_some(position + f64::from(beats) * frames_per_beat)
        });
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
            window_start: position,
            window_end,
            repeats_remaining: event.repeat,
            expires_at: duration_frames.map(|frames| self.write_head + frames),
            crossfade_frames: ms_to_frames(event.crossfade_ms, context.sample_rate),
        };
        self.fade = (head.crossfade_frames > 0).then_some(Fade {
            source: FadeSource::Live,
            frame: 0,
            frames: head.crossfade_frames,
        });
        self.head = Some(head);
    }

    fn advance_head(&mut self) {
        let Some(mut head) = self.head else { return };
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
        let mut buffer_events = [EMPTY; 256];
        let mut len = 0;
        for timed in events_in.iter() {
            if let Event::Buffer(event) = timed.event {
                buffer_events[len] = TimedBufferEvent {
                    offset: timed.offset,
                    event,
                };
                len += 1;
            }
        }
        self.process(context, bus, &buffer_events[..len]);
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
}
