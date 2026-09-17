# 05 — The engine keeps strips across an install

The real fix `LOOSE_ENDS.md` names, and the prerequisite `plugin-hosting/`
step 06 depends on.

## Build

- A `ChannelStrip` records the `ChannelId` it was built for, and
  `RenderState` keeps a `ChannelId → strip index` map.
- When a project is installed over a live one, the control thread works out,
  for each incoming channel, whether the outgoing generation has a strip with
  the same id and an equivalent chain (same device ids and kinds in the same
  order, and the same source kind). Those strips are **moved** into the new
  generation rather than rebuilt; everything else is built fresh, as today.
- The move itself has to happen on the audio thread at the swap, because the
  outgoing strips are live until then. Moving a strip is moving a `Box`, not
  cloning or freeing one, so it is realtime-safe. Every strip that isn't
  carried over leaves through the reclaim ring, as it does now.
- The meters, device telemetry, audio tap plan, sample slots and slice slots
  are all per-position tables. Each one is either rebuilt for the new
  positions, or keyed so a moved strip keeps its meter. `LOOSE_ENDS.md` lists
  them. Before starting, list every per-channel table in `render.rs` and
  `lib.rs`; the survey counted about 45 index-keyed sites.
- Parameter values differ between the two projects even when the chain is the
  same (the edit may have been an undo that changed a knob). A carried strip
  therefore receives the incoming project's parameters as events, not as a
  rebuild.

## Test

- A pure reorder: every node is carried over, nothing reaches the reclaim ring
  except the outgoing `RenderState` shell, and a delay tail playing on a moved
  channel continues across the swap (compare samples before and after).
- A delete: the deleted channel's strip is reclaimed and the others are carried.
- A chain edit on one channel: that strip is rebuilt and the rest are carried.
- `CountingAllocator`: the swap block allocates and frees nothing.

## Acceptance

Listen: move and delete channels while a song with delay and reverb tails
plays. Nothing cuts except the channel you deleted.
