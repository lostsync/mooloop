# Step 01: what this is

`AUDIO_ARCHITECTURE.md`'s migration step 6: generalize the render plan from one
audio output edge per bus to typed audio and dependency edges. It is the last
thing holding `poly-synth-v2/` and `drum-synth-v2/` out of `archive/`, and it
is the only item on `FOCUS.md`'s active sequence that is architecture rather
than surface.

## What already exists, and what does not

The declaration half is finished. `mooloop_core::outlet` is the vocabulary —
`OutletDomain`, `OutletTap`, `OutletDescriptor`, `PublishesOutlets` — and both
instruments have filled it in. ML-P8 declares seven audio outlets (`Osc 1/2/3`,
`Sub`, `Noise`, `Pre-Filter Mix`, `Filter`) with their tap points; DS-01
declares four (`Tone`, `Noise`, `Body`, `Pre-Shape`). `OutletDomain` already
makes the control/audio refusal structural, so nothing can reach a control
destination by being numeric.

What does not exist is anything that can carry those samples, or anything that
would want them. Three things are missing and they are one step because none of
them is worth building alone:

1. **An edge.** A compiled statement that channel *B* consumes channel *A*'s
   outlet *n*, with storage the engine owns and a render order that makes the
   samples present when *B* asks.
2. **A tap.** ML-P8 computes `Osc 1` internally and throws it away. Nothing
   writes it anywhere, and materializing all seven unconditionally would cost
   56 KB on every channel for a feature nobody switched on.
3. **A consumer.** No device in the program has an audio input other than the
   chain it sits in.

## The shape of the decision

**Same-block delivery, not one block of latency.** The control outlets take
the other answer — published at the end of a block, read in the next one —
and that is right for them: it makes graph order stop mattering, costs one
block on a signal that already only updates per block, and is identical
offline and live.

An audio edge cannot take it. A block-sized delay is a delay whose length is
the *host's buffer size*, so the same project would render differently at 128
and 512 frames, which `AUDIO_ARCHITECTURE.md` forbids in as many words: "block
boundaries are an execution detail and must not change feedback delay,
retained-buffer behavior, automation timing, or offline output." The
whole-project null test would fail, and it would be right to.

So the samples must be present in the same block, which means the producer must
render before the consumer, which means the channel loop needs a compiled order
the way the bus walk already has one. **That is the actual content of this
plan**: not a buffer, but a schedule. The buffer is easy once the order is
decided.

**It is an edge type, not an ML-P8 feature.** `FOCUS.md` says so, and the
reason is that the same mechanism is what parallel sends and sidechains want
later — a producer, a consumer, a compiled order, and refusal of cycles. A
private pointer from ML-P8 to one consumer would work, would be half the code,
and would have to be deleted the first time a send arrived. Step 06 of
`poly-synth-v2/` forbids it directly: "Do not add a private bus pointer or
same-block callback escape hatch to make an ML-P8 demo work."

**The consumer is a source device.** Adam's call, 2026-09-05: a channel whose
generator is `Aux In`, and whose sound is another channel's published audio
outlet. It is the smallest consumer that makes the edge audible end to end, it
reuses the generator slot rather than inventing a rack concept, and it lands
ML-P8 step 06's acceptance case exactly — a muted `Osc 3` feeding another chain
while staying absent from ML-P8's own mix. A sidechain key input needs the
dependency-edge half as well (a signal that schedules a producer without being
summed into the consumer), and `FOCUS.md` rules out new effect kinds, so an
"Ext In" rack slot is out on its own terms.

## What this plan does not do

- **No parallel sends.** A channel still feeds exactly one bus. The edge built
  here is an auxiliary input to a *device*, not a second output from a strip.
- **No sidechain key inputs.** They need a dependency edge that schedules a
  producer without mixing it in. The compiled order built here is what they
  would hang off; nothing here builds the port.
- **No feedback cycles.** A cycle is refused at compile time and the
  subscription is kept as inspectable orphan state. External audio feedback
  needs the graph compiler's general delay policy, which is migration step 7.
- **No change to `compile_latency`.** Every audio outlet declared today taps
  *inside* the generator, before the channel's effect chain, so an edge's
  arrival carries no chain latency and the compensation plan is unaffected.
  This is a fact about the current taps rather than a rule, and step 02 writes
  it down as one so an `Output`-tapped outlet cannot be added later without
  meeting it.

## Reading order

02 compiles the graph, 03 materializes the taps, 04 builds the consumer, 05 is
acceptance. 02 and 03 are both engine-and-core work with no user-visible
surface; nothing is audible until 04.
