//! Candidate B: STFT phase vocoder, the frequency-domain family, with
//! Laroche-Dolson identity phase locking.
//!
//! Shape of the algorithm as implemented here:
//!
//! - Fixed synthesis hop `Hs = N / 4`; the analysis pointer advances by
//!   `Hs / ratio` and the phase math uses the *actual* integer distance between
//!   consecutive analysis frames, so fractional ratios do not bias pitch.
//! - Per bin, the instantaneous frequency is recovered from the principal
//!   argument of the heterodyned phase advance and re-integrated at the
//!   synthesis hop. That is what makes the result pitch-correct.
//! - Identity phase locking ties every bin in a spectral peak's region of
//!   influence to that peak's propagated phase, keeping a partial's sidelobes
//!   coherent. Without it the result is the textbook "phasy" phase vocoder;
//!   the `PLAIN` ablation measures that difference.
//! - Stereo is handled by propagating one phase trajectory from the mid signal
//!   and rotating both channels by the same correction, which preserves the
//!   inter-channel phase difference exactly. The `INDEPENDENT` preset does what
//!   a naive two-mono-instance implementation does, to quantify image damage.
//! - The startup overlap ramp is divided out by the partial COLA sum, so this
//!   candidate is not penalised for a fade-in a real implementation would fix.

use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use crate::{hann, Source, Stretcher};

#[derive(Clone, Copy)]
pub struct PvocConfig {
    pub name: &'static str,
    pub fft_size: usize,
    pub phase_lock: bool,
    pub transient_reset: bool,
    pub independent_channels: bool,
}

impl PvocConfig {
    /// The serious phase-vocoder candidate.
    pub const LOCKED: Self = Self {
        name: "pvoc_locked",
        fft_size: 2048,
        phase_lock: true,
        transient_reset: false,
        independent_channels: false,
    };
    /// Locked plus phase reset on the prepared onset table: the frequency-domain
    /// equivalent of WSOLA's transient snapping.
    pub const LOCKED_TRANSIENT: Self = Self {
        name: "pvoc_transient",
        fft_size: 2048,
        phase_lock: true,
        transient_reset: true,
        independent_channels: false,
    };
    /// Shorter window: better transients, worse frequency resolution.
    pub const SHORT: Self = Self {
        name: "pvoc_short",
        fft_size: 1024,
        phase_lock: true,
        transient_reset: true,
        independent_channels: false,
    };
    /// Ablation: no phase locking.
    pub const PLAIN: Self = Self {
        name: "pvoc_plain",
        fft_size: 2048,
        phase_lock: false,
        transient_reset: false,
        independent_channels: false,
    };
    /// Ablation: two independent mono instances, as a naive port would do.
    pub const INDEPENDENT: Self = Self {
        name: "pvoc_indep",
        fft_size: 2048,
        phase_lock: true,
        transient_reset: false,
        independent_channels: true,
    };
}

pub struct Pvoc {
    cfg: PvocConfig,
    hs: usize,
    bins: usize,
    fwd: Arc<dyn Fft<f32>>,
    inv: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    spec: [Vec<Complex<f32>>; 2],
    mid: Vec<Complex<f32>>,
    ref_phase: Vec<f32>,
    ref_mag: Vec<f32>,
    window_fn: Vec<f32>,
    cola_ramp: Vec<f32>,
    cola_steady: f32,
    prev_phase: [Vec<f32>; 2],
    out_phase: [Vec<f32>; 2],
    locked: Vec<f32>,
    peak_of: Vec<u32>,
    acc: Vec<[f32; 2]>,
    head: usize,
    ready: Vec<[f32; 2]>,
    ready_pos: usize,
    ready_len: usize,
    analysis_pos: f64,
    prev_analysis: i64,
    emitted: usize,
    ratio: f64,
    first_frame: bool,
    next_onset: usize,
}

fn wrap_pi(x: f32) -> f32 {
    let tau = core::f32::consts::TAU;
    let mut x = x % tau;
    if x > core::f32::consts::PI {
        x -= tau;
    } else if x < -core::f32::consts::PI {
        x += tau;
    }
    x
}

