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
use crate::event::{Event, EventList};
use crate::filter::{AllPass, OnePoleLp};
use crate::lfo::Lfo;
use crate::node::{AudioNode, ProcessContext};
use crate::smooth::Smoothed;

const MAX_DELAY_MS: f32 = 64.0;
const MAX_PHASER_STAGES: usize = 12;
const TONE_MIN_HZ: f32 = 350.0;
const TONE_MAX_HZ: f32 = 20_000.0;
/// Time constant for depth, feedback, spread, tone, and color: all continuous
/// and audible, all currently stepped once per block on every knob move.
/// Rate is deliberately excluded — it feeds a phase increment, so a step in
/// rate is not a step in output. Stages and mode are discrete.
const PARAM_SMOOTH_S: f32 = 0.01;

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
    depth: Smoothed,
    feedback: Smoothed,
    spread: Smoothed,
    tone: Smoothed,
    color: Smoothed,
}

impl ModulationEffect {
    pub fn new(params: ModulationParams, sample_rate: u32) -> Self {
        let smoothed = |initial| Smoothed::new(initial, PARAM_SMOOTH_S, sample_rate);
        Self {
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
            depth: smoothed(params.depth.clamp(0.0, 1.0)),
            feedback: smoothed(params.feedback.clamp(-0.92, 0.92)),
            spread: smoothed(params.spread.clamp(0.0, 1.0)),
            tone: smoothed(params.tone.clamp(0.0, 1.0)),
            color: smoothed(params.color.clamp(0.0, 1.0)),
        }
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
    }

    fn apply_param(&mut self, id: u32, value: f32) {
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
            MODULATION_PARAM_STAGES => self.params.stages = value.round().clamp(4.0, 12.0) as u8,
            _ => {}
        }
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

    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        for i in start..end {
            let depth = self.depth.advance();
            let feedback = self.feedback.advance();
            let spread = self.spread.advance();
            let tone = self.tone.advance();
            let color = self.color.advance();
            // Two taps of one LFO cycle rather than two independently
            // drifting oscillators, so the stereo image stays locked even
            // as `spread` moves. Peek both before advancing once.
            let sweep_l = self.lfo.peek_offset(0.0, LfoWave::Sine);
            let sweep_r = self.lfo.peek_offset(spread * 0.25, LfoWave::Sine);
            self.lfo.skip(1, self.params.rate_hz, self.sample_rate);
            let (input_l, input_r) = (bus.l[i], bus.r[i]);
            let (wet_l, wet_r) = match self.params.mode {
                ModulationMode::Phaser => self.phaser_sample(
                    input_l, input_r, feedback, sweep_l, sweep_r, depth, color, tone,
                ),
                mode => self.delay_sample(
                    input_l, input_r, mode, depth, feedback, spread, sweep_l, sweep_r, color, tone,
                ),
            };
            bus.l[i] = wet_l;
            bus.r[i] = wet_r;
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
        self.tone_filter(wet_l, wet_r, tone)
    }

    #[allow(clippy::too_many_arguments)]
    fn phaser_sample(
        &mut self,
        input_l: f32,
        input_r: f32,
        feedback: f32,
        sweep_l: f32,
        sweep_r: f32,
        depth: f32,
        color: f32,
        tone: f32,
    ) -> (f32, f32) {
        let stages = usize::from(self.params.stages).clamp(4, MAX_PHASER_STAGES);
        let mut left = input_l + self.feedback_l * feedback;
        let mut right = input_r + self.feedback_r * feedback;
        for stage in 0..stages {
            let tilt = (stage as f32 / (stages - 1).max(1) as f32 - 0.5) * 1.1;
            left = self.phaser_l[stage].next(
                left,
                allpass_coefficient(
                    self.phaser_hz(sweep_l + tilt, depth, color),
                    self.sample_rate,
                ),
            );
            right = self.phaser_r[stage].next(
                right,
                allpass_coefficient(
                    self.phaser_hz(sweep_r + tilt, depth, color),
                    self.sample_rate,
                ),
            );
        }
        self.feedback_l = left;
        self.feedback_r = right;
        self.tone_filter(left, right, tone)
    }

    fn phaser_hz(&self, lfo: f32, depth: f32, color: f32) -> f32 {
        let center = 220.0 * 28.0f32.powf(color);
        let octaves = 0.15 + depth * 2.2;
        (center * 2.0f32.powf(lfo * octaves)).clamp(60.0, self.sample_rate as f32 * 0.42)
    }

    fn tone_filter(&mut self, left: f32, right: f32, tone: f32) -> (f32, f32) {
        let hz = TONE_MIN_HZ * (TONE_MAX_HZ / TONE_MIN_HZ).powf(tone);
        self.tone_l.set_cutoff(hz, self.sample_rate.max(1));
        self.tone_r.set_cutoff(hz, self.sample_rate.max(1));
        (self.tone_l.next_sample(left), self.tone_r.next_sample(right))
    }
}

fn ring_frames(sample_rate: u32) -> usize {
    (MAX_DELAY_MS * sample_rate as f32 / 1_000.0) as usize + 8
}

fn allpass_coefficient(hz: f32, sample_rate: u32) -> f32 {
    let g = (core::f32::consts::PI * hz / sample_rate.max(1) as f32).tan();
    ((1.0 - g) / (1.0 + g)).clamp(-0.999, 0.999)
}

impl AudioNode for ModulationEffect {
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
        }
        let frames = ctx.frames.min(bus.capacity());
        let mut position = 0;
        for event in events_in.iter() {
            let offset = (event.offset as usize).min(frames).max(position);
            self.process_range(bus, position, offset);
            if let Event::ParamValue { id, value } = event.event {
                self.apply_param(id, value);
            }
            position = offset;
        }
        self.process_range(bus, position, frames);
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
        use crate::event::TimedEvent;

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
        let max_step = (1..frames)
            .map(|i| (bus.l[i] - bus.l[i - 1]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_step < 0.2,
            "depth change left a discontinuity of {max_step}"
        );
    }
}
