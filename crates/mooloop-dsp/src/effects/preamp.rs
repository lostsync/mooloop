//! The channel strip's input stage, insertable on its own.
//!
//! [`crate::preamp`] is the stage; this is the device around it. The strip
//! will carry the same voicings when it exists
//! (`docs/plans/archive/console/03`), and this is deliberately not waiting
//! for it: a rack had nowhere to automate
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

use crate::analysis::{SpectrumAnalyzer, SPECTRUM_BINS, SPECTRUM_FLOOR_DB};
use crate::bus::StereoBus;
use crate::event::{Event, EventList};
use crate::node::{AudioNode, ProcessContext};
use crate::preamp::{preamp_voicing, Preamp};
use crate::smooth::Smoothed;

/// Drive, mix and output all scale amplitude directly, so a step in any of
/// them is either a click or a zipper. Matches `drive.rs` for the same reason.
const PARAM_SMOOTH_S: f32 = 0.005;

/// How much a band has to gain for the display to read it full.
///
/// A band the stage did not touch reads zero, and one that gained this much
/// or more reads one.
///
/// Twelve decibels, measured rather than picked: `Iron` pushed 12 dB and
/// trimmed back puts about 6.5 dB into its second harmonic's band over what
/// the input already had there, which reads a little over half scale. At
/// twenty-four the same signal was a quarter-lit, which is a display you
/// squint at rather than read.
///
/// A band the input already had content in moves *less*, not more, because
/// the comparison is against what was there. So a busy mix lights up less
/// than a sine does, which is correct and worth expecting.
const DEVIATION_FULL_DB: f32 = 12.0;

/// Below this the comparison is two noise floors differing by a decibel, and
/// showing it would make a quiet passage shimmer. The display is blank there
/// rather than busy.
const DEVIATION_GATE_DB: f32 = -70.0;

pub struct PreampEffect {
    params: PreampParams,
    sample_rate: u32,
    left: Preamp,
    right: Preamp,
    drive: Smoothed,
    mix: Smoothed,
    output: Smoothed,
    /// What arrived and what left, band by band.
    ///
    /// **Two analyzers rather than one, because the interesting question is
    /// what the stage *added*.** One analyzer draws the signal, which a
    /// preamp barely changes the shape of; the difference between these two
    /// draws the distortion, and it is the only display that can show the
    /// thing the tilt exists for -- obvious on a kick, nearly clean on a hat.
    ///
    /// Compared as magnitudes rather than nulled as a residual on purpose.
    /// Nulling `wet - g * dry` is purer in principle and depends on getting
    /// `g` exactly right: half a decibel of mismatch leaks the fundamental
    /// back in far above the harmonics being looked for, and lights up the
    /// band the *note* is in. A magnitude difference does not care about
    /// phase or a small gain error, and the two analyzers leak identically so
    /// even the rectangular window's skirts largely cancel.
    dry: Box<SpectrumAnalyzer>,
    wet: Box<SpectrumAnalyzer>,
    /// The last deviation computed, waiting for the host to take it.
    pending_display: Option<[f32; SPECTRUM_BINS]>,
}

impl PreampEffect {
    pub fn new(params: PreampParams, sample_rate: u32) -> Self {
        let smoothed = |initial| Smoothed::new(initial, PARAM_SMOOTH_S, sample_rate);
        let table = preamp_voicing(params.voicing);
        Self {
            params,
            sample_rate,
            left: Preamp::new(table, sample_rate),
            right: Preamp::new(table, sample_rate),
            drive: smoothed(db_to_linear(params.drive_db.clamp(-24.0, 24.0))),
            mix: smoothed(params.mix.clamp(0.0, 1.0)),
            output: smoothed(db_to_linear(params.output_db.clamp(-24.0, 24.0))),
            dry: Box::new(SpectrumAnalyzer::new()),
            wet: Box::new(SpectrumAnalyzer::new()),
            pending_display: None,
        }
    }

