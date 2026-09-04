# Fix the edit loop — plan status

Not started. Written 2026-09-03, out of a session that went looking for
whether to replace Slint with egui and found that the toolkit is the wrong
question to be asking first.

## The problem, in Adam's words

> you make 3 decent sized edits, go to test it, ah shit failed, ah shit
> failed again, ok that time it worked. so now for 1 step forward it took 45
> minutes instead of the 45 seconds it took to actually generate the code.

That is the whole of it. Not memory, not frame rate, not lines of markup.
**One step forward costs 45 minutes, and about 44 of them are waiting.**

## Why this plan exists rather than `egui-view-layer/`

`docs/plans/egui-view-layer/` is a live proposal to replace Slint, and its
strongest argument is compile cost. Everything it claims has now been
measured and holds up: the egui spike checks in 0.38 s inside 0.19 GB where
`mooloop-ui` needs 26–41 s inside 3.4 GB, and
`egui-view-layer/slint-split-experiment.md` establishes that this is not an
artefact of how mooloop invokes Slint.

But a port is weeks of Adam's supervision and cannot be cheaply undone, and
the thing it would buy is *a faster loop*. This plan tries to buy the same
thing for days instead of weeks, so that the port is either unnecessary or
chosen with the alternative already exhausted.

**The honest ceiling, stated up front.** Splitting device faces into their
own crates is measured at 2 s for a face edit against 31 s today. It does
nothing for `main.slint` or `controls.slint`, which stay at 30–56 s. If most
of the editing turns out to happen there, this plan will not be enough, and
that is a finding rather than a failure -- it is the evidence that decides
`egui-view-layer/`.

## What is already known

Measured on the build box (62 GB, 8 cores, rustc 1.98, sccache bypassed
where noted), and on the laptop where marked. See
`docs/plans/egui-view-layer/slint-split-experiment.md` for method.

| | |
| --- | --- |
| `cargo check -p mooloop-ui`, box | 69 s |
| edit `main.slint`, re-check `mooloop-app`, box | 31 s |
| edit `main.slint`, re-**build** `mooloop-app`, box | 56 s |
| edit `controls.slint`, re-check, box | 30 s |
| edit a device face **split into its own crate** | **2 s check, 3 s build** |
| `cargo check -p mooloop-ui`, laptop | 26–41 s, 3.2–3.4 GB |
| `cargo build -p mooloop-ui`, laptop | ~4 min |
| `cargo test -p mooloop-session` | 87 tests, under 1 s |
| `scripts/slint-sketch` | 0.05 s type-check, 0.2 s screenshot |

Two things are already proven and sitting unmerged on
`spike/slint-split-build`:

- **A device face can live in its own crate.** `crates/mooloop-ui-ds01` does
  it, `main.slint` has a `ComponentContainer` where the face was, and
  `mooloop-app` wires them. `cargo test --workspace` is 1083 passing.
- **It requires an experimental Slint feature.**
  `SLINT_ENABLE_EXPERIMENTAL_FEATURES=1`, for a `ComponentContainer` that
  Slint has left out of its standard type register since v1.5.0 and still
  excludes on master.

## The shape of the fix

Cheapest first, measuring after each, stopping as soon as the pain is gone.
Nothing here is worth doing if the step before it already fixed the problem.

| Step | What it does | Expected cost |
| --- | --- | --- |
| `01` | Measure the loop as Adam actually runs it | half a day |
| `02` | Get the build off the laptop | a day |
| `03` | Make the cheap checks catch what the expensive ones are catching now | a day |
| `04` | Split the faces that are actually being edited | two to four days |
| `05` | Decide, with numbers, whether `egui-view-layer/` still has a case | an hour |

**Step 01 is not optional and not a formality.** This plan was written
without knowing two things that change every other step: what is failing on
those two failed attempts, and whether the binary being run is built on the
laptop or on the box. Do not skip ahead on a guess.
