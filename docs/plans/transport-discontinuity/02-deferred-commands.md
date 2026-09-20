# 02 — A command that names when it lands

Read `00-status.md` first. This step gives the control plane the granularity
it does not have, and the Pattern-mode choke is the first thing it retires.

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

The scheduler already splits a block at musical edges.
`Transport::advance_looped` returns spans and `RenderState::process` runs the
sequencer once per span (`render.rs:5367` and the loop below it) — that is
where a loop fold is applied mid-block today. A deferred command applies at a
span boundary, using the machinery that exists, rather than needing a second
clock.

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

## What it retires

In Pattern mode, a switch taken at the pattern's end lands at
`wrap_tick(tick, length) == 0` whatever the incoming pattern's length is. The
playhead does not move, so it is not a seek, so `seeked` is not set, so
nothing is choked. The glitch does not get quieter; it stops existing.

One case still owes a release and should be bounded rather than ignored: a
note whose length overhangs the pattern end. Its note-off is scheduled past
the boundary in a pattern that is no longer being scheduled. That is a small,
nameable set — the engine can release exactly those voices instead of every
voice on every channel — and it is the first thing step 03's hook is good for.

## The question for Adam

**Should the pattern selector queue by default, or keep switching
immediately?**

- *Queued* is Elektron and Ableton: press it, the change takes at the end of
  the pattern, the button shows it is pending. It is the one that makes the
  glitch impossible rather than rare.
- *Immediate* is FL and is what mooloop does today. It stays possible under
  this mechanism — `MusicalEdge::Now` is a legal edge — and could be a
  modifier on the click.

This is a feel decision and it is his. Both are cheap once the mechanism
exists; the mechanism is what this step is for.

## The face

Queued needs a pending state on the pattern selector, and a pending state is a
new property crossing `main.slint`. AGENTS.md's rule applies: cross once, with
everything batched. Iterate the look with `scripts/slint-sketch` first — it
takes `ui/main.slint` itself in about 2.7 s — and build once.

## What it does not do

It does not make commands sample-accurate. A deferred command lands at a span
boundary, which is where a musical edge is, and that is the granularity
musical changes want. Giving every command an offset is a larger change with
no caller asking for it.

It does not touch parameter smoothing. A knob turn is not a musical edge and
should stay immediate-and-smoothed.
