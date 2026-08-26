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
use crate::scale::hz_from_normalized;
use mooloop_core::{
    clamp01, LoopMode, RetriggerMode, SamplerParams, VoiceMode, MAX_CHOKE_GROUP, MAX_SAMPLER_VOICES,
};

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
            let click = if t < 0.003 {
                (1.0 - t / 0.003) * 0.6
            } else {
                0.0
            };
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

    fn release_with(&mut self, seconds: f32) {
        self.release_dec = self.level / (seconds.max(MIN_STAGE_S) * self.sample_rate as f32);
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

    fn is_releasing(&self) -> bool {
        self.stage == Stage::Release
    }
}

const CHOKE_RELEASE_S: f32 = 0.005;

/// One independently enveloped sample playback voice.
struct Voice {
    event_id: u64,
    midi_note: u8,
    age: u64,
    sample: Option<Arc<SampleData>>,
    play_pos: f64,
    playback_rate: f64,
    direction: f64,
    env: AdsrEnv,
    velocity_amp: f32,
    filter_low: [f32; 2],
    filter_band: [f32; 2],
    held_frame: [f32; 2],
    hold_remaining: u32,
    loop_enabled: bool,
    active: bool,
}

impl Voice {
    fn new(sample_rate: u32) -> Self {
        Self {
            event_id: 0,
            midi_note: 60,
            age: 0,
            sample: None,
            play_pos: 0.0,
            playback_rate: 1.0,
            direction: 1.0,
            env: AdsrEnv::new(sample_rate),
            velocity_amp: 0.0,
            filter_low: [0.0, 0.0],
            filter_band: [0.0, 0.0],
            held_frame: [0.0, 0.0],
            hold_remaining: 0,
            loop_enabled: false,
            active: false,
        }
    }

    /// Clear audible voice state while retaining the sample handle. Keeping
    /// that `Arc` avoids moving a potentially large sample deallocation onto
    /// the realtime thread when a channel changes source.
    fn reset(&mut self, params: SamplerParams, sample_rate: u32) {
        self.event_id = 0;
        self.midi_note = 60;
        self.age = 0;
        self.play_pos = 0.0;
        self.playback_rate = 1.0;
        self.direction = 1.0;
        self.env = AdsrEnv::new(sample_rate);
        self.env.configure(params);
        self.velocity_amp = 0.0;
        self.filter_low = [0.0, 0.0];
        self.filter_band = [0.0, 0.0];
        self.held_frame = [0.0, 0.0];
        self.hold_remaining = 0;
        self.loop_enabled = false;
        self.active = false;
    }
}

/// The sampler node.
pub struct Sampler {
    sample_slot: Arc<ArcSwapOption<SampleData>>,
    params: SamplerParams,
    sample_rate: u32,
    voices: [Voice; MAX_SAMPLER_VOICES as usize],
    next_age: u64,
}

impl Sampler {
    /// Construct with a shared sample slot. The engine publishes samples into
    /// the same slot from the non-RT thread.
    pub fn new(
        sample_slot: Arc<ArcSwapOption<SampleData>>,
        mut params: SamplerParams,
        sample_rate: u32,
    ) -> Self {
        params.polyphony = params.polyphony.clamp(1, MAX_SAMPLER_VOICES);
        params.choke_group = params.choke_group.min(MAX_CHOKE_GROUP);
        let mut voices = std::array::from_fn(|_| Voice::new(sample_rate));
        for voice in &mut voices {
            voice.env.configure(params);
        }
        Self {
            sample_slot,
            params,
            sample_rate,
            voices,
            next_age: 1,
        }
    }

    /// Replace the parameter set. Called from the RT command drain.
    pub fn set_params(&mut self, mut params: SamplerParams) {
        params.polyphony = params.polyphony.clamp(1, MAX_SAMPLER_VOICES);
        params.choke_group = params.choke_group.min(MAX_CHOKE_GROUP);
        self.params = params;
        for (index, voice) in self.voices.iter_mut().enumerate() {
            voice.env.configure(params);
            if index >= params.polyphony as usize {
                voice.active = false;
            }
        }
    }

    pub fn choke_group(&self) -> u8 {
        self.params.choke_group
    }

