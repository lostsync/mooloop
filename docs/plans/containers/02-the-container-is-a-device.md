# 02 — The container is a device

`EffectKind::Chain` joins the twelve kinds already there. It takes a rack row,
it takes an id, it saves, it loads, it bypasses, it takes a preset. At the end
of this step it is audibly nothing: it processes its run in order and passes
the result on, which is exactly what happens today without it. Step 03 gives
it a mix.

Building it silent first is deliberate. A container that changes the sound and
changes the rack's structure in the same step gives a null test nothing to
stand on.

## The representation

A container declares how many of the rows after it belong to it.

```rust
pub struct ChainParams {
    /// How many of the rows following this one are inside it.
    pub children: u8,
    /// Dry/wet across the whole run. Inert until step 03.
    pub mix: f32,
}
```

The children are the next `children` entries of the same
`Vec<EffectSlotState>`. There is one flat list of devices; some of them say
how far they reach. The tree is derived, the way rack position is now derived
from device identity.

**Why not `Chain(Vec<EffectSlotState>)`.** `EffectParams`, `EffectSlotState`
and `EngineCommand` all derive `Copy`, and the last one is load bearing: the
command ring is preallocated POD, and `bridge.rs:27` records that it was
narrowed precisely to stop a whole rack travelling on it. A `Vec` inside
`EffectParams` breaks all three at once and buys nothing the span does not
already give.

What the span gives, beyond keeping `Copy`:

- The engine's `[Option<Box<dyn AudioNode + Send>>; 256]` stays flat and its
  inner loop stays sequential. A container is not a node at all — see step 03.
- `chain_latency` (`mixer.rs:273`) stays correct without being edited. The
  children are rows of the same chain, so the sum already includes them.
- The brief's central rule — "the rack cannot tell the difference between a
  container and a leaf device" — becomes structurally true rather than an
  invariant somebody has to maintain.

`EffectKind::Chain::latency_frames()` is therefore `0`, and question 4 in this
plan's `README.md` explains why that is the honest number rather than a
shortcut.

## The invariants

Two, and they are the whole correctness argument for the representation:

1. **Spans nest.** For any two containers, one run contains the other or they
   are disjoint. Straddling is unrepresentable in a well-formed chain and must
   be unreachable through any edit.
2. **A span ends inside the chain.** `slot + 1 + children <= effects.len()`.

Enforced in one place — `structure.rs`, beside the edits that could break
them — with `integrity.rs` reporting a malformed chain the way it reports an
address that names nothing. A hand-edited project is a real input; the
existing integrity pass is where it gets caught, not a `debug_assert`.

Depth is capped. `CAPACITY_POLICY.md` gets an entry: containers nest four
deep, chosen because step 03 preallocates one dry buffer per open span and the
price is per-container, not per-slot. Four is a limit on the gesture, not on
the format — a deeper chain loads and is reported by `integrity.rs`, the same
way an over-long chain is.

## What structural edits have to learn

`move_effect`, `insert_effect` and `remove_effect` currently move one row.
They gain a notion of a run:

- **Insert inside a container** grows the enclosing container's `children`,
  and every container enclosing *that* one, out to the chain.
- **Remove a container** removes its run. A box is deleted with its contents;
  that is what "bypasses as a unit, saves as one preset" implies about
  deletion too. Emptying the box first is the user's business, and step 04
  gives them an unwrap gesture so it is one click rather than N drags.
- **Remove a leaf inside a container** shrinks every container enclosing it.
- **Move** moves the whole run and re-parents it, adjusting the `children` of
  every container it leaves and every container it enters. A container cannot
  be moved inside itself; that refusal belongs next to `would_create_cycle`
  in spirit, though it is arithmetic rather than a graph walk.

Every one of these is a `Vec` splice plus arithmetic on `children`. None of
them touches an address, because step 01 already made addresses immune.

## Persistence

Additive, under `PROJECT_FORMAT.md`'s defaulted-field rule. A `chain` variant
joins `EffectParams`' existing `#[serde(tag = "type", content = "state")]`
envelope, which is the same shape `ChannelSource` uses and the reason new
kinds have always joined without a version bump. A reader that predates this
step fails the load with "unknown variant", which is the correct and existing
behaviour for a project using a device the reader does not have.

`contains = ["effect_params"]` on a leaf preset is unchanged, so
`docs/plans/preset-system/04`'s condition — that going specific first left
room for a container entry rather than a redefinition — still holds. Step 05
writes the new entry.

## Done when

- A chain can be built, saved, reloaded and rendered with containers nested up
  to the cap, and the render is sample-identical to the same devices with the
  containers removed.
- Every structural edit preserves both invariants, over the edits themselves
  rather than over a hand-written table of cases.
- `integrity.rs` reports a straddling or over-running span in the same voice
  it reports a route that names nothing.

**Acceptance case.** Take a channel that renders a bar of audio through
Filter → Drive → Delay. Wrap the Drive in a container, save, reload, render.
The result is bit-identical to the render before the wrap. Wrap the container
in another container and it is still bit-identical. A container that is not
yet doing anything must not be doing anything.
