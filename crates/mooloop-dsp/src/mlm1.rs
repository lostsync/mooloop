//! The ML-M1: one voice, two envelopes, and a filter that tracks the
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
//! - three selectable filter characters — a four-pole [`Ladder`], a
//!   three-pole [`Acid`], and the linear [`Svf`] — with saturation *ahead* of
//!   the filter rather than after it, so the oscillator mixer is a tone
//!   control: pushing more level into the filter changes the character and
//!   not merely the gain.
//!
//! - velocity Accent pushes the filter envelope and pre-filter drive while
//!   preserving velocity's amplitude role.

use crate::bus::StereoBus;
use crate::env::Adsr;
use crate::event::{Event, EventList};
use crate::filter::{soft_ceiling, Acid, Ladder, PreDrive, Svf};
use crate::heldnotes::{HeldNote, HeldNotes};
use crate::node::{AudioNode, ProcessContext};
use crate::osc::Osc;
use crate::scale::hz_from_normalized;
use crate::smooth::Smoothed;
use crate::synth_voice::{note_to_freq, MIN_GLIDE_S, PARAM_SMOOTH_S, STOP_RELEASE_S};
use mooloop_core::{EnvTrigger, FilterModel, GlideMode, MlM1Params};

/// The voice's absolute output reference, set so one oscillator at its 0 dB
/// top (which the default patch runs at) peaks within a dB of
/// `mooloop_core::gain::REFERENCE_PEAK_DBFS` (-12 dBFS) at the master.
const VOICE_OUTPUT_REFERENCE: f32 = 0.36;

/// Middle C (MIDI 60). Keytracking is referenced here, so a patch voiced
/// around the middle of the keyboard keeps its cutoff where it was set.
const KEYTRACK_REFERENCE_HZ: f32 = 261.625_58;

/// How much a full-accent, full-velocity note multiplies the filter envelope
/// amount. A third of the knob's six octaves is two more octaves of sweep,
/// which is the difference between a note and an accented note rather than
/// between two instruments.
const ACCENT_ENV_SCALE: f32 = 1.0 / 3.0;

/// How much a full-accent, full-velocity note adds to the drive knob. Chosen
/// to be clearly audible while leaving headroom at the top of the knob, so an
/// accented patch never asks for the channel fader back.
const ACCENT_DRIVE_PUSH: f32 = 0.35;

/// The three filter characters, and the switch between them.
///
/// Each model keeps its own state rather than sharing one array, because they
/// are different filters with different orders and not one filter with a mode
/// flag. Holding all three costs eleven floats, which is nothing next to
/// making the switch a special case.
struct VoiceFilter {
    ladder: Ladder,
    acid: Acid,
    clean: Svf,
}

impl VoiceFilter {
    fn new() -> Self {
        Self {
            ladder: Ladder::new(),
            acid: Acid::new(),
            clean: Svf::new(),
        }
    }

    fn reset(&mut self) {
        self.ladder.reset();
        self.acid.reset();
        self.clean.reset();
    }

    /// Dispatched per sample rather than per block. The model is constant
    /// across a block, so the branch predicts perfectly, and hoisting it would
    /// mean three copies of the render loop.
    fn next_sample(
        &mut self,
        model: FilterModel,
        input: f32,
        cutoff_hz: f32,
        resonance: f32,
        sample_rate: u32,
    ) -> f32 {
        match model {
            FilterModel::Ladder => {
                let k = Ladder::feedback_at(cutoff_hz, resonance, sample_rate);
                self.ladder.next_sample(input, cutoff_hz, resonance, sample_rate)
                    * makeup(LADDER_MAKEUP_DB, LADDER_MAKEUP_KNEE, 0.0, k)
            }
            FilterModel::Acid => {
                let k = Acid::feedback_at(cutoff_hz, resonance, sample_rate);
                self.acid.next_sample(input, cutoff_hz, resonance, sample_rate)
                    * makeup(ACID_MAKEUP_DB, ACID_MAKEUP_KNEE, ACID_MAKEUP_STATIC_DB, k)
            }
            FilterModel::Clean => {
                let k = clean_feedback_at(cutoff_hz, resonance, sample_rate);
                self.clean.next_sample(input, cutoff_hz, resonance, sample_rate)
                    * makeup(CLEAN_MAKEUP_DB, CLEAN_MAKEUP_KNEE, 0.0, k)
            }
        }
    }
}

