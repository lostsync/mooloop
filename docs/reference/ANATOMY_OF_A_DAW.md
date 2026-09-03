# Anatomy of a DAW — reference architecture

Every subsystem in a digital audio workstation: who owns its state, which thread may touch it, and
how the pieces wire together in REAPER, Ableton Live, Bitwig, Ardour and FL Studio — followed by
where a Rust/Slint groove sequencer (Mooloop) plugs into the model.

Emphasis: **ownership & data flow**, **thread / real-time boundaries**. September 2026.
Diagrams from the illustrated version are reproduced here as node/edge listings.

---

## 1. Thread & process domains

A DAW is not organised around features. It is organised around one hard deadline: a callback fires
every 64–1024 frames and must return finished audio before the driver's buffer drains. Miss it and
you get a click, which users hear as a bug in your product regardless of whose plugin caused it.
Every architectural decision below is downstream of that deadline.

> The audio callback is a hostile execution environment that happens to live inside your process.
> Design the app as two programs that share an address space and exchange messages.

Inside the callback you cannot lock (priority inversion against a non-RT thread), allocate or free
(the allocator locks), touch the filesystem or the network, page in memory, or do anything whose
worst case you can't bound. Outside it, you can do all of those — so nearly every subsystem gets
split into a non-RT half that owns the data and an RT half that reads a prepared, immutable view.

### FIGURE 1 — the domain map

```
NON-REAL-TIME                              |  REAL-TIME (audio callback)
                                           |
UI / MAIN THREAD                           |  transport & sample clock
  project model            undo stack      |      v
  arrange / piano roll     mixer views     |  event dispatch
  plugin windows           selection/tools |    (MIDI + automation, split to sample-accurate slices)
  control surfaces         scripting / API |      v
                                           |  graph executor
WORKER POOL (non-RT)                       |    (topologically ordered node list, fixed block size)
  disk streamer            peak/waveform   |      v
  media import             SRC / conform   |  [instruments] [FX nodes] [sends/returns]
  offline render           freeze / bounce |      v
  plugin scan              preset load     |  summing -> bus tree -> master
  autosave writer          node reclaim    |      v
  video decode             index / search  |  capture taps -> record rings
                                           |      v
                                           |  driver buffer out
                                           |
                                           |  (tap) -> meter / param snapshot
                                           |
                                           |  RULES: no locks, no malloc/free, no syscalls,
                                           |  no page faults, no unbounded loops — worst case,
                                           |  every block, forever.

THE FIVE SANCTIONED CROSSINGS
  1. edit commands      non-RT -> RT    SPSC ring, preallocated
  2. graph publish      non-RT -> RT    one atomic pointer store
  3. reclaim queue      RT -> non-RT    superseded graphs, dropped off-RT
  4. audio rings        both ways       disk playback / capture
  5. meters, params     RT -> non-RT    wait-free snapshot
```

Three crossings are message passing, one is a pointer publish, one is a pair of sample ring
buffers. Note what does **not** cross: the project model is never read from the audio thread, and
the audio graph is never mutated in place from the UI thread. If you want a sixth crossing, you
usually want a different data layout instead.

