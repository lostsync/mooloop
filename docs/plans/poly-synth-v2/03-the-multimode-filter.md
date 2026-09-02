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

## What landed

The voice around the network: two envelopes, the four filter modes, both
velocity depths, keytracking, and the feedback loop with drive inside it.
Parameter ids 42-54, exactly as reserved.

### The filter

One shared `Svf` stage, with a second one that only runs for LP24. This is a
**response menu, not a character menu** — which is the difference from the
ML-M1, whose three models are genuinely different filters and therefore need
per-model makeup gain. These four come off one linear stage, so the only
compensation they need is about slope:

- **`LP24_CORNER_SCALE = 1.553774`.** Two cascaded sections reach -3 dB at
  `sqrt(sqrt(2) - 1)` of one section's corner, so without this the Cutoff knob
  would mean two different frequencies depending on the slope and switching
  mode would be a tuning change. A test pins the two low-passes to the same
  passband share within 30%.
- **`LP24_RESONANCE_SHARE = 0.62`.** Resonance compounds through a cascade;
  splitting it keeps the knob meaning about the same amount of peak in both.

Measured at cutoff 0.5 with a saw at A2, as shares of total energy:

| Mode | 60-160 Hz | 1.3-2.6 kHz |
| --- | --- | --- |
| LP12 | 0.727 | 0.0008 |
| LP24 | 0.722 | 0.0004 |
| BP12 | 0.223 | 0.042 |
| HP12 | 0.006 | 0.272 |

The two low-passes keep the same bottom and LP24 halves the band above the
corner, which is the whole claim: same corner, steeper skirt.

### The loop

```text
mix + soft_ceiling(previous filter output * feedback) -> PreDrive -> filter
```

Drive is `PreDrive`, the same level-following stage the ML-M1 uses, and it
sits **before** the filter and **inside** the loop. That is what bounds the
loop's energy, and it is why feedback changes the tone rather than only the
gain. There is no limiter after the voice sum; `soft_ceiling` is exactly
transparent below its knee, so an ordinary patch never meets it and only a
runaway does. `VOICE_FEEDBACK_RANGE = 1.15`, provisional until step 07.

A one-pole DC blocker sits on the feedback tap only, not on the audible path:
a resonant filter under asymmetric drive walks off centre, and in a loop that
offset compounds, but the patch's own bias is not ours to remove.

### One thing the plan asked for that the first cut got wrong

"Reassigning a stolen slot cannot emit the previous note's feedback tail."
The loop state was cleared in `restart()`, which only runs for a *fresh*
slot — stealing a sounding voice deliberately keeps its oscillator phases,
because restarting them under a running envelope is a click, and it was
keeping the loop with them. `clear_loop()` is now separate and runs in both
places a slot changes hands, plus when a voice falls idle. The test that
caught it asserts every idle voice's tap is exactly zero.

### Velocity

Amp Velocity is a **crossfade, not a multiply**: at zero depth a soft note is
a full-level note rather than a silent one, so the control is a depth on
velocity rather than a switch that turns it off. Filter Velocity *adds* to the
envelope amount rather than scaling it, so a patch with no envelope depth can
still be played into the filter. A test pins that filter velocity never moves
the VCA by itself.

### The face

The device outgrew its row and the answer was structural rather than
cramming: `DeviceRackMetrics.face-height` is a global constant and the "3U" in
the header is a label, so there is no taller device to become.

The left column's five tabs are now the network grid's five rows, in the same
order and with the same names — OSC 1/2/3, SUB, NOISE. Pick a source there,
see everything it reaches in the grid. That absorbed the sub's octave/wave/
source and the noise's colour, which had been a leftover panel, and freed the
whole right side for one VOICE region holding the filter and the amplifier
with their two envelopes side by side, where the difference between them is
the point.
