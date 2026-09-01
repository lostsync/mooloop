# Bitcrush: add a row of toggles for different bitcrusher math styles

## Problem

From `docs/EFFECTS_FEEDBACK.md`: "Bitcrush: Few diff ways to do the math -
maybe set as a row of toggles to swap between bitcrusher styles."

`BitcrushEffect` (`crates/mooloop-dsp/src/effects/bitcrush.rs`)
implements exactly one algorithm today: amplitude quantization (bit
depth) plus sample-and-hold decimation (downsample), deliberately
non-oversampled per the module's own header comment ("the aliasing
produced by decimating without a band-limiting filter is the effect, not
a defect"). Adding "styles" means adding genuinely different quantization/
decimation math, not re-skinning the same one.

## What to do

1. Decide the concrete style list before writing code — candidates worth
   considering given the existing "aliasing is the point" philosophy:
   plain truncation (current), dither-before-quantize, a different
   sample-hold curve (e.g. linear-interpolated hold vs. hard-held), or a
   companding/µ-law-style nonlinear quantizer. Pick a short fixed set (2-4)
   per `docs/UI_DESIGN.md`'s control-selection table.
   ADAM.md's taste brief is relevant background here: he's drawn to
   "signal hiding inside noise" and dislikes decorative lo-fi that isn't
   actually reactive — favor styles that behave differently under
   signal, not cosmetic variations on the same math.
2. Add a mode field to `BitcrushParams` (mooloop-core) and branch
   `BitcrushEffect::process` on it. Keep bit-depth/downsample as shared
   inputs across styles where that makes sense, rather than duplicating
   unrelated parameters per style.
3. Add the style row to `bitcrush-device.slint` (if that's the actual
   filename — confirm) as a `SelectorBank` (matching the unified style
   from `04-clean-up-device-headers.md`), not a dropdown.

## Verification

`cargo test -p mooloop-dsp` covering each new style's quantization math; a
software-rendered snapshot of the updated Bitcrush face with the new
toggle row; a listening check (or measured output diff) confirming each
style is audibly/numerically distinct from the others.
