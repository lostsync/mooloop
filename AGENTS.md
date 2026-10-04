# Agent Operating Contract

The shared workflow contract for every agent working in this repository
(Claude Code, Codex, opencode, and whatever else is in rotation). Read it
before starting work; `CLAUDE.md` points here rather than repeating it.

## Git workflow: mandatory

`main` is read/merge-only for code. Do not edit any repository-tracked file
in its checkout unless Adam explicitly instructs you to do so, **with one
exception: a change that touches only Markdown files** may be made and
committed directly on `main`. The pre-commit hook enforces the line: it
accepts a commit on `main` only when every staged path ends in `.md`.
Activate it once per clone with `git config core.hooksPath .githooks`. A doc
update that belongs to a code change still goes with that change, in its
worktree.

Before every task, run `git status --short --branch`.

- If the tree has unrelated changes, stop and ask Adam whether to commit,
  discard, or split them before starting another task. Never stash and forget
  them. An untracked Markdown file under `docs/` is the standing exception:
  commit it alongside doc work without asking.
- If you are on `main` and the task touches anything but Markdown, create a
  sibling task worktree before editing:

  ```sh
  git worktree add ../mooloop-worktrees/<branch-name> -b <type>/<slug> main
  ```

  Use `feat/`, `fix/`, `refactor/`, `chore/`, or `spike/` prefixes. Do all
  edits, builds, and tests in that worktree. One task uses one branch and one
  worktree.
- Commit small, buildable changes. Do not use `--no-verify`, rewrite shared
  history, or commit generated output, `target/`, or secrets.
- Sign in once in `CONTRIBUTORS.md`: if your model+harness pair has no row,
  add one; if it already has one, leave it alone.
- Finish with a clean worktree, proportional verification, and a fast-forward
  merge to `main`. Do not merge, force-push, reset, or delete a worktree with
  uncommitted or unmerged work without Adam's explicit confirmation.

## Tracking work: Linear

Linear is where work is tracked: every plan, every step, every defect, and
every question for Adam. The workspace has one team, **Mooloop** (`MOO`), and
the tree cites its issues as `MOO-n`. **The repository holds the content**
(work orders, reasoning, measurements, what a change taught) **and Linear
holds the state**: what is open, what is next, what is blocked, and what is
waiting on Adam. A state written in both drifts, so a document links the
issue rather than restating where it stands. GitHub issues are not used for
tracking; do not file there.

**File it rather than write it down.** A defect, gap or finding you are not
fixing in this change becomes an issue: not a row in `LOOSE_ENDS.md`, not a
paragraph in a report, not a line in a handoff message. Search Linear first;
the backlog already holds near-duplicates. Every issue gets:

- exactly one `Team` label, the team that owns the code where the fix lands
  (`docs/TEAMS.md`);
- one type label (`Bug`, `Feature` or `Improvement`), and `Fable` as well if
  a Fable review found it;
- a project: the plan it belongs to, or `Loose ends` for a defect that
  belongs to no feature (a question no plan has reached yet may go without);
- a description a stranger could act on: what happens, where to start (file
  and symbol), and how to tell it is fixed.

**A plan is a project.** Each directory in `docs/plans/` has a Linear project,
and each step that has not landed has an issue titled `NN · <step title>`
that links its step file; so does anything its `00-status.md` records as
still owed. A new plan gets both on the day it is written. When the last step
lands, the directory moves to `archive/` and the project to Completed in the
same sitting.

| Status | Means |
| --- | --- |
| Backlog | Filed, not scheduled. |
| Todo | Scheduled: this is next. |
| In Progress | Somebody has started. Set it when you start, so a session that dies leaves a trail. |
| In Review | The change is on a branch or a PR and not yet on `main`. |
| Done | The change is on `main`, and a comment names the commit. |
| Canceled | Not doing it, or not a bug. A comment says why. |
| Duplicate | Marked as a duplicate of the issue that carries the work. |

