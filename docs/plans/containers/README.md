# Device containers

Written 2026-09-06 from `reference/CONTAINERS.md`, after checking that brief
against the source. The brief asked four questions before any step could be
written; this file answers them, and records the three places the brief and
the source disagree. `01`–`06` are the steps.

## Where the brief is wrong

The brief says so itself: *"Where this brief and the source disagree, the
source is right and this document is wrong. Say so plainly."* Three things.

**1. Effects already have a wet/dry control, and it is already latency
compensated.** The brief opens with "there is no wet/dry control on an effect,
because a dry path needs something with an inside to route around". Every rack
row has had one since the gain-structure work: `EffectSlotState.wet_dry`
(`effect.rs:2273`) is persisted, `EngineCommand::SetEffectWetDry`
(`bridge.rs:230`) carries it, `EffectChain.dry` (`render.rs:411`) is the
preallocated dry copy, `EffectChain.dry_align` (`render.rs:402`) is the
per-slot `IntegerDelay` that keeps the blend from combing, and
`main.slint:3159` draws the knob. `EffectSlotState::of_kind` even picks
per-kind defaults — 0.25 for the reverbs, 0.5 for Modulation.

What is missing is a wet/dry across *a run of several devices*. That is a real
gap and a container is the right fix for it, but it is a narrower gap than the
brief claims, and the machinery is not missing — it is present one level down.
This is good news for the price: step 03 generalises an existing mechanism
from one slot to a span, rather than inventing one.

**2. There is no "device face currently open" to lose.** The brief predicts
that "the device face currently open is probably identified by index. It will
follow the position rather than the device once inserts above it become
possible." The rack has no open/closed face: `effect-slots` is a flat model of
`EffectSlotRow` (`main.slint:144`) and *every* row draws its full face inline,
side by side, at its own rack-unit width. There is nothing that holds one row
as "the open one". What does hold a row by index is smaller and is listed in
question 1 below.

**3. A container's latency cannot be declared by its kind.** The brief asks
whether "a container's latency can be declared without the container being
built". It can, but not through the interface that exists: `latency_frames` is
a method on `EffectKind` (`effect.rs:52`), a function of the kind *alone*,
and a container's latency is a function of its contents. See question 4.

## The four questions

### 1. What breaks today if a rack row's index changes?

Eleven places name an effect slot by position. Two *author* the number; the
rest derive it.

Persisted, and therefore the expensive ones:

| Where | What it holds |
| --- | --- |
| `ModRoute.destination` (`modulation.rs:52`) | `ParamAddr.owner = ParamOwner::Effect { slot }` |
| `AutomationLane.target` (`automation.rs`) | the same `ParamAddr` |

In memory, rewritten by `SlotRemap` on every structural edit:

| Where | What it holds |
| --- | --- |
| `Session::automation_target` (`session.rs:53`) | the lane the piano roll is showing |
| `Session::pending_preset_save` (`session.rs:45`) | `PresetSaveTarget::Effect { target, slot }` — a save dialog in flight |
| `Session::effect_preset_names` (`session.rs:149`) | `HashMap<(EffectTarget, u8), String>` — the label a row wears |

Derived per call and therefore harmless:

`Session::channel_modulation_destination` (`session.rs:858`),
`Session::descriptor_for` (`session.rs:630`), `integrity.rs:1241` and `:1291`
(validation), `render.rs:660` (the engine builds `ParamAddr::effect(scope,
slot, id)` fresh from the position it is standing on), `render.rs:2331`
(`restore_base_param` goes the other way, address → position), and
`ui/lib.rs:2542` (encoding an owner for one Slint row).

**The two that author rather than derive** are `pending_preset_save` and
`effect_preset_names`. The brief predicted "one or two places", by analogy
with the Math module's `input_slot`, and predicted where: "most likely in the
pending-preset-save target and anywhere the UI holds a row by index across an
edit." Both correct. `session.rs:725` remaps all five in-memory holders in one
pass, and its comment on the save dialog names the bug it exists to prevent.

`SlotRemap` (`structure.rs:35`) is the mechanism, and its module doc argues
explicitly *against* durable ids: *"The alternative — durable ids on effect
slots, resolved to positions on every read — was what modulator sources
needed, because a route names a source from elsewhere. Nothing outside a chain
names an effect slot except through `ParamAddr`, and `ParamAddr` travels
through here."* That argument is sound today and stops being sound the moment
a position is a path. Step 01 reverses it, and rewrites that comment rather
than leaving it to contradict the code.

