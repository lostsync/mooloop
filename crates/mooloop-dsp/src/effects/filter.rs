//! Stereo state-variable filter effect with low-, band-, and high-pass modes.

use mooloop_core::{
    FilterMode, FilterParams, FilterSlope, FILTER_PARAM_CUTOFF_HZ, FILTER_PARAM_DRIVE,
    FILTER_PARAM_MODE, FILTER_PARAM_RESONANCE, FILTER_PARAM_SLOPE,
};

use crate::bus::StereoBus;
use crate::event::{Event, EventList};
use crate::filter::{apply_drive, Svf};
use crate::node::{AudioNode, ProcessContext};
use crate::smooth::Smoothed;

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
    left: [Svf; 2],
    right: [Svf; 2],
    params: FilterParams,
    sample_rate: u32,
    cutoff: Smoothed,
    resonance: Smoothed,
    drive: Smoothed,
}

impl FilterEffect {
    pub fn new(params: FilterParams, sample_rate: u32) -> Self {
        Self {
            left: [Svf::new(), Svf::new()],
            right: [Svf::new(), Svf::new()],
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

    fn select_output(mode: FilterMode, output: (f32, f32, f32)) -> f32 {
        match mode {
            FilterMode::LowPass => output.0,
            FilterMode::BandPass => output.1,
            FilterMode::HighPass => output.2,
        }
    }

    fn process_channel(
        stages: &mut [Svf; 2],
        input: f32,
        cutoff: f32,
        resonance: f32,
        sample_rate: u32,
        mode: FilterMode,
        slope: FilterSlope,
    ) -> f32 {
        let first = Self::select_output(
            mode,
            stages[0].next_sample_lp_bp_hp(input, cutoff, resonance, sample_rate),
        );
        let second = Self::select_output(
            mode,
            stages[1].next_sample_lp_bp_hp(first, cutoff, resonance, sample_rate),
        );
        if slope == FilterSlope::Db24 {
            second
        } else {
            first
        }
    }

    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        let sr = self.sample_rate;
        for i in start..end {
            let cutoff = self.cutoff.advance();
            let resonance = self.resonance.advance();
            let drive = self.drive.advance();
            let mode = self.params.mode;
            let slope = self.params.slope;
            bus.l[i] = Self::process_channel(
                &mut self.left,
                apply_drive(bus.l[i], drive),
                cutoff,
                resonance,
                sr,
                mode,
                slope,
            );
            bus.r[i] = Self::process_channel(
                &mut self.right,
                apply_drive(bus.r[i], drive),
                cutoff,
                resonance,
                sr,
                mode,
                slope,
            );
        }
    }
}

impl AudioNode for FilterEffect {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        let frames = ctx.frames.min(bus.capacity());

        // Split the block at parameter events: process, apply, repeat —
        // the same shape instruments use for note events.
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
        let settle = frames / 2;
        let energy: f32 = bus.l[settle..frames].iter().map(|s| s * s).sum();
        (energy / (frames - settle) as f32).sqrt()
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
        let rms =
            |range: &[f32]| (range.iter().map(|s| s * s).sum::<f32>() / range.len() as f32).sqrt();
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
        let max_step = (1..frames)
            .map(|i| (bus.l[i] - bus.l[i - 1]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_step < 0.1,
            "cutoff change left a discontinuity of {max_step}"
        );
    }
}
