//! Lightweight comb/allpass ("plate") reverb.
//!
//! A classic Schroeder/Freeverb-style feedback network: 8 parallel comb
//! filters feeding 4 series allpass filters, per channel. Unlike
//! [`super::reverb::ReverbEffect`]'s partitioned FFT convolution, there is no
//! impulse response and no per-block transform — CPU cost is a small fixed
//! number of buffer taps per sample, completely independent of `decay_s`.
//! That's the whole point of this effect: a much cheaper alternative for
//! material that doesn't need the convolution reverb's generated-room
//! precision.
//!
//! Mono-summed input feeds two independent networks (one per output
//! channel) whose tap lengths differ by a fixed offset, so the two channels
//! decorrelate — that difference *is* the stereo image, mixed to taste by
//! `width`.

use mooloop_core::{
    PlateParams, PLATE_PARAM_DAMPING, PLATE_PARAM_DECAY_S, PLATE_PARAM_SIZE, PLATE_PARAM_WIDTH,
};

use crate::bus::StereoBus;
use crate::event::{Event, EventList};
use crate::filter::OnePoleLp;
use crate::node::{AudioNode, ProcessContext};
use crate::smooth::Smoothed;

const NUM_COMBS: usize = 8;
const NUM_ALLPASS: usize = 4;

/// Classic Freeverb tuning lengths in samples, at a 44100 Hz reference. The
/// right channel's network uses each of these plus [`STEREO_SPREAD`].
const COMB_TUNING_L: [usize; NUM_COMBS] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNING_L: [usize; NUM_ALLPASS] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;
const TUNING_SAMPLE_RATE: f32 = 44_100.0;

/// `size` (0..1) maps onto this tap-length multiplier range. Default `size`
/// (0.5) lands exactly on 1.0x — the canonical Freeverb tuning.
const SIZE_MIN_MULTIPLIER: f32 = 0.5;
const SIZE_MAX_MULTIPLIER: f32 = 1.5;

/// Schroeder's standard allpass feedback gain.
const ALLPASS_GAIN: f32 = 0.5;

/// Comb feedback is derived from a per-tap RT60 formula; clamped short of 1
/// so a pathological `decay_s`/size combination can never make a comb ring
/// forever.
const FEEDBACK_MAX: f32 = 0.97;

/// Damping sweep range: `damping = 1.0` is dark (heavy high-frequency loss
/// per bounce), `damping = 0.0` is essentially transparent.
const DAMP_MIN_HZ: f32 = 1_000.0;
const DAMP_MAX_HZ: f32 = 18_000.0;

/// Width is the only thing smoothed sample-by-sample (it directly scales
/// amplitude, so a block-boundary step would click); `size`/`decay_s`/
/// `damping` changes apply immediately, same as the existing reverb not
/// smoothing shape/decay changes either.
const WIDTH_SMOOTH_S: f32 = 0.02;

/// Input is mono-summed before the network (matches the convolution
/// reverb's convention); this scales it back down before the 8-way parallel
/// comb sum so the combined output isn't inherently 8x hot.
const INPUT_GAIN: f32 = 1.0 / NUM_COMBS as f32;

fn size_multiplier(size: f32) -> f32 {
    SIZE_MIN_MULTIPLIER + size.clamp(0.0, 1.0) * (SIZE_MAX_MULTIPLIER - SIZE_MIN_MULTIPLIER)
}

/// Samples for a base (44100 Hz reference) tap length at `sample_rate` and
/// tuning `multiplier`.
fn scale_len(base: usize, sample_rate: u32, multiplier: f32) -> usize {
    ((base as f32 * multiplier * sample_rate as f32 / TUNING_SAMPLE_RATE).round() as usize).max(1)
}

/// One feedback comb: a delay tap with damped feedback. `buffer` is sized
/// once for the largest `size` can ask for; `len` is the currently active
/// span within it, so `size` changes never reallocate on the audio thread.
struct Comb {
    base_len: usize,
    buffer: Vec<f32>,
    len: usize,
    index: usize,
    feedback: f32,
    damp: OnePoleLp,
}

impl Comb {
    fn new(base_len: usize, sample_rate: u32) -> Self {
        let capacity = scale_len(base_len, sample_rate, SIZE_MAX_MULTIPLIER);
        Self {
            base_len,
            buffer: vec![0.0; capacity],
            len: capacity,
            index: 0,
            feedback: 0.0,
            damp: OnePoleLp::new(),
        }
    }

