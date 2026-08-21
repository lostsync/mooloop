//! A percussive synth: kicks, snares, and hats generated per trigger, with no
//! sample data involved. It slots into a channel exactly like the sampler —
//! same `AudioNode` trait, same segment-based, sample-accurate event
//! handling.
//!
//! One channel plays one [`DrumMode`] at a time, matching the
//! one-sound-per-channel groovebox model. Knobs for the other modes are
//! retained in [`DrumSynthParams`] so switching modes never loses settings.
//!
//! Voices are a small fixed pool: each trigger grabs a free voice or steals
//! the oldest, like the sampler's bounded voice allocation. Drums are
//! one-shot by nature; note-offs are ignored and `Choke` fast-fades every
//! ringing voice.

use crate::bus::StereoBus;
use crate::env::{ExpDecay, DECAY_TAIL_CONSTANTS};
use crate::event::{Event, EventList};
use crate::filter::{apply_drive, OnePoleHp};
use crate::node::{AudioNode, ProcessContext};
use crate::osc::{Noise, Osc};
use mooloop_core::{
    DrumMode, DrumSynthParams, HatCharacter, KickCharacter, OscWave, SnareCharacter,
    MAX_CHOKE_GROUP, MAX_DRUM_VOICES,
};

/// Fast fade used for chokes and transport stops (seconds). The coefficient
/// is scaled so the fade effectively completes within this window rather than
/// treating it as a single time constant.
const CHOKE_DECAY_S: f32 = 0.005;

/// Beater click duration for the kick (seconds).
const CLICK_S: f32 = 0.003;

/// The two detuned square frequencies (Hz, before keyboard tracking) that
/// give the hat its metallic edge.
const HAT_METAL_A_HZ: f32 = 587.33;
const HAT_METAL_B_HZ: f32 = 845.07;

fn lerp(a: f32, b: f32, x: f32) -> f32 {
    a + (b - a) * x.clamp(0.0, 1.0)
}

/// One independently enveloped drum hit.
struct DrumVoice {
    active: bool,
    age: u64,
    mode: DrumMode,
    amp_env: ExpDecay,
    noise_env: ExpDecay,
    sweep_env: ExpDecay,
    body_osc: Osc,
    metal_osc_a: Osc,
    metal_osc_b: Osc,
    hp: OnePoleHp,
    noise: Noise,
    /// Keyboard tracking multiplier resolved at trigger time.
    pitch_factor: f32,
    velocity_amp: f32,
    click_remaining: u32,
    click_total: u32,
}

impl DrumVoice {
    fn new(seed: u32) -> Self {
        Self {
            active: false,
            age: 0,
            mode: DrumMode::Kick,
            amp_env: ExpDecay::new(),
            noise_env: ExpDecay::new(),
            sweep_env: ExpDecay::new(),
            body_osc: Osc::new(),
            metal_osc_a: Osc::new(),
            metal_osc_b: Osc::new(),
            hp: OnePoleHp::new(),
            noise: Noise::new(seed),
            pitch_factor: 1.0,
            velocity_amp: 0.0,
            click_remaining: 0,
            click_total: 1,
        }
    }

    fn choke(&mut self, sample_rate: u32) {
        let coeff = (-DECAY_TAIL_CONSTANTS / (CHOKE_DECAY_S * sample_rate as f32)).exp();
        self.amp_env.set_coeff(coeff);
        self.noise_env.set_coeff(coeff);
        self.click_remaining = 0;
    }

    fn reset(&mut self, seed: u32) {
        self.active = false;
        self.age = 0;
        self.mode = DrumMode::Kick;
        self.amp_env = ExpDecay::new();
        self.noise_env = ExpDecay::new();
        self.sweep_env = ExpDecay::new();
        self.body_osc.reset();
        self.metal_osc_a.reset();
        self.metal_osc_b.reset();
        self.hp.reset();
        self.noise.reset(seed);
        self.pitch_factor = 1.0;
        self.velocity_amp = 0.0;
        self.click_remaining = 0;
        self.click_total = 1;
    }
}

