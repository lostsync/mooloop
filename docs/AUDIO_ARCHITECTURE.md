# Audio Core Architecture

Status: target architecture and migration contract, September 2026.

This document defines the audio subsystem Mooloop is growing toward. It is
not a proposal to become a general DAW, plugin host, or audio server. The goal
is a small instrument whose audio core has the same qualities as a mature
platform subsystem: explicit ownership, compiled topology, predictable time,
and a narrow realtime surface that remains pleasant to extend.

`PRODUCT.md` decides what the instrument is. `CURRENT.md` records what exists.
This document owns the boundary between editable musical state and audio
execution.

`ARCHITECTURE_REVIEW.md` grades the implementation against an external
reference and agrees with this document almost everywhere. Its one finding
against the engine was that migration step 5 below — graph-wide latency
compensation — was the next infrastructure step and cheaper then than it would
ever be again. It landed on 2026-09-05, and step 6's first half — the typed
audio edge itself — landed the same day. What remains of step 6 is parallel
sends and sidechain inputs, both of which now hang off a compiled edge model
rather than waiting for one.

## Design Character

The audio API should make the safe and musically correct operation the easy
one. A caller editing a project should not need to know buffer order. A DSP
author should not need to know mixer indices. The realtime callback should not
need to interpret project data, allocate an object, discover graph topology,
or decide how malformed state is repaired.

The system is split into three layers:

```text
editable Project
      |
      | validate, normalize, instantiate, allocate
      v
prepared RenderPlan + RenderState
      |
      | bounded swap at a block boundary
      v
realtime Executor -> JACK / offline sink
```

The same prepared state and executor serve realtime and offline rendering.
JACK is an I/O adapter, not the owner of musical semantics.

## Control Plane

The control plane owns operations whose cost or lifetime is unsuitable for an
audio callback:

- validating and normalizing project documents;
- compiling graph topology and latency compensation;
- constructing instruments, effects, buffers, and delay lines;
- decoding samples and preparing immutable sample assets;
- assigning monotonically increasing generations to structural snapshots;
- reclaiming replaced graphs, nodes, samples, and retained-audio buffers.

A structural edit produces a complete, internally consistent object. The
audio side never observes an edge without its schedule, a latency value
without its compensation storage, or a node without its prepared buffers.

High-rate value edits are different. Transport changes and already-addressed
parameter events may cross bounded lock-free queues as small POD messages.
Queue overflow must be observable to the sender; silent divergence between
the visible project and audible engine is not an acceptable steady-state
contract.

## Control Graph Within A Channel

The normal audio topology of a channel remains an ordered source-and-insert
rack. Its modulation topology is explicit channel state: a `ModRack` owns
control sources and routes, while sources, inserts, and the strip own their
parameters. A device must not hold a private copy of the channel's LFOs or
know which external control signals currently target it. It may own authored
modulation that is part of its own DSP contract -- for example per-voice
envelopes, oscillator cross-modulation, or an instrument-specific LFO -- and
publish selected signals through typed outlets. Cross-device consumption then
uses the channel route and timing rules below.

Each route joins a stable source or outlet reference to a stable `ParamAddr`
destination through a bounded transform (depth, polarity, and later shaping).
Source metadata declares value semantics, control rate, and latency; parameter
metadata declares range, curve, and modulation eligibility. The current
runtime uses fixed arrays and a bounded source taxonomy to retain predictable
work -- eight module slots and sixteen routes a channel -- but those
implementation capacities are not the persistent or product meaning of the
number. Both are compile-time constants with a measured, linear price;
`CAPACITY_POLICY.md` says why a ceiling is not the same as a reservation.

At each declared control tick, the executor evaluates a source, applies each
route transform, adds offsets to the destination's base value, and puts the
resolved natural-unit value on the existing sample-timed parameter path. A
device outlet consumed across a device boundary is read on the following block
unless a future contract explicitly compiles a different declared latency.
Display telemetry is never a control input.

This is graph-capable data, not a second audio graph or a mandate to build a
graph editor. A later zoomed-out view may visualize the same routes and the
ordered audio chain. It must edit the same prepared channel state and preserve
the rack as the normal interaction.

## Graph Compiler

The project mixer is editable data, not an execution plan. Compilation turns
it into a fixed-capacity `RenderPlan` containing at least:

- normalized node and port identities;
- validated audio and dependency edges;
- a topological execution order;
- buffer assignments and mix operations;
- cumulative node latency and per-edge compensation;
- diagnostics for repaired legacy data or refused edits;
- a generation identifying the project state it represents.

Today each bus has one audio destination, so its routing is a tree directed
toward the master and a compact bus permutation is sufficient. Parallel sends
and sidechains will turn the dependency model into a DAG. They should extend
the compiler's edge model rather than add a second scheduler inside individual
effects.

Cycles remain invalid graph topology. Musical feedback is an explicit node or
edge kind with a defined delay measured in frames, gain behavior, and
persistence. It must not inherit the current JACK quantum as an accidental
one-block delay.

