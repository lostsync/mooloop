//! A three-oscillator polyphonic synth. Built on the same primitives as the
//! mono synth — band-limited oscillators, ADSR, state-variable low-pass, LFO,
//! and drive — but with an independent voice pool, per-voice envelope and
//! filter, and a stereo spread.

use crate::bus::{pan_gains, StereoBus};
use crate::env::Adsr;
use crate::event::{Event, EventList};
use crate::filter::{apply_drive, Svf};
use crate::lfo::Lfo;
use crate::node::{AudioNode, ProcessContext};
use crate::osc::Osc;
use crate::scale::hz_from_normalized;
use crate::smooth::Smoothed;
use mooloop_core::{PolySynthParams, MAX_POLY_VOICES};

/// Minimum glide time; at or below this, pitch changes are instant.
const MIN_GLIDE_S: f32 = 1.0e-3;

/// Fast release used when the transport stops (seconds).
const STOP_RELEASE_S: f32 = 0.005;

/// Lag applied to parameters that scale the signal directly.
const PARAM_SMOOTH_S: f32 = 0.005;

/// MIDI note number to frequency in Hz (A4 = 69 = 440 Hz).
fn note_to_freq(note: u8) -> f32 {
    440.0 * 2.0_f32.powf((f32::from(note.min(127)) - 69.0) / 12.0)
}

/// Stereo position for a voice from the active voice index and the current
/// polyphony count. Returns a pan in `[-1, 1]`; centre when spread is zero or
/// there is only one active voice slot.
fn voice_pan(voice_index: usize, polyphony: u8, spread: f32) -> f32 {
    if polyphony <= 1 {
        return 0.0;
    }
    let count = polyphony.clamp(1, MAX_POLY_VOICES) as f32;
    let index = (voice_index as f32).min(count - 1.0);
    let normalized = 2.0 * index / (count - 1.0) - 1.0;
    normalized * spread.clamp(0.0, 1.0)
}

struct PolyVoice {
    active: bool,
    event_id: u64,
    note: u8,
    age: u64,
    env: Adsr,
    oscs: [Osc; 3],
    current_freq: f32,
    target_freq: f32,
    filter: Svf,
    /// Velocity gain, smoothed so that a stolen retrigger at a different
    /// velocity slides rather than steps.
    velocity_amp: Smoothed,
    osc_level: [Smoothed; 3],
    cutoff: Smoothed,
    drive: Smoothed,
}

impl PolyVoice {
    fn new(sample_rate: u32) -> Self {
        let smoothed = |initial| Smoothed::new(initial, PARAM_SMOOTH_S, sample_rate);
        Self {
            active: false,
            event_id: 0,
            note: 0,
            age: 0,
            env: Adsr::new(sample_rate),
            oscs: [Osc::new(), Osc::new(), Osc::new()],
            current_freq: 0.0,
            target_freq: 0.0,
            filter: Svf::new(),
            velocity_amp: smoothed(0.0),
            osc_level: [smoothed(0.0), smoothed(0.0), smoothed(0.0)],
            cutoff: smoothed(1.0),
            drive: smoothed(0.0),
        }
    }

    /// Adopt the current parameters without a ramp. Only safe when the voice
    /// is starting from silence.
    fn snap_to(&mut self, params: &PolySynthParams, velocity_amp: f32) {
        self.velocity_amp.reset_to(velocity_amp);
        for (smoothed, osc) in self.osc_level.iter_mut().zip(params.osc.iter()) {
            smoothed.reset_to(osc.level.clamp(0.0, 1.0));
        }
        self.cutoff.reset_to(params.filter_cutoff.clamp(0.0, 1.0));
        self.drive.reset_to(params.drive.clamp(0.0, 1.0));
    }
}

/// The poly synth node.
pub struct PolySynth {
    params: PolySynthParams,
    sample_rate: u32,
    voices: [PolyVoice; MAX_POLY_VOICES as usize],
    next_age: u64,
    /// Free running unless the LFO is set to retrigger, so it keeps its phase
    /// across the gaps between notes.
    lfo: Lfo,
}

