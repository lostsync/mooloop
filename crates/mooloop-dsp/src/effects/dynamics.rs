//! Gate, compressor, and limiter.
//!
//! All three share `crate::dynamics` for detection and gain computation, and
//! all three detect on the maximum of the two channels and apply one gain to
//! both — see that module's note on stereo linking.
//!
//! The compressor and limiter use the smoothed-detector topology: attack and
//! release shape how the *level* is tracked, and the static curve then maps
//! that level to a gain. The gate instead ramps its *gain*, because a gate's
//! attack and release describe how fast it opens and shuts, which is not the
//! same statement.

use mooloop_core::{
    CompressorParams, GateParams, LimiterParams, COMP_PARAM_ATTACK_MS, COMP_PARAM_KNEE_DB,
    COMP_PARAM_MAKEUP_DB, COMP_PARAM_RATIO, COMP_PARAM_RELEASE_MS, COMP_PARAM_THRESHOLD_DB,
    GATE_PARAM_ATTACK_MS, GATE_PARAM_HOLD_MS, GATE_PARAM_RANGE_DB, GATE_PARAM_RELEASE_MS,
    GATE_PARAM_THRESHOLD_DB, LIMITER_PARAM_CEILING_DB, LIMITER_PARAM_GAIN_DB,
    LIMITER_PARAM_RELEASE_MS, COMP_PARAM_MIX,
};
use mooloop_core::effect::LIMITER_LATENCY_FRAMES;
use mooloop_core::gain::{db_to_linear_unfloored, linear_to_db_unfloored};

use crate::bus::StereoBus;
use crate::dynamics::{compressor_gain_db, gate_gain_db, time_coeff, EnvelopeFollower};
use crate::event::EventList;
use crate::node::{AudioNode, Discontinuity, DynamicsFrame, ProcessContext};
use crate::smooth::Smoothed;
use super::{process_param_split, RangeProcessor};

/// Time constant for the gain-shaping controls on the compressor and
/// limiter: threshold/ratio/makeup and ceiling/gain all feed straight into a
/// per-sample gain with no smoothing of their own, unlike the gate (which
/// already ramps its output gain toward any new target) or the detector
/// (which smooths the *level*, not these).
const PARAM_SMOOTH_S: f32 = 0.005;

/// Level of the louder channel, which is what every effect here detects on.
fn linked_peak(l: f32, r: f32) -> f32 {
    l.abs().max(r.abs())
}

/// The running block extremes each dynamics effect reports for its display.
///
/// Extremes rather than the last sample of the block: attack times here go
/// down to 0.05 ms, so a device can open, clamp a transient, and be halfway
/// released again inside one buffer. A display fed end-of-block samples
/// would simply never see the moments that matter.
#[derive(Debug, Clone, Copy)]
struct DynamicsBlock {
    /// Loudest detector level of the block, linear, referred to node input.
    detector: f32,
    /// Deepest reduction of the block in dB, always <= 0.
    reduction_db: f32,
}

impl DynamicsBlock {
    fn new() -> Self {
        Self {
            detector: 0.0,
            reduction_db: 0.0,
        }
    }

    fn begin(&mut self) {
        self.detector = 0.0;
        self.reduction_db = 0.0;
    }

    fn observe(&mut self, detector: f32, reduction_db: f32) {
        self.detector = self.detector.max(detector);
        self.reduction_db = self.reduction_db.min(reduction_db);
    }

    fn frame(&self) -> DynamicsFrame {
        DynamicsFrame {
            detector_db: linear_to_db_unfloored(self.detector),
            reduction_db: self.reduction_db,
        }
    }
}

// --- Gate ------------------------------------------------------------------

/// How far under its threshold the gate's level has to fall before it shuts
/// (MOO-142). Two thresholds, open at the knob and shut this far below it,
/// so material sitting on the line holds the gate one way or the other
/// instead of flicking it every cycle.
pub const GATE_HYSTERESIS_DB: f32 = 6.0;
/// Release of the gate's level detector. Its attack is instant, so the gate
/// opens on the first sample over; the release carries the level across the
/// troughs of a waveform, which a bare sample level drops into every half
/// cycle.
const GATE_DETECTOR_RELEASE_MS: f32 = 5.0;

pub struct GateEffect {
    params: GateParams,
    sample_rate: u32,
    /// The level the thresholds are judged against.
    detector: EnvelopeFollower,
    /// Whether the gate is open, which the two thresholds latch.
    open: bool,
    /// Current attenuation in dB, ramped toward the target.
    gain_db: f32,
    /// Samples of hold remaining once the level has fallen under the lower
    /// threshold.
    hold_remaining: u32,
    block: DynamicsBlock,
}

impl GateEffect {
    pub fn new(params: GateParams, sample_rate: u32) -> Self {
        let mut detector = EnvelopeFollower::new();
        detector.set_times(0.0, GATE_DETECTOR_RELEASE_MS, sample_rate);
        Self {
            params,
            sample_rate,
            detector,
            open: false,
            // Start shut, so a gate on a silent channel does not pass a burst
            // before its first ramp.
            gain_db: params.range_db,
            block: DynamicsBlock::new(),
            hold_remaining: 0,
        }
    }

    pub fn params(&self) -> GateParams {
        self.params
    }

    pub fn set_params(&mut self, params: GateParams) {
        self.params = params;
    }

}

impl RangeProcessor for GateEffect {
    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        let GateParams {
            threshold_db,
            attack_ms,
            hold_ms,
            release_ms,
            range_db,
        } = self.params;
        let open_coeff = time_coeff(attack_ms, self.sample_rate);
        let shut_coeff = time_coeff(release_ms, self.sample_rate);
        let hold_samples = (hold_ms * 0.001 * self.sample_rate as f32) as u32;
        let shut_db = threshold_db - GATE_HYSTERESIS_DB;

        for i in start..end {
            let level = self.detector.process(linked_peak(bus.l[i], bus.r[i]));
            let level_db = linear_to_db_unfloored(level);

            if level_db >= threshold_db {
                self.open = true;
                self.hold_remaining = hold_samples;
            } else if level_db < shut_db {
                // Between the two thresholds nothing changes: that band is
                // the hysteresis.
                if self.hold_remaining > 0 {
                    self.hold_remaining -= 1;
                } else {
                    self.open = false;
                }
            }

            let target_db = if self.open { 0.0 } else { range_db.min(0.0) };
            let coeff = if target_db > self.gain_db {
                open_coeff
            } else {
                shut_coeff
            };
            self.gain_db = target_db + coeff * (self.gain_db - target_db);
            self.block.observe(level, self.gain_db);

            let gain = db_to_linear_unfloored(self.gain_db);
            bus.l[i] *= gain;
            bus.r[i] *= gain;
        }
    }

    fn apply_param(&mut self, id: u32, value: f32) {
        match id {
            GATE_PARAM_THRESHOLD_DB => self.params.threshold_db = value.clamp(-80.0, 0.0),
            GATE_PARAM_ATTACK_MS => self.params.attack_ms = value.clamp(0.05, 100.0),
            GATE_PARAM_HOLD_MS => self.params.hold_ms = value.clamp(0.0, 500.0),
            GATE_PARAM_RELEASE_MS => self.params.release_ms = value.clamp(1.0, 2_000.0),
            GATE_PARAM_RANGE_DB => self.params.range_db = value.clamp(-80.0, 0.0),
            _ => {}
        }
    }
}

