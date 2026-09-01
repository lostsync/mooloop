# Modulator capacity plan status

Eight modules per channel is fine to play with. This plan is about the
number being *allowed to move* — so raising it is a constant edit and an
argument about memory, never a hunt through layout code.

`docs/plans/modulator-modules/` is complete and this does not reopen it.
Nothing here changes what a module is, how routes are addressed, or the
32-frame control contract. Durable `ModSourceId` already landed, so slot
numbers are an implementation detail rather than something a saved project
depends on; that is the precondition this plan builds on.

## What it costs today, measured

Every number below is `MAX_CHANNELS = 256` multiplied by a per-channel
array, preallocated at startup. Measured on 2026-09-01 at three capacities:

| Slots | Command ring | DSP racks | Control outputs | Meters | Total |
| --- | --- | --- | --- | --- | --- |
| 8 | 936 KiB | 200 KiB | 2048 KiB | 8 KiB | **3.1 MiB** |
| 16 | 1544 KiB | 400 KiB | 4096 KiB | 16 KiB | **5.9 MiB** |
| 32 | 2760 KiB | 800 KiB | 8192 KiB | 32 KiB | **11.5 MiB** |

The surprise is where the weight sits. The command ring is the thing
`bridge.rs` warns about and the thing step 03 pinned a test to, but it is
not the biggest line: **control outputs are**, at roughly twice the ring.
`ControlOutputs` is `[[f32; slots]; 256 control ticks]` per channel, held
for 256 channels — a full block's worth of resolved control signal for
every channel a project could ever have, whether or not it exists.

So the honest reading is that capacity is not expensive; **dimensioning
everything at `MAX_CHANNELS` is**. A project with sixteen channels pays
for two hundred and forty it does not have, at every capacity.

## Order

1. `01-capacity-is-a-constant.md` — remove the layout and control
   assumptions that quietly cap the number, so raising it is one edit.
   No memory change, no protocol change. **Landed 2026-09-01** on
   `spike/mod-capacity`: grid rows follow capacity and scroll, the math
   input jack became a name-carrying picker, and
   `the_module_grid_scales_with_capacity_alone` renders the shelf at eight
   and sixteen so a re-introduced literal fails a test.
2. `02-size-by-what-exists.md` — dimension the per-channel engine arrays
   by the live channel count rather than by `MAX_CHANNELS`. This is where
   the memory actually is, and it pays off at every capacity.
3. `03-per-slot-commands.md` — stop shipping the whole rack by value on
   every edit, so the ring stops growing with capacity at all.

Steps 2 and 3 are independent of each other and of step 1. Step 1 is the
one that has to land before the number moves; 2 and 3 are what make it
cheap to keep moving.

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
