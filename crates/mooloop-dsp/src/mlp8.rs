//! The ML-P8: eight voices around a three-oscillator network.
//!
//! Deliberately not a variant of [`crate::polysynth`]. That device layers
//! three oscillators; this one wires them together, per
//! `docs/plans/poly-synth-v2/01-what-poly-is.md`. What is here now, from step
//! 02 of that plan:
//!
//! - six directed cross-modulation routes, one for every ordered pair, all
//!   reading the source's previous sample so the graph is causal and its
//!   result does not depend on the order the oscillators happen to be stored
//!   in,
//! - per-oscillator self-feedback through the same one-sample state, so an
//!   oscillator can be made unstable without spending a pair amount on it,
//! - noise into all three phase inputs, independent of whether noise is
//!   audible in the mixer,
//! - hard sync from any oscillator to any other, band-limited by
//!   [`crate::osc::sync_blep`] rather than reset naively,
//! - a derived sub that follows its source's pitch and sync reference but not
//!   its cross-modulation, so a fundamental stays underneath a mangled
//!   carrier,
//! - deterministic per-voice colored noise.
//!
//! Level is a mixer control, not an on switch: an oscillator at `-inf` still
//! runs, still modulates, and still syncs. The one thing that skips an
//! oscillator is nothing reading it at all, which is decided once per block.
//!
//! Step 03 put a voice around that network: two envelopes, a four-mode filter
//! with keytracking and both velocity depths, and a feedback loop with the
//! drive inside it.
//!
//! Step 04 is the instrument's own modulation, and it is why a complete ML-P8
//! patch needs nothing from the channel modulation shelf:
//!
//! - one global, audio-rate LFO with six waves, of which two are not periodic,
//!   plus Warp, Slew, and a retrigger policy,
//! - six per-voice sources — the LFO, both envelopes, velocity, key, and gate
//!   — reaching a list of continuous destinations through authored routes,
//! - routes compiled flat, evaluated per sample, and resolved as authored base
//!   plus offset then clamped through the destination's own descriptor range,
//!   so the knob keeps meaning what it says.
//!
//! Step 05 finishes the pool around all of it. A note is a *group* of physical
//! slots from that point on: Unison spends the eight rather than adding to
//! them, groups are allocated and stolen whole, and Detune and Spread place a
//! group's members symmetrically about the note that was played. Drift is
//! stable per-slot character with no runtime entropy anywhere in it, so Drift
//! 0 is not "almost authored" but bit-for-bit authored. The chorus is the last
//! thing in the path and the least important control on the face: four fixed
//! policies over the shared [`ModulationEffect`], running on ML-P8's own
//! scratch buses so it can never read what another generator already put on
//! the channel.

use crate::bus::{pan_gains, StereoBus};
use crate::effects::ModulationEffect;
use crate::env::Adsr;
use crate::event::{Event, EventList};
use crate::filter::{soft_ceiling, PreDrive, Svf};
use crate::node::{AudioNode, ProcessContext};
use crate::osc::{sync_blep, Noise, Osc};
use crate::scale::hz_from_normalized;
use crate::smooth::Smoothed;
use crate::synth_voice::{note_to_freq, MIN_GLIDE_S, PARAM_SMOOTH_S, STOP_RELEASE_S};
use mooloop_core::mlp8::{
    MlP8ControlOutlets, MlP8Routes, MLP8_CONTROL_OUTLETS, MLP8_MAX_ROUTES, MLP8_MOD_DESTS,
    OUTLET_AMP_ENV, OUTLET_FILTER_ENV, OUTLET_GATE, OUTLET_LFO, OUTLET_NOTE, OUTLET_TRIGGER,
    OUTLET_VELOCITY,
};
use mooloop_core::{
    mlp8::xmod_index, MlP8Chorus, MlP8FilterMode, MlP8LfoParams, MlP8LfoRetrigger, MlP8LfoWave,
    MlP8ModDest, MlP8ModSource, MlP8Params, ModulationMode, ModulationParams, OscWave, SubWave,
    MLP8_VOICES,
};

/// The voice's absolute output reference, set so one oscillator at its 0 dB
/// top (which the default patch runs at) peaks within a dB of
/// `mooloop_core::gain::REFERENCE_PEAK_DBFS` (-12 dBFS) at the master.
///
/// The same figure the v1 poly synth uses, and measured rather than derived:
/// the default patch is one band-limited saw into a VCA with nothing between,
/// which is exactly that synth's default path too. `gain_structure_tests`
/// holds it.
const VOICE_OUTPUT_REFERENCE: f32 = 0.51;

/// Phase deviation, in cycles, that one modulation route reaches at 100%.
///
/// Two whole cycles is well past the point where a phase-modulated oscillator
/// stops sounding like the waveform it started as, which is the range the
/// instrument is for. Provisional until step 07's listening pass: the plan
/// asks for subtle animation through metallic spectra before the last quarter
/// turns hostile, and only ears can say whether this is it.
const ROUTE_MAX_CYCLES: f32 = 2.0;

/// Total phase deviation the summed modulation inputs asymptote to.
///
/// Larger than [`ROUTE_MAX_CYCLES`] so two routes at full depth still add
/// rather than immediately squashing each other, and finite so no combination
/// of routes — including a self-feedback loop reading its own last sample —
/// can run the phase away.
const PHASE_BOUND_CYCLES: f32 = 4.0;

/// Mix level below which a source is genuinely silent rather than on its way
/// there. Well under the smallest step the level control can make and well
/// over `f32`'s subnormal range.
const LEVEL_EPSILON: f32 = 1.0e-6;

/// Middle C (MIDI 60). Keytracking is referenced here, so a patch voiced
/// around the middle of the keyboard keeps its cutoff where it was set.
const KEYTRACK_REFERENCE_HZ: f32 = 261.625_58;

/// Octaves the filter envelope sweeps at full depth.
const FILTER_ENV_OCTAVES: f32 = 6.0;

/// What the voice feedback control reaches at its extremes, as a fraction of
/// the filter's own output fed back to its input.
///
/// Chosen so the top of the knob sustains rather than merely gets loud: below
/// this the loop is a colour, above it the drive inside the loop is doing all
/// the work and the control stops changing anything. The bound is the drive
/// stage and [`soft_ceiling`], not a limiter after the voice sum -- that is
/// the plan's rule and it is what keeps feedback a timbre rather than a
/// volume.
const VOICE_FEEDBACK_RANGE: f32 = 1.15;

/// Cutoff below which the filter is treated as open and skipped entirely.
/// The normalized scale is perceptual, so this is the very top of the knob.
const FILTER_OPEN: f32 = 0.999;

/// Cents of pitch drift one slot reaches at Drift 100%.
///
/// A ceiling to tune in step 07, not an identity claim. Five cents is the
/// point where eight voices stop being eight copies and start being an
/// ensemble; past it the instrument sounds out of tune with itself rather
/// than alive.
const DRIFT_PITCH_CENTS: f32 = 5.0;

/// How much of that an individual oscillator adds on top of its voice's
/// shared offset.
///
/// Shared plus smaller independent, per the plan: the shared part is what
/// makes a *voice* sit a little off, and the independent part is what stops
/// its three oscillators being one detuned copy of each other.
const DRIFT_OSC_SHARE: f32 = 0.4;

/// Octaves of cutoff drift at Drift 100%.
const DRIFT_CUTOFF_OCTAVES: f32 = 0.15;

/// Fraction an envelope time is stretched or shortened at Drift 100%.
/// Attack, decay and release only: a drifting sustain is a drifting level,
/// which is a mix error rather than character.
const DRIFT_TIME_FRACTION: f32 = 0.07;

/// Cents the outermost member of a unison group reaches at Detune 100%.
///
/// The curve to it is squared, like every other signed depth on this device,
/// so the low half of the knob buys the fine beating a chorus of voices is
/// actually for.
const DETUNE_MAX_CENTS: f32 = 40.0;

/// Frames of ML-P8's own output the finisher works on at a time.
///
/// The chorus needs buses of its own — it may not read or rewrite the
/// channel's — and reserving [`crate::bus::MAX_BLOCK_SIZE`] for them would
/// put 128 KB on every materialized channel to serve a control that is off by
/// default, on a device that on most channels is not even the active
/// generator. Rendering in chunks
/// costs one `Prepared` and one character refresh per chunk, which is
/// nanoseconds against ten milliseconds of audio, and 512 frames is a whole
/// cycle at every buffer size this program is played at.
const CHORUS_CHUNK: usize = 512;

/// How long the chorus's wet gain takes to cross a mode change.
const CHORUS_FADE_S: f32 = 0.02;

/// Wet gain below which the chorus is treated as silent, which is what makes
/// `Off` an exact bypass rather than an inaudible one.
const CHORUS_SILENT: f32 = 1.0e-4;

/// One-pole corner the dark end of Noise Color rolls down to.
const NOISE_DARK_HZ: f32 = 700.0;
/// One-pole corner the bright end rolls up from.
const NOISE_BRIGHT_HZ: f32 = 4_000.0;

/// A modulation amount in signed percent, as a signed phase deviation in
/// cycles.
///
/// Squared in magnitude, so the low half of the knob buys fine animation and
/// the travel is not compressed into the first few percent. Sign is kept
/// separately rather than squaring the signed value, so an automation lane
/// passing through zero inverts the modulation phase instead of folding.
fn route_depth(percent: f32) -> f32 {
    let unit = (percent * 0.01).clamp(-1.0, 1.0);
    unit * unit.abs() * ROUTE_MAX_CYCLES
}

/// Smooth, bounded limit on the summed phase modulation.
///
/// `x / (1 + |x| / B)`: identity slope at zero, asymptotic to `±B`, and
/// differentiable everywhere, so it bounds the cyclic modulation graph
/// without flattening the musically useful range or introducing a corner an
/// automation sweep would hear. This is the safety mechanism the plan asks
/// for, and it is inside the sound rather than being a limiter bolted on
/// after the voice sum.
fn bound_phase(sum: f32) -> f32 {
    sum / (1.0 + sum.abs() / PHASE_BOUND_CYCLES)
}

/// Deterministic per-voice noise with a continuous dark/white/bright tilt.
///
/// Two one-pole states rather than one: the dark end wants a corner low enough
/// to be a rumble and the bright end wants one high enough to be air, and a
/// single shared corner cannot be both. The RMS compensation is derived from
/// the coefficient rather than tabulated, so the colour does not change level
/// with the sample rate.
#[derive(Clone, Copy)]
struct ColoredNoise {
    rng: Noise,
    dark: f32,
    bright: f32,
}

/// The per-block constants a [`ColoredNoise`] runs from. Derived once rather
/// than per sample, and per sample rate rather than per patch.
#[derive(Clone, Copy)]
struct NoiseColor {
    dark_coeff: f32,
    dark_gain: f32,
    bright_coeff: f32,
    bright_gain: f32,
}

impl NoiseColor {
    fn new(sample_rate: u32) -> Self {
        let coeff = |hz: f32| {
            1.0 - (-core::f32::consts::TAU * hz / sample_rate as f32).exp()
        };
        let dark_coeff = coeff(NOISE_DARK_HZ).clamp(1.0e-4, 1.0);
        let bright_coeff = coeff(NOISE_BRIGHT_HZ).clamp(1.0e-4, 1.0);
        // A one-pole low-pass of white noise has variance `a / (2 - a)`; the
        // matching high-pass has `1 - 2a + a / (2 - a)`. Both are inverted
        // here so the three colours are the same loudness.
        let lp_var = |a: f32| a / (2.0 - a);
        Self {
            dark_coeff,
            dark_gain: lp_var(dark_coeff).max(1.0e-6).sqrt().recip(),
            bright_coeff,
            bright_gain: (1.0 - 2.0 * bright_coeff + lp_var(bright_coeff))
                .max(1.0e-6)
                .sqrt()
                .recip(),
        }
    }
}

impl ColoredNoise {
    fn new(seed: u32) -> Self {
        Self {
            rng: Noise::new(seed),
            dark: 0.0,
            bright: 0.0,
        }
    }

    fn reset(&mut self, seed: u32) {
        self.rng.reset(seed);
        self.dark = 0.0;
        self.bright = 0.0;
    }

    /// `tilt` is the Noise Color knob as a unit value: `-1` dark, `0` white,
    /// `+1` bright.
    fn next_sample(&mut self, tilt: f32, color: &NoiseColor) -> f32 {
        let white = self.rng.next_sample();
        self.dark += color.dark_coeff * (white - self.dark);
        self.bright += color.bright_coeff * (white - self.bright);
        if tilt < 0.0 {
            let dark = self.dark * color.dark_gain;
            white + (dark - white) * -tilt
        } else {
            let bright = (white - self.bright) * color.bright_gain;
            white + (bright - white) * tilt
        }
    }
}

/// The voice's multimode filter: two cascaded state-variable stages, of which
/// the second only runs for the four-pole mode.
///
/// A response menu, not a character menu. The ML-M1's Model switch chooses
/// between three *different filters* with different saturation, and it needs
/// per-model makeup gain because they are not the same circuit. These four
/// come off one linear stage, so the only compensation they need is the one
/// below, and it is about slope rather than about character.
#[derive(Clone, Copy)]
struct VoiceFilter {
    first: Svf,
    second: Svf,
}

impl VoiceFilter {
    fn new() -> Self {
        Self {
            first: Svf::new(),
            second: Svf::new(),
        }
    }

    fn reset(&mut self) {
        self.first.reset();
        self.second.reset();
    }

    fn next_sample(
        &mut self,
        mode: MlP8FilterMode,
        input: f32,
        cutoff_hz: f32,
        resonance: f32,
        sample_rate: u32,
    ) -> f32 {
        match mode {
            MlP8FilterMode::Lp12 => self.first.next_sample(input, cutoff_hz, resonance, sample_rate),
            MlP8FilterMode::Lp24 => {
                // Two identical stages put the -3 dB point most of an octave
                // below one stage's, so the Cutoff knob would mean two
                // different frequencies depending on the slope. The corner is
                // pushed up by the amount a cascade drops it, and the
                // resonance is split so the pair peaks about as hard as the
                // single stage rather than twice as hard.
                let corrected = (cutoff_hz * LP24_CORNER_SCALE).min(sample_rate as f32 * 0.45);
                let shared = resonance * LP24_RESONANCE_SHARE;
                let first = self
                    .first
                    .next_sample(input, corrected, shared, sample_rate);
                self.second
                    .next_sample(first, corrected, shared, sample_rate)
            }
            MlP8FilterMode::Bp12 => {
                self.first
                    .next_sample_lp_bp_hp(input, cutoff_hz, resonance, sample_rate)
                    .1
            }
            MlP8FilterMode::Hp12 => {
                self.first
                    .next_sample_lp_hp(input, cutoff_hz, resonance, sample_rate)
                    .1
            }
        }
    }
}

/// Cutoff multiplier that puts LP24's corner where LP12's is. Two cascaded
/// one-pole-pair sections reach -3 dB at `sqrt(sqrt(2) - 1)` of a single
/// section's corner; this is its reciprocal.
const LP24_CORNER_SCALE: f32 = 1.553_774;

/// How much of the Resonance knob each LP24 stage gets. Resonance compounds
/// through a cascade, so splitting it keeps the knob meaning roughly the same
/// amount of peak in both low-pass modes.
const LP24_RESONANCE_SHARE: f32 = 0.62;

/// How much of one cycle full Slew rounds off.
///
/// A fraction of the cycle rather than a time in seconds, which is the whole
/// idea: a sample-and-hold at half Slew is the same wander at 0.2 Hz as at
/// 20 Hz, so the control survives being automated alongside Rate instead of
/// meaning something different at every speed.
const LFO_SLEW_MAX_CYCLES: f32 = 0.5;

/// The narrowest either half of a warped cycle may become. A pivot that
/// reaches the edge is a shape with no rising side at all, which is a
/// division by zero before it is a sound.
const LFO_WARP_MIN_SIDE: f32 = 0.02;

/// The chaos pair's frequency ratio. Irrational on purpose: a rational ratio
/// closes the figure and the "chaos" wave becomes a periodic one.
const CHAOS_RATIO: f32 = 0.618_034;

/// How hard each phasor bends the other's rate. Weak coupling lets the pair
/// phase-lock, which is the failure mode that would quietly turn this back
/// into a periodic wave, so it is set well past that.
const CHAOS_COUPLING: f32 = 0.85;

/// Seed for the sample-and-hold sequence. Fixed, so two renders of the same
/// events are the same samples.
const LFO_SH_SEED: u32 = 0x1d3f_a7c5;

/// ML-P8's own LFO: one global shape, evaluated per sample.
///
/// Global rather than per voice because it is the instrument's clock; the
/// route amounts are what make it land differently on each voice.
struct MlP8Lfo {
    phase: f32,
    /// The current sample-and-hold value, already warped.
    hold: f32,
    noise: Noise,
    /// The chaos generator's whole state: two phasors, each modulating the
    /// other's rate. Bounded by construction because the output is a sum of
    /// sines rather than an accumulator that could run away.
    chaos: [f32; 2],
    /// The slew filter's memory, which is also the LFO's output.
    slewed: f32,
    /// Whether `slewed` has ever been written. A slew starting from zero
    /// would ramp in from silence on the first note; starting from the
    /// shape's own first value does not.
    primed: bool,
}

impl MlP8Lfo {
    fn new() -> Self {
        let mut noise = Noise::new(LFO_SH_SEED);
        let hold = noise.next_sample();
        Self {
            phase: 0.0,
            hold,
            noise,
            // Not both zero: identical phases stay identical forever, and the
            // pair would sum to one sine rather than wander.
            chaos: [0.0, 0.37],
            slewed: 0.0,
            primed: false,
        }
    }

    /// The value this LFO last emitted.
    ///
    /// The same number the voices were driven with, not a second evaluation
    /// of the shape: the `LFO` outlet's contract is that it publishes the
    /// signal the instrument actually used, and re-deriving it here would be
    /// a way for the two to disagree.
    fn value(&self) -> f32 {
        self.slewed
    }

