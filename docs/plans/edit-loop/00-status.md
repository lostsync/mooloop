# Fix the edit loop — plan status

**Steps 01 and 02 landed 2026-09-04. Step 03 is closed unstarted, on a
measurement. Step 04 waits on one number that only a future session can
produce.** Written 2026-09-03, out of a session that went looking for whether
to replace Slint with egui and found the toolkit was the wrong question to be
asking first.

What changed: `scripts/antibox` picks incremental compilation for dev builds
and sccache for release builds instead of always sccache (64% off a workspace
test run), `AGENTS.md` carries the verification ladder and the rule to batch
face-contract edits, the mockup tool is behind a Cargo feature (12.6% of the
window's generated module, out of every build), and `scripts/mooloop-run` is
one command from edit to running application.

What did not change, and cannot from inside Slint: `main.slint`, which is 29%
of all `.slint` edits and coupled into 79% of the rest. That is what step 04
hands to `docs/plans/egui-view-layer/`.

## The problem, in Adam's words

> you make 3 decent sized edits, go to test it, ah shit failed, ah shit
> failed again, ok that time it worked. so now for 1 step forward it took 45
> minutes instead of the 45 seconds it took to actually generate the code.

## The measurement was wrong, and the correction is the finding — 2026-09-04

`scripts/loop-profile` scored a backgrounded build as free: it measured the
gap between a tool call and its result, and a backgrounded call returns
immediately. Fixed, it recovers **10.1 hours** of compiler time the original
61% never saw, and it splits the number in two -- time that *stopped* the
session, and time that overlapped other work.

| | Blocking share | Backgrounded cargo |
| --- | --- | --- |
| the 8 sessions under 40% | 3-38% | **76% of their cargo time** |
| the 10 sessions over 60% | 65-92% | **5%** |

Correlation between the two: **-0.73**.

**The good sessions did not have faster builds. They had builds nobody was
watching.** That is available today, costs nothing, needs no flag and no
toolkit, and it is a larger effect than anything else in this plan. It is
now the third rule in `AGENTS.md`'s ladder.

The session that wrote this ran 19 minutes of compiler time at **0% blocking**.

Corrected totals across the nineteen sessions: 27.9 h of cargo, of which
17.8 h blocking against 31.3 h active -- **57% blocking**, not 61%, with
10.1 h already overlapped.

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

(Corrected above: 57% once backgrounded runs are measured rather than scored
as free. The figures in this section are the original ones, kept because the
rest of the plan was written from them.)

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

## The other half: getting to something you can hear

Measured on the box, `cargo build --release -p mooloop-app`, which is what
turns an edit into an application with sound coming out of it:

| Edit | Rebuild |
| --- | --- |
| a Rust file | **12 s** |
| a device face **split into its own crate** | **13 s** |
| `main.slint` | **522 s — 8.7 minutes** |
| cold, first of the day | 697 s |

**Rust work is fine.** Twelve seconds from edit to runnable binary is not
what anybody is complaining about, and it means step 02 is narrower than it
looked: the build machinery is not the problem, the shell's markup is.

**A `main.slint` edit costs nearly nine minutes to hear**, on the fast
machine, and nothing in this plan changes that. Splitting device faces takes
a face edit from that to thirteen seconds; it leaves `main.slint` exactly
where it is. That is the sentence step 04 has to weigh.

## What this plan is, therefore

Not a toolkit migration. Not a test-writing exercise. **A discipline about
which rung of the verification ladder to stand on, plus making the top rung
cheaper.**

| Step | What it does | Expected cost |
| --- | --- | --- |
| `01` | The verification ladder: rules for when to climb it, and a cheaper top rung | a day |
| `02` | Get the build, and the run, off the laptop | a day |
| `03` | Split the device faces actually being edited | **not worth starting — measured below** |
| `04` | Decide whether `egui-view-layer/` still has a case | an hour |

Step 01 is expected to be most of the win for Rust work. Most of it is a
flag: `scripts/antibox` disables incremental compilation so sccache can
work, which costs 64% on a workspace test run and is measured in that
step. Step 03 is the only one that touches UI work. They fix different
sessions and neither fixes both.

## The mockup tool, out of the shipping build — measured 2026-09-04

Step 01's smaller win turned out to be three times its estimate. Moving the
mockup tool to its own Slint entry point behind a Cargo feature takes the
window's generated module from **43.9 MB to 38.4 MB** -- 5.5 MB, 12.6%, out of
every build including release. The spike had estimated 1.78 MB and 4.3% from
its own rig.

## Where the UI edits actually go — measured 2026-09-04

Step 03 is days of work on a Slint feature the vendor disowns, so before
starting it, the question of which files the UI work is *in*. Three months of
`.slint` history, 502 file-touches:

| File | Touches | Share |
| --- | --- | --- |
| `main.slint` | 145 | 29% |
| `controls.slint` | 32 | 6% |
| the twenty-one device faces, together | 178 | 35% |

The faces look like a good target until the coupling is measured:

- **61 of 77 commits that touch a device face also touch `main.slint`** — 79%.
- **Only 10 of 78 touch no other `.slint` file at all.**
- Of the `main.slint` lines those commits change, **25% are forwarding
  property declarations and `root.*` bindings** — the boilerplate a split
  deletes. **The other 75% is shell work a split does not touch.**

So the 13 s-against-522 s headline applies to about one face commit in eight.
For the rest, the split removes a quarter of the coupling and the `main.slint`
rebuild is still paid. **Step 03 should not be started**; the reasoning is
written into it, and it is what step 04 decides on.

(The 25% is a regex over diff lines — property declarations and `root.*`
bindings — so multi-line and callback forwards are undercounted. The direction
is not in doubt; the exact figure is a proxy.)

## What this plan cannot fix

The measured session was a Rust refactor. A UI-heavy session has a different
shape, and for those the binding cost is `mooloop-ui` itself: 148 s a check
on the box, about four minutes to build on the laptop, and a laptop that
during the session that wrote this could not complete the check at all
because it had zero free memory and a full swap.

Splitting device faces into their own crates is measured at **2 s to check
and 13 s to a runnable release binary**, against 31 s and 8.7 minutes. That
is real and it is large. But it does nothing for `main.slint` or
`controls.slint`, which stay at 30 s to check and 8.7 minutes to build.

**So the ceiling is `main.slint` itself**, and no arrangement of Slint
crates reaches it -- `slint-split-experiment.md` established that the
generated module's cost is not an artefact of how mooloop invokes Slint. If
the remaining time turns out to be going there, this plan cannot fix it, and
that is the finding that decides `docs/plans/egui-view-layer/`. It is much
better evidence than any benchmark, because it is measured against the work
rather than against a probe.

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
