# Incremental structure — plan status

Linear: [MOO-30](https://linear.app/mooloop/issue/MOO-30/incremental-structure-stop-swapping-the-whole-renderstate-for-an-edit).

**Written 2026-09-17. Finished 2026-09-18, not yet heard.** Steps 01, 02
and 05 landed; 03 and 04 were decided against in 05, with Adam choosing
between the three ways forward. The archive waits on a listen.

**Active from 2026-09-18.** A note recorded the plan as
parked on Adam's *"im not convinced we're going to make it perfect by chasing
this thread"*; he corrected that the same day -- he meant not chasing the glitch
*beyond* this plan, not shelving it. So this plan is the whole of that thread,
and anything past step 05 is not to be opened. One piece of "the cheap half"
landed the same day by a cheaper route -- see the struck bullet under "What
does not".

It came out of Adam's reaction to `channel-identity/04`, on hearing a channel
move drop audio:

> *"a real engine would let us dynamically allocate and destroy audio paths at
> will without missing a sample. it should not make audio skip to add or delete
> a track if it isnt dsp heavy."*

He is right, and the engine already does it in places. This plan is about
making the rest of it work the way the parts that already work do.

## What already misses no samples

Every device edit goes through `RealtimeCommand::Structural`: the node is
built on the control thread, handed over a bounded ring, installed into the
live graph in place, and the node it displaces travels back through the
reclaim ring to be freed off-thread. Insert, remove, move, wrap in a
container, bypass, wet/dry, every knob. Nothing allocates on the callback and
nothing stops.

**Adding a channel is already incremental too** —
`StructuralCommand::AddChannel` carries prebuilt storage. It does not swap the
renderer and it does not drop audio.

## What does not

Everything that reaches `EngineHandle::install_project`: a channel moved,
pasted or deleted; a track added, removed or moved; a pattern cloned, cleared
or removed; an effect preset loaded; any undo or redo. Each builds a complete
new `RenderState` on the control thread and swaps the whole box.

`channel-identity` steps 04 and 05 made that far less destructive — the
transport carries across, and so does the live strip of every channel the edit
did not change. What remains is:

- **Tracks are rebuilt unconditionally**, because a track has no identity.
  A track added while a song plays still empties every bus strip in it.
- ~~**And so is every channel feeding a moved track.**~~ **Fixed
  2026-09-18 without a `TrackId`.** `Project::rescope_tracks_after` renumbers
  `channel.setup.channel.bus`, which made `channel-identity/05`'s carry reject
  every channel routed to a moved track. The carry now compares the setup
  *except* the bus (`same_strip` in `mooloop-engine/src/lib.rs`) and re-reads
  the destination from the incoming project after the swap, as it already did
  for the compensation delay. So the first bullet of "the cheap half" below
  is done by other means, and only the bus-side carry is left of it.
- **The swap is still a swap.** Even when every strip is carried, the install
  goes through prepare-on-control-thread, swap-on-audio-thread, reclaim. For
  adding a track that is a great deal of machinery for what ought to be one
  ring command.

## The shape

The same one `channel-identity` used, one list over, plus the incremental path
the device edits already demonstrate.

| Step | What |
| --- | --- |
| 01 | `TrackId`, minting, load-time assignment — `ChannelId` copied wholesale, including the lesson that `selected_channel`-style fields want it first. **`channel.bus` holding an id is most of the audible win on its own**: a track move then stops touching a channel's setup, so every channel carries through it. **Landed 2026-09-18** as identity only; see below |
| 02 | A `ChannelStrip` records its own `ChannelId`, and a `BusStrip` its `TrackId`. `channel-identity/05` deliberately did not need this, because the control thread did the matching; an incremental edit needs the audio thread to know what it is holding **Landed 2026-09-18** as the bus-side carry; the strips do not record their ids yet -- see below |
| 03 | `StructuralCommand::{AddTrack, RemoveTrack, MoveTrack}`, mirroring `AddChannel` **Not built, by decision 2026-09-18** -- see step 05 |
| 04 | `StructuralCommand::{RemoveChannel, MoveChannel}`, so the three channel edits stop reaching `install_project` at all **Not built, by decision 2026-09-18** -- see step 05 |
| 05 | What is left that still needs a whole-state swap — a document open, an undo — and whether it should **Decided 2026-09-18**: the swap stays, and loses nothing it did not have to -- see below |

## What step 01 actually did

Identity, and nothing that names a track by it. `BusSetup.id: TrackId`,
`Project.next_track_id`, `mint_track_id`, `Project::assign_track_ids` (run
beside `assign_channel_ids` at load and in the integrity pass),
`Project::track_index`, `track.id.duplicate`, and the session carrying the
mint both ways. `default_buses` mints the master as track 0, so a bank built
without a load already obeys the rule.

**`channel.bus` stayed a seat**, which is not what the table asked for, and
it did not need to move: the carry fix earlier the same day already stops a
track move from rebuilding the channels that feed it, by comparing their
setup *except* the bus. Converting `bus`, a track's `output`, its sends and
`EffectTarget::Bus` to ids is a second channel-identity migration, and
nothing in steps 02 to 04 needs it -- the engine is index-addressed, and
`TrackEdit` already renumbers every seat on the control thread.

## What step 02 actually did

The bus-side carry, and not the half of the row that puts an id on each strip.

`CarryPlan { channels, tracks }` replaces the bare channel list.
`carry_tracks` matches by `TrackId` and compares with `same_track`, which is
`same_strip`'s question for a track: everything but where the track sends
its audio (`output`, `sends`) and its `solo`, because none of those is strip
content. `carry_strips_from` then moves the live `BusStrip` across and hands
back what the incoming project compiles from the *whole* graph -- the
compensation ring, the solo verdict, the console switch and its accumulator.
`a_carried_track_takes_the_new_graphs_solo_verdict` guards that half, and was
checked against a tree without the swap-back.

`a_track_move_keeps_the_tail_on_the_track` is the acceptance case: a reverb
ringing on a track after its note has ended sounds the same across a track
move as with no edit at all, and is cut to under half by the channel-only
carry this step replaced.

**Strips recording their own ids** was the row's other half, and it belongs
to whichever of 03 and 04 turns out to need the audio thread to know what it
holds. The carry decides on the control thread and did not.

**What a swap still empties** is what the carry deliberately leaves with the
fresh strip: every send's state and every compensation ring. Audible only in
a song with a send or a latency-reporting device, with audio in flight
through one at the moment of the edit.

## Step 05: what still swaps, and why it stays

**Every edit still reaches `install_project`, and nothing it is not about is
emptied.** After 02 the one thing a swap still lost was the compiled half of
the graph: every compensation ring and every send's ring started from silence,
because the carry deliberately took those from the incoming project. The
session made it worse than it looked -- it forgets what it sent on every
install and resends the whole plan a tick later, each ring freshly built, so
carrying a ring through the install alone would have been undone one tick
later.

So the rule went into the engine, in the three places a ring arrives: **a
ring the same length as the live one is the live one's job, and the live one
stays** (`keep_live_ring` in `render.rs`). The install carry keeps a carried
strip's ring when the delay it is owed did not change; `SetCompensation`
hands a same-length ring straight back for reclaim; and a send bank arriving
-- by install or by `SetTrackGraph` -- adopts the old bank's ring for every
edge that is the same edge, found through `CarryPlan`'s seat maps, which
cover every surviving channel and track rather than only the carried ones.
`render::kept_rings` holds all five cases and was checked against a tree
without the rule.

**Why 03 and 04 were not built.** Structural commands for channel and track
edits would renumber around twenty index-keyed structures on the audio thread
at once -- strips, event lists, control outputs, mod racks, every pattern's
note lists, the gate table, recording state, the audio-slot bank -- plus the
handle's mirrors, where the install does it atomically on the control thread
today. After 02 and this, that risk would buy machinery rather than anything
audible: the only difference left between an edit and no edit is the
control-thread cost of preparing a `RenderState`, and nobody has heard that.
Adam chose this over building them (2026-09-18).

**What that leaves true, for whoever reopens it:** the answers to the open
questions below are unchanged by this decision rather than settled by it. An
undo is still a swap, the ordering question never arose, and the `carry_plan`
is load-bearing rather than dead code.

## The cheap half

Steps 01 and a bus-side `carry_plan` are probably where most of the *audible*
improvement is, and neither needs the incremental commands in 03 and 04:

- ~~`TrackId` on `channel.bus` stops a track move changing any channel's setup,
  so every channel carries (the finding above).~~ Done 2026-09-18 by making
  the carry ignore the bus instead; a `TrackId` is no longer needed for it.
- A `BusStrip` carried the way a `ChannelStrip` already is closes the other
  half.

That is worth knowing before starting, because it means this plan can be
stopped after 01-02 with the glitch gone and the swap still in place -- and
the rest of it is then about machinery rather than about what anybody hears.

## Open questions

- **Does an undo have to be a swap?** It restores a whole document, so it is
  the one case where "rebuild everything" is honest. But undoing a channel
  delete mid-song is an edit like any other, and `channel-identity/04` already
  decided it should keep playing. Probably it becomes a diff against the live
  project and a sequence of incremental commands, which is a bigger idea than
  the rest of this plan.
- **What is the ordering guarantee between a structural command and the value
  commands around it?** The ring is ordered, which is most of the answer, but
  a channel removal renumbers the seats every later command names. The install
  path sidesteps this by being atomic. This needs stating before step 04.
- **How much of `install_project` survives?** If a document open is the only
  caller left, the prepare/swap/reclaim machinery is still worth its weight —
  but the `carry_plan` in `channel-identity/05` would be dead code, and should
  be deleted rather than left looking load-bearing.

## Why it is worth doing

Not for its own sake. `LOOSE_ENDS.md` has the cost written out: a structural
edit stops and empties things it has no business touching, and the reason has
always been that channel and track identity was positional. That reason is now
mostly gone. The remaining work is finishing the job rather than starting one.
