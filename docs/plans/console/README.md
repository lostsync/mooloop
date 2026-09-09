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

**That policy is retired.** See "What the first draft of this file said" at the
bottom, and `docs/TERMINOLOGY.md`: grouping is routing several tracks to one
track, and it takes nothing away.

That is the missing half of `MIXER_PLAN.md`, and this plan is what follows
from it. On top of it sits the part that is actually about sound: a per-strip
channel strip device, preamp modelling later, and Airwindows-style **console
summing**. Everything here is **out by default and free while it is out**.

The intended outcome is a mixer that reads as a small console you arrived at
by making music, not a routing spreadsheet you configured.

## Two decisions this plan makes, before the steps

**Rewritten 2026-09-09**, after Adam described the model plainly and the first
pair turned out to be a spreadsheet's idea of a console rather than a console.
`docs/TERMINOLOGY.md` is the vocabulary; read it first. The originals are kept
at the bottom of this file, because knowing which way the mistake ran is worth
more than a clean page.

### 1. The mixer is tracks. All of them. Always.

Every channel has a track, permanently. A track has a fader, pan, mute, its
own device rack, a strip, sends and one output, and **it keeps all of that
when it is grouped** — grouping changes where a track's output goes, it does
not take the track away.

There are also tracks that no channel feeds. They are made the same way, drawn
the same way, and run the same code.

**Bus and send are roles, not types.** A track fed by other tracks' outputs is
being used as a bus; one fed by their sends is a send; one fed by a channel is
an ordinary track; and a track can be all three at once. Adam: *"if you set it
up as a send, it is a send. if it is a bus, it is a bus. i dont really want to
have to make an fx/aux channel specifically. it just isn't needed."*

So `MIXER_PLAN.md`'s `+ Track` / `+ Bus` / `+ Send` is retired outright, and
so is the *signal slot* name — the unification was right and the word for it
is **track**.

### 2. Two racks, on purpose, and that is why channel and track stay separate

A channel's rack is part of the instrument: *"plugged in and 'captured to
tape' — part of the instrument signal."* A track's rack is glue and post.
The distinction is a convention rather than an enforcement — *"you could still
throw buffer on the drum buss or whatever"* — and nothing refuses a device in
either place.

This is Maschine's arrangement, and it is the reason mooloop keeps two words
where most DAWs have one. It also means the existing shape is already right:
`Channel { source, effects, output, bus }` feeding `MixerBus { effects, output
}` **is** the two-rack model. What is missing is that a track is not presented
as one, cannot be freely made, named or routed, and there is not one per
channel.

### 3. Keep `EffectTarget`; make the track list dynamic. Do not rename the code mid-feature.

The incremental path still holds, and now for a plainer reason: the target
shape is one species of thing, and `EffectTarget { Channel, Bus }` is two. That
rename is mechanical, it touches the meter address space, both compilers and
both block loops, and it should be its own change rather than a rider on a
feature. `docs/TERMINOLOGY.md` carries the mapping until then.

## The sequence, and why it is not the brief's order

Adam's list order was reorder -> group -> console mixer -> sends. This plan
reorders it once, deliberately, and the reason is `FOCUS.md`'s own rule:
*prefer changes that produce a musical decision over changes that merely add
capacity.* Steps 01, 02 and 03 all end in something audible on today's tree.
Steps 04-06 are the structural block, and none of them makes a new sound.

**[`THE-STRIP.md`](THE-STRIP.md) is Adam's mockup of the finished strip**,
drawn 2026-09-09 while step 02 was being built, plus the rulings he gave on it
the same day. Read it before steps 03, 05 or 06: it reshapes all three, and it
settles the one thing this plan had left implicit -- the channel strip is not
a device you insert, it is what every strip has. It also names the four
voicings (**Moo / Grip / Punch / Iron**, for the sound rather than for any
hardware) and records how their distortion becomes a specified, tested number
rather than an adjective.

| Step | What it ends in | Needs |
| --- | --- | --- |
| [01](01-a-channel-can-be-moved.md) | a channel can be dragged to another row | nothing |
| [02](02-console-summing.md) | two channels glue when summed | nothing |
| [03](03-the-channel-strip-device.md) | EQ + comp in one face, four voicings | nothing |
| [04](04-the-mixer-is-tracks.md) | every channel has a track, and the mixer draws them | 01 |
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

## Settled, 2026-09-09

The four questions this plan opened, and Adam's answers.

1. **Order.** 01 -> 02 -> 03 -> 04 -> 05, as recommended. Character before
   structure.
2. **The console ceiling.** Accepted, and the reason is the product's rather
   than the engine's: *"you can do a perfectly clean mix on it if you want but
   you could also drive it and get something nice in return."* The bound is
   the effect. `GAIN_STRUCTURE.md` says so and step 02 meters it.
3. **Console scope.** Confirmed, with a correction to how it was written down.
   The Airwindows gesture is a `Channel` plugin last on each track and a
   `Buss` plugin on the bus they all reach, and *"a buss like that would
   decode on input"* -- so decode-at-every-bus-input is the right mechanism.
   What Adam wants that the plugin pair does not give is that **the bus is
   invisible**: there is no device to place and no bus to create. Master is
   already a summing point, so switching console on for two channels makes
   them glue with nothing configured.

   The consequence, which is the useful half: **the channel faders are the
   drive and the bus fader is the volume.** A bus's fader is already after its
   input sum in the block order, so turning a bus down is level without
   changing character, and turning its feeders down is less drive. *"If I want
   those sources to be quieter I need to turn down the mixer busses that have
   this analog/nonlinear summing mode enabled."*
4. **`FOCUS.md`.** Left alone. Adam: *"that document exists because i ignore
   it. i dont think there is anything in it that can't wait."* So this plan
   does not appear there, and `docs/plans/README.md` is where its state is
   recorded.

## What the first draft of this file said, and why it was wrong

Kept rather than deleted, because the shape of the mistake is the useful part.

> **1. The mixer is a *view of strips*, not a second set of objects.** […] do
> not auto-create a bus per channel. A lone channel's mixer strip *is* that
> channel. Grouping creates a bus, and the group's members drop out of the
> mixer because their out is now the group.
>
> `[MASTER] | one strip per group | one strip per ungrouped channel | manual buses`

Two things are wrong with it. **A grouped channel does not lose its strip** —
on a desk you still want its fader, its EQ and its sends after you have routed
it to the drum bus, and a mixer that removes it is deriving a view rather than
modelling a console. And **the strip list is not derived at all**; it is just
the tracks, in order, which is what makes it a place you can learn.

The policy it was built on — *a mixer strip is created by a musical act, not
an administrative one* — was an answer to a problem that does not arise once
every channel simply has a track: there is no auto-assignment to undo, because
there is no assignment step.

The symptom, noticed before the cause: step 02's analog-sum switch had to go
on a channel's **rack row**, because the mixer draws no channel tracks for it
to live on. That is the instrument's room, not the mixer's. It moves when
step 04 lands.