/// Resonance makeup gain, as a linear multiplier.
///
/// Resonance changes a filter's output level, and by different amounts in
/// different directions per model: the two nonlinear models lose level, because
/// their stages only ever integrate a bounded shaper output and so compress as
/// the feedback path drives them harder, while the linear [`Svf`] *gains* level
/// from its resonant peak. Measured across a cutoff/resonance grid, that put
/// the three models up to 10.4 dB apart at identical settings, which is what
/// the Model switch sounded like before this existed.
///
/// The loss is close to logarithmic in the feedback gain `k`, which is why the
/// curve is shaped on `k` rather than on the Resonance knob: `k` already folds
/// in the cutoff tracking, so one term covers both axes.
///
/// This lives here rather than inside the filters because "the three models
/// should be equally loud" is the ML-M1's policy, not a property of a ladder.
/// A [`Ladder`] used somewhere else should not arrive pre-trimmed to match two
/// filters it has never heard of, and [`Svf`] is shared with the v1 synths and
/// the filter effect, where changing its level would be a regression.
fn makeup(depth_db: f32, knee: f32, static_db: f32, k: f32) -> f32 {
    let db = depth_db * (1.0 + knee * k).ln() + static_db;
    10.0_f32.powf(db / 20.0)
}

/// [`Svf`] is linear and has no feedback gain to publish, so the Clean model
/// gets the same shape driven by an equivalent term. The `1.5` matches the
/// tracking both nonlinear models use.
fn clean_feedback_at(cutoff_hz: f32, resonance: f32, sample_rate: u32) -> f32 {
    let sr = sample_rate as f32;
    let cutoff = cutoff_hz.clamp(20.0, sr * 0.45);
    let g = 1.0 - (-core::f32::consts::TAU * cutoff / sr).exp();
    resonance.clamp(0.0, 1.0) * (1.0 + 1.5 * g)
}

// Fitted by least squares against the measured grid, per model, so that each
// model holds its own zero-resonance level as resonance rises. Worst-case
// spread between the three models falls from 10.8 dB to 2.4 dB; the remainder
// is mostly the honest slope difference between a three-pole and a four-pole
// filter, which is character rather than error.
//
// These are a measured first pass and are meant to be tuned by ear.
const LADDER_MAKEUP_DB: f32 = 14.30;
const LADDER_MAKEUP_KNEE: f32 = 0.04;
const ACID_MAKEUP_DB: f32 = 1.80;
const ACID_MAKEUP_KNEE: f32 = 1.24;
/// Acid is the one model that also sits low at *zero* resonance, because its
/// corner is calibrated well below the other two -- see
/// [`crate::filter`]'s `ACID_POLE_COMPENSATION`. Until that is re-derived,
/// this closes the standing offset without touching its voice. The fit wanted
/// no static term at all for the other two.
const ACID_MAKEUP_STATIC_DB: f32 = 2.10;
/// Negative: the linear filter is the one that gets *louder* with resonance.
const CLEAN_MAKEUP_DB: f32 = -0.90;
const CLEAN_MAKEUP_KNEE: f32 = 7.88;

struct MlM1Voice {
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
    /// Saturation runs here, between the mix and the filter. That placement is
    /// the whole difference between a filter with a drive knob and a filter
    /// you play by pushing level into it.
    pre_drive: PreDrive,
    filter: VoiceFilter,
    /// Velocity gain, smoothed so that a retrigger at a different velocity
    /// slides rather than steps.
    velocity_amp: Smoothed,
    osc_level: [Smoothed; 3],
    cutoff: Smoothed,
    drive: Smoothed,
}