impl AudioNode for GateEffect {
    /// Nothing here stores audio: the output is the input times a gain, so a
    /// silent block comes out silent whatever the gain computer is doing. What
    /// has to settle before the device can be left alone is the *state*, and
    /// for a reason that is easy to miss — a node frozen mid-release wakes up
    /// still holding the reduction it had when the music stopped, and applies
    /// it to the next transient. Waiting for that state to stop moving is
    /// what makes waking up indistinguishable from never having slept.
    ///
    /// For the gate that is the hold and the shut ramp, and the question is
    /// asked in the strictest form there is: **would another sample of
    /// silence move the ramp at all?**
    ///
    /// Not "is it close to shut". A one-pole ramp in `f32` does not reach its
    /// target — it reaches a fixed point of its own recurrence and stops,
    /// which for the default gate is a fiftieth of a dB short of the full
    /// range. Asking whether the next step is a step, rather than how far it
    /// still has to go, gets the exact answer for nothing: two `exp` calls
    /// once a block, against a gate that would otherwise never sleep.
    fn is_at_rest(&self) -> bool {
        if self.hold_remaining != 0 || self.open || !self.detector.is_at_rest() {
            return false;
        }
        let shut = gate_gain_db(
            linear_to_db_unfloored(0.0),
            self.params.threshold_db,
            self.params.range_db,
        );
        let coeff = if shut > self.gain_db {
            time_coeff(self.params.attack_ms, self.sample_rate)
        } else {
            time_coeff(self.params.release_ms, self.sample_rate)
        };
        shut + coeff * (self.gain_db - shut) == self.gain_db
    }

    fn dynamics_frame(&self) -> Option<DynamicsFrame> {
        Some(self.block.frame())
    }

    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        self.block.begin();
        if ctx.sample_rate != self.sample_rate {
            self.sample_rate = ctx.sample_rate;
            self.detector
                .set_times(0.0, GATE_DETECTOR_RELEASE_MS, self.sample_rate);
        }
        let frames = ctx.frames.min(bus.capacity());
        process_param_split(self, bus, events_in, frames);
    }
}

// --- Compressor ------------------------------------------------------------

pub struct CompressorEffect {
    params: CompressorParams,
    sample_rate: u32,
    detector: EnvelopeFollower,
    threshold_db: Smoothed,
    ratio: Smoothed,
    makeup_db: Smoothed,
    /// `db_to_linear_unfloored(makeup_db.value())`, cached rather than
    /// recomputed every sample: `makeup_db` is automated approximately never
    /// (`reports/fable-2026-09-22.md` finding 2), so this is almost always
    /// the same number. Kept current whenever `makeup_db` is not settled;
    /// reused as-is while it is (see [`Smoothed::is_settled`]).
    makeup: f32,
    /// The parallel balance (MOO-142): linear, like the channel strip's,
    /// so half-wet is half of each rather than the host's equal-power
    /// blend, which runs a compressor and its own input 3 dB hot.
    mix: Smoothed,
    block: DynamicsBlock,
}

impl CompressorEffect {
    pub fn new(params: CompressorParams, sample_rate: u32) -> Self {
        let mut detector = EnvelopeFollower::new();
        detector.set_times(params.attack_ms, params.release_ms, sample_rate);
        let smoothed = |initial| Smoothed::new(initial, PARAM_SMOOTH_S, sample_rate);
        let makeup_db = smoothed(params.makeup_db.clamp(0.0, 24.0));
        let makeup = db_to_linear_unfloored(makeup_db.value());
        Self {
            params,
            sample_rate,
            detector,
            threshold_db: smoothed(params.threshold_db.clamp(-60.0, 0.0)),
            ratio: smoothed(params.ratio.clamp(1.0, 20.0)),
            makeup_db,
            makeup,
            mix: smoothed(params.mix.clamp(0.0, 1.0)),
            block: DynamicsBlock::new(),
        }
    }

    pub fn params(&self) -> CompressorParams {
        self.params
    }

    /// Replace the parameter set wholesale (project load) — jump straight to
    /// the new values, there is nothing to click coming from a fresh load.
    pub fn set_params(&mut self, params: CompressorParams) {
        self.params = params;
        self.detector
            .set_times(params.attack_ms, params.release_ms, self.sample_rate);
        self.threshold_db.reset_to(params.threshold_db.clamp(-60.0, 0.0));
        self.ratio.reset_to(params.ratio.clamp(1.0, 20.0));
        self.makeup_db.reset_to(params.makeup_db.clamp(0.0, 24.0));
        self.mix.reset_to(params.mix.clamp(0.0, 1.0));
        // `reset_to` snaps straight to settled, so nothing downstream would
        // ever recompute the cache for this value without this line.
        self.makeup = db_to_linear_unfloored(self.makeup_db.value());
    }

}

impl RangeProcessor for CompressorEffect {
    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        let knee_db = self.params.knee_db;

        for i in start..end {
            let threshold_db = self.threshold_db.advance();
            let ratio = self.ratio.advance();
            let makeup_db = self.makeup_db.advance();
            // `db_to_linear_unfloored` is a `powf`, paid every sample below
            // for a value that is almost always sitting still: recompute it
            // only while the smoother has not yet settled, and reuse the
            // cached figure the rest of the time. Bit-identical while
            // settled (the cache holds exactly what this call would have
            // produced); a bounded, inaudible lag of at most one sample
            // during the ~5 ms the smoother is still moving otherwise.
            if !self.makeup_db.is_settled() {
                self.makeup = db_to_linear_unfloored(makeup_db);
            }
            let makeup = self.makeup;
            let envelope = self.detector.process(linked_peak(bus.l[i], bus.r[i]));
            let reduction_db =
                compressor_gain_db(linear_to_db_unfloored(envelope), threshold_db, ratio, knee_db);
            self.block.observe(envelope, reduction_db);
            let gain = db_to_linear_unfloored(reduction_db) * makeup;
            // At a mix of one this is the gain exactly, so a compressor
            // that was never given a mix sounds as it always did.
            let mix = self.mix.advance();
            let gain = if mix == 1.0 { gain } else { (1.0 - mix) + gain * mix };
            bus.l[i] *= gain;
            bus.r[i] *= gain;
        }
    }

    fn apply_param(&mut self, id: u32, value: f32) {
        match id {
            COMP_PARAM_THRESHOLD_DB => {
                self.params.threshold_db = value.clamp(-60.0, 0.0);
                self.threshold_db.set_target(self.params.threshold_db);
            }
            COMP_PARAM_RATIO => {
                self.params.ratio = value.clamp(1.0, 20.0);
                self.ratio.set_target(self.params.ratio);
            }
            COMP_PARAM_ATTACK_MS => self.params.attack_ms = value.clamp(0.05, 200.0),
            COMP_PARAM_RELEASE_MS => self.params.release_ms = value.clamp(5.0, 2_000.0),
            COMP_PARAM_KNEE_DB => self.params.knee_db = value.clamp(0.0, 24.0),
            COMP_PARAM_MAKEUP_DB => {
                self.params.makeup_db = value.clamp(0.0, 24.0);
                self.makeup_db.set_target(self.params.makeup_db);
            }
            COMP_PARAM_MIX => {
                self.params.mix = value.clamp(0.0, 1.0);
                self.mix.set_target(self.params.mix);
            }
            _ => {}
        }
        self.detector
            .set_times(self.params.attack_ms, self.params.release_ms, self.sample_rate);
    }
}

