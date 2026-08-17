//! A sample-playback instrument. The first real `AudioNode` in mooloop.
//!
//! Behaviour:
//! - On a `NoteOn` event, captures the currently-published sample (from the
//!   shared `ArcSwapOption` slot) and starts a voice from `params.start`.
//! - The voice runs through an ADSR amplitude envelope. In loop mode `Off`,
//!   reaching `loop_end` enters release; in `Forward`/`Pingpong`, the voice
//!   loops until retrigged or released.
//! - Sample rate conversion is linear-interpolated; can be upgraded later.
//!
//! Processing is **segment-based**: the block is split at each event's
//! sample offset, the voice renders the segment, then the event is applied.
//! This keeps note timing sample-accurate at any block size without a
//! per-sample event scan.

use std::sync::Arc;

use crate::bus::StereoBus;
use crate::event::{Event, EventList};
use crate::node::{AudioNode, ProcessContext};
use mooloop_core::{clamp01, LoopMode, SamplerParams};

use arc_swap::ArcSwapOption;

/// Minimum envelope stage time, to avoid divide-by-zero and infinite rates.
const MIN_STAGE_S: f32 = 1.0e-4;

/// Decoded sample data: stereo frames of f32 in `[-1, 1]`, plus the source
/// sample rate and a root MIDI note (default middle-C).
pub struct SampleData {
    pub frames: Vec<[f32; 2]>,
    pub sample_rate: u32,
    pub root_note: u8,
}

