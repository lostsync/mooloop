# Pattern bank floor status

Nothing has landed. This directory is a work order and a set of measurements,
written 2026-09-08 out of an audio-dropout investigation that turned out to be
about something else entirely.

## What the investigation established, so it is not re-derived

- **The dropouts had an unrelated cause and it is fixed.** `rtkit` had demoted
  every realtime thread on the machine after its canary starved, leaving
  PipeWire's data loop at `SCHED_OTHER`. `OPERATIONS.md` records how to spot
  it. PipeWire now runs `SCHED_FIFO` 60 by rlimit and mooloop's callback 55.
- **The engine's DSP is not a problem.** PipeWire's own accounting of the
  mooloop node reads `B/Q` 0.13-0.16 — 13-16% of a 21.3 ms quantum — on the
  heaviest document that exists, agreeing with `block_cost`.
- **The 1.07 GB floor is real and is the pattern bank.** Exact arithmetic and
  the blocked-fix argument are in `README.md`.
- **The floor's per-edit cost is the live problem**, at 20 ms an edit and a UI
  thread that saturates during a drag.
- **It does not reach the audio thread.** Tested and refuted, not assumed.

## Step 01 — not started

`01-a-pattern-bank-that-fits-the-song.md` sets out the options and now prices
them, from a prototype on the unmerged `spike/pattern-bank-cost` branch. The
pricing changed the recommendation: option A alone is a tenfold memory win and
an 11% install win, because what makes an install expensive is the *number* of
`ChannelPattern` allocations rather than their size. A and B together take a
fifteen-channel install from 20.05 ms to **2.14 ms** and the floor from 1069.9
MB to **18.4 MB**.

Still not choosing — B is the larger piece of work and the decision is about
what is worth doing next, not about which is better.

## Where this sits against FOCUS.md

**Parked, by Adam on 2026-09-12**, under `FOCUS.md`'s "deliberately not now".
It was not on anyone's list; it is a bug that fell out of chasing a different
one. What makes it safe to park is that the measurements are committed either
way, so nothing has to be re-derived to take it later. The honest argument
against is that the memory is reserved rather than used, the machine
showed no pressure, and nothing a user does is broken by it. The honest
argument for is that a 20 ms edit is felt every time anybody drags anything,
and it gets worse with every channel added to a song.

## What else now waits on it, 2026-09-17

An architecture pass asked whether mooloop should grow per-track clips, on the
strength of one case: an instrument track -- one channel, a four-minute part,
two thousand notes -- fits neither `MAX_PATTERN_STEPS` nor
`MAX_NOTES_PER_CHANNEL_PATTERN`, and a `PatternPlacement` places every channel's
whole pattern at once (`crates/mooloop-core/src/playlist.rs:40`), so there is no
per-channel clip for it to live in either. **Adam settled it the same day in
favour of the groovebox: patterns stay, and there are no per-track clips.**
`docs/CAPACITY_POLICY.md` carries the decision and the constants it rests on.

That turns this plan from a memory bug into a prerequisite. With clips ruled
out, the only remaining answer to a part that is genuinely too long is to raise
`MAX_PATTERN_STEPS` -- and `MAX_NOTES_PER_CHANNEL_PATTERN` is
`MAX_PATTERN_STEPS * 4` rather than a number of its own
(`crates/mooloop-core/src/pattern.rs:13` and `:27`), so raising one raises both
and the 1.00 GiB `README.md` measures scales with it. Sixty-four-bar patterns
would reserve 4 GiB; a four-minute pattern at 120 BPM about 7.5 GiB. Sizing the
bank from the project is what makes the ceiling liftable at all, so this lands
before anyone raises it.

This does not unpark the plan. Nothing has yet asked for a pattern longer than
sixteen bars, and the parking argument above is unchanged. It is here so that
the day something does ask -- a hosted CLAP instrument is the likeliest -- the
order is already decided rather than argued then.
