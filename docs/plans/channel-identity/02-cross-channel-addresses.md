# 02 — Saved fields that name another channel

Four saved fields point at a channel other than the one they are stored in.
Today `Project::rescope_after` renumbers each of them on every structural
edit, and a missed case silently points at the wrong channel.

| Field | Where |
| --- | --- |
| `ControlBinding.target` (`ControlTarget::Param(ParamAddr)`) | `core/src/control.rs` |
| `EnvelopeParams.input_channel: u8` (`u8::MAX` = parked) | `core/src/modulation.rs` |
| `AuxInParams.source_channel: i16` (`NO_SOURCE` = none) | `core/src/aux_in.rs` |
| `Project.selected_channel: u8` | `core/src/project.rs` |

## Build

- Each of the four stores a `ChannelId`. Because step 01 gives an old file
  id = position, **the number already in an old file is the right id**, so
  the serde form doesn't change. Only the type does, and the existing
  sentinels map onto `ChannelId::UNASSIGNED`.
- A control binding's `ParamAddr` still carries an `EffectTarget::Channel(u8)`
  scope, which the engine needs. Store the binding as `(ChannelId, ParamAddr
  with the scope ignored)` and resolve the scope when the map is compiled for
  the engine. Don't change `ParamAddr` itself: it is on the realtime path and
  in every lane.
- Delete the arms of `rescope_after` that only existed for these four:
  `rescope_subscription`, the envelope-gate half of
  `ModRack::rescope_channels`, and `ControlMap::rescope_channels`. What
  replaces them is resolution: an id whose channel is gone resolves to
  nothing, which is what the old sentinel-parking tried to do by hand.
- The engine still receives indices. Resolve at the places the session
  builds `SetModRoute`, Aux In subscriptions, and the compiled control map.

## Test

Before the change, write the test that moves a channel above a gate source
and asserts the gate still listens to the same channel, and run it green on
the old tree: renumbering already handles that case. Then the new test the
old code *cannot* pass: a control binding on channel 5, with channels 2 and 3
deleted in one undo step and then restored by redo, still targets the same
channel. If that one also passes on the old tree, say so in the status file
and drop it rather than keeping a test that proves nothing.

`scripts/dupe-audit` afterwards: no new `rescope` twin.
