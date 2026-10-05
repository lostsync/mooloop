# Song modulation

Modulators stop belonging to a channel and belong to the song. Any modulator
can drive any knob anywhere: a channel's source, inserts or strip, a mixer
track's inserts and fader, or the master. They get a pane of their own.
Planned 2026-10-05 for 0.1.7. Linear: project **Song modulation**, one issue
per step (`00-status.md` has the list).

This is the first push of a larger idea, and on purpose the smallest one:
**move what we have and make it work document-wide.** The same five module
kinds (LFO, Envelope, Step, Random, Math), the same Assign gesture, the same
module surfaces. Nothing new to learn except that the reach is wider.

## Why, in Adam's words

**2026-10-05**, planning 0.1.7, on what the modulator push is:

> 0.1.6 was originally going to be the modulator push, where we re-scope the
> modulation from per-channel to document-wide and move it into its own
> pane.

and his answers to the plan's questions the same day:

> what envelope? a modulator envelope like we already have? you pick the
> input from a list of outlets sending compatible events
>
> channels wont have modulators but yeah i think assignments would
> copy/paste
>
> [how many modulators per song?] all of them
>
> i sent an html/js mockup in another thread. it will just go in its own
> pane

and how to sequence it:

> usually for something of this size we'd scope it into pushes, make plans
> for each push, file issues for the plans, and then work from the issues.
> let's start by moving what we have and making it work document-wide.
> we'll see how that went

## What it reverses, and what it keeps

**Reversed: "a channel owns its modulation sources and routes"**
(`MODULATION.md`, *Decisions* and *Channel collection*, which also say
*"don't re-litigate"*). This plan is the re-litigation, on Adam's word.
Step 05 rewrites those sections.

**Reversed: "the modulation rack's move ... comes back only when he raises
it"** (`FOCUS.md`, `SCOPE.md` §4). He raised it.

**Kept:**
- **Base plus offset.** A route adds an offset around the knob's value (or
  a lane's); it never replaces it. Unchanged.
- **The control rate.** Modulators tick every 32 frames. Unchanged.
- **Durable identity.** A module is named by a `ModSourceId`, never by its
  place in a list, so reordering moves nothing a route means.
- **Device-local modulation.** ML-P8's internal LFO routes and every
  per-voice envelope stay inside their device. Only the channel rack moves.
- **Every saved song sounds the same.** A 0.1.6 song opens with its racks
  converted, its routes intact, its LFOs at the same rate and phase.

## The later direction this is the first stage of

Adam's *Song Patch* prototype (designed 2026-09-23 to 09-27, not in the
repo): one song-wide patching canvas in its own pane, with typed boxes
(`lfo`, `step`, `chance`, `chord`, `* -0.5` ...), control wires and note
wires, and song inlets and outlets as tags. Prototype:
https://claude.ai/artifact/BBYu543x1WZ8VAf2MrGCUY. Project memory
`song-patch-design` has the points agreed so far.

**This plan does not build the canvas.** It builds what the canvas will stand
on: one song-wide set of modules, routes that reach anything, sources picked
from a list of outlets, and a pane that is not the device rack. The pane's
contents in step 04 are today's shelf, so the canvas can later replace them
without moving anything underneath. Each step says where it is choosing the
shape the canvas will want.

## What exists (survey, 2026-10-05, `main` at `d3acbd6`)

**The rack is a channel's.**
- `ModRack` (`core/src/modulation.rs:1860`) is `Copy`: 8 slots, 16 routes
  (`MAX_MODULATORS_PER_CHANNEL`, `MAX_MOD_ROUTES_PER_CHANNEL`, `:1716`,
  `:1719`) and a per-rack `next_source_id`. It lives in
  `ChannelSetup.modulation` (`core/src/project.rs:326`).
- A route is `ModRoute { source, source_slot, destination: ParamAddr, depth,
  polarity }` (`:1626`). **The destination already names its channel or bus**
  (`ParamAddr.scope`, `:262`). Its doc says that was done *"so enabling
  cross-channel modulation later is a routing change"*.
- A source is `ModSourceRef` (`core/src/mod_metadata.rs:317`): a module id, a
  generator outlet, or the mod wheel and aftertouch. **The last two mean "my
  channel's" implicitly**; they carry no channel.

**Everything around the route forces "own channel".**
- The engine resolves a channel's routes only on that channel
  (`render.rs` `ModulationBlock`, built at `:10602`). **A bus chain is handed
  no modulation at all** (`:11188-11200`): that is MOO-497.