/// The drum synth node.
pub struct DrumSynth {
    params: DrumSynthParams,
    sample_rate: u32,
    voices: [DrumVoice; MAX_DRUM_VOICES as usize],
    next_age: u64,
}

impl DrumSynth {
    pub fn new(mut params: DrumSynthParams, sample_rate: u32) -> Self {
        params.choke_group = params.choke_group.min(MAX_CHOKE_GROUP);
        let voices = std::array::from_fn(|index| DrumVoice::new(0x9E37_79B9 + index as u32));
        Self {
            params,
            sample_rate,
            voices,
            next_age: 1,
        }
    }

    /// Replace the parameter set. Called from the RT command drain.
    pub fn set_params(&mut self, mut params: DrumSynthParams) {
        params.choke_group = params.choke_group.min(MAX_CHOKE_GROUP);
        self.params = params;
    }

    pub fn choke_group(&self) -> u8 {
        self.params.choke_group
    }

    /// Render one deterministic hit through the production voice path and
    /// reduce it to min/max bins suitable for a waveform overview.
    pub fn preview_waveform(params: DrumSynthParams, bins: usize) -> (Vec<f32>, Vec<f32>) {
        if bins == 0 {
            return (Vec::new(), Vec::new());
        }

        const PREVIEW_SAMPLE_RATE: u32 = 48_000;
        const PREVIEW_SECONDS: f32 = 0.3;
        let frame_count = (PREVIEW_SAMPLE_RATE as f32 * PREVIEW_SECONDS) as usize;
        let mut synth = Self::new(params, PREVIEW_SAMPLE_RATE);
        synth.trigger(60, 127);
        let params = synth.params;
        let voice = &mut synth.voices[0];
        let mut minimums = vec![f32::INFINITY; bins];
        let mut maximums = vec![f32::NEG_INFINITY; bins];

        for frame in 0..frame_count {
            let sample = Self::render_sample(params, PREVIEW_SAMPLE_RATE, voice);
            let bin = (frame * bins / frame_count).min(bins - 1);
            minimums[bin] = minimums[bin].min(sample);
            maximums[bin] = maximums[bin].max(sample);
        }

        let peak = minimums
            .iter()
            .chain(&maximums)
            .fold(1.0_f32, |peak, sample| peak.max(sample.abs()));
        for sample in minimums.iter_mut().chain(&mut maximums) {
            if !sample.is_finite() {
                *sample = 0.0;
            } else {
                *sample /= peak;
            }
        }
        (minimums, maximums)
    }

    /// Immediately invalidate all active voices.
    pub fn reset(&mut self) {
        for (index, voice) in self.voices.iter_mut().enumerate() {
            voice.reset(0x9E37_79B9 + index as u32);
        }
        self.next_age = 1;
    }

    fn select_voice(&self) -> usize {
        if let Some(index) = self.voices.iter().position(|voice| !voice.active) {
            return index;
        }
        self.voices
            .iter()
            .enumerate()
            .min_by_key(|(_, voice)| voice.age)
            .map(|(index, _)| index)
            .unwrap_or(0)
    }

    fn trigger(&mut self, note: u8, velocity: u8) {
        let params = self.params;
        let sr = self.sample_rate;
        let index = self.select_voice();
        let age = self.next_age;
        self.next_age = self.next_age.wrapping_add(1).max(1);

        // Keyboard tracking relative to middle C, plus the tune knob.
        let semitones = f32::from(note.min(127)) - 60.0 + params.tune_semitones.clamp(-48.0, 48.0);
        let voice = &mut self.voices[index];
        voice.active = true;
        voice.age = age;
        voice.mode = params.mode;
        voice.pitch_factor = 2.0_f32.powf(semitones / 12.0);
        voice.velocity_amp = f32::from(velocity) / 127.0;
        voice.body_osc.reset();
        voice.metal_osc_a.reset();
        voice.metal_osc_b.reset();
        voice.amp_env.set_time(params.decay, sr);
        voice.amp_env.trigger();
        match params.mode {
            DrumMode::Kick => {
                voice.sweep_env.set_time(params.kick_sweep, sr);
                voice.sweep_env.trigger();
                voice.noise_env.set_coeff(0.0);
                voice.click_total = (CLICK_S * sr as f32).max(1.0) as u32;
                voice.click_remaining = voice.click_total;
            }
            DrumMode::Snare => {
                voice.noise_env.set_time(params.snare_noise_decay, sr);
                voice.noise_env.trigger();
                voice
                    .hp
                    .set_cutoff(lerp(900.0, 9_500.0, params.snare_noise_color), sr);
                voice.click_remaining = 0;
            }
            DrumMode::Hat => {
                voice.hp.set_cutoff(params.hat_hp_hz, sr);
                voice.noise_env.set_coeff(0.0);
                voice.click_remaining = 0;
            }
        }
    }