impl MlM1Voice {
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
            pre_drive: PreDrive::new(),
            filter: VoiceFilter::new(),
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
    fn configure_envelopes(&mut self, params: &MlM1Params) {
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
    fn snap_to(&mut self, params: &MlM1Params, velocity_amp: f32) {
        self.velocity_amp.reset_to(velocity_amp);
        for (smoothed, osc) in self.osc_level.iter_mut().zip(params.osc.iter()) {
            smoothed.reset_to(osc.level.clamp(0.0, 1.0));
        }
        self.cutoff.reset_to(params.filter_cutoff.clamp(0.0, 1.0));
        self.drive.reset_to(params.drive.clamp(0.0, 1.0));
    }
}

/// The ML-M1 node.
pub struct MlM1 {
    params: MlM1Params,
    sample_rate: u32,
    voice: MlM1Voice,
    /// Every note currently down, not just the one sounding. This is what
    /// makes trills, fallback, and note priority possible at all.
    held: HeldNotes,
}

impl MlM1 {
    pub fn new(params: MlM1Params, sample_rate: u32) -> Self {
        let mut voice = MlM1Voice::new(sample_rate);
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
    pub fn set_params(&mut self, params: MlM1Params) {
        self.params = params;
        self.voice.configure_envelopes(&params);
    }

    /// Apply one descriptor-addressed parameter, leaving the rest alone.
    ///
    /// Routed through `set_params` so a control-rate change gets exactly the
    /// same clamping and voice reconfiguration a whole-struct update does.
    fn apply_param(&mut self, id: u32, value: f32) {
        let mut params = mooloop_core::GeneratorParams::MlM1(self.params);
        if params.set(id, value).is_none() {
            return;
        }
        if let mooloop_core::GeneratorParams::MlM1(params) = params {
            self.set_params(params);
        }
    }

    /// Immediately invalidate the active voice and return every oscillator
    /// and filter to its initial state.
    pub fn reset(&mut self) {
        self.voice = MlM1Voice::new(self.sample_rate);
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
        let accent = params.accent.clamp(0.0, 1.0);
        let resonance = params.filter_resonance.clamp(0.0, 1.0);
        let keytrack = params.filter_keytrack.clamp(0.0, 1.0);
        let model = params.filter_model;
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
            // Accent rides the same smoothed velocity the VCA uses. That is
            // not a shortcut: it gives per-note capture, the priority
            // fallback's winning-note velocity, and the legato slide for free,
            // because `velocity_amp` already carries all three. At `accent`
            // zero this is exactly zero and everything below is untouched.
            let accent_depth = accent * velocity;

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

            let cutoff = voice.cutoff.advance();
            // Added to the *smoothed* drive rather than applied as a stage of
            // its own, so it inherits click-safety instead of needing its own.
            let drive =
                (voice.drive.advance() + accent_depth * ACCENT_DRIVE_PUSH).clamp(0.0, 1.0);

            // Saturate, then filter. The pre-drive stage keeps its own level
            // estimate, so it has to see every sample even when the filter is
            // bypassed below — otherwise the estimate would be stale the
            // moment a knob moved back into the filtered path.
            let driven = voice.pre_drive.next_sample(mix, drive, sr);

            // Bypassed entirely when the filter is fully open and nothing is
            // moving it — keytrack has to be in that test, or a tracking patch
            // with the cutoff knob at the top would skip the filter it is
            // supposed to be playing.
            let filtered = if cutoff >= 0.999
                && env_amount.abs() <= f32::EPSILON
                && resonance <= f32::EPSILON
                && keytrack <= f32::EPSILON
            {
                driven
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
                // Accent scales the knob rather than adding to it, which is
                // what keeps the bypass test above honest: an effective
                // amount of zero stays zero however hard the note was hit.
                // It also preserves a negative amount's direction — accent
                // deepens whatever the patch already does.
                let accented_amount = env_amount * (1.0 + accent_depth * ACCENT_ENV_SCALE);
                let octaves = voice.filter_env.level() * accented_amount * 6.0 + keytrack_oct;
                let cutoff_hz = (base_hz * octaves.exp2()).clamp(20.0, max_hz);
                voice
                    .filter
                    .next_sample(model, driven, cutoff_hz, resonance, sr)
            };

            // The ceiling is transparent for the two nonlinear models, which
            // are bounded by construction; it is there for the linear one,
            // which is not.
            let sample = soft_ceiling(filtered)
                * voice.amp_env.level()
                * velocity
                * VOICE_OUTPUT_REFERENCE;
            bus.l[i] += sample;
            bus.r[i] += sample;
        }
    }
}

impl AudioNode for MlM1 {
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
    fn saw_patch() -> MlM1Params {
        let mut params = MlM1Params::default();
        params.osc[0] = OscParams {
            wave: OscWave::Saw,
            level: 1.0,
            ..OscParams::default()
        };
        params
    }

    fn note_on_at(offset: u32, id: u64, note: u8, velocity: u8) -> TimedEvent {
        TimedEvent {
            offset,
            event: Event::NoteOn {
                id,
                note,
                velocity,
            },
        }
    }

    fn render(params: MlM1Params, note: u8, frames: usize) -> Vec<f32> {
        render_at(params, note, 127, frames)
    }

    fn render_at(params: MlM1Params, note: u8, velocity: u8, frames: usize) -> Vec<f32> {
        let mut synth = MlM1::new(params, SR);
        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        events.push(note_on_at(0, 1, note, velocity));
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
        let mut synth = MlM1::new(MlM1Params::default(), SR);
        let mut bus = StereoBus::with_capacity(256);
        synth.process(&ctx(256), &mut bus, &EventList::empty(), None);
        assert_eq!(bus.peak(256), (0.0, 0.0));
    }

