# 02 — The selection holds an id

**Landed 2026-09-17, one field of the four this step was written for.** The
other three each turned out to be waiting on something, and they are now
[06](06-the-remaining-cross-channel-addresses.md). What this file described
and what was buildable came apart on contact; the survey below is what the
code actually said.

## What landed

`Project.selected_channel` is a `ChannelId`.

The serde form did not change. It is a bare number exactly as it was, and a
song written before channels had identities reads its old index as an id --
which names the same channel, because step 01 gives a bank with no identities
its positions. Every file written so far means the same thing after this
change as before it.

**It is also the field that was broken.** As a position the selection had to
be renumbered by every structural edit, and two of the three did it wrong:

- `insert_channel` never touched `selected_channel` at all, so adding a
  channel above the selected one silently moved the selection onto its
  neighbour.
- `remove_channel` *clamped* -- `min(len - 1)` -- rather than following the
  edit. That is only accidentally right when the selection sits at the end of
  the bank. Select channel 1 of four and delete channel 0, and the selection
  stayed on index 1, which is now a different channel.

Both were verified failing on the tree before the change, which is the only
way to know a test of this shape is doing anything -- `AGENTS.md`'s
`bar-arithmetic` lesson, applied to a test rather than to a check. The pair
of them is `the_selection_survives_an_edit_to_another_channel`.

`insert_channel` and `move_channel` now leave the selection alone, because
there is nothing to renumber. `remove_channel` answers only for the one case
that needs an answer -- the selected channel was the one deleted -- and hands
the selection to whoever closed the gap. `Project::selected_index` is the
only place that resolves it, and it falls back to the first channel.

One thing moved in `integrity.rs` on the way past: the `assign_channel_ids`
normalization at the top of `check_channel_ids` is no longer gated on
`doctor.apply`. Every check downstream that resolves a channel by id is
unanswerable until it has run, and gating it made `inspect_project` report a
stale selection on an ordinary song that simply had not been given its ids
yet. `inspect_project` hands the pass a clone, which is what makes that safe.

## What the plan asked for that was not true

- **"Only the type changes, the serde form doesn't."** True for the
  selection. Not true for the envelope gate, whose parked sentinel is
  `u8::MAX` = 255 while `ChannelId::UNASSIGNED` is `u32::MAX`; 255 is a
  perfectly ordinary id, so that one needs a real decode rule rather than a
  reinterpretation.
- **"Resolve the scope when the map is compiled for the engine."** The
  control map is never compiled for the engine. It lives entirely on the
  control thread and is dispatched from `session/src/midi.rs`. That makes
  bindings *easier* than the plan thought, not harder -- but it also means
  the resolution point the plan named does not exist.
- **The test this file asked for proves nothing.** A control binding on
  channel 5, with channels 2 and 3 deleted and then restored, comes back
  naming channel 5 on the old tree: the removals renumber it down to 3 and
  the insertions renumber it back up. Index arithmetic is symmetric under
  undo, so the case is green before the fix. Dropped, as this file instructed.
  The two selection tests are what replaced it.
