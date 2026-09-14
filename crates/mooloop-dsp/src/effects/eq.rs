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
//!
//! The bank is built from [`crate::biquad::Biquad`], which was promoted out
//! of this file by the `share-dsp-primitives` plan and then had no caller
//! here for a fortnight, so a fix to the shared RBJ primitive could silently
//! miss the only EQ that needed it.

use mooloop_core::{EqBand, EqParams, EQ_MAX_BANDS};

use crate::biquad::Biquad;
use crate::bus::StereoBus;
use crate::event::EventList;
use crate::node::{AudioNode, ProcessContext};
use super::{process_param_split, RangeProcessor};

const PASS_STAGES: usize = 6;

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

    /// One band's coefficients, from the one place those laws live.
    ///
    /// **A shelf's `q` is its slope.** Until 2026-09-14 this called
    /// `Biquad::shelf`, which takes no Q at all, so a band's Q knob did
    /// nothing whatever while that band was a shelf -- and said nothing about
    /// it. The strip had run `shelf_slope` since it was built, and its own
    /// comment says why: a band's `q` knob is its slope when it is a shelf
    /// and its Q when it is a bell, which is what makes the same knob honest
    /// in both positions.
    ///
    /// The bell arm was not wrong, and was worse than wrong: it applied a
    /// **private copy** of `eq_effective_q`, byte-identical and therefore
    /// green, in a codebase whose core function carries the sentence "the law
    /// written twice is the law that drifts, and the copy that drifts is the
    /// one deciding what is heard." Both arms are `Biquad::eq_band` now, which
    /// the strip calls too, so there is nothing left to hold in agreement.
    ///
    /// **This changes how an existing shelf boost sounds**, and it has to:
    /// the two shelf forms reach `alpha` differently, so the only shelf they
    /// agree on is a flat one. A shelf at 0 dB is unaffected whatever its
    /// slope -- `A == 1` makes numerator and denominator identical -- so a
    /// default EQ, whose two shelves rest at 0 dB, sounds exactly as it did.
    /// A song that boosted one does not. `docs/plans/eq-v2/00-status.md`
    /// records that as the thing to listen to.
    fn set_band_coefficients(filter: &mut Biquad, band: EqBand, sr: u32) {
        if !band.enabled {
            *filter = Biquad::identity();
            return;
        }
        filter.eq_band(
            band.kind,
            band.frequency_hz,
            band.gain_db,
            band.q,
            band.q_profile,
            sr,
        );
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

}

impl RangeProcessor for EqEffect {
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

    fn apply_param(&mut self, id: u32, value: f32) {
        let mut state = mooloop_core::EffectParams::Eq(self.params);
        if state.set(id, value).is_some() {
            if let mooloop_core::EffectParams::Eq(params) = state {
                self.params = params;
                self.update_coefficients();
            }
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
        process_param_split(self, bus, events_in, frames);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Event;

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

        // Switch band 0 off half way through. Two events until `eq-v2/01`:
        // one to *select* band 0 and one to switch "the selected band" off.
        // A band's On switch is its own id now, so the selection is not part
        // of the message and this is one event.
        let mut events = EventList::empty();
        events.push(crate::event::TimedEvent {
            offset: (frames / 2) as u32,
            event: Event::ParamValue {
                id: mooloop_core::eq_band_param(0, mooloop_core::EQ_BAND_ON),
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
