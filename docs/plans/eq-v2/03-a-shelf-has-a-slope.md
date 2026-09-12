# 03 — A shelf has a slope, and a bell narrows as it is pushed

Two laws the channel strip already runs and the effect EQ does not. Both are
written, tested, and in `mooloop-core` or `mooloop-dsp` already; this step is
calling them.

## A shelf's Q is its slope

`effects/eq.rs:88` calls `filter.shelf(frequency_hz, gain_db, low, sr)`. There is
no Q argument, so a band's Q knob does nothing at all while that band is a
shelf -- and the knob does not say so.

The strip calls `stage.shelf_slope(frequency_hz, band.gain_db, band.q, low,
sample_rate)`, and `strip.rs` says why: "a band's `q` knob is its slope when it
is a shelf and its Q when it is a bell -- which is what makes the same knob
honest in both positions." `Biquad::shelf_slope` exists and is tested.

## A bell narrows as it is pushed

`eq_effective_q(q, gain_db, profile)` is the proportional-Q law, in
`mooloop-core`, applied by the strip's bank and by every display that plots the
curve that is *running* rather than the curve that was dialled.

The effect EQ has `EqQProfile` per band and -- following step 01, per band by its
own id -- but does not apply the law. The measurements say the law is right: the
VEQ4's LMF bell is Q ~= 0.28 at +4 dB and Q ~= 0.61 at +8 dB.

## Watch for

The two laws interact through the display. A shelf's slope and a bell's
effective Q come from the same knob, so whatever the plot does has to branch on
kind exactly where the DSP does. Step 02's "read it by id from the table"
arrangement is what keeps that from becoming two copies of one branch.

`eq_effective_q` takes a profile, and on the strip the profile comes from the
*voicing*. The effect EQ has no voicing and the profile is per band, which is the
right answer here -- the strip's own plan records that a voicing selects laws and
never values, and a per-band choice of law is a value.

## Acceptance

- A shelf's Q knob visibly changes the curve and the audio, and the two agree.
- A bell boosted hard narrows, by the same law the strip uses, from the same
  function.
- A test compares the effect EQ's designed coefficients against the strip's for
  one identical band, so the two banks cannot diverge while claiming the same
  laws.
