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
    LIMITER_PARAM_RELEASE_MS,
};

use crate::bus::StereoBus;
use crate::dynamics::{
    compressor_gain_db, db_to_lin, gate_gain_db, limiter_gain_db, lin_to_db, time_coeff,
    EnvelopeFollower,
};
use crate::event::{Event, EventList};
use crate::node::{AudioNode, ProcessContext};

/// Peak detection for the limiter is effectively instantaneous: its whole job
/// is to not let anything through, so its attack is not a user control.
const LIMITER_ATTACK_MS: f32 = 0.05;

/// Level of the louder channel, which is what every effect here detects on.
fn linked_peak(l: f32, r: f32) -> f32 {
    l.abs().max(r.abs())
}

// --- Gate ------------------------------------------------------------------

pub struct GateEffect {
    params: GateParams,
    sample_rate: u32,
    /// Current attenuation in dB, ramped toward the target.
    gain_db: f32,
    /// Samples of hold remaining since the level last cleared the threshold.
    hold_remaining: u32,
}

impl GateEffect {
    pub fn new(params: GateParams, sample_rate: u32) -> Self {
        Self {
            params,
            sample_rate,
            // Start shut, so a gate on a silent channel does not pass a burst
            // before its first ramp.
            gain_db: params.range_db,
            hold_remaining: 0,
        }
    }

    pub fn params(&self) -> GateParams {
        self.params
    }

    pub fn set_params(&mut self, params: GateParams) {
        self.params = params;
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

        for i in start..end {
            let level_db = lin_to_db(linked_peak(bus.l[i], bus.r[i]));

            if level_db >= threshold_db {
                self.hold_remaining = hold_samples;
            } else if self.hold_remaining > 0 {
                self.hold_remaining -= 1;
            }

            // Held open counts as open: the hold exists to stop the gate
            // chattering on material sitting right at the threshold.
            let open = level_db >= threshold_db || self.hold_remaining > 0;
            let target_db = if open {
                0.0
            } else {
                gate_gain_db(level_db, threshold_db, range_db)
            };
            let coeff = if target_db > self.gain_db {
                open_coeff
            } else {
                shut_coeff
            };
            self.gain_db = target_db + coeff * (self.gain_db - target_db);

            let gain = db_to_lin(self.gain_db);
            bus.l[i] *= gain;
            bus.r[i] *= gain;
        }
    }
}

impl AudioNode for GateEffect {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        self.sample_rate = ctx.sample_rate;
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

// --- Compressor ------------------------------------------------------------

pub struct CompressorEffect {
    params: CompressorParams,
    sample_rate: u32,
    detector: EnvelopeFollower,
}

impl CompressorEffect {
    pub fn new(params: CompressorParams, sample_rate: u32) -> Self {
        let mut detector = EnvelopeFollower::new();
        detector.set_times(params.attack_ms, params.release_ms, sample_rate);
        Self {
            params,
            sample_rate,
            detector,
        }
    }

    pub fn params(&self) -> CompressorParams {
        self.params
    }

    pub fn set_params(&mut self, params: CompressorParams) {
        self.params = params;
        self.detector
            .set_times(params.attack_ms, params.release_ms, self.sample_rate);
    }

    fn apply_param(&mut self, id: u32, value: f32) {
        match id {
            COMP_PARAM_THRESHOLD_DB => self.params.threshold_db = value.clamp(-60.0, 0.0),
            COMP_PARAM_RATIO => self.params.ratio = value.clamp(1.0, 20.0),
            COMP_PARAM_ATTACK_MS => self.params.attack_ms = value.clamp(0.05, 200.0),
            COMP_PARAM_RELEASE_MS => self.params.release_ms = value.clamp(5.0, 2_000.0),
            COMP_PARAM_KNEE_DB => self.params.knee_db = value.clamp(0.0, 24.0),
            COMP_PARAM_MAKEUP_DB => self.params.makeup_db = value.clamp(0.0, 24.0),
            _ => {}
        }
        self.detector
            .set_times(self.params.attack_ms, self.params.release_ms, self.sample_rate);
    }

    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        let CompressorParams {
            threshold_db,
            ratio,
            knee_db,
            makeup_db,
            ..
        } = self.params;
        let makeup = db_to_lin(makeup_db);

