//! A compact multi-mode modulation processor.
//!
//! Chorus, flange, ensemble, and ADT are distinct policies over one short,
//! fractional stereo delay line. Phaser deliberately stays in this device
//! because it shares the LFO, parameter contract, and musical use, while its
//! all-pass cascade is less costly and more accurate than faking it with a
//! delay tap. The host owns dry/wet blending; this node returns wet signal.

use mooloop_core::{
    LfoWave, ModulationMode, ModulationParams, MODULATION_PARAM_COLOR, MODULATION_PARAM_DEPTH,
    MODULATION_PARAM_FEEDBACK, MODULATION_PARAM_MODE, MODULATION_PARAM_RATE_HZ,
    MODULATION_PARAM_SPREAD, MODULATION_PARAM_STAGES, MODULATION_PARAM_TONE,
};

use crate::bus::StereoBus;
use crate::delayline::{DelayLine, MIN_READ_OFFSET};
use crate::event::EventList;
use crate::filter::{AllPass, OnePoleLp};
use crate::lfo::Lfo;
use crate::node::{feedback_tail_frames, AudioNode, Discontinuity, ProcessContext};
use crate::smooth::Smoothed;
use super::{process_param_split, RangeProcessor};

const MAX_DELAY_MS: f32 = 64.0;
const MAX_PHASER_STAGES: usize = 12;
const TONE_MIN_HZ: f32 = 350.0;
const TONE_MAX_HZ: f32 = 20_000.0;
/// Time constant for depth, feedback, spread, tone, and color: all continuous
/// and audible, all currently stepped once per block on every knob move.
/// Rate is deliberately excluded — it feeds a phase increment, so a step in
/// rate is not a step in output. Stages and mode are discrete.
const PARAM_SMOOTH_S: f32 = 0.01;
/// How often the phaser works out its all-pass coefficients, in samples
/// (MOO-235). Between two control points each coefficient moves in a
/// straight line, so a stage costs a multiply-add a sample instead of an
/// `exp2` and a `tan`. At the fastest rate (12 Hz) and full depth the sweep
/// moves a stage about 0.03 octave in this span, which a straight line
/// follows closely (`control_rate_phaser_matches_the_per_sample_formula`
/// pins how closely). 16 measured 12 dB further from the per-sample formula
/// for about 2.5 us a block less at 8 stages; the phaser is far under
/// Chorus's cost either way, so the closer one won. Every coefficient stays
/// inside `(-0.999, 0.999)` at both ends, so every point of the line does
/// too and each first-order stage stays stable.
const PHASER_CONTROL_FRAMES: u32 = 8;
/// `log2(220)` and `log2(28)`: the phaser's centre is `220 * 28^color` Hz,
/// whose log is a straight line in `color`.
const PHASER_LOG2_BASE_HZ: f32 = 7.781_359_7;
const PHASER_LOG2_COLOR_SPAN: f32 = 4.807_355;
/// The most a feedback resonance may lift a steady tone, as a gain: 4, or
/// +12 dB (MOO-200). A comb or an all-pass loop with feedback `fb` peaks at
/// `1 / (1 - |fb|)`, which reaches 4 at `|fb| = 0.75` and +22 dB at the
/// knob's end, 0.92. See [`resonance_trim`].
const RESONANCE_CEILING: f32 = 4.0;

pub struct ModulationEffect {
    params: ModulationParams,
    sample_rate: u32,
    line: DelayLine,
    lfo: Lfo,
    feedback_l: f32,
    feedback_r: f32,
    tone_l: OnePoleLp,
    tone_r: OnePoleLp,
    phaser_l: [AllPass; MAX_PHASER_STAGES],
    phaser_r: [AllPass; MAX_PHASER_STAGES],
    /// Per-stage tilt for `phaser_sample`, a pure function of `stages`
    /// (`reports/fable-2026-09-22.md` finding 2): rebuilt in
    /// [`Self::rebuild_tilt`] whenever `stages` changes rather than once per
    /// stage per sample. Only `[0..stages)` is meaningful; entries beyond
    /// the current stage count are stale and never read.
    tilt: [f32; MAX_PHASER_STAGES],
    /// The phaser's control-rate coefficients (MOO-235): the value each
    /// stage uses at this sample, the one it reaches at the next control
    /// point, and the per-sample step between them.
    phaser_coeffs: PhaserCoeffs,
    /// Samples left until the next control point. Zero means "now".
    phaser_countdown: u32,
    /// Whether `phaser_coeffs` describes where the LFO is. False after
    /// anything that moves the LFO or the stage table without the sample
    /// loop seeing it: construction, a reset, a skipped block, a stage count
    /// or sample-rate change, or a stretch in another mode.
    phaser_primed: bool,
    /// The Tone value the tone filters' cutoff was last set for, so the
    /// `powf` and two `exp` only run while Tone moves (MOO-235). NaN when
    /// no cutoff has been set, which no Tone value equals.
    tone_set_for: f32,
    depth: Smoothed,
    feedback: Smoothed,
    spread: Smoothed,
    tone: Smoothed,
    color: Smoothed,
}

impl ModulationEffect {
    pub fn new(params: ModulationParams, sample_rate: u32) -> Self {
        let smoothed = |initial| Smoothed::new(initial, PARAM_SMOOTH_S, sample_rate);
        let mut effect = Self {
            params,
            sample_rate,
            line: DelayLine::with_capacity_frames(ring_frames(sample_rate)),
            lfo: Lfo::new(),
            feedback_l: 0.0,
            feedback_r: 0.0,
            tone_l: OnePoleLp::new(),
            tone_r: OnePoleLp::new(),
            phaser_l: [AllPass::default(); MAX_PHASER_STAGES],
            phaser_r: [AllPass::default(); MAX_PHASER_STAGES],
            tilt: [0.0; MAX_PHASER_STAGES],
            phaser_coeffs: PhaserCoeffs::default(),
            phaser_countdown: 0,
            phaser_primed: false,
            tone_set_for: f32::NAN,
            depth: smoothed(params.depth.clamp(0.0, 1.0)),
            feedback: smoothed(params.feedback.clamp(-0.92, 0.92)),
            spread: smoothed(params.spread.clamp(0.0, 1.0)),
            tone: smoothed(params.tone.clamp(0.0, 1.0)),
            color: smoothed(params.color.clamp(0.0, 1.0)),
        };
        effect.rebuild_tilt();
        effect
    }

    pub fn params(&self) -> ModulationParams {
        self.params
    }

