//! The channel strip every track has: the input stage, four EQ bands and a
//! compressor, under one strip-wide voicing.
//!
//! `mooloop_core::strip` is the parameter set a project persists; this is
//! what runs. `docs/plans/archive/console/03-the-channel-strip-device.md`
//! is the work order, and three things in here are its decisions rather than
//! arithmetic.
//!
//! # A voicing selects laws, never values
//!
//! Nothing a voicing does moves a number a knob on the face shows. It owns
//! [`crate::preamp::PreampVoicing`] (the harmonic profile, the tilt and the
//! slew limit, all measured), the **Q law**, the **curve above the knee**,
//! and the **programme dependence** — and every one of those is either
//! invisible or drawn. A voicing that changed the attack time under a knob
//! reading 10 ms would have nowhere to show it, which is why none of them
//! does.
//!
//! # Out means out
//!
//! Each section runs only while its own switch is in, so the strip's cost on
//! an untouched project is reading three booleans. A section switched *in*
//! has its state cleared and its smoothed values put at its knobs first
//! ([`Strip::apply_param`]): a filter bank and a detector that were last fed
//! audio ten minutes ago would otherwise put that audio into the first block
//! back, and a smoother that has not advanced since then would spend that
//! block gliding up from a setting the user has already changed. Those are
//! the two artefacts "free while it is out" could plausibly produce.
//!
//! # It is not a device, so it is not an `AudioNode`
//!
//! No `process(ctx, bus, events)`, no rest/tail contract of its own, and no
//! slot. The bus block loop calls [`Strip::process_block`] at the pinned
//! position (`mooloop_core::mixer::STRIP_PIN`) and asks
//! [`Strip::is_at_rest`] as one more term in `BusStrip::is_resting`. Making
//! it a node would mean giving it an `EffectKind`, which is the thing Adam's
//! mockup ruled out.

use mooloop_core::strip::{
    strip_band_of, StripParams, STRIP_COMP_ATTACK_MS, STRIP_COMP_IN,
    STRIP_COMP_MAKEUP_DB, STRIP_COMP_MIX, STRIP_COMP_RATIO, STRIP_COMP_RELEASE_MS,
    STRIP_COMP_THRESHOLD_DB, STRIP_DRIVE_DB, STRIP_EQ_BANDS, STRIP_EQ_IN, STRIP_PRE_IN,
    STRIP_VOICING,
};
use mooloop_core::{db_to_linear, eq_effective_q, EqBandKind, EqQProfile, StripBand};

use crate::biquad::Biquad;
use crate::bus::StereoBus;
use crate::dynamics::{compressor_gain_db, db_to_lin, lin_to_db, EnvelopeFollower};
use crate::node::DynamicsFrame;
use crate::preamp::Preamp;
use crate::smooth::Smoothed;

/// Drive, mix and makeup all scale amplitude directly, so a step in
/// any of them is a click. The same constant the dynamics effects and the
/// preamp device use, for the same reason.
const PARAM_SMOOTH_S: f32 = 0.005;

/// Overshoot over which a voicing's `ratio_bend` reaches its full effect.
///
/// 24 dB rather than a smaller number because the bend is meant to be the
/// character of *being driven*, not a second knee: at 6 dB over the
/// threshold every voicing is still close to the ratio its knob says.
const BEND_RANGE_DB: f32 = 24.0;

/// The programme-dependent stage's two time constants, as multiples of the
/// knob's *release*.
///
/// Both are relative to the release rather than absolute, which is what
/// makes "a long passage" mean long by the standard the user set: at a
/// 100 ms release the second stage charges over 200 ms and lets go over
/// 800 ms.
///
/// Charging slowly is the half that makes it programme dependent rather than
/// merely slow. The first version scaled the charge off the *attack* -- 12 ms
/// at a 1 ms attack -- which was fast enough to fill on a single transient,
/// so a short burst and a long passage released within 10% of each other and
/// `programme_dependence_remembers_how_long_the_loud_part_lasted` failed by
/// almost exactly nothing. A second stage that fills instantly is a slower
/// release with extra arithmetic.
const PROGRAMME_RELEASE_SCALE: f32 = 8.0;
const PROGRAMME_CHARGE_SCALE: f32 = 2.0;

/// One voicing's EQ frequencies: the hertz behind each band's positions,
/// outer bands first and [`mooloop_core::strip::STRIP_BAND_POSITIONS`] long.
///
/// **A band is stepped, and this is what the steps are worth.** Adam,
/// 2026-09-11: *"i want to have a selectable range of freqs in the eq bands,
/// not fully parametric... i say we dont label. we just have N
/// positions/band and they're selectable. tooltip/statusbar can carry the
/// specific frequency information to the user on hover."*
///
/// That is what lets a voicing own the frequencies without breaking the rule
/// this module opens on. A voicing may not move a number the face shows; the
/// face shows a *position*, and 3 of 7 is true under all four. What the
/// position is worth arrives on hover, from the voicing that is running --
/// so `Iron` can sit a mid at 2.2 kHz where `Moo` puts it at 3 without
/// anything on screen becoming false.
///
/// Switching voicing therefore keeps the position and changes the frequency,
/// which is what swapping a channel module on a desk does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StripEqTable {
    pub bands: [&'static [f32]; STRIP_EQ_BANDS],
}

impl StripEqTable {
    /// The frequency this band's `position` selects, clamped into the table
    /// rather than wrapped: a stale position from a shorter table lands on
    /// the nearest end, which is audible and sane, where a wrap would move a
    /// high shelf to the bottom of the band.
    pub fn frequency(&self, band: usize, position: u8) -> f32 {
        let row = self.bands[band.min(STRIP_EQ_BANDS - 1)];
        row[(position as usize).min(row.len() - 1)]
    }

    /// The position nearest `hz`, in log distance because that is how a
    /// frequency is heard and how the response plot's axis is spaced.
    ///
    /// For the plot's drag: a dragged point snaps to a position rather than
    /// moving between them, which is the feedback a stepped band should give.
    pub fn nearest(&self, band: usize, hz: f32) -> u8 {
        let row = self.bands[band.min(STRIP_EQ_BANDS - 1)];
        let target = hz.max(1.0).ln();
        let mut best = 0usize;
        let mut best_distance = f32::INFINITY;
        for (index, frequency) in row.iter().enumerate() {
            let distance = (frequency.ln() - target).abs();
            if distance < best_distance {
                best_distance = distance;
                best = index;
            }
        }
        best as u8
    }
}

/// `Moo`'s positions: an even, uncoloured spread, with the middle of each
/// band landing where the strip's own default frequency used to be. The
/// voicing that is not pretending to be furniture does not borrow anyone's
/// switch positions.
pub const MOO_EQ: StripEqTable = StripEqTable {
    bands: [
        &[4_000.0, 6_000.0, 8_000.0, 12_000.0, 16_000.0],
        &[800.0, 1_200.0, 2_000.0, 3_000.0, 4_500.0, 6_000.0, 9_000.0],
        &[100.0, 180.0, 280.0, 400.0, 600.0, 900.0, 1_400.0],
        &[40.0, 65.0, 100.0, 150.0, 220.0],
    ],
};

/// `Grip`'s positions, from the SSL 4000's own knob.
///
/// Its EQ is continuous, but the knobs are *labelled*, and the low shelf's
/// five printed marks are exactly `20 / 50 / 100 / 200 / 450` -- so that band
/// is the desk's rather than a choice. The other three are spreads over the
/// spans the same panel prints: LMF 200 Hz-2.5 kHz, HMF 600 Hz-7 kHz, HF
/// 1.5-16 kHz.
///
/// Front-panel values, not measurements. The harmonic profiles beside them
/// came off a spectrum analyser (`spikes/preamp-measure/`); these came off a
/// photograph of a panel, and the difference is worth knowing when somebody
/// later wants to defend one of these numbers.
pub const GRIP_EQ: StripEqTable = StripEqTable {
    bands: [
        &[3_000.0, 5_000.0, 8_000.0, 12_000.0, 16_000.0],
        &[600.0, 1_000.0, 1_500.0, 2_500.0, 3_500.0, 5_000.0, 7_000.0],
        &[200.0, 300.0, 450.0, 700.0, 1_000.0, 1_600.0, 2_500.0],
        &[20.0, 50.0, 100.0, 200.0, 450.0],
    ],
};

