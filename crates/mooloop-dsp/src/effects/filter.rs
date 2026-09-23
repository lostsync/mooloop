//! Stereo state-variable filter effect with low-, band-, and high-pass modes.

use mooloop_core::{
    FilterMode, FilterParams, FilterSlope, FILTER_PARAM_CUTOFF_HZ, FILTER_PARAM_DRIVE,
    FILTER_PARAM_MODE, FILTER_PARAM_RESONANCE, FILTER_PARAM_SLOPE,
};

use crate::bus::StereoBus;
use crate::event::EventList;
use crate::filter::{SvfCascade, SvfCoeffs, SvfOutput, SvfSlope};
use crate::shaper::{apply_drive, soft_ceiling};
use crate::modulator::CONTROL_RATE_FRAMES;
use crate::node::{AudioNode, ProcessContext};
use crate::smooth::Smoothed;
use super::{process_param_split, RangeProcessor};

/// Cutoff tracks the knob closely: `Svf` is built to stay well behaved with
/// cutoff moving every sample, so there is no reason to lag a sweep.
const CUTOFF_SMOOTH_S: f32 = 0.003;
/// Resonance is a coarser control; a slightly longer lag still reads as
/// instant while smoothing over any coefficient step.
const RESONANCE_SMOOTH_S: f32 = 0.01;
/// Saturation changes harmonic content directly, so avoid a coefficient step
/// when it is automated.
const DRIVE_SMOOTH_S: f32 = 0.005;

/// A stereo state-variable filter. Each channel keeps a two-stage SVF cascade:
/// 12 dB/oct uses the first stage and 24 dB/oct uses both.
pub struct FilterEffect {
    left: SvfCascade,
    right: SvfCascade,
    params: FilterParams,
    sample_rate: u32,
    cutoff: Smoothed,
    resonance: Smoothed,
    drive: Smoothed,
}

impl FilterEffect {
    pub fn new(params: FilterParams, sample_rate: u32) -> Self {
        Self {
            left: SvfCascade::new(),
            right: SvfCascade::new(),
            params,
            sample_rate,
            cutoff: Smoothed::new(params.cutoff_hz.max(0.0), CUTOFF_SMOOTH_S, sample_rate),
            resonance: Smoothed::new(
                params.resonance.clamp(0.0, 1.0),
                RESONANCE_SMOOTH_S,
                sample_rate,
            ),
            drive: Smoothed::new(params.drive.clamp(0.0, 1.0), DRIVE_SMOOTH_S, sample_rate),
        }
    }

    pub fn params(&self) -> FilterParams {
        self.params
    }

    /// Replace the parameter set. Called from the engine's command path when
    /// the whole set changes at once (e.g. project load) — jump straight to
    /// the new values, there is nothing to click coming from a fresh load.
    pub fn set_params(&mut self, params: FilterParams) {
        self.params = params;
        self.cutoff.reset_to(params.cutoff_hz.max(0.0));
        self.resonance.reset_to(params.resonance.clamp(0.0, 1.0));
        self.drive.reset_to(params.drive.clamp(0.0, 1.0));
    }

    fn shape(mode: FilterMode, slope: FilterSlope) -> (SvfOutput, SvfSlope) {
        let output = match mode {
            FilterMode::LowPass => SvfOutput::Low,
            FilterMode::BandPass => SvfOutput::Band,
            FilterMode::HighPass => SvfOutput::High,
        };
        let slope = match slope {
            FilterSlope::Db12 => SvfSlope::Db12,
            FilterSlope::Db24 => SvfSlope::Db24,
        };
        (output, slope)
    }

    /// One channel: drive, the shared compensated cascade (MOO-124: the 24 dB
    /// mode used to run two stages at the full resonance with no
    /// compensation, +40 dB at the cutoff), and the voice ceiling on the way
    /// out, so a resonant peak on a hot input bends instead of leaving the
    /// effect at any level at all.
    fn process_channel(
        cascade: &mut SvfCascade,
        input: f32,
        coeffs: &SvfCoeffs,
        output: SvfOutput,
        slope: SvfSlope,
    ) -> f32 {
        soft_ceiling(cascade.tick_with(input, coeffs, output, slope))
    }
}