    /// Replace the parameter set wholesale (project load) — jump straight to
    /// the new values, there is nothing to click coming from a fresh load.
    pub fn set_params(&mut self, params: ModulationParams) {
        self.params = params;
        self.depth.reset_to(params.depth.clamp(0.0, 1.0));
        self.feedback.reset_to(params.feedback.clamp(-0.92, 0.92));
        self.spread.reset_to(params.spread.clamp(0.0, 1.0));
        self.tone.reset_to(params.tone.clamp(0.0, 1.0));
        self.color.reset_to(params.color.clamp(0.0, 1.0));
        self.rebuild_tilt();
    }

    /// Refill [`Self::tilt`] from the current, clamped `stages` -- the same
    /// formula `phaser_sample` used to evaluate per stage per sample, now
    /// evaluated once whenever `stages` can have changed (construction, a
    /// wholesale param load, and the `MODULATION_PARAM_STAGES` case of
    /// [`RangeProcessor::apply_param`]).
    fn rebuild_tilt(&mut self) {
        let stages = usize::from(self.params.stages).clamp(4, MAX_PHASER_STAGES);
        let denom = (stages - 1).max(1) as f32;
        for (stage, tilt) in self.tilt.iter_mut().enumerate().take(stages) {
            *tilt = (stage as f32 / denom - 0.5) * 1.1;
        }
        self.phaser_primed = false;
    }

    /// Replace `start..end` of `bus` with this node's wet output, without the
    /// event handling [`AudioNode::process`] wraps it in.
    ///
    /// The rack host always has a whole block and an event list; a device that
    /// embeds this processor as a finisher has neither — it has whatever range
    /// its own event splitting left, and its mode comes from its own patch. So
    /// the range is the reusable unit, and `process` is one caller of it.
    pub fn process_wet(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        self.process_range(bus, start, end);
    }