/// `Punch`'s positions, from the API 550A's high band and the UA 610's bass.
///
/// `5k / 7k / 10k / 12.5k / 15k` is the 550A's HF switch, five for five, and
/// the low shelf is built around the 610's `70 / 100 / 200`. The two mids are
/// spreads in the 550A's spirit -- it has five where this has seven.
pub const PUNCH_EQ: StripEqTable = StripEqTable {
    bands: [
        &[5_000.0, 7_000.0, 10_000.0, 12_500.0, 15_000.0],
        &[800.0, 1_200.0, 1_800.0, 2_500.0, 3_500.0, 5_000.0, 7_000.0],
        &[100.0, 150.0, 220.0, 300.0, 450.0, 700.0, 1_000.0],
        &[50.0, 70.0, 100.0, 150.0, 220.0],
    ],
};

/// `Iron`'s positions: lower and rounder than the other three, everywhere.
///
/// The EMI-derived voicing sits its mids and its top below where the desks
/// do -- a 2.2 kHz mid rather than 2.5 or 3, a 7 kHz shelf rather than 8 or
/// 10 -- which is most of what makes the same position sound like a
/// different module when the voicing changes under it.
pub const IRON_EQ: StripEqTable = StripEqTable {
    bands: [
        &[3_500.0, 5_000.0, 7_000.0, 10_000.0, 14_000.0],
        &[700.0, 1_000.0, 1_400.0, 2_200.0, 3_200.0, 4_500.0, 6_000.0],
        &[60.0, 90.0, 140.0, 200.0, 300.0, 480.0, 750.0],
        &[35.0, 60.0, 90.0, 130.0, 180.0],
    ],
};

/// One voicing's laws, as data.
///
/// Every field is a number somebody picked and can re-pick, and none of them
/// is shown on a knob. Deliberately a plain table with no behaviour
/// attached, exactly like [`crate::preamp::PreampVoicing`] — so the next
/// measurement pass edits four rows and nothing else.
#[derive(Debug, Clone, Copy)]
pub struct StripVoicing {
    /// The input stage. Measured, 2026-09-10; see `crate::preamp`.
    pub preamp: crate::preamp::PreampVoicing,
    /// Whether a boosted band narrows. The familiar SSL/API trait, and the
    /// one law here that changes what a Q knob means — which is why the
    /// response display plots the Q that is running.
    pub proportional_q: bool,
    /// How the ratio moves as the signal goes further over the knee, per
    /// [`BEND_RANGE_DB`] of overshoot. 0 is the literal ratio line the knob
    /// reads; positive steepens toward a vari-mu, which clamps harder the
    /// harder it is hit; negative eases, which is what makes a voicing feel
    /// loud without clamping.
    pub ratio_bend: f32,
    /// Strength of the programme-dependent release stage, in `[0, 1]`. 0 is
    /// the knob's release and nothing else.
    pub programme: f32,
    /// What each of this voicing's band positions is worth in hertz.
    pub eq: StripEqTable,
}

impl StripVoicing {
    /// Whether this voicing does nothing at all: the null case, and the
    /// reason a track with its sections out is bit-identical to no track.
    pub fn is_transparent(&self) -> bool {
        self.preamp.is_transparent() && self.ratio_bend == 0.0 && self.programme == 0.0
    }

    fn q_profile(&self) -> EqQProfile {
        if self.proportional_q {
            EqQProfile::Proportional
        } else {
            EqQProfile::Constant
        }
    }
}

/// `Moo` — the house voicing, and the identity.
pub const MOO_STRIP: StripVoicing = StripVoicing {
    preamp: crate::preamp::MOO_PREAMP,
    eq: MOO_EQ,
    proportional_q: false,
    ratio_bend: 0.0,
    programme: 0.0,
};

/// `Grip` — fast, tight, controlled. Glue rather than warmth.
///
/// Proportional Q, because a boost that narrows as it grows is most of why
/// the desk this is after can be pushed without smearing. The ratio line is
/// literal: `Grip`'s firmness is meant to be predictable, and the knee knob
/// is where its softness is chosen.
pub const GRIP_STRIP: StripVoicing = StripVoicing {
    preamp: crate::preamp::GRIP_PREAMP,
    eq: GRIP_EQ,
    proportional_q: true,
    ratio_bend: 0.0,
    programme: 0.15,
};

/// `Punch` — forward transients and thick low-mids.
///
/// The ratio *eases* above the knee, which is the mechanism behind "louder
/// at the same level": a moderate passage is compressed at the ratio the
/// knob says and a transient over it is compressed less, so the transient
/// survives.
pub const PUNCH_STRIP: StripVoicing = StripVoicing {
    preamp: crate::preamp::PUNCH_PREAMP,
    eq: PUNCH_EQ,
    proportional_q: true,
    ratio_bend: -0.5,
    programme: 0.35,
};

/// `Iron` — transformer warmth, and the slowest of the four.
///
/// Constant Q, so the EQ stays broad however hard it is used, and a ratio
/// that *steepens* with overshoot — a vari-mu leans on the loud parts and
/// leaves the rest, which with strong programme dependence is a compressor
/// that is hard to hear working.
pub const IRON_STRIP: StripVoicing = StripVoicing {
    preamp: crate::preamp::IRON_PREAMP,
    eq: IRON_EQ,
    proportional_q: false,
    ratio_bend: 0.6,
    programme: 0.6,
};

/// The voicing table for a persisted choice.
///
/// Here rather than in `mooloop-core` for the reason
/// [`crate::preamp::preamp_voicing`] gives: the table is a DSP fact and the
/// choice is a project fact, and `mooloop-core` does not depend on
/// `mooloop-dsp`.
pub fn strip_voicing(voicing: mooloop_core::PreampVoicing) -> StripVoicing {
    match voicing {
        mooloop_core::PreampVoicing::Moo => MOO_STRIP,
        mooloop_core::PreampVoicing::Grip => GRIP_STRIP,
        mooloop_core::PreampVoicing::Punch => PUNCH_STRIP,
        mooloop_core::PreampVoicing::Iron => IRON_STRIP,
    }
}

/// The ratio in force `over_db` above the knee.
///
/// Continuous, equal to `ratio` at the threshold whatever the bend, and
/// monotone in the bend — so the output curve stays monotone in the input at
/// either sign. `a_bent_ratio_stays_monotone` is that as a test, because a
/// gain computer whose output falls as its input rises is a compressor that
/// inverts transients.
fn bent_ratio(ratio: f32, over_db: f32, bend: f32) -> f32 {
    let t = (over_db.max(0.0) / BEND_RANGE_DB) * bend;
    if t >= 0.0 {
        ratio * (1.0 + t)
    } else {
        1.0 + (ratio - 1.0) / (1.0 - t)
    }
}

/// The compressor's static curve, as this strip is running it: output level
/// in dB for `floor_db..0` of input, evenly sampled.
///
/// For the face. `DynamicsCurveDisplay` holds a gain computer of its own and
/// could draw a threshold, a ratio and a knee unaided -- but not the
/// voicing's bend above the knee, and the alternative to handing the display
/// the real curve is teaching the markup a second gain computer. Two
/// formulas under one name, and the copy that drifts is the one a user is
/// reading. So the curve is sampled here, by the same two functions the
/// audio path calls.
///
/// **Every control on the page is in it, including the one that moves the
/// line rather than shaping it.** `mix` blends the curve back toward unity,
/// so at 0 this draws the straight line the section is actually passing --
/// the first version left it out and drew a section clamping while it passed
/// the signal through exactly, with the knob that said so under the plot.
/// The arithmetic below is `process_comp`'s own, per sample of level instead
/// of per sample of audio.
///
/// There were two such controls until 2026-09-11, when the input trim went
/// with the face Adam redrew: *"no input gain."*
pub fn static_curve_db(params: &StripParams, floor_db: f32, samples: usize) -> Vec<f32> {
    let voicing = strip_voicing(params.voicing);
    let samples = samples.max(2);
    let makeup = db_to_lin(params.makeup_db);
    let mix = params.mix.clamp(0.0, 1.0);
    (0..samples)
        .map(|index| {
            let input_db = floor_db + (-floor_db) * index as f32 / (samples - 1) as f32;
            let over = input_db - params.threshold_db;
            let ratio = bent_ratio(params.ratio, over, voicing.ratio_bend);
            let reduction =
                compressor_gain_db(input_db, params.threshold_db, ratio, params.knee_db);
            let wet = db_to_lin(reduction) * makeup;
            input_db + lin_to_db((1.0 - mix) + mix * wet)
        })
        .collect()
}

