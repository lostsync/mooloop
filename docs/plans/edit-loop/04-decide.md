# 04 — Decide whether `egui-view-layer/` still has a case

Read `00-status.md` and steps `01` to `03`, which are all now closed.

## The question

`docs/plans/egui-view-layer/` proposes replacing Slint. Its case has been
measured and every part of it holds: the egui spike checks in 0.38 s inside
0.19 GB, the whole-window sketch on `spike/egui-view-layer` draws eight panes
at 107 fps with a full rack, the step grid's drag is easier in immediate mode
than in Slint, and `slint-split-experiment.md` shows the compile cost is not an
artefact of how mooloop invokes Slint.

None of that was ever the real question. The real question is whether one step
forward still costs 45 minutes.

## What the plan changed

**Step 01, landed.** `scripts/antibox` now picks incremental compilation for
dev builds and sccache for release builds instead of always sccache, which is
measured at 64% off `cargo test --workspace` (332 s to 118 s) and 70% off
`cargo test -p mooloop-session` (31 s to 18 s). `AGENTS.md` carries the
verification ladder, so the workspace run is named a commit-time command
rather than the thing to reach for after every edit. The mockup tool is behind
a Cargo feature and out of the shipping build.

On the measured session's own traffic — 21 workspace test runs, 104 minutes —
the flag and the rule together are worth about 85 minutes of a 224-minute
session.

**Step 02, landed.** `scripts/mooloop-run` is one command from edit to running
application, `--dev-bin` gives the fast profile, `--keep-symbols` gives
backtraces, and `--prune` stops the box filling itself up.

**Step 03, closed unstarted.** Splitting device faces into their own crates
works and is measured at 13 s against 522 s, but 79% of face commits also edit
`main.slint`, only one in eight touches nothing else, and three quarters of
the `main.slint` churn in those commits is shell work a split does not remove.
Days of work on a feature Slint has disowned for two years, to make one commit
in eight fast.

## What that leaves

**The Rust half of the loop is fixed, and the UI half is not.**

Nothing in this plan moved `main.slint`. It still checks in about 30 s and
takes 8.7 minutes to a release binary, it is 29% of all `.slint` edits, and it
is coupled into 79% of the rest. The split experiment established why: remove
every device face and the generated module falls 41% while peak RSS falls 15%,
because what remains is `MainWindow` itself with 217 properties and 296
callbacks. **The shell is the floor, and no arrangement of Slint crates gets
under it.**

That is precisely this step's second outcome, the one it was written to
recognise:

> **The loop is better but still bad, and what is left is `mooloop-ui`.** That
> is the outcome the split cannot reach, and it is the strongest possible
> argument for the port — much stronger than any compile-time table, because
> it is measured against real work rather than against a benchmark.

It is stronger here than the plan anticipated, because step 03 did not have to
be built to find it out. The coupling measurement cost an hour and answered
the same question the split would have answered after four days.

## The finding that arrived while writing this

`scripts/loop-profile` was measuring backgrounded builds as free, and fixing
it changed the answer more than any step in this plan did. Across the same
nineteen sessions: the eight that spent under 40% of their time blocked had
backgrounded 76% of their compiler time, the ten over 60% had backgrounded 5%,
and the correlation is -0.73.

**The difference between a good session and a bad one was never how fast the
build was. It was whether anyone was watching it.**

That reframes the whole question this step asks. Two of the three levers here
-- the profile flag and the ladder -- make the compiler do less work. This one
makes the wait stop counting, and it is bigger than both, free, and already
available. It is now a rule in `AGENTS.md`.

It also weakens the argument for the port, honestly. A large part of what
looked like a toolkit problem was a *habit* problem, and habits are cheaper to
change than view layers. `main.slint` is still 8.7 minutes and still
unreachable from inside Slint -- but 8.7 minutes you are not sitting through
is a different complaint from 8.7 minutes you are.

## The number arrived — 2026-09-04

`feat/device-presets` is the first long session run under the new defaults.
`scripts/loop-profile b9107524`:

| | Blocking share | Workspace runs |
| --- | --- | --- |
| the nineteen sessions this plan was written from | 57% | 21, in the worst |
| the preset session | **10%** | 4 |