**Bitwig goes one step further.** It separates *processes*, not just threads: application, audio
engine and plugins run in different processes, so an engine crash leaves the UI alive ("simply
reload the engine and keep working") and a plugin crash doesn't take the engine with it. Plugin
hosting granularity is user-selectable — within Bitwig, together, by manufacturer, by plugin, or
individually — a straight RAM-versus-isolation dial.

---

## 2. Who owns what

"Ownership" means: exactly one thread may mutate this, and everyone else gets a copy, a snapshot,
or a message. Most DAW bugs that present as glitches, stuck notes or corrupted projects are
ownership violations.

| Subsystem | Authoritative owner | Mutated on | What RT sees | Crossing |
|---|---|---|---|---|
| Session / project model | UI thread | UI thread only | nothing | — |
| Undo history | UI thread | UI thread only | nothing | — |
| Audio graph (node list) | UI builds, RT reads | built off-RT, published | immutable snapshot | atomic pointer swap |
| Node DSP state | RT thread | RT only, after publish | owns it outright | — |
| Plugin parameters | split (see below) | both, by protocol | value + gesture events | command ring both ways |
| Plugin opaque state | plugin | plugin main thread | nothing | save/load off-RT only |
| Transport position | RT thread | RT advances it | owns it outright | snapshot out for the UI |
| Tempo / time-sig map | UI thread | built off-RT | immutable snapshot | published with graph |
| Automation curves | UI thread | built off-RT | baked event stream | published with graph |
| MIDI clip data | UI thread | built off-RT | sorted, flattened events | published with graph |
| Media files & peaks | worker pool | workers | ring buffer contents | audio ring (play) |
| Record capture | RT writes | RT appends frames | owns the write side | audio ring (capture) |
| Meters / scopes | RT writes | RT stores atomics | owns the write side | wait-free snapshot |
| Plugin database | worker pool | scan worker | nothing | — |

### The parameter problem

Parameters are the one place where bidirectional ownership is unavoidable: a knob can be turned in
the GUI, driven by automation, moved by a control surface, or changed by the plugin itself. The
standard resolution: **nobody owns the value; the RT thread owns the *current* value and everyone
else owns *requests*.** Each source posts timestamped change events to the RT thread; the RT thread
applies them in order and publishes the result back for display. Gesture begin/end markers (touch,
latch) tell the automation writer when a human is holding the control — that is what makes
touch-mode automation work.

---

## 3. The audio graph

Two models compete and most real DAWs are a hybrid.

- **Fixed pipeline** — a track is a hardcoded chain (input, instrument, insert slots, fader, pan,
  sends, output) and "routing" means choosing where the taps go. Far easier to make deterministic,
  to compensate for latency, and to render in a UI.
- **Free graph** — everything is a node, connections are arbitrary. This is what people mean when
  they say a DAW is modular.

Ardour, Pro Tools and REAPER's track model are fixed pipelines with escape hatches (REAPER's
routing matrix and nestable track-as-bus is a very wide hatch). Bitwig's device chains and Reason's
rack are free graphs inside a fixed outer track. FL Studio splits it: pattern-based channels feed a
fixed mixer whose insert routing you patch by hand.

### FIGURE 2 — a track is a pipeline with named taps

```
AUTOMATION & MODULATION (parameter ports, not signal ports)
  curves · LFOs · envelopes · macros · control surface · MIDI CC
        |            |             |
        v            v             v
  [audio in 3/4]\
  [MIDI in/keys ]-> input stage -> instrument -> FX chain -> fader & pan -> output
  [timeline item]/  (arm/monitor)  (events->audio) (serial)   (gain/pan law)  (bus assign)
                                                     ^   |          |            |
  [sidechain src] - - - - - - - - - - - - - - - - - -+   |          |            |
   (2nd input port on a node, NOT a chain link)          |          |            |
                                              pre-fader tap    post-fader tap    |
                                                       v          v              |
                                                    FX bus    reverb return      |
                                                  (parallel)  (shared, 100% wet) |
                                                       \          /              /
                                                        v        v              v
                                          master bus — summing, master chain, limiter
                                                             |
                                                             v
                                       hardware out · render target · monitoring path
```

The pre-fader tap is what makes a headphone cue mix independent of the fader; the post-fader tap is
what makes a reverb send track the fader. Sidechain arrives as a **second input port on a node**,
never as a chain link — which is why it can legally come from a track further down the topological
order only if you accept one block of delay.

### Scheduling the graph

Once per block the executor walks a pre-computed topological order. Feedback is either forbidden or
resolved by inserting a one-block delay at a designated break edge — you must choose, because a
cycle in a pull-model graph is an infinite recursion. Parallelism is where hosts differ most:

- **Per-block fork/join.** Split the node list into levels, hand each level to a worker pool,
  barrier between levels. Simple, correct, wastes cores whenever a level is uneven — which is
  always, because one track has a convolution reverb and eleven have a gain node.
