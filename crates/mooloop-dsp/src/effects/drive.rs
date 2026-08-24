//! Drive / saturation. Four shaping curves, a post-shaper spectral tilt, and
//! a dry/wet blend.
//!
//! Runs 2x oversampled (see `crate::shaper`): a memoryless nonlinearity
//! generates harmonics above the input spectrum, and at base rate those fold
//! back down as inharmonic fizz that does not track pitch.

use mooloop_core::{
    DriveCurve, DriveParams, DRIVE_PARAM_CURVE, DRIVE_PARAM_DRIVE, DRIVE_PARAM_MIX,
    DRIVE_PARAM_OUTPUT, DRIVE_PARAM_TONE,
};

use crate::bus::StereoBus;
use crate::event::{Event, EventList};
use crate::node::{AudioNode, ProcessContext};
use crate::shaper::{drive_compensation, shape, Oversampler2x, OVERSAMPLER_LATENCY_FRAMES};
use crate::smooth::Smoothed;

/// Corner frequency of the tilt filter's low/high split.
const TONE_SPLIT_HZ: f32 = 1_500.0;
/// How much the tilt can boost the high band at `tone == 1`.
const TONE_MAX_BOOST: f32 = 3.0;
/// Time constant for drive, tone, mix, and output: all scale amplitude or
/// harmonic balance directly, so a step here is either a click or zipper.
const PARAM_SMOOTH_S: f32 = 0.005;

pub struct DriveEffect {
    params: DriveParams,
    sample_rate: u32,
    left: Oversampler2x,
    right: Oversampler2x,
    /// The wet oversampling path is delayed. Keep the dry path at the same
    /// arrival time so partial mixes do not comb-filter inside the effect.
    dry_l: [f32; OVERSAMPLER_LATENCY_FRAMES],
    dry_r: [f32; OVERSAMPLER_LATENCY_FRAMES],
    dry_pos: usize,
    /// One-pole low-pass state for the tone tilt, per channel.
    tone_lp_l: f32,
    tone_lp_r: f32,
    tone_coeff: f32,
    drive: Smoothed,
    tone: Smoothed,
    mix: Smoothed,
    output: Smoothed,
}

impl DriveEffect {
    pub fn new(params: DriveParams, sample_rate: u32) -> Self {
        let smoothed = |initial| Smoothed::new(initial, PARAM_SMOOTH_S, sample_rate);
        Self {
            params,
            sample_rate,
            left: Oversampler2x::new(),
            right: Oversampler2x::new(),
            dry_l: [0.0; OVERSAMPLER_LATENCY_FRAMES],
            dry_r: [0.0; OVERSAMPLER_LATENCY_FRAMES],
            dry_pos: 0,
            tone_lp_l: 0.0,
            tone_lp_r: 0.0,
            tone_coeff: tone_coeff(sample_rate),
            drive: smoothed(params.drive.clamp(1.0, 64.0)),
            tone: smoothed(params.tone.clamp(-1.0, 1.0)),
            mix: smoothed(params.mix.clamp(0.0, 1.0)),
            output: smoothed(params.output.clamp(0.0, 2.0)),
        }
    }

    pub fn params(&self) -> DriveParams {
        self.params
    }

    /// Replace the parameter set wholesale (project load) — jump straight to
    /// the new values, there is nothing to click coming from a fresh load.
    pub fn set_params(&mut self, params: DriveParams) {
        self.params = params;
        self.drive.reset_to(params.drive.clamp(1.0, 64.0));
        self.tone.reset_to(params.tone.clamp(-1.0, 1.0));
        self.mix.reset_to(params.mix.clamp(0.0, 1.0));
        self.output.reset_to(params.output.clamp(0.0, 2.0));
    }

    fn apply_param(&mut self, id: u32, value: f32) {
        match id {
            DRIVE_PARAM_DRIVE => {
                self.params.drive = value.clamp(1.0, 64.0);
                self.drive.set_target(self.params.drive);
            }
            DRIVE_PARAM_CURVE => self.params.curve = DriveCurve::from_index(value.round() as i32),
            DRIVE_PARAM_TONE => {
                self.params.tone = value.clamp(-1.0, 1.0);
                self.tone.set_target(self.params.tone);
            }
            DRIVE_PARAM_MIX => {
                self.params.mix = value.clamp(0.0, 1.0);
                self.mix.set_target(self.params.mix);
            }
            DRIVE_PARAM_OUTPUT => {
                self.params.output = value.clamp(0.0, 2.0);
                self.output.set_target(self.params.output);
            }
            _ => {}
        }
    }

