//! A three-oscillator mono synth. It slots into a channel exactly like the
//! sampler — same `AudioNode` trait, same segment-based, sample-accurate
//! event handling.
//!
//! Deliberately compact: three band-limited oscillators with per-osc pitch
//! and level into one envelope-modulated low-pass filter, one ADSR, glide,
//! and drive. Mono because note duration and note-off semantics come first
//! in this project (see `PRODUCT.md`), and a single gated voice exercises
//! exactly that contract.

use crate::bus::StereoBus;
use crate::env::Adsr;
use crate::event::{Event, EventList};
use crate::filter::{apply_drive, Svf};
use crate::node::{AudioNode, ProcessContext};
use crate::osc::Osc;
use mooloop_core::MonoSynthParams;

/// Minimum glide time; at or below this, pitch changes are instant.
const MIN_GLIDE_S: f32 = 1.0e-3;

/// Fast release used when the transport stops (seconds).
const STOP_RELEASE_S: f32 = 0.005;

/// MIDI note number to frequency in Hz (A4 = 69 = 440 Hz).
fn note_to_freq(note: u8) -> f32 {
    440.0 * 2.0_f32.powf((f32::from(note.min(127)) - 69.0) / 12.0)
}

struct MonoVoice {
    active: bool,
    event_id: u64,
    env: Adsr,
    oscs: [Osc; 3],
    current_freq: f32,
    target_freq: f32,
    filter: Svf,
    velocity_amp: f32,
}

impl MonoVoice {
    fn new(sample_rate: u32) -> Self {
        Self {
            active: false,
            event_id: 0,
            env: Adsr::new(sample_rate),
            oscs: [Osc::new(), Osc::new(), Osc::new()],
            current_freq: 0.0,
            target_freq: 0.0,
            filter: Svf::new(),
            velocity_amp: 0.0,
        }
    }
}

/// The mono synth node.
pub struct MonoSynth {
    params: MonoSynthParams,
    sample_rate: u32,
    voice: MonoVoice,
}

impl MonoSynth {
    pub fn new(params: MonoSynthParams, sample_rate: u32) -> Self {
        let mut voice = MonoVoice::new(sample_rate);
        voice
            .env
            .configure(params.attack, params.decay, params.sustain, params.release);
        Self {
            params,
            sample_rate,
            voice,
        }
    }

    /// Replace the parameter set. Called from the RT command drain.
    pub fn set_params(&mut self, params: MonoSynthParams) {
        self.params = params;
        self.voice
            .env
            .configure(params.attack, params.decay, params.sustain, params.release);
    }

    /// Immediately invalidate the active voice and return every oscillator
    /// and filter to its initial state.
    pub fn reset(&mut self) {
        self.voice = MonoVoice::new(self.sample_rate);
        self.voice.env.configure(
            self.params.attack,
            self.params.decay,
            self.params.sustain,
            self.params.release,
        );
    }

    pub fn choke(&mut self) {
        self.release_all();
    }

    fn note_on(&mut self, event_id: u64, note: u8, velocity: u8) {
        let was_active = self.voice.active;
        self.voice.event_id = event_id;
        self.voice.target_freq = note_to_freq(note);
        if !was_active {
            // Fresh start: no glide from silence, clean filter and phases.
            self.voice.current_freq = self.voice.target_freq;
            self.voice.filter.reset();
            for osc in &mut self.voice.oscs {
                osc.reset();
            }
        } else if self.params.glide <= MIN_GLIDE_S {
            self.voice.current_freq = self.voice.target_freq;
        }
        self.voice.velocity_amp = f32::from(velocity) / 127.0;
        self.voice.env.note_on();
        self.voice.active = true;
    }

    fn note_off(&mut self, event_id: u64) {
        if self.voice.active && self.voice.event_id == event_id {
            self.voice.env.release();
        }
    }

    fn release_all(&mut self) {
        if self.voice.active && !self.voice.env.is_releasing() {
            self.voice.env.release_with(STOP_RELEASE_S);
        }
    }

    fn render_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        if !self.voice.active {
            return;
        }
        let params = self.params;
        let sr = self.sample_rate;
        let voice = &mut self.voice;

