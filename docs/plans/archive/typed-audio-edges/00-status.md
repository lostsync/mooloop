# Typed audio edges plan status

**All five steps are in, closed 2026-09-05. A channel set to `Aux In` plays
another channel's published audio outlet, in the same block, and an
oscillator muted in ML-P8's own mix is audible through one while staying
absent from its producer's output — which is what `poly-synth-v2/` step 06 and
`drum-synth-v2/` step 07 were both waiting for.**

`AUDIO_ARCHITECTURE.md`'s migration step 6 is half done by this: the typed
*audio* edge exists. Parallel sends and sidechain key inputs do not, and the
plan said so from `01`; what changed is that they now hang off a compiled edge
model rather than waiting for one.

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

## Step 03 landed: the taps

Storage is keyed by the **(producer, outlet) pair**, not by the consumer, and
the compiler assigns the indices because it is the only place that sees every
subscription at once. Two channels reading the same `Osc 3` share one buffer;
step 02's `one_outlet_feeds_as_many_consumers_as_ask_for_it` is the case that
makes a per-consumer scheme visibly wrong, since it would write the same
samples twice and cost storage proportional to how many are listening rather
than to what is listened to.

Two things the compiler half turned up:

- **A refusal has to give its tap back.** Assigning an index while resolving
  and then losing the edge to a cycle leaves the engine holding a buffer
  nothing reads. The table is rebuilt from the survivors in one pass at the
  end rather than un-assigned as refusals happen, which keeps the indices
  dense -- and dense is what lets the engine treat `tap_count()` as the number
  of buffers to allocate rather than as a high-water mark.
- **`is_empty` became the tap count.** It was "does any edge resolve", which
  is the same question asked of the wrong table. What the engine actually
  needs to know is whether there is anything to allocate, and on every project
  that has never used an edge the answer is zero.

Three more from building the engine half:

- **A subscriber is a reason to *run* a source, and that is not optional.**
  `mooloop_dsp::mlp8` skips an oscillator whose level is zero and which
  nothing else needs, which is exactly the oscillator the acceptance case
  subscribes to. Without folding tap demand into `Prepared`'s `osc_needed` /
  `sub_needed` / `noise_needed`, the headline test would have published the
  zeros of a source that was never computed — and it would have looked like a
  routing bug rather than a skipping one. DS-01's `body_live` has the same
  shape and got the same treatment. This is the single thing most likely to be
  got wrong by a later device implementing outlets.
- **The consumer reads through a scratch copy, not a borrow.** A channel may
  write its own taps and read another channel's in the same call. Those are
  provably different buffers — a device cannot subscribe to itself — but
  nothing in the type system knows it, and the alternatives were unsafe
  aliasing or one input buffer per channel (16 MB across the bank). One
  scratch buffer for the whole engine, and a `frames`-long copy per consumer
  per block, is a far smaller price than either.
- **A muted producer renders only when somebody reads it.** `03` asked for "a
  muted channel still renders its tap" and taken literally that would make
  mute stop saving any work at all. The rule as built is narrower and is the
  one the plan actually wanted: a muted channel that owes a tap renders its
  generator and stops before the chain, the meters and the bus sum; a muted
  channel nobody reads still skips at the top of the loop, as it always did.

## Step 04 landed: Aux In

`DeviceKind::AuxIn`, `GeneratorParams::AuxIn`, `ChannelSource::AuxIn`,
serialized as `aux_in`, three descriptor-addressed parameters, a two-rack-unit
face. Four things the doing changed:

- **Aux In publishes its own output.** The plan did not say it would, and
  without it `05`'s cycle acceptance is unbuildable: ML-P8 and DS-01 never
  consume, so with a consumer that never produces, no ring can be constructed
  from the program at all and `compile_audio_graph`'s cycle refusal would be
  reachable only from a unit test. Publishing also makes an obvious thing
  possible — one tap reaching two chains through a passthrough — so it is
  worth having on its own terms rather than only for the test.
- **`OutletTap` grew `PreChain`.** Aux In's whole content *is* its output, so
  a pre-level tap would republish exactly what it read and say nothing. None
  of the existing tap points names "the generator's finished output, before
  the channel's effect chain", and `Output` could not be reused: it is
  deliberately the late tap that `TapIsLate` refuses, and weakening it to make
  one device fit would have deleted a designed refusal.
