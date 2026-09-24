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

use mooloop_core::{eq_band_of, eq_pass_of, eq_plot_frequency, EqBand, EqParams, EQ_MAX_BANDS};

use crate::biquad::Biquad;
use crate::bus::StereoBus;
use crate::event::EventList;
use crate::modulator::CONTROL_RATE_FRAMES;
use crate::node::{AudioNode, ControlCurve, ProcessContext};
use super::{process_curve_split, process_param_split, CurveFrame, RangeProcessor};

const PASS_STAGES: usize = 6;

/// Every stage the bank can hold: seven bands, then a high-pass and a
/// low-pass of up to six stages each.
pub const STAGES: usize = EQ_MAX_BANDS + PASS_STAGES * 2;

pub struct EqEffect {
    params: EqParams,
    sample_rate: u32,
    left: [Biquad; STAGES],
    right: [Biquad; STAGES],
    /// Which stages are doing something. Kept rather than re-scanned from
    /// `params` on every rebuild, because [`Self::resolve_dirty`] updates it
    /// incrementally alongside whichever stages it redesigns.
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
    /// One stage's own liveness, tracked alongside `active` so a partial
    /// rebuild can fold a changed stage into the compact `active` list
    /// without re-scanning every field of `params`.
    live: [bool; STAGES],
    /// Which stages have a parameter change waiting to be designed in.
    /// Marked by `apply_param`, resolved lazily by [`Self::resolve_dirty`] --
    /// see that function's doc comment for why the rebuild happens there and
    /// not eagerly inside `apply_param`.
    ///
    /// Starts entirely `true` so `new`'s own call to `resolve_dirty` builds
    /// the whole bank the same way any later edit's does -- one rebuild
    /// path, not a separate one-off build in the constructor plus an
    /// incremental one afterwards.
    dirty: [bool; STAGES],
    /// This block's curves, captured by `apply_curves` and consumed by
    /// `process`. Empty on a block with nothing modulated or automated, in
    /// which case `process` falls back to the ordinary event-timed split.
    curve_frame: CurveFrame,
    /// Test-only instrumentation: how many times each stage has actually
    /// been redesigned. A behavioural test cannot otherwise tell a
    /// dirty-flag skip apart from a redundant rebuild that happens to land
    /// on the same coefficients -- both produce identical output by
    /// definition -- so this counts the thing the mechanism claims to save.
    #[cfg(test)]
    rebuilds: [u32; STAGES],
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
            live: [false; STAGES],
            // Every stage starts dirty, so the resolve below builds the
            // whole bank exactly as the old unconditional
            // `update_coefficients` did. Built here, eagerly, rather than
            // left for the first `process_range` -- `is_at_rest` reads
            // `active`/`left`/`right` through `&self` and cannot resolve a
            // dirty flag itself, so a node that had never rendered would
            // otherwise answer "at rest" about a bank it had not designed
            // yet.
            dirty: [true; STAGES],
            curve_frame: CurveFrame::empty(),
            #[cfg(test)]
            rebuilds: [0; STAGES],
        };
        effect.resolve_dirty();
        effect
    }

    /// Redesign whichever stages `apply_param` marked since the last call,
    /// and refold the compact `active` list if any of them changed liveness.
    ///
    /// Called from [`RangeProcessor::process_range`] rather than from
    /// `apply_param` itself, which is what makes this a *coalescing* rebuild
    /// rather than merely a targeted one: `process_curve_split` and
    /// `process_param_split` both call every tick's `apply_param`s before
    /// the `process_range` that renders that tick, so several parameters
    /// changing at the same control tick mark their stages dirty and are
    /// resolved together, once, immediately before the audio that needs
    /// them -- rather than once per `apply_param` call, which is what let a
    /// size-like knob and a fast automation lane on the same stage pay for
    /// the same trig twice in one tick under the old eager scheme.
    fn resolve_dirty(&mut self) {
        let mut refolded = false;
        for band in 0..EQ_MAX_BANDS {
            if !self.dirty[band] {
                continue;
            }
            let value = self.params.bands[band];
            design_band(&mut self.left[band], value, self.sample_rate);
            design_band(&mut self.right[band], value, self.sample_rate);
            self.live[band] = value.enabled;
            self.dirty[band] = false;
            refolded = true;
            #[cfg(test)]
            {
                self.rebuilds[band] += 1;
            }
        }
        for (pass_index, high) in [(0usize, true), (1usize, false)] {
            let filter = if high {
                self.params.high_pass
            } else {
                self.params.low_pass
            };
            for stage in 0..PASS_STAGES {
                let index = EQ_MAX_BANDS + pass_index * PASS_STAGES + stage;
                if !self.dirty[index] {
                    continue;
                }
                let enabled = filter.enabled && stage < filter.slope.stages();
                if enabled {
                    self.left[index].pass(filter.frequency_hz, filter.q, high, self.sample_rate);
                    self.right[index].pass(filter.frequency_hz, filter.q, high, self.sample_rate);
                } else {
                    self.left[index] = Biquad::identity();
                    self.right[index] = Biquad::identity();
                }
                self.live[index] = enabled;
                self.dirty[index] = false;
                refolded = true;
                #[cfg(test)]
                {
                    self.rebuilds[index] += 1;
                }
            }
        }
        if refolded {
            self.active_len = 0;
            for (index, live) in self.live.iter().enumerate() {
                if *live {
                    self.active[self.active_len] = index as u8;
                    self.active_len += 1;
                }
            }
        }
    }

    /// Mark every stage `id` affects dirty, without redesigning anything --
    /// [`Self::resolve_dirty`] does that, lazily, coalescing however many
    /// parameters changed at this control tick into one pass.
    fn mark_dirty(&mut self, id: u32) {
        if let Some((band, _field)) = eq_band_of(id) {
            if band < EQ_MAX_BANDS {
                self.dirty[band] = true;
            }
            return;
        }
        if let Some((pass, _field)) = eq_pass_of(id) {
            for stage in 0..PASS_STAGES {
                let index = EQ_MAX_BANDS + pass * PASS_STAGES + stage;
                if index < STAGES {
                    self.dirty[index] = true;
                }
            }
        }
    }
}

