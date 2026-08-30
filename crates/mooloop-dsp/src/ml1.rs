//! The ML-1: one voice, two envelopes, and a filter that tracks the
//! keyboard.
//!
//! Deliberately not a variant of [`crate::monosynth`]. That device is the poly
//! synth with the voice count set to one; this one is built around the filter
//! and around note behaviour, per `docs/plans/mono-synth-v2/01-what-mono-is.md`.
//! What is here now:
//!
//! - a separate amplitude ADSR and filter ADSR, so a pluck (fast filter decay
//!   under a flat amplitude) and a swell (slow filter opening under a fast
//!   attack) are both reachable,
//! - cutoff keytracking referenced to middle C, derived from the *gliding*
//!   frequency so a slide sweeps the filter with the pitch,
//! - no device-local LFO. Modulation is channel state and arrives through the
//!   ordinary descriptor parameter events.
//!
//! - a real held-note stack ([`crate::heldnotes`]) with note priority, and
//!   independent legato and glide-mode switches, so note transitions are
//!   something the player performs rather than something the synth decides.
//!
//! Still to come, and called out so the gaps read as sequencing rather than
//! oversight: the Ladder and Acid filter models with saturation moved *ahead*
//! of the filter, and Accent. Drive stays post-filter until the model work
//! lands, because moving it without the makeup-gain scheme that step designs
//! would change loudness rather than character.

use crate::bus::StereoBus;
use crate::env::Adsr;
use crate::event::{Event, EventList};
use crate::filter::{apply_drive, Svf};
use crate::heldnotes::{HeldNote, HeldNotes};
use crate::node::{AudioNode, ProcessContext};
use crate::osc::Osc;
use crate::scale::hz_from_normalized;
use crate::smooth::Smoothed;
use mooloop_core::{EnvTrigger, GlideMode, Ml1Params};

/// Minimum glide time; at or below this, pitch changes are instant.
const MIN_GLIDE_S: f32 = 1.0e-3;

/// The voice's absolute output reference, set so one oscillator at its 0 dB
/// top (which the default patch runs at) peaks within a dB of
/// `mooloop_core::gain::REFERENCE_PEAK_DBFS` (-12 dBFS) at the master.
const VOICE_OUTPUT_REFERENCE: f32 = 0.36;

/// Fast release used when the transport stops (seconds).
const STOP_RELEASE_S: f32 = 0.005;

/// Lag applied to parameters that scale the signal directly.
const PARAM_SMOOTH_S: f32 = 0.005;

/// Middle C (MIDI 60). Keytracking is referenced here, so a patch voiced
/// around the middle of the keyboard keeps its cutoff where it was set.
const KEYTRACK_REFERENCE_HZ: f32 = 261.625_58;

/// MIDI note number to frequency in Hz (A4 = 69 = 440 Hz).
fn note_to_freq(note: u8) -> f32 {
    440.0 * 2.0_f32.powf((f32::from(note.min(127)) - 69.0) / 12.0)
}