    /// Forget the delay line, the filters, and the LFO phase.
    ///
    /// Non-allocating, so a device may call it when its own mode changes and
    /// the line's contents are older than anything it should be reading.
    pub fn reset(&mut self) {
        self.line.clear();
        self.lfo = Lfo::new();
        self.phaser_primed = false;
        self.feedback_l = 0.0;
        self.feedback_r = 0.0;
        self.tone_l.reset();
        self.tone_r.reset();
        for stage in &mut self.phaser_l {
            stage.reset();
        }
        for stage in &mut self.phaser_r {
            stage.reset();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn delay_sample(
        &mut self,
        input_l: f32,
        input_r: f32,
        mode: ModulationMode,
        depth: f32,
        feedback: f32,
        spread: f32,
        lfo_l: f32,
        lfo_r: f32,
        color: f32,
        tone: f32,
    ) -> (f32, f32) {
        let (base_ms, swing_ms) = match mode {
            ModulationMode::Chorus => (8.0 + 18.0 * color, 0.5 + 10.0 * depth),
            ModulationMode::Flange => (0.7 + 5.0 * color, 0.15 + 5.5 * depth),
            ModulationMode::Ensemble => (6.0 + 14.0 * color, 1.0 + 8.0 * depth),
            ModulationMode::Adt => (14.0 + 28.0 * color, 0.08 + 2.5 * depth),
            ModulationMode::Phaser => unreachable!(),
        };
        let to_frames = |ms: f32| {
            (ms.max(0.0) * self.sample_rate as f32 / 1_000.0)
                .clamp(MIN_READ_OFFSET, self.line.max_read_offset())
        };
        let delay_l = to_frames(base_ms + swing_ms * lfo_l);
        let delay_r = to_frames(base_ms + swing_ms * lfo_r);
        let (tap_ll, tap_lr) = self.line.read(delay_l);
        let (tap_rl, tap_rr) = self.line.read(delay_r);
        let mut wet_l = tap_ll;
        let mut wet_r = tap_rr;
        if mode == ModulationMode::Ensemble {
            let offset = to_frames(base_ms + swing_ms * 0.47);
            let (third_l, third_r) = self.line.read(offset);
            wet_l = (tap_ll + tap_rl + third_l) / 3.0;
            wet_r = (tap_rr + tap_lr + third_r) / 3.0;
        } else if mode == ModulationMode::Adt {
            // Keep a fixed second voice so the mode remains a double tracker
            // at zero depth; the LFO then becomes tape-speed wander.
            let (fixed_l, fixed_r) = self.line.read(to_frames(base_ms * 1.37));
            wet_l = (tap_ll + fixed_l) * 0.5;
            wet_r = (tap_rr + fixed_r) * 0.5;
        }
        self.feedback_l = wet_l;
        self.feedback_r = wet_r;
        let cross = spread * 0.35;
        self.line.write(
            input_l + feedback * (self.feedback_l * (1.0 - cross) + self.feedback_r * cross),
            input_r + feedback * (self.feedback_r * (1.0 - cross) + self.feedback_l * cross),
        );
        let trim = resonance_trim(feedback);
        self.tone_filter(wet_l * trim, wet_r * trim, tone)
    }

    /// One sample of the all-pass cascade, at the control-rate coefficients
    /// [`Self::advance_phaser_control`] keeps (MOO-235).
    fn phaser_sample(&mut self, input_l: f32, input_r: f32, feedback: f32, tone: f32) -> (f32, f32) {
        let stages = usize::from(self.params.stages).clamp(4, MAX_PHASER_STAGES);
        let coeffs = &mut self.phaser_coeffs;
        let mut left = input_l + self.feedback_l * feedback;
        for ((stage, coeff), step) in self.phaser_l[..stages]
            .iter_mut()
            .zip(&mut coeffs.now_l[..stages])
            .zip(&coeffs.step_l[..stages])
        {
            left = stage.next(left, *coeff);
            *coeff += step;
        }
        let mut right = input_r + self.feedback_r * feedback;
        for ((stage, coeff), step) in self.phaser_r[..stages]
            .iter_mut()
            .zip(&mut coeffs.now_r[..stages])
            .zip(&coeffs.step_r[..stages])
        {
            right = stage.next(right, *coeff);
            *coeff += step;
        }
        self.feedback_l = left;
        self.feedback_r = right;
        let trim = resonance_trim(feedback);
        self.tone_filter(left * trim, right * trim, tone)
    }

    /// Called once a sample before [`Self::phaser_sample`], and before the
    /// LFO advances. At a control point, land every stage exactly on the
    /// coefficient it was heading for and aim it at the one the sweep reaches
    /// [`PHASER_CONTROL_FRAMES`] samples on. Unprimed, it first works out
    /// where the sweep is now, so the line starts from the truth.
    ///
    /// "Samples on" means everything the coefficient depends on: the LFO
    /// phase that far ahead at the current rate, and `depth`, `color` and
    /// `spread` where their lags will be by then. Read at their present value
    /// instead, a Depth or Color move put the whole sweep a span behind for
    /// the 50 ms the lag takes: measured with a 16-sample span, a Color move
    /// alone took the difference from the per-sample formula from 64 to 53 dB
    /// under the signal. A Rate change unprimes, so the span it lands in restarts
    /// from the real phase. Each control point starts from the real phase,
    /// so nothing drifts.
    fn advance_phaser_control(&mut self, depth: f32, color: f32, spread: f32) {
        if self.phaser_primed && self.phaser_countdown > 0 {
            self.phaser_countdown -= 1;
            return;
        }
        let stages = usize::from(self.params.stages).clamp(4, MAX_PHASER_STAGES);
        let sample_rate = self.sample_rate.max(1);
        let coefficient = |lfo: f32, depth: f32, color: f32| {
            let log2_center = PHASER_LOG2_BASE_HZ + PHASER_LOG2_COLOR_SPAN * color;
            let octaves = 0.15 + depth * 2.2;
            allpass_coefficient(phaser_hz(lfo, log2_center, octaves, sample_rate), sample_rate)
        };
        if !self.phaser_primed {
            let sweep_l = self.lfo.peek_offset(0.0, LfoWave::Sine);
            let sweep_r = self.lfo.peek_offset(spread * 0.25, LfoWave::Sine);
            for stage in 0..stages {
                let tilt = self.tilt[stage];
                self.phaser_coeffs.target_l[stage] = coefficient(sweep_l + tilt, depth, color);
                self.phaser_coeffs.target_r[stage] = coefficient(sweep_r + tilt, depth, color);
            }
            self.phaser_primed = true;
        }
        let ahead = |lag: &Smoothed| {
            if lag.is_settled() {
                lag.value()
            } else {
                let mut lag = *lag;
                lag.advance_by(PHASER_CONTROL_FRAMES as usize)
            }
        };
        let (depth, color, spread) = (ahead(&self.depth), ahead(&self.color), ahead(&self.spread));
        let phase_ahead = PHASER_CONTROL_FRAMES as f32
            * self.params.rate_hz.clamp(0.0, sample_rate as f32 * 0.25)
            / sample_rate as f32;
        let sweep_l = self.lfo.peek_offset(phase_ahead, LfoWave::Sine);
        let sweep_r = self.lfo.peek_offset(phase_ahead + spread * 0.25, LfoWave::Sine);
        let per_frame = 1.0 / PHASER_CONTROL_FRAMES as f32;
        let coeffs = &mut self.phaser_coeffs;
        for stage in 0..stages {
            let tilt = self.tilt[stage];
            let target_l = coefficient(sweep_l + tilt, depth, color);
            let target_r = coefficient(sweep_r + tilt, depth, color);
            coeffs.now_l[stage] = coeffs.target_l[stage];
            coeffs.now_r[stage] = coeffs.target_r[stage];
            coeffs.step_l[stage] = (target_l - coeffs.now_l[stage]) * per_frame;
            coeffs.step_r[stage] = (target_r - coeffs.now_r[stage]) * per_frame;
            coeffs.target_l[stage] = target_l;
            coeffs.target_r[stage] = target_r;
        }
        self.phaser_countdown = PHASER_CONTROL_FRAMES - 1;
    }

    /// The wet path's low-pass. Its cutoff is set only when `tone` differs
    /// from the value it was last set for, so a Tone at rest costs two
    /// one-pole samples and nothing else (MOO-235). The coefficient is the
    /// same bits either way.
    fn tone_filter(&mut self, left: f32, right: f32, tone: f32) -> (f32, f32) {
        if tone != self.tone_set_for {
            let hz = TONE_MIN_HZ * (TONE_MAX_HZ / TONE_MIN_HZ).powf(tone);
            self.tone_l.set_cutoff(hz, self.sample_rate.max(1));
            self.tone_r.set_cutoff(hz, self.sample_rate.max(1));
            self.tone_set_for = tone;
        }
        (self.tone_l.next_sample(left), self.tone_r.next_sample(right))
    }
}

/// How far to turn the wet signal down so a feedback resonance peaks no
/// higher than [`RESONANCE_CEILING`] (MOO-200): `min(1, 4 * (1 - |fb|))`.
///
/// Unity up to `|fb| = 0.75`, so moderate feedback, every factory preset
/// and ML-P8's finishing chorus sound exactly as they did. Past it the trim
/// follows the loop's own peak down, so the resonance keeps sharpening all
/// the way to the knob's end, and rings just as long, but its peak stays at
/// +12 dB: at 0.92 the trim is 0.32, -9.9 dB. It scales the wet output only,
/// never the loop, so the loop's character is untouched; a
/// linear trim bounds the gain at every input level, where a `tanh` knee in
/// the loop (the Delay's, MOO-124) would leave a quiet input its +22 dB and
/// distort a loud one. Lowering the knob's top to 0.75 would bound it too,
/// and take the jet with it.
///
/// Every mode takes it. Chorus, Flange and Phaser feed back one tap or one
/// cascade, so their loop peaks at the whole `|fb|`. Ensemble and ADT feed
/// back an average of taps at different delays, which agree at low
/// frequencies, so they get most of the way there too: untrimmed at full
/// feedback, +16 and +14 dB around 50 to 160 Hz (`modulation_gain_table`,
/// 2026-09-26).
#[inline]
fn resonance_trim(feedback: f32) -> f32 {
    (RESONANCE_CEILING * (1.0 - feedback.abs())).min(1.0)
}

fn ring_frames(sample_rate: u32) -> usize {
    (MAX_DELAY_MS * sample_rate as f32 / 1_000.0) as usize + 8
}

/// The coefficients of both channels' all-pass stages, at control rate.
#[derive(Clone, Copy, Default)]
struct PhaserCoeffs {
    now_l: [f32; MAX_PHASER_STAGES],
    now_r: [f32; MAX_PHASER_STAGES],
    step_l: [f32; MAX_PHASER_STAGES],
    step_r: [f32; MAX_PHASER_STAGES],
    target_l: [f32; MAX_PHASER_STAGES],
    target_r: [f32; MAX_PHASER_STAGES],
}

/// A stage's corner, `center * 2^(lfo*octaves)`, as one `exp2` of a sum:
/// `log2_center` is `log2(220 * 28^color)` and `octaves` the depth term,
/// both worked out by the caller, and `lfo` is the sweep plus the stage's
/// tilt. It replaced two `powf` a call (`reports/fable-2026-09-22.md`
/// finding 2), and since MOO-235 it runs at control rate, not per sample.
fn phaser_hz(lfo: f32, log2_center: f32, octaves: f32, sample_rate: u32) -> f32 {
    (log2_center + lfo * octaves)
        .exp2()
        .clamp(60.0, sample_rate as f32 * 0.42)
}

fn allpass_coefficient(hz: f32, sample_rate: u32) -> f32 {
    let g = (core::f32::consts::PI * hz / sample_rate.max(1) as f32).tan();
    ((1.0 - g) / (1.0 + g)).clamp(-0.999, 0.999)
}

impl RangeProcessor for ModulationEffect {
    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        for i in start..end {
            let depth = self.depth.advance();
            let feedback = self.feedback.advance();
            let spread = self.spread.advance();
            let tone = self.tone.advance();
            let color = self.color.advance();
            let (input_l, input_r) = (bus.l[i], bus.r[i]);
            let (wet_l, wet_r) = match self.params.mode {
                ModulationMode::Phaser => {
                    // Reads the LFO ahead of itself, so before it advances.
                    self.advance_phaser_control(depth, color, spread);
                    self.lfo.skip(1, self.params.rate_hz, self.sample_rate);
                    self.phaser_sample(input_l, input_r, feedback, tone)
                }
                mode => {
                    // A stretch in another mode leaves the phaser's
                    // coefficients behind the LFO.
                    self.phaser_primed = false;
                    // Two taps of one LFO cycle rather than two independently
                    // drifting oscillators, so the stereo image stays locked
                    // even as `spread` moves. Peek both before advancing once.
                    let sweep_l = self.lfo.peek_offset(0.0, LfoWave::Sine);
                    let sweep_r = self.lfo.peek_offset(spread * 0.25, LfoWave::Sine);
                    self.lfo.skip(1, self.params.rate_hz, self.sample_rate);
                    self.delay_sample(
                        input_l, input_r, mode, depth, feedback, spread, sweep_l, sweep_r, color,
                        tone,
                    )
                }
            };
            bus.l[i] = wet_l;
            bus.r[i] = wet_r;
        }
    }

    fn apply_param(&mut self, id: u32, value: f32) {
        // Rate, Depth, Color and Spread all steer the phaser's sweep, and the
        // span in flight was aimed along their old values. A real change
        // restarts it from where the sweep is (MOO-235); a value sent again
        // unchanged, as a lane or a route does every block, does not.
        let before = (
            self.params.rate_hz,
            self.params.depth,
            self.params.color,
            self.params.spread,
        );
        match id {
            MODULATION_PARAM_MODE => {
                self.params.mode = ModulationMode::from_index(value.round() as i32)
            }
            MODULATION_PARAM_RATE_HZ => self.params.rate_hz = value.clamp(0.02, 12.0),
            MODULATION_PARAM_DEPTH => {
                self.params.depth = value.clamp(0.0, 1.0);
                self.depth.set_target(self.params.depth);
            }
            MODULATION_PARAM_COLOR => {
                self.params.color = value.clamp(0.0, 1.0);
                self.color.set_target(self.params.color);
            }
            MODULATION_PARAM_FEEDBACK => {
                self.params.feedback = value.clamp(-0.92, 0.92);
                self.feedback.set_target(self.params.feedback);
            }
            MODULATION_PARAM_SPREAD => {
                self.params.spread = value.clamp(0.0, 1.0);
                self.spread.set_target(self.params.spread);
            }
            MODULATION_PARAM_TONE => {
                self.params.tone = value.clamp(0.0, 1.0);
                self.tone.set_target(self.params.tone);
            }
            MODULATION_PARAM_STAGES => {
                self.params.stages = value.round().clamp(4.0, 12.0) as u8;
                self.rebuild_tilt();
            }
            _ => {}
        }
        let after = (
            self.params.rate_hz,
            self.params.depth,
            self.params.color,
            self.params.spread,
        );
        if after != before {
            self.phaser_primed = false;
        }
    }
}

impl ModulationEffect {
    /// Empty the line, the feedback and the filters, keeping the LFO phase.
    fn clear_tail(&mut self) {
        self.line.clear();
        self.feedback_l = 0.0;
        self.feedback_r = 0.0;
        self.tone_l.reset();
        self.tone_r.reset();
        for stage in self.phaser_l.iter_mut().chain(&mut self.phaser_r) {
            stage.reset();
        }
    }
}

impl AudioNode for ModulationEffect {
    /// Clears the line and the filters, and **keeps the LFO phase**. This is
    /// the narrower reset `Self::reset` is not: that one restarts the LFO,
    /// which is right when the device's own mode changes and wrong here. A
    /// free-running modulator has to arrive at the same phase whether or not
    /// the transport was seeked, for the same reason `skip_block` exists.
    ///
    /// Only on a kind that [invalidates tails](Discontinuity::invalidates_tails):
    /// a program change and a loop fold leave the line ringing.
    fn on_discontinuity(&mut self, kind: Discontinuity) {
        if !kind.invalidates_tails() {
            return;
        }
        self.clear_tail();
    }

