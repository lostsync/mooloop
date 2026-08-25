# Smooth effect parameters (this is an audible bug, not a refactor)

## Problem

`crates/mooloop-dsp/src/smooth.rs` exists, is well documented, and its own
header states the reason it exists:

> Parameters arrive from the UI once per block, so using them raw means a
> knob turn steps the signal at every block boundary — zipper noise, or a
> plain click when the parameter scales amplitude directly.

`Smoothed` is used by exactly two nodes: `MonoSynth` and `PolySynth`
(`monosynth.rs:18`, `polysynth.rs:13`). **None of the ten effects use it.**
Verified by import survey: no file under `crates/mooloop-dsp/src/effects/`
imports `crate::smooth`.

So every effect does exactly what that doc comment warns against. Turning
filter cutoff zippers; anything scaling amplitude (drive gain, delay
feedback, wet/dry, dynamics makeup) steps at the block boundary. At a
1024-frame period that's a step every 21 ms during a drag.

This belongs first in the plan because it is a defect users can hear, and
because it is the single clearest case of "a primitive exists and is not
being called."

## What to do

Go effect by effect. For each, identify which parameters are *continuous
and audible* and wrap those — and only those — in `Smoothed`:

- `effects/filter.rs` — cutoff, resonance. Note `Svf` is explicitly
  documented (`filter.rs:6-8`) as staying well behaved while cutoff moves
  every sample, so per-sample smoothed cutoff is exactly what it was built
  for.
- `effects/drive.rs` — drive amount, tone, output level.
- `effects/delay.rs` — feedback, tone/damping, wet level. Delay *time* is
  special: it already has `ReadHead`'s fade machinery
  (`delayline.rs:158` `is_fading`, `:167` `jump_to`) for exactly this, so
  leave time alone and don't double up.
- `effects/modulation.rs` — depth, feedback, spread, tone. Rate can stay
  raw (it feeds a phase increment; a step in rate is not a step in output).
- `effects/dynamics.rs` — threshold, ratio, makeup. The envelope follower
  (`dynamics.rs:50`) already smooths the *detector*; this is about the
  parameters feeding it.
- `effects/bitcrush.rs` — level/mix. Bit depth and rate are intentionally
  steppy; do not smooth those.
- `effects/eq.rs` — the harder case. Its `Biquad` recomputes coefficients
  on parameter change (`eq.rs:113` `update_coefficients`) and a coefficient
  jump on a filter with state is a click. Smoothing coefficients directly
  is wrong (it can leave the filter momentarily unstable). Either smooth
  the *control* values and redesign coefficients per block, or accept the
  current behaviour and note why. Decide explicitly rather than by default.
- `effects/reverb.rs` — wet level only; the IR itself is swapped
  out-of-band already.

Parameters arrive as sample-timed `Event::ParamValue` and each effect
already splits its render at event offsets (see the `process_range` /
event-walk pattern in `eq.rs:139-151`, mirrored across the effects). So the
natural shape is: on the event, `set_target` instead of assigning the field;
in the per-sample loop, `advance()`.

## Constraints

- `Smoothed::advance()` is one branch, a subtract, a multiply and an add
  (`smooth.rs:59-70`). Cheap, but it is per-sample per-parameter — don't
  smooth twenty parameters on a node that only needs three.
- Use `reset_to` (`smooth.rs:47`) on construction and on any preset/patch
  load, so loading a project doesn't ramp every parameter from its default.
  The doc comment calls this out: "Use when there is nothing to click."
- Pick time constants per parameter, not one global value. Gain-ish things
  want a few ms; a filter cutoff sweep wants to track the knob, not lag
  visibly behind it.

## Verification

- `cargo test -p mooloop-dsp --release`.
- Add a per-effect test in the shape of "step a parameter mid-buffer, assert
  no sample-to-sample discontinuity above a threshold" — this is the
  regression test that keeps the next effect from being written without
  smoothing.
- Manual, and the real check: drag filter cutoff across its range on a
  sustained pad and confirm the zipper noise is gone. Do the same on drive
  and on delay feedback.