impl RangeProcessor for FilterEffect {
    /// Coefficients, not cutoff/resonance, are what varies per sample here.
    /// `Svf::tick` used to re-derive `g`/`a1`/`a2`/`a3`/`damping` (a `tan()`
    /// and a divide) from `self.cutoff.advance()` on every sample; instead
    /// this walks the range in `CONTROL_RATE_FRAMES`-sized chunks (the rate
    /// the engine already resolves modulation at), deriving the coefficient
    /// set once at each chunk's start and once at its end -- the end from
    /// `advance_by`, which lands the smoother exactly where that many
    /// per-sample `advance()` calls would have -- and lerps between the two
    /// per sample. The smoother now smooths the coefficient set, not the
    /// Hz, but at the same rate it always settled at: chunking at the
    /// control rate rather than lerping across the whole range (which can
    /// be much longer than one control tick when nothing further automates
    /// this parameter) is what keeps a one-off knob turn's settle time
    /// matching `CUTOFF_SMOOTH_S`/`RESONANCE_SMOOTH_S` instead of stretching
    /// it out to however long the block happens to be.
    /// See `reports/fable-2026-09-22.md` finding 2, Plan B.
    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        let sr = self.sample_rate;
        let (output, slope) = Self::shape(self.params.mode, self.params.slope);

        let mut pos = start;
        while pos < end {
            let chunk_end = (pos + CONTROL_RATE_FRAMES).min(end);
            let chunk_frames = chunk_end - pos;

            let start_coeffs = SvfCascade::coeffs(
                output,
                slope,
                self.cutoff.value(),
                self.resonance.value(),
                sr,
            );
            let cutoff_end = self.cutoff.advance_by(chunk_frames);
            let resonance_end = self.resonance.advance_by(chunk_frames);
            let end_coeffs = SvfCascade::coeffs(output, slope, cutoff_end, resonance_end, sr);

            for (n, i) in (pos..chunk_end).enumerate() {
                let t = if chunk_frames > 1 {
                    n as f32 / (chunk_frames - 1) as f32
                } else {
                    1.0
                };
                let coeffs = start_coeffs.lerp(&end_coeffs, t);
                let drive = self.drive.advance();
                bus.l[i] = Self::process_channel(
                    &mut self.left,
                    apply_drive(bus.l[i], drive),
                    &coeffs,
                    output,
                    slope,
                );
                bus.r[i] = Self::process_channel(
                    &mut self.right,
                    apply_drive(bus.r[i], drive),
                    &coeffs,
                    output,
                    slope,
                );
            }

            pos = chunk_end;
        }
    }

    fn apply_param(&mut self, id: u32, value: f32) {
        match id {
            FILTER_PARAM_CUTOFF_HZ => {
                self.params.cutoff_hz = value.max(0.0);
                self.cutoff.set_target(self.params.cutoff_hz);
            }
            FILTER_PARAM_RESONANCE => {
                self.params.resonance = value.clamp(0.0, 1.0);
                self.resonance.set_target(self.params.resonance);
            }
            FILTER_PARAM_MODE => {
                self.params.mode = FilterMode::from_index(value.round() as i32);
            }
            FILTER_PARAM_SLOPE => self.params.slope = FilterSlope::from_index(value.round() as i32),
            FILTER_PARAM_DRIVE => {
                self.params.drive = value.clamp(0.0, 1.0);
                self.drive.set_target(self.params.drive);
            }
            _ => {}
        }
    }
}

impl AudioNode for FilterEffect {
    /// Rest is decided from the cascade's own state rather than from a tail
    /// in frames, because there is no honest fixed number here: the settling
    /// time of a state-variable filter runs from a few samples at low
    /// resonance to effectively forever as it approaches self-oscillation.
    /// Reading the four stages is four pairs of comparisons and is exact.
    fn is_at_rest(&self) -> bool {
        self.cutoff.is_settled()
            && self.resonance.is_settled()
            && self.drive.is_settled()
            && self.left.is_at_rest()
            && self.right.is_at_rest()
    }

    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        let frames = ctx.frames.min(bus.capacity());
        process_param_split(self, bus, events_in, frames);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// Settled RMS of the filter's output for a steady sine input.
    fn filtered_sine_rms(freq_hz: f32, params: FilterParams) -> f32 {
        let sr = 48_000u32;
        let frames = sr as usize / 2;
        let mut bus = StereoBus::with_capacity(frames);
        for i in 0..frames {
            let t = i as f32 / sr as f32;
            let s = (t * freq_hz * core::f32::consts::TAU).sin();
            bus.l[i] = s;
            bus.r[i] = s;
        }
        let mut effect = FilterEffect::new(params, sr);
        let events = EventList::empty();
        effect.process(&context(frames), &mut bus, &events, None);
        // Skip the filter's startup transient.
        crate::testkit::rms(&bus.l[frames / 2..frames])
    }

    #[test]
    fn low_pass_attenuates_high_frequencies() {
        let params = FilterParams {
            cutoff_hz: 1_000.0,
            ..FilterParams::default()
        };
        let low = filtered_sine_rms(200.0, params);
        let high = filtered_sine_rms(8_000.0, params);
        assert!(
            low > high * 4.0,
            "low {low} should pass far more than high {high}"
        );
    }