    /// Measured against the longest tap the device can be asked for rather
    /// than the one the mode is currently using: `MAX_DELAY_MS` is both the
    /// ring's capacity and the furthest back `depth`, `spread` and the LFO can
    /// put a read head, and a sleeping ring stops advancing. The phaser mode
    /// has no line at all — its all-pass cascade settles in a handful of
    /// samples — so the same number covers it many times over.
    fn tail_frames(&self) -> u32 {
        // A knob still travelling keeps the device awake: freezing a lag
        // halfway would leave it there, and the stage would come back with
        // values its own parameters do not describe.
        if !self.depth.is_settled()
            || !self.feedback.is_settled()
            || !self.spread.is_settled()
            || !self.tone.is_settled()
            || !self.color.is_settled()
        {
            return u32::MAX;
        }
        let trip = MAX_DELAY_MS * 0.001 * self.sample_rate.max(1) as f32;
        let gain = self.feedback.value().abs().max(self.params.feedback.abs());
        feedback_tail_frames(gain, trip)
    }

    /// The LFO is the whole device: it runs on the clock, not on the input,
    /// so it has to arrive at the same phase whether or not the stage was
    /// called. One `skip` per frame rather than one for the block, because
    /// that is what the sample loop does and the two do not land in the same
    /// place.
    fn skip_block(&mut self, ctx: &ProcessContext) {
        self.lfo.skip(ctx.frames, self.params.rate_hz, self.sample_rate);
        self.phaser_primed = false;
        // The line is written too, and not because of what is in it -- it is
        // silent either way. `DelayLine::read` derives its interpolation
        // fraction from `write - 1 - offset`, so where the write head sits
        // changes the last few bits of that fraction. A frozen ring would
        // come back reading between different samples than a running one, and
        // a chorus's read head moves fast enough over noisy material for that
        // to show. Keeping ring time and wall time the same thing is a
        // requirement about where the head ends up, not about how it got
        // there, so the zeros go in as spans.
        self.line.write_silence(ctx.frames);
    }

    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        let sample_rate = ctx.sample_rate.max(1);
        if sample_rate != self.sample_rate {
            self.sample_rate = sample_rate;
            self.depth.set_time(PARAM_SMOOTH_S, sample_rate);
            self.feedback.set_time(PARAM_SMOOTH_S, sample_rate);
            self.spread.set_time(PARAM_SMOOTH_S, sample_rate);
            self.tone.set_time(PARAM_SMOOTH_S, sample_rate);
            self.color.set_time(PARAM_SMOOTH_S, sample_rate);
            self.phaser_primed = false;
            self.tone_set_for = f32::NAN;
        }
        let frames = ctx.frames.min(bus.capacity());
        process_param_split(self, bus, events_in, frames);
        // A NaN or an infinity that got into the line or the all-passes would stay there for
        // good (MOO-176). It comes out as silence, and they start clean.
        if super::scrub_non_finite(bus, frames) {
            self.clear_tail();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// **Free-running state keeps running**, the rest-and-tail rule applied
    /// to a discontinuity. The LFO advances whether or not this node is
    /// called, so it has to arrive at the same phase across a seek exactly as
    /// it does across a sleep -- otherwise a bounce stops matching the take
    /// it was rendered against, and *how* it differs depends on where the
    /// player happened to seek.
    ///
    /// This is what separates `on_discontinuity` from the device's own
    /// `reset`, which restarts the LFO deliberately: that is right when the
    /// device's mode changes and wrong when the transport moves.
    #[test]
    fn a_seek_empties_the_line_and_keeps_the_lfo_phase() {
        let frames = 4_096;
        let mut bus = StereoBus::with_capacity(frames);
        bus.l[0] = 1.0;
        bus.r[0] = 1.0;
        let mut effect = ModulationEffect::new(
            ModulationParams {
                tone: 1.0,
                ..ModulationParams::default()
            },
            SR,
        );
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);

        // Read through `peek_offset`, which is a pure function of the phase:
        // the field itself belongs to `lfo.rs`.
        let phase_before = effect.lfo.peek_offset(0.0, LfoWave::Sine);
        effect.on_discontinuity(Discontinuity::Seek);
        assert_eq!(
            effect.lfo.peek_offset(0.0, LfoWave::Sine),
            phase_before,
            "the seek restarted a free-running LFO"
        );

        let mut after = StereoBus::with_capacity(frames);
        effect.process(&context(frames), &mut after, &EventList::empty(), None);
        let peak = after.l.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
        assert!(
            peak <= 1e-6,
            "the line kept audio from before the seek: {peak}"
        );

        // The control: `reset` is the wider one and *does* restart it, which
        // is what makes the assertion above a distinction rather than a
        // coincidence.
        effect.reset();
        assert_ne!(
            effect.lfo.peek_offset(0.0, LfoWave::Sine),
            phase_before,
            "reset was expected to restart the LFO; if it no longer does, the \
             test above has stopped proving anything"
        );
    }

