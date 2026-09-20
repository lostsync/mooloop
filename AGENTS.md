# Agent Operating Contract

The shared workflow contract for every agent working in this repository
(Claude Code, Codex, opencode, and whatever else is in rotation). Read it
before starting work; `CLAUDE.md` points here rather than repeating it.

## Git workflow: mandatory

`main` is read/merge-only for code. Do not edit any repository-tracked file
in its checkout unless Adam explicitly instructs you to do so, **with one
exception: a change that touches only Markdown files** may be made and
committed directly on `main`. The pre-commit hook enforces the line -- it
accepts a commit on `main` only when every staged path ends in `.md`. A doc
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
- Before every commit, update your model+harness row in `CONTRIBUTORS.md`.
  It is a roster and it is a table: bump `Last seen` and `Sessions`, and leave
  `Known for` alone unless it has stopped being true. Detail belongs in the
  commit message, or in `docs/JOURNAL.md` if it is a narrative worth keeping.
- Finish with a clean worktree, proportional verification, and a fast-forward
  merge to `main`. Do not merge, force-push, reset, or delete a worktree with
  uncommitted or unmerged work without Adam's explicit confirmation.

The tracked pre-commit hook rejects ordinary commits on `main` other than
Markdown-only ones; activate it
once per clone with `git config core.hooksPath .githooks`.

## CodeGraph

This project has a CodeGraph MCP server (`codegraph_*` tools) indexing every
symbol and edge. Adam wants it used. Check it and index or init as needed
without asking. Prefer it over grep for structural questions — where a symbol
is defined, what calls it, what an edit would break — and keep grep for
literal text. `.cursor/rules/codegraph.mdc` has the tool-by-question table.

## Task context

Source and tests are the truth for current behavior. Read only the documents
that affect the decision at hand.

| Task | Required context |
| --- | --- |
| What is being built next, and in what order | `docs/FOCUS.md`, then the matching `docs/plans/<name>/00-status.md` |
| Product or architecture decision | `docs/PRODUCT.md`, then the relevant architecture/design document |
| Open-ended priority or scope choice | `docs/FOCUS.md` and `docs/SCOPE.md` |
| What is left before the feature freeze, and how big it is | `docs/SCOPE.md` |
| Which plans are live, and what state each is in | `docs/plans/README.md` |
| Broad existing user surface or known gap | `docs/CURRENT.md` |
| A small known gap you are about to rediscover | `docs/LOOSE_ENDS.md` |
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
documents.

Active work orders live in `docs/plans/<name>/`, numbered and worked in
order; `00-status.md` says what has landed. Update that status when a step
lands. Completed plan directories move to `docs/plans/archive/`.

`docs/README.md` indexes every document and states its one job, for anything
this table does not cover.

## Duplication

A value written down twice is this codebase's characteristic fault. A pass on
2026-09-10 found ten defects and five of them were one shape: a number or a
rule spelled in both a Rust table and the Slint markup, the copies drifting,
nothing able to notice. Three of the *tests* guarding that boundary had
themselves stopped guarding it -- one parsed its own source two different
ways, one covered the generator faces while claiming to cover any range
written twice, and two held their own copy of `TICKS_PER_STEP`, so they would
have passed while the roll drew every note in the wrong place. All three were
green throughout.

`scripts/dupe-audit` runs the searches that found them. It takes about a
second, needs no build, and reports leads rather than failures -- it is not a
gate and CI does not run it. Run it when you have half an hour and no
particular task, or when you are about to touch a shared constant:

```sh
scripts/dupe-audit              # every check
scripts/dupe-audit twin-names   # one of them
scripts/dupe-audit --list       # what they are
```

A sixth check, `one-sided-test`, is a regression guard rather than a lead
generator: zero hits is its expected result and its answer. It reports a test
named for a markup file that never reads one, which is item 2 above. It was
verified by running it against `7024b2b~1`, where it finds the defect, and its
first version *missed* it -- because the broken test mentioned `main.slint` in
its own comment, and a mention is not a read. Validate a check of this kind
against the commit it was written for, or it is decoration.

A fifth check, `unchecked-face`, was added on 2026-09-12 and is a different
shape from the other four: it does not look for a duplicate, it looks for a
duplicate **nothing is watching**. `slint_face_agreement.rs` holds a device's
face and its descriptor table together, and it works from a hand-written list
of faces -- for a reason it states itself, that a derivation would move with
whichever side it was derived from. A hand-written list then has the failure
that keeps recurring here: the generator faces were covered for months while
the effect faces were not. The check reports the faces the test cannot see,
and it separates the two reasons a face might not be in it. Eight faces and
twenty-three numbers were outside it the day it was written.