impl AudioNode for CompressorEffect {
    /// Nothing here stores audio: the output is the input times a gain, so a
    /// silent block comes out silent whatever the gain computer is doing. What
    /// has to settle before the device can be left alone is the *state*, and
    /// for a reason that is easy to miss — a node frozen mid-release wakes up
    /// still holding the reduction it had when the music stopped, and applies
    /// it to the next transient. Running until the detector has released is
    /// what makes waking up indistinguishable from never having slept.
    ///
    /// The detector is what settles here, and it settles *exactly*: it snaps
    /// the last of the gap rather than decaying through it forever, so a
    /// follower fed silence reaches zero and holds it, and freezing it there
    /// is the same thing as continuing to run it.
    fn is_at_rest(&self) -> bool {
        // The detector, and the three lags feeding the gain computer. A knob
        // still travelling would be frozen where it was, and a detector still
        // releasing would come back holding reduction the music has already
        // stopped asking for.
        self.detector.is_at_rest()
            && self.threshold_db.is_settled()
            && self.ratio.is_settled()
            && self.makeup_db.is_settled()
            && self.mix.is_settled()
    }

    fn dynamics_frame(&self) -> Option<DynamicsFrame> {
        Some(self.block.frame())
    }

    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        self.block.begin();
        if ctx.sample_rate != self.sample_rate {
            self.sample_rate = ctx.sample_rate;
            self.detector.set_times(
                self.params.attack_ms,
                self.params.release_ms,
                self.sample_rate,
            );
            self.threshold_db.set_time(PARAM_SMOOTH_S, ctx.sample_rate);
            self.ratio.set_time(PARAM_SMOOTH_S, ctx.sample_rate);
            self.makeup_db.set_time(PARAM_SMOOTH_S, ctx.sample_rate);
            self.mix.set_time(PARAM_SMOOTH_S, ctx.sample_rate);
        }
        let frames = ctx.frames.min(bus.capacity());
        process_param_split(self, bus, events_in, frames);
    }
}

// --- Limiter ---------------------------------------------------------------

/// Taps per phase of the true-peak interpolator. Twelve, as in ITU-R BS.1770's
/// 4x reference filter: enough to find an inter-sample peak to a fraction of
/// a decibel on programme material.
const TRUE_PEAK_TAPS: usize = 12;
/// The interpolated points lie between the samples this far and one less
/// back from the newest, half the filter's length.
const TRUE_PEAK_DELAY: usize = TRUE_PEAK_TAPS / 2;
/// Oversampling factor of the true-peak detector.
const TRUE_PEAK_FACTOR: usize = 4;

/// Frames of lookahead the gain computer sees before a sample is output. The
/// declared latency is this plus what the true-peak interpolator needs to see
/// past a point; see [`LimiterEffect`].
const LOOKAHEAD: usize = LIMITER_LATENCY_FRAMES as usize + 1 - TRUE_PEAK_DELAY;
/// Width of the sliding minimum: the lookahead, the interpolator's reach, and
/// one frame for the sample after an inter-sample peak.
const MIN_WINDOW: usize = LOOKAHEAD + TRUE_PEAK_DELAY + 1;

/// A 4x windowed-sinc interpolator over one channel, for finding the peaks a
/// DAC will draw between samples.
#[derive(Clone, Copy)]
struct TruePeak {
    /// The last [`TRUE_PEAK_TAPS`] samples, oldest first.
    history: [f32; TRUE_PEAK_TAPS],
}

/// The three in-between phases' kernels, `[phase - 1][tap]`, built once.
struct TruePeakKernels([[f32; TRUE_PEAK_TAPS]; TRUE_PEAK_FACTOR - 1]);

impl TruePeakKernels {
    fn new() -> Self {
        let mut kernels = [[0.0f32; TRUE_PEAK_TAPS]; TRUE_PEAK_FACTOR - 1];
        let half = TRUE_PEAK_DELAY as f64;
        for (phase, kernel) in kernels.iter_mut().enumerate() {
            let fraction = (phase + 1) as f64 / TRUE_PEAK_FACTOR as f64;
            let mut sum = 0.0f64;
            for (tap, coeff) in kernel.iter_mut().enumerate() {
                // Distance from this tap's sample to the point being found,
                // which lies `fraction` of the way past the sample
                // `TRUE_PEAK_DELAY` back from the newest.
                let distance = (half - 1.0) + fraction - tap as f64;
                let sinc = if distance == 0.0 {
                    1.0
                } else {
                    let x = core::f64::consts::PI * distance;
                    x.sin() / x
                };
                // Blackman, over the kernel's reach.
                let w = distance / half;
                let window = 0.42
                    + 0.5 * (core::f64::consts::PI * w).cos()
                    + 0.08 * (2.0 * core::f64::consts::PI * w).cos();
                let value = sinc * window.max(0.0);
                *coeff = value as f32;
                sum += value;
            }
            // Unity at DC, so a held level reads as itself.
            for coeff in kernel.iter_mut() {
                *coeff = (*coeff as f64 / sum) as f32;
            }
        }
        Self(kernels)
    }
}

impl TruePeak {
    const fn new() -> Self {
        Self {
            history: [0.0; TRUE_PEAK_TAPS],
        }
    }

    /// Take a sample and return the largest magnitude among it and the three
    /// points between the two samples `TRUE_PEAK_DELAY` back.
    fn push(&mut self, sample: f32, kernels: &TruePeakKernels) -> f32 {
        self.history.copy_within(1.., 0);
        self.history[TRUE_PEAK_TAPS - 1] = sample;
        let mut peak = sample.abs();
        for kernel in &kernels.0 {
            let value: f32 = kernel
                .iter()
                .zip(&self.history)
                .map(|(coeff, sample)| coeff * sample)
                .sum();
            peak = peak.max(value.abs());
        }
        peak
    }
}