    #[test]
    fn chorus_returns_an_impulse_after_its_short_delay() {
        let frames = 4_096;
        let mut bus = StereoBus::with_capacity(frames);
        bus.l[0] = 1.0;
        bus.r[0] = 1.0;
        let mut effect = ModulationEffect::new(
            ModulationParams {
                tone: 1.0,
                ..ModulationParams::default()
            },
            SR,
        );
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);
        assert!(bus.l[400..].iter().any(|sample| sample.abs() > 1e-4));
    }

    #[test]
    fn phaser_and_chorus_have_distinct_responses() {
        let frames = 2_048;
        let mut chorus_bus = StereoBus::with_capacity(frames);
        let mut phaser_bus = StereoBus::with_capacity(frames);
        for frame in 0..frames {
            let sample = (frame as f32 * 0.13).sin();
            chorus_bus.l[frame] = sample;
            chorus_bus.r[frame] = sample;
            phaser_bus.l[frame] = sample;
            phaser_bus.r[frame] = sample;
        }
        let mut chorus = ModulationEffect::new(ModulationParams::default(), SR);
        let mut phaser = ModulationEffect::new(
            ModulationParams {
                mode: ModulationMode::Phaser,
                ..ModulationParams::default()
            },
            SR,
        );
        chorus.process(&context(frames), &mut chorus_bus, &EventList::empty(), None);
        phaser.process(&context(frames), &mut phaser_bus, &EventList::empty(), None);
        let difference: f32 = chorus_bus
            .l
            .iter()
            .zip(&phaser_bus.l)
            .map(|(a, b)| (a - b).abs())
            .sum();
        assert!(difference > 10.0);
    }

    #[test]
    fn depth_change_mid_block_does_not_click() {
        use crate::event::{Event, TimedEvent};

        let frames = 8_192;
        let mut bus = StereoBus::with_capacity(frames);
        for i in 0..frames {
            let s = (i as f32 / SR as f32 * 220.0 * core::f32::consts::TAU).sin() * 0.4;
            bus.l[i] = s;
            bus.r[i] = s;
        }
        let mut effect = ModulationEffect::new(
            ModulationParams {
                depth: 0.0,
                tone: 1.0,
                ..ModulationParams::default()
            },
            SR,
        );
        let mut events = EventList::empty();
        assert!(events.push(TimedEvent {
            offset: (frames / 2) as u32,
            event: Event::ParamValue {
                id: MODULATION_PARAM_DEPTH,
                value: 1.0,
            },
        }));
        effect.process(&context(frames), &mut bus, &events, None);
        let max_step = crate::testkit::max_step(&bus.l[..frames]);
        assert!(
            max_step < 0.2,
            "depth change left a discontinuity of {max_step}"
        );
    }