    #[test]
    fn note_on_at_offset_is_sample_accurate() {
        let frames = 512;
        let k = 200usize;
        let mut synth = MlM1::new(saw_patch(), SR);
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
        let mut synth = MlM1::new(params, SR);
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

        let mut synth = MlM1::new(params, SR);
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
        let mut synth = MlM1::new(saw_patch(), SR);
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
    fn sustained(params: impl FnOnce(&mut MlM1Params)) -> MlM1 {
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
        MlM1::new(p, SR)
    }

    const SUSTAIN: f32 = 0.4;

    /// Feed the events one sample at a time and report the highest level each
    /// envelope reaches after `from`. Above sustain means it restarted.
    fn envelope_peaks(
        synth: &mut MlM1,
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

    /// A sine so that saturation is unmistakable in the spectrum — a saw
    /// already carries every harmonic, so driving one compresses its
    /// structure rather than adding to it, which makes it a poor probe.
    fn sine_patch() -> MlM1Params {
        let mut params = saw_patch();
        params.osc[0] = OscParams {
            wave: OscWave::Sine,
            level: 1.0,
            ..OscParams::default()
        };
        params.sustain = 1.0;
        params
    }

    /// Drive's contract: harmonic content moves, loudness does not.
    #[test]
    fn drive_adds_harmonics_without_adding_level() {
        let mut params = sine_patch();
        let frames = SR as usize / 4;
        let window = SR as usize / 10..frames;

        let clean = render(params, 45, frames);
        params.drive = 1.0;
        let driven = render(params, 45, frames);

        let level_db =
            20.0 * (rms(&driven[window.clone()]) / rms(&clean[window.clone()])).log10();
        assert!(
            level_db.abs() < 2.0,
            "drive moved the level {level_db:.1} dB"
        );
        assert!(
            brightness(&driven[window.clone()]) > brightness(&clean[window]) * 1.5,
            "drive did not add harmonics"
        );
    }

    /// The placement test, and the point of the whole step: the saturation is
    /// *ahead* of the filter, so closing the filter removes the harmonics
    /// drive just added. Were it after the filter, the cutoff knob could not
    /// touch them and drive would brighten a closed patch exactly as much as
    /// an open one.
    #[test]
    fn the_drive_stage_sits_ahead_of_the_filter() {
        let frames = SR as usize / 4;
        let window = SR as usize / 10..frames;

        let added_brightness = |cutoff: f32| {
            let mut params = sine_patch();
            params.filter_cutoff = cutoff;
            let clean = render(params, 45, frames);
            params.drive = 1.0;
            let driven = render(params, 45, frames);
            brightness(&driven[window.clone()]) / brightness(&clean[window.clone()])
        };

        let open = added_brightness(1.0);
        let closed = added_brightness(0.35);
        assert!(
            closed < open * 0.7,
            "the filter barely touched drive's harmonics (open {open:.2}, closed {closed:.2}), \
             which is what post-filter drive would look like"
        );
    }

    /// The other half, and the thing post-filter drive cannot do: with drive
    /// up, an oscillator's level knob changes the timbre and not merely the
    /// gain, because it changes how hard the mix pushes into the shaper.
    #[test]
    fn an_oscillator_level_is_a_tone_control_when_drive_is_up() {
        let frames = SR as usize / 4;
        let window = SR as usize / 10..frames;

        let timbre_at = |level: f32| {
            let mut params = sine_patch();
            params.drive = 1.0;
            params.osc[0].level = level;
            brightness(&render(params, 45, frames)[window.clone()])
        };

        // Compared as brightness, which is scale-invariant, so this is a shape
        // difference and not the level difference that comes with it.
        let quiet = timbre_at(0.3);
        let loud = timbre_at(1.0);
        assert!(
            loud > quiet * 1.2,
            "oscillator level did not change the timbre: {quiet:.3} -> {loud:.3}"
        );
    }

    #[test]
    fn resonant_filter_and_drive_stay_bounded() {
        for model in [FilterModel::Ladder, FilterModel::Acid, FilterModel::Clean] {
            let mut params = saw_patch();
            params.filter_model = model;
            params.osc[1].level = 1.0;
            params.osc[2].level = 1.0;
            params.filter_cutoff = 0.2;
            params.filter_resonance = 1.0;
            params.filter_env_amount = 1.0;
            params.filter_decay = 0.05;
            params.filter_sustain = 0.0;
            params.filter_keytrack = 1.0;
            params.drive = 1.0;
            // Accent belongs in the worst case, not beside it: the bound has
            // to hold for a full-velocity note in a fully accented patch, or
            // Accent is a gain-staging trap rather than a control.
            params.accent = 1.0;

            let frames = SR as usize;
            let rendered = render(params, 36, frames);
            let peak = rendered.iter().fold(0.0_f32, |a, s| a.max(s.abs()));
            assert!(peak.is_finite(), "{model:?} went non-finite");
            assert!(peak <= 1.0, "{model:?} peaked at {peak}");
        }
    }

    /// Acid's whole use case is a sequenced line where the filter envelope is
    /// doing the musical work, so at the same envelope amount its sweep has to
    /// be the more pronounced one. If the two models swept alike, Acid would
    /// not be earning its place on the switch.
    #[test]
    fn the_acid_sweeps_harder_than_the_ladder_at_the_same_settings() {
        let sweep_depth = |model: FilterModel| {
            let mut params = saw_patch();
            params.filter_model = model;
            params.sustain = 1.0;
            params.filter_cutoff = 0.4;
            params.filter_resonance = 0.6;
            params.filter_env_amount = 0.5;
            params.filter_attack = 0.001;
            params.filter_decay = 0.1;
            params.filter_sustain = 0.0;

            let frames = (SR as f32 * 0.2) as usize;
            let rendered = render(params, 45, frames);
            let window = SR as usize / 200;
            let early = brightness(&rendered[window..window * 2]);
            let late = brightness(&rendered[frames - window * 4..frames]);
            early / late
        };

        let ladder = sweep_depth(FilterModel::Ladder);
        let acid = sweep_depth(FilterModel::Acid);
        assert!(
            acid > ladder * 1.1,
            "acid swept {acid:.2}x against the ladder's {ladder:.2}x"
        );
    }

    /// Largest sample-to-sample jump, the same measure the smoothing tests use.
    fn max_step(samples: &[f32]) -> f32 {
        samples
            .windows(2)
            .fold(0.0_f32, |worst, pair| worst.max((pair[1] - pair[0]).abs()))
    }

    /// Switching model on a sounding voice must not click. Each model keeps
    /// its own state, so the incoming one starts from wherever it last left
    /// off rather than from zero, and the parameter smoothing covers the rest.
    #[test]
    fn switching_filter_model_mid_note_does_not_step_the_output() {
        let mut params = saw_patch();
        params.sustain = 1.0;
        params.filter_cutoff = 0.4;
        params.filter_resonance = 0.5;

        let mut synth = MlM1::new(params, SR);
        let frames = 4096;
        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 45));
        synth.process(&ctx(frames), &mut bus, &events, None);
        let settled = max_step(&bus.l[frames / 2..frames]);

        for model in [FilterModel::Acid, FilterModel::Clean, FilterModel::Ladder] {
            let mut bus = StereoBus::with_capacity(frames);
            let mut events = EventList::empty();
            events.push(TimedEvent {
                offset: 0,
                event: Event::ParamValue {
                    id: mooloop_core::generator::SYNTH_PARAM_FILTER_MODEL,
                    value: model.to_index() as f32,
                },
            });
            synth.process(&ctx(frames), &mut bus, &events, None);
            let stepped = max_step(&bus.l[..frames]);
            assert!(
                stepped < settled.max(0.01) * 3.0,
                "switching to {model:?} stepped {stepped:.4} against a settled {settled:.4}"
            );
        }
    }