    pub fn choke(&mut self) {
        let sr = self.sample_rate;
        for voice in self.voices.iter_mut().filter(|voice| voice.active) {
            voice.choke(sr);
        }
    }

    /// Render one sample of one voice. Mono by design; the channel pan/gain
    /// stage places it in the stereo field.
    fn render_sample(params: DrumSynthParams, sample_rate: u32, voice: &mut DrumVoice) -> f32 {
        voice.amp_env.advance();
        voice.noise_env.advance();
        let amp = voice.velocity_amp;
        let out = match voice.mode {
            DrumMode::Kick => {
                voice.sweep_env.advance();
                let sweep = voice.sweep_env.level();
                let (pitch_scale, click_scale, body_gain, punch_scale) = match params.kick_character
                {
                    KickCharacter::Sub => (0.82, 0.35, 1.25, 0.55),
                    KickCharacter::Punch => (1.0, 1.25, 1.0, 1.4),
                    KickCharacter::Deep => (0.72, 0.55, 1.15, 0.9),
                    KickCharacter::Kit => (1.0, 1.0, 1.0, 1.0),
                    KickCharacter::Dnb => (1.18, 1.35, 0.95, 1.6),
                };
                let end_hz = (params.kick_end_hz * pitch_scale).max(1.0);
                let ratio = ((params.kick_start_hz * pitch_scale).max(1.0) / end_hz).max(1.0);
                let freq = end_hz * ratio.powf(sweep) * voice.pitch_factor;
                let body = voice
                    .body_osc
                    .next_sample(freq, OscWave::Sine, 0.5, sample_rate);
                let transient = 1.0 + params.punch.clamp(0.0, 1.0) * punch_scale * sweep.powf(0.45);
                let click = if voice.click_remaining > 0 {
                    voice.click_remaining -= 1;
                    let fade = voice.click_remaining as f32 / voice.click_total as f32;
                    voice.noise.next_sample() * params.kick_click * click_scale * fade
                } else {
                    0.0
                };
                (body * body_gain * transient + click) * voice.amp_env.level() * amp
            }
            DrumMode::Snare => {
                let (body_scale, tone2_scale, noise_scale, noise_gain, punch_scale) =
                    match params.snare_character {
                        SnareCharacter::Pop => (1.0, 1.0, 1.0, 1.0, 0.85),
                        SnareCharacter::Snap => (1.25, 1.65, 1.0, 1.12, 1.35),
                        SnareCharacter::Power => (0.88, 0.75, 0.82, 1.25, 1.5),
                        SnareCharacter::Clap => (0.6, 1.8, 1.25, 1.45, 1.05),
                        SnareCharacter::Rim => (1.75, 2.35, 0.42, 0.45, 1.55),
                    };
                let freq = params.snare_tone_hz * body_scale * voice.pitch_factor;
                let body1 = voice
                    .body_osc
                    .next_sample(freq, OscWave::Sine, 0.5, sample_rate)
                    * voice.amp_env.level();
                let body2 = voice.metal_osc_a.next_sample(
                    params.snare_tone2_hz * tone2_scale * voice.pitch_factor,
                    OscWave::Triangle,
                    0.5,
                    sample_rate,
                ) * voice.amp_env.level()
                    * params.snare_tone2_mix.clamp(0.0, 1.0);
                let transient = 1.0
                    + params.punch.clamp(0.0, 1.0) * punch_scale * voice.amp_env.level().powf(2.0);
                let body = (body1 + body2) * transient;
                let noise = voice.hp.next_sample(voice.noise.next_sample())
                    * voice.noise_env.level()
                    * noise_gain;
                let mix = (params.snare_noise_mix * noise_scale).clamp(0.0, 1.0);
                ((1.0 - mix) * body + mix * noise) * amp
            }
            DrumMode::Hat => {
                let (metal_scale, hp_input_scale, gain_scale) = match params.hat_character {
                    HatCharacter::Soft => (0.45, 0.72, 0.8),
                    HatCharacter::Tight => (1.0, 1.0, 1.0),
                    HatCharacter::Metal => (1.45, 0.95, 1.0),
                    HatCharacter::Sizzle => (0.85, 1.28, 1.15),
                    HatCharacter::Trash => (1.65, 0.82, 1.25),
                };
                let metal = (voice.metal_osc_a.next_sample(
                    HAT_METAL_A_HZ * voice.pitch_factor,
                    OscWave::Pulse,
                    0.5,
                    sample_rate,
                ) + voice.metal_osc_b.next_sample(
                    HAT_METAL_B_HZ * voice.pitch_factor,
                    OscWave::Pulse,
                    0.5,
                    sample_rate,
                )) * 0.5;
                let metallic = (params.hat_metallic * metal_scale).clamp(0.0, 1.0);
                let source = metallic * metal + (1.0 - metallic) * voice.noise.next_sample();
                voice.hp.next_sample(source * hp_input_scale)
                    * voice.amp_env.level()
                    * amp
                    * gain_scale
            }
        };
        apply_drive(out, params.drive)
    }

