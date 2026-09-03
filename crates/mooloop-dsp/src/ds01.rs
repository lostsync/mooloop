//! DS-01: one universal percussion voice.
//!
//! Not a mode-union. There is no Kick/Snare/Hat selector and no second
//! synthesis engine; kick, snare, rim, clap, tom, hat, cowbell and the sounds
//! with no name are configurations of the same architecture, and every
//! control is live in every one of them. That is what makes one descriptor id
//! mean one thing forever, which is the whole reason the device exists — see
//! `docs/plans/drum-synth-v2/01-what-ds01-is.md`.
//!
//! This step builds the tone layer, the noise layer, the amplitude and pitch
//! envelopes, and the voice pool. The body resonator (04), the burst (05) and
//! the shape stage (06) are later layers into the same sum.
//!
//! ## Latched versus continuous
//!
//! A drum hit is short and its interesting part is the first few
//! milliseconds, so "what does changing this parameter do to a hit already
//! sounding?" is a real question with a musical answer. DS-01 publishes it as
//! a rule rather than inheriting it from wherever the code happens to read a
//! struct:
//!
//! - **Latched** at [`Voice::trigger`], from the values current at that
//!   sample, and never revisited for the life of that hit: envelope times,
//!   pitch depth, note pitch and Tune, velocity. [`Latched`] is the whole
//!   list, and nothing outside it is read from a latch.
//! - **Continuous**, read every control tick and smoothed where a step would
//!   click: the layer levels and the output level, the filter's cutoff,
//!   resonance and morph, the noise rate, and the tone's wave morph and
//!   spread. [`Continuous`] is that list.
//!
//! Latched is not a lesser status. Because each hit re-latches, an LFO on Amp
//! Decay produces a hat pattern whose hits differ from one another, which is
//! the musically useful reading and the one a per-sample re-derivation would
//! destroy.
//!
//! The control tick is the event split: `process` renders up to each event
//! offset and then applies the event, and modulation arrives as timed
//! `Event::ParamValue` at the engine's 32-frame control rate. So a continuous
//! parameter is re-read exactly when it changes, which is finer than per
//! block and coarser than per sample — the same arrangement ML-P8 uses.
//!
//! ## The three the tables do not name
//!
//! `01-what-ds01-is.md` says a parameter in neither table is a bug in that
//! document rather than a free choice here, and three of this step's are:
//! Partials, Noise Colour and Retrigger. All three are structural discretes,
//! which is why they escaped two tables about what a *sounding* hit follows.
//! Resolved as:
//!
//! - **Partials is latched.** It decides how many oscillators this hit has,
//!   and a hit does not grow one halfway through.
//! - **Noise Colour and Retrigger are read where they are used** — the colour
//!   per sample, the retrigger rule at the next trigger. Step 02 defines
//!   changing colour between hits as fine and mid-hit as undefined, so
//!   neither reading is wrong; this is the one that keeps the generator's
//!   phase across a level move.

use crate::bus::StereoBus;
use crate::env::ExpDecay;
use crate::event::{Event, EventList};
use crate::filter::{soft_ceiling, Svf};
use crate::node::{AudioNode, ProcessContext};
use crate::osc::{Noise, Osc};
use crate::smooth::Smoothed;
use mooloop_core::{
    ds01, Ds01NoiseColor, Ds01Params, Ds01Retrigger, OscWave, DS01_MAX_PARTIALS, DS01_VOICES,
    MAX_CHOKE_GROUP,
};

/// The voice's absolute output reference, provisional until step 06 settles
/// the gain contract and the shape stage that goes with it.
///
/// Set so the default patch — one tone layer at its default level, the device
/// Level at 0.8, full velocity — peaks within a dB of
/// `mooloop_core::gain::GENERATOR_OUTPUT_REFERENCE_DBFS`, which is v1's own
/// calibration. A v1 kick and a DS-01 kick therefore sit in the same place in
/// a mix. `default_patch_peaks_at_the_generator_reference` is what holds it
/// there.
const VOICE_OUTPUT_REFERENCE: f32 = 0.4444;

/// Where the device's output bound starts to bend, and what it is asymptotic
/// to, both in output units.
///
/// Not [`soft_ceiling`]'s numbers, which are calibrated against ML-P8's voice
/// nominal of about 0.7 and sit at 1.5 and 2.5 — above full scale, because
/// there a channel fader is still downstream. DS-01 needs a bound that holds
/// *at the device output* for every control combination, so it is stated
/// here: 0.7 is well above the reference peak a loud patch reaches, and 1.0
/// is full scale, which the asymptote approaches and never crosses.
///
/// This is audible saturation and not a hidden limiter: a patch driven past
/// the knee gets dirty rather than quietly held. Step 06 folds it into the
/// shape stage, which is where a character control can decide *how* it bends.
const DEVICE_KNEE: f32 = 0.7;
const DEVICE_CEILING: f32 = 1.0;

/// How fast a continuous control reaches a new value.
///
/// Short and constant. A drum tail is short enough that a slow smoother is
/// audible as a swell rather than as the absence of a click, so this is sized
/// to cover a step and nothing more.
const SMOOTHING_S: f32 = 0.002;

/// The TR-808's six square-oscillator frequencies, as ratios of the lowest.
///
/// Not an emulation and not presented as one: it is a published inharmonic
/// ratio set that is known to ring like struck metal, which is exactly what
/// the partial bank and the Metal noise colour both want. v1 hardcodes two of
/// these as absolute frequencies; DS-01 reaches them as a patch.
const METAL_RATIOS: [f32; DS01_MAX_PARTIALS as usize] =
    [1.0, 1.482_7, 1.800_3, 2.546_0, 2.630_3, 3.896_7];

/// Output gain of the pink filter, which is what brings its heavily
/// low-passed sum back to white's level. Measured rather than derived: see
/// `noise_colours_share_a_level`.
const PINK_GAIN: f32 = 0.27;

