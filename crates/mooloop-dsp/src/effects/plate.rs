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
    PlateParams, PLATE_PARAM_DAMPING, PLATE_PARAM_DECAY_S, PLATE_PARAM_PREDELAY_MS,
    PLATE_PARAM_SIZE, PLATE_PARAM_WIDTH,
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

/// Width is smoothed sample-by-sample because it directly scales amplitude.
/// Size moves a delay read head, so it gets its own slightly slower glide
/// rather than clearing the network or jumping to uncorrelated history.
const WIDTH_SMOOTH_S: f32 = 0.02;
const SIZE_GLIDE_S: f32 = 0.04;
/// Pre-delay is a moving read head. It glides rather than crossfading two
/// taps, so automation has the natural, brief pitch movement of a changing
/// delay time without ever jumping to uncorrelated history.
const PREDELAY_SMOOTH_S: f32 = 0.02;
const PREDELAY_MAX_MS: f32 = 200.0;

/// Input is mono-summed before the network (matches the convolution
/// reverb's convention); this scales it back down before the 8-way parallel
/// comb sum so the combined output isn't inherently 8x hot.
const INPUT_GAIN: f32 = 1.0 / NUM_COMBS as f32;

/// The plate's absolute output reference. The comb sum's resonant buildup
/// depends on decay, size, and where the input's energy sits against the
/// comb poles, so the network has no single unity; this pins typical
/// material within a few dB of level-matched, like the convolution
/// reverb's IR energy target.
///
/// This was 0.45, calibrated against a *peak* comparison -- which a plate
/// passes easily, because its diffuse output has a far lower crest factor
/// than the dry transient it is matched against. Measured on steady-state
/// sustain instead, that reference ran +3.8 dB hot, so the mix knob was
/// already near reverb/dry parity by 30%. Scaled down by that 3.8 dB.
/// Enforced by `steady_state_wet_path_is_level_matched` in
/// `gain_structure_tests.rs`.
const OUTPUT_REFERENCE: f32 = 0.29;

fn size_multiplier(size: f32) -> f32 {
    SIZE_MIN_MULTIPLIER + size.clamp(0.0, 1.0) * (SIZE_MAX_MULTIPLIER - SIZE_MIN_MULTIPLIER)
}

/// Samples for a base (44100 Hz reference) tap length at `sample_rate` and
/// tuning `multiplier`.
fn scale_len(base: usize, sample_rate: u32, multiplier: f32) -> usize {
    ((base as f32 * multiplier * sample_rate as f32 / TUNING_SAMPLE_RATE).round() as usize).max(1)
}

/// A mono ring with a linearly interpolated read head. The writer always
/// traverses the full preallocated capacity; moving a delay length therefore
/// changes only where history is read, never the ownership or contents of the
/// storage.
struct Ring {
    buffer: Vec<f32>,
    write: usize,
}

impl Ring {
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

    fn read(&self, delay: f32) -> f32 {
        let capacity = self.buffer.len();
        let delay = delay.clamp(1.0, capacity as f32 - 2.0);
        let base = delay.floor();
        let fraction = delay - base;
        let index = (self.write + capacity - base as usize) % capacity;
        let previous = if index == 0 { capacity - 1 } else { index - 1 };
        self.buffer[index] * (1.0 - fraction) + self.buffer[previous] * fraction
    }
}

/// One feedback comb: a delay tap with damped feedback. Its ring is sized
/// once for the largest `size` can ask for; the active read length glides so
/// a size event cannot clear a tail or do buffer-wide work in `process()`.
struct Comb {
    base_len: usize,
    ring: Ring,
    len: f32,
    target_len: f32,
    feedback: f32,
    damp: OnePoleLp,
}

impl Comb {
    fn new(base_len: usize, sample_rate: u32) -> Self {
        let capacity = scale_len(base_len, sample_rate, SIZE_MAX_MULTIPLIER) + 2;
        let len = scale_len(base_len, sample_rate, 1.0) as f32;
        Self {
            base_len,
            ring: Ring::with_capacity(capacity),
            len,
            target_len: len,
            feedback: 0.0,
            damp: OnePoleLp::new(),
        }
    }