- **Work-stealing over the DAG.** Each node becomes ready when its predecessors finish; idle
  threads steal. Better utilisation, harder to keep deterministic and much harder to keep RT-safe.
- **Anticipative processing.** REAPER runs plugin processing "at irregular intervals, often out of
  order, and slightly ahead of time," buffering results so cores rarely need to synchronise —
  Cockos report over 95% utilisation across eight cores. The cost is latency you have to give back:
  it is disabled for live input monitoring, where you cannot precompute audio not yet played.

**Every parallelism scheme buys throughput with lookahead, and lookahead is exactly what a
live-monitoring or live-looping path cannot have.** Expect two scheduling modes in any DAW that
takes both seriously.

---

## 4. Mutating the graph without stopping the music

The single most important mechanism in the design, and the one most hobby DAWs get wrong by
reaching for a mutex. The user drags a plugin into a chain while audio is playing. The audio thread
is, at that instant, halfway through a block. You cannot lock it out, and you cannot reshape a
structure it is walking.

### FIGURE 3 — copy-on-write publication

```
UI / EDIT THREAD                      SHARED                 AUDIO CALLBACK

1. clone the node list          
   (shallow: handles, not DSP state)
        v
2. apply the edit
   (insert node, rewire, re-sort, re-PDC)
        v
3. publish  ------------------>  current graph  ---------> 4a. load the pointer, ONCE
   (one store, release)          (one atomic                   (top of block, acquire)
                                  pointer — the                     v
                                  whole handoff)              4b. process the whole block
                                                                  against that snapshot
                                                                  and no other
                                                                      v
5. drop the old graph  <-----  reclaim ring  <------------  4c. superseded? push the handle
   (allocator lock is legal      (bounded, never                 out — push only, never
    here)                         blocks the callback)           drop on this thread
```

Two details make or break this in practice:

1. **DSP state must survive the swap** — the reverb tail cannot restart because you inserted an EQ
   three slots earlier. Nodes are reference-counted and shared between the old and new graph; only
   the topology is copied.
2. **The edit thread must not spin publishing** — dragging a fader sends a parameter message down
   the command ring, it does not rebuild a graph sixty times a second. *Structural edits rebuild;
   value edits message.*

In Rust: the published graph is an `Arc<Graph>` swapped through `arc-swap` or a hand-rolled
`AtomicPtr`; nodes are `Arc<Node>` shared across versions; the reclaim path is an SPSC ring of
`Arc`s that a non-RT thread drains and drops. The borrow checker will not save you from the RT
rules — nothing in the type system stops you calling `Vec::push` in a callback — so the discipline
must be enforced by module boundaries and by never handing the RT half an allocator-owning type.

---

## 5. MIDI, automation and modulation

Three features to a user, one subsystem to the engine: **timestamped events delivered to a node's
parameter or note ports, sample-accurately, within a block.** Only the origin and storage differ.

- **MIDI** — from clips (baked into a flat, sorted list when the graph is published) or from
  hardware (arrives asynchronously, timestamped and queued into the next block).
- **Automation** — a curve in the project model, sampled into events at publish time or evaluated
  on the fly against the transport position. Per-parameter, with breakpoints, interpolation shape,
  and a per-lane mode (read/touch/latch/write).
- **Modulation** — automation whose source is another node: LFO, envelope follower, macro knob,
  step sequencer. Bitwig's unified modulation system is the fullest expression: any modulator to
  any device, track or plugin parameter, at audio rate.

Consequence: **parameters need two update paths** — an event path for sample-accurate changes and a
per-block path for smoothed values. Anything that would zipper (gain, cutoff, pan) gets a one-pole
smoother inside the node, driven by the target value the events set. This is also what "automation
resolution" settings actually control.

### FIGURE 4 — why events carry sample offsets

```
ONE BLOCK — 512 FRAMES @ 44.1 kHz
frame 0                                                                    frame 512
|--- render 0–114 ---|--- render 114–282 ---|--- render 282–396 ---|-- render 396–512 --|
                     ^                      ^                      ^
                 note on @114      automation pt @282          note off @396

sample-accurate: the block is split at every event offset; nodes are called once per sub-range

naive (apply everything at block start):
|<--------------------------- one grid step -------------------------------->|
  every note and every curve point quantises to an 11.6 ms grid — audible as sloppy timing,
  stair-stepped filter sweeps, and a groove that drifts with buffer size
```

