# Spread the partitioned-convolution MAC work across JACK blocks

## Goal

Turn the one-block-in-eight spike (see `01-understand-current-partition-schedule.md`)
into roughly-even per-block cost, so a 2s reverb tail is affordable at a
64 or 128 frame JACK period without xruns.

## Approach (confirm against findings from step 01 before committing to
this shape — adjust if the actual code structure differs)

The FFT/IFFT pair (`fft()` at `reverb.rs:259`, called twice per partition
run) is inherently block-shaped and can't be trivially chunked frame by
frame. But the accumulate loop:

```rust
for partition in 0..self.prepared.partitions {
    ...
    for bin in 0..FFT_FRAMES {
        input[bin].mul_add(left[bin], &mut self.fft_left[bin]);
        input[bin].mul_add(right[bin], &mut self.fft_right[bin]);
    }
}
```

(`reverb.rs:381-391`) is `partitions` independent chunks of identical
work. This is the part actually scaling with tail length (188 partitions
for a 2s tail vs. 53 for 0.5s), and it's independent per partition — nice
to be interruptible.

Two viable shapes, pick based on step 01's findings and prototype both if
unsure:

1. **Spread partitions across intervening small blocks.** Instead of
   accumulating all `partitions` MACs inside one call when the 512-sample
   input window fills, do a fraction of the partitions on every
   `AudioNode::process()` call (each call already knows `ctx.frames`, so
   compute `partitions_per_call = ceil(partitions * frames / CONVOLUTION_BLOCK_FRAMES)`),
   carrying partial `fft_left`/`fft_right` accumulation across calls, and
   only run the FFT/IFFT pair once the full window's worth of partition
   work has actually completed. This keeps the FFT itself as one lump but
   removes the MAC lump, which is the dominant cost at high partition
   counts (2s tail: MACs over 188×1024 complex bins vs. three 1024-point
   FFTs — check with a probe which actually dominates before assuming).
2. **Cap partitions processed per block and defer the rest to a
   background/preparation stage** (more invasive; only pursue if (1)
   doesn't get the max-case under budget) — e.g. process the convolution
   at a coarser internal block size on a lookahead basis, feeding from a
   short internal delay so the audio thread always has precomputed output
   ready. This adds latency (already nonzero — `latency_frames()` reports
   `CONVOLUTION_BLOCK_FRAMES`) and must update `latency_frames()` and the
   dry-align path (`DryAlign`, referenced from `render.rs`'s `EffectChain`)
   to match, or the wet/dry blend goes out of sync.

Start with (1). It's a smaller change and keeps `latency_frames()`
unchanged.

## What to do

1. Prototype option 1 in `ReverbEffect`: track a `partitions_done: usize`
   (or similar) alongside `history_index`, spread the
   `for partition in 0..self.prepared.partitions` loop across however
   many `process()` calls occur before `input_fill` reaches
   `CONVOLUTION_BLOCK_FRAMES`, and only run `fft()` (both directions) plus
   the overlap-save bookkeeping once all partitions for that window are
   done.
2. Handle the case where `frames` doesn't evenly divide the partition
   count or the 512-sample window — the last chunk before the window
   closes must do whatever partitions remain, even if that's more than
   the per-block average, so correctness never depends on block size
   dividing evenly. Guarantee: by the time `input_fill` wraps, all
   partitions for that window are done, full stop.
3. Confirm this doesn't break with a JACK period *larger* than 512 frames
   (e.g. 1024) — in that regime a single `process()` call already spans
   more than one window's worth of samples; make sure the loop structure
   still does the right amount of work per call in that case too (it
   already needs to handle multiple partition-window boundaries within
   one `process()` call, per the existing `while self.output_index <
   CONVOLUTION_BLOCK_FRAMES` / fill-and-flush logic — check whether that
   loop exists already or needs adding).
4. Preserve bit-exact (or perceptually identical — some float reordering
   tolerance is fine) output vs. the current implementation: the *sum* of
   MAC work done per window must be identical, only its distribution
   across calls changes.

## Verification

- Re-run the timing probe from step 01 (2s tail, 64-frame blocks) and
  confirm max-per-block drops well under the 1333us budget — target
  something like <200us worst case, matching the mean-ish cost rather
  than the current 8x spike.
- Existing reverb tests (search `crates/mooloop-dsp/src/effects/reverb.rs`
  for `#[cfg(test)] mod tests`) must still pass — these presumably check
  IR convolution correctness (impulse response, energy, etc.); if they
  compare exact sample output against a fixed expectation, confirm the
  new call-spread schedule doesn't change output at window boundaries.
- Manual: load a 2s decay room reverb on a channel, set JACK buffer to
  64 frames (now easy after
  `docs/plans/archive/reduce-audio-jack-buffer-size/`),
  play a busy pattern, and confirm no xruns (`jack_cpu_load`, or watch
  `EngineEvent::Xrun` / the eprintln in `crates/mooloop-engine/src/graph.rs`
  for "JACK reported an xrun").
- Re-check `crates/mooloop-dsp/src/effects/reverb.rs`'s existing test
  module for latency/alignment assertions and make sure they still hold —
  `latency_frames()` must not have silently changed if this plan stayed
  within option 1's scope.
