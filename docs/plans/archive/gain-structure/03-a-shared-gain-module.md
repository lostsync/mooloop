# One gain module, mirrored into Slint

## Problem

There is no shared definition of decibels in this codebase.

- `crates/mooloop-ui/src/meter.rs:3` owns `MIN_DB`, `:71` `linear_to_db`,
  and `:80` `db_to_linear` — in the *UI* crate, where the engine cannot
  reach them.
- `crates/mooloop-ui/ui/main.slint:1479` open-codes the inverse as
  `pow(10, v / 20)` with a hand-written `v <= -59.9` floor check, and
  again at `:2128`.
- `MAX_LINEAR_GAIN` (`crates/mooloop-core/src/channel.rs:23`) and
  `MAX_TRIM_GAIN` (`crates/mooloop-engine/src/render.rs:60`) are the same
  4.0 declared twice, in two crates, with two doc comments.

Every step after this one needs these conversions, and step 04 needs a
fader taper that Rust and Slint must agree on exactly — a taper that
disagrees across the boundary means a fader whose readout does not match
its audio.

## What to do

1. Add `crates/mooloop-core/src/gain.rs` and export it from `lib.rs`:

   - `MIN_DB` (-60.0) and `MAX_DB` (+12.0).
   - `db_to_linear(db) -> f32` / `linear_to_db(linear) -> f32`, moved from
     `crates/mooloop-ui/src/meter.rs` with their existing semantics intact:
     at or below `MIN_DB` is silence (0.0), not residual gain. The tests at
     `meter.rs:91-104` move with them.
   - `MAX_LINEAR_GAIN`, with `channel.rs:23` re-exporting it and
     `render.rs:60`'s `MAX_TRIM_GAIN` deleted in favour of it.
   - `fader_position_to_db(position) -> f32` and
     `fader_db_to_position(db) -> f32`, implementing the breakpoint table in
     `01-the-gain-contract.md` by linear interpolation *in dB*. They must
     round-trip; assert that.
   - `format_db(db) -> String` producing the `TrimKnob` strings (`-inf`,
     `±0.0 dB`, `+3.0 dB`). Used by Rust-side readouts; Slint gets its own
     copy below.

2. Add `crates/mooloop-ui/ui/gain.slint` with a `GainMath` global mirroring
   the same functions — `db-to-linear`, `linear-to-db`,
   `fader-position-to-db`, `fader-db-to-position`, `format-db` — following
   the shape of the existing `MeterScaleMath` global
   (`crates/mooloop-ui/ui/meters.slint:12`). `TrimKnob`'s value-text
   expression (`controls.slint:987-989`) becomes `GainMath.format-db` so
   there is one formatter, not two.

3. Add a Rust test that walks the breakpoint table and a spread of
   intermediate positions, asserting the Rust and Slint tapers agree. Slint
   globals are not callable from Rust, so this means either exporting the
   table as a shared constant both sides read, or a small
   `.slint`-evaluating test. Prefer the former: define the breakpoints once
   in `gain.rs`, and generate or hand-mirror them into `gain.slint` with a
   test that fails loudly if the two lists diverge.

## Constraints

- `mooloop-core` is the shared crate; the module must not pull in UI or
  engine dependencies.
- These functions are called per control change, not per sample, so
  clarity beats speed. Do not add a lookup table.
- Deleting `MAX_TRIM_GAIN` touches the clamps at `render.rs:544-545`,
  `:660`, `:1509`, and `:1520`. Their behaviour must not change in this
  step — this is a pure consolidation.

## Verification

`cargo test -p mooloop-core` for the new module, `cargo test -p
mooloop-ui` and `cargo test -p mooloop-engine` for the moves. The
characterization tests from step 02 must all still pass unchanged: nothing
audible changes in this step.
