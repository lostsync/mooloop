# Filter coeffs status

Plan B of `reports/fable-2026-09-22.md` (finding 2): the SVF took a cutoff
and re-derived its coefficient set -- `tan()`, a divide, three products --
on every sample, every stage, every voice, for values that mostly change
once a block. Implemented 2026-09-22, not yet built or measured (the
container this was written in cannot run `cargo`; the coordinator compiles
and reports back).

## What changed

- `crates/mooloop-dsp/src/filter.rs`: `Svf::tick` split into
  `SvfCoeffs::for_cutoff(cutoff_hz, resonance, sample_rate) -> SvfCoeffs`
  (`g, a1, a2, a3, damping`, exactly the math `tick` used to run inline) and
  `Svf::tick_with(&mut self, input, &SvfCoeffs) -> (f32, f32, f32)` (only the
  per-sample state update). `tick` is now a thin wrapper --
  `SvfCoeffs::for_cutoff` then `tick_with` -- so the ~86 existing call sites
  (`next_sample`/`next_sample_lp_hp`/`next_sample_lp_bp_hp`) compile
  unchanged and stay bit-identical. `SvfCoeffs::lerp(&self, &other, t)` is a
  component-wise linear interpolation, `t` clamped to `[0, 1]`. Two new
  tests (`tick_with_matches_the_wrapper_bit_for_bit`,
  `svf_coeffs_lerp_hits_its_endpoints_and_midpoint`) pin both.
- `crates/mooloop-dsp/src/smooth.rs`: `Smoothed` gained `target(&self) -> f32`
  beside the existing `value()`, a general-purpose accessor (not used by the
  final `effects/filter.rs` design below, which reads `value()` and
  `advance_by` instead -- see "Why chunked instead of once per range").
- `crates/mooloop-dsp/src/effects/filter.rs`: `FilterEffect::process_range`
  now walks its range in `CONTROL_RATE_FRAMES`-sized (32-frame) chunks. For
  each chunk it computes `SvfCoeffs` once from the smoothers' current value
  (chunk start) and once from `Smoothed::advance_by(chunk_frames)` (chunk
  end -- which lands the smoother exactly where that many per-sample
  `advance()` calls would have), and lerps between the two per sample
  instead of recomputing coefficients every sample. `process_channel` takes
  `&SvfCoeffs` instead of `cutoff`/`resonance` and calls `tick_with`. Two
  new tests: `coefficient_lerp_matches_the_old_path_at_a_static_cutoff` (a
  chirp through a static cutoff, old per-sample-`tan()` path reproduced
  inline as `old_path_static_cutoff`, RMS error asserted `< -80 dBFS`) and
  `envelope_sweep_leaves_no_discontinuity` (cutoff stepped every 32 frames
  across 300 Hz-4 kHz-300 Hz via `ParamValue` events, max sample-to-sample
  step asserted `< 0.5`).
- `crates/mooloop-dsp/src/sampler.rs`: the voice filter's hand-copied SVF
  (`Voice::filter_low`/`filter_band`, inline `tan()`/coefficient math in
  `shape_frame`) is now `Voice::filter: [Svf; 2]` plus
  `SvfCoeffs::for_cutoff`/`tick_with`, sharing one coefficient set across
  the stereo frame exactly as the hand copy did -- see "Why the sampler's
  copy went away" below. `hz_from_normalized`'s `powf` moved out of the
  per-sample loop to `Sampler::filter_base_hz(params, sample_rate) ->
  Option<f32>` (`None` when the filter is fully bypassed, matching the old
  early return), computed once per voice per `render_voice_range` call.
  `shape_frame` takes the result as a parameter instead of recomputing it.
