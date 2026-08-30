//! Feedback-delay-network hall reverb.
//!
//! Eight delay lines feed back through a normalized Hadamard matrix. Input
//! passes a pre-delay and a chain of Schroeder allpass diffusers before it
//! reaches them; each line's return is damped and attenuated to hit a target
//! RT60 before it is mixed back in. Cost is a fixed handful of taps and
//! multiplies per sample and does not vary with `decay_s` at all.
//!
//! ## Why this shape
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
//!   `docs/MODULATION_PLAN.md` ever asked an effect to do.
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
use crate::event::{Event, EventList};
use crate::filter::OnePoleLp;
use crate::node::{AudioNode, ProcessContext};
use crate::smooth::Smoothed;

/// Delay lines in the network. Eight is the point where the echo density of
/// the first 50 ms already reads as a room; sixteen doubles the per-sample
/// cost for a difference the input diffusers largely supply anyway.
const LINES: usize = 8;
const DIFFUSERS: usize = 4;

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

/// `size` (0..1) maps onto this tap-length multiplier range, geometrically,
/// so the default 0.5 lands exactly on the tuning above. The ends are a
/// small ~8 ms-shortest chamber and a ~125 ms-longest cathedral.
const SIZE_MIN_MULTIPLIER: f32 = 0.4;
const SIZE_MAX_MULTIPLIER: f32 = 2.5;

/// Feedback gain ceiling. The Hadamard matrix is orthonormal and the damping
/// filters have gain at most one, so the loop is stable whenever every
/// per-line gain is under one; this leaves margin so that a pathological
/// `decay_s`/`size` pair cannot ring indefinitely.
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

/// Schroeder allpass gain at `diffusion = 1`. Past about 0.75 the stages
/// start to ring audibly rather than smear.
const DIFFUSION_MAX_GAIN: f32 = 0.72;

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
const OUTPUT_REFERENCE: f32 = 0.188;

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
    fn read(&self, delay: f32) -> f32 {
        let capacity = self.buffer.len();
        let delay = delay.clamp(1.0, capacity as f32 - 2.0);
        let base = delay.floor();
        let frac = delay - base;
        let back = base as usize;
        let index = (self.write + capacity - back) % capacity;
        let previous = if index == 0 { capacity - 1 } else { index - 1 };
        self.buffer[index] * (1.0 - frac) + self.buffer[previous] * frac
    }
}

/// A Schroeder allpass diffuser: a delay tap whose feedforward and feedback
/// gains cancel, so it reshapes echo density without colouring the response.
/// Distinct from `crate::filter::AllPass`, which is a single-sample phase
/// stage rather than a delay-line diffuser.
struct Diffuser {
    ring: Ring,
    len: f32,
    target_len: f32,
}

impl Diffuser {
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

    fn process(&mut self, input: f32, gain: f32, glide: f32) -> f32 {
        self.len += (self.target_len - self.len) * glide;
        let delayed = self.ring.read(self.len);
        let stored = input + delayed * gain;
        self.ring.write(stored);
        delayed - stored * gain
    }
}

/// One delay line of the network, with its damping filter, feedback gain,
/// and modulation phase.
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
}