Two things it deliberately does not do. It does not look for duplicated
*formulas*: four synths computing `sin(2*pi*phi)` will agree forever, because
a sine has no parameters to drift. And it does not chase Rust constants
spelled as bare numbers in `.slint`, which is a real gap -- `TICKS_PER_STEP`
is a literal `24` twenty-five times -- but a bare number carries no identity
and the check produced a hundred leads for one answer. That one is written
down in `LOOSE_ENDS.md`, where a sentence can say which number matters.

**The fault worth looking for is one level up from a duplicate.** The
2026-09-12 audit removed a fair amount of copied code and almost none of it
could have hurt anybody: copied arithmetic does not drift. What it found six
times was a *guard* that had come off a value written twice, and every one of
those values was still correct when it was found -- so the tests were green, the
program was right, and nothing would have reported the drift on the day it
happened. In order:

1. `slint_fader_taper_matches_the_rust_breakpoints` checked two lists in
   `GainMath` that nothing read. The taper the faders run spelled its
   breakpoints inline and no test evaluated it.
2. `musical_divisions_match_the_snap_table_in_main_slint` compared its table
   with a literal copy written inside the test, and never opened `main.slint`.
3. `Divisions.beats` mirrored `ModTimeDivision::beats` -- twenty-one values,
   after a factor-of-four disagreement that the table exists to prevent -- with
   no test at all.
4. Aux In's descriptor default held a hand-evaluated `reference_level_gain()`
   pinned to nothing, while the sampler's identical literal had a test written
   for exactly that reason.
5. Four effect faces were outside `slint_face_agreement.rs`, whose list had
   simply never been extended to them.
6. Nothing checked any stepped parameter's *positions*, only its default --
   which is how one EQ id came to decode to two enums of different arity.

The question that found all six is short: **does anything read the copy the test
checks?** Ask it of any mirrored value, and ask it before believing a green
suite. `scripts/dupe-audit one-sided-test` automates the one of the six a search
can reach; the rest need the question asked by hand.

A seventh check, `bar-arithmetic`, was added 2026-09-15 with
`docs/plans/archive/musical-time/`, and its lesson is about *when* a check gets
written. The plan's own mutation table opens with "run it before the fix or it
is decoration", so it was written against the unfixed tree -- and it reported
**ten** sites where the survey that commissioned it had found eight. Two
places set `beats_per_bar` to a literal `4` and nobody had noticed either. A
check written after its fix is shaped to report what its author already knows
about; a check written before it is shaped by the tree.

It is also narrower than the plan asked for, and that is recorded in its
docstring rather than quietly. The plan wanted `% 4` and `/ 4` swept on any
line mentioning `bar`, `beat` or `tick`; beat and tick catch
`MidiMessage::song_position_ticks`, whose `ppq / 4` is the MIDI spec's
sixteenth-note beat and must *not* move if mooloop's grid does. **One
permanent false positive is worse than a narrow check**, because a check that
is never clean stops being read.

An eighth check, `popup-close-order`, was added 2026-09-20 with MOO-53, and it
is not looking for a duplicate at all -- it looks for a **sequence**. Closing a
`PopupWindow` tears down the repeater item whose handler is still running, so a
callback written after the `close()` never lands: the menu opens, draws
correctly, and does nothing. That has now been found four separate times --
`BusPicker`, the rack's preset menu, `PickerChip`'s `MenuField`, and the
channel rack's add-channel menu, which shipped High as MOO-53 -- and each was
diagnosed from scratch, because nothing in the tree remembered the previous
three.

It is in this file because it is the same *lesson* as the rest, arriving as an
order rather than as a copy: what fails is invisible to every test that does
not click. `source_kind_menu.rs` held the add-channel menu's eight labels
against `DeviceKind::label()` and was **entirely correct** the whole time the
menu added no channel -- the rows were built from the right list, in the right
order, and sent the right index to a call that was never reached. Ask of a
control's test not only *does anything read the copy this checks*, but **does
anything here press the button**.

