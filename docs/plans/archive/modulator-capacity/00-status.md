# Modulator capacity plan status

Eight modules per channel is fine to play with. This plan is about the
number being *allowed to move* — so raising it is a constant edit and an
argument about memory, never a hunt through layout code.

`docs/plans/archive/modulator-modules/` is complete and this does not reopen it.
Nothing here changes what a module is, how routes are addressed, or the
32-frame control contract. Durable `ModSourceId` already landed, so slot
numbers are an implementation detail rather than something a saved project
depends on; that is the precondition this plan builds on.

## What it costs today, measured

Modulator capacity itself is cheap, and grows linearly:

| Slots | Command ring | DSP racks | Control outputs | Meters |
| --- | --- | --- | --- | --- |
| 8 | 936 KiB | 200 KiB | 2048 KiB | 8 KiB |
| 16 | 1544 KiB | 400 KiB | 4096 KiB | 16 KiB |
| 32 | 2760 KiB | 800 KiB | 8192 KiB | 32 KiB |

Step 03 has since taken the command-ring column out of that table
altogether. Modulation edits name one fact each, so the ring is 136 KiB at
any capacity — the width is set by a synth's parameter block now, not by
anything modulation ships.

**A first version of this plan stopped there and drew the wrong
conclusion.** Measuring only the modulator arrays put the graph's total at
3.1 MiB and named control outputs as the biggest line. Measuring the whole
render graph, which step 02 now pins in a test, says otherwise:

| Per channel | × 256 channels |
| --- | --- |
| `ChannelStrip` 150,904 B | **37.7 MiB** |
| `EventList` 10,248 B | 2.5 MiB |
| `ControlOutputs` 8,192 B | 2.0 MiB |
| `ModRack` + `ModulatorRack` 1,732 B | 433 KiB |
| **171,076 B** | **42.8 MiB** |

That table is what it was before the fix. Boxing the effect slot state
(step 04) took the graph to 11.6 MiB, and materializing channels from the
project (step 02) took a sixteen-channel project to **1.1 MiB** — 439 KiB
reserved plus 45 KiB a channel. Both ceilings are untouched; the graph
simply stopped paying for them in advance.

Modulation is one percent of it. The strip is 88%, and inside the strip
the weight is `EffectChain` at 140 KiB — because `MAX_CHANNELS` and
`MAX_EFFECTS_PER_CHANNEL` are both the u8 index space, so the graph
reserves the *product*: 65,536 effect slots, each with a 320-byte pending
queue, for a project that will populate a few dozen.

So the conclusion survives in a stronger form than it was first written.
Capacity is not expensive; **dimensioning by ceilings is**, and it is
expensive whether or not the modulator count ever moves. The lesson worth
keeping is that the number was invisible at every individual definition
and only appeared when they were multiplied — which is why step 02 leaves
a test behind rather than a paragraph.

## Order

1. `01-capacity-is-a-constant.md` — remove the layout and control
   assumptions that quietly cap the number, so raising it is one edit.
   No memory change, no protocol change. **Landed 2026-09-01** on
   `spike/mod-capacity`: grid rows follow capacity and scroll, the math
   input jack became a name-carrying picker, and
   `the_module_grid_scales_with_capacity_alone` renders the shelf at eight
   and sixteen so a re-introduced literal fails a test.
2. `04-lazy-effect-slots.md` — stop reserving every addressable effect
   slot's state up front. **Landed 2026-09-01**: 42.8 MiB → 11.6 MiB with
   both ceilings untouched.
3. `02-size-by-what-exists.md` — materialize channels from the project
   instead of reserving 256 of everything. **Landed 2026-09-01**: shape A,
   11.6 MiB → 1.1 MiB at sixteen channels.
4. `03-per-slot-commands.md` — stop shipping the whole rack by value on
   every edit, so the ring stops growing with capacity at all.
   **Landed 2026-09-01**: `EngineCommand` 936 → 136 bytes, so the ring is
   136 KiB rather than 936 KiB and does not move when capacity does.

Every step has landed. Three departures from what step 03 was written
expecting:

- **The wide command was deleted rather than kept.** The plan reserved
  `SetChannelModulation` for project load, presets and undo. All three
  already rebuild the renderer through `EngineHandle::install_project`, so
  once the gestures were narrowed the variant had no caller — and keeping a
  dead wide variant would have held the ring at 936 bytes, which is the
  whole thing this step was for. A future channel-preset verb belongs on
  the structural ring, which may box because it has a reclaim path.
- **A sixth command, `MoveModulator`.** The plan listed five. Reordering
  the grid is a real gesture that would otherwise have been the one thing
  still forcing a whole-rack send; both racks run the same permutation, so
  routes and a math module's input slot cross the move on either side.
- **The incremental restore was the existing diff, not new machinery.**
  Every narrow arm runs through one `edit_modulation` helper that keeps the
  before-and-after rack and hands back any destination that lost its last
  route. It also reaches `restore_base_param`, which the whole-rack path
  never did — so a route dropped from a *generator* parameter now restores
  its base, where before only effect parameters did.

Capacity is now free of memory arithmetic at every layer that used to carry
it. Raising `MAX_MODULATORS_PER_CHANNEL` costs the DSP racks, the control
outputs and the meters, all linear and all small; nothing else moves.

## What this plan refuses to do

- No dynamic per-channel slot counts. A channel's capacity stays a
  compile-time constant, because a variable-length rack on the realtime
  path buys a bounds check on every tick and an allocation story on every
  edit, to save memory that step 02 recovers without either.
- No heap-allocated modules. `modulator.rs` is deliberately inline: no
  kind allocates, so the effect chain's install/reclaim machinery would
  add a `Box` drop to the path of every rack edit and buy nothing.
- No raising the number as part of this plan. The point is that raising
  it becomes a one-line decision with a measured price attached, not that
  it happens now.