    /// Restart the cycle. The accumulator goes to zero rather than to the
    /// Phase parameter, because Phase is a read offset — see `value_at`.
    fn retrigger(&mut self) {
        self.phase = 0.0;
        self.chaos = [0.0, 0.37];
        self.hold = self.noise.next_sample();
        self.primed = false;
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    /// The rate this LFO is running at, in Hz.
    fn rate_hz(params: &MlP8LfoParams, bpm: f64) -> f32 {
        if params.synced {
            params.rate_division.rate_hz(bpm)
        } else {
            params.rate_hz
        }
    }

    /// One sample, bipolar in `[-1, 1]`.
    fn next_sample(&mut self, params: &MlP8LfoParams, bpm: f64, sample_rate: u32) -> f32 {
        let sr = sample_rate as f32;
        // A quarter of the sample rate is the same ceiling the shared LFO
        // uses: past it the "shape" is decided by where the samples land
        // rather than by the wave.
        let rate = Self::rate_hz(params, bpm).clamp(0.0, sr * 0.25);
        let dt = rate / sr;

        let raw = match params.wave {
            MlP8LfoWave::Chaos => {
                self.advance_chaos(dt);
                bias(0.5 * (sin_tau(self.chaos[0]) + sin_tau(self.chaos[1])), params.warp)
            }
            MlP8LfoWave::SampleHold => self.hold,
            wave => {
                let read = (self.phase + params.phase).rem_euclid(1.0);
                periodic_shape(warp_phase(read, params.warp), wave)
            }
        };

        self.advance(dt, params);
        self.slew(raw, rate, params.slew, sr)
    }

    /// Advance the phase accumulator, refreshing the held value on the wrap.
    fn advance(&mut self, dt: f32, params: &MlP8LfoParams) {
        let next = self.phase + dt;
        if next >= 1.0 {
            // Warp biases the *distribution* here rather than skewing a phase,
            // because a held value has no phase to skew. One control, and the
            // wave decides which of its two honest meanings applies.
            self.hold = bias(self.noise.next_sample(), params.warp);
        }
        self.phase = next.fract();
    }

    /// Two phasors, each bending the other's rate.
    ///
    /// Deterministic, allocation-free, and bounded without a clamp: the
    /// output is a sum of sines, so there is no accumulator that can escape.
    /// The phases themselves are kept in `[0, 1)` so they cannot drift into
    /// the range where `f32` stops resolving a cycle.
    fn advance_chaos(&mut self, dt: f32) {
        let a = sin_tau(self.chaos[0]);
        let b = sin_tau(self.chaos[1]);
        self.chaos[0] = (self.chaos[0] + dt * (1.0 + CHAOS_COUPLING * b)).rem_euclid(1.0);
        self.chaos[1] =
            (self.chaos[1] + dt * (CHAOS_RATIO + CHAOS_COUPLING * a)).rem_euclid(1.0);
    }

    /// A one-pole whose time constant is a fraction of the cycle.
    fn slew(&mut self, target: f32, rate_hz: f32, amount: f32, sample_rate: f32) -> f32 {
        if !self.primed {
            self.slewed = target;
            self.primed = true;
            return target;
        }
        let amount = amount.clamp(0.0, 1.0);
        if amount <= 0.0 || rate_hz <= 0.0 {
            self.slewed = target;
            return target;
        }
        let tau = amount * LFO_SLEW_MAX_CYCLES / rate_hz;
        let coeff = (-1.0 / (tau * sample_rate)).exp();
        self.slewed = target + (self.slewed - target) * coeff;
        self.slewed
    }
}

fn sin_tau(phase: f32) -> f32 {
    (phase * core::f32::consts::TAU).sin()
}

/// Skew a phase about a moved pivot, so half the cycle is spent on each side
/// of it. Turns a triangle into a ramp, a pulse into a variable width, and a
/// sine into a shape that leans.
fn warp_phase(phase: f32, warp: f32) -> f32 {
    let warp = warp.clamp(-1.0, 1.0);
    if warp == 0.0 {
        return phase;
    }
    let pivot = (0.5 * (1.0 - warp)).clamp(LFO_WARP_MIN_SIDE, 1.0 - LFO_WARP_MIN_SIDE);
    if phase < pivot {
        0.5 * phase / pivot
    } else {
        0.5 + 0.5 * (phase - pivot) / (1.0 - pivot)
    }
}

/// Bias a bipolar value toward the extremes or toward the centre, keeping its
/// sign and its bounds. This is Warp's meaning for the two waves that have no
/// phase: positive pushes values out to the rails, negative pulls them in.
fn bias(value: f32, warp: f32) -> f32 {
    let warp = warp.clamp(-1.0, 1.0);
    if warp == 0.0 {
        return value;
    }
    let exponent = if warp > 0.0 {
        1.0 - 0.75 * warp
    } else {
        1.0 - 2.0 * warp
    };
    value.signum() * value.abs().powf(exponent)
}

/// The four waves that have a phase. All leave zero rising at phase zero, so
/// a retriggered LFO never steps the sound at the note boundary.
fn periodic_shape(phase: f32, wave: MlP8LfoWave) -> f32 {
    match wave {
        MlP8LfoWave::Sine => sin_tau(phase),
        MlP8LfoWave::Triangle => 1.0 - 4.0 * ((phase + 0.25).fract() - 0.5).abs(),
        MlP8LfoWave::Ramp => 2.0 * (phase + 0.5).fract() - 1.0,
        MlP8LfoWave::Pulse => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        // Handled by the caller; neither is periodic.
        MlP8LfoWave::SampleHold | MlP8LfoWave::Chaos => 0.0,
    }
}

/// One internal route, reduced to the work the audio path actually does.
///
/// No descriptor id, no enum to match on a destination, no lookup: a source
/// index, a slot index, and the factor to multiply by. Everything that needed
/// a table was resolved when the topology was compiled.
///
/// The two indices are bytes rather than words on purpose. There are six
/// sources and thirty-one destinations, one node of this device exists on
/// every live channel, and a `usize` pair per row costs 320 bytes there to
/// address a range that fits in two.
#[derive(Clone, Copy)]
struct CompiledRoute {
    /// The authored route's durable id. Carried so an amount arriving from an
    /// automation lane can find its row without the topology being rebuilt.
    id: u16,
    source: u8,
    slot: u8,
    /// The destination's full span, which is what an amount of 100% covers.
    /// Kept per row so [`CompiledRoutes::set_amount`] needs no descriptor.
    span: f32,
    scale: f32,
}

/// A patch's routes, compiled flat.
struct CompiledRoutes {
    routes: [CompiledRoute; MLP8_MAX_ROUTES],
    len: usize,
    /// Which destination slots any route touches, so a voice reads an offset
    /// only where one can exist. An unrouted patch does no per-sample work at
    /// all.
    touched: [bool; MLP8_MOD_DESTS],
    /// The span each destination clamps into, resolved once here rather than
    /// looked up per sample. Indexed by slot, like everything else the voice
    /// reads back.
    bounds: [(f32, f32); MLP8_MOD_DESTS],
    any: bool,
}

impl CompiledRoutes {
    fn new() -> Self {
        Self {
            routes: [CompiledRoute {
                id: 0,
                source: 0,
                slot: 0,
                span: 0.0,
                scale: 0.0,
            }; MLP8_MAX_ROUTES],
            len: 0,
            touched: [false; MLP8_MOD_DESTS],
            bounds: std::array::from_fn(|slot| MlP8ModDest::ALL[slot].range()),
            any: false,
        }
    }

    /// Resolve the authored route list into flat operations.
    ///
    /// Bounded and allocation-free — sixteen routes against a fixed
    /// destination list — which is what makes it safe to run from the
    /// parameter drain rather than needing a separate prepared-topology
    /// handoff. It is not per block and never per sample: it runs when the
    /// patch's topology changes.
    ///
    /// A route at zero amount keeps its row. It costs one multiply by zero,
    /// and it is what lets an automation lane sweep that amount up from
    /// silence through [`Self::set_amount`] instead of forcing a rebuild in
    /// the middle of a block.
    fn compile(&mut self, routes: &MlP8Routes) {
        self.len = 0;
        self.touched = [false; MLP8_MOD_DESTS];
        for route in routes.iter() {
            if self.len == MLP8_MAX_ROUTES {
                break;
            }
            let Some(slot) = route.dest.slot() else {
                continue;
            };
            if !route.dest.is_legal() {
                continue;
            }
            let span = route.dest.full_range();
            self.routes[self.len] = CompiledRoute {
                id: route.id,
                source: route.source.to_index() as u8,
                slot: slot as u8,
                span,
                scale: route_scale(route.amount, span),
            };
            self.len += 1;
            self.touched[slot] = true;
        }
        self.any = self.len > 0;
    }

    /// Move one route's depth, by durable id, leaving the topology alone.
    ///
    /// This is the path a route-amount automation lane takes every control
    /// tick, so it does no allocation, no descriptor lookup, and no work
    /// proportional to anything but the route count.
    fn set_amount(&mut self, id: u16, amount: f32) -> bool {
        for route in &mut self.routes[..self.len] {
            if route.id == id {
                route.scale = route_scale(amount, route.span);
                return true;
            }
        }
        false
    }

    /// Take new depths from a route list of the *same* topology.
    ///
    /// The compiled rows are a subsequence of the authored routes, in order,
    /// so this walks both once rather than searching. The caller has already
    /// established that the topology matches.
    fn retune(&mut self, routes: &MlP8Routes) {
        let mut next = 0usize;
        for authored in routes.iter() {
            let Some(row) = self.routes[..self.len].get_mut(next) else {
                break;
            };
            if row.id != authored.id {
                continue;
            }
            row.scale = route_scale(authored.amount, row.span);
            next += 1;
        }
    }
}

/// A route's authored percent, as the offset one unit of its source produces
/// in the destination's own units.
fn route_scale(percent: f32, span: f32) -> f32 {
    (percent * 0.01).clamp(-1.0, 1.0) * span
}

/// The per-voice values the routes read, in [`MlP8ModSource::ALL`] order.
///
/// An array rather than a struct because a compiled route holds its source as
/// an index: that is the whole reason the audio path does not match on an
/// enum per sample.
type ModSources = [f32; MlP8ModSource::ALL.len()];

/// Dense slot indices the voice reads back. Derived from the same `ALL` order
/// the UI lists, rather than written out twice.
mod slot {
    use mooloop_core::mlp8::{MlP8ModDest, MLP8_MOD_DESTS};

    /// Resolve at startup rather than per sample, and panic loudly here
    /// rather than silently mis-routing if the destination list is reordered.
    const fn find(target: MlP8ModDest) -> usize {
        let mut i = 0;
        while i < MLP8_MOD_DESTS {
            if MlP8ModDest::ALL[i].same(target) {
                return i;
            }
            i += 1;
        }
        panic!("destination is not in MlP8ModDest::ALL")
    }

    pub const OSC_SEMIS: [usize; 3] = [
        find(MlP8ModDest::Param {
            id: mooloop_core::mlp8::osc_param(0, mooloop_core::mlp8::OSC_OFFSET_SEMITONES),
        }),
        find(MlP8ModDest::Param {
            id: mooloop_core::mlp8::osc_param(1, mooloop_core::mlp8::OSC_OFFSET_SEMITONES),
        }),
        find(MlP8ModDest::Param {
            id: mooloop_core::mlp8::osc_param(2, mooloop_core::mlp8::OSC_OFFSET_SEMITONES),
        }),
    ];
    pub const OSC_WIDTH: [usize; 3] = [
        find(MlP8ModDest::Param {
            id: mooloop_core::mlp8::osc_param(0, mooloop_core::mlp8::OSC_OFFSET_PULSE_WIDTH),
        }),
        find(MlP8ModDest::Param {
            id: mooloop_core::mlp8::osc_param(1, mooloop_core::mlp8::OSC_OFFSET_PULSE_WIDTH),
        }),
        find(MlP8ModDest::Param {
            id: mooloop_core::mlp8::osc_param(2, mooloop_core::mlp8::OSC_OFFSET_PULSE_WIDTH),
        }),
    ];
    pub const OSC_LEVEL: [usize; 3] = [
        find(MlP8ModDest::Param {
            id: mooloop_core::mlp8::osc_param(0, mooloop_core::mlp8::OSC_OFFSET_LEVEL),
        }),
        find(MlP8ModDest::Param {
            id: mooloop_core::mlp8::osc_param(1, mooloop_core::mlp8::OSC_OFFSET_LEVEL),
        }),
        find(MlP8ModDest::Param {
            id: mooloop_core::mlp8::osc_param(2, mooloop_core::mlp8::OSC_OFFSET_LEVEL),
        }),
    ];
    pub const SUB_LEVEL: usize = find(MlP8ModDest::Param {
        id: mooloop_core::mlp8::PARAM_SUB_LEVEL,
    });
    pub const NOISE_LEVEL: usize = find(MlP8ModDest::Param {
        id: mooloop_core::mlp8::PARAM_NOISE_LEVEL,
    });
    pub const NOISE_COLOR: usize = find(MlP8ModDest::Param {
        id: mooloop_core::mlp8::PARAM_NOISE_COLOR,
    });
    pub const XMOD: usize = find(MlP8ModDest::Param {
        id: mooloop_core::mlp8::PARAM_XMOD_BASE,
    });
    pub const NOISE_TO_OSC: usize = find(MlP8ModDest::Param {
        id: mooloop_core::mlp8::PARAM_NOISE_TO_OSC_BASE,
    });
    pub const OSC_FEEDBACK: usize = find(MlP8ModDest::Param {
        id: mooloop_core::mlp8::PARAM_OSC_FEEDBACK_BASE,
    });
    pub const VOICE_FEEDBACK: usize = find(MlP8ModDest::Param {
        id: mooloop_core::mlp8::PARAM_VOICE_FEEDBACK,
    });
    pub const CUTOFF: usize = find(MlP8ModDest::Param {
        id: mooloop_core::mlp8::PARAM_FILTER_CUTOFF,
    });
    pub const RESONANCE: usize = find(MlP8ModDest::Param {
        id: mooloop_core::mlp8::PARAM_FILTER_RESONANCE,
    });
    pub const ENV_AMOUNT: usize = find(MlP8ModDest::Param {
        id: mooloop_core::mlp8::PARAM_FILTER_ENV_AMOUNT,
    });
    pub const DRIVE: usize = find(MlP8ModDest::Param {
        id: mooloop_core::mlp8::PARAM_DRIVE,
    });
    pub const VCA_LEVEL: usize = find(MlP8ModDest::VcaLevel);
    pub const PAN: usize = find(MlP8ModDest::Pan);
}

/// The stable per-slot offsets Drift scales, all bipolar in `[-1, 1]` except
/// the start phases, which are cycles in `[0, 1)`.
///
/// Derived from the slot index at construction and never touched again. That
/// is the whole point: the plan forbids runtime entropy, so "how far is this
/// voice off" has to be a property of the slot rather than of when the note
/// arrived, and an offline render has to reproduce a live take exactly.
#[derive(Clone, Copy)]
struct Drift {
    /// The whole voice's pitch offset, shared by its three oscillators.
    voice_pitch: f32,
    /// Each oscillator's smaller offset on top of that.
    osc_pitch: [f32; 3],
    cutoff: f32,
    attack: f32,
    decay: f32,
    release: f32,
    /// Where each oscillator starts its cycle, in `[0, 1)`.
    phase: [f32; 3],
}

impl Drift {
    fn for_slot(slot: u32) -> Self {
        // One deterministic sequence per slot rather than eight hand-written
        // tables: the numbers themselves carry no meaning, only their being
        // fixed and different does.
        let mut rng = Noise::new(drift_seed(slot));
        let mut bipolar = || rng.next_sample();
        Self {
            voice_pitch: bipolar(),
            osc_pitch: std::array::from_fn(|_| bipolar()),
            cutoff: bipolar(),
            attack: bipolar(),
            decay: bipolar(),
            release: bipolar(),
            phase: std::array::from_fn(|_| bipolar() * 0.5 + 0.5),
        }
    }
}

/// A slot's drift seed. A different odd multiplier from [`noise_seed`], so a
/// voice's character is not a restatement of its noise sequence.
fn drift_seed(slot: u32) -> u32 {
    0x85EB_CA6B_u32.wrapping_mul(slot.wrapping_add(1)) | 1
}

/// ML-P8's finishing chorus: one shared [`ModulationEffect`], four fixed
/// policies, and the wet gain that crosses between them.
///
/// It is not a modulation source and its delay LFO is not published. The
/// authored LFO on the MOD page is the instrument's clock; this one is part
/// of a finishing processor and belongs to nobody.
struct Chorus {
    effect: ModulationEffect,
    /// The mode the effect is currently configured for, which lags the
    /// patch's by however long the wet gain takes to reach zero.
    active: MlP8Chorus,
    /// The wet gain. A mode change is a topology change inside the finisher,
    /// so it happens with this already at zero rather than under the audio.
    gain: Smoothed,
}

impl Chorus {
    fn new(mode: MlP8Chorus, sample_rate: u32) -> Self {
        Self {
            effect: ModulationEffect::new(mode_params(mode), sample_rate),
            active: mode,
            // A project that saved with a chorus opens with it, rather than
            // fading it in over the first twenty milliseconds of the song.
            gain: Smoothed::new(
                f32::from(u8::from(mode != MlP8Chorus::Off)),
                CHORUS_FADE_S,
                sample_rate,
            ),
        }
    }

    fn reset(&mut self, mode: MlP8Chorus, sample_rate: u32) {
        *self = Self::new(mode, sample_rate);
    }

    /// Bring the stage in line with the patch, and say whether it can be
    /// skipped entirely.
    ///
    /// Called at range boundaries, not per sample: the swap has to wait for
    /// the gain to run down anyway, so deciding it once a range costs at most
    /// one block of extra fade and saves a branch in the sample loop.
    fn settle(&mut self, wanted: MlP8Chorus) -> bool {
        if self.active != wanted && self.gain.value() <= CHORUS_SILENT {
            self.active = wanted;
            self.gain.reset_to(0.0);
            // The line holds audio from before the change, and at `Off` from
            // before an arbitrarily long silence. Nothing should read it, and
            // clearing it is what lets `Off` skip the processor outright
            // instead of having to keep writing silence through it.
            self.effect.reset();
            if wanted != MlP8Chorus::Off {
                self.effect.set_params(mode_params(wanted));
            }
        }
        let reached = self.active == wanted && wanted != MlP8Chorus::Off;
        self.gain.set_target(f32::from(u8::from(reached)));
        self.active == MlP8Chorus::Off && self.gain.value() <= CHORUS_SILENT
    }
}

/// The four fixed policies, as settings of the shared modulation effect.
///
/// Provisional until step 07's listening pass, which is also where the plan
/// says a Mix control would have to be argued for. There is none here: the
/// dry passes at unity and the wet is added beside it, which is how a chorus
/// on a synth's output behaves and what keeps the modes distinguishable
/// without a second knob.
fn mode_params(mode: MlP8Chorus) -> ModulationParams {
    match mode {
        // Never read: `Off` clears the effect and skips it. Kept a legal
        // value so constructing the stage does not need a special case.
        MlP8Chorus::Off => ModulationParams::default(),
        // I: slow and narrow. The one that makes a pad sit in a mix without
        // announcing itself.
        MlP8Chorus::One => ModulationParams {
            mode: ModulationMode::Chorus,
            rate_hz: 0.42,
            depth: 0.30,
            color: 0.30,
            feedback: 0.0,
            spread: 0.55,
            tone: 0.80,
            stages: 8,
        },
        // II: faster, deeper, and with a little feedback, so it is a
        // different decision rather than more of the first one.
        MlP8Chorus::Two => ModulationParams {
            mode: ModulationMode::Chorus,
            rate_hz: 1.15,
            depth: 0.62,
            color: 0.60,
            feedback: 0.18,
            spread: 0.90,
            tone: 0.70,
            stages: 8,
        },
        // Ensemble: the wider three-tap algorithm, which is the one that
        // stops sounding like a delay and starts sounding like more players.
        MlP8Chorus::Ensemble => ModulationParams {
            mode: ModulationMode::Ensemble,
            rate_hz: 0.70,
            depth: 0.55,
            color: 0.45,
            feedback: 0.0,
            spread: 1.0,
            tone: 0.75,
            stages: 8,
        },
    }
}

/// One physical voice. Eight of these exist for the life of the device.
struct Voice {
    active: bool,
    /// Whether this voice's note is still held.
    ///
    /// Distinct from [`Self::active`], which stays true through the release
    /// tail. This is the `Gate` modulation source, and it is also what the
    /// LFO's `Chord` retrigger policy asks about: "is a note already down".
    gate: bool,
    event_id: u64,
    note: u8,
    /// This voice's note as a bipolar offset from middle C, which is what the
    /// `Key` source reads. Four octaves either side reaches full depth: that
    /// spans a piano, so an ordinary keyboard uses the whole control rather
    /// than the middle third of it.
    key: f32,
    age: u64,
    /// Stable across the device's life, so a voice's noise sequence and its
    /// per-slot drift are properties of the slot rather than of the order
    /// notes arrived in.
    slot: u32,
    /// Where this voice sits in its note group, and how large that group is.
    ///
    /// A group is allocated, stolen and released whole. These two are what
    /// make Detune and Spread symmetric *about the note that was played*
    /// rather than about the pool, so a group's centre pitch is the note and
    /// its centre of image is the middle.
    member: u8,
    members: u8,
    /// This slot's stable drift offsets. Fixed for the device's life.
    drift: Drift,
    env: Adsr,
    filter_env: Adsr,
    filter: VoiceFilter,
    drive: PreDrive,
    /// The filter's previous output. The explicit one sample of delay that
    /// makes the voice's feedback loop causal, exactly as `taps` does for the
    /// oscillator network.
    feedback_tap: f32,
    /// Blocks the DC a resonant filter under asymmetric drive can accumulate
    /// in the loop; without it a feedback patch slowly walks off centre.
    dc_x: f32,
    dc_y: f32,
    oscs: [Osc; 3],
    /// Each oscillator's previous pre-Level sample. This is the modulation
    /// tap, and the one sample of delay that makes the cyclic graph causal.
    taps: [f32; 3],
    /// Band-limited step corrections owed to the next sample by a sync reset.
    sync_carry: [f32; 3],
    sub: Osc,
    sub_carry: f32,
    noise: ColoredNoise,
    noise_tap: f32,
    current_freq: f32,
    target_freq: f32,
    /// The velocity this voice's note was played at, in `[0, 1]`.
    ///
    /// Kept beside the smoothed gain because they answer different
    /// questions. The smoother is what the VCA uses, and on a stolen slot it
    /// is still sliding from the previous note; the `Velocity` outlet
    /// publishes the note's own velocity, which is a fact about the event
    /// rather than about the ramp.
    velocity: f32,
    /// Velocity gain, smoothed so a stolen retrigger at a different velocity
    /// slides rather than steps.
    velocity_amp: Smoothed,
    osc_level: [Smoothed; 3],
    sub_level: Smoothed,
    noise_level: Smoothed,
    cutoff: Smoothed,
    drive_amount: Smoothed,
    feedback: Smoothed,
    /// This voice's summed internal-route offsets, in each destination's own
    /// units, indexed by [`MlP8ModDest::slot`]. Written once per sample and
    /// read wherever the destination is used; only the slots a route actually
    /// touches are ever written or read.
    mod_offsets: [f32; MLP8_MOD_DESTS],
    /// Drift and Detune resolved into one frequency multiplier per
    /// oscillator, refreshed once per render range.
    ///
    /// Exactly `1.0` when both controls are at zero, and `x * 1.0` is `x`
    /// bit for bit — which is what makes "Drift 0 is exactly authored" an
    /// identity rather than a tolerance.
    pitch_scale: [f32; 3],
    /// Drift's cutoff offset as a frequency multiplier. Exactly `1.0` at
    /// Drift 0, for the same reason.
    cutoff_scale: f32,
    /// Where Spread puts this voice, in `[-1, 1]`, and the pan gains it
    /// resolves to. Both are constant across a render range, so the gains are
    /// computed once rather than per sample; at Spread 0 they are the centre
    /// pair the channel strip would have applied anyway.
    spread_pan: f32,
    spread_gain: (f32, f32),
}

impl Voice {
    fn new(slot: u32, sample_rate: u32) -> Self {
        let smoothed = |initial| Smoothed::new(initial, PARAM_SMOOTH_S, sample_rate);
        Self {
            active: false,
            gate: false,
            event_id: 0,
            note: 0,
            key: 0.0,
            age: 0,
            slot,
            env: Adsr::new(sample_rate),
            filter_env: Adsr::new(sample_rate),
            filter: VoiceFilter::new(),
            drive: PreDrive::new(),
            feedback_tap: 0.0,
            dc_x: 0.0,
            dc_y: 0.0,
            oscs: [Osc::new(); 3],
            taps: [0.0; 3],
            sync_carry: [0.0; 3],
            sub: Osc::new(),
            sub_carry: 0.0,
            member: 0,
            members: 1,
            drift: Drift::for_slot(slot),
            noise: ColoredNoise::new(noise_seed(slot)),
            noise_tap: 0.0,
            current_freq: 0.0,
            target_freq: 0.0,
            velocity: 0.0,
            velocity_amp: smoothed(0.0),
            osc_level: [smoothed(0.0); 3],
            sub_level: smoothed(0.0),
            noise_level: smoothed(0.0),
            cutoff: smoothed(1.0),
            drive_amount: smoothed(0.0),
            feedback: smoothed(0.0),
            mod_offsets: [0.0; MLP8_MOD_DESTS],
            pitch_scale: [1.0; 3],
            cutoff_scale: 1.0,
            spread_pan: 0.0,
            spread_gain: pan_gains(0.0),
        }
    }

