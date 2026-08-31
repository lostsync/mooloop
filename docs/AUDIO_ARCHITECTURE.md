# Audio Core Architecture

Status: target architecture and migration contract, August 2026.

This document defines the audio subsystem Mooloop is growing toward. It is
not a proposal to become a general DAW, plugin host, or audio server. The goal
is a small instrument whose audio core has the same qualities as a mature
platform subsystem: explicit ownership, compiled topology, predictable time,
and a narrow realtime surface that remains pleasant to extend.

`PRODUCT.md` decides what the instrument is. `CURRENT.md` records what exists.
This document owns the boundary between editable musical state and audio
execution.

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
runtime may use fixed arrays and a small source taxonomy to retain predictable
work, but those implementation capacities are not the persistent or product
meaning of "four modulation slots."

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
declicked transition between preallocated delays. Bypass normally retains the
declared latency so toggling it does not move the channel in time.

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
   for the existing mixer tree.
6. Generalize the render plan from one audio output edge per bus to typed audio
   and dependency edges. Add parallel sends, then auxiliary sidechain inputs.
7. Route retained-audio buffers and explicit feedback through the same port,
   timing, preparation, and reclamation contracts.

Each step must leave the application usable. The architecture earns its keep
by making the next musical feature smaller, not by maximizing abstraction.
