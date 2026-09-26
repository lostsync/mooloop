//! Feedback-delay-network hall reverb with in-loop diffusion.
//!
//! Eight delay lines feed back through a normalized Hadamard matrix. Input
//! passes a pre-delay, a low cut, and a chain of Schroeder allpass diffusers
//! before it reaches them; each line's return is damped, attenuated to hit a
//! target RT60, and — the part that makes the tail read as a *room* rather than
//! a *box* — passed through its own allpass diffuser **inside** the feedback
//! loop before it is written back. Cost is a fixed handful of taps and
//! multiplies per sample and does not vary with `decay_s` at all.
//!
//! ## Why the diffusion is in the loop
//!
//! An FDN whose only diffusion is on the input smears the *first* arrival and
//! then leaves the tail to the bare delay lines recirculating through the
//! matrix. Eight lines is a sparse set of late echoes, and a sparse late echo
//! train is exactly what a listener hears as metallic ringing and springy
//! flutter: the tail never fills in, so it rings on a handful of modes instead
//! of blooming into a wash. Putting a short allpass in series with each delay
//! line means every lap around the network multiplies the echo density instead
//! of merely relocating it — after a few laps the tail is dense enough to read
//! as air rather than as a comb. This is the same trick a Griesinger/Dattorro
//! tank uses, carried onto an FDN so the network keeps its hall-like spatial
//! spread. The allpasses are unity-magnitude, so they reshape density without
//! touching the per-line decay budget the feedback gains solve for; the trip
//! length the gains *are* solved against just includes the allpass length.
//!
//! ## Why this shape at all
//!
//! This device used to be a partitioned FFT convolution against a generated
//! room impulse response. That had three problems this structure does not:
//!
//! - **Load distribution.** All partitions were multiplied and accumulated in
//!   the one `process` call where the 512-sample input window filled, so a
//!   2 s tail spent ~1400 us in one block out of eight and nothing in the
//!   rest — over budget at a 64-frame JACK period despite an affordable mean.
//!   An FDN's cost is flat by construction; there is no window and no spike.
//! - **Modulation.** A convolution node cannot accept a parameter change: its
//!   response has to be regenerated and FFT-partitioned off-thread, then
//!   swapped in whole. The old node ignored `events_in` outright, so every
//!   modulation route pointed at a reverb knob was silently inert even though
//!   the destination metadata declared it legal. Here every parameter is an
//!   ordinary `Event::ParamValue` applied at its sample offset, which is all
//!   `docs/MODULATION.md` ever asked an effect to do.
//! - **Sound.** A finite image-source model plus a filtered noise tail is
//!   geometrically defensible and static: nothing in it moves, so the tail
//!   rings rather than blooms. The delay lines here are slowly and
//!   independently modulated, which is what keeps a long tail from settling
//!   into a fixed set of modes.
//!
//! Input is summed to mono before the network, matching the plate; stereo
//! comes out of two orthogonal taps across the delay lines, so the two
//! channels are decorrelated rather than panned. Wet/dry is the generic
//! per-slot blend in `EffectChain`, not a parameter here.

use mooloop_core::{
    ReverbParams, REVERB_PARAM_DAMPING, REVERB_PARAM_DECAY_S, REVERB_PARAM_DIFFUSION,
    REVERB_PARAM_LOW_CUT_HZ, REVERB_PARAM_MODULATION, REVERB_PARAM_PREDELAY_MS, REVERB_PARAM_SIZE,
    REVERB_PARAM_WIDTH,
};

use crate::bus::StereoBus;
use crate::event::EventList;
use crate::filter::OnePoleLp;
use crate::node::{AudioNode, Discontinuity, ProcessContext};
use crate::smooth::Smoothed;
use super::{process_param_split, RangeProcessor};

/// Delay lines in the network. Eight lines carry the hall's spatial spread;
/// the echo density the first design leaned on line count for is now supplied
/// by the in-loop allpasses, which is what lets eight stay affordable and
/// still read as dense.
const LINES: usize = 8;
const DIFFUSERS: usize = 4;

/// Frames [`ReverbEffect::process_chunk`] runs its input stage across before
/// the loop reads it. The scratch rows live on the stack, two of this many
/// floats; a longer range is taken in pieces.
const CHUNK: usize = 128;

/// Reference sample rate the tuning lengths below are written at.
const TUNING_SAMPLE_RATE: f32 = 48_000.0;

/// Delay lengths in samples at [`TUNING_SAMPLE_RATE`] and `size` 0.5. Primes,
/// so the lines' echo trains stay out of phase with each other and the
/// network does not collapse onto a common period. Spread 21..50 ms: short
/// enough that the tail is dense from the start, long enough that the
/// spacing between arrivals reads as a hall rather than a box.
const LINE_TUNING: [usize; LINES] = [1013, 1201, 1409, 1601, 1811, 2003, 2213, 2411];

/// Input diffuser lengths in samples at [`TUNING_SAMPLE_RATE`] and `size`
/// 0.5, ascending so each stage smears the previous one's output across a
/// longer window. Coprime with each other and with [`LINE_TUNING`].
const DIFFUSER_TUNING: [usize; DIFFUSERS] = [229, 331, 557, 719];

/// In-loop allpass lengths in samples at [`TUNING_SAMPLE_RATE`] and `size`
/// 0.5, one per delay line. Short (2.6..11 ms) and prime, coprime with each
/// other and with [`LINE_TUNING`], so each line's recirculating signal is
/// smeared across a different, incommensurate window every lap. These are the
/// stages that turn eight sparse echo trains into a continuous wash.
const LOOP_DIFFUSER_TUNING: [usize; LINES] = [127, 179, 241, 293, 359, 421, 479, 541];

/// `size` (0..1) maps onto this tap-length multiplier range, geometrically,
/// so the default 0.5 lands exactly on the tuning above. The ends are a
/// small ~8 ms-shortest chamber and a ~125 ms-longest cathedral.
const SIZE_MIN_MULTIPLIER: f32 = 0.4;
const SIZE_MAX_MULTIPLIER: f32 = 2.5;

/// Feedback gain ceiling. The Hadamard matrix is orthonormal, the damping
/// filters have gain at most one, and an allpass has magnitude exactly one, so
/// the loop is a contraction whenever every per-line gain is under one; this
/// leaves margin so that a pathological `decay_s`/`size` pair cannot ring
/// indefinitely.
const FEEDBACK_MAX: f32 = 0.9995;

/// Damping is a one-pole lowpass *inside* the feedback loop, so its effect
/// compounds once per trip around a delay line — sixty-odd times over a
/// typical tail. That is why the coefficient stays in a narrow band near
/// transparency: `damping = 0` must leave the coefficient at exactly 1.0
/// (the filter passes its input through untouched), and even the maximum
/// only takes it to `1 - DAMP_MAX_LOSS`, which is already a very dark hall
/// after cascading. Freeverb's damping tops out in the same region and for
/// the same reason.
const DAMP_MAX_LOSS: f32 = 0.45;

/// Schroeder allpass gain at `diffusion = 1` for the *input* diffusers. Past
/// about 0.75 the stages start to ring audibly rather than smear.
const DIFFUSION_MAX_GAIN: f32 = 0.72;

/// Fixed gain of the in-loop allpasses. High enough that each lap adds real
/// density, low enough that the stages smear rather than ring — the same
/// ceiling the input diffusers respect, held constant here because in-loop
/// diffusion is structural to the hall rather than a control. It is not driven
/// by `diffusion`: that knob shapes the *onset* (discrete echoes to a wash)
/// while these keep the *tail* from going sparse, and coupling them would make
/// a low Diffuse setting metallic again.
const LOOP_DIFFUSION_GAIN: f32 = 0.6;

/// Peak delay-line modulation depth in milliseconds at `modulation = 1`.
/// Enough to keep the tail's modes from standing still; below the point
/// where the pitch movement is heard as vibrato on sustained material.
const MOD_DEPTH_MS: f32 = 0.45;

/// Modulation rates in Hz, one per line, mutually incommensurate so the
/// lines never sweep together and produce a single audible wobble.
const MOD_RATE_HZ: [f32; LINES] = [0.317, 0.457, 0.631, 0.729, 0.853, 0.971, 1.093, 1.217];

/// Smoothing time for the parameters that scale amplitude or read position
/// directly. Size and decay are excluded: they retune the network rather
/// than scale it, same as the plate rebuilding its feedback on a size change.
const SMOOTH_S: f32 = 0.02;

/// Time constant the delay lengths glide over on a `size` change. Long
/// enough that a jumped knob does not chirp, short enough that a swept one
/// tracks: at a full-range jump the read heads move a few samples per sample
/// and the tail zips like tape, which is the honest sound of a room changing
/// size and is what every other reverb with a modulatable size does.
const SIZE_GLIDE_S: f32 = 0.05;

/// Pre-delay is smoothed far more slowly than the amplitude controls. It
/// moves a read head, so a fast ramp is a pitch bend; at 120 ms a modulated
/// pre-delay glides like tape rather than chirping.
const PREDELAY_SMOOTH_S: f32 = 0.12;

/// The hall's absolute output reference, the same kind of constant as the
/// plate's. A feedback network has no natural unity: its steady-state level
/// depends on decay, size, and how the input's energy sits against the
/// network's modes. This pins typical sustained material within a couple of
/// dB of the dry path it is blended against, and is enforced by
/// `steady_state_wet_path_is_level_matched` in `gain_structure_tests.rs`.
const OUTPUT_REFERENCE: f32 = 0.185;

