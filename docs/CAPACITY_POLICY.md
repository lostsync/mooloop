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

## The same lesson, unlearned: the strip bank

The paragraph above was written about effect slots and is currently true of
channel strips, at a much larger scale. `mooloop-engine`'s
`block_cost::prepared_project_memory` measures it: **a one-channel project
with no effects allocates about 1.07 GB**, and a fifteen-channel one with a
full chain on every channel allocates 1.10 GB. The song is nearly irrelevant;
the floor is the number.

It multiplies out the same way the effect slots did, from two decisions that
each look reasonable alone:

- `RenderState` preallocates `MAX_CHANNELS` — 256 — channel strips, whether or
  not the project has that many channels.
- `ChannelStrip` holds *every* generator at once — sampler, drum synth, mono,
  poly, ML-M1, ML-P8, DS-01, aux in — with `active_source` naming which one
  runs. Switching a channel's generator therefore allocates nothing on the
  audio thread, which is the point.

Neither is visible at its own definition. Together they are 256 strips × 8
generators, of which a fifteen-channel song uses fifteen.

Both halves have the same remedy the modulator-capacity plan used, and the
machinery already exists: strips and generators are prepared off the audio
thread and installed through the structural-command and reclaim path that
effects, buffers and whole projects already travel. Making a strip arrive when
a channel does, and a generator when a channel selects it, keeps every ceiling
where it is and stops dimensioning by them.

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
  usually get to state. `docs/plans/console/THE-STRIP.md` first fixed the face
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
