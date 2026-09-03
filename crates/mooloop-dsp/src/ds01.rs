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
use crate::env::{shape, Ahd, AhdShape, DECAY_TAIL_CONSTANTS};
use crate::event::{Event, EventList};
use crate::filter::{apply_drive, soft_ceiling, OnePoleHp, Svf};
use crate::modulator::CONTROL_RATE_FRAMES;
use crate::node::{AudioNode, ProcessContext};
use crate::osc::{Noise, Osc};
use crate::shaper;
use crate::smooth::Smoothed;
use mooloop_core::{
    body_mode_ratio, ds01, Ds01Character, Ds01EnvParams, Ds01ModSource, Ds01NoiseColor,
    Ds01Params, Ds01Retrigger, DriveCurve, OscWave, ParamDescriptor,
    DS01_BITS_TRANSPARENT, DS01_BODY_MODES, DS01_BURST_MAX_S, DS01_MATRIX_ROWS,
    DS01_MAX_PARTIALS, DS01_MAX_REPEATS, DS01_VOICES, MAX_CHOKE_GROUP,
};

/// One DS-01 envelope block as the shared envelope's shape.
fn ahd_shape(env: &Ds01EnvParams) -> AhdShape {
    AhdShape {
        attack_s: env.attack,
        hold_s: env.hold,
        decay_s: env.decay,
        curve: env.curve,
        sustain: env.sustain,
        release_s: env.release,
        gate: env.gate,
    }
}

/// The device's output reference, and the whole of DS-01's gain contract.
///
/// v1 has one `OUTPUT_REFERENCE` constant doing three jobs at once — a mix
/// decision, a character control, and a safety bound. Here they are three
/// things: `PARAM_LEVEL` is the mix decision, the shape stage is the
/// character, [`device_bound`] is the safety bound, and this is only the
/// reference they are all measured against.
///
/// The contract, under `docs/GAIN_STRUCTURE.md`:
///
/// - **One layer at its default level with Drive at 0 is the device
///   reference.** The default patch — the tone layer at 1.0, the device Level
///   at 0.8, full velocity, a shaper that is exactly transparent — peaks
///   within a dB of `gain::GENERATOR_OUTPUT_REFERENCE_DBFS`, which is v1's own
///   calibration, so a v1 kick and a DS-01 kick sit in the same place in a
///   mix.
/// - **Adding the noise or the body layer does not turn the tone down.** They
///   are separate summed layers with their own levels, not a crossfade.
/// - **Drive is compensated but not normalized.** Raising it changes timbre
///   substantially more than it changes level, and it is still allowed to
///   make a hit louder, because that is what drive does.
///
/// `the_default_patch_peaks_at_the_generator_reference` holds the first,
/// `adding_a_layer_does_not_turn_the_tone_down` the second, and
/// `drive_changes_timbre_more_than_level` the third.
const VOICE_OUTPUT_REFERENCE: f32 = 0.4444;

/// Input gain at full Drive. Shared with `filter::apply_drive`, which is the
/// Soft character, so all four characters reach the same distance.
const DRIVE_GAIN_RANGE: f32 = 15.0;

/// How far Bias offsets the signal before the nonlinearity. Enough that the
/// top of the control pushes a quiet tail entirely onto one side of the
/// curve, which is the gating and spitting the step asks for.
const BIAS_DEPTH: f32 = 0.6;

/// How much of the negative half the Crush character keeps.
const CRUSH_NEGATIVE: f32 = 0.15;

/// The quantization Crush applies on its own, before the Bits control gets
/// its turn. Four bits: this is the damaged one.
const CRUSH_STEP: f32 = 1.0 / 16.0;

/// Quantization step for a bit depth, or `0` where the reducer is exactly
/// transparent.
///
/// Exactly rather than nearly: the default patch has to reach the gain
/// reference through a shaper doing nothing at all, and `(x * 32768).round()
/// / 32768` is not `x`.
fn bit_step(bits: f32) -> f32 {
    if bits >= DS01_BITS_TRANSPARENT {
        0.0
    } else {
        1.0 / 2.0_f32.powf(bits.max(1.0) - 1.0)
    }
}

fn quantize(input: f32, step: f32) -> f32 {
    if step <= 0.0 {
        input
    } else {
        (input / step).round() * step
    }
}

/// The shape stage: drive with a selectable character, an asymmetry, and a
/// bit reducer.
///
/// It sits **after** the amplitude envelope rather than before it, which is
/// where `01-what-ds01-is.md`'s first sketch of the signal path put it. That
/// diagram has been corrected. The reason is the property step 06 asks Fold
/// to have: folding is a function of instantaneous amplitude, so the shape of
/// a hit changes across its own decay *for free* — but only if the decay has
/// already happened by the time the signal reaches the folder. Ahead of the
/// envelope, a tone-only patch presents the shaper with a constant amplitude
/// and every hit folds identically.
///
/// The same choice is what makes velocity reach the colour: a harder hit is a
/// hotter signal into the nonlinearity, which is the "colour that reacts to
/// level, timing and the source" the taste brief asks for rather than a fixed
/// percentage of an effect.
fn shape_stage(
    input: f32,
    drive: f32,
    character: Ds01Character,
    bias: f32,
    step: f32,
) -> f32 {
    // Bias pushes the signal onto an uneven part of the curve, which is what
    // produces the even harmonics. Its own DC is left in and taken out by the
    // output high-pass rather than subtracted back out here, because at the
    // top of the range the offset *is* the effect.
    let biased = input + bias * BIAS_DEPTH;
    let gain = 1.0 + drive.clamp(0.0, 1.0) * DRIVE_GAIN_RANGE;
    let driven = match character {
        // v1's curve, called rather than re-derived, so "reproduces v1's
        // drive curve" is an identity instead of a tolerance.
        Ds01Character::Soft => apply_drive(biased, drive),
        Ds01Character::Hard => {
            shaper::shape(DriveCurve::Hard, biased * gain)
                * shaper::drive_compensation(DriveCurve::Hard, gain)
        }
        Ds01Character::Fold => {
            shaper::shape(DriveCurve::Fold, biased * gain)
                * shaper::drive_compensation(DriveCurve::Fold, gain)
        }
        Ds01Character::Crush => {
            let x = biased * gain;
            // Asymmetric rather than full-wave rectification: the negative
            // half is nearly gone, so the spectrum fills with even harmonics
            // and the fundamental partly doubles, and zero in is still zero
            // out. A full-wave rectifier leaves a DC pedestal under silence,
            // and the high-pass removing it afterwards is not the same thing
            // as it never being there.
            let rectified = if x >= 0.0 {
                x.tanh()
            } else {
                x.tanh() * CRUSH_NEGATIVE
            };
            quantize(
                rectified * shaper::drive_compensation(DriveCurve::Soft, gain),
                CRUSH_STEP,
            )
        }
    };
    quantize(driven, step)
}

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

/// Roughly how many samples a preview render costs, whatever span it covers.
///
/// The rate is derived from this and the span rather than fixed, so a long
/// patch does not make the display expensive in proportion to its tail.
const PREVIEW_SAMPLES: usize = 16_384;

/// The node's noise seed, and the seed the per-hit random is derived from.
const SEED: u32 = 0x9E37_79B9;

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

/// How much Damping shortens a mode, per octave-ish of separation from the
/// fundamental. At full damping the membrane's top mode rings roughly twenty
/// times shorter than its fundamental, which is the woodblock end of the
/// control; at zero every mode rings for the stated decay, which is the bell.
const DAMPING_DEPTH: f32 = 4.0;

/// What each mode contributes to the layer. Upper modes carry less, as they
/// do in a struck object, so Ratio changes the material rather than the
/// balance.
const BODY_MODE_WEIGHT: [f32; DS01_BODY_MODES] = [1.0, 0.6, 0.4];

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
    body_pitch: f32,
    body_ratio: f32,
    body_decay: f32,
    body_damping: f32,
    body_excite: f32,
    drive: f32,
    character: Ds01Character,
    bias: f32,
    bit_step: f32,
    output_hp: f32,
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
            body_pitch: p.body_pitch.max(1.0),
            body_ratio: p.body_ratio.clamp(0.0, 1.0),
            body_decay: p.body_decay.max(0.001),
            body_damping: p.body_damping.clamp(0.0, 1.0),
            body_excite: p.body_excite.clamp(0.0, 1.0),
            drive: p.drive.clamp(0.0, 1.0),
            character: p.character,
            bias: p.bias.clamp(0.0, 1.0),
            bit_step: bit_step(p.bits),
            output_hp: p.output_hp.clamp(5.0, 2_000.0),
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

/// One tuned mode: a two-pole resonator, `y[n] = x[n] + a1 y[n-1] + a2
/// y[n-2]`, whose poles sit at `r e^{±jw}`.
///
/// Not a biquad band-pass, and the difference is the point. A band-pass is
/// parameterized by Q, so the same Q is a different ring time at every pitch
/// and "Body Decay" would stop being a time. Here the pole radius comes
/// straight from the decay in seconds, so a mode at 60 Hz and a mode at 6 kHz
/// ring for exactly as long as each other — which is what
/// `body_decay_is_a_time_at_every_pitch` measures.
///
/// The two input gains are derived rather than tuned, because the same
/// resonator has to be struck and to be driven:
///
/// - a strike is an impulse, whose response peaks at `1 / sin(w)`, so a
///   strike is scaled by `sin(w)` and rings at about unity at every pitch;
/// - a continuous excitation accumulates, with an output RMS of about
///   `1 / (sin(w) * sqrt(2 (1 - r^2)))` for unit-variance input, so it is
///   scaled by the inverse of that. Without it, an eight-second ring would be
///   forty decibels louder than a short one for the same noise going in.
#[derive(Clone, Copy, Debug, Default)]
struct Resonator {
    a1: f32,
    a2: f32,
    strike_gain: f32,
    excite_gain: f32,
    y1: f32,
    y2: f32,
}

impl Resonator {
    /// Recompute this mode's coefficients. Called on the control tick, never
    /// per sample.
    fn set(&mut self, freq_hz: f32, decay_s: f32, sample_rate: u32) {
        let sr = sample_rate as f32;
        let w = core::f32::consts::TAU * freq_hz / sr;
        let r = (-DECAY_TAIL_CONSTANTS / (decay_s.max(0.001) * sr)).exp();
        self.a1 = 2.0 * r * w.cos();
        self.a2 = -(r * r);
        let sin_w = w.sin().abs().max(1.0e-4);
        self.strike_gain = sin_w;
        self.excite_gain = sin_w * (2.0 * (1.0 - r * r)).max(0.0).sqrt();
    }

    fn tick(&mut self, input: f32) -> f32 {
        let out = input + self.a1 * self.y1 + self.a2 * self.y2;
        self.y2 = self.y1;
        self.y1 = out;
        out
    }