    /// Accent's migration promise. At zero the synth must be exactly what it
    /// was before the knob existed, which means velocity scales amplitude and
    /// touches nothing else — so two velocities render the *same waveform* at
    /// two gains, sample for sample.
    #[test]
    fn accent_at_zero_leaves_velocity_scaling_amplitude_and_nothing_else() {
        let mut params = saw_patch();
        params.accent = 0.0;
        params.filter_cutoff = 0.4;
        params.filter_resonance = 0.6;
        params.filter_env_amount = 0.8;
        params.drive = 0.6;
        params.sustain = 1.0;

        let frames = SR as usize / 4;
        let loud = render_at(params, 45, 127, frames);
        let soft = render_at(params, 45, 40, frames);
        let ratio = 40.0 / 127.0;

        let worst = loud
            .iter()
            .zip(soft.iter())
            .map(|(l, s)| (l * ratio - s).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            worst < 1.0e-6,
            "velocity changed more than the gain: worst sample differs by {worst:e}"
        );
    }

    /// And the other side of it: with Accent up, the same two velocities must
    /// differ in *shape*, not only in level. Brightness is scale-invariant, so
    /// it cannot be satisfied by the amplitude scaling that is there anyway.
    ///
    /// The filter half. Held at a sustained envelope level and a low base
    /// cutoff so the extra sweep lands well inside the audible range instead
    /// of against the Nyquist clamp, where two different amounts of "wide
    /// open" measure the same.
    #[test]
    fn accent_opens_the_filter_further_for_a_harder_note() {
        let frames = SR as usize / 4;
        let window = SR as usize / 20..frames;

        let shape_gap = |accent: f32| {
            let mut params = saw_patch();
            params.accent = accent;
            params.filter_cutoff = 0.05;
            params.filter_env_amount = 1.0;
            params.filter_sustain = 1.0;
            params.sustain = 1.0;
            let loud = render_at(params, 45, 127, frames);
            let soft = render_at(params, 45, 40, frames);
            brightness(&loud[window.clone()]) / brightness(&soft[window.clone()])
        };

        let flat = shape_gap(0.0);
        let accented = shape_gap(1.0);
        assert!(
            (flat - 1.0).abs() < 0.02,
            "velocity changed the timbre with Accent at zero: {flat:.3}"
        );
        assert!(
            accented > 1.15,
            "Accent did not open the filter harder for the harder note: \
             {flat:.3} -> {accented:.3}"
        );
    }

    /// The drive half, isolated: the filter is out of the picture entirely, so
    /// the only thing left that velocity can move is the saturation.
    #[test]
    fn accent_drives_harder_for_a_harder_note() {
        let frames = SR as usize / 4;
        let window = SR as usize / 20..frames;

        let shape_gap = |accent: f32| {
            let mut params = sine_patch();
            params.accent = accent;
            params.filter_env_amount = 0.0;
            params.drive = 0.3;
            let loud = render_at(params, 45, 127, frames);
            let soft = render_at(params, 45, 40, frames);
            brightness(&loud[window.clone()]) / brightness(&soft[window.clone()])
        };

        let flat = shape_gap(0.0);
        let accented = shape_gap(1.0);
        assert!(
            (flat - 1.0).abs() < 0.02,
            "velocity changed the timbre with Accent at zero: {flat:.3}"
        );
        assert!(
            accented > 1.1,
            "Accent did not push the drive harder for the harder note: \
             {flat:.3} -> {accented:.3}"
        );
    }

