# Plan: Musical time has one spelling

A run at the duplication fault, following
`docs/workflows/rust-slint-boundary/`. It came out of
`buffer-implementation/03-freeze-and-the-grid.md`, whose step 5 wants
bars:beats:ticks readouts on the Buffer face and found that there is no such
thing to reuse — only pieces of one, in three crates and a markup file.

Nothing on screen should change when this lands. **If something does, the fix
is wrong.**

## Status

| Step | State |
|---|---|
| `01-one-bar-one-home.md` | Landed 2026-09-15 |
| `02-position-and-duration.md` | Landed 2026-09-15 |
| `03-the-guard.md` | Landed 2026-09-15 |

### What the doing changed about the plan

**The guard was written first, not last.** Step 03's mutation table opens with
"run it *before* the fix or it is decoration", and the only way to do that
honestly was to write `dupe-audit bar-arithmetic` against the unfixed tree.
It reported **ten** sites where the survey had found eight: `session.rs:475`
and `project.rs:896` both set `beats_per_bar` to a literal `4` and neither was
in the table above. A check written after its fix would have been shaped to
report exactly what the author already knew about.

**The check is narrower than step 03 specified, for a reason worth keeping.**
It asked for `% 4` and `/ 4` on any line mentioning `bar`, `beat` *or* `tick`.
Beat and tick sweep in `MidiMessage::song_position_ticks`, whose `ppq / 4` is
the MIDI spec's sixteenth-note beat -- a different four, which must not move if
mooloop's grid does. One permanent false positive is worse than a narrow
check, because a check that is never clean stops being read. The docstring
says so.

**Every row of step 03's mutation table was run and its message read**, and
two rows needed more than the table said.

- "`STEPS_PER_BAR` back to a literal `16`" **passes on its own**, because 16
  is still the right answer -- check A asserts the *relationship*, so it only
  fires once the copies have actually parted. Moving `BEATS_PER_BAR` to 3 as
  well is what makes it fail (`left: 16, right: 12`). The literal alone is
  caught by `dupe-audit`, not by the test, and that division of labour is the
  point: the search finds the copy, the test finds the drift.
- The same is true of `frames_per_bar` back to `240.0`: `dupe-audit` reports
  `sampler.rs`, and check A only fails once the constant has moved
  (`left: 96000, right: 72000`).

The rest fired as written: the tripwire reported `0` against `148` when B's
sweep was emptied rather than passing in silence; a 1-based `BbtDuration`
printed `(2, 1, 0)` for one bar; a 0-based `BbtPosition` failed in `time.rs`
*and* in `session/engine.rs`, which is the session test the plan predicted;
`BbtText` with a `.` separator failed check D against `Display`; and removing
its `out property` failed to compile with *"Element 'BbtText' does not have a
property 'formatted'"*, which the test's own comment says is the check working.

**`transport::beat_in_bar` stopped wrapping negative positions**, and that is
a deliberate behaviour change on an input nothing can produce: `seek` refuses a
negative tick and advancing only adds, so the old `rem_euclid` was defending
against a position the engine cannot reach. `BbtPosition` counts from zero, so
the shared derivation floors instead. The test says which case is which.

## What the survey found

Stage 01 of the workflow says to size a sweep from the tree and never from the
note. The note said three spellings. The tree says nine.

**"Four beats to the bar", spelled nine times:**

| Where | Spelling |
|---|---|
| `mooloop-core/src/time.rs:50` | `Ticks::beat_in_bar` → `% 4`, *"assumes 4/4 for now"* |
| `mooloop-core/src/pattern.rs:18` | `STEPS_PER_BEAT: u16 = 4` |
| `mooloop-core/src/playlist.rs:8` | `STEPS_PER_BAR: u32 = 16` — a literal, not `STEPS_PER_BEAT * 4` |
| `mooloop-core/src/sampler.rs:379` | `frames_per_bar` → `240.0` |
| `mooloop-dsp/src/buffer_device.rs:119` | its own `240.0` |
| `mooloop-engine/src/render.rs:2630` | `const BEATS_PER_BAR: f32 = 4.0` |
| `mooloop-engine/src/transport.rs:99` | `beat_in_bar` → `rem_euclid(4)`, *"Assumes 4/4 for now"* |
| `mooloop-session/src/engine.rs:760` | test: `TICKS_PER_BAR / 4` |
| `mooloop-core/src/project.rs:743` | `beats_per_bar: u8` — persisted, and read by nothing |