Two things about how it reports. It was written against the unfixed tree, where
it finds MOO-53 along with the four other sites; and it deliberately reports
close-first rows whose lists are written out rather than repeated, which are
believed to work. Those are one refactor away from being the defect, and that
refactor is exactly what happened here -- the add-channel menu was four
hand-written rows, correct, until the day it became a `for` over eight. So its
hits are leads that need the rows read, not a list of bugs.

There is a third limit, and it is the one to keep in mind when a check comes
back clean. **`repeated-line` matches bytes, so a rename hides a copy from it
completely.** On 2026-09-12 it reported the effect event-splitting loop in ten
files; there were twelve. `modulation.rs` had spelled the loop variables
`position`/`offset` instead of `pos`/`off`, and `eq.rs` had put its `if let` on
one line. Both were the same ten lines doing the same thing, and neither was
visible to a text search. A clean `repeated-line` is evidence that nothing was
copied *verbatim*, which is weaker than it looks: the copy most likely to
diverge is the one somebody edited on the way past.

What found those two was reading every `fn process` in the module and asking
which ones split their own block. That does not generalise into a search, but
it does generalise into a habit: when a trait has one required method and a
dozen implementors, read all twelve before believing they differ.

Copied arithmetic is mostly waste. Copied numbers and copied policies are
what diverge silently, and they are what the checks are aimed at.

## Parameter identity across the session boundary

Moving session editing into `mooloop-session` left two integer address spaces
on opposite sides of a crate boundary. They are not interchangeable:

- `EffectSlotRow.pN`, modulation arrays, automation lanes and engine events
  use a parameter's stable descriptor **id**.
- `Session::set_effect_param(slot, param_index, normalized)` is a face-editing
  API and takes the parameter's **position in the kind's descriptor table**.

Most effect tables originally had `id == position`, which hid the distinction.
Buffer's retired ids made the table sparse on 2026-09-16: forwarding its ids
through the session API made JUMP operate Reverse, REV and STUT do nothing,
and QUANT operate Stutter while the DSP and session tests remained green.

At this boundary, name an integer `id` or `param_index` according to what it
is; never use one as the other because the values happen to agree. Derive a
conversion from `EffectKind::descriptors()` rather than spelling another map.
A regression test for face wiring must read the production `.slint` callback
and compare it with that table -- a test that calls the session or DSP directly
does not cross the boundary that failed. When changing or extracting an API,
audit every plain integer it accepts for this kind of lost semantic type.

## Documentation is part of the change

`docs/CURRENT.md` describes the application as it exists. A change that adds,
removes, or alters user-visible behavior updates it in the same commit; so
does a change that invalidates a fact stated in any other document. Leaving
a document to be corrected later is how it stops being trusted.

## Slint

This project pins Slint `1.17.1`. Before editing `.slint`, `slint::` Rust API,
or `slint-build`, consult the version-matched documentation:
`https://releases.slint.dev/1.17.1/docs/slint/`, and
`https://docs.rs/i-slint-backend-testing/1.17.1/` for the `ElementHandle` API
the UI tests and the MCP server both drive. If the pinned version changes, use
the matching release URL instead of relying on latest-version knowledge.

To see what the real interface does rather than what the source implies, run
`scripts/mooloop-mcp`: it starts the application with Slint's embedded MCP
server, whose tools read the live element tree and click, type, drag, and
screenshot it. `docs/OPERATIONS.md` has the details.

Do not reach for `cargo build` to find out whether a `.slint` edit is valid or
what it looks like. `scripts/slint-sketch` type-checks a scratch `.slint`
against the real widgets in about 0.05s and screenshots it in about 0.2s,
where `cargo build -p mooloop-ui` is about four minutes for any edit at all.
Iterate there, then build once.

**It also takes `ui/main.slint` itself**, which is not obvious from the name
and is worth knowing before any structural edit: `scripts/slint-sketch
crates/mooloop-ui/ui/main.slint` type-checks the whole real window in about
2.7s, and `--shot` renders it in about 3s with empty models -- enough to see
the layout, the chrome, the toolbar and the status bar, though not anything
driven by a Rust-supplied model. For a change that moves markup around rather
than one that needs live data, that is the check-and-look loop, at roughly a
hundredth of a `mooloop-ui` build. Found 2026-09-08, restructuring the work
area. It needs `slint-viewer` installed locally; it
is deliberately not a workspace dependency, and the build never refers to it.
See `docs/OPERATIONS.md`.

## Verification and operations