impl PolySynth {
    pub fn new(params: PolySynthParams, sample_rate: u32) -> Self {
        let polyphony = params.polyphony.clamp(1, MAX_POLY_VOICES);
        let mut voices = std::array::from_fn(|_| PolyVoice::new(sample_rate));
        for voice in &mut voices {
            voice
                .env
                .configure(params.attack, params.decay, params.sustain, params.release);
        }
        let mut synth = Self {
            params,
            sample_rate,
            voices,
            next_age: 1,
            lfo: Lfo::new(),
        };
        synth.apply_params_to_voices(polyphony);
        synth
    }

    /// Replace the parameter set. Called from the RT command drain.
    pub fn set_params(&mut self, params: PolySynthParams) {
        let polyphony = params.polyphony.clamp(1, MAX_POLY_VOICES);
        self.params = params;
        self.apply_params_to_voices(polyphony);
    }

    fn apply_params_to_voices(&mut self, polyphony: u8) {
        for (index, voice) in self.voices.iter_mut().enumerate() {
            voice.env.configure(
                self.params.attack,
                self.params.decay,
                self.params.sustain,
                self.params.release,
            );
            if index >= polyphony as usize {
                voice.active = false;
            }
        }
    }

    /// Immediately invalidate every voice and return every oscillator and
    /// filter to its initial state.
    pub fn reset(&mut self) {
        let polyphony = self.params.polyphony.clamp(1, MAX_POLY_VOICES);
        for voice in &mut self.voices {
            *voice = PolyVoice::new(self.sample_rate);
            voice.env.configure(
                self.params.attack,
                self.params.decay,
                self.params.sustain,
                self.params.release,
            );
        }
        for (index, voice) in self.voices.iter_mut().enumerate() {
            if index >= polyphony as usize {
                voice.active = false;
            }
        }
        self.next_age = 1;
        self.lfo = Lfo::new();
    }

    pub fn choke(&mut self) {
        self.release_all();
    }

    fn voice_limit(&self) -> usize {
        self.params.polyphony.clamp(1, MAX_POLY_VOICES) as usize
    }

    fn any_active(&self) -> bool {
        self.voices[..self.voice_limit()].iter().any(|v| v.active)
    }