    /// Accent is captured per note, so falling back to a still-held note has
    /// to restore *that* note's accent, not keep the released note's. Both
    /// notes are the same pitch so nothing but velocity can move the timbre.
    #[test]
    fn the_priority_fallback_restores_the_winning_notes_accent() {
        let mut params = saw_patch();
        params.accent = 1.0;
        params.env_trigger = EnvTrigger::Legato;
        params.filter_cutoff = 0.25;
        params.filter_env_amount = 0.6;
        params.sustain = 1.0;
        params.filter_sustain = 1.0;
        params.attack = 0.001;
        params.filter_attack = 0.001;

        let frames = SR as usize * 3 / 4;
        let segment = SR as usize / 4;
        let mut synth = MlM1::new(params, SR);
        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        events.push(note_on_at(0, 1, 45, 127));
        events.push(note_on_at(segment as u32, 2, 45, 30));
        events.push(TimedEvent {
            offset: (segment * 2) as u32,
            event: Event::NoteOff { id: 2, note: 45 },
        });
        synth.process(&ctx(frames), &mut bus, &events, None);
        let rendered = &bus.l[..frames];

        // Skip the first tenth of each segment so the 5 ms velocity ramp and
        // the envelope attack are behind us.
        let settled = |n: usize| {
            let start = n * segment + segment / 10;
            brightness(&rendered[start..(n + 1) * segment])
        };
        let (accented, soft, restored) = (settled(0), settled(1), settled(2));
        assert!(
            soft < accented * 0.95,
            "the soft note did not take the accent down: {accented:.3} -> {soft:.3}"
        );
        assert!(
            (restored - accented).abs() < accented * 0.05,
            "the fallback did not restore the held note's accent: \
             {accented:.3} -> {soft:.3} -> {restored:.3}"
        );
    }

    /// Accent rides the smoothed velocity and is folded into the smoothed
    /// drive, so an accented note landing over a sounding one in `Legato`
    /// slides. Any step would be a click, and a click is a discontinuity far
    /// larger than the waveform's own sample-to-sample motion.
    #[test]
    fn an_accented_note_over_a_sounding_voice_does_not_step() {
        let mut params = sine_patch();
        params.accent = 1.0;
        params.env_trigger = EnvTrigger::Legato;
        params.filter_cutoff = 0.25;
        params.filter_env_amount = 0.6;
        params.filter_sustain = 1.0;
        params.drive = 0.5;

        let frames = SR as usize / 2;
        let landing = SR as usize / 4;
        let mut synth = MlM1::new(params, SR);
        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        events.push(note_on_at(0, 1, 45, 30));
        events.push(note_on_at(landing as u32, 2, 45, 127));
        synth.process(&ctx(frames), &mut bus, &events, None);
        let rendered = &bus.l[..frames];

        let biggest_step = |range: std::ops::Range<usize>| {
            rendered[range]
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0_f32, f32::max)
        };
        // The tail of the ramp is the loudest part of the note, so its own
        // motion is the fair yardstick for the ramp itself.
        let steady = biggest_step(frames - SR as usize / 20..frames);
        let across = biggest_step(landing - 8..landing + 8);
        assert!(
            across <= steady * 1.5,
            "the accent stepped rather than slid: {across:.4} against a steady {steady:.4}"
        );
    }

    // --- The factory bank -------------------------------------------------
    //
    // `docs/plans/mono-synth-v2/08-mono-factory-patches.md` asks for these
    // checks once the bank exists, on the grounds that they are cheapest to
    // catch here. They run over the shipped patches rather than over invented
    // ones deliberately: a bound that holds for a test patch and not for
    // Round Bass has told nobody anything.

    use mooloop_core::mlm1_factory;
    use mooloop_core::generator::SYNTH_PARAM_ACCENT;
    use mooloop_core::{
        SYNTH_PARAM_DRIVE, SYNTH_PARAM_FILTER_CUTOFF, SYNTH_PARAM_FILTER_RESONANCE,
    };

