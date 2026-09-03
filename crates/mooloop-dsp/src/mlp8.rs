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
//! The filter, the two envelopes, and the voice feedback loop are step 03;
//! this device currently runs its source mix straight into the VCA.

use crate::bus::{pan_gains, StereoBus};
use crate::env::Adsr;
use crate::event::{Event, EventList};
use crate::filter::{soft_ceiling, PreDrive, Svf};
use crate::node::{AudioNode, ProcessContext};
use crate::osc::{sync_blep, Noise, Osc};
use crate::scale::hz_from_normalized;
use crate::smooth::Smoothed;
use crate::synth_voice::{note_to_freq, MIN_GLIDE_S, PARAM_SMOOTH_S, STOP_RELEASE_S};
use mooloop_core::mlp8::{MlP8Routes, MLP8_MAX_ROUTES, MLP8_MOD_DESTS};
use mooloop_core::{
    mlp8::xmod_index, MlP8FilterMode, MlP8LfoParams, MlP8LfoWave, MlP8Params,
    OscWave, SubWave, MLP8_VOICES,
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

/// One physical voice. Eight of these exist for the life of the device.
/// How much of one cycle full Slew rounds off.
///
/// A fraction of the cycle rather than a time in seconds, which is the whole
/// idea: a sample-and-hold at half Slew is the same wander at 0.2 Hz as at
/// 20 Hz, so the control survives being automated alongside Rate instead of
/// meaning something different at every speed.
// Step 04 builds the LFO and the route compiler; step 05 is what reads
// them. Allowed by item rather than by file so the day a later step
// leaves something genuinely unused, the lint still says so.
#[allow(dead_code)]
const LFO_SLEW_MAX_CYCLES: f32 = 0.5;

/// The narrowest either half of a warped cycle may become. A pivot that
/// reaches the edge is a shape with no rising side at all, which is a
/// division by zero before it is a sound.
#[allow(dead_code)]
const LFO_WARP_MIN_SIDE: f32 = 0.02;

/// The chaos pair's frequency ratio. Irrational on purpose: a rational ratio
/// closes the figure and the "chaos" wave becomes a periodic one.
#[allow(dead_code)]
const CHAOS_RATIO: f32 = 0.618_034;

/// How hard each phasor bends the other's rate. Weak coupling lets the pair
/// phase-lock, which is the failure mode that would quietly turn this back
/// into a periodic wave, so it is set well past that.
#[allow(dead_code)]
const CHAOS_COUPLING: f32 = 0.85;

/// Seed for the sample-and-hold sequence. Fixed, so two renders of the same
/// events are the same samples.
#[allow(dead_code)]
const LFO_SH_SEED: u32 = 0x1d3f_a7c5;

/// ML-P8's own LFO: one global shape, evaluated per sample.
///
/// Global rather than per voice because it is the instrument's clock; the
/// route amounts are what make it land differently on each voice.
#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
fn sin_tau(phase: f32) -> f32 {
    (phase * core::f32::consts::TAU).sin()
}

/// Skew a phase about a moved pivot, so half the cycle is spent on each side
/// of it. Turns a triangle into a ramp, a pulse into a variable width, and a
/// sine into a shape that leans.
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct CompiledRoute {
    source: usize,
    slot: usize,
    scale: f32,
}

/// A patch's routes, compiled flat.
#[allow(dead_code)]
struct CompiledRoutes {
    routes: [CompiledRoute; MLP8_MAX_ROUTES],
    len: usize,
    /// Which destination slots any route touches, so a voice zeroes and reads
    /// only those. An unrouted patch does no per-sample work at all.
    touched: [bool; MLP8_MOD_DESTS],
    any: bool,
}

#[allow(dead_code)]
impl CompiledRoutes {
    fn new() -> Self {
        Self {
            routes: [CompiledRoute {
                source: 0,
                slot: 0,
                scale: 0.0,
            }; MLP8_MAX_ROUTES],
            len: 0,
            touched: [false; MLP8_MOD_DESTS],
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
            let scale = route.amount.clamp(-1.0, 1.0) * route.dest.full_range();
            if scale == 0.0 {
                continue;
            }
            self.routes[self.len] = CompiledRoute {
                source: route.source.to_index() as usize,
                slot,
                scale,
            };
            self.len += 1;
            self.touched[slot] = true;
        }
        self.any = self.len > 0;
    }
}

/// Dense slot indices the voice reads back. Derived from the same `ALL` order
/// the UI lists, rather than written out twice.
#[allow(dead_code)]
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

struct Voice {
    active: bool,
    event_id: u64,
    note: u8,
    age: u64,
    /// Stable across the device's life, so a voice's noise sequence and its
    /// later per-slot drift are properties of the slot rather than of the
    /// order notes arrived in.
    slot: u32,
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
    /// Velocity gain, smoothed so a stolen retrigger at a different velocity
    /// slides rather than steps.
    velocity_amp: Smoothed,
    osc_level: [Smoothed; 3],
    sub_level: Smoothed,
    noise_level: Smoothed,
    cutoff: Smoothed,
    drive_amount: Smoothed,
    feedback: Smoothed,
}

impl Voice {
    fn new(slot: u32, sample_rate: u32) -> Self {
        let smoothed = |initial| Smoothed::new(initial, PARAM_SMOOTH_S, sample_rate);
        Self {
            active: false,
            event_id: 0,
            note: 0,
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
            noise: ColoredNoise::new(noise_seed(slot)),
            noise_tap: 0.0,
            current_freq: 0.0,
            target_freq: 0.0,
            velocity_amp: smoothed(0.0),
            osc_level: [smoothed(0.0); 3],
            sub_level: smoothed(0.0),
            noise_level: smoothed(0.0),
            cutoff: smoothed(1.0),
            drive_amount: smoothed(0.0),
            feedback: smoothed(0.0),
        }
    }

    /// Return every piece of network state to its start. Called when a slot
    /// that was genuinely idle takes a note, so a repeated note renders
    /// identically rather than inheriting a phase from whatever came before.
    fn restart(&mut self) {
        for osc in &mut self.oscs {
            osc.reset();
        }
        self.taps = [0.0; 3];
        self.sync_carry = [0.0; 3];
        self.sub.reset();
        self.sub_carry = 0.0;
        self.noise.reset(noise_seed(self.slot));
        self.noise_tap = 0.0;
        self.filter.reset();
        self.drive = PreDrive::new();
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
struct Prepared {
    ratio: [f32; 3],
    wave: [OscWave; 3],
    pulse_width: [f32; 3],
    /// `xmod[from][to]`, already in cycles.
    xmod: [[f32; 3]; 3],
    feedback: [f32; 3],
    noise_to_osc: [f32; 3],
    sync_master: [Option<usize>; 3],
    osc_needed: [bool; 3],
    noise_needed: bool,
    noise_tilt: f32,
    sub_source: usize,
    sub_ratio: f32,
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

impl Prepared {
    fn new(params: &MlP8Params, sample_rate: u32) -> Self {
        let mut ratio = [1.0_f32; 3];
        let mut wave = [OscWave::Saw; 3];
        let mut pulse_width = [0.5_f32; 3];
        for (index, osc) in params.osc.iter().enumerate() {
            let semis = osc.semitones.clamp(-48.0, 48.0) + osc.cents.clamp(-100.0, 100.0) / 100.0;
            ratio[index] = (semis / 12.0).exp2();
            wave[index] = osc.wave;
            pulse_width[index] = osc.pulse_width;
        }

        let mut xmod = [[0.0_f32; 3]; 3];
        for (from, row) in xmod.iter_mut().enumerate() {
            for (to, depth) in row.iter_mut().enumerate() {
                if from != to {
                    *depth = route_depth(params.xmod[xmod_index(from, to)]);
                }
            }
        }
        let feedback = std::array::from_fn(|n| route_depth(params.osc_feedback[n]));
        let noise_to_osc = std::array::from_fn(|n| route_depth(params.noise_to_osc[n]));
        let sync_master: [Option<usize>; 3] = std::array::from_fn(|n| {
            // An oscillator syncing to itself is not a topology, it is a
            // stuck phase. The UI excludes it; this makes the DSP agree
            // whatever a project file says.
            params.sync_source[n].master().filter(|m| *m != n)
        });

        let sub_source = params.sub_source.index();
        let sub_needed = params.sub_level > 0.0;
        let audible = |n: usize| params.osc[n].level > 0.0;

        // What makes an oscillator live: it is heard, it modulates something,
        // it syncs something, or the sub divides it. Level alone does not
        // decide, because a muted oscillator is a legitimate modulator — that
        // is the point of the device.
        let osc_needed: [bool; 3] = std::array::from_fn(|n| {
            audible(n)
                || (0..3).any(|to| to != n && xmod[n][to] != 0.0)
                || feedback[n] != 0.0
                || sync_master.contains(&Some(n))
                || (sub_needed && sub_source == n)
        });
        let noise_needed = params.noise_level > 0.0 || noise_to_osc.iter().any(|a| *a != 0.0);

        Self {
            ratio,
            wave,
            pulse_width,
            xmod,
            feedback,
            noise_to_osc,
            sync_master,
            osc_needed,
            noise_needed,
            noise_tilt: (params.noise_color * 0.01).clamp(-1.0, 1.0),
            sub_source,
            sub_ratio: ratio[sub_source] / params.sub_octave.divisor(),
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
            // filter", so only the low-pass modes can be skipped.
            filter_open: matches!(params.filter_mode, MlP8FilterMode::Lp12 | MlP8FilterMode::Lp24)
                && params.filter_cutoff >= FILTER_OPEN
                && params.filter_resonance <= f32::EPSILON
                && params.filter_env_amount.abs() <= f32::EPSILON
                && params.filter_velocity.abs() <= f32::EPSILON
                && params.filter_keytrack <= f32::EPSILON
                && params.drive <= f32::EPSILON
                && params.voice_feedback.abs() <= f32::EPSILON,
        }
    }
}

/// The ML-P8 node.
pub struct MlP8 {
    params: MlP8Params,
    sample_rate: u32,
    voices: [Voice; MLP8_VOICES],
    next_age: u64,
}

impl MlP8 {
    pub fn new(params: MlP8Params, sample_rate: u32) -> Self {
        let mut synth = Self {
            params,
            sample_rate,
            voices: std::array::from_fn(|slot| Voice::new(slot as u32, sample_rate)),
            next_age: 1,
        };
        synth.apply_params_to_voices();
        synth
    }

    /// Replace the parameter set. Called from the RT command drain.
    pub fn set_params(&mut self, params: MlP8Params) {
        self.params = params;
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

    fn apply_params_to_voices(&mut self) {
        for voice in &mut self.voices {
            voice.env.configure(
                self.params.attack,
                self.params.decay,
                self.params.sustain,
                self.params.release,
            );
            voice.filter_env.configure(
                self.params.filter_attack,
                self.params.filter_decay,
                self.params.filter_sustain,
                self.params.filter_release,
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
        self.apply_params_to_voices();
    }

    pub fn choke(&mut self) {
        self.release_all();
    }

    /// An idle slot if there is one, otherwise the oldest sounding note.
    ///
    /// Eight is the whole pool; there is no polyphony parameter to consult,
    /// which is what makes the stealing rule and the CPU ceiling honest.
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

    fn note_on(&mut self, event_id: u64, note: u8, velocity: u8) {
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
            // Fresh slot: no glide from silence, and every piece of network
            // state starts where it started last time.
            voice.current_freq = voice.target_freq;
            voice.restart();
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

    fn note_off(&mut self, event_id: u64) {
        for voice in self
            .voices
            .iter_mut()
            .filter(|voice| voice.active && voice.event_id == event_id)
        {
            voice.env.release();
            voice.filter_env.release();
        }
    }

    fn release_all(&mut self) {
        for voice in &mut self.voices {
            if voice.active && !voice.env.is_releasing() {
                voice.env.release_with(STOP_RELEASE_S);
                voice.filter_env.release_with(STOP_RELEASE_S);
            }
        }
    }

    fn render_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        if start >= end {
            return;
        }
        let params = self.params;
        let sr = self.sample_rate;
        let prepared = Prepared::new(&params, sr);
        let glide_coeff = (-1.0 / (params.glide.max(MIN_GLIDE_S) * sr as f32)).exp();
        let (gain_l, gain_r) = pan_gains(0.0);

        for voice in self.voices.iter_mut() {
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
        }

        for frame in start..end {
            for voice in self.voices.iter_mut() {
                if !voice.active {
                    continue;
                }
                voice.env.advance();
                voice.filter_env.advance();
                if voice.env.is_idle() {
                    voice.active = false;
                    voice.clear_loop();
                    continue;
                }
                voice.current_freq +=
                    (voice.target_freq - voice.current_freq) * (1.0 - glide_coeff);
                let velocity = voice.velocity_amp.advance();
                let mix = voice.next_sample(&prepared, sr);
                let shaped = voice.shape(&prepared, mix, velocity, sr);
                // Velocity at the VCA is a crossfade from "every note the
                // same" to "every note as played", not a multiply -- at zero
                // depth a soft note is a full-level note rather than silence.
                let amp = 1.0 - prepared.amp_velocity * (1.0 - velocity);
                let sample = shaped * voice.env.level() * amp * VOICE_OUTPUT_REFERENCE;
                bus.l[frame] += sample * gain_l;
                bus.r[frame] += sample * gain_r;
            }
        }
    }
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
        let level: [f32; 3] = std::array::from_fn(|n| self.osc_level[n].advance());
        let sub_level = self.sub_level.advance();
        let noise_level = self.noise_level.advance();
        let live: [bool; 3] =
            std::array::from_fn(|n| prep.osc_needed[n] || level[n] > LEVEL_EPSILON);
        let sub_live = prep.sub_needed || sub_level > LEVEL_EPSILON;
        let noise_live = prep.noise_needed || noise_level > LEVEL_EPSILON;

        let noise = if noise_live {
            self.noise.next_sample(prep.noise_tilt, &prep.color)
        } else {
            0.0
        };

        let mut value = [0.0_f32; 3];
        let mut wrap = [None; 3];
        let mut freq = [0.0_f32; 3];
        let mut offset = [0.0_f32; 3];
        for index in 0..3 {
            if !live[index] {
                continue;
            }
            let mut phase_mod = prep.feedback[index] * self.taps[index]
                + prep.noise_to_osc[index] * self.noise_tap;
            for source in 0..3 {
                if source != index {
                    phase_mod += prep.xmod[source][index] * self.taps[source];
                }
            }
            offset[index] = bound_phase(phase_mod);
            freq[index] = self.current_freq * prep.ratio[index];
            let step = self.oscs[index].next_step(
                freq[index],
                prep.wave[index],
                prep.pulse_width[index],
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
                prep.pulse_width[index],
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
            let sub_freq = self.current_freq * prep.sub_ratio;
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
        let cutoff = self.cutoff.advance();
        let drive = self.drive_amount.advance();
        let feedback = self.feedback.advance();
        if prep.filter_open && feedback == 0.0 && drive == 0.0 {
            return mix;
        }

        // One sample of delay, bounded before it re-enters. `soft_ceiling` is
        // exactly transparent below its knee, so an ordinary patch is
        // untouched and only a runaway meets it.
        let returned = soft_ceiling(self.feedback_tap * feedback * VOICE_FEEDBACK_RANGE);
        let driven = self.drive.next_sample(mix + returned, drive, sample_rate);

        let base_hz = hz_from_normalized(cutoff, prep.max_hz);
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
        let depth = prep.env_amount + prep.filter_velocity * velocity;
        let cutoff_hz =
            (tracked * (self.filter_env.level() * depth * FILTER_ENV_OCTAVES).exp2())
                .clamp(20.0, prep.max_hz);

        let filtered =
            self.filter
                .next_sample(prep.mode, driven, cutoff_hz, prep.resonance, sample_rate);

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
    use mooloop_core::mlp8::xmod_index;
    use mooloop_core::{MlP8FilterMode, SubOctave, SubSource, SyncSource};

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
