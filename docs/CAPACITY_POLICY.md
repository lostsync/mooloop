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

**And it now bounds what can be put in the bank.** On 2026-09-21 the fix for
opening an automation lane on the audio thread
(`reports/fable-2026-09-21.md`, finding 1) was written as "give every lane
slot its point storage up front". Eight slots per `ChannelPattern`, 1024
points of 12 bytes each, is 96 KB per channel-pattern and **6 GiB** across
the bank -- six times the floor this section is about, arrived at by exactly
the multiplication it warns about, and nobody would have noticed at any
single definition. What landed instead keeps the storage a lane already has
(closing one vacates its slot and keeps the vector, so the callback never
frees) and hands a slot that has never held one a spare from a pool refilled
off the thread. The pool is a reserve, not a cap: when it is empty the lane
still opens. **A ceiling costs nothing; dimensioning by one costs
everything, and it costs more the moment anything per-slot grows.**

### The track bank, measured 2026-09-09

The same lesson again, found and half-fixed the same day. `MAX_BUSES` was
seventeen -- a small product cap of exactly the kind this document opens by
forbidding -- *and* the engine preallocated all seventeen strips whether or
not a song had them. `block_cost::track_memory` measures both halves:

```text
  a project with 1 track          1067.95 MB
  a project with 17 tracks        1069.95 MB
  marginal cost of one track        128.0 KB

  dimensioned by MAX_BUSES = 17, whatever the song holds:
    DeviceMeters spectrum           12.85 MB   (273 targets x 257 stages x 48 bins)
    ...of which per bus              48.2 KB
```

**Making strips arrive with the project saved 2.00 MB of pure floor**, which
is the reserving-versus-dimensioning distinction applied to tracks. It is a
small number beside the gigabyte above and it is the whole of what a fixed
bank was buying.

**Raising the ceiling is the other half and is not free.** At 48.2 KB per bus
of fixed cost, taking `MAX_BUSES` to the `u8` address space would add 11.25 MB
before anybody makes anything:

```text
  MAX_BUSES =  32  adds    0.71 MB      MAX_BUSES = 128  adds    5.22 MB
  MAX_BUSES =  64  adds    2.21 MB      MAX_BUSES = 256  adds   11.25 MB
```

The reason was this document's own subject one level down: almost all of it
was `DeviceMeters`'s spectrum array, `(MAX_CHANNELS + MAX_BUSES) * (MAX_EFFECTS_
PER_CHANNEL + 1) * SPECTRUM_BINS` -- 12.85 MB of storage for analyzers that are
individually gated by `spectrum_enabled` and almost never on.

**Fixed the same day**, and the numbers moved as predicted. The spectra are a
pool of `SPECTRUM_SLOTS` now, handed to whichever stages are subscribed, and
the subscription flag doubles as the slot index so the audio thread's existing
atomic load is also the lookup:

```text
                          before        after
  spectrum storage       12.85 MB      12.0 KB   (a pool; does not scale)
  per-bus ceiling cost    48.2 KB       8.0 KB
  MAX_BUSES = 256 adds   11.25 MB       1.87 MB
  a 1-track project      1067.95 MB   1055.11 MB
