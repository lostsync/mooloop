//! A three-oscillator polyphonic synth. Built on the same primitives as the
//! mono synth — band-limited oscillators, ADSR, state-variable low-pass, LFO,
//! and drive — but with an independent voice pool, per-voice envelope and
//! filter, and a stereo spread.

use crate::bus::{pan_gains, StereoBus};
use crate::env::Adsr;
use crate::event::{Event, EventList};
use crate::filter::{apply_drive, Svf};
use crate::heldnotes::{HeldNote, HeldNotes};
use crate::lfo::Lfo;
use crate::node::{AudioNode, ProcessContext, SourceNode};
use crate::taps::AudioTaps;
use crate::osc::Osc;
use crate::scale::hz_from_normalized;
use crate::smooth::Smoothed;
use crate::synth_voice::{note_to_freq, MIN_GLIDE_S, PARAM_SMOOTH_S, STOP_RELEASE_S};
use mooloop_core::{
    EnvTrigger, PolySynthParams, MAX_POLY_VOICES, OSC_CENT_RANGE, OSC_SEMITONE_RANGE,
};

/// The voice's absolute output reference, set so one oscillator at its 0 dB
/// top (which the default patch runs at) peaks within a dB of
/// `mooloop_core::gain::REFERENCE_PEAK_DBFS` (-12 dBFS) at the master.
const VOICE_OUTPUT_REFERENCE: f32 = 0.51;

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
    /// Whether the last block ran with the transport playing. A stop releases
    /// what was sounding when it happened, once, rather than on every stopped
    /// block: a note played while stopped -- a MIDI keyboard, an audition --
    /// has to last until its own note-off.
    was_playing: bool,
    /// What is under the player's fingers, in mono mode.
    ///
    /// Empty and untouched while `mono_mode` is off, and the module it comes
    /// from was built outside any one instrument for exactly this: the ML-M1
    /// needed it first and its own header says the poly synth would want the
    /// same thing "the moment it grows a mono mode".
    held: HeldNotes,
}

