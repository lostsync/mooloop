# Skip effect slots whose input is silent and whose state is at rest

## Problem

`EffectChain::process` (`crates/mooloop-engine/src/render.rs:329`) runs
every occupied, non-bypassed node every block, unconditionally. There is no
path that considers whether the node has anything to do. A project with a
reverb, a delay, and an EQ on a channel that is not currently sounding pays
full price for all three, forever.

(This is separate from — and should land after —
`docs/plans/skip-empty-effect-slots/`, which stops the loop from scanning
all 256 slots. That one removes the cost of *absent* nodes; this one
removes the cost of *quiet* nodes.)

## What to do

1. Add per-slot silence tracking to `EffectChain`, sized like the existing
   fixed arrays (`render.rs:118` shows the `[bool; MAX_EFFECTS_PER_CHANNEL]`
   pattern to follow — keep it fixed-size, no allocation):
   - `silent_frames: [u32; MAX_EFFECTS_PER_CHANNEL]` — consecutive frames of
     silent *input* seen by this slot.
2. In the process loop, before calling `node.process(...)`, decide whether
   to skip:
   - Compute the incoming peak. **Do not add a scan for this** — the chain
     already calls `bus.peak(context.frames)` in the bypass path
     (`render.rs:361`) and `process_block_inner` computes strip peaks for
     the meters (`render.rs:1113`). Reuse or hoist that result so silence
     detection is free.
   - If peak is below a silence threshold, add `context.frames` to
     `silent_frames[slot]`; otherwise reset it to 0.
   - Skip `node.process` when
     `silent_frames[slot] > node.tail_frames()` **or** `node.is_at_rest()`.
   - When skipping, still publish zeroed input/output meters for the slot
     (mirror what the bypass branch does at `render.rs:360-364`) so the UI
     shows silence rather than a frozen last value.
3. **Keep the dry-align ring fed while skipping.** The bypass branch already
   handles this (`render.rs:353-359`) precisely because a stale ring
   produces a wrong blend on re-enable. A skipped slot has the same hazard:
   if `dry_align[slot]` is `Some`, keep pushing the (silent) input through
   it, or reset it, but do not just leave it holding stale audio.
4. Waking up must be instant and click-free: the first block with non-silent
   input processes normally, with no ramp-in and no state reset. This falls
   out naturally if skipping never mutates node state — verify it does not.
5. Pick the silence threshold deliberately and put the reasoning in a
   comment. It should sit below the noise floor of anything audible but
   above denormal range; note that `enable_flush_to_zero()` in
   `crates/mooloop-engine/src/graph.rs` already handles the denormal case,
   so this threshold is about audibility, not about avoiding slow floats.

## Risks to watch

- A node that under-reports `tail_frames` gets its tail cut off mid-decay.
  This is the main way this change can be *heard*. Bias every tail estimate
  long, and treat any report of a truncated reverb/delay tail as a
  `tail_frames` bug in step 01, not a threshold-tuning problem here.
- Nodes that generate output from nothing (an LFO-driven tremolo at full
  depth, a modulation device with feedback still ringing) must never report
  rest while they're still moving. Step 01's per-node test is what protects
  this.

## Verification

- `cargo test -p mooloop-engine --release` — all 57 existing tests. Pay
  particular attention to the bypass-equivalence test around
  `render.rs:2042` (`bypassed should match dry`); an analogous
  skip-equivalence test belongs next to it: a chain fed silence with
  skipping enabled must produce bit-identical output to the same chain with
  skipping forced off.
- Add a test that a reverb's tail is *not* truncated: burst, then silence,
  and assert the decaying output matches the always-process reference for
  the full tail length.
- Measure: extend the throwaway probe used in
  `docs/plans/skip-empty-effect-slots/` to a chain of 3-4 effects on an
  idle channel and record the before/after per-block cost.
