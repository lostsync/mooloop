//! Seven-band parametric equalizer with dedicated high- and low-pass filters.
//!
//! The filter bank is entirely fixed-size: coefficient changes are made only
//! at sample-timed parameter boundaries, and processing itself performs no
//! allocation or locking.
//!
//! Deliberately **not** running through `crate::smooth` like the other
//! effects (see `docs/plans/archive/share-dsp-primitives/01-smooth-effect-parameters.md`).
//! `apply_param` already recomputes coefficients at the exact sample offset
//! of the event, so a knob drag does not zipper the way an unsmoothed
//! amplitude parameter does elsewhere. What's left is a different artifact:
//! each `Biquad` carries state (`z1`, `z2`) across the coefficient swap, so a
//! large jump can still produce a brief transient, and directly smoothing
//! *coefficients* risks a momentarily-unstable filter rather than fixing the
//! click. The real fix is crossfading old/new coefficient sets over a short
//! window — for up to 19 biquad stages per channel, that is real per-sample
//! cost for an artifact that in practice shows up on hard knob jumps, not
//! typical band-gain automation. Deferred; revisit if it turns out to be
//! audible in practice.

use mooloop_core::{EqBand, EqBandKind, EqParams, EQ_MAX_BANDS};

use crate::bus::StereoBus;
use crate::event::{Event, EventList};
use crate::node::{AudioNode, ProcessContext};

const PASS_STAGES: usize = 6;

#[derive(Clone, Copy)]
struct Biquad {
    b0: f32, b1: f32, b2: f32, a1: f32, a2: f32,
    z1: f32, z2: f32,
}

impl Biquad {
    const fn identity() -> Self { Self { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, z1: 0.0, z2: 0.0 } }
    fn process(&mut self, input: f32) -> f32 {
        let out = self.b0 * input + self.z1;
        self.z1 = self.b1 * input - self.a1 * out + self.z2;
        self.z2 = self.b2 * input - self.a2 * out;
        out
    }
    fn set_normalized(&mut self, b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) {
        let inv = a0.max(1e-12).recip();
        self.b0 = b0 * inv; self.b1 = b1 * inv; self.b2 = b2 * inv;
        self.a1 = a1 * inv; self.a2 = a2 * inv;
    }
    fn peak(&mut self, frequency: f32, q: f32, gain_db: f32, sr: u32) {
        let w = core::f32::consts::TAU * frequency.clamp(20.0, sr as f32 * 0.45) / sr as f32;
        let alpha = w.sin() / (2.0 * q.clamp(0.15, 30.0));
        let a = 10.0_f32.powf(gain_db.clamp(-24.0, 24.0) / 40.0);
        self.set_normalized(1.0 + alpha * a, -2.0 * w.cos(), 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * w.cos(), 1.0 - alpha / a);
    }
    fn shelf(&mut self, frequency: f32, gain_db: f32, low: bool, sr: u32) {
        let w = core::f32::consts::TAU * frequency.clamp(20.0, sr as f32 * 0.45) / sr as f32;
        let a = 10.0_f32.powf(gain_db.clamp(-24.0, 24.0) / 40.0);
        let alpha = w.sin() * 0.5 * (a + a.recip()).sqrt();
        let beta = 2.0 * a.sqrt() * alpha;
        let c = w.cos();
        if low {
            self.set_normalized(a * ((a + 1.0) - (a - 1.0) * c + beta), 2.0 * a * ((a - 1.0) - (a + 1.0) * c), a * ((a + 1.0) - (a - 1.0) * c - beta), (a + 1.0) + (a - 1.0) * c + beta, -2.0 * ((a - 1.0) + (a + 1.0) * c), (a + 1.0) + (a - 1.0) * c - beta);
        } else {
            self.set_normalized(a * ((a + 1.0) + (a - 1.0) * c + beta), -2.0 * a * ((a - 1.0) + (a + 1.0) * c), a * ((a + 1.0) + (a - 1.0) * c - beta), (a + 1.0) - (a - 1.0) * c + beta, 2.0 * ((a - 1.0) - (a + 1.0) * c), (a + 1.0) - (a - 1.0) * c - beta);
        }
    }
    fn pass(&mut self, frequency: f32, q: f32, high: bool, sr: u32) {
        let w = core::f32::consts::TAU * frequency.clamp(20.0, sr as f32 * 0.45) / sr as f32;
        let alpha = w.sin() / (2.0 * q.clamp(0.15, 30.0));
        let c = w.cos();
        if high {
            self.set_normalized((1.0 + c) * 0.5, -(1.0 + c), (1.0 + c) * 0.5, 1.0 + alpha, -2.0 * c, 1.0 - alpha);
        } else {
            self.set_normalized((1.0 - c) * 0.5, 1.0 - c, (1.0 - c) * 0.5, 1.0 + alpha, -2.0 * c, 1.0 - alpha);
        }
    }
}