/// Impulses per second in the Velvet noise colour. Sparse enough to hear as
/// individual clicks at the bottom of the rate range and dense enough to read
/// as texture at the top.
const VELVET_DENSITY_HZ: f32 = 1_500.0;

/// Phase deviation, in cycles, at full FM Amount. Two cycles is well past the
/// point where the spectrum stops being a tone and starts being a clang,
/// which is what the control is for.
const FM_MAX_CYCLES: f32 = 2.0;

/// Everything one hit latches at trigger. Split out as its own type so
/// "nothing else in the voice reads a latched parameter" is checkable by
/// looking at one struct rather than by auditing a render loop.
#[derive(Clone, Copy, Debug)]
struct Latched {
    /// Note and Tune, resolved to a multiplier on the tone pitch.
    pitch_factor: f32,
    /// Velocity, already crossfaded through Velocity Amount.
    velocity_amp: f32,
    /// Bipolar pitch excursion in semitones.
    pitch_depth: f32,
    /// Partial count. Structural, so it is latched with the hit rather than
    /// changing how many oscillators a sounding voice has.
    partials: u8,
}

impl Default for Latched {
    fn default() -> Self {
        Self {
            pitch_factor: 1.0,
            velocity_amp: 0.0,
            pitch_depth: 0.0,
            partials: 1,
        }
    }
}

/// The continuous controls, resolved once per control tick and shared by
/// every sounding voice.
///
/// Device-wide rather than per voice because that is what they are: a mix
/// move is a device move. Per-voice modulation of the same values arrives in
/// step 07 as an offset the voice adds, not as a second copy of the base.
#[derive(Clone, Copy, Debug)]
struct Continuous {
    tone_wave: (OscWave, OscWave),
    tone_wave_mix: f32,
    tone_spread: f32,
    tone_pitch: f32,
    fm_amount: f32,
    fm_ratio: f32,
    noise_color: Ds01NoiseColor,
    noise_rate: f32,
    filter_morph: f32,
    filter_cutoff: f32,
    filter_res: f32,
}

impl Continuous {
    fn new(p: &Ds01Params) -> Self {
        let (waves, mix) = morph_waves(p.tone_wave);
        Self {
            tone_wave: waves,
            tone_wave_mix: mix,
            tone_spread: p.tone_spread.clamp(0.0, 1.0),
            tone_pitch: p.tone_pitch.max(1.0),
            fm_amount: p.tone_fm_amount.clamp(0.0, 1.0),
            fm_ratio: p.tone_fm_ratio.max(0.01),
            noise_color: p.noise_color,
            noise_rate: p.noise_rate.max(1.0),
            filter_morph: p.filter_morph.clamp(0.0, 1.0),
            filter_cutoff: p.filter_cutoff.max(1.0),
            filter_res: p.filter_res.clamp(0.0, 1.0),
        }
    }
}

/// Split the wave morph into the pair it sits between and the mix across it.
///
/// Sine > triangle > saw > pulse, three equal segments. Reading one phase
/// twice rather than mixing two oscillators is what keeps the morph exactly
/// continuous — see [`Osc::next_step_morph`].
fn morph_waves(morph: f32) -> ((OscWave, OscWave), f32) {
    const ORDER: [OscWave; 4] = [
        OscWave::Sine,
        OscWave::Triangle,
        OscWave::Saw,
        OscWave::Pulse,
    ];
    let position = morph.clamp(0.0, 1.0) * (ORDER.len() - 1) as f32;
    let index = (position.floor() as usize).min(ORDER.len() - 2);
    ((ORDER[index], ORDER[index + 1]), position - index as f32)
}

/// The noise layer's source, before the rate reducer and the filter.
///
/// Colour is structural because the generators genuinely differ; the rate
/// reducer downstream applies to all four, which is what keeps the section
/// from having an inert control in any configuration.
#[derive(Clone, Copy, Debug)]
struct NoiseSource {
    white: Noise,
    /// Paul Kellet's economy pink filter state.
    pink: [f32; 3],
    /// Squares for the Metal colour, ring-modulated against each other.
    metal: [Osc; 3],
}

impl NoiseSource {
    fn new(seed: u32) -> Self {
        Self {
            white: Noise::new(seed),
            pink: [0.0; 3],
            metal: [Osc::new(); 3],
        }
    }

    fn reset(&mut self, seed: u32) {
        self.white.reset(seed);
        self.pink = [0.0; 3];
        for osc in &mut self.metal {
            osc.reset();
        }
    }

    /// One sample of the selected colour, in roughly `[-1, 1]`.
    fn next_sample(&mut self, color: Ds01NoiseColor, base_hz: f32, sample_rate: u32) -> f32 {
        let white = self.white.next_sample();
        match color {
            Ds01NoiseColor::White => white,
            Ds01NoiseColor::Pink => {
                // Paul Kellet's three-pole economy pink filter, coefficients
                // as published rather than reassembled from two variants of
                // it. The trailing gain is what keeps pink and white at
                // roughly one level, so changing colour is a timbre change
                // and not a level change — `noise_colours_share_a_level`
                // holds it there.
                self.pink[0] = 0.997_65 * self.pink[0] + white * 0.099_046;
                self.pink[1] = 0.963_00 * self.pink[1] + white * 0.296_516_4;
                self.pink[2] = 0.570_00 * self.pink[2] + white * 1.052_691_3;
                (self.pink[0] + self.pink[1] + self.pink[2] + white * 0.1848) * PINK_GAIN
            }
            Ds01NoiseColor::Velvet => {
                // Sparse signed impulses. The density is fixed; the rate
                // reducer downstream is what thins or thickens it.
                let probability = (VELVET_DENSITY_HZ / sample_rate as f32).clamp(0.0, 1.0);
                if (white * 0.5 + 0.5) < probability {
                    if self.white.next_sample() >= 0.0 {
                        1.0
                    } else {
                        -1.0
                    }
                } else {
                    0.0
                }
            }
            Ds01NoiseColor::Metal => {
                // Three squares against three ratios of the same base, one
                // pair ring-modulated into the other, which is the cheapest
                // route to a dense inharmonic spectrum with no pitch.
                let a = self.metal[0].next_sample(base_hz, OscWave::Pulse, 0.5, sample_rate);
                let b = self.metal[1].next_sample(
                    base_hz * METAL_RATIOS[2],
                    OscWave::Pulse,
                    0.5,
                    sample_rate,
                );
                let c = self.metal[2].next_sample(
                    base_hz * METAL_RATIOS[5],
                    OscWave::Pulse,
                    0.5,
                    sample_rate,
                );
                (a * b + c) * 0.5
            }
        }
    }
}

