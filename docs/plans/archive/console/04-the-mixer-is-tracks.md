# 04 — The mixer is tracks

The structural step, and much smaller than the version it replaces. Read
`docs/TERMINOLOGY.md` first; this step is where its words become the interface.

Everything above ships without it, and one thing above is waiting on it:
step 02's analog-sum switch is on a channel's *rack row* because the mixer
draws no track for it to live on, which is the instrument's room rather than
the mixer's.

## What it is

**A channel is assigned to a track, several channels may share one, and the
mixer draws every track.** Today the mixer draws seventeen buses and no
channels at all, which is why a channel has nowhere in the mixer to be.

Not one track per channel. That was this file's first draft and Adam's
default-project sketch below is what corrects it: a kit is several channels on
*one* track, which is also what a desk does — *"a mixer has channels that
tracks e.g. from a tape are assigned to."* The assignment is the point, and it
is many-to-one.

```text
  [ MASTER ]  [ Kick ]  [ Snare ]  [ Hat ]  [ Drums ]  [ Verb ]
                └──────────┴─────────┘          ▲         ▲
                     routed to Drums            │         │
                                          a bus, because  a send, because
                                          tracks feed it  sends feed it
```

Nothing in that picture is a different kind of object. `Drums` and `Verb` are
tracks that no channel feeds; what they *are* is decided by what reaches them.

## What has to change

- **`buses: Vec<BusSetup>` stops being padded to seventeen.** `default_buses`
  (`mixer.rs`) and `normalized_buses` (`session.rs`) both exist to guarantee
  every index is materialised; both go. Tracks are made and destroyed.
- **Tracks get stable ids**, minted from position on load the way
  `Project::assign_device_ids` mints device ids — *"a no-op rather than a
  migration"*, which keeps this inside `FORMAT_VERSION = 1`.
- **A channel's `bus` field becomes its track.** The field already exists and
  already means this; what changes is that a new project uses it (see the
  default below) instead of pointing everything at the master.
- **Tracks can be renamed.** This is where the `RenameBus` gap in
  `LOOSE_ENDS.md` closes — `MixerBus.name` saves and loads and nothing can set
  it, and a bus called "Drums" is most of why the mixer exists.
- **`sync_mixer` draws the track list** rather than the padded bus vec, and
  `MixerPane` already scrolls rather than compresses (`mixer.slint`), so a
  variable count needs no layout change. `tests/mixer_snapshot.rs` hard-codes
  strip coordinates and will need updating.

`compile_bus_graph` currently skips channels as edges on the grounds that they
all render before any bus. **That still holds** and should stay: a channel
feeds its track, and every channel renders before any track.

## The default project is the specification

Adam, on why he wanted channel grouping at all:

> *"i wanted to have channel groups so i could make the default new song be a
> drum kit, grouped, sent to mixer track 1, and then a monosynth or something
> on mixer 2, and maybe one track set up as a reverb send — a reasonable,
> modest default that sort of also demonstrates what can be done just by
> already having had it done to it."*

That is worth reading as the acceptance case for this whole step, because it
exercises every part of the model at once and nothing that is not in it:

```text
  channels                    tracks
  ────────                    ──────
  Kick   ┐
  Snare  ├──────────────────▶ 1  Drums ──┐
  Hat    ┘                               ├──▶ Master
  Bass ─────────────────────▶ 2  Bass ───┤
                       send ▶ 3  Verb ───┘
```

Three channels sharing one track is the grouping. `Verb` is a track that no
channel feeds and that a send reaches, which is the whole of "a send is a
track you set up as one". Nothing here is a type; every column is the same
object.

**It is also the demonstration.** A blank project teaches nothing; this one
shows a group, a bus and a send by having already done them. The starter kit
(`Project::starter_kit`) is where it goes.

Two things it depends on that are not in this step: the send needs step 05,
and the reverb on the return is an ordinary device on that track's rack. So
this default arrives in pieces — the grouping and the two tracks here, the
send when 05 lands.

## What grouping is, and the one thing still open

Grouping several channels onto one track is **routing**, and it needs no new
concept: it is the `bus` field several channels already share. That is what
the default above is made of.

What is not settled is whether the **sequencer rack** should also *show* them
as a group — a "Drums" header with the three channels under it, which is what
this plan's first draft proposed and which is a real convenience once a kit is
eight channels. That is presentation, it is separable, and it should be its
own decision rather than a rider here.

## What it must not do

- **It must not remove a track from the mixer when it is routed somewhere.**
  That was the first draft's design and it is the thing this step exists to
  not do. A grouped track keeps its fader, its rack, its strip and its sends.
- **It must not introduce a track *type*.** No `+ Bus`, no `+ Send`, no aux
  channel. One `+`, one kind of thing, and routing decides the rest.

## Where the routing controls go

Not crammed into the strip. Adam wants a **left sidebar carrying options and
settings for the currently selected item**, and routing setup is what it is
for. That sidebar is already recorded — `ENHANCEMENTS.md`, and
`interface-iteration/03-channel-identity.md` builds its first contents (a
channel's name and colour) — so this step should put the minimum routing
control it needs on the strip and let the sidebar take the rest when it
arrives. Do not build the sidebar here.

## Verification

`cargo test -p mooloop-core -p mooloop-session`, then the realtime/offline
equality harness — `compensation_renders_the_same_at_any_block_size`, the
offline/live equality test in `render.rs`, and `audio_edge_tests.rs`. A
dynamic track list changes what `compile_bus_graph` and `compile_latency` are
handed, and those three are what say the two renderers still agree.
