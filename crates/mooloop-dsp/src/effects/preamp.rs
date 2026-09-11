//! The channel strip's input stage, insertable on its own.
//!
//! [`crate::preamp`] is the stage; this is the device around it. The strip
//! will carry the same voicings when it exists (`docs/plans/console/03`), and
//! this is deliberately not waiting for it: a rack had nowhere to automate
//! gain in the middle of a chain, and a preamp is a gain stage that happens
//! to have a character.
//!
//! **Not oversampled, unlike `drive`.** The voicings are authored at the
//! -12 dBFS operating level and reach a few percent THD there; `drive` exists
//! to be pushed to tens of percent and needs the oversampler for it. Adding
//! one here would buy inaudible alias rejection and cost every strip the
//! oversampler's latency, which is the thing that makes this device usable in
//! the middle of a chain at all.

use mooloop_core::{
    db_to_linear, PreampParams, PreampVoicing, PREAMP_PARAM_DRIVE_DB, PREAMP_PARAM_MIX,
    PREAMP_PARAM_OUTPUT_DB, PREAMP_PARAM_VOICING,
};

use crate::bus::StereoBus;
use crate::event::{Event, EventList};
use crate::node::{AudioNode, ProcessContext};
use crate::preamp::{Preamp, GRIP_PREAMP, IRON_PREAMP, MOO_PREAMP, PUNCH_PREAMP};
use crate::smooth::Smoothed;

/// Drive, mix and output all scale amplitude directly, so a step in any of
/// them is either a click or a zipper. Matches `drive.rs` for the same reason.
const PARAM_SMOOTH_S: f32 = 0.005;

/// The voicing table for a persisted choice.
///
/// The mapping lives here rather than in `mooloop-core` because the table is
/// a DSP fact — what the curve is made of — and the choice is a project fact.
/// `mooloop-core` does not depend on `mooloop-dsp`, which is what keeps the
/// two from being one type that has to be both.
fn voicing_table(voicing: PreampVoicing) -> crate::preamp::PreampVoicing {
    match voicing {
        PreampVoicing::Moo => MOO_PREAMP,
        PreampVoicing::Grip => GRIP_PREAMP,
        PreampVoicing::Punch => PUNCH_PREAMP,
        PreampVoicing::Iron => IRON_PREAMP,
    }
}

pub struct PreampEffect {
    params: PreampParams,
    sample_rate: u32,
    left: Preamp,
    right: Preamp,
    drive: Smoothed,
    mix: Smoothed,
    output: Smoothed,
}

impl PreampEffect {
    pub fn new(params: PreampParams, sample_rate: u32) -> Self {
        let smoothed = |initial| Smoothed::new(initial, PARAM_SMOOTH_S, sample_rate);
        let table = voicing_table(params.voicing);
        Self {
            params,
            sample_rate,
            left: Preamp::new(table, sample_rate),
            right: Preamp::new(table, sample_rate),
            drive: smoothed(db_to_linear(params.drive_db.clamp(-24.0, 24.0))),
            mix: smoothed(params.mix.clamp(0.0, 1.0)),
            output: smoothed(db_to_linear(params.output_db.clamp(-24.0, 24.0))),
        }
    }

    pub fn params(&self) -> PreampParams {
        self.params
    }

    /// Replace the parameter set wholesale (project load) — jump straight to
    /// the new values, there is nothing to click coming from a fresh load.
    pub fn set_params(&mut self, params: PreampParams) {
        let voicing_changed = params.voicing != self.params.voicing;
        self.params = params;
        if voicing_changed {
            self.rebuild_voicing();
        }
        self.drive
            .reset_to(db_to_linear(params.drive_db.clamp(-24.0, 24.0)));
        self.mix.reset_to(params.mix.clamp(0.0, 1.0));
        self.output
            .reset_to(db_to_linear(params.output_db.clamp(-24.0, 24.0)));
    }

    /// Swap the stage for a different voicing.
    ///
    /// A voicing is a different set of filters, so this rebuilds rather than
    /// retunes, and the new stage starts empty. That is a discontinuity, and
    /// it is the right one: the alternative is running the old voicing's
    /// filter state through the new one's curve, which is neither voicing.
    fn rebuild_voicing(&mut self) {
        let table = voicing_table(self.params.voicing);
        self.left = Preamp::new(table, self.sample_rate);
        self.right = Preamp::new(table, self.sample_rate);
    }

