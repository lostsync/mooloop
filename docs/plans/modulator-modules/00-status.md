# Modulator modules plan status

Step 01 landed on `refactor/modulator-modules` (2026-08-31): descriptor
tables and `get`/`set` by wire id for both modulator kinds, the shelf
collapsed onto one `param-changed` verb (net −351 lines), param edits
undo, and sources can be removed. The envelope's gate input stayed a
dedicated jack verb by design. The mono/poly device-LFO surface is a
separate legacy surface that would benefit from the same collapse later;
it is not part of this plan. Steps 02 and 03 are not started, and each
starts from a fresh worktree off `main` once the previous step merges.

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
