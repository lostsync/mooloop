# Modulator modules plan status

Step 01 landed on `refactor/modulator-modules` (2026-08-31): descriptor
tables and `get`/`set` by wire id for both modulator kinds, the shelf
collapsed onto one `param-changed` verb (net −351 lines), param edits
undo, and sources can be removed. The envelope's gate input stayed a
dedicated jack verb by design. The mono/poly device-LFO surface is a
separate legacy surface that would benefit from the same collapse later;
it is not part of this plan.

Step 02 landed on `feat/module-vocabulary` (2026-09-01): step, random,
and math kinds, each a descriptor table plus a tick, with the add menu's
two callbacks collapsed into one `source-added(kind)` verb and the
per-kind name matches replaced by `ModulatorKind::badge`. Three things
the plan left open resolved in the doing:

- The slot-order rule needed no machinery. `outputs` already holds last
  tick's value everywhere the evaluation pass has not reached, so a math
  module reading a lower slot sees this tick and one reading itself or a
  higher slot sees the previous — for free, and self-reference is bounded
  by the module's own output clamp rather than by a cycle check.
- Random kept the LFO's three-id tempo-syncable rate rather than the
  division-only clock the plan describes, because it is a promotion of
  that LFO's hidden sample-and-hold and dropping the free rate would be a
  regression for anything migrating off the waveform. Step is
  division-only, as written.
- `Clamp` needed a second and third id (`clamp_low`, `clamp_high`) beside
  the arithmetic operand, so all three defaults could be no-ops.

Step 03 is not started, and starts from a fresh worktree off `main` once
step 02 merges.

Adam pulled this in explicitly on 2026-08-31: the modulation
rack becomes the power plant of the app — a grid of small modules, each a
discrete control-signal device, pluggable across the app the way the mod rack
already gestures at. `NODE_MODEL.md` records the wider conversation; this
plan is the part of it that is now scheduled.

This supersedes the "More modulation taxonomy for its own sake" deferral in
`FOCUS.md` for exactly the steps written here, and no further. The spec
(`MODULATOR_SYSTEM_SPEC.md`) remains authoritative for the routing model;
nothing here replaces `ParamAddr`, routes, destination policy, or the
32-frame control contract.

## Order

1. `01-modulators-join-the-param-system.md` — the foundation refactor. No
   new capability; modulators adopt the descriptor/param paradigm effects
   already use, the UI glue collapses from per-field plumbing to one verb,
   param edits become undoable, and sources become deletable.
2. `02-the-module-vocabulary.md` — first new module kinds (step sequencer,
   random/drunk, math/clamp), proving the refactor made kinds cheap.
   **Landed 2026-09-01.**
3. `03-the-grid.md` — the expanded grid presentation and capacity growth.

Each step is one branch. A step lands playable, saveable, and renderable
before the next starts.

## What this plan refuses to do

- No audio-domain modules through the control rack. `clip~`, grain
  generators, FFT, resamplers are effects (the insert rack already hosts
  that domain) or wait on the typed audio edges `AUDIO_ARCHITECTURE.md`
  defers. An envelope follower arrives later as a device outlet under the
  one-block control-table rule, not as a borrowed bus.
- No node canvas. The grid is a presentation of the same rack; a graph view
  remains the optional last step of the spec's delivery order.
- No second routing language. New modules are sources like the LFO is a
  source; routes, polarity, depth, and destination policy are unchanged.