    fn set_size(&mut self, sample_rate: u32, size: f32) {
        self.target_len = (scale_len(self.base_len, sample_rate, size_multiplier(size)) as f32)
            .min(self.ring.buffer.len() as f32 - 2.0);
    }

    fn set_decay(&mut self, sample_rate: u32, decay_s: f32) {
        let seconds = self.target_len / sample_rate.max(1) as f32;
        self.feedback = 10f32
            .powf(-3.0 * seconds / decay_s.max(0.01))
            .clamp(0.0, FEEDBACK_MAX);
    }

    fn set_damping(&mut self, sample_rate: u32, damping: f32) {
        let t = damping.clamp(0.0, 1.0);
        let hz = DAMP_MAX_HZ * (DAMP_MIN_HZ / DAMP_MAX_HZ).powf(t);
        self.damp.set_cutoff(hz, sample_rate);
    }

    fn process(&mut self, input: f32, size_glide: f32) -> f32 {
        self.len += (self.target_len - self.len) * size_glide;
        let output = self.ring.read(self.len);
        let damped = self.damp.next_sample(output);
        self.ring.write(input + damped * self.feedback);
        output
    }
}

/// A Schroeder allpass diffuser: fixed unity-magnitude gain, so it reshapes
/// the echo density without changing overall level. Distinct from
/// `crate::filter::AllPass`, which is a single-sample phase-shift stage, not
/// a delay-line diffuser.
struct SchroederAllpass {
    base_len: usize,
    ring: Ring,
    len: f32,
    target_len: f32,
}

impl SchroederAllpass {
    fn new(base_len: usize, sample_rate: u32) -> Self {
        let capacity = scale_len(base_len, sample_rate, SIZE_MAX_MULTIPLIER) + 2;
        let len = scale_len(base_len, sample_rate, 1.0) as f32;
        Self {
            base_len,
            ring: Ring::with_capacity(capacity),
            len,
            target_len: len,
        }
    }

    fn set_size(&mut self, sample_rate: u32, size: f32) {
        self.target_len = (scale_len(self.base_len, sample_rate, size_multiplier(size)) as f32)
            .min(self.ring.buffer.len() as f32 - 2.0);
    }

    fn process(&mut self, input: f32, size_glide: f32) -> f32 {
        self.len += (self.target_len - self.len) * size_glide;
        let buffered = self.ring.read(self.len);
        let output = -input + buffered;
        self.ring.write(input + buffered * ALLPASS_GAIN);
        output
    }
}

pub struct PlateEffect {
    params: PlateParams,
    sample_rate: u32,
    /// Input history for pre-delay. It is allocated at the maximum supported
    /// delay when the node is constructed, never in the audio callback.
    predelay: Ring,
    predelay_samples: Smoothed,
    combs_l: [Comb; NUM_COMBS],
    combs_r: [Comb; NUM_COMBS],
    allpass_l: [SchroederAllpass; NUM_ALLPASS],
    allpass_r: [SchroederAllpass; NUM_ALLPASS],
    wet1: Smoothed,
    wet2: Smoothed,
    size_glide: f32,
}

