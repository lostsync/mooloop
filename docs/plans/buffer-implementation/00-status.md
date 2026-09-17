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
| 6. The 2U face | Landed 2026-09-16 |
| Playability pass, after the first play-through | Landed 2026-09-16, and superseded the same day |
| Gesture rebuild: the turntable model retired | Landed 2026-09-16 |
| Alongside: acceptance test 8 | Closed for the Buffer operations 2026-09-16 |

**All six steps and the gesture rebuild have landed.** What is left is not
construction: it is the judgement the plan exists to make, and it needs ears
and a screen.
`buffer_workflow_tests.rs` carries six of `FOCUS.md`'s seven acceptance
clauses -- generate sound, capture it at a chosen insert point, sequence an
audible transformation, survive save and reload, render the same offline, and
render the same twice. The seventh, *show what the read head is doing*, has
its telemetry tested and its face screenshotted, and whether a person can read
it is not a thing a test can answer.

**So the plan does not archive yet.** `FOCUS.md`'s step 2 ends "if that
workflow is not materially better than bouncing a sample and loading it again,
record why before expanding the device", and that sentence is addressed to
Adam. Listening is a step, not a formality.

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

**The face waited three steps and crossed once**, which is what `AGENTS.md`'s
cost table asks for: five eight-minute builds for five knobs that were about
to be rearranged anyway was the alternative. `Rate`, `Freeze`, `Length`,
`Loop`, `Jump`, `Quantize` and `Quant Grid` were reachable from the automation
lane's picker in the meantime, which is a real surface rather than a debug one.

**The row carried two indexing schemes and they had always agreed.** (It
was three, not two: the edit path through `Session::set_effect_param` is
positional, and the paragraph below missed it. See the correction under the
playability pass.)
`EffectSlotRow.pN` was filled by descriptor *position*; `modulation_allowed`
and `destination_depths` are filled by descriptor *id*, which
`descriptor_slots` sizes as `max(id) + 1`. Every kind had dense ids from zero,
so the two were the same number and the markup could read `p2` and
`modulation-allowed[2]` and mean one thing. Retiring `Offset` parted them: the
Buffer's table starts at id 1, reaches 9, and has a hole at 0. The row is
id-indexed now, which is the scheme an on-disk identifier already uses.

**That parting had already produced a live defect, and a test caught it, and
the fix went in the wrong place.** In step 3 `slint_face_agreement` reported
"a knob routes modulation to parameter 0, which Buffer does not describe" --
correctly, because the face still said `[0]` for the knob that had become id
2. Rewriting `face_param_id` to index by position made the test pass and left
Position's modulation overlay reading a slot nothing writes: an arc that would
never have drawn, on the one control the whole device is about. The guard was
right and the markup was wrong. **When a guard fails, work out which side
moved before deciding which side to change.**

**The buttons are macros over published parameters and the debug events are
gone.** `debug_buffer_event` and `held_reverse_event` are deleted:
`REV` is `Rate := 1 - rate`, which is negation in normalized units;
`STUT` is Length to a sixteenth, Loop on, and a Jump, restored on release.
Nothing the mouse can reach is unreachable from a lane or a modulator, which
was the point rather than the tidiness.

**A ramp fixture makes an equal-power crossfade overshoot both its ends.**
`fill_ramp` writes each frame's own number so a read position can be
identified from the sample value, which means the "audio" is enormous DC:
fading between 30 000 and 24 000 peaks at 38 000, higher than either. The
first loop test read that as the window escaping. Tests that assert *where*
the head is have to set `crossfade_ms` to zero, and the two that do now say
so.

## The playability pass, 2026-09-16

The whole of `03` had landed and the device was played for the first time. Three
of the four things Adam reported were one defect each, and one of them was the
device's headline control.

> **Correction, same evening.** This section misdiagnosed its headline
> report. REV did nothing because **the face was addressing the wrong
> parameters**, not because of the Rate problem described below.
> `Session::set_effect_param` takes a parameter's *position* in the
> descriptor table, and the face and its Rust handlers passed descriptor
> *ids*. Once `Offset` was retired, the Buffer's ids and positions no
> longer lined up, so REV (id 3) wrote position 3, which was Freeze. The
> Rate defect below was real and was fixed, but it was found by reasoning
> from the DSP instead of by following an actual press to the device, and
> every DSP and session test was green the whole time. Codex found the
> wiring fault after the gesture rebuild had carried it forward (JUMP then
> operated Reverse). It is fixed in `76b4409` and guarded by
> `the_buffer_face_sends_descriptor_positions_for_edits`, and `AGENTS.md`
> now has a section on the two address spaces. **When a control "does
> nothing", trace the press before diagnosing the device.**

