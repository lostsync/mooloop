//! Stereo delay with a damped, cross-feedable feedback path and three ways of
//! responding to a moving delay time.
//!
//! Built on [`crate::delayline::DelayLine`], the ring primitive shared with
//! the retained-audio buffer device. Nothing here is delay-specific about the
//! buffer itself — this effect is one set of read-head policies over it, and
//! the buffer device will be another.

use mooloop_core::{
    DelayMode, DelayParams, DELAY_MAX_TIME_MS, DELAY_PARAM_CROSS, DELAY_PARAM_FEEDBACK,
    DELAY_PARAM_MIX, DELAY_PARAM_MODE, DELAY_PARAM_TIME_MS, DELAY_PARAM_TONE,
};

use crate::bus::StereoBus;
use crate::delayline::{DelayLine, ReadHead, MIN_READ_OFFSET};
use crate::event::{Event, EventList};
use crate::filter::OnePoleLp;
use crate::node::{AudioNode, ProcessContext};
use crate::smooth::Smoothed;

/// Crossfade applied when the head jumps: a digital time change, or a reverse
/// window wrapping. About 5 ms at 48 kHz — long enough to hide a jump between
/// uncorrelated audio, short enough not to smear a rhythmic one.
const FADE_FRAMES: u32 = 256;

/// Most the offset may glide per frame in tape mode. This is what sets how
/// far the repeats detune while the time is moving: at 0.05 the read rate
/// deviates by 5%, a bit under a semitone.
const TAPE_GLIDE_PER_FRAME: f32 = 0.05;

/// Feedback damping range, swept by `tone`.
const TONE_MIN_HZ: f32 = 200.0;
const TONE_MAX_HZ: f32 = 20_000.0;

/// Time constant for feedback and wet level: both scale amplitude directly,
/// so a block-boundary step there is an audible click, not just zipper.
const GAIN_SMOOTH_S: f32 = 0.005;
/// Time constant for the damping coefficient. Smoothing the coefficient
/// itself, not the `tone` control that derives it, skips a `powf` per
/// sample — the coefficient is already bounded in a stable range at both
/// ends, so interpolating it directly cannot destabilize the one-pole.
const DAMP_SMOOTH_S: f32 = 0.01;

pub struct DelayEffect {
    params: DelayParams,
    sample_rate: u32,
    line: DelayLine,
    head: ReadHead,
    /// Delay time in frames, as last resolved from `params.time_ms`.
    target_offset: f32,
    /// One-pole low-pass on the feedback path, per channel.
    damp_l: OnePoleLp,
    damp_r: OnePoleLp,
    feedback: Smoothed,
    damp_coeff: Smoothed,
    mix: Smoothed,
}

impl DelayEffect {
    /// Allocates a ring sized for [`DELAY_MAX_TIME_MS`]. Non-realtime only.
    pub fn new(params: DelayParams, sample_rate: u32) -> Self {
        let frames = ring_frames(sample_rate);
        let mut effect = Self {
            params,
            sample_rate,
            line: DelayLine::with_capacity_frames(frames),
            head: ReadHead::new(MIN_READ_OFFSET),
            target_offset: MIN_READ_OFFSET,
            damp_l: OnePoleLp::new(),
            damp_r: OnePoleLp::new(),
            feedback: Smoothed::new(params.feedback.clamp(0.0, 0.98), GAIN_SMOOTH_S, sample_rate),
            damp_coeff: Smoothed::new(
                tone_coeff(params.tone, sample_rate),
                DAMP_SMOOTH_S,
                sample_rate,
            ),
            mix: Smoothed::new(params.mix.clamp(0.0, 1.0), GAIN_SMOOTH_S, sample_rate),
        };
        effect.target_offset = effect.resolve_offset();
        effect.head = ReadHead::new(effect.target_offset);
        effect
    }

    pub fn params(&self) -> DelayParams {
        self.params
    }

    /// Replace the parameter set wholesale (project load) — jump straight to
    /// the new values, there is nothing to click coming from a fresh load.
    pub fn set_params(&mut self, params: DelayParams) {
        self.params = params;
        self.target_offset = self.resolve_offset();
        self.head = ReadHead::new(self.target_offset);
        self.feedback.reset_to(params.feedback.clamp(0.0, 0.98));
        self.damp_coeff
            .reset_to(tone_coeff(params.tone, self.sample_rate));
        self.mix.reset_to(params.mix.clamp(0.0, 1.0));
    }

