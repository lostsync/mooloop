# Skip unused effect slots instead of scanning all 256 every block

## Problem

`EffectChain::process` (`crates/mooloop-engine/src/render.rs:335`) loops
`0..MAX_EFFECTS_PER_CHANNEL` (256) for *every* channel and bus, every
block, regardless of how many slots are actually occupied. Per iteration
it does a telemetry check, a bypass check, and (critically)
`self.events[slot].clear()` — `PendingEffectParams` is 192 bytes, so an
empty chain still memsets ~49KB and walks six 256-entry arrays for no
reason.

Measured with an otherwise-idle `RenderState` (no effects, no samples),
release build, `process_block`:

```
channels= 1  64 frames:  43.8 us/block  (0.4ms JACK budget at 64/48k)
channels= 8  64 frames:  64.5 us/block
channels=16  64 frames:  98.4 us/block
channels=32  64 frames: 159.4 us/block   (~12% of budget doing nothing)
```

This is fixed overhead independent of block size (nearly identical at 64
and 128 frames), and it evicts cache lines the reverb/delay/filter state
actually needs, compounding the reverb load-spike problem in
`docs/plans/amortize-reverb-partition-cost/`.

## What to do

Add a `bound: usize` field to `EffectChain` — one past the highest slot
index that currently holds a node — and iterate `0..self.bound` in
`process` instead of `0..MAX_EFFECTS_PER_CHANNEL`.

1. Add `bound: usize` to `EffectChain`, initialized to `0` in `new()`.
2. Update every mutator that can change occupancy to keep `bound` correct:
   - `install()` (`render.rs:~276`): `self.bound = self.bound.max(slot + 1)`
     on successful install.
   - `remove()` (`render.rs:~234`): after clearing the slot, recompute via
     a `rposition` scan (`self.nodes.iter().rposition(Option::is_some).map_or(0, |i| i + 1)`)
     — removal is not a hot path, an O(n) scan here is fine.
   - `swap()` (`render.rs:~264`): `self.bound = self.bound.max(slot_a.max(slot_b) + 1)`.
   - `clear()` (`render.rs:~150`): reset `self.bound = 0`.
   - `load()` (`render.rs:~303`, called only off the realtime thread):
     ensure `bound` reflects the loaded slot count after the loop, or call
     `install()`'s bound-update logic consistently — `load()` currently
     calls `self.install(...)` per slot, so it should fall out for free;
     verify it does.
3. Change the loop head in `process()` from
   `for slot in 0..MAX_EFFECTS_PER_CHANNEL` to `for slot in 0..self.bound`.
4. Double check `replace_if_kind` (resource-backed hot-swap path) doesn't
   need a `bound` update — it only ever replaces an already-occupied slot,
   so it shouldn't change `bound`, but confirm that assumption against
   how it's called.

## Verification

- `cargo test -p mooloop-engine --release` — all existing tests must still
  pass unmodified (this is a pure optimization, no behavior change).
- Re-run the measurement above (a throwaway `#[cfg(test)]` probe timing
  `process_block` in a loop is fine, delete it before committing) and
  confirm the empty-chain cost drops to roughly array-size-independent
  (single-digit microseconds regardless of `MAX_CHANNELS`/`MAX_BUSES`
  scanned, dominated by the channels/buses actually in the project).
- Manually load a project with effects on several slots, including a
  non-contiguous arrangement (slot 0 and slot 5 occupied, 1-4 empty) via
  swap/remove, and confirm audio and telemetry (spectrum analyzer,
  input/output meters) are unaffected — occupancy tracking must not skip
  a slot that's actually in use.
