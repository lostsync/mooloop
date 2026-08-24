# Skip whole channel strips that are idle

## Why this is the actual win

Step 02 saves per-effect cost. This step saves per-*channel* cost, and on a
realistic project it's much larger: on a 32-channel arrangement with four
things sounding at a given moment, 28 strips are rendering silence through
their generator, their whole effect chain, their pan stage, and summing it
into a bus.

Today the only strip-level skip is mute
(`crates/mooloop-engine/src/render.rs:1103`, `if strip.output.muted
{ continue; }`). Everything else runs.

## What to do

1. In `process_block_inner` (`render.rs:1043`), for each strip in the
   channel loop (`render.rs:1097-1123`), skip the whole body when **all** of:
   - the strip has no events this block (`self.events[index]` is empty),
   - its generator reports idle (needs `Sampler::is_idle`,
     `sampler.rs:171`, made public — plus the synth equivalents from
     `01-add-rest-and-tail-to-audionode.md`),
   - its effect chain reports fully at rest (add
     `EffectChain::is_at_rest(&self) -> bool` that ands over occupied
     slots' skip conditions from step 02).
2. A skipped strip must still:
   - clear its bus (or be guaranteed nothing reads stale contents — check
     `strip.bus.clear(frames)` at `render.rs:1106` and whether anything
     downstream reads the strip bus outside the add-to-destination path),
   - publish zeroed device meters and playhead positions rather than
     leaving the last non-zero values pinned in the UI
     (`render.rs:1108-1112`),
   - contribute nothing to its destination bus — which is automatic if it's
     genuinely silent, but assert it rather than assume it.
3. Do **not** extend this to bus strips (`render.rs:1128` onward) in this
   step. The comment at `render.rs:1145-1147` documents a deliberate
   decision that a muted bus still processes so its tails decay; buses are
   few, shared, and the reasoning there is subtler. Leave them running and
   revisit separately if measurement says they matter.

## Verification

- `cargo test -p mooloop-engine --release`.
- A skip-equivalence test at the strip level: render a fixed project twice,
  once with strip skipping enabled and once forced off, and assert the
  master output is bit-identical. This is the test that matters most — if
  it passes on a project with reverbs, delays, and long sampler tails, the
  change is safe.
- Measure with the render probe on a synthetic 32-channel project with one
  channel sounding. Expected shape: per-block cost tracks the number of
  *sounding* channels, not the number of channels in the project.
