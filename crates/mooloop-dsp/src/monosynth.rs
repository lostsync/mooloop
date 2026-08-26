//! A three-oscillator mono synth. It slots into a channel exactly like the
//! sampler — same `AudioNode` trait, same segment-based, sample-accurate
//! event handling.
//!
//! Deliberately compact: three band-limited oscillators with per-osc pitch
//! and level into one envelope-modulated low-pass filter, one ADSR, glide,
//! drive, and one LFO. Mono because note duration and note-off semantics come
//! first in this project (see `PRODUCT.md`), and a single gated voice
//! exercises exactly that contract.

use crate::bus::StereoBus;
use crate::env::Adsr;
use crate::event::{Event, EventList};
use crate::filter::{apply_drive, Svf};
use crate::lfo::Lfo;
use crate::node::{AudioNode, ProcessContext};
use crate::osc::Osc;
use crate::scale::hz_from_normalized;
use crate::smooth::Smoothed;
use mooloop_core::MonoSynthParams;

/// Minimum glide time; at or below this, pitch changes are instant.
const MIN_GLIDE_S: f32 = 1.0e-3;

/// Fast release used when the transport stops (seconds).
const STOP_RELEASE_S: f32 = 0.005;

/// Lag applied to parameters that scale the signal directly. Long enough to
/// bury a block-boundary step, short enough to feel immediate on a knob.
const PARAM_SMOOTH_S: f32 = 0.005;

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
    /// Velocity gain, smoothed so that a retrigger at a different velocity
    /// slides rather than steps.
    velocity_amp: Smoothed,
    osc_level: [Smoothed; 3],
    cutoff: Smoothed,
    drive: Smoothed,
}