    /// Changing the effective length resets the tap: there is no clean way
    /// to resize a ring's active span mid-stream without either a click or
    /// an interpolated crossfade, and a room-size change is a rare, coarse
    /// gesture — the existing convolution reverb produces the same kind of
    /// hard discontinuity on any shape change, via a full async IR swap.
    fn set_size(&mut self, sample_rate: u32, size: f32) {
        let len = scale_len(self.base_len, sample_rate, size_multiplier(size)).min(self.buffer.len());
        if len != self.len {
            self.buffer.fill(0.0);
            self.index = 0;
            self.len = len;
        }
    }

    fn set_decay(&mut self, sample_rate: u32, decay_s: f32) {
        let seconds = self.len as f32 / sample_rate.max(1) as f32;
        self.feedback = 10f32
            .powf(-3.0 * seconds / decay_s.max(0.01))
            .clamp(0.0, FEEDBACK_MAX);
    }

    fn set_damping(&mut self, sample_rate: u32, damping: f32) {
        let t = damping.clamp(0.0, 1.0);
        let hz = DAMP_MAX_HZ * (DAMP_MIN_HZ / DAMP_MAX_HZ).powf(t);
        self.damp.set_cutoff(hz, sample_rate);
    }

    fn process(&mut self, input: f32) -> f32 {
        let output = self.buffer[self.index];
        let damped = self.damp.next_sample(output);
        self.buffer[self.index] = input + damped * self.feedback;
        self.index += 1;
        if self.index >= self.len {
            self.index = 0;
        }
        output
    }
}

/// A Schroeder allpass diffuser: fixed unity-magnitude gain, so it reshapes
/// the echo density without changing overall level. Distinct from
/// `crate::filter::AllPass`, which is a single-sample phase-shift stage, not
/// a delay-line diffuser.
struct SchroederAllpass {
    base_len: usize,
    buffer: Vec<f32>,
    len: usize,
    index: usize,
}

impl SchroederAllpass {
    fn new(base_len: usize, sample_rate: u32) -> Self {
        let capacity = scale_len(base_len, sample_rate, SIZE_MAX_MULTIPLIER);
        Self {
            base_len,
            buffer: vec![0.0; capacity],
            len: capacity,
            index: 0,
        }
    }

    fn set_size(&mut self, sample_rate: u32, size: f32) {
        let len = scale_len(self.base_len, sample_rate, size_multiplier(size)).min(self.buffer.len());
        if len != self.len {
            self.buffer.fill(0.0);
            self.index = 0;
            self.len = len;
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let buffered = self.buffer[self.index];
        let output = -input + buffered;
        self.buffer[self.index] = input + buffered * ALLPASS_GAIN;
        self.index += 1;
        if self.index >= self.len {
            self.index = 0;
        }
        output
    }
}

pub struct PlateEffect {
    params: PlateParams,
    sample_rate: u32,
    combs_l: [Comb; NUM_COMBS],
    combs_r: [Comb; NUM_COMBS],
    allpass_l: [SchroederAllpass; NUM_ALLPASS],
    allpass_r: [SchroederAllpass; NUM_ALLPASS],
    wet1: Smoothed,
    wet2: Smoothed,
}

impl PlateEffect {
    pub fn new(params: PlateParams, sample_rate: u32) -> Self {
        let combs_l = std::array::from_fn(|i| Comb::new(COMB_TUNING_L[i], sample_rate));
        let combs_r =
            std::array::from_fn(|i| Comb::new(COMB_TUNING_L[i] + STEREO_SPREAD, sample_rate));
        let allpass_l = std::array::from_fn(|i| SchroederAllpass::new(ALLPASS_TUNING_L[i], sample_rate));
        let allpass_r = std::array::from_fn(|i| {
            SchroederAllpass::new(ALLPASS_TUNING_L[i] + STEREO_SPREAD, sample_rate)
        });
        let width = params.width.clamp(0.0, 1.0);
        let mut effect = Self {
            params,
            sample_rate,
            combs_l,
            combs_r,
            allpass_l,
            allpass_r,
            wet1: Smoothed::new(0.5 + width * 0.5, WIDTH_SMOOTH_S, sample_rate),
            wet2: Smoothed::new(0.5 - width * 0.5, WIDTH_SMOOTH_S, sample_rate),
        };
        effect.resize();
        effect.rebuild_damping();
        effect
    }

    fn resize(&mut self) {
        let size = self.params.size;
        let sample_rate = self.sample_rate;
        for comb in self.combs_l.iter_mut().chain(self.combs_r.iter_mut()) {
            comb.set_size(sample_rate, size);
        }
        for allpass in self.allpass_l.iter_mut().chain(self.allpass_r.iter_mut()) {
            allpass.set_size(sample_rate, size);
        }
        self.rebuild_feedback();
    }