impl Pvoc {
    pub fn new(cfg: PvocConfig, ratio: f64) -> Self {
        let n = cfg.fft_size;
        let hs = n / 4;
        let bins = n / 2 + 1;
        let mut planner = FftPlanner::new();
        let fwd = planner.plan_fft_forward(n);
        let inv = planner.plan_fft_inverse(n);
        let scratch_len = fwd
            .get_inplace_scratch_len()
            .max(inv.get_inplace_scratch_len());
        let window_fn = hann(n);

        // Partial and steady-state sums of w^2 at the synthesis hop, used to
        // normalize the overlap-add including the startup ramp.
        let mut cola_ramp = vec![0.0f32; n];
        for (i, slot) in cola_ramp.iter_mut().enumerate() {
            let mut m = 0usize;
            let mut sum = 0.0f32;
            while m * hs <= i {
                let j = i - m * hs;
                if j < n {
                    sum += window_fn[j] * window_fn[j];
                }
                m += 1;
            }
            *slot = sum;
        }
        let mut cola_steady = 0.0f32;
        {
            let i = (n / 2) as i64;
            let span = (n / hs) as i64;
            let mut m = -span;
            while m <= span {
                let j = i - m * hs as i64;
                if (0..n as i64).contains(&j) {
                    cola_steady += window_fn[j as usize] * window_fn[j as usize];
                }
                m += 1;
            }
        }

        Self {
            cfg,
            hs,
            bins,
            fwd,
            inv,
            scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            spec: [
                vec![Complex::new(0.0, 0.0); n],
                vec![Complex::new(0.0, 0.0); n],
            ],
            mid: vec![Complex::new(0.0, 0.0); n],
            ref_phase: vec![0.0; bins],
            ref_mag: vec![0.0; bins],
            window_fn,
            cola_ramp,
            cola_steady,
            prev_phase: [vec![0.0; bins], vec![0.0; bins]],
            out_phase: [vec![0.0; bins], vec![0.0; bins]],
            locked: vec![0.0; bins],
            peak_of: vec![0; bins],
            acc: vec![[0.0; 2]; n],
            head: 0,
            ready: vec![[0.0; 2]; hs],
            ready_pos: 0,
            ready_len: 0,
            analysis_pos: 0.0,
            prev_analysis: 0,
            emitted: 0,
            ratio,
            first_frame: true,
            next_onset: 0,
        }
    }

    /// Assign each bin to the nearest spectral peak, splitting adjacent peaks
    /// at the magnitude minimum between them.
    fn assign_peaks(&mut self) {
        let bins = self.bins;
        let mag = &self.ref_mag;
        let mut last_peak = 0usize;
        let mut first = true;
        for k in 0..bins {
            let is_peak = mag[k] > 1.0e-9
                && (k < 2 || mag[k] > mag[k - 2])
                && (k < 1 || mag[k] > mag[k - 1])
                && (k + 1 >= bins || mag[k] > mag[k + 1])
                && (k + 2 >= bins || mag[k] > mag[k + 2]);
            if !is_peak {
                continue;
            }
            if first {
                for slot in self.peak_of[..=k].iter_mut() {
                    *slot = k as u32;
                }
                first = false;
            } else {
                let mut split = last_peak;
                let mut lowest = f32::INFINITY;
                for j in last_peak..=k {
                    if mag[j] < lowest {
                        lowest = mag[j];
                        split = j;
                    }
                }
                for slot in self.peak_of[last_peak..split].iter_mut() {
                    *slot = last_peak as u32;
                }
                for slot in self.peak_of[split..=k].iter_mut() {
                    *slot = k as u32;
                }
            }
            last_peak = k;
        }
        if first {
            for slot in self.peak_of.iter_mut() {
                *slot = 0;
            }
        } else {
            for slot in self.peak_of[last_peak..bins].iter_mut() {
                *slot = last_peak as u32;
            }
        }
    }