impl PlateEffect {
    pub fn new(params: PlateParams, sample_rate: u32) -> Self {
        let sample_rate = sample_rate.max(1);
        let combs_l = std::array::from_fn(|i| Comb::new(COMB_TUNING_L[i], sample_rate));
        let combs_r =
            std::array::from_fn(|i| Comb::new(COMB_TUNING_L[i] + STEREO_SPREAD, sample_rate));
        let allpass_l =
            std::array::from_fn(|i| SchroederAllpass::new(ALLPASS_TUNING_L[i], sample_rate));
        let allpass_r = std::array::from_fn(|i| {
            SchroederAllpass::new(ALLPASS_TUNING_L[i] + STEREO_SPREAD, sample_rate)
        });
        let width = params.width.clamp(0.0, 1.0);
        let mut effect = Self {
            params,
            sample_rate,
            predelay: Ring::with_capacity(predelay_capacity(sample_rate)),
            predelay_samples: Smoothed::new(
                predelay_samples(params.predelay_ms, sample_rate),
                PREDELAY_SMOOTH_S,
                sample_rate,
            ),
            combs_l,
            combs_r,
            allpass_l,
            allpass_r,
            wet1: Smoothed::new(0.5 + width * 0.5, WIDTH_SMOOTH_S, sample_rate),
            wet2: Smoothed::new(0.5 - width * 0.5, WIDTH_SMOOTH_S, sample_rate),
            size_glide: glide_coeff(SIZE_GLIDE_S, sample_rate),
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
            PLATE_PARAM_PREDELAY_MS => {
                self.params.predelay_ms = value.clamp(0.0, PREDELAY_MAX_MS);
                self.predelay_samples
                    .set_target(predelay_samples(self.params.predelay_ms, self.sample_rate));
            }
            _ => {}
        }
    }

    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        for i in start..end {
            let dry = (bus.l[i] + bus.r[i]) * 0.5 * INPUT_GAIN;
            let delay = self.predelay_samples.advance();
            // Read before writing so an N-sample pre-delay delays an impulse
            // by exactly N frames. At 0 ms we deliberately bypass the ring:
            // this retains the old Plate path bit-for-bit.
            let input = if delay <= f32::EPSILON {
                dry
            } else {
                self.predelay.read(delay)
            };
            self.predelay.write(dry);

            let mut wet_l = 0.0f32;
            for comb in self.combs_l.iter_mut() {
                wet_l += comb.process(input, self.size_glide);
            }
            let mut wet_r = 0.0f32;
            for comb in self.combs_r.iter_mut() {
                wet_r += comb.process(input, self.size_glide);
            }

            for allpass in self.allpass_l.iter_mut() {
                wet_l = allpass.process(wet_l, self.size_glide);
            }
            for allpass in self.allpass_r.iter_mut() {
                wet_r = allpass.process(wet_r, self.size_glide);
            }

            let wet1 = self.wet1.advance();
            let wet2 = self.wet2.advance();
            bus.l[i] = (wet_l * wet1 + wet_r * wet2) * OUTPUT_REFERENCE;
            bus.r[i] = (wet_r * wet1 + wet_l * wet2) * OUTPUT_REFERENCE;
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
            self.sample_rate = ctx.sample_rate.max(1);
            self.rebuild_feedback();
            self.rebuild_damping();
            self.wet1.set_time(WIDTH_SMOOTH_S, ctx.sample_rate);
            self.wet2.set_time(WIDTH_SMOOTH_S, ctx.sample_rate);
            self.predelay_samples
                .set_time(PREDELAY_SMOOTH_S, self.sample_rate);
            self.predelay_samples
                .set_target(predelay_samples(self.params.predelay_ms, self.sample_rate));
            self.size_glide = glide_coeff(SIZE_GLIDE_S, ctx.sample_rate);
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

fn glide_coeff(time_s: f32, sample_rate: u32) -> f32 {
    let samples = (time_s.max(1.0e-5) * sample_rate.max(1) as f32).max(1.0);
    1.0 - (-1.0 / samples).exp()
}

fn predelay_samples(predelay_ms: f32, sample_rate: u32) -> f32 {
    predelay_ms.clamp(0.0, PREDELAY_MAX_MS) * 0.001 * sample_rate.max(1) as f32
}

fn predelay_capacity(sample_rate: u32) -> usize {
    (PREDELAY_MAX_MS * 0.001 * sample_rate.max(1) as f32).ceil() as usize + 4
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
    fn predelay_moves_the_wet_onset_by_the_requested_frames() {
        const PREDELAY_MS: f32 = 50.0;
        const FRAMES: usize = 16_000;
        let reference = impulse_response(PlateParams::default(), FRAMES);
        let delayed = impulse_response(
            PlateParams {
                predelay_ms: PREDELAY_MS,
                ..PlateParams::default()
            },
            FRAMES,
        );
        let frames = predelay_samples(PREDELAY_MS, 48_000) as usize;

        assert!(
            delayed.l[..frames].iter().all(|sample| *sample == 0.0)
                && delayed.r[..frames].iter().all(|sample| *sample == 0.0),
            "the wet path must remain silent before pre-delay elapses"
        );
        for i in 0..FRAMES - frames {
            assert_eq!(delayed.l[i + frames], reference.l[i]);
            assert_eq!(delayed.r[i + frames], reference.r[i]);
        }
    }

    #[test]
    fn zero_predelay_bypasses_the_input_history() {
        let mut effect = PlateEffect::new(PlateParams::default(), 48_000);
        effect.predelay.buffer.fill(1.0);
        let mut bus = StereoBus::with_capacity(512);
        effect.process(&context(512), &mut bus, &EventList::empty(), None);
        assert!(
            bus.l[..512]
                .iter()
                .chain(&bus.r[..512])
                .all(|sample| *sample == 0.0),
            "0 ms must take the legacy direct-input path, not read the ring"
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

    #[test]
    fn size_change_preserves_delay_history() {
        let mut comb = Comb::new(COMB_TUNING_L[0], 48_000);
        comb.set_size(48_000, 0.5);
        comb.set_decay(48_000, 4.0);
        comb.set_damping(48_000, 0.2);
        for frame in 0..4_000 {
            comb.process(if frame == 0 { 1.0 } else { 0.0 }, 1.0);
        }

        let history = comb.ring.buffer.clone();
        let write = comb.ring.write;
        comb.set_size(48_000, 1.0);

        assert_eq!(
            comb.ring.buffer, history,
            "retuning must not clear the ring"
        );
        assert_eq!(
            comb.ring.write, write,
            "retuning must not reset the write head"
        );
    }

    #[test]
    fn sweeping_size_does_not_click_or_silence_the_tail() {
        const BLOCK: usize = 256;
        const TONE_HZ: f32 = 80.0;
        let mut effect = PlateEffect::new(PlateParams::default(), 48_000);
        let mut phase = 0.0f32;
        let mut fill_tone = |bus: &mut StereoBus| {
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

        for _ in 0..64 {
            let mut bus = StereoBus::with_capacity(BLOCK);
            fill_tone(&mut bus);
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
                    id: PLATE_PARAM_SIZE,
                    value: step as f32 / 63.0,
                },
            });
            let mut bus = StereoBus::with_capacity(BLOCK);
            fill_tone(&mut bus);
            effect.process(&context(BLOCK), &mut bus, &events, None);
            for sample in &bus.l[..BLOCK] {
                if let Some(previous) = previous {
                    worst_step = worst_step.max((sample - previous).abs());
                }
                peak = peak.max(sample.abs());
                previous = Some(*sample);
            }
        }

        assert!(peak > 1e-4, "the tone should have excited the network");
        assert!(
            worst_step < peak * 0.1,
            "a size sweep stepped the output by {worst_step}, {:.1}% of the peak",
            100.0 * worst_step / peak,
        );
    }

    #[test]
    fn sweeping_predelay_does_not_click_or_silence_the_tail() {
        const BLOCK: usize = 256;
        const TONE_HZ: f32 = 80.0;
        let mut effect = PlateEffect::new(PlateParams::default(), 48_000);
        let mut phase = 0.0f32;
        let mut fill_tone = |bus: &mut StereoBus| {
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

        for _ in 0..64 {
            let mut bus = StereoBus::with_capacity(BLOCK);
            fill_tone(&mut bus);
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
                    id: PLATE_PARAM_PREDELAY_MS,
                    value: if step % 2 == 0 { 200.0 } else { 0.0 },
                },
            });
            let mut bus = StereoBus::with_capacity(BLOCK);
            fill_tone(&mut bus);
            effect.process(&context(BLOCK), &mut bus, &events, None);
            for sample in &bus.l[..BLOCK] {
                if let Some(previous) = previous {
                    worst_step = worst_step.max((sample - previous).abs());
                }
                peak = peak.max(sample.abs());
                previous = Some(*sample);
            }
        }

        assert!(peak > 1e-4, "the tone should have excited the network");
        assert!(
            worst_step < peak * 0.1,
            "a pre-delay sweep stepped the output by {worst_step}, {:.1}% of the peak",
            100.0 * worst_step / peak,
        );
    }
}
