//! Generated-room convolution reverb.
//!
//! The room generator and the player deliberately meet at [`StereoIr`]. A
//! future WAV/AIFF loader only needs to decode and resample into that type;
//! the realtime player is unaware whether its response came from a measured
//! space or the generator below.
//!
//! Convolution uses fixed 512-frame overlap-save partitions. Preparing the
//! partition spectra allocates and runs FFTs on the control side. The audio
//! path only uses preallocated complex buffers and reports its one-partition
//! latency to the effect host, which already aligns the generic dry path.
//!
//! `ReverbEffect::process` ignores `events_in` entirely: unlike the other
//! effects, there is no per-effect wet-level parameter here to run through
//! `crate::smooth`. Wet/dry is the *generic* per-slot blend every effect
//! sits behind (`EffectChain::wet_dry` in `mooloop-engine`), and that blend
//! is itself unsmoothed — a real gap, but outside this crate and this
//! effect, and shared by every other node in the chain rather than specific
//! to reverb. See `docs/plans/share-dsp-primitives/01-smooth-effect-parameters.md`.

use mooloop_core::{ReverbMaterial, ReverbParams, ReverbShape};

use crate::bus::StereoBus;
use crate::event::EventList;
use crate::node::{AudioNode, ProcessContext};

/// Partition size balances the roughly 11 ms host-compensated latency at
/// 48 kHz against long-room CPU cost. It is deliberately fixed: an IR swap
/// then changes spectra, not realtime storage shape.
pub const CONVOLUTION_BLOCK_FRAMES: usize = 512;
const FFT_FRAMES: usize = CONVOLUTION_BLOCK_FRAMES * 2;
const MAX_IR_SECONDS: f32 = 2.0;
const SOUND_SPEED_MPS: f32 = 343.0;

/// Target gain for the generated stereo IR. A strictly unit norm leaves
/// each output channel about 3 dB under its input for broadband material
/// (the two channels share the budget); the margin here lands a 100% wet
/// blend level-matched with the dry path, verified against the engine's
/// wet/dry measurement in `gain_structure_tests.rs`.
const IR_ENERGY_TARGET: f32 = 1.5;

/// Worst-case steady-tone gain any single frequency may see through the
/// normalized IR, as a multiple of [`IR_ENERGY_TARGET`] (about +5 dB).
/// Constraining the peak as well as the probe average is what stops the
/// diffuse tail from handing a sustained low partial far over the dry path
/// while the broadband average still matches; combined with the tail's
/// tilt-bounding air component it keeps tonal material within a few dB of
/// the target. The tonal peak is measured against the engine's wet/dry
/// characterization in `gain_structure_tests.rs`.
const IR_TONAL_CEILING: f32 = 1.75;

#[derive(Clone, Copy, Default)]
struct Complex {
    re: f32,
    im: f32,
}

impl Complex {
    const ZERO: Self = Self { re: 0.0, im: 0.0 };

    fn mul_add(self, rhs: Self, sum: &mut Self) {
        sum.re += self.re * rhs.re - self.im * rhs.im;
        sum.im += self.re * rhs.im + self.im * rhs.re;
    }
}

/// A mono-in, stereo-out impulse response. Generated rooms use the same
/// representation as measured stereo IRs, keeping import out of the DSP node.
#[derive(Clone, Debug)]
pub struct StereoIr {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
}

impl StereoIr {
    pub fn new(left: Vec<f32>, right: Vec<f32>) -> Self {
        let frames = left.len().min(right.len());
        Self {
            left: left.into_iter().take(frames).collect(),
            right: right.into_iter().take(frames).collect(),
        }
    }

    pub fn frames(&self) -> usize {
        self.left.len().min(self.right.len())
    }
}

/// FFT-domain response prepared entirely off the audio thread.
pub struct PreparedIr {
    partitions: usize,
    left: Vec<Complex>,
    right: Vec<Complex>,
}

