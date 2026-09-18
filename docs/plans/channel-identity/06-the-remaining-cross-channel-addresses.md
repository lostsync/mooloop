# 06 — The other three fields that name another channel

Split out of [02](02-cross-channel-addresses.md) on 2026-09-17, when the
selection went over and the other three did not. Each is waiting on something
different, and none of them is waiting on the same thing, which is why they
are one file rather than three steps.

All three are still positions today, renumbered by `Project::rescope_after`
on every structural edit. That works -- the existing tests cover it -- and
the cost of leaving them is that a missed case is silent, not that anything
is broken right now.

**Tracked as three Linear issues since 2026-09-18**, because one "not started"
covering all three hid which of them anybody could pick up: control bindings
are [MOO-31](https://linear.app/mooloop/issue/MOO-31) and are ready, Aux In is
[MOO-32](https://linear.app/mooloop/issue/MOO-32) and is decided but unbuilt,
the gate is [MOO-33](https://linear.app/mooloop/issue/MOO-33) and is blocked.
This file stays the source of truth; the issues mirror it.

## Control bindings — unblocked, needs one decision

`ControlBinding.target` is `ControlTarget::Param(ParamAddr)`, and a
`ParamAddr` carries `scope: EffectTarget::Channel(u8)`.

**Nothing about this is realtime.** The control map never reaches the engine;
`ControlMapState` resolves ports on the control thread and
`session/src/midi.rs` dispatches a binding by reading its `ParamAddr` in three
places. So this one can be done whenever.

The decision it needs is how to store a scope that is an id **without leaving
a `ParamAddr` lying around whose own `scope` field is stale**. Storing
`(ChannelId, ParamAddr)` and agreeing to ignore the `ParamAddr`'s scope is
exactly the shape `AGENTS.md`'s "Parameter identity across the session
boundary" section was written about: one value, two meanings, agreeing until
they do not.

The honest decomposition is to stop storing a `ParamAddr` at all and store
what it is made of:

```rust
enum ControlScope { Channel(ChannelId), Bus(u8) }
ControlTarget::Param { scope: ControlScope, owner: ParamOwner, param: u32 }
```

with a `resolve(&Project) -> Option<ParamAddr>` that is the only way to get an
addressable one. That **does** change the saved form of a binding, which is
what step 02 promised it would not -- so it wants a defaulted decode of the
old shape, reading its `scope` channel as an id the way everything else here
reads an old index.

## The envelope gate — waiting on the engine's map

`EnvelopeParams.input_channel: u8` is read **on the audio thread**, as an
index into the gate array (`mooloop-dsp/src/modulator.rs`, in `process`).
`ModRack` is `Copy` and ships to the engine verbatim, so the field the
document holds is the field the DSP reads.

Converting it therefore needs one of:

- an id-to-strip resolution in the engine, so the engine can resolve the field
  itself; or
- the session rewriting the field to an index on the way in, which leaves the
  document's copy and the engine's copy holding different things in one
  integer. That is the Buffer face bug again and should not be done.

**05 landed and did not deliver this.** This file, and the status file, said
the gate half was waiting on step 05 because the plan's layer table promised
the engine "a map from `ChannelId` to its strip, used at install time". Step
05 then found it did not need one: the carry is decided on the *control*
thread by comparing two `Project` values, so the audio thread only swaps
boxes and the strips never learn their ids. `ChannelId` does not appear in
`mooloop-engine` at all except in prose. Corrected 2026-09-18, after the
claim "so all three are unblocked" had been written into three places.

So the gate half is still blocked, on the same thing it was always blocked
on -- and that is now
[`incremental-structure/` step 02](../incremental-structure/00-status.md),
which gives a `ChannelStrip` its own `ChannelId` because an incremental edit
needs the audio thread to know what it is holding. Whoever does that step
should expect this one to follow it.

Its sentinel needs a decode rule too. Parked is `u8::MAX` = 255, and 255 is
an ordinary `ChannelId` -- so unlike the selection, the old number cannot
simply be reinterpreted. An old file's 255 has to become
`ChannelId::UNASSIGNED` explicitly.

## Aux In's source — the parameter stays a position

`AuxInParams.source_channel: i16` is not only a cross-channel address. It is
an **addressable parameter**: `PARAM_SOURCE_CHANNEL`, with a `ParamDescriptor`
whose range is `NO_SOURCE ..= MAX_CHANNELS - 1` and whose curve is
`Stepped(MAX_CHANNELS + 1)`. The UI knob reads it, and a control binding can
be learned onto it.

A `ChannelId` does not fit that value space. Id 47 is ordinary after enough
edits and is outside both the range and the step count, so making the field an
id would leave the descriptor describing something the field no longer is.

**Decided 2026-09-17 (Adam): the parameter stays a position.** The durable
identity lives beside it as the authoritative field, and the position is
derived from it. Two fields, each with one meaning, rather than one field with
two.

That is a slightly bigger change than the other two -- it touches
`subscription()`, `set_subscription()`, the descriptor get/set pair, and the
UI's source picker -- and it should be done when the audio-edge work is next
open rather than on its own.
