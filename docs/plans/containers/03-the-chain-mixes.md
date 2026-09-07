# 03 — The chain mixes

The audible step, and the one that closes the wet-FX gap. A container keeps a
copy of what came into it, runs its devices, and crossfades the two on the way
out — delaying the copy by the run's latency so the blend does not comb.

Every part of that already exists one level down. `EffectChain` (`render.rs`)
holds a preallocated `dry: StereoBus`, a per-slot `dry_align: IntegerDelay`
sized from `dry_path_latency_frames`, and a crossfade, and it has held them
since the gain-structure and latency-compensation work. This step generalises
the mechanism from one slot to a span. It does not invent one.

## Where the state lives

A container is **not** an `AudioNode`. It has no `process`; there is nothing
for it to process, because its children are rows of the same chain and the
chain's own loop already runs them in order. Making it a node would mean
giving it a child chain to own, which is the `Vec` that step 02 refused.

What it needs instead is per-container storage, and that hangs off the slot:

```rust
struct ContainerRun {
    /// Input to the run, kept for the crossfade at its end.
    dry: StereoBus,
    /// Delays `dry` by the run's total latency.
    align: Option<Box<IntegerDelay>>,
    /// Last row of the run, resolved on each structural change.
    end: usize,
}
```

Boxed and hung on the existing `Option<Box<EffectSlot>>`, allocated on the
control thread and installed through `StructuralCommand`, reclaimed rather
than dropped — the pattern every heap-owning piece of chain state already
follows, and for the reason `EffectSlot`'s own comment gives: an
addressable-but-empty slot should cost a pointer. A chain with no containers
pays nothing. A `StereoBus` at `MAX_BLOCK_SIZE` is not free, and there are 272
chains; preallocating four per chain against a cap nobody reaches is the
version of this that should not be built.

## The realtime pass

The chain's loop gains a stack of open runs, fixed at the depth cap, no
allocation:

- Entering a container row: copy the bus into that container's `dry`, push it.
- Leaving row *n*: pop every run whose `end` is *n*, innermost first, and for
  each, delay its `dry` by the run's latency and crossfade at `mix`.

Nothing else about the loop changes. Leaf slots keep their own wet/dry, their
own trims, their own analyzer and their own silence counter, and a container
is transparent to all of it.

**Bypass.** Bypassing a container skips its whole run and delays the signal by
the run's total latency, so the chain's declared latency is unchanged and the
channel does not move in time. This is not a new rule: `mixer.rs:273` says a
bypassed slot counts, and gives the argument — a bypass that shortened the
chain would make A/B-ing an effect also A/B the timing. The compensation work
made per-device bypass time-transparent; a container must not be the one place
that regresses it.

**Latency.** The run's total is the sum of its children's declared
`latency_frames`, computed on the control thread when the run is installed —
the same number the chain-level plan already sums, taken over a sub-range.
`dsp/effects/mod.rs:302` holds a device's declaration against its node at two
sample rates; extend it so a container's declared run latency is held against
what its dry path actually delays by. The comment on that test explains why:
two numbers for one fact is how they come to disagree, and a disagreement here
is silent.

## Done when

- A container at 100% wet is bit-identical to its devices without the
  container, in a live render and an offline one, at every block size.
- A container at 0% wet is bit-identical to the container bypassed, and both
  are bit-identical to the chain with the run's latency compensated and
  nothing else — the dry path is delayed, not undelayed.
- A container holding a latency-producing device (Drive is the only one, which
  is why `transparent_drive` exists in `render.rs:5450`) blends without
  combing, and the null against a hand-delayed dry path is exact rather than
  close.

**Acceptance case, and it is the one the brief names.** A container holding
Reverb at 50% mix, against the same Reverb in a bare slot at 50% wet/dry:
identical. Then a container holding Drive → Reverb at 50% mix, which has no
equivalent today at all — that is the wet-FX gap, and hearing it is the point
of the step. Render both offline and at three block sizes; assert the offline
render is sample-identical to the live one, which is the bar
`latency-compensation/` set and the reason it can be met here.