struct Ml1Voice {
    active: bool,
    event_id: u64,
    /// Scales the VCA and decides when the voice goes idle.
    amp_env: Adsr,
    /// Sweeps the cutoff. Deliberately *not* consulted for idleness: a long
    /// filter release must not hold a silent voice alive.
    filter_env: Adsr,
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

impl Ml1Voice {
    fn new(sample_rate: u32) -> Self {
        let smoothed = |initial| Smoothed::new(initial, PARAM_SMOOTH_S, sample_rate);
        Self {
            active: false,
            event_id: 0,
            amp_env: Adsr::new(sample_rate),
            filter_env: Adsr::new(sample_rate),
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

    /// Point both envelopes at the current parameters. They are configured
    /// together everywhere, so they are configured together here too — the
    /// bug this avoids is a filter envelope left on stale times after a knob
    /// move.
    fn configure_envelopes(&mut self, params: &Ml1Params) {
        self.amp_env
            .configure(params.attack, params.decay, params.sustain, params.release);
        self.filter_env.configure(
            params.filter_attack,
            params.filter_decay,
            params.filter_sustain,
            params.filter_release,
        );
    }

    /// Adopt the current parameters without a ramp. Only safe when the voice
    /// is silent — there is nothing to click.
    fn snap_to(&mut self, params: &Ml1Params, velocity_amp: f32) {
        self.velocity_amp.reset_to(velocity_amp);
        for (smoothed, osc) in self.osc_level.iter_mut().zip(params.osc.iter()) {
            smoothed.reset_to(osc.level.clamp(0.0, 1.0));
        }
        self.cutoff.reset_to(params.filter_cutoff.clamp(0.0, 1.0));
        self.drive.reset_to(params.drive.clamp(0.0, 1.0));
    }
}

/// The ML-1 node.
pub struct Ml1 {
    params: Ml1Params,
    sample_rate: u32,
    voice: Ml1Voice,
    /// Every note currently down, not just the one sounding. This is what
    /// makes trills, fallback, and note priority possible at all.
    held: HeldNotes,
}

impl Ml1 {
    pub fn new(params: Ml1Params, sample_rate: u32) -> Self {
        let mut voice = Ml1Voice::new(sample_rate);
        voice.configure_envelopes(&params);
        voice.snap_to(&params, 0.0);
        Self {
            params,
            sample_rate,
            voice,
            held: HeldNotes::new(),
        }
    }

    /// Replace the parameter set. Called from the RT command drain.
    pub fn set_params(&mut self, params: Ml1Params) {
        self.params = params;
        self.voice.configure_envelopes(&params);
    }

    /// Apply one descriptor-addressed parameter, leaving the rest alone.
    ///
    /// Routed through `set_params` so a control-rate change gets exactly the
    /// same clamping and voice reconfiguration a whole-struct update does.
    fn apply_param(&mut self, id: u32, value: f32) {
        let mut params = mooloop_core::GeneratorParams::Ml1(self.params);
        if params.set(id, value).is_none() {
            return;
        }
        if let mooloop_core::GeneratorParams::Ml1(params) = params {
            self.set_params(params);
        }
    }

    /// Immediately invalidate the active voice and return every oscillator
    /// and filter to its initial state.
    pub fn reset(&mut self) {
        self.voice = Ml1Voice::new(self.sample_rate);
        self.voice.configure_envelopes(&self.params);
        self.voice.snap_to(&self.params, 0.0);
        self.held.clear();
    }

    pub fn choke(&mut self) {
        self.release_all();
    }

    fn note_on(&mut self, event_id: u64, note: u8, velocity: u8) {
        // Both switches key off the same question: were notes overlapping?
        let was_overlapping = !self.held.is_empty();
        let was_sounding = self.voice.active;
        self.held.push(HeldNote {
            event_id,
            note,
            velocity,
        });

        // Under `Low` or `High` priority a note can be pressed and still lose
        // to something already held. Then nothing happens at all: no pitch
        // change and no retrigger. It is on the stack, so releasing the
        // winner will fall back to it.
        let Some(winner) = self.held.winner(self.params.priority) else {
            return;
        };
        if winner.event_id != event_id {
            return;
        }

        if !was_sounding {
            // Fresh start: no glide from silence, clean filter and phases,
            // and every smoothed parameter taken up immediately.
            self.start_voice(note, velocity);
            self.voice.event_id = event_id;
            return;
        }

        // A note landing over a still-sounding release tail is where the two
        // glide modes part company: `Always` slides into the tail, `Legato`
        // only glides when the notes genuinely overlapped.
        let glide = if was_overlapping {
            true
        } else {
            self.params.glide_mode == GlideMode::Always
        };
        self.retarget(note, velocity, glide);

        // Legato holds the envelopes only for a genuinely overlapping note.
        // Taking over a release tail is a new note by any reading, and
        // restarting there is what stops the voice from fading out under it.
        if !was_overlapping || self.params.env_trigger == EnvTrigger::Retrig {
            self.voice.amp_env.note_on();
            self.voice.filter_env.note_on();
        }
        self.voice.event_id = event_id;
        self.voice.active = true;
    }

    /// Take the voice from silence. Nothing is smoothed or glided into,
    /// because there is nothing to click against.
    fn start_voice(&mut self, note: u8, velocity: u8) {
        let velocity_amp = f32::from(velocity) / 127.0;
        self.voice.target_freq = note_to_freq(note);
        self.voice.current_freq = self.voice.target_freq;
        self.voice.filter.reset();
        for osc in &mut self.voice.oscs {
            osc.reset();
        }
        self.voice.snap_to(&self.params, velocity_amp);
        self.voice.velocity_amp.set_target(velocity_amp);
        self.voice.amp_env.note_on();
        self.voice.filter_env.note_on();
        self.voice.active = true;
    }

    /// Move the sounding voice to a different note without touching its
    /// envelopes. This is the whole of what a fallback is, and it is why a
    /// trill works: releasing the top note of two returns to the lower one as
    /// a pitch change, not as a new note.
    fn retarget(&mut self, note: u8, velocity: u8, glide: bool) {
        self.voice.target_freq = note_to_freq(note);
        if !glide || self.params.glide <= MIN_GLIDE_S {
            self.voice.current_freq = self.voice.target_freq;
        }
        // While the voice is still sounding the new velocity has to slide in:
        // stepping the gain mid-note is as audible as stepping the envelope.
        self.voice
            .velocity_amp
            .set_target(f32::from(velocity) / 127.0);
    }

    fn note_off(&mut self, event_id: u64) {
        // A `NoteOff` for something not held is stale by definition. Bailing
        // here is what keeps it from releasing a newer note.
        if !self.held.remove(event_id) {
            return;
        }
        if !self.voice.active {
            return;
        }
        match self.held.winner(self.params.priority) {
            // Still holding something: this is a pitch change, never a
            // retrigger, whatever the envelope trigger mode says.
            Some(winner) => {
                if winner.event_id != self.voice.event_id {
                    self.retarget(winner.note, winner.velocity, true);
                    self.voice.event_id = winner.event_id;
                }
            }
            None => {
                self.voice.amp_env.release();
                self.voice.filter_env.release();
            }
        }
    }

    /// Transport stop and choke. The stack has to go with the voice: a held
    /// entry left behind would resurrect the voice on the next `NoteOff`
    /// fallback, long after the transport said stop.
    fn release_all(&mut self) {
        self.held.clear();
        if self.voice.active && !self.voice.amp_env.is_releasing() {
            self.voice.amp_env.release_with(STOP_RELEASE_S);
            self.voice.filter_env.release_with(STOP_RELEASE_S);
        }
    }

    fn render_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        let params = self.params;
        let sr = self.sample_rate;
        let voice = &mut self.voice;

        if !voice.active {
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
        let keytrack = params.filter_keytrack.clamp(0.0, 1.0);
        let max_hz = sr as f32 * 0.45;

        for i in start..end {
            voice.current_freq += (voice.target_freq - voice.current_freq) * (1.0 - glide_coeff);

            voice.amp_env.advance();
            voice.filter_env.advance();
            // Idleness is the amplitude envelope's call alone.
            if voice.amp_env.is_idle() {
                voice.active = false;
                return;
            }

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
                        voice.current_freq * ratio[index],
                        osc_params.wave,
                        osc_params.pulse_width,
                        sr,
                    );
            }

            // Envelope- and keytrack-modulated low-pass, same perceptual
            // mapping the sampler uses. Bypassed entirely when fully open and
            // nothing is moving it — keytrack has to be in that test, or a
            // tracking patch with the cutoff knob at the top would skip the
            // filter it is supposed to be playing.
            let cutoff = voice.cutoff.advance();
            let drive = voice.drive.advance();
            let filtered = if cutoff >= 0.999
                && env_amount.abs() <= f32::EPSILON
                && resonance <= f32::EPSILON
                && keytrack <= f32::EPSILON
            {
                mix
            } else {
                let base_hz = hz_from_normalized(cutoff, max_hz);
                // Read off `current_freq` rather than the note number so a
                // glide carries the cutoff along with the pitch, which is what
                // a slide is expected to sound like and costs nothing extra.
                let keytrack_oct = if keytrack <= f32::EPSILON {
                    0.0
                } else {
                    keytrack * (voice.current_freq / KEYTRACK_REFERENCE_HZ).log2()
                };
                let octaves = voice.filter_env.level() * env_amount * 6.0 + keytrack_oct;
                let cutoff_hz = (base_hz * octaves.exp2()).clamp(20.0, max_hz);
                voice.filter.next_sample(mix, cutoff_hz, resonance, sr)
            };

            let sample = apply_drive(filtered, drive)
                * voice.amp_env.level()
                * velocity
                * VOICE_OUTPUT_REFERENCE;
            bus.l[i] += sample;
            bus.r[i] += sample;
        }
    }
}

impl AudioNode for Ml1 {
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
                Event::ParamValue { id, value } => self.apply_param(id, value),
                Event::Buffer(_) | Event::BufferRelease | Event::BufferScrub { .. } => {}
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
    use mooloop_core::{NotePriority, OscParams, OscWave};