impl SampleData {
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// A punchy synthesised kick so the app makes sound out of the box. The
    /// user can load a real WAV to replace it.
    pub fn default_kick(sample_rate: u32) -> Arc<Self> {
        let dur_s = 0.25;
        let n = (dur_s * sample_rate as f64) as usize;
        let mut frames = Vec::with_capacity(n);
        let mut phase = 0.0_f64;
        for i in 0..n {
            let t = i as f64 / sample_rate as f64;
            // Exponential pitch drop 150 Hz -> 50 Hz across the body.
            let pitch = 150.0 * (50.0_f64 / 150.0).powf(t / dur_s);
            phase += pitch / sample_rate as f64;
            if phase >= 1.0 {
                phase -= 1.0;
            }
            let body = (phase * core::f64::consts::TAU).sin();
            // Click at the very start for beater attack.
            let click = if t < 0.003 { (1.0 - t / 0.003) * 0.6 } else { 0.0 };
            let amp = (-t * 12.0).exp();
            let s = ((body + click) * amp) as f32;
            frames.push([s, s]);
        }
        Arc::new(Self {
            frames,
            sample_rate,
            root_note: 60,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// A scalar ADSR envelope. `advance` moves it one sample; the caller reads
/// `level` to shape amplitude.
#[derive(Clone, Copy, Debug)]
struct AdsrEnv {
    stage: Stage,
    level: f32,
    attack_inc: f32,
    decay_dec: f32,
    sustain: f32,
    release_dec: f32,
    release_s: f32,
    sample_rate: u32,
}

impl AdsrEnv {
    fn new(sample_rate: u32) -> Self {
        Self {
            stage: Stage::Idle,
            level: 0.0,
            attack_inc: 0.0,
            decay_dec: 0.0,
            sustain: 0.0,
            release_dec: 0.0,
            release_s: MIN_STAGE_S,
            sample_rate,
        }
    }

    /// Recompute rates from a parameter set.
    fn configure(&mut self, p: SamplerParams) {
        let sr = self.sample_rate as f32;
        self.attack_inc = 1.0 / (p.attack.max(MIN_STAGE_S) * sr);
        self.decay_dec = (1.0 - p.sustain) / (p.decay.max(MIN_STAGE_S) * sr);
        self.sustain = clamp01(p.sustain);
        self.release_s = p.release.max(MIN_STAGE_S);
    }

    fn note_on(&mut self) {
        self.stage = Stage::Attack;
        self.level = 0.0;
    }

    /// Enter release from the current level.
    fn release(&mut self) {
        self.release_dec = self.level / (self.release_s * self.sample_rate as f32);
        self.stage = Stage::Release;
    }

    fn advance(&mut self) {
        match self.stage {
            Stage::Idle => self.level = 0.0,
            Stage::Attack => {
                self.level += self.attack_inc;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.level -= self.decay_dec;
                if self.level <= self.sustain {
                    self.level = self.sustain;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => {}
            Stage::Release => {
                self.level -= self.release_dec;
                if self.level <= 0.0 {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }
    }

    fn is_idle(&self) -> bool {
        self.stage == Stage::Idle
    }
}

/// One playback voice. Monophonic for now — a new note retriggers and cuts
/// the previous voice (standard drum-sampler behaviour).
struct Voice {
    sample: Option<Arc<SampleData>>,
    play_pos: f64,
    direction: f64,
    env: AdsrEnv,
    velocity_amp: f32,
    active: bool,
}

impl Voice {
    fn new(sample_rate: u32) -> Self {
        Self {
            sample: None,
            play_pos: 0.0,
            direction: 1.0,
            env: AdsrEnv::new(sample_rate),
            velocity_amp: 0.0,
            active: false,
        }
    }
}

/// The sampler node.
pub struct Sampler {
    sample_slot: Arc<ArcSwapOption<SampleData>>,
    params: SamplerParams,
    sample_rate: u32,
    voice: Voice,
}

impl Sampler {
    /// Construct with a shared sample slot. The engine publishes samples into
    /// the same slot from the non-RT thread.
    pub fn new(
        sample_slot: Arc<ArcSwapOption<SampleData>>,
        params: SamplerParams,
        sample_rate: u32,
    ) -> Self {
        let mut voice = Voice::new(sample_rate);
        voice.env.configure(params);
        Self {
            sample_slot,
            params,
            sample_rate,
            voice,
        }
    }

    /// Replace the parameter set. Called from the RT command drain.
    pub fn set_params(&mut self, params: SamplerParams) {
        self.params = params;
        self.voice.env.configure(params);
    }

    fn trigger(&mut self, velocity: u8) {
        let sample = self.sample_slot.load_full();
        self.voice.sample = sample;
        self.voice.active = self.voice.sample.is_some();
        if !self.voice.active {
            return;
        }
        let len = self.voice.sample.as_ref().unwrap().len().max(1);
        let start = clamp01(self.params.start) * len as f32;
        self.voice.play_pos = start as f64;
        self.voice.direction = 1.0;
        self.voice.velocity_amp = (velocity as f32) / 127.0;
        self.voice.env.note_on();
    }

    /// Normalized loop bounds resolved against the current sample length.
    fn loop_bounds(&self, len: usize) -> (f64, f64) {
        let l = len.max(1) as f32;
        let ls = clamp01(self.params.loop_start) * l;
        let le = clamp01(self.params.loop_end).max(ls + 1.0) * l;
        (ls as f64, le as f64)
    }

    /// Render the voice into `bus[start..end]`, adding into the buffers.
    /// Handles looping, envelope advancement, and voice termination.
    fn render_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        if !self.voice.active {
            return;
        }
        let engine_sr = self.sample_rate as f64;

        for i in start..end {
            let Some(sample) = self.voice.sample.as_ref() else {
                self.voice.active = false;
                return;
            };
            let len = sample.len();
            if len == 0 {
                self.voice.active = false;
                return;
            }

            // Advance envelope; if it finished during release, end voice.
            self.voice.env.advance();
            if self.voice.env.is_idle() {
                self.voice.active = false;
                return;
            }
            let amp = self.voice.env.level * self.voice.velocity_amp;

            // Fetch interpolated frame.
            let pos = self.voice.play_pos;
            let idx = pos.floor() as isize;
            let frac = pos - idx as f64;
            let frame_at = |k: isize| -> [f32; 2] {
                if k < 0 {
                    return [0.0, 0.0];
                }
                let k = k as usize;
                if k >= len {
                    sample.frames[len - 1]
                } else {
                    sample.frames[k]
                }
            };
            let f0 = frame_at(idx);
            let f1 = frame_at(idx + 1);
            let l = f0[0] + (f1[0] - f0[0]) * frac as f32;
            let r = f0[1] + (f1[1] - f0[1]) * frac as f32;
            bus.l[i] += amp * l;
            bus.r[i] += amp * r;

            // Advance the read position and handle looping / end-of-region.
            let inc = sample.sample_rate as f64 / engine_sr;
            self.voice.play_pos += self.voice.direction * inc;

            let (ls, le) = self.loop_bounds(len);
            match self.params.loop_mode {
                LoopMode::Off => {
                    if self.voice.play_pos >= le {
                        self.voice.env.release();
                    }
                }
                LoopMode::Forward => {
                    if self.voice.play_pos >= le {
                        self.voice.play_pos = ls + (self.voice.play_pos - le);
                    }
                }
                LoopMode::Pingpong => {
                    if self.voice.direction > 0.0 && self.voice.play_pos >= le {
                        self.voice.play_pos = le - (self.voice.play_pos - le);
                        self.voice.direction = -1.0;
                    } else if self.voice.direction < 0.0 && self.voice.play_pos <= ls {
                        self.voice.play_pos = ls + (ls - self.voice.play_pos);
                        self.voice.direction = 1.0;
                    }
                }
            }
        }
    }
}

impl AudioNode for Sampler {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        let frames = ctx.frames.min(bus.capacity());

        // Split the block at event offsets: render, apply event, repeat.
        let mut pos = 0usize;
        for ev in events_in.iter() {
            let off = (ev.offset as usize).min(frames).max(pos);
            self.render_range(bus, pos, off);
            match ev.event {
                Event::NoteOn { velocity, .. } => self.trigger(velocity),
                Event::NoteOff { .. } => self.voice.env.release(),
                Event::ParamValue { .. } => {}
            }
            pos = off;
        }
        self.render_range(bus, pos, frames);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::TimedEvent;

    fn make_sampler(sr: u32) -> Sampler {
        let kick = SampleData::default_kick(sr);
        let slot: Arc<ArcSwapOption<SampleData>> = Arc::new(ArcSwapOption::from(Some(kick)));
        Sampler::new(slot, SamplerParams::default(), sr)
    }

    fn ctx(frames: usize, sr: u32) -> ProcessContext {
        ProcessContext {
            sample_rate: sr,
            frames,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        }
    }

    /// A note at offset K must produce exact silence before K and signal
    /// after — the point of segment-based processing.
    #[test]
    fn note_on_at_offset_is_sample_accurate() {
        let sr = 48_000;
        let frames = 512;
        let k = 200usize;
        let mut sampler = make_sampler(sr);
        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        events.push(TimedEvent {
            offset: k as u32,
            event: Event::NoteOn {
                note: 60,
                velocity: 127,
            },
        });

        sampler.process(&ctx(frames, sr), &mut bus, &events, None);

        assert!(bus.l[..k].iter().all(|s| *s == 0.0));
        assert!(bus.l[k..].iter().any(|s| s.abs() > 0.01));
    }

    /// No events, no prior note: silence (and no panic on the empty path).
    #[test]
    fn idle_is_silent() {
        let sr = 48_000;
        let mut sampler = make_sampler(sr);
        let mut bus = StereoBus::with_capacity(256);
        sampler.process(&ctx(256, sr), &mut bus, &EventList::empty(), None);
        assert_eq!(bus.peak(256), (0.0, 0.0));
    }
}