    fn apply_param(&mut self, id: u32, value: f32) {
        match id {
            PREAMP_PARAM_DRIVE_DB => {
                self.params.drive_db = value.clamp(-24.0, 24.0);
                self.drive.set_target(db_to_linear(self.params.drive_db));
            }
            PREAMP_PARAM_VOICING => {
                let voicing = PreampVoicing::from_index(value.round() as i32);
                if voicing != self.params.voicing {
                    self.params.voicing = voicing;
                    self.rebuild_voicing();
                }
            }
            PREAMP_PARAM_MIX => {
                self.params.mix = value.clamp(0.0, 1.0);
                self.mix.set_target(self.params.mix);
            }
            PREAMP_PARAM_OUTPUT_DB => {
                self.params.output_db = value.clamp(-24.0, 24.0);
                self.output.set_target(db_to_linear(self.params.output_db));
            }
            _ => {}
        }
    }

    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        for i in start..end {
            let drive = self.drive.advance();
            let mix = self.mix.advance();
            let output = self.output.advance();
            let (dry_l, dry_r) = (bus.l[i], bus.r[i]);

            let wet_l = self.left.process(dry_l, drive);
            let wet_r = self.right.process(dry_r, drive);

            bus.l[i] = (dry_l + (wet_l - dry_l) * mix) * output;
            bus.r[i] = (dry_r + (wet_r - dry_r) * mix) * output;
        }
    }
}

impl AudioNode for PreampEffect {
    /// Zero, and `mooloop-dsp` asserts this against `EffectKind::Preamp`.
    /// The tilt pair is a biquad sandwich and a DC blocker is one pole:
    /// every one of them is causal and none looks ahead.
    fn latency_frames(&self) -> u32 {
        0
    }

    /// Two things hold audio after the input stops: the tilt/untilt biquad
    /// pair, cornered as low as 150 Hz, and the 5 Hz DC blocker. The blocker
    /// is much the longer of the two — a one-pole at 5 Hz needs
    /// `ln(REST_EPSILON) / ln(1 - 2 pi f / fs)` samples to decay, which is
    /// about 0.6 s at 48 kHz and proportional to the rate.
    ///
    /// A second is comfortably over that at every supported rate, and the
    /// cost of over-reporting is one extra block of a cheap effect. `Moo`
    /// runs none of it and says so.
    fn tail_frames(&self) -> u32 {
        if !self.drive.is_settled() || !self.mix.is_settled() || !self.output.is_settled() {
            return u32::MAX;
        }
        if self.left.is_transparent() {
            return 0;
        }
        self.sample_rate
    }

