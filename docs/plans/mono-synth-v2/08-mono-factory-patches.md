# Mono factory patches

Implementation is not complete because every parameter moves. This step is the
listening pass, and it is where 04 and 05 get their final voicing.

## Why this is a step and not a checklist item

The Ladder's bass compensation, the Acid filter's resonance path, the Accent
depth constants, and the pre-drive makeup gain are all "tune by ear" in their
own steps. Doing that tuning against six concrete patches — rather than
against a sine and a spectrum plot — is what turns them from plausible to
right. Expect to make changes in 04, 05, and 06 during this step and to
record them there.

## The bank

Six patches, each proving something specific. Store them wherever the project
keeps factory content; if there is no mechanism yet, that is a prerequisite
and should be noted here rather than worked around by pasting TOML into a
test.

| Patch          | What it proves                                                    |
|----------------|-------------------------------------------------------------------|
| Round Bass     | Ladder weight and low-end stability under resonance               |
| Rubber Bass    | Filter envelope × resonance × pre-drive interaction               |
| Acid Line      | Acid model, Accent, legato slide, fast filter decay               |
| 101 Pluck      | Fast filter decay, keytrack, focused mono response                |
| Porta Lead     | Held-note stack, priority, both glide modes, legato env trigger   |
| Sequence Bleep | S&H LFO, PWM, and the LFO still being useful in a simple architecture |

Each patch has to be reachable *quickly* — a few knob moves from the default
saw, per the definition of done. If a patch needs fifteen precise settings to
work, the defaults or the ranges are wrong and that is the finding.

## Checks against the whole instrument

Run these once the bank exists, since they are cheapest to catch here:

- Every patch at maximum velocity stays within the peak bound. This is the
  practical version of the Accent gain-staging test.
- Transport stop during each patch releases cleanly and quickly.
- Automating cutoff, resonance, drive, and accent across their full range
  mid-note produces no clicks in any patch.
- Loading a pre-v2 project alongside the new bank works, and its patches still
  sound close to what they did.

## Done when

- All six patches exist, load, and reach their intended territory.
- Round Bass and Acid Line are unmistakably different instruments-worth of
  sound from the same oscillator settings.
- Any voicing constant changed during this pass is written back into 04, 05,
  or 06 with the reason.
- **The definition of done holds:** Mono and Poly loaded with the same saw
  lead to two clearly different workflows within a few knob moves.