impl PolySynth {
    pub fn new(params: PolySynthParams, sample_rate: u32) -> Self {
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
            was_playing: false,
            held: HeldNotes::new(),
        };
        synth.apply_params_to_voices(synth.voice_limit() as u8);
        synth
    }

    /// Replace the parameter set. Called from the RT command drain.
    pub fn set_params(&mut self, params: PolySynthParams) {
        // Leaving or entering mono mode drops whatever the stack was holding.
        // It is only read in mono mode, so an entry that survived the trip out
        // and back would be a note nobody is holding deciding the pitch of the
        // next one.
        if params.mono_mode != self.params.mono_mode {
            self.held.clear();
        }
        self.params = params;
        self.apply_params_to_voices(self.voice_limit() as u8);
    }

    /// Apply one descriptor-addressed parameter, leaving the rest alone.
    ///
    /// Routed through `set_params` rather than writing the field directly so a
    /// control-rate change gets exactly the same clamping and voice
    /// reconfiguration a whole-struct update does. Both are non-allocating.
    fn apply_param(&mut self, id: u32, value: f32) {
        let mut params = mooloop_core::GeneratorParams::PolySynth(self.params);
        if params.set(id, value).is_none() {
            return;
        }
        if let mooloop_core::GeneratorParams::PolySynth(params) = params {
            self.set_params(params);
        }
    }

    fn apply_params_to_voices(&mut self, polyphony: u8) {
        for (index, voice) in self.voices.iter_mut().enumerate() {
            voice.env.configure(
                self.params.attack,
                self.params.decay,
                self.params.sustain,
                self.params.release,
            );
            // A slot that Mono or a lowered Voices count no longer covers
            // fades out rather than stopping mid-waveform (MOO-110). It keeps
            // rendering until it has: `render_range` walks every slot.
            if index >= polyphony as usize && voice.active && !voice.env.is_releasing() {
                voice.env.release_with(STOP_RELEASE_S);
            }
        }
    }

    /// Immediately invalidate every voice and return every oscillator and
    /// filter to its initial state.
    pub fn reset(&mut self) {
        let polyphony = self.voice_limit() as u8;
        self.held.clear();
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

    /// How many voice slots are in play.
    ///
    /// **Mono mode goes through here rather than beside it.** Everything that
    /// needs to know how wide the synth is asks this one function --
    /// `apply_params_to_voices` deactivates the slots past it, `select_voice`
    /// and `any_active` only look inside it, and `render_range` hands it to
    /// `voice_pan`, which already returns centre at one. So mono mode centres
    /// the pan and retires the other fifteen voices without a second rule
    /// being written anywhere.
    fn voice_limit(&self) -> usize {
        if self.params.mono_mode {
            return 1;
        }
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
        // Every slot is busy: take one already releasing before one still
        // held, and the oldest of either (MOO-110). A held pad note outlives
        // the release tails around it.
        voices
            .iter()
            .enumerate()
            .min_by_key(|(_, voice)| (!voice.env.is_releasing(), voice.age))
            .map(|(index, _)| index)
            .unwrap_or(0)
    }

    fn note_on(&mut self, event_id: u64, note: u8, velocity: u8) {
        if self.params.mono_mode {
            self.mono_note_on(event_id, note, velocity);
            return;
        }
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

    /// One voice, a held-note stack, and the fallback rule.
    ///
    /// Deliberately a separate path rather than a flag threaded through
    /// `note_on`: the poly path is the calibrated one and it should stay
    /// legible. The rules are the ML-M1's, settled in
    /// `mono-synth-v2/03-the-held-note-stack.md`, and they are not
    /// re-litigated here -- above all that **a fallback is a pitch change and
    /// never a retrigger**, which is what makes a trill work.
    fn mono_note_on(&mut self, event_id: u64, note: u8, velocity: u8) {
        let was_overlapping = !self.held.is_empty();
        let was_any_active = self.any_active();
        self.held.push(HeldNote {
            event_id,
            note,
            velocity,
        });

        // Under `Low` or `High` a note can be pressed and still lose to
        // something already down. Then nothing happens at all: it is on the
        // stack, and releasing the winner will fall back to it.
        let Some(winner) = self.held.winner(self.params.note_priority) else {
            return;
        };
        if winner.event_id != event_id {
            return;
        }

        let velocity_amp = f32::from(velocity) / 127.0;
        let retrigger = !was_overlapping || self.params.env_trigger == EnvTrigger::Retrig;
        // Overlapping notes glide; a note landing on a release tail jumps.
        // That is the ML-M1's `GlideMode::Legato`, which is its default, and
        // it is the only glide rule this device has: the scope boundary in
        // `poly-v1-mono-mode/00-status.md` keeps ML-M1 identity out of here,
        // and a second glide mode is identity rather than competence.
        let glide = was_overlapping && self.params.glide > MIN_GLIDE_S;
        let params = self.params;
        let voice = &mut self.voices[0];
        let was_sounding = voice.active;

        voice.event_id = event_id;
        voice.note = note;
        voice.target_freq = note_to_freq(note);
        voice.active = true;
        if !was_sounding {
            // Fresh start: no glide from silence, clean filter and phases.
            voice.current_freq = voice.target_freq;
            voice.filter.reset();
            for osc in &mut voice.oscs {
                osc.reset();
            }
            voice.snap_to(&params, velocity_amp);
        } else if !glide {
            voice.current_freq = voice.target_freq;
        }
        voice.velocity_amp.set_target(velocity_amp);
        if retrigger || !was_sounding {
            voice.env.note_on();
        }

        if self.params.lfo.retrigger && !was_any_active {
            self.lfo.retrigger();
        }
    }

    /// Move the sounding mono voice to a different note without touching its
    /// envelope. The whole of what a fallback is.
    fn mono_retarget(&mut self, winner: HeldNote) {
        let glide = self.params.glide > MIN_GLIDE_S;
        let voice = &mut self.voices[0];
        voice.event_id = winner.event_id;
        voice.note = winner.note;
        voice.target_freq = note_to_freq(winner.note);
        if !glide {
            voice.current_freq = voice.target_freq;
        }
        // While the voice is still sounding the new velocity has to slide in:
        // stepping the gain mid-note is as audible as stepping the envelope.
        voice
            .velocity_amp
            .set_target(f32::from(winner.velocity) / 127.0);
    }

    fn note_off(&mut self, event_id: u64) {
        if self.params.mono_mode {
            // A `NoteOff` for something not held is stale by definition.
            // Bailing here is what keeps it from releasing a newer note.
            if !self.held.remove(event_id) || !self.voices[0].active {
                return;
            }
            match self.held.winner(self.params.note_priority) {
                Some(winner) => {
                    if winner.event_id != self.voices[0].event_id {
                        self.mono_retarget(winner);
                    }
                }
                None => self.voices[0].env.release(),
            }
            return;
        }
        for voice in self
            .voices
            .iter_mut()
            .filter(|voice| voice.active && voice.event_id == event_id)
        {
            voice.env.release();
        }
    }

    /// Transport stop and choke. The stack has to go with the voices: a held
    /// entry left behind would resurrect the voice on the next `NoteOff`
    /// fallback, long after the transport said stop.
    fn release_all(&mut self) {
        self.held.clear();
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
            let semis = osc.semitones.clamp(OSC_SEMITONE_RANGE.0, OSC_SEMITONE_RANGE.1)
                + osc.cents.clamp(OSC_CENT_RANGE.0, OSC_CENT_RANGE.1) / 100.0;
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
        let frames = end.saturating_sub(start);
        let mut cutoff = [0.0_f32; MAX_POLY_VOICES as usize];
        let mut base_hz = [0.0_f32; MAX_POLY_VOICES as usize];
        for (voice_index, voice) in voices.iter_mut().enumerate() {
            for (smoothed, osc) in voice.osc_level.iter_mut().zip(params.osc.iter()) {
                smoothed.set_target(osc.level.clamp(0.0, 1.0));
            }
            voice
                .cutoff
                .set_target(params.filter_cutoff.clamp(0.0, 1.0));
            voice.drive.set_target(params.drive.clamp(0.0, 1.0));
            // `hz_from_normalized`'s `powf` depends only on the smoothed
            // knob position, which has settled to a constant for most of a
            // note's life; resolve it once per voice per range instead of
            // every sample per voice. `advance_by` leaves `voice.cutoff`
            // exactly where `frames` calls to `advance()` would have.
            cutoff[voice_index] = voice.cutoff.advance_by(frames);
            base_hz[voice_index] = hz_from_normalized(cutoff[voice_index], max_hz);
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

                let drive = voice.drive.advance();
                let filtered = if cutoff[voice_index] >= 0.999
                    && env_amount.abs() <= f32::EPSILON
                    && resonance <= f32::EPSILON
                    && to_filter == 0.0
                {
                    mix
                } else {
                    let octaves = voice.env.level() * env_amount * 6.0 + lfo_value * to_filter;
                    let cutoff_hz = (base_hz[voice_index] * octaves.exp2()).clamp(20.0, max_hz);
                    voice
                        .filter
                        .next_sample_lp_hp(mix, cutoff_hz, resonance, sr)
                        .0
                };

                let sample = apply_drive(filtered, drive) * voice.env.level() * velocity * tremolo
                    * VOICE_OUTPUT_REFERENCE;
                let pan = voice_pan(voice_index, polyphony, spread);
                let (gain_l, gain_r) = pan_gains(pan);
                bus.l[i] += sample * gain_l;
                bus.r[i] += sample * gain_r;
            }
        }
    }
}