/// One band's coefficients, from the one place those laws live.
///
/// **A shelf's `q` is its slope.** Until 2026-09-14 this called
/// `Biquad::shelf`, which takes no Q at all, so a band's Q knob did nothing
/// whatever while that band was a shelf -- and said nothing about it. The
/// strip had run `shelf_slope` since it was built, and its own comment says
/// why: a band's `q` knob is its slope when it is a shelf and its Q when it
/// is a bell, which is what makes the same knob honest in both positions.
///
/// The bell arm was not wrong, and was worse than wrong: it applied a
/// **private copy** of `eq_effective_q`, byte-identical and therefore green,
/// in a codebase whose core function carries the sentence "the law written
/// twice is the law that drifts, and the copy that drifts is the one deciding
/// what is heard." Both arms are `Biquad::eq_band` now, which the strip calls
/// too, so there is nothing left to hold in agreement.
///
/// **This changes how an existing shelf boost sounds**, and it has to: the
/// two shelf forms reach `alpha` differently, so the only shelf they agree on
/// is a flat one. A shelf at 0 dB is unaffected whatever its slope -- `A == 1`
/// makes numerator and denominator identical -- so a default EQ, whose two
/// shelves rest at 0 dB, sounds exactly as it did. A song that boosted one
/// does not. `docs/plans/eq-v2/00-status.md` records that as the thing to
/// listen to.
fn design_band(filter: &mut Biquad, band: EqBand, sample_rate: u32) {
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
        sample_rate,
    );
}