The cost of getting this wrong scales with buffer size, so it passes testing at 64 frames and falls
apart at 1024 — which is exactly the setting people use when mixing. Build the sub-range split in
from the start; retrofitting means touching every node's `process()` signature.

---

## 6. Latency and delay compensation (PDC)

Any node may report a processing latency — linear-phase EQ, lookahead limiter, oversampled
saturator, amp sim with a long IR. The host must ensure that when two signals rejoin, they rejoin
in phase. This is a graph-wide computation, not a per-track one: delay inserted on one path becomes
latency for everything downstream of the merge.

### FIGURE 5 — PDC pushes latency downstream

```
WITHOUT COMPENSATION
                 /--> linear-phase EQ (reports 2048 frames) --\
   source ------<                                              >--> sum
                 \--> dry path — 0 frames ---------------------/

   result:   dry |                       | wet
                 |<---- 46.4 ms -------->|      comb filtering, smeared transients

WITH COMPENSATION
                 /--> linear-phase EQ — 2048 ---------------\
   source ------<                                            >--> sum
                 \--> HOST-INSERTED DELAY — 2048 frames ----/

   result:   both ||     aligned — and the whole path is now 2048 frames late
```

Compensation is a graph-wide fixed point: compute each node's cumulative latency, delay every
shorter sibling path to match the longest, propagate the maximum to the parent. Rebuild on every
structural edit (step 2 of Figure 3). Three consequences:

- **Live monitoring cannot be compensated.** You cannot delay a signal that hasn't been played yet.
  Hosts bypass the compensated path for armed tracks (REAPER's "reduce record latency", Pro Tools'
  low-latency mode) or push monitoring to the interface's own mixer. Same trade-off as anticipative
  processing — it argues for a distinct low-latency path through the graph, not a flag on the
  normal one.
- **The transport must report compensated positions.** A plugin behind 2048 frames of delay needs
  the song position *it* is rendering, or its tempo-synced LFO drifts.
- **Latency changes are structural edits.** A plugin switching 4x -> 16x oversampling mid-session
  triggers a recompute and a republish, not an in-place patch.

---

## 7. Time, transport and sync

There is exactly one authoritative clock, and it is the audio device's sample counter. Bars, beats,
SMPTE timecode, video frames and every external sync source are **mappings onto that counter**, not
clocks in their own right. Letting the tempo map or an external clock be the source of truth
produces the classic symptom of audio and MIDI drifting apart over a long session.

### FIGURE 6 — one clock, many mappings

```
EXTERNAL SOURCES                                              CONSUMERS

Ableton Link           \                                 /   event scheduler
 (beat, phase, quantum) \                               /     (clip -> block, with offsets)
MIDI clock in            \                             /     grid, snap, quantise
 (24 ppqn, jittery)       \-> tempo & meter map ------>/       (editing and looper launch)
MTC / LTC                 /   (beats <-> samples,      \      plugin transport info
 (SMPTE, for picture)    /     both directions)         \      (PPQ, bar start, tempo, playing)
host transport          /            ^                   \   playhead & scroll
 (when you are a plugin)             |                    \   (via snapshot, never a shared int)
                                     v
                            SAMPLE POSITION
                     monotonic, advanced by the device
                        THE ONLY SOURCE OF TRUTH
                                     |
                                     v
                          clock & timecode out
                    (MIDI clock, MTC, Link tempo publish)

An external clock never drives the sample counter — it adjusts the map, or you resample.
Two free-running crystals always drift; you choose where to absorb it.
```

Ableton Link is a good model to copy: its `SessionState` is captured and committed separately from
the audio thread and the app thread, with only the audio-thread pair documented as realtime-safe,
and the host is responsible for mapping Link's beat timeline onto its own sample time.

