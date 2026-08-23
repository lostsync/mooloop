# Capacity Policy

Mooloop must not impose small product caps on dynamic musical items. A user
should not encounter a limit such as “16 channels” or “8 effects” because it
made an implementation convenient.

The audio callback cannot allocate, block, or perform unbounded work. That is
an engine constraint, not permission to turn every collection into a tiny
fixed bank. Dynamic state is prepared off the audio thread; callback-facing
storage is then preallocated for the complete supported address space.

## Current boundaries

- The current channel and effect bridges use complete `u8` address spaces:
  256 of each. These are transitional bridge-format boundaries, not UI policy
  caps. `MIXER_PLAN.md` replaces positional channel/bus addressing with stable
  signal-slot identities and a per-project prepared render plan, removing the
  fixed mixer-bank model rather than normalizing it as permanent.
- Pattern IDs likewise use a complete `u8` address space (256 patterns).
- Event lists, block size, voice pools, sample memory, playlist span, and
  routing have explicit realtime, DSP, or file-format reasons. Any change to
  one must name the reason and show overflow behavior.
- The 16-insert mixer-bus bank is a legacy fixed-graph implementation detail,
  not a product decision. It is the next capacity-sweep candidate; do not use
  it as a precedent for new dynamic collections.

## Rule for new work

Before adding a numerical cap to a user-created collection, first use an
off-thread prepared, callback-safe representation. If a finite boundary is
truly required, document it next to the type and in the persisted-format
validation, make the UI communicate it honestly, and add a test at the
boundary. “It was easier to preallocate” is not a sufficient reason.
