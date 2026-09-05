# Step 03: compile cumulative latency for the tree

## What it computes

A pure function in `mooloop-core::mixer`, beside `compile_bus_graph`, which
already owns the topology. It takes what each producer costs and returns what
each producer must be delayed by.

```text
channel arrival  = sum of that channel's chain latencies
bus arrival      = max over its inputs of their arrival, plus its own chain
compensation(x)  = arrival at x's destination's summing point - arrival(x)
```

Buses are visited in the compiled render order, which is already topological
and already guarantees a bus is finished before the bus it feeds — so one
descending pass computes every arrival with no recursion and no second sort.
That is the same property `compile_bus_graph` was built for, reused rather than
rebuilt.

## Why it is pure, and in core

Because it is arithmetic over a topology, and the topology already lives here.
Keeping it out of the engine means it is testable without a `RenderState`,
without an audio thread, and without allocating anything — which matters,
because the interesting cases are graph shapes rather than sounds: a bus
feeding a bus, a channel on a bus whose sibling is longer, an empty bank, a
bank whose routes were repaired.

## The bounded result

Compensation is `u32` frames per channel and per bus, in fixed-capacity arrays
matching `MAX_CHANNELS` and `MAX_BUSES`. `Copy`, like `CompiledBusGraph`, so
the whole plan crosses to the engine by value with no allocation of its own.
What allocates is the delay storage the next step sizes from it.

## The one decision worth stating

**Bypass keeps its latency.** The architecture document says so, and the reason
is that the alternative is worse: a bypass that shortened the channel would
move it in time relative to every other channel, so A/B-ing an effect would
also A/B the timing and neither answer would be about the effect. A bypassed
node's latency is therefore summed exactly like a live one, which also makes
the plan independent of bypass state and so recompiled less often.

## Done when

- `compile_latency` returns per-channel and per-bus compensation for any legal
  bank, including a repaired one.
- A channel with a Drive and its sibling without one come out at the same
  arrival.
- A bus with its own latency pushes its inputs' compensation, not only its own.
- Nothing in the engine uses it yet.
