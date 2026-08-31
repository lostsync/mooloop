//! Bitcrush: amplitude quantization plus sample-and-hold decimation.
//!
//! Deliberately **not** oversampled, unlike `drive`. The aliasing produced by
//! decimating without a band-limiting filter is the effect, not a defect —
//! see `docs/MODULATION_PLAN.md` ("Anti-aliasing policy").

use mooloop_core::{
    BitcrushParams, BitcrushStyle, BITCRUSH_PARAM_BITS, BITCRUSH_PARAM_DOWNSAMPLE,
    BITCRUSH_PARAM_MIX, BITCRUSH_PARAM_STYLE,
};

use crate::bus::StereoBus;
use crate::event::{Event, EventList};
use crate::node::{AudioNode, ProcessContext};
use crate::smooth::Smoothed;

/// Mix is the only continuous, audible parameter here: bit depth and
/// downsample rate are intentionally steppy, the effect *is* the aliasing.
const MIX_SMOOTH_S: f32 = 0.005;

/// Companding amount for the `Mu` style — the classic telephony curve. Near
/// silence the effective resolution is far finer than `bits`, at full scale
/// far coarser; quiet material survives, loud material crushes.
const MU: f32 = 255.0;

pub struct BitcrushEffect {
    params: BitcrushParams,
    /// Most recently latched sample, held until the next hold boundary.
    held_l: f32,
    held_r: f32,
    /// The sample before that, kept so `Glide` can interpolate between latch
    /// points instead of stepping.
    prev_held_l: f32,
    prev_held_r: f32,
    /// Fractional sample-and-hold phase, in input samples.
    phase: f32,
    /// Whether anything has been latched yet. Without this the effect would
    /// output its initial silence for the first hold span.
    primed: bool,
    /// TPDF dither generators, one per channel. Seeded apart so the channels
    /// decorrelate; cheap xorshift is plenty for audible noise.
    noise_l: u32,
    noise_r: u32,
    /// Nothing else in this effect is sample-rate dependent; kept only to
    /// re-time `mix`'s smoothing coefficient if the client's rate changes.
    sample_rate: u32,
    mix: Smoothed,
}

impl BitcrushEffect {
    pub fn new(params: BitcrushParams) -> Self {
        // No sample rate is known yet; `process` re-times `mix` against the
        // real rate on its first call, same guard the rate-aware effects use.
        let sample_rate = 48_000;
        Self {
            params,
            held_l: 0.0,
            held_r: 0.0,
            prev_held_l: 0.0,
            prev_held_r: 0.0,
            phase: 0.0,
            primed: false,
            noise_l: 0x9e37_79b9,
            noise_r: 0x85eb_ca6b,
            sample_rate,
            mix: Smoothed::new(params.mix.clamp(0.0, 1.0), MIX_SMOOTH_S, sample_rate),
        }
    }

    pub fn params(&self) -> BitcrushParams {
        self.params
    }

    /// Replace the parameter set wholesale (project load) — jump straight to
    /// the new value, there is nothing to click coming from a fresh load.
    pub fn set_params(&mut self, params: BitcrushParams) {
        self.params = params;
        self.mix.reset_to(params.mix.clamp(0.0, 1.0));
    }

    fn apply_param(&mut self, id: u32, value: f32) {
        match id {
            BITCRUSH_PARAM_BITS => self.params.bits = value.clamp(1.0, 16.0),
            BITCRUSH_PARAM_DOWNSAMPLE => self.params.downsample = value.clamp(1.0, 64.0),
            BITCRUSH_PARAM_MIX => {
                self.params.mix = value.clamp(0.0, 1.0);
                self.mix.set_target(self.params.mix);
            }
            BITCRUSH_PARAM_STYLE => {
                self.params.style = BitcrushStyle::from_index(value.round() as i32)
            }
            _ => {}
        }
    }

    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        let BitcrushParams {
            bits,
            downsample,
            style,
            ..
        } = self.params;