    const SR: u32 = 48_000;

    fn ctx(frames: usize) -> ProcessContext {
        ProcessContext {
            sample_rate: SR,
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

    /// A saw at a fixed level, so every test hears the filter rather than the
    /// oscillator mix.
    fn saw_patch() -> Ml1Params {
        let mut params = Ml1Params::default();
        params.osc[0] = OscParams {
            wave: OscWave::Saw,
            level: 1.0,
            ..OscParams::default()
        };
        params
    }

    fn render(params: Ml1Params, note: u8, frames: usize) -> Vec<f32> {
        let mut synth = Ml1::new(params, SR);
        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, note));
        synth.process(&ctx(frames), &mut bus, &events, None);
        bus.l[..frames].to_vec()
    }

    /// Energy above `cutoff_hz`, as a crude one-pole high-passed RMS. Used
    /// where two renders are being compared at matched settings.
    fn high_energy(samples: &[f32], cutoff_hz: f32) -> f32 {
        let coeff = (-std::f32::consts::TAU * cutoff_hz / SR as f32).exp();
        let mut lp = 0.0;
        let mut sum = 0.0;
        for &sample in samples {
            lp += (1.0 - coeff) * (sample - lp);
            let hp = sample - lp;
            sum += hp * hp;
        }
        (sum / samples.len().max(1) as f32).sqrt()
    }

