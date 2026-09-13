# 06 — Layers and selectors: the decision, not the build

The three container kinds get conflated because they sound like one feature
with three settings. They are not, and this page exists so that saying no to
two of them is on the record with a price attached rather than being
rediscovered as an omission.

## Chain, which steps 02–04 build

A serial run with a dry/wet mix. One preallocated copy of the run's input, one
crossfade at its end, arithmetic on span lengths. No graph branching at all —
the children are rows of the same chain, and the engine's sequential loop
already runs them. This is the cheap one and it is most of the value: the
wet-FX gap, layering-by-nesting, a preset with an inside, and a home for a
modulator later.

## Layer — parallel branches, summed

**Not additive on top of chain.** A layer needs the audio graph to genuinely
split and re-merge, which is the machinery `FOCUS.md` parks under "parallel
sends and sidechains". The span representation cannot express it: a span is a
contiguous run of a sequential list, and parallel branches are not contiguous
in any order.

What it would actually cost, priced against what exists:

- N branch buffers per layer rather than one dry copy, all preallocated,
  installed structurally.
- Branch alignment to the longest branch, which is `compile_latency`
  (`mixer.rs`) applied inside a chain rather than across the mixer — the
  compensation work built the machinery, but not at this scale.
- A second representation in `EffectSlotState`, because the flat list stops
  being able to describe the structure.
- A drawing problem step 04 explicitly closed off: vertical adjacency now
  means "the chain continues", so a layer needs a visual treatment that does
  not exist.

`FOCUS.md` used to park sends and sidechains together, waiting for "a product
task that wants them". **Sends landed 2026-09-09 and did not need this**: a
send is a track-level route that carries its own compensation, so it is aligned
by construction. What a layer wants is parallel routing *inside* a chain, which
is still unbuilt and still parked. A layer device is the product task that
wants it. It is not one that has arrived.

## Selector — one active branch, others idle

Cheap once layer exists and nearly free to add then. Uninteresting before,
because with only a chain to hand it is a bypass with extra steps.

## The recommendation

Build chain. Let it prove the identity change from step 01 and the frame
contract from step 04 in real use. Then decide separately whether layers are
worth un-parking parallel routing — and decide it as a routing question, which
is what it is, rather than as a container feature.

## Done when

Nothing is built. This step closes when `00-status.md` records that chain
containers have been lived in and Adam has ruled on layers either way.