/// A lookahead true-peak limiter.
///
/// **Nothing it outputs peaks over the ceiling, between samples included,
/// without the clamp** (MOO-142). It used to be a 0.05 ms detector with a
/// hard clamp doing the real work on every transient: a clipper with a
/// release. The mixer has been latency compensated since 2026-09-05, so it
/// now looks ahead and declares the delay ([`LIMITER_LATENCY_FRAMES`]).
///
/// The gain computer, per frame:
///
/// 1. Each input frame's **true peak** -- the sample and the three 4x
///    points between two samples [`TRUE_PEAK_DELAY`] back -- sets the gain
///    that frame would need.
/// 2. A **sliding minimum** over [`MIN_WINDOW`] frames holds the deepest need
///    for as long as any sample it covers is still to come out.
/// 3. The **release** lets the held gain rise through a one-pole; it can
///    only ever lag *below* the minimum, never above it.
/// 4. A **moving average** over [`LOOKAHEAD`] frames turns the steps into a
///    ramp that has arrived by the time the peak comes out of the delay.
///
/// Every sample's gain is therefore at or below what every peak it
/// contributes to needs, which is the guarantee. The clamp stays behind it
/// as a backstop for what the 12-tap interpolator cannot see.
pub struct LimiterEffect {
    params: LimiterParams,
    sample_rate: u32,
    ceiling_db: Smoothed,
    gain_db: Smoothed,
    kernels: Box<TruePeakKernels>,
    peak_l: TruePeak,
    peak_r: TruePeak,
    /// The audio, held back by [`LIMITER_LATENCY_FRAMES`].
    delay_l: Box<[f32; LIMITER_LATENCY_FRAMES as usize]>,
    delay_r: Box<[f32; LIMITER_LATENCY_FRAMES as usize]>,
    delay_pos: usize,
    /// The sliding minimum's monotonic queue: values ascending from the
    /// front, with the frame each arrived on. A ring of [`MIN_WINDOW`].
    min_values: Box<[f32; MIN_WINDOW]>,
    min_frames: Box<[u64; MIN_WINDOW]>,
    min_front: usize,
    min_len: usize,
    /// The released gain, `<=` the sliding minimum.
    released: f32,
    release_coeff: f32,
    /// The moving average: the last [`LOOKAHEAD`] released gains and their
    /// sum.
    average: Box<[f32; LOOKAHEAD]>,
    average_pos: usize,
    average_sum: f64,
    /// Consecutive frames the released gain has been exactly one. Once the
    /// whole average window is, the sum is snapped back to exact so a quiet
    /// passage comes out bit-identical to its input.
    unity_run: usize,
    /// Consecutive silent input frames: the delay line is empty once it
    /// covers the latency.
    silent_run: usize,
    frame: u64,
    /// The clamp after the gain. Off only in the test that proves the gain
    /// computer holds the ceiling on its own.
    backstop: bool,
    block: DynamicsBlock,
}

impl LimiterEffect {
    pub fn new(params: LimiterParams, sample_rate: u32) -> Self {
        let smoothed = |initial| Smoothed::new(initial, PARAM_SMOOTH_S, sample_rate);
        Self {
            params,
            sample_rate,
            ceiling_db: smoothed(params.ceiling_db.clamp(-24.0, 0.0)),
            gain_db: smoothed(params.gain_db.clamp(0.0, 24.0)),
            kernels: Box::new(TruePeakKernels::new()),
            peak_l: TruePeak::new(),
            peak_r: TruePeak::new(),
            delay_l: Box::new([0.0; LIMITER_LATENCY_FRAMES as usize]),
            delay_r: Box::new([0.0; LIMITER_LATENCY_FRAMES as usize]),
            delay_pos: 0,
            min_values: Box::new([1.0; MIN_WINDOW]),
            min_frames: Box::new([0; MIN_WINDOW]),
            min_front: 0,
            min_len: 0,
            released: 1.0,
            release_coeff: time_coeff(params.release_ms, sample_rate),
            average: Box::new([1.0; LOOKAHEAD]),
            average_pos: 0,
            average_sum: LOOKAHEAD as f64,
            unity_run: LOOKAHEAD,
            silent_run: LIMITER_LATENCY_FRAMES as usize,
            frame: 0,
            backstop: true,
            block: DynamicsBlock::new(),
        }
    }

    pub fn params(&self) -> LimiterParams {
        self.params
    }

    /// Replace the parameter set wholesale (project load) — jump straight to
    /// the new values, there is nothing to click coming from a fresh load.
    pub fn set_params(&mut self, params: LimiterParams) {
        self.params = params;
        self.release_coeff = time_coeff(params.release_ms, self.sample_rate);
        self.ceiling_db.reset_to(params.ceiling_db.clamp(-24.0, 0.0));
        self.gain_db.reset_to(params.gain_db.clamp(0.0, 24.0));
    }

    /// Empty the delay line and let go of any reduction: the audio it held
    /// belongs somewhere the transport has left.
    fn clear(&mut self) {
        self.peak_l = TruePeak::new();
        self.peak_r = TruePeak::new();
        self.delay_l.fill(0.0);
        self.delay_r.fill(0.0);
        self.min_len = 0;
        self.released = 1.0;
        self.average.fill(1.0);
        self.average_sum = LOOKAHEAD as f64;
        self.unity_run = LOOKAHEAD;
        self.silent_run = LIMITER_LATENCY_FRAMES as usize;
    }

    /// Admit `need` to the sliding minimum at the current frame and return
    /// the minimum over the window.
    fn sliding_min(&mut self, need: f32) -> f32 {
        // Drop from the back everything no smaller: it can never be the
        // minimum again while `need` is in the window.
        while self.min_len > 0 {
            let back = (self.min_front + self.min_len - 1) % MIN_WINDOW;
            if self.min_values[back] < need {
                break;
            }
            self.min_len -= 1;
        }
        let slot = (self.min_front + self.min_len) % MIN_WINDOW;
        self.min_values[slot] = need;
        self.min_frames[slot] = self.frame;
        self.min_len += 1;
        // And from the front what has left the window.
        while self.min_frames[self.min_front] + (MIN_WINDOW as u64) <= self.frame {
            self.min_front = (self.min_front + 1) % MIN_WINDOW;
            self.min_len -= 1;
        }
        self.min_values[self.min_front]
    }
}