    /// `phaser_hz`'s `exp2`-of-a-sum is a deliberate reformulation of the
    /// original `center * 2^(lfo*octaves)` (two `powf`), not a bit-preserving
    /// hoist -- this pins the difference to float rounding rather than to
    /// changed behaviour, across the ranges `depth`, `color`, and the LFO
    /// sweep (plus tilt) actually take. Since MOO-235 `log2_center` is the
    /// closed form `log2(220) + color * log2(28)` rather than a `powf` and a
    /// `log2`, and the tolerance, a relative 1e-4, is unchanged.
    #[test]
    fn phaser_hz_matches_the_original_two_powf_formula() {
        for depth in [0.0f32, 0.15, 0.5, 0.85, 1.0] {
            for color in [0.0f32, 0.2, 0.45, 0.7, 1.0] {
                for lfo in [-1.55f32, -1.0, -0.3, 0.0, 0.4, 1.0, 1.55] {
                    let octaves = 0.15 + depth * 2.2;
                    // The formula as it read before this change.
                    let expected = {
                        let center = 220.0 * 28.0f32.powf(color);
                        (center * 2.0f32.powf(lfo * octaves)).clamp(60.0, SR as f32 * 0.42)
                    };
                    let log2_center = PHASER_LOG2_BASE_HZ + PHASER_LOG2_COLOR_SPAN * color;
                    let actual = phaser_hz(lfo, log2_center, octaves, SR);
                    let scale = expected.abs().max(1.0);
                    assert!(
                        (actual - expected).abs() / scale < 1.0e-4,
                        "depth {depth} color {color} lfo {lfo}: old {expected}, new {actual}"
                    );
                }
            }
        }
    }

    /// [`ModulationEffect::rebuild_tilt`] must land on exactly what
    /// `phaser_sample` used to compute inline, per stage per sample, or the
    /// hoist changes the phaser's sound rather than only its cost.
    #[test]
    fn tilt_table_matches_the_inline_per_stage_formula() {
        for stages in 4u8..=12 {
            let effect = ModulationEffect::new(
                ModulationParams {
                    mode: ModulationMode::Phaser,
                    stages,
                    ..ModulationParams::default()
                },
                SR,
            );
            let n = usize::from(stages);
            for stage in 0..n {
                let expected = (stage as f32 / (n - 1).max(1) as f32 - 0.5) * 1.1;
                assert_eq!(
                    effect.tilt[stage], expected,
                    "stages {stages}, stage {stage}"
                );
            }
        }
    }

    /// The tilt table has to follow a live `stages` change through
    /// `apply_param`, not just a fresh construction -- that path is
    /// `rebuild_tilt`'s other caller and the one the per-sample loop
    /// actually depends on after a knob move.
    #[test]
    fn changing_stages_mid_stream_rebuilds_the_tilt_table() {
        let mut effect = ModulationEffect::new(
            ModulationParams {
                mode: ModulationMode::Phaser,
                stages: 4,
                ..ModulationParams::default()
            },
            SR,
        );
        let four_stage_tilt_1 = effect.tilt[1];
        effect.apply_param(MODULATION_PARAM_STAGES, 12.0);
        assert_eq!(effect.params.stages, 12);
        let expected = (1.0 / 11.0 - 0.5) * 1.1;
        assert_eq!(effect.tilt[1], expected);
        assert_ne!(
            effect.tilt[1], four_stage_tilt_1,
            "the table did not change with the stage count"
        );
    }

    /// The phaser as it was before MOO-235 (with MOO-200's output trim,
    /// which came later and is not what it checks), sample by sample: every stage's
    /// coefficient from an `exp2` and a `tan` each sample, and the tone
    /// filter's cutoff from a `powf` and two `exp` each sample. Kept here as
    /// the reference the control-rate phaser is measured against, for its
    /// sound (`control_rate_phaser_matches_the_per_sample_formula`) and its
    /// cost (`phaser_cost`).
    struct PerSamplePhaser {
        params: ModulationParams,
        lfo: Lfo,
        stages_l: [AllPass; MAX_PHASER_STAGES],
        stages_r: [AllPass; MAX_PHASER_STAGES],
        tone_l: OnePoleLp,
        tone_r: OnePoleLp,
        feedback_l: f32,
        feedback_r: f32,
        depth: Smoothed,
        feedback: Smoothed,
        spread: Smoothed,
        tone: Smoothed,
        color: Smoothed,
    }

    impl PerSamplePhaser {
        fn new(params: ModulationParams) -> Self {
            let smoothed = |initial| Smoothed::new(initial, PARAM_SMOOTH_S, SR);
            Self {
                params,
                lfo: Lfo::new(),
                stages_l: [AllPass::default(); MAX_PHASER_STAGES],
                stages_r: [AllPass::default(); MAX_PHASER_STAGES],
                tone_l: OnePoleLp::new(),
                tone_r: OnePoleLp::new(),
                feedback_l: 0.0,
                feedback_r: 0.0,
                depth: smoothed(params.depth),
                feedback: smoothed(params.feedback),
                spread: smoothed(params.spread),
                tone: smoothed(params.tone),
                color: smoothed(params.color),
            }
        }

        fn apply(&mut self, id: u32, value: f32) {
            match id {
                MODULATION_PARAM_DEPTH => self.depth.set_target(value),
                MODULATION_PARAM_TONE => self.tone.set_target(value),
                MODULATION_PARAM_COLOR => self.color.set_target(value),
                MODULATION_PARAM_RATE_HZ => self.params.rate_hz = value,
                _ => unreachable!("the reference only follows the knobs its tests move"),
            }
        }

