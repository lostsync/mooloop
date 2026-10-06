# 01 — the song's modulation set

The document, the save file and the session learn that modulation belongs to
the song. Nothing in the engine changes yet: step 02 makes it play. The step
is headless and tested without a window.

## The type

A new `SongModulation` in `core/src/modulation.rs`, held by
`Project.modulation` (`core/src/project.rs`, beside `playlist`):
- `modules: Vec<SongModule>`, where a `SongModule` is today's `ModSlot`
  (`:1852`) plus a `name` and a `ModSourceId` that is **unique in the song**,
  minted from one song-level counter.
- `routes: Vec<ModRoute>`, today's route type.
- **Not `Copy`, and no fixed array.** The `[_; 8]` and `[_; 16]` of
  `ModRack` are a per-channel layout; the song's set is sized from the song.
  There is no count a user meets (Adam: *"all of them"*). The engine's
  preallocated ceiling is step 02's to set, from a measurement.

**Sources that meant "my channel" name their channel.**
`ModSourceRef::GeneratorOutlet` and `::Performance` (`mod_metadata.rs:317`)
gain a channel, by `ChannelId` (durable, so a channel move does not touch
them). The module inputs that read notes do the same:
- the Envelope's gate already has `input_channel` / `input_channel_id`
  (`modulation.rs:1195`);
- the LFO's retrigger, the Step's advance and the Random's trigger gain one
  (`dsp/modulator.rs:810-838` reads the owning channel today).

Model each as **an input naming an outlet** (Adam: *"you pick the input from
a list of outlets sending compatible events"*): an `InputSource` that is
`None`, `ChannelNotes(ChannelId)`, or later anything else that sends a gate.
That is the canvas's inlet in its first form; keep the type open to more
variants rather than a bare channel id.

**The Math module's input is a slot today.** `ModMathParams.input_slot`
(`modulation.rs:1344`) is an index into its channel's rack, clamped to the
eight slots (`:1517`) and exposed as a parameter (`MATH_PARAM_INPUT_SLOT`,
`:1439`). In a song-wide set an index is meaningless. Make it an input
naming an outlet too: `InputSource::Module(ModSourceId)`, the canvas's
module-to-module wire in its first form. Its field comment already expects
this. Conversion maps the old slot to that module's new song id. Its
parameter descriptor goes, and so does the slot remap in
`ModRack::retarget` (`:2262`) and `render.rs:7887`, because an id never
moves. `ParamOwner::Modulator { slot }` also names a module by slot. Nothing
authors one, so rekey it to a `ModSourceId` now with no migration. The
canvas's wire into a module's rate will then need no format change.

**The Random module's seed** comes from its slot index today
(`dsp/modulator.rs:725`). Seed it from its `ModSourceId` instead, so a song
plays the same random sequence whatever order its modules are listed in.
Converted songs must keep their sequence: seed a converted module from the
slot it had, recorded on the module, and a new one from its id.

## Channels own no modulators

`ChannelSetup.modulation` goes. Everything that reads it moves to
`Project.modulation`:
- `ChannelSetup::rescope_modulation` (`project.rs:540`) and every caller;
- `Project::rescope_after` / `rescope_tracks_after` (`:1928`, `:1870`) walk
  the song's routes instead of each channel's rack, with the same
  `ChannelEdit` / `TrackEdit` permutations (`core/src/structure.rs`);
- `remove_channel` (`:1718`) removes the routes that reach the removed
  channel, and the modules no route uses any more stay (a module with no
  route is still a module the user made);
- `forget_device` drops the routes into a removed device, song-wide;
- the two load migrations that walk `channels[].setup.modulation.routes`
  (`migrate_linear_strip_volume`, `:1574`; `migrate_retired_buffer_offset`,
  `:1629`) run on the converted set.

## Copy, paste and presets

**Paste** (`session/src/channel.rs:348`, `core/src/project.rs:1745`): the
clipboard carries the routes that point into the copied channel. Pasting
adds the same routes again, aimed at the new channel, **from the same
modules** (Adam: *"assignments would copy/paste"*). No module is duplicated.
If a module the clipboard names is gone (deleted since the copy), its routes
are dropped and the paste says so.

**A pasted channel's self-gates.** A module whose input is the copied
channel's notes stays on the original channel. Don't rewrite it.

**Channel presets and kits** carry a `ChannelSetup` today, rack included.
Saving one now writes the modules its routes use plus those routes, in a new
`modulation` table on the bundle. Loading one **adds** those modules to the
song with fresh ids, and their routes aimed at the receiving channel. A
module whose input was the preset's own channel's notes is aimed at the
receiving channel.

**ML-M1's factory patches** set `setup.modulation` (`project/src/factory.rs:295`,
`core/src/mlm1_factory.rs:49`). They become the same fragment and land the
same way.