        for i in start..end {
            let envelope = self.detector.process(linked_peak(bus.l[i], bus.r[i]));
            let reduction_db =
                compressor_gain_db(lin_to_db(envelope), threshold_db, ratio, knee_db);
            let gain = db_to_lin(reduction_db) * makeup;
            bus.l[i] *= gain;
            bus.r[i] *= gain;
        }
    }
}

impl AudioNode for CompressorEffect {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        if ctx.sample_rate != self.sample_rate {
            self.sample_rate = ctx.sample_rate;
            self.detector.set_times(
                self.params.attack_ms,
                self.params.release_ms,
                self.sample_rate,
            );
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

// --- Limiter ---------------------------------------------------------------

/// A fast peak limiter.
///
/// There is no lookahead, deliberately: lookahead means latency, and with no
/// plugin-delay compensation in the engine a limiter on one channel would
/// shift it against every other channel. The cost is that a sample-fast
/// transient can overshoot the ceiling slightly before the detector catches
/// it. Add lookahead when the engine can compensate for it, not before.
pub struct LimiterEffect {
    params: LimiterParams,
    sample_rate: u32,
    detector: EnvelopeFollower,
}

impl LimiterEffect {
    pub fn new(params: LimiterParams, sample_rate: u32) -> Self {
        let mut detector = EnvelopeFollower::new();
        detector.set_times(LIMITER_ATTACK_MS, params.release_ms, sample_rate);
        Self {
            params,
            sample_rate,
            detector,
        }
    }

    pub fn params(&self) -> LimiterParams {
        self.params
    }

    pub fn set_params(&mut self, params: LimiterParams) {
        self.params = params;
        self.detector
            .set_times(LIMITER_ATTACK_MS, params.release_ms, self.sample_rate);
    }

    fn apply_param(&mut self, id: u32, value: f32) {
        match id {
            LIMITER_PARAM_CEILING_DB => self.params.ceiling_db = value.clamp(-24.0, 0.0),
            LIMITER_PARAM_RELEASE_MS => self.params.release_ms = value.clamp(1.0, 500.0),
            LIMITER_PARAM_GAIN_DB => self.params.gain_db = value.clamp(0.0, 24.0),
            _ => {}
        }
        self.detector
            .set_times(LIMITER_ATTACK_MS, self.params.release_ms, self.sample_rate);
    }

    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        let LimiterParams {
            ceiling_db,
            gain_db,
            ..
        } = self.params;
        let drive = db_to_lin(gain_db);
        let ceiling = db_to_lin(ceiling_db);

        for i in start..end {
            let l = bus.l[i] * drive;
            let r = bus.r[i] * drive;

            let envelope = self.detector.process(linked_peak(l, r));
            let reduction = db_to_lin(limiter_gain_db(lin_to_db(envelope), ceiling_db));

            // The detector's release leaves the gain high for a moment after a
            // peak passes, so clamp as a backstop. Without it the released
            // gain can let a following sample through above the ceiling.
            bus.l[i] = (l * reduction).clamp(-ceiling, ceiling);
            bus.r[i] = (r * reduction).clamp(-ceiling, ceiling);
        }
    }
}

impl AudioNode for LimiterEffect {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        if ctx.sample_rate != self.sample_rate {
            self.sample_rate = ctx.sample_rate;
            self.detector
                .set_times(LIMITER_ATTACK_MS, self.params.release_ms, self.sample_rate);
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

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0f32, |a, s| a.max(s.abs()))
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
        let dip_len = 480; // 10 ms
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
        let reduction_db = lin_to_db(loud_out) - lin_to_db(loud_in);
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
                },
                SR,
            );
            effect.process(&context(frames), &mut bus, &EventList::empty(), None);
            peak(&bus.l[frames / 2..])
        };
        let plain = run(0.0);
        let made_up = run(9.0);
        let lift_db = lin_to_db(made_up) - lin_to_db(plain);
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
            let ceiling = db_to_lin(ceiling_db);
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
        for (i, expected) in reference.iter().enumerate() {
            assert!(
                (bus.l[i] - expected).abs() < 1e-5,
                "quiet signal altered at {i}: {} vs {expected}",
                bus.l[i]
            );
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
}
