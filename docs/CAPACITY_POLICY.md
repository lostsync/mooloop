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
