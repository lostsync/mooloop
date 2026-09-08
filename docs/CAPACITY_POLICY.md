# Capacity Policy

Mooloop must not impose small product caps on dynamic musical items. A user
should not encounter a limit such as “16 channels” or “8 effects” because it
made an implementation convenient.

The audio callback cannot allocate, block, or perform unbounded work. That is
an engine constraint, not permission to turn every collection into a tiny
fixed bank. Dynamic state is prepared off the audio thread; callback-facing
storage is then preallocated for the complete supported address space.

## Reserving is not the same as bounding

A ceiling costs nothing. *Dimensioning by* a ceiling costs a great deal, and
the two are easy to confuse. `MAX_CHANNELS` and `MAX_EFFECTS_PER_CHANNEL` are
both the `u8` index space, and the render graph once reserved their product —
65,536 effect slots, each with a 320-byte pending queue — so an empty project
paid 42.8 MiB before it held anything. Boxing the slot state and materializing
channels from the project took a sixteen-channel project to 1.1 MiB with both
ceilings untouched (`docs/plans/archive/modulator-capacity/00-status.md`).

The lesson is the one this policy already implies, stated the other way round:
a large address space is fine, and preallocating the whole of it in advance is
the thing to avoid. The number was invisible at every individual definition
and only appeared when they were multiplied, which is why that plan left a
test measuring the whole graph rather than a paragraph.

## The same lesson, unlearned: the pattern bank

The paragraph above was written about effect slots and is currently true of
patterns, at twenty-five times the scale.
`block_cost::prepared_project_memory` measures it: **a project with no
channels at all allocates about 1.07 GB**, and a fifteen-channel one with a
full chain on every channel allocates 1.10 GB. The song is nearly irrelevant;
the floor is the number.

`block_cost::render_state_floor_by_component` says where it is, and it is one
line:

```rust
// Sequencer::new
let patterns = (0..MAX_PATTERNS)
    .map(|_| Pattern::with_steps(MAX_CHANNELS, MAX_PATTERN_STEPS as usize))
    .collect();
```

`MAX_PATTERNS`, `MAX_CHANNELS` and `MAX_PATTERN_STEPS` are 256, 256 and 256.
That is 65,536 `ChannelPattern`s of 256 steps each — measured at **1051 MB of
the 1070**, against 13 MB for `DeviceTelemetry`, 1.6 MB for `DeviceMeters` and
under half a megabyte for all 256 modulator racks together. `Sequencer::new`
takes `initial_channels` and `active_patterns` and uses neither for sizing:
they set counters on a bank that was already built at full extent.

It is the same product this policy already has a paragraph about — the render
graph's `MAX_CHANNELS × MAX_EFFECTS_PER_CHANNEL`, which cost 42.8 MiB and was
fixed — and it was invisible for the same reason: each constant is defensible
where it is defined, and nothing multiplies them where a reader would look.

The remedy is the one the rest of the engine already uses. `RenderState.strips`
is documented as "one entry per channel the project actually has, not per
addressable channel", and `control_outputs` cites
`docs/plans/archive/modulator-capacity/` for the same move. The pattern bank
simply never had it done: it should size from the project and grow through the
structural path the way channels do, leaving every ceiling exactly where it is.

Not yet done. The sequencer is indexed by pattern and channel throughout, so
this is a real change rather than a one-line one, and the measurements are
committed so it has a before to point at.

### What the floor costs per edit, which is the part that hurts

The gigabyte would be affordable if it were paid once. It is not.
`EngineHandle::install_project` builds a *complete* new `RenderState` and
returns the displaced one through the reclaim ring to be dropped by `poll` —
both on the UI thread — and every `PendingEngineMessage::ProjectEdit` goes
through it. `block_cost::project_install_cost` measures that round trip at
**20 ms for a fifteen-channel project** and 48 ms for a thirty-two channel
one.

A pointer drag reports an edit on every move frame; `history::Entry::gesture`
exists precisely because it does. So a drag asks the UI thread for a 20 ms
allocate-and-free of a gigabyte, sixty times a second, and the thread
saturates — measured at 65% of a core sustained across a 46-minute session,
against 0.8% when idle.