/// One independently enveloped hit.
struct Voice {
    active: bool,
    age: u64,
    seed: u32,
    latched: Latched,
    amp_env: ExpDecay,
    pitch_env: ExpDecay,
    /// One oscillator per partial. Unused partials are never stepped.
    tone: [Osc; DS01_MAX_PARTIALS as usize],
    fm: Osc,
    noise: NoiseSource,
    filter: Svf,
    /// Rate-reducer state: the held sample and the fraction of a held period
    /// elapsed.
    held_noise: f32,
    hold_phase: f32,
}

impl Voice {
    fn new(seed: u32) -> Self {
        Self {
            active: false,
            age: 0,
            seed,
            latched: Latched::default(),
            amp_env: ExpDecay::new(),
            pitch_env: ExpDecay::new(),
            tone: [Osc::new(); DS01_MAX_PARTIALS as usize],
            fm: Osc::new(),
            noise: NoiseSource::new(seed),
            filter: Svf::new(),
            held_noise: 0.0,
            hold_phase: 1.0,
        }
    }

    fn reset(&mut self) {
        self.active = false;
        self.age = 0;
        self.latched = Latched::default();
        self.amp_env = ExpDecay::new();
        self.pitch_env = ExpDecay::new();
        for osc in &mut self.tone {
            osc.reset();
        }
        self.fm.reset();
        self.noise.reset(self.seed);
        self.filter.reset();
        self.held_noise = 0.0;
        self.hold_phase = 1.0;
    }

    /// Start a hit. Every value this reads from `params` is in [`Latched`];
    /// nothing continuous is captured here.
    fn trigger(&mut self, params: &Ds01Params, note: u8, velocity: u8, sample_rate: u32) {
        let semitones = f32::from(note.min(127)) - 60.0 + params.tune.clamp(-48.0, 48.0);
        let velocity_unit = f32::from(velocity) / 127.0;
        let amount = params.velocity_amount.clamp(0.0, 1.0);
        self.latched = Latched {
            pitch_factor: 2.0_f32.powf(semitones / 12.0),
            // A crossfade rather than a multiply: at 0 every hit is equally
            // loud, at 1 it follows the note as played. v1 could only do the
            // latter.
            velocity_amp: 1.0 - amount + amount * velocity_unit,
            pitch_depth: params.pitch_depth.clamp(-60.0, 60.0),
            partials: params.tone_partials.clamp(1, DS01_MAX_PARTIALS),
        };
        self.active = true;
        for osc in &mut self.tone {
            osc.reset();
        }
        self.fm.reset();
        self.filter.reset();
        self.hold_phase = 1.0;
        self.amp_env.set_time(params.amp_decay, sample_rate);
        self.amp_env.trigger();
        self.pitch_env.set_time(params.pitch_decay, sample_rate);
        self.pitch_env.trigger();
    }

    /// Fade this hit out over `seconds`, as a release on the amplitude
    /// envelope rather than as a coefficient stamped over it, so step 03's
    /// envelope shapes do not have to special-case a choke.
    fn choke(&mut self, seconds: f32, sample_rate: u32) {
        if !self.active {
            return;
        }
        self.amp_env.set_time(seconds.max(0.001), sample_rate);
    }

    /// One sample of this voice, pre-level. Mono by design; the channel strip
    /// places it in the stereo field, as it does for every other generator.
    fn render_sample(
        &mut self,
        c: &Continuous,
        tone_level: f32,
        noise_level: f32,
        sample_rate: u32,
    ) -> f32 {
        self.amp_env.advance();
        self.pitch_env.advance();

        let swept = c.tone_pitch
            * self.latched.pitch_factor
            * 2.0_f32.powf(self.latched.pitch_depth * self.pitch_env.level() / 12.0);

        // Both layers run whatever their level is. A layer at zero is a mix
        // decision and not a mode: it keeps its phase, so a level returning
        // from zero resumes rather than restarting, and step 04's body
        // excitation reads the same pre-level tap.
        let tone = self.render_tone(c, swept, sample_rate);

        let noise = self.render_noise(c, swept, sample_rate);

        // The voice's own guard, before any level scales it: a state-variable
        // filter at full resonance is linear and will happily hand back
        // several times full scale, which is what this catches. The device
        // bound below is a different job and is sized differently.
        soft_ceiling(
            (tone * tone_level + noise * noise_level)
                * self.amp_env.level()
                * self.latched.velocity_amp,
        )
    }

