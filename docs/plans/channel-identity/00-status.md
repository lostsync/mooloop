# Channel identity — plan status

**Written 2026-09-17. Steps 01, 02 and 03 landed 2026-09-17.** It came out of the architecture
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

**Which layers change** (the survey counted sites on 2026-09-17):

| Layer | Sites | Decision |
| --- | --- | --- |
| Saved document | 7 | The three fields that name *another* channel switch to `ChannelId`: control bindings, the envelope gate's `input_channel`, and Aux In's `source_channel`. `selected_channel` too. Routes and lanes stay as they are: they are nested inside their channel and `integrity::rescoped_home` already forces their scope to it. |
| Session / control thread | ~20 | Keyed by `ChannelId`. This is where the parallel lists and the fields `rescope_after` misses live. Resolve id → index once, at the point a command is sent. |
| Engine | ~45 | **Stays a `u8` index.** The engine gains exactly one new thing: a map from `ChannelId` to its strip, used at install time. |
| UI | ~30 | Slint keeps row positions. The id is resolved at the callback boundary in `ui/src/lib.rs`. |

## Steps

| Step | What | Rung | State |
| --- | --- | --- | --- |
| [01](01-the-id.md) | `ChannelId`, minting, load-time assignment, fresh ids for kits and pastes | core, project | **landed 2026-09-17** |
| [02](02-cross-channel-addresses.md) | `selected_channel` holds an id | core, project, session | **landed 2026-09-17**, one field of four |
| [03](03-session-keys.md) | Session state keyed by id; the parallel sample list folds into the channel | session, UI build | **landed 2026-09-17** |
| [04](04-keep-the-transport.md) | An install carries the transport across (the interim fix `LOOSE_ENDS.md` names) | engine, UI | not started |
| [05](05-strips-by-id.md) | The engine keeps strips whose id and chain survive an install | engine | not started |
| [06](06-the-remaining-cross-channel-addresses.md) | The other three fields that name another channel | core, session, dsp | not started; the gate half waits on 05 |

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

## Open questions

None of these needs Adam before step 04.

- Step 04 changes what a user hears during an edit: the song keeps playing
  across a paste or a move. That should be listened to, not only tested.