    /// Split the low and high bands and re-weight the high one. At `tone == 0`
    /// the two bands sum back to the input exactly.
    fn tilt(&self, sample: f32, state: &mut f32, tone: f32) -> f32 {
        *state = *state + (sample - *state) * self.tone_coeff;
        let low = *state;
        let high = sample - low;
        let gain = if tone >= 0.0 {
            1.0 + tone * TONE_MAX_BOOST
        } else {
            1.0 + tone
        };
        low + high * gain
    }

    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        let curve = self.params.curve;

        for i in start..end {
            let drive = self.drive.advance();
            let tone = self.tone.advance();
            let mix = self.mix.advance();
            let output = self.output.advance();
            let compensation = drive_compensation(curve, drive);
            let (dry_l, dry_r) = (bus.l[i], bus.r[i]);

            let wet_l = self.left.process(dry_l, |x| shape(curve, x * drive)) * compensation;
            let wet_r = self.right.process(dry_r, |x| shape(curve, x * drive)) * compensation;

            let mut tone_l = self.tone_lp_l;
            let mut tone_r = self.tone_lp_r;
            let wet_l = self.tilt(wet_l, &mut tone_l, tone);
            let wet_r = self.tilt(wet_r, &mut tone_r, tone);
            self.tone_lp_l = tone_l;
            self.tone_lp_r = tone_r;

            let aligned_l = self.dry_l[self.dry_pos];
            let aligned_r = self.dry_r[self.dry_pos];
            self.dry_l[self.dry_pos] = dry_l;
            self.dry_r[self.dry_pos] = dry_r;
            self.dry_pos = (self.dry_pos + 1) % OVERSAMPLER_LATENCY_FRAMES;

            bus.l[i] = (aligned_l + (wet_l - aligned_l) * mix) * output;
            bus.r[i] = (aligned_r + (wet_r - aligned_r) * mix) * output;
        }
    }
}

fn tone_coeff(sample_rate: u32) -> f32 {
    let sr = sample_rate.max(1) as f32;
    1.0 - (-core::f32::consts::TAU * TONE_SPLIT_HZ / sr).exp()
}

impl AudioNode for DriveEffect {
    fn latency_frames(&self) -> u32 {
        OVERSAMPLER_LATENCY_FRAMES as u32
    }

    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        if ctx.sample_rate != self.sample_rate {
            self.sample_rate = ctx.sample_rate;
            self.tone_coeff = tone_coeff(ctx.sample_rate);
            self.drive.set_time(PARAM_SMOOTH_S, ctx.sample_rate);
            self.tone.set_time(PARAM_SMOOTH_S, ctx.sample_rate);
            self.mix.set_time(PARAM_SMOOTH_S, ctx.sample_rate);
            self.output.set_time(PARAM_SMOOTH_S, ctx.sample_rate);
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
    use crate::event::TimedEvent;

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

    fn sine_bus(frames: usize, freq: f32, amplitude: f32) -> StereoBus {
        let mut bus = StereoBus::with_capacity(frames);
        for i in 0..frames {
            let s = (i as f32 / 48_000.0 * freq * core::f32::consts::TAU).sin() * amplitude;
            bus.l[i] = s;
            bus.r[i] = s;
        }
        bus
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn hard_clipping_flattens_peaks() {
        let frames = 4_096;
        let mut bus = sine_bus(frames, 220.0, 1.0);
        let mut effect = DriveEffect::new(
            DriveParams {
                drive: 8.0,
                curve: DriveCurve::Hard,
                ..DriveParams::default()
            },
            48_000,
        );
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);
        // A hard-clipped sine approaches a square wave: its RMS climbs toward
        // its peak instead of sitting at peak/sqrt(2).
        let settled = &bus.l[1_024..frames];
        let peak = settled.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(
            rms(settled) > peak * 0.85,
            "expected a near-square wave, got rms {} vs peak {peak}",
            rms(settled)
        );
    }

    #[test]
    fn zero_mix_preserves_the_signal_at_the_declared_latency() {
        let frames = 2_048;
        let mut bus = sine_bus(frames, 440.0, 0.5);
        let reference = bus.l[..frames].to_vec();
        let mut effect = DriveEffect::new(
            DriveParams {
                drive: 32.0,
                mix: 0.0,
                ..DriveParams::default()
            },
            48_000,
        );
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);
        assert_eq!(effect.latency_frames(), OVERSAMPLER_LATENCY_FRAMES as u32);
        assert!(bus.l[..OVERSAMPLER_LATENCY_FRAMES]
            .iter()
            .all(|sample| sample.abs() < 1e-6));
        for i in OVERSAMPLER_LATENCY_FRAMES..frames {
            let expected = reference[i - OVERSAMPLER_LATENCY_FRAMES];
            assert!(
                (bus.l[i] - expected).abs() < 1e-6,
                "dry path altered at {i}: {} vs {expected}",
                bus.l[i],
            );
        }
    }

