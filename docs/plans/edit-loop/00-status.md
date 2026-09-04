# Fix the edit loop — plan status

Not started. Written 2026-09-03, out of a session that went looking for
whether to replace Slint with egui and found the toolkit was the wrong
question to be asking first.

## The problem, in Adam's words

> you make 3 decent sized edits, go to test it, ah shit failed, ah shit
> failed again, ok that time it worked. so now for 1 step forward it took 45
> minutes instead of the 45 seconds it took to actually generate the code.

## The problem, measured

Nineteen sessions in this repository's transcript history, counting only
gaps under five minutes so that Adam being away from the keyboard is not
counted as the loop being slow:

| | |
| --- | --- |
| Active working time | **31.1 hours** |
| Of that, spent waiting on `cargo` | **19.1 hours** |
| Share of the working day spent waiting on the compiler | **61%** |
| Cargo invocations | 1,139 |

**Six of every ten working hours are spent waiting for a build.** That is
the plan.

## Where it goes, and it is not where anyone guessed

The `mooloop-session` extraction (`d8e3dd32`, 3.7 active hours, 2.9 of them
cargo) broken down by what each run was and what it found:

| Command | Where | Runs | Mean | Total |
| --- | --- | --- | --- | --- |
| `cargo test --workspace` | box | 21 | **298 s** | **104 min** |
| `cargo clippy --workspace` | box | 18 | 132 s | 40 min |
| `cargo check -p mooloop-ui` | box | 9 | 148 s | 22 min |
| `cargo test -p mooloop-session` | laptop | 30 | **4 s** | 2 min |
| `cargo check -p mooloop-session` | laptop | 17 | **1 s** | 0.3 min |

**166 of those 172 minutes were 48 workspace-wide runs.** The targeted
inner loop -- 47 runs of `-p mooloop-session` -- cost two and a half minutes
in total.

Two things this kills:

- **It is not compile errors.** Only 8 runs of 119 failed to compile, and
  they were caught in 21 s on average because `check` catches them. Two
  minutes of 172. Adam's "you forgot a `()`" theory and the agent's
  "check before handing over" theory are both wrong, or rather both already
  solved.
- **It is not failing runs at all.** 61 of the 65 `cargo test` runs
  *passed*. The time is not going on mistakes; it is going on confirming, at
  five minutes a go, twenty-one times, that nothing broke.

The per-run mean is what separates a good session from a bad one. Across all
nineteen it ranges from 2 s to 117 s, and the sessions at the bottom of that
range cost almost nothing.

## What this plan is, therefore

Not a toolkit migration. Not a test-writing exercise. **A discipline about
which rung of the verification ladder to stand on, plus making the top rung
cheaper.**

| Step | What it does | Expected cost |
| --- | --- | --- |
| `01` | The verification ladder: rules for when to climb it, and a cheaper top rung | a day |
| `02` | Get the build, and the run, off the laptop | a day |
| `03` | Split the device faces actually being edited | two to four days, optional, probably last |
| `04` | Decide whether `egui-view-layer/` still has a case | an hour |

Step 01 is expected to be most of the win and it changes no code.

## What this plan cannot fix

The measured session was a Rust refactor. A UI-heavy session has a different
shape, and for those the binding cost is `mooloop-ui` itself: 148 s a check
on the box, about four minutes to build on the laptop, and a laptop that
during the session that wrote this could not complete the check at all
because it had zero free memory and a full swap.

Splitting device faces into their own crates is measured at **2 s** against
31 s and is real, but it does nothing for `main.slint` or `controls.slint`,
which stay at 30–56 s. If UI work turns out to be where the remaining time
goes, this plan will not be enough -- and that is the finding that decides
`docs/plans/egui-view-layer/`, and much better evidence for it than any
benchmark.

## Evidence already on disk

Two unmerged branches, both with their findings written down:

- **`spike/egui-view-layer`** — a whole-window egui sketch, eight panes,
  drawn against the real session and a running engine. Checks in 0.38 s
  inside 0.19 GB.
- **`spike/slint-split-build`** — `crates/mooloop-ui-ds01`, a device face
  compiled as its own crate, with `main.slint` reaching it through a
  `ComponentContainer`. `cargo test --workspace` green at 1083 passing.
  `docs/plans/egui-view-layer/slint-split-experiment.md` has the method.

Also on that branch: `spikes/slint-units/`, a rig that measures any of this,
including `verification-rungs.sh` and `release-loop.sh`, which step 01 and
step 02 both want and which were still running when this was written.

And in this repository, `scripts/loop-profile`, which produced every number
above and which step 04 uses to decide whether the plan worked:

```sh
scripts/loop-profile              # every session, and the 61%
scripts/loop-profile d8e3dd32     # one session, by command
```