/// A mono ring buffer with a linearly interpolated read head.
///
/// Deliberately not `crate::delayline::DelayLine`: that type is stereo (half
/// of it would be wasted eight times over) and interpolates with a 4-point
/// Hermite kernel sized for the buffer device's reverse and repitched reads.
/// Here the head moves by well under a sample per sample, and linear's
/// gentle high-frequency droop at fractional offsets is indistinguishable
/// from the damping already in the loop.
struct Ring {
    buffer: Vec<f32>,
    write: usize,
}

impl Ring {
    /// Zero the stored audio and return the write head to the start.
    /// Allocation-free, but it touches the whole ring: a discontinuity, not a
    /// block.
    fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.write = 0;
    }

    /// Allocates: construct off the audio thread.
    fn with_capacity(frames: usize) -> Self {
        Self {
            buffer: vec![0.0; frames.max(4)],
            write: 0,
        }
    }

    fn write(&mut self, value: f32) {
        self.buffer[self.write] = value;
        self.write += 1;
        if self.write >= self.buffer.len() {
            self.write = 0;
        }
    }

    /// Read `delay` samples behind the write head. `delay` is clamped into
    /// the ring, so a parameter can never read uninitialized history.
    ///
    /// The whole part is a truncating conversion, not `floor`: the clamp
    /// leaves `delay` at least one, where the two agree to the bit, and on
    /// x86-64's baseline (no SSE4.1) `floor` is a call into libm. Twenty-one
    /// reads a frame made that twenty-one calls (MOO-254). A NaN converts to
    /// zero either way.
    fn read(&self, delay: f32) -> f32 {
        let capacity = self.buffer.len();
        let delay = delay.clamp(1.0, capacity as f32 - 2.0);
        let back = delay as i32 as usize;
        let frac = delay - back as f32;
        // `back` is clamped inside the ring and the head is too, so the sum
        // is under two laps and one subtraction reduces it. The eight lines
        // and twelve diffusers read at least once a sample each, so this is
        // twenty integer divisions a frame that do not happen.
        let raw = self.write + capacity - back;
        let index = if raw >= capacity { raw - capacity } else { raw };
        let previous = if index == 0 { capacity - 1 } else { index - 1 };
        self.buffer[index] * (1.0 - frac) + self.buffer[previous] * frac
    }

    /// [`Self::read`] as it was before MOO-254, with `floor`: the reference
    /// the restructured loop is pinned against.
    #[cfg(test)]
    fn read_before_moo254(&self, delay: f32) -> f32 {
        let capacity = self.buffer.len();
        let delay = delay.clamp(1.0, capacity as f32 - 2.0);
        let base = delay.floor();
        let frac = delay - base;
        let back = base as usize;
        let raw = self.write + capacity - back;
        let index = if raw >= capacity { raw - capacity } else { raw };
        let previous = if index == 0 { capacity - 1 } else { index - 1 };
        self.buffer[index] * (1.0 - frac) + self.buffer[previous] * frac
    }
}

/// A Schroeder allpass diffuser: a delay tap whose feedforward and feedback
/// gains cancel, so it reshapes echo density without colouring the response.
/// Distinct from `crate::filter::AllPass`, which is a single-sample phase
/// stage rather than a delay-line diffuser. Used both for the input chain and,
/// one per line, inside the feedback loop.
struct Diffuser {
    ring: Ring,
    len: f32,
    target_len: f32,
}

impl Diffuser {
    fn clear(&mut self) {
        self.ring.clear();
    }

    fn new(base_len: usize, sample_rate: u32) -> Self {
        let capacity = scaled_len(base_len, sample_rate, SIZE_MAX_MULTIPLIER) + 4;
        let len = capacity as f32 - 4.0;
        Self {
            ring: Ring::with_capacity(capacity),
            len,
            target_len: len,
        }
    }

    fn set_len(&mut self, len: f32) {
        self.target_len = len.clamp(1.0, self.ring.buffer.len() as f32 - 2.0);
    }

    /// The part of `process` that runs on the clock rather than on the
    /// input, for a block the host skipped. See `AudioNode::skip_block`.
    fn step_silent(&mut self, glide: f32) {
        self.len += (self.target_len - self.len) * glide;
    }

    /// Whether the glide has stopped moving `len`. It never reaches
    /// `target_len` in float: it stalls within a rounding step of it, where
    /// one more step returns `len` itself. From there every later step does
    /// too, so a caller that sees this may skip the glide until the target
    /// or the coefficient changes, and stay bit-identical (MOO-254).
    fn at_rest(&self, glide: f32) -> bool {
        self.len + (self.target_len - self.len) * glide == self.len
    }

    fn process(&mut self, input: f32, gain: f32, glide: f32) -> f32 {
        self.len += (self.target_len - self.len) * glide;
        self.process_at_rest(input, gain)
    }

    /// [`Self::process`] without the glide, for a diffuser [`Self::at_rest`].
    fn process_at_rest(&mut self, input: f32, gain: f32) -> f32 {
        let delayed = self.ring.read(self.len);
        let stored = input + delayed * gain;
        self.ring.write(stored);
        delayed - stored * gain
    }

    #[cfg(test)]
    fn process_before_moo254(&mut self, input: f32, gain: f32, glide: f32) -> f32 {
        self.len += (self.target_len - self.len) * glide;
        let delayed = self.ring.read_before_moo254(self.len);
        let stored = input + delayed * gain;
        self.ring.write(stored);
        delayed - stored * gain
    }
}

/// One delay line of the network, with its damping filter, feedback gain,
/// modulation phase, and the in-loop allpass its return is diffused through.
struct Line {
    ring: Ring,
    /// Nominal length in samples, before modulation. Glides toward
    /// `target_len` rather than jumping: see [`SIZE_GLIDE_S`].
    len: f32,
    /// Where `size` most recently asked this line to end up.
    target_len: f32,
    /// Peak modulation excursion in samples.
    mod_depth: f32,
    /// Phase increment per sample, in turns.
    mod_step: f32,
    phase: f32,
    damp: OnePoleLp,
    feedback: f32,
    /// The allpass the recirculating signal passes through before it is
    /// written back into the line. This is what makes the tail dense.
    loop_ap: Diffuser,
}

impl Line {
    /// Forget the stored audio and the damping state -- **but not `phase`**.
    /// The line's modulation is free-running: it advances whether or not this
    /// node is called, so it has to arrive at the same place across a seek
    /// exactly as it does across a sleep, or a bounce stops matching a take.
    fn clear(&mut self) {
        self.ring.clear();
        self.damp.reset();
        self.feedback = 0.0;
        self.loop_ap.clear();
    }

    fn new(base_len: usize, loop_base_len: usize, sample_rate: u32, index: usize) -> Self {
        // Room for the longest size plus the modulation excursion and the
        // interpolator's reach, so `size` never has to reallocate.
        let capacity = scaled_len(base_len, sample_rate, SIZE_MAX_MULTIPLIER)
            + mod_depth_samples(sample_rate) as usize
            + 8;
        let mut damp = OnePoleLp::new();
        damp.set_coeff(1.0);
        let len = scaled_len(base_len, sample_rate, 1.0) as f32;
        Self {
            ring: Ring::with_capacity(capacity),
            len,
            target_len: len,
            mod_depth: 0.0,
            mod_step: MOD_RATE_HZ[index] / sample_rate.max(1) as f32,
            // Spread the starting phases evenly so the lines do not all
            // begin at the same excursion.
            phase: index as f32 / LINES as f32,
            damp,
            feedback: 0.0,
            loop_ap: Diffuser::new(loop_base_len, sample_rate),
        }
    }

    /// Advance the modulation oscillator and the size glide without reading,
    /// for a block the host skipped. The in-loop allpass glides here too, so a
    /// size change that lands mid-sleep is tracked exactly. See
    /// `AudioNode::skip_block`.
    fn step_silent(&mut self, glide: f32) {
        self.len += (self.target_len - self.len) * glide;
        self.phase += self.mod_step;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        self.loop_ap.step_silent(glide);
    }

    /// Whether both of this line's glides, its length's and its loop
    /// allpass's, have stalled. See [`Diffuser::at_rest`].
    fn at_rest(&self, glide: f32) -> bool {
        self.len + (self.target_len - self.len) * glide == self.len && self.loop_ap.at_rest(glide)
    }

    /// Advance the modulation oscillator and read the line, as the sample
    /// loop did before MOO-254 ran the eight lines side by side.
    ///
    /// The modulator is a triangle rather than a sine: it costs an absolute
    /// value instead of a `sin`, and at these depths and rates the difference
    /// is a slightly different distribution of the same small pitch drift.
    #[cfg(test)]
    fn read_before_moo254(&mut self, glide: f32) -> f32 {
        self.len += (self.target_len - self.len) * glide;
        self.phase += self.mod_step;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        let triangle = 4.0 * (self.phase - 0.5).abs() - 1.0;
        self.ring.read_before_moo254(self.len + triangle * self.mod_depth)
    }
}