    /// Delay time in frames, clamped to what the ring can actually serve.
    fn resolve_offset(&self) -> f32 {
        let frames = self.params.time_ms.max(0.0) / 1_000.0 * self.sample_rate as f32;
        frames.clamp(MIN_READ_OFFSET, self.line.max_read_offset())
    }

    fn apply_param(&mut self, id: u32, value: f32) {
        match id {
            DELAY_PARAM_TIME_MS => {
                self.params.time_ms = value.clamp(0.0, DELAY_MAX_TIME_MS);
                self.target_offset = self.resolve_offset();
            }
            DELAY_PARAM_FEEDBACK => {
                self.params.feedback = value.clamp(0.0, 0.98);
                self.feedback.set_target(self.params.feedback);
            }
            DELAY_PARAM_MODE => self.params.mode = DelayMode::from_index(value.round() as i32),
            DELAY_PARAM_CROSS => self.params.cross = value.clamp(0.0, 1.0),
            DELAY_PARAM_TONE => {
                self.params.tone = value.clamp(0.0, 1.0);
                self.damp_coeff
                    .set_target(tone_coeff(self.params.tone, self.sample_rate));
            }
            DELAY_PARAM_MIX => {
                self.params.mix = value.clamp(0.0, 1.0);
                self.mix.set_target(self.params.mix);
            }
            _ => {}
        }
    }

    /// How far the head's offset should drift this frame, and any jump the
    /// mode wants. Drift is expressed relative to the advancing write head:
    /// 0 holds the delay time, 2 reads backwards at unity rate.
    fn step_head(&mut self) -> f32 {
        match self.params.mode {
            DelayMode::Digital => {
                // Sub-frame differences are not worth a crossfade; anything
                // larger is a real time change and would click.
                if (self.head.offset() - self.target_offset).abs() > 1.0 {
                    self.head.jump_to(self.target_offset, FADE_FRAMES);
                }
                0.0
            }
            DelayMode::Tape => {
                // Gliding the offset *is* the repitch: the effective read rate
                // is `1 - drift`, so the buffered audio slows or speeds up on
                // the way to the new time, exactly like tape.
                (self.target_offset - self.head.offset())
                    .clamp(-TAPE_GLIDE_PER_FRAME, TAPE_GLIDE_PER_FRAME)
            }
            DelayMode::Reverse => {
                // Reading backwards at unity rate while the write head runs
                // forwards costs two frames of offset per frame. The window is
                // one delay time long, and has to outlast the crossfade.
                let window = self.target_offset.max(FADE_FRAMES as f32 * 2.0);
                if self.head.offset() >= MIN_READ_OFFSET + window {
                    self.head.jump_to(MIN_READ_OFFSET, FADE_FRAMES);
                }
                2.0
            }
        }
    }

    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        let cross = self.params.cross;

        for i in start..end {
            let feedback = self.feedback.advance();
            let damp_coeff = self.damp_coeff.advance();
            let mix = self.mix.advance();
            let (dry_l, dry_r) = (bus.l[i], bus.r[i]);

            let drift = self.step_head();
            let (wet_l, wet_r) = self.head.read(&self.line);

            // Damp inside the feedback loop so each repeat is darker than the
            // last, rather than filtering the output once.
            self.damp_l.set_coeff(damp_coeff);
            self.damp_r.set_coeff(damp_coeff);
            let damped_l = self.damp_l.next_sample(wet_l);
            let damped_r = self.damp_r.next_sample(wet_r);

            let fed_l = damped_l * (1.0 - cross) + damped_r * cross;
            let fed_r = damped_r * (1.0 - cross) + damped_l * cross;

            self.line
                .write(dry_l + fed_l * feedback, dry_r + fed_r * feedback);
            self.head.advance(drift);

            bus.l[i] = dry_l + (wet_l - dry_l) * mix;
            bus.r[i] = dry_r + (wet_r - dry_r) * mix;
        }
    }
}

/// Ring size for the longest supported delay, plus the interpolator's margin.
fn ring_frames(sample_rate: u32) -> usize {
    (DELAY_MAX_TIME_MS / 1_000.0 * sample_rate as f32) as usize + 8
}