Do not run Cargo commands concurrently. Read
[docs/OPERATIONS.md](docs/OPERATIONS.md) before running Cargo, UI
snapshots, or the live application; it contains this machine's memory limits
and rendering procedures.

### The verification ladder

Sixty-one percent of Adam's working time goes on `cargo`, and almost all of it
on workspace-wide runs reached for out of caution rather than need. **Stay on
the lowest rung that can see the thing you just changed, and climb only when
you are about to stop.**

| Rung | Command | Cost | Use it |
| --- | --- | --- | --- |
| 1 | `cargo check -p <crate>` | ~1 s | after every edit |
| 2 | `cargo test -p <crate>` | 4 s laptop / 18 s box | after every edit that changes behaviour |
| 3 | `cargo test --workspace --exclude mooloop-ui` | 43 s | before handing work over |
| 4 | `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` | 118 s + ~88 s | before committing, and nothing smaller than a milestone |

- **`cargo test --workspace` is a commit-time command, not an iteration-time
  command.** One measured session spent 104 minutes on twenty-one of them,
  and sixty-one of its sixty-five test runs passed. The time went on
  confirming nothing had broken, not on finding anything.
- **Never run rung 4 to find out whether something compiles.** That is
  rung 1, and it is a hundred times cheaper.
- **Iterate on the laptop, verify on the box.** Rungs 1 and 2 on a single
  small crate are eightfold faster locally. Rungs 3 and 4, and anything
  touching `mooloop-ui`, need memory the laptop does not have -- send them to
  the remote build box with `scripts/antibox`, which picks incremental
  compilation for dev builds and sccache for release builds on its own.
- **A cloud container is neither machine.** An agent session on the web
  runner starts with no `mold`, no JACK or font headers, and an empty
  `target/`, so the build fails before it compiles anything and the error
  names the wrong cause. `docs/OPERATIONS.md`'s *Working In A Cloud
  Container* has the two environment settings that fix it, and what a run
  there costs.
- **`.slint` edits do not need a build at all.** `scripts/slint-sketch`
  type-checks against the real widgets in about 0.05 s, where a
  `mooloop-ui` build is minutes.

Rung 3 earns its place because `mooloop-ui`'s seven test binaries are most of
what a workspace run compiles and links; excluding them is 70% off.

### Background anything above rung 2

**This is the single largest difference between a fast session and a slow
one, and it is free.** Across nineteen measured sessions, the ones that spent
under 40% of their time blocked on `cargo` had backgrounded 76% of their
compiler time; the ones over 60% had backgrounded 5%. Correlation -0.73. The
good sessions did not have faster builds -- they had builds that were not
being watched.

So run rungs 3 and 4 with `run_in_background: true` and keep working; the
harness notifies on exit. Do not poll the output file -- a run piped through
`tail` writes nothing until it finishes, so polling reads an empty file and
learns only that time has passed.

The exception is when the very next thing you do depends on the result and
there is nothing else to usefully do. That is rarer than it feels: there is
almost always another file to read, another edit to prepare, or a measurement
to take.

### Run clippy the way CI runs it, or you are not running it

**`-- -D warnings`.** `.github/workflows/ci.yml` denies warnings; a bare
`cargo clippy` does not, and the difference is the whole result. On
2026-09-08 a `field_reassign_with_default` in a `mooloop-session` test was
noticed during a release, checked with a bare `cargo clippy`, seen to exit 0,
and written into `LOOSE_ENDS.md` as a tolerated warning worth fixing
sometime. It had been failing CI on `main` on every push for two days.

An exit code only answers the question you asked. Ask CI's.

### Never read a piped run's exit code

**A command piped into `tail` reports `tail`'s exit status, not cargo's.**
`cargo test ... | tail -40` exits 0 when every test failed, because `tail`
succeeded at printing the failures. This is the sharper half of the warning
above: an empty output file at least *looks* uninformative, so it gets
re-read. A `0` looks like an answer, so it gets believed and reported on.

This is not hypothetical. On 2026-09-07 a `mooloop-ui` suite was reported
green on a piped exit code and had to be re-run before anyone could say
whether it was. It was -- which is the point: the claim was true and had not
been checked, and the same reading would have been made either way.

**Run it through `scripts/exit-code` and the question cannot be got wrong.**
It sends the output to a log, captures `$?` the instant the command returns,
prints the tail of the log and any failure lines in it, and exits with that
same status -- so the harness's own `[exited with code N]` is the command's too:

