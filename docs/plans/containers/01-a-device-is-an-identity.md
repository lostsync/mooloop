# 01 — A device is an identity, not a position

The prerequisite. Nothing about containers appears in this step; what appears
is the thing that has to be true before one can exist without every drag
becoming an addressing event.

Today a rack device *is* its position. `ParamOwner::Effect { slot }` is an
index into a flat `Vec`, and it is persisted into every modulation route and
every automation lane, so an edit that renumbers rows has to rewrite every
saved address that pointed past it. `SlotRemap` (`structure.rs:35`) is that
rewrite, and it works. It stops working when a position becomes a path.

This is the same change the repository already made for modulator sources, and
it is worth being explicit that this step *reverses a decision recorded in the
source*. `structure.rs:16` argues against durable device ids on the grounds
that "nothing outside a chain names an effect slot except through `ParamAddr`,
and `ParamAddr` travels through here." That was true and is about to stop
being true. Rewrite the comment; do not leave it standing against the code.

## The shape

```rust
/// Stable within one chain. Minted when the device is inserted, carried
/// through reorders, and never reused, so a route outlives a slot number.
pub struct DeviceId(pub u32);
```

- `EffectSlotState` gains `pub id: DeviceId`.
- `ChannelSetup` and `BusState` gain `next_device_id: u32`, the mint.
- `ParamOwner::Effect { slot: u8 }` becomes
  `ParamOwner::Effect { device: DeviceId }`.

## Why this is not a wire-format break

**In a project written before this step, a device's position is its
identity.** So `slot = 3` in an old file and `device = 3` in a new one name
the same device, and one `#[serde(alias = "slot")]` reads both.

That is `SavedModulatorSlot.id`'s trick exactly — its doc comment says legacy
rows "decode with the slot number as the id, which is exactly the id
`mod_metadata::local_slot_sources` was already handing out for them, so a
legacy route's `source_slot` maps onto it unchanged." The same sentence is
true here with `SlotRemap` in place of `local_slot_sources`.

Two consequences to get right:

- `EffectSlotState.id` persists as `#[serde(default)] id: Option<u32>`, absent
  in old files and in new ones only when it would be redundant. Filling it is
  positional, and `Vec<EffectSlotState>` deserializes without knowing its own
  index, so the fill belongs in a normalisation pass over `ChannelSetup` after
  decode — beside `pad_automation`, which exists for the same class of reason.
- `next_device_id` defaults to one past the highest id present, so a project
  that has never seen this code mints its first new device at `effects.len()`
  and cannot collide with anything a route already names.

The id is `u32` and monotonic. `u8` would fit `ParamOwner`'s current field
width and would be wrong: a 256-slot chain edited enough times exhausts it,
and a reused id hands a departed device's routes to a newcomer. `ModRack`
carries `next_source_id: u32` for this reason and the argument transfers
whole.

## The realtime path does not change

The engine keeps indexing by position, and resolves identity to position once
per structural change, exactly as `source_slot` is resolved now.

- `EffectSlot` (`render.rs:334`) gains `device: DeviceId`, arriving on
  `StructuralCommand::InstallEffect` beside the kind it already carries.
- `render.rs:660` builds `ParamAddr::effect(scope, slot as u8, descriptor.id)`
  fresh from the position it is standing on, once per parameter per block. It
  reads the slot's `device` field instead. A field read replaces a cast.
- `restore_base_param` (`render.rs:2329`) goes the other way, address to
  position, on route or lane removal. It gets `EffectChain::slot_of(DeviceId)`
  — a scan of the populated `bound`, on a path that already allocates nothing
  and runs once per removed route, never in a sample loop.
- `EngineCommand` variants keep carrying `slot: u8`. They are imperatives
  aimed at a position the sender has already resolved, not stored addresses,
  and the ring stays `Copy` and POD.

## What SlotRemap keeps

Almost nothing, and saying which is part of this step rather than a follow-up.

Gone: `SlotRemap::address`, `retarget_lanes`, `ModRack::retarget_effect_slots`,
`Session::retarget_effect_slots` and its five in-memory fixups
(`automation_target`, `pending_preset_save`, `effect_preset_names`, and the
route and lane passes). None of them has anything left to do — an id is not
renumbered by an edit, so there is nothing to re-point.

`effect_preset_names` is keyed `(EffectTarget, u8)` today. It rekeys to
`(EffectTarget, DeviceId)` and stops being fixed up.
`PresetSaveTarget::Effect { target, slot }` becomes `{ target, device }` and
loses the comment explaining why it has to follow a reorder, because it no
longer can fail to.

Kept: `ChannelEdit`, which is the *channel list*'s permutation and a separate
problem — a channel is still addressed by position, and this step does not
touch that.

Whether `SlotRemap` survives at all is a finding, not a decision to take in
advance: if `move_effect`/`insert_effect`/`remove_effect` end up returning
nothing anyone reads, the type goes and `structure.rs` keeps only the channel
half. Record the answer in `00-status.md` either way, because the brief asks
for it by name.

## Done when

- Reordering, inserting and deleting rack rows rewrite no address anywhere.
- A project saved before this step loads, renders, and re-saves with every
  route and lane pointing at the same device it pointed at before.
- `SlotRemap`'s remaining responsibilities are named in `00-status.md`.

**Acceptance case.** Build a channel with a route onto a filter's cutoff and
an automation lane onto a delay's feedback. Render offline. Insert a device
*above* both, drag the filter to the end of the chain, and render again with
the transport in the same place. The two renders differ only by what the new
device does — and the saved project's route and lane entries are
byte-identical across the edit, which is the thing that is not true today.

That last clause is the test worth writing, because it is the only one that
can tell the difference between "the remap ran correctly" and "there was
nothing to remap."
