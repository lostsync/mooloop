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

Planned 2026-10-05. Step 01 built 2026-10-05; 02 to 05 not started.

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
- Selection and arming are held by `ModSourceRef` and survive a channel
  change; the shelf shows a selected module only on a channel whose rack
  holds it.

## Open

- **Step 04:** Adam sees the pane's mock-up before its markup is built.

## Measurements

Step 02 records MOO-170's three `block_cost.rs` cases here, before and
after.
