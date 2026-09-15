# Poly v1 mono mode — plan status

## Step 01 landed 2026-09-15

The v1 poly has a mono mode: a held-note stack, a note priority, and an
envelope-trigger switch, on the three ids that were reserved for exactly this.
`DeviceKind::MonoSynth` is now deletable — that migration is the next branch
and is deliberately not this one.

### The step file named the wrong enum, and the behaviour it described is right

It asks for `SYNTH_PARAM_POLY_GLIDE_MODE` carrying `GlideMode`, and then
describes what the parameter does: *"Retrigger the envelopes only when the
stack was empty before the push... Under `Legato` with a note already down,
change pitch and leave both envelopes running."* That is `EnvTrigger`, whose
doc comment in `mlm1.rs` reads "Whether an overlapping note restarts the
envelopes" — and `GlideMode` is something else entirely and says so: *"Both
modes glide between overlapping notes and neither glides from silence. They
differ only over a still-sounding release tail."*

So id 18 is `SYNTH_PARAM_POLY_ENV_TRIGGER` and carries `EnvTrigger`. Shipping
the file's names would have put a control labelled for glide in charge of
envelopes, which is this codebase's recurring fault in its plainest form.

**Glide is therefore fixed rather than switched.** Overlapping notes glide;
a note landing on a release tail jumps. That is `GlideMode::Legato`, the
ML-M1's own default, and one rule instead of a fourth parameter — which is
the scope boundary doing its job: a second glide mode is ML-M1 identity
rather than v1 competence.

### What it cost, against what the step file expected

Less. `voice_limit()` was the only place that needed to learn about mono mode:
`apply_params_to_voices` retires the slots past it, `any_active` and
`select_voice` only look inside it, and `render_range` hands it to `voice_pan`,
which already answers centre at one. The step file's two separate instructions
— deactivate voices above zero, and centre the spread — are both that one
function.

### The open question is answered, in the direction the step file assumed

**Mono mode is its own toggle, not `Voices = 1`.** The deciding argument is
not the control count: a pool of one voice and a monosynth are different
instruments, and `Voices = 1` would have to mean one of them. It means the
pool today, in every project already saved. So the mode is a mode, `Voices`
greys out while it is on, and nothing that is already on disk changes meaning.

### Verified

Every line of the step file's Verify list, plus the one it did not ask for:

- **Old projects are bit-identical.** Two renders of a three-note gesture,
  one with the two mode fields at their *other* values, compared sample for
  sample — so what is asserted is not that the defaults are inert but that
  nothing outside `mono_mode` can reach the poly path at all. And the serde
  half: a manifest with the three keys cut out of it loads to exactly
  `PolySynthParams::default()`.
- Legato holds the envelope, Retrig restarts it, and a fallback restarts it
  in **neither** mode — in both directions, because releasing the newer note
  and releasing the older one fail differently.
- Three priorities pick three winners from one gesture.
- A transport stop clears the stack, and so does leaving mono mode.
- The gain calibration holds in mono mode:
  `the_poly_in_mono_mode_still_hits_the_reference_level`.

### Not done, and it is the acceptance test the next branch depends on

The step file's last clause — that a v1 mono patch can be reproduced closely
enough that migrating `MonoSynth` channels is mechanical — **has not been
played**. The parameter sets line up on paper (the v1 mono's thirty ids are
the poly's first thirty, by construction), but that is an argument and not a
listen. `FOCUS.md`'s rule stands: listening is a step. Do it before opening
the migration branch, and record the gap here if there is one.


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

1. ~~`01-mono-and-legato-toggles.md` — the parameters, the DSP, and the face.~~
   Landed 2026-09-15.

Migration of existing `MonoSynth` channels onto the toggled poly, and the
deletion of `DeviceKind::MonoSynth`, are deliberately **not** in this plan.
They are a separate branch that this one unblocks, and they should not be
attempted until the toggles have been played.

## ~~Open question for Adam~~ — answered by the build, 2026-09-15

**Does mono mode ship as its own toggle, or as `Voices = 1`?** Its own toggle.
See the status section at the top for why: `Voices = 1` already means a pool
of one voice in every saved project, and it cannot start meaning something
else.
