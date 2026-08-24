# Rebuild the delay and modulation devices on the extracted block

## Precondition

Only start this once `01-find-the-missing-middle-layer.md` has produced a
concrete block definition and both devices demonstrably get simpler under
it. If step 01 concluded the abstraction isn't there, skip this file.

## What to do

1. Implement the block from step 01's definition, with its own tests, and
   land it *unused*. Same discipline as
   `docs/plans/share-dsp-primitives/02-add-the-missing-primitives.md`:
   additive first, adoption second.
2. Rebuild `ModulationEffect` on it. This one first — it's the smaller
   device (299 lines) and its `Ensemble` mode already hand-rolls the
   multi-tap pattern, so it's the strongest test of whether the block is
   shaped right. If the block can't express ensemble cleanly, it's wrong.
   Note the phaser path deliberately does *not* go through the delay line
   (`modulation.rs:1-7` explains why: an all-pass cascade is cheaper and
   more accurate than faking it with a tap) — leave that alone.
3. Rebuild `DelayEffect` on it (550 lines, the bigger risk). Its
   tempo-sync, its cross-feedback modes, and its `ReadHead` fade-on-time-
   change all have to survive intact.
4. Only after both are rebuilt and passing, consider whether a new device
   falls out cheaply (a proper multi-tap, a ping-pong preset). Do not build
   a new device *before* the two existing ones are converted — a block
   validated only against a device written to fit it proves nothing.

## The bar

Both devices must get shorter. If `delay.rs` plus the new block is longer
than `delay.rs` was, and neither file is clearer, revert and record why in
step 01's notes. That's a legitimate outcome and worth writing down so the
question doesn't get reopened from scratch later.

## Verification

- `cargo test -p mooloop-dsp --release` after each device conversion,
  separately.
- `cargo test -p mooloop-engine --release`.
- Bit-exactness is unlikely to survive an operation reorder, so instead:
  render a fixed input through each device before and after at several
  parameter settings, and compare RMS and spectral envelope rather than
  samples. Any audible difference must be explained, not accepted.
- Manual A/B on both devices across all modes — this is the kind of change
  where a mode nobody tested silently breaks.