    /// Per-band decibels gained between what arrived and what left, **after
    /// the broadband level difference is taken out.**
    ///
    /// That subtraction is the whole of the difference between a display that
    /// shows distortion and one that shows the Drive knob. Drive is a gain:
    /// at +18 dB the output is louder in *every* band, and a naive
    /// `wet - dry` reads 0.77 of full scale in the band the note itself is
    /// in -- which a reader would take, reasonably, as "it is distorting my
    /// note". It was not. It was louder.
    ///
    /// So both spectra are referred to their own loudest band before being
    /// compared, which cancels any gain applied to all of them equally and
    /// leaves only the redistribution. That redistribution is what a
    /// nonlinearity does and what a gain cannot.
    ///
    /// Both vectors are normalized against the same [`SPECTRUM_FLOOR_DB`], so
    /// a difference in normalized level scales back to decibels by the floor
    /// -- which is why that constant is public rather than spelled here a
    /// second time.
    fn deviation(dry: &[f32; SPECTRUM_BINS], wet: &[f32; SPECTRUM_BINS]) -> [f32; SPECTRUM_BINS] {
        let span = -SPECTRUM_FLOOR_DB;
        let peak = |bands: &[f32; SPECTRUM_BINS]| bands.iter().copied().fold(0.0f32, f32::max);
        let level_change = peak(wet) - peak(dry);
        std::array::from_fn(|band| {
            let wet_db = SPECTRUM_FLOOR_DB + wet[band] * span;
            if wet_db < DEVIATION_GATE_DB {
                return 0.0;
            }
            let gained = (wet[band] - dry[band] - level_change) * span;
            (gained / DEVIATION_FULL_DB).clamp(0.0, 1.0)
        })
    }