        // Continuous in `bits`, so the control can be swept without stepping
        // between integer depths.
        let levels = 2.0f32.powf(bits);
        let step = 2.0 / levels;
        let hold = downsample.max(1.0);

        for i in start..end {
            let mix = self.mix.advance();
            let (dry_l, dry_r) = (bus.l[i], bus.r[i]);

            self.phase += 1.0;
            if !self.primed || self.phase >= hold {
                // Carry the remainder so fractional hold lengths average out
                // to the requested rate instead of rounding down every time.
                self.phase = if self.primed { self.phase - hold } else { 0.0 };
                self.primed = true;
                self.prev_held_l = self.held_l;
                self.prev_held_r = self.held_r;
                self.held_l = dry_l;
                self.held_r = dry_r;
            }

            // `Glide` draws a line between latch points; every other style
            // steps hard at the boundary.
            let t = self.phase / hold;
            let source_l = match style {
                BitcrushStyle::Glide => self.prev_held_l + (self.held_l - self.prev_held_l) * t,
                _ => self.held_l,
            };
            let source_r = match style {
                BitcrushStyle::Glide => self.prev_held_r + (self.held_r - self.prev_held_r) * t,
                _ => self.held_r,
            };

            let wet_l = crush(style, source_l, step, &mut self.noise_l);
            let wet_r = crush(style, source_r, step, &mut self.noise_r);

            bus.l[i] = dry_l + (wet_l - dry_l) * mix;
            bus.r[i] = dry_r + (wet_r - dry_r) * mix;
        }
    }
}

/// Snap one held sample to the coarsened grid, in the manner `style` names.
/// `noise` is the per-channel dither state; it is only advanced by `Dither`.
fn crush(style: BitcrushStyle, sample: f32, step: f32, noise: &mut u32) -> f32 {
    match style {
        BitcrushStyle::Crush | BitcrushStyle::Glide => quantize(sample, step),
        BitcrushStyle::Dither => {
            // TPDF spanning one step: two uniforms summed. Wide enough to
            // fully decorrelate the error from the signal without growing
            // the noise floor past what the depth loss already costs.
            let dither = (next_uniform(noise) + next_uniform(noise) - 1.0) * step;
            quantize(sample + dither, step)
        }
        BitcrushStyle::Mu => {
            // Compress, quantize in the compressed domain, expand back. The
            // quantizer still sees `bits` of levels; where they land changes.
            let sign = sample.signum();
            let compressed = sign * (1.0 + MU * sample.abs()).ln() / (1.0 + MU).ln();
            let quantized = quantize(compressed, step);
            sign * ((1.0 + MU).powf(quantized.abs()) - 1.0) / MU
        }
    }
}

fn next_uniform(state: &mut u32) -> f32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    // High 24 bits, so the float is uniform over [0, 1).
    (x >> 8) as f32 / 16_777_216.0
}

/// Round to the nearest multiple of `step`. `step` is never zero: `bits` is
/// clamped to at most 16, so the smallest step is 2^-15.
fn quantize(sample: f32, step: f32) -> f32 {
    (sample / step).round() * step
}

impl AudioNode for BitcrushEffect {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        if ctx.sample_rate != self.sample_rate {
            self.sample_rate = ctx.sample_rate;
            self.mix.set_time(MIX_SMOOTH_S, ctx.sample_rate);
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

    fn ramp_bus(frames: usize) -> StereoBus {
        let mut bus = StereoBus::with_capacity(frames);
        for i in 0..frames {
            let v = i as f32 / frames as f32 * 2.0 - 1.0;
            bus.l[i] = v;
            bus.r[i] = v;
        }
        bus
    }

    #[test]
    fn defaults_are_effectively_transparent() {
        let frames = 1_024;
        let mut bus = ramp_bus(frames);
        let reference = bus.l[..frames].to_vec();
        let mut effect = BitcrushEffect::new(BitcrushParams::default());
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);
        for (i, expected) in reference.iter().enumerate() {
            // 16 bits of quantization, no decimation: within one LSB.
            assert!(
                (bus.l[i] - expected).abs() <= 2.0 / 65_536.0 + 1e-6,
                "at {i}: {} vs {expected}",
                bus.l[i],
            );
        }
    }