### 2. Can `ParamOwner` express a nested device without a wire-format break?

Yes, and by the same trick `SavedModulatorSlot.id` used, which is why it
works: **in a project written before durable identity, a device's position is
its identity.** So `slot = 3` in an old file and `device = 3` in a new one
name the same device, and one `#[serde(alias = "slot")]` on a renamed field
reads both.

That makes the id `u32` rather than `u8`, which matters: identity has to be
monotonic and never reused (`ModRack.next_source_id` is `u32` for this
reason), and `u8` would exhaust after 256 mints on a 256-slot chain.

So the brief's assumption — that the id change has to come first — holds, but
not for the reason it gives. It is not that `ParamOwner` *cannot* express
nesting; a path would fit in the enum. It is that a path would have to be
rewritten on every edit at every level, which is `SlotRemap` again and worse.
An id is not rewritten at all.

### 3. Does the effect enum have room for a variant that holds children?

Not as written, and the obstacle is one word: `Copy`.

`EffectParams` (`effect.rs:1860`), `EffectSlotState` (`effect.rs:2267`) and
`EngineCommand` (`bridge.rs:38`) all derive `Copy`, and the last one is load
bearing — the command ring is preallocated and its entries are POD by
construction, with a comment (`bridge.rs:27`) recording that the ring was
narrowed precisely to stop whole-rack values travelling on it. A
`Chain(Vec<EffectSlotState>)` variant breaks all three at once.

The answer is not to break `Copy`. It is that **a container does not have to
hold its children to contain them.** A container is a row in the flat chain
that declares how many of the following rows it encloses:

```rust
EffectParams::Chain(ChainParams { children: u8, mix: f32, .. })
```

The children are the next `children` rows of the same `Vec<EffectSlotState>`.
The tree is derived from the flat list, exactly as rack position will be
derived from device identity. This keeps `Copy` on all three types, keeps the
engine's `[Option<Box<dyn AudioNode>>; 256]` flat and its inner loop
sequential, keeps `chain_latency` correct without being touched, and — the
part that matters for the brief's central rule — makes "the rack cannot tell
the difference between a container and a leaf device" structurally true rather
than a thing to be maintained. There is one list of devices; some of them
happen to say how far they reach.

Step 02 states the invariants this representation has to hold (spans nest,
never straddle) and where they are enforced.

### 4. Is a container's latency declarable pre-build?

The contract exists and a container does not fit through it.

`chain_latency` (`mixer.rs:273`) sums `effect.kind().latency_frames()` over
the chain on the control thread, before any node is built, and
`every_effect_kinds_declared_latency_matches_its_node`
(`dsp/effects/mod.rs:302`) holds the declaration and the running node together
at two sample rates — the test's own comment says a device whose latency
depended on the rate "could not be declared statically at all and this is
where that would be found out."

`latency_frames` takes `self: EffectKind`. A container's latency is a function
of its contents, so the declaration has to move from the kind to the slot:
`EffectSlotState::latency_frames()`, which is `self.kind().latency_frames()`
for every leaf and `0` for a container. Zero, not the sum — because with the
span representation the children are already rows of the same chain and
`chain_latency` already counts them. The number the mixer needs comes out
right without the sum being written anywhere.

The dry path is the part that does need the sum, and it is internal to the
chain: a container's dry copy must be delayed by the total latency of its own
run before it is crossfaded back in, which is the per-slot `dry_align`
generalised from one device to a span. Step 03 does that, and extends the
`dsp` agreement test to cover it.

## The steps

| Step | What lands |
| --- | --- |
| `01` | A device is an identity, not a position. `SlotRemap` loses its address job. |
| `02` | `EffectKind::Chain` exists, nests, saves, and loads. Silent — it processes its run and nothing else. |
| `03` | The chain mixes. Dry path, span latency, bypass as a unit. **This is the audible one.** |
| `04` | The rack draws the box, and devices can be dragged into and out of it. |
| `05` | A container saves as one preset, with the modulation inside it. |
| `06` | Layer and selector: the decision, priced, not built. |

Steps 01–03 are the plan. 04 makes it usable, 05 makes it worth having, and
06 is a page that says no on the record.
