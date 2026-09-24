# 07 — A branch is a run, and a layer is a device

Written 2026-09-21, against the tree as it stands after `gesture-undo/`
closed. This is the work order `06-layers-and-selectors.md` asks for, and the
first of four steps: **07** is the representation, **08** is the engine,
**09** is the drawing, **10** is the gestures and the preset.

## What 06 got wrong, and it is the reason this is affordable

`06` priced a layer at *"a second representation in `EffectSlotState`, because
the flat list stops being able to describe the structure"*, on the argument
that a span is a contiguous run and parallel branches are not contiguous in
any order.

**They are.** The flat list already describes a tree, and a tree already has
sibling subtrees — that is what a container's *direct children* are. Today
they run in series because the chain host runs every row in order; nothing in
the representation says they must. A layer is the same span with a different
rule about what to do with it:

```
Chain (children: 4)     Layer (children: 4)
  Drive                   Drive          ─┐ branch 1
  Delay                   Delay          ─┘   (Drive, then Delay)
  Chain (children: 1)     Chain (children: 1) ─┐ branch 2
    Reverb                  Reverb          ─┘   (Reverb)
```

Both are four rows under a head that says `children: 4`. The chain runs them
1→2→3→4. The layer splits its input across its **direct children** — the rows
whose `parent_of` is the layer itself — and sums what comes back. Walking
them is `run_of` from `span.start` until the span ends, which
`wrap_in_container` (`structure.rs:704-710`) already does to check a selection
is a whole number of runs. The primitive exists; nothing has needed to call it
for this reason yet.

So a branch is never a new kind of thing. A branch is a **direct child's
run**: a leaf direct child is a one-device branch, and a `Chain` direct child
is a multi-device branch, which is how a branch holding Drive → Delay is
expressible with no new syntax. Dropping a second device beside a leaf inside
a layer gives you two branches, not a two-device branch — and that is the
right default, because a layer whose rows do not stack is a layer nobody
asked for.

Three of `06`'s four prices stand, and they are steps 08 and 09:

- N branch buffers per *open layer depth* rather than per layer (08).
- Branch alignment to the longest branch (08), which is not `compile_latency`
  reapplied — see "Latency stops being a sum" below.
- A drawing that is not adjacency (09), which `04` closed off on purpose.

The one that does not stand is the representation, and it was the one that
made the other three look like a plan nobody would take.

## The one new rule, and where it is already written 33 times

`EffectParams::Chain(chain)` is how the whole workspace asks *"is this a
container, and how many rows does it hold?"* — 33 sites across seven files,
`span_of` (`structure.rs:163`) first among them. `EffectKind::Chain` appears
in ten more. Adding `EffectParams::Layer(LayerParams)` beside it means every
one of those sites is a place the two kinds can drift apart, and this
codebase's characteristic fault is a rule written twice.

**So the kind test does not get a second arm. It gets a predicate.** Before
`Layer` exists as a variant:

```rust
impl EffectParams {
    /// How many of the rows after this one are inside it, or `None` for a
    /// device that holds nothing.
    pub fn container_children(&self) -> Option<u8>;
    pub fn set_container_children(&mut self, children: u8);
    /// How the rows inside run: in order, or in parallel and summed.
    pub fn container_flow(&self) -> Option<ContainerFlow>;
}
```

`span_of`, `span_problem`, `resize_enclosing`, `insert_into_container`,
`can_insert_into_container`, `can_move_into_container`, `wrap_in_container`,
`move_effect_into_container` and `remove_effect` all read the predicate and
**none of them learns the word `Layer`**. That is the whole of the structural
change: a layer is a container, and containment is already correct.

`container_flow` is read in exactly two places — the engine's run loop (08)
and the rack's drawing (09) — and nowhere else. If a third site wants it,
that site is probably wrong.

> **Corrected 2026-09-21, by doing it: there is a third, and it is not
> wrong.** The latency walk below has to know how a container combines its
> contents — series adds, parallel takes the longest — and that is the same
> question, asked a third time. So `container_flow` has three readers: the
> latency walk, the run loop, and the drawing.
>
> It is also why `ContainerFlow` is **not** introduced by the predicate
> commit. With only `Chain` in the tree the enum would have one variant, and
> `Option<ContainerFlow>` with one variant is `is_container()` spelled a
> second way — the exact fault the commit exists to remove. Both variants
> arrive with `Layer`, in the commit where both are real and both are
> reachable by a test. Until then the walk is written so that
> `latency_of_run` is the one place the combine happens.

Do this as the first commit, on the tree as it is, with `Chain` still the
only container: the sweep is then a refactor with an existing suite over it,
and `scripts/dupe-audit` can be run either side of it. **A guard written
before its fix is shaped by the tree** (`AGENTS.md`), and here the guard is
the existing container suite, which must not move by one byte.

## Latency stops being a sum

`chain_latency` (`mixer.rs:883`) is:

```rust
effects.iter().map(|effect| effect.kind().latency_frames()).sum()
```