    fn render_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        let params = self.params;
        let sample_rate = self.sample_rate;
        for voice in &mut self.voices {
            if !voice.active {
                continue;
            }
            for i in start..end {
                let sample = Self::render_sample(params, sample_rate, voice);
                bus.l[i] += sample;
                bus.r[i] += sample;
                if voice.amp_env.is_idle() && voice.noise_env.is_idle() {
                    voice.active = false;
                    break;
                }
            }
        }
    }
}

impl AudioNode for DrumSynth {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        let frames = ctx.frames.min(bus.capacity());

        if !ctx.playing {
            self.choke();
        }

        // Split the block at event offsets: render, apply event, repeat.
        let mut pos = 0usize;
        for ev in events_in.iter() {
            let off = (ev.offset as usize).min(frames).max(pos);
            self.render_range(bus, pos, off);
            match ev.event {
                Event::NoteOn { note, velocity, .. } => self.trigger(note, velocity),
                // Drums are one-shot; note-offs end nothing.
                Event::NoteOff { .. } => {}
                Event::Choke => self.choke(),
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

    fn make_synth(sr: u32, params: DrumSynthParams) -> DrumSynth {
        DrumSynth::new(params, sr)
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

    fn note_on(offset: u32, note: u8) -> TimedEvent {
        TimedEvent {
            offset,
            event: Event::NoteOn {
                id: 0,
                note,
                velocity: 127,
            },
        }
    }

    #[test]
    fn idle_is_silent() {
        let sr = 48_000;
        let mut synth = make_synth(sr, DrumSynthParams::default());
        let mut bus = StereoBus::with_capacity(256);
        synth.process(&ctx(256, sr), &mut bus, &EventList::empty(), None);
        assert_eq!(bus.peak(256), (0.0, 0.0));
    }

    #[test]
    fn note_on_at_offset_is_sample_accurate() {
        let sr = 48_000;
        let frames = 512;
        let k = 200usize;
        let mut synth = make_synth(sr, DrumSynthParams::default());
        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        events.push(note_on(k as u32, 60));

        synth.process(&ctx(frames, sr), &mut bus, &events, None);

        assert!(bus.l[..k].iter().all(|s| *s == 0.0));
        assert!(bus.l[k..].iter().any(|s| s.abs() > 0.01));
    }

    #[test]
    fn every_mode_makes_sound_and_terminates() {
        let sr = 48_000;
        for mode in DrumMode::all() {
            let params = DrumSynthParams::preset(mode);
            let mut synth = make_synth(sr, params);
            let mut bus = StereoBus::with_capacity(sr as usize);
            let mut events = EventList::empty();
            events.push(note_on(0, 60));
            synth.process(&ctx(sr as usize, sr), &mut bus, &events, None);
            let (pl, pr) = bus.peak(sr as usize);
            assert!(pl > 0.05, "{mode:?} too quiet: {pl}");
            assert_eq!(pl, pr, "{mode:?} should render mono");
            // `ExpDecay::set_time`'s seconds argument is the time to become
            // inaudible; the longest default is the kick's 0.24 s amp decay.
            // Four seconds covers everything.
            let mut bus = StereoBus::with_capacity(sr as usize);
            for _ in 0..3 {
                bus.clear(sr as usize);
                synth.process(&ctx(sr as usize, sr), &mut bus, &EventList::empty(), None);
            }
            assert!(
                synth.voices.iter().all(|voice| !voice.active),
                "{mode:?} still ringing"
            );
        }
    }

    #[test]
    fn kick_decays_after_the_hit() {
        let sr = 48_000;
        let mut synth = make_synth(sr, DrumSynthParams::default());
        let mut bus = StereoBus::with_capacity(sr as usize);
        let mut events = EventList::empty();
        events.push(note_on(0, 60));
        synth.process(&ctx(sr as usize, sr), &mut bus, &events, None);
        let first_peak = bus.l[..2400]
            .iter()
            .fold(0.0_f32, |peak, s| peak.max(s.abs()));
        let late_peak = bus.l[24000..]
            .iter()
            .fold(0.0_f32, |peak, s| peak.max(s.abs()));
        assert!(first_peak > 0.5);
        assert!(late_peak < first_peak * 0.2);
    }

    #[test]
    fn punch_raises_the_kick_transient() {
        let sr = 48_000;
        let peak = |punch: f32| {
            let params = DrumSynthParams {
                punch,
                kick_click: 0.0,
                ..DrumSynthParams::default()
            };
            let mut synth = make_synth(sr, params);
            let mut bus = StereoBus::with_capacity(1024);
            let mut events = EventList::empty();
            events.push(note_on(0, 60));
            synth.process(&ctx(1024, sr), &mut bus, &events, None);
            bus.l[..512]
                .iter()
                .fold(0.0_f32, |peak, s| peak.max(s.abs()))
        };

        assert!(peak(1.0) > peak(0.0) * 1.25);
    }

    #[test]
    fn snare_character_and_second_tone_change_the_body() {
        let sr = 48_000;
        let render_peak = |character: SnareCharacter, tone2_mix: f32| {
            let params = DrumSynthParams {
                mode: DrumMode::Snare,
                snare_character: character,
                snare_noise_mix: 0.0,
                snare_tone2_mix: tone2_mix,
                ..DrumSynthParams::default()
            };
            let mut synth = make_synth(sr, params);
            let mut bus = StereoBus::with_capacity(2048);
            let mut events = EventList::empty();
            events.push(note_on(0, 60));
            synth.process(&ctx(2048, sr), &mut bus, &events, None);
            bus.l[..1024]
                .iter()
                .fold(0.0_f32, |peak, s| peak.max(s.abs()))
        };

        let pop = render_peak(SnareCharacter::Pop, 0.0);
        let rim = render_peak(SnareCharacter::Rim, 0.65);
        assert!((rim - pop).abs() > 0.05, "pop {pop}, rim {rim}");
    }

    #[test]
    fn note_off_does_not_stop_a_hit() {
        let sr = 48_000;
        let mut synth = make_synth(sr, DrumSynthParams::default());
        let mut bus = StereoBus::with_capacity(512);
        let mut events = EventList::empty();
        events.push(note_on(0, 60));
        events.push(TimedEvent {
            offset: 64,
            event: Event::NoteOff { id: 0, note: 60 },
        });
        synth.process(&ctx(512, sr), &mut bus, &events, None);
        assert!(synth.voices[0].active);
        assert!(bus.l[128..].iter().any(|s| s.abs() > 0.01));
    }

    #[test]
    fn choke_silences_quickly() {
        let sr = 48_000;
        let mut synth = make_synth(sr, DrumSynthParams::default());
        let mut bus = StereoBus::with_capacity(4096);
        let mut events = EventList::empty();
        events.push(note_on(0, 60));
        events.push(TimedEvent {
            offset: 100,
            event: Event::Choke,
        });
        synth.process(&ctx(4096, sr), &mut bus, &events, None);
        // The 5 ms fade completes shortly after the choke at offset 100.
        assert!(bus.l[1000..].iter().all(|s| s.abs() < 0.001));
        assert!(synth.voices.iter().all(|voice| !voice.active));
    }

    #[test]
    fn stopping_transport_chokes_ringing_voices() {
        let sr = 48_000;
        let mut synth = make_synth(sr, DrumSynthParams::default());
        let mut bus = StereoBus::with_capacity(64);
        let mut events = EventList::empty();
        events.push(note_on(0, 60));
        synth.process(&ctx(64, sr), &mut bus, &events, None);

        let mut stopped = ctx(64, sr);
        stopped.playing = false;
        synth.process(&stopped, &mut bus, &EventList::empty(), None);

        let mut bus = StereoBus::with_capacity(4096);
        synth.process(&ctx(4096, sr), &mut bus, &EventList::empty(), None);
        assert!(synth.voices.iter().all(|voice| !voice.active));
    }

    #[test]
    fn voice_pool_overflow_steals_the_oldest() {
        let sr = 48_000;
        let mut synth = make_synth(sr, DrumSynthParams::default());
        for _ in 0..MAX_DRUM_VOICES + 4 {
            synth.trigger(60, 100);
        }
        assert!(synth.voices.iter().all(|voice| voice.active));
        // Ages are unique and dense: the oldest voices were stolen.
        let mut ages: Vec<u64> = synth.voices.iter().map(|voice| voice.age).collect();
        ages.sort_unstable();
        assert_eq!(ages, (5..=MAX_DRUM_VOICES as u64 + 4).collect::<Vec<_>>());
    }

    #[test]
    fn midi_note_shifts_kick_pitch() {
        let sr = 48_000;
        let period_of = |note: u8| {
            let params = DrumSynthParams {
                kick_sweep: 10.0, // effectively no sweep; isolates tracking
                decay: 0.5,
                ..DrumSynthParams::default()
            };
            let mut synth = make_synth(sr, params);
            let mut bus = StereoBus::with_capacity(4096);
            let mut events = EventList::empty();
            events.push(note_on(0, note));
            synth.process(&ctx(4096, sr), &mut bus, &events, None);
            // Count upward zero crossings in a steady region.
            let mut crossings = 0u32;
            for window in bus.l[480..2400].windows(2) {
                if window[0] <= 0.0 && window[1] > 0.0 {
                    crossings += 1;
                }
            }
            crossings
        };
        let low = period_of(48);
        let high = period_of(60);
        // One octave up doubles the frequency: kick_end 48 Hz * 2 vs 4x from
        // middle C... exact ratio is what matters, not the absolute value.
        assert_eq!(high, low * 2);
    }

    #[test]
    fn drive_stays_bounded() {
        let sr = 48_000;
        let params = DrumSynthParams {
            drive: 1.0,
            kick_click: 1.0,
            ..DrumSynthParams::default()
        };
        let mut synth = make_synth(sr, params);
        let mut bus = StereoBus::with_capacity(2048);
        let mut events = EventList::empty();
        events.push(note_on(0, 60));
        synth.process(&ctx(2048, sr), &mut bus, &events, None);
        let (pl, _) = bus.peak(2048);
        assert!(pl <= 1.0);
    }

    #[test]
    fn preview_uses_the_production_voice_and_tracks_parameters() {
        let plain = DrumSynth::preview_waveform(DrumSynthParams::default(), 96);
        let altered = DrumSynth::preview_waveform(
            DrumSynthParams {
                kick_start_hz: 700.0,
                kick_click: 1.0,
                drive: 0.8,
                ..DrumSynthParams::default()
            },
            96,
        );
        assert_eq!(plain.0.len(), 96);
        assert_eq!(plain.1.len(), 96);
        assert_ne!(plain, altered);
        assert!(plain
            .0
            .iter()
            .chain(&plain.1)
            .all(|sample| (-1.0..=1.0).contains(sample)));
    }
}