    fn render_tone(&mut self, c: &Continuous, swept_hz: f32, sample_rate: u32) -> f32 {
        // FM tracks the swept pitch rather than the tuned one, so the timbre
        // holds its shape across a pitch envelope instead of turning into a
        // different sound halfway down the sweep.
        let phase_offset = if c.fm_amount > 0.0 {
            let modulator =
                self.fm
                    .next_sample(swept_hz * c.fm_ratio, OscWave::Sine, 0.5, sample_rate);
            modulator * c.fm_amount * c.fm_amount * FM_MAX_CYCLES
        } else {
            0.0
        };

        let partials = self.latched.partials.max(1) as usize;
        let mut sum = 0.0;
        for (index, osc) in self.tone.iter_mut().take(partials).enumerate() {
            // Spread runs the bank from unison at 0 to the metal ratios at 1.
            // At 0 every partial collapses onto the fundamental, so a
            // six-partial patch there is the one-partial patch to within the
            // rounding of summing and dividing by six — which is what makes
            // Spread a range with a meaningful bottom rather than a switch.
            let ratio = 1.0 + c.tone_spread * (METAL_RATIOS[index] - 1.0);
            sum += osc
                .next_step_morph(
                    swept_hz * ratio,
                    c.tone_wave,
                    c.tone_wave_mix,
                    0.5,
                    phase_offset,
                    sample_rate,
                )
                .value;
        }
        // Averaged, not summed: the bank has to keep the level the gain
        // contract calibrated at one partial.
        sum / partials as f32
    }

    fn render_noise(&mut self, c: &Continuous, swept_hz: f32, sample_rate: u32) -> f32 {
        // The rate reducer applies to every colour, which is what stops the
        // noise section from having an inert control in any configuration.
        // Above the running rate it is transparent by construction.
        let step = (c.noise_rate / sample_rate as f32).min(1.0);
        self.hold_phase += step;
        if self.hold_phase >= 1.0 {
            self.hold_phase -= 1.0;
            self.held_noise = self.noise.next_sample(c.noise_color, swept_hz, sample_rate);
        }

        let (low, band, high) =
            self.filter
                .next_sample_lp_bp_hp(self.held_noise, c.filter_cutoff, c.filter_res, sample_rate);
        morph3(low, band, high, c.filter_morph)
    }
}

/// The device's output bound. Exactly transparent below [`DEVICE_KNEE`],
/// asymptotic to [`DEVICE_CEILING`] above it.
fn device_bound(input: f32) -> f32 {
    let magnitude = input.abs();
    if magnitude <= DEVICE_KNEE {
        return input;
    }
    let headroom = DEVICE_CEILING - DEVICE_KNEE;
    let over = (magnitude - DEVICE_KNEE) / headroom;
    input.signum() * (DEVICE_KNEE + headroom * over.tanh())
}

/// Crossfade three filter outputs across a single 0..1 morph: low-pass at 0,
/// band-pass at the middle, high-pass at 1. A morph rather than a mode
/// selector, for the reason the wave morph is one.
fn morph3(low: f32, band: f32, high: f32, morph: f32) -> f32 {
    let position = morph.clamp(0.0, 1.0) * 2.0;
    if position <= 1.0 {
        low + (band - low) * position
    } else {
        band + (high - band) * (position - 1.0)
    }
}

/// The DS-01 node.
pub struct Ds01 {
    params: Ds01Params,
    sample_rate: u32,
    voices: [Voice; DS01_VOICES],
    next_age: u64,
    tone_level: Smoothed,
    noise_level: Smoothed,
    level: Smoothed,
}

impl Ds01 {
    pub fn new(mut params: Ds01Params, sample_rate: u32) -> Self {
        params.choke_group = params.choke_group.min(MAX_CHOKE_GROUP);
        Self {
            params,
            sample_rate,
            voices: std::array::from_fn(|index| {
                Voice::new(0x9E37_79B9_u32.wrapping_add(index as u32))
            }),
            next_age: 1,
            tone_level: Smoothed::new(params.tone_level, SMOOTHING_S, sample_rate),
            noise_level: Smoothed::new(params.noise_level, SMOOTHING_S, sample_rate),
            level: Smoothed::new(params.level, SMOOTHING_S, sample_rate),
        }
    }

    /// Replace the parameter set. Called from the RT command drain.
    pub fn set_params(&mut self, mut params: Ds01Params) {
        params.choke_group = params.choke_group.min(MAX_CHOKE_GROUP);
        self.params = params;
    }

    /// Apply one descriptor-addressed parameter, leaving the rest alone.
    ///
    /// Routed through the core setter so a control-rate change gets exactly
    /// the same clamping a whole-struct update does. Both are non-allocating.
    fn apply_param(&mut self, id: u32, value: f32) {
        let Some(descriptor) = ds01::descriptor(id) else {
            return;
        };
        let mut params = self.params;
        if ds01::set(&mut params, id, descriptor.clamp_natural(value)) {
            self.set_params(params);
        }
    }

    pub fn choke_group(&self) -> u8 {
        self.params.choke_group
    }