/// Length in samples of a base tuning value at `sample_rate` and `multiplier`.
fn scaled_len(base: usize, sample_rate: u32, multiplier: f32) -> usize {
    ((base as f32 * multiplier * sample_rate.max(1) as f32 / TUNING_SAMPLE_RATE).round() as usize)
        .max(1)
}

fn mod_depth_samples(sample_rate: u32) -> f32 {
    MOD_DEPTH_MS * 0.001 * sample_rate.max(1) as f32
}

fn size_multiplier(size: f32) -> f32 {
    SIZE_MIN_MULTIPLIER
        * (SIZE_MAX_MULTIPLIER / SIZE_MIN_MULTIPLIER).powf(size.clamp(0.0, 1.0))
}

/// In-place normalized fast Walsh-Hadamard transform.
///
/// This is the network's mixing matrix. It is orthonormal — so it can neither
/// add nor remove energy, which is what lets the per-line gains alone set the
/// decay time — and it is dense, so one trip through it spreads every line's
/// output across all eight. Twenty-four add/subtracts and eight multiplies,
/// against sixty-four multiply-accumulates for the same matrix written out.
fn hadamard(values: &mut [f32; LINES]) {
    let mut span = 1;
    while span < LINES {
        let mut base = 0;
        while base < LINES {
            for offset in base..base + span {
                let a = values[offset];
                let b = values[offset + span];
                values[offset] = a + b;
                values[offset + span] = a - b;
            }
            base += span * 2;
        }
        span *= 2;
    }
    // 1/sqrt(8): three butterfly stages each grew the norm by sqrt(2).
    const NORM: f32 = 0.353_553_4;
    for value in values.iter_mut() {
        *value *= NORM;
    }
}

/// Output tap signs. Two rows of the Hadamard matrix, which are orthogonal to
/// each other, so the left and right taps of the same network are
/// decorrelated: the stereo image is a genuinely different perspective on the
/// tail rather than a panned copy of one signal.
const TAP_L: [f32; LINES] = [1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0];
const TAP_R: [f32; LINES] = [1.0, -1.0, 1.0, -1.0, -1.0, 1.0, -1.0, 1.0];

/// Injection signs, a third orthogonal row, so the input excites every line
/// but does not arrive at the two output taps in phase.
const INJECT: [f32; LINES] = [1.0, 1.0, 1.0, 1.0, -1.0, -1.0, -1.0, -1.0];

pub struct ReverbEffect {
    params: ReverbParams,
    sample_rate: u32,
    predelay: Ring,
    predelay_samples: Smoothed,
    /// The `low_cut` control's one-pole, subtracted from its own input to
    /// make a highpass. Deliberately on the input and not in the feedback
    /// loop: a highpass inside the loop compounds once per trip the way
    /// damping does, and any corner high enough to control mud in one pass
    /// would strip the bass out of the tail entirely over sixty.
    low_cut: OnePoleLp,
    diffusers: [Diffuser; DIFFUSERS],
    lines: [Line; LINES],
    diffusion: Smoothed,
    width: Smoothed,
    /// One-pole coefficient the delay lengths glide with, derived from
    /// [`SIZE_GLIDE_S`] and cached so the inner loop never recomputes it.
    size_glide: f32,
    /// Per-dependency dirty flags, resolved lazily by [`Self::resolve_dirty`]
    /// from [`RangeProcessor::process_range`] -- the same coalescing pattern
    /// `PlateEffect` uses and for the same reason: `size` legitimately marks
    /// `feedback` dirty too (`rebuild_feedback`'s RT60 formula reads the
    /// `target_len` a size change moves), so what this buys is one rebuild
    /// per dependency per control tick even when several parameters driving
    /// it change together, rather than the old eager scheme's one rebuild
    /// per `apply_param` call.
    size_dirty: bool,
    feedback_dirty: bool,
    damping_dirty: bool,
    modulation_dirty: bool,
    /// Test-only: see `PlateEffect`'s identical field for why a behavioural
    /// test cannot otherwise tell a dirty-flag skip apart from a redundant
    /// rebuild landing on the same numbers.
    #[cfg(test)]
    rebuild_counts: ReverbRebuildCounts,
    /// Run the sample loop as it was before MOO-254, for comparison.
    #[cfg(test)]
    before_moo254: bool,
}

#[cfg(test)]
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
struct ReverbRebuildCounts {
    size: u32,
    feedback: u32,
    damping: u32,
    modulation: u32,
}

impl ReverbEffect {
    /// Allocates the network's rings; call from the control side.
    pub fn new(params: ReverbParams, sample_rate: u32) -> Self {
        let sample_rate = sample_rate.max(1);
        let max_predelay = predelay_capacity(sample_rate);
        let mut low_cut = OnePoleLp::new();
        low_cut.set_cutoff(params.low_cut_hz.clamp(20.0, 500.0), sample_rate);
        let mut effect = Self {
            params,
            sample_rate,
            predelay: Ring::with_capacity(max_predelay),
            predelay_samples: Smoothed::new(
                predelay_samples(params.predelay_ms, sample_rate),
                PREDELAY_SMOOTH_S,
                sample_rate,
            ),
            low_cut,
            diffusers: std::array::from_fn(|i| Diffuser::new(DIFFUSER_TUNING[i], sample_rate)),
            lines: std::array::from_fn(|i| {
                Line::new(LINE_TUNING[i], LOOP_DIFFUSER_TUNING[i], sample_rate, i)
            }),
            diffusion: Smoothed::new(
                params.diffusion.clamp(0.0, 1.0) * DIFFUSION_MAX_GAIN,
                SMOOTH_S,
                sample_rate,
            ),
            width: Smoothed::new(params.width.clamp(0.0, 1.0), SMOOTH_S, sample_rate),
            size_glide: glide_coeff(SIZE_GLIDE_S, sample_rate),
            size_dirty: true,
            feedback_dirty: true,
            damping_dirty: true,
            modulation_dirty: true,
            #[cfg(test)]
            rebuild_counts: ReverbRebuildCounts::default(),
            #[cfg(test)]
            before_moo254: false,
        };
        effect.resolve_dirty();
        effect
    }

    /// Resolve whatever `apply_param` (or a sample-rate change) marked dirty
    /// since the last call. Called from `process_range`, the same coalescing
    /// point `PlateEffect::resolve_dirty` uses and for the same reason: size
    /// first, because it moves the `target_len` the feedback formula reads,
    /// so several dependencies changing within one control tick resolve into
    /// one rebuild each rather than one per `apply_param` call.
    fn resolve_dirty(&mut self) {
        if self.size_dirty {
            self.do_resize();
            self.size_dirty = false;
            #[cfg(test)]
            {
                self.rebuild_counts.size += 1;
            }
        }
        if self.feedback_dirty {
            self.do_rebuild_feedback();
            self.feedback_dirty = false;
            #[cfg(test)]
            {
                self.rebuild_counts.feedback += 1;
            }
        }
        if self.damping_dirty {
            self.do_rebuild_damping();
            self.damping_dirty = false;
            #[cfg(test)]
            {
                self.rebuild_counts.damping += 1;
            }
        }
        if self.modulation_dirty {
            self.do_rebuild_modulation();
            self.modulation_dirty = false;
            #[cfg(test)]
            {
                self.rebuild_counts.modulation += 1;
            }
        }
    }

    /// Retune every delay length for the current `size`.
    ///
    /// This sets *targets*. Unlike the plate's comb resize, which clears its
    /// buffers and accepts the discontinuity, the lengths here glide, because
    /// `size` is a legal modulation destination and has to survive being
    /// swept: moving a read head straight to a new offset lands it on
    /// uncorrelated history, which is a click, not a room change.
    fn do_resize(&mut self) {
        let multiplier = size_multiplier(self.params.size);
        let sample_rate = self.sample_rate;
        for (i, line) in self.lines.iter_mut().enumerate() {
            let ceiling = line.ring.buffer.len() as f32 - mod_depth_samples(sample_rate) - 4.0;
            line.target_len =
                (scaled_len(LINE_TUNING[i], sample_rate, multiplier) as f32).min(ceiling.max(1.0));
            line.loop_ap
                .set_len(scaled_len(LOOP_DIFFUSER_TUNING[i], sample_rate, multiplier) as f32);
        }
        for (diffuser, base) in self.diffusers.iter_mut().zip(DIFFUSER_TUNING) {
            diffuser.set_len(scaled_len(base, sample_rate, multiplier) as f32);
        }
    }

    /// Solve each line's feedback gain for the target RT60.
    ///
    /// A signal circulating line `i` loses `gain` every `trip` samples, so to
    /// fall 60 dB in `decay_s` it needs `gain = 10^(-3 * trip / (decay * fs))`.
    /// The trip is the delay length **plus** the in-loop allpass length: the
    /// signal passes through both once per lap, and leaving the allpass out
    /// would make the tail run a touch long. Solving per line rather than
    /// sharing one gain is what makes the lines' decays line up: the long lines
    /// are attenuated less per trip because they make fewer trips.
    fn do_rebuild_feedback(&mut self) {
        let decay_s = self.params.decay_s.max(0.05);
        let sample_rate = self.sample_rate as f32;
        for line in self.lines.iter_mut() {
            // Solved against the target rather than the gliding current
            // length, so a size change costs one `powf` per line instead of
            // one per line per sample. Mid-glide the decay is briefly off by
            // the fraction the length still has to travel — a few tens of
            // milliseconds, under a control that is itself moving.
            let seconds = (line.target_len + line.loop_ap.target_len) / sample_rate;
            line.feedback = 10f32
                .powf(-3.0 * seconds / decay_s)
                .clamp(0.0, FEEDBACK_MAX);
        }
    }

