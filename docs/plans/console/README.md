# The console pass

Status: written 2026-09-09, from Adam's morning list. Steps 01-06 below; work
them in order and record what landed in `00-status.md`.

Four items on that list -- reorder channels, group channels, make the mixer
work like a console, proper sends and returns -- are **one design with a
character layer on top**, not four features. This directory is that design.

`docs/MIXER_PLAN.md` (August, never started) already argued the fixed
seventeen-bus bank should go, and proposed one *signal slot* primitive where
track / bus / group / return are roles rather than species. What it never had
was a **policy** for when a strip comes into existence. It offered `+ Track`,
`+ Bus`, `+ Send` -- three manual buttons, which is precisely the FL problem:
for sampler-based music every new sound is a new channel, so you either undo
the auto-assignment constantly or you assign by hand.

Adam's policy closes it:

> **A mixer strip is created by a musical act, not an administrative one.**
> Channels group; the group is summed; the group owns the strip; a grouped
> channel has no direct out.

That is the missing half of `MIXER_PLAN.md`, and this plan is what follows
from it. On top of it sits the part that is actually about sound: a per-strip
channel strip device, preamp modelling later, and Airwindows-style **console
summing**. Everything here is **out by default and free while it is out**.

The intended outcome is a mixer that reads as a small console you arrived at
by making music, not a routing spreadsheet you configured.

## Two decisions this plan makes, before the steps

### 1. The mixer is a *view of strips*, not a second set of objects

A channel already has everything a strip has: fader, pan, mute, an insert
chain, a compensation delay, meters, and one output edge. So does a bus. The
existing `EffectTarget { Channel(u8), Bus(u8) }` (`mixer.rs`) is already "a
strip is a strip", and it is the address used by every effect command, by
`ParamAddr::scope`, and by the meter cell layout (`meters.rs`, where a bus is
`MAX_CHANNELS + index`).

So: **do not auto-create a bus per channel.** A lone channel's mixer strip *is*
that channel. Grouping creates a bus, and the group's members drop out of the
mixer because their out is now the group. The mixer's strip list is therefore

```text
[MASTER] | one strip per group | one strip per ungrouped channel | manual buses
```

This is what avoids the double fader FL gets wrong, and it means the three
strip-level features below -- **sends, the console switch, the channel-strip
device** -- land on channels *and* buses at once. "A way to do sends and
returns without a mixer bus", which the brief asked for, then costs nothing
extra.

### 2. Keep `EffectTarget`; make the bus list dynamic. Do not do the full slot rewrite yet.

`MIXER_PLAN.md`'s `SignalSlotId` unification replaces `EffectTarget`
everywhere at once, including the meter address space, both compilers, and the
two block loops. It is the right eventual shape and the wrong first move. The
incremental path -- buses become a `Vec` with stable ids and group membership,
channels keep their own strip -- delivers every item on the list while
preserving `compile_bus_graph`'s shape, the meter layout, and the
realtime/offline equality harness that would otherwise all move at once.

Sends are the one item that *does* force a compiler change (step 05). That is
priced there rather than smuggled in early.

## The sequence, and why it is not the brief's order

Adam's list order was reorder -> group -> console mixer -> sends. This plan
reorders it once, deliberately, and the reason is `FOCUS.md`'s own rule:
*prefer changes that produce a musical decision over changes that merely add
capacity.* Steps 01, 02 and 03 all end in something audible on today's tree.
Steps 04-06 are the structural block, and none of them makes a new sound.

| Step | What it ends in | Needs |
| --- | --- | --- |
| [01](01-a-channel-can-be-moved.md) | a channel can be dragged to another row | nothing |
| [02](02-console-summing.md) | two channels glue when summed | nothing |
| [03](03-the-channel-strip-device.md) | EQ + comp in one face, four voicings | nothing |
| [04](04-groups-and-a-dynamic-bus-list.md) | channels group; the group owns the strip | 01 |
| [05](05-sends-and-returns.md) | a reverb return fed from two strips | 04 |
| [06](06-preamp-modelling.md) | not designed here | 02, 03 |