- **`ParamCurve::Stepped` widened from `u8` to `u16`.** A selector naming a
  channel needs `MAX_CHANNELS + 1` = 257 positions and a byte of *count*
  cannot express it. The width was never a design decision — it was the
  default integer — and the parameter's value being the channel index itself
  is what keeps the plan's "the stepped mapping is wire format" rule cheap:
  there is no index-to-value mapping for a face to hold a second copy of.
- **A departed producer sends the subscription to `DEPARTED_SOURCE`.** The
  modulation rack *deletes* a route whose channel is deleted, and the obvious
  move was to match it. `04` asks for something else — "leaves the
  subscription inspectable and the consumer silent, rather than pointing at
  whichever channel inherited the index" — so the subscription is retained and
  pointed at the last addressable index, where the compiler refuses it as
  `NoSuchChannel` and the face says so. The corner is written down in
  `aux_in.rs`: a song holding all 256 channels would resolve it, and cannot
  grow into that state, only shrink out of it.

## Step 05: the measurements

Twelve tests in `mooloop_engine::audio_edge_tests`, plus the device-level ones
in `mooloop_dsp` and the persistence round trip in `mooloop_core::project`.

- **The headline case, both halves.** An Aux In hears ML-P8's `Osc 3` while
  ML-P8's own output does not carry it, measured spectrally in both
  directions: the oscillator is tuned a fifth above the fundamental so a
  unison one could not have passed by accident. An edge that worked by leaking
  the oscillator into the producer's mix passes the first assertion and fails
  the second.
- **The same through DS-01's `Tone`**, because one device passing is not
  evidence that the edge type is general.
- **Same block.** The consumer's copy lands in the frame the producer made
  it, measured against the same note heard directly rather than computed from
  the tick, so the test calibrates itself against the engine's own scheduling.
  It fails by exactly one block if delivery ever becomes deferred.
- **Same at any block size**, sample for sample at 128 and 512 — the
  assertion a block-latency edge could never pass.
- **Offline equals live**, through the real `OfflineRenderer` to a Float32
  WAV, with an assertion that the window contains the edge before an assertion
  that the two agree.
- **A producer with the higher index still renders first**, so index order
  cannot be what satisfied the edge.
- **A muted producer publishes**, and reaches nothing else: measured by taking
  the subscription away and asserting the master is *exactly* silent, rather
  than by looking for the producer's fundamental in a spectrum.
- **A ring is refused and gives the edge back when broken**, and a
  three-channel ring too.
- **A modulator's phase does not move** when an edge is added.
- **Level takes a lane and a route**, through the engine rather than through
  the descriptor table: it is the generic parameter path that has to reach a
  generator kind that did not exist when it was written.
- **A project with an Aux In saves and reloads**, subscription intact, and a
  manifest with an empty `state.params` loads as an Aux In subscribed to
  nothing rather than failing.
- **Cost.** A sixteen-channel project with no edges pays 4 KiB more than
  before — twenty bytes a channel for the Aux In node, and eight and sixteen
  bytes a voice for ML-P8's and DS-01's published samples — and **no buffers
  at all**. Materialising ML-P8's seven stereo outlets unconditionally would
  have been 448 KB a channel and 7 MB across a full bank for a feature that is
  off by default. That is the figure this plan exists to avoid, and the
  footprint test is what says it was avoided.

## What this did not do

Unchanged from `01`, and worth restating because the next person will look:

- **No parallel sends.** A channel still feeds exactly one bus. What landed is
  an auxiliary input to a *device*, not a second output from a strip.
- **No sidechain key inputs.** They need the dependency-edge half: a signal
  that schedules a producer without being summed into the consumer. The
  compiled order built here is what they would hang off.
- **No feedback cycles.** A cycle is refused and its subscription kept.
  External audio feedback needs the graph compiler's general delay policy,
  which is `AUDIO_ARCHITECTURE.md`'s migration step 7.
- **`compile_latency` is untouched.** Every audio outlet declared today taps
  inside its generator, so an edge carries no chain latency.
  `EdgeRefusal::TapIsLate` is where that stops being an accident.

## Reading order

`01-what-this-is.md` for why the edge is same-block rather than one block
latent, which is the decision the rest of the plan falls out of, then 02
through 05 in order.
