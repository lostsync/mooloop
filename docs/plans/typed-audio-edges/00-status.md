# Typed audio edges plan status

**Step 02 is in. Nothing is audible yet, and nothing has moved: a project with
no subscriptions compiles to the order the engine already walks, which is
asserted rather than believed.**

## Step 02 landed: the graph compiles

`compile_audio_graph` in `mooloop_core::mixer`, beside `compile_bus_graph` and
`compile_latency`, which already own the topology and its timing. Pure, `Copy`,
fixed-capacity, no allocation. `AudioSubscription` lives in `outlet.rs` with
the rest of the published-signal vocabulary.

Three things the doing changed about the plan:

- **The compiler takes the descriptor tables, not a `bool`.** Step 02 proposed
  `publishes: &[bool]` so core's compiler would not have to know what a
  generator is. It does not have to — but three of the refusals it owes are
  properties of the *individual outlet* rather than of the channel, and a
  `bool` cannot tell "that channel publishes nothing" from "that channel does
  not publish outlet 7" from "outlet 0 is a control signal". Since
  `OutletDescriptor` is core's own type, taking
  `&[&'static [OutletDescriptor]]` costs no layering and buys the whole
  refusal table.
- **A late tap is refused, not asserted.** The plan said the compiler would
  assert that every subscribed outlet taps upstream of the effect chain, so an
  `Output`-tapped outlet added later could not silently arrive late. An
  assertion is the wrong instrument: it is a debug-build tripwire for a
  condition that is a legitimate authoring mistake at runtime. It is
  `EdgeRefusal::TapIsLate` instead, which the consumer's face can show, and
  `OutletTap::is_upstream_of_chain` is where the rule is written down. The
  underlying fact is unchanged — every tap declared today is inside its
  generator, so an edge carries no latency term and `compile_latency` is
  untouched.
- **The function is total where `compile_bus_graph` is fallible.** A cyclic
  bus bank has no valid schedule at all, so that compiler returns `None` and
  the caller repairs. A cyclic audio edge is different in kind: refusing it
  silences only the device that asked for it, because an audio edge is the
  consumer's *whole input* rather than a summing point for work already done.
  So a ring refuses its own edges, every other channel still renders, and the
  order is still a complete permutation of the bank.

Two smaller decisions worth knowing:

- **A refusal keeps its subscription.** `AudioEdge::Refused` carries the
  authored value and the reason, which is what `poly-synth-v2/` step 06 means
  by "retained as inspectable orphan state" and what the modulation rack
  already does for a route to a departed module.
  `breaking_a_ring_gives_the_other_edge_back` is the test that makes it worth
  something.
- **The consumer scan is quadratic on purpose.** A producer may have any
  number of consumers, so a reverse adjacency table would be
  `MAX_CHANNELS`-squared storage to avoid a `MAX_CHANNELS`-squared scan that
  runs on the control thread over at most 256 entries.

The tests are graph shapes rather than sounds, which is the same reason
`compile_latency`'s are: no edges, one edge whose producer has the *higher*
index so index order cannot satisfy it by accident, a chain of three, one
outlet feeding three consumers, all six per-edge refusals in one table, a
two-cycle, a three-cycle, a broken ring, and a short project that still
compiles a whole-bank order.

## What is next

Step 03: the taps. Nothing writes an outlet's samples anywhere yet, so a
resolved edge currently resolves to a buffer that does not exist.

## Reading order

`01-what-this-is.md` for why the edge is same-block rather than one block
latent, which is the decision the rest of the plan falls out of, then 02
through 05 in order.