    /// A cheap spectral-centroid proxy: the RMS of the first difference over
    /// the RMS of the signal, which rises with high-frequency content and is
    /// scale-invariant. Enough to say "this got brighter", which is all these
    /// tests need, and it does not confuse a level change for a timbre change
    /// the way a fixed-band energy measure does.
    fn brightness(samples: &[f32]) -> f32 {
        let level = rms(samples);
        if level <= 0.0 {
            return 0.0;
        }
        let diff: Vec<f32> = samples.windows(2).map(|w| w[1] - w[0]).collect();
        rms(&diff) / level
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt()
    }

    #[test]
    fn idle_is_silent() {
        let mut synth = Ml1::new(Ml1Params::default(), SR);
        let mut bus = StereoBus::with_capacity(256);
        synth.process(&ctx(256), &mut bus, &EventList::empty(), None);
        assert_eq!(bus.peak(256), (0.0, 0.0));
    }

    #[test]
    fn note_on_at_offset_is_sample_accurate() {
        let frames = 512;
        let k = 200usize;
        let mut synth = Ml1::new(saw_patch(), SR);
        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        events.push(note_on(k as u32, 0, 60));

        synth.process(&ctx(frames), &mut bus, &events, None);

        assert!(bus.l[..k].iter().all(|s| *s == 0.0));
        assert!(bus.l[k..].iter().any(|s| s.abs() > 0.001));
    }