    fn do_rebuild_damping(&mut self) {
        // `damping = 0` must be exactly transparent, not merely bright: a
        // coefficient of 1.0 makes `OnePoleLp::next_sample` return its input.
        let coeff = 1.0 - self.params.damping.clamp(0.0, 1.0) * DAMP_MAX_LOSS;
        for line in self.lines.iter_mut() {
            line.damp.set_coeff(coeff);
        }
    }

    fn do_rebuild_modulation(&mut self) {
        let depth = mod_depth_samples(self.sample_rate) * self.params.modulation.clamp(0.0, 1.0);
        for line in self.lines.iter_mut() {
            line.mod_depth = depth;
        }
    }
}

/// One-pole coefficient reaching ~63% of a step in `time_s`. The same
/// mapping `Smoothed` uses; spelled out here because the delay lengths are
/// smoothed inside their own structs rather than through a `Smoothed`.
fn glide_coeff(time_s: f32, sample_rate: u32) -> f32 {
    let samples = (time_s.max(1.0e-5) * sample_rate.max(1) as f32).max(1.0);
    1.0 - (-1.0 / samples).exp()
}

fn predelay_samples(predelay_ms: f32, sample_rate: u32) -> f32 {
    (predelay_ms.clamp(1.0, 200.0) * 0.001 * sample_rate.max(1) as f32).max(1.0)
}

fn predelay_capacity(sample_rate: u32) -> usize {
    (0.2 * sample_rate.max(1) as f32).ceil() as usize + 4
}

impl ReverbEffect {
    /// Up to [`CHUNK`] frames of the network, in two stages (MOO-254).
    ///
    /// **The input stage**, one stage at a time across the chunk: the
    /// pre-delay and low cut into a scratch row, then each diffuser over the
    /// whole row in turn. None of it reads the feedback loop, and each stage's
    /// state is its own, so only the loop order differs from running every
    /// stage for one sample before the next: each operation and its inputs are
    /// the same, and so is every sample.
    ///
    /// **The loop**, one sample at a time as before, but with the eight lines
    /// side by side: their lengths, phases, damping and allpass lengths are
    /// copied into `[f32; 8]` rows for the chunk and written back after it, so
    /// the glide, the modulator, the damping, the matrix and the allpass
    /// arithmetic are eight-wide row operations. Only the ring reads and
    /// writes stay one line at a time. Each line's arithmetic is the one
    /// `Line::read` and `Diffuser::process` did, in the same order.
    ///
    /// A glide that has stalled ([`Diffuser::at_rest`]) is skipped for the
    /// chunk, which is every glide but the few hundred milliseconds after a
    /// `size` change. `size` only changes between ranges, never inside one.
    // The loop indexes several rows by line on purpose: one index across all
    // of them is what lets each row operation be read as one vector step.
    #[allow(clippy::needless_range_loop)]
    fn process_chunk(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        let frames = end - start;
        let glide = self.size_glide;

        // Pre-delay. The one-pole highpass sits after it and before the
        // diffusers, so the network never sees subsonic content at all.
        let mut diffused = [0.0f32; CHUNK];
        let diffused = &mut diffused[..frames];
        for (value, (left, right)) in diffused
            .iter_mut()
            .zip(bus.l[start..end].iter().zip(&bus.r[start..end]))
        {
            let dry = (*left + *right) * 0.5;
            self.predelay.write(dry);
            let delayed = self.predelay.read(self.predelay_samples.advance());
            *value = delayed - self.low_cut.next_sample(delayed);
        }
        let mut gains = [0.0f32; CHUNK];
        let gains = &mut gains[..frames];
        for gain in gains.iter_mut() {
            *gain = self.diffusion.advance();
        }
        for diffuser in self.diffusers.iter_mut() {
            if diffuser.at_rest(glide) {
                for (value, gain) in diffused.iter_mut().zip(gains.iter()) {
                    *value = diffuser.process_at_rest(*value, *gain);
                }
            } else {
                for (value, gain) in diffused.iter_mut().zip(gains.iter()) {
                    *value = diffuser.process(*value, *gain, glide);
                }
            }
        }

        let gliding = !self.lines.iter().all(|line| line.at_rest(glide));
        let lines = &mut self.lines;
        let target_len: [f32; LINES] = std::array::from_fn(|j| lines[j].target_len);
        let mod_step: [f32; LINES] = std::array::from_fn(|j| lines[j].mod_step);
        let mod_depth: [f32; LINES] = std::array::from_fn(|j| lines[j].mod_depth);
        let feedback_gain: [f32; LINES] = std::array::from_fn(|j| lines[j].feedback);
        let ap_target_len: [f32; LINES] = std::array::from_fn(|j| lines[j].loop_ap.target_len);
        let mut len: [f32; LINES] = std::array::from_fn(|j| lines[j].len);
        let mut phase: [f32; LINES] = std::array::from_fn(|j| lines[j].phase);
        let mut damp: [OnePoleLp; LINES] = std::array::from_fn(|j| lines[j].damp);
        let mut ap_len: [f32; LINES] = std::array::from_fn(|j| lines[j].loop_ap.len);

        for (k, input) in diffused.iter().enumerate() {
            if gliding {
                for j in 0..LINES {
                    len[j] += (target_len[j] - len[j]) * glide;
                }
            }
            // The modulator is a triangle rather than a sine: it costs an
            // absolute value instead of a `sin`, and at these depths and rates
            // the difference is a slightly different distribution of the same
            // small pitch drift.
            let mut delay = [0.0f32; LINES];
            for j in 0..LINES {
                phase[j] += mod_step[j];
                if phase[j] >= 1.0 {
                    phase[j] -= 1.0;
                }
                let triangle = 4.0 * (phase[j] - 0.5).abs() - 1.0;
                delay[j] = len[j] + triangle * mod_depth[j];
            }

            // Read the network, tap the output, then close the loop. Tapping
            // the delayed line contents means the output carries the tail as
            // it currently stands rather than one bounce ahead.
            let mut taps = [0.0f32; LINES];
            for j in 0..LINES {
                taps[j] = lines[j].ring.read(delay[j]);
            }

            let mut wet_l = 0.0f32;
            let mut wet_r = 0.0f32;
            for j in 0..LINES {
                wet_l += taps[j] * TAP_L[j];
                wet_r += taps[j] * TAP_R[j];
            }

            // Damp and attenuate each return, mix through the matrix, then
            // pass the injected result through the line's in-loop allpass
            // before writing it back. The allpass is what makes the tail
            // dense instead of a bare eight-mode ring.
            let mut feedback = taps;
            for j in 0..LINES {
                feedback[j] = damp[j].next_sample(feedback[j]) * feedback_gain[j];
            }
            hadamard(&mut feedback);
            for j in 0..LINES {
                feedback[j] += input * INJECT[j];
            }
            if gliding {
                for j in 0..LINES {
                    ap_len[j] += (ap_target_len[j] - ap_len[j]) * glide;
                }
            }
            let mut delayed = [0.0f32; LINES];
            for j in 0..LINES {
                delayed[j] = lines[j].loop_ap.ring.read(ap_len[j]);
            }
            let mut stored = [0.0f32; LINES];
            let mut returned = [0.0f32; LINES];
            for j in 0..LINES {
                stored[j] = feedback[j] + delayed[j] * LOOP_DIFFUSION_GAIN;
                returned[j] = delayed[j] - stored[j] * LOOP_DIFFUSION_GAIN;
            }
            for j in 0..LINES {
                lines[j].loop_ap.ring.write(stored[j]);
                lines[j].ring.write(returned[j]);
            }

            // Mid/side width. The two taps are orthogonal, so at width 0 the
            // side component cancels to a mono centre and at 1 they stay as
            // decorrelated as the network makes them.
            let width = self.width.advance();
            let mid = (wet_l + wet_r) * 0.5;
            let side = (wet_l - wet_r) * 0.5 * width;
            bus.l[start + k] = (mid + side) * OUTPUT_REFERENCE;
            bus.r[start + k] = (mid - side) * OUTPUT_REFERENCE;
        }

        for (j, line) in lines.iter_mut().enumerate() {
            line.len = len[j];
            line.phase = phase[j];
            line.damp = damp[j];
            line.loop_ap.len = ap_len[j];
        }
    }

