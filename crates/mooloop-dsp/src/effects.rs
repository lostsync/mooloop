//! Chainable effect nodes. Effects implement `AudioNode` like instruments
//! do, but read and modify the bus in place (see `node.rs`'s processing
//! model) and stay ignorant of channel-strip concepts (gain/pan/mute) so the
//! same node can later run on a master or send bus without changes.

use mooloop_core::{FilterMode, FilterParams};

use crate::bus::StereoBus;
use crate::event::{Event, EventList};
use crate::filter::Svf;
use crate::node::{AudioNode, ProcessContext};

/// `Event::ParamValue` id for `FilterParams::cutoff_hz`.
pub const FILTER_PARAM_CUTOFF_HZ: u32 = 0;
/// `Event::ParamValue` id for `FilterParams::resonance`.
pub const FILTER_PARAM_RESONANCE: u32 = 1;
/// `Event::ParamValue` id for `FilterParams::mode` (0 = low-pass, >= 0.5 = high-pass).
pub const FILTER_PARAM_MODE: u32 = 2;

/// A stereo low-pass/high-pass filter built on two `Svf` instances, one per
/// channel. Parameter changes arrive as sample-timed `ParamValue` events
/// mixed into the channel's regular event list.
pub struct FilterEffect {
    left: Svf,
    right: Svf,
    params: FilterParams,
    sample_rate: u32,
}

impl FilterEffect {
    pub fn new(params: FilterParams, sample_rate: u32) -> Self {
        Self {
            left: Svf::new(),
            right: Svf::new(),
            params,
            sample_rate,
        }
    }

    pub fn params(&self) -> FilterParams {
        self.params
    }

    /// Replace the parameter set. Called from the engine's command path when
    /// the whole set changes at once (e.g. project load).
    pub fn set_params(&mut self, params: FilterParams) {
        self.params = params;
    }

    fn apply_param(&mut self, id: u32, value: f32) {
        match id {
            FILTER_PARAM_CUTOFF_HZ => self.params.cutoff_hz = value.max(0.0),
            FILTER_PARAM_RESONANCE => self.params.resonance = value.clamp(0.0, 1.0),
            FILTER_PARAM_MODE => {
                self.params.mode = if value >= 0.5 {
                    FilterMode::HighPass
                } else {
                    FilterMode::LowPass
                };
            }
            _ => {}
        }
    }

    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        let cutoff = self.params.cutoff_hz;
        let resonance = self.params.resonance;
        let sr = self.sample_rate;
        for i in start..end {
            let (in_l, in_r) = (bus.l[i], bus.r[i]);
            let (lp_l, hp_l) = self.left.next_sample_lp_hp(in_l, cutoff, resonance, sr);
            let (lp_r, hp_r) = self.right.next_sample_lp_hp(in_r, cutoff, resonance, sr);
            match self.params.mode {
                FilterMode::LowPass => {
                    bus.l[i] = lp_l;
                    bus.r[i] = lp_r;
                }
                FilterMode::HighPass => {
                    bus.l[i] = hp_l;
                    bus.r[i] = hp_r;
                }
            }
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
}