**`Rate` had no head to drive, so REV did nothing at all.** The device follows
its input -- a direct assignment, bit-identical and zero latency -- until
something detaches a read head, and the only things that did were Freeze, a
`Position` write and a gesture. `Rate` was read *by* a detached head and could
not create one. So over a live buffer at the default Position, the knob and the
REV button wrote a number the running program had no way to reach. It worked
frozen, which is where every test of it ran.

The plan's own note predicted the shape and stopped one step short: *"with only
step 1, `Rate` is a descriptor the lane picker lists and nothing in the running
program can make audible, because nothing detaches a free-running head until
Freeze does."* Freeze arrived, that sentence read as answered, and the live
case was never asked about. **A control that needs another control switched on
to do anything is only half built, and the half that is missing is invisible
from the tests of the other half.**

Rate now detaches at anything but unity and hands the head back at unity, and a
head it created *wraps* round the ring when it runs out of history rather than
returning to live -- otherwise a held REV reverses for one ring's worth of
history and then lets go on its own, which is the same failure a few seconds
later.

**The window could be drawn in front of the writer.** `fire` documents this for
a gesture and the parameter path reintroduced it exactly as
`02-control-and-modulation.md` said it would, through the one door nobody had
shut: a gesture's anchor is in the past because `offset_beats` put it there,
while `Position` at live **is** the write head, so a forward window opened from
it covered samples the writer had not reached. The window is slid back into
retained history now, and clamped to the ring's own length.

**STUT overrode the one control named for what it does.** It forced `Length` to
a sixteenth and restored the knob on release, so "how does one set the stutter
length?" had no answer: nothing on the face was it. STUT is LOOP plus a JUMP
now and leaves `Length` alone. Related, two rows down: the held-parameter map
was keyed by slot, so holding STUT and tapping REV threw away STUT's record and
left Loop on for good.

**Eight bars of history was a specification nobody had played.** `Position` is
normalized over the ring, so the ring's length *is* the position knob's
resolution -- at eight bars, halfway along the knob was four bars ago. Two bars
by default, and `bars` finally has a control: it cannot be a descriptor
parameter, because changing it reallocates, so it takes the road a tempo resize
already travels. The realtime side matches the occupant's allocation key before
it swaps, so the message has to carry the *old* configuration -- passing the new
params as both, which is right for a tempo change because a tempo change leaves
`bars` alone, would have had the swap silently refused.

**The drag across the history dropped half of itself.** `window-dragged` wrote
`Position` and `Loop` and put `to` in a `let _`, under a comment saying it set
the length too. `ModTimeDivision::nearest` is the missing half, snapping by
*ratio* rather than by difference because the grid is geometric.

## The gesture rebuild, 2026-09-16

**The playability pass above was the wrong fix, and Adam said so within the
hour: *"it's possibly actually worse now."*** It found real defects and
repaired them inside a model that was itself the problem. The concrete harm:
STUT was pointed at the shared `Length` knob, whose default is a whole bar, so
the stutter button started repeating bars. The repair made a symptom go away
by tightening the coupling that caused it.

What he asked for instead, nearly verbatim: *"its a buffer... if a button is
pushed, sample addresses... are calculated, and then we play the samples
between those two numbers."* JUMP has its own distance knob, REVERSE plays
backward from now, STUTTER is JUMP that repeats by its own length, and *"they
dont share any length settings really EXCEPT... quantizing start and length
device-wide."* And, of `Position`, which was nearly retired along with the
rest: *"the idea behind position was that you could manually draw in
automation of the playhead... you'd put a 1 measure sawtooth lfo on via
modulator rack and it would loop through the buffer... if it is moving, thats
what we should hear, otherwise its just live audio."*

So the device is now four sources in a fixed priority -- a held gesture, a
moving playhead, a frozen ring, the input -- and one head shape for all of
them: a position, a step and a region to wrap in. `reconsider` is the whole
arbitration rule. The chase, its time constant, the arrival and stillness
tests, `Drive`, `Scrub`, `armed_offset_frames`, the window's anchor rules and
the held-parameter bookkeeping in the UI are all gone. `Rate`, `Length` and
`Loop` are retired and their ids spent.

**The lesson is about which layer to fix.** Every defect the first pass found
was real, and every fix was locally correct, and the result was worse,
because each one added a rule to a model whose rules were the complaint. When
the report is "I don't understand what is difficult", the answer is not a
better explanation of the difficulty.

