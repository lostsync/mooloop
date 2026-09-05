# Step 02: the compiled audio graph

A pure function in `mooloop-core::mixer`, beside `compile_bus_graph` and
`compile_latency`, which already own the topology and its timing. It takes the
channels' declared subscriptions and answers two questions: **in what order do
the channels render**, and **which subscriptions are legal**.

## The subscription

One per channel, because the consumer built in step 04 is a source device and a
channel has one source:

```rust
pub struct AudioSubscription {
    pub channel: u8,
    pub outlet: u16,
}
```

It lives on the consumer's generator parameters rather than in a routing table
of its own. That is the same decision the modulation rack makes for a route's
destination, and for the same reason: the thing that owns the subscription is
the thing that would be meaningless without it, so a channel that changes its
generator kind cannot leave a stranded edge behind.

## What compiles

```rust
pub fn compile_audio_graph(
    subscriptions: &[Option<AudioSubscription>],
    publishes: &[bool],
) -> CompiledAudioGraph
```

`publishes[i]` is whether channel `i`'s generator declares any audio outlet at
all — a `bool` rather than the descriptor slice, so core's compiler does not
have to know what a generator is. `CompiledAudioGraph` is `Copy`,
fixed-capacity, and allocates nothing, like everything else in this file.

It answers:

- **`order()`** — the channel indices, producers before consumers. Channels
  with no edge at all keep their index order, so a project with no
  subscriptions renders in exactly the order it renders in today. That is not
  an optimization; it is what makes this step provably inaudible on its own.
- **`resolved(channel)`** — the subscription this channel actually gets, or
  `None`. A subscription that names a channel out of range, a channel whose
  generator publishes nothing, an outlet id that channel does not publish, or
  itself, resolves to `None`.
- **`refused(channel)`** — why, when it did not resolve. The consumer's face
  shows this; a silent `None` would be indistinguishable from a working edge
  into a silent producer.

## Cycles

The refusal that matters. A → B → A is the obvious one, and any longer ring is
the same thing. Two channels each subscribed to the other cannot both render
first, and the answer is not to pick one: it is to refuse both edges and say
so.

The sort is Kahn's algorithm over at most `MAX_CHANNELS` nodes with at most one
incoming edge each, which is small enough to do in fixed storage with no
recursion. Anything left with an unsatisfied dependency when the queue empties
is in a cycle; its subscription is refused, it renders in index order, and it
produces silence rather than stale audio.

**A refused subscription is retained, not deleted.** `poly-synth-v2/` step 06
asks for this directly — "incompatible routes are rejected visibly and retained
as inspectable orphan state when a project cannot resolve them" — and it is the
same rule the modulation rack follows for a route to a departed module. A user
who builds a cycle and then breaks it gets their edge back rather than having
to author it again.

## Latency

An edge's arrival carries the latency of its tap point. Every audio outlet
declared today is tapped inside the generator — `PreLevel`, `PreFilter`,
`PreShape`, `PreVca` — which is upstream of the channel's effect chain, so the
figure is zero and `compile_latency` needs no edge term.

That is a fact about the current taps and not a rule, so this step writes the
rule down where it will be met: `OutletTap` grows a method answering whether a
tap is upstream of the chain, and the compiler asserts every subscribed
outlet's tap is. An `Output`-tapped outlet added later fails that assertion
rather than silently arriving late, which is exactly the kind of drift that
would be inaudible until two channels were compared.

## Done when

- `compile_audio_graph` is pure, `Copy`, allocation-free, and tested against
  the graph shapes rather than against sounds: no edges, one edge, a chain of
  three, a self-subscription, a two-cycle, a three-cycle, an edge onto a
  channel whose generator publishes nothing, and an edge onto an outlet id that
  is not declared.
- A project with no subscriptions compiles to the identity order, asserted
  rather than assumed.
- A refused subscription is inspectable, and the reason distinguishes cycle
  from missing-producer from wrong-domain.
- Nothing in the engine has changed yet, and no render output moves.