    /// `Moo` is memoryless — the stage is skipped outright — so it is at rest
    /// as soon as its trims have stopped moving. Any other voicing has filter
    /// state that has to drain, which is what `tail_frames` is for.
    fn is_at_rest(&self) -> bool {
        self.left.is_transparent()
            && self.drive.is_settled()
            && self.mix.is_settled()
            && self.output.is_settled()
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
            self.rebuild_voicing();
            self.drive.set_time(PARAM_SMOOTH_S, ctx.sample_rate);
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

    const SAMPLE_RATE: u32 = 48_000;

    fn context(frames: usize) -> ProcessContext {
        ProcessContext {
            sample_rate: SAMPLE_RATE,
            frames,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        }
    }

    /// A 220 Hz sine at the operating level, which is where the voicings are
    /// authored and so the level anything about them should be asked at.
    fn tone(frames: usize) -> StereoBus {
        let mut bus = StereoBus::with_capacity(frames);
        let amplitude = mooloop_core::db_to_linear(mooloop_core::REFERENCE_PEAK_DBFS);
        for i in 0..frames {
            let phase = std::f32::consts::TAU * 220.0 * i as f32 / SAMPLE_RATE as f32;
            bus.l[i] = amplitude * phase.sin();
            bus.r[i] = amplitude * phase.sin();
        }
        bus
    }

    fn run(params: PreampParams, bus: &mut StereoBus, events: &EventList) {
        let frames = bus.capacity();
        let mut node = PreampEffect::new(params, SAMPLE_RATE);
        node.process(&context(frames), bus, events, None);
    }

    /// `Moo` at unity is the identity, sample for sample.
    ///
    /// Not "close to" the identity: `HarmonicProfile::TRANSPARENT` makes
    /// `Preamp` skip its whole chain, and `db_to_linear(0.0)` is exactly 1.0,
    /// so the arithmetic in `process_range` is `x + (x - x) * 1.0` times one.
    /// A device inserted and left alone must not colour anything, because a
    /// user reaching for it as a gain stage is entitled to that.
    #[test]
    fn moo_at_unity_is_bit_identical_to_no_device() {
        let mut bus = tone(512);
        let reference = tone(512);
        run(PreampParams::default(), &mut bus, &EventList::empty());
        for i in 0..512 {
            assert_eq!(bus.l[i], reference.l[i], "Moo altered sample {i}");
            assert_eq!(bus.r[i], reference.r[i], "Moo altered sample {i}");
        }
    }

    /// The trims still work in `Moo`, which is the whole reason the device is
    /// insertable: a chain had nowhere to automate gain.
    #[test]
    fn moo_still_carries_its_trims() {
        let mut bus = tone(512);
        run(
            PreampParams {
                output_db: -6.0,
                ..PreampParams::default()
            },
            &mut bus,
            &EventList::empty(),
        );
        let peak = bus.peak(512).0;
        let expected = mooloop_core::db_to_linear(mooloop_core::REFERENCE_PEAK_DBFS - 6.0);
        assert!(
            (peak - expected).abs() < 1e-3,
            "peaked at {peak}, expected about {expected}"
        );
    }

    /// An output-trim event lands where it is timed, which is what an
    /// automation lane is.
    #[test]
    fn an_output_event_takes_effect_at_its_own_offset() {
        let frames = 8192;
        let mut bus = tone(frames);
        let mut events = EventList::empty();
        events.push(TimedEvent {
            offset: (frames / 2) as u32,
            event: Event::ParamValue {
                id: PREAMP_PARAM_OUTPUT_DB,
                value: -24.0,
            },
        });
        run(PreampParams::default(), &mut bus, &events);

        let peak = |range: std::ops::Range<usize>| {
            bus.l[range].iter().fold(0.0f32, |m, s| m.max(s.abs()))
        };
        let unity = mooloop_core::db_to_linear(mooloop_core::REFERENCE_PEAK_DBFS);
        let before = peak(0..frames / 2);
        assert!((before - unity).abs() < 1e-3, "peaked at {before} before the event");

        // 50 ms past the event, which is ten times the smoother's own time,
        // so what is measured is the value and not the ramp.
        let settled = frames / 2 + SAMPLE_RATE as usize / 20;
        let after = peak(settled..frames);
        let expected = unity * mooloop_core::db_to_linear(-24.0);
        assert!(
            (after - expected).abs() < expected * 0.05,
            "after the event the peak was {after}, expected about {expected}"
        );
    }

    /// Each coloured voicing reaches the audio, and `Moo` does not.
    ///
    /// Deliberately weak about *how much*: `harmonics.rs` owns the harmonic
    /// targets and asserts them to 0.5 dB. What this holds is the wiring —
    /// that the persisted choice selects a different table and that the table
    /// gets as far as a sample.
    #[test]
    fn every_voicing_but_moo_changes_the_signal() {
        let reference = tone(2048);
        for voicing in [
            PreampVoicing::Moo,
            PreampVoicing::Grip,
            PreampVoicing::Punch,
            PreampVoicing::Iron,
        ] {
            let mut bus = tone(2048);
            run(
                PreampParams {
                    voicing,
                    ..PreampParams::default()
                },
                &mut bus,
                &EventList::empty(),
            );
            // Past the DC blocker's settling, so the difference measured is
            // the curve rather than the one-pole coming up.
            let difference = (1024..2048)
                .map(|i| (bus.l[i] - reference.l[i]).abs())
                .fold(0.0f32, f32::max);
            match voicing {
                PreampVoicing::Moo => {
                    assert_eq!(difference, 0.0, "Moo coloured the signal")
                }
                _ => assert!(
                    difference > 1e-5,
                    "{voicing:?} did not reach the audio: {difference:e}"
                ),
            }
        }
    }

    /// A dry mix is dry whatever the voicing is doing.
    #[test]
    fn mix_at_zero_is_the_dry_signal() {
        let reference = tone(1024);
        let mut bus = tone(1024);
        run(
            PreampParams {
                voicing: PreampVoicing::Iron,
                drive_db: 18.0,
                mix: 0.0,
                ..PreampParams::default()
            },
            &mut bus,
            &EventList::empty(),
        );
        for i in 0..1024 {
            assert_eq!(bus.l[i], reference.l[i], "a dry mix coloured sample {i}");
        }
    }
}