    /// A note in each patch's own register. A bass patch voiced at C1 says
    /// nothing about anything if it is only ever tested at C4, and the
    /// keytracking patches say the least of all.
    fn audition_notes(name: &str) -> &'static [u8] {
        match name {
            "Round Bass" | "Rubber Bass" | "Acid Line" => &[24, 36, 48],
            "Snap Pluck" | "Sequence Bleep" => &[48, 60, 72],
            _ => &[36, 60, 84],
        }
    }

    /// Every patch, at full velocity, across its register, stays inside the
    /// same bound the synthetic worst case uses. This is the practical form
    /// of the Accent gain-staging requirement: the patches that ship are the
    /// ones a user will actually drive.
    #[test]
    fn every_factory_patch_stays_within_the_peak_bound() {
        for patch in mlm1_factory::patches() {
            for &note in audition_notes(patch.name) {
                let frames = SR as usize;
                let rendered = render_at(patch.params, note, 127, frames);
                let peak = rendered.iter().fold(0.0_f32, |a, s| a.max(s.abs()));
                assert!(peak.is_finite(), "{} went non-finite at note {note}", patch.name);
                assert!(peak <= 1.0, "{} peaked at {peak} on note {note}", patch.name);
            }
        }
    }

    /// A patch that cannot be heard is not a patch. Cheap, but it is the
    /// check that catches a level or an envelope typo in the bank before any
    /// of the more specific tests below get a chance to be confusing.
    #[test]
    fn every_factory_patch_makes_a_sound() {
        for patch in mlm1_factory::patches() {
            let rendered = render_at(patch.params, 48, 100, (SR / 4) as usize);
            let level = rms(&rendered);
            assert!(
                level > 0.01,
                "{} rendered near-silence ({level:.5})",
                patch.name
            );
        }
    }

    /// Transport stop has to end every patch quickly, including the ones with
    /// a long release. `STOP_RELEASE_S` overrides the patch's own release for
    /// exactly this reason, and Porta Lead's quarter-second tail is what
    /// would expose it if that stopped being true.
    #[test]
    fn every_factory_patch_releases_promptly_on_a_transport_stop() {
        // Generous next to `STOP_RELEASE_S` (5 ms), because the envelope has
        // to reach silence and not merely start heading there. Still far
        // shorter than the longest release in the bank, which is the point.
        let allowed_frames = (SR as f32 * 0.05) as usize;
        for patch in mlm1_factory::patches() {
            let mut synth = MlM1::new(patch.params, SR);
            let mut bus = StereoBus::with_capacity(allowed_frames.max(512));
            let mut events = EventList::empty();
            events.push(note_on_at(0, 1, 48, 127));
            synth.process(&ctx(512), &mut bus, &events, None);

            let mut stopped = ctx(allowed_frames);
            stopped.playing = false;
            bus.clear(allowed_frames);
            synth.process(&stopped, &mut bus, &EventList::empty(), None);

            let tail = &bus.l[..allowed_frames];
            let residue = tail[tail.len() / 2..]
                .iter()
                .fold(0.0_f32, |a, s| a.max(s.abs()));
            assert!(
                residue < 1.0e-3,
                "{} was still sounding {residue:.5} after a stop",
                patch.name
            );
            assert!(
                !synth.voice.active || synth.voice.amp_env.is_idle(),
                "{} left a voice running after a stop",
                patch.name
            );
        }
    }

    /// Jumps one parameter in a single event under a held note, and reports
    /// the worst sample-to-sample step across the jump against the worst step
    /// in the settled signal either side of it.
    ///
    /// A single large jump is the right probe, and a slow ramp is not: five
    /// hundred steps across the range move the parameter by 0.002 each, which
    /// would not click even with the smoothing removed. What smoothing is for
    /// is the automation lane or the mouse that moves a knob from one end to
    /// the other in one block.
    ///
    /// Comparing against the settled steps on *both* sides is what keeps this
    /// honest in either direction: opening a filter genuinely raises the slew,
    /// so the after-state is the fair reference going up, and the before-state
    /// is the fair reference coming back down.
    fn jump_step_ratio(params: MlM1Params, param_id: u32, from: f32, to: f32) -> f32 {
        let frames = SR as usize / 2;
        let jump_at = frames / 2;
        // Comfortably longer than `PARAM_SMOOTH_S`, so the window contains the
        // whole of the smoothed transition and not a slice of it.
        let settle = (SR as f32 * 0.05) as usize;

        let mut synth = MlM1::new(params, SR);
        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        events.push(note_on_at(0, 1, 48, 110));
        events.push(TimedEvent {
            offset: 0,
            event: Event::ParamValue {
                id: param_id,
                value: from,
            },
        });
        events.push(TimedEvent {
            offset: jump_at as u32,
            event: Event::ParamValue {
                id: param_id,
                value: to,
            },
        });
        synth.process(&ctx(frames), &mut bus, &events, None);

        let before = max_step(&bus.l[jump_at - settle..jump_at]);
        let across = max_step(&bus.l[jump_at..jump_at + settle]);
        let after = max_step(&bus.l[jump_at + settle..frames]);
        // A floor, so a patch that is nearly silent either side cannot make
        // an inaudible wobble look like an infinite ratio.
        across / before.max(after).max(1.0e-4)
    }

    /// The plan asks for no clicks when the character controls are automated
    /// across their full range mid-note, in every patch.
    ///
    /// The amplitude sustain is forced up first. Four of the six patches decay
    /// to silence long before the jump, and a test that measured mostly
    /// silence would pass without having looked at anything. What is under
    /// test is the smoothing in the cutoff, drive and resonance path, which the
    /// amplitude envelope does not touch.
    #[test]
    fn automating_the_character_controls_mid_note_does_not_click() {
        for patch in mlm1_factory::patches() {
            let mut params = patch.params;
            params.sustain = 1.0;
            params.decay = 0.05;
            for (label, id) in [
                ("cutoff", SYNTH_PARAM_FILTER_CUTOFF),
                ("resonance", SYNTH_PARAM_FILTER_RESONANCE),
                ("drive", SYNTH_PARAM_DRIVE),
                ("accent", SYNTH_PARAM_ACCENT),
            ] {
                for (from, to) in [(0.0, 1.0), (1.0, 0.0)] {
                    let ratio = jump_step_ratio(params, id, from, to);
                    assert!(
                        ratio < 2.0,
                        "{}: jumping {label} {from} -> {to} stepped {ratio:.2}x \
                         the settled slew",
                        patch.name
                    );
                }
            }
        }
    }

    /// The plan's headline claim about the bank: Round Bass and Acid Line are
    /// two instruments, not one instrument twice. They share an oscillator
    /// setting — a single saw — so anything that separates them comes from
    /// the filter, and that is exactly what step 05 built the models for.
    #[test]
    fn round_bass_and_acid_line_are_different_instruments() {
        let patches = mlm1_factory::patches();
        let round = patches[0];
        let acid = patches[2];
        assert_eq!(round.name, "Round Bass");
        assert_eq!(acid.name, "Acid Line");

        let frames = (SR as f32 * 0.4) as usize;
        let round_sound = render_at(round.params, 36, 127, frames);
        let acid_sound = render_at(acid.params, 36, 127, frames);

        let round_brightness = brightness(&round_sound);
        let acid_brightness = brightness(&acid_sound);
        assert!(
            acid_brightness > round_brightness * 1.5,
            "the two bass patches are too alike: round {round_brightness:.4}, \
             acid {acid_brightness:.4}"
        );
    }

    /// Switching the Model switch must not act as a volume control.
    ///
    /// Adam's finding from the 08 listening pass: the three models had
    /// "pretty different apparent loudness between types". Measured, they sat
    /// up to 10.4 dB apart at identical cutoff and resonance -- the two
    /// nonlinear models lose level to compression as the feedback path drives
    /// them, while the linear one gains it from its resonant peak, so they
    /// diverge in *opposite* directions as Resonance comes up.
    ///
    /// The two existing guards could not see this. `resonant_filter_and_drive_stay_bounded`
    /// only asserts a peak ceiling, and the model-switch test measures the
    /// step at the moment of switching, not the level either side of it -- a
    /// steady 10 dB offset passes both.
    #[test]
    fn the_three_filter_models_are_matched_in_level() {
        const SR: u32 = 48_000;
        let mut phase = 0.0f32;
        let input: Vec<f32> = (0..SR as usize / 2)
            .map(|_| {
                let v = (phase * 2.0 - 1.0) * 0.251;
                phase = (phase + 110.0 / SR as f32).fract();
                v
            })
            .collect();
        let settle = input.len() / 4;

        let mut worst: (f32, f32, f32) = (0.0, 0.0, 0.0);
        for &cutoff in &[200.0f32, 500.0, 1000.0, 2000.0, 5000.0] {
            for &res in &[0.0f32, 0.3, 0.6, 0.9] {
                let levels: Vec<f32> = [FilterModel::Ladder, FilterModel::Acid, FilterModel::Clean]
                    .iter()
                    .map(|&model| {
                        let mut f = VoiceFilter::new();
                        let out: Vec<f32> = input
                            .iter()
                            .map(|&x| f.next_sample(model, x, cutoff, res, SR))
                            .collect();
                        let mean_sq = out[settle..].iter().map(|s| s * s).sum::<f32>()
                            / out[settle..].len() as f32;
                        20.0 * mean_sq.sqrt().max(1e-9).log10()
                    })
                    .collect();
                let spread = levels.iter().fold(f32::MIN, |a, &b| a.max(b))
                    - levels.iter().fold(f32::MAX, |a, &b| a.min(b));
                if spread > worst.0 {
                    worst = (spread, cutoff, res);
                }
            }
        }

        // Measured worst case is 2.4 dB, most of it the honest difference
        // between a three-pole and a four-pole slope. 3.5 dB leaves room to
        // voice the makeup constants by ear without the test becoming a
        // tripwire, while still catching the 10 dB regression it exists for.
        assert!(
            worst.0 < 3.5,
            "filter models are {:.2} dB apart at cutoff {} Hz, resonance {} -- \
             the Model switch is acting as a volume control",
            worst.0, worst.1, worst.2
        );
    }
}
