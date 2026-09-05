# Step 04: preallocated compensation delays in the engine

## What lands

A stereo integer-sample delay per channel and per bus, sized from step 03's
plan, allocated on the control thread and shipped through the structural
channel — the same ownership round trip `SetSamplerStretch` already makes, and
for the same reason: the buffer is kilobytes and the audio thread may neither
allocate it nor drop it.

`StructuralCommand::SetCompensation { target, delay: Option<Box<..>> }`. `None`
means "this producer is the longest path into its destination and needs no
delay", which is the common case and costs nothing.

## Where it runs

On the producer, immediately before it sums into its destination — after its
output stage, before `add_from`. One delay per producer rather than one per
edge, because the tree gives every producer exactly one destination. Step 6
turns that into one per edge; until then the two are the same thing and the
simpler one is honest.

## The part that is actually fiddly

The plan is global: installing a Drive on channel 3 changes the compensation
required on channels 1, 2 and 4, because it moves the master's arrival. So
every structural edit that can change a chain's latency has to recompile the
whole plan and reissue the delays that changed.

The edits that can are: installing, replacing or removing an effect; adding a
channel; changing a channel's bus; changing a bus's output; and loading a
project. Each already goes through the control side, which is what makes this
tractable — but it is a list that must not grow silently, so the recompile is
one function with one call site per edit rather than a flag each path
remembers to set.

## Transitions

Changing a delay's length while audio runs is a discontinuity. The document
allows either a new prepared plan or "a bounded, declicked transition between
preallocated delays". Take the simpler one first: the new delay arrives
zero-filled and the old one is reclaimed, which is a short gap rather than a
click, and structural edits already interrupt the thing being edited. If that
is audible in practice, the declicked crossfade is a later step and not a
redesign.

## Done when

- Every producer's compensation is installed, and reinstalled after each of
  the edits listed above.
- No allocation, no `Box` drop, and no graph traversal on the audio thread.
- The engine's footprint test records what the delays cost.