    #[test]
    fn low_bit_depth_collapses_onto_few_levels() {
        let frames = 4_096;
        let mut bus = ramp_bus(frames);
        let mut effect = BitcrushEffect::new(BitcrushParams {
            bits: 2.0,
            ..BitcrushParams::default()
        });
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);

        let mut levels: Vec<f32> = Vec::new();
        for i in 0..frames {
            if !levels.iter().any(|l| (l - bus.l[i]).abs() < 1e-6) {
                levels.push(bus.l[i]);
            }
        }
        // 2 bits over [-1, 1] with a step of 0.5 gives at most 5 reachable
        // levels: -1, -0.5, 0, 0.5, 1.
        assert!(
            levels.len() <= 5,
            "expected at most 5 quantization levels, found {}",
            levels.len()
        );
    }

    #[test]
    fn decimation_holds_samples_for_the_requested_span() {
        let frames = 64;
        let mut bus = ramp_bus(frames);
        let mut effect = BitcrushEffect::new(BitcrushParams {
            bits: 16.0,
            downsample: 4.0,
            ..BitcrushParams::default()
        });
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);
        // Every run of four consecutive outputs should be one held value.
        for start in (0..frames - 4).step_by(4) {
            let first = bus.l[start];
            for offset in 1..4 {
                assert!(
                    (bus.l[start + offset] - first).abs() < 1e-5,
                    "sample {} broke the hold starting at {start}",
                    start + offset
                );
            }
        }
    }

    #[test]
    fn fractional_decimation_averages_to_the_requested_rate() {
        let frames = 1_000;
        let mut bus = ramp_bus(frames);
        let mut effect = BitcrushEffect::new(BitcrushParams {
            bits: 16.0,
            downsample: 2.5,
            ..BitcrushParams::default()
        });
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);
        // Count value changes: a hold of 2.5 should latch ~frames/2.5 times.
        let mut latches = 0;
        for i in 1..frames {
            if (bus.l[i] - bus.l[i - 1]).abs() > 1e-9 {
                latches += 1;
            }
        }
        let expected = frames as f32 / 2.5;
        assert!(
            (latches as f32 - expected).abs() < expected * 0.05,
            "expected about {expected} latches, counted {latches}"
        );
    }

    #[test]
    fn zero_mix_leaves_the_signal_alone() {
        let frames = 512;
        let mut bus = ramp_bus(frames);
        let reference = bus.l[..frames].to_vec();
        let mut effect = BitcrushEffect::new(BitcrushParams {
            bits: 1.0,
            downsample: 32.0,
            mix: 0.0,
            ..BitcrushParams::default()
        });
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);
        for (i, expected) in reference.iter().enumerate() {
            assert!((bus.l[i] - expected).abs() < 1e-6, "dry path altered at {i}");
        }
    }

    #[test]
    fn param_events_take_effect_mid_block() {
        let frames = 2_048;
        let mut bus = ramp_bus(frames);
        let mut effect = BitcrushEffect::new(BitcrushParams::default());
        let mut events = EventList::empty();
        assert!(events.push(TimedEvent {
            offset: (frames / 2) as u32,
            event: Event::ParamValue {
                id: BITCRUSH_PARAM_BITS,
                value: 2.0,
            },
        }));
        effect.process(&context(frames), &mut bus, &events, None);

        let distinct = |range: &[f32]| {
            let mut seen: Vec<f32> = Vec::new();
            for v in range {
                if !seen.iter().any(|s| (s - v).abs() < 1e-6) {
                    seen.push(*v);
                }
            }
            seen.len()
        };
        let before = distinct(&bus.l[..frames / 2]);
        let after = distinct(&bus.l[frames / 2..]);
        assert!(
            after < 6 && before > 100,
            "bit depth drop did not take: {before} levels then {after}"
        );
    }

    #[test]
    fn mix_change_mid_block_does_not_click() {
        let frames = 4_096;
        let mut bus = ramp_bus(frames);
        let mut effect = BitcrushEffect::new(BitcrushParams {
            bits: 8.0,
            downsample: 1.0,
            mix: 0.0,
            ..BitcrushParams::default()
        });
        let mut events = EventList::empty();
        assert!(events.push(TimedEvent {
            offset: (frames / 2) as u32,
            event: Event::ParamValue {
                id: BITCRUSH_PARAM_MIX,
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

    fn dc_bus(frames: usize, value: f32) -> StereoBus {
        let mut bus = StereoBus::with_capacity(frames);
        for i in 0..frames {
            bus.l[i] = value;
            bus.r[i] = value;
        }
        bus
    }

    #[test]
    fn every_style_stays_finite_and_bounded_under_heavy_crush() {
        for style in [
            BitcrushStyle::Crush,
            BitcrushStyle::Dither,
            BitcrushStyle::Mu,
            BitcrushStyle::Glide,
        ] {
            let frames = 2_048;
            let mut bus = StereoBus::with_capacity(frames);
            for i in 0..frames {
                let t = i as f32 / 48_000.0;
                let s = (t * 110.0 * core::f32::consts::TAU).sin();
                bus.l[i] = s;
                bus.r[i] = s;
            }
            let mut effect = BitcrushEffect::new(BitcrushParams {
                bits: 1.0,
                downsample: 32.0,
                style,
                ..BitcrushParams::default()
            });
            effect.process(&context(frames), &mut bus, &EventList::empty(), None);
            for i in 0..frames {
                assert!(
                    bus.l[i].is_finite() && bus.l[i].abs() < 4.0,
                    "{style:?} produced {} at {i}",
                    bus.l[i]
                );
            }
        }
    }

    #[test]
    fn dither_breaks_up_a_signal_that_crush_freezes() {
        // DC at 0.3 with 2 bits: plain rounding lands on one grid point and
        // freezes; dithered rounding wanders across the grid instead.
        let frames = 512;
        let run = |style: BitcrushStyle| {
            let mut bus = dc_bus(frames, 0.3);
            let mut effect = BitcrushEffect::new(BitcrushParams {
                bits: 2.0,
                style,
                ..BitcrushParams::default()
            });
            effect.process(&context(frames), &mut bus, &EventList::empty(), None);
            bus.l[..frames].to_vec()
        };
        let crushed = run(BitcrushStyle::Crush);
        let dithered = run(BitcrushStyle::Dither);

        assert!(
            crushed.windows(2).all(|w| (w[0] - w[1]).abs() < 1e-6),
            "crush should freeze DC, output varied"
        );
        let distinct: Vec<f32> = {
            let mut seen: Vec<f32> = Vec::new();
            for v in &dithered {
                if !seen.iter().any(|s| (s - v).abs() < 1e-6) {
                    seen.push(*v);
                }
            }
            seen
        };
        assert!(
            distinct.len() > 1,
            "dither should modulate DC, output froze on {distinct:?}"
        );
    }

    #[test]
    fn style_param_event_switches_the_math_mid_block() {
        let frames = 1_024;
        let mut bus = dc_bus(frames, 0.3);
        let mut effect = BitcrushEffect::new(BitcrushParams {
            bits: 2.0,
            ..BitcrushParams::default()
        });
        let mut events = EventList::empty();
        assert!(events.push(TimedEvent {
            offset: (frames / 2) as u32,
            event: Event::ParamValue {
                id: BITCRUSH_PARAM_STYLE,
                value: BitcrushStyle::Dither.to_index() as f32,
            },
        }));
        effect.process(&context(frames), &mut bus, &events, None);

        let before_froze = bus.l[1..frames / 2]
            .windows(2)
            .all(|w| (w[0] - w[1]).abs() < 1e-6);
        let after_varies = {
            let mut distinct: Vec<f32> = Vec::new();
            for v in &bus.l[frames / 2..frames] {
                if !distinct.iter().any(|s| (s - v).abs() < 1e-6) {
                    distinct.push(*v);
                }
            }
            distinct.len() > 1
        };
        assert!(before_froze, "pre-event output should be the frozen crush");
        assert!(after_varies, "post-event output should be dithered noise");
    }

    #[test]
    fn mu_keeps_quiet_material_that_crush_mutes() {
        // A sine at 5% full scale through 2 bits: nearest-rounding snaps every
        // sample to zero — silence. The companded quantizer spends its levels
        // around silence instead, so a low-level replica survives.
        let frames = 4_096;
        let quiet_sine = |frames: usize| {
            let mut bus = StereoBus::with_capacity(frames);
            for i in 0..frames {
                let t = i as f32 / 48_000.0;
                let s = (t * 220.0 * core::f32::consts::TAU).sin() * 0.05;
                bus.l[i] = s;
                bus.r[i] = s;
            }
            bus
        };
        let run = |style: BitcrushStyle| {
            let mut bus = quiet_sine(frames);
            let mut effect = BitcrushEffect::new(BitcrushParams {
                bits: 2.0,
                style,
                ..BitcrushParams::default()
            });
            effect.process(&context(frames), &mut bus, &EventList::empty(), None);
            let peak_out = bus.l[..frames].iter().fold(0.0f32, |a, s| a.max(s.abs()));
            let max_error = bus.l[..frames]
                .iter()
                .copied()
                .zip(quiet_sine(frames).l[..frames].iter())
                .map(|(out, dry)| (out - dry).abs())
                .fold(0.0f32, f32::max);
            (peak_out, max_error)
        };
        let (crush_peak, _crush_error) = run(BitcrushStyle::Crush);
        let (mu_peak, mu_error) = run(BitcrushStyle::Mu);
        assert!(
            crush_peak < 1e-6,
            "2-bit crush should mute a 0.05 sine entirely, peak {crush_peak}"
        );
        assert!(
            mu_peak > 0.03,
            "companding should keep a quiet replica alive, peak {mu_peak}"
        );
        assert!(
            mu_error <= 0.05,
            "companded replica should stay within the crush step, error {mu_error}"
        );
    }

    #[test]
    fn glide_interpolates_instead_of_holding_flat() {
        // 3_100 Hz, not a frequency that divides neatly into the hold: a
        // 3 kHz sine is exactly 16 samples per period, so an 8-sample hold
        // latches on every zero crossing and both styles go silent.
        let frames = 512;
        let sine = |frames: usize| {
            let mut bus = StereoBus::with_capacity(frames);
            for i in 0..frames {
                let t = i as f32 / 48_000.0;
                let s = (t * 3_100.0 * core::f32::consts::TAU).sin();
                bus.l[i] = s;
                bus.r[i] = s;
            }
            bus
        };
        let run = |style: BitcrushStyle| {
            let mut bus = sine(frames);
            let mut effect = BitcrushEffect::new(BitcrushParams {
                bits: 16.0,
                downsample: 8.0,
                style,
                ..BitcrushParams::default()
            });
            effect.process(&context(frames), &mut bus, &EventList::empty(), None);
            bus.l[..frames].to_vec()
        };
        let crushed = run(BitcrushStyle::Crush);
        let glided = run(BitcrushStyle::Glide);

        // Crush repeats one value per 8-sample span; glide draws a slope
        // across the span, so the two must part ways somewhere.
        assert!(
            glided
                .iter()
                .zip(crushed.iter())
                .any(|(g, c)| (g - c).abs() > 1e-4),
            "glide never diverged from the hard hold"
        );
        // And it must not be just another staircase: some span should move
        // smoothly rather than repeat a single sample eight times.
        let moves_within_span = (0..frames - 8)
            .step_by(8)
            .any(|start| {
                (0..7).any(|o| (glided[start + o + 1] - glided[start + o]).abs() > 1e-5)
            });
        assert!(moves_within_span, "glide output was still a staircase");
    }
}
