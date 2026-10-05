# 02 — the engine runs one set

The engine stops keeping a rack per channel and runs the song's one set.
After this step a converted 0.1.6 song plays exactly as it did, and a route
to a mixer track's insert or fader moves it (MOO-497).

**Measure first.** Before changing anything, give `block_cost.rs` the cases
MOO-170 asks for, against the unchanged tree:
- today's widest channel (8 LFOs, 16 routes, `:1516-1652`), as the baseline;
- 8 channels each with a full rack, which is what a converted busy song
  becomes;
- 64 modules and 256 routes spread over 32 channels and 4 tracks, the
  song-wide case.
Record the numbers in `00-status.md`. They set the preallocated ceiling
below, and they are the before for the same cases after the change.

## The state

In `RenderState` (`render.rs:6089-6099`):
- `modulation: Vec<ModRack>` and `modulators: Vec<ModulatorRack>`, one per
  channel, become **one** song set and one `ModulatorRack` sized to it.
- `ControlOutputs` (`:1242`) becomes one table of `[module][tick]`, not one
  per channel.
- `published_outlets` stays per channel (it is the generator's), and the
  song set reads any channel's: a source naming `GeneratorOutlet{channel,
  outlet}` reads `strips[seat_of(channel)].published_outlets[outlet]`, still
  one block late.
- The mod wheel and aftertouch are read from the named channel's
  `expression`.

**Capacity.** The set is preallocated off the audio thread at a ceiling set
from the measurement, and grows by **replacement**: when an edit would pass
it, the session sends a larger set and the audio thread swaps it in and hands
the old one back to be dropped off-thread, as `ReclaimedEffect` does. The
callback never allocates (`AUDIO_ARCHITECTURE.md`). A user never meets the
ceiling.

## The tick

The control pass (`render.rs:10420-10477`) ticks the song's modules once per
control tick, **before anything renders**, as it does today. Sources still
come before every destination by construction.
- Module inputs read the gate table for the channel they name
  (`gate_ticks`, `:1251`), not the owning channel.
- **Math** reads lower-listed modules in the same tick and higher ones a tick
  late (`math_reads_lower_slots_now_and_higher_slots_one_tick_late`). Keep
  that rule over the song list.
- **While here, MOO-373:** the Step module and a synced Random module run on
  wall-clock time, so they don't start on the downbeat and a bounce differs
  from playback. The LFO was fixed for this (MOO-127). This step rewrites
  the loop they tick in; take the same song-position rule for both.

## Resolving routes per chain

Today each channel's `ModulationBlock` (`:1278`, built `:10602`) asks its
own rack, and a bus chain gets `None` (`:11188-11200`).

- **Index the routes by chain once per edit, off the audio thread.** For
  each channel and each bus, the list of routes that land on it, keyed for
  `offset_for` (`modulation.rs:2560`). A linear walk of every route for every
  destination is fine at 16 routes and not at 256.
- Every chain's walk is handed the song set and its own route list:
  - the source pass and the channel strip, as now;
  - **every bus chain** (`:11188`), so a track's inserts take routes;
  - **a bus's fader and pan**: `resolve_strip_segments` (`:4184`), which a
    channel already uses, replacing the bus walk's plain
    `strip.output.apply`. Song automation's step 04 (MOO-419) needs the same
    call for lanes; whichever lands first adds it for both.
- `effect_is_driven` (`:8090`) and `restore_base_param` (`:8025`) drop *"a
  route counts only on a channel"*.

**Sleeping chains.** A muted or silent channel stops resolving curves, and a
bus can sleep (`:11092-11130`). A route into a sleeping chain must not be
lost: when the chain wakes it reads the route's current value, not where it
left off. Test it with an LFO on a track's insert, the track silent for a
bar, then fed again.

## Edits, carry and install

- The commands lose their `channel` (step 01 added the variants). Handle
  them on the song set and delete step 01's shim.
- `edit_modulation` (`:7923`) and `move_modulator` (`:7892`) work on the song
  set. A removed route still restores its destination's base.
- **Carry.** Module phases now belong to the song, not a strip. An install
  carries the song's `ModulatorRack` whole, matched by `ModSourceId`, so an
  LFO keeps its phase across any install: undo, paste, channel move, load of
  the same song.
- **MOO-487 goes away**, and must be shown to: `same_strip_but_effects`
  (`engine/src/lib.rs:1415`) no longer compares a rack, so a route
  renumbered by a channel move no longer rebuilds the channel and cuts its
  notes. Turn `the_install_still_rebuilds_a_channel_whose_route_was_renumbered`
  (`channel_edit_tests.rs:480`) into its opposite.
- `reseat_channels` and `reseat_tracks` (`:6742`, `:6937`) rescope the song's
  routes once, not per seat. `ChannelReseat.rack` (`:4678`) goes.

## Meters

`ModulatorMeters` (`meters.rs:757`) publish one row per module, not
`MAX_CHANNELS * CONTROL_SOURCE_SLOTS` cells. The session reads every
module's meter, not the selected channel's.

## Done when

- `block_cost.rs` has the three cases, measured before and after, recorded
  in `00-status.md`. MOO-170 closes on it.
- Every fixture song renders the same offline before and after the change
  (a null test: the difference is silence), including its LFO phases and
  random sequences.
- A route from a module to a mixer track's insert and to its fader moves
  them, live and offline (MOO-497 closes).
- A channel move and a channel paste keep every other channel's notes
  sounding (MOO-487 closes).
- The Step module and a synced Random module start on the downbeat and a
  bounce matches playback (MOO-373 closes).
- An edit past the preallocated ceiling grows the set without the callback
  allocating (an `executor.rs`-style allocation test).
- `LISTENING.md` gets one item: a busy 0.1.6 song with modulation on several
  channels, before and after, which should sound identical.