    fn select_voice(&self) -> usize {
        let voices = &self.voices[..self.voice_limit()];
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

    fn note_on(&mut self, event_id: u64, note: u8, velocity: u8) {
        let was_any_active = self.any_active();
        let index = self.select_voice();
        let velocity_amp = f32::from(velocity) / 127.0;
        let age = self.next_age;
        self.next_age = self.next_age.wrapping_add(1).max(1);

        let voice = &mut self.voices[index];
        let stolen = voice.active;
        voice.event_id = event_id;
        voice.note = note;
        voice.age = age;
        voice.target_freq = note_to_freq(note);
        voice.active = true;

        if !stolen {
            // Fresh slot: no glide from silence, clean filter and phases.
            voice.current_freq = voice.target_freq;
            voice.filter.reset();
            for osc in &mut voice.oscs {
                osc.reset();
            }
            voice.snap_to(&self.params, velocity_amp);
        } else if self.params.glide <= MIN_GLIDE_S {
            voice.current_freq = voice.target_freq;
        }
        voice.velocity_amp.set_target(velocity_amp);
        voice.env.note_on();

        if self.params.lfo.retrigger && !was_any_active {
            self.lfo.retrigger();
        }
    }

    fn note_off(&mut self, event_id: u64) {
        for voice in self
            .voices
            .iter_mut()
            .filter(|voice| voice.active && voice.event_id == event_id)
        {
            voice.env.release();
        }
    }

    fn release_all(&mut self) {
        for voice in &mut self.voices {
            if voice.active && !voice.env.is_releasing() {
                voice.env.release_with(STOP_RELEASE_S);
            }
        }
    }

    fn render_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        let params = self.params;
        let sr = self.sample_rate;
        let lfo_params = params.lfo;
        let max_hz = sr as f32 * 0.45;
        let polyphony = self.voice_limit() as u8;
        let spread = params.spread.clamp(0.0, 1.0);
        let voices = &mut self.voices;
        let lfo = &mut self.lfo;

        // Per-oscillator pitch ratios from semitone/cent offsets.
        let mut ratio = [0.0_f32; 3];
        for (index, osc) in params.osc.iter().enumerate() {
            let semis = osc.semitones.clamp(-48.0, 48.0) + osc.cents.clamp(-100.0, 100.0) / 100.0;
            ratio[index] = 2.0_f32.powf(semis / 12.0);
        }

        let env_amount = params.filter_env_amount.clamp(-1.0, 1.0);
        let resonance = params.filter_resonance.clamp(0.0, 1.0);
        let to_pitch = lfo_params.to_pitch.clamp(-24.0, 24.0);
        let to_filter = lfo_params.to_filter.clamp(-4.0, 4.0);
        let to_pulse_width = lfo_params.to_pulse_width.clamp(-0.45, 0.45);
        let to_amp = lfo_params.to_amp.clamp(0.0, 1.0);
        let glide_coeff = (-1.0 / (params.glide.max(MIN_GLIDE_S) * sr as f32)).exp();

        // Signal-scaling parameters lag their targets; everything else is
        // cheap enough to read straight from the block's parameters.
        for voice in voices.iter_mut() {
            for (smoothed, osc) in voice.osc_level.iter_mut().zip(params.osc.iter()) {
                smoothed.set_target(osc.level.clamp(0.0, 1.0));
            }
            voice
                .cutoff
                .set_target(params.filter_cutoff.clamp(0.0, 1.0));
            voice.drive.set_target(params.drive.clamp(0.0, 1.0));
        }

        for i in start..end {
            let lfo_value = lfo.next_sample(lfo_params.rate_hz, lfo_params.wave, sr);
            let pitch_mod = if to_pitch == 0.0 {
                1.0
            } else {
                (lfo_value * to_pitch / 12.0).exp2()
            };
            let tremolo = 1.0 - to_amp * (1.0 - lfo_value) * 0.5;

            for (voice_index, voice) in voices.iter_mut().enumerate() {
                if !voice.active {
                    continue;
                }

                voice.env.advance();
                if voice.env.is_idle() {
                    voice.active = false;
                    continue;
                }

                voice.current_freq +=
                    (voice.target_freq - voice.current_freq) * (1.0 - glide_coeff);
                let velocity = voice.velocity_amp.advance();

                let mut mix = 0.0;
                for (osc_index, osc) in voice.oscs.iter_mut().enumerate() {
                    let osc_params = params.osc[osc_index];
                    let osc_level = voice.osc_level[osc_index].advance();
                    if osc_level <= 1.0e-5 && osc_params.level <= 1.0e-5 {
                        continue;
                    }
                    mix += osc_level
                        * osc.next_sample(
                            voice.current_freq * ratio[osc_index] * pitch_mod,
                            osc_params.wave,
                            osc_params.pulse_width + lfo_value * to_pulse_width,
                            sr,
                        );
                }

                let cutoff = voice.cutoff.advance();
                let drive = voice.drive.advance();
                let filtered = if cutoff >= 0.999
                    && env_amount.abs() <= f32::EPSILON
                    && resonance <= f32::EPSILON
                    && to_filter == 0.0
                {
                    mix
                } else {
                    let base_hz = hz_from_normalized(cutoff, max_hz);
                    let octaves = voice.env.level() * env_amount * 6.0 + lfo_value * to_filter;
                    let cutoff_hz = (base_hz * octaves.exp2()).clamp(20.0, max_hz);
                    voice
                        .filter
                        .next_sample_lp_hp(mix, cutoff_hz, resonance, sr)
                        .0
                };

                let sample = apply_drive(filtered, drive) * voice.env.level() * velocity * tremolo;
                let pan = voice_pan(voice_index, polyphony, spread);
                let (gain_l, gain_r) = pan_gains(pan);
                bus.l[i] += sample * gain_l;
                bus.r[i] += sample * gain_r;
            }
        }
    }
}

