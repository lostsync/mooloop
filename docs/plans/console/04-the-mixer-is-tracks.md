# 04 — The mixer is tracks

The structural step, and much smaller than the version it replaces. Read
`docs/TERMINOLOGY.md` first; this step is where its words become the interface.

Everything above ships without it, and one thing above is waiting on it:
step 02's analog-sum switch is on a channel's *rack row* because the mixer
draws no track for it to live on, which is the instrument's room rather than
the mixer's.

## What it is

**Every channel gets a track, and the mixer draws all of them.** Today the
mixer draws seventeen buses and no channels at all, which is why a channel has
nowhere in the mixer to be.

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
- **A channel's `bus` field becomes its track**, and a new project makes one
  track per channel instead of pointing every channel at the master. That is
  the whole of "every channel has a track"; the field already exists.
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