    /// Immediately invalidate all voices.
    pub fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.reset();
        }
        self.next_age = 1;
        self.tone_level.reset_to(self.params.tone_level);
        self.noise_level.reset_to(self.params.noise_level);
        self.level.reset_to(self.params.level);
    }

    pub fn choke(&mut self) {
        let seconds = self.params.choke_time;
        let sr = self.sample_rate;
        for voice in &mut self.voices {
            voice.choke(seconds, sr);
        }
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
        // Mono retrigger chokes what this channel is already playing before
        // it allocates, which is what a real 808 does and what a fast hat
        // pattern usually wants. Poly is v1's behaviour and stays the
        // default, so nothing about the feel of an existing pattern changes.
        if self.params.retrigger == Ds01Retrigger::Mono {
            self.choke();
        }
        let index = self.select_voice();
        let age = self.next_age;
        self.next_age = self.next_age.wrapping_add(1).max(1);
        let params = self.params;
        let sr = self.sample_rate;
        let voice = &mut self.voices[index];
        voice.age = age;
        voice.trigger(&params, note, velocity, sr);
    }

    fn render_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        if start >= end {
            return;
        }
        // One control tick: the continuous controls are resolved once here
        // and the smoothers are re-aimed, then every sample in the range runs
        // against them. `process` splits at every event offset, so this runs
        // again the moment any of them changes.
        let continuous = Continuous::new(&self.params);
        self.tone_level
            .set_target(self.params.tone_level.clamp(0.0, 1.0));
        self.noise_level
            .set_target(self.params.noise_level.clamp(0.0, 1.0));
        self.level.set_target(self.params.level.clamp(0.0, 1.0));

        let sr = self.sample_rate;
        if !self.voices.iter().any(|voice| voice.active) {
            // Nothing is sounding, but the smoothers still have to travel:
            // otherwise a level moved during silence would jump at the next
            // hit instead of already being there.
            self.tone_level.advance_by(end - start);
            self.noise_level.advance_by(end - start);
            self.level.advance_by(end - start);
            return;
        }

        for frame in start..end {
            let tone_level = self.tone_level.advance();
            let noise_level = self.noise_level.advance();
            let level = self.level.advance();
            let mut sum = 0.0;
            for voice in self.voices.iter_mut().filter(|voice| voice.active) {
                sum += voice.render_sample(&continuous, tone_level, noise_level, sr);
                if voice.amp_env.is_idle() {
                    voice.active = false;
                }
            }
            let sample = device_bound(sum * level * VOICE_OUTPUT_REFERENCE);
            bus.l[frame] += sample;
            bus.r[frame] += sample;
        }
    }

    /// Render one deterministic hit through the production voice path and
    /// reduce it to min/max bins suitable for a waveform overview. v1's best
    /// property, kept: the drawn hit is the hit.
    pub fn preview_waveform(params: Ds01Params, bins: usize) -> (Vec<f32>, Vec<f32>) {
        if bins == 0 {
            return (Vec::new(), Vec::new());
        }
        const PREVIEW_SAMPLE_RATE: u32 = 48_000;
        const PREVIEW_SECONDS: f32 = 0.3;
        let frames = (PREVIEW_SAMPLE_RATE as f32 * PREVIEW_SECONDS) as usize;

        let mut node = Self::new(params, PREVIEW_SAMPLE_RATE);
        node.trigger(60, 127);
        let mut bus = StereoBus::with_capacity(frames);
        node.render_range(&mut bus, 0, frames);

        let mut minimums = vec![f32::INFINITY; bins];
        let mut maximums = vec![f32::NEG_INFINITY; bins];
        for (frame, sample) in bus.l[..frames].iter().enumerate() {
            let bin = (frame * bins / frames).min(bins - 1);
            minimums[bin] = minimums[bin].min(*sample);
            maximums[bin] = maximums[bin].max(*sample);
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
}

impl AudioNode for Ds01 {
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

        // Split the block at event offsets: render, apply event, repeat. The
        // events arrive sorted, and parameter events precede note-ons at the
        // same offset, so a route aimed at a hit lands on *that* hit — see
        // `mooloop_engine::render`, which is where that order is established.
        let mut pos = 0usize;
        for ev in events_in.iter() {
            let offset = (ev.offset as usize).min(frames).max(pos);
            self.render_range(bus, pos, offset);
            match ev.event {
                Event::NoteOn { note, velocity, .. } => self.trigger(note, velocity),
                // One-shot for now. Step 03's gated envelopes are what give
                // note-off a meaning here.
                Event::NoteOff { .. } => {}
                Event::Choke => self.choke(),
                Event::ParamValue { id, value } => self.apply_param(id, value),
                Event::Buffer(_) | Event::BufferRelease | Event::BufferScrub { .. } => {}
            }
            pos = offset;
        }
        self.render_range(bus, pos, frames);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::TimedEvent;
    use mooloop_core::gain::{db_to_linear, GENERATOR_OUTPUT_REFERENCE_DBFS};

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

    fn note_on(offset: u32, note: u8, velocity: u8) -> TimedEvent {
        TimedEvent {
            offset,
            event: Event::NoteOn {
                id: 0,
                note,
                velocity,
            },
        }
    }

    fn param(offset: u32, id: u32, value: f32) -> TimedEvent {
        TimedEvent {
            offset,
            event: Event::ParamValue { id, value },
        }
    }

    /// Render one block through the node and hand back the left channel.
    fn render(node: &mut Ds01, frames: usize, events: &EventList) -> Vec<f32> {
        let mut bus = StereoBus::with_capacity(frames);
        node.process(&ctx(frames), &mut bus, events, None);
        bus.l[..frames].to_vec()
    }

    fn hit(params: Ds01Params, frames: usize) -> Vec<f32> {
        let mut node = Ds01::new(params, SR);
        let mut events = EventList::empty();
        events.push(note_on(0, 60, 127));
        render(&mut node, frames, &events)
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()))
    }

    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    fn upward_crossings(samples: &[f32]) -> usize {
        samples
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count()
    }

    /// What a DS-01 costs a channel, split where the engine's own footprint
    /// test cannot see it. Every live channel holds one node of every
    /// generator kind, so this is paid whether or not the channel is a drum
    /// — which is why it is pinned rather than described. Steps 04 through 07
    /// add a body resonator, a burst schedule and a matrix to the voice; if
    /// this number moves, check the new one is worth paying before updating
    /// it.
    #[test]
    fn a_voice_and_a_node_cost_what_their_layers_cost() {
        use core::mem::size_of;
        // Six tone oscillators for the partial bank, an FM modulator, the
        // four noise generators' shared state, a state-variable filter, the
        // rate reducer's held sample, two envelopes, and the latch.
        assert_eq!(size_of::<Voice>(), 200);
        // Eight of those, plus the parameter block and the three device-wide
        // smoothers that the layers share. The pool is 93% of the node.
        assert_eq!(size_of::<Ds01Params>(), 76);
        assert_eq!(size_of::<Ds01>(), 1_728);
        assert_eq!(size_of::<Voice>() * DS01_VOICES, 1_600);
    }

    #[test]
    fn idle_is_silent() {
        let mut node = Ds01::new(Ds01Params::default(), SR);
        assert_eq!(peak(&render(&mut node, 256, &EventList::empty())), 0.0);
    }

    #[test]
    fn note_on_at_offset_is_sample_accurate() {
        let mut node = Ds01::new(Ds01Params::default(), SR);
        let mut events = EventList::empty();
        events.push(note_on(200, 60, 127));
        let out = render(&mut node, 512, &events);
        assert!(out[..200].iter().all(|s| *s == 0.0));
        assert!(out[200..].iter().any(|s| s.abs() > 0.01));
    }

    /// The gain contract, pinned. The default patch at full velocity has to
    /// land where v1's default kick lands, so a v1 kick and a DS-01 kick sit
    /// in the same place in a mix and neither needs its fader moved.
    /// [`VOICE_OUTPUT_REFERENCE`] is the only thing holding this, and step 06
    /// replaces it with the shape stage's own reference.
    #[test]
    fn the_default_patch_peaks_at_the_generator_reference() {
        let reference = db_to_linear(GENERATOR_OUTPUT_REFERENCE_DBFS);
        let measured = peak(&hit(Ds01Params::default(), SR as usize));
        let error_db = 20.0 * (measured / reference).log10();
        assert!(
            error_db.abs() < 1.0,
            "default patch peaked at {measured}, reference is {reference} ({error_db} dB off)"
        );
    }

    #[test]
    fn a_hit_decays_and_frees_its_voice() {
        let mut node = Ds01::new(Ds01Params::default(), SR);
        let mut events = EventList::empty();
        events.push(note_on(0, 60, 127));
        let out = render(&mut node, SR as usize, &events);
        assert!(peak(&out[..2_400]) > 0.2);
        assert!(peak(&out[24_000..]) < peak(&out[..2_400]) * 0.2);

        // Four seconds is past the longest default tail.
        for _ in 0..4 {
            render(&mut node, SR as usize, &EventList::empty());
        }
        assert!(node.voices.iter().all(|voice| !voice.active));
    }

    /// Half of the latched-versus-continuous contract: an envelope time is
    /// resolved when the hit starts and is not revisited, which is what makes
    /// an LFO on Amp Decay produce a pattern whose hits differ from one
    /// another rather than one hit that changes shape as it rings.
    #[test]
    fn amp_decay_is_latched_at_the_hit_and_reaches_the_next_one() {
        let mut node = Ds01::new(Ds01Params::default(), SR);
        let mut events = EventList::empty();
        events.push(note_on(0, 60, 127));
        // Far shorter than the 0.24 s default, applied a long way into the
        // hit. The tail after it must be unaffected.
        events.push(param(4_800, ds01::PARAM_AMP_DECAY, 0.01));
        let changed = render(&mut node, 24_000, &events);

        let mut untouched = Ds01::new(Ds01Params::default(), SR);
        let mut plain = EventList::empty();
        plain.push(note_on(0, 60, 127));
        let reference = render(&mut untouched, 24_000, &plain);

        assert_eq!(
            changed, reference,
            "the sounding hit followed a latched parameter"
        );

        // The next hit does take it, from the same node, with no further
        // events: the value is still in place. Measured early, where the
        // reference is unambiguously still sounding — a 0.24 s decay is
        // already inaudible by the end of a half-second block, so a late
        // window would compare two silences.
        let mut next = EventList::empty();
        next.push(note_on(0, 60, 127));
        let after = render(&mut node, 24_000, &next);
        assert!(
            rms(&after[2_400..4_800]) < rms(&reference[2_400..4_800]) * 0.1,
            "the next hit ignored the new decay"
        );
    }

    /// The other half: a sweepable control moves the hit that is already
    /// sounding.
    #[test]
    fn filter_cutoff_is_continuous_within_a_sounding_hit() {
        let params = Ds01Params {
            tone_level: 0.0,
            noise_level: 1.0,
            filter_morph: 0.0,
            filter_cutoff: 18_000.0,
            amp_decay: 2.0,
            ..Ds01Params::default()
        };
        let mut node = Ds01::new(params, SR);
        let mut events = EventList::empty();
        events.push(note_on(0, 60, 127));
        events.push(param(4_800, ds01::PARAM_FILTER_CUTOFF, 60.0));
        let out = render(&mut node, 24_000, &events);

        let open = rms(&out[2_400..4_800]);
        let closed = rms(&out[12_000..14_400]);
        assert!(
            closed < open * 0.5,
            "closing the low-pass mid-hit did nothing: {open} then {closed}"
        );
    }

    /// The event-ordering contract from `01-what-ds01-is.md`, at the device:
    /// a parameter event at offset `n` is visible to a note-on at offset `n`.
    /// Without it a route aimed at a hit lands on the *next* hit, which is
    /// both wrong and untestable-looking.
    #[test]
    fn a_parameter_event_reaches_the_note_on_at_the_same_offset() {
        let long = Ds01Params {
            pitch_decay: 2.0,
            amp_decay: 2.0,
            ..Ds01Params::default()
        };
        let pitch_of = |events: &EventList| {
            let mut node = Ds01::new(long, SR);
            let out = render(&mut node, 9_600, events);
            upward_crossings(&out[..4_800])
        };

        let mut plain = EventList::empty();
        plain.push(note_on(64, 60, 127));

        // Pushed note-on first, so the list's own ordering is what puts the
        // parameter in front of it rather than the order of these two lines.
        let mut aimed = EventList::empty();
        aimed.push_ordered(note_on(64, 60, 127));
        aimed.push_ordered(param(64, ds01::PARAM_PITCH_DEPTH, -24.0));

        let default_pitch = pitch_of(&plain);
        let aimed_pitch = pitch_of(&aimed);
        assert!(
            aimed_pitch < default_pitch,
            "the hit ignored the parameter aimed at it: {default_pitch} then {aimed_pitch}"
        );
    }

    /// Spread at zero collapses the bank onto the fundamental, so a
    /// six-partial patch there is the one-partial patch. That is what makes
    /// Spread a range with a meaningful bottom rather than a switch, and it
    /// is the exception `01-what-ds01-is.md` names — Spread is inert at one
    /// partial, and nowhere else.
    ///
    /// Compared within rounding rather than exactly: the bank sums six
    /// identical values and divides by six, which is not the identity in
    /// `f32`.
    #[test]
    fn spread_at_zero_collapses_the_partial_bank() {
        let one = Ds01Params {
            tone_partials: 1,
            tone_spread: 0.0,
            ..Ds01Params::default()
        };
        let six = Ds01Params {
            tone_partials: 6,
            ..one
        };
        let difference = hit(one, 4_800)
            .iter()
            .zip(hit(six, 4_800).iter())
            .fold(0.0_f32, |worst, (a, b)| worst.max((a - b).abs()));
        assert!(difference < 1.0e-6, "six partials at Spread 0 differ by {difference}");

        let spread = Ds01Params {
            tone_spread: 1.0,
            ..six
        };
        assert_ne!(hit(six, 4_800), hit(spread, 4_800));
    }

    #[test]
    fn every_noise_colour_makes_sound_and_terminates() {
        for color in Ds01NoiseColor::ALL {
            let params = Ds01Params {
                tone_level: 0.0,
                noise_level: 1.0,
                noise_color: color,
                filter_morph: 0.5,
                ..Ds01Params::default()
            };
            let mut node = Ds01::new(params, SR);
            let mut events = EventList::empty();
            events.push(note_on(0, 60, 127));
            let out = render(&mut node, SR as usize, &events);
            assert!(peak(&out) > 0.01, "{color:?} made nothing");
            assert!(out.iter().all(|s| s.is_finite()), "{color:?} went non-finite");

            for _ in 0..4 {
                render(&mut node, SR as usize, &EventList::empty());
            }
            assert!(
                node.voices.iter().all(|voice| !voice.active),
                "{color:?} still ringing"
            );
        }
    }

    #[test]
    fn the_rate_reducer_applies_to_every_colour() {
        for color in Ds01NoiseColor::ALL {
            let base = Ds01Params {
                tone_level: 0.0,
                noise_level: 1.0,
                noise_color: color,
                filter_morph: 0.0,
                filter_cutoff: 18_000.0,
                ..Ds01Params::default()
            };
            let reduced = Ds01Params {
                noise_rate: 1_000.0,
                ..base
            };
            assert_ne!(
                hit(base, 4_800),
                hit(reduced, 4_800),
                "{color:?} ignored the rate reducer, so its section has an inert control"
            );
        }
    }

    /// Colour is a timbre choice, so switching it must not also be a level
    /// change that has to be undone on the Noise Level knob. White is the
    /// reference; the other three sit within a few dB of it.
    ///
    /// Velvet is excluded and is the exception that proves the rule: it is a
    /// sparse impulse train, so its energy is deliberately a function of its
    /// density rather than of a gain, and matching its RMS to white's would
    /// mean scaling individual impulses past full scale.
    #[test]
    fn noise_colours_share_a_level() {
        let energy = |color: Ds01NoiseColor| {
            let params = Ds01Params {
                tone_level: 0.0,
                noise_level: 1.0,
                noise_color: color,
                // Out of the filter's way: this measures the generators.
                filter_morph: 0.0,
                filter_cutoff: 18_000.0,
                filter_res: 0.0,
                amp_decay: 4.0,
                ..Ds01Params::default()
            };
            rms(&hit(params, 24_000))
        };

        let white = energy(Ds01NoiseColor::White);
        for color in [Ds01NoiseColor::Pink, Ds01NoiseColor::Metal] {
            let db = 20.0 * (energy(color) / white).log10();
            assert!(
                db.abs() < 4.0,
                "{color:?} is {db:.1} dB from white, so changing colour changes level"
            );
        }
    }

    #[test]
    fn velocity_amount_crossfades_between_flat_and_as_played() {
        let flat = Ds01Params {
            velocity_amount: 0.0,
            ..Ds01Params::default()
        };
        let played = Ds01Params::default();
        let soft = |params: Ds01Params| {
            let mut node = Ds01::new(params, SR);
            let mut events = EventList::empty();
            events.push(note_on(0, 60, 40));
            peak(&render(&mut node, 4_800, &events))
        };
        let loud = |params: Ds01Params| {
            let mut node = Ds01::new(params, SR);
            let mut events = EventList::empty();
            events.push(note_on(0, 60, 127));
            peak(&render(&mut node, 4_800, &events))
        };

        assert!((soft(flat) - loud(flat)).abs() < 1.0e-6);
        assert!(soft(played) < loud(played) * 0.5);
    }

    #[test]
    fn note_off_does_not_stop_a_hit() {
        let mut node = Ds01::new(Ds01Params::default(), SR);
        let mut events = EventList::empty();
        events.push(note_on(0, 60, 127));
        events.push(TimedEvent {
            offset: 64,
            event: Event::NoteOff { id: 0, note: 60 },
        });
        let out = render(&mut node, 4_800, &events);
        assert!(node.voices.iter().any(|voice| voice.active));
        assert!(peak(&out[2_400..]) > 0.01);
    }

    #[test]
    fn choke_silences_at_the_choke_time() {
        let mut node = Ds01::new(Ds01Params::default(), SR);
        let mut events = EventList::empty();
        events.push(note_on(0, 60, 127));
        events.push(TimedEvent {
            offset: 100,
            event: Event::Choke,
        });
        let out = render(&mut node, 8_192, &events);
        // The 5 ms default fade is finished well inside 4800 samples.
        assert!(peak(&out[4_800..]) < 0.001);
    }

    #[test]
    fn mono_retrigger_cuts_the_previous_hit_and_poly_does_not() {
        let sounding = |retrigger: Ds01Retrigger| {
            let params = Ds01Params {
                retrigger,
                amp_decay: 2.0,
                ..Ds01Params::default()
            };
            let mut node = Ds01::new(params, SR);
            let mut events = EventList::empty();
            events.push(note_on(0, 60, 127));
            events.push(note_on(2_400, 60, 127));
            render(&mut node, 9_600, &events);
            node.voices.iter().filter(|voice| voice.active).count()
        };
        assert_eq!(sounding(Ds01Retrigger::Poly), 2);
        assert_eq!(sounding(Ds01Retrigger::Mono), 1);
    }

    #[test]
    fn the_voice_pool_steals_the_oldest() {
        let params = Ds01Params {
            amp_decay: 4.0,
            ..Ds01Params::default()
        };
        let mut node = Ds01::new(params, SR);
        for _ in 0..DS01_VOICES + 4 {
            node.trigger(60, 100);
        }
        assert!(node.voices.iter().all(|voice| voice.active));
        let mut ages: Vec<u64> = node.voices.iter().map(|voice| voice.age).collect();
        ages.sort_unstable();
        assert_eq!(ages, (5..=DS01_VOICES as u64 + 4).collect::<Vec<_>>());
    }

    #[test]
    fn transport_stop_chokes_everything() {
        let mut node = Ds01::new(Ds01Params::default(), SR);
        let mut events = EventList::empty();
        events.push(note_on(0, 60, 127));
        render(&mut node, 64, &events);

        let mut stopped = ctx(64);
        stopped.playing = false;
        let mut bus = StereoBus::with_capacity(64);
        node.process(&stopped, &mut bus, &EventList::empty(), None);

        render(&mut node, 8_192, &EventList::empty());
        assert!(node.voices.iter().all(|voice| !voice.active));
    }

    /// Every control combination `sweep_cases` reaches, over one hit: the
    /// device output stays finite and under full scale. The bound is DS-01's
    /// own — [`device_bound`] — rather than a master limiter's, which is why
    /// this is asserted at the device rather than downstream.
    #[test]
    fn no_control_combination_pushes_a_hit_past_full_scale() {
        for params in sweep_cases() {
            let mut node = Ds01::new(params, SR);
            let mut events = EventList::empty();
            events.push(note_on(0, 60, 127));
            let out = render(&mut node, 12_000, &events);
            assert!(
                out.iter().all(|s| s.is_finite()),
                "non-finite sample from {params:?}"
            );
            assert!(peak(&out) <= 1.0, "peaked at {} from {params:?}", peak(&out));
        }
    }

    /// The same sweep with the whole pool sounding at once. Eight
    /// simultaneous full-velocity hits are polyphony rather than a control
    /// combination, and the bound holds there too — it is applied to the
    /// device's output, so it does not need to know how many voices made it.
    #[test]
    fn a_full_voice_pool_stays_bounded() {
        for params in sweep_cases() {
            let mut node = Ds01::new(params, SR);
            let mut events = EventList::empty();
            for voice in 0..DS01_VOICES as u32 {
                events.push_ordered(note_on(voice * 16, 36 + voice as u8 * 7, 127));
            }
            let out = render(&mut node, 12_000, &events);
            assert!(
                out.iter().all(|s| s.is_finite()),
                "non-finite sample from {params:?}"
            );
            assert!(peak(&out) <= 1.0, "peaked at {} from {params:?}", peak(&out));
        }
    }

    /// Every control at both ends of its range, one at a time, plus the
    /// combination that actually stacks.
    fn sweep_cases() -> Vec<Ds01Params> {
        let mut cases: Vec<Ds01Params> = vec![Ds01Params::default()];
        for descriptor in ds01::DESCRIPTORS.iter() {
            for end in [descriptor.min, descriptor.max] {
                let mut params = Ds01Params::default();
                ds01::set(&mut params, descriptor.id, descriptor.clamp_natural(end));
                cases.push(params);
            }
        }
        // Everything loud at once, which is the combination that actually
        // stacks: three sources, no filtering, the longest tails.
        let mut everything = Ds01Params::default();
        for (id, value) in [
            (ds01::PARAM_LEVEL, 1.0),
            (ds01::PARAM_TONE_LEVEL, 1.0),
            (ds01::PARAM_NOISE_LEVEL, 1.0),
            (ds01::PARAM_TONE_PARTIALS, 6.0),
            (ds01::PARAM_TONE_SPREAD, 1.0),
            (ds01::PARAM_TONE_FM_AMOUNT, 1.0),
            (ds01::PARAM_FILTER_RES, 1.0),
            (ds01::PARAM_FILTER_MORPH, 0.5),
            (ds01::PARAM_AMP_DECAY, 4.0),
        ] {
            ds01::set(&mut everything, id, value);
        }
        cases.push(everything);
        cases
    }

    #[test]
    fn preview_renders_through_the_production_voice() {
        let plain = Ds01::preview_waveform(Ds01Params::default(), 96);
        let altered = Ds01::preview_waveform(
            Ds01Params {
                tone_pitch: 700.0,
                tone_wave: 1.0,
                noise_level: 0.8,
                ..Ds01Params::default()
            },
            96,
        );
        assert_eq!(plain.0.len(), 96);
        assert_ne!(plain, altered);
        assert!(plain
            .0
            .iter()
            .chain(&plain.1)
            .all(|sample| (-1.0..=1.0).contains(sample)));
    }
}