    /// Normalized (0..1) playback position of every active voice, for a UI
    /// playhead. Inactive slots (and voices with no loaded sample) report
    /// `f32::NAN`, which the caller filters out rather than drawing a
    /// playhead at frame zero.
    pub fn voice_positions(&self) -> [f32; MAX_SAMPLER_VOICES as usize] {
        let mut positions = [f32::NAN; MAX_SAMPLER_VOICES as usize];
        for (voice, position) in self.voices.iter().zip(positions.iter_mut()) {
            if let (true, Some(sample)) = (voice.active, voice.sample.as_ref()) {
                let len = sample.len().max(1) as f64;
                *position = (voice.play_pos / len).clamp(0.0, 1.0) as f32;
            }
        }
        positions
    }

    /// Immediately invalidate all active voices. Sample handles remain owned
    /// by their slots/voices so resetting is allocation- and drop-free.
    pub fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.reset(self.params, self.sample_rate);
        }
        self.next_age = 1;
    }

    fn voice_limit(&self) -> usize {
        self.params.polyphony.clamp(1, MAX_SAMPLER_VOICES) as usize
    }

    fn select_voice(&self, midi_note: u8) -> usize {
        let voices = &self.voices[..self.voice_limit()];
        if self.params.retrigger_mode == RetriggerMode::Restart {
            if let Some((index, _)) = voices
                .iter()
                .enumerate()
                .filter(|(_, voice)| voice.active && voice.midi_note == midi_note)
                .min_by_key(|(_, voice)| voice.age)
            {
                return index;
            }
        }
        if let Some(index) = voices.iter().position(|voice| !voice.active) {
            return index;
        }
        voices
            .iter()
            .enumerate()
            .min_by_key(|(_, voice)| voice.age)
            .map(|(index, _)| index)
            .unwrap_or(0)
    }

    fn trigger(&mut self, event_id: u64, note: u8, velocity: u8) {
        let Some(sample) = self.sample_slot.load_full() else {
            return;
        };
        let index = self.select_voice(note);
        let len = sample.len().max(1);
        let (start, end) = Self::resolve_playback_bounds(self.params, len);
        let root_note = self.params.root_note.min(127);
        let key_semitones = i16::from(note.min(127)) - i16::from(root_note);
        let tuning = f64::from(self.params.tune_semitones.clamp(-48.0, 48.0))
            + f64::from(self.params.tune_cents.clamp(-100.0, 100.0)) / 100.0;
        let pitch_ratio = 2.0_f64.powf((f64::from(key_semitones) + tuning) / 12.0);
        let age = self.next_age;
        self.next_age = self.next_age.wrapping_add(1).max(1);

        let voice = &mut self.voices[index];
        voice.event_id = event_id;
        voice.midi_note = note;
        voice.age = age;
        voice.sample = Some(sample.clone());
        voice.active = !sample.is_empty();
        if !voice.active {
            return;
        }
        voice.play_pos = if self.params.reverse {
            end - 1.0
        } else {
            start
        };
        voice.playback_rate = sample.sample_rate as f64 / self.sample_rate as f64 * pitch_ratio;
        voice.direction = if self.params.reverse { -1.0 } else { 1.0 };
        voice.velocity_amp = f32::from(velocity) / 127.0;
        voice.filter_low = [0.0, 0.0];
        voice.filter_band = [0.0, 0.0];
        voice.held_frame = [0.0, 0.0];
        voice.hold_remaining = 0;
        voice.loop_enabled = self.params.loop_mode != LoopMode::Off;
        voice.env.configure(self.params);
        voice.env.note_on();
    }

    fn release_note(&mut self, event_id: u64) {
        let mode = self.params.voice_mode;
        for voice in self
            .voices
            .iter_mut()
            .filter(|voice| voice.active && voice.event_id == event_id)
        {
            match mode {
                VoiceMode::Gate => voice.env.release(),
                VoiceMode::OneShot if voice.loop_enabled => voice.loop_enabled = false,
                VoiceMode::OneShot => {}
            }
        }
    }

    fn release_all(&mut self) {
        for voice in self.voices.iter_mut().filter(|voice| voice.active) {
            if !voice.env.is_releasing() {
                voice.env.release();
            }
        }
    }

    pub fn choke(&mut self) {
        for voice in self.voices.iter_mut().filter(|voice| voice.active) {
            voice.loop_enabled = false;
            voice.env.release_with(CHOKE_RELEASE_S);
        }
    }

    fn shape_frame(
        params: SamplerParams,
        sample_rate: u32,
        voice: &mut Voice,
        frame: [f32; 2],
    ) -> [f32; 2] {
        let rate_reduction = clamp01(params.rate_reduction);
        let hold_frames = 1 + (rate_reduction * 31.0).round() as u32;
        if voice.hold_remaining == 0 {
            let bit_reduction = clamp01(params.bit_reduction);
            voice.held_frame = if bit_reduction <= f32::EPSILON {
                frame
            } else {
                let bits = (16.0 - bit_reduction * 12.0).round().clamp(4.0, 16.0);
                let scale = 2.0_f32.powf(bits - 1.0);
                [
                    (frame[0] * scale).round() / scale,
                    (frame[1] * scale).round() / scale,
                ]
            };
            voice.hold_remaining = hold_frames;
        }
        voice.hold_remaining -= 1;
        let mut frame = voice.held_frame;

        let drive = clamp01(params.drive);
        if drive > f32::EPSILON {
            let input_gain = 1.0 + drive * 15.0;
            let compensation = input_gain.tanh().recip();
            frame = [
                (frame[0] * input_gain).tanh() * compensation,
                (frame[1] * input_gain).tanh() * compensation,
            ];
        }

        let cutoff = clamp01(params.filter_cutoff);
        let env_amount = params.filter_env_amount.clamp(-1.0, 1.0);
        let resonance = clamp01(params.filter_resonance);
        if cutoff >= 0.999 && env_amount.abs() <= f32::EPSILON && resonance <= f32::EPSILON {
            return frame;
        }
        let max_hz = sample_rate as f32 * 0.45;
        let base_hz = hz_from_normalized(cutoff, max_hz);
        let cutoff_hz =
            (base_hz * 2.0_f32.powf(voice.env.level * env_amount * 6.0)).clamp(20.0, max_hz);
        // Topology-preserving state-variable low-pass. Unlike a biquad this
        // remains well behaved while cutoff and envelope move every sample.
        let g = (core::f32::consts::PI * cutoff_hz / sample_rate as f32).tan();
        let damping = (2.0 - resonance * 1.9).clamp(0.1, 2.0);
        let a1 = 1.0 / (1.0 + g * (g + damping));
        let a2 = g * a1;
        let a3 = g * a2;
        for (channel, output) in frame.iter_mut().enumerate() {
            let input = *output;
            let v3 = input - voice.filter_low[channel];
            let v1 = a1 * voice.filter_band[channel] + a2 * v3;
            let v2 = voice.filter_low[channel] + a2 * voice.filter_band[channel] + a3 * v3;
            voice.filter_band[channel] = 2.0 * v1 - voice.filter_band[channel];
            voice.filter_low[channel] = 2.0 * v2 - voice.filter_low[channel];
            *output = v2;
        }
        frame
    }

    /// Normalized playback region resolved against the current sample length.
    fn resolve_playback_bounds(params: SamplerParams, len: usize) -> (f64, f64) {
        let len = len.max(1) as f64;
        let start = f64::from(clamp01(params.start)) * len;
        let end = (f64::from(clamp01(params.end)) * len)
            .max(start + 1.0)
            .min(len);
        (start.min(end - 1.0), end)
    }

    #[cfg(test)]
    fn playback_bounds(&self, len: usize) -> (f64, f64) {
        Self::resolve_playback_bounds(self.params, len)
    }

    /// Normalized loop bounds resolved against the current sample length.
    fn resolve_loop_bounds(params: SamplerParams, len: usize) -> (f64, f64) {
        let len_f = len.max(1) as f64;
        let (play_start, play_end) = Self::resolve_playback_bounds(params, len);
        let loop_start =
            (f64::from(clamp01(params.loop_start)) * len_f).clamp(play_start, play_end - 1.0);
        let loop_end = (f64::from(clamp01(params.loop_end)) * len_f)
            .max(loop_start + 1.0)
            .min(play_end);
        (loop_start.min(loop_end - 1.0), loop_end)
    }

    #[cfg(test)]
    fn loop_bounds(&self, len: usize) -> (f64, f64) {
        Self::resolve_loop_bounds(self.params, len)
    }

    /// Render the voice into `bus[start..end]`, adding into the buffers.
    /// Handles looping, envelope advancement, and voice termination.
    fn render_voice_range(
        params: SamplerParams,
        sample_rate: u32,
        voice: &mut Voice,
        bus: &mut StereoBus,
        start: usize,
        end: usize,
    ) {
        if !voice.active {
            return;
        }
        for i in start..end {
            let Some(sample) = voice.sample.as_ref() else {
                voice.active = false;
                return;
            };
            let len = sample.len();
            if len == 0 {
                voice.active = false;
                return;
            }

            // Advance envelope; if it finished during release, end voice.
            voice.env.advance();
            if voice.env.is_idle() {
                voice.active = false;
                return;
            }
            let amp = voice.env.level * voice.velocity_amp;

            // Fetch interpolated frame.
            let pos = voice.play_pos;
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
            let frame = Self::shape_frame(
                params,
                sample_rate,
                voice,
                [
                    f0[0] + (f1[0] - f0[0]) * frac as f32,
                    f0[1] + (f1[1] - f0[1]) * frac as f32,
                ],
            );
            bus.l[i] += amp * frame[0];
            bus.r[i] += amp * frame[1];

            // Advance the read position and handle looping / end-of-region.
            voice.play_pos += voice.direction * voice.playback_rate;

            let (play_start, play_end) = Self::resolve_playback_bounds(params, len);
            let (ls, le) = Self::resolve_loop_bounds(params, len);
            let loop_mode = if voice.loop_enabled {
                params.loop_mode
            } else {
                LoopMode::Off
            };
            match loop_mode {
                LoopMode::Off => {
                    let reached_end = voice.direction > 0.0 && voice.play_pos >= play_end;
                    let reached_start = voice.direction < 0.0 && voice.play_pos < play_start;
                    if reached_end || reached_start {
                        // There is no audio beyond a non-looping region, so an
                        // envelope tail would only hold its final sample value.
                        voice.active = false;
                        return;
                    }
                }
                LoopMode::Forward => {
                    if voice.direction > 0.0 && voice.play_pos >= le {
                        voice.play_pos = ls + (voice.play_pos - le);
                    } else if voice.direction < 0.0 && voice.play_pos < ls {
                        voice.play_pos = le - (ls - voice.play_pos);
                    }
                }
                LoopMode::Pingpong => {
                    if voice.direction > 0.0 && voice.play_pos >= le {
                        voice.play_pos = le - (voice.play_pos - le);
                        voice.direction = -1.0;
                    } else if voice.direction < 0.0 && voice.play_pos <= ls {
                        voice.play_pos = ls + (ls - voice.play_pos);
                        voice.direction = 1.0;
                    }
                }
            }
        }
    }

    fn render_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        let params = self.params;
        let sample_rate = self.sample_rate;
        for voice in &mut self.voices[..params.polyphony.clamp(1, MAX_SAMPLER_VOICES) as usize] {
            Self::render_voice_range(params, sample_rate, voice, bus, start, end);
        }
    }

    #[cfg(test)]
    fn active_voice_count(&self) -> usize {
        self.voices.iter().filter(|voice| voice.active).count()
    }

    #[cfg(test)]
    fn shape_first_voice(&mut self, frame: [f32; 2]) -> [f32; 2] {
        Self::shape_frame(self.params, self.sample_rate, &mut self.voices[0], frame)
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

        if !ctx.playing {
            self.release_all();
        }

        // Split the block at event offsets: render, apply event, repeat.
        let mut pos = 0usize;
        for ev in events_in.iter() {
            let off = (ev.offset as usize).min(frames).max(pos);
            self.render_range(bus, pos, off);
            match ev.event {
                Event::NoteOn { id, note, velocity } => self.trigger(id, note, velocity),
                Event::NoteOff { id, .. } => self.release_note(id),
                Event::Choke => self.choke(),
                Event::ParamValue { .. } | Event::Buffer(_) | Event::BufferRelease => {}
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

    fn sampler_with_frames(sr: u32, len: usize, params: SamplerParams) -> Sampler {
        let frames = (0..len)
            .map(|index| {
                let value = index as f32 / len.max(1) as f32;
                [value, value]
            })
            .collect();
        let sample = Arc::new(SampleData {
            frames,
            sample_rate: sr,
            root_note: 60,
        });
        let slot = Arc::new(ArcSwapOption::from(Some(sample)));
        Sampler::new(slot, params, sr)
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
                id: 0,
                note: 60,
                velocity: 127,
            },
        });

        sampler.process(&ctx(frames, sr), &mut bus, &events, None);

        assert!(bus.l[..k].iter().all(|s| *s == 0.0));
        assert!(bus.l[k..].iter().any(|s| s.abs() > 0.01));
    }

    #[test]
    fn stale_note_off_does_not_release_a_retriggered_voice() {
        let sr = 48_000;
        let mut sampler = make_sampler(sr);
        sampler.set_params(SamplerParams {
            voice_mode: VoiceMode::Gate,
            ..SamplerParams::default()
        });
        let mut bus = StereoBus::with_capacity(64);
        let mut events = EventList::empty();
        events.push(TimedEvent {
            offset: 0,
            event: Event::NoteOn {
                id: 1,
                note: 60,
                velocity: 100,
            },
        });
        events.push(TimedEvent {
            offset: 8,
            event: Event::NoteOn {
                id: 2,
                note: 64,
                velocity: 100,
            },
        });
        events.push(TimedEvent {
            offset: 16,
            event: Event::NoteOff { id: 1, note: 60 },
        });

        sampler.process(&ctx(64, sr), &mut bus, &events, None);

        assert_eq!(sampler.voices[0].event_id, 2);
        assert!(!sampler.voices[0].env.is_releasing());
    }

    #[test]
    fn one_shot_ignores_note_off_without_a_loop() {
        let mut sampler = sampler_with_frames(48_000, 4096, SamplerParams::default());
        sampler.trigger(1, 60, 100);
        sampler.release_note(1);
        assert!(!sampler.voices[0].env.is_releasing());
        assert!(sampler.voices[0].active);
    }

    #[test]
    fn gated_note_enters_release_on_matching_note_off() {
        let params = SamplerParams {
            voice_mode: VoiceMode::Gate,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(48_000, 4096, params);
        sampler.trigger(7, 60, 100);
        sampler.release_note(7);
        assert!(sampler.voices[0].env.is_releasing());
    }

    #[test]
    fn one_shot_note_off_exits_loop_and_plays_tail() {
        let params = SamplerParams {
            loop_start: 0.2,
            loop_end: 0.4,
            loop_mode: LoopMode::Forward,
            voice_mode: VoiceMode::OneShot,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(48_000, 100, params);
        sampler.trigger(1, 60, 127);
        let mut bus = StereoBus::with_capacity(200);
        sampler.render_range(&mut bus, 0, 80);
        assert!(sampler.voices[0].active);
        assert!(sampler.voices[0].loop_enabled);

        sampler.release_note(1);
        sampler.render_range(&mut bus, 80, 200);
        assert!(!sampler.voices[0].active);
    }

    #[test]
    fn layered_retriggers_fill_the_bounded_voice_pool() {
        let params = SamplerParams {
            polyphony: 3,
            retrigger_mode: RetriggerMode::Layer,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(48_000, 4096, params);
        sampler.trigger(1, 60, 100);
        sampler.trigger(2, 60, 100);
        sampler.trigger(3, 60, 100);
        assert_eq!(sampler.active_voice_count(), 3);

        sampler.trigger(4, 60, 100);
        assert_eq!(sampler.active_voice_count(), 3);
        assert!(!sampler
            .voices
            .iter()
            .any(|voice| voice.active && voice.event_id == 1));
        assert!(sampler
            .voices
            .iter()
            .any(|voice| voice.active && voice.event_id == 4));
    }

    #[test]
    fn restart_reuses_the_oldest_matching_pitch() {
        let params = SamplerParams {
            polyphony: 4,
            retrigger_mode: RetriggerMode::Restart,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(48_000, 4096, params);
        sampler.trigger(1, 60, 100);
        sampler.trigger(2, 64, 100);
        sampler.trigger(3, 60, 100);
        assert_eq!(sampler.active_voice_count(), 2);
        assert!(!sampler
            .voices
            .iter()
            .any(|voice| voice.active && voice.event_id == 1));
        assert!(sampler
            .voices
            .iter()
            .any(|voice| voice.active && voice.event_id == 3));
    }

    #[test]
    fn choke_releases_every_active_voice_quickly() {
        let params = SamplerParams {
            polyphony: 4,
            retrigger_mode: RetriggerMode::Layer,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(48_000, 4096, params);
        sampler.trigger(1, 60, 100);
        sampler.trigger(2, 64, 100);
        let mut bus = StereoBus::with_capacity(512);
        sampler.render_range(&mut bus, 0, 32);
        sampler.choke();
        assert!(sampler
            .voices
            .iter()
            .filter(|voice| voice.active)
            .all(|voice| voice.env.is_releasing()));
        sampler.render_range(&mut bus, 32, 512);
        assert_eq!(sampler.active_voice_count(), 0);
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

    #[test]
    fn midi_note_controls_playback_rate() {
        let sr = 48_000;
        let frames = 128;
        let render_note = |note| {
            let mut sampler = sampler_with_frames(sr, 4096, SamplerParams::default());
            let mut bus = StereoBus::with_capacity(frames);
            let mut events = EventList::empty();
            events.push(TimedEvent {
                offset: 0,
                event: Event::NoteOn {
                    id: 0,
                    note,
                    velocity: 127,
                },
            });
            sampler.process(&ctx(frames, sr), &mut bus, &events, None);
            sampler.voices[0].play_pos
        };

        let root_position = render_note(60);
        let octave_position = render_note(72);
        assert!((root_position - 128.0).abs() < 0.001);
        assert!((octave_position - 256.0).abs() < 0.001);
    }

    #[test]
    fn coarse_and_fine_tuning_change_playback_rate() {
        let sr = 48_000;
        let render_with = |tune_semitones, tune_cents| {
            let params = SamplerParams {
                tune_semitones,
                tune_cents,
                ..SamplerParams::default()
            };
            let mut sampler = sampler_with_frames(sr, 4096, params);
            let mut bus = StereoBus::with_capacity(100);
            let mut events = EventList::empty();
            events.push(TimedEvent {
                offset: 0,
                event: Event::NoteOn {
                    id: 0,
                    note: 60,
                    velocity: 127,
                },
            });
            sampler.process(&ctx(100, sr), &mut bus, &events, None);
            sampler.voices[0].play_pos
        };

        assert!((render_with(12.0, 0.0) - 200.0).abs() < 0.001);
        assert!((render_with(0.0, 100.0) - 105.946).abs() < 0.01);
    }

    #[test]
    fn reverse_playback_starts_at_region_end_and_runs_backwards() {
        let sr = 48_000;
        let params = SamplerParams {
            start: 0.25,
            end: 0.75,
            reverse: true,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(sr, 1000, params);
        let mut bus = StereoBus::with_capacity(100);
        let mut events = EventList::empty();
        events.push(TimedEvent {
            offset: 0,
            event: Event::NoteOn {
                id: 0,
                note: 60,
                velocity: 127,
            },
        });

        sampler.process(&ctx(100, sr), &mut bus, &events, None);

        assert!((sampler.voices[0].play_pos - 649.0).abs() < 0.001);
        assert!(sampler.voices[0].active);
    }

    #[test]
    fn non_looping_voice_stops_at_trimmed_end() {
        let sr = 48_000;
        let params = SamplerParams {
            end: 0.25,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(sr, 100, params);
        let mut bus = StereoBus::with_capacity(64);
        let mut events = EventList::empty();
        events.push(TimedEvent {
            offset: 0,
            event: Event::NoteOn {
                id: 0,
                note: 60,
                velocity: 127,
            },
        });

        sampler.process(&ctx(64, sr), &mut bus, &events, None);

        assert!(!sampler.voices[0].active);
        assert!(bus.l[..25].iter().any(|sample| *sample > 0.0));
        assert!(bus.l[25..].iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn loop_bounds_use_normalized_start_and_end() {
        let params = SamplerParams {
            loop_start: 0.25,
            loop_end: 0.75,
            ..SamplerParams::default()
        };
        let sampler = sampler_with_frames(48_000, 100, params);
        assert_eq!(sampler.loop_bounds(100), (25.0, 75.0));
    }

    #[test]
    fn loop_region_is_clamped_to_playback_region() {
        let params = SamplerParams {
            start: 0.2,
            end: 0.8,
            loop_start: 0.0,
            loop_end: 1.0,
            ..SamplerParams::default()
        };
        let sampler = sampler_with_frames(48_000, 100, params);
        let (play_start, play_end) = sampler.playback_bounds(100);
        let (loop_start, loop_end) = sampler.loop_bounds(100);
        assert!((play_start - 20.0).abs() < 0.001);
        assert!((play_end - 80.0).abs() < 0.001);
        assert!((loop_start - 20.0).abs() < 0.001);
        assert!((loop_end - 80.0).abs() < 0.001);
    }

    #[test]
    fn forward_loop_wraps_inside_selected_region() {
        let sr = 48_000;
        let frames = 160;
        let params = SamplerParams {
            loop_start: 0.25,
            loop_end: 0.5,
            loop_mode: LoopMode::Forward,
            sustain: 1.0,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(sr, 64, params);
        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        events.push(TimedEvent {
            offset: 0,
            event: Event::NoteOn {
                id: 0,
                note: 60,
                velocity: 127,
            },
        });

        sampler.process(&ctx(frames, sr), &mut bus, &events, None);

        assert!(sampler.voices[0].active);
        assert!((16.0..32.0).contains(&sampler.voices[0].play_pos));
        assert!(bus.l.iter().any(|sample| *sample > 0.01));
    }

    #[test]
    fn rate_reduction_holds_sample_values() {
        let sr = 48_000;
        let params = SamplerParams {
            rate_reduction: 1.0,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(sr, 64, params);

        let first = sampler.shape_first_voice([0.125, -0.125]);
        let second = sampler.shape_first_voice([0.875, -0.875]);

        assert_eq!(first, second);
        assert_eq!(sampler.voices[0].hold_remaining, 30);
    }

    #[test]
    fn maximum_bit_reduction_quantizes_to_four_bits() {
        let sr = 48_000;
        let params = SamplerParams {
            bit_reduction: 1.0,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(sr, 64, params);

        assert_eq!(sampler.shape_first_voice([0.19, -0.19]), [0.25, -0.25]);
    }

    #[test]
    fn low_cutoff_filters_an_impulse() {
        let sr = 48_000;
        let params = SamplerParams {
            filter_cutoff: 0.0,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(sr, 64, params);

        let filtered = sampler.shape_first_voice([1.0, 1.0]);
        assert!(filtered[0] > 0.0);
        assert!(filtered[0] < 0.01);
    }

    #[test]
    fn resonant_filter_remains_finite() {
        let sr = 48_000;
        let params = SamplerParams {
            filter_cutoff: 0.7,
            filter_resonance: 1.0,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(sr, 64, params);
        for index in 0..20_000 {
            let input = if index == 0 { [1.0, 1.0] } else { [0.0, 0.0] };
            let output = sampler.shape_first_voice(input);
            assert!(output[0].is_finite() && output[1].is_finite());
        }
    }

    #[test]
    fn drive_adds_soft_saturation() {
        let sr = 48_000;
        let params = SamplerParams {
            drive: 1.0,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(sr, 64, params);
        let driven = sampler.shape_first_voice([0.2, -0.2]);
        assert!(driven[0] > 0.9);
        assert!(driven[1] < -0.9);
        assert!(driven[0] <= 1.0 && driven[1] >= -1.0);
    }

    #[test]
    fn stopping_transport_releases_a_looped_voice() {
        let sr = 48_000;
        let params = SamplerParams {
            loop_mode: LoopMode::Forward,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(sr, 64, params);
        let mut bus = StereoBus::with_capacity(32);
        let mut events = EventList::empty();
        events.push(TimedEvent {
            offset: 0,
            event: Event::NoteOn {
                id: 0,
                note: 60,
                velocity: 127,
            },
        });
        sampler.process(&ctx(32, sr), &mut bus, &events, None);

        let mut stopped = ctx(32, sr);
        stopped.playing = false;
        sampler.process(&stopped, &mut bus, &EventList::empty(), None);

        assert!(sampler.voices[0].env.is_releasing());
    }
}
