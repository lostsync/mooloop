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

`archive/ARCHITECTURE_REVIEW.md` grades the implementation against an external
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
realtime Executor -> driver (JACK / Core Audio) / offline sink
```

The same prepared state and executor serve realtime and offline rendering.
The driver is an I/O adapter, not the owner of musical semantics.

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

**Looking at something is not an edit, and must not reach the audio
thread.** Selecting a pattern, a channel, a bus, a device, a track or a pane
changes what is drawn and what the next edit will address. None of it changes
what is scheduled, so none of it may cost a voice, a parameter jump or a block
of work in the callback.

Where a navigation gesture genuinely has to inform the engine, the engine
charges for what *changed* rather than for the fact that a command arrived.
There is one such gesture: the active pattern is also the record target, so
`SetCurrentPattern` is still sent, and `set_current_pattern` answers whether
the selection actually moved.

This is written down because it was broken, and the bill was every sounding
voice on every channel -- in Song mode, where the selected pattern is not what
is playing. `scripts/dupe-audit navigation-sends` reports a selection handler
that sends anything else; a gesture that is really an edit says so in its name
(`on_automation_lane_opened`, not `…_selected`). MOO-57 and
`docs/plans/transport-discontinuity/`.

**Two mechanisms cross the boundary, and nothing else does**: the ordered
command stream, whose displaced heap objects come back through the reclaim
ring, and atomics. Nothing the audio thread reads from the control side is
reference-counted. The routing tables -- MIDI input, audio input, the buffer
MIDI map -- were `ArcSwap` cells until 2026-09-19; a guard held on the audio
thread could outlive the control thread's reference to a table it had just
replaced, and the free then ran in the callback (`reports/fable-2026-09-19.md`,
finding 2). They are `StructuralCommand::SetMidiRouting`,
`SetAudioInputRouting` and `SetBufferMidi` now. The same review's finding 1
was the same class by another door: a project install's carry plan was
dropped at the end of the install arm; it leaves with the retired renderer.
`installing_a_project_allocates_and_frees_nothing` and
`a_routing_change_frees_nothing_on_the_callback` measure both, around
`Executor::process` rather than around the one call inside it. (The per-channel
sample slot is still an `ArcSwapOption`, retired through the reclaim ring by
`load_full`, as `reports/fable-2026-09-17.md` finding 2 settled.)

**Sequencer storage is not allocated on an edit either, and is not
preallocated with the bank.** A pattern's automation lanes are a fixed array
of eight slots per channel-pattern; closing one vacates its slot and keeps
its point vector, so a lane edit on the callback never frees, and reopening
one never allocates. What the bank cannot do is own that vector up front:
256 patterns by 256 channels by eight lanes by 1024 points is 6 GiB, and
`CAPACITY_POLICY.md`'s *"the same lesson, unlearned"* is about this very
bank. A slot that has never held a lane takes its storage from
`LanePool`, refilled at project install; an empty pool still opens the lane,
by allocating, rather than refusing an edit the user asked for. Closing that
last hole means the session supplying the storage with the command, the way
`StructuralCommand` supplies a routing table -- `LOOSE_ENDS.md`
(`reports/fable-2026-09-21.md`, finding 1).

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

The three routing tables were the one exception to the second bullet, and
are not any more: `a576f17` reapplied the fix, and the paragraph a hundred
lines above -- *Two mechanisms cross the boundary* -- describes what they are
now. Recorded 2026-09-20 from `reports/fable-2026-09-20.md` finding 1, closed
2026-09-21.

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
and what a host may do with them. **Transport discontinuities are, as of
2026-09-20**: `AudioNode::on_discontinuity(Discontinuity)` tells a node that
time stopped being continuous, and names which kind -- `Seek`, `Stop` or
`ProgramChange`. A general "reset to construction state" verb is still not
part of the contract, and nothing has asked for one.

Before it there was one channel for this, and it was the wrong one: the host
synthesised `Event::Choke` into a node's event list, so *let go of these
notes* and *time moved* arrived as the same sentence. The voices heard it and
answered differently -- `release_all` in the synths, a hard fade in the
samplers -- and **everything that is not a voice heard nothing at all**, so a
delay line's contents and a reverb's tail carried across a seek as though they
belonged where the transport now is.

The rules, which are the rest-and-tail rules applied to a moment rather than
to silence:

- Called before the node is handed the block's events and before `process`.
  Every node is told, including sleeping and bypassed ones -- those are
  exactly the ones holding audio they would emit on waking.
- No allocation and no lock. It is the callback.
- **Free-running state keeps running.** An LFO that holds its phase across
  silence holds it across a seek, or a bounce stops matching a take. A node
  wanting that gets it by not implementing the method, which is the default.
- **Read the kind.** A seek invalidates audio in flight; a program change does
  not, and flushing a reverb because the player looked at another pattern is a
  worse artefact than the one this fixes.
- A node that cannot honour it declines in writing. Aux In and the
  retained-audio buffer both do; the buffer's ring is a performance somebody
  is playing, not audio from the old position.

`Event::Choke` remains, and is still correct for what it names: a choke group
cutting a hi-hat off. What changed is that the host stopped borrowing it to
mean something else.

**A device that contains a device forwards every hook it is given.** The
fan-out reaches the outer node and stops: `on_discontinuity`, `skip_block`,
`is_at_rest` and latency all have to be passed on again inside it. ML-P8 is
the case that found the rule -- its finishing chorus is a `ModulationEffect`,
which empties its line on a seek when it stands alone, and nothing was
telling the one inside, so a seek rang across it
(`reports/fable-2026-09-21.md`, finding 4). The plugin slot is the same shape
at a larger size, and `docs/plans/plugin-hosting/` should read it that way.

**A fold and a seek are different kinds.** `Discontinuity::LoopFold` says the
transport turned back at a loop end: time is discontinuous and the music
usually is not. Every node that clears on a `Seek` clears on a `LoopFold`
today, so the sound is what it always was; the variant exists so that a
device *can* keep its tail across the fold without also keeping it across a
seek, which under one name it could not.

**Not yet covered:** the console channel strip (`mooloop-dsp/src/strip.rs`) is
not an `AudioNode` and is not reached by the fan-out, so its EQ and compressor
state still crosses a seek. It has a `reset` of its own, so closing that is a
small change; nothing has heard it yet, which is why it is written down here
rather than guessed at.

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

## Capture

A channel records its audio input into a **take** (`audio-recording/03`,
`crates/mooloop-engine/src/take.rs`). The take lives on the recording
channel's strip -- so an install that carries the strip carries the take --
and holds an `rtrb` ring allocated on the control thread, handed over by
`StructuralCommand::StartTake`, and returned through the reclaim ring. Any
number of channels may hold one.

- **Read site.** Once per block, after every channel and track has rendered
  and before the preview is mixed in (`RenderState::advance_takes`). Each
  source is read where the block left it: a channel's output after its fader,
  pan and compensation, a track's after its balance and compensation, the
  master. So one read site serves every source, the render order does not
  change, and a resample of the master is the render bit for bit.
- **The hardware input** is one more source: a preallocated input bus the
  executor fills from the driver before the block renders
  (`process_with_input`), silent for a driver with no input and for every
  offline render. A take from it starts the driver's round-trip latency after
  its bar line.
- **Two drivers fill that bus differently, and one of them has two clocks.**
  JACK hands the process callback its own capture ports, so input and output
  are the same callback on the same clock and the bus is a copy. cpal has no
  duplex stream, so Core Audio runs a **second stream on a second thread and a
  second device clock**, and the output callback reads its frames from an
  `rtrb` ring the input callback writes (`coreaudio_driver.rs`). The ring is
  primed before the first read so ordinary jitter does not starve it.
  **Drift between two clocks is counted, not corrected**: a ring that starves
  reads silence, a ring that fills drops frames, both are counted, and the
  control thread reports the total when it moves. There is no resampler, so an
  input device that is not the output device will slip over a long take. It is
  not a problem on the machine that matters -- one device, one clock -- and
  fixing it properly is a resampler, which is its own piece of work.
- **Silence, not stale audio.** A channel that did not reach its output this
  block (muted, asleep), a muted or solo-silenced track, and a source that no
  longer exists are recorded as silence.
- **Sample-exact edges.** A take waits for the first bar line strictly after
  it was armed -- the pre-roll -- and starts on the frame that line falls on;
  a clip length ends it on the frame it runs out. A stopped transport ends it.
- **Overflow is counted, never silent.** Frames the ring has no room for are
  added to `TakeStatus::dropped`, and a take with any is reported damaged.
- **The drain** (`mooloop-session`'s `take.rs`) is the only place a take
  touches the disk. It ends when the engine has ended the take and every
  frame is read, or when the ring is abandoned -- the strip rebuilt or the
  channel gone -- and the rest is read. Either way the file is finalized --
  except at quit, which joins nothing and runs no destructor on the drain
  thread, so a take still live when the process exits leaves its header
  unpatched and its audio unreachable (`reports/fable-2026-09-20.md`
  finding 2, 2026-09-20).

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
   2026-09-09**, in `docs/plans/archive/console/` step 05. **Parallel sends** are a
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