```

So the ceiling is now liftable for under two megabytes rather than eleven, and
what remains dimensioned by it is honest: six meter cells, a subscription flag
and a collision counter per addressable stage.

It also unblocked something that was not the point and turned out to matter
more. The analyzer is blocky and unfluid (`ENHANCEMENTS.md` has the diagnosis),
and the fix wants far more bins -- which at the old array would have been 68 MB
at 256 bins, and is 64 KB now. **The capacity mistake was holding the display
quality hostage**, which is worth noticing: dimensioning by a ceiling does not
just cost memory, it makes the thing it dimensions unimprovable.

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

## The pattern ceilings are a model decision, not only an engine one

Two of the constants the section above multiplies are also the shape of the
product, and that came up as an architecture question on 2026-09-17: a real
instrument track -- one channel, a four-minute part, two thousand notes -- fits
nowhere in the current model, and there are two separate reasons why.

The first is storage. `MAX_PATTERN_STEPS` is 256 sixteenth cells
(`crates/mooloop-core/src/pattern.rs:13`), which at `STEPS_PER_BAR` of 16 is
sixteen bars, and `MAX_NOTES_PER_CHANNEL_PATTERN` is not an independent number
at all -- it is `MAX_PATTERN_STEPS * 4`, so 1024 (`pattern.rs:27`). Both are
reserved in full up front so that `apply_command` never grows them in the
callback, which is the whole subject of the section above. The four-minute part
is past the first and the two thousand notes are past the second.

The second is the arrangement. `PatternPlacement` is a pattern index and a
start tick and nothing else (`crates/mooloop-core/src/playlist.rs:40`);
`Sequencer::schedule_song` walks every active channel of the pattern a
placement names (`crates/mooloop-engine/src/sequencer.rs:850`), and a pattern
carries one `length_steps` shared by all of its channels (`pattern.rs:225`). A
placement is therefore every channel at once, on one 64-bar canvas
(`MAX_PLAYLIST_BARS`, `playlist.rs:8`, with a placement *start* past it refused
at `sequencer.rs:117`). There is no per-channel clip for a long part to live
in, and nothing short of inventing one would give it a place.

**Adam settled this the same day, in favour of the groovebox.** Patterns stay
and there are no per-track clips. `docs/plans/archive/audio-recording/00-status.md`
records the same ruling from the audio side that day -- audio records into the
sampler, "there are no per-track audio clips, and this plan must not add any"
-- so this was one decision made once about notes and audio together, not two
that happen to agree. It is a product decision rather than an implementation
accident, which is why it is written here rather than left for each feature to
rediscover: pattern-phase swing, `set_pattern_length` and clip automation are
all built on the shared timeline, and every one of them makes the alternative
more expensive without making it more likely.

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

A CLAP instrument is the likeliest thing to ask the question first, since a
hosted instrument is where a long recorded part would arrive. That is step 10
of `docs/plans/plugin-hosting/`, and the answer is settled before it starts
rather than during it.

## Current boundaries

- The current channel and effect bridges use complete `u8` address spaces:
  256 of each. These are transitional bridge-format boundaries, not UI policy
  caps. `archive/MIXER_PLAN.md` replaces positional channel/bus addressing with stable
  signal-slot identities and a per-project prepared render plan, removing the
  fixed mixer-bank model rather than normalizing it as permanent.
- Pattern IDs likewise use a complete `u8` address space (256 patterns).
- `MAX_NOTES_PER_CHANNEL_PATTERN` (1,024) is enforced where notes are made,
  as of 2026-09-23 (MOO-133): `ChannelState::has_room_for` is the one check,
  every session verb that adds a note asks it before changing anything, and a
  refusal is said in the status bar (`Session::take_note_refusal`). The
  save-time integrity check stays as the backstop for a document that
  arrives over the cap from elsewhere.
- Containers nest four deep (`MAX_CONTAINER_DEPTH`), and this is a limit on
  the *gesture* rather than on the format. The number bounds a real
  allocation — one dry buffer per open container in the realtime pass — rather
  than a data structure, which is the distinction the section above is about.

  **The gesture half is enforced as of 2026-09-14 and the format half is
  not.** `mooloop_core::can_wrap` and its two siblings refuse a wrap, an
  insert and a drag that would put a box past the cap, and the rack's wrap
  button asks the same function rather than comparing a depth of its own —
  before that, five clicks reached a box whose Mix did nothing at any value
  and which the rack drew no chrome for. A deeper chain loaded, and still
  loads, **unreported**: this entry used to claim the integrity pass caught it
  "the way an over-long one is", and that has never been true. It is not a
  one-line addition either, which is the part worth knowing — see
  `docs/LOOSE_ENDS.md`. A leaf is deliberately not capped: the number bounds
  open runs, so four nested boxes with a filter inside them is legal and it is
  the fifth *box* that is not.
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

- **Sends reserve nothing**, the way typed audio edges do not. A track's
  sends are a `Vec` in the document and a `Vec` in the prepared plan; the
  audio thread holds one compensation ring and one `Smoothed` per send that
  exists, and three 64 KB scratch buffers for the *whole engine* — allocated
  only when a project has a send at all. `block_cost::send_memory`, measured
  2026-09-09:

  ```text
    a project with no sends         1055.49 MB   (the floor above, unmoved)
    ...with one send                1055.68 MB
    ...with nine                    1055.68 MB

    the first send costs              193.2 KB   (three shared scratch buffers, once)
    each one after                     24.0 B
  ```

  The shape is what this policy asks for and the numbers say it plainly: the
  floor does not move at all, the *first* send buys the shared scratch, and
  the ninth is indistinguishable from the first. What a send costs beyond
  those bytes is its compensation ring, which is as long as the alignment it
  is owed and nothing at all when it is owed none.
  `render::tests::a_project_with_no_sends_allocates_nothing` guards the zero.

  The drawn side matters here too, and is the half this document does not
  usually get to state. `docs/plans/archive/console/THE-STRIP.md` first fixed the face
  at four send bars — a *drawn* limit rather than an engine one, which this
  document's own distinction would have permitted. Adam retired it on
  2026-09-09: the area draws exactly the sends that exist and scrolls past the
  room it has. A drawn ceiling is cheaper to remove than a dimensioned one and
  it is still a ceiling, and there was no reason to have one.

## Rule for new work

Before adding a numerical cap to a user-created collection, first use an
off-thread prepared, callback-safe representation. If a finite boundary is
truly required, document it next to the type and in the persisted-format
validation, make the UI communicate it honestly, and add a test at the
boundary. “It was easier to preallocate” is not a sufficient reason.
