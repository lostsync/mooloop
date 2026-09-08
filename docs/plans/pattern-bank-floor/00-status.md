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

`01-a-pattern-bank-that-fits-the-song.md` sets out the options. It deliberately
does not choose: the cheap partial fix and the correct one differ by a lot of
work and by how much they help, and that is a judgement about what Adam wants
next rather than about the code.

## Where this sits against FOCUS.md

Nowhere yet. It was not on anyone's list; it is a bug that fell out of chasing
a different one. `FOCUS.md` decides whether it is worth a step, and the honest
argument against is that the memory is reserved rather than used, the machine
showed no pressure, and nothing a user does is broken by it. The honest
argument for is that a 20 ms edit is felt every time anybody drags anything,
and it gets worse with every channel added to a song.