**Tempo maps.** A map that supports ramps needs an integral, not a lookup: converting a beat to a
sample means integrating `1/tempo(b)` over the interval. Store cumulative sample offsets at each
tempo node so conversion is O(log n) with a binary search, and cache the segment the transport is
currently inside — the RT thread asks for this many times per block.

**Loop points** make it worse: with a loop active, sample position is no longer monotonic in song
time even though it is monotonic in wall-clock time. Keep both. Nodes with tails (reverbs, delays)
need the wall-clock one, or they reset at every loop wrap.

---

## 8. Recording and live looping

Recording is the capture ring in Figure 1 read from the other end: the RT thread appends frames
into a preallocated ring, a disk thread drains it and writes. Ardour names this thread the
**butler**, and its design is worth copying — the process callback queues non-realtime work in a
bitset (`post_transport_work`) and wakes the butler, which checks between each block of I/O whether
transport work has arrived, so a large disk operation can never delay a locate or a stop.

Sizing is the whole game. The ring must cover the worst-case disk stall, not the average one; on
playback the same applies in reverse, with a prefetch window sized to cover a seek. Both are
bounded by RAM — which is why DAWs have a "disk buffer" preference, and why the answer to "why does
my project stutter when I scrub" is almost always that the prefetch window was invalidated faster
than the butler could refill it.

### Live looping is a state machine, not a feature

Live looping adds one requirement the rest of the DAW doesn't have: **state transitions must occur
at musically exact sample positions, decided in advance**, because the human pressed the pedal 40 ms
early. The transition is *scheduled*, not executed, when the input arrives — and the loop buffer is
allocated at arm time, off the RT thread, sized generously, because you cannot allocate when the
first pass turns out longer than you guessed.

### FIGURE 7 — the looper state machine

```
                                          overdub  <--toggle, quantised-->  playing
                                             |                                ^
                                             v                                |
                                        layer stack                           |
                                        (undo / redo)                         |
                                                                              |
  idle --arm--> armed --on the bar--> recording --length latched-------------/
   ^                                  (first pass sets length)                |
   |                                                                     --stop-->  stopped
   |                                                                                   |
   \------------------ clear (buffer returned to the pool off-RT) --------------------/

RT thread: flips a state variable, writes into a preallocated buffer, schedules the next
           transition. Nothing else.
Off-RT:    allocates the buffer at arm time (quantum x max passes); snapshots layers for undo;
           writes to disk if the loop is also a take.
```

Every quantised transition is **scheduled** at a sample position computed from the tempo map, not
executed when the trigger arrives. Build layer undo as a stack of buffers rather than in-place
summing, or "undo last overdub" becomes impossible — the classic hardware-looper limitation, and
one you have no reason to reproduce.

Punch recording, comping and take lanes fall out of the same machinery: a take is a captured region
with its own start offset, a comp is an edit list over takes, punch is a scheduled arm/disarm pair.
Keep captured audio immutable and represent every edit as a reference into it — that is what makes
non-destructive editing and unlimited undo cheap.

---

## 9. Plugins and virtual instruments

A plugin is a third-party DSP node with its own GUI, its own state format, its own opinions about
threading, and a non-zero chance of crashing or blocking inside your audio callback. Everything
unusual about plugin hosting follows from that last clause.

| Format | Parameters | Threading contract | What it costs the host |
|---|---|---|---|
| VST3 | normalised, sample-accurate queues | defined, but GUI/DSP separation is convention-heavy | bus/arrangement negotiation is fiddly |
| AU / AUv3 | AudioUnitParameter, render-notify | AUv3 gives real process isolation on Apple platforms | Apple-only; two eras of API |
| CLAP | events in the same stream as notes | explicit: every call annotated audio-thread or main-thread | little — the contract is the selling point |
| LV2 | ports, typed; atom messages | strict RT annotation via feature declarations | smaller commercial ecosystem |
| VST2 | float 0–1, opcodes | largely undefined — assume nothing | legacy burden, no licence |

