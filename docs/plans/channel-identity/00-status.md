# Channel identity — plan status

**Written 2026-09-17. Nothing has landed.** It came out of the architecture
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
| [01](01-the-id.md) | `ChannelId`, minting, load-time assignment, fresh ids for kits and pastes | core, project | not started |
| [02](02-cross-channel-addresses.md) | The four saved fields that name another channel hold an id | core, project, session | not started |
| [03](03-session-keys.md) | Session state keyed by id; the parallel sample list folds into the channel | session, UI build | not started |
| [04](04-keep-the-transport.md) | An install carries the transport across (the interim fix `LOOSE_ENDS.md` names) | engine, UI | not started |
| [05](05-strips-by-id.md) | The engine keeps strips whose id and chain survive an install | engine | not started |

Tracks (`BusSetup`) have the same problem under `TrackEdit` and the same
fix. They are left out of this plan on purpose: channels are what plugins and
recording need first, and a `TrackId` should copy whatever step 05 learns
rather than be designed alongside it.

## Open questions

None of these needs Adam before step 01.

- Step 04 changes what a user hears during an edit: the song keeps playing
  across a paste or a move. That should be listened to, not only tested.