    #[test]
    fn high_pass_attenuates_low_frequencies() {
        let params = FilterParams {
            cutoff_hz: 1_000.0,
            mode: FilterMode::HighPass,
            ..FilterParams::default()
        };
        let low = filtered_sine_rms(200.0, params);
        let high = filtered_sine_rms(8_000.0, params);
        assert!(
            high > low * 4.0,
            "high {high} should pass far more than low {low}"
        );
    }

    #[test]
    fn band_pass_attenuates_both_sides_of_the_cutoff() {
        let params = FilterParams {
            cutoff_hz: 1_000.0,
            mode: FilterMode::BandPass,
            ..FilterParams::default()
        };
        let low = filtered_sine_rms(100.0, params);
        let center = filtered_sine_rms(1_000.0, params);
        let high = filtered_sine_rms(8_000.0, params);

        assert!(low < center * 0.2, "low {low}, center {center}");
        assert!(high < center * 0.25, "high {high}, center {center}");
    }

    #[test]
    fn twenty_four_db_slope_attenuates_more_than_twelve_db() {
        let shallow = filtered_sine_rms(
            8_000.0,
            FilterParams {
                cutoff_hz: 1_000.0,
                slope: FilterSlope::Db12,
                ..FilterParams::default()
            },
        );
        let steep = filtered_sine_rms(
            8_000.0,
            FilterParams {
                cutoff_hz: 1_000.0,
                slope: FilterSlope::Db24,
                ..FilterParams::default()
            },
        );

        assert!(steep < shallow * 0.2, "12 dB {shallow}, 24 dB {steep}");
    }

    #[test]
    fn param_value_events_change_cutoff_mid_block() {
        let sr = 48_000u32;
        let frames = sr as usize / 2;
        let make_bus = || {
            let mut bus = StereoBus::with_capacity(frames);
            for i in 0..frames {
                let t = i as f32 / sr as f32;
                let s = (t * 8_000.0 * core::f32::consts::TAU).sin();
                bus.l[i] = s;
                bus.r[i] = s;
            }
            bus
        };
        // Start fully open, close to 100 Hz halfway through the block.
        let mut effect = FilterEffect::new(
            FilterParams {
                cutoff_hz: 20_000.0,
                ..FilterParams::default()
            },
            sr,
        );
        let mut events = EventList::empty();
        assert!(events.push(TimedEvent {
            offset: (frames / 2) as u32,
            event: Event::ParamValue {
                id: FILTER_PARAM_CUTOFF_HZ,
                value: 100.0,
            },
        }));
        let mut bus = make_bus();
        effect.process(&context(frames), &mut bus, &events, None);
        let rms = |range: &[f32]| {
            (range.iter().map(|s| s * s).sum::<f32>() / range.len() as f32).sqrt()
        };
        let before = rms(&bus.l[frames / 4..frames / 2]);
        let after = rms(&bus.l[3 * frames / 4..]);
        assert!(
            before > after * 4.0,
            "open half {before} should pass far more than closed half {after}"
        );
    }

    #[test]
    fn cutoff_change_mid_block_does_not_click() {
        let sr = 48_000u32;
        let frames = sr as usize / 2;
        let mut bus = StereoBus::with_capacity(frames);
        for i in 0..frames {
            let t = i as f32 / sr as f32;
            // A low, steady tone so the discontinuity under test isn't
            // swamped by the input's own frame-to-frame slope.
            let s = (t * 200.0 * core::f32::consts::TAU).sin();
            bus.l[i] = s;
            bus.r[i] = s;
        }
        let mut effect = FilterEffect::new(
            FilterParams {
                cutoff_hz: 20_000.0,
                ..FilterParams::default()
            },
            sr,
        );
        let mut events = EventList::empty();
        assert!(events.push(TimedEvent {
            offset: (frames / 2) as u32,
            event: Event::ParamValue {
                id: FILTER_PARAM_CUTOFF_HZ,
                value: 100.0,
            },
        }));
        effect.process(&context(frames), &mut bus, &events, None);
        let max_step = crate::testkit::max_step(&bus.l[..frames]);
        assert!(
            max_step < 0.1,
            "cutoff change left a discontinuity of {max_step}"
        );
    }