impl AudioNode for PolySynth {
    /// A generator's rest is voice bookkeeping it already does: a voice that
    /// has finished its release is marked inactive and its render is skipped
    /// outright, so with no voice active nothing is written into the bus. The LFO
    /// free-runs across the gap but only ever scales or detunes a voice, so
    /// it has nothing of its own to contribute.
    fn is_at_rest(&self) -> bool {
        !self.voices.iter().any(|voice| voice.active)
    }

    fn tail_frames(&self) -> u32 {
        0
    }

    /// The LFO advances a sample at a time inside the render loop whether or
    /// not a voice is sounding, so this mirrors that rather than taking one
    /// stride over the block: a sample-and-hold shape resolves per cycle, and
    /// the two do not land in the same place.
    ///
    /// `Lfo::skip` is that same per-sample walk with the shape evaluation
    /// left out, which is the only part of `next_sample` a skipped block was
    /// throwing away — and for a sine it was a transcendental a sample, on a
    /// block that renders nothing.
    fn skip_block(&mut self, ctx: &ProcessContext) {
        self.lfo
            .skip(ctx.frames, self.params.lfo.rate_hz, ctx.sample_rate);
    }

    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        let frames = ctx.frames.min(bus.capacity());

        if self.was_playing && !ctx.playing {
            self.release_all();
        }
        self.was_playing = ctx.playing;