impl RangeProcessor for LimiterEffect {
    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        for i in start..end {
            let ceiling_db = self.ceiling_db.advance();
            let drive = db_to_linear_unfloored(self.gain_db.advance());
            let ceiling = db_to_linear_unfloored(ceiling_db);

            let (l, r) = (bus.l[i] * drive, bus.r[i] * drive);

            let peak = self
                .peak_l
                .push(l, &self.kernels)
                .max(self.peak_r.push(r, &self.kernels));
            let need = if peak > ceiling { ceiling / peak } else { 1.0 };
            let held = self.sliding_min(need);
            self.released = if held <= self.released {
                held
            } else {
                let rising = held + self.release_coeff * (self.released - held);
                // The one-pole never lands; a gap this small is unity.
                if held - rising < 1.0e-6 {
                    held
                } else {
                    rising
                }
            };

            let leaving = self.average[self.average_pos];
            self.average[self.average_pos] = self.released;
            self.average_pos = (self.average_pos + 1) % LOOKAHEAD;
            self.average_sum += f64::from(self.released) - f64::from(leaving);
            self.unity_run = if self.released == 1.0 {
                self.unity_run.saturating_add(1)
            } else {
                0
            };
            let gain = if self.unity_run >= LOOKAHEAD {
                self.average_sum = LOOKAHEAD as f64;
                1.0
            } else {
                ((self.average_sum / LOOKAHEAD as f64) as f32).min(1.0)
            };

            let out_l = self.delay_l[self.delay_pos];
            let out_r = self.delay_r[self.delay_pos];
            self.delay_l[self.delay_pos] = l;
            self.delay_r[self.delay_pos] = r;
            self.delay_pos = (self.delay_pos + 1) % LIMITER_LATENCY_FRAMES as usize;
            self.silent_run = if l == 0.0 && r == 0.0 {
                self.silent_run.saturating_add(1)
            } else {
                0
            };
            self.frame += 1;

            // Referred back across the drive: the display's axis is this
            // node's input.
            self.block.observe(
                peak / drive.max(f32::MIN_POSITIVE),
                linear_to_db_unfloored(gain),
            );
            let (out_l, out_r) = (out_l * gain, out_r * gain);
            if self.backstop {
                bus.l[i] = out_l.clamp(-ceiling, ceiling);
                bus.r[i] = out_r.clamp(-ceiling, ceiling);
            } else {
                bus.l[i] = out_l;
                bus.r[i] = out_r;
            }
        }
    }

    fn apply_param(&mut self, id: u32, value: f32) {
        match id {
            LIMITER_PARAM_CEILING_DB => {
                self.params.ceiling_db = value.clamp(-24.0, 0.0);
                self.ceiling_db.set_target(self.params.ceiling_db);
            }
            LIMITER_PARAM_RELEASE_MS => {
                self.params.release_ms = value.clamp(1.0, 500.0);
                self.release_coeff = time_coeff(self.params.release_ms, self.sample_rate);
            }
            LIMITER_PARAM_GAIN_DB => {
                self.params.gain_db = value.clamp(0.0, 24.0);
                self.gain_db.set_target(self.params.gain_db);
            }
            _ => {}
        }
    }
}

impl AudioNode for LimiterEffect {
    /// The lookahead delay: the mixer compensates it, and the host delays the
    /// device's dry copy by it.
    fn latency_frames(&self) -> u32 {
        LIMITER_LATENCY_FRAMES
    }

    /// At rest once the delay line has emptied and the gain is back at unity
    /// with nothing travelling: a node frozen mid-release would wake up
    /// holding reduction the music has stopped asking for, and one frozen
    /// with audio in its delay would play it late.
    fn is_at_rest(&self) -> bool {
        self.silent_run >= LIMITER_LATENCY_FRAMES as usize
            && self.unity_run >= LOOKAHEAD
            && self.ceiling_db.is_settled()
            && self.gain_db.is_settled()
    }

    /// A seek empties the delay line: what it holds is the sound of where the
    /// transport was. Not on a loop fold or a program change, which
    /// [`Discontinuity::invalidates_tails`] says leave a tail alone.
    fn on_discontinuity(&mut self, kind: Discontinuity) {
        if kind.invalidates_tails() {
            self.clear();
        }
    }

    fn dynamics_frame(&self) -> Option<DynamicsFrame> {
        Some(self.block.frame())
    }

    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        self.block.begin();
        if ctx.sample_rate != self.sample_rate {
            self.sample_rate = ctx.sample_rate;
            self.release_coeff = time_coeff(self.params.release_ms, self.sample_rate);
            self.ceiling_db.set_time(PARAM_SMOOTH_S, ctx.sample_rate);
            self.gain_db.set_time(PARAM_SMOOTH_S, ctx.sample_rate);
        }
        let frames = ctx.frames.min(bus.capacity());
        process_param_split(self, bus, events_in, frames);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::peak;
    use crate::event::{Event, TimedEvent};

    const SR: u32 = 48_000;

    fn context(frames: usize) -> ProcessContext {
        ProcessContext {
            sample_rate: SR,
            frames,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        }
    }

    fn tone_bus(frames: usize, amplitude: f32) -> StereoBus {
        let mut bus = StereoBus::with_capacity(frames);
        for i in 0..frames {
            let s = (i as f32 / SR as f32 * 220.0 * core::f32::consts::TAU).sin() * amplitude;
            bus.l[i] = s;
            bus.r[i] = s;
        }
        bus
    }