    /// The per-sample path: the cascade deriving its coefficients fresh every
    /// sample from a held-constant cutoff/resonance, which is what
    /// `process_channel` did before it took a precomputed `SvfCoeffs`. A
    /// static cutoff is the case where the coefficient-lerp path's two
    /// endpoints are identical, so this is the tightest comparison available.
    fn old_path_static_cutoff(input: &[f32], params: FilterParams, sample_rate: u32) -> Vec<f32> {
        let (output, slope) = FilterEffect::shape(params.mode, params.slope);
        let mut cascade = SvfCascade::new();
        input
            .iter()
            .map(|&x| {
                soft_ceiling(cascade.next_sample(
                    x,
                    output,
                    slope,
                    params.cutoff_hz,
                    params.resonance,
                    sample_rate,
                ))
            })
            .collect()
    }

    /// **A NaN block heals (MOO-174).** A block with one NaN in it, then
    /// finite audio: the effect is producing finite audio again within one
    /// block, at every mode and slope, where its filter state used to stay
    /// NaN for good.
    #[test]
    fn a_nan_in_the_input_is_gone_within_a_block() {
        let sr = 48_000u32;
        const BLOCK: usize = 512;
        for mode in [FilterMode::LowPass, FilterMode::BandPass, FilterMode::HighPass] {
            for slope in [FilterSlope::Db12, FilterSlope::Db24] {
                let params = FilterParams {
                    cutoff_hz: 1_000.0,
                    resonance: 0.9,
                    mode,
                    slope,
                    drive: 0.3,
                };
                let mut effect = FilterEffect::new(params, sr);
                let mut phase = 0usize;
                for block in 0..4 {
                    let mut bus = StereoBus::with_capacity(BLOCK);
                    for index in 0..BLOCK {
                        let s = (phase as f32 * 440.0 / sr as f32 * core::f32::consts::TAU).sin() * 0.25;
                        bus.l[index] = s;
                        bus.r[index] = s;
                        phase += 1;
                    }
                    if block == 1 {
                        bus.l[100] = f32::NAN;
                        bus.r[100] = f32::NAN;
                    }
                    effect.process(&context(BLOCK), &mut bus, &EventList::empty(), None);
                    if block >= 2 {
                        assert!(
                            crate::testkit::all_finite(&bus.l[..BLOCK])
                                && crate::testkit::all_finite(&bus.r[..BLOCK]),
                            "{mode:?} {slope:?}: block {block} still carries the NaN"
                        );
                        assert!(crate::testkit::peak(&bus.l[..BLOCK]) > 0.0);
                    }
                }
            }
        }
    }

    /// **The gain bound (MOO-124).** At every mode, slope and resonance, a
    /// reference-level sine anywhere around the cutoff leaves the Filter
    /// effect no more than 21 dB louder, and nothing -- a full-scale sine
    /// straight into the peak -- leaves it above the voice ceiling. Until
    /// MOO-124 the 24 dB mode peaked at +40 dB with nothing after it.
    #[test]
    fn no_mode_slope_or_resonance_peaks_past_its_bound() {
        let sr = 48_000u32;
        let frames = sr as usize / 2;
        let peak_through = |params: FilterParams, freq: f32, level: f32| -> f32 {
            let mut bus = StereoBus::with_capacity(frames);
            for i in 0..frames {
                let s = (i as f32 / sr as f32 * freq * core::f32::consts::TAU).sin() * level;
                bus.l[i] = s;
                bus.r[i] = s;
            }
            let mut effect = FilterEffect::new(params, sr);
            effect.process(&context(frames), &mut bus, &EventList::empty(), None);
            crate::testkit::peak(&bus.l[frames / 2..frames])
        };
        const REFERENCE: f32 = 0.25;
        for mode in [FilterMode::LowPass, FilterMode::BandPass, FilterMode::HighPass] {
            for slope in [FilterSlope::Db12, FilterSlope::Db24] {
                for resonance in [0.0_f32, 0.7, 1.0] {
                    let params = FilterParams {
                        cutoff_hz: 1_000.0,
                        resonance,
                        mode,
                        slope,
                        drive: 0.0,
                    };
                    for step in -6..=6 {
                        let freq = 1_000.0 * 2.0_f32.powf(step as f32 / 12.0);
                        let gain = crate::testkit::db(peak_through(params, freq, REFERENCE) / REFERENCE);
                        assert!(
                            gain <= 21.0,
                            "{mode:?} {slope:?} resonance {resonance} at {freq:.0} Hz: +{gain:.1} dB"
                        );
                    }
                    let hot = peak_through(params, 1_000.0, 1.0);
                    assert!(
                        hot <= crate::shaper::VOICE_CEILING,
                        "{mode:?} {slope:?} resonance {resonance}: a full-scale sine left at {hot}"
                    );
                }
            }
        }
    }

