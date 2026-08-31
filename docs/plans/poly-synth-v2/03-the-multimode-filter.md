# Envelopes, multimode filter, and voice feedback

This step gives the oscillator network a complete subtractive voice and a
second nonlinear loop. It also folds in the independent filter-envelope work
that the previous plan left as a prerequisite.

## Two envelopes with separate jobs

Each physical voice owns:

- an amplitude ADSR, applied only to the VCA unless explicitly routed
  elsewhere on the MOD page;
- a filter ADSR, with a bipolar dedicated Filter Env Amount and availability
  as an internal modulation source.

Both envelopes are evaluated per sample and per voice. One envelope must never
be reduced to a channel value and then reapplied to every note in a chord.

Add native expression controls:

- **Amp Velocity:** 0-100%, crossfading from fixed unity response to full note
  velocity at the VCA;
- **Filter Velocity:** bipolar amount added to the filter-envelope depth;
- **Keytrack:** 0-200%, with 100% moving cutoff one octave per played octave.

These are normal instrument controls, saved with the patch and automatable.
They do not require channel modulation routes. Sustain is a level; any later
Drift control may vary envelope times but never sustain.

## Four filter modes

Use the existing topology-preserving SVF as the basis:

```rust
pub enum MlP8FilterMode { Lp12, Lp24, Bp12, Hp12 }
```

| Mode | Response | Implementation |
| --- | --- | --- |
| LP12 | Open two-pole low-pass | One SVF stage |
| LP24 | Four-pole low-pass | Two compensated cascaded stages |
| BP12 | Band-pass | First-stage BP output |
| HP12 | High-pass | First-stage HP output |

For LP24, tune resonance distribution and cutoff compensation so the Cutoff
knob refers to approximately the same corner in LP12 and LP24. Switching mode
mid-note must not clear filter state or step the output. Match once per
render range or use a prepared function path; do not branch on an unchanged
mode for every sample of every voice unnecessarily.

## The feedback loop

Add a bipolar **Voice Feedback** control. For each voice:

```text
source mix + bounded(previous filter output * feedback)
    -> drive/color
    -> multimode filter
    -> store for next sample
    -> VCA
```

The one-sample delay is explicit. Feedback is per voice, not a loop around the
eight-voice sum, so notes do not secretly modulate one another. The feedback
state resets when a slot is truly idle and is cleared before that slot is
reassigned; it is not cleared merely because a held note enters release.

Drive moves to the pre-filter position and sits inside the loop. This is a
design decision, not a listening-pass placeholder: the drive stage is what
bounds loop energy and makes feedback change tone rather than only gain. Keep
it level-compensated around the project reference and add a DC blocker if the
feedback/filter combination develops a bias.

Maximum positive and negative feedback should approach sustained or
self-oscillating behavior without NaNs, denormals, or unbounded output. Do not
hide a limiter after the voice sum. The explicit drive and loop bound are the
safety mechanism and part of the instrument's sound.

## Parameter IDs and UI

Reserve ML-P8 IDs 42-54 for filter mode, cutoff, resonance, Filter Env Amount,
drive, keytrack, filter ADSR, Amp Velocity, Filter Velocity, and Voice
Feedback. Reuse no provisional Poly-v2 ID: ML-P8 has a new parameter kind and
one reviewed descriptor table.

The AMP/FILTER page contains three visibly separate sections: Amplitude,
Filter, and Filter Envelope. Voice Feedback lives on ROUTE beside oscillator
self-feedback because it is heard as part of the network, even though its DSP
tap is after the filter.

Every continuous control in this step is a legal channel modulation
destination except envelope sustain where a later destination audit finds a
specific reason to exclude it. Filter mode is structural and defaults to
ineligible.

## Done when

- Amplitude and filter envelopes have independent times and levels on all
  eight voices.
- Amp Velocity at 0 produces equal VCA peaks across velocities; at 100 it
  follows note velocity. Filter Velocity is bipolar and never changes VCA
  gain by itself.
- Keytrack at 100% follows played pitch by one octave per octave.
- Each filter mode produces the expected response; LP12 and LP24 have
  comparable corner frequencies and clearly different slopes.
- Rapid per-voice envelope sweeps and channel-routed cutoff modulation remain
  finite at maximum resonance in all modes.
- Voice Feedback changes each mode materially, reaches unstable/industrial
  territory in its upper range, and remains numerically bounded at both
  polarities.
- Eight simultaneously held notes do not leak filter or feedback state into
  one another.
- Reassigning a stolen slot cannot emit the previous note's feedback tail.
- Mode, feedback, drive, velocity amount, and envelope-amount automation do
  not click.