**Done is a claim, so make it checkable.** An issue closes with a comment
naming the commit that fixed it, and that commit's message cites the issue.
A closed issue with no commit is a claim to verify, not a fact to rely on:
MOO-55 was moved to Done with no fix in the tree, and two review runs in a
row rediscovered its open edges.

### Questions for Adam: the `Q&A` labels

When something needs a decision only Adam can make (a taste call, a product
call, a trade-off the tree cannot settle, a mock-up only he can draw) or a
check only he can make on hardware no agent can reach, **ask it on an issue
and add the `Question` label.** That label is how he finds questions. It
belongs to the single-select `Q&A` group, whose other label is `Answer`: when
Adam answers, he swaps `Question` for `Answer`. A question recorded anywhere
else is one he does not know exists. Do not rely on assigning the issue or
@-mentioning him instead: Claude Code reaches Linear through his own account,
and Linear does not notify anyone of their own actions. Listening passes are
the one exception: they queue in `docs/LISTENING.md` (`docs/FOCUS.md`,
*Listening is a step*).

- Put the question on the issue whose work waits on it. If there is none,
  file one, with a project and a `Team` label, whose job is the question.
- **A question never rides on a Done issue.** Adam clears `Question` off
  closed issues, so a question left there is dropped without an answer. If
  the work closes while its question is still open, file a new issue for the
  question and link it.
- Ask in a comment that can be answered without reading anything else: the
  question in one sentence, the options, what you recommend and why, and
  what waits on the answer. Number several questions on one issue so he can
  answer by number.
- A document that mentions the question links the issue (`open: MOO-n`)
  rather than restating it.
- Do not stop for the answer. Carry on with whatever does not depend on it,
  and name the issue in your handoff.
- **Look for `Answer` at the start of a session**, and whenever he says he
  has answered something. For each issue carrying it: read his reply (the
  newest comments), acknowledge it in a comment that restates the ruling and
  what happens next, **remove `Answer`**, and move the issue along. If
  nothing it is `blockedBy` is still open, that means `Backlog` to `Todo`, or
  back to whatever state the question paused. If a blocker is still open, it
  stays where it is and the comment names the blocker. `Answer` means
  *answered, not yet acknowledged* and nothing else.
- Write the ruling into the document it governs, dated and in his words, in
  the change that acts on it.
- An answer that asks something back, or does not settle the question, gets
  a follow-up comment and goes back to `Question`.

### Unconfirmed bugs: the `Triage` label

A bug report nobody has reproduced (Adam's, a review's, or a suspicion from
reading code) is filed with `Bug` and `Triage`. MOO-89 is the shape: what is
suspected, the sequence that would show it, and the test that would settle it.

- **The first job on a `Triage` issue is to confirm or refute it, not to fix
  it.** Write the test that should fail, or reproduce it in the running
  application.