**Under this step's own 25% threshold**, and the ladder is visibly in use: the
failures it hit — four failing test runs and one that did not compile — were
caught at rung 2, `cargo test -p mooloop-ui` at 17-45 s, not by a workspace
run. That is the discipline the plan asked for, doing exactly what it was
supposed to do.

### What the same session disproved

One run took **2 h 10 m**, and the obvious explanation was wrong.

The guess was cold-start: a new worktree gets an empty target directory, this
plan's flag turns sccache off for dev builds, so the first build compiles
every dependency from source. Plausible, and false. Measured on a deliberately
emptied target directory, `cargo test --workspace --no-run`:

| | |
| --- | --- |
| cold, sccache | **431 s** |
| cold, incremental | **431 s** |

Identical. sccache makes no measurable difference to a cold build of this
workspace, so **the profile rule needs no cold-start exception** — incremental
is never worse, and on a warm tree it is 332 s against 118 s. A change to add
that exception was written, measured, found to buy nothing, and reverted.

The real shape of the slow run, from its own output: it compiled 38 crates in
**1 m 52 s**, ran its tests in 0.07 s, pulled its snapshot, and exited 0 --
with **two hours between its last line of output and its exit**. No concurrent
cargo job was on the box. Nothing in the log accounts for the gap. It was not
work; it was a stall.

So `scripts/antibox` now takes `--timeout`, default 45 minutes. The cause is
still unknown and one occurrence is not a pattern, but a run that stalls
should cost minutes rather than a night, and no build in this workspace
legitimately runs that long.

**This does not change the headline.** The session backgrounded that run, so
two hours of stall cost 10% blocking rather than an evening. The habit finding
covered for the failure, which is the strongest thing that could be said for
it.

## The one number that was still missing

Every figure above is measured against *past* sessions or against the build
itself. What has not happened yet is a long session run *under* the new
defaults, and that is what closes this step:

```sh
scripts/loop-profile              # the share of active time spent on cargo
```

It was 61% across the nineteen sessions this plan was written from, and the
count to watch beside it is workspace-wide runs in a single session, which was
twenty-one. Run it after the next long session and write the answer here.

The three outcomes it was written to choose between, with the answer now in:

- **Under 25%, and the session was UI-heavy.** The loop is fine; archive
  `egui-view-layer/` and record that its case was good and the problem went
  away by other means. Note that the number to read is now **`block`**, not
  total cargo time -- and that six of the nineteen original sessions were
  already under 25% by that measure, entirely through backgrounding.
- **Still bad, and the remaining time is `mooloop-ui`.** That is what
  everything above predicts. `egui-view-layer/`'s step 02 starts, with this
  paragraph written into it.
- **Still bad, and the remaining time is somewhere else entirely.** Then the
  diagnosis was wrong somewhere; go back to `00-status.md` before starting a
  port.

The honest reading of the evidence in hand *was* the second. The
backgrounding finding made it less clear-cut, and **the measurement came back
as the first**: 10%, on a session that was as UI-heavy as they get — a rack
row, a popup menu, a `main.slint` crossing, and two snapshot suites.

That is the outcome this step named as "archive `egui-view-layer/`". Before
doing that, one honest caveat: 10% is one session, and it is one whose worst
build was backgrounded rather than fast. `main.slint` is still 8.7 minutes and
still unreachable from inside Slint. What changed is that nobody sat through
it. Whether that is "the loop is fine" or "the loop is hidden" is a judgement,
and it is Adam's — but the number he set as the test has been met. This step should not
pre-empt it on Adam's behalf, because a toolkit replacement is a product
decision and one more measurement is cheap — but it should say plainly that
the evidence points one way, and it does.

## The thing to keep in view while deciding

`docs/FOCUS.md` says to prefer changes that produce a musical decision over
changes that add capacity. Both this plan and the egui port are capacity.
Neither makes a sound.

So the next task is not in this directory. It is `docs/FOCUS.md`'s sequence:
**finish ML-P8** — `docs/plans/poly-synth-v2/` steps 05 through 07 — which is
half built with its cost known. The honest measure of a fixed loop is that it
stops being something anyone thinks about, and the way to find out whether
this one is fixed is to go and use it on something that makes a noise.

## Done when

`scripts/loop-profile` has been run against a session that happened after
these changes, its number is written above, `egui-view-layer/00-status.md`
records the decision, and this directory moves to `archive/`.