Two things the rebuild turned up that the old model had hidden:

- **A playhead anchored to the writer cannot play at unity.** The span moves
  a frame per frame, so a ramp over it moves at its own speed *plus* the
  writer's -- a one-bar saw over a one-bar ring played at double speed and no
  setting gave unity. At the full span `Position` now addresses the ring in
  its own coordinates, which is also the coordinate the waveform is drawn in.
  The old model never met this because the chase absorbed it.
- **`now` is where the next frame goes, not the newest one.** Reverse started
  there first, and read the *oldest* sample in the ring. Everything that
  places a head "at now" now places it one frame behind.

## The acceptance suite, 2026-09-16

**"Freezing sounds like almost nothing happened" cannot be asserted end to
end, and finding out why was worth the detour.** It is true exactly when the
material is periodic at the buffer length, and nothing at project level can
produce such material: `RenderState` runs the transport but does not loop the
pattern, so a fixture is either one pass of a pattern or a held note whose
period has no relation to the ring's. A held tone frozen mid-cycle differs
from the live one by *more than either amplitude* -- which is phase, not a
defect. The device's own test makes the claim against a ring holding a whole
number of cycles, which is the only honest place for it today. The end-to-end
version wants a looping transport.

Two fixtures came out of that: a drum bar for the tests about *where the head
is*, and a held tone for the freeze tests, which need a full ring and audio
still playing at the same moment. A drum pattern cannot give both.

**Rewritten 2026-09-17 against the gesture model.** The suite was written
before the rebuild and two of its tests drew the retired `Rate`, `Length` and
`Loop`. The frozen-ring transformation is now a held REVERSE, checked against
the same freeze without it and matched sample for sample against the ring read
backward; the round-trip case is a quarter-quantized thirty-second STUTTER,
proved to have fired by repeating at exactly its length before the reload is
compared. The rewrite also found that the other tests had stopped meaning what
they said: every `Position` lane started at `1.0`, which is now a ring
coordinate rather than "live", and swept faster than `MAX_SWEEP_RATE` through
history the writer had not reached -- so they passed on silence. And the drum
fixture had put all sixteen hits in its first sixteen *ticks*. Both are fixed.

**The rewrite found three device defects, and they are fixed (2026-09-17).**
With a quantized gesture in the document, `two_offline_renders_of_one_document_agree`
rendered differently at 128- and 512-frame blocks:

- **A quantized gesture landed late by its offset inside the block.**
  `BufferDevice::start` measured its wait as though every press arrived on
  frame 0; it takes the press's frame now, as `request_freeze` always did, and
  the boundary is found from the press rather than the block start.
  `a_press_inside_a_block_waits_from_its_own_frame` pins it.
- **A gesture press replaced a waiting freeze.** There was one `armed` slot.
  A freeze has its own now, and each gate its own countdown; a freeze due on
  the same frame as a gesture lands first and the device reconsiders once, so
  the gesture plays over the frozen ring and letting go hands the head to the
  ring. A gate still waiting no longer counts as held -- before, a freeze
  landing or a playhead let go of started it early.
  `a_gesture_pressed_while_a_freeze_waits_keeps_the_freeze` and
  `a_landing_freeze_does_not_start_a_waiting_gesture` pin it.
- **Same-frame values applied in descriptor-table order**, so a gate beat
  `Quant Start` and waited on the old grid. The device applies a frame's
  settings before its actions (`Position`, `Freeze`, the gates); the table
  order is untouched, because the face addresses it by position.
  `a_gate_waits_on_a_grid_set_on_its_own_frame` pins it.

The quantized REVERSE still starts two frames behind the newest one
(`a_frozen_buffer_played_in_reverse_is_the_ring_backward` measures lag 2).
None of the fixes touched it: a press landing on a frame's end reads `now`
after that frame is written but before the writer advances.

## Still open from the earlier steps

- **A quantized press starts two frames behind the newest frame.** Harmless
  and unfixed; see the acceptance suite section above.
- **The locks half of acceptance test 8.** The allocation half closed with ten
  measured blocks. Nothing in the tree can express "no lock was taken on the
  callback", and nobody has proposed an instrument. `LOOSE_ENDS.md` carries it.
- **`BufferMidiMap` on `ParamAddr`** (`02`, step 5). Still a parallel
  source→destination system beside the general one.
- **The modulation shelf's source chip and the modulation arc on a knob**
  (`02`, step 4). Neither is Buffer-specific.