impl PreparedIr {
    pub fn from_stereo(ir: &StereoIr) -> Self {
        let partitions = ir.frames().max(1).div_ceil(CONVOLUTION_BLOCK_FRAMES);
        let mut left = vec![Complex::ZERO; partitions * FFT_FRAMES];
        let mut right = vec![Complex::ZERO; partitions * FFT_FRAMES];
        let mut scratch = [Complex::ZERO; FFT_FRAMES];

        for part in 0..partitions {
            let start = part * CONVOLUTION_BLOCK_FRAMES;
            scratch.fill(Complex::ZERO);
            for (i, bin) in scratch.iter_mut().take(CONVOLUTION_BLOCK_FRAMES).enumerate() {
                if let Some(sample) = ir.left.get(start + i) {
                    bin.re = *sample;
                }
            }
            fft(&mut scratch, false);
            left[part * FFT_FRAMES..(part + 1) * FFT_FRAMES].copy_from_slice(&scratch);

            scratch.fill(Complex::ZERO);
            for (i, bin) in scratch.iter_mut().take(CONVOLUTION_BLOCK_FRAMES).enumerate() {
                if let Some(sample) = ir.right.get(start + i) {
                    bin.re = *sample;
                }
            }
            fft(&mut scratch, false);
            right[part * FFT_FRAMES..(part + 1) * FFT_FRAMES].copy_from_slice(&scratch);
        }

        Self {
            partitions,
            left,
            right,
        }
    }

    fn partition(&self, channel: usize, index: usize) -> &[Complex] {
        let spectra = if channel == 0 {
            &self.left
        } else {
            &self.right
        };
        let start = index * FFT_FRAMES;
        &spectra[start..start + FFT_FRAMES]
    }
}

/// Generate an acoustically plausible, deterministic room IR from the compact
/// device parameters. Image sources give it position-dependent early
/// reflections; a material-filtered stochastic tail supplies the dense decay
/// that a small image-source set cannot represent economically.
pub fn generate_room_ir(params: ReverbParams, sample_rate: u32) -> StereoIr {
    let sample_rate = sample_rate.max(1);
    let max_frames = (MAX_IR_SECONDS * sample_rate as f32).round() as usize;
    let tail_seconds = params.decay_s.clamp(0.15, MAX_IR_SECONDS);
    let frames = ((tail_seconds * sample_rate as f32 * 1.12).round() as usize).clamp(
        CONVOLUTION_BLOCK_FRAMES,
        max_frames.max(CONVOLUTION_BLOCK_FRAMES),
    );
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];

    let width = params.width_m.clamp(2.0, 30.0);
    let depth = params.depth_m.clamp(2.0, 50.0);
    let height = params.height_m.clamp(2.0, 20.0);
    let source = [width * 0.32, depth * 0.29, height * 0.43];
    let mic = [
        width * params.capture_x.clamp(0.05, 0.95),
        depth * params.capture_y.clamp(0.05, 0.95),
        height * 0.47,
    ];
    let (reflect, brightness, tail_level, image_order) =
        room_character(params.shape, params.material);

    for x in -image_order..=image_order {
        for y in -image_order..=image_order {
            for z in -image_order..=image_order {
                let order = x.unsigned_abs() + y.unsigned_abs() + z.unsigned_abs();
                if order == 0 {
                    // The generic host already provides dry/wet. Including the
                    // direct path in the IR would double the unprocessed sound.
                    continue;
                }
                let image = [
                    image_coordinate(x, width, source[0]),
                    image_coordinate(y, depth, source[1]),
                    image_coordinate(z, height, source[2]),
                ];
                let dx = image[0] - mic[0];
                let dy = image[1] - mic[1];
                let dz = image[2] - mic[2];
                let distance = (dx * dx + dy * dy + dz * dz).sqrt().max(0.25);
                let frame = (distance / SOUND_SPEED_MPS * sample_rate as f32).round() as usize;
                if frame >= frames {
                    continue;
                }
                let amplitude = 0.62 * reflect.powi(order as i32) / (1.0 + distance * 0.42);
                let pan = (dx / (distance + 0.1)).clamp(-0.82, 0.82);
                let left_gain = (0.5 * (1.0 - pan)).sqrt();
                let right_gain = (0.5 * (1.0 + pan)).sqrt();
                add_impulse(&mut left, frame, amplitude * left_gain);
                add_impulse(&mut right, frame, amplitude * right_gain);
            }
        }
    }

    // Deterministic decorrelated diffuse tail. Its envelope reaches -60 dB at
    // `decay_s`; the material's brightness filters high frequencies before the
    // convolution stage instead of spending another realtime filter per node.
    let mut seed = seed_for(params);
    let mut low_l = 0.0;
    let mut low_r = 0.0;
    let tail_start = (sample_rate as f32 * 0.017) as usize;
    let lowpass = 0.035 + brightness * 0.22;
    // Cap the tail's spectral tilt. The one-pole alone is up to ~29 dB
    // darker at the top of the band, so a narrowband tone sampled a far
    // hotter point of the response than the broadband average and no single
    // scalar gain could level-match both. Mixing in a white component sized
    // to hold the top of the band within ~12 dB of the low bound the tilt;
    // the calibration below then places every frequency within a few dB of
    // the target.
    let air = (1.0 / (32.0 * lowpass)).sqrt();
    for frame in tail_start..frames {
        let seconds = frame as f32 / sample_rate as f32;
        let onset = ((seconds - 0.017) / 0.11).clamp(0.0, 1.0);
        let envelope = (-6.907_755 * seconds / tail_seconds).exp();
        let noise_l = next_noise(&mut seed);
        let noise_r = next_noise(&mut seed);
        low_l += (noise_l - low_l) * lowpass;
        low_r += (noise_r - low_r) * lowpass;
        let gain = tail_level * onset * envelope;
        left[frame] += (low_l + air * noise_l) * gain;
        right[frame] += (low_r + air * noise_r) * gain;
    }

    // Gain calibration targets equal wet/dry loudness for *tonal* as well as
    // broadband material. The IR's peak is dominated by its single early
    // reflection while its energy is dominated by the tail, so constraining
    // the peak said almost nothing about how loud sustained material comes
    // out — that was why 1% wet was audible. A pure total-L2 normalization
    // then matched broadband probes but let sustained low partials read up
    // to ~11 dB over dry: the diffuse tail's spectrum tilts low, so a
    // narrowband tone samples a hotter point of the response than the
    // broadband average. Instead the IR is unit-normalized and the final
    // gain is measured across representative spectral probes: steady tones
    // at bin k emerge with gain |H(f_k)|, and the bin-average of |H|² over
    // all bins equals the IR's L2 energy (Parseval), so the probe average
    // recovers the old broadband calibration while the peak bound caps the
    // worst single frequency. Both bounds are control-side; the audio path
    // is untouched.
    let energy = left
        .iter()
        .chain(right.iter())
        .map(|sample| sample * sample)
        .sum::<f32>()
        .sqrt();
    let gain = if energy > 1.0e-9 {
        let inv = 1.0 / energy;
        for sample in left.iter_mut().chain(right.iter_mut()) {
            *sample *= inv;
        }
        ir_calibration_gain(&left, &right, sample_rate)
    } else {
        1.0
    };
    for sample in left.iter_mut().chain(right.iter_mut()) {
        *sample *= gain;
    }

    StereoIr { left, right }
}