    /// The sample loop as it was before MOO-254: every stage for one sample,
    /// then the next sample. [`Self::process_chunk`] is pinned against it.
    #[cfg(test)]
    fn process_range_before_moo254(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        for i in start..end {
            let dry = (bus.l[i] + bus.r[i]) * 0.5;

            // Pre-delay. The one-pole highpass sits after it and before the
            // diffusers, so the network never sees subsonic content at all.
            self.predelay.write(dry);
            let delayed = self.predelay.read_before_moo254(self.predelay_samples.advance());
            let input = delayed - self.low_cut.next_sample(delayed);

            let diffusion = self.diffusion.advance();
            let mut diffused = input;
            for diffuser in self.diffusers.iter_mut() {
                diffused = diffuser.process_before_moo254(diffused, diffusion, self.size_glide);
            }

            // Read the network, tap the output, then close the loop. Tapping
            // the delayed line contents means the output carries the tail as
            // it currently stands rather than one bounce ahead.
            let mut taps = [0.0f32; LINES];
            for (tap, line) in taps.iter_mut().zip(self.lines.iter_mut()) {
                *tap = line.read_before_moo254(self.size_glide);
            }

            let mut wet_l = 0.0f32;
            let mut wet_r = 0.0f32;
            for index in 0..LINES {
                wet_l += taps[index] * TAP_L[index];
                wet_r += taps[index] * TAP_R[index];
            }

            // Damp and attenuate each return, mix through the matrix, then
            // pass the injected result through the line's in-loop allpass
            // before writing it back. The allpass is what makes the tail
            // dense instead of a bare eight-mode ring.
            let mut feedback = taps;
            for (value, line) in feedback.iter_mut().zip(self.lines.iter_mut()) {
                *value = line.damp.next_sample(*value) * line.feedback;
            }
            hadamard(&mut feedback);
            let glide = self.size_glide;
            for (index, line) in self.lines.iter_mut().enumerate() {
                let injected = feedback[index] + diffused * INJECT[index];
                let diffused_return = line.loop_ap.process_before_moo254(injected, LOOP_DIFFUSION_GAIN, glide);
                line.ring.write(diffused_return);
            }

            // Mid/side width. The two taps are orthogonal, so at width 0 the
            // side component cancels to a mono centre and at 1 they stay as
            // decorrelated as the network makes them.
            let width = self.width.advance();
            let mid = (wet_l + wet_r) * 0.5;
            let side = (wet_l - wet_r) * 0.5 * width;
            bus.l[i] = (mid + side) * OUTPUT_REFERENCE;
            bus.r[i] = (mid - side) * OUTPUT_REFERENCE;
        }
    }
}

impl RangeProcessor for ReverbEffect {
    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        self.resolve_dirty();
        #[cfg(test)]
        if self.before_moo254 {
            self.process_range_before_moo254(bus, start, end);
            return;
        }
        let mut at = start;
        while at < end {
            let next = end.min(at + CHUNK);
            self.process_chunk(bus, at, next);
            at = next;
        }
    }

    fn apply_param(&mut self, id: u32, value: f32) {
        match id {
            REVERB_PARAM_SIZE => {
                self.params.size = value.clamp(0.0, 1.0);
                // Marks feedback dirty too: `rebuild_feedback`'s RT60
                // formula reads `target_len`, which this changes.
                self.size_dirty = true;
                self.feedback_dirty = true;
            }
            REVERB_PARAM_DECAY_S => {
                self.params.decay_s = value.clamp(0.2, 20.0);
                self.feedback_dirty = true;
            }
            REVERB_PARAM_DAMPING => {
                self.params.damping = value.clamp(0.0, 1.0);
                self.damping_dirty = true;
            }
            REVERB_PARAM_PREDELAY_MS => {
                self.params.predelay_ms = value.clamp(1.0, 200.0);
                self.predelay_samples
                    .set_target(predelay_samples(self.params.predelay_ms, self.sample_rate));
            }
            REVERB_PARAM_DIFFUSION => {
                self.params.diffusion = value.clamp(0.0, 1.0);
                self.diffusion
                    .set_target(self.params.diffusion * DIFFUSION_MAX_GAIN);
            }
            REVERB_PARAM_WIDTH => {
                self.params.width = value.clamp(0.0, 1.0);
                self.width.set_target(self.params.width);
            }
            REVERB_PARAM_MODULATION => {
                self.params.modulation = value.clamp(0.0, 1.0);
                self.modulation_dirty = true;
            }
            REVERB_PARAM_LOW_CUT_HZ => {
                self.params.low_cut_hz = value.clamp(20.0, 500.0);
                self.low_cut
                    .set_cutoff(self.params.low_cut_hz, self.sample_rate);
            }
            _ => {}
        }
    }
}

impl ReverbEffect {
    /// Empty the network: the pre-delay, the input filter, the diffusers and
    /// the lines. The lines' modulation phase is kept; see `Line::clear`.
    fn clear_tail(&mut self) {
        self.predelay.clear();
        self.low_cut.reset();
        for diffuser in &mut self.diffusers {
            diffuser.clear();
        }
        for line in &mut self.lines {
            line.clear();
        }
    }
}

impl AudioNode for ReverbEffect {
    /// A seek invalidates every sample in the tail: it is the sound of a part
    /// of the song the transport has left. Ringing it out over the new
    /// position is the artefact this contract exists for.
    ///
    /// **Not on a program change.** Time is still continuous there -- the
    /// player looked at another pattern -- and flushing a reverb for it would
    /// be a worse artefact than the stranded note-off it came with. **Nor on
    /// a loop fold**: the tail wraps from the end of the loop into its start
    /// (MOO-59). [`Discontinuity::invalidates_tails`] is the rule.
    ///
    /// The lines' modulation phase is deliberately kept; see `Line::clear`.
    fn on_discontinuity(&mut self, kind: Discontinuity) {
        if !kind.invalidates_tails() {
            return;
        }
        self.clear_tail();
    }

    /// `decay_s` is an RT60, and it is an *upper* bound on the real one: the
    /// per-line gain is computed to hit it and then clamped down by
    /// `FEEDBACK_MAX`, and the damping filter inside the loop has gain at most
    /// one, so both can only make the tail shorter. Falling below
    /// `SILENCE_PEAK` is 140 dB, or 2.34 RT60s; three is the margin, and the
    /// quarter second on top covers the pre-delay ring, whose read head
    /// `predelay_ms` can move by up to 200 ms.
    fn tail_frames(&self) -> u32 {
        // A knob still travelling is a reason to keep running, whatever the
        // decay says: freezing a lag halfway would leave it there, and the
        // device would arrive at the next note somewhere its own parameters
        // do not describe. All three settle in tens of milliseconds against
        // a tail measured in seconds, so this costs nothing in practice and
        // makes the freeze exact rather than nearly so.
        if !self.predelay_samples.is_settled()
            || !self.diffusion.is_settled()
            || !self.width.is_settled()
        {
            return u32::MAX;
        }
        let seconds = 3.0 * self.params.decay_s.clamp(0.2, 20.0) + 0.25;
        let frames = seconds * self.sample_rate.max(1) as f32;
        frames.ceil().min(u32::MAX as f32) as u32
    }

