# Collapse the duplicate implementations onto the shared primitives

## Problem

Primitives exist and are simply not called. Verified duplications:

1. **`Svf`** — `sampler.rs:463-464` contains the SVF tick math inline, and
   the `damping` / `a1` lines are character-for-character identical to
   `filter.rs:56-57`. `filter.rs`'s own header acknowledges this: "The
   sampler keeps its own inline per-voice filter math; new instruments use
   these."
2. **`Adsr`** — `env.rs`'s header likewise: "The sampler keeps its own
   private ADSR (parameterized directly from `SamplerParams`); these are the
   generic versions new instruments use."
3. **`Lfo`** — `ModulationEffect` hand-rolls a phase accumulator and calls
   `sin()` directly (`modulation.rs:100-101` for the advance,
   `modulation.rs:164` for the spread phase), while `lfo.rs` provides
   exactly this with four more waveshapes, phase-aligned starts, and a
   retrigger. `Lfo` is currently used only by `MonoSynth`/`PolySynth`.
4. **One-pole lowpass** and the **log-frequency map** — the sites listed in
   `02-add-the-missing-primitives.md`.

The sampler cases are documented as deliberate, which is worth taking
seriously: the sampler is the oldest and hottest node in the crate
(1148 lines, the largest file) and its per-voice loop may be shaped around
the inlined math. Treat "leave it, and change the comment to say why" as a
legitimate outcome for those two — but make that call from measurement, not
from inertia.

## What to do

Take these one at a time, each independently revertable:

1. **`ModulationEffect` → `Lfo`.** The clearest win and the lowest risk.
   Replace the raw phase field with an `Lfo`, which immediately makes the
   other waveshapes available to the device (currently sine-only) and gets
   phase-aligned retrigger for free. Watch the stereo spread: it currently
   reads a second value at `phase + spread * 0.25` from the same
   accumulator, so either keep two `Lfo`s with an initial phase offset or
   add a phase-offset read to `Lfo`.
2. **The three one-pole tone/damping filters → `OnePoleLp`** (added in
   step 02), after confirming they actually want the same semantics.
3. **The four log-frequency maps → the shared helper**, including checking
   the Slint-side duplicate in the device faces.
4. **Sampler `Svf` / `Adsr`** — attempt, then measure. Convert, run the
   sampler's tests, and benchmark the per-voice loop before and after. If
   the shared version costs measurably more (it may not — `Svf::tick` is
   `#[inline]`-able and the math is identical), revert the conversion and
   replace the module-header comments in `filter.rs` and `env.rs` with the
   actual measured reason. A comment that says "the sampler has its own
   because sharing cost N% in the voice loop" is worth keeping; one that
   just states the fact is what let this drift.

## Constraints

- One primitive per commit. If output changes audibly, it must be obvious
  which swap did it.
- Some of these will change output in the last bit — different operation
  order, `Lfo`'s quarter-cycle-shifted shapes vs. a raw `sin()`. Decide
  per case whether bit-exactness matters (it does for anything a test
  asserts on) and if the change is intentional, say so in the commit.
- Do not "fix" behaviour while moving code. If the hand-rolled version had
  a quirk, port the quirk, then remove it in a separate commit where it can
  be judged on its own.

## Verification

- `cargo test -p mooloop-dsp --release` after each individual swap.
- `cargo test -p mooloop-engine --release` — the engine has render-level
  tests that will catch a changed effect output reaching the master.
- Manual A/B on the modulation device specifically, since it's the one
  swap that changes a user-visible feature surface (more waveshapes).
- After all of this: re-run the import survey
  (`grep -rn "use crate::" crates/mooloop-dsp/src`) and confirm the effects
  are actually reaching into the shared modules. If `effects/` still barely
  imports anything from the crate root, the collapse didn't happen.
