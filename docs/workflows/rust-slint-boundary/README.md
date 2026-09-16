# The Rust/Slint boundary

`mooloop-core` owns the numbers. `crates/mooloop-ui/ui/*.slint` draws them.
Nothing compiles the two together, so every value that appears on both sides is
a copy, and a copy that nothing reads back is free to drift.

This is the codebase's characteristic fault and `AGENTS.md` says so. What this
workflow adds is the part that document leaves as a habit: where the copies
hide, which of them are worth doing anything about, and how to tell a guard
that works from one that merely passes.

## Why these are invisible

A boundary fault has no failing symptom at the moment it is introduced. Both
sides are correct on the day they are written; the drift happens later, on an
ordinary edit, and the program remains *right* until the day it does not. So:

- **The tests are green throughout.** Nothing reads the copy.
- **Every rendered frame is self-consistent.** A snapshot cannot see it.
- **The code reads correctly.** Both spellings say the same number.

The 2026-09-12 audit recorded in `AGENTS.md` found six faults of one shape and
every one of the values was *still correct* when it was found. That is the
normal case. You are not usually looking for a wrong number; you are looking
for a number nothing would notice going wrong.

## The six shapes

Named from what has actually been found here, in rough order of how much a
drift would cost.

**1. A policy stated twice.** The worst, because the two copies are prose
decisions rather than numbers and diverge without either looking edited.
`RenderState::install_compensation` knew a bank whose routing does not sort has
no compensable sends; `Session::latency_plan` did not. Both were readable, both
were reasonable, and one was wrong.

**2. An id space mirrored.** The modulation shelf's `LfoParam`/`EnvParam`/
`StepParam`/`RandomParam`/`MathParam` globals are 42 parameter ids mirroring
`mooloop-core`'s `*_PARAM_*` constants, and those ids are the wire contract
every edit travels under. Renumber one side and it compiles, passes everything
else, and wires every control on that module to the wrong parameter. Their own
comment said they "are never renumbered" -- a policy, not a guard.

**3. A range or default written into a face.** The most common and the least
dangerous individually. A knob states `minimum`, `maximum`, `default-value`
that the descriptor table already states. `slint_face_agreement.rs` exists for
this; the fault is never that the test is wrong, it is that a face is not in it.

**4. A shared constant spelled inline.** `gain::MIN_DB` was written out 50
times across ten `.slint` files as `minimum-db` scale bottoms and as the
resting value of every meter reading, while the two colour thresholds beside it
were held to their Rust constants.

**5. A widget default nobody bound.** Not a copy at all -- the inverse. A
component draws something from a property the caller never sets, so the
interface shows a control that cannot work. Three `ChannelMeter`
instantiations drew a latching clip indicator with nothing behind it.

**6. A table with no reader.** The end state the others tend toward. Before
2026-09-14 nothing outside `modulation.rs` read `LFO_`, `ENVELOPE_`, `STEP_`,
`RANDOM_` or `MATH_DESCRIPTORS` anywhere in the workspace. They were `pub`,
exported from the crate root, and the authority for numbers being written out
beside them.

## The one question

`AGENTS.md` states it and it is the whole of the judgement stage:

> **Does anything read the copy the test checks?**

Ask it of any mirrored value, and ask it *before* believing a green suite. A
test named for a markup file that never opens one is the failure this question
catches, and `scripts/dupe-audit one-sided-test` automates the only version of
it a search can reach.

## The stages

| | |
| --- | --- |
| [01-find.md](01-find.md) | Where the copies are, and what each tool can and cannot see |
| [02-judge.md](02-judge.md) | Which leads are findings, and which findings are worth a change |
| [03-fix.md](03-fix.md) | The four fix shapes, and what each one costs |
| [04-guard.md](04-guard.md) | Writing the check that catches the next one |
| [05-verify.md](05-verify.md) | Mutation, the ladder, and three ways a green run lies |

## Runs

**2026-09-15.** `docs/plans/musical-time/`. "Four beats to the bar", spelled
nine times across six crates -- and the note that prompted it guessed three,
which is [01-find.md](01-find.md)'s rule about sizing a sweep from the tree
doing its job. One home (`time::BEATS_PER_BAR`), one pair of types
(`BbtPosition` and `BbtDuration`, two of them because a position counts from
one and a duration from zero), one `BbtText` component, and a
`dupe-audit bar-arithmetic` check.

Two things this run learned that the earlier ones had not. **The guard was
written first**, because [05-verify.md](05-verify.md)'s mutation table wanted
it run against the unfixed tree -- and it then reported ten sites where the
survey had found eight. And the check shipped **narrower than the plan
specified**, because the wider version had one permanent false positive: a
check that is never clean stops being read, which is a worse failure than a
gap that is written down.

**2026-09-13 to 2026-09-14.** Ten items closed across nine passes. Four were
live bugs rather than missing guards: a strip EQ band opening at 2 kHz where
the file meant 3 kHz, a clip latch a removed track left for whichever track
took its index, a spectrum subscription a deleted device left running, and clip
lamps that could not light. Six were guards missing from mirrored values.

Three of the fixes were to the *notes* rather than the code: `LOOSE_ENDS.md`
rows describing a tree that had already moved. See [02-judge.md](02-judge.md),
which is mostly about that.

**2026-09-12.** The audit `AGENTS.md` records. Six defects of one shape, every
value still correct, every test green.
