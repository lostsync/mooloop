# Block-constant work status

Plan C of `reports/fable-2026-09-22.md` (finding 2's phaser/compressor/
sampler items, and finding 6, FTZ on aarch64). Implemented and verified
2026-09-22 on `claude/sharp-hypatia-hy7frr`, on top of Plans A, B and D
(already landed on this branch). `cargo test -p mooloop-dsp`,
`cargo test -p mooloop-engine`, and
`cargo clippy -p mooloop-dsp -p mooloop-engine --all-targets -- -D warnings`
are all green.

## What changed

- `crates/mooloop-dsp/src/effects/modulation.rs` (the phaser):
  - `ModulationEffect` gained a `tilt: [f32; MAX_PHASER_STAGES]` field and
    `rebuild_tilt(&mut self)`, which fills it from the current, clamped
    `stages` with the exact formula `phaser_sample` used to evaluate inline
    per stage per sample. Called from `new`, `set_params`, and the
    `MODULATION_PARAM_STAGES` arm of `apply_param` -- everywhere `stages`
    can change -- so the table is always current. `phaser_sample` now reads
    `self.tilt[stage]` instead of recomputing it.
  - `phaser_hz` no longer takes `depth`/`color`; it takes `log2_center` and
    `octaves`, and folds `center * 2^(lfo*octaves)` into one `exp2` of a
    sum: `(log2_center + lfo * octaves).exp2()`. `process_range` computes
    `log2_center = (220.0 * 28.0f32.powf(color)).log2()` and
    `octaves = 0.15 + depth * 2.2` once per sample, right after advancing
    the `depth`/`color` smoothers and before the Phaser-mode branch, so
    each is evaluated once instead of up to 24 times (12 stages, 2
    channels). `allpass_coefficient` is untouched and still runs per stage
    per sample, because the LFO genuinely sweeps it.
  - Three new tests: `phaser_hz_matches_the_original_two_powf_formula`
    (the reformulated `phaser_hz` against the original two-`powf` formula,
    reimplemented inline in the test, across a grid of `depth`/`color`/LFO
    values -- asserts relative error `< 1e-4`, i.e. float-rounding-only),
    `tilt_table_matches_the_inline_per_stage_formula` (the table against
    the old inline formula for every stage count 4-12), and
    `changing_stages_mid_stream_rebuilds_the_tilt_table` (a live
    `apply_param` stage-count change is reflected in the table). The
    existing `phaser_and_chorus_have_distinct_responses` and
    `depth_change_mid_block_does_not_click` tests still pass unmodified,
    which is the audio-level regression check for this change.
- `crates/mooloop-engine/src/block_cost.rs`: `playing_effect_cost` gained a
  `Phaser12` row -- an `EffectSlotState::modulation` with `mode: Phaser,
  stages: 12` -- printed the same way as the other rows, beside the
  existing chorus-mode `Modulation` row (which exercises the delay path,
  not the phaser). Imports gained `ModulationMode`, `ModulationParams`.
- `crates/mooloop-dsp/src/effects/dynamics.rs` (compressor makeup):
  `CompressorEffect` gained a cached `makeup: f32` field. `process_range`
  now advances `makeup_db` and only recomputes
  `db_to_linear_unfloored(makeup_db)` when `!self.makeup_db.is_settled()`;
  while settled it reuses the cache. `Smoothed::is_settled()` already
  existed in `smooth.rs` (returns `current == target`, exact by
  construction) and needed no changes -- just reuse, as the report
  expected. `new()` and `set_params()` (which `reset_to`s the smoother
  straight to settled) both seed/refresh the cache so it is never stale on
  the first sample after either. Existing tests
  (`compressor_makeup_restores_level`,
  `compressor_makeup_change_mid_block_does_not_click`) pass unmodified.
- `crates/mooloop-dsp/src/filter.rs`: `apply_drive`'s body split into
  `drive_compensation(drive: f32) -> f32` (just the `tanh` that depends on
  `drive` alone) and `apply_drive_compensated(input: f32, drive: f32,
  compensation: f32) -> f32` (the per-sample `tanh` plus the supplied
  compensation). `apply_drive` itself is now `apply_drive_compensated(input,
  drive, drive_compensation(drive))` -- same formula, same call order,
  bit-identical -- so its ~10 other call sites (drumsynth, ds01, monosynth,
  polysynth, effects/filter.rs) are untouched.
- `crates/mooloop-dsp/src/sampler.rs` (`shape_frame`): `VoiceContext`
  gained `bit_scale: f32` (`2^(bits-1)` for the bit-reduction quantizer)
  and `drive_compensation: f32`, both functions of `params` alone.
  `render_range` computes them once and puts them in the `cx` it already
  builds once per call and hands to every voice; `render_voice_range`
  destructures and passes them through to `shape_frame`, which takes them
  as parameters instead of computing `scale` inline (still gated behind
  the same `bit_reduction <= f32::EPSILON` branch) or calling the plain
  `apply_drive`. Same formulas, same order of operations, bit-identical;
  the `#[cfg(test)] shape_first_voice` helper was updated the same way.
