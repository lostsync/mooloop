# Rebuild the delay and modulation devices on the extracted block

**Cancelled 2026-10-01 (MOO-150), because step 01 found no block to build.**
`01-find-the-missing-middle-layer.md` has the reasons under *The answer*.
In short: `DelayEffect` and `ModulationEffect` share a `DelayLine` and
little else. They disagree on what goes back into the line (one tap or a mix
of taps), where the damping sits (inside the loop or on the wet output), how
the loop is bounded (a `tanh` knee or a linear trim), how a NaN is handled,
and whether the read head fades. Every one of those is a deliberate,
documented, tested choice. What they do share already lives in shared code:
`DelayLine`, `node::feedback_tail_frames`, `Discontinuity::invalidates_tails`,
`write_silence`, `OnePoleLp` and `Smoothed`. What remains copied is about ten
lines of arithmetic, which a block would not make shorter.

## What would reopen it

A third device that needs a delay tap, such as a multi-tap or ping-pong delay.
None is planned. When one is, do these in order:

1. Write down that device's loop: what it feeds back, where it damps, and how
   it bounds the loop. Use the table in step 01.
2. If the loop matches `DelayEffect`'s, the block is `ReadHead` plus that
   loop, lifted out of `delay.rs`. Land it unused, then move `DelayEffect`
   onto it. `DelayEffect` has to get shorter, keep its three head modes, and
   keep `a_nan_sample_leaves_the_loop_after_one_trip` and the feedback bound
   test passing. Then build the new device on the block. This is Effects'
   work: the loop is effect policy, and `ReadHead` already supplies the
   primitive.
3. If the loop matches neither device, there is still no shared block. The
   new device gets its own loop over `DelayLine` and `ReadHead`.

Leave `ModulationEffect` alone in every case. Its loop feeds back a mix of
taps, and splitting that into per-tap feedback changes the sound.
