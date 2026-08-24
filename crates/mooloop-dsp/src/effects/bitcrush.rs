//! Bitcrush: amplitude quantization plus sample-and-hold decimation.
//!
//! Deliberately **not** oversampled, unlike `drive`. The aliasing produced by
//! decimating without a band-limiting filter is the effect, not a defect —
//! see `docs/MODULATION_PLAN.md` ("Anti-aliasing policy").

use mooloop_core::{
    BitcrushParams, BITCRUSH_PARAM_BITS, BITCRUSH_PARAM_DOWNSAMPLE, BITCRUSH_PARAM_MIX,
};

use crate::bus::StereoBus;
use crate::event::{Event, EventList};
use crate::node::{AudioNode, ProcessContext};
use crate::smooth::Smoothed;

/// Mix is the only continuous, audible parameter here: bit depth and
/// downsample rate are intentionally steppy, the effect *is* the aliasing.
const MIX_SMOOTH_S: f32 = 0.005;

pub struct BitcrushEffect {
    params: BitcrushParams,
    /// Most recently latched sample, held until the next hold boundary.
    held_l: f32,
    held_r: f32,
    /// Fractional sample-and-hold phase, in input samples.
    phase: f32,
    /// Whether anything has been latched yet. Without this the effect would
    /// output its initial silence for the first hold span.
    primed: bool,
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
            phase: 0.0,
            primed: false,
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
            _ => {}
        }
    }

    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        let BitcrushParams {
            bits, downsample, ..
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
                self.held_l = dry_l;
                self.held_r = dry_r;
            }

            let wet_l = quantize(self.held_l, step);
            let wet_r = quantize(self.held_r, step);

            bus.l[i] = dry_l + (wet_l - dry_l) * mix;
            bus.r[i] = dry_r + (wet_r - dry_r) * mix;
        }
    }
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
}