    /// The shape the v1 synth cannot express at all: the amplitude sits at
    /// full sustain while the filter shuts down underneath it.
    #[test]
    fn filter_decays_under_a_flat_amplitude() {
        let mut params = saw_patch();
        params.attack = 0.001;
        params.decay = 0.001;
        params.sustain = 1.0;
        // The base cutoff sits well above the note's fundamental, so the
        // sweep is the filter opening and closing over the harmonics rather
        // than the filter deleting the note.
        params.filter_cutoff = 0.45;
        params.filter_attack = 0.001;
        params.filter_decay = 0.1;
        params.filter_sustain = 0.0;
        params.filter_env_amount = 0.5;

        let frames = (SR as f32 * 0.2) as usize;
        let mut synth = Ml1::new(params, SR);
        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 48));
        synth.process(&ctx(frames), &mut bus, &events, None);

        // The amplitude envelope is genuinely flat at sustain for the whole
        // render. Asserted on the envelope rather than on output RMS, because
        // closing a low-pass lowers broadband level whatever the VCA is doing
        // — which is exactly the confusion this instrument exists to remove.
        assert!(
            (synth.voice.amp_env.level() - 1.0).abs() < 1.0e-3,
            "amp envelope left sustain: {}",
            synth.voice.amp_env.level()
        );