- The session refuses any other scope (`channel_modulation_destination`,
  `session.rs:1262`: *"Buses and another channel's controls stay deliberately
  outside this pass"*).
- Integrity points a route aimed at another channel back home
  (`integrity.rs:1676`).
- Paste and presets rescope every route onto the receiving channel
  (`ChannelSetup::rescope_modulation`, `project.rs:540`).
- LFO retrigger, Step advance and Random trigger read the owning channel's
  notes (`dsp/modulator.rs:810-838`). The Envelope picks a channel
  (`input_channel`, `modulation.rs:1195`).
- Random seeds from its slot index (`dsp/modulator.rs:725`).

**Engine shape.**
- All modulators tick at the top of each control tick, in channel order,
  before anything renders (`render.rs:10420-10477`). A song-wide pass keeps
  that order trivially: sources always come before destinations.
- Generator outlets publish into a per-channel table read one block late
  (`ChannelStrip.published_outlets`, `:5001`).
- Phases survive a channel edit only when the channel's strip is carried
  (`carry_strips_from`, `:7144`). `same_strip_but_effects` compares the rack
  (`engine/src/lib.rs:1415`), so a rack that changed by install rebuilds the
  strip and cuts its voices: that is MOO-487.
- Commands are channel-keyed and one fact each (`core/src/bridge.rs:415-457`).
- `block_cost.rs` measures one channel's control pass (`:1516-1652`) and
  nothing wider (MOO-170).

**Presets carry racks.** Channel presets and kits hold a whole
`ChannelSetup`, and ML-M1's factory patches set `setup.modulation`
(`project/src/factory.rs:295`).

**The shelf** is `ui/modulation-shelf.slint` (1735 lines), shown only while
a channel is in the rack (`main.slint:6801`, `if !root.editing-bus`). The
route-count dots on a face count the selected channel's rack only
(`ui/src/lib.rs:3670`).

## The shape

**The song owns one modulation set**, `Project.modulation`:
- modules, each with a song-unique `ModSourceId` and a name;
- routes, each from one source to one `ParamAddr` anywhere in the song;
- no count cap a user meets. Storage is sized from the song and grows by
  replacement off the audio thread (`CAPACITY_POLICY.md`), the way song
  automation sizes its lanes.

**A source is picked from a list of outlets.** A module's input (the
Envelope's gate, the LFO's retrigger, the Step's advance, the Random's
trigger) names an outlet that sends compatible events: a channel's notes, a
generator's control outlet, another module. Every source that used to mean
"my channel" now names its channel. This is the canvas's *inlet* in its first
form.

**Channels own no modulators.** Copying or pasting a channel copies the
routes that point into it, aimed at the copy, from the same modules. A
channel preset carries the modules its routes use and lands them in the song.

**Destinations stay `ParamAddr`**, addressed by seat and rescoped on every
channel and track edit, as pattern and song lanes are. Moving every address
to durable ids is one change for routes and both kinds of lane together, and
is not this plan.

**The pane** is a sixth `PaneViews` entry that holds today's shelf contents,
widened: the module grid, the selected module's surface, and its routes,
each route naming the channel or track it reaches.

## Steps

| # | Step | Team | Milestone |
| --- | --- | --- | --- |
| 01 | The song's modulation set | Document & Session | One set for the song |
| 02 | The engine runs one set | Parameters & Control | One set for the song |
| 03 | Assigning anywhere | Parameters & Control | Reach anything |
| 04 | The modulation pane | Interface | Reach anything |
| 05 | The contract, rewritten | Parameters & Control | Reach anything |

Something audible comes early: after 02 a converted 0.1.6 song plays exactly
as before, and a route to a mixer track's insert (MOO-497) moves it. After 04
you can make one.

## Not in this plan

- **The canvas, wires, and box vocabulary.** The direction, not this push.
- **Note wires and boxes that make notes** (`chord`, `chance`, `scale`).
- **New module kinds** (an audio follower, a two-input Math, note
  generators: `SHORT_NOTES.md`).
- **Modulators inside containers**, so a chain preset carries modulation
  (MOO-160).
- **Modulating a module's own parameters from another module**
  (`ParamOwner::Modulator`). Nothing authors one today; it stays unauthored.
- **Durable-id destinations** (see *The shape*).