    /// Return every piece of network state to its start. Called when a slot
    /// that was genuinely idle takes a note, so a repeated note renders
    /// identically rather than inheriting a phase from whatever came before.
    ///
    /// "Its start" is where Drift puts it. At Drift 0 that is phase zero for
    /// all three, which is exactly what [`Osc::reset`] does.
    fn restart(&mut self, drift: f32) {
        for (osc, phase) in self.oscs.iter_mut().zip(self.drift.phase) {
            osc.reset_to(drift * phase);
        }
        self.taps = [0.0; 3];
        self.sync_carry = [0.0; 3];
        self.sub.reset();
        self.sub_carry = 0.0;
        self.noise.reset(noise_seed(self.slot));
        self.noise_tap = 0.0;
        self.filter.reset();
        self.drive = PreDrive::new();
        self.mod_offsets = [0.0; MLP8_MOD_DESTS];
        self.clear_loop();
    }

    /// Forget what the feedback loop was holding.
    ///
    /// Separate from [`Self::restart`] because it happens in two places that
    /// restart does not cover: when a slot falls idle, and when a *sounding*
    /// slot is stolen. Stealing deliberately keeps the oscillator phases —
    /// restarting them under a running envelope is a click — but keeping the
    /// loop as well would let the new note be played by the old note's tail,
    /// which is the one thing the plan says a reassigned slot must not do.
    fn clear_loop(&mut self) {
        self.feedback_tap = 0.0;
        self.dc_x = 0.0;
        self.dc_y = 0.0;
    }

    /// Leave through the short de-click transition without adopting a note.
    ///
    /// A slot stolen by a *smaller* group has nothing to take on, and simply
    /// dropping it would be the one sample of silence in the middle of a
    /// sound that stealing exists to avoid. Its age goes to zero so it is
    /// first in line to be reused rather than outliving notes still held; two
    /// retiring slots sharing that age look like one group to
    /// [`MlP8::select_group`], which is harmless because both are leaving.
    fn retire(&mut self) {
        self.gate = false;
        self.age = 0;
        if self.active && !self.env.is_releasing() {
            self.env.release_with(STOP_RELEASE_S);
            self.filter_env.release_with(STOP_RELEASE_S);
        }
    }

    /// Sum this sample's internal routes into the offset table.
    ///
    /// Flat, branchless per route, and proportional to the routes a patch has
    /// rather than to the destination list: no descriptor is consulted, no
    /// enum is matched, and an unrouted patch returns immediately.
    fn resolve_routes(&mut self, routes: &CompiledRoutes, sources: &ModSources) {
        if !routes.any {
            return;
        }
        // Two passes over the routes rather than one over a list of the
        // destinations they touch. Clearing a shared destination twice is
        // free; keeping that list was thirty-two bytes on every voice of
        // every channel to save at most fifteen stores.
        for route in &routes.routes[..routes.len] {
            self.mod_offsets[route.slot as usize] = 0.0;
        }
        for route in &routes.routes[..routes.len] {
            self.mod_offsets[route.slot as usize] +=
                sources[route.source as usize] * route.scale;
        }
    }

    /// A phase-modulation amount, resolved before its curve is applied.
    ///
    /// The prepared value is returned untouched when nothing routes to this
    /// amount, so the common case pays one branch and no `powf`.
    #[inline]
    fn osc_depth(
        &self,
        routes: &CompiledRoutes,
        slot: usize,
        percent: f32,
        prepared: f32,
    ) -> f32 {
        if !routes.touched[slot] {
            return prepared;
        }
        route_depth(self.dest(routes, slot, percent))
    }

    /// One destination, resolved: authored base plus this voice's offsets,
    /// clamped through the destination's own descriptor range.
    ///
    /// The clamp is what keeps a routed value a value the knob could also
    /// have been set to, so "base plus offset" never means a cutoff past the
    /// top of its scale or a level above unity.
    #[inline]
    fn dest(&self, routes: &CompiledRoutes, slot: usize, base: f32) -> f32 {
        if !routes.touched[slot] {
            return base;
        }
        let (min, max) = routes.bounds[slot];
        (base + self.mod_offsets[slot]).clamp(min, max)
    }

    /// Resolve Drift, Detune, and Spread into the three per-voice multipliers
    /// the sample loop reads.
    ///
    /// Once per render range rather than per sample. All three are patch
    /// controls that an automation lane can sweep, and none of them is a
    /// modulation destination — so a range boundary is the finest they can
    /// change, and paying for them per sample would buy nothing.
    fn refresh_character(&mut self, drift: f32, detune: f32, spread: f32) {
        // Squared magnitude, like every other depth on this device, so the
        // low half of the knob is the fine beating rather than a rounding
        // error. Zero is exactly zero cents, not a small number.
        let detune_cents = detune * detune * DETUNE_MAX_CENTS * self.group_offset();
        let voice_cents = drift * self.drift.voice_pitch * DRIFT_PITCH_CENTS;
        self.pitch_scale = std::array::from_fn(|n| {
            let osc_cents = drift * self.drift.osc_pitch[n] * DRIFT_PITCH_CENTS * DRIFT_OSC_SHARE;
            ((detune_cents + voice_cents + osc_cents) / 1200.0).exp2()
        });
        self.cutoff_scale = (drift * self.drift.cutoff * DRIFT_CUTOFF_OCTAVES).exp2();
        self.spread_pan = spread * self.pan_offset();
        self.spread_gain = pan_gains(self.spread_pan);
    }

    /// This voice's symmetric position in its note group, in `[-1, 1]`.
    ///
    /// Zero-mean, which is what makes a detuned group still play the note it
    /// was sent. A group of one is at the centre by construction, so Detune
    /// at 1x moves nothing rather than moving everything the same way.
    fn group_offset(&self) -> f32 {
        let members = f32::from(self.members.max(1));
        if members <= 1.0 {
            0.0
        } else {
            f32::from(self.member) / (members - 1.0) * 2.0 - 1.0
        }
    }

    /// Where Spread puts this voice before the amount is applied.
    ///
    /// Across the group above 1x, and across the eight stable slot positions
    /// at 1x — so a chord occupies the field by where its notes landed, and
    /// occupies it the same way on every render rather than moving with the
    /// order the notes happened to arrive in.
    fn pan_offset(&self) -> f32 {
        if self.members > 1 {
            self.group_offset()
        } else {
            self.slot as f32 / (MLP8_VOICES - 1) as f32 * 2.0 - 1.0
        }
    }

    fn snap_to(&mut self, params: &MlP8Params, velocity_amp: f32) {
        self.velocity_amp.reset_to(velocity_amp);
        for (smoothed, osc) in self.osc_level.iter_mut().zip(params.osc.iter()) {
            smoothed.reset_to(osc.level.clamp(0.0, 1.0));
        }
        self.sub_level.reset_to(params.sub_level.clamp(0.0, 1.0));
        self.noise_level.reset_to(params.noise_level.clamp(0.0, 1.0));
        self.cutoff.reset_to(params.filter_cutoff.clamp(0.0, 1.0));
        self.drive_amount.reset_to(params.drive.clamp(0.0, 1.0));
        self.feedback.reset_to(params.voice_feedback.clamp(-1.0, 1.0));
    }
}

/// A voice slot's noise seed. Derived from the slot index by an odd
/// multiplier so the eight voices decorrelate, and fixed for the life of the
/// device so an offline render and a live take produce the same samples.
fn noise_seed(slot: u32) -> u32 {
    0x9E37_79B9_u32.wrapping_mul(slot.wrapping_add(1)) | 1
}

/// Everything about the patch that is constant across a render range.
///
/// Built once per range rather than consulted per sample, and it is where the
/// route topology is decided: an oscillator nothing reads is skipped, an
/// oscillator that is only a modulator is not.
struct Prepared<'a> {
    /// The patch's routes, compiled. Borrowed rather than copied: it is the
    /// one part of the prepared state that is rebuilt on a topology change
    /// rather than per render range.
    routes: &'a CompiledRoutes,
    ratio: [f32; 3],
    /// The authored pitch offset in semitones, kept apart from the cents so a
    /// route can move it and still be clamped through the semitone control's
    /// own range. Only read when something routes to it.
    semitones: [f32; 3],
    /// The cents half of the same tuning, as a ratio. Not routable on its own
    /// — a route reaching pitch reaches Semis, and cents stay the fine offset
    /// the patch authored.
    cents_ratio: [f32; 3],
    wave: [OscWave; 3],
    pulse_width: [f32; 3],
    /// `xmod[from][to]`, already in cycles.
    xmod: [[f32; 3]; 3],
    /// The same amounts as authored percent, for the routed path: the curve
    /// from percent to cycles has to be applied *after* the offset, or a
    /// route would move a number that has already been squared.
    xmod_percent: [[f32; 3]; 3],
    feedback: [f32; 3],
    feedback_percent: [f32; 3],
    noise_to_osc: [f32; 3],
    noise_to_osc_percent: [f32; 3],
    sync_master: [Option<usize>; 3],
    osc_needed: [bool; 3],
    noise_needed: bool,
    noise_tilt: f32,
    noise_color_percent: f32,
    sub_source: usize,
    /// How far below its source the sub sits. Kept as the divisor rather than
    /// as a finished ratio because the sub follows its source's *resolved*
    /// pitch, which is not known until the routes for that sample are in.
    sub_divisor: f32,
    sub_wave: OscWave,
    sub_needed: bool,
    color: NoiseColor,
    mode: MlP8FilterMode,
    resonance: f32,
    env_amount: f32,
    filter_velocity: f32,
    keytrack: f32,
    amp_velocity: f32,
    max_hz: f32,
    /// Whether the filter can be skipped for the whole range. Only true when
    /// it is wide open, unresonant, and nothing is moving it.
    filter_open: bool,
}

impl<'a> Prepared<'a> {
    fn new(params: &MlP8Params, sample_rate: u32, routes: &'a CompiledRoutes) -> Self {
        let mut ratio = [1.0_f32; 3];
        let mut semitones = [0.0_f32; 3];
        let mut cents_ratio = [1.0_f32; 3];
        let mut wave = [OscWave::Saw; 3];
        let mut pulse_width = [0.5_f32; 3];
        for (index, osc) in params.osc.iter().enumerate() {
            semitones[index] = osc.semitones.clamp(-48.0, 48.0);
            cents_ratio[index] = (osc.cents.clamp(-100.0, 100.0) / 1200.0).exp2();
            ratio[index] = (semitones[index] / 12.0).exp2() * cents_ratio[index];
            wave[index] = osc.wave;
            pulse_width[index] = osc.pulse_width;
        }

        let mut xmod = [[0.0_f32; 3]; 3];
        let mut xmod_percent = [[0.0_f32; 3]; 3];
        for (from, row) in xmod.iter_mut().enumerate() {
            for (to, depth) in row.iter_mut().enumerate() {
                if from != to {
                    xmod_percent[from][to] = params.xmod[xmod_index(from, to)];
                    *depth = route_depth(xmod_percent[from][to]);
                }
            }
        }
        let feedback_percent = params.osc_feedback;
        let noise_to_osc_percent = params.noise_to_osc;
        let feedback = std::array::from_fn(|n| route_depth(feedback_percent[n]));
        let noise_to_osc = std::array::from_fn(|n| route_depth(noise_to_osc_percent[n]));
        let sync_master: [Option<usize>; 3] = std::array::from_fn(|n| {
            // An oscillator syncing to itself is not a topology, it is a
            // stuck phase. The UI excludes it; this makes the DSP agree
            // whatever a project file says.
            params.sync_source[n].master().filter(|m| *m != n)
        });

        let sub_source = params.sub_source.index();
        // A route reaching a source's level means the authored level is no
        // longer the whole answer, so "nothing reads this" stops being a
        // question this block can settle. Skipping it would replace whatever
        // the route was about to do with silence.
        let routed = |slot: usize| routes.touched[slot];
        let sub_needed = params.sub_level > 0.0 || routed(slot::SUB_LEVEL);
        let audible = |n: usize| params.osc[n].level > 0.0 || routed(slot::OSC_LEVEL[n]);
        // The same for the amounts that decide whether one oscillator reaches
        // another: an XMOD route can wake a path the knobs left at zero.
        let modulates = |from: usize, to: usize| {
            xmod[from][to] != 0.0 || routed(slot::XMOD + xmod_index(from, to))
        };

        // What makes an oscillator live: it is heard, it modulates something,
        // it syncs something, or the sub divides it. Level alone does not
        // decide, because a muted oscillator is a legitimate modulator — that
        // is the point of the device.
        let osc_needed: [bool; 3] = std::array::from_fn(|n| {
            audible(n)
                || (0..3).any(|to| to != n && modulates(n, to))
                || feedback[n] != 0.0
                || routed(slot::OSC_FEEDBACK + n)
                || sync_master.contains(&Some(n))
                || (sub_needed && sub_source == n)
        });
        let noise_needed = params.noise_level > 0.0
            || routed(slot::NOISE_LEVEL)
            || (0..3).any(|n| noise_to_osc[n] != 0.0 || routed(slot::NOISE_TO_OSC + n));

        Self {
            routes,
            ratio,
            semitones,
            cents_ratio,
            wave,
            pulse_width,
            xmod,
            xmod_percent,
            feedback,
            feedback_percent,
            noise_to_osc,
            noise_to_osc_percent,
            sync_master,
            osc_needed,
            noise_needed,
            noise_tilt: (params.noise_color * 0.01).clamp(-1.0, 1.0),
            noise_color_percent: params.noise_color,
            sub_source,
            sub_divisor: params.sub_octave.divisor(),
            sub_wave: match params.sub_wave {
                SubWave::Sine => OscWave::Sine,
                SubWave::Square => OscWave::Pulse,
            },
            sub_needed,
            color: NoiseColor::new(sample_rate),
            mode: params.filter_mode,
            resonance: params.filter_resonance.clamp(0.0, 1.0),
            env_amount: params.filter_env_amount.clamp(-1.0, 1.0),
            filter_velocity: params.filter_velocity.clamp(-1.0, 1.0),
            keytrack: params.filter_keytrack.clamp(0.0, 2.0),
            amp_velocity: params.amp_velocity.clamp(0.0, 1.0),
            max_hz: sample_rate as f32 * 0.45,
            // A band-pass or high-pass at the top of its range is not "no
            // filter", so only the low-pass modes can be skipped. A route
            // aimed at any of these is one more thing that can move the
            // filter, so the whole shortcut stands down.
            filter_open: matches!(params.filter_mode, MlP8FilterMode::Lp12 | MlP8FilterMode::Lp24)
                && params.filter_cutoff >= FILTER_OPEN
                && params.filter_resonance <= f32::EPSILON
                && params.filter_env_amount.abs() <= f32::EPSILON
                && params.filter_velocity.abs() <= f32::EPSILON
                && params.filter_keytrack <= f32::EPSILON
                && params.drive <= f32::EPSILON
                && params.voice_feedback.abs() <= f32::EPSILON
                && !routed(slot::CUTOFF)
                && !routed(slot::RESONANCE)
                && !routed(slot::ENV_AMOUNT)
                && !routed(slot::DRIVE)
                && !routed(slot::VOICE_FEEDBACK),
        }
    }
}

/// The ML-P8 node.
pub struct MlP8 {
    params: MlP8Params,
    sample_rate: u32,
    voices: [Voice; MLP8_VOICES],
    next_age: u64,
    /// The instrument's own LFO. One per device rather than one per voice:
    /// it is the instrument's clock, and the route amounts are what make it
    /// land differently on each voice.
    lfo: MlP8Lfo,
    /// The authored routes, flattened. Rebuilt only when the topology moves.
    routes: CompiledRoutes,
    /// The age of the **focus group**: the group created by the most recent
    /// Note On, which is what the per-voice control outlets reduce over.
    ///
    /// It stays the focus through its release, so an envelope outlet has a
    /// coherent tail rather than snapping to an older chord note that
    /// happens to still be held. Zero means nothing has played yet, and no
    /// group ever carries that age.
    focus: u64,
    /// Whether a Note On has arrived since the outlets were last published.
    ///
    /// `Trigger` is "a note started since you last looked", so it is defined
    /// by the publication cadence rather than by a length in samples. That
    /// is also why publishing is the thing that clears it.
    triggered: bool,
    /// ML-P8's own summed output, before the finisher, and the chorus's wet
    /// copy of it.
    ///
    /// Allocated once at construction and never resized. The chorus processes
    /// *these*, never the channel bus, so it cannot read and rewrite audio
    /// another generator on the same channel already put there.
    scratch: StereoBus,
    wet: StereoBus,
    chorus: Chorus,
}

impl MlP8 {
    pub fn new(params: MlP8Params, sample_rate: u32) -> Self {
        let mut synth = Self {
            params,
            sample_rate,
            voices: std::array::from_fn(|slot| Voice::new(slot as u32, sample_rate)),
            next_age: 1,
            lfo: MlP8Lfo::new(),
            focus: 0,
            triggered: false,
            routes: CompiledRoutes::new(),
            scratch: StereoBus::with_capacity(CHORUS_CHUNK),
            wet: StereoBus::with_capacity(CHORUS_CHUNK),
            chorus: Chorus::new(params.chorus, sample_rate),
        };
        synth.routes.compile(&synth.params.routes);
        synth.apply_params_to_voices();
        synth
    }

    /// Replace the parameter set. Called from the RT command drain.
    ///
    /// The route table is rebuilt only when the *topology* moved, and merely
    /// retuned when the depths did. That matters because this is also the
    /// path every ordinary knob takes: a cutoff automation lane must not
    /// rebuild the topology sixteen times a block, and it would if a differing
    /// depth counted — which it does, the moment a route amount has been
    /// automated away from what the arriving parameter block still carries.
    pub fn set_params(&mut self, params: MlP8Params) {
        if self.params.routes.same_topology(&params.routes) {
            self.routes.retune(&params.routes);
        } else {
            self.routes.compile(&params.routes);
        }
        // Unison is the one control that changes what a *group* is, and a
        // group's size is fixed when it is allocated. Growing or shrinking a
        // sounding one in place would leave half a group behind, so the old
        // topology leaves through the same short transition a transport stop
        // uses and the new one applies to the next Note On.
        let regrouped = self.params.unison != params.unison;
        self.params = params;
        if regrouped {
            self.release_all();
        }
        self.apply_params_to_voices();
    }

    /// Apply one descriptor-addressed parameter, leaving the rest alone.
    ///
    /// Routed through `set_params` so a control-rate change gets exactly the
    /// same clamping a whole-struct update does. Both are non-allocating.
    fn apply_param(&mut self, id: u32, value: f32) {
        let mut params = mooloop_core::GeneratorParams::MlP8(self.params);
        if params.set(id, value).is_none() {
            return;
        }
        if let mooloop_core::GeneratorParams::MlP8(params) = params {
            self.set_params(params);
        }
    }

    /// Move one internal route's depth, by durable id.
    ///
    /// Separate from [`Self::apply_param`] because a route amount is not in
    /// the device's parameter table: it belongs to the route, and the route's
    /// id is its address. Writing it here rather than through `set_params`
    /// is also what keeps the promise that automating a route amount never
    /// rebuilds the topology.
    pub fn set_route_amount(&mut self, id: u16, amount: f32) {
        if self.params.routes.set_amount(id, amount) {
            // Read back rather than reused: the authored setter clamps, and
            // the compiled scale has to be the value the patch actually holds.
            let clamped = self
                .params
                .routes
                .get(id)
                .map(|route| route.amount)
                .unwrap_or(amount);
            self.routes.set_amount(id, clamped);
        }
    }

    fn apply_params_to_voices(&mut self) {
        // Drift's envelope share is applied here rather than in the sample
        // loop because an envelope time is only read when the stage it
        // belongs to is entered. At Drift 0 every factor is exactly 1.0 and
        // the times are the ones the patch authored.
        let drift = self.params.drift.clamp(0.0, 1.0);
        for voice in &mut self.voices {
            let stretch = |offset: f32| 1.0 + drift * offset * DRIFT_TIME_FRACTION;
            // Sustain is a level, not a time. A drifting sustain would be a
            // mix error wearing character's clothes, so the plan excludes it
            // and so does this.
            voice.env.configure(
                self.params.attack * stretch(voice.drift.attack),
                self.params.decay * stretch(voice.drift.decay),
                self.params.sustain,
                self.params.release * stretch(voice.drift.release),
            );
            voice.filter_env.configure(
                self.params.filter_attack * stretch(voice.drift.attack),
                self.params.filter_decay * stretch(voice.drift.decay),
                self.params.filter_sustain,
                self.params.filter_release * stretch(voice.drift.release),
            );
        }
    }

    /// Immediately invalidate every voice and return the whole network to its
    /// initial state.
    pub fn reset(&mut self) {
        for (slot, voice) in self.voices.iter_mut().enumerate() {
            *voice = Voice::new(slot as u32, self.sample_rate);
        }
        self.next_age = 1;
        self.focus = 0;
        self.triggered = false;
        self.lfo.reset();
        self.chorus.reset(self.params.chorus, self.sample_rate);
        self.routes.compile(&self.params.routes);
        self.apply_params_to_voices();
    }

