# 06 — Test the session layer

Read `00-status.md` first. This step is the reason for the other five.

Until now the plan has produced no capability. This is where it pays: every edit
the application performs becomes callable without a window, so it becomes
testable for the first time.

## What to cover, in priority order

**1. Structural retargeting.** The highest-value tests in the plan, because this
is where a real bug already lived. `docs/FOCUS.md` records that routes and
automation lanes named their destination by slot and their channel by index, so
any structural edit silently re-aimed them; `mooloop_core::structure` now states
each edit once as a permutation. What has never been tested is that the *session*
runs that permutation over everything holding a position.

Assert that after a channel reorder, a channel delete, an effect slot reorder,
and an effect removal:

- modulation routes still name the destination they named before,
- automation lanes still address the same parameter,
- the piano roll's selection still refers to the same notes,
- `effect_target` still points at something that exists.

**2. Undo.** `History<ProjectSnapshot>` with gesture coalescing has never had a
test. Assert that a drag stamped with one gesture token collapses to one entry,
that two drags separated by a release do not, that undo then redo is the
identity, and that a new edit truncates the redo branch.

**3. Document round-trip.** Open, edit, save, reopen, and compare the session.
`mooloop-project` tests the format; nothing tests that the session puts the same
thing back in.

**4. Command ordering.** Step 05's `tick` drains one ordered stream. Assert that
a structural install followed by a parameter change for the same slot reaches
the engine in that order, since the reverse would apply a value to a node that
does not exist yet.

**5. Note editing.** Selection, marquee combination, scale-about-a-tick, and
clipboard paste-as-phrase. These are pure functions over plain data now and are
cheap to cover.

## What not to do

**Do not test projection.** The `sync_*` functions push plain data into models;
asserting they do is testing Slint. UI appearance is covered by snapshots, per
`docs/AGENT_OPERATIONS.md`.

**Do not test the engine from here.** It has its own tests, including
`gain_structure_tests.rs` and the sequencer's drift coverage. A session test
that boots an engine is a slow integration test wearing a unit test's clothes;
if `tick` needs an engine, put a seam in front of it instead.

## Definition of done

`cargo test -p mooloop-session` covers the five areas above and runs fast enough
that it is worth running on every change — which, for a crate with no audio
device and no window, should mean under a second.

## Then

Move this directory to `docs/plans/archive/` and update
`docs/plans/README.md`. The egui plan becomes available at that point and not
before.