/// Final scalar for an IR with total stereo L2 norm of one: the tighter of
/// the broadband-probe average gain and the single-frequency ceiling, both
/// expressed against [`IR_ENERGY_TARGET`].
fn ir_calibration_gain(left: &[f32], right: &[f32], sample_rate: u32) -> f32 {
    let powers = probe_powers(left, right, sample_rate);
    if powers.is_empty() {
        return IR_ENERGY_TARGET;
    }
    let count = powers.len() as f32;
    let mean_power = powers.iter().sum::<f32>() / count;
    let peak_power = powers.iter().cloned().fold(0.0, f32::max);
    if mean_power <= 1.0e-12 || peak_power <= 1.0e-12 {
        return IR_ENERGY_TARGET;
    }
    let broadband = IR_ENERGY_TARGET / mean_power.sqrt();
    let tonal = IR_ENERGY_TARGET * IR_TONAL_CEILING / peak_power.sqrt();
    broadband.min(tonal)
}

/// |H(f)|² summed over both channels at log-spaced probe frequencies
/// spanning the musically relevant band. For a unit-L2 stereo IR the
/// bin-average of this quantity is one, so its probe-average plays the role
/// the total L2 norm used to play — without ignoring where in the spectrum
/// the energy sits. The response comes from one zero-padded FFT per
/// channel, so probes read the true response rather than sampling past it.
fn probe_powers(left: &[f32], right: &[f32], sample_rate: u32) -> Vec<f32> {
    const PROBES: usize = 96;
    const F_MIN_HZ: f32 = 50.0;
    const F_MAX_HZ: f32 = 16_000.0;
    let frames = left.len().max(right.len());
    if frames == 0 {
        return Vec::new();
    }
    let mut spectrum = vec![Complex::ZERO; frames.next_power_of_two()];
    let mut left_powers = vec![0.0; PROBES];
    let mut right_powers = vec![0.0; PROBES];
    for (channel_powers, samples) in [(&mut left_powers, left), (&mut right_powers, right)] {
        spectrum.fill(Complex::ZERO);
        for (bin, sample) in spectrum.iter_mut().zip(samples) {
            bin.re = *sample;
        }
        fft(&mut spectrum, false);
        let bin_hz = sample_rate as f32 / spectrum.len() as f32;
        let ratio = F_MAX_HZ / F_MIN_HZ;
        for (probe, power) in channel_powers.iter_mut().enumerate() {
            let frequency = F_MIN_HZ * ratio.powf(probe as f32 / (PROBES - 1) as f32);
            let bin = (frequency / bin_hz).round() as usize;
            if let Some(value) = spectrum.get(bin) {
                *power = value.re * value.re + value.im * value.im;
            }
        }
    }
    left_powers
        .iter()
        .zip(right_powers)
        .map(|(l, r)| l + r)
        .collect()
}

