# Understand the current convolution schedule and confirm the spike

## Problem

`ReverbEffect` (`crates/mooloop-dsp/src/effects/reverb.rs`) is
uniformly-partitioned convolution: `CONVOLUTION_BLOCK_FRAMES = 512`
(`reverb.rs:22`), and `process_partition()` (`reverb.rs:363`) does the
full FFT-multiply-accumulate-IFFT for *all* partitions in one shot,
triggered whenever `self.input` fills to 512 samples
(inside `AudioNode::process`, `reverb.rs:417`).

At a small JACK period (64 or 128 frames) this means 7-15 blocks of near
free-riding followed by one block that does the entire partitioned
convolution — hundreds of complex multiply-adds across every partition,
three 1024-point FFTs — in a single `process()` call.

Measured, one reverb instance, nothing else running, release build:

```
decay  frames  parts   mean     p50      p99      max     budget   worst
0.5s     64      53   21.6us    0.2us   320us    710us   1333us    53%
1.0s     64     105   32.9us    0.2us   478us    957us   1333us    72%
2.0s     64     188   54.0us    0.2us   708us   1400us   1333us   105%
```

Mean load is low (this is why it "should" be affordable), but p50 is
~0.2us and max blows the 64-frame budget outright for a 2s tail. This is
a load-*distribution* bug: the DSP cost per sample is fine, it's just all
paid in one out of every ~8 blocks.

## What to do (this file is investigation only, no code changes)

1. Read `process_partition()` and `AudioNode::process()` for
   `ReverbEffect` end to end and write down, precisely:
   - What triggers a partition run (`self.input_fill == CONVOLUTION_BLOCK_FRAMES`
     — confirm exact condition).
   - What `process_partition` actually computes: is it *all* partitions
     every time (uniform partitioning, not the input-side block only)?
     Confirm by reading the `for partition in 0..self.prepared.partitions`
     loop at `reverb.rs:381`.
   - How `output_left`/`output_right`/`output_index` deliver samples back
     out between partition runs — this is what has to keep working
     unchanged if partition work moves to a different cadence.
2. Confirm with a throwaway timing probe (same shape as the measurement
   above, deleted before committing) that the spike scales with
   `self.prepared.partitions`, not with `frames` — this determines whether
   spreading partition work *within* a 512-sample input window across the
   intervening small blocks is enough, or whether individual partitions
   need to be spread across multiple small blocks too.
3. Write a short note (can live in this plan's next file, or as a comment
   in the PR) confirming: is 512 the block size across which uniform
   partitioning is applied, or is there already a smaller inner
   granularity that inner partitioning already exploits? This matters
   because the amortization scheme in the next step depends on how much
   independent work exists to spread out.

## Why this step exists on its own

The fix in the next file assumes a specific structure (uniform
partitioning, one FFT trigger per 512 input samples, N independent
partition MACs that don't depend on each other). Don't start restructuring
until that's confirmed against the actual code — a wrong assumption here
produces a fix that doesn't actually flatten the spike, just moves it.
