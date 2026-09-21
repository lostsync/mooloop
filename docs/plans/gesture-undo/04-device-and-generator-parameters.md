# 04 · Device and generator parameters

The bulk of MOO-50, and the step this plan exists for. With 02 and 03 landed,
the mechanism and the brackets are there; this is routing the handlers through
them and settling the two rules that only bite here.

## The handlers

`effect-param-changed(int, int, float)` (`main.slint:1608`) and the
per-generator equivalents are where most of it lands — a face's own
`cutoff-changed` is wired straight through to one of these
(`main.slint:4760-4763` is the filter, and every face reads the same way).
That concentration is good news: a handful of handlers covers most of the
sixty-odd parameters, because the faces already funnel.

Work from step 01's list rather than from this paragraph. It is the thing
that knows which callbacks are unrecorded, and it will have been written
against the tree rather than from a reading.

## The two rules that only matter here

**A modulated or automated parameter undoes to its base value.** The resolved
value is not the stored one: a knob under a modulation route or an automation
lane is *displayed* at the resolved position and *stored* at the base. Undo
restores stored state, so this works correctly by construction — as long as
the snapshot is of the project and not of what the face is showing. The trap
is a handler that reads the face back and writes what it read.
`render.rs`'s write-precedence table and `docs/MODULATION.md:243-260` are the
statement of which writer owns the value; a test per case (plain, modulated,
automated) is what keeps this honest.

**A parameter edit and a structural edit share one history.** They already
do — the history is one list of project snapshots — so the requirement is
only that interleaving them cannot corrupt either. The case to test is the
one that was already broken for a different reason: edit a parameter, add a
channel, edit a parameter, undo three times, and assert each step.

## Labels

Sixty parameters is sixty strings unless the label comes from somewhere that
already exists. `EffectKind::descriptors()` has the name, it is the same table
the face draws from, and it cannot drift from what the user is looking at.
Use it. A label written by hand per callback is a value written twice in the
sense `AGENTS.md` means, and the duplication audit will find it later at
greater cost than writing it correctly now.

## Cost, and when to stop and measure

One snapshot pair per gesture is cheap for a small song and is not free for a
large one: `ProjectSnapshot` clones the project, and `edit_cost.rs` measures
exactly this. Take a measurement on a large project before and after this
step. If a gesture's snapshot is visible in the drag, that is the signal to
consider MOO-50's narrower alternative — a before/after value keyed by
`ParamAddr` rather than a whole-project snapshot — and it is a different
plan, not a widening of this one. **Measure before reaching for it.**

## Done when

- [ ] A knob drag, a wheel notch and a typed entry on any device parameter
      each undo as exactly one step.
- [ ] Undo and redo restore the parameter *and* the face agrees with the
      restored value.
- [ ] A modulated parameter and an automated parameter each undo to the base
      value, with the route and the lane intact. One test each.
- [ ] Parameter edits and structural edits interleave in one history without
      either being corrupted.
- [ ] Labels come from `EffectKind::descriptors()`, not from sixty literals.
- [ ] `edit_cost.rs` has a figure for a gesture's snapshot on a large project,
      recorded wherever the other `edit_cost` numbers live.