fn room_character(shape: ReverbShape, material: ReverbMaterial) -> (f32, f32, f32, i32) {
    let (reflect, brightness, tail_level) = match material {
        ReverbMaterial::Plaster => (0.82, 0.76, 0.090),
        ReverbMaterial::Wood => (0.76, 0.61, 0.082),
        ReverbMaterial::Brick => (0.87, 0.70, 0.098),
        ReverbMaterial::Curtain => (0.60, 0.30, 0.064),
    };
    match shape {
        ReverbShape::Studio => (reflect, brightness, tail_level, 2),
        ReverbShape::Chamber => (reflect * 0.96, brightness * 0.88, tail_level * 1.10, 3),
        ReverbShape::Hall => (reflect * 1.03, brightness * 0.93, tail_level * 1.22, 4),
    }
}

fn image_coordinate(index: i32, length: f32, source: f32) -> f32 {
    2.0 * index as f32 * length
        + if index & 1 == 0 {
            source
        } else {
            length - source
        }
}

fn add_impulse(buffer: &mut [f32], frame: usize, amplitude: f32) {
    if let Some(sample) = buffer.get_mut(frame) {
        *sample += amplitude;
    }
}

fn seed_for(params: ReverbParams) -> u32 {
    params.width_m.to_bits()
        ^ params.depth_m.to_bits().rotate_left(5)
        ^ params.height_m.to_bits().rotate_left(11)
        ^ params.capture_x.to_bits().rotate_left(17)
        ^ params.capture_y.to_bits().rotate_left(23)
        ^ (params.shape.to_index() as u32).wrapping_mul(0x9e37_79b9)
        ^ (params.material.to_index() as u32).wrapping_mul(0x85eb_ca6b)
        ^ 0x517c_c1b7
}

fn next_noise(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    ((*seed >> 8) as f32 / 8_388_608.0) - 1.0
}

/// In-place radix-2 complex FFT. The `inverse` form includes 1/N scaling.
fn fft(values: &mut [Complex], inverse: bool) {
    debug_assert!(values.len().is_power_of_two());
    let n = values.len();
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            values.swap(i, j);
        }
    }

    let sign = if inverse { 1.0 } else { -1.0 };
    let mut len = 2;
    while len <= n {
        let angle = sign * core::f32::consts::TAU / len as f32;
        let w_len = Complex {
            re: angle.cos(),
            im: angle.sin(),
        };
        for start in (0..n).step_by(len) {
            let mut w = Complex { re: 1.0, im: 0.0 };
            for offset in 0..len / 2 {
                let even = values[start + offset];
                let odd = values[start + offset + len / 2];
                let term = Complex {
                    re: odd.re * w.re - odd.im * w.im,
                    im: odd.re * w.im + odd.im * w.re,
                };
                values[start + offset] = Complex {
                    re: even.re + term.re,
                    im: even.im + term.im,
                };
                values[start + offset + len / 2] = Complex {
                    re: even.re - term.re,
                    im: even.im - term.im,
                };
                w = Complex {
                    re: w.re * w_len.re - w.im * w_len.im,
                    im: w.re * w_len.im + w.im * w_len.re,
                };
            }
        }
        len *= 2;
    }
    if inverse {
        let scale = 1.0 / n as f32;
        for value in values {
            value.re *= scale;
            value.im *= scale;
        }
    }
}

