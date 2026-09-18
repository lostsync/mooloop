# 03 — Session state keyed by id

**Landed 2026-09-17.**

## What landed

Everything in the session that was keyed by a channel seat is now keyed by
`ChannelId`, and the hand-maintained sample sidecar is gone.

**Keyed by `ChannelId` directly:** `source_preset_names`, `sample_request`,
`slice_audition`, `modulation_ui_channel`, and both arms of `PresetNaming`.

**Keyed by `ChainKey`:** `selected_device`, `effect_preset_names` and
`PresetSaveTarget::Effect`. `ChainKey` is `EffectTarget`'s twin for session
state -- `Channel(ChannelId) | Bus(u8)` -- and the asymmetry is the state of
the migration in one type: a channel is named durably, a bus is still a seat,
because a track has no identity yet.

**The parallel sample list is gone.** `ProjectSnapshot.samples` is a
`HashMap<ChannelId, Arc<SampleData>>`, and `ProjectSnapshot::seated` derives
the seat-ordered list the install wants from the project it is being installed
with. Three functions in `ui/src/lib.rs` used to keep a `Vec` in step by hand
-- a paste inserted, a delete removed, a move rotated, "or every sampler
between the two seats plays the wrong file" -- and none of them do anything to
it now.

**What stayed positional**, and why each is right to:

- `Session::selected`, a cursor.
- `ParamAddr` and `automation_target`, which are engine addresses.
- `compensation_sent`, `audio_graph_sent`, `ResolvedDocument.samples` and
  `ExportRequest.samples`: they mirror or feed something that works in seats.

`Session::rescope_after` is three lines and one of them is a comment about
what is no longer there. The shared `rescope_targets` walk became
`rescope_track_targets` and takes a `TrackEdit`, because after this there is
nothing for a *channel* edit to do in it.

## Two bugs it fixes

`rescope_after` never mentioned `slice_audition` or `modulation_ui_channel`,
so both had been mis-keyed by every structural edit since they were written.
Verified failing on the tree before the change.

`modulation_ui_channel` in particular now does what its own comment always
claimed: "changing channels clears both even when the new channel happens to
occupy the same runtime slot" is a statement about identity, and it was
written against a `usize` seat.

An in-flight sample load also improves. The old token walk carried a request
to its channel's new seat, but a completion compared against the seat it was
*asked* at -- so a load whose channel moved under it was discarded. Keyed by
identity there is nothing to carry and nothing to discard.

## One deliberate behaviour change

`selected_device` is no longer **cleared** when its channel is deleted. A
`ChainKey` naming a departed channel resolves to nothing, which is what
clearing arranged, and ids are never reused, so it cannot come to name a
stranger. Leaving it is better: undo restores the channel with the same id and
the selection returns, where the old walk had already thrown it away.
`selected_device_slot` is the only reader and it resolves. The same is true of
`effect_preset_names`, whose entries now survive an undo.

## What the tests had to learn

Three existing tests called `Session::rescope_after` without reordering
`session.channels`, which modelled nothing once the keys became identities --
the truth is in the rack now, so the rack has to move. They install first and
rescope second, which is what the application does (`ui/src/lib.rs` calls
`rescope_after` *after* `install_project_in_ui`).

One of them named channel 3 on a session that had one channel, and got away
with it because `source_preset_names` took any `u8`. There is no key for a
channel that is not there now, so the test had to mean what it said.