        // Glide: one-pole approach to the target frequency.
        let glide_coeff = (-1.0 / (params.glide.max(MIN_GLIDE_S) * sr as f32)).exp();

        // Per-oscillator pitch ratios from semitone/cent offsets.
        let mut ratio = [0.0_f32; 3];
        for (index, osc) in params.osc.iter().enumerate() {
            let semis = osc.semitones.clamp(-48.0, 48.0) + osc.cents.clamp(-100.0, 100.0) / 100.0;
            ratio[index] = 2.0_f32.powf(semis / 12.0);
        }

        for i in start..end {
            voice.current_freq += (voice.target_freq - voice.current_freq) * (1.0 - glide_coeff);

            voice.env.advance();
            if voice.env.is_idle() {
                voice.active = false;
                return;
            }

            let mut mix = 0.0;
            for (index, osc) in voice.oscs.iter_mut().enumerate() {
                let osc_params = params.osc[index];
                if osc_params.level <= f32::EPSILON {
                    continue;
                }
                mix += osc_params.level.clamp(0.0, 1.0)
                    * osc.next_sample(
                        voice.current_freq * ratio[index],
                        osc_params.wave,
                        osc_params.pulse_width,
                        sr,
                    );
            }

            // Envelope-modulated low-pass, same perceptual mapping the
            // sampler uses. Bypassed entirely when fully open.
            let cutoff = params.filter_cutoff.clamp(0.0, 1.0);
            let env_amount = params.filter_env_amount.clamp(-1.0, 1.0);
            let resonance = params.filter_resonance.clamp(0.0, 1.0);
            let filtered =
                if cutoff >= 0.999 && env_amount.abs() <= f32::EPSILON && resonance <= f32::EPSILON
                {
                    mix
                } else {
                    let max_hz = sr as f32 * 0.45;
                    let base_hz = 20.0 * (max_hz / 20.0).powf(cutoff);
                    let cutoff_hz = (base_hz * 2.0_f32.powf(voice.env.level() * env_amount * 6.0))
                        .clamp(20.0, max_hz);
                    voice.filter.next_sample(mix, cutoff_hz, resonance, sr)
                };

            let sample =
                apply_drive(filtered, params.drive) * voice.env.level() * voice.velocity_amp;
            bus.l[i] += sample;
            bus.r[i] += sample;
        }
    }
}

impl AudioNode for MonoSynth {
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
                Event::NoteOn { id, note, velocity } => self.note_on(id, note, velocity),
                Event::NoteOff { id, .. } => self.note_off(id),
                Event::Choke => self.release_all(),
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

    fn make_synth(sr: u32, params: MonoSynthParams) -> MonoSynth {
        MonoSynth::new(params, sr)
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
        let mut synth = make_synth(sr, MonoSynthParams::default());
        let mut bus = StereoBus::with_capacity(256);
        synth.process(&ctx(256, sr), &mut bus, &EventList::empty(), None);
        assert_eq!(bus.peak(256), (0.0, 0.0));
    }

    #[test]
    fn note_on_at_offset_is_sample_accurate() {
        let sr = 48_000;
        let frames = 512;
        let k = 200usize;
        let mut synth = make_synth(sr, MonoSynthParams::default());
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
        let mut synth = make_synth(sr, MonoSynthParams::default());
        let mut bus = StereoBus::with_capacity(4096);
        let mut events = EventList::empty();
        events.push(note_on(0, 7, 60));
        events.push(TimedEvent {
            offset: 100,
            event: Event::NoteOff { id: 7, note: 60 },
        });
        synth.process(&ctx(4096, sr), &mut bus, &events, None);
        // Default release is 0.15 s = 7200 samples; still ringing here but
        // decaying, and a long render afterwards must end the voice.
        assert!(synth.voice.active);
        let mut bus = StereoBus::with_capacity(16_000);
        synth.process(&ctx(16_000, sr), &mut bus, &EventList::empty(), None);
        assert!(!synth.voice.active);
        assert!(bus.l[8_000..].iter().all(|s| *s == 0.0));
    }