/// The strip's four-band EQ.
///
/// A fixed bank of four stages a channel, coefficients recomputed only when
/// a parameter moves. Shelves use [`Biquad::shelf_slope`], so a band's `q`
/// knob is its slope when it is a shelf and its Q when it is a bell —
/// which is what makes the same knob honest in both positions.
struct StripEq {
    left: [Biquad; STRIP_EQ_BANDS],
    right: [Biquad; STRIP_EQ_BANDS],
    /// Which stages are doing anything. A band at 0 dB is
    /// [`Biquad::identity`] — its output *is* its input and its state cannot
    /// evolve — so skipping it is exact rather than approximate, the same
    /// argument `EqEffect` records at length.
    active: [u8; STRIP_EQ_BANDS],
    active_len: usize,
}

impl StripEq {
    fn new() -> Self {
        Self {
            left: [Biquad::identity(); STRIP_EQ_BANDS],
            right: [Biquad::identity(); STRIP_EQ_BANDS],
            active: [0; STRIP_EQ_BANDS],
            active_len: 0,
        }
    }

    fn reset(&mut self) {
        for stage in self.left.iter_mut().chain(self.right.iter_mut()) {
            stage.reset();
        }
    }

    fn is_at_rest(&self) -> bool {
        self.active[..self.active_len].iter().all(|stage| {
            let stage = *stage as usize;
            self.left[stage].is_at_rest() && self.right[stage].is_at_rest()
        })
    }

    /// Rebuild every stage from `params`, and the active list with them.
    ///
    /// The list is recorded as each stage is set rather than re-derived
    /// afterwards, for the reason `EqEffect::update_coefficients` gives: the
    /// two would be the same rule written twice, and the copy that drifts is
    /// the one that decides whether a band is heard.
    fn update(&mut self, params: &StripParams, voicing: &StripVoicing, sample_rate: u32) {
        self.active_len = 0;
        for (index, band) in params.bands.iter().enumerate() {
            let live = band.gain_db != 0.0;
            if live {
                Self::design(&mut self.left[index], index, *band, voicing, sample_rate);
                Self::design(&mut self.right[index], index, *band, voicing, sample_rate);
                self.active[self.active_len] = index as u8;
                self.active_len += 1;
            } else {
                // Keep the stored samples: a band taken back to 0 dB stops
                // filtering from that sample and the tail of what it was
                // doing is already in the buffer, not in `z1`.
                self.left[index] = Biquad::identity();
                self.right[index] = Biquad::identity();
            }
        }
    }

    fn design(
        stage: &mut Biquad,
        index: usize,
        band: StripBand,
        voicing: &StripVoicing,
        sample_rate: u32,
    ) {
        let frequency_hz = voicing.eq.frequency(index, band.position);
        match band.kind {
            EqBandKind::Bell => stage.peak(
                frequency_hz,
                eq_effective_q(band.q, band.gain_db, voicing.q_profile()),
                band.gain_db,
                sample_rate,
            ),
            EqBandKind::LowShelf => {
                stage.shelf_slope(frequency_hz, band.gain_db, band.q, true, sample_rate)
            }
            EqBandKind::HighShelf => {
                stage.shelf_slope(frequency_hz, band.gain_db, band.q, false, sample_rate)
            }
        }
    }