impl AudioNode for PolySynth {
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

        let mut pos = 0usize;
        for ev in events_in.iter() {
            let off = (ev.offset as usize).min(frames).max(pos);
            self.render_range(bus, pos, off);
            match ev.event {
                Event::NoteOn { id, note, velocity } => self.note_on(id, note, velocity),
                Event::NoteOff { id, .. } => self.note_off(id),
                Event::Choke => self.release_all(),
                Event::ParamValue { .. } | Event::Buffer(_) => {}
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

    fn make_synth(sr: u32, params: PolySynthParams) -> PolySynth {
        PolySynth::new(params, sr)
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

    fn note_on(offset: u32, id: u64, note: u8) -> TimedEvent {
        TimedEvent {
            offset,
            event: Event::NoteOn {
                id,
                note,
                velocity: 127,
            },
        }
    }

    #[test]
    fn idle_is_silent() {
        let sr = 48_000;
        let mut synth = make_synth(sr, PolySynthParams::default());
        let mut bus = StereoBus::with_capacity(256);
        synth.process(&ctx(256, sr), &mut bus, &EventList::empty(), None);
        assert_eq!(bus.peak(256), (0.0, 0.0));
    }

    #[test]
    fn note_on_at_offset_is_sample_accurate() {
        let sr = 48_000;
        let frames = 512;
        let k = 200usize;
        let mut synth = make_synth(sr, PolySynthParams::default());
        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        events.push(note_on(k as u32, 0, 60));

        synth.process(&ctx(frames, sr), &mut bus, &events, None);

        assert!(bus.l[..k].iter().all(|s| *s == 0.0));
        assert!(bus.l[k..].iter().any(|s| s.abs() > 0.001));
    }

    #[test]
    fn gated_note_releases_on_matching_note_off() {
        let sr = 48_000;
        let mut synth = make_synth(sr, PolySynthParams::default());
        let mut bus = StereoBus::with_capacity(4096);
        let mut events = EventList::empty();
        events.push(note_on(0, 7, 60));
        events.push(TimedEvent {
            offset: 100,
            event: Event::NoteOff { id: 7, note: 60 },
        });
        synth.process(&ctx(4096, sr), &mut bus, &events, None);
        assert!(synth.voices.iter().any(|v| v.event_id == 7 && v.active));
        let mut bus = StereoBus::with_capacity(16_000);
        synth.process(&ctx(16_000, sr), &mut bus, &EventList::empty(), None);
        assert!(synth.voices.iter().all(|v| !v.active));
        assert!(bus.l[8_000..].iter().all(|s| *s == 0.0));
    }

    #[test]
    fn polyphony_allows_simultaneous_voices() {
        let sr = 48_000;
        let params = PolySynthParams {
            attack: 0.0001,
            sustain: 1.0,
            polyphony: 4,
            ..Default::default()
        };
        let mut synth = make_synth(sr, params);
        let mut bus = StereoBus::with_capacity(4096);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        events.push(note_on(0, 2, 64));
        events.push(note_on(0, 3, 67));
        synth.process(&ctx(4096, sr), &mut bus, &events, None);
        assert_eq!(synth.voices.iter().filter(|v| v.active).count(), 3);
    }

    #[test]
    fn exceeding_polyphony_steals_oldest_voice() {
        let sr = 48_000;
        let params = PolySynthParams {
            attack: 0.0001,
            sustain: 1.0,
            polyphony: 2,
            ..Default::default()
        };
        let mut synth = make_synth(sr, params);
        let mut bus = StereoBus::with_capacity(4096);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        events.push(note_on(0, 2, 64));
        events.push(note_on(0, 3, 67));
        synth.process(&ctx(4096, sr), &mut bus, &events, None);
        assert_eq!(synth.voices.iter().filter(|v| v.active).count(), 2);
        assert!(synth.voices.iter().any(|v| v.event_id == 2));
        assert!(synth.voices.iter().any(|v| v.event_id == 3));
    }

    #[test]
    fn spread_pans_voices_outward() {
        let sr = 48_000;
        let params = PolySynthParams {
            attack: 0.0001,
            sustain: 1.0,
            polyphony: 3,
            spread: 1.0,
            ..Default::default()
        };
        let mut synth = make_synth(sr, params);
        let mut bus = StereoBus::with_capacity(4096);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        events.push(note_on(0, 2, 64));
        events.push(note_on(0, 3, 67));
        synth.process(&ctx(4096, sr), &mut bus, &events, None);
        let (pl, pr) = bus.peak(4096);
        // With three voices spread hard L/centre/R, the two sides cannot be
        // identical.
        assert!((pl - pr).abs() > 1.0e-4);
    }

    #[test]
    fn resonant_filter_and_drive_stay_bounded() {
        let sr = 48_000;
        let params = PolySynthParams {
            filter_cutoff: 0.6,
            filter_resonance: 1.0,
            filter_env_amount: 1.0,
            drive: 1.0,
            sustain: 1.0,
            ..PolySynthParams::default()
        };
        let mut synth = make_synth(sr, params);
        let mut bus = StereoBus::with_capacity(8192);
        let mut events = EventList::empty();
        events.push(note_on(0, 0, 38));
        synth.process(&ctx(8192, sr), &mut bus, &events, None);
        let (pl, pr) = bus.peak(8192);
        assert!(pl.is_finite() && pr.is_finite());
        assert!(pl <= 1.0 && pr <= 1.0);
    }

    #[test]
    fn parameter_changes_mid_note_do_not_step() {
        let sr = 48_000;
        let params = PolySynthParams {
            osc: [
                mooloop_core::OscParams {
                    wave: mooloop_core::OscWave::Sine,
                    level: 0.8,
                    ..Default::default()
                },
                mooloop_core::OscParams::default(),
                mooloop_core::OscParams::default(),
            ],
            attack: 0.01,
            decay: 0.05,
            sustain: 1.0,
            ..PolySynthParams::default()
        };
        let mut synth = make_synth(sr, params);
        let mut bus = StereoBus::with_capacity(4096);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 36));
        synth.process(&ctx(4096, sr), &mut bus, &events, None);

        let mut silenced = params;
        silenced.osc[0].level = 0.0;
        silenced.drive = 1.0;
        synth.set_params(silenced);
        let mut bus = StereoBus::with_capacity(4096);
        synth.process(&ctx(4096, sr), &mut bus, &EventList::empty(), None);

        let max_step = bus.l[..4096]
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        assert!(max_step < 0.05, "{max_step}");
        let end_peak = bus.l[3500..4096]
            .iter()
            .map(|s| s.abs())
            .fold(0.0f32, f32::max);
        assert!(end_peak < 1.0e-3, "end_peak = {end_peak}");
    }

    #[test]
    fn stopping_transport_releases_voices() {
        let sr = 48_000;
        let mut synth = make_synth(sr, PolySynthParams::default());
        let mut bus = StereoBus::with_capacity(64);
        let mut events = EventList::empty();
        events.push(note_on(0, 0, 60));
        synth.process(&ctx(64, sr), &mut bus, &events, None);
        assert!(synth.voices.iter().any(|v| v.active));

        let mut stopped = ctx(64, sr);
        stopped.playing = false;
        synth.process(&stopped, &mut bus, &EventList::empty(), None);
        assert!(synth
            .voices
            .iter()
            .all(|v| !v.active || v.env.is_releasing()));
    }
}