    /// Plan B step 2's bit-compare: at an unchanging cutoff (drive off, so
    /// `apply_drive`'s pass-through doesn't add its own difference), the
    /// coefficient-lerp path's two endpoints coincide, so it should match
    /// the old per-sample-`tan()` path to well under -80 dBFS RMS.
    #[test]
    fn coefficient_lerp_matches_the_old_path_at_a_static_cutoff() {
        let sr = 48_000u32;
        let frames = sr as usize / 2;
        let params = FilterParams {
            cutoff_hz: 1_500.0,
            resonance: 0.4,
            mode: FilterMode::LowPass,
            slope: FilterSlope::Db24,
            drive: 0.0,
        };

        // A chirp, so the comparison covers the whole passband and stopband
        // rather than one frequency.
        let input: Vec<f32> = (0..frames)
            .map(|i| {
                let t = i as f32 / sr as f32;
                let freq = 80.0 + (12_000.0 - 80.0) * (i as f32 / frames as f32);
                (t * freq * core::f32::consts::TAU).sin()
            })
            .collect();

        let old = old_path_static_cutoff(&input, params, sr);

        let mut bus = StereoBus::with_capacity(frames);
        for (i, &x) in input.iter().enumerate() {
            bus.l[i] = x;
            bus.r[i] = x;
        }
        let mut effect = FilterEffect::new(params, sr);
        let events = EventList::empty();
        effect.process(&context(frames), &mut bus, &events, None);

        let mut error_energy = 0.0f64;
        let mut signal_energy = 0.0f64;
        for (&sample, &reference) in bus.l.iter().zip(old.iter()).take(frames) {
            let diff = (sample - reference) as f64;
            error_energy += diff * diff;
            signal_energy += (reference as f64) * (reference as f64);
        }
        let ratio_db = 10.0 * (error_energy / signal_energy.max(1.0e-30)).log10();
        assert!(
            ratio_db < -80.0,
            "coefficient-lerp path differs from the old per-sample path by {ratio_db:.1} dB RMS"
        );
    }

    /// Plan B step 2's zipper test: an envelope-style cutoff sweep, stepped
    /// every 32 frames (the engine's control-tick rate) via `ParamValue`
    /// events, must not leave a sample-to-sample step in the output bigger
    /// than a steady tone's own frame-to-frame slope would produce on its
    /// own -- i.e. no click from the coefficient ramp itself.
    #[test]
    fn envelope_sweep_leaves_no_discontinuity() {
        let sr = 48_000u32;
        // 200 control ticks' worth: comfortably under `EventList`'s
        // `MAX_EVENTS` (256) at one `ParamValue` per tick, which the full
        // half-second buffer the other tests use would not be.
        const TICK: usize = 32;
        let frames = 200 * TICK;
        let mut bus = StereoBus::with_capacity(frames);
        for i in 0..frames {
            let t = i as f32 / sr as f32;
            let s = (t * 220.0 * core::f32::consts::TAU).sin();
            bus.l[i] = s;
            bus.r[i] = s;
        }
        let mut effect = FilterEffect::new(
            FilterParams {
                cutoff_hz: 500.0,
                // Modest resonance: this test is about a click from the
                // coefficient ramp itself, not about how large a resonant
                // peak's own ringing gets -- that's a property of the SVF,
                // present identically before and after this change, and
                // covered by `filter.rs`'s resonance-taper tests instead.
                resonance: 0.3,
                ..FilterParams::default()
            },
            sr,
        );
        let mut events = EventList::empty();
        // A fast "envelope": sweep from 300 Hz to 4 kHz and back, one step
        // per control tick.
        let mut offset = 0u32;
        while (offset as usize) < frames {
            let phase = offset as f32 / frames as f32;
            let sweep = (phase * core::f32::consts::PI).sin();
            let cutoff = 300.0 + 3_700.0 * sweep;
            assert!(events.push(TimedEvent {
                offset,
                event: Event::ParamValue {
                    id: FILTER_PARAM_CUTOFF_HZ,
                    value: cutoff,
                },
            }));
            offset += TICK as u32;
        }
        effect.process(&context(frames), &mut bus, &events, None);

        let max_step = crate::testkit::max_step(&bus.l[..frames])
            .max(crate::testkit::max_step(&bus.r[..frames]));
        // Well above the ordinary per-sample slope of a 220 Hz tone through
        // this filter (well under 0.1 with no automation at all -- see
        // `cutoff_change_mid_block_does_not_click`'s bound), and well below
        // a real click (a full-scale reversal, order 1.0-2.0).
        assert!(
            max_step < 0.5,
            "envelope sweep left a discontinuity of {max_step}"
        );
    }
}
