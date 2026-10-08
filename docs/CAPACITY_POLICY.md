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

The remedy is the one the rest of the engine already uses. `RenderState.strips`
is documented as "one entry per channel the project actually has, not per
addressable channel", and `control_outputs` cites
`docs/plans/archive/modulator-capacity/` for the same move. The pattern bank
simply never had it done: it should size from the project and grow through the
structural path the way channels do, leaving every ceiling exactly where it is.
The sequencer is indexed by pattern and channel throughout, so this is a real
change rather than a one-line one; `docs/plans/pattern-bank-floor/` is that
change.

**And it now bounds what can be put in the bank.** Giving every automation
lane slot its point storage up front -- eight slots per `ChannelPattern`, 1024
points of 12 bytes each -- is 96 KB per channel-pattern and **6 GiB** across
the bank, six times the floor this section is about, arrived at by exactly
the multiplication it warns about. Instead a lane keeps the storage it already
has (closing one vacates its slot and keeps the vector, so the callback never
frees), and a slot that has never held one takes a spare from a pool refilled
off the thread. The pool is a reserve, not a cap: when it is empty the lane
still opens. **A ceiling costs nothing; dimensioning by one costs
everything, and it costs more the moment anything per-slot grows.**

### The track bank, measured 2026-09-09

`MAX_BUSES` is seventeen -- a small product cap of exactly the kind this
document opens by forbidding -- and the engine used to preallocate all
seventeen strips whether or not a song had them. `block_cost::track_memory`
measures both halves. **Making strips arrive with the project saved 2.00 MB
of pure floor**, which is the reserving-versus-dimensioning distinction
applied to tracks.

**Raising the ceiling is the other half and is not free.** Almost all of its
price was `DeviceMeters`'s spectrum array, `(MAX_CHANNELS + MAX_BUSES) *
(MAX_EFFECTS_PER_CHANNEL + 1) * SPECTRUM_BINS` -- 12.85 MB of storage for
analyzers that are individually gated by `spectrum_enabled` and almost never
on. The spectra are a pool of `SPECTRUM_SLOTS` now, handed to whichever
stages are subscribed, and the subscription flag doubles as the slot index so
the audio thread's existing atomic load is also the lookup:

```text
                          before        after
  spectrum storage       12.85 MB      12.0 KB   (a pool; does not scale)
  per-bus ceiling cost    48.2 KB       8.0 KB
  MAX_BUSES = 256 adds   11.25 MB       1.87 MB