```sh
scripts/exit-code cargo test -p mooloop-ui
scripts/exit-code cargo clippy --workspace --all-targets -- -D warnings
scripts/exit-code --tail 100 cargo test --workspace --exclude mooloop-ui
```

Background it exactly as you would background the bare command; the log path
is printed before the run starts.

By hand it is a redirect, and nothing at all between the run and the read:

```sh
cargo test -p mooloop-ui > /tmp/t.log 2>&1; echo "EXIT: $?"
grep -E '^test result' /tmp/t.log
```

The pipe is only half of it. The other half is that `$?` is whatever ran
*last*, so an `echo`, a `cd`, a `grep`, or a second cargo command written
between the run and the read answers in its place -- and an `echo` always
succeeded. Reading the `test result:` lines or clippy's own summary is just as
good and needs nothing remembered. What is never good enough is a
`[exited with code 0]` from a run with a pipe anywhere in it.

**Read those lines for failures; do not do arithmetic on them.** A workspace
run's test binaries write the same stream in parallel and occasionally
interleave, so a line can come out as

```
test result: ok     Running unittests src/lib.rs (.../mooloop_session-940dc...)
```

-- one binary's result with another's progress line spliced through it, and its
`129 passed` gone. On 2026-09-12 that made a summed total read 1404 where the
suite had run 1533, and the missing 129 looked exactly like a crate whose tests
had stopped existing, which is a real failure this file warns about elsewhere.
Ten minutes went on establishing that nothing was wrong.

So: grep for `test result: FAILED`, for `panicked at`, and for a non-zero count
in `N failed`. Those are robust against a splice because a spliced line cannot
invent a failure. A *sum* is not robust, and the exit code is better than a sum
-- this run was redirected rather than piped, so its `0` was cargo's own and was
right while the arithmetic was wrong.

`scripts/exit-code` greps the log for exactly those three markers, so its
report is splice-proof for the same reason.

`set -o pipefail` fixes the pipe half too, but only where a script owns the
whole shell; it is not on by default in the harness's shell, so do not assume
it.

### Never search for a process with the pattern you are typing

**`pgrep -f mooloop` matches the shell that is running `pgrep -f mooloop`.**
The harness runs each command as `zsh -c "<the whole command>"`, so the
pattern is sitting in an ancestor's `/proc/<pid>/cmdline` and `pgrep -f` finds
it there. Demonstrated on 2026-09-15 with a string that matched nothing on the
machine: `pgrep -af zzz-selftest-pattern` printed a process and exited 0. The
process was the harness's own shell.

Two failures come out of that, and the second is not recoverable. A search
reports the application running when it is not, so a stale window is chased
that does not exist -- and `pkill -f` on the same pattern kills the shell the
agent is running in, which ends the session. `ps aux | grep mooloop` has the
identical fault plus a piped exit code.

So use `scripts/procs`, which scans `/proc` from bash builtins and skips
everything in its own process tree by pid rather than by a pattern that has to
be written correctly each time:

```sh
scripts/procs mooloop                 # list; exit 1 when nothing matches
scripts/procs --kill mooloop          # SIGTERM the matches, report survivors
scripts/procs --kill --force mooloop  # SIGKILL whatever ignored SIGTERM
```

`scripts/procs --threads data-loop` matches thread names instead, and prints
each thread's scheduling policy and realtime priority -- the rtkit check in
`docs/OPERATIONS.md`, which was written as `chrt -p $(pgrep -f data-loop)` and
could never have worked, because a thread name appears in no command line and
the only thing `pgrep -f` could match was the shell asking.

It stays inside your own uid unless given `--any-user`, so a broad pattern
cannot reach a system daemon, and it refuses to `--kill` on a pattern shorter
than three characters. The `[m]ooloop` character-class trick works too, and is
forgotten under load; that is what makes it the wrong control.

### Order device work so the face contract comes last

A device has three parts and they differ by four orders of magnitude:

| Part | Cost to see it | Where |
| --- | --- | --- |
| DSP and engine code | 1-4 s | laptop |
| Face markup, visual only | 0.05 s via `scripts/slint-sketch` | laptop |
| The face *contract* — a new property or callback crossing `main.slint` and `lib.rs` | 30 s to check, 8.7 min to a release binary | box |

So get the DSP right against unit tests, iterate the face visually with
`slint-sketch`, and cross into `main.slint` **once**, with every new property
and callback batched into that one pass. The eight-minute build belongs once
per device feature, not once per knob.
