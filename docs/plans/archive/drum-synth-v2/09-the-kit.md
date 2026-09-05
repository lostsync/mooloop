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
| Ghost Snare | Tight Snare with velocity routed to decay and cutoff |

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

## What landed, and what still needs ears — 2026-09-04

**The bank exists and is shipped.** `mooloop_core::ds01_factory` holds
seventeen patches as data — the fifteen rows above, with the toms as three
tunings of one patch and Ghost Snare beside Tight Snare — and
`mooloop_project::seed_ds01_bank` writes them into
`presets/generators/ds01/` on first run, marker-guarded, on exactly the terms
the ML-M1 and effect banks already use.

Three things are worth recording about the shape it took:

- **They are generator presets, not channel presets.** The ML-M1 bank is
  channel-scoped because Sequence Bleep is nothing without a channel rack.
  DS-01's modulation is inside its voice, so a patch is a bare `Ds01Params`
  with nothing to re-scope — which is the simpler of the two forms and the one
  the preset plan says the source slot already covers.
- **The bank is the test fixture.** `one_architecture_reaches_a_kit` in
  `mooloop_dsp::ds01` used to hold its own thirteen patches; it now reads
  `ds01_factory::patches()`, so the patches that ship are the patches that are
  asserted. All seventeen sound, stay bounded, end, and are a different sound
  from every other, and between them they span more than eight times in length
  and four times in brightness. `one_tom_patch_tunes_across_a_range` and
  `a_ghost_hit_is_a_different_sound_not_a_quieter_one` now name bank patches
  rather than rebuilding them.
- **The Ride needed the gate, and the gate needed the test to release a
  note.** A gated patch rings for as long as it is written, so the acceptance
  loop's "it ends" assertion sends a note-off to any patch whose amplitude or
  noise envelope is gated. Excusing the Ride from that assertion instead would
  have left the one patch that uses the gate untested for termination.

**What has not happened is the listening.** Nobody has heard these. They are
patches reached by reasoning about the architecture and checked mechanically;
whether they sound *good*, and the range tuning that follows from that, is the
half of this step that needs Adam at the keyboard. So these stay open:

- Every item under "Range tuning" above. Each is a judgement against a sound,
  and a patch that turns out wrong is a finding about a range or a curve
  rather than a case to delete.
- The ROADMAP's drum range-and-scaling item, which this step is supposed to
  close. It stays open, and it stays pointed here.
- The recommendation on whether the default new-song kit moves to DS-01. The
  step says to make that call *after* the listening pass, and the listening
  pass has not happened.
- A full pattern using six of the patches, played, saved, reloaded and
  rendered offline. The mechanical half of that is already covered by
  `mooloop_engine::ds01_tests`, which asserts a DS-01 channel renders
  identically at 128 and 1024 frames; what is missing is doing it with the
  bank.

## Played, and closed — 2026-09-05

Adam played the device and the bank extensively on the night of 2026-09-04 and
closed the step: "ds-01 is good"; "they don't have to be flawless presets. for
now we just need something there, mostly to prove the system works"; "im happy
with what has been done so far."

**That restates the bar, and the restatement is the finding.** This step was
written as though the patches themselves were the deliverable, with range
tuning driven by taste as its second half. They are not: the deliverable is
the proof that one architecture reaches a kit from the controls, and seventeen
patches a musician can play and hear the range of deliver it. Authoring a bank
by taste is a later, dedicated push across every device, recorded under
"Deliberately not now" in `FOCUS.md`. A device step does not stay open waiting
for it.

Resolved by the pass:

- **Range tuning.** No corrections were raised against any range or curve. The
  ROADMAP's drum range-and-scaling item closes against this step, as it was
  always going to; reopen it against a concrete patch that a range fights.
- **The acceptance case.** The claim is the one the pass was for, and it held
  at the keyboard as well as in `one_architecture_reaches_a_kit`.

Carried out of the step, because it is a decision rather than work:

- **Whether the default new-song kit moves to DS-01.** The step said to make
  that call after the listening pass; the pass has happened, so the call is
  live. It is Adam's, nothing in the code waits on it, and v1 `DrumSynth`
  stays either way.
