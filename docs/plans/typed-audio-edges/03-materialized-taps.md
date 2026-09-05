# Step 03: taps that only exist when somebody is listening

The engine side. The compiled order from step 02 decides when a channel
renders; this step gives the producer somewhere to write and the consumer
somewhere to read, and makes sure neither costs anything on a project that uses
no edges.

## The cost, and why it decides the design

ML-P8 declares seven stereo audio outlets. At `MAX_BLOCK_SIZE` a stereo buffer
is 64 KB, so materializing all seven on every channel is 448 KB a channel and
7 MB across a full bank, for a feature that is off by default.
`poly-synth-v2/` step 06 forbids paying it: "Materialising seven stereo taps
with nothing able to read them would cost 56 KB a channel and put buffers in
the callback for no consumer."

So **a tap is allocated when it is subscribed and not before.** The compiled
graph names exactly the (channel, outlet) pairs somebody reads — at most one
per consumer, so at most `MAX_CHANNELS` taps exist however many outlets are
declared. Storage is preallocated on the control thread and carried in on the
`StructuralCommand` round trip `SetCompensation` and `SetSamplerStretch`
already use, reconciled from the pump by deriving the plan from the model
exactly as `Session::sync_compensation` does. That mechanism is built, tested,
and its failure mode — forgetting a call site — is already designed out.

## Storage is keyed by the pair, not by the consumer

The obvious shape — one buffer per consumer — is wrong, and the test that
catches it is `one_outlet_feeds_as_many_consumers_as_ask_for_it` from step 02.
Two channels subscribed to the same `Osc 3` would get two buffers holding the
same samples, and the producer would write it twice.

So a tap is keyed by the **(producer, outlet) pair**, deduplicated when the
graph compiles, and a resolved edge carries a tap *index* rather than a buffer
of its own. That bounds storage by the number of distinct pairs, which is at
most the number of consumers, and it makes the producer's work proportional to
what is actually being listened to rather than to how many listeners there are.

Assigning the index is the compiler's job, because it is the only place that
sees every subscription at once. Step 02 built the edges; this step adds the
index to them.

## Filling a tap

A generator is told, once per block, which of its outlets to write and where.
Not a borrowed reference held at construction: `AUDIO_ARCHITECTURE.md` is
explicit that "a node must not retain a borrowed bus reference received at
construction", and the taps are supplied for the duration of the call like a
plugin's port group.

The shape that survived thinking about it is **a slice of optional buffers
indexed by the device's own tap number**, prepared by the engine once per
block from the device's declared outlet order:

```rust
if let Some(tap) = &mut taps[MlP8::TAP_OSC3] {
    tap.l[frame] = pre_level_left;
    tap.r[frame] = pre_level_right;
}
```

Two alternatives were rejected. Passing the outlet *ids* and having the device
match on them puts a lookup in the sample loop. Handing the device one buffer
at a time cannot work at all: ML-P8's inner loop computes `Osc 1`, `Osc 2` and
`Osc 3` in the same pass and would need three simultaneous mutable borrows out
of one slice. Indexing by tap number is a fixed offset, costs an untaken
branch when nobody is subscribed, and keeps the id-to-index mapping in the
engine where the descriptor table already is.

It also means a device's tap numbering is part of its interface, in the same
way its parameter ids are. The declared outlet order is that numbering, so
there is one list rather than two that can disagree.

The device fills the tap at the point its descriptor declares, which is the
whole content of `OutletTap`:

- `PreLevel` is before the source's own level control, so **a source muted in
  the device's mix still publishes**. This is the surprising behaviour and it
  is the useful one — it is what makes a silent internal oscillator available
  to somebody else, and it is ML-P8 step 06's acceptance case.
- `PreFilter`, `PreShape`, `PreVca` are the named points inside the voice.
- Voice pan is applied; the tap is stereo because the outlet is declared
  stereo and a consumer should hear the device's own image.

Per-voice sources sum across active voices, which is what "the sum of the
pre-Level modulation taps across active voices" means in step 06's table. A
device with no active voices writes silence, not stale samples: the tap is
cleared each block by whoever owns it, so a producer that stops playing stops
publishing.

## Ordering, and the one thing that must not regress

The channel loop walks `graph.order()` instead of `0..active_channels`. Two
properties have to survive it, and both are easy to break:

- **A muted channel still renders its tap.** Mute is an output-stage decision
  about what reaches the bus; a producer that is muted is exactly the case the
  pre-level tap exists for. The current loop `continue`s on mute before the
  strip renders at all, so this is a real change and it needs its own test.
- **Modulators still tick for every channel, in index order, before any of
  them render.** They already do — the tick pass is a separate loop — and it
  must stay separate, because ticking in graph order would make a modulator's
  phase depend on a subscription somebody made on another channel.

## Done when

- A tap exists only for a subscribed outlet, asserted by the footprint test,
  which is the test that has asked this question twice already and got a
  design change out of it both times.
- A subscribed tap carries the producer's samples in the same block, and
  compares equal to the same signal rendered without the edge.
- A muted producer still fills its tap; a producer with no active voices fills
  it with silence rather than the previous block's audio.
- Modulator phase is unchanged by the presence of any edge.
- No allocation, no lock, and no I/O in the callback: the plan arrives
  prepared, and the displaced one is reclaimed off-thread.
