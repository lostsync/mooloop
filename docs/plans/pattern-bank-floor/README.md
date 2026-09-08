# The pattern bank floor

Every mooloop project, including one with no channels in it, allocates about
1.07 GB before it holds anything. `block_cost::render_state_floor_by_component`
puts 1051 MB of that in one expression, and the arithmetic is exact:

```text
MAX_PATTERNS x MAX_CHANNELS x MAX_NOTES_PER_CHANNEL_PATTERN x size_of::<NoteEvent>()
        256  x          256 x                          1024 x 16 bytes  =  1.00 GiB
```

The remaining ~27 MiB is the eight automation lanes each of those 65,536
`ChannelPattern`s also reserves.

This was not found by looking for it. It came out of a run of audio dropouts
that turned out to be `rtkit` demoting every realtime thread on Adam's
machine — see `OPERATIONS.md` — and the measurements that got written while
chasing that are what made it visible. Nothing about it is urgent: it is
*reserved* address space, the machine had no memory pressure, and the dropouts
had an unrelated and now-fixed cause.

What makes it worth a plan is the second-order cost. `EngineHandle::install_project`
builds a complete new `RenderState`, and **every** `PendingEngineMessage::ProjectEdit`
goes through it — so an ordinary edit allocates and frees this bank.
`block_cost::project_install_cost` measures that at **20 ms** for a fifteen
channel project. A pointer drag reports an edit on every move frame, so the
UI thread is asked for 1.2 seconds of work per second and saturates; measured
at 65% of a core across a 46-minute editing session against 0.8% idle. That
also explains an unsteady position readout, since the pump that advances it
cannot run on schedule.

It does **not** explain audio dropouts. That was proposed, tested and refuted:
`install_churn_disturbs_a_deadline_thread` runs a 21.3 ms deadline thread at
`SCHED_FIFO` 55 beside a thread installing at drag rate, and ninety-five
installs produced zero late wake-ups.

## Why the obvious fix is not the fix

`CAPACITY_POLICY.md` already names this shape — "a large address space is
fine, and preallocating the whole of it in advance is the thing to avoid" —
and the tree already has the remedy twice. `RenderState.strips` is "one entry
per channel the project actually has"; `control_outputs` cites
`docs/plans/archive/modulator-capacity/` for the same move. So the instinct is
to size the pattern bank from the project and grow it as channels and patterns
are added.

**That reservation is load-bearing, and its growth points are on the audio
thread.** Both halves have to be dealt with:

- `EngineCommand::UpsertNote`, `RemoveNote` and `SetStep` are handled in
  `RenderState::apply_command`, which `Graph::process` calls in the callback.
  `ChannelPattern::new` reserves `MAX_NOTES_PER_CHANNEL_PATTERN` — the most a
  pattern can hold — precisely so those pushes never reallocate. Shrinking the
  per-pattern reservation without moving note editing off the callback trades
  a memory bug for an allocation in the audio thread, which is worse.
- `Sequencer::add_pattern` and `set_active_channels` are reached the same way
  (`render.rs:3003` and `3030`). They are counter bumps today *because* the
  storage behind them already exists. Making them allocate is the same
  mistake.

`Sequencer::load_project` carries the comment that states the whole contract:
"Replace musical state without growing any realtime-owned allocation."

So this is not a constructor tweak. It is a question about where the boundary
between prepared and realtime-owned state should sit for the sequencer, and
the answer has to keep note editing allocation-free.

## Shape of the answer

The machinery already exists and is used by every other growable thing in the
engine: prepare off the audio thread, hand ownership over through
`StructuralCommand`, reclaim the displaced object through the reclaim ring.
`grow_channels` is documented as "Allocates; control thread only" and is the
model.

`01-a-pattern-bank-that-fits-the-song.md` works through the options, including
the cheap partial ones, and does not pick between them here — that is Adam's
call, and the measurements to judge it by are already committed.

## What is already in the tree

Nothing of the fix. What exists is the evidence, all of it `#[ignore]`d and
run deliberately in release:

| test | crate | says |
| --- | --- | --- |
| `prepared_project_memory` | engine | the 1.07 GB floor, and that the song barely moves it |
| `render_state_floor_by_component` | engine | that 1051 MB of it is `Sequencer::new` |
| `project_install_cost` | engine | 20 ms per edit at fifteen channels |
| `install_churn_disturbs_a_deadline_thread` | engine | that this does *not* disturb the audio thread |
| `block_cost_by_channels_and_buffer` | engine | the DSP is 13-16% of budget and not the problem |

Two attributions in `CAPACITY_POLICY.md` were wrong before these were written
— that the floor was 256 strips holding eight generators, and that it was the
bus bank's effect chains — and both are corrected there. Reasoning about which
large constant looked guilty produced two confident wrong answers; bracketing
the constructors produced the right one in a single run. Anyone picking this
up should measure rather than reason about it.