CLAP matters beyond its threading annotations because it moves a whole class of problem to the
host: rather than each plugin spawning its own worker threads — which is how you get twelve neural
amp sims fighting for cores — the **host** owns a pool of realtime threads and lends them out, and
can prioritise plugins that must finish early, such as those on a low-latency armed track. That is
exactly the scheduling authority the host needs anyway, and it makes a host-side thread pool a
first-class subsystem rather than an implementation detail.

**Sandboxing.** Running plugins out-of-process costs a context switch and a shared-memory hop per
block, and buys crash isolation, mixed-architecture bridging (32-bit, x86 on ARM, Windows plugins
under Wine), and a responsive UI while a plugin's constructor takes four seconds. Bitwig exposes
the granularity as a user setting — in-engine, all together, per manufacturer, per plugin, per
instance — and changing it requires an engine reload.

**The scan is a subsystem, not a script.** It must run out-of-process (a bad plugin will crash it),
be incremental and cached, record failures so you don't retry a known-bad binary every launch, and
be resumable. Users judge a DAW by its first launch, and the first launch is a plugin scan.

---

## 10. Track data, project model and undo

The project model is the DAW's real product — the audio engine is replaceable, the file format is
forever. It is owned entirely by the UI thread and is the only structure the RT side never touches.

- **Tracks** hold identity, routing, chain contents, automation lanes and clip references — not audio.
- **Clips / items** are references into media with an offset, length, fades, gain, stretch mode and
  loop flag. Immutable source, mutable reference: that is the whole non-destructive editing story.
- **Media** lives in a content-addressed or path-referenced pool with sidecar peak files. Peaks are
  a cache, must be regenerable, and must never block the UI — draw progressively.
- **Plugin state** is an opaque blob per instance plus the host-visible parameter values. Save
  both: the blob for fidelity, the parameters so you can still show a mixer when the plugin is missing.

**Undo** should be a command log over the project model, not snapshots — snapshots of a large
session are too big to take on every fader move, and coalescing works naturally on commands (a drag
becomes one entry). Commands must be invertible and carry enough context to reapply after unrelated
edits. Undo and the RT thread are almost entirely decoupled: undoing an edit is just another
structural edit that produces another graph publication.

**Autosave and crash recovery** belong to a worker: take a consistent snapshot of the project model
on the UI thread, serialise and write it on the worker, write to a temp file, rename atomically.
Never serialise while the model can change under you.

---

## 11. How the professional hosts actually differ

The subsystems above are near-universal. What distinguishes real DAWs is which single idea the
architecture is bent around. Externally observable behaviour except where a source is cited.

| Host | Graph model | Parallelism | Plugin isolation | Organised around |
|---|---|---|---|---|
| REAPER | fixed track pipeline, wide-open routing matrix; any track can be a bus | anticipative — out of order, ahead of time | optional per-plugin dedicated process / bridging | uniformity: everything is a track, everything is scriptable |
| Bitwig | nested device chains; free modulation graph inside a fixed track | multi-core engine in its own process | selectable: engine, together, by vendor, by plugin, per instance | modulation as a first-class citizen |
| Ableton Live | device chains and racks; fixed insert order per track | per-track threading with a serial chain limit | in-process; scanning and bridging separated | the Session/Arrangement duality — clip launch is the engine, not a view |
| Ardour | processor box per route; JACK-style arbitrary port connection | graph-level threading over a process callback | in-process; LV2/VST with optional bridges | the butler — explicit RT/non-RT split, documented |
| FL Studio | channel rack -> mixer inserts, routed by hand | per-generator and per-mixer-track threading | per-plugin bridged process option | the pattern, not the timeline — the playlist arranges patterns |
| Pro Tools | strict fixed pipeline, rigid insert/send slots | native pool plus optional hardware DSP path | AAX only, tightly specified | parity with a large-format console and its workflow |

Read that as a warning as much as a menu. **The organising idea propagates everywhere.** FL Studio's
pattern-first model is why its playlist behaves unlike any linear DAW's timeline; Bitwig's
modulation system is why its parameter subsystem had to be audio-rate from day one; Ardour's
documented butler design is why its transport handling is legible to contributors. Picking the idea
late means retrofitting it, and retrofitting an organising idea is a rewrite.

---

