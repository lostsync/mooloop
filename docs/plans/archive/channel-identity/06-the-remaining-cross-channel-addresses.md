# 06 — The other three fields that name another channel

Split out of [02](02-cross-channel-addresses.md) on 2026-09-17, when the
selection went over and the other three did not. Each was waiting on something
different, and none of them was waiting on the same thing, which is why they
are one file rather than three steps.

**All three landed on 2026-09-18**, as
[MOO-31](https://linear.app/mooloop/issue/MOO-31),
[MOO-32](https://linear.app/mooloop/issue/MOO-32) and
[MOO-33](https://linear.app/mooloop/issue/MOO-33). That closes the
`channel-identity` sequence: nothing in a song names a channel by its seat any
more. This file stays the source of truth; the issues mirror it.

They landed in two shapes, and the split is the useful thing to remember:

| | Where the identity went | Why |
| --- | --- | --- |
| Control binding | **Replaced** the seat | Nothing downstream wanted a seat |
| Aux In source | **Beside** the seat | The seat is an addressable parameter |
| Envelope gate | **Beside** the seat | The seat is read on the audio thread |

## Control bindings — the seat was replaced

`ControlBinding.target` was `ControlTarget::Param(ParamAddr)`, and a
`ParamAddr` carries `scope: EffectTarget::Channel(u8)`.

Nothing about this is realtime. The control map never reaches the engine;
`ControlMapState` resolves ports on the control thread and
`session/src/midi.rs` dispatches a binding by reading its address in three
places. So the honest decomposition was available: **stop storing a
`ParamAddr` and store what it is made of.**

```rust
pub enum ChainKey { Channel(ChannelId), Bus(u8) }       // core/src/mixer.rs
pub struct ParamKey { scope: ChainKey, owner: ParamOwner, param: u32 }
```

`ParamKey::resolve` takes the caller's `ChainKey -> EffectTarget` map and is
the only way to get an addressable `ParamAddr` out. The two types deliberately
do not convert silently, which is the answer to `AGENTS.md`'s "one value, two
meanings" -- there is no pair to keep in step because there is no pair.

Three things came out of building it that the plan had not predicted:

- **The saved form did not change one byte.** `ChannelId` is `#[serde(transparent)]`
  over its `u32` and `EffectTarget::Channel(3)` was already written as
  `{ channel = 3 }`, so `ChainKey::Channel(ChannelId(3))` writes the same TOML.
  An older file's index 3 therefore decodes as `ChannelId(3)`, which is the
  same reading `assign_channel_ids` gives that file's channels. Step 02
  promised the format would not move and worried this would break the promise;
  it did not, and no defaulted decode of an old shape was needed.
- **`ChainKey` was already written, in `mooloop-session`.** Step 03 had added
  `Channel(ChannelId) | Bus(u8)` for the session's own keys. A second copy in
  `mooloop-core` for the control map would have been the duplication
  `AGENTS.md` opens with, so the type moved down into `core` beside
  `EffectTarget` and the session re-exports it.
- **A binding onto a deleted channel is no longer dropped.** `ControlMap::rescope_channels`
  is gone -- there is nothing for a channel edit to renumber -- and with it the
  `retain` that threw such a binding away. It stops *resolving* instead: the
  mapping list draws "Unavailable parameter", exactly as it already did for a
  binding onto a deleted device, and it works again if the channel comes back.
  `rescope_tracks` remains, because a track is still a seat.

`Project::rescope_after` no longer touches the control map at all.

## Aux In's source — the parameter stayed a position

`AuxInParams.source_channel: i16` is not only a cross-channel address. It is an
**addressable parameter**: `PARAM_SOURCE_CHANNEL`, with a `ParamDescriptor`
whose range is `NO_SOURCE ..= MAX_CHANNELS - 1` and whose curve is
`Stepped(MAX_CHANNELS + 1)`. The UI knob reads it, a lane can automate it, and
a control binding can be learned onto it.

A `ChannelId` does not fit that value space. Id 47 is ordinary after enough
edits and is outside both the range and the step count, so making the field an
id would have left the descriptor describing something the field no longer was.

**Decided 2026-09-17 (Adam): the parameter stays a position.** The durable
identity lives beside it as the authoritative field, and the position is
derived from it. Two fields, each with one meaning, rather than one field with
two. `AuxInParams.source_id` is that field.

The write sites divide cleanly, and the rule at each is written where it is:

- The **source picker** knows both and sets both.
- A **parameter write** (`aux_in::set`, which a lane or a bound fader reaches)
  knows only a seat, so it clears the identity rather than leaving a stale one
  to contradict the seat just asked for. The next identify pass adopts whoever
  is sitting there.
- **`set_subscription`** does the same, for the same reason: it has no project
  to ask.

## The envelope gate — the same shape, a different reason

`EnvelopeParams.input_channel: u8` is read **on the audio thread**, as an index
into the gate array (`mooloop-dsp/src/modulator.rs`, in `process`). `ModRack`
is `Copy` and ships to the engine verbatim, so the field the document holds is
the field the DSP reads.

This file recorded the gate as **blocked** on one of two things: an
id-to-strip resolution inside the engine, or the session rewriting the field to
an index on the way in -- and it ruled the second out, as "the Buffer face bug
again", one integer holding different things on either side of a boundary.

**The block dissolved rather than being cleared**, and it was Adam's own
2026-09-17 decision on Aux In that dissolved it. Two fields, each with one
meaning, is not the rejected option: the document holds both, they agree by
construction, and the DSP reads the derived one. Nothing in the engine has to
learn what a `ChannelId` is, which is what step 05 had already found it did not
need to. So `input_channel_id` sits beside `input_channel` exactly as
`source_id` sits beside `source_channel`, and the gate did not wait for
`incremental-structure` step 02 after all.

Its sentinel needed the decode rule this file called for. Parked is
`u8::MAX` = 255, and 255 is an ordinary `ChannelId` -- so unlike the selection,
the old number cannot simply be reinterpreted. `ModRack::identify_gates`
refuses it explicitly, and a test pins that.

## The one pass both derived fields share

`Project::identify_channel_references` (seat → identity, for anything that
does not have one) and `Project::reseat_channel_references` (identity → seat,
for everything that does). Two halves rather than one function, because they
read and write opposite fields, and running them in that order after a
structural edit is correct whichever half a given reference needed.

`assign_channel_ids` **ends with the identify half**, rather than the caller
running it: a load that handed out channel ids and forgot to identify the
references would leave them positional and silently so, which is the exact
failure this plan exists to remove. `rescope_after` runs both, after the
positional walks it already ran -- those walks stay, because they are what
keeps a project built in code rather than loaded from a file correct, and they
are a no-op for an identified reference whose seat the reseat overwrites
anyway.
