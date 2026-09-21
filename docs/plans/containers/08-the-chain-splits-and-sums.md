# 08 — The chain splits and sums

The engine step. After `07` a layer is a container the renderer runs in
series; after this one it runs its branches in parallel and sums them.

This is the part `FOCUS.md` has parked since it was written as *"parallel
routing inside a chain"*, and it is the only genuinely new realtime machinery
in the four steps.

## Where it goes

`EffectChain` (`render.rs:952`) already has everything this needs except the
buffers:

| Already there | What it does |
| --- | --- |
| `nodes: [Option<Box<dyn AudioNode>>; 256]`, `bound` | the flat run, processed in order |
| `dry: StereoBus` | one scratch for the per-*slot* wet/dry blend |
| `container_dry: Option<Box<ContainerScratch>>` | one dry copy **per open nesting depth**, not per container |
| `EffectSlot::container_children`, `container_align` | the span, and the dry path's delay |
| `EffectChain::close_run` (`render.rs:1887`) | the crossfade when a run ends |
| `StructuralCommand::SetContainerSpan` | how the span reaches the audio thread |
| `UiState::publish_container_spans` | published after every structural edit |

The open-run stack that `03` built is the mechanism: the loop walks slots in
order and keeps a stack of the containers it is currently inside.

**A layer is one more thing on that stack, with two buffers instead of one.**

## The buffers

Per open *layer* depth, allocated the first time a layer is installed on this
chain and kept thereafter — the same growth rule `container_dry` states and
for the same reason (freeing is a deallocation reached from a structural
edit):

- **`branch_in`** — the layer's input, kept so each branch can start from it.
- **`branch_sum`** — where finished branches accumulate.

`container_dry`'s existing per-depth copy is still what the layer's own
`mix` blends against at the end, so a layer needs no third buffer. Two buses
× `MAX_CONTAINER_DEPTH` (4) × `MAX_BLOCK_SIZE` (8192) × 2 channels × 4 bytes
is **2 MiB per chain that has ever held a layer**, which is the number that
has to go in `00-status.md` and be weighed against `container_dry`'s 1 MiB.

If that is judged too much, the honest lever is `MAX_CONTAINER_DEPTH`, not a
smaller buffer: a block is a block. Say which was chosen and why.

## The run

At the layer's row, with span `S` from `SetContainerSpan`:

1. `branch_in.copy_from(bus)`, `container_dry[depth].copy_from(bus)`,
   `branch_sum.clear()`.
2. For each direct child `c` in `S` (walk `run_of` from `S.start`):
   - `bus.copy_from(branch_in)`
   - run `c`'s run as the chain already runs rows, per-slot blends and all
   - delay `bus` by `c`'s **alignment**, then `branch_sum.add_from(bus)`
3. `bus.copy_from(branch_sum)`, then the layer's `mix` against
   `container_dry[depth]`, delayed by the layer's declared latency exactly as
   `close_run` already does for a chain.

Three things to get right, each of which has already bitten this plan once:

- **The bypass branch must come before the generic one.** `03` shipped a
  bypassed container that delayed its own row and let its run play anyway,
  because the generic bypass arm ran first. A bypassed *layer* has the same
  failure with a worse symptom — every branch still sums. Write the test that
  fails on the unfixed order.
- **`is_at_rest` and the idle skip.** A branch that has gone quiet must not
  be skipped in a way that changes the sum's *count*; summing three branches
  where one is silent and summing two are the same number, so this is safe,
  but state it rather than assume it. The layer head's own `tail_frames` is
  `0` and `is_at_rest` is `true`, like `ContainerEffect`.
- **Unity, not equal power.** `02`'s load-bearing finding was that the
  per-slot wet/dry is an equal-power crossfade, so a transparent slot leaks
  `cos(pi/2)` of dry and the null is not bit-exact. A layer slot skips the
  leaf blend for the same reason. Branches sum at **unity** — two copies of
  the same signal are +6 dB and that is correct, because a layer is a layer.
  Anything else is a decision about gain that has to be Adam's, not a default
  chosen in an engine step.

## Alignment

Per branch, one `IntegerDelay` sized `max_branch_latency - this_branch`,
allocated on the control thread beside the node it belongs to — which is what
`dry_align` (`render.rs:969`) already is, per slot, for the leaf blend. Hang
the branch delay on the branch head's `EffectSlot`, next to `container_align`.

The numbers come from `07`'s latency tree: `run_latency` of each direct child,
and the layer's declared latency is their max. Nothing here recomputes a
latency on the audio thread.

A branch whose latency equals the max gets no delay at all (`None`), so the
common case — a layer of three branches that all declare zero — allocates
nothing and costs nothing. Drive is the only kind that declares a latency, so
that common case is most of them.

## Acceptance

The pair that matters, mirroring `03`'s:

- **`a_layer_of_one_branch_is_a_chain`** — bit-identical to `07`'s serial
  layer and to the equivalent `Chain`, at three block sizes. Without this,
  every other claim is a claim about nothing.
- **`two_identical_branches_are_exactly_six_dB`** — not "about 6 dB": the
  sum of a signal with itself is that signal doubled, sample for sample.

Beside them:

- **`a_branch_that_declares_latency_does_not_comb_against_one_that_does_not`**
  — Drive in branch 1, nothing in branch 2, the same source into both. Without
  alignment this is a 15-frame comb filter and it is the most audible thing
  this step can get wrong. Render it and assert against the aligned reference,
  not against a peak.
- **`a_bypassed_layer_is_its_input`** — and does not move the chain in time.
- **`a_layer_inside_a_chain_inside_a_layer`** — nesting composes to
  `MAX_CONTAINER_DEPTH`, and the buffers are indexed by depth so this is the
  case that catches an index-by-slot regression.
- **`a_layer_allocates_nothing_on_the_callback`** — using
  `CountingAllocator::allocations()` in `mooloop-engine`, the harness
  `archive/control-plane-seams/04` built and that `FOCUS.md` notes is barely
  covered. Install a layer, render blocks, assert zero. This is the first
  thing to be written with it that was not written *for* it.

Run the whole thing through `scripts/antibox`, not the laptop.

## The listening pass

**This is the step with a sound**, and it is the first thing in the plan that
a musician can hear as new: the same note through two branches of different
effects, summed. Do not close `08` on green tests. The case to play is a
drum loop into a layer of a clean branch and a Drive → Bitcrush branch, mix
swept — which is parallel drum compression, the oldest reason this device
exists.