    /// The eight lines' modulation never stops, and neither does the size
    /// glide underneath it. Both are advanced here exactly as the sample loop
    /// would have, which is what keeps a hall sounding the same after a bar
    /// of silence as it does without one — and keeps a bounce at 1024 frames
    /// a block identical to a take at 64.
    fn skip_block(&mut self, ctx: &ProcessContext) {
        let glide = self.size_glide;
        for _ in 0..ctx.frames {
            for diffuser in self.diffusers.iter_mut() {
                diffuser.step_silent(glide);
            }
            for line in self.lines.iter_mut() {
                line.step_silent(glide);
            }
        }
    }

    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        // A sample-rate change invalidates ring capacity, which cannot be
        // reallocated here; re-fit what does not need allocation and leave
        // the buffers alone. Same guard `DelayEffect` and `PlateEffect` use —
        // the engine builds nodes at the client's rate, so it never runs.
        if ctx.sample_rate != self.sample_rate {
            self.sample_rate = ctx.sample_rate.max(1);
            for (index, line) in self.lines.iter_mut().enumerate() {
                line.mod_step = MOD_RATE_HZ[index] / self.sample_rate as f32;
            }
            self.low_cut.set_cutoff(
                self.params.low_cut_hz.clamp(20.0, 500.0),
                self.sample_rate,
            );
            self.do_resize();
            self.do_rebuild_feedback();
            self.do_rebuild_damping();
            self.do_rebuild_modulation();
            #[cfg(test)]
            {
                self.rebuild_counts.size += 1;
                self.rebuild_counts.feedback += 1;
                self.rebuild_counts.damping += 1;
                self.rebuild_counts.modulation += 1;
            }
            self.predelay_samples
                .set_time(PREDELAY_SMOOTH_S, self.sample_rate);
            self.predelay_samples
                .set_target(predelay_samples(self.params.predelay_ms, self.sample_rate));
            self.diffusion.set_time(SMOOTH_S, self.sample_rate);
            self.width.set_time(SMOOTH_S, self.sample_rate);
            self.size_glide = glide_coeff(SIZE_GLIDE_S, self.sample_rate);
        }
        let frames = ctx.frames.min(bus.capacity());
        process_param_split(self, bus, events_in, frames);
        // A NaN or an infinity that got into the network would stay there for
        // good (MOO-176). It comes out as silence, and the tail starts clean.
        if super::scrub_non_finite(bus, frames) {
            self.clear_tail();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Event;

    fn context(frames: usize) -> ProcessContext {
        ProcessContext {
            sample_rate: 48_000,
            frames,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        }
    }

    /// Every dependency starts dirty at construction, so `new`'s own
    /// `resolve_dirty` should have run each rebuild exactly once.
    #[test]
    fn construction_resolves_every_dependency_exactly_once() {
        let effect = ReverbEffect::new(ReverbParams::default(), 48_000);
        assert_eq!(
            effect.rebuild_counts,
            ReverbRebuildCounts { size: 1, feedback: 1, damping: 1, modulation: 1 }
        );
    }

    /// `apply_param` alone must only mark a flag; `resolve_dirty` is what
    /// actually rebuilds, and only `process_range` calls that.
    #[test]
    fn apply_param_alone_defers_the_rebuild() {
        let mut effect = ReverbEffect::new(ReverbParams::default(), 48_000);
        effect.apply_param(REVERB_PARAM_MODULATION, 0.5);
        assert_eq!(
            effect.rebuild_counts,
            ReverbRebuildCounts { size: 1, feedback: 1, damping: 1, modulation: 1 }
        );
    }

    /// Changing `damping` alone must not rebuild `size`, `feedback` or
    /// `modulation`.
    #[test]
    fn changing_damping_alone_does_not_rebuild_the_others() {
        let mut effect = ReverbEffect::new(ReverbParams::default(), 48_000);
        effect.apply_param(REVERB_PARAM_DAMPING, 0.8);
        effect.resolve_dirty();
        assert_eq!(
            effect.rebuild_counts,
            ReverbRebuildCounts { size: 1, feedback: 1, damping: 2, modulation: 1 }
        );
    }

    /// Changing `modulation` alone must not rebuild `size`, `feedback` or
    /// `damping`.
    #[test]
    fn changing_modulation_alone_does_not_rebuild_the_others() {
        let mut effect = ReverbEffect::new(ReverbParams::default(), 48_000);
        effect.apply_param(REVERB_PARAM_MODULATION, 0.6);
        effect.resolve_dirty();
        assert_eq!(
            effect.rebuild_counts,
            ReverbRebuildCounts { size: 1, feedback: 1, damping: 1, modulation: 2 }
        );
    }

    /// The perf payoff: `size` and `decay_s` changing within one control
    /// tick resolve into exactly one feedback rebuild, not the two the old
    /// eager scheme paid (`resize` calling `rebuild_feedback` immediately,
    /// then the `decay_s` event calling it again in the same tick).
    #[test]
    fn size_and_decay_changing_at_one_tick_rebuild_feedback_only_once() {
        let mut effect = ReverbEffect::new(ReverbParams::default(), 48_000);
        effect.apply_param(REVERB_PARAM_SIZE, 0.7);
        effect.apply_param(REVERB_PARAM_DECAY_S, 6.0);
        effect.resolve_dirty();
        assert_eq!(
            effect.rebuild_counts,
            ReverbRebuildCounts { size: 2, feedback: 2, damping: 1, modulation: 1 },
            "size and decay changing at the same tick should cost one \
             feedback rebuild, not two"
        );
    }

    /// `predelay_ms`, `diffusion`, `width` and `low_cut_hz` are direct
    /// smoother/filter writes with no per-line bank loop behind them; none
    /// should mark anything dirty.
    #[test]
    fn smoother_only_params_never_touch_the_dirty_flagged_dependencies() {
        let mut effect = ReverbEffect::new(ReverbParams::default(), 48_000);
        effect.apply_param(REVERB_PARAM_PREDELAY_MS, 40.0);
        effect.apply_param(REVERB_PARAM_DIFFUSION, 0.3);
        effect.apply_param(REVERB_PARAM_WIDTH, 0.9);
        effect.apply_param(REVERB_PARAM_LOW_CUT_HZ, 200.0);
        effect.resolve_dirty();
        assert_eq!(
            effect.rebuild_counts,
            ReverbRebuildCounts { size: 1, feedback: 1, damping: 1, modulation: 1 }
        );
    }

    fn impulse_response(params: ReverbParams, frames: usize) -> StereoBus {
        let mut effect = ReverbEffect::new(params, 48_000);
        let mut bus = StereoBus::with_capacity(frames);
        bus.l[0] = 1.0;
        bus.r[0] = 1.0;
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);
        bus
    }

    /// Both channels' energy over `range`, by the kit's measure.
    fn energy(bus: &StereoBus, range: std::ops::Range<usize>) -> f32 {
        crate::testkit::energy(&bus.l[range.clone()]) + crate::testkit::energy(&bus.r[range])
    }

    #[test]
    fn hadamard_is_orthonormal() {
        // The decay times depend on this: if the matrix changed the norm,
        // the per-line feedback gains would no longer set RT60 on their own.
        let mut values = [0.3, -1.2, 0.7, 0.1, -0.4, 0.9, -0.6, 0.2];
        let before: f32 = values.iter().map(|v| v * v).sum();
        hadamard(&mut values);
        let after: f32 = values.iter().map(|v| v * v).sum();
        assert!(
            (before - after).abs() < 1e-4,
            "norm moved from {before} to {after}"
        );
    }

    #[test]
    fn impulse_produces_a_tail_that_outlasts_the_input() {
        let bus = impulse_response(ReverbParams::default(), 40_000);
        assert!(
            energy(&bus, 20_000..30_000) > 1e-8,
            "the tail should still be audible well after the impulse"
        );
    }

    #[test]
    fn decay_tail_shrinks_over_time() {
        let bus = impulse_response(ReverbParams::default(), 96_000);
        let early = energy(&bus, 5_000..10_000);
        let late = energy(&bus, 85_000..90_000);
        assert!(early > late, "early {early} should exceed late {late}");
        assert!(late > 1e-11, "the tail should not have gone fully silent");
    }

    #[test]
    fn longer_decay_retains_more_energy_at_a_fixed_offset() {
        let short = impulse_response(
            ReverbParams {
                decay_s: 0.5,
                ..ReverbParams::default()
            },
            96_000,
        );
        let long = impulse_response(
            ReverbParams {
                decay_s: 8.0,
                ..ReverbParams::default()
            },
            96_000,
        );
        let short_energy = energy(&short, 80_000..90_000);
        let long_energy = energy(&long, 80_000..90_000);
        assert!(
            long_energy > short_energy * 100.0,
            "decay_s=8 ({long_energy}) should far outlast decay_s=0.5 ({short_energy})"
        );
    }