impl MonoVoice {
    fn new(sample_rate: u32) -> Self {
        let smoothed = |initial| Smoothed::new(initial, PARAM_SMOOTH_S, sample_rate);
        Self {
            active: false,
            event_id: 0,
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
    /// is silent — there is nothing to click.
    fn snap_to(&mut self, params: &MonoSynthParams, velocity_amp: f32) {
        self.velocity_amp.reset_to(velocity_amp);
        for (smoothed, osc) in self.osc_level.iter_mut().zip(params.osc.iter()) {
            smoothed.reset_to(osc.level.clamp(0.0, 1.0));
        }
        self.cutoff.reset_to(params.filter_cutoff.clamp(0.0, 1.0));
        self.drive.reset_to(params.drive.clamp(0.0, 1.0));
    }
}

/// The mono synth node.
pub struct MonoSynth {
    params: MonoSynthParams,
    sample_rate: u32,
    voice: MonoVoice,
    /// Free running unless the LFO is set to retrigger, so it keeps its phase
    /// across notes and across silence.
    lfo: Lfo,
}

impl MonoSynth {
    pub fn new(params: MonoSynthParams, sample_rate: u32) -> Self {
        let mut voice = MonoVoice::new(sample_rate);
        voice
            .env
            .configure(params.attack, params.decay, params.sustain, params.release);
        voice.snap_to(&params, 0.0);
        Self {
            params,
            sample_rate,
            voice,
            lfo: Lfo::new(),
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
        self.voice.snap_to(&self.params, 0.0);
        self.lfo = Lfo::new();
    }

    pub fn choke(&mut self) {
        self.release_all();
    }

    fn note_on(&mut self, event_id: u64, note: u8, velocity: u8) {
        let was_active = self.voice.active;
        self.voice.event_id = event_id;
        self.voice.target_freq = note_to_freq(note);
        let velocity_amp = f32::from(velocity) / 127.0;
        if !was_active {
            // Fresh start: no glide from silence, clean filter and phases,
            // and every smoothed parameter taken up immediately.
            self.voice.current_freq = self.voice.target_freq;
            self.voice.filter.reset();
            for osc in &mut self.voice.oscs {
                osc.reset();
            }
            self.voice.snap_to(&self.params, velocity_amp);
        } else if self.params.glide <= MIN_GLIDE_S {
            self.voice.current_freq = self.voice.target_freq;
        }
        // While the voice is still sounding the new velocity has to slide in:
        // stepping the gain mid-note is as audible as stepping the envelope.
        self.voice.velocity_amp.set_target(velocity_amp);
        self.voice.env.note_on();
        self.voice.active = true;
        if self.params.lfo.retrigger {
            self.lfo.retrigger();
        }
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
        let params = self.params;
        let sr = self.sample_rate;
        let lfo_params = params.lfo;
        let voice = &mut self.voice;
        let lfo = &mut self.lfo;

        if !voice.active {
            // Keep a free-running LFO aligned with the transport across the
            // gaps between notes.
            lfo.skip(end.saturating_sub(start), lfo_params.rate_hz, sr);
            return;
        }

        // Glide: one-pole approach to the target frequency.
        let glide_coeff = (-1.0 / (params.glide.max(MIN_GLIDE_S) * sr as f32)).exp();

        // Per-oscillator pitch ratios from semitone/cent offsets.
        let mut ratio = [0.0_f32; 3];
        for (index, osc) in params.osc.iter().enumerate() {
            let semis = osc.semitones.clamp(-48.0, 48.0) + osc.cents.clamp(-100.0, 100.0) / 100.0;
            ratio[index] = 2.0_f32.powf(semis / 12.0);
        }

        // Signal-scaling parameters lag their targets; everything else is
        // cheap enough to read straight from the block's parameters.
        for (smoothed, osc) in voice.osc_level.iter_mut().zip(params.osc.iter()) {
            smoothed.set_target(osc.level.clamp(0.0, 1.0));
        }
        voice
            .cutoff
            .set_target(params.filter_cutoff.clamp(0.0, 1.0));
        voice.drive.set_target(params.drive.clamp(0.0, 1.0));

        let env_amount = params.filter_env_amount.clamp(-1.0, 1.0);
        let resonance = params.filter_resonance.clamp(0.0, 1.0);
        let to_pitch = lfo_params.to_pitch.clamp(-24.0, 24.0);
        let to_filter = lfo_params.to_filter.clamp(-4.0, 4.0);
        let to_pulse_width = lfo_params.to_pulse_width.clamp(-0.45, 0.45);
        let to_amp = lfo_params.to_amp.clamp(0.0, 1.0);
        let max_hz = sr as f32 * 0.45;

        for i in start..end {
            voice.current_freq += (voice.target_freq - voice.current_freq) * (1.0 - glide_coeff);

            voice.env.advance();
            if voice.env.is_idle() {
                voice.active = false;
                lfo.skip(end - i, lfo_params.rate_hz, sr);
                return;
            }

            let lfo_value = lfo.next_sample(lfo_params.rate_hz, lfo_params.wave, sr);
            let pitch_mod = if to_pitch == 0.0 {
                1.0
            } else {
                (lfo_value * to_pitch / 12.0).exp2()
            };
            let velocity = voice.velocity_amp.advance();

            let mut mix = 0.0;
            for (index, osc) in voice.oscs.iter_mut().enumerate() {
                let osc_params = params.osc[index];
                let osc_level = voice.osc_level[index].advance();
                if osc_level <= 1.0e-5 && osc_params.level <= 1.0e-5 {
                    continue;
                }
                mix += osc_level
                    * osc.next_sample(
                        voice.current_freq * ratio[index] * pitch_mod,
                        osc_params.wave,
                        osc_params.pulse_width + lfo_value * to_pulse_width,
                        sr,
                    );
            }

            // Envelope- and LFO-modulated low-pass, same perceptual mapping
            // the sampler uses. Bypassed entirely when fully open.
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
                voice.filter.next_sample(mix, cutoff_hz, resonance, sr)
            };

            // Tremolo modulates downward from unity, so depth never makes the
            // voice louder than it is without the LFO.
            let tremolo = 1.0 - to_amp * (1.0 - lfo_value) * 0.5;
            let sample = apply_drive(filtered, drive) * voice.env.level() * velocity * tremolo;
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

    /// A low sine, so that any step in the output is the synth's doing and
    /// not the waveform's: at 65 Hz a sine moves at most ~0.007 per sample.
    fn smooth_sine_params() -> MonoSynthParams {
        MonoSynthParams {
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
            ..MonoSynthParams::default()
        }
    }

    /// Largest sample-to-sample step in a rendered buffer.
    fn max_step(samples: &[f32]) -> f32 {
        samples
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn retriggering_a_sounding_voice_does_not_step_the_output() {
        let sr = 48_000;
        let mut synth = make_synth(sr, smooth_sine_params());
        let mut bus = StereoBus::with_capacity(4096);

        let mut events = EventList::empty();
        events.push(note_on(0, 1, 36));
        synth.process(&ctx(4096, sr), &mut bus, &events, None);
        assert!(max_step(&bus.l[..4096]) < 0.01);

        // A second note over the top of the first, at a much lower velocity:
        // both the envelope restart and the velocity change used to land as
        // a one-sample jump straight through zero.
        let mut bus = StereoBus::with_capacity(4096);
        let mut events = EventList::empty();
        events.push(TimedEvent {
            offset: 1000,
            event: Event::NoteOn {
                id: 2,
                note: 36,
                velocity: 30,
            },
        });
        synth.process(&ctx(4096, sr), &mut bus, &events, None);
        assert!(
            max_step(&bus.l[..4096]) < 0.01,
            "{}",
            max_step(&bus.l[..4096])
        );
    }

    #[test]
    fn a_note_over_a_releasing_tail_does_not_step_the_output() {
        let sr = 48_000;
        let mut synth = make_synth(sr, smooth_sine_params());
        let mut bus = StereoBus::with_capacity(4096);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 36));
        events.push(TimedEvent {
            offset: 2000,
            event: Event::NoteOff { id: 1, note: 36 },
        });
        synth.process(&ctx(4096, sr), &mut bus, &events, None);

        // Still in the 150 ms release when the next note arrives.
        let mut bus = StereoBus::with_capacity(4096);
        let mut events = EventList::empty();
        events.push(note_on(500, 2, 36));
        synth.process(&ctx(4096, sr), &mut bus, &events, None);
        assert!(
            max_step(&bus.l[..4096]) < 0.01,
            "{}",
            max_step(&bus.l[..4096])
        );
    }

    #[test]
    fn parameter_changes_mid_note_do_not_step_the_output() {
        let sr = 48_000;
        let params = smooth_sine_params();
        let mut synth = make_synth(sr, params);
        let mut bus = StereoBus::with_capacity(4096);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 36));
        synth.process(&ctx(4096, sr), &mut bus, &events, None);

        // Knob turns arrive between blocks; taken raw they step the signal.
        let mut silenced = params;
        silenced.osc[0].level = 0.0;
        silenced.drive = 1.0;
        synth.set_params(silenced);
        let mut bus = StereoBus::with_capacity(4096);
        synth.process(&ctx(4096, sr), &mut bus, &EventList::empty(), None);
        // A looser bound than the note tests: the ramps themselves move the
        // signal over 5 ms. It still fails by an order of magnitude if the
        // new level is taken up in one sample.
        assert!(
            max_step(&bus.l[..4096]) < 0.05,
            "{}",
            max_step(&bus.l[..4096])
        );
        // ...and the ramp does still arrive at the new level.
        assert!(bus.l[3500..4096].iter().all(|s| s.abs() < 1.0e-3));
    }

    #[test]
    fn an_idle_lfo_leaves_the_voice_untouched() {
        let sr = 48_000;
        let mut with_lfo = make_synth(sr, MonoSynthParams::default());
        let mut without = make_synth(
            sr,
            MonoSynthParams {
                lfo: mooloop_core::LfoParams {
                    rate_hz: 0.0,
                    ..Default::default()
                },
                ..MonoSynthParams::default()
            },
        );
        let mut a = StereoBus::with_capacity(4096);
        let mut b = StereoBus::with_capacity(4096);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        with_lfo.process(&ctx(4096, sr), &mut a, &events, None);
        without.process(&ctx(4096, sr), &mut b, &events, None);
        assert_eq!(a.l[..4096], b.l[..4096]);
    }

    #[test]
    fn lfo_pitch_depth_raises_the_pitch_over_the_rising_half_cycle() {
        let sr = 48_000;
        let base = MonoSynthParams {
            osc: [
                mooloop_core::OscParams {
                    wave: mooloop_core::OscWave::Sine,
                    level: 0.8,
                    ..Default::default()
                },
                mooloop_core::OscParams::default(),
                mooloop_core::OscParams::default(),
            ],
            attack: 0.001,
            sustain: 1.0,
            ..MonoSynthParams::default()
        };
        let crossings = |params: MonoSynthParams| {
            let mut synth = make_synth(sr, params);
            let mut bus = StereoBus::with_capacity(12_000);
            let mut events = EventList::empty();
            events.push(note_on(0, 1, 60));
            synth.process(&ctx(12_000, sr), &mut bus, &events, None);
            bus.l[..12_000]
                .windows(2)
                .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
                .count()
        };
        let modulated = MonoSynthParams {
            lfo: mooloop_core::LfoParams {
                rate_hz: 2.0,
                retrigger: true,
                to_pitch: 12.0,
                ..Default::default()
            },
            ..base
        };
        // A quarter second is the rising half of a 2 Hz sine, so the pitch
        // spends all of it above the played note.
        assert!(crossings(modulated) > crossings(base));
    }

    #[test]
    fn lfo_tremolo_silences_the_trough_at_full_depth() {
        let sr = 48_000;
        let params = MonoSynthParams {
            osc: [
                mooloop_core::OscParams {
                    level: 0.8,
                    ..Default::default()
                },
                mooloop_core::OscParams::default(),
                mooloop_core::OscParams::default(),
            ],
            attack: 0.0005,
            sustain: 1.0,
            lfo: mooloop_core::LfoParams {
                wave: mooloop_core::LfoWave::Square,
                rate_hz: 10.0,
                retrigger: true,
                to_amp: 1.0,
                ..Default::default()
            },
            ..MonoSynthParams::default()
        };
        let mut synth = make_synth(sr, params);
        let mut bus = StereoBus::with_capacity(4800);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        synth.process(&ctx(4800, sr), &mut bus, &events, None);

        // 10 Hz square: the first half cycle is 2400 samples of unity gain,
        // the second is 2400 samples of nothing.
        assert!(bus.l[100..2_300].iter().any(|s| s.abs() > 0.1));
        assert!(bus.l[2_400..4_800].iter().all(|s| *s == 0.0));
    }

    #[test]
    fn a_free_running_lfo_keeps_time_across_a_silent_gap() {
        let sr = 48_000;
        let params = MonoSynthParams {
            lfo: mooloop_core::LfoParams {
                wave: mooloop_core::LfoWave::Square,
                rate_hz: 10.0,
                to_amp: 1.0,
                ..Default::default()
            },
            attack: 0.0005,
            sustain: 1.0,
            ..MonoSynthParams::default()
        };
        let mut synth = make_synth(sr, params);
        // Silence for exactly half an LFO cycle, then a note: the LFO should
        // be in its trough, so the note starts muted.
        let mut bus = StereoBus::with_capacity(2400);
        synth.process(&ctx(2400, sr), &mut bus, &EventList::empty(), None);
        let mut bus = StereoBus::with_capacity(2400);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        synth.process(&ctx(2400, sr), &mut bus, &events, None);
        assert!(bus.l[..2_300].iter().all(|s| *s == 0.0));
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