    fn rebuild_feedback(&mut self) {
        let decay_s = self.params.decay_s;
        let sample_rate = self.sample_rate;
        for comb in self.combs_l.iter_mut().chain(self.combs_r.iter_mut()) {
            comb.set_decay(sample_rate, decay_s);
        }
    }

    fn rebuild_damping(&mut self) {
        let damping = self.params.damping;
        let sample_rate = self.sample_rate;
        for comb in self.combs_l.iter_mut().chain(self.combs_r.iter_mut()) {
            comb.set_damping(sample_rate, damping);
        }
    }

    fn apply_param(&mut self, id: u32, value: f32) {
        match id {
            PLATE_PARAM_SIZE => {
                self.params.size = value.clamp(0.0, 1.0);
                self.resize();
            }
            PLATE_PARAM_DECAY_S => {
                self.params.decay_s = value.clamp(0.2, 10.0);
                self.rebuild_feedback();
            }
            PLATE_PARAM_DAMPING => {
                self.params.damping = value.clamp(0.0, 1.0);
                self.rebuild_damping();
            }
            PLATE_PARAM_WIDTH => {
                self.params.width = value.clamp(0.0, 1.0);
                self.wet1.set_target(0.5 + self.params.width * 0.5);
                self.wet2.set_target(0.5 - self.params.width * 0.5);
            }
            _ => {}
        }
    }

    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        for i in start..end {
            let input = (bus.l[i] + bus.r[i]) * 0.5 * INPUT_GAIN;

            let mut wet_l = 0.0f32;
            for comb in self.combs_l.iter_mut() {
                wet_l += comb.process(input);
            }
            let mut wet_r = 0.0f32;
            for comb in self.combs_r.iter_mut() {
                wet_r += comb.process(input);
            }

            for allpass in self.allpass_l.iter_mut() {
                wet_l = allpass.process(wet_l);
            }
            for allpass in self.allpass_r.iter_mut() {
                wet_r = allpass.process(wet_r);
            }

            let wet1 = self.wet1.advance();
            let wet2 = self.wet2.advance();
            bus.l[i] = wet_l * wet1 + wet_r * wet2;
            bus.r[i] = wet_r * wet1 + wet_l * wet2;
        }
    }
}

impl AudioNode for PlateEffect {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        // A sample-rate change invalidates the buffers' capacity, which
        // can't be re-allocated on the audio thread; re-fit what doesn't
        // require reallocation and leave tap lengths as-is, same guard
        // `DelayEffect` uses — the engine constructs nodes at the client's
        // rate, so this never actually runs.
        if ctx.sample_rate != self.sample_rate {
            self.sample_rate = ctx.sample_rate;
            self.rebuild_feedback();
            self.rebuild_damping();
            self.wet1.set_time(WIDTH_SMOOTH_S, ctx.sample_rate);
            self.wet2.set_time(WIDTH_SMOOTH_S, ctx.sample_rate);
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

    fn impulse_response(params: PlateParams, frames: usize) -> StereoBus {
        let mut effect = PlateEffect::new(params, 48_000);
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
    fn impulse_produces_a_tail_that_outlasts_the_input() {
        let bus = impulse_response(PlateParams::default(), 20_000);
        assert!(
            energy(&bus, 10_000..15_000) > 1e-8,
            "the tail should still be audible well after the single-sample impulse"
        );
    }

    #[test]
    fn decay_tail_shrinks_over_time() {
        let bus = impulse_response(PlateParams::default(), 45_000);
        let early = energy(&bus, 2_000..5_000);
        let late = energy(&bus, 40_000..43_000);
        assert!(early > late, "early {early} should exceed late {late}");
        assert!(late > 1e-9, "the tail should not have gone fully silent");
    }

    #[test]
    fn higher_decay_param_retains_more_energy_at_a_fixed_offset() {
        let short = impulse_response(
            PlateParams {
                decay_s: 0.5,
                ..PlateParams::default()
            },
            22_000,
        );
        let long = impulse_response(
            PlateParams {
                decay_s: 5.0,
                ..PlateParams::default()
            },
            22_000,
        );
        let short_energy = energy(&short, 20_000..21_000);
        let long_energy = energy(&long, 20_000..21_000);
        assert!(
            long_energy > short_energy,
            "decay_s=5.0 ({long_energy}) should retain more energy than decay_s=0.5 ({short_energy}) at the same late offset"
        );
    }

    #[test]
    fn output_stays_bounded_for_extreme_params() {
        let params = PlateParams {
            decay_s: 10.0,
            damping: 0.0,
            ..PlateParams::default()
        };
        let mut effect = PlateEffect::new(params, 48_000);
        let frames = 48_000 * 3;
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
}