pub struct EqEffect {
    params: EqParams,
    sample_rate: u32,
    left: [Biquad; EQ_MAX_BANDS + PASS_STAGES * 2],
    right: [Biquad; EQ_MAX_BANDS + PASS_STAGES * 2],
}

impl EqEffect {
    pub fn new(params: EqParams, sample_rate: u32) -> Self {
        let mut effect = Self { params, sample_rate, left: [Biquad::identity(); EQ_MAX_BANDS + PASS_STAGES * 2], right: [Biquad::identity(); EQ_MAX_BANDS + PASS_STAGES * 2] };
        effect.update_coefficients();
        effect
    }

    fn effective_q(band: EqBand) -> f32 {
        let boost = match band.q_profile { mooloop_core::EqQProfile::Constant => 1.0, mooloop_core::EqQProfile::Proportional => 1.0 + band.gain_db.abs() / 12.0 };
        (band.q * boost).clamp(0.15, 30.0)
    }

    fn set_band_coefficients(filter: &mut Biquad, band: EqBand, sr: u32) {
        if !band.enabled { *filter = Biquad::identity(); return; }
        match band.kind {
            EqBandKind::Bell => filter.peak(band.frequency_hz, Self::effective_q(band), band.gain_db, sr),
            EqBandKind::LowShelf => filter.shelf(band.frequency_hz, band.gain_db, true, sr),
            EqBandKind::HighShelf => filter.shelf(band.frequency_hz, band.gain_db, false, sr),
        }
    }

    fn update_coefficients(&mut self) {
        for index in 0..EQ_MAX_BANDS {
            Self::set_band_coefficients(&mut self.left[index], self.params.bands[index], self.sample_rate);
            Self::set_band_coefficients(&mut self.right[index], self.params.bands[index], self.sample_rate);
        }
        for stage in 0..PASS_STAGES {
            let index = EQ_MAX_BANDS + stage;
            let enabled = self.params.high_pass.enabled && stage < self.params.high_pass.slope.stages();
            if enabled {
                self.left[index].pass(self.params.high_pass.frequency_hz, self.params.high_pass.q, true, self.sample_rate);
                self.right[index].pass(self.params.high_pass.frequency_hz, self.params.high_pass.q, true, self.sample_rate);
            } else { self.left[index] = Biquad::identity(); self.right[index] = Biquad::identity(); }
        }
        for stage in 0..PASS_STAGES {
            let index = EQ_MAX_BANDS + PASS_STAGES + stage;
            let enabled = self.params.low_pass.enabled && stage < self.params.low_pass.slope.stages();
            if enabled {
                self.left[index].pass(self.params.low_pass.frequency_hz, self.params.low_pass.q, false, self.sample_rate);
                self.right[index].pass(self.params.low_pass.frequency_hz, self.params.low_pass.q, false, self.sample_rate);
            } else { self.left[index] = Biquad::identity(); self.right[index] = Biquad::identity(); }
        }
    }

    fn apply_param(&mut self, id: u32, value: f32) {
        let mut state = mooloop_core::EffectParams::Eq(self.params);
        if state.set(id, value).is_some() {
            if let mooloop_core::EffectParams::Eq(params) = state {
                self.params = params;
                self.update_coefficients();
            }
        }
    }

    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        for frame in start..end {
            let mut l = bus.l[frame]; let mut r = bus.r[frame];
            for index in 0..self.left.len() { l = self.left[index].process(l); r = self.right[index].process(r); }
            bus.l[frame] = l; bus.r[frame] = r;
        }
    }
}

impl AudioNode for EqEffect {
    fn process(&mut self, ctx: &ProcessContext, bus: &mut StereoBus, events_in: &EventList, _events_out: Option<&mut EventList>) {
        let frames = ctx.frames.min(bus.capacity());
        let mut pos = 0;
        for event in events_in.iter() {
            let offset = (event.offset as usize).min(frames).max(pos);
            self.process_range(bus, pos, offset);
            if let Event::ParamValue { id, value } = event.event { self.apply_param(id, value); }
            pos = offset;
        }
        self.process_range(bus, pos, frames);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eq_boosts_the_selected_peak_frequency() {
        let sr = 48_000;
        let mut params = EqParams::default();
        params.bands[1].gain_db = 12.0;
        let mut effect = EqEffect::new(params, sr);
        let mut bus = StereoBus::with_capacity(sr as usize / 2);
        for i in 0..bus.capacity() { let sample = (i as f32 * 1_000.0 * core::f32::consts::TAU / sr as f32).sin(); bus.l[i] = sample; bus.r[i] = sample; }
        let ctx = ProcessContext { sample_rate: sr, frames: bus.capacity(), playing: true, bpm: 120.0, position_ticks: 0.0, position_frames: 0 };
        effect.process(&ctx, &mut bus, &EventList::empty(), None);
        let rms = (bus.l[bus.capacity()/2..].iter().map(|s| s*s).sum::<f32>() / (bus.capacity()/2) as f32).sqrt();
        assert!(rms > 1.1, "boosted sine RMS {rms}");
    }
}
