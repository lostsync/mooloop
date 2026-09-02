# The kit

The listening, range-tuning, and identity pass. Nothing in this step adds a
control; if it turns out a sound needs one, that is a finding to record and
argue rather than a knob to add quietly here.

## What this step is for

DS-01's whole argument is that one architecture reaches every drum type. That
claim is untested until somebody sits down and tries to make the drums. This
step is where the ranges, curves, and defaults chosen in steps 02 through 07
get corrected by the sounds, and where the plan finds out whether it was right.

The ROADMAP already carries an open item against v1 — "audit drum synth time
ranges and parameter scaling; defaults and useful knob travel should make
sub-100 ms percussion easy without making the rest of the range feel wrong."
That audit happens here, on DS-01, and the roadmap item is closed against this
step rather than against v1.

## The factory patches

These are the end-state test from `01`, made concrete. Each must be reachable
from the default patch using DS-01's own controls, with no insert effect and
no channel modulator.

| Patch | The layer that carries it |
| --- | --- |
| Sub Kick | Tone, deep pitch envelope, no noise |
| Kit Kick | Tone plus a short noise click |
| DnB Kick | Tone plus Fold drive and a fast pitch envelope |
| Tight Snare | Tone body plus band-passed noise, short |
| Deep Snare | The same, longer, with body at low Ratio |
| Rimshot | Body at high Ratio, impulse excite, very short |
| Clap | Noise, four impulses, negative spread and level step |
| Tom Low / Mid / High | Body at Ratio 0, one patch tuned three ways |
| Closed Hat | Tone partials at 6, pulse morph, high-pass noise |
| Open Hat | The same with a long decay, sharing a choke group |
| Ride | Noise excite into a long, lightly damped body, gated |
| Cowbell | Two partials, no noise, medium body |
| Clave | Body, impulse excite, high Ratio, very short |
| Zap | Tone with a large negative pitch depth |
| Ghost | Tight Snare with velocity routed to decay and cutoff |

**Tom Low / Mid / High must be one patch at three tunings**, not three
authored sounds. If tuning a tom two octaves makes it stop sounding like the
same drum, Body Pitch tracking or the resonator's decay-to-Q derivation is
wrong, and that is a step 04 bug found here.

**Ghost is the acceptance case for the whole instrument.** A hat or snare
pattern where the quiet hits are audibly a *different* sound — shorter, duller,
and softer — rather than the same sound turned down. If that is not
straightforward to build from the matrix, step 07's source set is wrong.

## The default new-song kit

Per decision 2 in `00-status.md`, v1 `DrumSynth` stays and old projects are
untouched. Whether the default new-song kit moves to DS-01 is a separate
decision to put to Adam **after** this step's listening pass, not before: the
kit is the first thing anyone hears, and it should only move once DS-01 is
demonstrably better at it. Record the recommendation here with the evidence.

## Range tuning

Correct these from the sounds, not from the spec:

- Envelope decay curves. The log range in step 02 was chosen for coverage, not
  for knob feel. Sub-100 ms percussion must be easy to dial.
- Pitch Env Depth default and taper. +21 semitones was picked to approximate
  v1's kick; the useful range is probably not symmetric.
- Body Decay's mapping to Q, verified across the pitch range.
- Drive compensation per character, so the four characters are equally usable
  at the same Drive setting.
- The device output reference, confirmed against `docs/GAIN_STRUCTURE.md` with
  the full kit playing rather than with one hit.

## Acceptance

- Every patch above exists, is saved as a factory patch, and loads.
- A full pattern using at least six of them plays, saves, reloads, and renders
  offline sample-identically.
- The kit's overall level under `GAIN_STRUCTURE.md` is correct with everything
  playing, not just per hit.
- The ROADMAP's drum range-and-scaling item is closed, and `docs/CURRENT.md`'s
  statement that "the drum synth is not addressable" is replaced with what is
  true.
- Write the findings into `00-status.md`, including anything the plan could
  not have known before it was built.