    pub fn choke(&mut self) {
        self.release_all();
    }

    /// Choose `size` slots for one note group: idle ones first, then whole
    /// older groups, oldest first.
    ///
    /// Returns the slots the new group will use and a mask of every slot the
    /// choice *reserved*, which is a larger set whenever the group being
    /// stolen is bigger than the one replacing it. Never a partial group in
    /// either direction: a group is the unit of allocation, of stealing, and
    /// of release, so half of one is a state this device does not have.
    ///
    /// Eight is the whole pool. There is no polyphony parameter to consult
    /// and Unison divides rather than multiplies, which is what makes the
    /// stealing rule and the CPU ceiling honest.
    fn select_group(&self, size: usize) -> ([usize; MLP8_VOICES], u8) {
        let mut chosen = [0usize; MLP8_VOICES];
        let mut count = 0;
        let mut reserved: u8 = 0;
        for (index, voice) in self.voices.iter().enumerate() {
            if count == size {
                break;
            }
            if !voice.active {
                chosen[count] = index;
                count += 1;
                reserved |= 1 << index;
            }
        }
        while count < size {
            // Age is a group property: every member of a group is stamped
            // with the same one when it is allocated, so "the oldest age
            // still on the board" names a whole group rather than a slot.
            let Some(oldest) = self
                .voices
                .iter()
                .enumerate()
                .filter(|(index, voice)| voice.active && reserved & (1 << index) == 0)
                .map(|(_, voice)| voice.age)
                .min()
            else {
                break;
            };
            for (index, voice) in self.voices.iter().enumerate() {
                if voice.active && reserved & (1 << index) == 0 && voice.age == oldest {
                    reserved |= 1 << index;
                    if count < size {
                        chosen[count] = index;
                        count += 1;
                    }
                }
            }
        }
        (chosen, reserved)
    }

    /// Whether any note is currently held. Held, not sounding: a chord whose
    /// notes are all in release is over as far as the LFO is concerned.
    fn any_gate_held(&self) -> bool {
        self.voices.iter().any(|voice| voice.gate)
    }

    fn note_on(&mut self, event_id: u64, note: u8, velocity: u8) {
        // Asked before the new note takes its slot, because `Chord` means
        // "the first note of a chord" and this note is not yet one of them.
        let retrigger_lfo = match self.params.lfo.retrigger {
            MlP8LfoRetrigger::Free => false,
            MlP8LfoRetrigger::Chord => !self.any_gate_held(),
            MlP8LfoRetrigger::Note => true,
        };
        let size = self.params.unison.voices().min(MLP8_VOICES);
        let (chosen, reserved) = self.select_group(size);
        let velocity_amp = f32::from(velocity) / 127.0;
        let age = self.next_age;
        self.next_age = self.next_age.wrapping_add(1).max(1);
        // The newest group is always the focus, which is also what makes
        // stealing need no rule of its own: the stealing Note On is the new
        // focus event.
        self.focus = age;
        self.triggered = true;
        let drift = self.params.drift.clamp(0.0, 1.0);

        // A stolen group can be larger than the one taking its place — a
        // patch that was at 8x when the note started and is at 2x now. The
        // six slots nobody reuses do not simply stop: they leave through the
        // same short de-click transition, because a group is released whole.
        for index in 0..MLP8_VOICES {
            if reserved & (1 << index) != 0 && !chosen[..size].contains(&index) {
                self.voices[index].retire();
            }
        }

        for (member, &index) in chosen[..size].iter().enumerate() {
            let voice = &mut self.voices[index];
            let stolen = voice.active;
            voice.event_id = event_id;
            voice.note = note;
            voice.key = key_offset(note);
            voice.age = age;
            voice.member = member as u8;
            voice.members = size as u8;
            voice.target_freq = note_to_freq(note);
            voice.velocity = velocity_amp;
            voice.active = true;
            voice.gate = true;

            if !stolen {
                // Fresh slot: no glide from silence, and every piece of
                // network state starts where it started last time.
                voice.current_freq = voice.target_freq;
                voice.restart(drift);
                voice.snap_to(&self.params, velocity_amp);
            } else {
                voice.clear_loop();
                if self.params.glide <= MIN_GLIDE_S {
                    voice.current_freq = voice.target_freq;
                }
            }
            voice.velocity_amp.set_target(velocity_amp);
            voice.env.note_on();
            voice.filter_env.note_on();
        }
        if retrigger_lfo {
            self.lfo.retrigger();
        }
    }

    fn note_off(&mut self, event_id: u64) {
        for voice in self
            .voices
            .iter_mut()
            .filter(|voice| voice.active && voice.event_id == event_id)
        {
            voice.gate = false;
            voice.env.release();
            voice.filter_env.release();
        }
    }

    fn release_all(&mut self) {
        for voice in &mut self.voices {
            voice.gate = false;
            if voice.active && !voice.env.is_releasing() {
                voice.env.release_with(STOP_RELEASE_S);
                voice.filter_env.release_with(STOP_RELEASE_S);
            }
        }
    }

    /// Render `start..end` of the channel bus, in pieces no longer than the
    /// finisher's scratch.
    ///
    /// The chunk is not a block: everything inside is either per sample or a
    /// function of the patch, so splitting a range changes nothing about what
    /// comes out — the bit-identity tests below cover exactly that. What it
    /// buys is a finisher that costs kilobytes a channel rather than
    /// [`crate::bus::MAX_BLOCK_SIZE`] ones.
    /// The instrument's published control signals, in outlet-id order.
    ///
    /// Reading is what clears `Trigger`, because a trigger is "a note started
    /// since you last looked" rather than a pulse of some length in samples:
    /// its width is the publication cadence, and only the caller knows that.
    /// The engine publishes once a block, which is what the outlet table
    /// declares; two Note Ons inside one block therefore raise `Trigger`
    /// once, and that is the documented rate rather than a dropped event.
    /// Every other outlet is a level, and reading one changes nothing.
    ///
    /// This is publication, not telemetry. The values are the ones the voices
    /// were actually driven with — the LFO reports what it emitted rather
    /// than re-deriving its shape — and nothing here is smoothed, sampled or
    /// dropped for display's sake.
    pub fn publish_outlets(&mut self) -> MlP8ControlOutlets {
        // The focus group, reduced. A group is allocated whole, so its
        // members share an age; averaging over them is what makes a unison
        // group publish one envelope rather than eight.
        let mut members = 0.0_f32;
        let mut amp = 0.0_f32;
        let mut filter = 0.0_f32;
        let mut velocity = 0.0_f32;
        let mut note = 0.0_f32;
        for voice in self.voices.iter().filter(|voice| voice.active) {
            if voice.age != self.focus {
                continue;
            }
            members += 1.0;
            amp += voice.env.level();
            filter += voice.filter_env.level();
            // The group's members share a note and a velocity, so these are
            // the same value however many times they are added.
            velocity = voice.velocity;
            note = f32::from(voice.note) / 127.0;
        }
        // When the focus group falls idle its outlets return to zero rather
        // than jumping backward to an older held note. That is the plan's
        // rule and it is what makes the tail readable: a decay that ends is
        // a decay, not a step to whatever else is down.
        let (amp, filter) = if members > 0.0 {
            (amp / members, filter / members)
        } else {
            (0.0, 0.0)
        };
        let triggered = std::mem::take(&mut self.triggered);

        let mut outlets: MlP8ControlOutlets = [0.0; MLP8_CONTROL_OUTLETS];
        outlets[OUTLET_LFO as usize] = self.lfo.value();
        outlets[OUTLET_AMP_ENV as usize] = amp;
        outlets[OUTLET_FILTER_ENV as usize] = filter;
        outlets[OUTLET_VELOCITY as usize] = if members > 0.0 { velocity } else { 0.0 };
        outlets[OUTLET_NOTE as usize] = if members > 0.0 { note } else { 0.0 };
        // Gate is deliberately not the focus group's gate. "Any note is
        // held" is the useful channel-level fact and it does not fall when
        // the newest note of a chord is released while an older one is still
        // down. It follows the scheduled Note On/Off, not the VCA's release
        // tail, so it is `gate` rather than `active`.
        outlets[OUTLET_GATE as usize] = f32::from(u8::from(self.any_gate_held()));
        outlets[OUTLET_TRIGGER as usize] = f32::from(u8::from(triggered));
        outlets
    }

    fn render_range(&mut self, bus: &mut StereoBus, start: usize, end: usize, bpm: f64) {
        let limit = self.scratch.capacity().max(1);
        let mut pos = start;
        while pos < end {
            let chunk = (pos + limit).min(end);
            self.render_chunk(bus, pos, chunk, bpm);
            pos = chunk;
        }
    }

    fn render_chunk(&mut self, bus: &mut StereoBus, start: usize, end: usize, bpm: f64) {
        if start >= end {
            return;
        }
        // Decided once a range, before anything is rendered: a mode change
        // has to wait for the wet gain to run down anyway.
        let bypass_chorus = self.chorus.settle(self.params.chorus);
        // Split the borrows by field: the prepared state holds the compiled
        // routes for the whole range while the voices are being written.
        let Self {
            params,
            sample_rate,
            voices,
            lfo,
            routes,
            scratch,
            wet,
            chorus,
            ..
        } = self;
        let params = *params;
        let sr = *sample_rate;
        let prepared = Prepared::new(&params, sr, routes);
        let glide_coeff = (-1.0 / (params.glide.max(MIN_GLIDE_S) * sr as f32)).exp();
        // With the chorus off the voices go straight onto the channel bus and
        // the finisher costs nothing at all — not a copy, not a delay line
        // write, not a branch in the sample loop. That is what "OFF is a true
        // bypass" has to mean.
        //
        // The channel bus is written at absolute frame indices; the scratch is
        // written from zero, because it is sized for one chunk rather than for
        // whatever range the caller asked for. `offset` is the difference.
        let offset = if bypass_chorus { 0 } else { start };
        let target: &mut StereoBus = if bypass_chorus {
            &mut *bus
        } else {
            for index in 0..end - start {
                scratch.l[index] = 0.0;
                scratch.r[index] = 0.0;
            }
            &mut *scratch
        };

        for voice in voices.iter_mut() {
            for (smoothed, osc) in voice.osc_level.iter_mut().zip(params.osc.iter()) {
                smoothed.set_target(osc.level.clamp(0.0, 1.0));
            }
            voice.sub_level.set_target(params.sub_level.clamp(0.0, 1.0));
            voice
                .noise_level
                .set_target(params.noise_level.clamp(0.0, 1.0));
            voice.cutoff.set_target(params.filter_cutoff.clamp(0.0, 1.0));
            voice.drive_amount.set_target(params.drive.clamp(0.0, 1.0));
            voice
                .feedback
                .set_target(params.voice_feedback.clamp(-1.0, 1.0));
            voice.refresh_character(
                params.drift.clamp(0.0, 1.0),
                params.detune.clamp(0.0, 1.0),
                params.spread.clamp(0.0, 1.0),
            );
        }

        for frame in start..end {
            // Global and free-running: advanced once per sample whether or not
            // anything is sounding, so a note landing mid-cycle finds the LFO
            // where the transport says it should be rather than where the last
            // note left it.
            let lfo_value = lfo.next_sample(&params.lfo, bpm, sr);

            for voice in voices.iter_mut() {
                if !voice.active {
                    continue;
                }
                voice.env.advance();
                voice.filter_env.advance();
                if voice.env.is_idle() {
                    voice.active = false;
                    voice.gate = false;
                    voice.clear_loop();
                    continue;
                }
                voice.current_freq +=
                    (voice.target_freq - voice.current_freq) * (1.0 - glide_coeff);
                let velocity = voice.velocity_amp.advance();
                // In `MlP8ModSource::ALL` order, which is the order a
                // compiled route's source index means.
                let sources: ModSources = [
                    lfo_value,
                    voice.env.level(),
                    voice.filter_env.level(),
                    velocity,
                    voice.key,
                    f32::from(u8::from(voice.gate)),
                ];
                voice.resolve_routes(routes, &sources);

                let mix = voice.next_sample(&prepared, sr);
                let shaped = voice.shape(&prepared, mix, velocity, sr);
                // Velocity at the VCA is a crossfade from "every note the
                // same" to "every note as played", not a multiply -- at zero
                // depth a soft note is a full-level note rather than silence.
                let amp = 1.0 - prepared.amp_velocity * (1.0 - velocity);
                // The voice's own level and position. Both rest at the value
                // the channel strip already provides -- unity and centre --
                // so with nothing routed they cost one untaken branch and
                // change not a sample.
                let voice_level = voice.dest(routes, slot::VCA_LEVEL, 1.0);
                // Spread is the base a Pan route moves from, so the two are
                // one position rather than two that fight. With neither in
                // play the pair is the centre one the channel strip would
                // have applied anyway, resolved once a range.
                let (gain_l, gain_r) = if routes.touched[slot::PAN] {
                    pan_gains(voice.dest(routes, slot::PAN, voice.spread_pan))
                } else {
                    voice.spread_gain
                };
                let sample =
                    shaped * voice.env.level() * amp * voice_level * VOICE_OUTPUT_REFERENCE;
                target.l[frame - offset] += sample * gain_l;
                target.r[frame - offset] += sample * gain_r;
            }
        }

        if !bypass_chorus {
            // The wet is a copy the effect turns into its own output, because
            // `ModulationEffect` returns wet in place and the dry has to
            // survive it. Dry at unity, wet beside it: physical voices and
            // the finisher both sum honestly, and there is no hidden divider
            // anywhere in this device.
            let frames = end - start;
            wet.l[..frames].copy_from_slice(&scratch.l[..frames]);
            wet.r[..frames].copy_from_slice(&scratch.r[..frames]);
            chorus.effect.process_wet(wet, 0, frames);
            for index in 0..frames {
                let gain = chorus.gain.advance();
                bus.l[start + index] += scratch.l[index] + wet.l[index] * gain;
                bus.r[start + index] += scratch.r[index] + wet.r[index] * gain;
            }
        }
    }
}

/// A note as a bipolar offset from middle C, which is what the `Key`
/// modulation source reads.
///
/// Full depth four octaves either side: that spans a piano, so an ordinary
/// keyboard uses the whole control rather than its middle third, and the two
/// dozen MIDI notes past each end of one simply hold at the rail.
fn key_offset(note: u8) -> f32 {
    ((f32::from(note) - 60.0) / 48.0).clamp(-1.0, 1.0)
}

impl Voice {
    /// One sample of the whole oscillator network, before the VCA.
    ///
    /// The order inside is the causality argument: every modulation input is
    /// read from the previous sample's taps, then all three oscillators
    /// advance, then sync resets are applied from wraps that all three
    /// reported. Nothing reads a value another oscillator produced this
    /// sample, so the six routes can all be active at once and the output does
    /// not depend on which oscillator is stored first.
    fn next_sample(&mut self, prep: &Prepared, sample_rate: u32) -> f32 {
        // Mix levels come first, because they decide what still has to run.
        // `Prepared` reads the *target* level, so a source turned down to
        // silence is "not needed" while its smoother is still on the way
        // there — and skipping it a block early would replace the ramp the
        // smoother exists for with a step.
        let routes = prep.routes;
        let smoothed_level: [f32; 3] = std::array::from_fn(|n| self.osc_level[n].advance());
        let smoothed_sub = self.sub_level.advance();
        let smoothed_noise = self.noise_level.advance();
        let level: [f32; 3] =
            std::array::from_fn(|n| self.dest(routes, slot::OSC_LEVEL[n], smoothed_level[n]));
        let sub_level = self.dest(routes, slot::SUB_LEVEL, smoothed_sub);
        let noise_level = self.dest(routes, slot::NOISE_LEVEL, smoothed_noise);
        let live: [bool; 3] =
            std::array::from_fn(|n| prep.osc_needed[n] || level[n] > LEVEL_EPSILON);
        let sub_live = prep.sub_needed || sub_level > LEVEL_EPSILON;
        let noise_live = prep.noise_needed || noise_level > LEVEL_EPSILON;

        let noise = if noise_live {
            let tilt = if routes.touched[slot::NOISE_COLOR] {
                (self.dest(routes, slot::NOISE_COLOR, prep.noise_color_percent) * 0.01)
                    .clamp(-1.0, 1.0)
            } else {
                prep.noise_tilt
            };
            self.noise.next_sample(tilt, &prep.color)
        } else {
            0.0
        };

        // Resolved once for all three, because the sub divides one of them
        // and has to follow the same answer.
        //
        // Drift and Detune are folded in here rather than applied to the
        // frequency, so the sub — which reads `ratio[sub_source]` — inherits
        // both without a second copy of the rule. Both are exactly `1.0` when
        // their controls are at zero, so this multiply is the identity.
        let ratio: [f32; 3] = std::array::from_fn(|n| {
            let authored = if routes.touched[slot::OSC_SEMIS[n]] {
                (self.dest(routes, slot::OSC_SEMIS[n], prep.semitones[n]) / 12.0).exp2()
                    * prep.cents_ratio[n]
            } else {
                prep.ratio[n]
            };
            authored * self.pitch_scale[n]
        });

        let mut value = [0.0_f32; 3];
        let mut wrap = [None; 3];
        let mut freq = [0.0_f32; 3];
        let mut offset = [0.0_f32; 3];
        for index in 0..3 {
            if !live[index] {
                continue;
            }
            // Every amount below is resolved as authored percent plus this
            // voice's offset, and only *then* mapped through `route_depth`.
            // Applying the offset to the already-curved value would move a
            // number that has been squared, so an amount would mean something
            // different at every point on the knob.
            let mut phase_mod = self.osc_depth(
                routes,
                slot::OSC_FEEDBACK + index,
                prep.feedback_percent[index],
                prep.feedback[index],
            ) * self.taps[index]
                + self.osc_depth(
                    routes,
                    slot::NOISE_TO_OSC + index,
                    prep.noise_to_osc_percent[index],
                    prep.noise_to_osc[index],
                ) * self.noise_tap;
            for source in 0..3 {
                if source != index {
                    phase_mod += self.osc_depth(
                        routes,
                        slot::XMOD + xmod_index(source, index),
                        prep.xmod_percent[source][index],
                        prep.xmod[source][index],
                    ) * self.taps[source];
                }
            }
            offset[index] = bound_phase(phase_mod);
            freq[index] = self.current_freq * ratio[index];
            let width = self.dest(routes, slot::OSC_WIDTH[index], prep.pulse_width[index]);
            let step = self.oscs[index].next_step(
                freq[index],
                prep.wave[index],
                width,
                offset[index],
                sample_rate,
            );
            value[index] = step.value + self.sync_carry[index];
            self.sync_carry[index] = 0.0;
            wrap[index] = step.wrap;
        }

        for index in 0..3 {
            let Some(master) = prep.sync_master[index] else {
                continue;
            };
            let (true, Some(frac)) = (live[index], wrap[master]) else {
                continue;
            };
            let height = self.oscs[index].sync_reset(
                frac,
                freq[index],
                prep.wave[index],
                self.dest(routes, slot::OSC_WIDTH[index], prep.pulse_width[index]),
                offset[index],
                sample_rate,
            );
            let (now, next) = sync_blep(height, frac);
            value[index] += now;
            self.sync_carry[index] = next;
        }

        // Sub follows its source's base pitch and its sync reference, and
        // nothing else: no cross-modulation reaches it, which is what leaves
        // a fundamental standing under a carrier that has been taken apart.
        let sub = if sub_live {
            // The sub divides its source, so it follows that oscillator's
            // *resolved* pitch. Reading the authored ratio here would leave
            // the fundamental behind the moment a route moved the oscillator
            // it is derived from.
            let sub_freq = self.current_freq * ratio[prep.sub_source] / prep.sub_divisor;
            let step = self
                .sub
                .next_step(sub_freq, prep.sub_wave, 0.5, 0.0, sample_rate);
            let mut sub = step.value + self.sub_carry;
            self.sub_carry = 0.0;
            if let Some(master) = prep.sync_master[prep.sub_source] {
                if let Some(frac) = wrap[master] {
                    let height =
                        self.sub
                            .sync_reset(frac, sub_freq, prep.sub_wave, 0.5, 0.0, sample_rate);
                    let (now, next) = sync_blep(height, frac);
                    sub += now;
                    self.sub_carry = next;
                }
            }
            sub
        } else {
            0.0
        };

        // The taps are the pre-Level signals, so muting an oscillator in the
        // mixer leaves every route that reads it untouched.
        self.taps = value;
        self.noise_tap = noise;

        let mut mix = 0.0;
        for index in 0..3 {
            mix += level[index] * value[index];
        }
        mix += sub_level * sub;
        mix + noise_level * noise
    }
}