/// Allocation-free partitioned convolution state. Input is intentionally
/// summed to mono before the IR: stereo IRs then describe a room perspective
/// and leave dry-channel width to the shared host blend.
pub struct ReverbEffect {
    prepared: PreparedIr,
    input_history: Vec<Complex>,
    history_index: usize,
    overlap: Box<[f32; CONVOLUTION_BLOCK_FRAMES]>,
    input: Box<[f32; CONVOLUTION_BLOCK_FRAMES]>,
    input_fill: usize,
    output_left: Box<[f32; CONVOLUTION_BLOCK_FRAMES]>,
    output_right: Box<[f32; CONVOLUTION_BLOCK_FRAMES]>,
    output_index: usize,
    fft_input: Box<[Complex; FFT_FRAMES]>,
    fft_left: Box<[Complex; FFT_FRAMES]>,
    fft_right: Box<[Complex; FFT_FRAMES]>,
}

impl ReverbEffect {
    /// Generate and prepare a room response on the caller's non-realtime
    /// thread. Use [`Self::from_ir`] for a measured response.
    pub fn new(params: ReverbParams, sample_rate: u32) -> Self {
        Self::from_ir(&generate_room_ir(params, sample_rate))
    }

    /// Prepare a response into a new player. This is the public boundary a
    /// future IR-file import path will use after decoding/resampling.
    pub fn from_ir(ir: &StereoIr) -> Self {
        let prepared = PreparedIr::from_stereo(ir);
        let input_history = vec![Complex::ZERO; prepared.partitions * FFT_FRAMES];
        Self {
            prepared,
            input_history,
            history_index: 0,
            overlap: Box::new([0.0; CONVOLUTION_BLOCK_FRAMES]),
            input: Box::new([0.0; CONVOLUTION_BLOCK_FRAMES]),
            input_fill: 0,
            output_left: Box::new([0.0; CONVOLUTION_BLOCK_FRAMES]),
            output_right: Box::new([0.0; CONVOLUTION_BLOCK_FRAMES]),
            output_index: CONVOLUTION_BLOCK_FRAMES,
            fft_input: Box::new([Complex::ZERO; FFT_FRAMES]),
            fft_left: Box::new([Complex::ZERO; FFT_FRAMES]),
            fft_right: Box::new([Complex::ZERO; FFT_FRAMES]),
        }
    }

    fn process_partition(&mut self) {
        for i in 0..CONVOLUTION_BLOCK_FRAMES {
            self.fft_input[i] = Complex {
                re: self.overlap[i],
                im: 0.0,
            };
            self.fft_input[CONVOLUTION_BLOCK_FRAMES + i] = Complex {
                re: self.input[i],
                im: 0.0,
            };
        }
        fft(&mut self.fft_input[..], false);
        let write_start = self.history_index * FFT_FRAMES;
        self.input_history[write_start..write_start + FFT_FRAMES]
            .copy_from_slice(&self.fft_input[..]);

        self.fft_left.fill(Complex::ZERO);
        self.fft_right.fill(Complex::ZERO);
        for partition in 0..self.prepared.partitions {
            let history = (self.history_index + self.prepared.partitions - partition)
                % self.prepared.partitions;
            let input = &self.input_history[history * FFT_FRAMES..(history + 1) * FFT_FRAMES];
            let left = self.prepared.partition(0, partition);
            let right = self.prepared.partition(1, partition);
            for bin in 0..FFT_FRAMES {
                input[bin].mul_add(left[bin], &mut self.fft_left[bin]);
                input[bin].mul_add(right[bin], &mut self.fft_right[bin]);
            }
        }
        fft(&mut self.fft_left[..], true);
        fft(&mut self.fft_right[..], true);
        for i in 0..CONVOLUTION_BLOCK_FRAMES {
            self.output_left[i] = self.fft_left[CONVOLUTION_BLOCK_FRAMES + i].re;
            self.output_right[i] = self.fft_right[CONVOLUTION_BLOCK_FRAMES + i].re;
        }
        self.overlap.copy_from_slice(&self.input[..]);
        self.history_index = (self.history_index + 1) % self.prepared.partitions;
        self.input_fill = 0;
        self.output_index = 0;
    }
}