/// Design the whole bank into `stages`, and report which of them are live.
///
/// One function rather than a loop here and a second one in
/// [`eq_response_db`], for the reason this file already gives about the
/// active list: the two would be the same rule written twice, and the copy
/// that drifts is the one that decides what is *seen*. The response plot does
/// not approximate this bank. It is this bank, evaluated.
///
/// The two pass loops were also the same ten lines twice, differing in a
/// field name and a boolean -- `AGENTS.md`'s note that `repeated-line` cannot
/// see a copy somebody renamed on the way past is about exactly this shape.
///
/// It writes into stages the caller owns rather than returning fresh ones,
/// because the audio path's stages carry `z1`/`z2` across a coefficient swap
/// and handing back new ones would zero them: a knob move would click.
pub fn design_eq_bank(
    params: &EqParams,
    sample_rate: u32,
    stages: &mut [Biquad; STAGES],
) -> [bool; STAGES] {
    // `live` is recorded per stage as it is set, rather than re-derived from
    // the parameters afterwards: the two would be the same rule written
    // twice, and the copy that drifts is the one that decides whether a band
    // is *heard*.
    let mut live = [false; STAGES];
    for (index, band) in params.bands.iter().enumerate() {
        design_band(&mut stages[index], *band, sample_rate);
        live[index] = band.enabled;
    }
    for (pass, (filter, high)) in [(&params.high_pass, true), (&params.low_pass, false)]
        .into_iter()
        .enumerate()
    {
        for stage in 0..PASS_STAGES {
            let index = EQ_MAX_BANDS + pass * PASS_STAGES + stage;
            let enabled = filter.enabled && stage < filter.slope.stages();
            if enabled {
                stages[index].pass(filter.frequency_hz, filter.q, high, sample_rate);
            } else {
                stages[index] = Biquad::identity();
            }
            live[index] = enabled;
        }
    }
    live
}

/// The bank's gain at each of `samples` points along the response plot's
/// frequency axis, in decibels.
///
/// **This is what the device does, not a shape that resembles it.** Every
/// stage is designed by the function the audio path designs it with and
/// evaluated by [`Biquad::magnitude_db`], which is held to a measured sine in
/// `biquad.rs`. The alternative -- and what `EqResponseDisplay` drew until
/// 2026-09-14 -- is a rational approximation in the markup, which could not
/// follow a shelf's slope, had no idea the pass filters existed, and was a
/// second law for the picture sitting next to the first law for the sound.
///
/// Sampled in Rust and handed over as a flat array, the way the channel
/// strip's compressor curve already is: `DynamicsCurveDisplay.curve-db` says
/// why in the same words.
pub fn eq_response_db(params: &EqParams, sample_rate: u32, samples: usize) -> Vec<f32> {
    let mut stages = [Biquad::identity(); STAGES];
    let live = design_eq_bank(params, sample_rate, &mut stages);
    let samples = samples.max(2);
    (0..samples)
        .map(|index| {
            let hz = eq_plot_frequency(index as f32 / (samples - 1) as f32);
            live.iter()
                .enumerate()
                .filter(|(_, live)| **live)
                .map(|(stage, _)| stages[stage].magnitude_db(hz, sample_rate))
                .sum()
        })
        .collect()
}

impl RangeProcessor for EqEffect {
    fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        // Lazily resolves whatever `apply_param` marked dirty since the last
        // call -- see `resolve_dirty`'s own doc comment for why the rebuild
        // lives here rather than in `apply_param`. Cheap when nothing is
        // dirty: nineteen bool reads, not nineteen redesigns.
        self.resolve_dirty();
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
                self.mark_dirty(id);
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

