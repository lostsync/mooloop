# 02 — A command that names when it lands

Read `00-status.md` first. This step gives the control plane the granularity
it does not have. It was written expecting to retire the Pattern-mode choke
with it; it does not, for reasons under "What it retires, and what it does
not" -- that falls to step 03.

## The gap

The control plane has two shapes and no third:

- **Events** are sample-accurate. A `TimedEvent` carries an `offset` into the
  block and `push_ordered` defines what happens at equal offsets.
- **Commands** are block-accurate and offset-free. The executor drains the
  ring at the top of the callback and applies each one immediately
  (`crates/mooloop-engine/src/executor.rs:181`). A command carries no offset,
  no target tick, and no way to say "not yet".

So anything that cannot be expressed as a note event lands as a step at an
arbitrary block edge — up to a full quantum of jitter, always at offset 0, and
never aligned to anything musical. Every fix the project has made for this has
been site-local continuity: `Smoothed` on the audible continuous effect
parameters, an ADSR attack that starts from the current level, the equal-power
crossfades in `DelayLine` and the container, choke-instead-of-mute,
`restore_lanes_left_behind`. Each closed one door. Discontinuity is still the
default and continuity is still opt-in per site, which is why a new feature
opens a new one.

## The change

A third shape: a command that names a musical edge and is held until the block
reaches it.

The scheduler already splits a block, and `RenderState::process` runs the
sequencer once per span — that is where a loop fold is applied mid-block
today. A deferred command applies at a span boundary, rather than needing a
second clock.

**Corrected while building it:** the block was *not* already split at musical
edges. `advance_looped` cut at one thing only, the loop end, and the per-span
scheduling pass ran only when a loop was installed — everything else took
`schedule_once` over the whole block. So the machinery to reuse was the shape
of the span loop, not a cut that already existed. `advance_looped` gained an
optional second cut, and the per-span pass now runs whenever there is a cut of
either kind.

The two cuts are not the same thing and the code says so. A fold is a
discontinuity and the span it opens carries `jumped`, which the renderer turns
into a release of every sounding voice. An edge is a boundary the music was
walking towards anyway: `jumped` stays false, and nothing is released. Where
both fall on the same frame the fold wins, because an edge resolved against a
position the transport is in the act of leaving means nothing.

Shape to aim for:

- `RealtimeCommand::Deferred { when: MusicalEdge, command: EngineCommand }`,
  where `MusicalEdge` starts as `PatternEnd` and has room for `Bar` and
  `Beat`.
- **One pending slot per deferred command kind, not a queue.** A second
  pattern switch before the boundary *replaces* the first. That is what a user
  means by re-queueing, and it makes the state bounded without a policy
  argument about overflow.
- The pending slot is renderer state, so it must survive an install the way
  `incremental-structure/` taught: carried, not re-sent.
- Cancellation has to exist. Stopping the transport, switching playback mode,
  or seeking must clear a pending edge rather than leave it to fire against a
  position that no longer means anything.

## What it retires, and what it does not

**Nothing, on its own — and the draft that claimed otherwise was wrong twice.**

It read: *a switch taken at the pattern's end lands at
`wrap_tick(tick, length) == 0` whatever the incoming pattern's length is, so
the playhead does not move, so nothing is choked.* Both halves fail.

First the arithmetic. Pattern position is `wrap_tick(song_tick, length)`, so a
pattern end of the *outgoing* pattern is not a boundary of the incoming one
unless the lengths agree: at tick 768, leaving a 384-tick pattern for a
512-tick one, `wrap_tick(768, 512)` is 256, not 0. The switch lands a third of
the way into the new pattern.

Then the deeper one, which is worth more than the arithmetic. It was
implemented — `seeked` set only when the pattern position actually moves —
and `switching_pattern_while_playing_releases_the_sounding_voices` failed it
immediately, reporting a voice still sounding at 0.2519 where it expected
silence. The test is right and the idea was wrong: **the release is owed
because the note source changed, not because the playhead moved.** The
note-off that would have ended a sounding voice lives in the pattern that
stopped being scheduled. Two patterns of the same length put the playhead in
exactly the same place and still strand it. The change was reverted; step 01's
rule stands unaltered.

So the choke that survives step 01 survives step 02 as well, which is what
`00-status.md` already says the "immediate" ruling cost this step. What
retires it is a mechanism that can release *the stranded voices only* — the
notes overhanging the boundary, a small and nameable set — instead of every
voice on every channel. That is step 03's hook, and this step does not
substitute for it.

## The ruling, 2026-09-20

**The pattern selector stays immediate.** Adam was asked whether it should
queue to the pattern end or keep switching now, and took immediate -- the FL
convention mooloop already has.

That changes what this step is for, and the change is worth being honest
about. It was written expecting queueing to be the thing that retired the
Pattern-mode choke; it does not, because an immediate switch in Pattern mode
under a running transport genuinely does strand a note-off, and step 01 left
that release exactly where it was. So this step is now about the missing
granularity on its own terms -- a tempo change that lands on the bar, a
pattern-length change that does not take effect mid-pass, a preset swap on the
downbeat -- and `MusicalEdge::PatternEnd` gains a caller when something asks
for one.

Queueing remains cheap to add on top if Adam ever wants it. Nothing below
assumes either answer.

## The face

**None, as built.** This section specified a pending state on the pattern
selector, which only queueing needs — and the ruling above made the selector
immediate, so nothing queues and nothing has a pending look. No property
crosses `main.slint` and no UI file is touched.

It comes back with the first caller that wants a gesture to visibly wait. The
advice stands for that day: cross once with everything batched, and iterate
with `scripts/slint-sketch` before building.

## What it does not do

It does not make commands sample-accurate. A deferred command lands at a span
boundary, which is where a musical edge is, and that is the granularity
musical changes want. Giving every command an offset is a larger change with
no caller asking for it.

It does not touch parameter smoothing. A knob turn is not a musical edge and
should stay immediate-and-smoothed.

## What landed, 2026-09-20

`MusicalEdge` (`Beat`, `Bar`, `PatternEnd`) in `mooloop-core`, beside
`EngineCommand`; `RealtimeCommand::Deferred` and `CommandSink::send_deferred`
to carry one; `Transport::advance_looped` gained the second cut; and the
renderer holds the pending commands, resolves an edge to an absolute tick when
it arrives, and applies each between spans.

Decisions worth not re-deriving:

- **Resolved once, on arrival, not every block.** A bar line is a position in
  the score, so the target survives a tempo change. Re-resolving each block
  would also make the target chase the playhead and never be reached.
- **One slot per command *kind*, in a fixed array of eight.** A second
  deferred tempo replaces the first; a deferred swing does not displace it.
  Bounded with no overflow policy, and a full array is a defect rather than a
  load — it is a `debug_assert!`, and the command is dropped rather than
  applied early, which would be a discontinuity at the worst moment.
- **A stopped transport applies immediately.** It reaches no edge, so a
  command parked against one would wait forever.
- **Stop, pause, seek and a mode change cancel.** Each invalidates the
  position the edge was resolved against; the target would either never
  arrive or arrive somewhere the player never aimed at. `PatternEnd` in Song
  mode is the one case that resolves rather than refuses — it falls back to
  the next bar, since Song mode crosses no pattern end.
- **Carried across an install**, in `adopt_performance_state`, with the
  position and the held keys. The control thread cannot know an install
  happened between its send and the edge.

**No production caller yet.** The pattern selector does not use it, per the
ruling; tempo-on-the-bar and a preset swap on the downbeat are the callers
named for it, and neither is built. The API is `pub`, so this is not dead
code, but it is unproven against a real gesture until one arrives.
