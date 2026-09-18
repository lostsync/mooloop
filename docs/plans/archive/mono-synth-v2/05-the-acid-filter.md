# The acid filter

The 303/101-adjacent half. It shares step 04's dispatch and pre-drive; this
step is voicing, not plumbing.

## What this is for

Sequenced lines. Fast filter decay, resonance that sings rather than merely
peaks, and a cutoff that reacts hard to the envelope and to drive. The
reference behaviour is a 16-step pattern with the filter envelope doing the
musical work — which is only possible at all because of the separate filter
ADSR from step 02.

The spec deliberately does **not** specify a topology. Voice it by ear against
that use case. What is specified is behaviour.

## Do this

### 1. What it has to do

1. **Fast, vocal resonance.** Resonance should be a formant, not a bump. It
   should be usable at high settings without the patch becoming a sine, which
   is where a plain SVF ends up.
2. **Aggressive envelope interaction.** A 100 ms filter decay at moderate env
   amount should produce a pronounced sweep, not a subtle one. If Ladder and
   Acid at identical settings produce a similar sweep, Acid is not done.
3. **Strong response to pre-drive.** Distortion into the filter is the
   character. It should sound different from Ladder's saturation — brighter
   and more forward rather than heavier.
4. **Stable at extreme resonance.** Same bound as Ladder: finite and ≤ 1.0 at
   maximum resonance, maximum drive, swept cutoff.

### 2. Implementation

A ladder variant with different feedback voicing and a different
nonlinearity placement is the cheapest path — it reuses the step 04 state
array and keeps `MonoVoice` small. A 3-pole-plus-zero arrangement, or a
diode-ladder-flavoured feedback path, are the usual answers for this
character. Try the cheap route first and only add distinct state if the
listening test rejects it.

The distinguishing behaviour is more about the resonance path and where the
nonlinearity sits than about pole count. Spend the tuning time there.

No new user parameters. Acid is a value of `filter_model` (ID 28, added in
step 04) and reuses Cutoff, Resonance, Env Amount, Keytrack, and Drive.

### 3. Where this gets voiced

Acid without Accent is half the story — velocity driving filter intensity is
what makes a sequenced line breathe, and that is step 06. Expect to come back
here after 06 and again during 08. Note tuning decisions in this file as they
are made, so the next pass does not re-derive them.

## Done when

- Ladder and Acid are audibly distinct at identical oscillator, envelope,
  cutoff, resonance, and drive settings. Assert a measurable spectral
  difference, not just "not bit-identical".
- Acid's envelope sweep is measurably more pronounced than Ladder's at the
  same env amount.
- Maximum resonance × maximum drive stays bounded, per model.
- A 16th-note sequence at 130 BPM with a 100 ms filter decay produces a
  distinct per-note sweep rather than a smear. This one is a listening check;
  record the patch in step 08's bank.
- Switching Ladder ↔ Acid mid-note does not step the output.
