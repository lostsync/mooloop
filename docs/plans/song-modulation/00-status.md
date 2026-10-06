# song-modulation — status

Linear: project [Song modulation](https://linear.app/mooloop/project/song-modulation-3e9d248073e2).
Release label `0.1.7`.

| Step | Issue | Milestone |
| --- | --- | --- |
| 01 | MOO-510 | One set for the song |
| 02 | MOO-511 | One set for the song |
| 03 | MOO-512 | Reach anything |
| 04 | MOO-513 | Reach anything |
| 05 | MOO-514 | Reach anything |

Each step is blocked by the one before it. Step 02 closes MOO-497, MOO-487,
MOO-373 and MOO-170 on the way.

Planned 2026-10-05. Step 01 built 2026-10-05, step 02 2026-10-06; 03 to 05
not started.

## Adam's rulings

**2026-10-05**, in the 0.1.7 planning thread (quoted in full in
`README.md`):

| Question | Answer |
| --- | --- |
| Does a channel keep modulators? | **No.** *"channels wont have modulators"* |
| What does a module listen to for notes? | *"you pick the input from a list of outlets sending compatible events"* |
| Does a copied channel bring its routes? | **Yes.** *"assignments would copy/paste"* |
| How many modulators can a song have? | *"all of them"*: no cap a user meets |
| Where do they live? | *"it will just go in its own pane"* |
| How big is this push? | *"let's start by moving what we have and making it work document-wide. we'll see how that went"* |

## Defaults the plan picked, open to Adam

- **Paste shares modules.** A pasted channel's routes come from the same
  modules as the original's, so one LFO drives both. The alternative is
  copying the modules too.
- **A channel preset brings its modules.** Loading one adds the modules its
  routes use to the song, as new modules.
- **0.1.6 opens a new song without its modulation.** It can't be made to
  refuse (step 01 says why). Recorded under MOO-379.

## Step 01: what was built, and where it differs from the plan

`Project.modulation` (`SongModulation`) holds every module and route;
`ChannelSetup` has no rack. A 0.1.6 song's racks are lifted on load, in
`Project::assign_channel_ids`, before the source-kind and migration passes,
which now run on the song's routes. Every release song in
`project/tests/fixtures/songs/` round-trips with no modulation table, and one
given a 0.1.6-style rack converts and round-trips
(`release_corpus.rs`).

**The shim.** The engine still runs one `ModRack` per channel. Each module
carries `rack: { channel, slot }`, the seat it runs in, and
`Project::channel_rack` / `SongModulation::channel_rack` builds the rack for
a channel from the modules seated there plus the routes landing on it. A
module seated elsewhere whose route lands on the channel (a pasted channel's,
until 03) runs as a guest in the first free slot. Session verbs edit that
rack and write it back with `store_channel_rack`, then diff every channel's
rack before and after into the existing per-channel `EngineCommand`s. Step
02 deletes `rack`, `channel_rack`, `store_channel_rack` and the diff.

Differences from the plan, each picked to keep this step small:

- **No new channel-less command variants.** The plan said to add them now;
  the shim feeds the per-channel commands instead, so 02 adds them with the
  engine that reads them.
- **Presets keep the `modulation` key on the channel document** rather than
  a new bundle table. It is the key 0.1.6 reads, so a preset saved now opens
  there with its modulation; the field is `ChannelSetup::preset_modulation`
  and a song never writes it. An Envelope gated by the preset's own channel
  is written with no gate identity, which means the receiving channel.
- **A module's seed** is stored (`seed`, the old slot for a converted
  module, the id for a new one) but the DSP still seeds by slot until 02.
- **Removing a channel keeps inputs and seats that name it**, like an
  envelope gate always did, so an undo or a re-insert finds them. Routes
  from its outlets and keyboard go. A load nulls an input naming no channel.
- **Loading a channel preset** drops the routes into that channel and from
  its outlets and keyboard, unseats the modules that sat on it (they stay in
  the song), then adds the preset's. A kit replaces all the song's
  modulation, as it replaced every rack before.
- **Paste** copies the routes into the copied channel and re-aims the ones
  from its own outlets and keyboard at the new channel. Routes whose module
  is gone are dropped and the status line says so.
- **A Math module still reads its input by rack slot**
  (`ModMathParams::input_slot`). Converted modules keep their slots, so a
  converted Math reads what it read. A guest landing in the empty slot a
  Math points at would be read by it; nothing in this step makes that
  likely. Rekeying the input to a module id (`InputSource::Module`) is
  left for 02, where the engine stops reading racks by slot.
