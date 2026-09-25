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
use crate::event::EventList;
use crate::filter::OnePoleLp;
use crate::node::{AudioNode, ProcessContext};
use crate::shaper::{
    reference_drive_compensation, shape_oversampled, Oversampler2x, OVERSAMPLER_LATENCY_FRAMES,
};
use crate::smooth::Smoothed;
use super::{process_param_split, RangeProcessor};

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
    /// One-pole low-pass for the tone tilt, per channel.
    tone_lp_l: OnePoleLp,
    tone_lp_r: OnePoleLp,
    drive: Smoothed,
    tone: Smoothed,
    mix: Smoothed,
    output: Smoothed,
    /// `reference_drive_compensation(compensation_for.0, compensation_for.1)`,
    /// kept while the curve and the smoothed drive stay where they were, so
    /// a drive at rest costs no `tanh` a frame for it (MOO-251). The same
    /// number either way, to the bit: it is the same call on the same
    /// arguments. The drive of `NaN` in `new` matches nothing, so the first
    /// frame computes it.
    compensation: f32,
    compensation_for: (DriveCurve, f32),
}

impl DriveEffect {
    pub fn new(params: DriveParams, sample_rate: u32) -> Self {
        let smoothed = |initial| Smoothed::new(initial, PARAM_SMOOTH_S, sample_rate);
        let mut tone_lp_l = OnePoleLp::new();
        let mut tone_lp_r = OnePoleLp::new();
        tone_lp_l.set_cutoff(TONE_SPLIT_HZ, sample_rate);
        tone_lp_r.set_cutoff(TONE_SPLIT_HZ, sample_rate);
        Self {
            params,
            sample_rate,
            left: Oversampler2x::new(),
            right: Oversampler2x::new(),
            dry_l: [0.0; OVERSAMPLER_LATENCY_FRAMES],
            dry_r: [0.0; OVERSAMPLER_LATENCY_FRAMES],
            dry_pos: 0,
            tone_lp_l,
            tone_lp_r,
            drive: smoothed(params.drive.clamp(1.0, 64.0)),
            tone: smoothed(params.tone.clamp(-1.0, 1.0)),
            mix: smoothed(params.mix.clamp(0.0, 1.0)),
            output: smoothed(params.output.clamp(0.0, 2.0)),
            compensation: 1.0,
            compensation_for: (params.curve, f32::NAN),
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

    /// Split the low and high bands and re-weight the high one. At `tone == 0`
    /// the two bands sum back to the input exactly.
    fn tilt(state: &mut OnePoleLp, sample: f32, tone: f32) -> f32 {
        let low = state.next_sample(sample);
        let high = sample - low;
        let gain = if tone >= 0.0 {
            1.0 + tone * TONE_MAX_BOOST
        } else {
            1.0 + tone
        };
        low + high * gain
    }

}

impl RangeProcessor for DriveEffect {
    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        let curve = self.params.curve;

        for i in start..end {
            let drive = self.drive.advance();
            let tone = self.tone.advance();
            let mix = self.mix.advance();
            let output = self.output.advance();
            if (curve, drive) != self.compensation_for {
                self.compensation = reference_drive_compensation(curve, drive);
                self.compensation_for = (curve, drive);
            }
            let compensation = self.compensation;
            let (dry_l, dry_r) = (bus.l[i], bus.r[i]);

            let wet_l =
                self.left.process(dry_l, |x| shape_oversampled(curve, x * drive)) * compensation;
            let wet_r =
                self.right.process(dry_r, |x| shape_oversampled(curve, x * drive)) * compensation;

            let wet_l = Self::tilt(&mut self.tone_lp_l, wet_l, tone);
            let wet_r = Self::tilt(&mut self.tone_lp_r, wet_r, tone);

            let aligned_l = self.dry_l[self.dry_pos];
            let aligned_r = self.dry_r[self.dry_pos];
            self.dry_l[self.dry_pos] = dry_l;
            self.dry_r[self.dry_pos] = dry_r;
            self.dry_pos += 1;
            if self.dry_pos == OVERSAMPLER_LATENCY_FRAMES {
                self.dry_pos = 0;
            }

            bus.l[i] = (aligned_l + (wet_l - aligned_l) * mix) * output;
            bus.r[i] = (aligned_r + (wet_r - aligned_r) * mix) * output;
        }
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
}

impl AudioNode for DriveEffect {
    fn latency_frames(&self) -> u32 {
        OVERSAMPLER_LATENCY_FRAMES as u32
    }

    /// Three things hold audio here after the input stops: the oversampler's
    /// two 31-tap half-band FIR histories, which between them span about 31
    /// base-rate frames; the internal ring that realigns the dry path against them,
    /// which is exactly `OVERSAMPLER_LATENCY_FRAMES`; and the 1.5 kHz tilt
    /// one-pole, whose free response needs `ln(REST_EPSILON) / ln(e^(-2 pi f
    /// / fs))` samples — about 106 at 48 kHz, and proportional to the rate.
    ///
    /// Twenty milliseconds is over five times their sum at every supported
    /// rate. Nothing here is worth deriving to the frame: the whole tail is a
    /// fraction of a millisecond of audio, and the cost of over-reporting it
    /// is one extra block of a cheap effect.
    fn tail_frames(&self) -> u32 {
        // Twenty milliseconds is shorter than a knob's lag takes to snap from
        // one end of the drive range to the other, so a smoother in flight is
        // its own reason to keep running.
        if !self.drive.is_settled()
            || !self.tone.is_settled()
            || !self.mix.is_settled()
            || !self.output.is_settled()
        {
            return u32::MAX;
        }
        (self.sample_rate / 50).saturating_add(OVERSAMPLER_LATENCY_FRAMES as u32)
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
            self.tone_lp_l.set_cutoff(TONE_SPLIT_HZ, ctx.sample_rate);
            self.tone_lp_r.set_cutoff(TONE_SPLIT_HZ, ctx.sample_rate);
            self.drive.set_time(PARAM_SMOOTH_S, ctx.sample_rate);
            self.tone.set_time(PARAM_SMOOTH_S, ctx.sample_rate);
            self.mix.set_time(PARAM_SMOOTH_S, ctx.sample_rate);
            self.output.set_time(PARAM_SMOOTH_S, ctx.sample_rate);
        }
        let frames = ctx.frames.min(bus.capacity());
        process_param_split(self, bus, events_in, frames);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::rms;
    use crate::event::{Event, TimedEvent};

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
        // Measured as character rather than level, because the compensation
        // holds the level: a hard-clipped sine squares off, so its RMS climbs
        // from peak/sqrt(2) toward its peak.
        let squareness = |samples: &[f32]| {
            let peak = samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
            rms(samples) / peak
        };
        let before = squareness(&bus.l[frames / 4..frames / 2]);
        let after = squareness(&bus.l[3 * frames / 4..frames]);
        assert!(
            before < 0.75 && after > 0.9,
            "drive rise had no effect: rms/peak {before} then {after}"
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

    /// The gain-structure contract (`docs/GAIN_STRUCTURE.md`): at the
    /// operating level, raising drive changes character, not level. Signals
    /// arrive near `REFERENCE_PEAK_DBFS`, so that is where the compensation
    /// has to hold the peak -- anchored at full scale instead, the default
    /// drive of 2 was +5.6 dB of plain volume on a Soft curve and drive 8
    /// nearly +12 dB, so the knob was mostly a volume knob.
    #[test]
    fn drive_holds_a_reference_level_peak_on_every_curve() {
        let reference = 10.0_f32.powf(mooloop_core::gain::REFERENCE_PEAK_DBFS / 20.0);
        let frames = 9_600;
        for curve in [
            DriveCurve::Soft,
            DriveCurve::Hard,
            DriveCurve::Fold,
            DriveCurve::Tape,
        ] {
            for drive in [1.0_f32, 2.0, 4.0, 8.0, 32.0, 64.0] {
                let mut bus = sine_bus(frames, 220.0, reference);
                let mut effect = DriveEffect::new(
                    DriveParams {
                        drive,
                        curve,
                        ..DriveParams::default()
                    },
                    48_000,
                );
                effect.process(&context(frames), &mut bus, &EventList::empty(), None);
                let settled = &bus.l[frames / 2..];
                let peak = settled.iter().fold(0.0f32, |a, s| a.max(s.abs()));
                let change_db = 20.0 * (peak / reference).log10();
                assert!(
                    change_db.abs() <= 1.0,
                    "{curve:?} at drive {drive} moved a reference-level peak by {change_db:+.2} dB"
                );
            }
        }
    }

    /// **Caching the compensation changes nothing, to the bit** (MOO-251).
    /// The loop as it was, with `reference_drive_compensation` called every
    /// frame and the dry ring wrapped by `%`, against the device, on every
    /// curve, with Drive moved twice and the curve switched mid-block.
    #[test]
    fn a_cached_compensation_is_bit_identical_to_one_per_frame() {
        let frames = 4_096;
        let moves = [
            (700u32, DRIVE_PARAM_DRIVE, 24.0f32),
            (1_900, DRIVE_PARAM_CURVE, 3.0),
            (2_500, DRIVE_PARAM_DRIVE, 3.0),
            (3_300, DRIVE_PARAM_CURVE, 0.0),
        ];
        for curve in [DriveCurve::Soft, DriveCurve::Hard, DriveCurve::Fold, DriveCurve::Tape] {
            let params = DriveParams {
                drive: 6.0,
                curve,
                tone: 0.3,
                mix: 0.8,
                ..DriveParams::default()
            };
            let mut bus = sine_bus(frames, 330.0, 0.4);
            let input = bus.l[..frames].to_vec();
            let mut events = EventList::empty();
            for (offset, id, value) in moves {
                assert!(events.push(TimedEvent {
                    offset,
                    event: Event::ParamValue { id, value },
                }));
            }
            let mut effect = DriveEffect::new(params, 48_000);
            effect.process(&context(frames), &mut bus, &events, None);

            // The same device, driven one frame at a time through the old
            // per-frame compensation.
            let mut reference = DriveEffect::new(params, 48_000);
            let mut dry = [0.0f32; OVERSAMPLER_LATENCY_FRAMES];
            let mut dry_pos = 0usize;
            for (i, &x) in input.iter().enumerate() {
                for &(_, id, value) in moves.iter().filter(|m| m.0 as usize == i) {
                    reference.apply_param(id, value);
                }
                let curve = reference.params.curve;
                let drive = reference.drive.advance();
                let tone = reference.tone.advance();
                let mix = reference.mix.advance();
                let output = reference.output.advance();
                let compensation = reference_drive_compensation(curve, drive);
                let wet = reference.left.process(x, |v| shape_oversampled(curve, v * drive)) * compensation;
                let wet = DriveEffect::tilt(&mut reference.tone_lp_l, wet, tone);
                let aligned = dry[dry_pos];
                dry[dry_pos] = x;
                dry_pos = (dry_pos + 1) % OVERSAMPLER_LATENCY_FRAMES;
                let expected = (aligned + (wet - aligned) * mix) * output;
                assert_eq!(
                    bus.l[i].to_bits(),
                    expected.to_bits(),
                    "{curve:?} at frame {i}: {} against {expected}",
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

    /// The Drive loop as it was before MOO-250 and MOO-251: the 32-tap
    /// oversampler, libm `tanh` inside it, and the compensation worked out
    /// every frame. Kept to measure the device against, for sound and cost.
    struct DriveBefore {
        effect: DriveEffect,
        left: crate::shaper::before_moo250::Oversampler2x,
        right: crate::shaper::before_moo250::Oversampler2x,
        dry_l: [f32; OVERSAMPLER_LATENCY_FRAMES],
        dry_r: [f32; OVERSAMPLER_LATENCY_FRAMES],
        dry_pos: usize,
    }

    impl DriveBefore {
        fn new(params: DriveParams) -> Self {
            Self {
                effect: DriveEffect::new(params, 48_000),
                left: crate::shaper::before_moo250::Oversampler2x::new(),
                right: crate::shaper::before_moo250::Oversampler2x::new(),
                dry_l: [0.0; OVERSAMPLER_LATENCY_FRAMES],
                dry_r: [0.0; OVERSAMPLER_LATENCY_FRAMES],
                dry_pos: 0,
            }
        }

        fn process(&mut self, bus: &mut StereoBus, frames: usize) {
            let e = &mut self.effect;
            let curve = e.params.curve;
            for i in 0..frames {
                let drive = e.drive.advance();
                let tone = e.tone.advance();
                let mix = e.mix.advance();
                let output = e.output.advance();
                let compensation = reference_drive_compensation(curve, drive);
                let (dry_l, dry_r) = (bus.l[i], bus.r[i]);
                let wet_l = self.left.process(dry_l, |x| crate::shaper::shape(curve, x * drive))
                    * compensation;
                let wet_r = self.right.process(dry_r, |x| crate::shaper::shape(curve, x * drive))
                    * compensation;
                let wet_l = DriveEffect::tilt(&mut e.tone_lp_l, wet_l, tone);
                let wet_r = DriveEffect::tilt(&mut e.tone_lp_r, wet_r, tone);
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

    /// The factory settings the cost and sound comparisons use: the
    /// default (Soft at 2), Tape Warmth, Soft Push and Hard Clip.
    fn drive_rows() -> [(&'static str, DriveParams); 4] {
        let with = |curve, drive, tone, mix| DriveParams {
            curve,
            drive,
            tone,
            mix,
            ..DriveParams::default()
        };
        [
            ("default (Soft 2)", DriveParams::default()),
            ("Tape Warmth", with(DriveCurve::Tape, 3.0, -0.2, 0.7)),
            ("Soft Push", with(DriveCurve::Soft, 4.0, 0.1, 1.0)),
            ("Hard Clip", with(DriveCurve::Hard, 12.0, 0.0, 1.0)),
        ]
    }

    /// A chord of saws at the operating level: harmonics all the way up, the
    /// way a Drive is actually fed.
    fn chord_input(frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| {
                [110.0f32, 164.8, 220.0, 277.2]
                    .iter()
                    .map(|hz| 2.0 * (i as f32 * hz / 48_000.0).fract() - 1.0)
                    .sum::<f32>()
                    * 0.06
            })
            .collect()
    }

    /// **The Drive sounds as it did** (MOO-250): the device against the loop
    /// before the half-band oversampler, the Padé `tanh` and the cached
    /// compensation, on a chord of saws at every factory setting above. For
    /// the smooth curves the difference is held 60 dB under the output, and
    /// 80 dB under it below 15 kHz; measured 2026-09-25 at 64 to 70 dB and 84
    /// to 91 dB. Hard Clip is held at 40 dB (measured 45): see below.
    #[test]
    fn the_drive_matches_the_loop_before_the_oversampler_changed() {
        let frames = 24_000;
        let input = chord_input(frames);
        for (label, params) in drive_rows() {
            let mut now = StereoBus::with_capacity(frames);
            now.l[..frames].copy_from_slice(&input);
            now.r[..frames].copy_from_slice(&input);
            let mut before = StereoBus::with_capacity(frames);
            before.l[..frames].copy_from_slice(&input);
            before.r[..frames].copy_from_slice(&input);
            DriveEffect::new(params, 48_000).process(
                &context(frames),
                &mut now,
                &EventList::empty(),
                None,
            );
            DriveBefore::new(params).process(&mut before, frames);
            let error: Vec<f32> = (0..frames).map(|i| now.l[i] - before.l[i]).collect();
            let full = crate::testkit::db(rms(&error) / rms(&before.l[..frames]));
            let band = (20.0, 15_000.0);
            let audible = crate::testkit::db(
                crate::testkit::band_rms(&error, 48_000, band)
                    / crate::testkit::band_rms(&before.l[..frames], 48_000, band),
            );
            println!("{label}: {full:.1} dB from the loop before, {audible:.1} dB below 15 kHz");
            if params.curve == DriveCurve::Hard {
                // A hard clip at drive 12 puts harmonics far past the 2x
                // rate's Nyquist, and what folds back in is set by each
                // kernel's stopband, which differ. So the two paths differ by
                // their aliasing, at every frequency; how loud the new path's
                // aliasing is against the old is
                // `oversampling_reduces_aliasing_below_the_fundamental`'s to
                // say, and it is no louder.
                assert!(full < -40.0, "{label}: only {full:.1} dB from before");
            } else {
                assert!(full < -60.0, "{label}: only {full:.1} dB from before");
                assert!(audible < -80.0, "{label}: only {audible:.1} dB from before below 15 kHz");
            }
        }
    }

    /// **What a Drive costs, before and after MOO-250 and MOO-251**, per
    /// 128-frame block, in one run: the loop as it was against the device, at
    /// the factory settings above, round-robin, each block's fastest of
    /// `REPS` passes (default 7).
    ///
    /// ```sh
    /// cargo test -p mooloop-dsp --release --lib -- drive_cost --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "measures wall time; run deliberately in release"]
    fn drive_cost() {
        use std::hint::black_box;
        use std::time::Instant;

        const BLOCK: usize = 128;
        const BLOCKS: usize = 750;
        enum Device {
            Now(Box<DriveEffect>),
            Before(Box<DriveBefore>),
        }
        let reps = std::env::var("REPS")
            .ok()
            .and_then(|reps| reps.parse().ok())
            .unwrap_or(7usize)
            .max(1);
        let input = chord_input(BLOCK * BLOCKS);
        let mut rows: Vec<(String, Device)> = Vec::new();
        for (label, params) in drive_rows() {
            rows.push((format!("{label}, before"), Device::Before(Box::new(DriveBefore::new(params)))));
            rows.push((format!("{label}, after"), Device::Now(Box::new(DriveEffect::new(params, 48_000)))));
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
                        Device::Before(before) => before.process(&mut bus, BLOCK),
                    }
                    black_box(&bus);
                    *best = (*best).min(start.elapsed().as_nanos());
                }
            }
        }
        println!("{reps} passes, {BLOCKS} blocks of {BLOCK}, each block's fastest pass");
        for ((label, _), best) in rows.iter().zip(&best) {
            let us = best.iter().sum::<u128>() as f64 / BLOCKS as f64 / 1_000.0;
            println!("{label:<28} {us:7.2} us/block");
        }
    }
}
