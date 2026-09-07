# Containers plan status

## Step 01 — a device is an identity, not a position

Landed on `feat/containers` (2026-09-06). A rack device now carries a
`DeviceId` minted when it is inserted, and every persisted address names that
id instead of a slot number. **Reordering, inserting and deleting rack rows
rewrite no address anywhere.**

In plain terms: dragging a device around the rack used to be an edit to the
song's saved wiring, because a route said "the device in slot 3" and slot 3
kept changing. It now says "device 3", and the position is worked out from the
rack when it is needed. The saved file does not change when you drag.

Where it is:

- `DeviceId` and `EffectSlotState.id` — `crates/mooloop-core/src/effect.rs`
- `ParamOwner::Effect { device }` — `crates/mooloop-core/src/modulation.rs`
- the mint, the derivation and the load-time fill —
  `mint_device_id`, `device_slot`, `slot_of`, `assign_device_ids` in
  `crates/mooloop-core/src/structure.rs`
- the engine's half — `EffectSlot::device` and `EffectChain::slot_of` in
  `crates/mooloop-engine/src/render.rs`

### What `SlotRemap` had left, which the brief asked to be named

**Nothing, and it is gone.** The plan said the answer was a finding rather
than a decision to take in advance; the finding is that the type had no
responsibility that survived. Everything it did was rewrite an address after a
chain edit, and no address is a position any more.

What went with it: `SlotRemap` itself, `SlotRemap::address`,
`structure::retarget_lanes`, `ModRack::retarget_effect_slots`,
`PatternChannel::retarget_lanes`, `Sequencer::retarget_lanes`,
`RenderState::retarget_effect_slots`, and `Session::retarget_effect_slots`
with all five of the in-memory fixups it ran.

What replaced it is one narrower verb, spelled the same way at each layer —
`forget_device` — called on removal and on nothing else, because a removal is
the only chain edit that can still make an address stop meaning something.

`ChannelEdit` stays exactly as it was. A *channel* is still addressed by
position, and a route scoped to channel 4 still has to become channel 3 when
channel 1 is deleted. That is a separate problem and this step did not touch
it. `structure.rs`'s module doc now says so, in place of the argument it used
to carry against durable device ids.

### Three things the doing turned up

- **The two places that authored a slot number rather than deriving one were
  exactly where the brief predicted.** `Session::pending_preset_save` and
  `Session::effect_preset_names` both held positions across an edit and both
  had to be remapped; they are now keyed by `DeviceId` and are not remapped at
  all. `Session::automation_target` was a third, but it derives.

- **A preset had to be stripped of identity on the way out, and refused it on
  the way in.** `take_preset_save` clears the id when it lifts a row off the
  chain, and `load_effect_preset` writes `preset.with_id(existing)` rather than
  `*effect = *preset`. Without the second of those, loading a preset over a row
  would have silently deleted that row's identity and orphaned every route
  pointing at it — a preset changes what a device sounds like, not which device
  it is.

- **`DeviceId` had to be `u32`, which is what forces the wire-format change**
  rather than the nesting the brief expected to force it. `ParamOwner::Effect`
  carried a `u8`, and identity has to be monotonic and never reused, so a
  256-slot chain edited enough times would exhaust the space and hand a
  departed device's routes to a newcomer. `#[serde(alias = "slot")]` is what
  makes the widening free: in a project written before this step, a device's
  position *was* its identity, so both keys name the same device.

### What it cost

Measured rather than assumed, as `docs/plans/archive/modulator-capacity/`
requires of anything that widens a stored type.

`ParamAddr` went from 12 bytes to 16 and `ModRoute` from 28 to 32, because
`ParamOwner::Effect` now holds a `u32` where it held a `u8`. That is 64 bytes
per channel's modulation rack, 16 KiB across the reserved 256 channels — the
same order and the same argument as the last time an address grew, and it is
storage rather than ring traffic.

**The command ring did not move.** `size_of::<EngineCommand>()` is still 136,
still floored by the poly synth's parameter block, and
`capacity_no_longer_moves_the_command_ring` now pins the new numbers with the
reasoning beside them.

### Acceptance