    #[test]
    fn the_gate_passes_signal_above_its_threshold() {
        let frames = 24_000;
        let mut bus = tone_bus(frames, 0.5); // about -6 dB
        let mut effect = GateEffect::new(
            GateParams {
                threshold_db: -40.0,
                attack_ms: 0.5,
                ..GateParams::default()
            },
            SR,
        );
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);
        // Past the opening ramp the signal should be essentially untouched.
        assert!(
            peak(&bus.l[frames / 2..]) > 0.49,
            "gate did not open: {}",
            peak(&bus.l[frames / 2..])
        );
    }

    #[test]
    fn the_gate_shuts_on_signal_below_its_threshold() {
        let frames = 48_000;
        let mut bus = tone_bus(frames, 0.001); // about -60 dB
        let mut effect = GateEffect::new(
            GateParams {
                threshold_db: -40.0,
                release_ms: 5.0,
                hold_ms: 0.0,
                range_db: -80.0,
                ..GateParams::default()
            },
            SR,
        );
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);
        assert!(
            peak(&bus.l[frames / 2..]) < 1e-5,
            "gate did not shut: {}",
            peak(&bus.l[frames / 2..])
        );
    }

    #[test]
    fn the_gate_hold_keeps_it_open_through_a_brief_dip() {
        let frames = 24_000;
        let dip_start = 8_000;
        // 50 ms: longer than the level detector takes to fall 40 dB, so
        // without a hold the gate has shut before the dip ends.
        let dip_len = 2_400;
        let build = |hold_ms: f32| {
            let mut bus = tone_bus(frames, 0.5);
            for i in dip_start..dip_start + dip_len {
                bus.l[i] = 0.0;
                bus.r[i] = 0.0;
            }
            // Loud again right after the dip.
            let mut effect = GateEffect::new(
                GateParams {
                    threshold_db: -40.0,
                    attack_ms: 0.5,
                    release_ms: 5.0,
                    hold_ms,
                    range_db: -80.0,
                },
                SR,
            );
            effect.process(&context(frames), &mut bus, &EventList::empty(), None);
            // Level immediately after the dip, before a slow attack could
            // have re-opened the gate.
            peak(&bus.l[dip_start + dip_len..dip_start + dip_len + 32])
        };
        let without_hold = build(0.0);
        let with_hold = build(100.0);
        assert!(
            with_hold > without_hold * 2.0,
            "hold did not keep the gate open: {with_hold} vs {without_hold}"
        );
    }

    #[test]
    fn the_compressor_reduces_gain_above_the_threshold() {
        let frames = 48_000;
        let quiet_in = 0.02; // about -34 dB, below threshold
        let loud_in = 0.5; // about -6 dB, above threshold

        let run = |amplitude: f32| {
            let mut bus = tone_bus(frames, amplitude);
            let mut effect = CompressorEffect::new(
                CompressorParams {
                    threshold_db: -18.0,
                    ratio: 4.0,
                    attack_ms: 1.0,
                    release_ms: 50.0,
                    knee_db: 0.0,
                    makeup_db: 0.0,
                    mix: 1.0,
                },
                SR,
            );
            effect.process(&context(frames), &mut bus, &EventList::empty(), None);
            peak(&bus.l[frames / 2..])
        };

        let quiet_out = run(quiet_in);
        let loud_out = run(loud_in);
        // Below the threshold: untouched.
        assert!(
            (quiet_out / quiet_in - 1.0).abs() < 0.02,
            "quiet signal was altered: {quiet_in} -> {quiet_out}"
        );
        // Above it: 12 dB over at 4:1 should come out about 9 dB down.
        let reduction_db = linear_to_db_unfloored(loud_out) - linear_to_db_unfloored(loud_in);
        assert!(
            (reduction_db + 9.0).abs() < 1.5,
            "expected about -9 dB, got {reduction_db}"
        );
    }

    #[test]
    fn compressor_makeup_restores_level() {
        let frames = 48_000;
        let run = |makeup_db: f32| {
            let mut bus = tone_bus(frames, 0.5);
            let mut effect = CompressorEffect::new(
                CompressorParams {
                    threshold_db: -18.0,
                    ratio: 4.0,
                    attack_ms: 1.0,
                    release_ms: 50.0,
                    knee_db: 0.0,
                    makeup_db,
                    mix: 1.0,
                },
                SR,
            );
            effect.process(&context(frames), &mut bus, &EventList::empty(), None);
            peak(&bus.l[frames / 2..])
        };
        let plain = run(0.0);
        let made_up = run(9.0);
        let lift_db = linear_to_db_unfloored(made_up) - linear_to_db_unfloored(plain);
        assert!(
            (lift_db - 9.0).abs() < 0.5,
            "makeup should add 9 dB, added {lift_db}"
        );
    }

    #[test]
    fn a_higher_ratio_compresses_harder() {
        let frames = 48_000;
        let run = |ratio: f32| {
            let mut bus = tone_bus(frames, 0.5);
            let mut effect = CompressorEffect::new(
                CompressorParams {
                    threshold_db: -18.0,
                    ratio,
                    attack_ms: 1.0,
                    release_ms: 50.0,
                    knee_db: 0.0,
                    makeup_db: 0.0,
                    mix: 1.0,
                },
                SR,
            );
            effect.process(&context(frames), &mut bus, &EventList::empty(), None);
            peak(&bus.l[frames / 2..])
        };
        let gentle = run(2.0);
        let hard = run(20.0);
        assert!(hard < gentle, "20:1 {hard} should beat 2:1 {gentle}");
        // 1:1 is a bypass.
        let unity = run(1.0);
        assert!((unity - 0.5).abs() < 0.02, "1:1 altered the signal: {unity}");
    }

    #[test]
    fn the_limiter_holds_the_ceiling() {
        for ceiling_db in [-0.3f32, -6.0, -12.0] {
            let frames = 48_000;
            let mut bus = tone_bus(frames, 0.9);
            let mut effect = LimiterEffect::new(
                LimiterParams {
                    ceiling_db,
                    release_ms: 50.0,
                    gain_db: 18.0,
                },
                SR,
            );
            effect.process(&context(frames), &mut bus, &EventList::empty(), None);
            let ceiling = db_to_linear_unfloored(ceiling_db);
            let out = peak(&bus.l[..frames]);
            assert!(
                out <= ceiling + 1e-4,
                "ceiling {ceiling_db} dB breached: {out} > {ceiling}"
            );
            // And it should actually be working the signal, not just muting.
            assert!(
                peak(&bus.l[frames / 2..]) > ceiling * 0.5,
                "limiter over-attenuated at {ceiling_db} dB"
            );
        }
    }

    #[test]
    fn the_limiter_leaves_quiet_signal_alone() {
        let frames = 24_000;
        let mut bus = tone_bus(frames, 0.1);
        let reference = bus.l[..frames].to_vec();
        let mut effect = LimiterEffect::new(
            LimiterParams {
                ceiling_db: 0.0,
                release_ms: 50.0,
                gain_db: 0.0,
            },
            SR,
        );
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);
        // Late by the lookahead the limiter declares, and otherwise the
        // signal exactly.
        let latency = LIMITER_LATENCY_FRAMES as usize;
        assert!(bus.l[..latency].iter().all(|sample| *sample == 0.0));
        for (i, expected) in reference[..frames - latency].iter().enumerate() {
            assert_eq!(bus.l[i + latency], *expected, "quiet signal altered at {i}");
        }
    }

    #[test]
    fn param_events_take_effect_mid_block() {
        let frames = 48_000;
        let mut bus = tone_bus(frames, 0.5);
        let mut effect = CompressorEffect::new(
            CompressorParams {
                threshold_db: 0.0,
                ratio: 1.0,
                attack_ms: 1.0,
                release_ms: 20.0,
                knee_db: 0.0,
                makeup_db: 0.0,
                mix: 1.0,
            },
            SR,
        );
        let mut events = EventList::empty();
        for (id, value) in [
            (COMP_PARAM_THRESHOLD_DB, -30.0f32),
            (COMP_PARAM_RATIO, 20.0f32),
        ] {
            assert!(events.push(TimedEvent {
                offset: (frames / 2) as u32,
                event: Event::ParamValue { id, value },
            }));
        }
        effect.process(&context(frames), &mut bus, &events, None);
        let before = peak(&bus.l[frames / 4..frames / 2]);
        let after = peak(&bus.l[3 * frames / 4..]);
        assert!(
            after < before * 0.5,
            "compression did not engage: {before} then {after}"
        );
    }

    #[test]
    fn compressor_makeup_change_mid_block_does_not_click() {
        let frames = 24_000;
        let mut bus = tone_bus(frames, 0.2);
        let mut effect = CompressorEffect::new(
            CompressorParams {
                threshold_db: 0.0,
                ratio: 1.0,
                attack_ms: 1.0,
                release_ms: 20.0,
                knee_db: 0.0,
                makeup_db: 0.0,
                mix: 1.0,
            },
            SR,
        );
        let mut events = EventList::empty();
        assert!(events.push(TimedEvent {
            offset: (frames / 2) as u32,
            event: Event::ParamValue {
                id: COMP_PARAM_MAKEUP_DB,
                value: 18.0,
            },
        }));
        effect.process(&context(frames), &mut bus, &events, None);
        let max_step = crate::testkit::max_step(&bus.l[..frames]);
        assert!(
            max_step < 0.1,
            "makeup change left a discontinuity of {max_step}"
        );
    }

    #[test]
    fn limiter_gain_change_mid_block_does_not_click() {
        let frames = 24_000;
        let mut bus = tone_bus(frames, 0.1);
        let mut effect = LimiterEffect::new(
            LimiterParams {
                ceiling_db: 0.0,
                release_ms: 20.0,
                gain_db: 0.0,
            },
            SR,
        );
        let mut events = EventList::empty();
        assert!(events.push(TimedEvent {
            offset: (frames / 2) as u32,
            event: Event::ParamValue {
                id: LIMITER_PARAM_GAIN_DB,
                value: 18.0,
            },
        }));
        effect.process(&context(frames), &mut bus, &events, None);
        let max_step = crate::testkit::max_step(&bus.l[..frames]);
        assert!(
            max_step < 0.1,
            "gain change left a discontinuity of {max_step}"
        );
    }

    #[test]
    fn the_compressors_reported_frame_matches_the_gain_it_applied() {
        let frames = 48_000;
        let amplitude = 0.5; // about -6 dB, 12 dB over the threshold
        let mut bus = tone_bus(frames, amplitude);
        let mut effect = CompressorEffect::new(
            CompressorParams {
                threshold_db: -18.0,
                ratio: 4.0,
                attack_ms: 1.0,
                release_ms: 50.0,
                knee_db: 0.0,
                makeup_db: 0.0,
                mix: 1.0,
            },
            SR,
        );
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);
        let frame = effect.dynamics_frame().expect("a compressor reduces gain");

        // The detector settles at the tone's own level, so the display's dot
        // lands where the signal really is on the input axis.
        assert!(
            (frame.detector_db - linear_to_db_unfloored(amplitude)).abs() < 1.0,
            "detector read {} for a {} dB tone",
            frame.detector_db,
            linear_to_db_unfloored(amplitude)
        );
        // 12 dB over at 4:1 is 9 dB of reduction, which is also what the
        // audio lost -- the reported number is the applied one, not a
        // separate estimate that could drift from it.
        assert!(
            (frame.reduction_db + 9.0).abs() < 1.5,
            "reported {} dB of reduction",
            frame.reduction_db
        );
        let measured_db = linear_to_db_unfloored(peak(&bus.l[frames / 2..])) - linear_to_db_unfloored(amplitude);
        assert!(
            (frame.reduction_db - measured_db).abs() < 1.0,
            "reported {} dB but the audio lost {measured_db} dB",
            frame.reduction_db
        );
    }

    #[test]
    fn a_reported_frame_covers_the_whole_block_not_just_its_last_sample() {
        // One loud burst at the top of the block, silence for the rest. With
        // a 500 ms release the device is still well into its recovery at the
        // end, but a display fed the final sample would understate what it
        // did; the frame must carry the extreme.
        let frames = 4_096;
        let mut bus = StereoBus::with_capacity(frames);
        for i in 0..64 {
            bus.l[i] = 0.9;
            bus.r[i] = 0.9;
        }
        let mut effect = LimiterEffect::new(
            LimiterParams {
                ceiling_db: -12.0,
                release_ms: 500.0,
                gain_db: 0.0,
            },
            SR,
        );
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);
        let frame = effect.dynamics_frame().expect("a limiter reduces gain");
        assert!(
            frame.reduction_db < -6.0,
            "a burst 11 dB over the ceiling reported only {} dB",
            frame.reduction_db
        );
        assert!(
            frame.detector_db > -3.0,
            "detector missed the burst, reading {}",
            frame.detector_db
        );
    }

    #[test]
    fn a_limiters_reported_level_is_referred_to_its_input() {
        // The detector sits after the input gain, but the display plots
        // against this node's input, so 12 dB of drive must not move the dot.
        let frames = 8_192;
        let run = |gain_db: f32| {
            let mut bus = tone_bus(frames, 0.1);
            let mut effect = LimiterEffect::new(
                LimiterParams {
                    ceiling_db: -1.0,
                    release_ms: 50.0,
                    gain_db,
                },
                SR,
            );
            effect.process(&context(frames), &mut bus, &EventList::empty(), None);
            effect.dynamics_frame().expect("a limiter reduces gain").detector_db
        };
        let plain = run(0.0);
        let driven = run(12.0);
        assert!(
            (plain - driven).abs() < 0.5,
            "input gain moved the reported level from {plain} to {driven}"
        );
    }

    #[test]
    fn every_dynamics_effect_reports_a_resting_frame_for_silence() {
        let frames = 4_096;
        let mut nodes: Vec<Box<dyn AudioNode>> = vec![
            Box::new(GateEffect::new(GateParams::default(), SR)),
            Box::new(CompressorEffect::new(CompressorParams::default(), SR)),
            Box::new(LimiterEffect::new(LimiterParams::default(), SR)),
        ];
        for node in nodes.iter_mut() {
            let mut bus = StereoBus::with_capacity(frames);
            node.process(&context(frames), &mut bus, &EventList::empty(), None);
            let frame = node.dynamics_frame().expect("all three reduce gain");
            assert!(
                frame.detector_db <= -100.0,
                "silence detected at {}",
                frame.detector_db
            );
            // A gate's whole job is to shut on silence, so it alone is
            // expected to be reducing here; none of them may report a lift.
            assert!(
                frame.reduction_db <= 0.0,
                "reported a gain increase: {}",
                frame.reduction_db
            );
        }
    }

    #[test]
    fn every_dynamics_effect_leaves_silence_silent() {
        let frames = 4_096;
        let mut nodes: Vec<Box<dyn AudioNode>> = vec![
            Box::new(GateEffect::new(GateParams::default(), SR)),
            Box::new(CompressorEffect::new(CompressorParams::default(), SR)),
            Box::new(LimiterEffect::new(LimiterParams::default(), SR)),
        ];
        for node in nodes.iter_mut() {
            let mut bus = StereoBus::with_capacity(frames);
            node.process(&context(frames), &mut bus, &EventList::empty(), None);
            for i in 0..frames {
                assert!(
                    bus.l[i].abs() < 1e-9 && bus.l[i].is_finite(),
                    "silence became {} at {i}",
                    bus.l[i]
                );
            }
        }
    }

    /// The largest magnitude of `samples` reconstructed at 4x, with a long
    /// windowed sinc of its own: the measurement, deliberately not the
    /// limiter's twelve-tap detector.
    fn true_peak_4x(samples: &[f32]) -> f32 {
        const HALF: isize = 32;
        let mut peak = crate::testkit::peak(samples);
        for k in 0..samples.len() as isize - 1 {
            for phase in 1..4 {
                let t = k as f64 + phase as f64 / 4.0;
                let mut value = 0.0f64;
                for j in (k - HALF + 1)..=(k + HALF) {
                    if j < 0 || j >= samples.len() as isize {
                        continue;
                    }
                    let d = t - j as f64;
                    let x = core::f64::consts::PI * d;
                    let sinc = x.sin() / x;
                    let w = d / HALF as f64;
                    let window = 0.42
                        + 0.5 * (core::f64::consts::PI * w).cos()
                        + 0.08 * (2.0 * core::f64::consts::PI * w).cos();
                    value += f64::from(samples[j as usize]) * sinc * window;
                }
                peak = peak.max(value.abs() as f32);
            }
        }
        peak
    }

    /// MOO-142's done-when: a fast transient through the limiter, with the
    /// clamp behind it switched off, comes out with no 4x-oversampled peak
    /// over the ceiling. Before, the detector's 0.05 ms attack let every
    /// transient's first samples through and the clamp squared them off.
    #[test]
    fn a_transient_comes_out_under_the_ceiling_between_samples_too_without_the_clamp() {
        for sr in crate::testkit::RATES {
            let frames = sr as usize / 2;
            let mut bus = StereoBus::with_capacity(frames);
            // Quiet programme, then a full-scale burst with its energy near
            // a quarter of the rate, where samples straddle the crests and
            // the true peak sits well above the sample peak.
            let burst_at = frames / 3;
            for i in 0..frames {
                let t = i as f32 / sr as f32;
                let quiet = 0.05 * (t * 220.0 * core::f32::consts::TAU).sin();
                let burst = if (burst_at..burst_at + 400).contains(&i) {
                    let phase = (i - burst_at) as f32;
                    0.95 * (phase * core::f32::consts::FRAC_PI_2 + 0.785).sin()
                        + 0.4 * (phase * 0.37).sin()
                } else {
                    0.0
                };
                bus.l[i] = quiet + burst;
                bus.r[i] = quiet - burst * 0.5;
            }
            for ceiling_db in [-0.3f32, -3.0, -12.0] {
                let mut run = StereoBus::with_capacity(frames);
                run.l[..frames].copy_from_slice(&bus.l[..frames]);
                run.r[..frames].copy_from_slice(&bus.r[..frames]);
                let mut limiter = LimiterEffect::new(
                    LimiterParams {
                        ceiling_db,
                        release_ms: 50.0,
                        gain_db: 6.0,
                    },
                    sr,
                );
                limiter.backstop = false;
                limiter.process(
                    &ProcessContext {
                        sample_rate: sr,
                        ..context(frames)
                    },
                    &mut run,
                    &EventList::empty(),
                    None,
                );
                let ceiling = db_to_linear_unfloored(ceiling_db);
                for (side, samples) in [("left", &run.l[..frames]), ("right", &run.r[..frames])] {
                    let over = true_peak_4x(samples);
                    assert!(
                        over <= ceiling * 1.005,
                        "{sr} Hz, ceiling {ceiling_db} dB: the {side} side's true peak is {:.2} dB",
                        crate::testkit::db(over)
                    );
                }
            }
        }
    }

    /// MOO-142's done-when: material sitting on the threshold does not make
    /// the gate chatter. A tone whose level wobbles a decibel either side of
    /// the threshold, with no hold at all, used to open and shut on every
    /// crossing; with a second, lower threshold to shut at, it opens once
    /// and stays open.
    #[test]
    fn material_sitting_on_the_threshold_does_not_make_the_gate_chatter() {
        let frames = SR as usize * 2;
        let threshold_db = -30.0f32;
        let mut bus = StereoBus::with_capacity(frames);
        for i in 0..frames {
            let t = i as f32 / SR as f32;
            // +-1 dB around the threshold, four times a second.
            let level_db = threshold_db + (t * 4.0 * core::f32::consts::TAU).sin();
            let s = (t * 220.0 * core::f32::consts::TAU).sin()
                * db_to_linear_unfloored(level_db);
            bus.l[i] = s;
            bus.r[i] = s;
        }
        let mut gate = GateEffect::new(
            GateParams {
                threshold_db,
                attack_ms: 0.5,
                hold_ms: 0.0,
                release_ms: 5.0,
                range_db: -80.0,
            },
            SR,
        );
        let mut openings = 0usize;
        let mut was_open = false;
        for block in bus.l.chunks(32).zip(bus.r.chunks(32)).map(|(l, r)| (l.to_vec(), r.to_vec())) {
            let mut chunk = StereoBus::with_capacity(block.0.len());
            chunk.l[..block.0.len()].copy_from_slice(&block.0);
            chunk.r[..block.1.len()].copy_from_slice(&block.1);
            gate.process(&context(block.0.len()), &mut chunk, &EventList::empty(), None);
            if gate.open != was_open {
                if gate.open {
                    openings += 1;
                }
                was_open = gate.open;
            }
        }
        assert_eq!(openings, 1, "the gate chattered: it opened {openings} times");
        assert!(was_open, "and it should have stayed open");
    }

    /// The compressor's own Mix is a linear parallel balance (MOO-142): at 0
    /// the input exactly, at 1 the compressor exactly as it was before Mix
    /// existed, and at a half, half of each -- not the host's equal-power
    /// blend, which sums a compressor with its own input 3 dB hot.
    #[test]
    fn the_compressor_mix_is_a_linear_parallel_balance() {
        let frames = 48_000;
        let run = |mix: f32| {
            let mut bus = tone_bus(frames, 0.5);
            let mut effect = CompressorEffect::new(
                CompressorParams {
                    threshold_db: -40.0,
                    ratio: 20.0,
                    attack_ms: 1.0,
                    release_ms: 50.0,
                    knee_db: 0.0,
                    makeup_db: 0.0,
                    mix,
                },
                SR,
            );
            effect.process(&context(frames), &mut bus, &EventList::empty(), None);
            bus.l[..frames].to_vec()
        };
        let dry = tone_bus(frames, 0.5).l[..frames].to_vec();
        assert_eq!(run(0.0), dry, "a mix of zero is the input exactly");
        let wet = run(1.0);
        let half = run(0.5);
        for i in frames / 2..frames {
            let expected = 0.5 * dry[i] + 0.5 * wet[i];
            assert!(
                (half[i] - expected).abs() < 1e-4,
                "at {i}: {} where half of each is {expected}",
                half[i]
            );
        }
    }
}