    /// The whole reason the loop diffusers were added: the late tail must be a
    /// continuous wash, not a handful of echoes fluttering. A metallic/springy
    /// FDN drops most of its energy between sparse arrivals, so its short-time
    /// envelope swings wildly frame to frame; a diffuse one decays smoothly.
    ///
    /// Measured as the largest jump in short-time RMS between adjacent ~11 ms
    /// frames across a mid-tail window, expressed against the window's mean.
    /// A dense tail sits well under 1.0 (the decay trend across one frame is
    /// tiny); a sparse one spikes far past it.
    #[test]
    fn the_late_tail_is_a_continuous_wash_not_a_flutter() {
        let bus = impulse_response(ReverbParams::default(), 96_000);
        const FRAME: usize = 512;
        let window = 24_000..72_000;
        let mut frame_rms = Vec::new();
        let mut start = window.start;
        while start + FRAME <= window.end {
            let e = energy(&bus, start..start + FRAME);
            frame_rms.push((e / (2 * FRAME) as f32).sqrt());
            start += FRAME;
        }
        let mean = frame_rms.iter().sum::<f32>() / frame_rms.len() as f32;
        assert!(mean > 1e-6, "the tail was silent, so this proves nothing");
        let worst_jump = frame_rms
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            worst_jump < mean * 0.5,
            "adjacent tail frames jumped by {worst_jump} against a mean of {mean} \
             ({:.0}%); the tail is fluttering rather than washing",
            100.0 * worst_jump / mean
        );
    }

    /// Pre-delay is the device's only true silence: nothing reaches the
    /// network until it elapses, so the wet output before it must be zero.
    #[test]
    fn predelay_holds_off_the_onset() {
        let bus = impulse_response(
            ReverbParams {
                predelay_ms: 50.0,
                ..ReverbParams::default()
            },
            48_000,
        );
        let silent = (50.0e-3 * 48_000.0) as usize;
        assert!(
            energy(&bus, 0..silent - 100) < 1e-12,
            "wet output should be silent through the pre-delay"
        );
        assert!(
            energy(&bus, silent..silent + 4_000) > 1e-9,
            "the tail should arrive once the pre-delay elapses"
        );
    }

    /// Damping is a high-frequency control, so it must cost the tail its top
    /// end without gutting its overall level.
    #[test]
    fn damping_darkens_the_tail() {
        let brightness = |damping: f32| {
            let bus = impulse_response(
                ReverbParams {
                    damping,
                    ..ReverbParams::default()
                },
                48_000,
            );
            // One-pole difference as a crude high-frequency estimate: the
            // sample-to-sample delta carries the top of the band.
            let window = 20_000..40_000;
            let mut high = 0.0f32;
            let mut total = 0.0f32;
            for i in window {
                let delta = bus.l[i] - bus.l[i - 1];
                high += delta * delta;
                total += bus.l[i] * bus.l[i];
            }
            high / total.max(1e-20)
        };
        let open = brightness(0.0);
        let dark = brightness(1.0);
        assert!(
            dark < open * 0.5,
            "damping 1.0 ({dark}) should be far darker than 0.0 ({open})"
        );
    }

    /// Low cut filters what enters the network, so raising it must remove
    /// low-frequency energy from the tail without touching the top.
    #[test]
    fn low_cut_removes_bass_from_the_tail() {
        let band_energy = |low_cut_hz: f32| {
            let bus = impulse_response(
                ReverbParams {
                    low_cut_hz,
                    ..ReverbParams::default()
                },
                48_000,
            );
            // Running sum over 64 samples is a crude lowpass; what survives
            // it is the bottom of the band.
            let window = 10_000..40_000;
            let mut low = 0.0f64;
            let mut total = 0.0f64;
            let mut accumulator = 0.0f32;
            for i in window {
                accumulator += bus.l[i] - bus.l[i - 64];
                low += (accumulator as f64) * (accumulator as f64);
                total += (bus.l[i] as f64) * (bus.l[i] as f64);
            }
            (low, total)
        };
        let (open_low, open_total) = band_energy(20.0);
        let (cut_low, cut_total) = band_energy(500.0);
        assert!(
            cut_low / open_low < 0.5,
            "a 500 Hz low cut should strip the tail's bass: {cut_low} vs {open_low}"
        );
        assert!(
            cut_total > open_total * 0.2,
            "it should not gut the whole tail: {cut_total} vs {open_total}"
        );
    }

    #[test]
    fn width_zero_collapses_the_tail_to_mono() {
        let bus = impulse_response(
            ReverbParams {
                width: 0.0,
                ..ReverbParams::default()
            },
            24_000,
        );
        for i in 0..24_000 {
            assert!(
                (bus.l[i] - bus.r[i]).abs() < 1e-6,
                "frame {i}: width 0 should leave the channels identical"
            );
        }
    }

    #[test]
    fn full_width_decorrelates_the_channels() {
        let bus = impulse_response(ReverbParams::default(), 48_000);
        let window = 10_000..40_000;
        let mut dot = 0.0f64;
        let mut left = 0.0f64;
        let mut right = 0.0f64;
        for i in window {
            let (l, r) = (bus.l[i] as f64, bus.r[i] as f64);
            dot += l * r;
            left += l * l;
            right += r * r;
        }
        let correlation = dot / (left * right).sqrt().max(1e-20);
        assert!(
            correlation.abs() < 0.4,
            "the two output taps should be largely decorrelated, got {correlation}"
        );
    }

    /// The whole point of the rewrite: parameters arrive as events and take
    /// effect, rather than needing a node swap. A mid-block decay change must
    /// be audible in the same render.
    #[test]
    fn a_param_event_changes_the_tail_within_the_block() {
        let render = |events: &EventList| {
            let mut effect = ReverbEffect::new(ReverbParams::default(), 48_000);
            let mut bus = StereoBus::with_capacity(48_000);
            bus.l[0] = 1.0;
            bus.r[0] = 1.0;
            effect.process(&context(48_000), &mut bus, events, None);
            energy(&bus, 40_000..48_000)
        };
        let mut shorten = EventList::empty();
        shorten.push(crate::event::TimedEvent {
            offset: 1_000,
            event: Event::ParamValue {
                id: REVERB_PARAM_DECAY_S,
                value: 0.3,
            },
        });
        let unchanged = render(&EventList::empty());
        let shortened = render(&shorten);
        assert!(
            shortened < unchanged * 0.5,
            "a decay event should shorten the tail in the same block: \
             {shortened} vs {unchanged}"
        );
    }

    /// The device has no lookahead and no partition window, so it must add
    /// no latency for the host to align against.
    #[test]
    fn the_network_reports_no_latency() {
        let effect = ReverbEffect::new(ReverbParams::default(), 48_000);
        assert_eq!(effect.latency_frames(), 0);
        assert_eq!(effect.dry_path_latency_frames(), 0);
    }

    #[test]
    fn output_stays_bounded_for_extreme_params() {
        let params = ReverbParams {
            decay_s: 20.0,
            size: 1.0,
            damping: 0.0,
            diffusion: 1.0,
            modulation: 1.0,
            ..ReverbParams::default()
        };
        let mut effect = ReverbEffect::new(params, 48_000);
        let frames = 48_000 * 4;
        let mut bus = StereoBus::with_capacity(frames);
        let mut seed = 0x1234_5678u32;
        for i in 0..frames {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = ((seed >> 8) as f32 / 8_388_608.0) - 1.0;
            bus.l[i] = noise;
            bus.r[i] = -noise;
        }
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);
        for sample in bus.l[..frames].iter().chain(bus.r[..frames].iter()) {
            assert!(sample.is_finite(), "output must never be NaN/Inf");
            assert!(sample.abs() < 20.0, "output should stay bounded: {sample}");
        }
    }

    /// A size sweep is a legal modulation destination, so it has to glide
    /// rather than step.
    ///
    /// Driven by a steady low sine, deliberately: the tail of an 80 Hz tone
    /// carries almost no high-frequency content, so consecutive output
    /// samples differ by well under a percent of the peak and any read-head
    /// discontinuity stands out against that. A noise excitation cannot show
    /// this — its own sample-to-sample deltas are the same size as the click
    /// being looked for.
    #[test]
    fn sweeping_size_does_not_click() {
        const BLOCK: usize = 256;
        const TONE_HZ: f32 = 80.0;
        let mut effect = ReverbEffect::new(ReverbParams::default(), 48_000);
        let mut phase = 0.0f32;
        let mut tone = |bus: &mut StereoBus| {
            for i in 0..BLOCK {
                phase += TONE_HZ / 48_000.0;
                if phase >= 1.0 {
                    phase -= 1.0;
                }
                let value = (phase * core::f32::consts::TAU).sin();
                bus.l[i] = value;
                bus.r[i] = value;
            }
        };

        // Let the network fill and settle before anything is swept.
        for _ in 0..64 {
            let mut bus = StereoBus::with_capacity(BLOCK);
            tone(&mut bus);
            effect.process(&context(BLOCK), &mut bus, &EventList::empty(), None);
        }

        let mut worst_step = 0.0f32;
        let mut peak = 0.0f32;
        let mut previous: Option<f32> = None;
        for step in 0..64 {
            let mut events = EventList::empty();
            events.push(crate::event::TimedEvent {
                offset: 0,
                event: Event::ParamValue {
                    id: REVERB_PARAM_SIZE,
                    value: step as f32 / 63.0,
                },
            });
            let mut bus = StereoBus::with_capacity(BLOCK);
            tone(&mut bus);
            effect.process(&context(BLOCK), &mut bus, &events, None);
            for i in 0..BLOCK {
                if let Some(previous) = previous {
                    worst_step = worst_step.max((bus.l[i] - previous).abs());
                }
                peak = peak.max(bus.l[i].abs());
                previous = Some(bus.l[i]);
            }
        }
        assert!(peak > 1e-4, "the tone should have excited the network");
        assert!(
            worst_step < peak * 0.1,
            "a size sweep stepped the output by {worst_step}, {:.1}% of the \
             {peak} peak; a glide should stay near the {:.2}% a smooth \
             {TONE_HZ} Hz tail moves per sample",
            100.0 * worst_step / peak,
            100.0 * core::f32::consts::TAU * TONE_HZ / 48_000.0,
        );
    }

    // --- MOO-254: the input stage a stage at a time, the lines side by side -

    /// The reverb settings of Adam's songs the issue names, and two more,
    /// rounded: `housey-dropout-factory`'s bus, `ok-then`'s two, `deep`'s and
    /// `sad_house`'s (the smallest size in any of them).
    fn song_reverbs() -> Vec<(&'static str, ReverbParams)> {
        let params = |size, decay_s, damping, predelay_ms, diffusion, modulation, low_cut_hz| {
            ReverbParams {
                size,
                decay_s,
                damping,
                predelay_ms,
                diffusion,
                width: 1.0,
                modulation,
                low_cut_hz,
            }
        };
        vec![
            ("housey", params(0.7996, 2.3905, 0.38, 12.0, 0.72, 0.3, 116.22)),
            ("ok-then a", params(0.7958, 0.3163, 0.0, 1.0, 1.0, 0.4482, 230.94)),
            ("ok-then b", params(0.5645, 3.5524, 0.38, 12.0, 0.72, 0.0504, 42.0)),
            ("deep", params(0.8455, 1.0337, 0.5974, 4.0859, 0.72, 0.3646, 42.0)),
            ("sad_house", params(0.059, 2.7889, 1.0, 1.104, 1.0, 0.2727, 42.0)),
        ]
    }

    fn context_at(sample_rate: u32, frames: usize) -> ProcessContext {
        ProcessContext {
            sample_rate,
            ..context(frames)
        }
    }

    /// A deterministic noise sample in -1..1.
    fn noise(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((*seed >> 8) as f32 / 8_388_608.0) - 1.0
    }

    /// Knobs turned under the render, as (frame, id, value): a size jump and
    /// a size sweep (the glides), and every other control once, some
    /// mid-block.
    fn reverb_moves() -> Vec<(usize, u32, f32)> {
        let mut moves = vec![
            (9_001, REVERB_PARAM_SIZE, 1.0),
            (20_000, REVERB_PARAM_PREDELAY_MS, 80.0),
            (21_111, REVERB_PARAM_DIFFUSION, 0.1),
            (23_000, REVERB_PARAM_WIDTH, 0.4),
            (25_555, REVERB_PARAM_DAMPING, 0.9),
            (27_000, REVERB_PARAM_DECAY_S, 6.0),
            (29_000, REVERB_PARAM_MODULATION, 1.0),
            (31_000, REVERB_PARAM_LOW_CUT_HZ, 300.0),
        ];
        for step in 0..40 {
            moves.push((40_000 + step * 97, REVERB_PARAM_SIZE, step as f32 / 39.0));
        }
        moves
    }

    /// Render `frames` of an impulse followed by bursts of noise, in blocks
    /// of `block`, stereo interleaved into one vector. `moves` are applied
    /// as they come. A stretch is skipped the way the host sleeps a device,
    /// and a seek lands later, so the two paths meet those too.
    fn render_reverb(
        params: ReverbParams,
        sample_rate: u32,
        block: usize,
        frames: usize,
        moves: &[(usize, u32, f32)],
        before: bool,
    ) -> Vec<f32> {
        let mut effect = ReverbEffect::new(params, sample_rate);
        effect.before_moo254 = before;
        let mut bus = StereoBus::with_capacity(block);
        let mut out = Vec::with_capacity(frames * 2);
        let mut seed = 0x0bad_5eedu32;
        let mut rendered = 0;
        let skip = 60_000..64_000;
        let seek_at = 70_000;
        while rendered < frames {
            let len = block.min(frames - rendered);
            if skip.contains(&rendered) {
                effect.skip_block(&context_at(sample_rate, len));
                out.extend(std::iter::repeat_n(0.0, len * 2));
                rendered += len;
                continue;
            }
            if (rendered..rendered + len).contains(&seek_at) {
                effect.on_discontinuity(Discontinuity::Seek);
            }
            let mut events = EventList::empty();
            for &(at, id, value) in moves {
                if rendered <= at && at < rendered + len {
                    events.push(crate::event::TimedEvent {
                        offset: (at - rendered) as u32,
                        event: Event::ParamValue { id, value },
                    });
                }
            }
            for i in 0..len {
                let frame = rendered + i;
                let loud = frame == 0 || (frame / 3_000) % 4 == 1;
                let value = if frame == 0 {
                    1.0
                } else if loud {
                    noise(&mut seed)
                } else {
                    0.0
                };
                bus.l[i] = value;
                bus.r[i] = -0.5 * value;
            }
            effect.process(&context_at(sample_rate, len), &mut bus, &events, None);
            for i in 0..len {
                out.push(bus.l[i]);
                out.push(bus.r[i]);
            }
            rendered += len;
        }
        out
    }

    fn assert_same_bits(name: &str, before: &[f32], now: &[f32]) {
        assert!(
            before.iter().any(|sample| *sample != 0.0),
            "{name}: rendered silence, which proves nothing"
        );
        assert_eq!(before.len(), now.len(), "{name}: the renders differ in length");
        if let Some(at) = before.iter().zip(now).position(|(a, b)| a.to_bits() != b.to_bits()) {
            let worst = before
                .iter()
                .zip(now)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            panic!(
                "{name}: MOO-254 changed a sample, first at frame {} ({} against {}), \
                 by at most {worst:e}",
                at / 2,
                before[at],
                now[at]
            );
        }
    }

    /// The restructured network is the old one to the bit: each song's
    /// reverb and the extremes, with and without knobs turned under it
    /// (sizes gliding and at rest), at four sample rates, in blocks shorter
    /// and longer than a chunk, across a skipped stretch and a seek.
    #[test]
    fn the_restructured_network_is_the_old_one_bit_for_bit() {
        let frames = 96_000;
        let mut cases = song_reverbs();
        cases.push(("default", ReverbParams::default()));
        cases.push((
            "smallest",
            ReverbParams {
                size: 0.0,
                modulation: 1.0,
                diffusion: 0.0,
                ..ReverbParams::default()
            },
        ));
        cases.push((
            "largest",
            ReverbParams {
                size: 1.0,
                modulation: 1.0,
                decay_s: 20.0,
                ..ReverbParams::default()
            },
        ));
        for (name, params) in &cases {
            for moves in [Vec::new(), reverb_moves()] {
                let moved = if moves.is_empty() { "" } else { ", knobs turned" };
                let name = format!("{name}{moved}");
                let before = render_reverb(*params, 48_000, 128, frames, &moves, true);
                let now = render_reverb(*params, 48_000, 128, frames, &moves, false);
                assert_same_bits(&name, &before, &now);
            }
        }
        let moves = reverb_moves();
        for (sample_rate, block) in [(44_100, 64), (22_050, 1_000), (96_000, 333), (48_000, 4_096)] {
            let name = format!("default at {sample_rate} Hz in {block}-frame blocks, knobs turned");
            let params = ReverbParams::default();
            let before = render_reverb(params, sample_rate, block, frames, &moves, true);
            let now = render_reverb(params, sample_rate, block, frames, &moves, false);
            assert_same_bits(&name, &before, &now);
        }
    }

    /// The skip the restructure leans on is real: a second after a size
    /// change every glide has stalled, so the steady state skips them all,
    /// and a size change sets them moving again.
    #[test]
    fn every_glide_stalls_after_a_size_change_and_a_new_one_restarts_them() {
        let at_rest = |effect: &ReverbEffect| {
            let glide = effect.size_glide;
            effect.lines.iter().all(|line| line.at_rest(glide))
                && effect.diffusers.iter().all(|diffuser| diffuser.at_rest(glide))
        };
        let mut effect = ReverbEffect::new(ReverbParams::default(), 48_000);
        let mut bus = StereoBus::with_capacity(48_000);
        effect.process(&context(48_000), &mut bus, &EventList::empty(), None);
        assert!(at_rest(&effect), "a second in, the glides should have stalled");
        let mut events = EventList::empty();
        events.push(crate::event::TimedEvent {
            offset: 0,
            event: Event::ParamValue {
                id: REVERB_PARAM_SIZE,
                value: 0.9,
            },
        });
        effect.process(&context(128), &mut bus, &events, None);
        assert!(!at_rest(&effect), "a size change should set the glides moving");
        effect.process(&context(48_000), &mut bus, &EventList::empty(), None);
        assert!(at_rest(&effect), "and a second later they should stall again");
    }

    /// **What MOO-254 saves**, in microseconds a 128-frame block at 48 kHz,
    /// before and after interleaved in one run so a shared machine's noise
    /// lands on both: every pass renders every (setting, path) pair once,
    /// and each block's cost is its fastest over the passes. Noise in
    /// bursts, as a send carries it. "size swept" moves `size` every block,
    /// so its glides never stall: the case the skip does not help.
    ///
    /// ```sh
    /// REPS=15 cargo test -p mooloop-dsp --release --lib reverb_path_cost -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "measures wall time; run deliberately in release"]
    fn reverb_path_cost() {
        use std::time::Instant;
        let reps = std::env::var("REPS")
            .ok()
            .and_then(|reps| reps.parse().ok())
            .unwrap_or(7usize)
            .max(1);
        let block = 128;
        let blocks = 2 * 48_000 / block;
        let mut cases: Vec<(&str, ReverbParams, bool)> = song_reverbs()
            .into_iter()
            .map(|(name, params)| (name, params, false))
            .collect();
        cases.push(("default", ReverbParams::default(), false));
        cases.push(("default, size swept", ReverbParams::default(), true));
        let paths = [true, false];
        let mut fastest = vec![vec![vec![f64::MAX; blocks]; paths.len()]; cases.len()];
        let mut bus = StereoBus::with_capacity(block);
        for _ in 0..reps {
            for (c, (_, params, swept)) in cases.iter().enumerate() {
                for (p, before) in paths.iter().enumerate() {
                    let mut effect = ReverbEffect::new(*params, 48_000);
                    effect.before_moo254 = *before;
                    let mut seed = 0x0bad_5eedu32;
                    for (index, slot) in fastest[c][p].iter_mut().enumerate() {
                        let mut events = EventList::empty();
                        if *swept {
                            let size = 0.3 + 0.4 * ((index % 64) as f32 / 63.0);
                            events.push(crate::event::TimedEvent {
                                offset: 0,
                                event: Event::ParamValue {
                                    id: REVERB_PARAM_SIZE,
                                    value: size,
                                },
                            });
                        }
                        let loud = (index / 24) % 4 == 1;
                        for i in 0..block {
                            let value = if loud { noise(&mut seed) } else { 0.0 };
                            bus.l[i] = value;
                            bus.r[i] = value;
                        }
                        let start = Instant::now();
                        effect.process(&context(block), &mut bus, &events, None);
                        let spent = start.elapsed().as_secs_f64() * 1.0e6;
                        std::hint::black_box(&bus);
                        *slot = slot.min(spent);
                    }
                }
            }
        }
        println!("us per 128-frame block at 48 kHz, fastest of {reps} passes, over {blocks} blocks");
        println!(
            "{:<22} {:>9} {:>9} {:>7} {:>9} {:>9}",
            "reverb", "before", "now", "saved", "p99 bef", "p99 now"
        );
        for (c, (name, _, _)) in cases.iter().enumerate() {
            let mean = |p: usize| fastest[c][p].iter().sum::<f64>() / blocks as f64;
            let p99 = |p: usize| {
                let mut sorted = fastest[c][p].clone();
                sorted.sort_by(f64::total_cmp);
                sorted[blocks * 99 / 100]
            };
            let (before, now) = (mean(0), mean(1));
            println!(
                "{name:<22} {before:>9.2} {now:>9.2} {:>6.0}% {:>9.2} {:>9.2}",
                100.0 * (before - now) / before,
                p99(0),
                p99(1),
            );
        }
    }
}