        fn sample(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
            let depth = self.depth.advance();
            let feedback = self.feedback.advance();
            let spread = self.spread.advance();
            let tone = self.tone.advance();
            let color = self.color.advance();
            let sweep_l = self.lfo.peek_offset(0.0, LfoWave::Sine);
            let sweep_r = self.lfo.peek_offset(spread * 0.25, LfoWave::Sine);
            self.lfo.skip(1, self.params.rate_hz, SR);
            let log2_center = (220.0 * 28.0f32.powf(color)).log2();
            let octaves = 0.15 + depth * 2.2;
            let stages = usize::from(self.params.stages).clamp(4, MAX_PHASER_STAGES);
            let denom = (stages - 1).max(1) as f32;
            let mut left = input_l + self.feedback_l * feedback;
            let mut right = input_r + self.feedback_r * feedback;
            for stage in 0..stages {
                let tilt = (stage as f32 / denom - 0.5) * 1.1;
                let coefficient = |sweep: f32| {
                    allpass_coefficient(phaser_hz(sweep + tilt, log2_center, octaves, SR), SR)
                };
                left = self.stages_l[stage].next(left, coefficient(sweep_l));
                right = self.stages_r[stage].next(right, coefficient(sweep_r));
            }
            self.feedback_l = left;
            self.feedback_r = right;
            // MOO-200's trim, which is not what this reference is for.
            let trim = resonance_trim(feedback);
            let (left, right) = (left * trim, right * trim);
            let hz = TONE_MIN_HZ * (TONE_MAX_HZ / TONE_MIN_HZ).powf(tone);
            self.tone_l.set_cutoff(hz, SR);
            self.tone_r.set_cutoff(hz, SR);
            (self.tone_l.next_sample(left), self.tone_r.next_sample(right))
        }
    }

    /// Noise under a 110 Hz saw: broadband, so every notch the sweep moves
    /// has something to cut.
    fn phaser_input(frames: usize) -> Vec<f32> {
        let mut noise = crate::osc::Noise::new(0x2350_1a7e);
        (0..frames)
            .map(|i| {
                let saw = 2.0 * (i as f32 * 110.0 / SR as f32).fract() - 1.0;
                0.3 * noise.next_sample() + 0.2 * saw
            })
            .collect()
    }

    /// The fastest, deepest sweep the knobs allow, with heavy feedback, so
    /// the notches are as sharp and move as fast as they can.
    fn fastest_phaser(stages: u8) -> ModulationParams {
        ModulationParams {
            mode: ModulationMode::Phaser,
            rate_hz: 12.0,
            depth: 1.0,
            color: 0.5,
            feedback: 0.85,
            spread: 0.5,
            tone: 1.0,
            stages,
            ..ModulationParams::default()
        }
    }

    /// **The control-rate phaser sounds like the per-sample one** (MOO-235).
    /// Two seconds of noise and a saw through the fastest, deepest, most
    /// resonant phaser the knobs allow, at 4, 8 and 12 stages, with Depth,
    /// Color and Tone moved mid-block and Rate changed on the way: the
    /// difference from the per-sample formula stays 66 dB under the wet
    /// signal, and no single sample is off by more than 0.002. Measured
    /// 2026-09-25: 81, 74 and 71 dB under, worst samples 3e-4, 7e-4 and
    /// 1e-3, at 4, 8 and 12 stages.
    #[test]
    fn control_rate_phaser_matches_the_per_sample_formula() {
        use crate::event::{Event, TimedEvent};

        const BLOCK: usize = 128;
        let blocks = 750;
        let input = phaser_input(BLOCK * blocks);
        // (block, offset, id, value)
        let moves = [
            (150usize, 37u32, MODULATION_PARAM_DEPTH, 0.35f32),
            (300, 5, MODULATION_PARAM_TONE, 0.4),
            (420, 100, MODULATION_PARAM_COLOR, 0.9),
            (500, 64, MODULATION_PARAM_RATE_HZ, 3.0),
            (600, 1, MODULATION_PARAM_DEPTH, 1.0),
        ];
        for stages in [4u8, 8, 12] {
            let params = fastest_phaser(stages);
            let mut effect = ModulationEffect::new(params, SR);
            let mut reference = PerSamplePhaser::new(params);
            let mut bus = StereoBus::with_capacity(BLOCK);
            let (mut diff, mut wet, mut worst) = (0.0f64, 0.0f64, 0.0f32);
            for block in 0..blocks {
                let chunk = &input[block * BLOCK..(block + 1) * BLOCK];
                bus.l[..BLOCK].copy_from_slice(chunk);
                bus.r[..BLOCK].copy_from_slice(chunk);
                let mut events = EventList::empty();
                for &(_, offset, id, value) in moves.iter().filter(|m| m.0 == block) {
                    assert!(events.push(TimedEvent {
                        offset,
                        event: Event::ParamValue { id, value },
                    }));
                }
                effect.process(&context(BLOCK), &mut bus, &events, None);
                for (i, &x) in chunk.iter().enumerate() {
                    for &(_, _, id, value) in
                        moves.iter().filter(|m| m.0 == block && m.1 as usize == i)
                    {
                        reference.apply(id, value);
                    }
                    let (l, r) = reference.sample(x, x);
                    for (got, want) in [(bus.l[i], l), (bus.r[i], r)] {
                        let error = got - want;
                        diff += f64::from(error * error);
                        wet += f64::from(want * want);
                        worst = worst.max(error.abs());
                    }
                }
            }
            let null_db = 10.0 * (diff / wet).log10();
            println!(
                "{stages} stages: the difference is {null_db:.1} dB under the wet signal, \
                 worst sample {worst:.2e}"
            );
            assert!(null_db < -66.0, "{stages} stages: the difference is only {null_db:.1} dB down");
            assert!(worst < 0.002, "{stages} stages: a sample is off by {worst}");
        }
    }

    /// A stretch in another mode, or a skipped block, moves the LFO without
    /// the phaser seeing it. Coming back, its coefficients have to start from
    /// where the sweep is, not where it was.
    #[test]
    fn the_phaser_catches_up_with_an_lfo_it_did_not_see_move() {
        const BLOCK: usize = 128;
        let params = fastest_phaser(8);
        let input = phaser_input(BLOCK * 40);
        for skip in [true, false] {
            let mut effect = ModulationEffect::new(params, SR);
            let mut bus = StereoBus::with_capacity(BLOCK);
            for block in 0..21 {
                let away = (10..20).contains(&block);
                if skip && away {
                    effect.skip_block(&context(BLOCK));
                    continue;
                }
                let mode = if away { ModulationMode::Chorus } else { ModulationMode::Phaser };
                effect.apply_param(MODULATION_PARAM_MODE, mode.to_index() as f32);
                let chunk = &input[block * BLOCK..(block + 1) * BLOCK];
                bus.l[..BLOCK].copy_from_slice(chunk);
                bus.r[..BLOCK].copy_from_slice(chunk);
                // One sample back in the phaser, so the first control point
                // after the gap is the one being judged.
                let frames = if block == 20 { 1 } else { BLOCK };
                effect.process(&context(frames), &mut bus, &EventList::empty(), None);
            }
            // The coefficient the next sample will use, against the one the
            // sweep says, read the way the per-sample formula reads it.
            let log2_center = PHASER_LOG2_BASE_HZ + PHASER_LOG2_COLOR_SPAN * params.color;
            let octaves = 0.15 + params.depth * 2.2;
            let sweep = effect.lfo.peek_offset(0.0, LfoWave::Sine);
            let truth =
                allpass_coefficient(phaser_hz(sweep + effect.tilt[0], log2_center, octaves, SR), SR);
            let now = effect.phaser_coeffs.now_l[0];
            assert!(
                (now - truth).abs() < 0.01,
                "skipped {skip}: stage 0 is at {now}, the sweep says {truth}"
            );
        }
    }