        let mut pos = 0usize;
        for ev in events_in.iter() {
            let off = (ev.offset as usize).min(frames).max(pos);
            self.render_range(bus, pos, off);
            match ev.event {
                Event::NoteOn { id, note, velocity } => self.note_on(id, note, velocity),
                Event::NoteOff { id, .. } => self.note_off(id),
                Event::Choke => self.release_all(),
                Event::ParamValue { id, value } => self.apply_param(id, value),
                Event::SourceRouteAmount { .. }
                | Event::Buffer(_)
                | Event::BufferRelease
                | Event::BufferScrub { .. } => {}
            }
            pos = off;
        }
        self.render_range(bus, pos, frames);
    }
}

/// Neither extra reaches this device: it reads no auxiliary input and
/// publishes no control outlets, so being a channel's source is exactly
/// being an `AudioNode`. The forward is what lets the strip stop caring
/// which of the eight it is holding.
impl SourceNode for PolySynth {
    fn process_source(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _source: Option<&StereoBus>,
        _ports: &mut AudioTaps<'_>,
    ) {
        self.process(ctx, bus, events_in, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::TimedEvent;
    use mooloop_core::NotePriority;

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

    fn note_off(offset: u32, id: u64, note: u8) -> TimedEvent {
        TimedEvent {
            offset,
            event: Event::NoteOff { id, note },
        }
    }

    const SUSTAIN: f32 = 0.4;

    /// A mono-mode synth that settles at a partial sustain, which is what
    /// makes a retrigger observable at all.
    ///
    /// `Adsr::note_on` deliberately does not reset the level -- it attacks
    /// from wherever the envelope already is, so a retrigger over a sounding
    /// voice does not click. So a restart is not a dip to zero; it is a
    /// *climb back to the peak* from sustain, and these tests measure that
    /// climb. The same argument, and the same numbers, as the ML-M1's.
    fn mono(edit: impl FnOnce(&mut PolySynthParams)) -> PolySynth {
        let mut params = PolySynthParams {
            mono_mode: true,
            attack: 0.005,
            decay: 0.01,
            sustain: SUSTAIN,
            release: 0.5,
            ..Default::default()
        };
        edit(&mut params);
        PolySynth::new(params, 48_000)
    }

    /// Feed the events one sample at a time and report the highest level the
    /// envelope reaches after `from`. Above sustain means it restarted.
    fn envelope_peak(synth: &mut PolySynth, events: &EventList, frames: usize, from: usize) -> f32 {
        let mut bus = StereoBus::with_capacity(frames);
        let mut peak = 0.0_f32;
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
            synth.process(&ctx(1, 48_000), &mut bus, &slice, None);
            if offset >= from {
                peak = peak.max(synth.voices[0].env.level());
            }
        }
        peak
    }

    /// **The test that matters most**, and the reason the three new fields
    /// default the way they do: a project saved before mono mode existed must
    /// render exactly as it did.
    ///
    /// Bit-identical, not close. The two mode fields are set to their *other*
    /// values here, so what is being asserted is not that the defaults happen
    /// to be inert but that nothing outside `mono_mode` can reach the poly
    /// path at all.
    #[test]
    fn nothing_the_mono_fields_say_matters_while_mono_mode_is_off() {
        let sr = 48_000;
        let render = |params: PolySynthParams| {
            let mut synth = make_synth(sr, params);
            let mut bus = StereoBus::with_capacity(8_192);
            let mut events = EventList::empty();
            events.push(note_on(0, 1, 60));
            events.push(note_on(64, 2, 64));
            events.push(note_on(128, 3, 67));
            events.push(note_off(2_000, 2, 64));
            synth.process(&ctx(8_192, sr), &mut bus, &events, None);
            (bus.l[..8_192].to_vec(), bus.r[..8_192].to_vec())
        };

        let (left, right) = render(PolySynthParams::default());
        let (other_left, other_right) = render(PolySynthParams {
            env_trigger: EnvTrigger::Legato,
            note_priority: NotePriority::High,
            ..Default::default()
        });
        assert_eq!(left, other_left, "the left channel moved");
        assert_eq!(right, other_right, "the right channel moved");
        assert!(
            left.iter().any(|s| s.abs() > 0.01),
            "the comparison ran on silence"
        );
    }

    /// Mono mode is one voice, and it is centred whatever Spread says --
    /// because it goes through `voice_limit`, which `voice_pan` already
    /// answers centre for.
    #[test]
    fn mono_mode_plays_one_centred_voice() {
        let sr = 48_000;
        let mut synth = make_synth(
            sr,
            PolySynthParams {
                mono_mode: true,
                attack: 0.0001,
                sustain: 1.0,
                polyphony: 8,
                spread: 1.0,
                ..Default::default()
            },
        );
        let mut bus = StereoBus::with_capacity(4_096);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        events.push(note_on(0, 2, 64));
        events.push(note_on(0, 3, 67));
        synth.process(&ctx(4_096, sr), &mut bus, &events, None);

        assert_eq!(synth.voices.iter().filter(|v| v.active).count(), 1);
        assert_eq!(bus.l[..4_096], bus.r[..4_096], "a mono voice was panned");
    }

    #[test]
    fn legato_changes_pitch_without_restarting_the_envelope() {
        let mut synth = mono(|p| p.env_trigger = EnvTrigger::Legato);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        events.push(note_on(2_000, 2, 67));

        let peak = envelope_peak(&mut synth, &events, 4_000, 2_000);
        assert!(
            peak <= SUSTAIN + 1.0e-3,
            "legato restarted the envelope (peak {peak})"
        );
        assert!((synth.voices[0].target_freq - note_to_freq(67)).abs() < 0.01);
    }

    #[test]
    fn retrig_restarts_the_envelope_on_an_overlapping_note() {
        let mut synth = mono(|p| p.env_trigger = EnvTrigger::Retrig);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        events.push(note_on(2_000, 2, 67));

        let peak = envelope_peak(&mut synth, &events, 4_000, 2_000);
        assert!(
            peak > 0.95,
            "retrig did not restart the envelope (peak {peak})"
        );
    }

    /// **A fallback is a pitch change and never a retrigger**, in *either*
    /// trigger mode. That is what makes a trill work, and it is the rule the
    /// ML-M1 settled; this is the same rule on the other synth.
    ///
    /// Both directions, because they fail differently: releasing the newer
    /// note is the one a voice pool gets wrong by staying put, and releasing
    /// the older one is the one it gets wrong by falling back at all.
    #[test]
    fn releasing_either_note_of_two_falls_back_without_restarting() {
        for trigger in [EnvTrigger::Retrig, EnvTrigger::Legato] {
            for (released, expected) in [(2_u64, 60_u8), (1, 67)] {
                let mut synth = mono(|p| p.env_trigger = trigger);
                let mut events = EventList::empty();
                events.push(note_on(0, 1, 60));
                events.push(note_on(1_000, 2, 67));
                events.push(note_off(2_000, released, if released == 2 { 67 } else { 60 }));

                let peak = envelope_peak(&mut synth, &events, 4_000, 2_000);
                assert!(
                    peak <= SUSTAIN + 1.0e-3,
                    "{trigger:?} restarted the envelope on fallback (peak {peak})"
                );
                assert!(
                    (synth.voices[0].target_freq - note_to_freq(expected)).abs() < 0.01,
                    "{trigger:?} releasing {released} left the voice on the wrong note"
                );
            }
        }
    }

    /// The same three-note gesture under `Last`, `Low` and `High` picks three
    /// different winners. A pool of one voice can only ever answer `Last`.
    #[test]
    fn note_priority_picks_three_different_winners() {
        for (priority, expected) in [
            (NotePriority::Last, 62_u8),
            (NotePriority::Low, 55),
            (NotePriority::High, 67),
        ] {
            let mut synth = mono(|p| p.note_priority = priority);
            let mut events = EventList::empty();
            events.push(note_on(0, 1, 67));
            events.push(note_on(500, 2, 55));
            events.push(note_on(1_000, 3, 62));

            envelope_peak(&mut synth, &events, 2_000, 2_000);
            assert!(
                (synth.voices[0].target_freq - note_to_freq(expected)).abs() < 0.01,
                "{priority:?} did not settle on note {expected}"
            );
        }
    }

    /// A held entry that survived a stop would resurrect the voice on the
    /// next `NoteOff` fallback, long after the transport said stop -- and,
    /// worse, steal the next note's pitch.
    #[test]
    fn a_transport_stop_clears_the_held_notes() {
        let sr = 48_000;
        let mut synth = mono(|_| {});
        let mut bus = StereoBus::with_capacity(1_024);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        events.push(note_on(100, 2, 67));
        synth.process(&ctx(1_024, sr), &mut bus, &events, None);
        assert!(!synth.held.is_empty());

        let stopped = ProcessContext {
            playing: false,
            ..ctx(1_024, sr)
        };
        synth.process(&stopped, &mut bus, &EventList::empty(), None);
        assert!(synth.held.is_empty(), "a held note survived the stop");
    }

    /// Leaving mono mode and coming back must not find the stack as it was
    /// left: those notes are not under anybody's fingers any more.
    #[test]
    fn leaving_mono_mode_drops_what_was_held() {
        let sr = 48_000;
        let mut synth = mono(|_| {});
        let mut bus = StereoBus::with_capacity(512);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        synth.process(&ctx(512, sr), &mut bus, &events, None);
        assert!(!synth.held.is_empty());

        let mut params = synth.params;
        params.mono_mode = false;
        synth.set_params(params);
        assert!(synth.held.is_empty());
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

    /// With every voice busy, a new note takes one that is releasing before
    /// one still held, even a younger one (MOO-110).
    #[test]
    fn a_full_pool_steals_a_releasing_voice_before_a_held_one() {
        let mut synth = make_synth(
            48_000,
            PolySynthParams {
                polyphony: 2,
                release: 1.0,
                ..Default::default()
            },
        );
        synth.note_on(1, 60, 100);
        synth.note_on(2, 64, 100);
        synth.note_off(2);
        synth.note_on(3, 67, 100);
        assert!(
            synth.voices.iter().any(|v| v.active && v.event_id == 1),
            "the held note was stolen"
        );
        assert!(synth.voices.iter().any(|v| v.event_id == 3));
    }

    /// Lowering Voices, or switching Mono on, fades the voices it retires
    /// rather than cutting them (MOO-110).
    #[test]
    fn retired_voices_fade_rather_than_stop() {
        let sr = 48_000;
        let mut synth = make_synth(
            sr,
            PolySynthParams {
                polyphony: 4,
                attack: 0.0001,
                sustain: 1.0,
                ..Default::default()
            },
        );
        let mut bus = StereoBus::with_capacity(512);
        let mut events = EventList::empty();
        for (index, note) in [60u8, 64, 67].into_iter().enumerate() {
            events.push(note_on(0, index as u64 + 1, note));
        }
        synth.process(&ctx(512, sr), &mut bus, &events, None);
        let mut params = synth.params;
        params.mono_mode = true;
        synth.set_params(params);
        assert!(synth.voices[1..3]
            .iter()
            .all(|v| v.active && v.env.is_releasing()));
        let mut bus = StereoBus::with_capacity(1_024);
        synth.process(&ctx(1_024, sr), &mut bus, &EventList::empty(), None);
        assert!(synth.voices[1..].iter().all(|v| !v.active), "the fade never ended");
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

        let max_step = crate::testkit::max_step(&bus.l[..4096]);
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

    /// A note played with the transport already stopped -- a MIDI keyboard,
    /// an audition -- is held until its own note-off. Releasing on every
    /// stopped block cut it to a blip.
    #[test]
    fn a_note_played_while_stopped_is_held() {
        let sr = 48_000;
        let mut synth = make_synth(sr, PolySynthParams::default());
        let mut bus = StereoBus::with_capacity(64);
        let mut stopped = ctx(64, sr);
        stopped.playing = false;
        synth.process(&stopped, &mut bus, &EventList::empty(), None);

        let mut events = EventList::empty();
        events.push(note_on(0, 0, 60));
        synth.process(&stopped, &mut bus, &events, None);
        for _ in 0..8 {
            synth.process(&stopped, &mut bus, &EventList::empty(), None);
        }
        assert!(
            synth
                .voices
                .iter()
                .any(|v| v.active && !v.env.is_releasing()),
            "the stopped transport let go of the key"
        );
    }
}
