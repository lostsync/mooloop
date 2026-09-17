# 03 — Session state keyed by id

## Build

**Keyed by `ChannelId` instead of position:**

- `selected_device` and `effect_preset_names`. `(ChannelId, DeviceId)` is now
  a stable key for a device anywhere in the song.
- `source_preset_names` (a `HashMap<u8, _>` today) and
  `PresetNaming::Source { channel }`.
- `sample_request` (the `control-plane-seams/05` token), together with
  `LoadTarget.channel` and `LoadResult.channel`. A completed load whose
  channel is gone is dropped, and one whose channel moved still lands on it.
  That is better than today, where a moved channel's load is discarded.
- `slice_audition`, and `modulation_ui_channel`. The survey found that
  `rescope_after` does not update these two today; check that before relying
  on it, and if they really are missed, add them to the test below first.

**The parallel sample list goes away.** `ProjectSnapshot.samples` is a
`Vec<Option<Arc<SampleData>>>` kept in step with `channels` by hand, and
`ui/src/lib.rs` edits it in three places whenever channels change. Key it by
`ChannelId`, so a channel edit no longer has to touch it.

**What stays positional:**

- `Session::selected: usize`: it is a cursor.
- `compensation_sent` and `audio_graph_sent`: they mirror what the engine
  holds, and the engine works in positions.

`Session::rescope_after` should now be much shorter. Whatever it still does
should be only what an index truly requires.

**UI:** Slint callbacks keep passing row numbers. `ui/src/lib.rs` resolves a
row to an id once, where each callback is handled.
`sync_effect_spectrum_subscriptions` keys its state by id.

## Test

Using the session tests that already drive channel edits (`mooloop-session`):
for a delete, a paste and a move, every keyed map still points at the same
channel afterwards, and undo/redo restores the samples without the parallel
list. Run the test against the old tree first; any fields that fail there
are the missed-rescope bugs mentioned above.

## Rung

Session, then a `mooloop-ui` build, because the callback boundary moves.
