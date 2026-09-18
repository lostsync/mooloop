# Channel identity — plan status

Linear: [MOO-15](https://linear.app/mooloop/issue/MOO-15/channel-identity-give-a-channel-a-durable-channelid)
(Done). Step 06 split into
[MOO-31](https://linear.app/mooloop/issue/MOO-31/control-bindings-name-a-channel-by-id-not-by-seat),
[MOO-32](https://linear.app/mooloop/issue/MOO-32/aux-ins-source-a-durable-identity-beside-the-position-parameter) and
[MOO-33](https://linear.app/mooloop/issue/MOO-33/the-envelope-gate-names-a-channel-by-id)
(all Done).

**Written 2026-09-17. Finished, heard and archived 2026-09-18.** All six steps
landed. Adam, on the finished arc:

> *"it is still glitchy sounding when you move stuff around, but it is a lot
> better and the audio routing does survive moves. if we've done the channel id
> work let's just say it's done."*

**The half this plan is answerable for is the routing, and it passed.** An Aux
In, an envelope gate and a control binding all keep naming their channel
rather than whoever took its seat. The residual glitch is the other half --
the renderer still being rebuilt -- which is `incremental-structure/`, and he
is deliberately not chasing it: *"im not convinced we're going to make it
perfect by chasing this thread."*

One thing landed after the acceptance and before the archive:
`reports/fable-2026-09-18.md` found that step 04 named neither `held_keys` nor
the in-flight `recording` capture, and the fix for that is in step 04's own
file. It was step 04 that made the omission matter -- while a structural edit
stopped the song there was nothing to lose.

It came out of the architecture
section of `reports/fable-2026-09-17.md`, and Adam asked for it the same day:
a channel gets a durable id, the way a device already has one, before plugin
hosting starts keying anything by channel position.

## Why now

A channel today is its position in `Project.channels`. Every insert, remove
and move is one renumbering value, `ChannelEdit` (`core/src/structure.rs`),
applied twice: to the document (`Project::rescope_after`, `project.rs`) and to
the session (`Session::rescope_after`, `session.rs`). The engine never sees
the edit, because **every channel edit except Add rebuilds the whole
`RenderState`** through `install_project`.

That rebuild is what makes this more than tidiness:

- **It already costs something.** `LOOSE_ENDS.md`, "Every structural edit stops
  the song": a paste, delete or move stops and rewinds the transport and
  empties every voice, tail and delay line in the project, including on
  channels the edit never touched. That entry already names the fix: strips
  keyed by a durable id, so an install that finds the same id with the same
  chain keeps its node, and a move becomes a relabel the audio thread never
  sees.
- **Plugins make it expensive.** `plugin-hosting/` mints a project-wide
  `PluginSlotId` precisely so a channel move does not renumber plugins, but
  it never mentions `install_project`. As things stand, any channel edit
  would tear down and reload every plugin in the song. **Step 05 here is a
  prerequisite of `plugin-hosting/` step 06** (the first plugin in a chain),
  and that plan's status now says so.
- **Recording will want it.** A take in progress, and the sample-load token
  `control-plane-seams/05` had to key by index "until a durable channel
  identity exists", both need to name a channel across an edit.

## The shape

Copy `DeviceId` (`core/src/effect.rs`), which already solved every part of
this once:

- `ChannelId(u32)`, `Copy`, saved as a bare number, `UNASSIGNED = u32::MAX`,
  minted from `Project.next_channel_id` and never reused.
- **Old files need no migration.** An `assign_channel_ids` pass, run beside
  `assign_device_ids` before `integrity::repair_project`, gives every channel
  its position as its id when none has one. So in an old file, an address
  that says `channel = 3` means id 3, which is the same channel.
  `FORMAT_VERSION` stays 1, and every new field is defaulted.
- **Kits, channel documents and pasted channels drop their ids and get new
  ones**, the way device presets already do.

**The four-fields table below was too optimistic**, and step 02 found out how
on contact. Only `selected_channel` converted with no change to the saved
form. Of the other three, the envelope gate has an audio-thread reader and so
depends on step 05 rather than preceding it; Aux In's source is an addressable
parameter whose descriptor range an id does not fit, and Adam settled on
2026-09-17 that the parameter stays a position with the identity beside it;
and a control binding is unblocked but needs a decision about how to hold a
scope that is an id. All three are
[06](06-the-remaining-cross-channel-addresses.md).

**Corrected twice on 2026-09-18.** First: the gate half was said to be waiting
on step 05, because the layer table promised the engine an id-to-strip map
there. Step 05 landed *without* one -- it decides the carry on the control
thread from two `Project` values, so the strips never learn their ids -- so the
gate looked blocked on `incremental-structure/` step 02 instead.

Then: **it was not blocked on that either.** Two fields, each with one
meaning -- the shape Adam had already settled for Aux In the day before -- is
not the "rewrite the field on the way in" this plan ruled out, and it needs
nothing from the engine at all. All three landed the same day. [06](06-the-remaining-cross-channel-addresses.md)
has the three shapes and which of them each field took.

**Which layers change** (the survey counted sites on 2026-09-17):

| Layer | Sites | Decision |
| --- | --- | --- |
| Saved document | 7 | The three fields that name *another* channel switch to `ChannelId`: control bindings, the envelope gate's `input_channel`, and Aux In's `source_channel`. `selected_channel` too. Routes and lanes stay as they are: they are nested inside their channel and `integrity::rescoped_home` already forces their scope to it. |
| Session / control thread | ~20 | Keyed by `ChannelId`. This is where the parallel lists and the fields `rescope_after` misses live. Resolve id → index once, at the point a command is sent. |
| Engine | ~45 | **Stays a `u8` index**, and in the end gained nothing at all: step 05 does its matching on the control thread, and step 06's two derived fields reach the engine as the seats they always were. `ChannelId` does not appear in `mooloop-engine`. |
| UI | ~30 | Slint keeps row positions. The id is resolved at the callback boundary in `ui/src/lib.rs`. |

## Steps

| Step | What | Rung | State |
| --- | --- | --- | --- |
| [01](01-the-id.md) | `ChannelId`, minting, load-time assignment, fresh ids for kits and pastes | core, project | **landed 2026-09-17** |
| [02](02-cross-channel-addresses.md) | `selected_channel` holds an id | core, project, session | **landed 2026-09-17**, one field of four |
| [03](03-session-keys.md) | Session state keyed by id; the parallel sample list folds into the channel | session, UI build | **landed 2026-09-17** |
| [04](04-keep-the-transport.md) | An install carries the transport across (the interim fix `LOOSE_ENDS.md` names) | engine, UI | **landed 2026-09-17**, listened to |
| [05](05-strips-by-id.md) | The engine keeps strips whose id and chain survive an install | engine | **landed 2026-09-17**, listened to 2026-09-18 |
| [06](06-the-remaining-cross-channel-addresses.md) | The other three fields that name another channel | core, session, dsp | **landed 2026-09-18** as MOO-31, MOO-32 and MOO-33 |

Tracks (`BusSetup`) have the same problem under `TrackEdit` and the same
fix. They are left out of this plan on purpose: channels are what plugins and
recording need first, and a `TrackId` should copy whatever step 05 learns
rather than be designed alongside it.

## What step 01 actually did

Three things the plan did not say, recorded here rather than left to be
rediscovered in step 02:

- **A kit and a channel document hold `ChannelSetup`, not `ProjectChannel`**,
  so neither carries a channel id and there is nothing to strip on save. The
  rule the plan wanted still exists, but it lands on the *merge*: a kit entry
  that falls past the end of the song makes a channel and is minted like any
  other, while an entry landing on a live channel keeps that channel's id,
  because it changes what the channel plays rather than which one it is.
  `PROJECT_FORMAT.md` says so beside the device-preset rule.
- **The session had to carry the id and the mint.** `Session` does not hold a
  `Project`: it decomposes one into `ChannelState` on the way in and rebuilds
  one on the way out. Those two directions run on different paths, and the
  difference is worth knowing before step 03 reasons about cost:

  - `project_snapshot` runs on **every undoable edit**, including drawing one
    note -- twice, for the `before` and `after` of a history entry
    (`record_project_history`, `ui/src/lib.rs`). The session itself is
    mutated in place and is *not* rebuilt; the snapshots only go on the undo
    stack.
  - `replace_project` runs on the narrower set: a channel insert, delete,
    move or paste, a kit load, a track add, an undo, a redo, a load. Those
    go through `ProjectEditSender` to the install at `ui/src/lib.rs`, which
    also tears down the whole `RenderState`.

  So a field not carried in both directions is not reset by the next edit --
  it is reset by the next **undo**, having been silently absent from every
  history entry since. Without `ChannelState.id` and `Session.next_channel_id`
  travelling both ways, the ids `add_channel` minted would vanish there. That
  is the fault `ChannelState::next_device_id`'s own comment was written for,
  and it is the minimum of step 03, not the whole of it: the session's ~20
  parallel lists are still keyed by index.
- **`insert_channel` mints unconditionally** rather than only for a paste.
  It is the paste path, what it is handed is a clipboard copy of a channel
  very probably still in the song, and making the mint the insert's own rule
  is what makes two channels wearing one id unreachable rather than merely
  unlikely. `channel.id.duplicate` in `integrity.rs` then guards only the
  hand-edited file, which is the only way in that remains.

Eleven round-trip tests in `mooloop-project` had to say
`ProjectChannel::ds01(0, 1).with_id(id)` where they said
`ProjectChannel::ds01(0, 1)`, and one had to stop building a full bank by
cloning one channel thirty-two times. Both are the same correction: a channel
is now a thing with an identity, and a test that makes one has to say which.

## What step 02 actually did

`Project.selected_channel` is a `ChannelId`, and the bug that made it worth
doing first is fixed: an insert above the selected channel never moved the
selection at all, and a removal clamped instead of following. Both were
verified failing on the tree before the change.

The step's own proposed test -- a control binding surviving a delete and a
redo -- passes on the old tree, because index arithmetic is symmetric under
undo. It was dropped, as that file instructed, and two selection tests
replaced it.

## What step 03 actually did

All of it, plus `PresetNaming`, which the step did not list and which is the
clearest case in the session for a durable key: it is resolved when a save
dialog is confirmed and applied when the file has landed, so a channel edit
during a slow write could put a preset label on a channel nobody saved from.

The two fields the step flagged as suspect -- `slice_audition` and
`modulation_ui_channel` -- really were missed by `Session::rescope_after`, and
had been mis-keyed by every structural edit since they were written. Verified
failing before the change.

`ChainKey` is the new type it needed: `EffectTarget`'s twin for session state,
`Channel(ChannelId) | Bus(u8)`. A bus is still a seat, so
`ChainKey::after_track` is where the remaining half of the migration is
spelled -- and it is the shape a `TrackId` would delete.

## What step 04 actually did

All of it, and wider than it asked. The step proposed keying on
`ProjectEdit { edit: Some(..) }`, which covers the three channel edits and
would have left a track add, a track move, an effect preset load and a sample
load still stopping the song -- all four go through the same install, and
`LOOSE_ENDS.md`'s own list names them. The distinction that matters is edit
versus open, and it falls on the function boundary: every `ProjectEdit` is an
edit and the three other callers of `install_project_in_ui` are the opens.
Undo and redo keep the transport too.

## What step 05 actually did

All of it, for channels, and with less machinery than it asked for. The match
is `ChannelId` plus `ChannelSetup` equality rather than structural equivalence
plus parameter replay, and the decision is made on the control thread, so the
audio thread only swaps boxes. The strip does not record its own id, which the
plan wanted; that is only needed if the *audio thread* does the matching, and
it will be needed by the incremental work below.

Two hazards, both silent, both found by asking what a carried strip holds that
does not come from its own channel's setup: the compensation ring (derived from
the whole project) and the audio slot (rebound every install -- a carried strip
would keep reading the retired generation's, so a sample loaded onto a
reordered channel would never be heard). `05-strips-by-id.md` has both.

**`plugin-hosting/` step 06 is now unblocked**, which was the other reason
this plan exists.

## What step 06 actually did

All three fields, in one day, in two shapes. A control binding's target
**replaced** its seat with a `ParamKey` over a `ChainKey`, because nothing
downstream of the control map wanted a seat. Aux In's source and the envelope
gate's input each kept their seat and took an identity **beside** it, for two
different reasons -- one is an addressable parameter whose descriptor range an
id does not fit, the other is read on the audio thread.

Three things worth keeping:

- **The saved form of a binding did not move one byte**, which step 02 had
  worried it would. `ChannelId` is transparent and `EffectTarget::Channel` was
  already `{ channel = 3 }`, so an old index decodes as the id it already
  meant.
- **`ChainKey` moved down into `mooloop-core`.** Step 03 had written it in
  `mooloop-session`; the control map needed the same enum, and two copies of
  `Channel(ChannelId) | Bus(u8)` across a crate boundary is the duplication
  `AGENTS.md` opens with.
- **The gate's block dissolved rather than being cleared.** It had been
  recorded twice as waiting on something, and both times the thing it was
  waiting for was a way for the *engine* to resolve an id. It never needed
  one.

## The question this plan did not answer

Adam, 2026-09-17, on hearing step 04: *"a real engine would let us dynamically
allocate and destroy audio paths at will without missing a sample. it should
not make audio skip to add or delete a track if it isnt dsp heavy."*

He is right, and the engine already does it in places -- every device edit, and
adding a *channel*, go through `StructuralCommand` and miss no samples. Adding
a **track** does not, and neither does anything else that reaches
`install_project`. Step 05 removed the destruction; the swap is still there.

Making track edits incremental, the way channel adds already are, wants a plan
of its own. It needs a `TrackId` (which this plan deliberately deferred so it
could copy whatever 05 learned) and a `ChannelId` on the strip itself.

## Open questions

None of these needs Adam before step 04.

- **Step 05 was listened to on 2026-09-18 and the channel cases are closed.**
  Adam: *"a lot better than it was... not jarring when it happens during
  production."* What is left is a **track** move, which still glitches --
  and not only because bus strips are rebuilt: `rescope_tracks_after`
  renumbers `channel.setup.channel.bus`, so every channel feeding a moved
  track fails the setup comparison and is rebuilt as well. A `TrackId` fixes
  both halves at once. `incremental-structure/` has it.
- **Step 04 was listened to on 2026-09-17 and keeps time.** The dropout is
  still there and is on *every* channel, not the moved one -- a null install
  silences the master exactly as completely as a reorder, measured at the
  executor. Step 05 is what closes it, and that measurement is kept as the
  test it will invert.
