# Step 02: a device's latency is declared, not discovered

## The problem this solves

The plan has to be compiled on the **control thread**, because compensation
storage is allocated before activation and the audio thread may not allocate.
But latency is a property of a running node, and the control thread does not
hold running nodes — the engine does.

So the control side needs to answer "how much latency will a Drive with these
parameters have?" without building one. That is a declaration, and it belongs
in `mooloop-core` beside the parameter table, for the same reason
`OutletDescriptor` does: it is part of what a device promises, not an
implementation detail somebody measures.

## What lands

`EffectKind::latency_frames()` in `mooloop-core::effect`, returning the base-rate
frames that kind adds. Drive returns the oversampler's figure; everything else
returns zero today.

`OVERSAMPLER_LATENCY_FRAMES` moves from `mooloop-dsp` to `mooloop-core` and
`mooloop-dsp` reads it from there. It is an interface number now — the control
thread sizes a delay from it — rather than a private property of the
oversampler.

## The trap, and the guard

There are now two places a device's latency is written: the declaration in
core, and `AudioNode::latency_frames` on the built node. Two numbers for one
fact is how they come to disagree, and a disagreement here is silent: the
compensation would be built to the wrong length and the misalignment it was
meant to fix would move rather than go away.

`every_effect_kinds_declared_latency_matches_its_node` builds one node per kind
at two sample rates and asserts the two answers agree. It lives in
`mooloop-dsp`, which is the one crate that can see both.

## Why not derive the declaration from a built node instead

Because building a node allocates, and the control side would then have to
build a throwaway Drive to find out how long a delay to allocate. The
declaration is cheaper and it is also more honest: a device that cannot say
what it costs before it runs is a device the graph compiler cannot plan
around, which is precisely what step 6 will need.

## Done when

- `EffectKind::latency_frames` exists and every kind answers it.
- The node and the declaration are asserted to agree, per kind, per sample
  rate.
- Nothing else changes: no delay is inserted yet, no plan is compiled yet.