- `crates/mooloop-dsp/src/osc.rs` (sine): added a 2048-entry lookup table
  (`SINE_TABLE`, built once via `std::sync::OnceLock`, the same pattern
  `interpolate::SincTable::shared` uses) and `table_sine(phase) -> f32`,
  linear interpolation between the two nearest entries. `wave_value`'s
  `OscWave::Sine` arm now calls `table_sine(phase)` instead of
  `(phase * TAU).sin()`. `Osc::new()` forces the table build, mirroring
  `SincTable::shared()` being forced at device construction, so no
  audio-thread call is ever the first to build it. New test
  `sine_table_thd_is_below_90_db`: 440 Hz at 48 kHz over exactly 1,200
  samples (11 whole cycles, `1200 * 440 / 48000 == 11`, so a single-period
  DFT is exact and there is no leakage to separate from real distortion --
  the same trick `sync_blep_gets_the_harmonics_closer_than_a_naive_reset`
  already used in this file). **Measured THD: -118.20 dB**, comfortably
  under the -90 dB bound. Module header comment updated: sine is no longer
  "naive" in the literal sense (triangle still is); both still alias
  negligibly, which is what the header's claim was actually about.
- `crates/mooloop-engine/src/executor.rs` (`enable_flush_to_zero`): added
  `#[cfg(target_arch = "aarch64")]` and `#[cfg(not(any(target_arch =
  "x86_64", target_arch = "aarch64")))]` arms alongside the existing
  `#[cfg(target_arch = "x86_64")]` one. The aarch64 arm reads FPCR (`mrs
  {0}, fpcr`, 64-bit `reg`), sets bit 24 (FZ), and writes it back (`msr
  fpcr, {0}`), with a comment on why there is no separate DAZ bit to set at
  this width and why FZ16 does not apply. The doc comment above the whole
  function now says both targets instead of implying x86_64 only.
- `crates/mooloop-engine/src/render.rs`: three `size_of` footprint
  assertions in `footprint::the_render_graph_costs_what_the_project_uses`
  moved by the phaser's new 48-byte `tilt` table (`ModulationEffect` is
  held by value inside `MlP8` as the finishing chorus, which is held by
  value inside `ChannelStrip`): `size_of::<MlP8>()` 5,904 -> 5,952,
  `size_of::<ChannelStrip>()` 42,456 -> 42,504, the `per_live` sum 155,488
  -> 155,536, and the final rolled-up `(fixed + per_live * 16) / 1024`
  crossed a KiB boundary, 2,916 -> 2,917. Each site got a comment recording
  why, in the style the rest of that test already uses. This is the one
  test this plan's changes touched; everything else needed no test
  updates beyond the new tests listed above.

## Deviations from the report's literal wording, and why

- **"Compute `center`/`octaves` once per `process_range`" was implemented
  as once per sample, not once per call to `process_range`.** `depth` and
  `color` are `Smoothed` values that genuinely change sample to sample for
  up to `PARAM_SMOOTH_S` (10 ms) after a parameter event, including partway
  through a `process_range` call that does not itself start at an event
  (the smoother can still be catching up from an event earlier in the
  block). Hoisting the computation to the top of `process_range` and
  holding it fixed for the whole call would have changed the audible
  output during that transient -- not bit-identical, and not merely a
  float-rounding difference, a real behavior change. The report's own
  `phaser_hz`'s two `powf` become one `exp2` of a per-sample sum"
  instruction (and the task's explicit permission to leave a genuinely
  per-sample value alone) point the same way: what was actually redundant
  was computing `center`/`octaves` up to 24 times *within one sample* (once
  per stage per channel) for a value `depth`/`color` already fixed once
  that sample. That redundancy is what got hoisted; the smoothing itself
  did not.
  - This is a pure hoist (frequency, not value) plus the explicitly-
    requested `exp2` reformulation, verified two ways:
    `phaser_hz_matches_the_original_two_powf_formula` pins the
    reformulation to float rounding, and
    `tilt_table_matches_the_inline_per_stage_formula` /
    `changing_stages_mid_stream_rebuilds_the_tilt_table` pin the tilt
    hoist exactly. The existing render-level phaser tests
    (`phaser_and_chorus_have_distinct_responses`,
    `depth_change_mid_block_does_not_click`) pass unmodified, which is the
    "before/after render test" the plan asked for as an alternative to
    reasoning it through in comments alone.
- **Step 3's "compute once per `render_range`" for the sampler's
  `bit_scale`/`drive_compensation`** is implemented literally as written:
  `self.params` (unlike the phaser's `depth`/`color`) is not smoothed and
  is genuinely fixed for the whole `render_range` call, so no transient
  question arises there.
- **`apply_drive`'s public signature is unchanged.** The report only asked
  for the sampler's call sites to stop paying the compensation `tanh`
  twice a sample; `apply_drive` has ~10 other callers across drumsynth,
  ds01, monosynth, polysynth, and the filter effect that are out of this
  plan's scope, so the compensation-only part was split into
  `drive_compensation`/`apply_drive_compensated` and `apply_drive` kept as
  a thin, bit-identical wrapper over them rather than being restructured
  itself.

## aarch64 (untestable here)

This box is x86_64 only; the aarch64 target was installed
(`rustup target add aarch64-unknown-linux-gnu`) to get as close to the
requested verification as this environment allows, but
`cargo check -p mooloop-engine --target aarch64-unknown-linux-gnu` cannot
complete: `jack-sys` and `mp3lame-sys` are unconditional native
dependencies of `mooloop-engine` and their build scripts need a full
aarch64 cross toolchain (`aarch64-linux-gnu-gcc`, a matching `pkg-config`
sysroot) that this container does not have, and installing one is well
outside this plan's scope. As a narrower check, the `enable_flush_to_zero`
aarch64 function was copied into a standalone file and compiled on its own
with `rustc --target aarch64-unknown-linux-gnu --crate-type lib
--emit=metadata` (no workspace dependencies, so no native build scripts
involved), which succeeded -- the inline asm syntax, register classes, and
types check for that target. The macOS CI job remains the real aarch64
run, per the report.

## Not touched

`mooloop-ui` was not touched, per instructions. No commits were made; the
tree has the working changes only.