- Confirmed: remove `Triage`, say in a comment how it was reproduced (the
  test's name, or the steps), and set a priority. It is an ordinary bug now.
- Refuted: say in a comment what was checked and against which commit, and
  cancel it.
- A bug you have already reproduced is filed without `Triage`.

### Releases: the `Release` labels

The single-select `Release` group has one label per version (`0.1.4` to
`0.1.10`), so that a push has a boundary. Linear's own Releases feature is
paid, and **Initiatives are not used**: an initiative holds projects, not
issues, so it cannot scope a push. Don't attach projects to one.

- **An open issue with the next version's label is in that push. Nothing else
  is.** Adam chooses which issues get the label. An issue you file during a
  push goes in unlabelled unless he adds it, even when it looks small.
- A closed issue carries the version that shipped it, from `0.1.5` on.
- When a version is cut, its labelled issues that are still open are Adam's
  call. He can move them to the next version or take the label off.

If Linear is unreachable, say so in your handoff and put the question or the
finding there, so the next session can file it.

## CodeGraph

This project has a CodeGraph MCP server (`codegraph_*` tools) indexing every
symbol and edge. Adam wants it used. Check it and index or init as needed
without asking. Prefer it over grep for structural questions (where a symbol
is defined, what calls it, what an edit would break) and keep grep for
literal text. `.cursor/rules/codegraph.mdc` has the tool-by-question table.

## Task context

Source and tests are the truth for current behaviour. Read only the documents
that affect the decision at hand.

| Task | Required context |
| --- | --- |
| What is being built next, and in what order | `docs/FOCUS.md`, then the plan's Linear project and its `docs/plans/<name>/` |
| Product or architecture decision | `docs/PRODUCT.md`, then the relevant architecture/design document |
| Open-ended priority or scope choice | `docs/FOCUS.md` and `docs/SCOPE.md` |
| What is left before the feature freeze, and how big it is | `docs/SCOPE.md` |
| Which plans are live, and what state each is in | The projects in Linear; `docs/plans/README.md` lists them |
| Whether a defect is already known, or what is waiting on Adam | Linear: search the `MOO` team, and the `Question` and `Answer` labels |
| A change that alters a sound or a look only Adam can judge | `docs/LISTENING.md` |
| Broad existing user surface or known gap | `docs/CURRENT.md`, then the one area file in `docs/current/` it points to |
| A small known gap you are about to rediscover | Linear first, then `docs/LOOSE_ENDS.md` for the gaps recorded before it |
| Which team owns a file, a finding, or a Linear issue | `docs/TEAMS.md` |
| A value stated in both Rust and `.slint`, or a run at the duplication fault | `docs/workflows/rust-slint-boundary/` |
| UI layout, controls, or interaction | `docs/UI_DESIGN.md` |
| A new shortcut, menu row, or command surface | `docs/ACTIONS.md` |
| Modulation sources, routes, or destination policy | `docs/MODULATION.md` |
| Retained-audio buffer work | `docs/BUFFER_ENGINE.md` |
| Audio-engine contract work | `docs/AUDIO_ARCHITECTURE.md` |
| Extracting or publishing a reusable DSP unit | `docs/COMPOSABLE_DEVICE_UNITS.md` |
| Gain, level, or metering work | `docs/GAIN_STRUCTURE.md` |
| Save/load, migration, or a new persisted field | `docs/PROJECT_FORMAT.md` |
| Adding a limit to anything a user can create | `docs/CAPACITY_POLICY.md` |
| Reaching for a widget that may already exist | `docs/WIDGET_INVENTORY.md` |

Current explicit user feedback and purpose-built UI designs outrank these
documents. `docs/README.md` indexes every document and states its one job,
for anything this table does not cover.

Active work orders live in `docs/plans/<name>/`, numbered and worked in
order, and each has a Linear project with one issue per step. When a step
lands, close its issue and write in `00-status.md` what the doing found and
changed about the plan. Completed plan directories move to
`docs/plans/archive/`.

## Duplication

A value written down twice is this codebase's characteristic fault: a number
or a rule spelled in both a Rust table and the Slint markup, the copies
drifting, and nothing able to notice. More than once, the tests guarding that
boundary had themselves stopped guarding it and stayed green.

`scripts/dupe-audit` runs the searches that find it. It takes about a second,
needs no build, and reports leads rather than failures; it is not a gate and
CI does not run it. Run it when you have half an hour and no particular task,
or when you are about to touch a shared constant:

```sh
scripts/dupe-audit              # every check
scripts/dupe-audit twin-names   # one of them
scripts/dupe-audit --list       # what they are
```

Each check's docstring says what it looks for, what it deliberately skips,
and the commit it was validated against. Most are lead generators; a few
(`one-sided-test`, `navigation-sends`) are regression guards whose expected
answer is zero, and `unrecorded-edit` is a count working down to zero.

**The fault worth looking for is one level up from a duplicate: a guard that
has come off a value written twice.** Copied arithmetic does not drift, and
the values were all still correct when found, so nothing reported the drift.
Before believing a green suite, ask of any mirrored value:

- **Does anything read the copy the test checks?** A test that compares a
  table with a literal copy written inside the test, or checks a list that
  nothing reads, guards nothing.
- **Does anything here press the button?** A control's data can be entirely
  correct while the control does nothing. Closing a `PopupWindow` before its
  callback runs (`popup-close-order`) shipped four times this way.

When you write a check:

- **Write it before the fix, and validate it against the commit it was
  written for.** Run against the unfixed tree, its silence is the check
  failing; written afterwards, it is shaped by what its author already knew.
- **One permanent false positive is worse than a narrow check**, because a
  check that is never clean stops being read. Name and reason any allowance
  in the check itself.
- **A check is only as honest as its list of places to look.** Callbacks are
  wired by macro as well as by `window.on_`, and edits arrive from the pump
  as well as from callbacks. A resolver that has to guess must fail towards
  reporting, never towards printing a zero.
- **Calibrate it against a real case from the tree**, not only the example
  that commissioned it.

A clean `repeated-line` is weaker than it looks: it matches bytes, so a
renamed variable hides a copy completely, and the copy most likely to
diverge is the one somebody edited on the way past. When a trait has one
required method and a dozen implementors, read all twelve before believing
they differ.

## Parameter identity across the session boundary

Two integer address spaces meet at the `mooloop-session` crate boundary, and
they are not interchangeable:

- `EffectSlotRow.pN`, modulation arrays, automation lanes and engine events
  use a parameter's stable descriptor **id**.
- `Session::set_effect_param(slot, param_index, normalized)` is a face-editing
  API and takes the parameter's **position in the kind's descriptor table**.

Most effect tables have `id == position`, which hides the distinction.
Buffer's retired ids make its table sparse, and forwarding ids through the
session API once made its face operate the wrong parameters while the DSP
and session tests stayed green.

At this boundary, name an integer `id` or `param_index` according to what it
is; never use one as the other because the values happen to agree. Derive a
conversion from `EffectKind::descriptors()` rather than spelling another map.
A regression test for face wiring must read the production `.slint` callback
and compare it with that table; a test that calls the session or DSP directly
does not cross the boundary that failed. When changing or extracting an API,
audit every plain integer it accepts for this kind of lost semantic type.

## Documentation is part of the change

`docs/CURRENT.md` and its area files in `docs/current/` describe the
application as it exists. A change that adds, removes, or alters user-visible
behaviour updates the area's file in the same commit; so does a change that
invalidates a fact stated in any other document. Leaving a document to be
corrected later is how it stops being trusted.

**A working document says what is true now.** When a change makes a passage
untrue, rewrite or delete it; do not append a dated paragraph after it. How
it got that way belongs in the commit message, the Linear issue, or
`docs/JOURNAL.md`. Each document in `docs/` has one job (`docs/README.md`):
keep to it, and keep it short enough to be read.

## Comments

A code comment is one of three kinds, and each has one place (Adam,
2026-09-30):

1. **`///` and `//!` state the contract**: what the item does, what it
   guarantees, its units, which thread may call it. Write it for someone
   reading the rendered docs, who sees nothing but the signature beside it.
   A still-true design reason may stay here as one sentence. A trait
   impl's doc says only what the impl adds to or changes about the trait's
   contract.
2. **`//` in a body says why the code is this way**, and only where the code
   cannot say it itself. Tests are never rendered, so everything in them is
   this kind.
3. **History goes elsewhere**: what the code used to do, which issue or plan
   step changed it, what broke before. That belongs in the commit message,
   the Linear issue or `docs/JOURNAL.md`.
   - A link to a plan or design document is a reference, not history; keep
     it. A bare "step 05" is history; drop it.
   - An issue id stays only while its issue holds a decision that is still
     open, and the sentence around it says what is *to come*, in the future
     tense.
   - Where a closed issue was the only path to evidence (a measurement, a
     spec table), name the evidence in the comment, not the ticket.
   - Who owns the code is `docs/TEAMS.md`'s to say; do not copy it into a
     module doc, where it will drift.

A comment pass is about stating contracts at least as much as cutting
history; the pilot added more lines than it removed, and its useful find was
seven docs that had stopped being true.

**Keep the contract true as the code moves.** When you change a function,
fold what you learned into its contract and delete what is no longer true;
do not append a paragraph. When you change what a function is *used for*, a
caller in another crate included, update that function's contract in the
same change. A comment that grows by one paragraph and one issue id per fix
is how `carry_strips_from` came to have 121 comment lines for 82 lines of
code (`docs/workflows/accretion/`).

A crate that has been through a comment pass gets `#![warn(missing_docs)]`,
so the contract cannot quietly go missing again.

## Slint

This project pins Slint `1.18.1`. Before editing `.slint`, `slint::` Rust API,
or `slint-build`, consult the version-matched documentation:
`https://releases.slint.dev/1.18.1/docs/slint/`, and
`https://docs.rs/i-slint-backend-testing/1.18.1/` for the `ElementHandle` API
the UI tests and the MCP server both drive. If the pinned version changes, use
the matching release URL instead of relying on latest-version knowledge.

**Do not reach for `cargo build` to find out whether a `.slint` edit is valid
or what it looks like.** `scripts/slint-sketch` type-checks a scratch `.slint`
against the real widgets in about 0.05 s and screenshots it in about 0.2 s,
where `cargo build -p mooloop-ui` is about four minutes for any edit at all.
It also takes the real window: `scripts/slint-sketch
crates/mooloop-ui/ui/main.slint` type-checks it in about 3 s, and `--shot`
renders it with empty models, which is enough to see the layout, the chrome,
the toolbar and the status bar. Iterate there, then build once. It needs
`slint-viewer` installed locally; that is deliberately not a workspace
dependency.

To see what the real interface does with live models behind it, run
`scripts/mooloop-mcp`: it starts the application with Slint's embedded MCP
server, whose tools read the live element tree and click, type, drag, and
screenshot it. `docs/OPERATIONS.md` has the details of both.

## Verification and operations

Do not run Cargo commands concurrently. Read
[docs/OPERATIONS.md](docs/OPERATIONS.md) before running Cargo, UI
snapshots, or the live application; it contains this machine's memory limits,
the build box, and the rendering procedures. A Claude Code web container is
different again (15 GB and no swap, so an overshoot is an OOM kill rather
than a slowdown): see *Working In A Cloud Container* there.

### The verification ladder

Most of Adam's working time goes on `cargo`, and almost all of it on
workspace-wide runs reached for out of caution rather than need. **Stay on
the lowest rung that can see the thing you just changed, and climb only when
you are about to stop.**

| Rung | Command | Cost | Use it |
| --- | --- | --- | --- |
| 1 | `cargo check -p <crate>` | ~1 s | after every edit |
| 2 | `cargo test -p <crate>` | 4 s laptop / 18 s box | after every edit that changes behaviour |
| 3 | `cargo test --workspace --exclude mooloop-ui` | 43 s | before handing work over |
| 4 | `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` | 118 s + ~88 s | before committing, and nothing smaller than a milestone |

- **`cargo test --workspace` is a commit-time command, not an iteration-time
  command.** Never run rung 4 to find out whether something compiles. That is
  rung 1, and it is a hundred times cheaper.
- **Iterate on the laptop, verify on the box.** Rungs 1 and 2 on a single
  small crate are fastest locally. Rungs 3 and 4, and anything touching
  `mooloop-ui`, need memory the laptop does not have: send them to the
  remote build box with `scripts/antibox`.
- **`.slint` edits do not need a build at all** (see *Slint* above).

Rung 3 earns its place because `mooloop-ui`'s seven test binaries are most of
what a workspace run compiles and links; excluding them is 70% off.

### Background anything above rung 2

**This is the largest difference between a fast session and a slow one, and
it is free.** Measured sessions that backgrounded most of their compiler time
spent far less time blocked; they did not have faster builds, they had builds
nobody was watching. Run rungs 3 and 4 with `run_in_background: true` and keep
working; the harness notifies on exit. Do not poll the output file: a run
piped through `tail` writes nothing until it finishes.

The exception is when the very next thing you do depends on the result and
there is nothing else to usefully do. That is rarer than it feels.

### Run clippy the way CI runs it, or you are not running it

**`-- -D warnings`.** `.github/workflows/ci.yml` denies warnings; a bare
`cargo clippy` does not, and the difference is the whole result. A warning
checked with a bare `cargo clippy` was once recorded as tolerated while it
failed CI on `main` on every push for two days. An exit code only answers the
question you asked. Ask CI's.

### Never read a piped run's exit code

**A command piped into `tail` reports `tail`'s exit status, not cargo's.**
`cargo test ... | tail -40` exits 0 when every test failed. And `$?` is
whatever ran *last*, so an `echo`, a `cd`, a `grep`, or a second cargo command
between the run and the read answers in its place.

**Run it through `scripts/exit-code` and the question cannot be got wrong.**
It sends the output to a log, captures `$?` the instant the command returns,
prints the tail of the log and any failure lines in it, and exits with that
same status, so the harness's own `[exited with code N]` is the command's too:

```sh
scripts/exit-code cargo test -p mooloop-ui
scripts/exit-code cargo clippy --workspace --all-targets -- -D warnings
scripts/exit-code --tail 100 cargo test --workspace --exclude mooloop-ui
```

Background it exactly as you would the bare command; the log path is printed
before the run starts. By hand it is a redirect, with nothing at all between
the run and the read:

```sh
cargo test -p mooloop-ui > /tmp/t.log 2>&1; echo "EXIT: $?"
grep -E '^test result' /tmp/t.log
```

**Read the summary lines for failures; do not do arithmetic on them.** A
workspace run's test binaries write the same stream in parallel, so a
`test result:` line can have another binary's progress spliced through it
and lose its count. Grep for `test result: FAILED`, for `panicked at`, and
for a non-zero `N failed`: a splice cannot invent a failure, so those are
robust where a sum is not. `scripts/exit-code` greps for exactly those three.
`set -o pipefail` is not on in the harness's shell, so do not assume it.

### Never search for a process with the pattern you are typing

**`pgrep -f mooloop` matches the shell that is running `pgrep -f mooloop`**,
because the harness runs each command as `zsh -c "<the whole command>"`. So a
search reports the application running when it is not, and `pkill -f` on the
same pattern kills the shell the agent is running in, which ends the session.
`ps aux | grep mooloop` has the identical fault plus a piped exit code. Use
`scripts/procs`, which skips everything in its own process tree by pid:

```sh
scripts/procs mooloop                 # list; exit 1 when nothing matches
scripts/procs --kill mooloop          # SIGTERM the matches, report survivors
scripts/procs --kill --force mooloop  # SIGKILL whatever ignored SIGTERM
scripts/procs --threads data-loop     # thread names, with scheduling policy
```

It stays inside your own uid unless given `--any-user`, and it refuses to
`--kill` on a pattern shorter than three characters.

### Order device work so the face contract comes last

A device has three parts and they differ by four orders of magnitude:

| Part | Cost to see it | Where |
| --- | --- | --- |
| DSP and engine code | 1-4 s | laptop |
| Face markup, visual only | 0.05 s via `scripts/slint-sketch` | laptop |
| The face *contract*: a new property or callback crossing `main.slint` and `lib.rs` | 30 s to check, 8.7 min to a release binary | box |

So get the DSP right against unit tests, iterate the face visually with
`slint-sketch`, and cross into `main.slint` **once**, with every new property
and callback batched into that one pass. The eight-minute build belongs once
per device feature, not once per knob.