Nothing in 02 or 03 gets rebuilt if Adam prefers the brief's order and 04
moves up.

## What the exploration found that changes the shape of the work

Recorded here because each one is a fact about today's tree that a step below
depends on, and rediscovering them is the expensive part.

- **`OutletTap::Output` is declared and nothing publishes one**
  (`outlet.rs`). `compile_audio_graph` refuses it as
  `EdgeRefusal::TapIsLate`. That is the hook a post-fader send needs, already
  named.
- **`compile_latency` collapses per-edge into per-producer on purpose**, and
  says so in its own doc comment: *"Each producer has exactly one destination,
  so 'per edge' and 'per producer' are the same thing."* Sends delete that
  sentence. `CompiledLatency` becomes per-edge and
  `ChannelStrip::compensation` / `BusStrip::compensation` become a delay per
  outgoing edge.
- **There is no gain smoothing at strip level at all.** `OutputStage` stamps
  raw gain per block, or per 32-frame control tick when modulated.
  `MIXER_PLAN.md` requires send levels to be smoothed -- that is a gap to
  fill, not an addition. `mooloop-dsp/src/smooth.rs::Smoothed` exists.
- **`AudioTapBank` (`render.rs`) is already the buffer-ownership prototype**
  the general plan needs: conditional, deduplicated, plan-driven allocation
  with a reclaim path. Extend it; do not invent a second mechanism.
- **`ChannelEdit` has only `Removed` / `Inserted`.** A reorder is not an
  expressible edit. Three durable things address a channel by index and
  already funnel through `Project::rescope_after`: `ModRoute` destinations,
  `AutomationLane.target`, and `AuxInParams.source_channel`.
- **Things that address a channel by index and are *not* rescoped**:
  `Session.selected_device`, `Session.selected_source`,
  `Session.automation_target`, `Session.effect_preset_names`,
  `Session.source_preset_names`, `Session.pending_preset_save`, and the
  parallel `samples: Vec<...>` sidecar in `ProjectSnapshot`/`ProjectEdit`. A
  move breaks all of them today.
- **`Session::reset_channel_source` rewrites the name from the index**, so
  changing a channel's device destroys any name it had. Found on the way past;
  it belongs to channel identity, and is recorded in
  `interface-iteration/03-channel-identity.md` rather than here.
- **Patterns can already be renamed** (`main.slint`, `pattern-renamed`). What
  is missing is pattern *colour*.

## Recorded, not built here

Where one of these already has a home, that home is named rather than
duplicated.

- **Channel names and colours** -- already fully planned in
  `interface-iteration/03-channel-identity.md`.
- **Pattern rename** -- already works. No action.
- **Pattern colour** -- folded into `interface-iteration/03-channel-identity.md`,
  since it is the same defaulted-field shape.
- **Playlist as DAW lanes, zoom-to-patterns, song-level automation** -- the
  largest unrecorded item on the list, and a design question rather than a
  gap. It is in `IDEAS.md` beside the tracker entry, which is already holding
  the open fork about whether the modulator tracker and the automation tracker
  are one design or two. Song-level automation is the third thing that wants
  the same answer, and they are recorded together so they are not decided
  separately.

## Open, and Adam's call

1. **Order.** The recommendation is 01 -> 02 -> 03 -> 04 -> 05, character
   before structure. The brief's order is defensible and just means 04 moves
   up.
2. **The console ceiling.** `GAIN_STRUCTURE.md` says *"Nothing bounds a sample
   in the live path"*, and console decode is `asin`, whose domain is +/-1: a
   decode point therefore **hard-limits at 0 dBFS**. Recommendation: accept
   it, say so in `GAIN_STRUCTURE.md`, and meter it, since that ceiling *is*
   the effect. Step 02 states the case in full.
3. **Console scope.** One algorithm for the whole mixer, a switch per strip,
   decode at each bus input rather than one global decoder.
4. **`FOCUS.md`.** This is a new sequence and it displaces or joins step 3
   (the interface iteration), which still has two open steps. Rewriting
   `FOCUS.md` is Adam's call and is deliberately not done inside the first
   branch.