    /// The EQ's native curve path: every modulated or automated band/pass
    /// field this block resolved to, copied into `curve_frame` for `process`
    /// to consume. This is what removes the EQ from the shared, 256-slot
    /// `EventList`'s capacity -- the device finding 3 names as the worst
    /// offender, at up to fifty destinations (`EQ_DESCRIPTOR_COUNT`) that
    /// used to compete for the same list every other driven device did too.
    fn apply_curves(
        &mut self,
        curves: &[ControlCurve<'_>],
        _tick_frames: usize,
        _fallback: &mut EventList,
    ) -> u64 {
        self.curve_frame.capture(curves);
        0
    }

    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        let frames = ctx.frames.min(bus.capacity());
        if self.curve_frame.is_empty() {
            process_param_split(self, bus, events_in, frames);
        } else {
            // Taken rather than borrowed: `process_curve_split` needs `self`
            // by `&mut` for `apply_param`/`process_range` at the same time
            // it reads the curves, and those curves live in a field of
            // `self` -- the borrow checker cannot see that the two uses
            // don't alias, so the frame has to move out first. `take`
            // leaves `self.curve_frame` at `CurveFrame::default()`, which is
            // empty, so this also does the clearing the old code did
            // separately.
            let curve_frame = std::mem::take(&mut self.curve_frame);
            process_curve_split(
                self,
                bus,
                events_in,
                &curve_frame,
                CONTROL_RATE_FRAMES,
                frames,
            );
        }
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

    /// Every stage is dirty right after construction -- `new`'s own call to
    /// `resolve_dirty` is what builds the bank -- so every one of the
    /// nineteen stages should have been designed exactly once.
    #[test]
    fn construction_designs_every_stage_exactly_once() {
        let effect = EqEffect::new(EqParams::default(), 48_000);
        assert_eq!(effect.rebuilds, [1u32; STAGES]);
    }

    /// The dirty-flag mechanism's whole claim: changing one band's parameter
    /// redesigns that band and *only* that band. A behavioural comparison of
    /// the output cannot tell a skip apart from a redundant rebuild landing
    /// on the same coefficients -- both are correct and both look identical
    /// from outside -- so this reads the per-stage rebuild counter the
    /// mechanism keeps for exactly this reason.
    #[test]
    fn changing_one_band_only_redesigns_that_band() {
        let mut effect = EqEffect::new(EqParams::default(), 48_000);
        effect.apply_param(mooloop_core::eq_band_param(2, mooloop_core::EQ_BAND_GAIN), 6.0);
        // `apply_param` only marks the flag; `resolve_dirty` is what a real
        // block calls (from `process_range`) to fold it in.
        assert_eq!(
            effect.rebuilds, [1u32; STAGES],
            "apply_param alone must not have redesigned anything yet"
        );
        effect.resolve_dirty();
        for (index, count) in effect.rebuilds.iter().enumerate() {
            if index == 2 {
                assert_eq!(*count, 2, "band 2 should have been redesigned once more");
            } else {
                assert_eq!(
                    *count, 1,
                    "stage {index} was redesigned though only band 2's own \
                     parameter changed"
                );
            }
        }
    }

    /// A high-pass field change redesigns every one of that pass filter's
    /// own stages (they share `frequency_hz`/`q`, so all of them genuinely
    /// depend on it) and touches nothing else -- not the low-pass, which
    /// shares no state with the high-pass, and not any band.
    #[test]
    fn changing_the_high_pass_does_not_redesign_the_low_pass_or_any_band() {
        let mut effect = EqEffect::new(EqParams::default(), 48_000);
        effect.apply_param(
            mooloop_core::eq_pass_param(mooloop_core::EQ_HIGH_PASS, mooloop_core::EQ_PASS_FREQ),
            120.0,
        );
        effect.resolve_dirty();
        let high_pass_start = EQ_MAX_BANDS;
        let low_pass_start = EQ_MAX_BANDS + PASS_STAGES;
        for (index, count) in effect.rebuilds.iter().enumerate() {
            if (high_pass_start..low_pass_start).contains(&index) {
                assert_eq!(*count, 2, "high-pass stage {index} should have been redesigned");
            } else {
                assert_eq!(
                    *count, 1,
                    "stage {index} was redesigned though only the high-pass changed"
                );
            }
        }
    }

    /// Two parameters changing at the same control tick -- exactly what
    /// `process_curve_split` and `process_param_split` both do before
    /// calling `process_range` once -- resolve in one pass: each touched
    /// stage is redesigned once, not once per `apply_param` call that
    /// touched it.
    #[test]
    fn two_params_changing_at_one_tick_coalesce_into_one_resolve() {
        let mut effect = EqEffect::new(EqParams::default(), 48_000);
        effect.apply_param(mooloop_core::eq_band_param(0, mooloop_core::EQ_BAND_GAIN), 3.0);
        effect.apply_param(mooloop_core::eq_band_param(0, mooloop_core::EQ_BAND_Q), 2.0);
        effect.resolve_dirty();
        assert_eq!(
            effect.rebuilds[0], 2,
            "band 0 should have been redesigned once for the tick, not once \
             per parameter that changed on it"
        );
    }

    /// The curve path is a genuine addition, not a different device: driven
    /// through `apply_curves` + `process`, the EQ reaches the same output as
    /// the ordinary event path given the same per-tick values.
    #[test]
    fn the_curve_path_and_the_event_path_agree() {
        let sr = 48_000;
        let frames = 128usize;
        let gain_id = mooloop_core::eq_band_param(0, mooloop_core::EQ_BAND_GAIN);

        let mut via_events = EqEffect::new(EqParams::default(), sr);
        let mut events = EventList::empty();
        events.push_ordered(crate::event::TimedEvent {
            offset: 0,
            event: Event::ParamValue { id: gain_id, value: 6.0 },
        });
        events.push_ordered(crate::event::TimedEvent {
            offset: 32,
            event: Event::ParamValue { id: gain_id, value: 9.0 },
        });
        let mut bus_events = signal(frames);
        via_events.process(&ctx_for(sr, frames), &mut bus_events, &events, None);

        let mut via_curves = EqEffect::new(EqParams::default(), sr);
        let values = [6.0f32, 9.0, 9.0, 9.0];
        let curves = [ControlCurve { id: gain_id, values: &values, kind: crate::node::CurveKind::Value }];
        via_curves.apply_curves(&curves, CONTROL_RATE_FRAMES, &mut EventList::empty());
        let mut bus_curves = signal(frames);
        via_curves.process(&ctx_for(sr, frames), &mut bus_curves, &EventList::empty(), None);

        assert_eq!(bus_events.l[..frames], bus_curves.l[..frames]);
        assert_eq!(bus_events.r[..frames], bus_curves.r[..frames]);
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

    /// **The plot is held to a sine going through the bank.** Not to a second
    /// derivation of the same coefficients, and not to a shape that resembles
    /// them: a tone at each of eight frequencies is rendered through a real
    /// `EqEffect` and its output level compared with what `eq_response_db`
    /// says the device does there.
    ///
    /// This is the standard `02-the-curve-tells-the-truth.md` asked the step
    /// to state out loud. The answer is the strong one -- the drawn curve is
    /// the running filter's magnitude response, to a tenth of a decibel --
    /// and it is reachable only because the curve is sampled in Rust from the
    /// same `design_eq_bank` the audio path calls. The markup's old rational
    /// approximation could not have met it at any tolerance.
    ///
    /// The bank under test has all three band kinds boosted, a proportional-Q
    /// bell, and both pass filters in at different slopes, because those are
    /// the parts the old approximation got wrong or left out entirely.
    #[test]
    fn the_plotted_curve_is_what_a_sine_measures_through_the_bank() {
        let sr = 48_000;
        let mut params = all_off();
        params.bands[0] = EqBand {
            enabled: true,
            kind: mooloop_core::EqBandKind::LowShelf,
            frequency_hz: 200.0,
            gain_db: 6.0,
            q: 1.2,
            q_profile: mooloop_core::EqQProfile::Constant,
        };
        params.bands[1] = EqBand {
            enabled: true,
            kind: mooloop_core::EqBandKind::Bell,
            frequency_hz: 1_000.0,
            gain_db: -8.0,
            q: 2.0,
            q_profile: mooloop_core::EqQProfile::Proportional,
        };
        params.bands[2] = EqBand {
            enabled: true,
            kind: mooloop_core::EqBandKind::HighShelf,
            frequency_hz: 6_000.0,
            gain_db: 9.0,
            q: 0.6,
            q_profile: mooloop_core::EqQProfile::Constant,
        };
        params.high_pass.enabled = true;
        params.high_pass.frequency_hz = 80.0;
        params.high_pass.slope = mooloop_core::EqSlope::Db18;
        params.low_pass.enabled = true;
        params.low_pass.frequency_hz = 12_000.0;
        params.low_pass.slope = mooloop_core::EqSlope::Db12;

        let samples = 129;
        let curve = eq_response_db(&params, sr, samples);

        // The tone is placed at the frequency a *sample of the curve* stands
        // for, rather than at a round number near one. A 36 dB/oct pass
        // filter moves half a decibel between two adjacent points of a
        // 129-point axis, so reading the nearest point to 60 Hz would be
        // measuring the axis's resolution rather than the plot's honesty.
        for index in [8, 24, 40, 56, 72, 88, 104, 120] {
            let hz = eq_plot_frequency(index as f32 / (samples - 1) as f32);
            let drawn = curve[index];

            let frames = sr as usize;
            let mut effect = EqEffect::new(params, sr);
            let mut bus = StereoBus::with_capacity(frames);
            for frame in 0..frames {
                let sample = (frame as f32 * hz * core::f32::consts::TAU / sr as f32).sin();
                bus.l[frame] = sample;
                bus.r[frame] = sample;
            }
            effect.process(&ctx_for(sr, frames), &mut bus, &EventList::empty(), None);

            // A whole number of cycles at the end of the render: the start is
            // the bank's transient, and an RMS over a fraction of a cycle
            // measures where the window landed.
            let period = sr as f32 / hz;
            let span = ((frames as f32 / 2.0 / period).floor() * period).round() as usize;
            let tail = &bus.l[frames - span..frames];
            let rms = (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt();
            let measured = 20.0 * (rms * core::f32::consts::SQRT_2).log10();

            assert!(
                (drawn - measured).abs() < 0.1,
                "at {hz} Hz the plot draws {drawn} dB and the bank does {measured} dB"
            );
        }
    }

    /// A bank with nothing switched on draws a flat line at 0 dB, which is
    /// the same claim `an_eq_with_nothing_enabled_passes_the_signal_through_untouched`
    /// makes about the samples.
    #[test]
    fn a_bank_with_nothing_on_draws_a_flat_line() {
        for value in eq_response_db(&all_off(), 48_000, 64) {
            assert!(value.abs() < 1e-4, "an empty bank drew {value} dB");
        }
    }

    /// A boosted band lifts a tone sitting on its own centre.
    ///
    /// The band and the tone both come from `EQ_DEFAULT_BAND_HZ` rather than
    /// from a literal: this test boosted band 2 and measured at 1 kHz, which
    /// were the same place until the seven bands spread out on 2026-09-15 and
    /// were an octave and a half apart afterwards -- so it went on measuring
    /// the skirt of a band it had moved away from.
    #[test]
    fn eq_boosts_the_selected_peak_frequency() {
        let sr = 48_000;
        const BAND: usize = 3;
        let tone_hz = mooloop_core::EQ_DEFAULT_BAND_HZ[BAND];
        let mut params = EqParams::default();
        params.bands[BAND].gain_db = 12.0;
        let mut effect = EqEffect::new(params, sr);
        let mut bus = StereoBus::with_capacity(sr as usize / 2);
        for i in 0..bus.capacity() { let sample = (i as f32 * tone_hz * core::f32::consts::TAU / sr as f32).sin(); bus.l[i] = sample; bus.r[i] = sample; }
        let ctx = ProcessContext { sample_rate: sr, frames: bus.capacity(), playing: true, bpm: 120.0, position_ticks: 0.0, position_frames: 0 };
        effect.process(&ctx, &mut bus, &EventList::empty(), None);
        let rms = (bus.l[bus.capacity()/2..].iter().map(|s| s*s).sum::<f32>() / (bus.capacity()/2) as f32).sqrt();
        assert!(rms > 1.1, "boosted sine RMS {rms}");
    }
}