    fn produce_frame(&mut self, src: &Source<'_>) {
        let n = self.cfg.fft_size;
        let hs = self.hs;
        let bins = self.bins;

        if let Some((ls, le)) = src.loop_bounds {
            if le > ls {
                let span = (le - ls) as f64;
                while self.analysis_pos >= le as f64 {
                    self.analysis_pos -= span;
                    self.prev_analysis -= span as i64;
                    self.next_onset = 0;
                }
            }
        }

        let pos = self.analysis_pos.round() as i64;
        let ha = (pos - self.prev_analysis).max(1) as f32;
        self.prev_analysis = pos;

        for i in 0..n {
            let f = src.frame(pos + i as i64);
            let w = self.window_fn[i];
            self.spec[0][i] = Complex::new(f[0] * w, 0.0);
            self.spec[1][i] = Complex::new(f[1] * w, 0.0);
        }
        self.fwd
            .process_with_scratch(&mut self.spec[0], &mut self.scratch);
        self.fwd
            .process_with_scratch(&mut self.spec[1], &mut self.scratch);
        for i in 0..n {
            self.mid[i] = (self.spec[0][i] + self.spec[1][i]) * 0.5;
        }

        let mut reset = self.first_frame;
        if self.cfg.transient_reset && !src.onsets.is_empty() {
            while self.next_onset < src.onsets.len() && (src.onsets[self.next_onset] as i64) < pos {
                self.next_onset += 1;
            }
            if let Some(&onset) = src.onsets.get(self.next_onset) {
                if (onset as i64) < pos + hs as i64 {
                    reset = true;
                    self.next_onset += 1;
                }
            }
        }

        let channels = if self.cfg.independent_channels { 2 } else { 1 };
        for ch in 0..channels {
            for k in 0..bins {
                let v = if self.cfg.independent_channels {
                    self.spec[ch][k]
                } else {
                    self.mid[k]
                };
                self.ref_phase[k] = v.arg();
                self.ref_mag[k] = v.norm();
            }

            // Phase propagation.
            for k in 0..bins {
                let phi = self.ref_phase[k];
                let omega = core::f32::consts::TAU * k as f32 / n as f32;
                let dphi = wrap_pi(phi - self.prev_phase[ch][k] - ha * omega);
                let freq = omega + dphi / ha;
                if reset {
                    self.out_phase[ch][k] = phi;
                } else {
                    self.out_phase[ch][k] += hs as f32 * freq;
                }
                self.prev_phase[ch][k] = phi;
            }

            if self.cfg.phase_lock {
                self.assign_peaks();
                for k in 0..bins {
                    let p = self.peak_of[k] as usize;
                    self.locked[k] =
                        self.out_phase[ch][p] + (self.ref_phase[k] - self.ref_phase[p]);
                }
            } else {
                self.locked[..bins].copy_from_slice(&self.out_phase[ch][..bins]);
            }

            // Rotate the channel(s) onto the propagated phase. In shared mode
            // the rotation is relative to the mid phase, which leaves each
            // channel's own offset from mid untouched.
            let targets: [usize; 2] = if self.cfg.independent_channels {
                [ch, ch]
            } else {
                [0, 1]
            };
            let target_count = if self.cfg.independent_channels { 1 } else { 2 };
            for &t in targets.iter().take(target_count) {
                for k in 0..bins {
                    let rot = self.locked[k] - self.ref_phase[k];
                    let (s, c) = rot.sin_cos();
                    let v = self.spec[t][k] * Complex::new(c, s);
                    self.spec[t][k] = v;
                    if k > 0 && k < n - k {
                        self.spec[t][n - k] = v.conj();
                    }
                }
            }
        }

        for t in 0..2 {
            self.inv
                .process_with_scratch(&mut self.spec[t], &mut self.scratch);
        }

        let scale = 1.0 / n as f32;
        for i in 0..n {
            let w = self.window_fn[i];
            let a = self.head + i;
            let a = if a >= n { a - n } else { a };
            self.acc[a][0] += self.spec[0][i].re * w * scale;
            self.acc[a][1] += self.spec[1][i].re * w * scale;
        }
        self.first_frame = false;

        for i in 0..hs {
            let a = self.head + i;
            let a = if a >= n { a - n } else { a };
            let out_index = self.emitted + i;
            let norm = if out_index < n {
                self.cola_ramp[out_index].max(1.0e-3)
            } else {
                self.cola_steady
            };
            self.ready[i] = [self.acc[a][0] / norm, self.acc[a][1] / norm];
            self.acc[a] = [0.0, 0.0];
        }
        self.head += hs;
        if self.head >= n {
            self.head -= n;
        }
        self.emitted += hs;
        self.ready_pos = 0;
        self.ready_len = hs;
        self.analysis_pos += hs as f64 / self.ratio;
    }
}

impl Stretcher for Pvoc {
    fn name(&self) -> &'static str {
        self.cfg.name
    }

    fn reset(&mut self, start_frame: usize) {
        for f in self.acc.iter_mut() {
            *f = [0.0, 0.0];
        }
        for ch in 0..2 {
            for v in self.prev_phase[ch].iter_mut() {
                *v = 0.0;
            }
            for v in self.out_phase[ch].iter_mut() {
                *v = 0.0;
            }
        }
        self.head = 0;
        self.ready_pos = 0;
        self.ready_len = 0;
        self.emitted = 0;
        self.analysis_pos = start_frame as f64;
        self.prev_analysis = start_frame as i64 - self.hs as i64;
        self.first_frame = true;
        self.next_onset = 0;
    }

    fn set_ratio(&mut self, ratio: f64) {
        self.ratio = ratio.clamp(0.05, 20.0);
    }

    fn render(&mut self, src: &Source<'_>, out: &mut [[f32; 2]]) {
        let mut written = 0;
        while written < out.len() {
            if self.ready_pos >= self.ready_len {
                self.produce_frame(src);
            }
            let n = (out.len() - written).min(self.ready_len - self.ready_pos);
            out[written..written + n]
                .copy_from_slice(&self.ready[self.ready_pos..self.ready_pos + n]);
            self.ready_pos += n;
            written += n;
        }
    }

    fn latency_frames(&self) -> usize {
        // Zero, for the same reason as WSOLA: the sample is resident, so the
        // analysis window is lookahead into the buffer, not delay in time.
        0
    }

    fn state_bytes(&self) -> usize {
        let c = std::mem::size_of::<Complex<f32>>();
        self.scratch.capacity() * c
            + self.spec[0].capacity() * c
            + self.spec[1].capacity() * c
            + self.mid.capacity() * c
            + self.ref_phase.capacity() * 4
            + self.ref_mag.capacity() * 4
            + self.window_fn.capacity() * 4
            + self.cola_ramp.capacity() * 4
            + self.prev_phase.iter().map(|v| v.capacity() * 4).sum::<usize>()
            + self.out_phase.iter().map(|v| v.capacity() * 4).sum::<usize>()
            + self.locked.capacity() * 4
            + self.peak_of.capacity() * 4
            + self.acc.capacity() * 8
            + self.ready.capacity() * 8
    }
}

/// Frames of sample read ahead of the nominal analysis position.
pub fn lookahead_frames(cfg: &PvocConfig) -> usize {
    cfg.fft_size
}
