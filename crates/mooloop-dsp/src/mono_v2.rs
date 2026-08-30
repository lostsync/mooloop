//! The v2 mono synth: one voice, two envelopes, and a filter that tracks the
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
//! Still to come, and called out so the gaps read as sequencing rather than
//! oversight: the held-note stack and its legato/priority modes, the Ladder
//! and Acid filter models with saturation moved *ahead* of the filter, and
//! Accent. Drive stays post-filter until the model work lands, because moving
//! it without the makeup-gain scheme that step designs would change loudness
//! rather than character.

use crate::bus::StereoBus;
use crate::env::Adsr;
use crate::event::{Event, EventList};
use crate::filter::{apply_drive, Svf};
use crate::node::{AudioNode, ProcessContext};
use crate::osc::Osc;
use crate::scale::hz_from_normalized;
use crate::smooth::Smoothed;
use mooloop_core::MonoV2Params;

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

struct MonoV2Voice {
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

impl MonoV2Voice {
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
    fn configure_envelopes(&mut self, params: &MonoV2Params) {
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
    fn snap_to(&mut self, params: &MonoV2Params, velocity_amp: f32) {
        self.velocity_amp.reset_to(velocity_amp);
        for (smoothed, osc) in self.osc_level.iter_mut().zip(params.osc.iter()) {
            smoothed.reset_to(osc.level.clamp(0.0, 1.0));
        }
        self.cutoff.reset_to(params.filter_cutoff.clamp(0.0, 1.0));
        self.drive.reset_to(params.drive.clamp(0.0, 1.0));
    }
}

/// The v2 mono synth node.
pub struct MonoV2 {
    params: MonoV2Params,
    sample_rate: u32,
    voice: MonoV2Voice,
}

impl MonoV2 {
    pub fn new(params: MonoV2Params, sample_rate: u32) -> Self {
        let mut voice = MonoV2Voice::new(sample_rate);
        voice.configure_envelopes(&params);
        voice.snap_to(&params, 0.0);
        Self {
            params,
            sample_rate,
            voice,
        }
    }

    /// Replace the parameter set. Called from the RT command drain.
    pub fn set_params(&mut self, params: MonoV2Params) {
        self.params = params;
        self.voice.configure_envelopes(&params);
    }

    /// Apply one descriptor-addressed parameter, leaving the rest alone.
    ///
    /// Routed through `set_params` so a control-rate change gets exactly the
    /// same clamping and voice reconfiguration a whole-struct update does.
    fn apply_param(&mut self, id: u32, value: f32) {
        let mut params = mooloop_core::GeneratorParams::MonoV2(self.params);
        if params.set(id, value).is_none() {
            return;
        }
        if let mooloop_core::GeneratorParams::MonoV2(params) = params {
            self.set_params(params);
        }
    }

    /// Immediately invalidate the active voice and return every oscillator
    /// and filter to its initial state.
    pub fn reset(&mut self) {
        self.voice = MonoV2Voice::new(self.sample_rate);
        self.voice.configure_envelopes(&self.params);
        self.voice.snap_to(&self.params, 0.0);
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
        self.voice.amp_env.note_on();
        self.voice.filter_env.note_on();
        self.voice.active = true;
    }

    fn note_off(&mut self, event_id: u64) {
        if self.voice.active && self.voice.event_id == event_id {
            self.voice.amp_env.release();
            self.voice.filter_env.release();
        }
    }

    fn release_all(&mut self) {
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

impl AudioNode for MonoV2 {
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
    use mooloop_core::{OscParams, OscWave};

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
    fn saw_patch() -> MonoV2Params {
        let mut params = MonoV2Params::default();
        params.osc[0] = OscParams {
            wave: OscWave::Saw,
            level: 1.0,
            ..OscParams::default()
        };
        params
    }

    fn render(params: MonoV2Params, note: u8, frames: usize) -> Vec<f32> {
        let mut synth = MonoV2::new(params, SR);
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
        let mut synth = MonoV2::new(MonoV2Params::default(), SR);
        let mut bus = StereoBus::with_capacity(256);
        synth.process(&ctx(256), &mut bus, &EventList::empty(), None);
        assert_eq!(bus.peak(256), (0.0, 0.0));
    }

    #[test]
    fn note_on_at_offset_is_sample_accurate() {
        let frames = 512;
        let k = 200usize;
        let mut synth = MonoV2::new(saw_patch(), SR);
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
        let mut synth = MonoV2::new(params, SR);
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

        let mut synth = MonoV2::new(params, SR);
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
        let mut synth = MonoV2::new(saw_patch(), SR);
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