## 12. Mooloop overlay

A FruityLoops-adjacent groove sequencer in Rust with a Slint UI already contains four of the
subsystems above: pattern/step data, instrument nodes with voice allocation, a mixer, and a UI
bound to parameters. What it probably does not yet have is the boundary in Figure 1 — the piece
whose absence gets more expensive every week.

### FIGURE 8 — the growth path

```
EXISTS TODAY                              GROWTH PATH (dependency order, not user value)

  pattern grid & step data                1. command ring + graph publish
  voice allocation & synth nodes    ----->    the refactor everything else assumes —
  mixer & master output                       do it while it is small
  Slint UI & parameter binding            2. sample-accurate event dispatch
                                              attaches to: the step sequencer you have
SETTLE THESE FIRST —                      3. plugin host, out of process
EACH IS A REWRITE LATER                       attaches to: the node interface — CLAP first
                                          4. automation lanes & modulators
  1. does process() take a sub-range,         attaches to: parameter ports — free once 2 exists
     or a whole block?                    5. arrangement over patterns
     -> every node's signature                attaches to: pattern data — the FL model,
                                              not a linear timeline
  2. fixed pipeline or free graph?        6. audio tracks + disk streaming
     -> PDC as a walk or a fixed point        attaches to: the mixer — brings the butler,
                                              peaks, media pool
  3. node state behind Arc, or owned      7. live looper
     by the engine?                           attaches to: capture rings + tempo map —
     -> whether a graph swap can               needs 6, wants 8
        preserve tails                    8. sync — Link, MIDI clock, host transport
                                              attaches to: the tempo map — cheap if
  4. is the sample counter the only            choice 4 went well
     clock?
     -> sync as a feature or a refactor
```

Items 1 and 2 are pure infrastructure and will feel like a week with nothing to show. Every item
below them is two to five times more expensive if they are skipped, because each one adds nodes,
parameters and threads to a structure that cannot yet be edited safely while playing.

### Rust specifics worth deciding early

- **The RT half should not be able to see an allocator.** Split the engine into a crate whose
  public types are all preallocated and whose dependencies are audited for allocation. You cannot
  get this from the borrow checker, but you can get most of it from a module boundary and a deny-list.
- **Use `Arc` for node sharing and never `Arc::drop` on the RT thread.** Push handles to a reclaim
  ring; a worker drains it. `arc-swap` gives you the publish side; `rtrb` or a hand-rolled SPSC
  gives you the rings.
- **Slint is on the UI side of the boundary, full stop.** Meters and playhead come from a wait-free
  snapshot polled on a UI timer, not from a channel the audio thread might block on. A bounded
  channel that drops on full is correct here — a missed meter frame is invisible, a blocked
  callback is a click.
- **Consider shipping the engine as a CLAP plugin too.** If the node interface and the host
  transport contract are clean enough that Mooloop can run inside another DAW, they are clean
  enough to host other people's plugins — and it is a much cheaper way to find out than building
  the host first.

### The one thing worth arguing about

Item 5 — arrangement over patterns — is where the identity of the product is decided, not item 6.
If Mooloop is FruityLoops-adjacent, the pattern is the unit and the arrangement is a sequence of
pattern placements; a linear multitrack timeline bolted on later will fight the step data forever.
That is FL Studio's organising idea and the reason its playlist behaves the way it does. Choose it
deliberately now, or inherit it by accident.

---

## Sources

Claims about specific hosts; everything else is general practice, and the comparison table
describes externally observable behaviour except where a source is given.

- Bitwig — Modern Foundations: https://www.bitwig.com/modern-foundations/
- Bitwig — plug-in handling & options: https://www.bitwig.com/userguide/latest/vst_plug-in_handling_and_options/
- Ardour — transport threading design: https://ardour.org/transport_threading.html
- Sound On Sound — REAPER, running multiple plug-ins: https://www.soundonsound.com/techniques/running-multiple-plug-ins
- CLAP — thread pool: https://cleveraudio.org/1-feature-overview/_thread-pool/
- Ableton Link — documentation: https://ableton.github.io/link/