    #[test]
    fn tone_tilts_the_balance_without_changing_at_zero() {
        let frames = 8_192;
        // Sum of a low and a high tone; the tilt should move their ratio.
        let make = || {
            let mut bus = StereoBus::with_capacity(frames);
            for i in 0..frames {
                let t = i as f32 / 48_000.0;
                let s = (t * 150.0 * core::f32::consts::TAU).sin() * 0.3
                    + (t * 9_000.0 * core::f32::consts::TAU).sin() * 0.3;
                bus.l[i] = s;
                bus.r[i] = s;
            }
            bus
        };
        let run = |tone: f32| {
            let mut bus = make();
            let mut effect = DriveEffect::new(
                DriveParams {
                    drive: 1.0,
                    curve: DriveCurve::Soft,
                    tone,
                    ..DriveParams::default()
                },
                48_000,
            );
            effect.process(&context(frames), &mut bus, &EventList::empty(), None);
            bus.l[2_048..frames].to_vec()
        };
        let dark = rms(&run(-1.0));
        let flat = rms(&run(0.0));
        let bright = rms(&run(1.0));
        assert!(
            dark < flat && flat < bright,
            "tilt did not order: dark {dark}, flat {flat}, bright {bright}"
        );
    }

    #[test]
    fn param_events_take_effect_mid_block() {
        let frames = 8_192;
        let mut bus = sine_bus(frames, 300.0, 0.2);
        let mut effect = DriveEffect::new(
            DriveParams {
                drive: 1.0,
                curve: DriveCurve::Hard,
                ..DriveParams::default()
            },
            48_000,
        );
        let mut events = EventList::empty();
        assert!(events.push(TimedEvent {
            offset: (frames / 2) as u32,
            event: Event::ParamValue {
                id: DRIVE_PARAM_DRIVE,
                value: 64.0,
            },
        }));
        effect.process(&context(frames), &mut bus, &events, None);
        let before = rms(&bus.l[frames / 4..frames / 2]);
        let after = rms(&bus.l[3 * frames / 4..frames]);
        assert!(
            after > before * 2.0,
            "drive rise had no effect: {before} then {after}"
        );
    }

    #[test]
    fn every_curve_stays_finite_and_bounded_under_extreme_drive() {
        for curve in [
            DriveCurve::Soft,
            DriveCurve::Hard,
            DriveCurve::Fold,
            DriveCurve::Tape,
        ] {
            let frames = 2_048;
            let mut bus = sine_bus(frames, 110.0, 1.0);
            let mut effect = DriveEffect::new(
                DriveParams {
                    drive: 64.0,
                    curve,
                    ..DriveParams::default()
                },
                48_000,
            );
            effect.process(&context(frames), &mut bus, &EventList::empty(), None);
            for i in 0..frames {
                assert!(
                    bus.l[i].is_finite() && bus.l[i].abs() < 8.0,
                    "{curve:?} produced {} at {i}",
                    bus.l[i]
                );
            }
        }
    }

    #[test]
    fn mix_change_mid_block_does_not_click() {
        // Soft-clipped, not hard: the waveform itself should have no sharp
        // corners, so any spike at the event boundary is the mix step, not
        // the shaper's own natural slope.
        let frames = 8_192;
        let mut bus = sine_bus(frames, 200.0, 0.5);
        let mut effect = DriveEffect::new(
            DriveParams {
                drive: 4.0,
                curve: DriveCurve::Soft,
                mix: 0.0,
                ..DriveParams::default()
            },
            48_000,
        );
        let mut events = EventList::empty();
        assert!(events.push(TimedEvent {
            offset: (frames / 2) as u32,
            event: Event::ParamValue {
                id: DRIVE_PARAM_MIX,
                value: 1.0,
            },
        }));
        effect.process(&context(frames), &mut bus, &events, None);
        let step = |i: usize| (bus.l[i] - bus.l[i - 1]).abs();
        let steady_state = (frames / 4..frames / 2 - 1).map(step).fold(0.0f32, f32::max);
        let at_boundary = (frames / 2..frames / 2 + 32).map(step).fold(0.0f32, f32::max);
        assert!(
            at_boundary < steady_state * 3.0 + 0.02,
            "mix change left a discontinuity of {at_boundary} vs steady-state {steady_state}"
        );
    }
}