## Save, load and converting a 0.1.6 song

- A new top-level `modulation` table in the song (`PROJECT_FORMAT.md`). No
  `FORMAT_VERSION` bump (`project/src/lib.rs:40`): the format grows by
  defaulted fields.
- **Loading a song that has `channels[].setup.modulation`** lifts every
  channel's rack into the song's set, in channel order:
  - each module gets a fresh song-wide id, and its routes follow it;
  - an outlet or performance source gets its channel's `ChannelId`;
  - LFO, Step and Random note inputs get their old channel's notes;
  - a Math module's `input_slot` becomes the song id of the module that
    sat in that slot (an empty slot becomes `None`);
  - a route with a `Channel` scope keeps it (`rescope_modulation` already
    pointed it at its own channel);
  - each module is named `<channel name> <kind> <n>`, so the pane can tell
    two LFOs apart.
- A converted song **saves in the new shape** and no longer writes
  per-channel racks.
- **An older build opening a new song** ignores the unknown table and plays
  with no modulation, and its next save drops it. 0.1.6 checks `contains`
  only on effect presets; a song goes straight to `validate_envelope`
  (`project/src/lib.rs:1461`). So nothing this build writes can make 0.1.6
  refuse. Record it in `PROJECT_FORMAT.md` as a known case of MOO-379 (*A
  song saved by a newer build opens silently in an older one*), and fix the
  general case there rather than here.
- `integrity::repair_project` checks the set:
  - module ids unique; a route naming a missing module is dropped;
  - a route aimed at another channel is **no longer pointed home**: delete
    that rule (`integrity.rs:1676`) and its test
    `a_route_stranded_on_another_channels_index_is_pointed_home`, and check
    the address exists wherever it points;
  - an input naming a channel or module that is gone becomes `None`;
  - non-finite depths are fitted, as `check_modulation` does (`:1764`).

## Session verbs

Rekey `session/src/modulation.rs` from the selected channel to the song.
The verbs keep their names and drop the channel:
`add_modulation_source`, `remove_modulation_source`,
`move_modulation_source`, `set_modulator_param`, `set_route_polarity`,
`remove_route`, plus a new `set_module_input(id, InputSource)` that replaces
`set_envelope_input_channel` (`:273`) and covers all four kinds.

- A new module's input defaults to the selected channel's notes, so adding
  an Envelope with a channel selected behaves as it does today.
- Selection and arming (`session.rs:127`) name a module by `ModSourceId`, not
  a slot within a channel, and no longer reset when the selected channel
  changes (`ui/src/lib.rs:6044`).
- Undo stays whole-project snapshots. `project_snapshot` must clone
  `Project.modulation`, or an undo would install a song with none.

**Commands.** `core/src/bridge.rs:415-457` drop the `channel` field. Add the
variants now; the engine keeps its per-channel handling until 02, fed by a
shim that maps each module to the channel it came from. Don't let the shim
outlive 02.

## Done when

- `Project.modulation` exists, and `ChannelSetup.modulation` does not.
- Every song under `tests/fixtures/songs/` loads converted, saves, and loads
  again with the same modules, routes and inputs (a round-trip test per
  fixture). A song with no modulation saves with no `modulation` table.
- Channel move, remove and insert, and track move and remove, rescope the
  song's routes, with the existing rescope tests moved onto the new set and
  passing.
- Paste adds the copied channel's routes from the same modules; a channel
  preset lands its modules and routes; an ML-M1 factory patch sounds as it
  did (its routes reach its own channel).
- Integrity's new rules have a test each, and the "pointed home" rule is
  gone.
- `PROJECT_FORMAT.md` documents the table, the conversion and what 0.1.6
  does with a new song.
