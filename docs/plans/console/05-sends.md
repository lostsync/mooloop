# 05 — Sends

The step that forces per-edge latency compensation, and the reason it is last.

**Rewritten 2026-09-09**, after Adam read the first draft. It was written
before `docs/TERMINOLOGY.md` landed and it was wrong in four places; the
originals are at the bottom, because knowing which way the mistake ran is
worth more than a clean page.

Because of decision 1 in the `README.md`, a send is a strip-level feature and
therefore lands on channels *and* tracks at once — which is how "a way to do
sends and returns without a mixer bus", the thing the brief asked for, costs
nothing extra here.

## What a send is

**A route to a track, not a kind of track.** Adam: *"a send is a route to a
mixer track, not the track itself. if you create a route from a track to
another track, the source track will gain a fader in the sends area."* That is
Reaper's model and it is already what `TERMINOLOGY.md` says: bus and send are
roles a track is put in by what routes into it.

So there is nothing to create. `Session::add_send` pushes an `AuxSend` onto
the source track and the track at the far end becomes an effects return by
being sent to — and stops being one when the last send goes away.

**There is no return object, and there is no way to feed one back.** Adam:
*"returns arent really a thing in this paradigm… i dont really know why we
need it. i dont think we do?"* Right, and the machinery agrees: a send from a
track back into something that already reaches it is a cycle, and cycles are
refused rather than delayed. The answer to "process the dry and the wet
together" is to route both to a third track, which is his own answer and needs
nothing built.

`would_create_cycle` is the one rule for both of a track's outgoing edges, so
the send target menu greys exactly what the output picker greys, for the same
reason and with the same sentence.

## Four is not a limit

Adam: *"i drew 4 sends bc that's how many fit in my drawing. if there are no
sends, we wouldnt show any. we're not limiting to 4… if we gain more than will
fit, that area should scroll."*

This amends [`THE-STRIP.md`](THE-STRIP.md)'s "Sends are four bars, inline. Not
a list, not a dialog." The sends area draws exactly the sends that exist —
none at all on a track with none — and scrolls when they outgrow the room,
rather than the strip growing or the faders shrinking. Nothing anywhere
reserves for a number of sends: a `Vec` in the model, a `Vec` in the engine's
bank, and `docs/CAPACITY_POLICY.md` keeps its rule.

## The tap point is an interface, not a device

Adam: *"im not opposed to creating a send device… or we just let a send be
created at any audio boundary? i kinda like that better…no device, the
available points are in the interface when you create the send."*

The menu he described, top to bottom: **post-fader** (the default),
**pre-fader**, then a drill-down into a device's individual signal outs where
they exist, or the output of any device in the chain. With a mark in the chain
wherever a send taps it.

The reason not to make it a device is stronger than taste: `OutletDescriptor`
already exists and DS-01 and ML-P8 already publish per-oscillator outs, so a
send device would be a second way of saying a thing the model can already say.

**This step builds the top two.** The drill-down taps are stage 2 — see below.

## Three things changed shape

**`CompiledBusGraph::destinations` did *not* have to be replaced.** The first
draft said it must be replaced rather than widened. Under Adam's model a track
still has exactly one output, so the `[u8; MAX_BUSES]` permutation stays true.
What dies is *one edge per node*, and that lives elsewhere:

- **`compile_bus_graph`'s in-degree** counts sends now, so a send from A to B
  orders B after A. Still one `[u8; MAX_BUSES]` permutation, and `feeding` is
  a `u16` because outputs are one per track and sends are not.
- **`reaches` became a search.** It walked a single-successor chain, which was
  exact while a track had one outgoing edge. It is a depth-first walk over a
  fixed visited set now: no allocation, and it terminates on a bank that is
  already cyclic because a node is only pushed once.
- **`compile_latency` became per-edge**, which is the sentence its own doc
  comment used to deny. The answer is still one `u32` per edge; there are
  simply more edges than producers.

**`StructuralCommand::SetCompensation` did *not* need an edge id.** A
producer's main edge is still one per producer, so `EffectTarget` keys it
correctly. A send's delay rides in the bank beside the send it belongs to.

**`OutputStage` is *not* where smoothing landed.** The gap the draft found is
real — there is no strip-level gain smoothing at all, a fader stamps its value
per block — but a send level is per-edge state, so `Smoothed` went on the
send. Per *sample*, not per control tick: `apply_pan_segments` steps 32 frames
wide because its values come from the control rate and it has no finer answer,
where a `Smoothed` does. The fader's own zipper stays a separate known gap.

**`OutletTap::Output` and `EdgeRefusal::TapIsLate` were not touched.**
`TapIsLate` is a rule about *aux-in subscriptions*, which land pre-chain in the
consumer and cannot be aligned there. A send is a producer-side edge that
carries its own compensation, so it is aligned by construction and never goes
through `compile_audio_graph`. Unifying the two edge systems is worth
recording and is not this step.

## Where a send leaves, and what that costs

Two new frames in each block loop, at points that already existed:

