# Plan: The channel buffer device

A retained-audio device that is always recording the last N bars, and a set of
controls for turning that history into an instrument. `docs/BUFFER_ENGINE.md`
is the hypothesis; the numbered files here are the work order and win where
they disagree with it.

Added 2026-09-16, late: the plan ran for weeks without one of these, which is
why `FOCUS.md` had been carrying its state.

## Status

| Step | State |
|---|---|
| `01-the-whole-thing.md` | Landed. Acceptance test 8 closed 2026-09-16 |
| `02-control-and-modulation.md` | Landed |
| `03-freeze-and-the-grid.md` | In progress — see the build order below |

`03`'s build order has six steps and an "alongside".

| `03` step | State |
|---|---|
| 1. Rate, and a head that runs without a writer | Landed 2026-09-16 |
| 2. Freeze | Landed 2026-09-16 |
| 3. Position replaces Offset | Landed 2026-09-16 |
| 4. Length, Loop and Jump on the shared grid | Landed 2026-09-16 |
| 5. Quantized freeze, and BBT everywhere | Landed 2026-09-16 |
| 6. The 2U face | Not started |
| Alongside: acceptance test 8 | Closed for the Buffer operations 2026-09-16 |

`musical-time/`, which step 5 waits on, landed 2026-09-15 and is in
`archive/`. `BbtDuration` ships with no caller; step 5 is it.

## What the doing has changed about the plan

**Steps 1 and 2 landed as one commit.** The document says so itself —
*"Freeze and Rate are therefore one decision, not two. Do not take one without
the other"* — and the reason survives contact: with only step 1, `Rate` is a
descriptor the lane picker lists and nothing in the running program can make
audible, because nothing detaches a free-running head until Freeze does. A
control that lists and does nothing is the defect `ui-consistency-pass/` spent
six steps removing.

**The clock and the writer were the same number, and Freeze separated them.**
`expires_at` counted against `write_head`. Stop the writer during a `Steps(n)`
gesture and it would have repeated forever. `frames_elapsed` is the clock now.
Nothing in the plan predicted this; it is what "the writer was the time base"
means in practice, one layer below where the document says it.

**A tempo change would have destroyed frozen audio.** The plan says to lock
HISTORY while frozen and gives `bars` as the reason. `bars` has no control, so
that read as theoretical — but `resize_buffers` fires on an ordinary tempo
change and rebuilds the ring. `AudioNode::holds_frozen_audio` lets the chain
refuse the swap, down the same reclaim path a mismatched kind already takes.
**The trigger was the common case, not the documented one.**

**"Release the chase once it arrives" needed a second condition, and finding
it took a measurement.** The plan says a static Position hands the head to
`Rate`; release on arrival alone does that, and it also ruins a *sweep* --
during a slow one the head is always within a frame of the target, so it
released on every tick, free-ran past, and was dragged back. Measured at +1.00
alternating with -0.07 every 32 frames, which is a warble rather than a scrub.
The rule that works is arrival **plus stillness**: the request has to have
stopped moving for longer than the chase's own time constant. Arrival is an
audio-thread fact and stillness is a control-plane one, which is why one
condition could not do both.

Related, and the same shape: **the chase target has to travel with the
writer.** `Scrub::offset_frames` did that before this step and the comment
beside it said why; taking it out to make the target absolute reintroduced the
warble it was written to prevent. Frozen, the write head is static and the same
expression is absolute anyway -- so one mechanism covers both states, which is
what "Freeze latches what *now* means" turns out to mean in code.

**`Freeze` persists and the frozen audio does not.** A project saved frozen
reopens frozen over an empty ring. That is the honest consequence of "persisting
frozen content is out of scope", and it is written into `BufferParams`'s doc
comment and `CURRENT.md` rather than left to be discovered.

**Most of step 5's "BBT everywhere" had already landed with
`musical-time/`.** `PositionReadout` draws a `BbtText`, `transport_position`
goes through `BbtPosition`, and the duration/position distinction has its
test. What was left was the *caller*: a one-bar Length reads `1:0:0`, which is
`BbtDuration`'s first use anywhere and the reason there are two types. Printed
through `BbtPosition` the same bar reads `2:1:0`.

**A latching parameter is not a trigger, and the arm/cancel rule got that
wrong first.** "A second press cancels the armed freeze" was written as "a
repeated request cancels", which is right for a button and wrong for a
`Freeze` that a lane writes every control tick: it armed, cancelled, armed and
cancelled, and would never have landed. The rule is that *the opposite*
request cancels and a repeat is a held value. **The test named for the cancel
passed on the broken version**, because it pressed twice by writing 1.0 twice
-- which is exactly what a lane does and exactly what must not cancel.

**The face is three steps behind the table, on purpose.** `Rate`, `Freeze`,
`Length`, `Loop` and `Jump` are all published, automatable and tested, and
none of them has a knob. Step 6 is the 2U face that gives them one, and
crossing into `main.slint` once for all of them is what
`AGENTS.md`'s cost table asks for -- the alternative was five eight-minute
builds for five knobs that are about to be rearranged anyway. Until then they
are reachable from the automation lane's picker, which is a real surface
rather than a debug one.

**A ramp fixture makes an equal-power crossfade overshoot both its ends.**
`fill_ramp` writes each frame's own number so a read position can be
identified from the sample value, which means the "audio" is enormous DC:
fading between 30 000 and 24 000 peaks at 38 000, higher than either. The
first loop test read that as the window escaping. Tests that assert *where*
the head is have to set `crossfade_ms` to zero, and the two that do now say
so.

## Still open from the earlier steps

- **The locks half of acceptance test 8.** The allocation half closed with ten
  measured blocks. Nothing in the tree can express "no lock was taken on the
  callback", and nobody has proposed an instrument. `LOOSE_ENDS.md` carries it.
- **`BufferMidiMap` on `ParamAddr`** (`02`, step 5). Still a parallel
  source→destination system beside the general one.
- **The modulation shelf's source chip and the modulation arc on a knob**
  (`02`, step 4). Neither is Buffer-specific.
