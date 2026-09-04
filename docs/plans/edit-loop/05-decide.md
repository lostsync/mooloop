# 05 — Decide whether `egui-view-layer/` still has a case

Read `00-status.md` and whatever steps 01 to 04 actually recorded.

This step is an hour of writing, and it is the point of the plan.

## The question

`docs/plans/egui-view-layer/` proposes replacing Slint. Its case has been
measured and every part of it holds: the egui spike checks in 0.38 s inside
0.19 GB, the whole-window sketch on `spike/egui-view-layer` draws eight panes
at 107 fps with a full rack, the step grid's drag is easier in immediate mode
than in Slint, and `slint-split-experiment.md` shows the compile cost is not
an artefact of how mooloop invokes Slint.

None of that was ever the real question. The real question is whether one
step forward still costs 45 minutes.

## How to answer it

Re-run step 01's cycle -- the same change, or one the same size -- and put
the two tables side by side. Then say plainly which of these happened:

- **The loop is fine now.** Write that in
  `docs/plans/egui-view-layer/00-status.md`, move that directory to
  `archive/`, and record that the case was good and the problem it solved
  went away by other means. The spike branches stay as evidence.
- **The loop is better but still bad, and the remaining cost is
  `main.slint` and `controls.slint`.** That is the outcome the split cannot
  reach, and it is the strongest possible argument for the port -- much
  stronger than any compile-time table, because it is measured against real
  work rather than against a benchmark. `egui-view-layer/`'s step 02 starts
  with that written into it.
- **The loop did not improve.** Then something in the diagnosis was wrong.
  Do not start a port on a wrong diagnosis; go back to step 01 and find out
  what was actually being waited on.

## The thing to keep in view while deciding

`docs/FOCUS.md` says to prefer changes that produce a musical decision over
changes that add capacity. Both this plan and the egui port are capacity.
Neither makes a sound. Whichever way this goes, it should end with the next
thing being musical -- the sampler v2 work, DS-01's kit, the ML-M1 finding
that is still waiting on Adam's ear -- because the honest measure of a fixed
loop is that it stops being something anyone thinks about.

## Done when

`egui-view-layer/00-status.md` records the decision and the numbers it was
made on, this directory moves to `archive/`, and the next task comes from
`docs/FOCUS.md`.