```

So the ceiling is now liftable for under two megabytes rather than eleven, and
what remains dimensioned by it is honest: six meter cells, a subscription flag
and a collision counter per addressable stage.

The old array was also holding the display quality hostage: the analyzer
wants far more bins (`ENHANCEMENTS.md` has the diagnosis), which at the old
array would have been 68 MB at 256 bins, and is 64 KB now. Dimensioning by a
ceiling does not just cost memory, it makes the thing it dimensions
unimprovable.

### What the floor costs per edit, which is the part that hurts

The gigabyte would be affordable if it were paid once. It is not.
`EngineHandle::install_project` builds a *complete* new `RenderState`,
carrying unchanged strips across by `ChannelId`, and returns the displaced
one through the reclaim ring to be dropped by `poll`, both on the UI thread.
`block_cost::project_install_cost` measures that round trip at **20 ms for a
fifteen-channel project** and 48 ms for a thirty-two channel one;
`docs/plans/pattern-bank-floor/README.md` has the measurements. Repeated at
gesture rate, that saturates the UI thread, which then cannot run the pump on
schedule.

Most edits do not pay it. A knob, a note, and adding or moving an effect are
engine commands. A channel, track or pattern list edit (add, delete, move,
paste, clone, clear) is a `PendingEngineMessage::ProjectEdit` carrying its
`ListEdit`, and the pump sends it to the engine as one command. What still
installs a whole project:

- every undo and redo, including the undo of a knob turn or a note;
- device cut, paste and duplicate;
- opening or starting a song, and loading a kit, channel preset or generator
  preset;
- a list edit the pump merged with older queued edits, or one the engine
  refused.

It does not reach the audio thread. The hypothesis that a gigabyte of
`mmap`/`munmap` would, through page faults and TLB shootdown IPIs, **was
tested and is wrong**: `install_churn_disturbs_a_deadline_thread` runs a
thread on a 21.3 ms period at `SCHED_FIFO` 55, the priority mooloop's own
callback holds, beside a thread installing projects at drag rate, and
ninety-five installs produced zero late wake-ups. The test stays as the
harness for asking the same thing again about some other change.

Two independent fixes, either of which helps:

- make the render state cheap to build, per the section above, so an install
  costs what a fifteen-channel project's worth of state costs rather than what
  256 channels' worth does;
- stop rebuilding it for edits that do not change the whole document. List
  edits have stopped. Undo and redo applying the difference the same way is
  open: MOO-469.

Neither is a thing to do casually — it is the core of the render state and
the edit path either side of it.

## The pattern ceilings are a model decision, not only an engine one

Two of the constants the section above multiplies are also the shape of the
product: a real instrument track -- one channel, a four-minute part, two
thousand notes -- fits nowhere in the current model, and there are two
separate reasons why.

The first is storage. `MAX_PATTERN_STEPS` is 256 sixteenth cells
(`crates/mooloop-core/src/pattern.rs:13`), which at `STEPS_PER_BAR` of 16 is
sixteen bars, and `MAX_NOTES_PER_CHANNEL_PATTERN` is not an independent number
at all -- it is `MAX_PATTERN_STEPS * 4`, so 1024 (`pattern.rs:27`). Both are
reserved in full up front so that `apply_command` never grows them in the
callback. The four-minute part is past the first and the two thousand notes
are past the second.

The second is the arrangement. `PatternPlacement` is a pattern index and a
start tick and nothing else (`crates/mooloop-core/src/playlist.rs:40`);
`Sequencer::schedule_song` walks every active channel of the pattern a
placement names (`crates/mooloop-engine/src/sequencer.rs:850`), and a pattern
carries one `length_steps` shared by all of its channels (`pattern.rs:225`). A
placement is therefore every channel at once, on one 64-bar canvas
(`MAX_PLAYLIST_BARS`, `playlist.rs:8`, with a placement *start* past it refused
at `sequencer.rs:117`). There is no per-channel clip for a long part to live
in, and nothing short of inventing one would give it a place.

**Adam settled this on 2026-09-17, in favour of the groovebox.** Patterns
stay and there are no per-track clips. The same ruling covers audio -- audio
records into the sampler, and there are no per-track audio clips
(`docs/plans/archive/audio-recording/00-status.md`) -- so this is one
decision about notes and audio together. It is a product decision rather than
an implementation accident: pattern-phase swing, `set_pattern_length` and
clip automation are all built on the shared timeline, and every one of them
makes the alternative more expensive without making it more likely.

What the decision does not do is make the ceiling go away. If a part ever
genuinely needs more than sixteen bars, **the answer is to raise
`MAX_PATTERN_STEPS`, not to add clips** -- and that is the move this document
has to be read before making, because the two numbers multiply rather than
stand alone. `MAX_NOTES_PER_CHANNEL_PATTERN` follows `MAX_PATTERN_STEPS`, and
the preallocated bank is `MAX_PATTERNS x MAX_CHANNELS x
MAX_NOTES_PER_CHANNEL_PATTERN x size_of::<NoteEvent>()`, so the floor is linear
in the ceiling: sixteen bars is the 1051 MB measured above (`256 x 256 x 1024 x
16 bytes`, exactly 1.00 GiB), sixty-four bars would be 4 GiB, and the hundred
and twenty bars a four-minute part wants at 120 BPM would be about 7.5 GiB.
**So `docs/plans/pattern-bank-floor/` has to land first.** Raising the ceiling
while the bank is still dimensioned by it multiplies the largest number in the
engine, which is exactly the fault this document opens by naming.

## Current boundaries

- The current channel and effect bridges use complete `u8` address spaces:
  256 of each. These are transitional bridge-format boundaries, not UI policy
  caps. `archive/MIXER_PLAN.md` replaces positional channel/bus addressing with stable
  signal-slot identities and a per-project prepared render plan, removing the
  fixed mixer-bank model rather than normalizing it as permanent.
- Pattern IDs likewise use a complete `u8` address space (256 patterns).
- `MAX_NOTES_PER_CHANNEL_PATTERN` (1,024) is enforced where notes are made:
  `ChannelState::has_room_for` is the one check, every session verb that adds
  a note asks it before changing anything, and a refusal is said in the
  status bar (`Session::take_note_refusal`). The save-time integrity check
  stays as the backstop for a document that arrives over the cap from
  elsewhere.
- Containers nest four deep (`MAX_CONTAINER_DEPTH`), and this is a limit on
  the *gesture* rather than on the format. The number bounds a real
  allocation — one dry buffer per open container in the realtime pass — rather
  than a data structure, which is the distinction the section above is about.

  **The gesture half is enforced and the format half is not.**
  `mooloop_core::can_wrap` and its two siblings refuse a wrap, an insert and a
  drag that would put a box past the cap, and the rack's wrap button asks the
  same function rather than comparing a depth of its own. A deeper chain
  loads **unreported**, and reporting it is not a one-line addition -- see
  `docs/LOOSE_ENDS.md`. A leaf is deliberately not capped: the number bounds
  open runs, so four nested boxes with a filter inside them is legal and it is
  the fifth *box* that is not.
- Event lists, block size, voice pools, sample memory, playlist span, and
  routing have explicit realtime, DSP, or file-format reasons. Any change to
  one must name the reason and show overflow behavior.
- The 16-insert mixer-bus bank is a legacy fixed-graph implementation detail,
  not a product decision. It is the next capacity-sweep candidate; do not use
  it as a precedent for new dynamic collections.
- Modulation has no count a user meets (0.1.7). The song's set is sized from
  the song, and any edit that changes its shape installs a whole new set
  built off the audio thread, the old one dropped off it, which is this
  policy's grow-by-replacement rule (`MODULATION.md`, *Song collection*).
  Only a channel preset's rack keeps the old constants, eight modules and
  sixteen routes, because it is the shape 0.1.6 reads.
- **A note wire holds 64 notes sounding at once** (`note_patch::MAX_LINK_NOTES`,
  song patch step 07), as many as a channel's sequenced-voice table, because
  every source's voice pool is smaller: a link that fills it is already
  stealing. A NoteOn past it, or one its channel's 256-event list has no room
  for, is refused whole and counted, never half-played, and its NoteOff then
  releases nothing; the notes-out tag's edge goes red for a moment.
  `note_patch::tests::a_full_link_refuses_and_counts` is the boundary test.
  Releases owed by a change of set past 256 become a Choke on the channel.
- **A note box holds 64 notes in and 64 pitches out** (song patch step 08),
  the link's number for the link's reason, and writes at most 512 events a
  block (`note_patch::BOX_EVENTS`, twice a channel's list). A NoteOn that
  would pass either, or leave too little room to release everything the box
  then holds, is refused whole; a box has no tag to count it on.

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

- **A channel holds one instrument, not all eight.** `ChannelStrip` holds one
  `Box<dyn SourceNode + Send>`, so a source kind is paid for only on a channel
  that plays it, on the heap; the footprint test (`render.rs`,
  `the_render_graph_costs_what_the_project_uses`) counts the widest one, DS-01
  at 6,960 bytes. The price is that a source change is an ownership move:
  the new device is built on the control thread and the old one leaves
  through the reclaim ring, so the change can land a block or more late when
  the ring is full (Adam, 2026-09-22: *"totally fine"*). The slot has no
  per-kind cap, so a ninth kind, a hosted plugin included, costs nothing
  until a channel plays it.

- **Sends reserve nothing**, the way typed audio edges do not. A track's
  sends are a `Vec` in the document and a `Vec` in the prepared plan; the
  audio thread holds one compensation ring and one `Smoothed` per send that
  exists, and three 64 KB scratch buffers for the *whole engine* — allocated
  only when a project has a send at all. `block_cost::send_memory` shows the
  floor unmoved by sends, the first send buying the shared scratch (193.2 KB,
  once), and each one after costing 24 bytes. What a send costs beyond those
  bytes is its compensation ring, which is as long as the alignment it is
  owed and nothing at all when it is owed none.
  `render::tests::a_project_with_no_sends_allocates_nothing` guards the zero.

  The drawn side counts too. The sends area draws exactly the sends that
  exist and scrolls past the room it has (Adam, 2026-09-09, retiring the four
  send bars `docs/plans/archive/console/THE-STRIP.md` first drew). A drawn
  ceiling is cheaper to remove than a dimensioned one and it is still a
  ceiling, and there was no reason to have one.

## Rule for new work

Before adding a numerical cap to a user-created collection, first use an
off-thread prepared, callback-safe representation. If a finite boundary is
truly required, document it next to the type and in the persisted-format
validation, make the UI communicate it honestly, and add a test at the
boundary. “It was easier to preallocate” is not a sufficient reason.