    fn reset(&mut self) {
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/// Three tuned resonators in parallel, struck by an impulse, by the noise
/// layer, or by a crossfade of the two.
#[derive(Clone, Copy, Debug)]
struct Body {
    modes: [Resonator; DS01_BODY_MODES],
    /// Per-mode mute, smoothed. A mode above Nyquist is silenced rather than
    /// folded down into an alias that does not move with the pitch, and the
    /// silencing is a ramp so a sweep through the top of the range is not a
    /// click.
    audible: [Smoothed; DS01_BODY_MODES],
    /// Samples of impulse left to inject. Step 05's burst is what re-arms
    /// this mid-hit.
    impulse: u32,
    /// How hard the pending impulse strikes, which is the burst's own level
    /// for the impulse that armed it.
    strike_level: f32,
}

impl Body {
    fn new(sample_rate: u32) -> Self {
        Self {
            modes: [Resonator::default(); DS01_BODY_MODES],
            audible: [Smoothed::new(0.0, SMOOTHING_S, sample_rate); DS01_BODY_MODES],
            impulse: 0,
            strike_level: 1.0,
        }
    }

    fn reset(&mut self) {
        for mode in &mut self.modes {
            mode.reset();
        }
        self.impulse = 0;
        self.strike_level = 1.0;
    }

    /// Arm a strike of `level`. Deliberately does not clear the resonators:
    /// the object being struck may still be ringing.
    fn strike(&mut self, level: f32) {
        self.impulse = 1;
        self.strike_level = level;
    }

    /// Recompute every mode for this control tick.
    fn prepare(&mut self, fundamental_hz: f32, c: &Continuous, sample_rate: u32) {
        let nyquist = sample_rate as f32 * 0.45;
        for (index, mode) in self.modes.iter_mut().enumerate() {
            let ratio = body_mode_ratio(index, c.body_ratio);
            let freq = fundamental_hz * ratio;
            // Damping is high-frequency loss, so it is a function of how far
            // a mode sits above the fundamental rather than of its index:
            // at Ratio 0 the modes are closer together and damping bites
            // less, which is why a harmonic body stays a drum while a
            // membrane turns into a woodblock.
            let decay = c.body_decay / (1.0 + c.body_damping * DAMPING_DEPTH * (ratio - 1.0));
            mode.set(freq, decay, sample_rate);
            self.audible[index].set_target(if freq < nyquist { 1.0 } else { 0.0 });
        }
    }

    fn tick(&mut self, excite: f32, noise: f32) -> f32 {
        let strike = if self.impulse > 0 {
            self.impulse -= 1;
            self.strike_level
        } else {
            0.0
        };
        let mut sum = 0.0;
        for (index, mode) in self.modes.iter_mut().enumerate() {
            let input = strike * mode.strike_gain * (1.0 - excite)
                + noise * mode.excite_gain * excite;
            sum += mode.tick(input) * BODY_MODE_WEIGHT[index] * self.audible[index].advance();
        }
        sum
    }
}

/// How far Spread bends the gaps, in octaves of gap ratio at each end. At
/// `-1` every gap is half the one before it and at `+1` twice, which covers
/// the buzz roll and the drag without the schedule needing a second control.
const SPREAD_OCTAVES: f32 = 1.0;

/// How far Level Step moves each impulse, in octaves of amplitude ratio.
const LEVEL_STEP_OCTAVES: f32 = 1.0;

/// The eight source values one voice presents to the matrix.
///
/// Four are latched at the hit and four are live. Kept as one struct so
/// "which sources exist" is a single place, and so a route can read one
/// without the matrix knowing which kind it is.
#[derive(Clone, Copy, Debug, Default)]
struct Sources {
    velocity: f32,
    note: f32,
    amp_env: f32,
    noise_env: f32,
    mod_env: f32,
    burst_index: f32,
    alternator: f32,
    random: f32,
}

impl Sources {
    fn get(&self, source: Ds01ModSource) -> f32 {
        match source {
            Ds01ModSource::None => 0.0,
            Ds01ModSource::Velocity => self.velocity,
            Ds01ModSource::Note => self.note,
            Ds01ModSource::AmpEnv => self.amp_env,
            Ds01ModSource::NoiseEnv => self.noise_env,
            Ds01ModSource::ModEnv => self.mod_env,
            Ds01ModSource::BurstIndex => self.burst_index,
            Ds01ModSource::HitAlternator => self.alternator,
            Ds01ModSource::Random => self.random,
        }
    }
}

/// The destination descriptors, resolved once when the parameters change so
/// the audio path never searches a table.
type MatrixDests = [Option<&'static ParamDescriptor>; DS01_MATRIX_ROWS];

/// Shape a route's source before it is scaled. `0` is the identity.
///
/// Deliberately *not* [`shape`]'s own neutral, which is exponential — the
/// right answer for an envelope, where curve 0 has to be v1's decay law, and
/// the wrong one for a route, where the middle of a bipolar control has to
/// mean "no shaping". Reusing it directly makes a route at its default curve
/// deliver almost nothing until its source is near the top, which looks like
/// a dead route rather than a shaped one.
///
/// The two ends are the same two shapes an envelope has, so a curve means the
/// same thing to a musician in both places even though its middle does not.
fn route_shape(position: f32, curve: f32) -> f32 {
    let linear = position.clamp(0.0, 1.0);
    let curve = curve.clamp(-1.0, 1.0);
    let toward = if curve >= 0.0 {
        shape(linear, 0.0)
    } else {
        shape(linear, -1.0)
    };
    linear + (toward - linear) * curve.abs()
}

/// Apply the rows whose destination's latching matches `latched` into a copy
/// of `base`.
///
/// Routes add an offset in *normalized* destination space around the base
/// value, exactly as a channel route does — never an absolute write — so a
/// knob and a route compose rather than fight. Rows accumulate, because each
/// reads the value the ones before it left; addition in normalized space
/// means the result does not depend on the order they are in.
///
/// A route to a latched destination is evaluated at the trigger and one to a
/// continuous destination every control tick. That is not a separate rule: it
/// falls straight out of `01-what-ds01-is.md`'s two tables, which
/// `ds01::is_latched` is.
fn apply_matrix(
    base: &Ds01Params,
    dests: &MatrixDests,
    sources: &Sources,
    latched: bool,
) -> Ds01Params {
    let mut out = *base;
    for (route, dest) in base.matrix.iter().zip(dests.iter()) {
        let Some(dest) = dest else { continue };
        if !route.is_active() || ds01::is_latched(dest.id) != latched {
            continue;
        }
        let value = sources.get(route.source);
        // The curve shapes the source before it is scaled. A bipolar source
        // is shaped by magnitude and keeps its sign, so a curve bends both
        // halves the same way instead of turning one of them inside out.
        let shaped = if route.source.is_bipolar() {
            route_shape(value.abs(), route.curve) * value.signum()
        } else {
            route_shape(value, route.curve)
        };
        let Some(current) = ds01::get(&out, dest.id) else {
            continue;
        };
        let moved = dest.from_normalized(dest.to_normalized(current) + route.amount * shaped);
        ds01::set(&mut out, dest.id, moved);
    }
    out
}

/// One deterministic value per hit, bipolar.
///
/// A function of the node seed and the hit counter and nothing else, so an
/// offline render and a live take of the same event stream produce identical
/// samples. That is the property that separates this from a humanize control.
fn hit_random(seed: u32, hit: u64) -> f32 {
    let mut x = seed ^ (hit as u32).wrapping_mul(0x9E37_79B9);
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    (x >> 8) as f32 / 8_388_608.0 - 1.0
}

/// One trigger's impulse schedule, latched at the hit.
///
/// One voice, not one voice per impulse, for three reasons the plan names: an
/// eight-repeat burst does not consume the whole pool; the body resonator
/// keeps ringing *across* the impulses instead of being restarted, which is
/// what makes a burst a clap rather than four claps; and the impulse index is
/// available as a per-impulse modulation source, which step 07 publishes as
/// Burst Index.
#[derive(Clone, Copy, Debug, Default)]
struct Burst {
    /// Impulses still to fire after the current one.
    remaining: u8,
    /// Samples until the next impulse.
    countdown: u32,
    /// Gap to the next impulse, in samples, before this impulse's scaling.
    gap: f32,
    /// What the gap is multiplied by at each impulse. Below one accelerates.
    gap_ratio: f32,
    /// Amplitude of the impulse now sounding.
    level: f32,
    level_ratio: f32,
    /// Pitch offset of the impulse now sounding, in semitones.
    pitch: f32,
    pitch_step: f32,
    /// Which impulse is sounding, and how many there are, for Burst Index.
    index: u8,
    /// Read by [`Self::position`], which step 07 routes. Kept here rather
    /// than added there because the schedule is what knows it.
    #[allow(dead_code)]
    total: u8,
    /// Samples the schedule has committed to so far, against
    /// [`DS01_BURST_MAX_S`].
    elapsed: f32,
}

impl Burst {
    fn start(&mut self, params: &Ds01Params, sample_rate: u32) {
        let total = params.burst_repeats.clamp(1, DS01_MAX_REPEATS);
        let gap_ratio = 2.0_f32.powf(params.burst_spread.clamp(-1.0, 1.0) * SPREAD_OCTAVES);
        let level_ratio =
            2.0_f32.powf(params.burst_level_step.clamp(-1.0, 1.0) * LEVEL_STEP_OCTAVES);
        // Normalized so the loudest impulse is the reference one, whichever
        // it is. A negative step then fades from a full first hit, and a
        // positive one builds *to* a full last hit rather than past it — so
        // Level Step shapes a burst without making it louder than the single
        // hit it replaces.
        let loudest = level_ratio.powi(i32::from(total) - 1).max(1.0);

        *self = Self {
            remaining: total - 1,
            countdown: (params.burst_spacing.max(0.0) * sample_rate as f32) as u32,
            gap: params.burst_spacing.max(0.0) * sample_rate as f32,
            gap_ratio,
            level: 1.0 / loudest,
            level_ratio,
            pitch: 0.0,
            pitch_step: params.burst_pitch_step.clamp(-24.0, 24.0),
            index: 0,
            total,
            elapsed: params.burst_spacing.max(0.0) * sample_rate as f32,
        };
    }

    /// Advance the schedule one sample. `true` when this sample is an impulse.
    fn tick(&mut self, sample_rate: u32) -> bool {
        if self.remaining == 0 {
            return false;
        }
        if self.countdown > 0 {
            self.countdown -= 1;
            return false;
        }
        self.remaining -= 1;
        self.index += 1;
        self.level *= self.level_ratio;
        self.pitch += self.pitch_step;
        self.gap *= self.gap_ratio;
        // The schedule may shape a hit; it may not extend a voice's lifetime
        // indefinitely. A decelerating eight-impulse burst at the top of
        // Spacing would otherwise place its last hit a minute after its
        // first, so the burst ends at the impulse that would take the whole
        // schedule past the bound. The bound is on the total rather than on
        // one gap, because eight legal gaps still add up.
        if self.elapsed + self.gap > DS01_BURST_MAX_S * sample_rate as f32 {
            self.remaining = 0;
        } else {
            self.elapsed += self.gap;
        }
        self.countdown = self.gap as u32;
        true
    }

    /// Where this impulse sits in the burst, `0` at the first and `1` at the
    /// last. Constant within an impulse, and `0` throughout at Repeats 1.
    ///
    /// Step 07 publishes this as the Burst Index matrix source. It is written
    /// and tested here because the schedule is what knows it, and because the
    /// contract — what a burst's *shape* looks like to a route — belongs
    /// beside the schedule that produces it rather than beside the matrix
    /// that reads it.
    #[allow(dead_code)]
    fn position(&self) -> f32 {
        if self.total <= 1 {
            0.0
        } else {
            f32::from(self.index) / f32::from(self.total - 1)
        }
    }
}

/// The three envelope shapes a burst's later impulses re-fire.
///
/// Latched with the rest of the schedule: an envelope time that moved
/// mid-burst would give one hit two shapes, which is the same reason
/// `01-what-ds01-is.md` latches them at the trigger in the first place.
#[derive(Clone, Copy, Debug, Default)]
struct BurstShapes {
    amp: AhdShape,
    noise: AhdShape,
    pitch: AhdShape,
}

/// One independently enveloped hit.
struct Voice {
    active: bool,
    age: u64,
    seed: u32,
    /// The note event this hit belongs to, so a note-off reaches the voice it
    /// started rather than every voice playing the same note.
    event_id: u64,
    /// Whether this hit is still waiting on a note-off. True only while a
    /// gated envelope is running, and it is what keeps a written ride from
    /// being stolen halfway through the note that asked for it.
    gate_held: bool,
    latched: Latched,
    amp_env: Ahd,
    pitch_env: Ahd,
    /// The noise layer's own contour, so a snare's rattle can outlive its
    /// shell tone or stop before it.
    noise_env: Ahd,
    /// The one with no other job. Step 07 publishes it as a matrix source;
    /// it runs now so that when it does, its history is the same one the
    /// other three have had.
    mod_env: Ahd,
    /// One oscillator per partial. Unused partials are never stepped.
    tone: [Osc; DS01_MAX_PARTIALS as usize],
    fm: Osc,
    noise: NoiseSource,
    filter: Svf,
    body: Body,
    burst: Burst,
    /// The shapes the burst re-fires the envelopes with, latched at the hit
    /// alongside everything else the schedule decided.
    shapes: BurstShapes,
    /// What this voice presents to the matrix. The latched four are set at
    /// the trigger; the live four are refreshed each control tick.
    sources: Sources,
    /// Rate-reducer state: the held sample and the fraction of a held period
    /// elapsed.
    held_noise: f32,
    hold_phase: f32,
}

impl Voice {
    fn new(seed: u32, sample_rate: u32) -> Self {
        Self {
            active: false,
            age: 0,
            seed,
            event_id: 0,
            gate_held: false,
            latched: Latched::default(),
            amp_env: Ahd::new(),
            pitch_env: Ahd::new(),
            noise_env: Ahd::new(),
            mod_env: Ahd::new(),
            tone: [Osc::new(); DS01_MAX_PARTIALS as usize],
            fm: Osc::new(),
            noise: NoiseSource::new(seed),
            filter: Svf::new(),
            body: Body::new(sample_rate),
            burst: Burst::default(),
            shapes: BurstShapes::default(),
            sources: Sources::default(),
            held_noise: 0.0,
            hold_phase: 1.0,
        }
    }

    fn reset(&mut self) {
        self.active = false;
        self.age = 0;
        self.event_id = 0;
        self.gate_held = false;
        self.latched = Latched::default();
        self.amp_env.reset();
        self.pitch_env.reset();
        self.noise_env.reset();
        self.mod_env.reset();
        for osc in &mut self.tone {
            osc.reset();
        }
        self.fm.reset();
        self.noise.reset(self.seed);
        self.filter.reset();
        self.body.reset();
        self.burst = Burst::default();
        self.sources = Sources::default();
        self.held_noise = 0.0;
        self.hold_phase = 1.0;
    }

    /// Start a hit. Every value this reads from `params` is in [`Latched`];
    /// nothing continuous is captured here.
    fn trigger(
        &mut self,
        params: &Ds01Params,
        event_id: u64,
        note: u8,
        velocity: u8,
        sample_rate: u32,
    ) {
        let semitones = f32::from(note.min(127)) - 60.0 + params.tune.clamp(-48.0, 48.0);
        let velocity_unit = f32::from(velocity) / 127.0;
        let amount = params.velocity_amount.clamp(0.0, 1.0);
        self.latched = Latched {
            pitch_factor: 2.0_f32.powf(semitones / 12.0),
            // A crossfade rather than a multiply: at 0 every hit is equally
            // loud, at 1 it follows the note as played. v1 could only do the
            // latter.
            velocity_amp: 1.0 - amount + amount * velocity_unit,
            pitch_depth: params.pitch.depth.clamp(-60.0, 60.0),
            partials: params.tone_partials.clamp(1, DS01_MAX_PARTIALS),
        };
        self.active = true;
        self.event_id = event_id;
        self.gate_held =
            params.amp.gate || params.noise_env.gate || params.mod_env.gate;
        for osc in &mut self.tone {
            osc.reset();
        }
        self.fm.reset();
        self.filter.reset();
        self.hold_phase = 1.0;
        // The resonators are *not* cleared: a hit strikes an object that may
        // still be ringing, which is what makes a fast pattern on a bell
        // build rather than restart. The burst leans on the same property
        // across the impulses of one hit.
        self.body.strike(1.0);
        // The pitch envelope has no gate half, so hold, sustain and release
        // are not controls it has rather than controls it ignores.
        self.shapes = BurstShapes {
            amp: ahd_shape(&params.amp),
            noise: ahd_shape(&params.noise_env),
            pitch: AhdShape {
                attack_s: params.pitch.attack,
                hold_s: 0.0,
                decay_s: params.pitch.decay,
                curve: params.pitch.curve,
                sustain: 0.0,
                release_s: 0.0,
                gate: false,
            },
        };
        self.amp_env.trigger(self.shapes.amp, sample_rate);
        self.noise_env.trigger(self.shapes.noise, sample_rate);
        self.mod_env.trigger(ahd_shape(&params.mod_env), sample_rate);
        self.pitch_env.trigger(self.shapes.pitch, sample_rate);
        self.burst.start(params, sample_rate);
    }

    /// Re-read the four live sources. Called once per control tick, beside
    /// the body's coefficients, because both are things the voice knows and
    /// the tick is when the matrix asks.
    fn refresh_sources(&mut self) {
        self.sources.amp_env = self.amp_env.level();
        self.sources.noise_env = self.noise_env.level();
        self.sources.mod_env = self.mod_env.level();
        self.sources.burst_index = self.burst.position();
    }

    /// Whether this hit still has impulses to fire. A voice is not free while
    /// it does, even if its amplitude envelope has run out between them.
    fn burst_pending(&self) -> bool {
        self.burst.remaining > 0
    }

    /// Note-off. Only the gated envelopes hear it; a one-shot envelope in the
    /// same patch keeps ignoring it, which is what keeps a drum a drum.
    fn note_off(&mut self) {
        self.amp_env.release();
        self.noise_env.release();
        self.mod_env.release();
        self.gate_held = false;
    }

    /// Fade this hit out over `seconds`, as a release on the amplitude
    /// envelope rather than as a coefficient stamped over it, so step 03's
    /// envelope shapes do not have to special-case a choke.
    fn choke(&mut self, seconds: f32, sample_rate: u32) {
        if !self.active {
            return;
        }
        // The VCA is what a choke has to move, so it is the only envelope
        // this touches: the other three are inside it.
        self.amp_env.release_over(seconds, sample_rate);
        self.gate_held = false;
    }

    /// One sample of this voice, pre-level. Mono by design; the channel strip
    /// places it in the stereo field, as it does for every other generator.
    fn render_sample(
        &mut self,
        c: &Continuous,
        tone_level: f32,
        noise_level: f32,
        body_level: f32,
        sample_rate: u32,
    ) -> f32 {
        // The schedule runs first, so an impulse's own sample is the one the
        // re-fired envelopes produce rather than the one after it. The mod
        // envelope is deliberately not re-fired: it is the contour with no
        // fixed job, and a burst-wide shape is more use than eight copies of
        // a short one.
        if self.burst.tick(sample_rate) {
            self.amp_env.retrigger(self.shapes.amp, sample_rate);
            self.noise_env.retrigger(self.shapes.noise, sample_rate);
            self.pitch_env.retrigger(self.shapes.pitch, sample_rate);
            // Struck again without clearing: the resonators ring *across* the
            // impulses, which is what makes a burst a clap rather than four
            // claps.
            self.body.strike(self.burst.level);
        }

        let amp = self.amp_env.tick();
        let pitch = self.pitch_env.tick();
        let noise_contour = self.noise_env.tick();
        // Reaches nothing until step 07 routes it. Running it now is what
        // makes it the same envelope by then, rather than one that starts
        // its life already special-cased.
        self.mod_env.tick();

        let swept = c.tone_pitch
            * self.latched.pitch_factor
            * 2.0_f32.powf((self.latched.pitch_depth * pitch + self.burst.pitch) / 12.0);

        // Both layers run whatever their level is. A layer at zero is a mix
        // decision and not a mode: it keeps its phase, so a level returning
        // from zero resumes rather than restarting, and step 04's body
        // excitation reads the same pre-level tap.
        let tone = self.render_tone(c, swept, sample_rate);

        let noise = self.render_noise(c, swept, sample_rate);
        // The burst's level is a *strike force*, so it scales what each
        // impulse excites rather than the voice's output. Scaling the output
        // would step the body's ring every time a later impulse changed the
        // level, which is a click in a tail that is supposed to carry across
        // the burst.
        let struck = self.burst.level;
        // The body is driven by the noise layer's *pre-level* signal, per
        // `01-what-ds01-is.md`: a layer at zero level still exists at its
        // pre-level tap, so a patch can drive the resonators with noise
        // nobody hears directly.
        let body = self.body.tick(c.body_excite, noise * struck);

        // The voice's own guard, before any level scales it: a state-variable
        // filter at full resonance is linear and will happily hand back
        // several times full scale, which is what this catches. The device
        // bound below is a different job and is sized differently.
        // The voice's own guard, before the shaper: a state-variable filter
        // at full resonance is linear and will happily hand back several
        // times full scale, and the shaper should be given a signal rather
        // than an explosion. The device bound later is a different job and is
        // sized differently.
        let vca = soft_ceiling(
            ((tone * tone_level + noise * noise_contour * noise_level) * struck
                + body * body_level)
                * amp
                * self.latched.velocity_amp,
        );
        shape_stage(vca, c.drive, c.character, c.bias, c.bit_step)
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

/// Resolve every row's destination descriptor. Off the audio path: a matrix
/// edit is a structural change, and the search belongs with it.
fn resolve_dests(params: &Ds01Params) -> MatrixDests {
    std::array::from_fn(|row| ds01::descriptor(params.matrix[row].dest))
}

/// The DS-01 node.
pub struct Ds01 {
    params: Ds01Params,
    sample_rate: u32,
    voices: [Voice; DS01_VOICES],
    next_age: u64,
    tone_level: Smoothed,
    noise_level: Smoothed,
    body_level: Smoothed,
    level: Smoothed,
    /// One high-pass for the device, not one per voice: it is the output
    /// stage, and the DC that Bias creates sums like everything else.
    output_hp: OnePoleHp,
    /// Every row's destination descriptor, resolved when the parameters
    /// change so the audio path never searches the table.
    matrix_dests: MatrixDests,
    /// Hits this node has played. Drives the alternator and the per-hit
    /// random, and is therefore what makes both a function of the event
    /// stream rather than of the wall clock.
    hit_count: u64,
    /// One resolved control set per voice, because the matrix is per voice:
    /// two hits at different velocities have to be able to disagree about
    /// where the filter is.
    voice_continuous: [Continuous; DS01_VOICES],
}

impl Ds01 {
    pub fn new(mut params: Ds01Params, sample_rate: u32) -> Self {
        params.choke_group = params.choke_group.min(MAX_CHOKE_GROUP);
        Self {
            params,
            sample_rate,
            voices: std::array::from_fn(|index| {
                Voice::new(SEED.wrapping_add(index as u32), sample_rate)
            }),
            next_age: 1,
            tone_level: Smoothed::new(params.tone_level, SMOOTHING_S, sample_rate),
            noise_level: Smoothed::new(params.noise_level, SMOOTHING_S, sample_rate),
            body_level: Smoothed::new(params.body_level, SMOOTHING_S, sample_rate),
            level: Smoothed::new(params.level, SMOOTHING_S, sample_rate),
            output_hp: OnePoleHp::new(),
            matrix_dests: resolve_dests(&params),
            hit_count: 0,
            voice_continuous: [Continuous::new(&params); DS01_VOICES],
        }
    }

    /// Replace the parameter set. Called from the RT command drain.
    pub fn set_params(&mut self, mut params: Ds01Params) {
        params.choke_group = params.choke_group.min(MAX_CHOKE_GROUP);
        self.params = params;
        self.matrix_dests = resolve_dests(&params);
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
        self.hit_count = 0;
        self.tone_level.reset_to(self.params.tone_level);
        self.noise_level.reset_to(self.params.noise_level);
        self.body_level.reset_to(self.params.body_level);
        self.level.reset_to(self.params.level);
        self.output_hp.reset();
    }

    pub fn choke(&mut self) {
        let seconds = self.params.choke_time;
        let sr = self.sample_rate;
        for voice in &mut self.voices {
            voice.choke(seconds, sr);
        }
    }

    /// A free slot, then the oldest hit that is not being held open, and only
    /// then the oldest hit at all.
    ///
    /// The middle rule is what step 03's gate mode costs: a ride written to
    /// ring for two bars must not be taken by the hat pattern underneath it.
    /// It gives way when the pool is genuinely exhausted, because a device
    /// that refuses to play a note is worse than one that cuts a tail.
    fn select_voice(&self) -> usize {
        if let Some(index) = self.voices.iter().position(|voice| !voice.active) {
            return index;
        }
        let oldest = |held: bool| {
            self.voices
                .iter()
                .enumerate()
                .filter(|(_, voice)| voice.gate_held == held)
                .min_by_key(|(_, voice)| voice.age)
                .map(|(index, _)| index)
        };
        oldest(false).or_else(|| oldest(true)).unwrap_or(0)
    }

    fn trigger(&mut self, event_id: u64, note: u8, velocity: u8) {
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
        self.hit_count = self.hit_count.wrapping_add(1);

        // The four latched sources, decided here rather than in the voice:
        // the alternator and the random are properties of *this channel's*
        // run of hits, so they survive voice stealing and are the same
        // offline as live.
        let sources = Sources {
            velocity: f32::from(velocity.min(127)) / 127.0,
            note: f32::from(note.min(127)) / 127.0,
            alternator: if self.hit_count % 2 == 1 { 1.0 } else { -1.0 },
            random: hit_random(SEED, self.hit_count),
            ..Sources::default()
        };
        // Routes to a latched destination are evaluated once, here, so a hit
        // whose shape a route decided keeps that shape for its whole life.
        let params = apply_matrix(&self.params, &self.matrix_dests, &sources, true);

        let sr = self.sample_rate;
        let voice = &mut self.voices[index];
        voice.age = age;
        voice.trigger(&params, event_id, note, velocity, sr);
        voice.sources = sources;
    }

    /// Route a note-off to the hit it started. Voices whose envelopes are all
    /// one-shot ignore it, which is v1's behaviour and stays the default.
    fn note_off(&mut self, event_id: u64) {
        for voice in self
            .voices
            .iter_mut()
            .filter(|voice| voice.active && voice.event_id == event_id)
        {
            voice.note_off();
        }
    }

    /// Render `start..end` in control ticks.
    ///
    /// The tick is a real interval, not the gap between events. It has to be:
    /// DS-01's own matrix moves things with nothing arriving from outside —
    /// an envelope opening a filter, Burst Index walking a pitch across a
    /// roll — so a range resolved once at its start would hold the first
    /// tick's values for a whole block. `process` still splits at every event
    /// offset on top of this, which only makes the grid finer.
    fn render_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        let mut at = start;
        while at < end {
            let tick_end = (at + CONTROL_RATE_FRAMES).min(end);
            self.render_tick(bus, at, tick_end);
            at = tick_end;
        }
    }

    fn render_tick(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        if start >= end {
            return;
        }
        // The continuous controls are resolved once here and the smoothers
        // re-aimed, then every sample in the tick runs against them.
        let continuous = Continuous::new(&self.params);
        self.output_hp.set_cutoff(continuous.output_hp, self.sample_rate);
        self.tone_level
            .set_target(self.params.tone_level.clamp(0.0, 1.0));
        self.noise_level
            .set_target(self.params.noise_level.clamp(0.0, 1.0));
        self.body_level
            .set_target(self.params.body_level.clamp(0.0, 1.0));
        self.level.set_target(self.params.level.clamp(0.0, 1.0));

        let sr = self.sample_rate;
        if !self.voices.iter().any(|voice| voice.active) {
            // Nothing is sounding, but the smoothers still have to travel:
            // otherwise a level moved during silence would jump at the next
            // hit instead of already being there.
            self.tone_level.advance_by(end - start);
            self.noise_level.advance_by(end - start);
            self.body_level.advance_by(end - start);
            self.level.advance_by(end - start);
            // Nothing is feeding it, so the high-pass starts the next hit
            // from rest rather than from a state left over from the last one.
            self.output_hp.reset();
            return;
        }

        // The body is skipped only when its level is zero *and* its smoother
        // has arrived — ML-P8's lesson about level-gated skipping, written
        // down in advance: a knob reaching zero does not mean the ramp has,
        // and skipping early replaces it with a step. The resonators are
        // cleared while skipped, so a level returning from zero starts from
        // the next strike rather than resuming a frozen ring.
        let body_live = self.body_level.value() > 0.0 || self.params.body_level > 0.0;
        // A route to a continuous destination is resolved per voice per
        // tick, which is the whole reason DS-01 has a matrix the channel rack
        // cannot substitute for: two hits sounding at once have to be able to
        // disagree about where the filter is. When no row asks for that, all
        // eight voices share the one resolved set.
        let per_voice = self
            .params
            .matrix
            .iter()
            .zip(self.matrix_dests.iter())
            .any(|(route, dest)| {
                route.is_active() && dest.is_some_and(|d| !ds01::is_latched(d.id))
            });
        for (index, voice) in self.voices.iter_mut().enumerate() {
            if !voice.active {
                continue;
            }
            voice.refresh_sources();
            self.voice_continuous[index] = if per_voice {
                Continuous::new(&apply_matrix(
                    &self.params,
                    &self.matrix_dests,
                    &voice.sources,
                    false,
                ))
            } else {
                continuous
            };
            let voice_continuous = &self.voice_continuous[index];
            if body_live {
                voice.body.prepare(
                    voice_continuous.body_pitch * voice.latched.pitch_factor,
                    voice_continuous,
                    sr,
                );
            } else {
                voice.body.reset();
            }
        }

        let voice_continuous = &self.voice_continuous;
        for frame in start..end {
            let tone_level = self.tone_level.advance();
            let noise_level = self.noise_level.advance();
            let body_level = self.body_level.advance();
            let level = self.level.advance();
            let mut sum = 0.0;
            for (index, voice) in self.voices.iter_mut().enumerate() {
                if !voice.active {
                    continue;
                }
                let body_level = if body_live { body_level } else { 0.0 };
                sum += voice.render_sample(
                    &voice_continuous[index],
                    tone_level,
                    noise_level,
                    body_level,
                    sr,
                );
                if voice.amp_env.is_idle() && !voice.burst_pending() {
                    voice.active = false;
                    voice.gate_held = false;
                }
            }
            let sample = device_bound(self.output_hp.next_sample(sum) * level * VOICE_OUTPUT_REFERENCE);
            bus.l[frame] += sample;
            bus.r[frame] += sample;
        }
    }

    /// Render one deterministic hit through the production voice path and
    /// reduce it to min/max bins suitable for a waveform overview. v1's best
    /// property, kept: the drawn hit is the hit.
    ///
    /// `seconds` is the span the caller is drawing, because DS-01's scopes
    /// follow the patch rather than a fixed window — a preview that was
    /// always 300 ms would draw a four-second ride as a spike in the corner
    /// of a scope that is showing four seconds.
    ///
    /// The rate falls as the span grows, so the work stays near
    /// [`PREVIEW_SAMPLES`] however long the patch is. That is still the
    /// production voice path, clocked slower; a preview of a four-second tail
    /// at the full rate is a fifth of a second of arithmetic on the UI
    /// thread, which is what `08-the-face.md` says must not happen per
    /// keystroke.
    pub fn preview_waveform(
        params: Ds01Params,
        bins: usize,
        seconds: f32,
    ) -> (Vec<f32>, Vec<f32>) {
        if bins == 0 {
            return (Vec::new(), Vec::new());
        }
        let seconds = seconds.clamp(0.01, 8.0);
        let rate = (PREVIEW_SAMPLES as f32 / seconds).clamp(8_000.0, 48_000.0) as u32;
        let frames = ((rate as f32 * seconds) as usize).max(bins);

        let mut node = Self::new(params, rate);
        node.trigger(0, 60, 127);
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
                Event::NoteOn { id, note, velocity } => self.trigger(id, note, velocity),
                Event::NoteOff { id, .. } => self.note_off(id),
                Event::Choke => self.choke(),
                Event::ParamValue { id, value } => self.apply_param(id, value),
                // ML-P8's routes are addressed by a durable id because its
                // route list is a variable-length thing a patch grows. DS-01's
                // eight rows are fixed and are ordinary parameters at
                // `matrix_param(row, MATRIX_OFFSET_AMOUNT)`, so an amount
                // arrives as a `ParamValue` like any other knob and this
                // reaches nothing here.
                Event::SourceRouteAmount { .. } => {}
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
    use mooloop_core::{Ds01EnvParams, Ds01PitchEnvParams};

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
        // rate reducer's held sample, the latch, four envelopes, and the
        // three-mode body.
        //
        // Step 03 nearly doubled this: an `Ahd` is 44 bytes against
        // `ExpDecay`'s 8, and there are four of them where there were two.
        // That is what a real envelope shape costs, and it is worth paying —
        // v1's single rate law is the largest single reason its snare and its
        // hat sound like the same instrument. Step 04 added the body's 116,
        // and step 05 the burst's schedule plus the three envelope shapes it
        // re-fires.
        assert_eq!(size_of::<crate::env::Ahd>(), 48);
        assert_eq!(size_of::<Body>(), 116);
        assert_eq!(size_of::<Burst>(), 36);
        assert_eq!(size_of::<Sources>(), 32);
        assert_eq!(size_of::<Voice>(), 656);
        // Eight of those, plus the parameter block — which the matrix's
        // eight rows dominate — the four device-wide smoothers the layers
        // share, and one resolved control set per voice, because the matrix
        // is per voice and two hits have to be able to disagree.
        assert_eq!(size_of::<Ds01Params>(), 352);
        assert_eq!(size_of::<Ds01>(), 6_352);
        assert_eq!(size_of::<Voice>() * DS01_VOICES, 5_248);
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
            amp: Ds01EnvParams::one_shot(2.0),
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
            pitch: Ds01PitchEnvParams {
                decay: 2.0,
                ..Ds01PitchEnvParams::default()
            },
            amp: Ds01EnvParams::one_shot(2.0),
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
                amp: Ds01EnvParams::one_shot(4.0),
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

    /// Attack must not cost the transient. At Attack 0 the hit is at full
    /// amplitude on its first samples rather than one sample into a ramp.
    ///
    /// Measured on the noise layer, which is the one source with energy at
    /// sample zero: both band-limited oscillator shapes start their cycle at
    /// or near zero by construction, so a tone layer would be measuring
    /// PolyBLEP rather than the envelope.
    #[test]
    fn a_zero_attack_does_not_soften_the_transient() {
        let onset = |attack: f32| {
            let params = Ds01Params {
                tone_level: 0.0,
                noise_level: 1.0,
                filter_morph: 0.0,
                filter_cutoff: 18_000.0,
                amp: Ds01EnvParams {
                    attack,
                    ..Ds01EnvParams::one_shot(0.24)
                },
                ..Ds01Params::default()
            };
            let out = hit(params, 4_800);
            (peak(&out[..8]), peak(&out))
        };

        let (instant_onset, instant_peak) = onset(0.0);
        // The first eight samples already reach the loudest the hit gets:
        // nothing softened them.
        assert!(
            instant_onset > instant_peak * 0.5,
            "a zero attack opened at {instant_onset} against a peak of {instant_peak}"
        );

        // A ramp that was asked for is still a ramp: ten milliseconds in,
        // eight samples is 1.6% of the way up.
        let (ramped_onset, _) = onset(0.01);
        assert!(
            ramped_onset < instant_onset * 0.05,
            "a 10 ms attack opened at {ramped_onset} against {instant_onset}"
        );
    }

    /// A gated amplitude envelope rings for the length of a held note and
    /// then releases. This is the ride cymbal, the held shaker and the
    /// sustained noise wash — sounds v1 cannot make at all, because its
    /// note-offs end nothing and its one envelope has no sustain.
    #[test]
    fn a_gated_amplitude_envelope_rings_for_the_length_of_a_held_note() {
        let params = Ds01Params {
            amp: Ds01EnvParams {
                decay: 0.05,
                sustain: 0.6,
                release: 0.05,
                gate: true,
                ..Ds01EnvParams::one_shot(0.05)
            },
            ..Ds01Params::default()
        };
        let mut node = Ds01::new(params, SR);
        let mut events = EventList::empty();
        events.push(note_on(0, 60, 127));
        let held = render(&mut node, SR as usize, &events);
        // Still sounding a whole second after a hit whose decay is 50 ms.
        assert!(rms(&held[40_000..]) > 0.01, "the held note stopped on its own");
        assert!(node.voices.iter().any(|voice| voice.gate_held));

        let mut off = EventList::empty();
        off.push(TimedEvent {
            offset: 0,
            event: Event::NoteOff { id: 0, note: 60 },
        });
        render(&mut node, SR as usize, &off);
        assert!(
            node.voices.iter().all(|voice| !voice.active),
            "the note-off did not release it"
        );
    }

    /// The other half of the same rule: gate is per envelope, so a one-shot
    /// envelope beside a gated one keeps ignoring note-off.
    #[test]
    fn a_one_shot_envelope_beside_a_gated_one_still_ignores_note_off() {
        let params = Ds01Params {
            // The noise layer is held; the amplitude envelope is not.
            noise_level: 1.0,
            noise_env: Ds01EnvParams {
                sustain: 0.8,
                gate: true,
                ..Ds01EnvParams::one_shot(0.05)
            },
            amp: Ds01EnvParams::one_shot(2.0),
            ..Ds01Params::default()
        };
        let mut node = Ds01::new(params, SR);
        let mut events = EventList::empty();
        events.push(note_on(0, 60, 127));
        events.push(TimedEvent {
            offset: 4_800,
            event: Event::NoteOff { id: 0, note: 60 },
        });
        let out = render(&mut node, 24_000, &events);
        // The amplitude envelope is a 2 s one-shot: the hit is still going
        // long after the note-off that ended the noise layer.
        assert!(rms(&out[12_000..]) > 0.001, "a one-shot envelope obeyed note-off");
        assert!(node.voices.iter().any(|voice| voice.active));
    }

    /// A voice held open by a gated envelope is not free to steal — a ride
    /// written to ring must survive the hat pattern underneath it — and it
    /// gives way once the pool is genuinely exhausted, because a device that
    /// refuses to play a note is worse than one that cuts a tail.
    #[test]
    fn a_held_voice_is_stolen_only_when_the_pool_is_exhausted() {
        let gated = Ds01Params {
            amp: Ds01EnvParams {
                sustain: 0.7,
                gate: true,
                ..Ds01EnvParams::one_shot(0.05)
            },
            ..Ds01Params::default()
        };
        let mut node = Ds01::new(gated, SR);
        // One held hit, then seven one-shots that fill the rest of the pool.
        node.trigger(1, 60, 127);
        node.set_params(Ds01Params {
            amp: Ds01EnvParams::one_shot(4.0),
            ..Ds01Params::default()
        });
        for id in 2..=DS01_VOICES as u64 {
            node.trigger(id, 60, 127);
        }
        assert!(node.voices.iter().all(|voice| voice.active));

        // The pool is full and the held voice is the oldest. The next hits
        // take the one-shots instead.
        for id in 100..107 {
            node.trigger(id, 60, 127);
        }
        assert!(
            node.voices.iter().any(|voice| voice.event_id == 1),
            "the held voice was stolen while the pool still had one-shots"
        );

        // Now every voice is held. It has to give way.
        for voice in &mut node.voices {
            voice.gate_held = true;
        }
        node.trigger(200, 60, 127);
        assert!(
            node.voices.iter().all(|voice| voice.event_id != 1),
            "a full pool of held voices refused to play a note"
        );
    }

    /// The noise envelope shapes the noise layer and nothing else, which is
    /// what lets a snare's rattle outlive its shell tone or stop before it.
    #[test]
    fn the_noise_envelope_shapes_only_the_noise_layer() {
        let tail = |noise_level: f32, decay: f32| {
            let params = Ds01Params {
                tone_level: 0.0,
                noise_level,
                noise_env: Ds01EnvParams::one_shot(decay),
                amp: Ds01EnvParams::one_shot(2.0),
                ..Ds01Params::default()
            };
            rms(&hit(params, 24_000)[12_000..])
        };

        // The rattle outlives the shell tone, or stops well before it: the
        // amplitude envelope is a 2 s one-shot in both cases.
        assert!(
            tail(1.0, 2.0) > tail(1.0, 0.01) * 20.0,
            "{} then {}",
            tail(1.0, 0.01),
            tail(1.0, 2.0)
        );

        // And with the layer silent it reaches nothing at all, which is the
        // "only" half of the claim.
        assert_eq!(
            hit(
                Ds01Params {
                    noise_level: 0.0,
                    noise_env: Ds01EnvParams::one_shot(0.01),
                    ..Ds01Params::default()
                },
                12_000
            ),
            hit(
                Ds01Params {
                    noise_level: 0.0,
                    noise_env: Ds01EnvParams::one_shot(2.0),
                    ..Ds01Params::default()
                },
                12_000
            )
        );
    }

    /// Every envelope shape, over every layer, terminates and stays bounded.
    /// The one exception is a gated envelope with a sustain, which is
    /// supposed to wait — so it is tested with the note-off it is waiting for.
    #[test]
    fn every_envelope_shape_makes_sound_and_terminates() {
        for attack in [0.0, 0.01, 0.5] {
            for hold in [0.0, 0.05] {
                for curve in [-1.0, 0.0, 1.0] {
                    for gate in [false, true] {
                        let env = Ds01EnvParams {
                            attack,
                            hold,
                            decay: 0.1,
                            curve,
                            sustain: 0.5,
                            release: 0.05,
                            gate,
                        };
                        let params = Ds01Params {
                            noise_level: 0.6,
                            amp: env,
                            noise_env: env,
                            mod_env: env,
                            ..Ds01Params::default()
                        };
                        let mut node = Ds01::new(params, SR);
                        let mut events = EventList::empty();
                        events.push(note_on(0, 60, 127));
                        let out = render(&mut node, SR as usize, &events);
                        assert!(out.iter().all(|s| s.is_finite()), "{env:?} went non-finite");
                        assert!(peak(&out) <= 1.0, "{env:?} peaked at {}", peak(&out));
                        assert!(peak(&out) > 0.01, "{env:?} made nothing");

                        let mut off = EventList::empty();
                        off.push(TimedEvent {
                            offset: 0,
                            event: Event::NoteOff { id: 0, note: 60 },
                        });
                        render(&mut node, SR as usize, &off);
                        for _ in 0..2 {
                            render(&mut node, SR as usize, &EventList::empty());
                        }
                        assert!(
                            node.voices.iter().all(|voice| !voice.active),
                            "{env:?} stranded a voice"
                        );
                    }
                }
            }
        }
    }

    /// A body-only patch: the VCA held wide open by a gated envelope at full
    /// sustain, so what is measured is the resonators' own ring and not the
    /// amplitude envelope's.
    fn body_only(pitch: f32, ratio: f32, decay: f32, damping: f32) -> Ds01Params {
        Ds01Params {
            tone_level: 0.0,
            noise_level: 0.0,
            body_level: 1.0,
            body_pitch: pitch,
            body_ratio: ratio,
            body_decay: decay,
            body_damping: damping,
            body_excite: 0.0,
            amp: Ds01EnvParams {
                sustain: 1.0,
                gate: true,
                ..Ds01EnvParams::one_shot(0.002)
            },
            ..Ds01Params::default()
        }
    }

    /// Seconds until the signal is 60 dB below its own peak.
    fn decay_time_s(samples: &[f32]) -> f32 {
        let floor = peak(samples) * 0.001;
        let last = samples
            .iter()
            .rposition(|s| s.abs() > floor)
            .unwrap_or(samples.len());
        last as f32 / SR as f32
    }

    /// Normalized autocorrelation at the fundamental's period: how periodic
    /// the layer is at the pitch it was tuned to.
    fn periodicity(samples: &[f32], freq: f32) -> f32 {
        let lag = (SR as f32 / freq).round() as usize;
        let window = &samples[..samples.len() - lag];
        let energy: f32 = window.iter().map(|s| s * s).sum();
        if energy <= 0.0 {
            return 0.0;
        }
        let correlation: f32 = window
            .iter()
            .zip(samples[lag..].iter())
            .map(|(a, b)| a * b)
            .sum();
        correlation / energy
    }

    /// Ratio 0 is a pitched drum and Ratio 1 is a material. This is the whole
    /// design in one control, so it is measured rather than described: at 0
    /// the layer repeats at the period it was tuned to, and at 1 it does not
    /// repeat at that period at all.
    #[test]
    fn ratio_sweeps_the_body_from_a_pitch_to_a_material() {
        let render_body = |ratio: f32, pitch: f32| hit(body_only(pitch, ratio, 1.0, 0.0), 12_000);

        let harmonic = periodicity(&render_body(0.0, 220.0)[2_400..9_600], 220.0);
        let membrane = periodicity(&render_body(1.0, 220.0)[2_400..9_600], 220.0);
        assert!(harmonic > 0.8, "a harmonic body is only {harmonic} periodic");
        assert!(
            membrane < harmonic * 0.7,
            "a membrane is {membrane} periodic against {harmonic}"
        );

        // And the pitched end tracks the note, like the tone layer.
        let pitch_of = |note: u8| {
            let mut node = Ds01::new(body_only(220.0, 0.0, 1.0, 0.0), SR);
            let mut events = EventList::empty();
            events.push(note_on(0, note, 127));
            upward_crossings(&render(&mut node, 12_000, &events)[2_400..9_600])
        };
        let low = pitch_of(48);
        let high = pitch_of(60);
        assert!(
            (high as f32 - low as f32 * 2.0).abs() < low as f32 * 0.1,
            "an octave gave {low} then {high} crossings"
        );
    }

    /// Body Decay is a time, not a Q. The resonator's pole radius comes
    /// straight from the seconds asked for, so a mode at 60 Hz and one at
    /// 2 kHz ring for the same length — which a band-pass parameterized by Q
    /// would not do, and which is why this device does not use one.
    #[test]
    fn body_decay_is_a_time_at_every_pitch() {
        let measured: Vec<f32> = [80.0, 400.0, 2_000.0]
            .into_iter()
            .map(|pitch| decay_time_s(&hit(body_only(pitch, 0.0, 0.3, 0.0), 24_000)))
            .collect();
        for time in &measured {
            assert!(
                (time - 0.225).abs() < 0.06,
                "a 0.3 s body decayed in {time} s (all: {measured:?})"
            );
        }
    }

    /// Damping is high-frequency loss: it shortens the modes above the
    /// fundamental and leaves the fundamental alone. That is the difference
    /// between a bell and a woodblock.
    #[test]
    fn damping_shortens_the_upper_modes_and_not_the_fundamental() {
        let bright = hit(body_only(220.0, 1.0, 0.3, 0.0), 24_000);
        let damped = hit(body_only(220.0, 1.0, 0.3, 1.0), 24_000);

        // Well into the tail, the damped patch has lost its upper modes, so
        // it crosses zero far less often for the same fundamental.
        let bright_rate = upward_crossings(&bright[4_800..12_000]);
        let damped_rate = upward_crossings(&damped[4_800..12_000]);
        assert!(
            (damped_rate as f32) < bright_rate as f32 * 0.7,
            "damping left {damped_rate} crossings against {bright_rate}"
        );
        // The fundamental is untouched, so the layer still rings about as
        // long. The window is longer than the decay, so both figures are
        // real rather than the end of the buffer.
        let bright_time = decay_time_s(&bright);
        let damped_time = decay_time_s(&damped);
        assert!(bright_time < 0.4, "the window clipped the tail at {bright_time}");
        assert!(
            damped_time > bright_time * 0.8,
            "damping cut the fundamental too: {bright_time} then {damped_time}"
        );
    }

    /// Excite decides what the resonators are hit with. At 0 it is the strike
    /// — which is clave, rim and woodblock — and at 1 it is the noise layer,
    /// which is cymbal shimmer and the ring under a snare.
    #[test]
    fn excite_crossfades_from_a_strike_to_the_noise_layer() {
        // The noise layer's *level* is zero throughout: the body reads its
        // pre-level tap, per `01-what-ds01-is.md`.
        let with_excite = |excite: f32| {
            let params = Ds01Params {
                body_excite: excite,
                // Silent, but wide open: the body reads the noise layer's
                // pre-level tap, and a high-passed hiss has nothing at 220 Hz
                // to drive a resonator tuned there with.
                noise_level: 0.0,
                filter_morph: 0.0,
                filter_cutoff: 18_000.0,
                ..body_only(220.0, 0.0, 0.5, 0.3)
            };
            let out = hit(params, 12_000);
            (peak(&out[..480]), rms(&out[4_800..]))
        };

        let (struck_onset, struck_tail) = with_excite(0.0);
        let (driven_onset, driven_tail) = with_excite(1.0);
        // A strike is all onset; a driven resonator keeps being fed.
        assert!(struck_onset > driven_onset * 2.0, "{struck_onset} vs {driven_onset}");
        assert!(driven_tail > struck_tail, "{struck_tail} vs {driven_tail}");
    }

    /// With the layer at zero level, none of its five other controls reaches
    /// the output — which is what makes skipping it safe, and is the
    /// observable half of "Body Level at 0 costs approximately nothing".
    #[test]
    fn a_silent_body_layer_ignores_every_body_control() {
        let quiet = Ds01Params {
            body_level: 0.0,
            ..Ds01Params::default()
        };
        let fiddled = Ds01Params {
            body_pitch: 3_000.0,
            body_ratio: 1.0,
            body_decay: 8.0,
            body_damping: 1.0,
            body_excite: 1.0,
            ..quiet
        };
        assert_eq!(hit(quiet, 12_000), hit(fiddled, 12_000));
    }

    /// A patch whose every impulse is a short broadband burst, so the
    /// schedule is visible in the output as separate onsets.
    fn burst_patch(repeats: u8, spread: f32, level_step: f32, pitch_step: f32) -> Ds01Params {
        Ds01Params {
            tone_level: 0.0,
            noise_level: 1.0,
            filter_morph: 0.0,
            filter_cutoff: 18_000.0,
            burst_repeats: repeats,
            burst_spacing: 0.02,
            burst_spread: spread,
            burst_level_step: level_step,
            burst_pitch_step: pitch_step,
            amp: Ds01EnvParams::one_shot(0.006),
            noise_env: Ds01EnvParams::one_shot(0.006),
            ..Ds01Params::default()
        }
    }

    /// Sample index of each rising edge in a block-wise envelope of `samples`.
    fn onsets(samples: &[f32]) -> Vec<usize> {
        const BLOCK: usize = 32;
        let floor = peak(samples) * 0.1;
        let mut found = Vec::new();
        let mut was_above = false;
        for (index, block) in samples.chunks(BLOCK).enumerate() {
            let above = peak(block) > floor;
            if above && !was_above {
                found.push(index * BLOCK);
            }
            was_above = above;
        }
        found
    }

    /// Repeats 1 is an ordinary hit: the other four burst controls are live
    /// the moment it moves and reach nothing before that.
    #[test]
    fn repeats_of_one_is_an_ordinary_hit() {
        let plain = Ds01Params::default();
        let fiddled = Ds01Params {
            burst_spacing: 0.5,
            burst_spread: -1.0,
            burst_level_step: -1.0,
            burst_pitch_step: 24.0,
            ..plain
        };
        assert_eq!(hit(plain, 24_000), hit(fiddled, 24_000));
        assert_eq!(onsets(&hit(burst_patch(1, 0.0, 0.0, 0.0), 24_000)).len(), 1);
    }

    /// One trigger, several impulses, evenly spaced at Spread 0 — which is
    /// the machine-gun end of the control.
    #[test]
    fn a_burst_fires_its_impulses_on_the_schedule() {
        let out = hit(burst_patch(4, 0.0, 0.0, 0.0), 24_000);
        let times = onsets(&out);
        assert_eq!(times.len(), 4, "found {times:?}");
        let spacing = (0.02 * SR as f32) as usize;
        for pair in times.windows(2) {
            let gap = pair[1] - pair[0];
            assert!(
                gap.abs_diff(spacing) < 64,
                "a 20 ms gap came out {gap} samples: {times:?}"
            );
        }
    }

    /// Negative Spread accelerates — each gap shorter than the last, which is
    /// the clap and the buzz roll. Positive decelerates, which is a drag.
    #[test]
    fn spread_accelerates_and_decelerates_the_gaps() {
        let gaps = |spread: f32| {
            let times = onsets(&hit(burst_patch(4, spread, 0.0, 0.0), 48_000));
            assert_eq!(times.len(), 4, "spread {spread} gave {times:?}");
            times.windows(2).map(|p| p[1] - p[0]).collect::<Vec<_>>()
        };
        let accelerating = gaps(-1.0);
        assert!(
            accelerating.windows(2).all(|p| p[1] < p[0]),
            "not accelerating: {accelerating:?}"
        );
        let decelerating = gaps(1.0);
        assert!(
            decelerating.windows(2).all(|p| p[1] > p[0]),
            "not decelerating: {decelerating:?}"
        );
    }

    /// Level Step shapes a burst without making it louder than the single hit
    /// it replaces: the sequence is normalized so its loudest impulse is the
    /// reference one, whichever end of the control it sits at.
    #[test]
    fn level_step_shapes_a_burst_without_raising_its_peak() {
        let single = peak(&hit(burst_patch(1, 0.0, 0.0, 0.0), 24_000));
        for step in [-1.0, -0.5, 0.5, 1.0] {
            let out = hit(burst_patch(4, 0.0, step, 0.0), 24_000);
            assert!(
                peak(&out) <= single * 1.05,
                "level step {step} peaked at {} against a single hit's {single}",
                peak(&out)
            );
        }

        // And it is a shape, not a no-op: a falling step is loud first and a
        // rising one is loud last.
        let falling = hit(burst_patch(4, 0.0, -1.0, 0.0), 24_000);
        let rising = hit(burst_patch(4, 0.0, 1.0, 0.0), 24_000);
        assert!(peak(&falling[..2_400]) > peak(&falling[2_400..4_800]));
        assert!(peak(&rising[..2_400]) < peak(&rising[2_400..4_800]));
    }

    /// Pitch Step moves each impulse, cumulatively: a fill that climbs, or a
    /// tom roll that falls.
    #[test]
    fn pitch_step_moves_each_impulse() {
        let params = Ds01Params {
            tone_level: 1.0,
            noise_level: 0.0,
            burst_repeats: 4,
            burst_spacing: 0.05,
            burst_pitch_step: 12.0,
            pitch: Ds01PitchEnvParams {
                depth: 0.0,
                ..Ds01PitchEnvParams::default()
            },
            amp: Ds01EnvParams::one_shot(0.04),
            ..Ds01Params::default()
        };
        let out = hit(params, 24_000);
        let per_impulse: Vec<usize> = (0..4)
            .map(|index| {
                let start = index * (0.05 * SR as f32) as usize;
                upward_crossings(&out[start..start + 1_200])
            })
            .collect();
        assert!(
            per_impulse.windows(2).all(|p| p[1] > p[0]),
            "an octave a step gave {per_impulse:?}"
        );
    }

    /// An eight-repeat burst is one voice, not eight. That is what keeps a
    /// roll from consuming the whole pool, and it is why the body resonator
    /// can ring across the impulses at all.
    #[test]
    fn an_eight_repeat_burst_uses_one_voice() {
        let mut node = Ds01::new(burst_patch(DS01_MAX_REPEATS, 0.0, 0.0, 0.0), SR);
        let mut events = EventList::empty();
        events.push(note_on(0, 60, 127));
        // A hundred milliseconds in, the schedule still owes four impulses,
        // so this is measured while the burst is live rather than after it.
        let out = render(&mut node, 4_800, &events);
        assert_eq!(onsets(&out).len(), 5);
        assert_eq!(node.voices.iter().filter(|voice| voice.active).count(), 1);
    }

    /// The resonators ring *across* the impulses rather than being restarted,
    /// which is what makes a burst a clap rather than four claps: four
    /// strikes into one object leave more energy in it than one strike does.
    #[test]
    fn the_body_rings_across_a_burst() {
        let energy = |repeats: u8| {
            let params = Ds01Params {
                burst_repeats: repeats,
                burst_spacing: 0.02,
                // Well under the voice's own ceiling, so this measures the
                // resonator rather than the bound above it.
                body_level: 0.25,
                ..body_only(220.0, 0.0, 1.0, 0.0)
            };
            let out = hit(params, 24_000);
            out.iter().map(|s| (*s as f64) * (*s as f64)).sum::<f64>()
        };

        // Total energy is the discriminator, not the tail's level. A
        // resonator cleared at each strike ends up holding about one
        // strike's worth however many it was given — three truncated 20 ms
        // fragments plus one full tail, which measures at roughly 1.06x — so
        // anything well above that is the ring carrying across.
        //
        // It is 1.65x rather than 4x because the resonator is linear and the
        // strikes land at unrelated phases of its cycle, so they partly
        // cancel. That is the same reason a flam on a bell sounds like a flam
        // and not like one hit four times as hard.
        let one = energy(1);
        let four = energy(4);
        assert!(
            four > one * 1.4,
            "four strikes left {four} of energy against one strike's {one}"
        );
    }

    /// Burst Index: 0 at the first impulse, 1 at the last, constant within an
    /// impulse, and 0 throughout at Repeats 1. Step 07 publishes it as a
    /// matrix source — a shape across a flam or a roll, which is consistent
    /// displacement rather than the randomness the taste brief rules out —
    /// and the contract is testable before anything routes it.
    #[test]
    fn burst_index_runs_from_zero_to_one_across_the_impulses() {
        let mut burst = Burst::default();
        burst.start(
            &Ds01Params {
                burst_repeats: 4,
                burst_spacing: 0.01,
                ..Ds01Params::default()
            },
            SR,
        );

        let mut seen = vec![burst.position()];
        for _ in 0..(0.06 * SR as f32) as usize {
            if burst.tick(SR) {
                seen.push(burst.position());
            }
        }
        assert_eq!(seen.len(), 4, "{seen:?}");
        for (index, position) in seen.iter().enumerate() {
            let want = index as f32 / 3.0;
            assert!((position - want).abs() < 1.0e-6, "{seen:?}");
        }

        burst.start(&Ds01Params::default(), SR);
        for _ in 0..4_800 {
            assert!(!burst.tick(SR), "an ordinary hit fired a second impulse");
            assert_eq!(burst.position(), 0.0);
        }
    }

    /// A voice is not free while it still owes impulses, even if its
    /// amplitude envelope ran out between them — and every configuration
    /// still ends. The schedule may shape a hit; it may not extend a voice's
    /// lifetime indefinitely.
    #[test]
    fn every_burst_configuration_terminates() {
        for repeats in [1, 4, DS01_MAX_REPEATS] {
            for spread in [-1.0, 0.0, 1.0] {
                for spacing in [0.001, 0.5] {
                    let params = Ds01Params {
                        burst_spacing: spacing,
                        ..burst_patch(repeats, spread, -1.0, 0.0)
                    };
                    let mut node = Ds01::new(params, SR);
                    let mut events = EventList::empty();
                    events.push(note_on(0, 60, 127));
                    let out = render(&mut node, SR as usize, &events);
                    assert!(out.iter().all(|s| s.is_finite()));
                    assert!(peak(&out) <= 1.0);

                    // `DS01_BURST_MAX_S` bounds the schedule, so five seconds
                    // covers the longest of them plus its tail.
                    for _ in 0..5 {
                        render(&mut node, SR as usize, &EventList::empty());
                    }
                    assert!(
                        node.voices.iter().all(|voice| !voice.active),
                        "repeats {repeats}, spread {spread}, spacing {spacing} stranded a voice"
                    );
                }
            }
        }
    }

    /// Soft with no bias and full bit depth *is* v1's drive curve — the same
    /// function, called rather than re-derived — so an old-sounding patch
    /// stays reachable rather than approximately reachable.
    #[test]
    fn soft_with_no_bias_and_full_bits_is_v1s_drive_curve() {
        for drive in [0.0, 0.25, 0.6, 1.0] {
            for input in [-1.0, -0.4, -0.05, 0.0, 0.05, 0.4, 1.0] {
                let ours = shape_stage(input, drive, Ds01Character::Soft, 0.0, bit_step(16.0));
                assert_eq!(ours, apply_drive(input, drive), "drive {drive}, input {input}");
            }
        }
    }

    /// At its defaults the whole stage is exactly transparent, which is what
    /// lets the gain contract be calibrated against a path with nothing in it.
    #[test]
    fn the_shape_stage_is_transparent_at_its_defaults() {
        let defaults = Ds01Params::default();
        for input in [-0.9, -0.3, 0.0, 0.3, 0.9] {
            assert_eq!(
                shape_stage(
                    input,
                    defaults.drive,
                    defaults.character,
                    defaults.bias,
                    bit_step(defaults.bits)
                ),
                input
            );
        }
        assert_eq!(bit_step(DS01_BITS_TRANSPARENT), 0.0);
        assert_eq!(quantize(0.123_456, 0.0), 0.123_456);
    }

    /// Fold is the character that reacts to level, and it is why the shape
    /// stage sits after the amplitude envelope rather than before it: because
    /// folding is a function of instantaneous amplitude, the spectrum of one
    /// hit changes across its own decay for free. Measured as crest factor,
    /// which a folded loud signal collapses and a quiet one leaves alone.
    #[test]
    fn fold_changes_the_spectrum_across_one_hit() {
        let params = Ds01Params {
            drive: 0.6,
            character: Ds01Character::Fold,
            amp: Ds01EnvParams::one_shot(0.5),
            pitch: Ds01PitchEnvParams {
                depth: 0.0,
                ..Ds01PitchEnvParams::default()
            },
            ..Ds01Params::default()
        };
        let out = hit(params, 24_000);
        let crest = |window: &[f32]| peak(window) / rms(window).max(1.0e-9);
        let loud = crest(&out[480..2_400]);
        let quiet = crest(&out[16_000..20_000]);
        assert!(
            quiet > loud * 1.15,
            "the folded hit kept its shape across the decay: {loud} then {quiet}"
        );
    }

    /// Bias creates a DC offset by design — at the top of the range the
    /// offset is the effect — and the output high-pass is what takes it back
    /// out.
    #[test]
    fn the_output_high_pass_removes_the_dc_that_bias_creates() {
        let biased = Ds01Params {
            drive: 0.5,
            bias: 1.0,
            amp: Ds01EnvParams::one_shot(1.0),
            ..Ds01Params::default()
        };
        let out = hit(biased, 24_000);
        let mean = out[4_800..].iter().sum::<f32>() / out[4_800..].len() as f32;
        assert!(
            mean.abs() < peak(&out) * 0.02,
            "a mean of {mean} against a peak of {}",
            peak(&out)
        );
        // And it is not vacuous: bias changed the sound.
        assert_ne!(
            out,
            hit(
                Ds01Params {
                    bias: 0.0,
                    ..biased
                },
                24_000
            )
        );
    }

    /// Drive is compensated but not normalized: raising it changes timbre
    /// substantially more than it changes level, and it is still allowed to
    /// make a hit louder, because that is what drive does.
    #[test]
    fn drive_changes_timbre_more_than_level() {
        let measure = |drive: f32| {
            let params = Ds01Params {
                drive,
                amp: Ds01EnvParams::one_shot(1.0),
                pitch: Ds01PitchEnvParams {
                    depth: 0.0,
                    ..Ds01PitchEnvParams::default()
                },
                ..Ds01Params::default()
            };
            let out = hit(params, 24_000);
            let window = &out[480..12_000];
            (rms(window), peak(window) / rms(window).max(1.0e-9))
        };
        let (clean_rms, clean_crest) = measure(0.0);
        let (driven_rms, driven_crest) = measure(1.0);

        let level_db = 20.0 * (driven_rms / clean_rms).log10();
        assert!(level_db.abs() < 6.0, "drive moved the level {level_db} dB");
        // A sine driven into a soft clip approaches a square: its crest
        // factor falls from 1.41 toward 1.
        assert!(
            driven_crest < clean_crest * 0.85,
            "crest went {clean_crest} to {driven_crest}"
        );
    }

    /// Layers sum honestly: adding the noise or the body never turns the tone
    /// down, because they are separate summed layers with their own levels
    /// rather than a crossfade or an automatic balance.
    ///
    /// Stated as superposition, which is what that claim actually means and
    /// is sharper than a level comparison: two layers together are exactly
    /// the two layers apart, sample for sample. Measured below the voice's
    /// ceiling and the device's bound, since the whole point of those is that
    /// they are *not* linear once a patch drives them.
    #[test]
    fn adding_a_layer_does_not_turn_the_tone_down() {
        let quiet = Ds01Params {
            tone_level: 0.4,
            level: 0.5,
            amp: Ds01EnvParams::one_shot(1.0),
            ..Ds01Params::default()
        };
        for added in [
            Ds01Params {
                noise_level: 0.3,
                ..quiet
            },
            Ds01Params {
                body_level: 0.3,
                ..quiet
            },
        ] {
            let alone = hit(quiet, 12_000);
            let layer = hit(
                Ds01Params {
                    tone_level: 0.0,
                    ..added
                },
                12_000,
            );
            let both = hit(added, 12_000);
            let worst = both
                .iter()
                .zip(alone.iter().zip(layer.iter()))
                .fold(0.0_f32, |worst, (sum, (a, b))| {
                    worst.max((sum - (a + b)).abs())
                });
            assert!(
                worst < 1.0e-6,
                "the layers did not sum: worst sample differs by {worst}"
            );
        }
    }

    /// Every character, at both ends of every shaper control, over a full
    /// pool. The bound is audible saturation rather than a hidden limiter, so
    /// it is asserted at the device output.
    #[test]
    fn every_character_stays_bounded_and_finite() {
        for character in Ds01Character::ALL {
            for drive in [0.0, 1.0] {
                for bias in [0.0, 1.0] {
                    for bits in [1.0, DS01_BITS_TRANSPARENT] {
                        let params = Ds01Params {
                            drive,
                            character,
                            bias,
                            bits,
                            level: 1.0,
                            noise_level: 1.0,
                            body_level: 1.0,
                            ..Ds01Params::default()
                        };
                        let mut node = Ds01::new(params, SR);
                        let mut events = EventList::empty();
                        for voice in 0..DS01_VOICES as u32 {
                            events.push_ordered(note_on(voice * 16, 36 + voice as u8 * 7, 127));
                        }
                        let out = render(&mut node, 12_000, &events);
                        let label = format!("{character:?} drive {drive} bias {bias} bits {bits}");
                        assert!(out.iter().all(|s| s.is_finite()), "{label} went non-finite");
                        assert!(peak(&out) <= 1.0, "{label} peaked at {}", peak(&out));
                    }
                }
            }
        }
    }

    /// A patch with one route and nothing else moving: the tone layer alone,
    /// quiet enough to stay below the voice ceiling and the device bound so
    /// two voices sum linearly.
    fn routed(source: Ds01ModSource, dest: u32, amount: f32) -> Ds01Params {
        let mut params = Ds01Params {
            tone_level: 1.0,
            noise_level: 0.0,
            level: 0.4,
            amp: Ds01EnvParams::one_shot(1.0),
            pitch: Ds01PitchEnvParams {
                depth: 0.0,
                ..Ds01PitchEnvParams::default()
            },
            ..Ds01Params::default()
        };
        params.matrix[0] = mooloop_core::Ds01Route {
            source,
            dest,
            amount,
            curve: 0.0,
        };
        params
    }

    /// The test the matrix exists for. A channel source produces one number
    /// per control tick for the whole channel; two hits sounding at once with
    /// different velocities have to be able to disagree about their pitch,
    /// and that is not expressible as a channel-rate signal.
    ///
    /// Proved as superposition: the two hits together are exactly the two
    /// hits apart. A per-channel source would give both voices the same
    /// velocity, so the pair would be two copies of one pitch instead.
    #[test]
    fn a_route_is_per_voice_and_not_per_channel() {
        let params = routed(Ds01ModSource::Velocity, ds01::PARAM_TONE_PITCH, 0.5);
        let one_hit = |velocity: u8| {
            let mut node = Ds01::new(params, SR);
            let mut events = EventList::empty();
            events.push(note_on(0, 60, velocity));
            render(&mut node, 12_000, &events)
        };
        let soft = one_hit(40);
        let loud = one_hit(127);
        assert_ne!(soft, loud, "velocity did not reach the pitch at all");

        let mut node = Ds01::new(params, SR);
        let mut events = EventList::empty();
        events.push_ordered(note_on(0, 60, 40));
        events.push_ordered(note_on(0, 60, 127));
        let both = render(&mut node, 12_000, &events);

        let worst = both
            .iter()
            .zip(soft.iter().zip(loud.iter()))
            .fold(0.0_f32, |worst, (sum, (a, b))| worst.max((sum - (a + b)).abs()));
        assert!(
            worst < 1.0e-5,
            "the two hits did not keep their own velocities: worst {worst}"
        );
    }

    /// Burst Index puts a shape across a flam or a roll: four impulses inside
    /// one voice, four distinct pitches. Consistent displacement rather than
    /// noise, which is the distinction the taste brief draws.
    #[test]
    fn burst_index_gives_one_voice_four_pitches() {
        let mut params = routed(Ds01ModSource::BurstIndex, ds01::PARAM_TONE_PITCH, 0.4);
        params.burst_repeats = 4;
        params.burst_spacing = 0.05;
        params.amp = Ds01EnvParams::one_shot(0.04);

        let out = hit(params, 24_000);
        let per_impulse: Vec<usize> = (0..4)
            .map(|index| {
                let start = index * (0.05 * SR as f32) as usize;
                upward_crossings(&out[start..start + 1_200])
            })
            .collect();
        assert!(
            per_impulse.windows(2).all(|pair| pair[1] > pair[0]),
            "one voice gave {per_impulse:?}"
        );
    }

    /// The 808 open/closed alternation and the every-other-hat ghost. It is a
    /// property of the channel's run of hits rather than of a voice, so it
    /// survives voice stealing.
    #[test]
    fn the_hit_alternator_alternates_and_survives_stealing() {
        let params = Ds01Params {
            amp: Ds01EnvParams::one_shot(4.0),
            ..routed(Ds01ModSource::HitAlternator, ds01::PARAM_TONE_PITCH, 0.3)
        };
        let mut node = Ds01::new(params, SR);
        let mut seen = Vec::new();
        // More hits than the pool has voices, so the last several are steals.
        for hit in 0..DS01_VOICES as u64 + 6 {
            node.trigger(hit, 60, 127);
            let voice = node
                .voices
                .iter()
                .max_by_key(|voice| voice.age)
                .expect("a voice was allocated");
            seen.push(voice.sources.alternator);
        }
        for (index, value) in seen.iter().enumerate() {
            let want = if index % 2 == 0 { 1.0 } else { -1.0 };
            assert_eq!(*value, want, "hit {index} of {seen:?}");
        }
    }

    /// Random is deterministic: derived from the node seed and the hit
    /// counter, so an offline render and a live take of the same event stream
    /// produce identical samples. That is what separates it from a humanize
    /// control, and it is why it is safe to route at all.
    #[test]
    fn random_renders_identically_for_the_same_event_stream() {
        let params = routed(Ds01ModSource::Random, ds01::PARAM_TONE_PITCH, 0.5);
        let stream = |frames: usize| {
            let mut node = Ds01::new(params, SR);
            let mut out = Vec::new();
            for block in 0..4 {
                let mut events = EventList::empty();
                events.push(note_on(0, 60 + block, 100));
                out.extend(render(&mut node, frames, &events));
            }
            out
        };
        assert_eq!(stream(6_000), stream(6_000));

        // And successive hits do get different values, or the source would be
        // a constant with a good story.
        let mut node = Ds01::new(params, SR);
        node.trigger(1, 60, 100);
        let first = node.voices.iter().max_by_key(|v| v.age).unwrap().sources.random;
        node.trigger(2, 60, 100);
        let second = node.voices.iter().max_by_key(|v| v.age).unwrap().sources.random;
        assert_ne!(first, second);
        assert!((-1.0..=1.0).contains(&first) && (-1.0..=1.0).contains(&second));
    }

    /// A route's curve is neutral in the middle. The envelope's is not — its
    /// zero is v1's exponential decay law — and reusing that here makes a
    /// route at its default curve deliver almost nothing until its source is
    /// near the top.
    #[test]
    fn a_route_curve_is_neutral_in_the_middle() {
        for position in [0.0, 0.25, 0.5, 0.75, 1.0] {
            assert!(
                (route_shape(position, 0.0) - position).abs() < 1.0e-6,
                "curve 0 moved {position}"
            );
        }
        // The ends are the same two shapes an envelope has.
        assert!(route_shape(0.5, 1.0) < 0.05, "the exponential end is not steep");
        assert!(route_shape(0.5, -1.0) > 0.95, "the logarithmic end is not flat");
        for curve in [-1.0, -0.5, 0.0, 0.5, 1.0] {
            assert!(route_shape(0.0, curve).abs() < 1.0e-6);
            assert!((route_shape(1.0, curve) - 1.0).abs() < 1.0e-6);
        }
    }

    /// Amount is an ordinary automatable parameter, which is how a channel
    /// LFO scales a per-hit relationship without knowing anything about
    /// voices. Source and Destination are structural and are not.
    #[test]
    fn a_route_amount_is_an_ordinary_parameter() {
        let amount_id = mooloop_core::matrix_param(0, ds01::MATRIX_OFFSET_AMOUNT);
        let params = routed(Ds01ModSource::Velocity, ds01::PARAM_FILTER_CUTOFF, 0.0);
        let quiet = Ds01Params {
            tone_level: 0.0,
            noise_level: 1.0,
            filter_morph: 0.0,
            ..params
        };

        // Scaled by an event mid-render, on a continuous destination, so it
        // reaches the hit that is already sounding.
        let mut node = Ds01::new(quiet, SR);
        let mut events = EventList::empty();
        events.push(note_on(0, 60, 127));
        events.push(param(4_800, amount_id, 1.0));
        let scaled = render(&mut node, 24_000, &events);

        let mut untouched = Ds01::new(quiet, SR);
        let mut plain = EventList::empty();
        plain.push(note_on(0, 60, 127));
        let flat = render(&mut untouched, 24_000, &plain);

        assert_eq!(scaled[..4_800], flat[..4_800], "the amount reached backwards");
        assert_ne!(
            scaled[12_000..],
            flat[12_000..],
            "scaling the amount did nothing"
        );

        // And Source and Destination refuse the same treatment.
        for offset in [ds01::MATRIX_OFFSET_SOURCE, ds01::MATRIX_OFFSET_DEST] {
            let id = mooloop_core::matrix_param(0, offset);
            let descriptor = ds01::descriptor(id).unwrap();
            assert!(matches!(
                descriptor.curve,
                mooloop_core::ParamCurve::Stepped(_)
            ));
        }
    }

    /// Routes add an offset around the base value rather than writing it, so
    /// a knob and a route compose instead of fighting — and a route to a
    /// latched destination is evaluated at the trigger, so the hit it shaped
    /// keeps that shape.
    #[test]
    fn a_route_offsets_the_base_and_latches_where_the_table_says() {
        // Turning the knob under a route moves the result by the same amount
        // it moves the knob.
        let with_pitch = |pitch: f32, amount: f32| {
            let mut params = routed(Ds01ModSource::Velocity, ds01::PARAM_TONE_PITCH, amount);
            params.tone_pitch = pitch;
            upward_crossings(&hit(params, 12_000)[480..9_600])
        };
        assert!(with_pitch(320.0, 0.0) > with_pitch(160.0, 0.0));
        assert!(with_pitch(160.0, 0.3) > with_pitch(160.0, 0.0));
        assert!(with_pitch(320.0, 0.3) > with_pitch(320.0, 0.0));

        // Amp Decay is latched, so a route onto it decides the hit's shape
        // once. Moving the route's amount mid-hit reaches the *next* one.
        let amount_id = mooloop_core::matrix_param(0, ds01::MATRIX_OFFSET_AMOUNT);
        let params = routed(Ds01ModSource::Velocity, ds01::PARAM_AMP_DECAY, 0.0);
        let mut node = Ds01::new(params, SR);
        let mut events = EventList::empty();
        events.push(note_on(0, 60, 127));
        events.push(param(2_400, amount_id, -1.0));
        let sounding = render(&mut node, 24_000, &events);

        let mut untouched = Ds01::new(params, SR);
        let mut plain = EventList::empty();
        plain.push(note_on(0, 60, 127));
        assert_eq!(
            sounding,
            render(&mut untouched, 24_000, &plain),
            "a latched destination followed a route mid-hit"
        );

        let mut next = EventList::empty();
        next.push(note_on(0, 60, 127));
        let after = render(&mut node, 24_000, &next);
        assert!(
            rms(&after[2_400..4_800]) < rms(&sounding[2_400..4_800]) * 0.5,
            "the next hit ignored the route"
        );
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
                amp: Ds01EnvParams::one_shot(2.0),
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
            amp: Ds01EnvParams::one_shot(4.0),
            ..Ds01Params::default()
        };
        let mut node = Ds01::new(params, SR);
        for _ in 0..DS01_VOICES + 4 {
            node.trigger(0, 60, 100);
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
        let plain = Ds01::preview_waveform(Ds01Params::default(), 96, 0.3);
        let altered = Ds01::preview_waveform(
            Ds01Params {
                tone_pitch: 700.0,
                tone_wave: 1.0,
                noise_level: 0.8,
                ..Ds01Params::default()
            },
            96,
            0.3,
        );
        assert_eq!(plain.0.len(), 96);
        assert_ne!(plain, altered);

        // The span is the caller's, and a longer one is a different drawing
        // rather than the same one with empty space after it.
        let long = Ds01::preview_waveform(Ds01Params::default(), 96, 4.0);
        assert_eq!(long.0.len(), 96);
        assert_ne!(plain, long);
        assert!(long.0.iter().chain(&long.1).all(|s| s.is_finite()));
        assert!(plain
            .0
            .iter()
            .chain(&plain.1)
            .all(|sample| (-1.0..=1.0).contains(sample)));
    }
}