impl Voice {
    /// The nonlinear half of the voice: the feedback loop, the drive inside
    /// it, and the filter.
    ///
    /// The order is the design decision. Drive sits *before* the filter and
    /// *inside* the loop, so it is what bounds the loop's energy — which is
    /// what makes feedback change the tone rather than only the gain. A
    /// limiter after the voice sum would have been the easy version and would
    /// have made the control a volume knob with a ceiling.
    fn shape(&mut self, prep: &Prepared, mix: f32, velocity: f32, sample_rate: u32) -> f32 {
        let routes = prep.routes;
        let smoothed_cutoff = self.cutoff.advance();
        let smoothed_drive = self.drive_amount.advance();
        let smoothed_feedback = self.feedback.advance();
        let cutoff = self.dest(routes, slot::CUTOFF, smoothed_cutoff);
        let drive = self.dest(routes, slot::DRIVE, smoothed_drive);
        let feedback = self.dest(routes, slot::VOICE_FEEDBACK, smoothed_feedback);
        if prep.filter_open && feedback == 0.0 && drive == 0.0 {
            return mix;
        }

        // One sample of delay, bounded before it re-enters. `soft_ceiling` is
        // exactly transparent below its knee, so an ordinary patch is
        // untouched and only a runaway meets it.
        let returned = soft_ceiling(self.feedback_tap * feedback * VOICE_FEEDBACK_RANGE);
        let driven = self.drive.next_sample(mix + returned, drive, sample_rate);

        // Drift's cutoff share rides on the authored corner rather than on
        // the tracked one, so it is a property of the voice and not something
        // that grows as a patch climbs the keyboard.
        let base_hz = hz_from_normalized(cutoff, prep.max_hz) * self.cutoff_scale;
        // Keytracking reads the *gliding* frequency, so a slide sweeps the
        // filter with the pitch instead of stepping at the note boundary.
        let tracked = if prep.keytrack <= 0.0 {
            base_hz
        } else {
            let octaves = (self.current_freq.max(1.0) / KEYTRACK_REFERENCE_HZ).log2();
            base_hz * (octaves * prep.keytrack).exp2()
        };
        // Velocity adds to the envelope's depth rather than scaling it, so a
        // patch with no envelope amount can still be played into the filter.
        // The routed part is the authored Env Amount only: Filter Velocity is
        // a dedicated playing behaviour, not a route destination.
        let depth = self.dest(routes, slot::ENV_AMOUNT, prep.env_amount)
            + prep.filter_velocity * velocity;
        let cutoff_hz =
            (tracked * (self.filter_env.level() * depth * FILTER_ENV_OCTAVES).exp2())
                .clamp(20.0, prep.max_hz);

        let resonance = self.dest(routes, slot::RESONANCE, prep.resonance);
        let filtered =
            self.filter
                .next_sample(prep.mode, driven, cutoff_hz, resonance, sample_rate);

        // A resonant filter driven asymmetrically walks off centre, and in a
        // loop that offset compounds. One-pole DC blocker on the tap only, so
        // the audible path keeps whatever bias the patch actually has.
        let blocked = filtered - self.dc_x + DC_BLOCK_COEFF * self.dc_y;
        self.dc_x = filtered;
        self.dc_y = blocked;
        self.feedback_tap = blocked;
        filtered
    }
}

/// One-pole DC blocker coefficient: a corner around 5 Hz at any supported
/// sample rate, which is below the lowest note and above where a drifting
/// offset becomes a problem.
const DC_BLOCK_COEFF: f32 = 0.999;