Two tells worth keeping. The comment *"assumes 4/4 for now"* appears in three
crates, which is three people independently deciding the same thing and none
of them finding the others. And `frames_per_bar`'s doc comment says it uses
*"the convention the buffer device already uses"* — while the buffer device
spells its own `240.0` rather than calling it. **A prose comment is doing the
job the compiler should be doing.**

**`project.beats_per_bar` is the ninth and the worst.** It is a persisted
field, it round-trips, `integrity.rs:526` repairs it with the message *"set
the timebase to 4 beats per bar"* — and nothing else in the workspace reads it.
The project format promises a time signature the code cannot honour.

**Two independent derivations feed the same window:**

- `engine/transport.rs:97` `beat_in_bar()` → `RenderReport` → `executor` →
  `bridge` → `mooloop-ui`'s `set_beat_in_bar`.
- `session/engine.rs:706` `transport_position()` → `TransportPosition {bar,
  beat, tick}` → `PositionReadout`.

Both compute "which beat are we on" by hand, in different crates, from the
same clock. They agree today because both are hardcoded to the same 4.

**One type exists that would have prevented all of it, and nothing calls it.**
`mooloop-core::time::Ticks` has `beat()`, `beat_in_bar()` and `within_beat()`.
`Ppq` and `ticks_per_sample` have readers; **`Ticks` has none outside its own
tests.** This is the `LFO_DESCRIPTORS` shape from `01-find.md` — a table with
no reader, while everyone hand-rolls what it does.

**Formatting is spelled once, and is about to be twice.**
`toolbar.slint:483`'s `PositionReadout` owns the `pad` + join that produces
`001:1:000`. The Buffer face would be the second copy.

**No duration formatter exists at all.** `session/engine.rs:752` already names
the trap in a test comment — *"one-based where the readout shows them and
zero-based where it does not, which is easy to get backwards"* — so the next
person to format a one-bar length as `2:1:0` will have been warned by a
comment and nothing else.

## The judgement

Stage 02's three questions.

**Is it still true?** Verified against the tree, not the note. Yes, all nine.

**Is the copy real?** For the derivations and the numbers, yes. For the
*arithmetic*, `02-judge.md` would normally say no — copied arithmetic over a
constant that cannot change does not drift. The exception here is that
`beats_per_bar` is a persisted project field, so this is a constant somebody
has already declared an intention to change. It is the case the workflow calls
"correct today, and nothing would report it going wrong".

**Is it worth a change?** Bounded, yes. One home for the convention, one type
for the two readouts, one guard. **Not actual 5/4 support.** `sampler.rs:375`
records why the audio path hardcoded it — *"`beats_per_bar` is project metadata
the audio thread is not given"* — and that reason still holds. Per `03-fix.md`
shape C: fix the thing that diverged, write down the thing that did not.

## Where this sits

It is a prerequisite for `buffer-implementation/03`'s step 5, and it should be
its own run rather than folded into that step: it touches six crates, and a
Buffer step carrying a six-crate refactor is a diff nobody can review.

Small enough to take before the Buffer work or between its steps. It is not in
`FOCUS.md`'s sequence and does not need to be — `FOCUS.md`'s own rule covers
it, as a fix that unblocks the active step.

## Out of scope

- **Real time-signature support.** Threading a signature to the audio thread,
  bars of other lengths, a signature change mid-song. This plan makes 4/4 a
  single named fact instead of nine anonymous ones, which is what a later
  signature change would need first.
- **Deleting `project.beats_per_bar`.** It is harmless, it round-trips, and
  removing a persisted field is a format break for no gain. Step 1 makes it
  honest instead.
- **The roll's grid constants.** `RollMetrics` in `piano-grid.slint` and
  `roll_metrics.rs` already solved that half, and they are the model this plan
  copies rather than something it revisits.