    /// **What the phaser costs against Chorus**, per 128-frame block, before
    /// and after MOO-235, in one run. Every row is played once per pass,
    /// `REPS` passes (default 7), and each block's cost is its fastest over
    /// the passes, so a burst of the shared box's other work drops out.
    ///
    /// ```sh
    /// cargo test -p mooloop-dsp --release --lib -- phaser_cost --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "measures wall time; run deliberately in release"]
    fn phaser_cost() {
        use std::hint::black_box;
        use std::time::Instant;

        const BLOCK: usize = 128;
        const BLOCKS: usize = 750;
        enum Device {
            Now(Box<ModulationEffect>),
            Before(Box<PerSamplePhaser>),
        }
        let reps = std::env::var("REPS")
            .ok()
            .and_then(|reps| reps.parse().ok())
            .unwrap_or(7usize)
            .max(1);
        let input = phaser_input(BLOCK * BLOCKS);
        let mut rows: Vec<(String, Device)> = vec![(
            "chorus (default)".into(),
            Device::Now(Box::new(ModulationEffect::new(ModulationParams::default(), SR))),
        )];
        for stages in [4u8, 8, 12] {
            let params = ModulationParams {
                mode: ModulationMode::Phaser,
                stages,
                ..ModulationParams::default()
            };
            rows.push((
                format!("phaser {stages:>2}, per sample (before)"),
                Device::Before(Box::new(PerSamplePhaser::new(params))),
            ));
            rows.push((
                format!("phaser {stages:>2}, control rate (after)"),
                Device::Now(Box::new(ModulationEffect::new(params, SR))),
            ));
        }
        let mut best = vec![vec![u128::MAX; BLOCKS]; rows.len()];
        let mut bus = StereoBus::with_capacity(BLOCK);
        for _ in 0..reps {
            for (row, (_, device)) in rows.iter_mut().enumerate() {
                for (block, best) in best[row].iter_mut().enumerate() {
                    let chunk = &input[block * BLOCK..(block + 1) * BLOCK];
                    bus.l[..BLOCK].copy_from_slice(chunk);
                    bus.r[..BLOCK].copy_from_slice(chunk);
                    let start = Instant::now();
                    match device {
                        Device::Now(effect) => {
                            effect.process(&context(BLOCK), &mut bus, &EventList::empty(), None)
                        }
                        Device::Before(reference) => {
                            for i in 0..BLOCK {
                                let (l, r) = reference.sample(bus.l[i], bus.r[i]);
                                bus.l[i] = l;
                                bus.r[i] = r;
                            }
                        }
                    }
                    black_box(&bus);
                    *best = (*best).min(start.elapsed().as_nanos());
                }
            }
        }
        let mean_us =
            |row: &[u128]| row.iter().sum::<u128>() as f64 / BLOCKS as f64 / 1_000.0;
        let chorus = mean_us(&best[0]);
        println!("{reps} passes, {BLOCKS} blocks of {BLOCK}, each block's fastest pass");
        for ((label, _), best) in rows.iter().zip(&best) {
            let us = mean_us(best);
            println!(
                "{label:<34} {us:7.2} us/block {:7.1} ns/frame {:5.2}x chorus",
                us * 1_000.0 / BLOCK as f64,
                us / chorus
            );
        }
    }

    /// **The trim leaves moderate feedback alone and caps the resonance**
    /// (MOO-200): unity up to `|fb| = 0.75`, and the loop's peak
    /// `1 / (1 - |fb|)` times the trim never past +12 dB, either sign.
    #[test]
    fn resonance_trim_is_unity_below_the_knee_and_caps_the_peak() {
        for step in -92..=92 {
            let fb = step as f32 / 100.0;
            let trim = resonance_trim(fb);
            if fb.abs() <= 0.75 {
                assert_eq!(trim, 1.0, "feedback {fb} was trimmed");
            }
            let peak = trim / (1.0 - fb.abs());
            assert!(peak <= RESONANCE_CEILING * 1.000_1, "feedback {fb} peaks at {peak}");
        }
        assert!((resonance_trim(0.92) - 0.32).abs() < 1.0e-5);
    }

    /// RMS of the flanger's impulse response in `[from_ms, to_ms)`, at a
    /// fixed delay (depth 0, the slowest rate), in dB.
    fn flanger_ring_db(feedback: f32, from_ms: f32, to_ms: f32) -> f32 {
        let frames = SR as usize / 8;
        let mut bus = StereoBus::with_capacity(frames);
        bus.l[0] = 1.0;
        bus.r[0] = 1.0;
        let mut effect = ModulationEffect::new(
            ModulationParams {
                mode: ModulationMode::Flange,
                rate_hz: 0.02,
                depth: 0.0,
                color: 1.0,
                feedback,
                tone: 1.0,
                ..ModulationParams::default()
            },
            SR,
        );
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);
        let at = |ms: f32| (ms * SR as f32 / 1_000.0) as usize;
        crate::testkit::db(crate::testkit::rms(&bus.l[at(from_ms)..at(to_ms)]))
    }

    /// **The jet survives the trim** (MOO-200). A flanger at full feedback
    /// still rings: 80 ms on, its impulse response has fallen far less from
    /// its first echoes than at the trim's knee, because the trim scales the
    /// output and never the loop. Lowering the knob's top would fail this.
    #[test]
    fn a_flanger_at_full_feedback_still_rings() {
        let decay = |fb: f32| flanger_ring_db(fb, 85.0, 105.0) - flanger_ring_db(fb, 5.0, 25.0);
        let (full, knee) = (decay(0.92), decay(0.75));
        println!("80 ms of ring: {full:.1} dB at 0.92, {knee:.1} dB at 0.75");
        assert!(full > -30.0, "the full-feedback flanger stopped ringing: {full:.1} dB");
        assert!(
            full > knee + 12.0,
            "past the knee the ring should keep lengthening: {full:.1} against {knee:.1} dB"
        );
    }
}