    #[test]
    fn stale_note_off_does_not_release_a_retriggered_voice() {
        let sr = 48_000;
        let mut synth = make_synth(sr, MonoSynthParams::default());
        let mut bus = StereoBus::with_capacity(64);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        events.push(note_on(8, 2, 64));
        events.push(TimedEvent {
            offset: 16,
            event: Event::NoteOff { id: 1, note: 60 },
        });
        synth.process(&ctx(64, sr), &mut bus, &events, None);
        assert!(synth.voice.active);
        assert_eq!(synth.voice.event_id, 2);
        assert!(!synth.voice.env.is_releasing());
    }

    #[test]
    fn no_glide_changes_pitch_instantly() {
        let sr = 48_000;
        let mut synth = make_synth(sr, MonoSynthParams::default());
        synth.note_on(1, 60, 100);
        synth.note_on(2, 72, 100);
        assert_eq!(synth.voice.current_freq, note_to_freq(72));
    }

    #[test]
    fn glide_approaches_pitch_gradually() {
        let sr = 48_000;
        let params = MonoSynthParams {
            glide: 0.1,
            ..MonoSynthParams::default()
        };
        let mut synth = make_synth(sr, params);
        synth.note_on(1, 60, 100);
        synth.note_on(2, 72, 100);

        // Immediately after the second note the frequency must still be near
        // the old one.
        assert!((synth.voice.current_freq - note_to_freq(60)).abs() < 1.0);

        let mut bus = StereoBus::with_capacity(sr as usize);
        synth.process(&ctx(1024, sr), &mut bus, &EventList::empty(), None);
        let early = synth.voice.current_freq;
        // One second is ten glide time constants: fully converged.
        synth.process(&ctx(sr as usize, sr), &mut bus, &EventList::empty(), None);
        let late = synth.voice.current_freq;

        assert!(early < note_to_freq(72));
        assert!((late - note_to_freq(72)).abs() < 1.0);
    }

    #[test]
    fn oscillator_levels_gate_their_contribution() {
        let sr = 48_000;
        let only_osc2 = MonoSynthParams {
            osc: [
                mooloop_core::OscParams {
                    level: 0.0,
                    ..Default::default()
                },
                mooloop_core::OscParams {
                    level: 0.8,
                    semitones: 12.0,
                    ..Default::default()
                },
                mooloop_core::OscParams {
                    level: 0.0,
                    ..Default::default()
                },
            ],
            attack: 0.0001,
            sustain: 1.0,
            ..MonoSynthParams::default()
        };
        let mut synth = make_synth(sr, only_osc2);
        synth.note_on(1, 60, 127);
        let mut bus = StereoBus::with_capacity(4096);
        synth.render_range(&mut bus, 0, 4096);
        // Osc 2 is an octave up: count upward zero crossings (~523 Hz).
        let mut crossings = 0u32;
        for window in bus.l[480..].windows(2) {
            if window[0] <= 0.0 && window[1] > 0.0 {
                crossings += 1;
            }
        }
        let expected = 523.0 * (4096 - 480) as f32 / sr as f32;
        assert!((crossings as f32 - expected).abs() <= 1.0, "{crossings}");
    }

    #[test]
    fn resonant_filter_and_drive_stay_bounded() {
        let sr = 48_000;
        let params = MonoSynthParams {
            filter_cutoff: 0.6,
            filter_resonance: 1.0,
            filter_env_amount: 1.0,
            drive: 1.0,
            sustain: 1.0,
            ..MonoSynthParams::default()
        };
        let mut synth = make_synth(sr, params);
        let mut bus = StereoBus::with_capacity(8192);
        let mut events = EventList::empty();
        events.push(note_on(0, 0, 38));
        synth.process(&ctx(8192, sr), &mut bus, &events, None);
        let (pl, pr) = bus.peak(8192);
        assert!(pl.is_finite() && pr.is_finite());
        assert!(pl <= 1.0);
    }

    #[test]
    fn stopping_transport_releases_the_voice() {
        let sr = 48_000;
        let mut synth = make_synth(sr, MonoSynthParams::default());
        let mut bus = StereoBus::with_capacity(64);
        let mut events = EventList::empty();
        events.push(note_on(0, 0, 60));
        synth.process(&ctx(64, sr), &mut bus, &events, None);

        let mut stopped = ctx(64, sr);
        stopped.playing = false;
        synth.process(&stopped, &mut bus, &EventList::empty(), None);
        assert!(synth.voice.env.is_releasing());
    }
}