impl AudioNode for ReverbEffect {
    fn latency_frames(&self) -> u32 {
        CONVOLUTION_BLOCK_FRAMES as u32
    }

    fn dry_path_latency_frames(&self) -> u32 {
        // There is no direct component in the generated IR. Delaying the
        // host dry branch would move this whole insert against other channels;
        // keep the convolution latency as the room return's natural pre-delay.
        0
    }

    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        _events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        let frames = ctx.frames.min(bus.capacity());
        for frame in 0..frames {
            let input = (bus.l[frame] + bus.r[frame]) * 0.5;
            let (left, right) = if self.output_index < CONVOLUTION_BLOCK_FRAMES {
                let out = (
                    self.output_left[self.output_index],
                    self.output_right[self.output_index],
                );
                self.output_index += 1;
                out
            } else {
                (0.0, 0.0)
            };
            self.input[self.input_fill] = input;
            self.input_fill += 1;
            if self.input_fill == CONVOLUTION_BLOCK_FRAMES {
                self.process_partition();
            }
            bus.l[frame] = left;
            bus.r[frame] = right;
        }
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

    #[test]
    fn identity_ir_is_delayed_by_one_partition_and_preserves_stereo_gain() {
        let mut effect = ReverbEffect::from_ir(&StereoIr::new(vec![1.0], vec![0.5]));
        let mut bus = StereoBus::with_capacity(CONVOLUTION_BLOCK_FRAMES * 2);
        bus.l[0] = 1.0;
        bus.r[0] = 1.0;
        effect.process(
            &context(CONVOLUTION_BLOCK_FRAMES * 2),
            &mut bus,
            &EventList::empty(),
            None,
        );
        assert!(bus.l[..CONVOLUTION_BLOCK_FRAMES]
            .iter()
            .all(|sample| sample.abs() < 1e-5));
        assert!((bus.l[CONVOLUTION_BLOCK_FRAMES] - 1.0).abs() < 1e-4);
        assert!((bus.r[CONVOLUTION_BLOCK_FRAMES] - 0.5).abs() < 1e-4);
    }

    #[test]
    fn generated_room_tonal_gain_stays_under_ceiling() {
        let rooms = [
            ReverbParams::default(),
            ReverbParams {
                shape: ReverbShape::Hall,
                material: ReverbMaterial::Brick,
                decay_s: 2.0,
                ..ReverbParams::default()
            },
            ReverbParams {
                shape: ReverbShape::Studio,
                material: ReverbMaterial::Curtain,
                decay_s: 0.4,
                ..ReverbParams::default()
            },
        ];
        for params in rooms {
            let ir = generate_room_ir(params, 48_000);
            let peak = probe_powers(&ir.left, &ir.right, 48_000)
                .iter()
                .cloned()
                .fold(0.0, f32::max)
                .sqrt();
            assert!(
                peak <= IR_ENERGY_TARGET * IR_TONAL_CEILING * 1.02,
                "room {params:?}: worst single-frequency gain {peak:.2} exceeds ceiling",
            );
        }
    }

    #[test]
    fn generated_room_changes_with_capture_position() {
        let a = generate_room_ir(ReverbParams::default(), 48_000);
        let b = generate_room_ir(
            ReverbParams {
                capture_x: 0.18,
                capture_y: 0.22,
                ..ReverbParams::default()
            },
            48_000,
        );
        assert!(a.frames() >= CONVOLUTION_BLOCK_FRAMES);
        assert_ne!(a.left, b.left);
        assert!(a.left.iter().any(|sample| sample.abs() > 1e-5));
    }

    #[test]
    fn generated_room_has_a_decay_tail() {
        let ir = generate_room_ir(
            ReverbParams {
                decay_s: 1.5,
                ..ReverbParams::default()
            },
            48_000,
        );
        let early = ir.left[1_000..4_000]
            .iter()
            .map(|sample| sample * sample)
            .sum::<f32>();
        let late = ir.left[30_000..40_000]
            .iter()
            .map(|sample| sample * sample)
            .sum::<f32>();
        assert!(early > late, "generated tail must decay");
        assert!(late > 1e-7, "generated tail must remain audible");
    }
}