impl AudioNode for MlP8 {
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
            self.render_range(bus, pos, off, ctx.bpm);
            match ev.event {
                Event::NoteOn { id, note, velocity } => self.note_on(id, note, velocity),
                Event::NoteOff { id, .. } => self.note_off(id),
                Event::Choke => self.release_all(),
                Event::ParamValue { id, value } => self.apply_param(id, value),
                Event::SourceRouteAmount { route, amount } => {
                    self.set_route_amount(route, amount)
                }
                Event::Buffer(_) | Event::BufferRelease | Event::BufferScrub { .. } => {}
            }
            pos = off;
        }
        self.render_range(bus, pos, frames, ctx.bpm);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::TimedEvent;
    use mooloop_core::mlp8::{xmod_index, PARAM_FILTER_CUTOFF, PARAM_VOICE_FEEDBACK};
    use mooloop_core::{MlP8FilterMode, MlP8Unison, SubOctave, SubSource, SyncSource};

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

    /// A held note, rendered as one block. `note` is MIDI; the returned
    /// samples are the left channel, which is the whole signal at centre pan.
    fn render(params: MlP8Params, note: u8, frames: usize) -> Vec<f32> {
        render_chord(params, &[note], frames)
    }

    fn render_chord(params: MlP8Params, notes: &[u8], frames: usize) -> Vec<f32> {
        let mut synth = MlP8::new(params, SR);
        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        for (index, note) in notes.iter().enumerate() {
            events.push(note_on(0, index as u64 + 1, *note));
        }
        synth.process(&ctx(frames), &mut bus, &events, None);
        bus.l[..frames].to_vec()
    }

    fn rms(signal: &[f32]) -> f32 {
        let sum: f64 = signal.iter().map(|s| (*s as f64) * (*s as f64)).sum();
        (sum / signal.len() as f64).sqrt() as f32
    }

    /// Energy in a narrow band around `hz`, from a naive DFT.
    ///
    /// A band rather than one bin: these renders are neither an integer number
    /// of periods nor of constant amplitude — an envelope is running — so a
    /// single bin scallops badly and would measure the window rather than the
    /// signal.
    fn magnitude_at(signal: &[f32], hz: f32) -> f32 {
        let n = signal.len();
        let centre = (hz * n as f32 / SR as f32).round() as usize;
        (centre.saturating_sub(2)..=centre + 2)
            .map(|bin| {
                let step = -core::f64::consts::TAU * bin as f64 / n as f64;
                let (mut re, mut im) = (0.0_f64, 0.0_f64);
                for (index, sample) in signal.iter().enumerate() {
                    let angle = step * index as f64;
                    re += *sample as f64 * angle.cos();
                    im += *sample as f64 * angle.sin();
                }
                ((re * re + im * im).sqrt() / n as f64) as f32
            })
            .sum()
    }

    /// One saw, everything else off. The instrument's starting point.
    fn init_saw() -> MlP8Params {
        MlP8Params::default()
    }

    // --- Step 04: the instrument's own modulation -------------------------

    fn dest(id: u32) -> MlP8ModDest {
        MlP8ModDest::Param { id }
    }

    /// Author one route and set its depth, panicking rather than silently
    /// producing an unrouted patch if the destination is not legal.
    fn route(
        params: &mut MlP8Params,
        source: MlP8ModSource,
        dest: MlP8ModDest,
        percent: f32,
    ) -> u16 {
        let id = params
            .routes
            .add(source, dest)
            .expect("route should be accepted");
        assert!(params.routes.set_amount(id, percent));
        id
    }

    /// A held note rendered as one block, with the events supplied.
    fn render_with(params: MlP8Params, events: EventList, frames: usize) -> Vec<f32> {
        let mut synth = MlP8::new(params, SR);
        let mut bus = StereoBus::with_capacity(frames);
        synth.process(&ctx(frames), &mut bus, &events, None);
        bus.l[..frames].to_vec()
    }

    /// The step's headline claim: a patch that moves, with nothing at all in
    /// the channel modulation shelf, and three different relationships
    /// running at once on the same voices.
    #[test]
    fn a_patch_moves_on_its_own_state_alone() {
        let mut base = init_saw();
        base.osc[1].level = 0.0;
        base.filter_mode = MlP8FilterMode::Lp12;
        base.filter_cutoff = 0.5;
        base.filter_decay = 0.4;
        base.filter_sustain = 0.2;
        base.drive = 0.3;
        let plain = render(base, 60, 8192);

        let mut moving = base;
        // Filter envelope into cross-modulation, velocity into the voice
        // feedback loop, and the LFO into cutoff -- the three the plan names.
        route(
            &mut moving,
            MlP8ModSource::FilterEnv,
            dest(mooloop_core::mlp8::PARAM_XMOD_BASE + xmod_index(1, 0) as u32),
            70.0,
        );
        route(
            &mut moving,
            MlP8ModSource::Velocity,
            dest(PARAM_VOICE_FEEDBACK),
            40.0,
        );
        moving.lfo.rate_hz = 6.0;
        route(
            &mut moving,
            MlP8ModSource::Lfo,
            dest(PARAM_FILTER_CUTOFF),
            -35.0,
        );
        assert_eq!(moving.routes.len(), 3);

        let modulated = render(moving, 60, 8192);
        assert!(
            modulated.iter().all(|s| s.is_finite()),
            "an internally modulated patch produced a non-finite sample"
        );
        assert!(
            plain
                .iter()
                .zip(modulated.iter())
                .any(|(a, b)| (a - b).abs() > 1e-6),
            "three simultaneous internal routes changed nothing"
        );

        // And each one carries its own weight: removing any of the three
        // leaves a different render.
        for drop in 0..3 {
            let mut two = moving;
            let id = two.routes.iter().nth(drop).unwrap().id;
            assert!(two.routes.remove(id));
            let without = render(two, 60, 8192);
            assert!(
                modulated
                    .iter()
                    .zip(without.iter())
                    .any(|(a, b)| (a - b).abs() > 1e-6),
                "route {drop} contributed nothing to the sound"
            );
        }
    }

    /// Per voice, not per device. Two notes played at once with different
    /// velocities must reach the destination differently, which is the whole
    /// reason this is not a channel modulation route.
    #[test]
    fn velocity_and_envelope_land_per_voice() {
        let mut params = init_saw();
        params.amp_velocity = 0.0;
        params.filter_mode = MlP8FilterMode::Lp12;
        params.filter_cutoff = 0.25;
        // Velocity all the way to the voice's own level: with Amp Velocity at
        // zero, this is the only thing that can make two notes differ.
        route(
            &mut params,
            MlP8ModSource::Velocity,
            MlP8ModDest::VcaLevel,
            -100.0,
        );

        let render_at = |velocity: u8| {
            let mut events = EventList::empty();
            events.push(TimedEvent {
                offset: 0,
                event: Event::NoteOn {
                    id: 1,
                    note: 60,
                    velocity,
                },
            });
            render_with(params, events, 4096)
        };
        let soft = rms(&render_at(20));
        let hard = rms(&render_at(127));
        assert!(
            soft > hard * 1.5,
            "velocity did not reach the voice: soft {soft}, hard {hard}"
        );

        // Both at once, on separate voices. If the device collapsed velocity
        // to a last-note value, the two would be indistinguishable from two
        // notes at the same velocity.
        let chord = |a: u8, b: u8| {
            let mut events = EventList::empty();
            events.push(TimedEvent {
                offset: 0,
                event: Event::NoteOn {
                    id: 1,
                    note: 60,
                    velocity: a,
                },
            });
            events.push(TimedEvent {
                offset: 0,
                event: Event::NoteOn {
                    id: 2,
                    note: 67,
                    velocity: b,
                },
            });
            rms(&render_with(params, events, 4096))
        };
        assert!(
            (chord(20, 127) - chord(127, 127)).abs() > 1e-4,
            "two velocities in one chord resolved to one value"
        );
    }

    /// Key is bipolar about middle C, so the same route pushes a low note and
    /// a high note in opposite directions from the authored centre.
    #[test]
    fn key_tracks_bipolar_about_middle_c() {
        assert_eq!(key_offset(60), 0.0);
        assert_eq!(key_offset(108), 1.0);
        assert_eq!(key_offset(12), -1.0);
        // Past the rails it holds rather than wrapping or growing.
        assert_eq!(key_offset(127), 1.0);
        assert_eq!(key_offset(0), -1.0);

        // Onto cutoff rather than the voice level, because a route may duck a
        // voice but not push it past unity: a destination with room on both
        // sides of its centre is what shows the sign.
        let mut params = init_saw();
        params.amp_velocity = 0.0;
        params.filter_mode = MlP8FilterMode::Lp12;
        params.filter_cutoff = 0.45;
        route(&mut params, MlP8ModSource::Key, dest(PARAM_FILTER_CUTOFF), 100.0);

        // The same pitch each time, so what changes is only where the route
        // put the cutoff -- read as how much of the saw survived the filter.
        let brightness = |note: u8| {
            let signal = render(params, note, 4096);
            magnitude_at(&signal, note_to_freq(note) * 5.0)
        };
        assert!(
            brightness(84) > brightness(60) * 1.2,
            "a note above middle C did not open the filter"
        );
        assert!(
            brightness(36) < brightness(60) * 0.8,
            "a note below middle C did not close the filter"
        );
    }

    /// Gate is the held note, not the sounding one: it falls at Note Off
    /// while the release tail is still running.
    #[test]
    fn gate_falls_at_note_off_not_at_silence() {
        let mut params = init_saw();
        params.release = 1.0;
        params.amp_velocity = 0.0;
        route(&mut params, MlP8ModSource::Gate, MlP8ModDest::VcaLevel, -100.0);

        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        events.push(TimedEvent {
            offset: 2048,
            event: Event::NoteOff { id: 1, note: 60 },
        });
        let signal = render_with(params, events, 8192);

        // Held, the gate ducks the voice all the way to silence.
        assert!(
            rms(&signal[512..2000]) < 1e-6,
            "the gate did not reach the voice while the note was held"
        );
        // Released, the gate is low and the release tail is audible -- which
        // it would not be if Gate followed the envelope instead of the note.
        assert!(
            rms(&signal[2100..3000]) > 1e-3,
            "the gate did not fall at Note Off"
        );
    }

    /// Free, Chord, and Note differ exactly at the Note On boundaries the
    /// plan names, and nowhere else.
    #[test]
    fn the_three_retrigger_policies_differ_at_their_documented_boundaries() {
        let mut base = init_saw();
        base.amp_velocity = 0.0;
        base.lfo.rate_hz = 3.0;
        base.lfo.wave = MlP8LfoWave::Ramp;
        route(&mut base, MlP8ModSource::Lfo, MlP8ModDest::VcaLevel, -80.0);

        // A chord: one note, then a second while the first is still held,
        // then a third after both are released.
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        events.push(note_on(2000, 2, 64));
        events.push(TimedEvent {
            offset: 4000,
            event: Event::NoteOff { id: 1, note: 60 },
        });
        events.push(TimedEvent {
            offset: 4000,
            event: Event::NoteOff { id: 2, note: 64 },
        });
        events.push(note_on(6000, 3, 67));

        let at = |retrigger| {
            let mut params = base;
            params.lfo.retrigger = retrigger;
            let mut list = EventList::empty();
            for event in events.iter() {
                list.push(*event);
            }
            render_with(params, list, 8192)
        };
        let free = at(MlP8LfoRetrigger::Free);
        let chord = at(MlP8LfoRetrigger::Chord);
        let note = at(MlP8LfoRetrigger::Note);

        // Free never resets, so nothing after the first note matches the
        // other two.
        assert!(
            free.iter()
                .zip(chord.iter())
                .skip(6000)
                .any(|(a, b)| (a - b).abs() > 1e-6),
            "Free and Chord agreed after a fresh chord started"
        );
        // Chord and Note agree up to the second note of the chord: neither
        // has retriggered since the first Note On.
        assert!(
            chord
                .iter()
                .zip(note.iter())
                .take(2000)
                .all(|(a, b)| (a - b).abs() <= 1e-6),
            "Chord and Note diverged before the second note arrived"
        );
        // And they differ from there, because Note reset on it and Chord did
        // not.
        assert!(
            chord
                .iter()
                .zip(note.iter())
                .skip(2000)
                .take(1500)
                .any(|(a, b)| (a - b).abs() > 1e-6),
            "Note did not reset the LFO inside a chord"
        );
        // Chord *does* reset for the third note, which starts a new chord.
        assert!(
            free.iter()
                .zip(chord.iter())
                .take(2000)
                .all(|(a, b)| (a - b).abs() <= 1e-6),
            "Chord reset somewhere the first note had not"
        );
    }

    /// Every wave, including the two that are not periodic, is deterministic
    /// and bounded — over a render long enough for a chaotic recurrence or an
    /// accumulating phase to escape if it were going to.
    #[test]
    fn every_lfo_wave_is_bounded_and_reproducible() {
        for wave in MlP8LfoWave::ALL {
            let mut lfo = MlP8Lfo::new();
            let mut twin = MlP8Lfo::new();
            let params = MlP8LfoParams {
                wave,
                rate_hz: 7.3,
                warp: 0.6,
                slew: 0.35,
                ..MlP8LfoParams::default()
            };
            // Sixty seconds at 48 kHz.
            for index in 0..SR as usize * 60 {
                let value = lfo.next_sample(&params, 120.0, SR);
                assert!(
                    value.is_finite() && (-1.001..=1.001).contains(&value),
                    "{wave:?} left its range at sample {index}: {value}"
                );
                assert_eq!(
                    value,
                    twin.next_sample(&params, 120.0, SR),
                    "{wave:?} did not render identically twice"
                );
            }
        }
    }

    /// Chaos is not sample-and-hold under another name, and it is not
    /// periodic: it keeps moving, and it does not repeat inside a window a
    /// periodic wave at the same rate would repeat many times over.
    #[test]
    fn chaos_wanders_rather_than_holding_or_repeating() {
        let params = MlP8LfoParams {
            wave: MlP8LfoWave::Chaos,
            rate_hz: 2.0,
            ..MlP8LfoParams::default()
        };
        let mut lfo = MlP8Lfo::new();
        let values: Vec<f32> = (0..SR as usize * 8)
            .map(|_| lfo.next_sample(&params, 120.0, SR))
            .collect();

        // It never holds still: consecutive samples differ almost everywhere,
        // which is exactly what a sample-and-hold does not do.
        let held = values.windows(2).filter(|w| w[0] == w[1]).count();
        assert!(
            held * 100 < values.len(),
            "chaos held its value for {held} of {} samples",
            values.len()
        );
        // And one cycle in is not the same as two cycles in. A rational
        // frequency ratio would close the figure and make it periodic.
        let cycle = (SR as f32 / 2.0) as usize;
        let drift: f32 = values[cycle..cycle * 2]
            .iter()
            .zip(values[cycle * 2..cycle * 3].iter())
            .map(|(a, b)| (a - b).abs())
            .sum::<f32>()
            / cycle as f32;
        assert!(drift > 0.05, "chaos repeated its cycle: mean drift {drift}");
    }

    /// Sample-and-hold is the other half of that pair: it *does* hold, and
    /// it changes exactly once per cycle.
    #[test]
    fn sample_and_hold_steps_once_per_cycle() {
        let params = MlP8LfoParams {
            wave: MlP8LfoWave::SampleHold,
            rate_hz: 10.0,
            ..MlP8LfoParams::default()
        };
        let mut lfo = MlP8Lfo::new();
        let seconds = 4;
        let values: Vec<f32> = (0..SR as usize * seconds)
            .map(|_| lfo.next_sample(&params, 120.0, SR))
            .collect();
        let steps = values.windows(2).filter(|w| w[0] != w[1]).count() as i32;
        // Once per cycle, give or take the boundary: the last wrap lands on
        // the sample after the window, and two draws in a row could in
        // principle repeat a value.
        let cycles = 10 * seconds as i32;
        assert!(
            (steps - cycles).abs() <= 1,
            "sample-and-hold stepped {steps} times in {cycles} cycles"
        );
    }

    /// Base plus offset, clamped through the destination's own range. A route
    /// deep enough to push a control past its end leaves it at the end rather
    /// than past it.
    #[test]
    fn a_route_resolves_as_base_plus_offset_and_clamps() {
        let mut routes = CompiledRoutes::new();
        let mut authored = MlP8Routes::default();
        let cutoff = authored
            .add(MlP8ModSource::AmpEnv, dest(PARAM_FILTER_CUTOFF))
            .unwrap();
        assert!(authored.set_amount(cutoff, 100.0));
        routes.compile(&authored);

        let mut voice = Voice::new(0, SR);
        // Cutoff spans 0..1, so 100% of an envelope at full level is +1.0.
        voice.resolve_routes(&routes, &[0.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(voice.mod_offsets[slot::CUTOFF], 1.0);
        assert_eq!(voice.dest(&routes, slot::CUTOFF, 0.5), 1.0);
        assert_eq!(voice.dest(&routes, slot::CUTOFF, 0.0), 1.0);

        // Half the envelope is half the offset, and the base still counts.
        voice.resolve_routes(&routes, &[0.0, 0.5, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(voice.dest(&routes, slot::CUTOFF, 0.25), 0.75);

        // An untouched destination is the base, whatever the table holds.
        assert_eq!(voice.dest(&routes, slot::DRIVE, 0.4), 0.4);
    }

    /// Two routes onto one destination add before the clamp, rather than the
    /// last one winning.
    #[test]
    fn routes_sharing_a_destination_sum() {
        let mut authored = MlP8Routes::default();
        for source in [MlP8ModSource::AmpEnv, MlP8ModSource::Velocity] {
            let id = authored.add(source, dest(PARAM_FILTER_CUTOFF)).unwrap();
            assert!(authored.set_amount(id, 20.0));
        }
        let mut routes = CompiledRoutes::new();
        routes.compile(&authored);

        let mut voice = Voice::new(0, SR);
        voice.resolve_routes(&routes, &[0.0, 1.0, 0.0, 1.0, 0.0, 0.0]);
        assert!((voice.mod_offsets[slot::CUTOFF] - 0.4).abs() < 1e-6);
        assert_eq!(routes.len, 2, "two routes, one destination");
    }

    /// The route table survives an ordinary knob change: only the routes
    /// moving rebuilds it.
    #[test]
    fn an_ordinary_parameter_change_does_not_rebuild_the_topology() {
        let mut params = init_saw();
        let id = route(
            &mut params,
            MlP8ModSource::Lfo,
            dest(PARAM_FILTER_CUTOFF),
            50.0,
        );
        let mut synth = MlP8::new(params, SR);
        let before = synth.routes.routes[0].scale;

        synth.apply_param(PARAM_FILTER_CUTOFF, 0.3);
        assert_eq!(synth.routes.len, 1);
        assert_eq!(synth.routes.routes[0].id, id);
        assert_eq!(synth.routes.routes[0].scale, before);

        // The amount, on the other hand, moves in place.
        synth.set_route_amount(id, -50.0);
        assert_eq!(synth.routes.len, 1);
        assert_eq!(synth.routes.routes[0].id, id);
        assert_eq!(synth.routes.routes[0].scale, -before);
        // And the authored value follows, so a later whole-block install does
        // not step back to the depth the patch was saved with.
        assert_eq!(params_amount(&synth.params.routes, id), -50.0);
    }

    fn params_amount(routes: &MlP8Routes, id: u16) -> f32 {
        routes.get(id).expect("route should still exist").amount
    }

    /// A route amount arriving as an event is sample-timed like any other
    /// automation, and a route sitting at zero is still there to be swept up
    /// from — which is the thing a compiler that dropped zero rows would have
    /// broken.
    #[test]
    fn a_route_amount_is_sample_timed_and_survives_zero() {
        let mut params = init_saw();
        params.amp_velocity = 0.0;
        params.lfo.rate_hz = 5.0;
        let id = route(&mut params, MlP8ModSource::Lfo, MlP8ModDest::VcaLevel, 0.0);

        let mut synth = MlP8::new(params, SR);
        let mut bus = StereoBus::with_capacity(8192);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        events.push(TimedEvent {
            offset: 4096,
            event: Event::SourceRouteAmount {
                route: id,
                amount: -90.0,
            },
        });
        synth.process(&ctx(8192), &mut bus, &events, None);

        let before = rms(&bus.l[512..4000]);
        let after = rms(&bus.l[4200..8000]);
        assert!(
            before > 0.0 && (before - after).abs() > before * 0.1,
            "a route amount at zero could not be automated up: {before} -> {after}"
        );
        // The topology never moved: one row, same id, same destination.
        assert_eq!(synth.routes.len, 1);
        assert_eq!(synth.routes.routes[0].id, id);
    }

    /// An unrouted patch is bit-for-bit what it was before this step existed.
    #[test]
    fn an_unrouted_patch_is_untouched_by_the_route_machinery() {
        let mut params = init_saw();
        params.osc[1].level = 0.6;
        params.osc[2].level = 0.4;
        params.xmod[xmod_index(1, 0)] = 40.0;
        params.filter_cutoff = 0.4;
        params.filter_resonance = 0.5;
        params.drive = 0.2;
        params.voice_feedback = 0.3;
        params.sub_level = 0.5;
        params.noise_level = 0.2;
        assert!(params.routes.is_empty());
        let plain = render(params, 60, 4096);

        // The same patch with one route authored at zero depth. Every code
        // path the routes turn on now runs, and the samples are identical.
        let mut routed = params;
        route(&mut routed, MlP8ModSource::Lfo, dest(PARAM_FILTER_CUTOFF), 0.0);
        let with_zero = render(routed, 60, 4096);
        assert_eq!(plain, with_zero, "a zero-depth route changed the sound");
    }

    /// A route to a source's level wakes it: the block-level skip cannot
    /// decide from the authored level alone once something else can move it.
    #[test]
    fn a_route_can_raise_a_source_the_mixer_silenced() {
        let mut params = init_saw();
        params.osc[0].level = 0.0;
        params.amp_velocity = 0.0;
        route(
            &mut params,
            MlP8ModSource::AmpEnv,
            dest(mooloop_core::mlp8::osc_param(0, mooloop_core::mlp8::OSC_OFFSET_LEVEL)),
            100.0,
        );
        assert!(
            rms(&render(params, 60, 4096)) > 1e-3,
            "a route could not raise an oscillator the mixer had silenced"
        );
    }

    /// The whole thing, rendered twice in one process and compared. Chaos,
    /// sample-and-hold, per-voice noise and a feedback loop all at once.
    #[test]
    fn a_fully_modulated_patch_renders_identically_twice() {
        let mut params = init_saw();
        params.osc[1].level = 0.5;
        params.noise_level = 0.3;
        params.filter_cutoff = 0.35;
        params.filter_resonance = 0.6;
        params.voice_feedback = 0.4;
        params.lfo.wave = MlP8LfoWave::Chaos;
        params.lfo.rate_hz = 9.0;
        params.lfo.slew = 0.4;
        params.lfo.warp = 0.5;
        route(&mut params, MlP8ModSource::Lfo, dest(PARAM_FILTER_CUTOFF), 60.0);
        route(
            &mut params,
            MlP8ModSource::FilterEnv,
            dest(mooloop_core::mlp8::PARAM_NOISE_COLOR),
            80.0,
        );
        route(&mut params, MlP8ModSource::Key, MlP8ModDest::Pan, 100.0);
        route(
            &mut params,
            MlP8ModSource::Velocity,
            dest(PARAM_VOICE_FEEDBACK),
            -50.0,
        );

        let first = render_chord(params, &[48, 55, 60, 64, 67, 72, 76, 79], 8192);
        let second = render_chord(params, &[48, 55, 60, 64, 67, 72, 76, 79], 8192);
        assert_eq!(first, second, "a modulated eight-voice chord drifted");
        assert!(
            first.iter().all(|s| s.is_finite()),
            "a fully modulated chord produced a non-finite sample"
        );
    }

    /// Pan is a per-voice destination, so a route on it moves voices in the
    /// stereo field that the channel strip's own pan could only move together.
    #[test]
    fn a_pan_route_separates_voices_across_the_field() {
        let mut params = init_saw();
        params.amp_velocity = 0.0;
        route(&mut params, MlP8ModSource::Key, MlP8ModDest::Pan, 100.0);

        let mut synth = MlP8::new(params, SR);
        let mut bus = StereoBus::with_capacity(4096);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 36));
        events.push(note_on(0, 2, 84));
        synth.process(&ctx(4096), &mut bus, &events, None);

        // Two notes either side of middle C, one pushed left and one right.
        // Without the route both channels would carry the same sum.
        let (left, right) = (rms(&bus.l[..4096]), rms(&bus.r[..4096]));
        assert!(
            (left - right).abs() < left * 0.2,
            "a symmetric pair did not stay balanced overall"
        );
        let correlation: f32 = bus.l[..4096]
            .iter()
            .zip(bus.r[..4096].iter())
            .map(|(a, b)| a * b)
            .sum::<f32>()
            / 4096.0;
        let energy = rms(&bus.l[..4096]) * rms(&bus.r[..4096]);
        assert!(
            correlation < energy * 0.99,
            "the two channels were identical, so nothing was panned"
        );
    }

    /// The claim the device is built on: Level is a mixer control, not an on
    /// switch. Osc 2 is silent in the mix and still changes what Osc 1 sounds
    /// like.
    #[test]
    fn a_muted_oscillator_modulates_without_being_heard() {
        let mut silent = init_saw();
        silent.osc[1].level = 0.0;
        let plain = render(silent, 60, 4096);

        let mut modulating = silent;
        modulating.xmod[xmod_index(1, 0)] = 60.0;
        let modulated = render(modulating, 60, 4096);

        assert!(
            rms(&modulated) > 0.0,
            "a modulated oscillator produced silence"
        );
        assert!(
            plain.iter().zip(modulated.iter()).any(|(a, b)| a != b),
            "a muted modulator changed nothing"
        );

        // And the mute is real: raising only Osc 2's own level, with no
        // routes at all, is what adds its signal to the mix.
        let mut audible = silent;
        audible.osc[1].level = 1.0;
        assert!(
            rms(&render(audible, 60, 4096)) > rms(&plain) * 1.2,
            "Level did not add the second oscillator to the mix"
        );
    }

    /// All six directions, including the pairs that close a loop. A route in
    /// the reverse direction must produce a stable result rather than
    /// depending on which oscillator the loop is entered from.
    #[test]
    fn every_directed_xmod_route_is_audible_and_stable() {
        let mut base = init_saw();
        // All three oscillators running, only the first one heard, so a route
        // in any direction has something to modulate and something to be
        // modulated by.
        base.osc[1].level = 0.0;
        base.osc[2].level = 0.0;
        base.osc[1].semitones = 0.0;
        base.osc[2].semitones = 0.0;
        base.osc[1].cents = 0.0;
        base.osc[2].cents = 0.0;
        // Give the silent pair a reason to exist so `osc_needed` keeps them.
        base.xmod[xmod_index(1, 0)] = 1.0;
        base.xmod[xmod_index(2, 0)] = 1.0;
        let reference = render(base, 60, 4096);

        for from in 0..3 {
            for to in 0..3 {
                if from == to {
                    continue;
                }
                let mut params = base;
                params.xmod[xmod_index(from, to)] = 70.0;
                let out = render(params, 60, 4096);
                assert!(
                    out.iter().all(|s| s.is_finite()),
                    "{from}->{to} produced a non-finite sample"
                );
                assert!(
                    out.iter().zip(reference.iter()).any(|(a, b)| a != b),
                    "{from}->{to} changed nothing"
                );
            }
        }
    }

    /// A cycle in the modulation graph is the interesting case: 1->2 and 2->1
    /// both at depth. The one-sample tap makes it causal, so it has to be both
    /// finite and reproducible.
    #[test]
    fn a_two_way_xmod_loop_is_bounded_and_reproducible() {
        let mut params = init_saw();
        params.osc[1].level = 0.7;
        params.osc[1].semitones = 0.0;
        params.osc[1].cents = 0.0;
        params.xmod[xmod_index(0, 1)] = 100.0;
        params.xmod[xmod_index(1, 0)] = -100.0;
        params.osc_feedback = [100.0, -100.0, 0.0];

        let first = render(params, 72, 8192);
        let second = render(params, 72, 8192);
        assert_eq!(first, second, "a modulation loop did not render identically");
        assert!(
            first.iter().all(|s| s.is_finite() && s.abs() < 4.0),
            "a modulation loop ran away"
        );
    }

    /// Noise reaches the phase inputs whether or not it reaches the mixer.
    #[test]
    fn noise_modulates_each_oscillator_while_staying_out_of_the_mix() {
        let base = init_saw();
        let clean = render(base, 60, 4096);
        assert_eq!(base.noise_level, 0.0);

        for target in 0..3 {
            let mut params = base;
            // Osc 1 is the only audible one, so route through it: a route into
            // a silent oscillator would be inaudible for the wrong reason.
            params.osc[target].level = if target == 0 { 1.0 } else { 0.0 };
            params.xmod[xmod_index(target, 0)] = if target == 0 { 0.0 } else { 80.0 };
            params.noise_to_osc[target] = 90.0;
            let out = render(params, 60, 4096);
            assert!(
                out.iter().all(|s| s.is_finite()),
                "noise into osc {target} produced a non-finite sample"
            );
            assert!(
                out.iter().zip(clean.iter()).any(|(a, b)| a != b),
                "noise into osc {target} changed nothing"
            );
        }
    }

    /// Both polarities of self-feedback, from a trace to the maximum. The
    /// bound is part of the sound, so this asks for finite and increasingly
    /// different rather than for a level.
    #[test]
    fn oscillator_self_feedback_travels_and_stays_finite() {
        let base = init_saw();
        let clean = render(base, 60, 4096);
        let mut previous_distance = 0.0_f32;
        for amount in [5.0_f32, 25.0, 60.0, 100.0] {
            for sign in [1.0_f32, -1.0] {
                let mut params = base;
                params.osc_feedback[0] = amount * sign;
                let out = render(params, 60, 4096);
                assert!(
                    out.iter().all(|s| s.is_finite() && s.abs() < 4.0),
                    "feedback {amount} at sign {sign} left the bound"
                );
                if sign > 0.0 {
                    let distance = out
                        .iter()
                        .zip(clean.iter())
                        .map(|(a, b)| (a - b).abs())
                        .fold(0.0_f32, f32::max);
                    assert!(
                        distance > previous_distance,
                        "feedback {amount} did not go further than the step below"
                    );
                    previous_distance = distance;
                }
            }
        }
    }

    /// Every legal master/slave pair, with cross-modulation active at the same
    /// time. Sync shapes the slave between the master's wraps; XMOD shapes it
    /// in between, and the two together must stay bounded.
    #[test]
    fn every_sync_pair_works_with_xmod_active() {
        for slave in 0..3 {
            for master in 0..3 {
                if slave == master {
                    continue;
                }
                let mut params = init_saw();
                for (index, osc) in params.osc.iter_mut().enumerate() {
                    osc.semitones = 0.0;
                    osc.cents = 0.0;
                    osc.level = if index == slave { 1.0 } else { 0.0 };
                }
                // The slave runs well above the master, which is what makes
                // sync a sound rather than a phase reset -- and at a ratio
                // that is clearly not a whole number, because a slave at an
                // exact multiple of the master is already periodic at the
                // master's rate and sync has nothing to add.
                params.osc[slave].semitones = 17.0;
                params.xmod[xmod_index(master, slave)] = 40.0;
                let free = render(params, 60, 8192);

                params.sync_source[slave] = SyncSource::from_index(master as i32 + 1);
                let out = render(params, 60, 8192);
                assert!(
                    out.iter().all(|s| s.is_finite() && s.abs() < 4.0),
                    "sync {master}->{slave} left the bound"
                );
                // What sync does is give the slave the master's period, so a
                // slave tuned a nineteenth above the note starts carrying the
                // note's own fundamental instead of only its own.
                let played = note_to_freq(60);
                let (synced, unsynced) = (magnitude_at(&out, played), magnitude_at(&free, played));
                assert!(
                    synced > unsynced * 4.0,
                    "sync {master}->{slave} left the fundamental at {synced} against {unsynced} free"
                );
            }
        }
    }

    /// A synced oscillator that is not otherwise heard still delivers its
    /// edges: the sync source selector is topology, not a mix decision.
    #[test]
    fn sync_from_a_muted_master_still_resets_the_slave() {
        let mut params = init_saw();
        params.osc[1].level = 0.0;
        params.osc[1].semitones = -12.0;
        params.osc[1].cents = 0.0;
        let free = render(params, 60, 8192);

        params.sync_source[0] = SyncSource::Osc2;
        let synced = render(params, 60, 8192);
        assert!(
            free.iter().zip(synced.iter()).any(|(a, b)| a != b),
            "a muted master did not sync anything"
        );
    }

    /// Sub is a fundamental, and the test for a fundamental is that it is
    /// where it says it is — an octave or two below the played note, and still
    /// there when the carrier above it has been taken apart by XMOD.
    #[test]
    fn sub_holds_its_octave_under_deep_cross_modulation() {
        let note = 60_u8;
        for (octave, divisor) in [(SubOctave::Minus1, 2.0_f32), (SubOctave::Minus2, 4.0)] {
            let mut params = init_saw();
            params.sub_level = 1.0;
            params.sub_octave = octave;
            params.sub_source = SubSource::Osc1;
            // A carrier under heavy modulation, so the sub is the only stable
            // thing in the signal.
            params.osc[1].level = 0.0;
            params.osc[1].semitones = 7.0;
            params.xmod[xmod_index(1, 0)] = 100.0;
            params.osc_feedback[0] = 60.0;

            let out = render(params, note, 16384);
            let sub_hz = note_to_freq(note) / divisor;
            let magnitude = magnitude_at(&out, sub_hz);
            assert!(
                magnitude > 0.02,
                "{octave:?} sub at {sub_hz} Hz measured {magnitude}"
            );
        }
    }

    #[test]
    fn sub_contributes_exactly_nothing_at_silence() {
        let mut params = init_saw();
        params.sub_level = 0.0;
        let without = render(params, 60, 4096);

        // Changing every other sub control must not move a single sample
        // while its level is at the bottom.
        params.sub_octave = SubOctave::Minus2;
        params.sub_wave = SubWave::Square;
        params.sub_source = SubSource::Osc3;
        assert_eq!(render(params, 60, 4096), without);
    }

    /// Noise Color is a tilt, and the three positions have to be different in
    /// spectrum without being different in level — otherwise the knob is a
    /// volume control wearing a tone control's name.
    #[test]
    fn noise_color_tilts_the_spectrum_without_moving_the_level() {
        let mut params = init_saw();
        params.osc[0].level = 0.0;
        params.noise_level = 1.0;

        let mut levels = Vec::new();
        let mut lows = Vec::new();
        for tilt in [-100.0_f32, 0.0, 100.0] {
            params.noise_color = tilt;
            let out = render(params, 60, 16384);
            levels.push(rms(&out));
            lows.push(magnitude_at(&out, 120.0) + magnitude_at(&out, 200.0));
        }
        assert!(
            lows[0] > lows[1] && lows[1] > lows[2],
            "colour did not tilt the low end: {lows:?}"
        );
        let (min, max) = (
            levels.iter().cloned().fold(f32::MAX, f32::min),
            levels.iter().cloned().fold(0.0_f32, f32::max),
        );
        let spread_db = 20.0 * (max / min).log10();
        println!("noise colour levels {levels:?} ({spread_db:.1} dB apart)");
        assert!(spread_db < 3.0, "colour moved the level by {spread_db:.1} dB");
    }

    /// Two fresh devices, the same events, the same samples. The noise seeds
    /// come from slot indices and nothing in the network reads runtime
    /// entropy, so an offline render and a live take agree.
    #[test]
    fn the_whole_network_renders_bit_identically_across_instances() {
        let mut params = init_saw();
        params.osc[1].level = 0.6;
        params.osc[2].level = 0.4;
        params.sub_level = 0.5;
        params.noise_level = 0.3;
        params.noise_color = -40.0;
        params.xmod = [55.0, -40.0, 30.0, -25.0, 70.0, -60.0];
        params.noise_to_osc = [30.0, -20.0, 45.0];
        params.osc_feedback = [40.0, -30.0, 20.0];
        params.sync_source = [SyncSource::Off, SyncSource::Osc1, SyncSource::Osc2];

        let notes = [48, 55, 60, 64, 67, 71, 74, 79];
        let first = render_chord(params, &notes, 8192);
        let second = render_chord(params, &notes, 8192);
        assert_eq!(first, second);
    }

    /// Eight notes with every destructive control at its maximum. This is the
    /// worst case the plan asks to stay finite, and it is also where an
    /// oscillator-order dependency would show up.
    #[test]
    fn a_full_chord_at_worst_case_stays_finite() {
        let mut params = init_saw();
        for osc in params.osc.iter_mut() {
            osc.level = 1.0;
            osc.wave = OscWave::Pulse;
        }
        params.sub_level = 1.0;
        params.sub_wave = SubWave::Square;
        params.noise_level = 1.0;
        params.xmod = [100.0; 6];
        params.noise_to_osc = [100.0; 3];
        params.osc_feedback = [100.0; 3];
        params.sync_source = [SyncSource::Osc3, SyncSource::Osc1, SyncSource::Osc2];

        let out = render_chord(params, &[36, 43, 48, 55, 60, 67, 72, 79], 16384);
        assert!(out.iter().all(|s| s.is_finite()), "worst case went non-finite");
        let peak = out.iter().fold(0.0_f32, |a, s| a.max(s.abs()));

        // Honest summing, so this is allowed well over full scale: nothing
        // normalizes by source or by voice count, and the plan says so. What
        // it may not do is exceed what honest summing predicts -- five unit
        // sources a voice, eight voices, at the voice reference and centre
        // pan -- because that would mean the phase bound had stopped working
        // rather than that the patch is loud.
        let ceiling = 5.0 * VOICE_OUTPUT_REFERENCE * MLP8_VOICES as f32 * pan_gains(0.0).0;
        println!("worst-case eight-note peak: {peak:.3}, honest ceiling {ceiling:.3}");
        assert!(peak < ceiling, "worst case peaked at {peak}, over {ceiling}");
    }

    /// Turning a source down is a ramp, and the skip has to wait for it.
    ///
    /// `Prepared` decides what runs from the *target* levels, so the moment
    /// the knob reaches zero the oscillator stops being "needed" while its
    /// smoother is still a few milliseconds from silence. Skipping it there
    /// would put a step exactly where the smoother exists to prevent one.
    #[test]
    fn a_source_turned_down_ramps_instead_of_stepping() {
        const TAIL: usize = 8192;
        let mut synth = MlP8::new(init_saw(), SR);
        let mut bus = StereoBus::with_capacity(TAIL);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        synth.process(&ctx(1024), &mut bus, &events, None);

        let before = bus.l[1023];
        let mut silenced = init_saw();
        silenced.osc[0].level = 0.0;
        synth.set_params(silenced);
        bus.clear(TAIL);
        synth.process(&ctx(TAIL), &mut bus, &EventList::empty(), None);

        // The first sample of the next block continues from the last one
        // rather than dropping to nothing.
        let step = (bus.l[0] - before).abs();
        println!("step across the level change: {step:.5}");
        assert!(step < 0.02, "turning a level down stepped by {step}");
        // And it does reach silence, so the guard is a ramp and not a leak.
        // The smoother's 5 ms figure is a time constant, not a duration --
        // a single 1024-frame block leaves over a percent of it -- so this
        // waits several of them rather than assuming one block is enough.
        assert!(
            bus.l[TAIL - 1].abs() < 1.0e-6,
            "the ramp never finished: {}",
            bus.l[TAIL - 1]
        );
    }

    /// Band energy around `hz`, as a share of the whole signal's. Enough to
    /// answer "did this mode keep the lows or throw them away".
    fn band_share(signal: &[f32], low_hz: f32, high_hz: f32) -> f32 {
        let n = signal.len();
        let bin_hz = SR as f32 / n as f32;
        let (mut band, mut total) = (0.0_f64, 0.0_f64);
        for bin in 1..n / 2 {
            let step = -core::f64::consts::TAU * bin as f64 / n as f64;
            let (mut re, mut im) = (0.0_f64, 0.0_f64);
            for (index, sample) in signal.iter().enumerate() {
                let angle = step * index as f64;
                re += *sample as f64 * angle.cos();
                im += *sample as f64 * angle.sin();
            }
            let power = re * re + im * im;
            total += power;
            let hz = bin as f32 * bin_hz;
            if hz >= low_hz && hz <= high_hz {
                band += power;
            }
        }
        (band / total.max(1.0e-12)) as f32
    }

    /// A bright patch to put through the filter: a saw with plenty above and
    /// below the corner the tests move.
    fn filter_bed() -> MlP8Params {
        let mut params = init_saw();
        params.filter_cutoff = 0.5;
        params.sustain = 1.0;
        params
    }

    /// Each mode has to do what its name says. Low-pass keeps the lows and
    /// loses the highs, high-pass the reverse, band-pass keeps neither end.
    #[test]
    fn each_filter_mode_produces_its_own_response() {
        let note = 45_u8; // A2, ~110 Hz: harmonics either side of the corner.
        let mut shares = Vec::new();
        for mode in [
            MlP8FilterMode::Lp12,
            MlP8FilterMode::Lp24,
            MlP8FilterMode::Bp12,
            MlP8FilterMode::Hp12,
        ] {
            let mut params = filter_bed();
            params.filter_mode = mode;
            let out = render(params, note, 4096);
            assert!(out.iter().all(|s| s.is_finite()), "{mode:?} went non-finite");
            // Just above the corner, not at the top of the spectrum: two
            // octaves up both low-pass modes have thrown everything away and
            // the comparison is between two noise floors. Slope is only
            // visible where the skirts still have something in them.
            let low = band_share(&out, 60.0, 160.0);
            let high = band_share(&out, 1300.0, 2600.0);
            println!("{mode:?}: low {low:.4}, high {high:.4}");
            shares.push((mode, low, high));
        }
        let get = |m: MlP8FilterMode| shares.iter().find(|(k, _, _)| *k == m).unwrap();
        let (_, lp12_low, lp12_high) = *get(MlP8FilterMode::Lp12);
        let (_, lp24_low, lp24_high) = *get(MlP8FilterMode::Lp24);
        let (_, hp_low, hp_high) = *get(MlP8FilterMode::Hp12);
        let (_, bp_low, bp_high) = *get(MlP8FilterMode::Bp12);

        // The low-passes keep the fundamental; the high-pass throws it away.
        assert!(lp12_low > 0.1, "LP12 lost its low end: {lp12_low}");
        assert!(
            hp_low < lp12_low * 0.4,
            "HP12 kept the lows: {hp_low} against LP12's {lp12_low}"
        );
        assert!(
            hp_high > lp12_high * 5.0,
            "HP12 has no top: {hp_high} against LP12's {lp12_high}"
        );
        // Four poles throw away more of the same top than two do. This is the
        // assertion the whole mode set exists for, and it only means anything
        // because the corner test below pins the two to the same frequency.
        assert!(
            lp24_high < lp12_high * 0.7,
            "LP24 ({lp24_high}) is not steeper than LP12 ({lp12_high})"
        );
        // ...while keeping the same bottom, which is what makes "steeper"
        // mean a steeper skirt rather than a lower corner.
        assert!(
            (lp24_low - lp12_low).abs() < lp12_low * 0.15,
            "LP24 ({lp24_low}) moved the passband against LP12 ({lp12_low})"
        );
        // Band-pass rejects both ends: less bottom than a low-pass, less top
        // than a high-pass.
        assert!(bp_low < lp12_low, "BP12 kept as much bottom as LP12");
        assert!(bp_high < hp_high, "BP12 kept as much top as HP12");
    }

    /// The Cutoff knob has to mean about the same frequency in both low-pass
    /// modes, or switching slope becomes a tuning change.
    #[test]
    fn lp12_and_lp24_share_a_corner() {
        // Measured where the two-pole response is already well down, so the
        // comparison is about where the corner sits rather than how steep the
        // skirt is.
        let mut params = filter_bed();
        params.filter_mode = MlP8FilterMode::Lp12;
        let lp12 = band_share(&render(params, 45, 4096), 60.0, 400.0);
        params.filter_mode = MlP8FilterMode::Lp24;
        let lp24 = band_share(&render(params, 45, 4096), 60.0, 400.0);
        let ratio = lp24 / lp12;
        println!("passband share LP12 {lp12:.4}, LP24 {lp24:.4} (ratio {ratio:.2})");
        assert!(
            (0.8..=1.3).contains(&ratio),
            "the corner moved between slopes: {ratio:.2}"
        );
    }

    /// Amp Velocity is a depth on velocity, not a switch. At zero every note
    /// is the same level; at full it follows the note.
    #[test]
    fn amp_velocity_crossfades_between_fixed_and_played() {
        let peak = |depth: f32, velocity: u8| {
            let mut params = init_saw();
            params.amp_velocity = depth;
            let mut synth = MlP8::new(params, SR);
            let mut bus = StereoBus::with_capacity(4096);
            let mut events = EventList::empty();
            events.push(TimedEvent {
                offset: 0,
                event: Event::NoteOn {
                    id: 1,
                    note: 60,
                    velocity,
                },
            });
            synth.process(&ctx(4096), &mut bus, &events, None);
            bus.l[..4096].iter().fold(0.0_f32, |a, s| a.max(s.abs()))
        };

        let (soft, loud) = (peak(0.0, 32), peak(0.0, 127));
        assert!(
            (soft - loud).abs() < loud * 0.02,
            "at zero depth velocity still moved the VCA: {soft} vs {loud}"
        );
        let (soft, loud) = (peak(1.0, 32), peak(1.0, 127));
        assert!(
            soft < loud * 0.4,
            "at full depth velocity barely moved the VCA: {soft} vs {loud}"
        );
    }

    /// Filter Velocity moves the filter and nothing else. It is bipolar and
    /// must not become a second amplitude control.
    #[test]
    fn filter_velocity_never_moves_the_vca_by_itself() {
        let peak = |velocity: u8| {
            let mut params = init_saw();
            // No amp velocity, so anything that moves the peak came from the
            // filter path.
            params.amp_velocity = 0.0;
            params.filter_velocity = 1.0;
            params.filter_cutoff = 1.0;
            let mut synth = MlP8::new(params, SR);
            let mut bus = StereoBus::with_capacity(4096);
            let mut events = EventList::empty();
            events.push(TimedEvent {
                offset: 0,
                event: Event::NoteOn {
                    id: 1,
                    note: 60,
                    velocity,
                },
            });
            synth.process(&ctx(4096), &mut bus, &events, None);
            bus.l[..4096].iter().fold(0.0_f32, |a, s| a.max(s.abs()))
        };
        let (soft, loud) = (peak(1), peak(127));
        println!("filter-velocity peaks: soft {soft:.4}, loud {loud:.4}");
        assert!(
            (soft - loud).abs() < loud * 0.05,
            "filter velocity changed the VCA: {soft} vs {loud}"
        );
    }

    /// Keytrack at 100% moves the corner one octave per played octave, so a
    /// patch voiced in the middle is not a thud at the top.
    #[test]
    fn keytrack_follows_the_played_octave() {
        let brightness = |note: u8, keytrack: f32| {
            let mut params = filter_bed();
            params.filter_keytrack = keytrack;
            band_share(&render(params, note, 4096), 2000.0, 12000.0)
        };
        // Two octaves up. Without tracking the note loses its top; with
        // tracking the corner climbs with it.
        let untracked_low = brightness(48, 0.0);
        let untracked_high = brightness(72, 0.0);
        let tracked_high = brightness(72, 1.0);
        println!(
            "high band: C3 {untracked_low:.4}, C5 off {untracked_high:.4}, C5 tracked {tracked_high:.4}"
        );
        assert!(
            tracked_high > untracked_high * 1.5,
            "keytrack did not open the filter for a higher note"
        );
    }

    /// The loop has to reach unstable territory at both polarities and stay
    /// finite there. The bound is the drive stage and the soft ceiling, not a
    /// limiter after the sum.
    #[test]
    fn voice_feedback_travels_and_stays_bounded() {
        let mut previous = 0.0_f32;
        for amount in [0.0_f32, 0.35, 0.7, 1.0] {
            for sign in [1.0_f32, -1.0] {
                let mut params = filter_bed();
                params.filter_resonance = 0.8;
                params.drive = 0.4;
                params.voice_feedback = amount * sign;
                let out = render(params, 45, 8192);
                assert!(
                    out.iter().all(|s| s.is_finite() && s.abs() < 4.0),
                    "feedback {amount} at sign {sign} left the bound"
                );
                if sign > 0.0 {
                    let energy: f32 = out.iter().map(|s| s * s).sum::<f32>();
                    if amount > 0.0 {
                        assert!(
                            energy > previous,
                            "feedback {amount} added nothing over the step below"
                        );
                    }
                    previous = energy;
                }
            }
        }
    }

    /// Eight held notes must not hear each other. The filter and the feedback
    /// delay are per voice, not a loop around the sum.
    #[test]
    fn voices_do_not_leak_filter_or_feedback_into_one_another() {
        let mut params = filter_bed();
        params.filter_resonance = 0.7;
        params.voice_feedback = 0.8;
        params.drive = 0.5;

        // One note alone, then the same note inside a chord. Its own
        // contribution cannot depend on what else is sounding, so the chord
        // must be the sum of its parts.
        let notes = [36, 43, 48, 55, 60, 67, 72, 79];
        let chord = render_chord(params, &notes, 8192);
        let mut summed = vec![0.0_f32; 8192];
        for note in notes {
            for (acc, s) in summed.iter_mut().zip(render(params, note, 8192)) {
                *acc += s;
            }
        }
        let worst = chord
            .iter()
            .zip(summed.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        println!("chord vs sum of voices, worst sample: {worst:.6}");
        assert!(worst < 1.0e-4, "voices interacted: worst {worst}");
    }

    /// A stolen slot must not emit the previous note's feedback tail.
    #[test]
    fn a_stolen_slot_starts_from_a_clean_loop() {
        let mut params = filter_bed();
        params.voice_feedback = 0.9;
        params.filter_resonance = 0.85;
        params.drive = 0.6;
        params.release = 0.001;

        let mut synth = MlP8::new(params, SR);
        let mut bus = StereoBus::with_capacity(2048);
        let mut events = EventList::empty();
        for index in 0..8u8 {
            events.push(note_on(0, u64::from(index) + 1, 40 + index * 4));
        }
        synth.process(&ctx(2048), &mut bus, &events, None);
        for index in 0..8u8 {
            let mut off = EventList::empty();
            off.push(TimedEvent {
                offset: 0,
                event: Event::NoteOff {
                    id: u64::from(index) + 1,
                    note: 40 + index * 4,
                },
            });
            bus.clear(2048);
            synth.process(&ctx(2048), &mut bus, &off, None);
        }
        // Everything has been released and run out; nothing may still be
        // holding energy in a loop.
        for _ in 0..8 {
            bus.clear(2048);
            synth.process(&ctx(2048), &mut bus, &EventList::empty(), None);
        }
        assert!(
            synth.voices.iter().all(|v| !v.active),
            "a voice never went idle"
        );
        for voice in &synth.voices {
            assert_eq!(voice.feedback_tap, 0.0, "an idle voice kept a feedback tail");
        }
    }

    /// Eight is the pool. A ninth note takes the oldest slot rather than
    /// finding a hidden one.
    #[test]
    fn a_ninth_note_steals_the_oldest_voice() {
        let mut synth = MlP8::new(init_saw(), SR);
        let mut bus = StereoBus::with_capacity(512);
        let mut events = EventList::empty();
        for index in 0..9u8 {
            events.push(note_on(0, u64::from(index) + 1, 48 + index * 3));
        }
        synth.process(&ctx(512), &mut bus, &events, None);
        assert_eq!(synth.voices.iter().filter(|v| v.active).count(), MLP8_VOICES);
        // The first note's slot was taken by the ninth.
        assert_eq!(synth.voices[0].event_id, 9);
    }

    // --- Step 05: allocation, character, and the finisher -----------------

    /// A quiet sine with a flat middle. Pitch drift moves a sine's frequency
    /// and not its amplitude, which is what lets the sustain tests below
    /// separate "the times drifted" from "the level drifted".
    fn sine_bed() -> MlP8Params {
        let mut params = init_saw();
        params.osc[0].wave = OscWave::Sine;
        params.attack = 0.001;
        params.decay = 0.001;
        params.sustain = 0.6;
        params.release = 0.1;
        params
    }

    /// Both channels, which spread and chorus are the whole point of.
    fn render_stereo(params: MlP8Params, notes: &[u8], frames: usize) -> (Vec<f32>, Vec<f32>) {
        let mut synth = MlP8::new(params, SR);
        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        for (index, note) in notes.iter().enumerate() {
            events.push(note_on(0, index as u64 + 1, *note));
        }
        synth.process(&ctx(frames), &mut bus, &events, None);
        (bus.l[..frames].to_vec(), bus.r[..frames].to_vec())
    }

    /// Play `notes` all at once and report the synth, so a test can look at
    /// what the pool actually did with them.
    fn played(params: MlP8Params, notes: &[u8], frames: usize) -> MlP8 {
        let mut synth = MlP8::new(params, SR);
        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        for (index, note) in notes.iter().enumerate() {
            events.push(note_on(0, index as u64 + 1, *note));
        }
        synth.process(&ctx(frames), &mut bus, &events, None);
        synth
    }

    /// How many distinct notes are still held, and how many slots they cost.
    fn held(synth: &MlP8) -> (usize, usize) {
        let mut ids: Vec<u64> = synth
            .voices
            .iter()
            .filter(|voice| voice.gate)
            .map(|voice| voice.event_id)
            .collect();
        let slots = ids.len();
        ids.sort_unstable();
        ids.dedup();
        (ids.len(), slots)
    }

    /// The headline of the step: Unison spends the eight slots, it never adds
    /// to them, and the note polyphony it leaves is exactly the division.
    #[test]
    fn unison_divides_the_pool_it_never_grows_it() {
        let notes = [48, 52, 55, 59, 62, 65, 69, 72];
        for (unison, expected) in [
            (MlP8Unison::X1, 8),
            (MlP8Unison::X2, 4),
            (MlP8Unison::X4, 2),
            (MlP8Unison::X8, 1),
        ] {
            let mut params = init_saw();
            params.unison = unison;
            let synth = played(params, &notes, 2048);
            let (voices, slots) = held(&synth);
            assert_eq!(
                voices, expected,
                "{unison:?} left {voices} notes sounding, not {expected}"
            );
            // And it spent the whole pool doing it, rather than reaching past
            // it or leaving half of it idle.
            assert_eq!(slots, MLP8_VOICES, "{unison:?} occupied {slots} slots");
            assert!(
                synth.voices.iter().filter(|v| v.active).count() <= MLP8_VOICES,
                "{unison:?} built more than eight physical voices"
            );
        }
    }

    /// A group is the unit of allocation. Playing more notes than the pool
    /// can hold at 4x steals a whole group at a time, so every note that is
    /// still held is held by all four of its voices — never by three.
    #[test]
    fn a_group_is_allocated_and_stolen_whole() {
        let mut params = init_saw();
        params.unison = MlP8Unison::X4;
        // Three notes into a pool that holds two groups of four.
        let synth = played(params, &[48, 55, 62], 2048);
        for voice in synth.voices.iter().filter(|v| v.gate) {
            let siblings = synth
                .voices
                .iter()
                .filter(|other| other.gate && other.event_id == voice.event_id)
                .count();
            assert_eq!(
                siblings,
                usize::from(voice.members),
                "event {} is held by {siblings} of {} voices",
                voice.event_id,
                voice.members
            );
        }
        let (voices, _) = held(&synth);
        assert_eq!(voices, 2, "a third group was allocated into a full pool");
    }

    /// Note Off is a group operation too: one event id releases every member,
    /// and none of them is left gated behind.
    #[test]
    fn note_off_releases_every_member_of_its_group() {
        let mut params = init_saw();
        params.unison = MlP8Unison::X4;
        let mut synth = MlP8::new(params, SR);
        let mut bus = StereoBus::with_capacity(1024);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 48));
        events.push(note_on(0, 2, 60));
        synth.process(&ctx(1024), &mut bus, &events, None);
        assert_eq!(held(&synth), (2, 8));

        let mut off = EventList::empty();
        off.push(TimedEvent {
            offset: 0,
            event: Event::NoteOff { id: 1, note: 48 },
        });
        synth.process(&ctx(1024), &mut bus, &off, None);
        let (voices, slots) = held(&synth);
        assert_eq!((voices, slots), (1, 4), "half a group survived its Note Off");
        // The released four are still sounding out their tail, not cut.
        assert!(
            synth.voices.iter().filter(|v| v.active).count() > 4,
            "the released group was dropped rather than released"
        );
    }

    /// Shrinking Unison under a sounding chord leaves no partial group. The
    /// old topology leaves through the short transition; it is not resized.
    #[test]
    fn changing_unison_releases_the_old_groups_rather_than_resizing_them() {
        let mut params = init_saw();
        params.unison = MlP8Unison::X8;
        let mut synth = MlP8::new(params, SR);
        let mut bus = StereoBus::with_capacity(1024);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 48));
        synth.process(&ctx(1024), &mut bus, &events, None);
        assert_eq!(held(&synth), (1, 8));

        params.unison = MlP8Unison::X2;
        synth.set_params(params);
        assert_eq!(held(&synth).1, 0, "a group stayed gated across the change");
        assert!(
            synth.voices.iter().all(|v| !v.active || v.env.is_releasing()),
            "a voice kept sounding at its old group size"
        );

        // And the next note gets the new topology, whole.
        let mut next = EventList::empty();
        next.push(note_on(0, 2, 60));
        synth.process(&ctx(1024), &mut bus, &next, None);
        assert_eq!(held(&synth), (1, 2));
    }

    /// A slot stolen by a smaller group has nothing to adopt. It has to leave
    /// through the release rather than simply stopping, which is the one
    /// sample of silence in the middle of a sound the plan forbids.
    #[test]
    fn a_slot_stolen_by_a_smaller_group_is_released_not_cut() {
        let mut params = sine_bed();
        params.unison = MlP8Unison::X8;
        let mut synth = MlP8::new(params, SR);
        let mut bus = StereoBus::with_capacity(4096);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 48));
        synth.process(&ctx(4096), &mut bus, &events, None);

        // The whole pool is one group, and it is still sounding out the
        // release the Unison change started. A 2x note arriving into that
        // steals all eight and has a use for two of them.
        params.unison = MlP8Unison::X2;
        synth.set_params(params);
        let mut next = EventList::empty();
        next.push(note_on(0, 2, 60));
        bus.clear(4096);
        // A short block, because the five-millisecond transition is the thing
        // being looked at: run a long one and every surplus slot has already
        // finished leaving by the time the assertion reads it.
        synth.process(&ctx(128), &mut bus, &next, None);

        assert_eq!(held(&synth), (1, 2));
        let retired = synth.voices.iter().filter(|v| v.active && !v.gate).count();
        assert_eq!(retired, 6, "{retired} of the six surplus slots were released");
        // The step where they should have been cut off is not in the audio.
        let step = bus.l[..128]
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0_f32, f32::max);
        assert!(step < 0.1, "the handoff stepped by {step}");
    }

    /// Choke and transport stop reach whole groups by construction, and a
    /// group that has left is left with no member still gated.
    #[test]
    fn choke_and_transport_stop_take_whole_groups() {
        for stop_the_transport in [false, true] {
            let mut params = init_saw();
            params.unison = MlP8Unison::X4;
            let mut synth = MlP8::new(params, SR);
            let mut bus = StereoBus::with_capacity(1024);
            let mut events = EventList::empty();
            events.push(note_on(0, 1, 48));
            events.push(note_on(0, 2, 60));
            synth.process(&ctx(1024), &mut bus, &events, None);
            assert_eq!(held(&synth), (2, 8));

            let mut stop = EventList::empty();
            let mut context = ctx(1024);
            if stop_the_transport {
                context.playing = false;
            } else {
                stop.push(TimedEvent {
                    offset: 0,
                    event: Event::Choke,
                });
            }
            synth.process(&context, &mut bus, &stop, None);
            assert_eq!(held(&synth), (0, 0), "a member survived the stop");
            // Released, not cut: they are still sounding their short tail.
            assert!(
                synth.voices.iter().all(|v| !v.active || v.env.is_releasing()),
                "a voice was left running without a gate"
            );
        }
    }

    /// Detune and Spread are symmetric about the note, so a group plays the
    /// pitch it was sent and sits in the middle of the field.
    #[test]
    fn detune_and_spread_are_symmetric_about_the_note() {
        let mut params = init_saw();
        params.unison = MlP8Unison::X4;
        params.detune = 0.8;
        params.spread = 1.0;
        let synth = played(params, &[60], 1024);
        let group: Vec<&Voice> = synth.voices.iter().filter(|v| v.gate).collect();
        assert_eq!(group.len(), 4);

        let offsets: f32 = group.iter().map(|v| v.group_offset()).sum();
        assert!(offsets.abs() < 1.0e-6, "the group is not zero-mean: {offsets}");
        let pans: f32 = group.iter().map(|v| v.spread_pan).sum();
        assert!(pans.abs() < 1.0e-6, "the image is not centred: {pans}");
        // The outermost members reach the ends, so the amount means what it
        // says rather than a fraction of it.
        let extremes = group
            .iter()
            .map(|v| v.spread_pan)
            .fold((0.0_f32, 0.0_f32), |(lo, hi), p| (lo.min(p), hi.max(p)));
        assert!((extremes.0 + 1.0).abs() < 1.0e-6 && (extremes.1 - 1.0).abs() < 1.0e-6);
    }

    /// Zero means exactly zero, not a small number: both controls leave the
    /// pitch and the pan bit-for-bit where the patch put them.
    #[test]
    fn detune_and_spread_at_zero_change_nothing() {
        let mut params = init_saw();
        params.unison = MlP8Unison::X4;
        let synth = played(params, &[60], 1024);
        for voice in synth.voices.iter().filter(|v| v.gate) {
            assert_eq!(voice.pitch_scale, [1.0; 3], "detune moved a pitch at zero");
            assert_eq!(voice.spread_pan, 0.0, "spread moved a voice at zero");
            assert_eq!(voice.spread_gain, pan_gains(0.0));
        }
        // Four voices of the same note at the same pitch and pan is four
        // times one voice, exactly.
        let (left, right) = render_stereo(params, &[60], 4096);
        assert_eq!(left, right, "a centred group is not centred");
    }

    /// At 1x, Spread places notes by their stable slot positions, so a chord
    /// occupies the field the same way on every render rather than following
    /// the order the notes arrived in.
    #[test]
    fn spread_at_1x_places_a_chord_by_slot_and_repeats_it() {
        let mut params = init_saw();
        params.spread = 1.0;
        let (left, right) = render_stereo(params, &[48, 55, 62, 69], 4096);
        assert_ne!(left, right, "a spread chord is still centred");
        // Deterministic: the same chord twice is the same image twice.
        let (again_l, again_r) = render_stereo(params, &[48, 55, 62, 69], 4096);
        assert_eq!((left, right), (again_l, again_r));

        // And it is the slot that decides, not the arrival order: slot 0 sits
        // hard left and slot 7 hard right whatever is playing.
        let synth = played(params, &[60], 1024);
        assert!((synth.voices[0].pan_offset() + 1.0).abs() < 1.0e-6);
        assert!((synth.voices[MLP8_VOICES - 1].pan_offset() - 1.0).abs() < 1.0e-6);
    }

    /// Drift 0 is not "nearly authored". Every multiplier it introduces is
    /// exactly one, so the render is bit-identical to the same patch with the
    /// control never touched.
    #[test]
    fn drift_zero_is_bit_for_bit_the_authored_patch() {
        let mut params = sine_bed();
        params.filter_cutoff = 0.5;
        params.filter_keytrack = 1.0;
        let authored = render_chord(params, &[48, 60, 67], 8192);
        params.drift = 0.0;
        assert_eq!(render_chord(params, &[48, 60, 67], 8192), authored);

        let synth = played(params, &[60], 512);
        for voice in &synth.voices {
            assert_eq!(voice.pitch_scale, [1.0; 3]);
            assert_eq!(voice.cutoff_scale, 1.0);
        }
    }

    /// Drift 100 differs measurably, uses no entropy, and does not move the
    /// sustain *level* — only the times either side of it.
    #[test]
    fn drift_full_differs_deterministically_without_touching_sustain() {
        let mut params = sine_bed();
        params.filter_cutoff = 0.5;
        let flat = render_chord(params, &[48, 60, 67], 16384);
        params.drift = 1.0;
        let drifted = render_chord(params, &[48, 60, 67], 16384);
        assert_ne!(flat, drifted, "Drift 100 changed nothing");

        // No runtime entropy: two fresh devices agree sample for sample.
        assert_eq!(render_chord(params, &[48, 60, 67], 16384), drifted);

        // The sustained level is the patch's, not the slot's.
        //
        // One note, one sine, and the filter left open, because that is the
        // only arrangement in which the sustained peak measures the envelope
        // and nothing else: a chord's peak follows the phases its members
        // happen to be at, and a filter's output follows its cutoff, and
        // Drift moves both of those on purpose.
        let mut open = sine_bed();
        let peak = |signal: &[f32]| {
            signal[8192..].iter().fold(0.0, |a: f32, s| a.max(s.abs()))
        };
        let flat_sustain = peak(&render(open, 60, 16384));
        open.drift = 1.0;
        let drifted_sustain = peak(&render(open, 60, 16384));
        assert!(
            (flat_sustain - drifted_sustain).abs() < flat_sustain * 0.01,
            "sustain moved from {flat_sustain} to {drifted_sustain}"
        );
        // And it is the sustain that was measured, not silence.
        assert!(flat_sustain > 0.2, "the bed did not reach sustain");
    }

    /// Drift's start phases are a property of the slot, so a note played
    /// twice into the same idle slot renders identically both times.
    #[test]
    fn drift_start_phase_is_a_slot_property_not_a_new_number_each_note() {
        let mut params = sine_bed();
        params.drift = 1.0;
        let first = render(params, 60, 4096);
        let second = render(params, 60, 4096);
        assert_eq!(first, second);
        // And the slots differ from each other, which is the point of it.
        let synth = played(params, &[60], 512);
        assert_ne!(synth.voices[0].drift.phase, synth.voices[1].drift.phase);
    }

    /// OFF contributes no chorus processing at all: the stage reports itself
    /// bypassed and its wet gain is exactly zero, so the voices reach the
    /// channel bus without a copy or a delay line in the way.
    #[test]
    fn chorus_off_is_a_true_bypass() {
        let mut synth = MlP8::new(init_saw(), SR);
        assert!(synth.chorus.settle(MlP8Chorus::Off));
        assert_eq!(synth.chorus.gain.value(), 0.0);
        assert_eq!(synth.chorus.active, MlP8Chorus::Off);
    }

    /// The three live modes are distinct from OFF and from each other, and
    /// each of them processes only ML-P8's own output.
    #[test]
    fn the_three_chorus_modes_are_distinct() {
        let mut params = sine_bed();
        params.osc[0].wave = OscWave::Saw;
        let dry = render_chord(params, &[48, 60], 16384);
        let mut rendered = vec![dry.clone()];
        for mode in [MlP8Chorus::One, MlP8Chorus::Two, MlP8Chorus::Ensemble] {
            params.chorus = mode;
            let wet = render_chord(params, &[48, 60], 16384);
            assert!(wet.iter().all(|s| s.is_finite()), "{mode:?} went non-finite");
            for (index, other) in rendered.iter().enumerate() {
                assert_ne!(&wet, other, "{mode:?} is the same as mode {index}");
            }
            rendered.push(wet);
        }
    }

    /// The chorus reads ML-P8's scratch bus, never the channel's. A bus that
    /// already carries another generator's audio comes back with that audio
    /// untouched and ML-P8 added to it.
    #[test]
    fn the_chorus_never_rewrites_what_was_already_on_the_bus() {
        const FRAMES: usize = 4096;
        let mut params = sine_bed();
        params.chorus = MlP8Chorus::Ensemble;
        let mut synth = MlP8::new(params, SR);
        let mut bus = StereoBus::with_capacity(FRAMES);
        // Somebody else's signal, already summed onto the channel.
        let existing: Vec<f32> = (0..FRAMES)
            .map(|i| (i as f32 * 0.01).sin() * 0.25)
            .collect();
        bus.l[..FRAMES].copy_from_slice(&existing);
        bus.r[..FRAMES].copy_from_slice(&existing);

        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        synth.process(&ctx(FRAMES), &mut bus, &events, None);

        // ML-P8's own contribution, rendered alone.
        let mine = render(params, 60, FRAMES);
        for index in 0..FRAMES {
            let expected = existing[index] + mine[index];
            assert!(
                (bus.l[index] - expected).abs() < 1.0e-6,
                "frame {index}: {} is not {expected}",
                bus.l[index]
            );
        }
    }

    /// A mode change on sounding material crosses rather than steps, and OFF
    /// is reached the same way, so neither direction clicks.
    #[test]
    fn a_chorus_mode_change_crosses_instead_of_stepping() {
        const FRAMES: usize = 24_000;
        let mut params = sine_bed();
        let steady = |signal: &[f32]| {
            signal
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0_f32, f32::max)
        };

        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        let held_still = render_with(params, events, FRAMES);
        let baseline = steady(&held_still);

        for (from, to) in [
            (MlP8Chorus::Off, MlP8Chorus::Ensemble),
            (MlP8Chorus::Ensemble, MlP8Chorus::Off),
            (MlP8Chorus::One, MlP8Chorus::Two),
        ] {
            params.chorus = from;
            let mut events = EventList::empty();
            events.push(note_on(0, 1, 60));
            // Well past the attack, so the change is the only thing moving.
            events.push(TimedEvent {
                offset: 12_000,
                event: Event::ParamValue {
                    id: mooloop_core::mlp8::PARAM_CHORUS,
                    value: to.to_index() as f32,
                },
            });
            let crossed = render_with(params, events, FRAMES);
            let step = steady(&crossed);
            assert!(
                step < baseline * 4.0 + 0.01,
                "{from:?} to {to:?} stepped by {step} against a baseline of {baseline}"
            );
            assert!(crossed.iter().all(|s| s.is_finite()));
        }
    }

    /// Everything at once, at the top of every control the step adds: eight
    /// physical voices, one note, full drift, full detune, full spread, and
    /// the widest chorus. It has to stay finite and stay inside what honest
    /// summing predicts.
    #[test]
    fn the_worst_case_of_this_step_stays_finite_and_bounded() {
        let mut params = init_saw();
        for osc in params.osc.iter_mut() {
            osc.level = 1.0;
        }
        params.sub_level = 1.0;
        params.noise_level = 1.0;
        params.unison = MlP8Unison::X8;
        params.drift = 1.0;
        params.detune = 1.0;
        params.spread = 1.0;
        params.chorus = MlP8Chorus::Ensemble;
        params.voice_feedback = 1.0;
        params.drive = 1.0;

        let out = render_chord(params, &[36, 48, 60, 72], 16384);
        assert!(out.iter().all(|s| s.is_finite()), "worst case went non-finite");
        let peak = out.iter().fold(0.0_f32, |a, s| a.max(s.abs()));
        // Five unit sources a voice, eight voices, at the voice reference and
        // hard left -- plus the wet the finisher adds beside the dry, which
        // is bounded by the dry because it is a filtered sum of delayed taps.
        let ceiling = 2.0 * 5.0 * VOICE_OUTPUT_REFERENCE * MLP8_VOICES as f32;
        println!("step 05 worst-case peak: {peak:.3}, honest ceiling {ceiling:.3}");
        assert!(peak < ceiling, "worst case peaked at {peak}, over {ceiling}");
        // And unison did not quietly buy a ninth voice to do it with.
        let synth = played(params, &[36, 48, 60, 72], 4096);
        assert!(synth.voices.iter().filter(|v| v.active).count() <= MLP8_VOICES);
    }

    /// The whole step, rendered twice: unison, drift, detune, spread, and the
    /// chorus are all deterministic, so an offline render is the take.
    #[test]
    fn the_finished_voice_renders_identically_across_instances() {
        let mut params = init_saw();
        params.unison = MlP8Unison::X4;
        params.drift = 0.75;
        params.detune = 0.5;
        params.spread = 0.8;
        params.chorus = MlP8Chorus::Two;
        let first = render_stereo(params, &[48, 60], 12_288);
        let second = render_stereo(params, &[48, 60], 12_288);
        assert_eq!(first, second);
    }

    /// The factory identity case the plan names: 1x, Drift 0, Chorus OFF is
    /// what a fresh device already is, so step 07's listening pass starts
    /// from the instrument rather than from its finishers.
    #[test]
    fn the_default_patch_is_the_acceptance_case() {
        let params = MlP8Params::default();
        assert_eq!(params.unison, MlP8Unison::X1);
        assert_eq!(params.chorus, MlP8Chorus::Off);
        assert_eq!(params.drift, 0.0);
        assert_eq!(params.detune, 0.0);
        assert_eq!(params.spread, 0.0);
    }

    // --- Step 06: what the instrument publishes ---------------------------

    fn note_off(offset: u32, id: u64, note: u8) -> TimedEvent {
        TimedEvent {
            offset,
            event: Event::NoteOff { id, note },
        }
    }

    /// Render one block of `frames` with `events`, then publish. Returns the
    /// outlets as they stand at the end of that block.
    fn outlets_after(synth: &mut MlP8, events: EventList, frames: usize) -> MlP8ControlOutlets {
        let mut bus = StereoBus::with_capacity(frames);
        synth.process(&ctx(frames), &mut bus, &events, None);
        synth.publish_outlets()
    }

    /// The published LFO is the signal the voices were driven with, not a
    /// second evaluation of the same shape. Re-deriving it is exactly how the
    /// two would come to disagree, so the test compares against a bare LFO
    /// advanced over the same span.
    #[test]
    fn the_lfo_outlet_is_the_signal_the_instrument_used() {
        let mut params = init_saw();
        params.lfo.rate_hz = 3.0;
        let mut synth = MlP8::new(params, SR);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        let published = outlets_after(&mut synth, events, 4096)[OUTLET_LFO as usize];

        let mut reference = MlP8Lfo::new();
        for _ in 0..4096 {
            reference.next_sample(&params.lfo, 120.0, SR);
        }
        assert_eq!(published, reference.value());
        // And it is not vacuously zero.
        assert!(published.abs() > 1.0e-3, "the LFO published {published}");
    }

    /// A unison group publishes one envelope, not eight. The reduction is the
    /// mean over the focus group, which is the thing that makes the outlet
    /// usable at any Unison setting.
    #[test]
    fn the_envelope_outlets_reduce_over_the_focus_group() {
        let mut params = sine_bed();
        params.unison = MlP8Unison::X4;
        params.drift = 1.0;
        let mut synth = MlP8::new(params, SR);
        let mut events = EventList::empty();
        events.push(note_on(0, 1, 60));
        let outlets = outlets_after(&mut synth, events, 12_000);

        // Drift moves the four members' envelope times apart, so their levels
        // genuinely differ and the mean is doing work rather than averaging
        // four copies of one number.
        let levels: Vec<f32> = synth
            .voices
            .iter()
            .filter(|voice| voice.active && voice.age == synth.focus)
            .map(|voice| voice.env.level())
            .collect();
        assert_eq!(levels.len(), 4);
        let mean = levels.iter().sum::<f32>() / 4.0;
        assert!(
            (outlets[OUTLET_AMP_ENV as usize] - mean).abs() < 1.0e-6,
            "published {} against a group mean of {mean}",
            outlets[OUTLET_AMP_ENV as usize]
        );
        assert!(outlets[OUTLET_AMP_ENV as usize] > 0.0);
    }

    /// The focus is the newest group and it stays the focus through its own
    /// release. When it falls idle its outlets go to zero rather than
    /// jumping backward to an older note that is still held.
    #[test]
    fn the_focus_follows_the_newest_note_and_falls_to_zero_rather_than_backward() {
        let mut params = sine_bed();
        params.release = 0.05;
        let mut synth = MlP8::new(params, SR);

        // An old note, held for the whole test.
        let mut held = EventList::empty();
        held.push(note_on(0, 1, 36));
        let low = outlets_after(&mut synth, held, 4096);
        assert!((low[OUTLET_NOTE as usize] - 36.0 / 127.0).abs() < 1.0e-6);

        // A newer note takes the focus immediately.
        let mut newer = EventList::empty();
        newer.push(note_on(0, 2, 84));
        let high = outlets_after(&mut synth, newer, 4096);
        assert!((high[OUTLET_NOTE as usize] - 84.0 / 127.0).abs() < 1.0e-6);

        // Released, it stays the focus through its tail: the envelope is
        // still falling rather than already zero.
        let mut release = EventList::empty();
        release.push(note_off(0, 2, 84));
        let tail = outlets_after(&mut synth, release, 512);
        assert!((tail[OUTLET_NOTE as usize] - 84.0 / 127.0).abs() < 1.0e-6);
        assert!(tail[OUTLET_AMP_ENV as usize] > 0.0, "the tail was cut short");
        assert!(tail[OUTLET_AMP_ENV as usize] < high[OUTLET_AMP_ENV as usize]);

        // Idle, its outlets return to zero. The low note is still held, and
        // the point is that Note does *not* go back to it.
        let quiet = outlets_after(&mut synth, EventList::empty(), 12_000);
        assert_eq!(quiet[OUTLET_NOTE as usize], 0.0);
        assert_eq!(quiet[OUTLET_AMP_ENV as usize], 0.0);
        assert_eq!(quiet[OUTLET_VELOCITY as usize], 0.0);
        // But Gate is not the focus's gate: the older note is still down.
        assert_eq!(quiet[OUTLET_GATE as usize], 1.0);
    }

    /// Gate and Trigger say different things, which is the reason for having
    /// both. Gate stays high across overlapping held notes; Trigger fires
    /// once per Note On and is cleared by being published.
    #[test]
    fn gate_spans_overlapping_notes_while_trigger_fires_once_each() {
        let mut synth = MlP8::new(sine_bed(), SR);

        let mut first = EventList::empty();
        first.push(note_on(0, 1, 48));
        let a = outlets_after(&mut synth, first, 512);
        assert_eq!(a[OUTLET_GATE as usize], 1.0);
        assert_eq!(a[OUTLET_TRIGGER as usize], 1.0);

        // No new note: the gate holds, the trigger does not repeat.
        let b = outlets_after(&mut synth, EventList::empty(), 512);
        assert_eq!(b[OUTLET_GATE as usize], 1.0);
        assert_eq!(b[OUTLET_TRIGGER as usize], 0.0);

        // A second note overlapping the first fires again.
        let mut second = EventList::empty();
        second.push(note_on(0, 2, 55));
        let c = outlets_after(&mut synth, second, 512);
        assert_eq!(c[OUTLET_TRIGGER as usize], 1.0);

        // Releasing the newer note leaves the gate high, because the older
        // one is still held. This is the case a focus-following gate would
        // get wrong.
        let mut release_new = EventList::empty();
        release_new.push(note_off(0, 2, 55));
        let d = outlets_after(&mut synth, release_new, 512);
        assert_eq!(d[OUTLET_GATE as usize], 1.0);

        // Only the last Note Off lowers it, and it follows the scheduled
        // event rather than the VCA's release tail.
        let mut release_old = EventList::empty();
        release_old.push(note_off(0, 1, 48));
        let e = outlets_after(&mut synth, release_old, 64);
        assert_eq!(e[OUTLET_GATE as usize], 0.0);
        assert!(e[OUTLET_AMP_ENV as usize] > 0.0, "the tail ended with the gate");
    }

    /// Velocity publishes the note's own velocity, not the smoothed VCA
    /// gain. On a stolen slot the smoother is still sliding from the
    /// previous note, and publishing that would make the outlet a fact about
    /// the ramp rather than about the event.
    #[test]
    fn velocity_publishes_the_event_not_the_ramp() {
        let mut synth = MlP8::new(sine_bed(), SR);
        let mut loud = EventList::empty();
        loud.push(TimedEvent {
            offset: 0,
            event: Event::NoteOn { id: 1, note: 60, velocity: 127 },
        });
        assert_eq!(outlets_after(&mut synth, loud, 4096)[OUTLET_VELOCITY as usize], 1.0);

        // Fill the pool, then steal with a quiet note. The stolen slot's
        // smoother is mid-slide; the outlet is not.
        let mut chord = EventList::empty();
        for index in 1..MLP8_VOICES as u64 + 1 {
            chord.push(TimedEvent {
                offset: 0,
                event: Event::NoteOn {
                    id: index + 1,
                    note: 48 + index as u8,
                    velocity: 127,
                },
            });
        }
        outlets_after(&mut synth, chord, 4096);

        let mut quiet = EventList::empty();
        quiet.push(TimedEvent {
            offset: 0,
            event: Event::NoteOn { id: 99, note: 72, velocity: 32 },
        });
        let stolen = outlets_after(&mut synth, quiet, 8);
        let expected = 32.0 / 127.0;
        assert!(
            (stolen[OUTLET_VELOCITY as usize] - expected).abs() < 1.0e-6,
            "published {} rather than {expected}",
            stolen[OUTLET_VELOCITY as usize]
        );
        // The smoother really is still elsewhere, so the two are distinct.
        let focus = synth
            .voices
            .iter()
            .find(|voice| voice.active && voice.age == synth.focus)
            .unwrap();
        assert!((focus.velocity_amp.value() - expected).abs() > 0.05);
    }

    /// Publication is deterministic: the same event stream publishes the same
    /// values twice, which is what makes an offline render agree with a live
    /// take once the engine reads these.
    #[test]
    fn the_outlets_publish_identically_across_instances() {
        let run = || {
            let mut params = sine_bed();
            params.unison = MlP8Unison::X2;
            params.drift = 0.5;
            params.lfo.rate_hz = 5.0;
            let mut synth = MlP8::new(params, SR);
            let mut published = Vec::new();
            for block in 0..8 {
                let mut events = EventList::empty();
                if block % 3 == 0 {
                    events.push(note_on(0, block as u64 + 1, 48 + block as u8 * 2));
                }
                published.push(outlets_after(&mut synth, events, 1024));
            }
            published
        };
        assert_eq!(run(), run());
    }

    /// A skipped oscillator has to be skipped for the right reason. Muting
    /// one that nothing reads must not change the two that are heard.
    #[test]
    fn skipping_an_unread_oscillator_changes_nothing() {
        let mut heard = init_saw();
        heard.osc[1].level = 0.8;
        heard.xmod[xmod_index(0, 1)] = 50.0;
        let with_third_silent = render(heard, 60, 4096);

        // Osc 3 is silent and nothing routes from it, so it is genuinely
        // unused -- and giving it a wave, a tuning, and a pulse width must
        // stay inaudible.
        let mut noisy = heard;
        noisy.osc[2].wave = OscWave::Pulse;
        noisy.osc[2].semitones = 5.0;
        noisy.osc[2].pulse_width = 0.2;
        assert_eq!(render(noisy, 60, 4096), with_third_silent);
    }
}


