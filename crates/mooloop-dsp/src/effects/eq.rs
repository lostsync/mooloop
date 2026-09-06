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
//!
//! **Only the stages that are switched on are run.** The bank is nineteen
//! biquads a channel and a working band count of three or four is normal, so
//! running the lot was mostly arithmetic on identity coefficients. Skipping
//! them is exact rather than an approximation — see [`EqEffect::active`].

use mooloop_core::{EqBand, EqBandKind, EqParams, EQ_MAX_BANDS};

use crate::bus::StereoBus;
use crate::event::{Event, EventList};
use crate::node::{AudioNode, ProcessContext, REST_EPSILON};

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
    /// With no input, `process` returns `z1`, so a stage whose two stored
    /// samples are both below [`REST_EPSILON`] can only emit values below it.
    fn is_at_rest(&self) -> bool {
        self.z1.abs() <= REST_EPSILON && self.z2.abs() <= REST_EPSILON
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

/// Every stage the bank can hold: seven bands, then a high-pass and a
/// low-pass of up to six stages each.
const STAGES: usize = EQ_MAX_BANDS + PASS_STAGES * 2;

pub struct EqEffect {
    params: EqParams,
    sample_rate: u32,
    left: [Biquad; STAGES],
    right: [Biquad; STAGES],
    /// Which stages are doing something, rebuilt whenever the coefficients
    /// are.
    ///
    /// A stage that is switched off is set to [`Biquad::identity`], which is
    /// `out = input` with no state to evolve -- so running it is arithmetic
    /// whose result is the input it was given, and skipping it is exact
    /// rather than approximate. The bank is nineteen stages and a working
    /// band count of three or four is normal, so running all of them was
    /// four-fifths waste: `eq_costs_what_its_enabled_bands_cost` measures the
    /// difference and `a_disabled_band_is_bit_identical_to_not_having_it`
    /// holds the exactness the skip rests on.
    active: [u8; STAGES],
    active_len: usize,
}

impl EqEffect {
    pub fn new(params: EqParams, sample_rate: u32) -> Self {
        let mut effect = Self {
            params,
            sample_rate,
            left: [Biquad::identity(); STAGES],
            right: [Biquad::identity(); STAGES],
            active: [0; STAGES],
            active_len: 0,
        };
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
        // `live` is recorded per stage as it is set, rather than re-derived
        // from the parameters afterwards: the two would be the same rule
        // written twice, and the copy that drifts is the one that decides
        // whether a band is *heard*.
        let mut live = [false; STAGES];
        for (index, band) in self.params.bands.iter().enumerate() {
            Self::set_band_coefficients(&mut self.left[index], *band, self.sample_rate);
            Self::set_band_coefficients(&mut self.right[index], *band, self.sample_rate);
            live[index] = band.enabled;
        }
        for stage in 0..PASS_STAGES {
            let index = EQ_MAX_BANDS + stage;
            let enabled = self.params.high_pass.enabled && stage < self.params.high_pass.slope.stages();
            if enabled {
                self.left[index].pass(self.params.high_pass.frequency_hz, self.params.high_pass.q, true, self.sample_rate);
                self.right[index].pass(self.params.high_pass.frequency_hz, self.params.high_pass.q, true, self.sample_rate);
            } else { self.left[index] = Biquad::identity(); self.right[index] = Biquad::identity(); }
            live[index] = enabled;
        }
        for stage in 0..PASS_STAGES {
            let index = EQ_MAX_BANDS + PASS_STAGES + stage;
            let enabled = self.params.low_pass.enabled && stage < self.params.low_pass.slope.stages();
            if enabled {
                self.left[index].pass(self.params.low_pass.frequency_hz, self.params.low_pass.q, false, self.sample_rate);
                self.right[index].pass(self.params.low_pass.frequency_hz, self.params.low_pass.q, false, self.sample_rate);
            } else { self.left[index] = Biquad::identity(); self.right[index] = Biquad::identity(); }
            live[index] = enabled;
        }
        self.active_len = 0;
        for (index, live) in live.iter().enumerate() {
            if *live {
                self.active[self.active_len] = index as u8;
                self.active_len += 1;
            }
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
        // Nothing switched on is not "an EQ that happens to be flat": it is
        // no filter at all, and the samples should not be touched.
        if self.active_len == 0 {
            return;
        }
        let active = &self.active[..self.active_len];
        for frame in start..end {
            let mut l = bus.l[frame];
            let mut r = bus.r[frame];
            for stage in active {
                let index = *stage as usize;
                l = self.left[index].process(l);
                r = self.right[index].process(r);
            }
            bus.l[frame] = l;
            bus.r[frame] = r;
        }
    }
}

impl AudioNode for EqEffect {
    /// Only the stages that are switched on carry state: a disabled one is
    /// [`Biquad::identity`], whose `z1`/`z2` never leave zero. So this walks
    /// the same `active` list the sample loop does, which on a working band
    /// count of three or four is a dozen comparisons rather than seventy-six.
    fn is_at_rest(&self) -> bool {
        self.active[..self.active_len].iter().all(|&stage| {
            let stage = stage as usize;
            self.left[stage].is_at_rest() && self.right[stage].is_at_rest()
        })
    }

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

    fn ctx_for(sr: u32, frames: usize) -> ProcessContext {
        ProcessContext {
            sample_rate: sr,
            frames,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        }
    }

    /// A noise-ish input, deterministic so two runs compare exactly.
    fn signal(frames: usize) -> StereoBus {
        let mut bus = StereoBus::with_capacity(frames);
        let mut x = 0x1234_5678u32;
        for index in 0..frames {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let sample = (x >> 8) as f32 / (1 << 23) as f32 - 1.0;
            bus.l[index] = sample * 0.5;
            bus.r[index] = sample * 0.4;
        }
        bus
    }

    /// Every stage off. `EqParams::default()` enables bands 0, 1 and 2 at
    /// 0 dB -- flat to the ear but *computed*, so a test about exactness has
    /// to start from a bank it controls entirely rather than from the
    /// default.
    fn all_off() -> EqParams {
        let mut params = EqParams::default();
        for band in &mut params.bands {
            band.enabled = false;
        }
        params.high_pass.enabled = false;
        params.low_pass.enabled = false;
        params
    }

    fn render(params: EqParams, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let sr = 48_000;
        let mut effect = EqEffect::new(params, sr);
        let mut bus = signal(frames);
        effect.process(&ctx_for(sr, frames), &mut bus, &EventList::empty(), None);
        (bus.l[..frames].to_vec(), bus.r[..frames].to_vec())
    }

    /// The property the stage skip rests on, and the reason it is a
    /// correctness test rather than a benchmark: a disabled band is
    /// [`Biquad::identity`], whose output *is* its input and whose state
    /// cannot evolve, so a bank that steps over it must produce the same
    /// samples as one that runs it. Bit-identical, not close.
    #[test]
    fn a_disabled_band_is_bit_identical_to_not_having_it() {
        let frames = 4_096;
        let mut one = all_off();
        one.bands[1].enabled = true;
        one.bands[1].gain_db = 9.0;
        one.bands[1].frequency_hz = 900.0;

        // The same working band, with the other six switched off around it
        // and carrying settings that would be very audible if they ran.
        let mut padded = one;
        for index in [0, 2, 3, 4, 5, 6] {
            padded.bands[index].enabled = false;
            padded.bands[index].gain_db = 18.0;
            padded.bands[index].frequency_hz = 40.0 * (index as f32 + 1.0);
        }

        let (left, right) = render(one, frames);
        let (padded_left, padded_right) = render(padded, frames);
        assert_eq!(left, padded_left, "a disabled band moved the left channel");
        assert_eq!(right, padded_right, "a disabled band moved the right channel");
        assert!(
            left.iter().any(|s| s.abs() > 0.01),
            "the comparison ran on silence"
        );
    }

    /// An EQ with nothing switched on is not a flat filter that happens to
    /// return its input -- it must not touch the samples at all.
    #[test]
    fn an_eq_with_nothing_enabled_passes_the_signal_through_untouched() {
        let frames = 1_024;
        let (left, right) = render(all_off(), frames);
        let source = signal(frames);
        assert_eq!(left, source.l[..frames].to_vec());
        assert_eq!(right, source.r[..frames].to_vec());
    }

    /// Switching a band off mid-block has to take effect from that sample,
    /// and the stage list is rebuilt on the same path the coefficients are --
    /// so this is what fails if the two ever stop being updated together.
    #[test]
    fn disabling_a_band_mid_block_stops_it_from_that_sample() {
        let sr = 48_000;
        let frames = 2_048;
        let mut params = all_off();
        params.bands[0].enabled = true;
        params.bands[0].gain_db = 18.0;
        params.bands[0].frequency_hz = 500.0;

        // Select band 0, then switch it off half way through.
        let mut events = EventList::empty();
        events.push(crate::event::TimedEvent {
            offset: 0,
            event: Event::ParamValue {
                id: mooloop_core::EQ_PARAM_TARGET,
                value: 0.0,
            },
        });
        events.push(crate::event::TimedEvent {
            offset: (frames / 2) as u32,
            event: Event::ParamValue {
                id: mooloop_core::EQ_PARAM_ENABLED,
                value: 0.0,
            },
        });

        let mut effect = EqEffect::new(params, sr);
        let mut bus = signal(frames);
        effect.process(&ctx_for(sr, frames), &mut bus, &events, None);
        let source = signal(frames);

        // Before the event the band is working; after it the samples are the
        // input untouched.
        assert!(
            (0..frames / 2).any(|i| (bus.l[i] - source.l[i]).abs() > 1e-4),
            "the band was not audible before it was switched off"
        );
        for index in frames / 2..frames {
            assert_eq!(
                bus.l[index], source.l[index],
                "frame {index} was still filtered after the band was switched off"
            );
        }
    }

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