It was also proposed here as the reason an edit is *audible* — that a
gigabyte of `mmap`/`munmap` reaches the audio thread through page faults and
TLB shootdown IPIs, which no scheduling priority defers. **That was tested and
it is wrong.** `install_churn_disturbs_a_deadline_thread` runs a thread on a
21.3 ms period beside a thread installing projects at drag rate, and on the
machine this was reported from — eight cores, `SCHED_FIFO` 55, the priority
mooloop's own callback holds — ninety-five installs produced *zero* late
wake-ups, with a worst lateness of 0.02 ms against a 21.3 ms period. The idle
column was 0.03 ms, so the loaded run was if anything quieter.

What the install cost does explain is the interface. A saturated UI thread
cannot run the pump on schedule, and the pump is what advances the position
readout — so the clock reads unevenly for the same reason the drag stutters,
with no audio fault involved at all.

The dropouts reported alongside this had a complete and separate cause:
`rtkit` had demoted every realtime thread on the machine, leaving PipeWire's
data loop at `SCHED_OTHER`. `docs/OPERATIONS.md` records how to recognise it.
The test above stays because a refuted hypothesis with a measurement behind it
is worth more than an open question, and because it is the harness for asking
the same thing again about some other change.

Two independent fixes, either of which helps:

- make the render state cheap to build, per the section above, so an install
  costs what a fifteen-channel project's worth of state costs rather than what
  256 channels' worth does;
- stop rebuilding it for edits that do not change structure. A note move or a
  knob is already expressible on the POD command path, and the structural
  command path already exists for the ones that are not.

Not yet done, and not a thing to do casually — it is the core of the render
state and the edit path either side of it. The measurements are committed so
the decision has a before to point at.

## Current boundaries

- The current channel and effect bridges use complete `u8` address spaces:
  256 of each. These are transitional bridge-format boundaries, not UI policy
  caps. `MIXER_PLAN.md` replaces positional channel/bus addressing with stable
  signal-slot identities and a per-project prepared render plan, removing the
  fixed mixer-bank model rather than normalizing it as permanent.
- Pattern IDs likewise use a complete `u8` address space (256 patterns).
- Containers nest four deep (`MAX_CONTAINER_DEPTH`), and this is a limit on
  the *gesture* rather than on the format: a deeper chain loads and is
  reported by the integrity pass the way an over-long one is. The number
  bounds a real allocation — one dry buffer per open container in the realtime
  pass — rather than a data structure, which is the distinction the section
  above is about.
- Event lists, block size, voice pools, sample memory, playlist span, and
  routing have explicit realtime, DSP, or file-format reasons. Any change to
  one must name the reason and show overflow behavior.
- The 16-insert mixer-bus bank is a legacy fixed-graph implementation detail,
  not a product decision. It is the next capacity-sweep candidate; do not use
  it as a precedent for new dynamic collections.
- Modulation capacity — eight modules and sixteen routes a channel — is a
  compile-time constant with a measured, linear price, deliberately rather
  than a layout assumption: the grid's rows follow the constant and a test
  renders the shelf at eight and at sixteen so a re-introduced literal fails.
  Raising it is one edit. It stays a constant because a variable-length rack
  on the realtime path buys a bounds check every tick and an allocation story
  every edit, to save memory the reservation fix already recovered.

- Typed audio edges reserve nothing at all, which is the shape this policy
  asks for. A channel's subscription is one optional value; the *buffers* are
  the expensive part — 64 KB each — and exist only for the distinct
  (producer, outlet) pairs somebody actually reads, allocated off the audio
  thread and installed with the schedule they belong to. A project that has
  never authored an edge holds none, and the footprint test says so. The two
  finite numbers around it are declaration bounds rather than caps on
  anything a user creates: `MAX_DEVICE_AUDIO_TAPS` (8) is the widest audio
  outlet table a *device* may declare, checked by `outlet::tests::check_table`
  so an over-wide table fails at its own test rather than losing its last
  outlets silently; `aux_in::MAX_SOURCE_OUTLET` (63) is the highest outlet id
  the Aux In selector can name, eight times the widest table declared, with a
  test that fails the day a device publishes an id past it.

## Rule for new work

Before adding a numerical cap to a user-created collection, first use an
off-thread prepared, callback-safe representation. If a finite boundary is
truly required, document it next to the type and in the persisted-format
validation, make the UI communicate it honestly, and add a test at the
boundary. “It was easier to preallocate” is not a sufficient reason.