/// One-pole coefficient for the feedback damping, swept exponentially so the
/// knob's lower half does something useful.
fn tone_coeff(tone: f32, sample_rate: u32) -> f32 {
    let sr = sample_rate.max(1) as f32;
    let hz = TONE_MIN_HZ * (TONE_MAX_HZ / TONE_MIN_HZ).powf(tone.clamp(0.0, 1.0));
    (1.0 - (-core::f32::consts::TAU * hz / sr).exp()).clamp(0.0, 1.0)
}

impl AudioNode for DelayEffect {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        // A sample-rate change invalidates the ring's sizing and the resolved
        // delay time; rebuilding the ring here would allocate on the audio
        // thread, so only the time is re-resolved. The engine constructs
        // nodes at the client's rate, so this is a guard, not a path.
        if ctx.sample_rate != self.sample_rate {
            self.sample_rate = ctx.sample_rate;
            self.target_offset = self.resolve_offset();
            self.feedback.set_time(GAIN_SMOOTH_S, ctx.sample_rate);
            self.damp_coeff.set_time(DAMP_SMOOTH_S, ctx.sample_rate);
            self.mix.set_time(GAIN_SMOOTH_S, ctx.sample_rate);
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
    use mooloop_core::DelayTimeDivision;

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

    /// An impulse at frame 0, silence after.
    fn impulse_bus(frames: usize) -> StereoBus {
        let mut bus = StereoBus::with_capacity(frames);
        bus.l[0] = 1.0;
        bus.r[0] = 1.0;
        bus
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn an_impulse_reappears_after_the_delay_time() {
        let frames = 8_192;
        let time_ms = 100.0;
        let mut bus = impulse_bus(frames);
        let mut effect = DelayEffect::new(
            DelayParams {
                time_ms,
                feedback: 0.0,
                mix: 1.0,
                tone: 1.0,
                ..DelayParams::default()
            },
            SR,
        );
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);

        let expected = (time_ms / 1_000.0 * SR as f32) as usize;
        let peak_at = (0..frames)
            .max_by(|a, b| bus.l[*a].abs().partial_cmp(&bus.l[*b].abs()).unwrap())
            .unwrap();
        assert!(
            (peak_at as i64 - expected as i64).abs() <= 4,
            "echo landed at {peak_at}, expected about {expected}"
        );
    }

    #[test]
    fn beat_divisions_resolve_to_transport_time() {
        assert!((DelayTimeDivision::Quarter.time_ms(120.0) - 500.0).abs() < f32::EPSILON);
        assert!((DelayTimeDivision::DottedEighth.time_ms(120.0) - 375.0).abs() < f32::EPSILON);
        assert!((DelayTimeDivision::EighthTriplet.time_ms(120.0) - (1_000.0 / 6.0)).abs() < 0.001);
        assert!((DelayTimeDivision::Quarter.time_ms(90.0) - (2_000.0 / 3.0)).abs() < 0.001);
    }

    #[test]
    fn feedback_produces_successive_decaying_repeats() {
        let frames = 16_384;
        let time_ms = 50.0;
        let mut bus = impulse_bus(frames);
        let mut effect = DelayEffect::new(
            DelayParams {
                time_ms,
                feedback: 0.7,
                mix: 1.0,
                tone: 1.0,
                cross: 0.0,
                ..DelayParams::default()
            },
            SR,
        );
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);

        let spacing = (time_ms / 1_000.0 * SR as f32) as usize;
        let peak_near = |center: usize| {
            let lo = center.saturating_sub(8);
            let hi = (center + 8).min(frames);
            bus.l[lo..hi].iter().fold(0.0f32, |a, s| a.max(s.abs()))
        };
        let first = peak_near(spacing);
        let second = peak_near(spacing * 2);
        let third = peak_near(spacing * 3);

        assert!(first > 0.5, "first repeat too quiet: {first}");
        assert!(
            second < first && second > first * 0.4,
            "second repeat {second} should decay from {first}, not vanish"
        );
        assert!(
            third < second && third > 0.05,
            "third repeat {third} should follow {second}"
        );
    }

