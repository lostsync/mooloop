# 01 — `ChannelId`

## Build

- In `mooloop-core`, next to `DeviceId` (`effect.rs`): `ChannelId(u32)` with
  `UNASSIGNED = u32::MAX`, `Default` = unassigned, and serde as a bare number.
  Copy `DeviceId`'s doc comment on why unassigned is the top of the range: an
  unminted id should address nothing, not the first channel.
- Add a defaulted `id: ChannelId` to `ProjectChannel` (`project.rs`), and a
  defaulted `next_channel_id: u32` to `Project`. Skip serializing either when
  it holds its default.
- Add `mint_channel_id(&mut u32)` beside `mint_device_id` (`structure.rs`).
  `insert_channel` and the session's add-channel path both mint through it.
- Add `Project::assign_channel_ids`, modelled on `assign_device_ids`: if no
  channel has an id, each takes its position; then raise the counter above
  the highest id; then give any remaining unassigned channel a fresh one. Call
  it from `mooloop-project` wherever `assign_device_ids` is called (the
  project, kit and channel-document load paths), and before
  `integrity::repair_project`.
- Kit and channel documents: clear the id on save and mint on load, the same
  as device presets (`PROJECT_FORMAT.md` records that rule for devices; add
  the channel line beside it).
- The channel clipboard: a pasted channel mints a new id. Two channels with
  the same id is the one state this step must make impossible.
- Add a `Project::channel_index(ChannelId) -> Option<usize>` lookup. It is the
  only id → position conversion anything should use.

## Test

- An old project with no ids loads with id = position, and saving it then
  writes those ids.
- Delete a channel, add a channel: the new id is not the deleted one.
- Paste a channel twice: three distinct ids.
- A kit load gives new ids even when the kit file carries some.
- `integrity` refuses, or repairs with a doctor message, a file that has two
  channels with the same id. Say which, and write the rule into
  `PROJECT_FORMAT.md`.

## Docs

`PROJECT_FORMAT.md`: the field, the defaulting rule, and the uniqueness rule.