Malformed stored routes are normalized before compilation. Missing buses and
out-of-range destinations route to the master. A graph containing a genuine
cycle is reported and may be repaired to the documented all-to-master safe
plan when loading a project; an interactive edit that would create one is
refused without changing the running generation.

## Realtime Executor

The executor owns all mutable state touched per block: node state, scratch
audio, event lists, meters, transport, and the active render plan. Its callback
contract is strict:

- no allocation or deallocation;
- no reference-count transition that might destroy a large object;
- no locks, I/O, logging, graph traversal, or topology validation;
- bounded work derived from declared capacities;
- identical signal behavior for realtime and offline block sizes.

Prepared states and structural nodes cross to the executor by ownership. At a
block boundary it may swap pointers or fixed-size values. Anything displaced
returns through a bounded reclaim channel and is destroyed on the control
thread. If reclaim capacity is unavailable, the executor applies backpressure
by leaving the structural edit queued; it never drops the object itself.

## DSP Node Contract

`AudioNode` is the small realtime interface, not the whole object model. Its
eventual processing contract needs to describe:

- main audio inputs and outputs;
- zero or more typed auxiliary inputs such as sidechains;
- sample-timed note and parameter events as separate streams;
- reported processing latency in frames;
- tail behavior and reset/transport discontinuities;
- stable parameter descriptors and instance identity.

Latency and tail are implemented; "Rest And Tail" below states what they mean
and what a host may do with them. Reset and transport discontinuities are not.

`COMPOSABLE_DEVICE_UNITS.md` defines the recursive design contract above and
below this adapter: primitives and composites have intentional parameters,
typed inlets, and typed outlets, while private fixed topology stays private.
That contract does not require every primitive to implement `AudioNode` or
become a runtime graph node. `AudioNode` remains the prepared realtime adapter
for a whole processing unit; discoverable metadata and editable composition
belong to the control plane.

The current in-place stereo method remains useful for ordinary instruments and
inserts. Auxiliary input should be supplied for each process call by the
executor; a node must not retain a borrowed bus reference received at
construction. The API can grow through a process-buffer view while preserving
a convenience adapter for simple in-place stereo nodes.

Node latency is a property of the active processing path. Bypass, dry/wet
mixing, and latency-changing parameters need defined behavior. An effect with
an oversampled wet path must align its own dry path before the graph compiler
can compensate that effect against neighbouring paths.

### Rest And Tail

A node says whether it has anything to do, and the host stops calling it when
it has not. Three defaulted methods carry it, and the defaults mean "never
skip me" so a node that has not opted in behaves exactly as it did before:

- `tail_frames` — how long this node can still be heard after its input goes
  silent. `u32::MAX` means unbounded or unknown.
- `is_at_rest` — whether its own state has settled, so that silent input
  produces silent output now. A statement about the node, not about the audio
  it was last handed: the host tracks input silence, because that is the only
  side that can still count while the node is asleep.
- `skip_block` — called in place of `process` for a block the host skips, so
  the node can move whatever it runs on the clock rather than on the audio.

The host skips a slot whose input has been silent longer than its tail, or
whose state has settled, once its dry-path aligner has emptied. A whole
channel strip is skipped when it has no events, its generator is at rest and
has been putting out silence, and every occupied slot in its chain would be
skipped. Nothing is published: the device meters are peak-hold cells the GUI
empties as it reads them, so not writing one is publishing silence.

Two rules follow from the block-boundary rule above, and both were found by
breaking them:

- **A tail must cover any ring a parameter can move a read head inside**, not
  just the time the device takes to go quiet. A sleeping node's delay lines
  stop advancing, so a delay time swept up afterwards would read audio from
  before the silence.
- **Free-running state has to keep running.** An LFO that keeps its phase
  across silence, a reverb's line modulation, a sample-and-hold's counter: if
  those freeze, a device comes back somewhere else, and *how far* depends on
  the host's buffer size — which would make a bounce stop matching a take.
  `skip_block` mirrors the sample loop rather than closed-forming it, because
  a phase that arrived by a different route is a different phase.

A device that cannot honour this declines in writing rather than by omission.
The retained-audio buffer makes sound out of a silent input by design; Aux In's
sound is another channel's and can start without an event of its own; ML-P8
with its chorus switched on has a delay line and an LFO in its finisher.

## Audio Buffers And Ports

`StereoBus` is the current fixed-format buffer and remains a good optimized
primitive. Graph-level APIs should refer to typed ports and buffer handles,
not borrow another node's storage at construction. This keeps ownership
centralized and permits the compiler to reuse or permanently assign storage
without exposing that decision to DSP code.

Mooloop may remain fixed to planar stereo for its first-class channel and bus
paths. External I/O and future plugins can add explicit mono or multichannel
layouts without making every built-in effect dynamically shaped.

Summing and balance are distinct from mono panning:

- the historical channel pan law is retained for project compatibility;
- a stereo bus at centre is level-neutral;
- inserting zeroed bus stages must not change level;
- a balance control must not add 3 dB merely because it reaches an endpoint.