    /// Swap the stage for a different voicing.
    ///
    /// A voicing is a different set of filters, so this rebuilds rather than
    /// retunes, and the new stage starts empty. That is a discontinuity, and
    /// it is the right one: the alternative is running the old voicing's
    /// filter state through the new one's curve, which is neither voicing.
    fn rebuild_voicing(&mut self) {
        let table = preamp_voicing(self.params.voicing);
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
        let watching = self.params.display_enabled;
        for i in start..end {
            let drive = self.drive.advance();
            let mix = self.mix.advance();
            let output = self.output.advance();
            let (dry_l, dry_r) = (bus.l[i], bus.r[i]);

            let wet_l = self.left.process(dry_l, drive);
            let wet_r = self.right.process(dry_r, drive);

            let out_l = (dry_l + (wet_l - dry_l) * mix) * output;
            let out_r = (dry_r + (wet_r - dry_r) * mix) * output;
            bus.l[i] = out_l;
            bus.r[i] = out_r;

            if watching {
                // What arrived, against what the strip will actually hear --
                // after the blend and the trim, because a parallel mix at 20%
                // really is distorting the output by a fifth as much.
                self.dry.write((dry_l + dry_r) * 0.5);
                self.wet.write((out_l + out_r) * 0.5);
            }
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

    /// This device draws what it *did*, not what arrived, so it owns its
    /// stage and the host's generic input analyzer stands down.
    fn provides_display_spectrum(&self) -> bool {
        self.params.display_enabled
    }

    fn take_display_spectrum(&mut self) -> Option<[f32; SPECTRUM_BINS]> {
        self.pending_display.take()
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
        if self.params.display_enabled {
            self.dry.prepare(ctx.sample_rate);
            self.wet.prepare(ctx.sample_rate);
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

        // Both rings advance together and are the same length, so they come
        // due on the same sample. Taking one without the other would compare
        // this block's output against the previous block's input.
        if self.params.display_enabled {
            if let (Some(dry), Some(wet)) = (self.dry.take(), self.wet.take()) {
                self.pending_display = Some(Self::deviation(&dry, &wet));
            }
        }
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

    /// Run enough blocks for both rings to fill and a hop to come due.
    fn deviation_of(params: PreampParams, hz: f32, amplitude: f32) -> [f32; SPECTRUM_BINS] {
        let mut node = PreampEffect::new(params, SAMPLE_RATE);
        let frames = 512;
        let mut phase = 0.0f32;
        let step = std::f32::consts::TAU * hz / SAMPLE_RATE as f32;
        for _ in 0..16 {
            let mut bus = StereoBus::with_capacity(frames);
            for i in 0..frames {
                let sample = amplitude * phase.sin();
                phase += step;
                bus.l[i] = sample;
                bus.r[i] = sample;
            }
            node.process(&context(frames), &mut bus, &EventList::empty(), None);
            if let Some(levels) = node.take_display_spectrum() {
                return levels;
            }
        }
        panic!("no display frame after 16 blocks");
    }

    fn band_hz(index: usize) -> f32 {
        20.0_f32 * (21_600.0_f32 / 20.0_f32).powf(index as f32 / (SPECTRUM_BINS - 1) as f32)
    }

    /// **The claim, as a test.** A voicing driven into distortion lights the
    /// bands its harmonics land in, and not the one the note is in.
    ///
    /// That second half is the whole reason this compares magnitudes instead
    /// of nulling a residual: a gain mismatch would light the fundamental's
    /// own band, which is exactly the band a reader would take as "it is
    /// distorting my note".
    #[test]
    fn the_display_finds_the_harmonics_and_not_the_fundamental() {
        let deviation = deviation_of(
            PreampParams {
                voicing: PreampVoicing::Iron,
                drive_db: 12.0,
                output_db: -12.0,
                display_enabled: true,
                ..PreampParams::default()
            },
            220.0,
            mooloop_core::db_to_linear(mooloop_core::REFERENCE_PEAK_DBFS),
        );

        let nearest = |hz: f32| {
            (0..SPECTRUM_BINS)
                .min_by(|a, b| {
                    (band_hz(*a) - hz).abs().total_cmp(&(band_hz(*b) - hz).abs())
                })
                .unwrap()
        };
        let fundamental = deviation[nearest(220.0)];
        let second = deviation[nearest(440.0)];
        let third = deviation[nearest(660.0)];
        println!(
            "220 Hz {fundamental:.3}, 440 Hz {second:.3}, 660 Hz {third:.3}"
        );
        assert!(
            second > 0.05 || third > 0.05,
            "neither harmonic band moved: 2nd {second:.3}, 3rd {third:.3}"
        );
        assert!(
            fundamental < 0.1,
            "the fundamental's own band read {fundamental:.3}: the comparison \
             is reading the Drive knob rather than distortion"
        );
        assert!(
            fundamental < second.max(third),
            "the fundamental moved more than its harmonics"
        );
    }

    /// `Moo` is the identity, so there is nothing to show and the display
    /// says so rather than drawing the noise floor against itself.
    #[test]
    fn a_transparent_voicing_deviates_nowhere() {
        let deviation = deviation_of(
            PreampParams {
                display_enabled: true,
                ..PreampParams::default()
            },
            220.0,
            mooloop_core::db_to_linear(mooloop_core::REFERENCE_PEAK_DBFS),
        );
        for (band, value) in deviation.iter().enumerate() {
            assert_eq!(
                *value, 0.0,
                "Moo deviated at band {band} ({:.0} Hz)",
                band_hz(band)
            );
        }
    }

    /// Nothing is analyzed, and nothing is published, while nobody is
    /// looking. The two analyzers are most of this device's cost.
    #[test]
    fn the_display_costs_nothing_while_it_is_off() {
        let params = PreampParams {
            voicing: PreampVoicing::Iron,
            drive_db: 18.0,
            ..PreampParams::default()
        };
        assert!(!params.display_enabled, "the fixture proves nothing if it is on");
        let mut node = PreampEffect::new(params, SAMPLE_RATE);
        assert!(!node.provides_display_spectrum());
        for _ in 0..16 {
            let mut bus = tone(512);
            node.process(&context(512), &mut bus, &EventList::empty(), None);
            assert!(node.take_display_spectrum().is_none());
        }
    }
}
