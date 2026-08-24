# Verify the pump/render fixes with Slint's built-in perf logging

## Why this is its own step

The two prior fixes in this plan are targeted at specific measured costs
in the pump closure, but the actual visible symptom ("UI updates aren't
fluid") is a frame-rate/layer-churn problem, and mooloop already ships a
way to observe that directly: `SLINT_DEBUG_PERFORMANCE=refresh_lazy,console`
plus the `MOOLOOP_AUTODRIVE=1` headless self-test
(`crates/mooloop-ui/src/lib.rs:~5502-5620`), which drives dozens of real
UI callbacks (step edits, piano-roll edits, mixer routing, effect
add/param/reorder) and exits with a report. This step establishes a
before/after baseline instead of trusting the micro-benchmarks in isolation.

## What to do

1. Before making any pump changes (or on the commit immediately prior to
   this plan's other two steps), capture a baseline:
   ```
   MOOLOOP_AUTODRIVE=1 SLINT_DEBUG_PERFORMANCE=refresh_lazy,console \
     <release binary> 2>&1 | tee before.log
   ```
   Record average FPS and "layers created" from the first-paint line
   (observed baseline during investigation: `average frames per second: 2
   details from last frame: [160 layers created]` on first paint, settling
   to 60fps afterward on a small 2-channel autodrive project — the number
   to watch is whether layer count or FPS regresses as the autodrive
   script grows the project, e.g. after `invoke_add_channel_clicked` and
   `invoke_add_effect_clicked` calls stack up).
2. After steps 01 and 02 land, re-run the identical command, diff FPS and
   layer counts.
3. Separately, run this against a *larger* synthetic project than
   autodrive currently builds — if autodrive tops out around a handful of
   channels/effects, that's not exercising the `MAX_CHANNELS`/
   `MAX_EFFECTS_PER_CHANNEL` scaling problems this whole investigation
   started from. Consider temporarily extending the autodrive script (or
   writing a one-off) to add ~16-32 channels with a full effect chain
   each, and confirm the pump no longer scales badly with project size
   after 01/02 land — the goal is that pump cost should scale with what's
   *visible/selected*, not with total project size, since most of the
   models being touched are scoped to the selected channel/bus.
4. If FPS or layer count still regresses badly on a large project after
   01/02, that's a separate finding — write it up as a new plan folder
   rather than scope-creeping this one (likely candidates: the
   `main.slint` step-grid repeater combining `border-radius` + `clip:
   true` forcing per-cell offscreen layers — observed 160 layers on first
   paint — worth its own investigation into whether that combination is
   necessary or whether the border-radius/clip could move to a shared
   parent).

## Verification

This step *is* verification for the rest of the plan — its output is a
before/after comparison, not a further code change. Record the numbers in
the PR description for `docs/plans/reduce-ui-pump-overhead/`.