## Time

`ProcessContext` is the single per-block clock. Frame position is authoritative
for DSP; PPQ position is the musical projection of that clock. Future tempo
maps should be compiled into bounded block segments rather than queried from
project objects in each node.

Sample-timed events use offsets within the current block and deterministic
ordering at equal offsets. Control-rate modulation may compile into several
such offsets per block. Block boundaries are an execution detail and must not
change feedback delay, retained-buffer behavior, automation timing, or offline
output.

## Latency Compensation

Each node reports integer latency frames initially. The graph compiler sums
serial latency and inserts compensation on shorter inputs at every summing or
dependency point. Compensation storage is allocated before activation.

The current one-destination mixer is cheap to compensate because each node has
one downstream audio edge. Parallel sends and sidechains require the general
DAG rule: compute the longest upstream arrival at each consumer and delay every
shorter input by the difference. A sidechain also adds a dependency edge to
the schedule even when it is not mixed into the consumer's output.

Changing latency while active requires a new prepared plan or a bounded,
declicked transition between preallocated delays. The mixer takes the first:
a changed delay arrives zero-filled and the displaced one is reclaimed, which
is a short gap rather than a click, on an edit that already interrupts what it
edits. Bypass retains the declared latency so toggling it does not move the
channel in time — which means the *container* delays a bypassed node's signal
by its declared frames rather than passing it through, since the compensation
plan sums bypassed slots.

## Lifecycle And Fault Model

Every structural installation has a generation and one of three outcomes:

1. prepared and activated;
2. refused before activation with a diagnostic;
3. superseded by a newer generation and reclaimed off-thread.

There is no partially active generation. Realtime queues expose saturation,
xruns remain measurable, and repair of a project document is visible to the
control plane. Tests should include an allocation detector around the callback,
realtime/offline null comparisons across several block sizes, graph property
tests, impulse-based latency tests, and saturation/backpressure tests for every
ownership queue.

## Migration Sequence

Steps 1 through 5 have landed, and so has the first half of 6. Sends,
sidechains and step 7 are next.

1. Move project construction out of the JACK callback. Prepare a complete
   render state on the control thread, swap it at a block boundary, and return
   the old state for deferred destruction.
2. Replace loose bus destinations plus a permutation with one compiled bus
   plan that owns both. Normalize short and malformed banks at its boundary.
3. Separate channel pan from stereo bus balance so existing projects retain
   their level while routing remains neutral.
4. Add node latency reporting and align effects' internal parallel paths,
   beginning with the oversampled drive.
5. Introduce preallocated compensation delays and compile cumulative latency
   for the existing mixer tree. **Landed 2026-09-05**, in
   `docs/plans/archive/latency-compensation/`: a device declares its latency without
   being built, `compile_latency` turns the tree into a per-producer delay, and
   the delays are installed structurally and reconciled by deriving the plan
   rather than tracking it. Parallel sends, sidechains and limiter lookahead no
   longer wait on it.
6. Generalize the render plan from one audio output edge per bus to typed audio
   and dependency edges. Add parallel sends, then auxiliary sidechain inputs.
   **The typed audio edge landed 2026-09-05**, in
   `docs/plans/archive/typed-audio-edges/`: `compile_audio_graph` turns the
   channels' declared subscriptions into a render order and a refusal for each
   edge that cannot resolve, a producer fills a tap only when somebody has
   subscribed to it, and Aux In is the source device that reads one. Delivery
   is same-block by construction — the consumer renders after its producer —
   so the whole-project null holds across block sizes and an export matches a
   live take sample for sample.

   Two things it deliberately did not do, and **the first of them landed
   2026-09-09**, in `docs/plans/console/` step 05. **Parallel sends** are a
   second outgoing edge from a strip, and they turned out not to extend this
   mechanism at all: a send is a producer-side edge that carries its own
   compensation, so it is aligned by construction and never goes through
   `compile_audio_graph`. What it did change is the three places the
   one-edge-per-node assumption actually lived — `compile_latency` became
   per-edge, `reaches` became a depth-first search, and the block loop gained
   two capture points and one emission point. `CompiledBusGraph::destinations`
   was not touched, because a track still has exactly one *output*. Sends are
   strip-level and a channel's compiles, but only a track can author one today.

   **Sidechain key inputs** are still absent. They need the dependency-edge
   half — a signal that schedules a producer without being summed into the
   consumer — which the compiled order built here is what they would hang off.
   A cycle is refused and its subscription retained as inspectable orphan
   state; external audio feedback still needs step 7's general delay policy.

   That leaves two edge systems wearing one vocabulary, which is recorded
   rather than resolved: `EdgeRefusal::TapIsLate` reads as a rule about taps
   and is a rule about *consumers*, since an aux-in edge lands pre-chain where
   no delay can go. Unifying them is its own change.
7. Route retained-audio buffers and explicit feedback through the same port,
   timing, preparation, and reclamation contracts.

Each step must leave the application usable. The architecture earns its keep
by making the next musical feature smaller, not by maximizing abstraction.
