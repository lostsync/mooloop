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
| `01-one-bar-one-home.md` | Not started |
| `02-position-and-duration.md` | Not started |
| `03-the-guard.md` | Not started |

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
