# Poly v1 mono mode — plan status

Not started.

This plan records a decision Adam made on 2026-08-30, during the ML-M1
restructure. It was captured as a consequence note inside
`docs/plans/mono-synth-v2/00-status.md` and had no plan of its own; this is
that plan.

## The decision

**The original poly synth is kept, and gains a mono/poly toggle and a legato
toggle. The original mono synth is deleted once its channels have somewhere to
migrate to, which is that toggled poly.**

The reasoning, in Adam's framing: the v1 poly is a simple three-oscillator
synth that sounds good, and losing it is not worth it. It should cover the
"sometimes that is all you need" case — including the mono case — so that the
v1 mono synth has a migration target and can go.

Three synths end up in the project, not two: the toggled v1 poly, the ML-M1,
and ML-P8.

## Why this is worth doing before it looks urgent

It is the only thing blocking deletion of `DeviceKind::MonoSynth`, and that
deletion is what lets `DeviceKind::MlM1` take the plain name. Until then:

- The device picker carries two mono-capable synths, shown as "Mono" and
  "ML-M1" (`crates/mooloop-ui/src/lib.rs:1798`).
- `POLY_DESCRIPTORS` keeps copying `MONO_DESCRIPTORS`, an inheritance that
  `docs/plans/mono-synth-v2/00-status.md` describes as surviving "by design
  until the later v1 migration".
- `MlM1` stays a transitional name in `DeviceKind`.

None of that is broken, but all of it is carrying cost, and the work to clear
it is small.

## What already exists

- **The held-note stack is already shared.** `crates/mooloop-dsp/src/heldnotes.rs`
  was deliberately built as its own module rather than inside the ML-M1,
  "because the poly synth needs the same thing for its mono mode". It offers
  `push`, `remove`, `winner(NotePriority)`, `clear`, `len`, `is_empty`.
- **The enums exist.** `NotePriority` (`Last`/`Low`/`High`), `EnvTrigger`
  (`Retrig`/…) and `GlideMode` (`Always`/`Legato`) are in
  `crates/mooloop-core/src/mlm1.rs`, with `from_index` converters.
- **Parameter id space is already reserved.** `crates/mooloop-core/src/generator.rs:184`
  reads: "17-19 are deliberately unused, so the v1 synths keep room to grow
  without reaching into the ML-M1 block below." Three free ids for at most
  three new parameters. This is the plan's single luckiest fact and it should
  be used rather than worked around.
- **`PolySynthParams` is `#[serde(default)]`** (`crates/mooloop-core/src/synth.rs:416`),
  so added fields load conservatively in old projects with no migration step.

## What is actually wrong today

`PolySynth::note_on` (`crates/mooloop-dsp/src/polysynth.rs:200`) at
`polyphony == 1` is a voice-stealing pool of one, not a monosynth:

- `voice.env.note_on()` is called unconditionally, so every overlapping note
  retriggers both envelopes. There is no legato.
- No held-note record exists, so releasing the newer of two held notes leaves
  the voice on that note instead of falling back to the one still down.
- Note priority is always "last", which is one of three musically standard
  answers.

This is the same list `docs/plans/mono-synth-v2/03-the-held-note-stack.md`
opens with, for the same reason, and the fix is the same module.

## Scope boundary

This plan makes the v1 poly a competent mono synth. It does **not** give it
ML-M1 identity. Specifically, do not port pre-filter drive, the Ladder or Acid
filter models, Accent, or the split filter envelope. If a v1 mono patch needs
those to survive migration, that is a finding about the migration, not a
licence to widen this plan — record it.

Keep the v1 poly's existing calibration. It is a gain reference as well as a
synth.

## Steps

1. `01-mono-and-legato-toggles.md` — the parameters, the DSP, and the face.

Migration of existing `MonoSynth` channels onto the toggled poly, and the
deletion of `DeviceKind::MonoSynth`, are deliberately **not** in this plan.
They are a separate branch that this one unblocks, and they should not be
attempted until the toggles have been played.

## Open question for Adam

**Does mono mode ship as its own toggle, or as `Voices = 1`?**

The parameter already exists and already clamps to 1. Reusing it means no new
id and no redundant control; against that, "Voices 1" and "Mono" are not quite
the same claim — a mono synth's note behaviour is a mode, and a user looking
for it will look for the word. Step 01 assumes a distinct toggle and explains
the cost; overrule it there if the simpler reading is preferred.