- Selection and arming are held by `ModSourceRef` and survive a channel
  change; the shelf shows a selected module only on a channel whose rack
  holds it.

## Step 02: what was built, and where it differs from the plan

The engine runs one `SongModulator` (`engine/src/song_modulation.rs`): the
song's modules tick once per control tick, in list order, before anything
renders, and every chain reads the routes filed under it
(`CompiledModulation::chain_routes`, core `modulation_plan.rs`, compiled off
the audio thread). A track's inserts and fader take routes (MOO-497). The
session derives the set and diffs it against what it last sent once a pump
tick (`Session::sync_modulation`, beside `sync_audio_graph`), so the
modulation verbs only edit the document. A channel's strip no longer
compares racks, so a move or paste keeps other channels' voices (MOO-487).
A clocked Step and a clocked, synced Random follow the song position by the
synced LFO's rule (MOO-373).

Differences from the plan:

- **No ceiling.** Every change of shape (a module or route added, removed
  or reordered, an input repointed, a channel or track moved) arrives as a
  whole new set built off the audio thread
  (`StructuralCommand::SetModulation`); the callback carries each surviving
  module's state into it by id and hands the old set back to be dropped off
  it. Only a module's params and a route's depth and polarity are narrow
  commands, retuned in place. So the "edit past the ceiling" case is any
  edit that grows the set, and its test installs a set of 41 modules and 82
  routes over one of 1 and 2 with no allocation, free or lock on the
  callback (`song_modulation_tests.rs`).
- **Seats stay until 04.** `rack`, `channel_rack` and `store_channel_rack`
  remain, because the modulation shelf still shows a channel's modules as a
  rack. The engine does not read them, and the per-channel command diff is
  gone. Step 04's pane removes them.
- **Math reads a module, not a slot.** A Math module's operand is
  `InputSource::Module(id)`; a song saved by step 01 is given the module in
  its old slot on load. Load integrity nulls one naming a missing module.
- **The seed drives the DSP.** A Random module is seeded from its `seed`; a
  converted module's seed is its old slot, which is what its rack seeded it
  with, so converted songs null.
- **A module's state is carried by id across every install**, not only a
  reorder: undo, paste, a channel or track move, a load of the same song.
  An Envelope whose input changed releases.
- **A moving route keeps what it drives awake.** A track fed again after a
  silence reads its routes where they are, because a swept filter or fader
  never settles enough to let the track sleep; the same song unrouted does
  sleep. Tested both ways against a render that never sleeps.
- **A removed device leaves its routes in the engine's set** until the
  session's next set arrives without them; `forget_device` only forgets
  lanes.

## Open

- **Step 04:** Adam sees the pane's mock-up before its markup is built.

## Measurements

**Null.** `engine/tests/modulation_null.rs` renders every release song in
`project/tests/fixtures/songs/`, plain and with a busy 0.1.6-style rack on
every channel (eight modules of every kind, sixteen routes, an envelope
gated by the next channel, Math reading a lower and a higher slot), and
holds each against a render from the tree before step 02. All fourteen
renders null at `0e0` on the commit before MOO-373. MOO-373 then changes the
busy songs on purpose: their clocked Step and synced Random now follow the
song position.

**MOO-170's three cases** (`block_cost.rs`, `DEVICE_COST=modulation`,
release, 128-frame blocks, per-block minimum of round-robin passes, µs;
"over" is the case's mean less its unmodulated reference). This container
is shared and noisy: the references moved by up to 25% between runs, so
the "over" column is the one to read. The after numbers are two runs of
`REPS=11`.

| Case | Before: mean | Before: over | After: mean | After: over |
| --- | --- | --- | --- | --- |
| ML-P8 + Filter, nothing driven | 11.2 | -- | 11.1-11.3 | -- |
| 8 LFOs, 16 routes onto one filter | 12.6 | 1.4 | 13.2-13.7 | 2.1-2.4 |
| 8 channels, nothing driven | 80.7 | -- | 83.8-85.7 | -- |
| 8 channels x 8 LFOs, 128 routes | 123.1 | 42.4 | 124.4-125.4 | 39.7-40.5 |
| 32 channels + 4 tracks, nothing driven | 384.0 | -- | 347.7-352.5 | -- |
| 64 modules, 256 routes over 32 channels + 4 tracks | 530.6 | 146.6 | 519.3-522.1 | 169.6-171.7 |

The wide case costs about 15% more over its reference than the racks did,
and the one-channel case about a microsecond more. The eight-channel case
is unchanged. Not profiled; the likely causes are what the one set does per
block that the racks did not: every module's output copied into a
tick-by-module table, and a meter cell per module plus each live channel's
outlet and keyboard cells published.