`a_reorder_does_not_change_one_byte_of_the_saved_addresses`
(`crates/mooloop-session/tests/structure.rs`) is the case the plan asked for:
it snapshots the saved routes and lanes of a project, performs a reorder, an
insert and a second reorder, and asserts the saved addresses are identical.
That assertion could not be written under the slot scheme by construction —
there, every one of those edits had to change them.

Beside it, `reordering_effects_leaves_every_address_untouched` and the engine's
`a_route_and_a_lane_follow_their_device_through_a_reorder_and_die_with_it`
both changed from "the permutation ran correctly" to "there was nothing to
permute", which is the difference this step is for.

## Step 02 — the container is a device

Landed on `feat/containers` (2026-09-07). `EffectKind::Chain` is the
thirteenth effect kind. It takes a rack row, an identity, a bypass, a preset
slot and a place in the saved project, and it is **audibly nothing**: a chain
with containers in it renders bit-identically to the same chain with the
containers removed.

In plain terms: the box exists and you can put devices in it. It does not do
anything to the sound yet — that is step 03 — and the point of building it
silent first is that step 03's "at 100% wet it sounds the same" test needs
something to sound the same *as*.

### The representation, and why

A container does not hold its children. `ChainParams.children`
(`crates/mooloop-core/src/effect.rs`) says how many of the rows *after* it are
inside it, and those rows live in the same `Vec<EffectSlotState>`. The tree is
derived from the flat list, the way a device's position is derived from its
identity.

That keeps `EffectParams`, `EffectSlotState` and `EngineCommand` `Copy`, keeps
the engine's `[Option<Box<dyn AudioNode>>; 256]` flat, and — the part that
matters — makes "the rack cannot tell the difference between a container and
a leaf device" structurally true rather than an invariant to be maintained.

`chain_latency` needed no change at all: the children are rows of the same
chain, so the sum already counts them, and `EffectKind::Chain::latency_frames`
is `0` rather than a sum that would count them twice.

### Where it is

- `ChainParams`, `EffectKind::Chain`, `CHAIN_PARAM_MIX` —
  `crates/mooloop-core/src/effect.rs`
- the span primitives and the two invariants — `span_of`, `run_of`,
  `depth_at`, `parent_of`, `span_problem`, `wrap_in_container`,
  `unwrap_container` in `crates/mooloop-core/src/structure.rs`
- the transparent slot occupant — `crates/mooloop-dsp/src/effects/container.rs`
- the transparent *slot* — `EffectChain::is_container` and its branch in
  `crates/mooloop-engine/src/render.rs`
- the face — `crates/mooloop-ui/ui/container-device.slint`

### One thing the doing turned up, and it is load bearing

**A transparent node in a slot is not a transparent slot.** The first version
gave the container an ordinary rack row and let the chain host blend it like
any other device. It failed the null test immediately: the per-slot wet/dry is
an *equal-power* crossfade, so even at unity a transparent node leaks a
`cos(pi/2)` of the dry alongside itself — about 6e-8, inaudible, and not
bit-exact. The buffer device's Follow mode had already recorded exactly this,
and the note was read as a quirk of that device rather than as a fact about
the blend.

So a container slot now skips the leaf blend entirely, which is also the
correct design rather than a workaround: its `mix` is a blend across a *span*,
not across a device, and step 03 is where the chain host learns to apply one.

### Acceptance

`a_container_that_is_not_doing_anything_is_not_doing_anything`
(`crates/mooloop-engine/src/container_tests.rs`) renders a Filter → Drive →
Delay chain, wraps parts of it in containers at three mixes and two depths,
and asserts the master is sample-identical at three block sizes. Drive is in
the chain deliberately: it is the only kind that declares a latency, so a
container that disturbed the compensation plan would show as a shift rather
than as a difference in timbre.

Beside it: a bypassed container does not move the chain in time, a nesting
survives a save and reload and still renders the same, and a `chain` params
table with neither field set decodes to the defaults.

### Not in this step

The rack draws the container's face but not a *frame* around its run, and
there is no gesture to make one — `Session::wrap_effects_in_container` and
`unwrap_container_at` exist and are tested, but nothing in the interface calls
them yet. `EffectSlotRow` already carries `children` and `depth` so that the
face contract crosses `main.slint` once rather than twice. That is step 04.

## Steps 03–06

Not started.