    #[test]
    fn zero_mix_leaves_the_signal_alone() {
        let frames = 4_096;
        let mut bus = StereoBus::with_capacity(frames);
        for i in 0..frames {
            let s = (i as f32 / SR as f32 * 220.0 * core::f32::consts::TAU).sin() * 0.5;
            bus.l[i] = s;
            bus.r[i] = s;
        }
        let reference = bus.l[..frames].to_vec();
        let mut effect = DelayEffect::new(
            DelayParams {
                time_ms: 30.0,
                feedback: 0.8,
                mix: 0.0,
                ..DelayParams::default()
            },
            SR,
        );
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);
        for (i, expected) in reference.iter().enumerate() {
            assert!((bus.l[i] - expected).abs() < 1e-6, "dry path altered at {i}");
        }
    }

    #[test]
    fn cross_feedback_moves_repeats_to_the_other_channel() {
        let frames = 16_384;
        let time_ms = 40.0;
        // Impulse on the left only.
        let mut bus = StereoBus::with_capacity(frames);
        bus.l[0] = 1.0;
        let mut effect = DelayEffect::new(
            DelayParams {
                time_ms,
                feedback: 0.7,
                cross: 1.0,
                mix: 1.0,
                tone: 1.0,
                ..DelayParams::default()
            },
            SR,
        );
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);

        let spacing = (time_ms / 1_000.0 * SR as f32) as usize;
        let peak = |buf: &[f32], center: usize| {
            let lo = center.saturating_sub(8);
            let hi = (center + 8).min(frames);
            buf[lo..hi].iter().fold(0.0f32, |a, s| a.max(s.abs()))
        };
        // First repeat stays left (it is the direct tap); the second, having
        // gone through the crossed feedback path, arrives on the right.
        let second_left = peak(&bus.l, spacing * 2);
        let second_right = peak(&bus.r, spacing * 2);
        assert!(
            second_right > second_left * 4.0,
            "ping-pong did not cross: left {second_left}, right {second_right}"
        );
    }

    #[test]
    fn tone_darkens_the_repeats() {
        let frames = 16_384;
        let run = |tone: f32| {
            let mut bus = impulse_bus(frames);
            let mut effect = DelayEffect::new(
                DelayParams {
                    time_ms: 40.0,
                    feedback: 0.8,
                    tone,
                    mix: 1.0,
                    ..DelayParams::default()
                },
                SR,
            );
            effect.process(&context(frames), &mut bus, &EventList::empty(), None);
            // Late repeats have been through the damping several times.
            rms(&bus.l[frames / 2..])
        };
        let dark = run(0.0);
        let bright = run(1.0);
        assert!(
            bright > dark * 2.0,
            "damping had little effect: dark {dark}, bright {bright}"
        );
    }

    #[test]
    fn changing_time_mid_block_does_not_click() {
        let frames = 32_768;
        let mut bus = StereoBus::with_capacity(frames);
        for i in 0..frames {
            let s = (i as f32 / SR as f32 * 330.0 * core::f32::consts::TAU).sin() * 0.4;
            bus.l[i] = s;
            bus.r[i] = s;
        }
        let mut effect = DelayEffect::new(
            DelayParams {
                time_ms: 200.0,
                feedback: 0.4,
                mix: 1.0,
                tone: 1.0,
                ..DelayParams::default()
            },
            SR,
        );
        let mut events = EventList::empty();
        assert!(events.push(TimedEvent {
            offset: (frames / 2) as u32,
            event: Event::ParamValue {
                id: DELAY_PARAM_TIME_MS,
                value: 60.0,
            },
        }));
        effect.process(&context(frames), &mut bus, &events, None);

        // A hard jump between two uncorrelated points in a 330 Hz sine would
        // leave a step far larger than the waveform's own frame-to-frame slope.
        let max_step = (1..frames)
            .map(|i| (bus.l[i] - bus.l[i - 1]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_step < 0.2,
            "time change left a discontinuity of {max_step}"
        );
    }

    #[test]
    fn feedback_change_mid_block_does_not_click() {
        let frames = 32_768;
        let mut bus = StereoBus::with_capacity(frames);
        for i in 0..frames {
            let s = (i as f32 / SR as f32 * 330.0 * core::f32::consts::TAU).sin() * 0.4;
            bus.l[i] = s;
            bus.r[i] = s;
        }
        let mut effect = DelayEffect::new(
            DelayParams {
                time_ms: 20.0,
                feedback: 0.0,
                mix: 1.0,
                tone: 1.0,
                ..DelayParams::default()
            },
            SR,
        );
        let mut events = EventList::empty();
        assert!(events.push(TimedEvent {
            offset: (frames / 2) as u32,
            event: Event::ParamValue {
                id: DELAY_PARAM_FEEDBACK,
                value: 0.9,
            },
        }));
        effect.process(&context(frames), &mut bus, &events, None);
        let max_step = (1..frames)
            .map(|i| (bus.l[i] - bus.l[i - 1]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_step < 0.2,
            "feedback change left a discontinuity of {max_step}"
        );
    }

    /// Stability of the feedback loop itself: excite it once, then let it run
    /// on silence. A sustained input is not the right probe here — a 0.98
    /// feedback loop driven by a full-scale sine correctly settles around
    /// 1/(1-0.98) times its input, which says nothing about stability.
    #[test]
    fn every_mode_decays_on_silence_at_maximum_feedback() {
        for mode in [DelayMode::Digital, DelayMode::Tape, DelayMode::Reverse] {
            let frames = 8_192;
            let mut effect = DelayEffect::new(
                DelayParams {
                    time_ms: 120.0,
                    tempo_sync: false,
                    time_division: DelayTimeDivision::default(),
                    feedback: 0.98,
                    mode,
                    cross: 0.5,
                    tone: 1.0,
                    mix: 1.0,
                },
                SR,
            );

            let mut bus = impulse_bus(frames);
            effect.process(&context(frames), &mut bus, &EventList::empty(), None);
            let early = bus.l[..frames].iter().fold(0.0f32, |a, s| a.max(s.abs()));

            let mut late = 0.0f32;
            for _ in 0..24 {
                let mut silence = StereoBus::with_capacity(frames);
                effect.process(&context(frames), &mut silence, &EventList::empty(), None);
                late = silence.l[..frames].iter().fold(0.0f32, |a, s| a.max(s.abs()));
                for i in 0..frames {
                    assert!(
                        silence.l[i].is_finite() && silence.l[i].abs() < 10.0,
                        "{mode:?} produced {} at {i}",
                        silence.l[i]
                    );
                }
            }
            assert!(
                late < early,
                "{mode:?} did not decay: started {early}, ended {late}"
            );
        }
    }

    #[test]
    fn reverse_mode_reads_backwards() {
        // A rising ramp read backwards must fall within each window.
        //
        // The head retreats two frames per frame (reading backwards while the
        // write head advances), so it traverses a `window`-long span in
        // `window / 2` frames -- that, not `window`, is the cycle length.
        let frames = 32_768;
        let mut bus = StereoBus::with_capacity(frames);
        for i in 0..frames {
            let v = i as f32 / frames as f32;
            bus.l[i] = v;
            bus.r[i] = v;
        }
        let time_ms = 100.0;
        let mut effect = DelayEffect::new(
            DelayParams {
                time_ms,
                tempo_sync: false,
                time_division: DelayTimeDivision::default(),
                feedback: 0.0,
                mode: DelayMode::Reverse,
                mix: 1.0,
                tone: 1.0,
                cross: 0.0,
            },
            SR,
        );
        effect.process(&context(frames), &mut bus, &EventList::empty(), None);

        let window = (time_ms / 1_000.0 * SR as f32) as usize;
        let cycle = window / 2;
        // Start a few cycles in, so the ring holds real history rather than
        // the silence a reverse head reads before any exists.
        let start = cycle * 5;
        let local_mean = |center: usize| {
            let lo = center - 64;
            let hi = center + 64;
            bus.l[lo..hi].iter().sum::<f32>() / (hi - lo) as f32
        };
        // Both points sit inside one cycle and clear of the wrap crossfade.
        let early = local_mean(start + cycle / 4);
        let late = local_mean(start + 3 * cycle / 4);
        assert!(
            late < early,
            "reverse output should fall as the input rises: early {early}, late {late}"
        );
    }

    #[test]
    fn param_events_take_effect_mid_block() {
        let frames = 8_192;
        let mut bus = impulse_bus(frames);
        let mut effect = DelayEffect::new(
            DelayParams {
                time_ms: 40.0,
                feedback: 0.0,
                mix: 0.0,
                tone: 1.0,
                ..DelayParams::default()
            },
            SR,
        );
        let mut events = EventList::empty();
        assert!(events.push(TimedEvent {
            offset: 100,
            event: Event::ParamValue {
                id: DELAY_PARAM_MIX,
                value: 1.0,
            },
        }));
        effect.process(&context(frames), &mut bus, &events, None);
        // With mix raised before the echo returns, the repeat is audible.
        let spacing = (40.0 / 1_000.0 * SR as f32) as usize;
        let echo = bus.l[spacing - 8..spacing + 8]
            .iter()
            .fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(echo > 0.5, "mix change did not take: echo peak {echo}");
    }
}
