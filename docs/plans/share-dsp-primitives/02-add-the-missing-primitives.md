# Add the primitives that are missing, so the duplicates have somewhere to go

## Problem

Some things are reimplemented across the crate not out of carelessness but
because the shared version does not exist. Adding these is a prerequisite
for `03-collapse-duplicate-implementations.md`; on its own it is purely
additive and can land safely without touching any node.

Measured duplication (each verified by reading the sites):

| Thing | Shared version | Copies in the tree |
|---|---|---|
| One-pole lowpass | **absent** (`filter.rs` has `OnePoleHp` only) | `effects/drive.rs:34-35` (`tone_lp_l/r`), `effects/delay.rs:41-42` (`damp_l/r`), `effects/modulation.rs` (`tone_l/r`) |
| Log-frequency map `20·(max/20)^x` | **absent** | `monosynth.rs:259`, `polysynth.rs:330`, `sampler.rs:457`, `analysis.rs:55` |
| Biquad + RBJ designers | **private to `eq.rs`** (`eq.rs:16-61`: `peak`, `shelf`, `pass`, `set_normalized`) | nothing else can use them |
| All-pass stage | **private to `modulation.rs`** (`modulation.rs:38-47`) | — |

## What to do

1. **`OnePoleLp` in `crates/mooloop-dsp/src/filter.rs`**, next to the
   existing `OnePoleHp` (`filter.rs:76`) and matching its API exactly
   (`new`, `set_cutoff(hz, sample_rate)`, `reset`, `next_sample`). Three
   call sites currently spell the same one-pole three slightly different
   ways; one of them is the "tilt" in `drive.rs:107` and one is feedback
   damping in the delay, so check whether they want identical semantics
   before assuming a single type covers all three. If one genuinely differs
   (e.g. it's a tilt, not a lowpass), give it its own named primitive rather
   than forcing it into `OnePoleLp` with a flag.
2. **Promote `Biquad` out of `eq.rs`** into its own module (or into
   `filter.rs` alongside `Svf`), keeping the RBJ-cookbook designers
   (`peak`, `shelf`, `pass`) as public constructors. Two independent filter
   kernels in one crate is fine — SVF and biquad have genuinely different
   trade-offs, and `filter.rs:6-8` already explains when to reach for the
   SVF — but the biquad being unreachable outside the EQ is not.
   Reformat while moving: `eq.rs`'s `Biquad` is written in a dense
   multi-statement-per-line style that doesn't match the rest of the crate.
3. **Promote `AllPass`** (`modulation.rs:38-47`) similarly. It's four lines
   and it's the building block of phasers, reverb diffusers, and
   fractional-delay interpolation — it should not be locked inside one
   effect.
4. **A log-frequency mapping helper.** Four sites compute
   `20.0 * (max_hz / 20.0).powf(x)` to turn a normalized 0..1 knob into Hz.
   Put one `pub fn` somewhere sensible (a new `scale.rs`, or `filter.rs`)
   with its inverse alongside it, since the UI needs the inverse to display
   the value. Check whether the UI is separately duplicating the forward map
   in Slint (`filter-device.slint` computes
   `round(20 * pow(1000, root.cutoff))` inline for its readout) — if the
   Rust and Slint versions ever disagree, the knob lies about its own value.

## Constraints

- Purely additive. Do not change any existing node in this step; the
  swap-over is step 03, so a bisect can separate "the primitive is wrong"
  from "the adoption is wrong."
- Every new primitive needs the same treatment the existing ones got: a doc
  comment explaining *when to reach for it and when not to* (the existing
  modules are good at this — `lfo.rs:3-6` explaining why it's separate from
  `osc.rs` is the standard to match), and unit tests in the style of
  `filter.rs:127-180`.

## Verification

- `cargo test -p mooloop-dsp --release` — should be unchanged plus new
  tests, since nothing calls the new code yet.
- For the promoted `Biquad` and `AllPass`: character-identical behaviour to
  the originals. Easiest proof is to move the code rather than retype it,
  and confirm the existing `eq.rs` and `modulation.rs` tests still pass
  against the promoted version in step 03.