    fn process(&mut self, bus: &mut StereoBus, frames: usize) {
        if self.active_len == 0 {
            return;
        }
        let active = &self.active[..self.active_len];
        for frame in 0..frames {
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

/// One track's strip, at one sample rate.
pub struct Strip {
    params: StripParams,
    sample_rate: u32,
    voicing: StripVoicing,
    drive_l: Preamp,
    drive_r: Preamp,
    drive: Smoothed,
    eq: StripEq,
    /// The knob's detector.
    detector: EnvelopeFollower,
    /// The programme-dependent one: slower to charge and slower to let go,
    /// blended into the first by the voicing's `programme`.
    programme: EnvelopeFollower,
    threshold: Smoothed,
    ratio: Smoothed,
    mix: Smoothed,
    makeup: Smoothed,
    /// Block extremes, for the face's gain-reduction meter and curve. Same
    /// reason `DynamicsBlock` holds extremes rather than the last sample:
    /// attack times reach 0.05 ms, so a strip can clamp and half-release
    /// inside one buffer.
    detector_peak: f32,
    reduction_db: f32,
}

impl Strip {
    pub fn new(params: StripParams, sample_rate: u32) -> Self {
        let voicing = strip_voicing(params.voicing);
        let smoothed = |initial| Smoothed::new(initial, PARAM_SMOOTH_S, sample_rate);
        let mut strip = Self {
            params,
            sample_rate,
            voicing,
            drive_l: Preamp::new(voicing.preamp, sample_rate),
            drive_r: Preamp::new(voicing.preamp, sample_rate),
            drive: smoothed(db_to_linear(params.drive_db)),
            eq: StripEq::new(),
            detector: EnvelopeFollower::new(),
            programme: EnvelopeFollower::new(),
            threshold: smoothed(params.threshold_db),
            ratio: smoothed(params.ratio),
            mix: smoothed(params.mix),
            makeup: smoothed(db_to_linear(params.makeup_db)),
            detector_peak: 0.0,
            reduction_db: 0.0,
        };
        strip.eq.update(&params, &voicing, sample_rate);
        strip.set_detector_times();
        strip
    }

    pub fn params(&self) -> &StripParams {
        &self.params
    }

    /// Replace the parameter set wholesale (project load): jump straight to
    /// the new values, since there is nothing to click coming from a load.
    pub fn set_params(&mut self, params: StripParams) {
        self.params = params;
        self.voicing = strip_voicing(params.voicing);
        self.rebuild_drive();
        self.eq.update(&params, &self.voicing, self.sample_rate);
        self.set_detector_times();
        self.snap_drive();
        self.snap_comp();
    }

    /// Put a section's smoothed values *at* its knobs rather than gliding to
    /// them, for the two moments there is nothing to be continuous with: a
    /// document arriving, and a section being switched in.
    ///
    /// A smoother only advances while its own section runs, so a knob turned
    /// while the section was out leaves the smoother holding a value from
    /// whenever it last ran. Gliding from that is not click-free -- the
    /// discontinuity is the section starting, not the gain moving -- it is
    /// five milliseconds of the *old* setting at the front of the first
    /// block back, which is the same artefact `apply_param` clears the
    /// filter bank and the detector to avoid.
    fn snap_drive(&mut self) {
        self.drive.reset_to(db_to_linear(self.params.drive_db));
    }

    fn snap_comp(&mut self) {
        self.threshold.reset_to(self.params.threshold_db);
        self.ratio.reset_to(self.params.ratio);
        self.mix.reset_to(self.params.mix);
        self.makeup.reset_to(db_to_linear(self.params.makeup_db));
    }

    /// Move one parameter, and report whether it was one this strip has.
    ///
    /// The `in` switches clear their own section's state on the way in, and
    /// put its smoothed values at its knobs. That is deliberate rather than
    /// tidy: a filter bank and a detector last fed audio before the section
    /// was switched out would otherwise put that audio into the first block
    /// after it comes back, and a smoother that has not advanced since then
    /// would spend that block gliding up from a setting the user has already
    /// changed. Either way "out means out" would have an artefact attached
    /// to it.
    pub fn apply_param(&mut self, id: u32, value: f32) -> bool {
        if !self.params.set(id, value) {
            return false;
        }
        // Read back rather than trusting `value`: `StripParams::set` clamps,
        // so this is the only number the model and the audio can agree on.
        let stored = self.params.get(id).unwrap_or(value);
        if strip_band_of(id).is_some() {
            // Every field a band has is a coefficient, so there is no field
            // to filter for: the bank is redesigned whichever one moved.
            self.eq
                .update(&self.params, &self.voicing, self.sample_rate);
            return true;
        }
        match id {
            STRIP_VOICING => {
                self.voicing = strip_voicing(self.params.voicing);
                self.rebuild_drive();
                // The Q law and the ratio bend are the voicing's other two
                // halves, so the bank is redesigned and the detector
                // retimed with it.
                self.eq
                    .update(&self.params, &self.voicing, self.sample_rate);
                self.set_detector_times();
            }
            STRIP_PRE_IN => {
                if self.params.pre_in {
                    self.rebuild_drive();
                    self.snap_drive();
                }
            }
            STRIP_EQ_IN => {
                if self.params.eq_in {
                    self.eq.reset();
                }
            }
            STRIP_COMP_IN => {
                if self.params.comp_in {
                    self.detector.reset();
                    self.programme.reset();
                    self.snap_comp();
                    self.reduction_db = 0.0;
                    self.detector_peak = 0.0;
                }
            }
            STRIP_DRIVE_DB => self.drive.set_target(db_to_linear(stored)),
            STRIP_COMP_THRESHOLD_DB => self.threshold.set_target(stored),
            STRIP_COMP_RATIO => self.ratio.set_target(stored),
            STRIP_COMP_MIX => self.mix.set_target(stored),
            STRIP_COMP_MAKEUP_DB => self.makeup.set_target(db_to_linear(stored)),
            STRIP_COMP_ATTACK_MS | STRIP_COMP_RELEASE_MS => self.set_detector_times(),
            _ => {}
        }
        true
    }

    /// Swap the input stage for a different voicing.
    ///
    /// Rebuilds rather than retunes, for the reason `PreampEffect` gives: a
    /// voicing is a different set of filters, and running the old one's
    /// state through the new one's curve is neither voicing.
    fn rebuild_drive(&mut self) {
        self.drive_l = Preamp::new(self.voicing.preamp, self.sample_rate);
        self.drive_r = Preamp::new(self.voicing.preamp, self.sample_rate);
    }

    fn set_detector_times(&mut self) {
        let (attack, release) = (self.params.attack_ms, self.params.release_ms);
        // The knob's detector, then the programme stage, which is timed off
        // the release alone -- see the constants.
        self.detector.set_times(attack, release, self.sample_rate);
        self.programme.set_times(
            release * PROGRAMME_CHARGE_SCALE,
            release * PROGRAMME_RELEASE_SCALE,
            self.sample_rate,
        );
    }

    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        if sample_rate == self.sample_rate {
            return;
        }
        self.sample_rate = sample_rate;
        self.rebuild_drive();
        self.eq.update(&self.params, &self.voicing, sample_rate);
        self.set_detector_times();
        for smoothed in [
            &mut self.drive,
            &mut self.threshold,
            &mut self.ratio,
            &mut self.mix,
            &mut self.makeup,
        ] {
            smoothed.set_time(PARAM_SMOOTH_S, sample_rate);
        }
    }

    pub fn reset(&mut self) {
        self.drive_l.reset();
        self.drive_r.reset();
        self.eq.reset();
        self.detector.reset();
        self.programme.reset();
        self.reduction_db = 0.0;
        self.detector_peak = 0.0;
    }

    /// What the compressor did in the block just rendered, or `None` while
    /// the section is out — where a meter should read nothing rather than
    /// hold whatever it last saw.
    pub fn dynamics_frame(&self) -> Option<DynamicsFrame> {
        self.params.comp_in.then_some(DynamicsFrame {
            detector_db: lin_to_db(self.detector_peak),
            reduction_db: self.reduction_db,
        })
    }

    /// Whether every section that is in has settled.
    ///
    /// A section that is *out* contributes nothing to the answer, because it
    /// is not running and its state is cleared on the way back in. What has
    /// to settle is the same thing `CompressorEffect::is_at_rest` waits for:
    /// a detector frozen mid-release wakes up holding reduction the music
    /// stopped asking for, and a filter frozen with a sample in it wakes up
    /// and emits it.
    pub fn is_at_rest(&self) -> bool {
        if self.params.pre_in && !(self.drive_l.is_at_rest() && self.drive_r.is_at_rest()) {
            return false;
        }
        if self.params.eq_in && !self.eq.is_at_rest() {
            return false;
        }
        if self.params.pre_in && !self.drive.is_settled() {
            return false;
        }
        if self.params.comp_in
            && !(self.detector.is_at_rest()
                && self.programme.is_at_rest()
                && self.threshold.is_settled()
                && self.ratio.is_settled()
                && self.mix.is_settled()
                && self.makeup.is_settled())
        {
            return false;
        }
        true
    }

    /// Run the sections that are in, in the order the mockup draws them.
    ///
    /// Three passes over the block rather than one fused loop, because each
    /// one is skipped independently and a fused loop would have to ask all
    /// three questions per sample to save two reads of a buffer that is
    /// already in cache.
    pub fn process_block(&mut self, bus: &mut StereoBus, frames: usize) {
        let frames = frames.min(bus.capacity());
        if self.params.pre_in {
            self.process_drive(bus, frames);
        }
        if self.params.eq_in {
            self.eq.process(bus, frames);
        }
        if self.params.comp_in {
            self.process_comp(bus, frames);
        } else {
            self.detector_peak = 0.0;
            self.reduction_db = 0.0;
        }
    }

    fn process_drive(&mut self, bus: &mut StereoBus, frames: usize) {
        for frame in 0..frames {
            let drive = self.drive.advance();
            bus.l[frame] = self.drive_l.process(bus.l[frame], drive);
            bus.r[frame] = self.drive_r.process(bus.r[frame], drive);
        }
    }

    fn process_comp(&mut self, bus: &mut StereoBus, frames: usize) {
        self.detector_peak = 0.0;
        self.reduction_db = 0.0;
        let knee_db = self.params.knee_db;
        let bend = self.voicing.ratio_bend;
        let programme = self.voicing.programme;
        for frame in 0..frames {
            let threshold_db = self.threshold.advance();
            let ratio = self.ratio.advance();
            let mix = self.mix.advance();
            let makeup = self.makeup.advance();
            // `mix` at 0 gives back the input exactly, which is what makes
            // this a parallel balance rather than the device host's blend.
            let dry_l = bus.l[frame];
            let dry_r = bus.r[frame];
            let wet_l = dry_l;
            let wet_r = dry_r;
            // Linked detection on the louder channel, like every dynamics
            // effect in this crate: detecting per channel lets a loud left
            // duck only itself and walks the image around.
            let level = wet_l.abs().max(wet_r.abs());
            let fast = self.detector.process(level);
            let envelope = if programme > 0.0 {
                let slow = self.programme.process(level);
                // Only ever holds the envelope *up*: a stage that could pull
                // it down would be a second attack, and the knob's is the
                // attack.
                fast + programme * (slow - fast).max(0.0)
            } else {
                fast
            };
            let envelope_db = lin_to_db(envelope);
            let effective_ratio = bent_ratio(ratio, envelope_db - threshold_db, bend);
            let reduction_db =
                compressor_gain_db(envelope_db, threshold_db, effective_ratio, knee_db);
            self.detector_peak = self.detector_peak.max(envelope);
            self.reduction_db = self.reduction_db.min(reduction_db);
            let gain = db_to_lin(reduction_db) * makeup;
            bus.l[frame] = dry_l * (1.0 - mix) + wet_l * gain * mix;
            bus.r[frame] = dry_r * (1.0 - mix) + wet_r * gain * mix;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::strip::{
        strip_band_param, STRIP_BAND_FREQ, STRIP_BAND_GAIN, STRIP_BAND_KIND,
        STRIP_BAND_POSITIONS, STRIP_BAND_Q, STRIP_COMP_KNEE_DB, STRIP_EQ_BANDS,
    };
    use mooloop_core::PreampVoicing;

    const SAMPLE_RATE: u32 = 48_000;

    fn noise(frames: usize) -> StereoBus {
        let mut bus = StereoBus::with_capacity(frames);
        let mut x = 0x9E37_79B9u32;
        for index in 0..frames {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let sample = (x >> 8) as f32 / (1 << 23) as f32 - 1.0;
            bus.l[index] = sample * 0.25;
            bus.r[index] = sample * 0.2;
        }
        bus
    }

    fn sine(frames: usize, hz: f32, amplitude: f32) -> StereoBus {
        let mut bus = StereoBus::with_capacity(frames);
        for index in 0..frames {
            let phase = std::f32::consts::TAU * hz * index as f32 / SAMPLE_RATE as f32;
            bus.l[index] = amplitude * phase.sin();
            bus.r[index] = amplitude * phase.sin();
        }
        bus
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    /// A default strip with `edit` applied.
    ///
    /// Not `let mut p = StripParams::default();` followed by assignments:
    /// clippy's `field_reassign_with_default` denies exactly that and CI
    /// runs with `-D warnings`, which `AGENTS.md` records as having been
    /// missed once already.
    fn tweak(edit: impl FnOnce(&mut StripParams)) -> StripParams {
        let mut params = StripParams::default();
        edit(&mut params);
        params
    }

    fn run(params: StripParams, mut bus: StereoBus) -> StereoBus {
        let frames = bus.capacity();
        let mut strip = Strip::new(params, SAMPLE_RATE);
        strip.process_block(&mut bus, frames);
        bus
    }

    /// **The claim that entitles the strip to exist on every track.** A
    /// default strip is out, and out is not "flat": the samples are not
    /// touched at all. Bit-identical, not close.
    #[test]
    fn a_strip_that_is_out_does_not_touch_a_sample() {
        let frames = 4_096;
        let source = noise(frames);
        let out = run(StripParams::default(), noise(frames));
        assert_eq!(out.l[..frames], source.l[..frames]);
        assert_eq!(out.r[..frames], source.r[..frames]);
    }

    /// And it stays out with a voicing selected and every section's values
    /// set to something violent -- because what decides is the switches.
    #[test]
    fn a_loaded_strip_that_is_out_still_does_not_touch_a_sample() {
        let frames = 2_048;
        let params = tweak(|params| {
            params.voicing = PreampVoicing::Iron;
            params.drive_db = 18.0;
            params.bands[0].gain_db = 15.0;
            params.threshold_db = -48.0;
            params.ratio = 20.0;
            params.makeup_db = 12.0;
        });
        let source = noise(frames);
        let out = run(params, noise(frames));
        assert_eq!(out.l[..frames], source.l[..frames]);
        assert_eq!(out.r[..frames], source.r[..frames]);
    }

    /// `Moo` at unity with every section *in* is still the identity, to the
    /// smoothers' tolerance. This is the null case the voicing is named for:
    /// a transparent profile, a literal ratio line, no programme dependence,
    /// and a flat EQ.
    #[test]
    fn moo_with_every_section_in_and_nothing_set_is_transparent() {
        let frames = 4_096;
        let params = tweak(|params| {
            params.pre_in = true;
            params.eq_in = true;
            params.comp_in = true;
            params.threshold_db = 0.0;
            params.knee_db = 0.0;
        });
        let source = noise(frames);
        let out = run(params, noise(frames));
        for index in 0..frames {
            assert!(
                (out.l[index] - source.l[index]).abs() < 1e-6,
                "frame {index}: {} vs {}",
                out.l[index],
                source.l[index]
            );
        }
        assert!(MOO_STRIP.is_transparent());
        for voicing in [GRIP_STRIP, PUNCH_STRIP, IRON_STRIP] {
            assert!(!voicing.is_transparent());
        }
    }

    /// A boosted band raises the frequency its *position* selects and
    /// leaves 100 Hz where it was. Acceptance case 2, and the shape of the
    /// whole EQ claim -- asked of the position rather than of a frequency,
    /// since 2026-09-11 there is no frequency to set.
    #[test]
    fn a_boosted_band_lifts_its_own_frequency_and_not_its_neighbour() {
        let frames = SAMPLE_RATE as usize / 4;
        let params = tweak(|params| {
            params.eq_in = true;
            params.bands[1].position = 1;
            params.bands[1].gain_db = 12.0;
            params.bands[1].q = 2.0;
        });
        // Whatever `Moo` puts there, rather than a number spelled here: the
        // table is the DSP's and this test is about the bank.
        let boosted_hz = MOO_STRIP.eq.frequency(1, 1);

        let lifted = run(params, sine(frames, boosted_hz, 0.2));
        let reference = sine(frames, boosted_hz, 0.2);
        let settled = frames / 2..frames;
        let ratio = rms(&lifted.l[settled.clone()]) / rms(&reference.l[settled.clone()]);
        assert!(ratio > 2.5, "a 12 dB boost should be about 4x: {ratio}");

        let away = run(params, sine(frames, 100.0, 0.2));
        let away_reference = sine(frames, 100.0, 0.2);
        let away_ratio = rms(&away.l[settled.clone()]) / rms(&away_reference.l[settled]);
        assert!(
            (away_ratio - 1.0).abs() < 0.1,
            "100 Hz should be untouched: {away_ratio}"
        );
    }

    /// A band at 0 dB is not run at all, so switching a neighbouring band on
    /// beside it cannot change what it does. The exactness the stage skip
    /// rests on, stated the way `EqEffect`'s own test states it.
    ///
    /// The bit-identity is asserted *and* the skip is, because on its own
    /// the identity does not distinguish them: a cookbook peak or shelf at
    /// 0 dB has `b == a` term for term, so its normalized coefficients come
    /// out exactly `identity`'s and running the stage would have produced
    /// the same samples. The claim the code makes is that it is not run, and
    /// `active_len` is the only place that is visible.
    #[test]
    fn a_flat_band_is_bit_identical_to_not_having_it() {
        let frames = 2_048;
        let one = tweak(|params| {
            params.eq_in = true;
            params.bands[2].gain_db = 9.0;
        });
        // The descriptor's own ceiling rather than a number written here: a
        // fourth copy of the shelf bands' Q maximum is the duplication
        // `AGENTS.md` opens on, and holding a test to a range means reading
        // the range.
        let q_ceiling = StripParams::descriptor(strip_band_param(0, STRIP_BAND_Q))
            .expect("band 0 has a Q descriptor")
            .max;
        let mut padded = one;
        for index in [0, 1, 3] {
            padded.bands[index].gain_db = 0.0;
            assert!(
                padded.set(strip_band_param(index, STRIP_BAND_Q), q_ceiling),
                "band {index} refused its own ceiling"
            );
        }
        let plain = run(one, noise(frames));
        let with_flat = run(padded, noise(frames));
        assert_eq!(plain.l[..frames], with_flat.l[..frames]);
        assert!(plain.l[..frames].iter().any(|s| s.abs() > 0.01));

        let strip = Strip::new(padded, SAMPLE_RATE);
        assert_eq!(
            strip.eq.active_len, 1,
            "three bands at 0 dB were still in the bank's run"
        );
    }

    /// A shelf switched to a bell keeps its frequency and changes its shape,
    /// which is acceptance case 3. Measured a long way below the corner,
    /// where a high shelf is still lifting and a bell has let go.
    #[test]
    fn a_shelf_switched_to_a_bell_changes_shape_and_keeps_its_frequency() {
        let frames = SAMPLE_RATE as usize / 4;
        let shelf = tweak(|params| {
            params.eq_in = true;
            params.bands[0].position = 0;
            params.bands[0].gain_db = 12.0;
        });
        let mut bell = shelf;
        bell.set(strip_band_param(0, STRIP_BAND_KIND), 0.0);
        assert_eq!(bell.bands[0].position, shelf.bands[0].position);

        let settled = frames / 2..frames;
        let reference = rms(&sine(frames, 12_000.0, 0.2).l[settled.clone()]);
        let shelf_high = rms(&run(shelf, sine(frames, 12_000.0, 0.2)).l[settled.clone()]);
        let bell_high = rms(&run(bell, sine(frames, 12_000.0, 0.2)).l[settled]);
        assert!(
            shelf_high > bell_high * 1.5,
            "a shelf holds 12 kHz up where a bell at 4 kHz does not: {shelf_high} vs {bell_high} (flat {reference})"
        );
    }

    /// The compressor reduces when the signal is over the threshold, and the
    /// makeup puts the level back without moving the detector. Acceptance
    /// case 4's first and third clauses.
    #[test]
    fn the_compressor_reduces_and_makeup_restores() {
        let frames = SAMPLE_RATE as usize / 4;
        let params = tweak(|params| {
            params.comp_in = true;
            params.threshold_db = -30.0;
            params.ratio = 8.0;
            params.attack_ms = 1.0;
        });

        let settled = frames / 2..frames;
        let reference = rms(&sine(frames, 200.0, 0.25).l[settled.clone()]);
        let squashed = rms(&run(params, sine(frames, 200.0, 0.25)).l[settled.clone()]);
        assert!(
            squashed < reference * 0.6,
            "8:1 over a -30 dB threshold should be obvious: {squashed} vs {reference}"
        );

        let mut made_up = params;
        made_up.makeup_db = 12.0;
        let restored = rms(&run(made_up, sine(frames, 200.0, 0.25)).l[settled]);
        assert!(
            restored > squashed * 3.0,
            "12 dB of makeup should be about 4x: {restored} vs {squashed}"
        );
    }

    /// `mix` at 0 is the dry signal **exactly**, whatever else the section is
    /// set to -- which is what makes it a parallel balance rather than the
    /// device host's blend. Acceptance case 4's second clause.
    #[test]
    fn a_mix_of_zero_is_the_dry_signal_exactly() {
        let frames = 2_048;
        let params = tweak(|params| {
            params.comp_in = true;
            params.mix = 0.0;
            params.threshold_db = -50.0;
            params.ratio = 20.0;
            params.makeup_db = 18.0;
        });
        let source = noise(frames);
        let out = run(params, noise(frames));
        for index in 0..frames {
            assert!(
                (out.l[index] - source.l[index]).abs() < 1e-6,
                "frame {index} moved: {} vs {}",
                out.l[index],
                source.l[index]
            );
        }
    }

    /// Acceptance case 6: a voicing with programme dependence lets go more
    /// slowly after a long loud passage than after a short one, at the same
    /// knob setting. The voicing that has none must recover in the *same*
    /// number of blocks either way, which is the control that says this
    /// measures the mechanism rather than the measurement.
    ///
    /// Counted in blocks to recovery rather than measured as a level,
    /// because the quantity programme dependence changes is a *time*. The
    /// first version of this test compared the RMS of a quiet tone over a
    /// fixed tail and separated the two cases by three percent, which was
    /// the mechanism working and the measurement looking at the wrong thing.
    #[test]
    fn programme_dependence_remembers_how_long_the_loud_part_lasted() {
        let long_frames = SAMPLE_RATE as usize / 2;
        let short_frames = SAMPLE_RATE as usize / 100;
        let block = 64;

        let blocks_to_recover = |voicing: PreampVoicing, loud_frames: usize| -> usize {
            let params = tweak(|params| {
                params.comp_in = true;
                params.voicing = voicing;
                params.threshold_db = -30.0;
                params.ratio = 8.0;
                params.attack_ms = 1.0;
                params.release_ms = 100.0;
            });
            let mut strip = Strip::new(params, SAMPLE_RATE);
            let mut loud = sine(loud_frames, 200.0, 0.5);
            strip.process_block(&mut loud, loud_frames);
            assert!(
                strip.dynamics_frame().unwrap().reduction_db < -3.0,
                "{voicing:?} was not compressing to begin with"
            );
            // Silence, so what is being timed is the release and nothing
            // else. Capped well past the slowest voicing's own stage.
            for count in 1..4_000 {
                let mut quiet = StereoBus::with_capacity(block);
                strip.process_block(&mut quiet, block);
                if strip.dynamics_frame().unwrap().reduction_db > -0.5 {
                    return count;
                }
            }
            unreachable!("{voicing:?} never recovered");
        };

        for voicing in [PreampVoicing::Punch, PreampVoicing::Iron] {
            let after_long = blocks_to_recover(voicing, long_frames);
            let after_short = blocks_to_recover(voicing, short_frames);
            assert!(
                after_long > after_short * 2,
                "{voicing:?} should hold on far longer after a long passage: \
                 {after_long} blocks against {after_short}"
            );
        }
        let after_long = blocks_to_recover(PreampVoicing::Moo, long_frames);
        let after_short = blocks_to_recover(PreampVoicing::Moo, short_frames);
        // Within a few blocks rather than equal: the two passages end on
        // different phases of the tone, so the detector starts its release
        // from a slightly different place. That is 1%, against the factor of
        // two above.
        assert!(
            (after_long as i32 - after_short as i32).abs() < after_short as i32 / 20,
            "Moo has no programme dependence, so its release cannot depend on \
             history: {after_long} blocks against {after_short}"
        );
    }

    /// The two bends do what their names say, at the same knob setting: the
    /// one that steepens reduces more far above the knee and the one that
    /// eases reduces less, and both agree with the literal line at the
    /// threshold itself.
    #[test]
    fn the_ratio_bend_leans_the_right_way_and_meets_the_line_at_the_knee() {
        let ratio = 4.0;
        assert!((bent_ratio(ratio, 0.0, 0.6) - ratio).abs() < 1e-6);
        assert!((bent_ratio(ratio, 0.0, -0.5) - ratio).abs() < 1e-6);
        assert!((bent_ratio(ratio, 30.0, 0.0) - ratio).abs() < 1e-6);
        assert!(bent_ratio(ratio, 24.0, 0.6) > ratio);
        assert!(bent_ratio(ratio, 24.0, -0.5) < ratio);
        assert!(bent_ratio(ratio, 24.0, -0.5) > 1.0, "never below unity");
    }

    /// The curve the COMP page draws is the transfer the section is
    /// running, which means the knob that moves the line is in it: `mix` at
    /// 0 draws the straight line.
    ///
    /// Written because the first version drew neither it nor the input trim,
    /// so a section set to pass the signal through exactly was drawn
    /// clamping it, with the knob that said so directly underneath. The trim
    /// went with the redrawn face on 2026-09-11, which is why this now has
    /// one half to assert instead of two.
    #[test]
    fn the_drawn_curve_is_the_transfer_the_section_is_running() {
        let base = tweak(|params| {
            params.comp_in = true;
            params.threshold_db = -24.0;
            params.ratio = 8.0;
            params.knee_db = 0.0;
        });
        let floor = -60.0;
        let samples = 121;
        let at = |curve: &[f32], input_db: f32| -> f32 {
            let travel = (input_db - floor) / -floor;
            curve[(travel * (samples - 1) as f32).round() as usize]
        };

        // Compressing: 12 dB over a -24 dB threshold at 8:1 leaves 1.5 over.
        let curve = static_curve_db(&base, floor, samples);
        assert!(
            (at(&curve, -12.0) - -22.5).abs() < 0.2,
            "8:1 should put -12 dB out at -22.5: {}",
            at(&curve, -12.0)
        );

        // `mix` at 0 is the straight line, to the sample.
        let dry = static_curve_db(&StripParams { mix: 0.0, ..base }, floor, samples);
        for (index, output_db) in dry.iter().enumerate() {
            let input_db = floor + -floor * index as f32 / (samples - 1) as f32;
            assert!(
                (output_db - input_db).abs() < 1e-4,
                "mix 0 should draw unity at {input_db}: {output_db}"
            );
        }

        // And the threshold still decides where the bend starts, which is
        // what the trim used to move.
        assert!(
            (at(&curve, -36.0) - -36.0).abs() < 0.1,
            "-36 dB is below the threshold and untouched: {}",
            at(&curve, -36.0)
        );
    }

    /// A gain computer whose output falls as its input rises inverts
    /// transients, so the bend has to keep the curve monotone at either
    /// sign. Walked across 90 dB in tenths.
    #[test]
    fn a_bent_ratio_stays_monotone() {
        for bend in [-0.5f32, 0.0, 0.6, 1.0] {
            let mut previous = f32::NEG_INFINITY;
            for step in 0..900 {
                let input_db = -80.0 + step as f32 * 0.1;
                let over = input_db + 18.0;
                let reduction =
                    compressor_gain_db(input_db, -18.0, bent_ratio(4.0, over, bend), 6.0);
                let output = input_db + reduction;
                assert!(
                    output >= previous - 1e-4,
                    "bend {bend} at {input_db}: {output} after {previous}"
                );
                assert!(reduction <= 1e-6, "bend {bend} boosted: {reduction}");
                previous = output;
            }
        }
    }

    /// Each voicing's drive section reaches the harmonic profile its table
    /// states, which is acceptance case 5. `harmonics.rs` asserts the
    /// profiles themselves; what this says is that the *strip* runs them --
    /// the coloured three make far more second or third harmonic than `Moo`,
    /// which makes none.
    #[test]
    fn every_voicings_drive_reaches_its_own_character() {
        let hz = 100.0;
        let frames = (SAMPLE_RATE as f32 / hz) as usize * 40;
        let measure = |voicing: PreampVoicing, harmonic: usize| -> f32 {
            let params = tweak(|params| {
                params.pre_in = true;
                params.voicing = voicing;
            });
            let out = run(params, sine(frames, hz, 0.251));
            let step = std::f32::consts::TAU * hz * harmonic as f32 / SAMPLE_RATE as f32;
            let (mut sin_sum, mut cos_sum) = (0.0f64, 0.0f64);
            for (index, sample) in out.l[..frames].iter().enumerate() {
                let phase = step * index as f32;
                sin_sum += (*sample * phase.sin()) as f64;
                cos_sum += (*sample * phase.cos()) as f64;
            }
            let scale = 2.0 / frames as f64;
            (((sin_sum * scale).powi(2) + (cos_sum * scale).powi(2)).sqrt()) as f32
        };
        let moo_second = measure(PreampVoicing::Moo, 2);
        assert!(moo_second < 1e-6, "Moo colours nothing: {moo_second}");
        assert!(
            measure(PreampVoicing::Iron, 2) > measure(PreampVoicing::Iron, 3),
            "Iron is even-dominant"
        );
        assert!(
            measure(PreampVoicing::Grip, 3) > measure(PreampVoicing::Grip, 2),
            "Grip is odd-dominant"
        );
        assert!(
            measure(PreampVoicing::Punch, 2) > measure(PreampVoicing::Moo, 2),
            "Punch colours"
        );
    }

    /// Switching a section in clears its own state, so what it does in its
    /// first block cannot depend on audio from before it was switched out.
    /// The one artefact "free while it is out" could plausibly produce.
    #[test]
    fn switching_a_section_in_starts_it_empty() {
        let frames = 512;
        let params = tweak(|params| {
            params.eq_in = true;
            params.bands[0].gain_db = 15.0;
        });
        let mut strip = Strip::new(params, SAMPLE_RATE);
        let mut loud = sine(frames, 6_000.0, 0.9);
        strip.process_block(&mut loud, frames);

        strip.apply_param(STRIP_EQ_IN, 0.0);
        let mut silence = StereoBus::with_capacity(frames);
        strip.process_block(&mut silence, frames);
        strip.apply_param(STRIP_EQ_IN, 1.0);
        let mut quiet = StereoBus::with_capacity(frames);
        strip.process_block(&mut quiet, frames);
        let leaked = quiet.l[..frames].iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(leaked < 1e-9, "the bank came back holding audio: {leaked}");
    }

    /// And it starts it at the knobs it is *currently* set to, not at the
    /// ones it had when it was switched out.
    ///
    /// A section's smoothers only advance while that section runs, so every
    /// knob turned while it was out is a value the smoother has never seen.
    /// Switching in and gliding up from the stale one would put five
    /// milliseconds of the old setting at the front of the first block back.
    /// Measured against a strip that was *born* with these values, which is
    /// the same audio by definition.
    #[test]
    fn a_section_switched_in_starts_at_the_knobs_it_is_set_to() {
        let frames = 256;
        let settings = tweak(|params| {
            params.comp_in = true;
            // Under the threshold with no knee, so the gain computer does
            // nothing and what is left is the makeup -- the smoothed value
            // a stale smoother would get wrong.
            params.threshold_db = 0.0;
            params.knee_db = 0.0;
            params.makeup_db = 18.0;
        });

        let mut switched = Strip::new(StripParams::default(), SAMPLE_RATE);
        for id in [
            STRIP_COMP_THRESHOLD_DB,
            STRIP_COMP_KNEE_DB,
            STRIP_COMP_MAKEUP_DB,
        ] {
            assert!(switched.apply_param(id, settings.get(id).unwrap()));
        }
        // A block while the section is still out, so the smoothers cannot
        // have crept toward the new values on their own.
        let mut ignored = sine(frames, 200.0, 0.2);
        switched.process_block(&mut ignored, frames);
        assert!(switched.apply_param(STRIP_COMP_IN, 1.0));

        let mut turned = sine(frames, 200.0, 0.2);
        switched.process_block(&mut turned, frames);
        let born = run(settings, sine(frames, 200.0, 0.2));
        for index in 0..frames {
            assert!(
                (turned.l[index] - born.l[index]).abs() < 1e-6,
                "frame {index}: switched in gives {}, where a strip born with \
                 the same settings gives {}",
                turned.l[index],
                born.l[index]
            );
        }
    }

    /// A strip that is out is at rest whatever it has been fed, and a strip
    /// with a section in reports at rest only once that section has
    /// settled -- which is what `BusStrip::is_resting` asks it.
    #[test]
    fn rest_follows_the_sections_that_are_in() {
        let frames = 256;
        let params = tweak(|params| {
            params.comp_in = true;
            params.threshold_db = -40.0;
            // Short, because the follower has to decay to `REST_EPSILON` before
            // it snaps -- about twenty time constants -- and the test pays for
            // every one of them in blocks.
            params.release_ms = 50.0;
        });
        let mut strip = Strip::new(params, SAMPLE_RATE);
        assert!(strip.is_at_rest(), "an untouched strip is at rest");
        let mut loud = sine(frames, 200.0, 0.5);
        strip.process_block(&mut loud, frames);
        assert!(!strip.is_at_rest(), "the detector is still holding");

        // Long enough for a 50 ms release to run all the way out.
        for _ in 0..500 {
            let mut silence = StereoBus::with_capacity(frames);
            strip.process_block(&mut silence, frames);
        }
        assert!(strip.is_at_rest(), "the detector never settled");

        strip.apply_param(STRIP_COMP_IN, 0.0);
        let mut loud = sine(frames, 200.0, 0.5);
        strip.process_block(&mut loud, frames);
        assert!(strip.is_at_rest(), "a section that is out cannot hold rest");
    }

    /// Every parameter the model has reaches the audio, and nothing else is
    /// accepted. A section whose knob turned and whose DSP did not is the
    /// failure this catches, and it is the one a face cannot see.
    #[test]
    fn every_parameter_is_accepted_and_strangers_are_not() {
        let mut strip = Strip::new(StripParams::default(), SAMPLE_RATE);
        for descriptor in StripParams::descriptors() {
            assert!(
                strip.apply_param(descriptor.id, descriptor.default),
                "{} ({}) was refused",
                descriptor.name,
                descriptor.id
            );
        }
        assert!(!strip.apply_param(0, 1.0));
        assert!(!strip.apply_param(9_999, 1.0));
    }

    /// Bands hold four fields and the bank is rebuilt from all of them, so a
    /// change to any one of the four has to be audible. Written because the
    /// obvious mistake is to rebuild on gain alone.
    #[test]
    fn every_band_field_reaches_the_bank() {
        let frames = 1_024;
        let base = tweak(|params| {
            params.eq_in = true;
            // Every band boosted, because a band at 0 dB is not run at all --
            // which is the point of the test beside this one, and would make
            // this one pass for the wrong reason on three bands out of four.
            for band in &mut params.bands {
                band.gain_db = 6.0;
            }
        });
        let reference = run(base, noise(frames));
        for band in 0..STRIP_EQ_BANDS {
            for (field, value) in [
                (STRIP_BAND_FREQ, 700.0),
                (STRIP_BAND_GAIN, -9.0),
                (STRIP_BAND_Q, 1.5),
                // The type switch is the one field whose *other* position
                // depends on the band: two bands arrive as shelves and two
                // as bells, and setting a band to what it already is
                // changes nothing.
                (
                    STRIP_BAND_KIND,
                    1.0 - base.get(strip_band_param(band, STRIP_BAND_KIND)).unwrap(),
                ),
            ] {
                let mut moved = base;
                assert!(moved.set(strip_band_param(band, field), value));
                let out = run(moved, noise(frames));
                let moved_any = (0..frames).any(|i| (out.l[i] - reference.l[i]).abs() > 1e-6);
                assert!(moved_any, "band {band} field {field} changed nothing");
            }
        }
    }

    /// The strip's cost while it is out is what pays for it existing on
    /// every track, so its *size* is worth knowing rather than assuming.
    /// Named as a measurement, not a limit: if it grows, the number to argue
    /// with is here.
    #[test]
    fn a_strip_is_a_few_hundred_bytes() {
        let size = std::mem::size_of::<Strip>();
        assert!(
            size < 1_024,
            "a strip is {size} bytes, against the 128 KB a track already costs"
        );
    }

    /// Every voicing offers a position for every step the model declares,
    /// the frequencies rise, and none of them is outside the audio band.
    ///
    /// The lengths are the load-bearing half: `mooloop_core::strip` says how
    /// many positions a band has and the knob is built from that, so a table
    /// one short would leave the top of a knob selecting a frequency that is
    /// not there -- clamped, silently, to the one below it.
    #[test]
    fn every_voicings_table_has_a_position_for_every_step() {
        for (name, voicing) in [
            ("Moo", MOO_STRIP),
            ("Grip", GRIP_STRIP),
            ("Punch", PUNCH_STRIP),
            ("Iron", IRON_STRIP),
        ] {
            for band in 0..STRIP_EQ_BANDS {
                let row = voicing.eq.bands[band];
                assert_eq!(
                    row.len(),
                    STRIP_BAND_POSITIONS[band] as usize,
                    "{name} band {band} has {} positions where the model declares {}",
                    row.len(),
                    STRIP_BAND_POSITIONS[band]
                );
                for pair in row.windows(2) {
                    assert!(
                        pair[1] > pair[0],
                        "{name} band {band} is not rising: {pair:?}"
                    );
                }
                assert!(
                    row[0] >= 20.0 && row[row.len() - 1] <= 20_000.0,
                    "{name} band {band} leaves the audio band: {row:?}"
                );
            }
        }
    }

    /// **The point of the whole change**: the same position is a different
    /// frequency under a different voicing, and the difference reaches the
    /// audio rather than only the readout.
    ///
    /// `Iron` sits lower than `Moo` on every band -- a 2.2 kHz mid where
    /// `Moo` puts 3 -- which is what makes swapping the voicing feel like
    /// swapping a channel module rather than turning a colour knob.
    #[test]
    fn a_position_is_a_different_frequency_under_a_different_voicing() {
        for band in 0..STRIP_EQ_BANDS {
            let position = DEFAULT_POSITIONS_FOR_TEST[band];
            let moo = MOO_STRIP.eq.frequency(band, position);
            let iron = IRON_STRIP.eq.frequency(band, position);
            assert!(
                iron < moo,
                "band {band} at position {position}: Iron {iron} should sit under Moo {moo}"
            );
        }

        // And it is audible: a tone at what `Moo`'s middle position selects
        // is lifted by a band there, and lifted less once the voicing moves
        // the band out from under it.
        let frames = SAMPLE_RATE as usize / 4;
        let settled = frames / 2..frames;
        let tone_hz = MOO_STRIP.eq.frequency(1, 3);
        let lift = |voicing: PreampVoicing| {
            let params = tweak(|params| {
                params.eq_in = true;
                params.voicing = voicing;
                params.bands[1].position = 3;
                params.bands[1].gain_db = 15.0;
                params.bands[1].q = 4.0;
            });
            rms(&run(params, sine(frames, tone_hz, 0.2)).l[settled.clone()])
        };
        let on_it = lift(PreampVoicing::Moo);
        let moved = lift(PreampVoicing::Iron);
        assert!(
            on_it > moved * 1.2,
            "the same position under Iron should not lift {tone_hz} Hz as much: \
             {on_it} against {moved}"
        );
    }

    /// `nearest` is `frequency`'s inverse on the positions themselves, which
    /// is what lets the response plot's drag snap instead of sliding.
    #[test]
    fn the_nearest_position_to_a_positions_own_frequency_is_itself() {
        for voicing in [MOO_STRIP, GRIP_STRIP, PUNCH_STRIP, IRON_STRIP] {
            for band in 0..STRIP_EQ_BANDS {
                for position in 0..STRIP_BAND_POSITIONS[band] {
                    let hz = voicing.eq.frequency(band, position);
                    assert_eq!(voicing.eq.nearest(band, hz), position, "band {band}");
                }
            }
        }
        // And a frequency between two positions goes to the nearer in *log*
        // distance, which is how the plot's axis is spaced and how a
        // frequency is heard. `Moo`'s top band steps 4 kHz to 6 kHz, whose
        // geometric mean is 4899 and whose arithmetic mean is 5000 -- so
        // 4950 is nearer 6 kHz by ratio and nearer 4 kHz by subtraction, and
        // this is the one probe that tells the two apart.
        assert_eq!(
            MOO_STRIP.eq.nearest(0, 4_950.0),
            1,
            "4950 Hz should snap up to 6 kHz; a linear distance would send it down to 4"
        );
    }

    /// The default position, spelled here because `mooloop-core` does not
    /// export it and this file should not guess at it.
    const DEFAULT_POSITIONS_FOR_TEST: [u8; STRIP_EQ_BANDS] = [2, 3, 3, 2];

    /// The four tables name their own input stage, and `preamp_voicing`
    /// names it again for the preamp *device*. Two statements of one
    /// association, so this is the check that they agree -- a strip running
    /// `Iron` and a `Preamp` set to `Iron` have to be the same stage, or the
    /// voicing means two things.
    #[test]
    fn a_voicings_input_stage_is_the_same_one_the_device_gets() {
        for choice in [
            PreampVoicing::Moo,
            PreampVoicing::Grip,
            PreampVoicing::Punch,
            PreampVoicing::Iron,
        ] {
            assert_eq!(
                strip_voicing(choice).preamp,
                crate::preamp::preamp_voicing(choice),
                "{choice:?}"
            );
        }
    }

    /// **The Q law is stated twice and only one of them is heard.**
    ///
    /// [`StripVoicing::proportional_q`] is what the bank is designed
    /// against; `StripParams::proportional_q` is the same rule spelled again
    /// in `mooloop-core`, because `strip_row` plots the running Q and
    /// `mooloop-core` cannot see this table. Two answers to "does `Grip`
    /// narrow a boosted band", in two crates, and the one that drifts is
    /// whichever is edited second -- at which point the response display
    /// draws a curve the audio is not running, which is the exact failure
    /// "a voicing selects laws, never values" was adopted to avoid.
    ///
    /// So they are held to each other here, the way
    /// `a_voicings_input_stage_is_the_same_one_the_device_gets` holds the
    /// preamp table to `preamp_voicing`. `mooloop-dsp` depends on
    /// `mooloop-core`, so this is the only side that can ask.
    #[test]
    fn a_voicings_q_law_is_the_one_the_display_plots() {
        for choice in [
            PreampVoicing::Moo,
            PreampVoicing::Grip,
            PreampVoicing::Punch,
            PreampVoicing::Iron,
        ] {
            let params = tweak(|params| {
                params.voicing = choice;
                params.bands[1] = mooloop_core::StripBand {
                    gain_db: 12.0,
                    ..params.bands[1]
                };
            });
            let voicing = strip_voicing(choice);
            assert_eq!(
                voicing.proportional_q,
                params.proportional_q(),
                "{choice:?} narrows a boosted band in one crate and not the other"
            );
            // And the number, not just the flag: the display plots
            // `effective_q` and the bank is designed at `eq_effective_q`
            // under this voicing's profile, so those have to be one value.
            assert_eq!(
                params.effective_q(1),
                eq_effective_q(
                    params.bands[1].q,
                    params.bands[1].gain_db,
                    voicing.q_profile()
                ),
                "{choice:?} plots a Q it is not running"
            );
        }
    }
}