impl Line {
    fn new(base_len: usize, sample_rate: u32, index: usize) -> Self {
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
        }
    }

    /// Advance the modulation oscillator and read the line.
    ///
    /// The modulator is a triangle rather than a sine: it costs an absolute
    /// value instead of a `sin`, and at these depths and rates the difference
    /// is a slightly different distribution of the same small pitch drift.
    fn read(&mut self, glide: f32) -> f32 {
        self.len += (self.target_len - self.len) * glide;
        self.phase += self.mod_step;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        let triangle = 4.0 * (self.phase - 0.5).abs() - 1.0;
        self.ring.read(self.len + triangle * self.mod_depth)
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
            lines: std::array::from_fn(|i| Line::new(LINE_TUNING[i], sample_rate, i)),
            diffusion: Smoothed::new(
                params.diffusion.clamp(0.0, 1.0) * DIFFUSION_MAX_GAIN,
                SMOOTH_S,
                sample_rate,
            ),
            width: Smoothed::new(params.width.clamp(0.0, 1.0), SMOOTH_S, sample_rate),
            size_glide: glide_coeff(SIZE_GLIDE_S, sample_rate),
        };
        effect.resize();
        effect.rebuild_damping();
        effect.rebuild_modulation();
        effect
    }

    /// Retune every delay length for the current `size`, then re-solve the
    /// feedback gains, which depend on the lengths.
    ///
    /// This sets *targets*. Unlike the plate's comb resize, which clears its
    /// buffers and accepts the discontinuity, the lengths here glide, because
    /// `size` is a legal modulation destination and has to survive being
    /// swept: moving a read head straight to a new offset lands it on
    /// uncorrelated history, which is a click, not a room change.
    fn resize(&mut self) {
        let multiplier = size_multiplier(self.params.size);
        let sample_rate = self.sample_rate;
        for (line, base) in self.lines.iter_mut().zip(LINE_TUNING) {
            let ceiling = line.ring.buffer.len() as f32 - mod_depth_samples(sample_rate) - 4.0;
            line.target_len =
                (scaled_len(base, sample_rate, multiplier) as f32).min(ceiling.max(1.0));
        }
        for (diffuser, base) in self.diffusers.iter_mut().zip(DIFFUSER_TUNING) {
            diffuser.set_len(scaled_len(base, sample_rate, multiplier) as f32);
        }
        self.rebuild_feedback();
    }

    /// Solve each line's feedback gain for the target RT60.
    ///
    /// A signal circulating line `i` loses `gain` every `len` samples, so to
    /// fall 60 dB in `decay_s` it needs `gain = 10^(-3 * len / (decay * fs))`.
    /// Solving per line rather than sharing one gain is what makes the lines'
    /// decays line up: the long lines are attenuated less per trip because
    /// they make fewer trips.
    fn rebuild_feedback(&mut self) {
        let decay_s = self.params.decay_s.max(0.05);
        let sample_rate = self.sample_rate as f32;
        for line in self.lines.iter_mut() {
            // Solved against the target rather than the gliding current
            // length, so a size change costs one `powf` per line instead of
            // one per line per sample. Mid-glide the decay is briefly off by
            // the fraction the length still has to travel — a few tens of
            // milliseconds, under a control that is itself moving.
            let seconds = line.target_len / sample_rate;
            line.feedback = 10f32
                .powf(-3.0 * seconds / decay_s)
                .clamp(0.0, FEEDBACK_MAX);
        }
    }

    fn rebuild_damping(&mut self) {
        // `damping = 0` must be exactly transparent, not merely bright: a
        // coefficient of 1.0 makes `OnePoleLp::next_sample` return its input.
        let coeff = 1.0 - self.params.damping.clamp(0.0, 1.0) * DAMP_MAX_LOSS;
        for line in self.lines.iter_mut() {
            line.damp.set_coeff(coeff);
        }
    }

    fn rebuild_modulation(&mut self) {
        let depth = mod_depth_samples(self.sample_rate) * self.params.modulation.clamp(0.0, 1.0);
        for line in self.lines.iter_mut() {
            line.mod_depth = depth;
        }
    }

    fn apply_param(&mut self, id: u32, value: f32) {
        match id {
            REVERB_PARAM_SIZE => {
                self.params.size = value.clamp(0.0, 1.0);
                self.resize();
            }
            REVERB_PARAM_DECAY_S => {
                self.params.decay_s = value.clamp(0.2, 20.0);
                self.rebuild_feedback();
            }
            REVERB_PARAM_DAMPING => {
                self.params.damping = value.clamp(0.0, 1.0);
                self.rebuild_damping();
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
                self.rebuild_modulation();
            }
            REVERB_PARAM_LOW_CUT_HZ => {
                self.params.low_cut_hz = value.clamp(20.0, 500.0);
                self.low_cut
                    .set_cutoff(self.params.low_cut_hz, self.sample_rate);
            }
            _ => {}
        }
    }

    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        for i in start..end {
            let dry = (bus.l[i] + bus.r[i]) * 0.5;

            // Pre-delay. The one-pole highpass sits after it and before the
            // diffusers, so the network never sees subsonic content at all.
            self.predelay.write(dry);
            let delayed = self.predelay.read(self.predelay_samples.advance());
            let input = delayed - self.low_cut.next_sample(delayed);

            let diffusion = self.diffusion.advance();
            let mut diffused = input;
            for diffuser in self.diffusers.iter_mut() {
                diffused = diffuser.process(diffused, diffusion, self.size_glide);
            }

            // Read the network, tap the output, then close the loop. Tapping
            // before damping and attenuation means the output carries the
            // tail as it currently stands rather than one bounce ahead.
            let mut taps = [0.0f32; LINES];
            for (tap, line) in taps.iter_mut().zip(self.lines.iter_mut()) {
                *tap = line.read(self.size_glide);
            }

            let mut wet_l = 0.0f32;
            let mut wet_r = 0.0f32;
            for index in 0..LINES {
                wet_l += taps[index] * TAP_L[index];
                wet_r += taps[index] * TAP_R[index];
            }

            let mut feedback = taps;
            for (value, line) in feedback.iter_mut().zip(self.lines.iter_mut()) {
                *value = line.damp.next_sample(*value) * line.feedback;
            }
            hadamard(&mut feedback);
            for (index, line) in self.lines.iter_mut().enumerate() {
                line.ring
                    .write(feedback[index] + diffused * INJECT[index]);
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

impl AudioNode for ReverbEffect {
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
            self.resize();
            self.rebuild_damping();
            self.rebuild_modulation();
            self.predelay_samples
                .set_time(PREDELAY_SMOOTH_S, self.sample_rate);
            self.predelay_samples
                .set_target(predelay_samples(self.params.predelay_ms, self.sample_rate));
            self.diffusion.set_time(SMOOTH_S, self.sample_rate);
            self.width.set_time(SMOOTH_S, self.sample_rate);
            self.size_glide = glide_coeff(SIZE_GLIDE_S, self.sample_rate);
        }
        let frames = ctx.frames.min(bus.capacity());
        let mut pos = 0usize;
        for ev in events_in.iter() {
            let off = (ev.offset as usize).min(frames).max(pos);
            self.process_range(bus, pos, off);
            if let Event::ParamValue { id, value } = ev.event {
                self.apply_param(id, value);
            }
            pos = off;
        }
        self.process_range(bus, pos, frames);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn impulse_response(params: ReverbParams, frames: usize) -> StereoBus {
        let mut effect = ReverbEffect::new(params, 48_000);
        let mut bus = StereoBus::with_capacity(frames);
        bus.l[0] = 1.0;
        bus.r[0] = 1.0;
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);
        bus
    }

    fn energy(bus: &StereoBus, range: std::ops::Range<usize>) -> f32 {
        bus.l[range.clone()]
            .iter()
            .chain(bus.r[range].iter())
            .map(|sample| sample * sample)
            .sum()
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
}