and `02` recorded with some satisfaction that containers needed no change to
it, because a container's children are rows of the same chain and the sum
already counts them.

**That is true exactly as long as every row is in series.** The frames a
signal actually spends inside a layer is the *longest* branch, not the total
of all of them. A layer holding a 15-frame Drive in one branch and a 15-frame
Drive in the other declares 30 under the current sum and costs 15, so the
channel is compensated 15 frames late against every other channel — which is
audible as the channel sitting behind the song, not as a defect in the layer.

So `chain_latency` and `run_latency` (`mixer.rs:902`) become a tree walk:
a run's latency is the sum of its direct children's, and a **layer's** is the
max. Two notes on doing it:

- Write it and its test **before** `Layer` exists. With only chains in the
  tree a tree walk and a flat sum agree on every arrangement, so the test is
  "the walk still equals the sum over every container shape the suite builds",
  and it passes on the unfixed tree. Then `Layer` arrives and exactly one arm
  differs. A latency change that lands in the same commit as a new device
  kind cannot be bisected against either.
- `EffectKind::Chain::latency_frames()` stays `0` and so does `Layer`'s. The
  head declares nothing; the walk reads its span.

`compile_latency` is not touched. It compensates *producers into a summing
point* across the mixer, and a layer's branches are not producers — the whole
point of doing this inside the chain is that a branch has no strip, no send
and no place in the bus graph. **Aligning branches is step 08's own delay, one
per branch, sized `max_branch - this_branch`.** Saying that out loud is worth
a paragraph because `06` said "which is `compile_latency` (`mixer.rs`) applied
inside a chain", and it is not: `compile_latency` compiles a tree of strips,
and there is no strip here to compile.

> **Two corrections from landing the third commit, 2026-09-22** (the whole
> account is in `00-status.md`). There is no `LayerParams`: `ChainParams`
> became `ContainerParams` and both kinds hold it, because two structs with
> the same two fields meaning the same things is a rule written twice. And
> the latency walk's `max` arm does **not** arrive with `Layer`: while a
> layer runs in series, a walk that took its longest branch would declare
> less latency than the renderer takes, so it lands in 08 with the render
> that makes it true.

## What this step builds

1. **The predicate sweep**, above. One commit, no behaviour change, the
   container suite green and byte-identical renders.
2. **The latency tree**, above. One commit, no behaviour change, with the
   test that says so.
3. **`EffectKind::Layer` and `LayerParams { children, mix }`** — the same two
   fields `ChainParams` has, for the same two reasons, and `mix` means the
   same thing: the blend of the whole run against the copy taken as the box
   opened. A fourteenth effect kind, and the second that is not an effect.
4. **Persistence and integrity.** `EffectParams` is a tagged enum, so `layer`
   is a new tag and every older project decodes unchanged. `integrity.rs`
   already repairs three container states it cannot produce
   (`EffectKind::Chain` appears there three times); it learns the same three
   for a layer by reading the predicate rather than by gaining a branch.
5. **`of_kind(EffectKind::Layer)`** defaults `mix` to 1.0, like a chain.
   A layer at full wet is its branches summed; at 0 it is its input.

## What it does not build

Nothing renders in parallel yet. A `Layer` installed by this step runs its
rows **in series**, exactly as a `Chain` does, because the engine has not
learned `container_flow`. That is deliberate and it is the same trick `02`
used: the box lands silent so that `08`'s "one branch is bit-identical to a
chain" has something to be identical *to*, and so that a project written by
this step and opened after `08` is the same project.

It is also the reason this step can land on `main` without a listening pass.

## Acceptance

- `a_layer_running_in_series_is_a_chain` — a Filter → Drive → Delay chain,
  wrapped in a `Layer` and in a `Chain` at the same mix, renders sample-
  identical at three block sizes. Drive is in it for the reason `02` gives:
  it is the only kind that declares a latency.
- `every_container_predicate_agrees_with_the_kind_it_replaced` — the sweep's
  guard, and it is the `dupe-audit` question in test form: *does anything read
  the copy the test checks?* Assert `container_children()` against
  `EffectParams::Chain(c) => c.children` for every kind, so a fifteenth kind
  that forgets the predicate fails here rather than silently reporting `None`.
- `the_latency_walk_equals_the_flat_sum_for_every_serial_arrangement` — over
  the container shapes `structure.rs`'s own tests already build.
- A layer survives save, reload and a reorder, and `span_problem` reports a
  straddling layer in the same words it reports a straddling chain.

## Sizes to record

`CAPACITY_POLICY.md` and the precedent in `archive/modulator-capacity/`: state
`size_of::<EffectParams>()`, `size_of::<EffectSlotState>()` and
`size_of::<EngineCommand>()` before and after. `LayerParams` is two fields
that `ChainParams` already has, so the expectation is that nothing moves and
the command ring's 136 bytes stay floored by the poly synth's parameter block
— but `capacity_no_longer_moves_the_command_ring` is the test that has to say
so, not this paragraph.