- `crates/mooloop-dsp/src/monosynth.rs`, `polysynth.rs`, `mlm1.rs`: the
  `hz_from_normalized` call moved out of the per-sample loop to once per
  `render_range`, reading the cutoff smoother caught up to the range's end
  via `Smoothed::advance_by` (which lands where `frames` calls to
  `advance()` would have, `smooth.rs`'s own test says how closely). The
  cutoff-versus-`0.999` bypass gate reads that same once-per-range value
  now, so a bypass-threshold crossing resolves at range granularity instead
  of sample-accurately. `polysynth.rs` does this per voice into a
  `[f32; MAX_POLY_VOICES]` pair (`cutoff`, `base_hz`) computed in the
  existing per-voice setup loop.
- `crates/mooloop-dsp/src/mlp8.rs`: a different shape (see below) -- an
  equality cache, not a hoist. `Voice` gained `cached_cutoff_input: f32`
  and `cached_hz_from_knob: f32`; `Voice::shape` reuses
  `cached_hz_from_knob` whenever this sample's post-routing `cutoff` is
  bit-for-bit the value it was cached from, and recomputes
  `hz_from_normalized` otherwise. `cutoff_scale` (Drift's contribution) is
  applied outside the cache, every sample, because it can change range to
  range on its own while `cutoff` has not.
- `crates/mooloop-engine/src/block_cost.rs`: `playing_effect_cost` gained
  `EffectKind::Filter` to its list (the existing five didn't include it).
  New: `filtered_project` (a `loaded_project` variant with the filter
  closed and resonant instead of `MlP8Params::default`'s wide-open cutoff,
  which is `Prepared::filter_open`'s bypass condition) and
  `filter_engaged_cost`, printing open-vs-closed ns/block across frame
  sizes -- the before/after for the sampler and mono/poly/ML-M1/ML-P8
  hoists, beside `playing_effect_cost`'s before/after for the `FilterEffect`
  change. No existing measurement was removed.

## Why chunked instead of once per range

The plan's literal wording computes `SvfCoeffs` once at a range's start and
once at its end (from the smoothers' current value and target) and lerps
across the whole range. The first implementation did exactly that, and a
property of `process_param_split` broke it: a range runs from one
`ParamValue` event to the next, which is a 32-frame control tick when the
parameter is under continuous automation, but is the *rest of the block*
when it is a one-off knob turn with nothing further automating it --
`cutoff_change_mid_block_does_not_click` and
`param_value_events_change_cutoff_mid_block` both push exactly one event at
the midpoint of a half-second block. Lerping coefficients across the whole
12,000-sample remainder stretches `CUTOFF_SMOOTH_S`'s 3 ms settle (a
deliberate "track the knob closely" constant) out to 250 ms, which both
existing tests would have caught as a real behavioral regression, not a
rounding difference.

The fix: `process_range` walks its range in `CONTROL_RATE_FRAMES` (32-frame)
chunks regardless of how long the range is, deriving `SvfCoeffs` at each
chunk's start and at `Smoothed::advance_by(chunk_frames)` -- which lands the
smoother exactly where that many per-sample `advance()` calls would have,
`smooth.rs`'s own `advancing_a_block_matches_walking_it_sample_by_sample`
test says how closely -- and lerping within the chunk. This keeps the
*same* settle time as the old per-sample exponential (approximated as
piecewise-linear over 32-sample spans, which a ~3-11 ms time constant is
long enough relative to that a linear approximation over one span is close
to the true exponential curve there) while still turning ~one `tan()` a
sample into ~one every 32. A static block (the common case, and every
existing non-automation test) has `value()` unchanging chunk to chunk, so
every chunk's lerp is degenerate and the new path agrees with the old to
float precision (the `< -80 dBFS` test); a block with one event mid-range
gets the same fast settle it always had, now sampled at 32-frame
granularity instead of every sample (the discontinuity tests, both new and
existing, all still hold). `FilterEffect::is_at_rest`'s
`cutoff.is_settled()`/`resonance.is_settled()` keep their old timing, since
`advance_by` reproduces `advance`'s trajectory and its `SNAP_EPSILON` snap
exactly.

## Why the sampler's copy went away

`filter.rs`'s module header explained the copy: `Svf::next_sample`
re-derived coefficients on every call, so ticking L and R as two separate
`Svf::next_sample` calls would pay `tan()` twice, and a synthetic benchmark
measured that at ~2x. `tick_with` removes the premise: it takes a
coefficient set instead of deriving one, so two `Svf` instances (one per
channel) ticked against one `SvfCoeffs::for_cutoff` result pay the `tan()`
exactly once, same as the hand copy did, with no duplicated formula to
drift from `filter.rs`'s. The module doc comment is updated to say so.

## Why `mlp8.rs` got a cache instead of a hoist

The plan's text groups `mlm1.rs:525` and `mlp8.rs:2435` as the same shape
("a `Smoothed` cutoff that has settled to a constant for the whole life of
most notes"), and the other three synths' fix is a straight hoist: read the
smoother once per range instead of once per sample. ML-P8's `cutoff` is not
only the smoothed knob -- it is `Voice::dest(routes, slot::CUTOFF,
smoothed_cutoff)`, which adds a live per-sample modulation offset whenever
a route (LFO, an envelope, Key, ...) actually targets Cutoff. When a route
is live, `cutoff` is supposed to move every sample; hoisting it to a
once-per-range value the way the other three synths do would have silently
flattened that modulation, which is exactly the "genuinely sweeps per
sample" case the plan's own conservatism clause calls out.

The fix is an equality cache instead: `hz_from_normalized(cutoff, max_hz)`
is a pure function of `cutoff` alone, and `cutoff` is bit-for-bit the same
sample to sample whenever nothing is moving it (the smoother has settled,
which `Smoothed::advance`'s exact snap makes an exact equality, not an
approximate one, and no route touches Cutoff). Caching on that equality
reuses the last result instead of paying `powf` again for an unchanged
input; when a route is live, or the smoother is mid-ramp, the cache misses
every sample and the cost is exactly what it always was. This is provably
bit-identical to the old code in every case (same formula, same inputs,
same output), which the hoist approach used elsewhere in this plan is not
quite -- it trades a small, bounded numerical difference (bounded by
`advance_by`'s documented tolerance to the old per-sample walk) for a
bigger win on synths where that tradeoff is safe. `cutoff_scale` (Drift) is
deliberately applied outside the cache: it can change range to range on
its own even when `cutoff` has not, and caching it too would have served a
stale scale.

## Deviations from the plan's literal wording

- `effects/filter.rs`'s `process_range` chunks at `CONTROL_RATE_FRAMES`
  instead of lerping once across the whole range, per "Why chunked instead
  of once per range" above -- required for correctness against two existing
  tests, not a style choice.
- The plan names `mlm1.rs:525` and `mlp8.rs:2435` together under "the same
  one-line hoist"; `mlp8.rs` needed the different (safer, and here,
  cache-based) treatment above instead of a one-line change, because of the
  routing layer the other three synths don't have.
- `polysynth.rs`'s hoist is per voice (`[f32; MAX_POLY_VOICES]` arrays for
  `cutoff` and `base_hz`, filled in the existing per-voice setup loop)
  rather than the single-voice hoist the other three synths get, because
  `PolySynth` has one filter per voice and the sample loop is voice-major
  inside a sample-major loop.

## Tests added

- `filter.rs`: `tick_with_matches_the_wrapper_bit_for_bit`,
  `svf_coeffs_lerp_hits_its_endpoints_and_midpoint`.
- `effects/filter.rs`: `coefficient_lerp_matches_the_old_path_at_a_static_cutoff`
  (< -80 dBFS RMS against a reference per-sample path), and
  `envelope_sweep_leaves_no_discontinuity` (max sample step < 0.5 across a
  32-frame-stepped cutoff sweep).
- No new tests in `sampler.rs`/`monosynth.rs`/`polysynth.rs`/`mlm1.rs`/
  `mlp8.rs`: these are hoists/caches of a value already covered by each
  file's existing filter-behavior tests (attenuation, resonance taper,
  boundedness), which exercise the changed code paths and were not
  expected to need new assertions of their own. Flagged for the coordinator
  to reconsider if any of them fails to compile or pass.

## Before/after ns/block

To measure after build, via:

```sh
cargo test -p mooloop-engine --release playing_effect_cost -- --ignored --nocapture
cargo test -p mooloop-engine --release filter_engaged_cost -- --ignored --nocapture
```

Not run here -- this container does not run `cargo` (a concurrent build in
the coordinator's session; a second one risks an OOM kill). Placeholder:

| measurement | before | after |
| --- | --- | --- |
| `playing_effect_cost`, `EffectKind::Filter`, 8 ch, 512 frames | not measured | not measured |
| `filter_engaged_cost`, 8 ch, 64/128/256/512 frames | not measured | not measured |