        // Early is taken while the filter envelope is still near its peak —
        // a 100 ms decay is most of the way down by 20 ms — and late is well
        // after it has reached its zero sustain.
        let window = SR as usize / 200;
        let early = &bus.l[window..window * 2];
        let late = &bus.l[frames - window * 4..frames];
        // Brightness relative to the signal's own level, so this measures the
        // filter closing and not the level change that comes with it.
        let early_ratio = brightness(early);
        let late_ratio = brightness(late);
        assert!(
            late_ratio < early_ratio * 0.5,
            "filter did not close: early {early_ratio} late {late_ratio}"
        );
    }

    /// A short filter release under a long amplitude release darkens the tail,
    /// and the voice still ends when the amplitude envelope says so.
    #[test]
    fn filter_release_is_independent_of_amp_release() {
        let mut params = saw_patch();
        params.sustain = 1.0;
        params.release = 1.0;
        params.filter_cutoff = 0.3;
        params.filter_sustain = 1.0;
        params.filter_release = 0.02;
        params.filter_env_amount = 0.6;

        let mut synth = Ml1::new(params, SR);
        let frames = SR as usize / 2;
        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 48));
        events.push(TimedEvent {
            offset: SR / 10,
            event: Event::NoteOff { id: 1, note: 48 },
        });
        synth.process(&ctx(frames), &mut bus, &events, None);

        let held = &bus.l[SR as usize / 50..SR as usize / 10];
        let tail = &bus.l[SR as usize / 4..frames];
        assert!(
            synth.voice.active,
            "a one-second amp release must outlive a 20 ms filter release"
        );
        assert!(rms(tail) > 0.0);
        assert!(
            high_energy(tail, 1200.0) / rms(tail)
                < high_energy(held, 1200.0) / rms(held) * 0.6,
            "the tail should be darker relative to its own level"
        );
    }

    #[test]
    fn keytrack_opens_the_filter_with_the_note() {
        let mut params = saw_patch();
        params.filter_cutoff = 0.3;
        params.filter_keytrack = 1.0;
        let frames = SR as usize / 10;

        let low = render(params, 48, frames);
        let high = render(params, 60, frames);
        assert!(
            high_energy(&high, 2000.0) > high_energy(&low, 2000.0) * 1.5,
            "an octave up should be audibly brighter: low {} high {}",
            high_energy(&low, 2000.0),
            high_energy(&high, 2000.0)
        );

        params.filter_keytrack = 0.0;
        let low = render(params, 48, frames);
        let high = render(params, 60, frames);
        let ratio = high_energy(&high, 2000.0) / high_energy(&low, 2000.0).max(1.0e-9);
        assert!(
            ratio < 1.5,
            "with keytrack off the cutoff must not follow the note (ratio {ratio})"
        );
    }

    #[test]
    fn a_stale_note_off_does_not_release_a_retriggered_voice() {
        let mut synth = Ml1::new(saw_patch(), SR);
        let mut bus = StereoBus::with_capacity(1024);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        events.push(note_on(10, 2, 60));
        events.push(TimedEvent {
            offset: 20,
            event: Event::NoteOff { id: 1, note: 60 },
        });
        synth.process(&ctx(1024), &mut bus, &events, None);
        assert!(!synth.voice.amp_env.is_releasing());
        assert!(!synth.voice.filter_env.is_releasing());
    }

    fn note_off(offset: u32, id: u64, note: u8) -> TimedEvent {
        TimedEvent {
            offset,
            event: Event::NoteOff { id, note },
        }
    }

    /// A synth that settles at a partial sustain, which is what makes a
    /// retrigger observable at all.
    ///
    /// `Adsr::note_on` deliberately does not reset the level — it attacks from
    /// wherever the envelope already is, so a retrigger over a sounding voice
    /// does not click. So a restart is not a dip to zero; it is a *climb back
    /// to the peak* from sustain. These tests measure that climb.
    fn sustained(params: impl FnOnce(&mut Ml1Params)) -> Ml1 {
        let mut p = saw_patch();
        p.attack = 0.005;
        p.decay = 0.01;
        p.sustain = SUSTAIN;
        p.release = 0.5;
        p.filter_attack = 0.005;
        p.filter_decay = 0.01;
        p.filter_sustain = SUSTAIN;
        p.filter_release = 0.5;
        params(&mut p);
        Ml1::new(p, SR)
    }

    const SUSTAIN: f32 = 0.4;

    /// Feed the events one sample at a time and report the highest level each
    /// envelope reaches after `from`. Above sustain means it restarted.
    fn envelope_peaks(
        synth: &mut Ml1,
        events: &EventList,
        frames: usize,
        from: usize,
    ) -> (f32, f32) {
        let mut bus = StereoBus::with_capacity(frames);
        let (mut amp, mut filter) = (0.0_f32, 0.0_f32);
        for offset in 0..frames {
            let mut slice = EventList::empty();
            for ev in events.iter() {
                if ev.offset as usize == offset {
                    slice.push(TimedEvent {
                        offset: 0,
                        event: ev.event,
                    });
                }
            }
            synth.process(&ctx(1), &mut bus, &slice, None);
            if offset >= from {
                amp = amp.max(synth.voice.amp_env.level());
                filter = filter.max(synth.voice.filter_env.level());
            }
        }
        (amp, filter)
    }

    #[test]
    fn legato_changes_pitch_without_restarting_either_envelope() {
        let mut synth = sustained(|p| p.env_trigger = EnvTrigger::Legato);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        events.push(note_on(2000, 2, 67));

        let (amp, filter) = envelope_peaks(&mut synth, &events, 4000, 2000);
        assert!(
            amp <= SUSTAIN + 1.0e-3 && filter <= SUSTAIN + 1.0e-3,
            "legato restarted an envelope (amp {amp}, filter {filter})"
        );
        assert!((synth.voice.target_freq - note_to_freq(67)).abs() < 0.01);
    }

    #[test]
    fn retrig_restarts_both_envelopes_on_an_overlapping_note() {
        let mut synth = sustained(|p| p.env_trigger = EnvTrigger::Retrig);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        events.push(note_on(2000, 2, 67));

        let (amp, filter) = envelope_peaks(&mut synth, &events, 4000, 2000);
        assert!(
            amp > 0.95 && filter > 0.95,
            "retrig did not restart both envelopes (amp {amp}, filter {filter})"
        );
    }

    /// Releasing the winner while another note is still down is a pitch
    /// change, not a note-on — in *either* trigger mode. That is what makes a
    /// trill work.
    #[test]
    fn releasing_the_winner_falls_back_without_restarting_envelopes() {
        for trigger in [EnvTrigger::Retrig, EnvTrigger::Legato] {
            let mut synth = sustained(|p| p.env_trigger = trigger);
            let mut events = EventList::empty();
            events.push(note_on(0, 1, 60));
            events.push(note_on(1000, 2, 67));
            events.push(note_off(2000, 2, 67));

            let (amp, filter) = envelope_peaks(&mut synth, &events, 4000, 2000);
            assert!(
                amp <= SUSTAIN + 1.0e-3 && filter <= SUSTAIN + 1.0e-3,
                "{trigger:?} restarted an envelope on fallback (amp {amp}, filter {filter})"
            );
            assert!(
                (synth.voice.target_freq - note_to_freq(60)).abs() < 0.01,
                "{trigger:?} did not fall back to the held note"
            );
        }
    }

    #[test]
    fn each_priority_takes_its_own_note() {
        for (priority, expected) in [
            (NotePriority::Last, 55),
            (NotePriority::Low, 48),
            (NotePriority::High, 67),
        ] {
            let mut synth = sustained(|p| p.priority = priority);
            let mut bus = StereoBus::with_capacity(512);
            let mut events = EventList::empty();
            events.push(note_on(0, 1, 67));
            events.push(note_on(1, 2, 48));
            events.push(note_on(2, 3, 55));
            synth.process(&ctx(512), &mut bus, &events, None);
            assert!(
                (synth.voice.target_freq - note_to_freq(expected)).abs() < 0.01,
                "{priority:?} chose {} Hz, wanted note {expected}",
                synth.voice.target_freq
            );
        }
    }

    /// Under `Low`, a higher note pressed over a held one loses. It does not
    /// take the voice and it does not retrigger — but it is on the stack, so
    /// releasing the low note hands the voice to it.
    #[test]
    fn a_losing_note_does_not_take_the_voice_but_is_still_held() {
        let mut synth = sustained(|p| p.priority = NotePriority::Low);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 48));
        events.push(note_on(2000, 2, 72));

        let (amp, filter) = envelope_peaks(&mut synth, &events, 4000, 2000);
        assert!(
            amp <= SUSTAIN + 1.0e-3 && filter <= SUSTAIN + 1.0e-3,
            "a losing note retriggered the envelopes (amp {amp}, filter {filter})"
        );
        assert!((synth.voice.target_freq - note_to_freq(48)).abs() < 0.01);

        let mut bus = StereoBus::with_capacity(512);
        let mut events = EventList::empty();
        events.push(note_off(0, 1, 48));
        synth.process(&ctx(512), &mut bus, &events, None);
        assert!(!synth.voice.amp_env.is_releasing());
        assert!((synth.voice.target_freq - note_to_freq(72)).abs() < 0.01);
    }

    #[test]
    fn releasing_a_note_that_is_not_the_winner_changes_nothing() {
        let mut synth = sustained(|_| {});
        let mut bus = StereoBus::with_capacity(512);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        events.push(note_on(1, 2, 67));
        events.push(note_off(2, 1, 60));
        synth.process(&ctx(512), &mut bus, &events, None);

        assert!(!synth.voice.amp_env.is_releasing());
        assert!((synth.voice.target_freq - note_to_freq(67)).abs() < 0.01);
    }

    /// The one row of the table where the two glide modes disagree: a note
    /// landing on a still-sounding release tail.
    #[test]
    fn glide_modes_differ_only_over_a_release_tail() {
        for (mode, glides) in [(GlideMode::Always, true), (GlideMode::Legato, false)] {
            let mut synth = sustained(|p| {
                p.glide = 0.5;
                p.glide_mode = mode;
            });
            let mut bus = StereoBus::with_capacity(4096);
            let mut events = EventList::empty();
            events.push(note_on(0, 1, 48));
            synth.process(&ctx(2048), &mut bus, &events, None);

            // Release, let the tail run, then land a new note on top of it.
            let mut events = EventList::empty();
            events.push(note_off(0, 1, 48));
            synth.process(&ctx(2048), &mut bus, &events, None);
            let mut events = EventList::empty();
            events.push(note_on(0, 2, 72));
            synth.process(&ctx(1), &mut bus, &events, None);

            let started_at_the_old_pitch =
                (synth.voice.current_freq - note_to_freq(48)).abs() < 5.0;
            assert_eq!(
                started_at_the_old_pitch, glides,
                "{mode:?} over a release tail started at {} Hz",
                synth.voice.current_freq
            );
        }
    }

    /// Both modes glide between genuinely overlapping notes, and neither
    /// glides from silence.
    #[test]
    fn overlapping_notes_glide_and_silence_never_does() {
        for mode in [GlideMode::Always, GlideMode::Legato] {
            let mut synth = sustained(|p| {
                p.glide = 0.5;
                p.glide_mode = mode;
            });
            let mut bus = StereoBus::with_capacity(2048);
            let mut events = EventList::empty();
            events.push(note_on(0, 1, 48));
            synth.process(&ctx(1), &mut bus, &events, None);
            assert!(
                (synth.voice.current_freq - note_to_freq(48)).abs() < 0.01,
                "{mode:?} glided from silence"
            );

            let mut events = EventList::empty();
            events.push(note_on(0, 2, 72));
            synth.process(&ctx(1), &mut bus, &events, None);
            assert!(
                (synth.voice.current_freq - note_to_freq(48)).abs() < 5.0,
                "{mode:?} did not glide between overlapping notes"
            );
        }
    }

    #[test]
    fn a_transport_stop_clears_the_held_notes() {
        let mut synth = sustained(|_| {});
        let mut bus = StereoBus::with_capacity(512);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        events.push(note_on(1, 2, 67));
        synth.process(&ctx(512), &mut bus, &events, None);

        let mut stopped = ctx(512);
        stopped.playing = false;
        synth.process(&stopped, &mut bus, &EventList::empty(), None);
        assert!(synth.held.is_empty());

        // A late NoteOff must not resurrect the voice through a fallback.
        let mut events = EventList::empty();
        events.push(note_off(0, 2, 67));
        synth.process(&stopped, &mut bus, &events, None);
        assert!(synth.voice.amp_env.is_releasing() || !synth.voice.active);
    }

    #[test]
    fn resonant_filter_and_drive_stay_bounded() {
        let mut params = saw_patch();
        params.osc[1].level = 1.0;
        params.osc[2].level = 1.0;
        params.filter_cutoff = 0.2;
        params.filter_resonance = 1.0;
        params.filter_env_amount = 1.0;
        params.filter_decay = 0.05;
        params.filter_sustain = 0.0;
        params.filter_keytrack = 1.0;
        params.drive = 1.0;

        let frames = SR as usize;
        let rendered = render(params, 36, frames);
        let peak = rendered.iter().fold(0.0_f32, |a, s| a.max(s.abs()));
        assert!(peak.is_finite(), "output went non-finite");
        assert!(peak <= 1.0, "peak {peak} exceeded full scale");
    }
}
