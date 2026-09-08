# 01 — A pattern bank that fits the song

The bank is `MAX_PATTERNS x MAX_CHANNELS` `ChannelPattern`s, each reserving
`MAX_NOTES_PER_CHANNEL_PATTERN` notes and eight automation lanes, built in full
by `Sequencer::new` and never grown afterwards. A song using 24 patterns and 15
channels uses 360 of the 65,536 pairs, and pays for all of them on every edit.

`README.md` says why the reservation cannot simply be made smaller: note
editing happens in the audio callback and the reservation is what keeps it
allocation-free.

## The options, cheapest first

### A. Reserve notes in proportion to the pattern, not the maximum

`ChannelPattern::new(num_steps)` reserves `num_steps * 4` notes, capped at
`MAX_NOTES_PER_CHANNEL_PATTERN`. `Sequencer::new` passes `MAX_PATTERN_STEPS`,
so every pattern reserves the ceiling — including the ones that are sixteen
steps long, which is most of them.

Passing the pattern's actual length would make a 16-step pattern reserve 64
notes rather than 1024. That is a sixteenfold cut with **no change to the
growth contract at all**, because a pattern that is lengthened already goes
through `set_pattern_length`, which is where the reservation would grow.

The catch, and it is the whole of the design work: `Pattern::set_length_steps`
*clamps* the new length to what the reserved `capacity_ticks` already allows.
Reserve sixteen steps and the pattern can never be lengthened past sixteen —
so the reservation genuinely has to grow, and `set_pattern_length` is reached
from `apply_command` (`render.rs:3016`), on the audio thread.

That one command therefore has to move to the structural path. It is a far
smaller move than relocating note editing, and it is the only one option A
needs: `UpsertNote` keeps its guarantee, because a pattern that has not been
lengthened still has room for every note its current length can hold.

**This is the cheapest real win and the one to cost first.**

### B. Size the bank to the project and grow it structurally

The `strips` and `control_outputs` treatment: allocate what the project needs,
and let `add_pattern` and `set_active_channels` become structural commands
that prepare storage off the audio thread and hand it over.

Correct, and the biggest reduction — 360 pairs instead of 65,536. Also the most
work, because it changes how three commands reach the engine and adds a
prepared-bank object to the reclaim path.

Worth noting that A and B multiply: both together take a fifteen-channel,
24-pattern song from 1.00 GiB of reserved notes to a few hundred kilobytes.

### C. Stop rebuilding the render state for edits that change no structure

Orthogonal to both, and it attacks the symptom rather than the floor. A note
move or a knob turn is already expressible on the POD command path; the
structural path already exists for the edits that are not. If an ordinary edit
stopped calling `install_project`, the 20 ms would not be paid at all — even
with the bank exactly as it is today.

This is the largest change and the one that fixes the thing actually being
felt. It is also the one most likely to be wanted for its own sake, since
rebuilding an entire executor to move a note is doing a lot of work to
express a small intention.

## What they are actually worth, measured

Prototyped on `spike/pattern-bank-cost` (unmerged, and not correct — it models
the load path only and leaves `add_pattern` / `set_active_channels` on the
audio thread). Numbers from `prepared_project_memory` and
`project_install_cost`, release, one test at a time:

| | today | A alone | A + B |
| --- | --- | --- | --- |
| floor, 1 channel no effects | 1069.9 MB | 109.9 MB | **18.4 MB** |
| memory, 15 channels 3 effects | 1081.4 MB | 121.4 MB | **30.0 MB** |
| install, 15 channels 3 effects | 20.05 ms | 17.80 ms | **2.14 ms** |
| install, 32 channels 10 effects | 48.48 ms | 41.50 ms | **5.23 ms** |

**This reorders the options, and against what this document first said.**
A was called the cheapest real win. For memory it is — tenfold — but it takes
only 11% off the install, because the cost of building the bank is dominated
by the *number* of allocations rather than their size: 65,536
`ChannelPattern`s each allocating a notes vector and a lanes vector, whatever
those vectors then reserve. A makes each one smaller. Only B stops making
most of them at all.

So if the thing to fix is the 20 ms edit — and that is the thing anyone
actually feels — **B is not optional and A is not sufficient**. Together they
take a fifteen-channel install to 2.14 ms, which is a 60 Hz drag costing about
13% of a core instead of 120%.

A is still worth having beside B, and is cheap once B exists: the two
multiply, and A is what takes the remaining bank from megabytes to
hundreds of kilobytes.

C remains untested and remains the largest change. It is also the only one
that makes the number zero rather than small.

## How to know it worked

The measurements are committed and are the acceptance criteria:

```sh
scripts/antibox cargo test -p mooloop-engine --release \
  prepared_project_memory project_install_cost render_state_floor_by_component \
  -- --ignored --nocapture
```

Before: 1069.95 MB floor, 20.05 ms per install at fifteen channels, 1051.51 MB
of the floor in `Sequencer::new`.

`install_churn_disturbs_a_deadline_thread` should keep reporting zero late
wake-ups; it did before the change and a regression there would mean the work
introduced an audio-thread allocation, which is the one outcome worse than the
bug.

## What must not regress

- **No allocation in `apply_command`.** Every option above moves work off the
  audio thread; none may move work onto it. This is the constraint that makes
  the obvious fix not the fix.
- **`load_project` stays allocation-free from the audio thread's point of
  view.** It runs on the control thread inside `install_project` and the
  prepared state crosses as a pointer; that is fine and is the model.
- **Note editing stays exact.** A pattern that silently drops a note because
  its lane was not materialized is a worse bug than the memory. The
  `Option`-returning `channel()` accessors make that failure quiet, so any
  growth path needs a test that adds a note to a channel and pattern that did
  not exist when the project loaded.