```text
effects.process(strip.bus, …)
    <- pre-fader sends capture here
output.apply_pan_segments(strip.bus, …)      // or apply_balance
    <- post-fader sends capture here
compensation.process(strip.bus)              // the main edge, in place, as before
destination.bus.add_from(strip.bus)
    <- sends emit here
```

Captured rather than emitted in place, because the tracks a send reaches are
not reachable while the strip that owns the buffer is borrowed. Each send then
copies its tap into a work buffer, applies its own smoothed level, applies its
own delay and sums — the copy is what lets one producer owe two summing points
two different delays.

**A strip with no sends walks a zero-length slice**, and a project with no
sends holds no bank, no rings and none of the three scratch buffers. That is
the "free while it is out" rule this plan is held to, and there is a test that
says so.

Two things fell out and are asserted rather than assumed:

- **A send is always linear.** Analog sum is what a strip does to its
  *output*; a send is a feed into another strip's input, which encodes on its
  own switch or not at all. This is also why `Session::console_plan` needed no
  change.
- **Mute silences a track's sends, pre-fader ones included.** A desk's mute is
  "this strip contributes nothing anywhere", and it is what the channel loop
  already did by skipping the rest of its body.

And one that is a decision rather than a consequence: **a disabled send stays
in the graph.** It orders its target and it still counts at the summing point
it reaches. Dropping it would mean a mute switch re-timed the mix.

## Where it goes on the wire

`EngineCommand::InstallBusGraph` is retired. Routing stopped being one `u8`
per track the moment a send carried a compensation ring, and a ring is a heap
object the audio thread may neither allocate nor free. So the graph and its
sends travel together as `StructuralCommand::SetTrackGraph`, derived and
diffed once a pump tick beside `sync_compensation`, `sync_audio_graph` and
`sync_console_sums` — one command, because a send whose target the render
order has not been told about would arrive a block late.

Level, tap and enable stay POD (`SetSendLevel` / `SetSendEnabled` /
`SetSendTap`), addressed by producer and by position in that producer's own
run of sends. Otherwise a fader drag would rebuild a plan, and every ring in
it, sixty times a second.

## Stage 2 — the drill-down taps

Not built here, and cleanly separable because of one measurement: **pre-fader
and post-fader arrive at the same time.** Both are after the chain, and a
fader declares no latency. So stage 1 needed no new latency arithmetic at all
beyond a per-edge delay.

A tap *inside* the chain does. Stage 2 is:

- `EffectChain::process` learns to emit a copy of its working buffer after
  slot *k*;
- `chain_latency` gains a prefix variant, so a tap's arrival is the sum of the
  slots upstream of it, and `SendTap` gains `AfterDevice` / `Outlet` variants
  that carry it;
- the tap menu grows its lower tiers, and the chain grows the mark Adam asked
  for — `DeviceHeader`'s colour dot and `DeviceFrame`'s z-ordered overlay are
  the two idioms to copy.

Everything the outlet half needs already exists: `OutletDescriptor`,
`MAX_DEVICE_AUDIO_TAPS`, and the deduplicated tap buffers in `AudioTapBank`.

## Acceptance

`MIXER_PLAN.md`'s, with the return removed and the alignment case sharpened:

- post-fader sends from two strips following their faders;
- one switched to pre-fader, with the documented different result;
- a latency-bearing device on the track a send feeds, and the dry path still
  aligned where the two meet again — asserted sample for sample at three block
  sizes, with the send *disabled* so that timing is isolated from level;
- a send that would loop refused, and the refusal naming the track;
- mute silencing a track's sends;
- a level change ramping rather than stepping;
- a project with no sends allocating nothing.

## What the first draft of this file said, and why it was wrong

Kept rather than deleted, because the shape of the mistake is the useful part.

> A strip gains `sends: Vec<AuxSend { target, level, tap: Pre | Post, enabled }>`
> […] Adam's mockup draws **four send bars inline on the strip** […] Keep the
> `Vec` in the model and draw four.
>
> **`CompiledBusGraph::destinations: [u8; MAX_BUSES]`** is one output per node
> and must be **replaced, not widened**.
>
> **`OutputStage` has to learn to smooth.**
>
> `OutletTap::Output` […] is currently declared with nothing publishing one --
> this is what publishes it. `EdgeRefusal::TapIsLate` is the refusal that has
> to learn about latency-bearing taps instead of refusing them outright.

The model line survived intact, which is worth noticing: what was wrong was
never the data.

Everything else mistook *where* the one-output assumption lived. It is not in
the destination table — a track really does have one output — it is in the
latency compiler, the cycle check and the block loop. Naming the wrong three
files made the step look like a rewrite of the schedule the realtime side
rests on, which is why it was scheduled last and approached with care it did
not need.

And it read `TapIsLate` as a rule about taps, when it is a rule about the
*consumer*: an aux-in edge lands pre-chain, where there is nowhere to put a
delay. A send has somewhere to put one. Two edge systems, one word, and the
word was doing the arguing.

The four-bar ceiling is the one Adam corrected himself, above.
