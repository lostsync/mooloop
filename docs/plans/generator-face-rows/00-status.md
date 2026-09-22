# Generator face rows status

Plan E of `reports/fable-2026-09-22.md` (finding 4), landed as a pilot on one
face: Aux In, the smallest generator. Written from a Claude Code web
container, where the hard constraint was no Cargo command at all -- another
agent was compiling concurrently on a 15 GB/no-swap box -- so every check
below is `scripts/slint-sketch` or a read; nothing has been compiled or run.

## What landed

- `crates/mooloop-ui/ui/main.slint`: a `SourceRow` struct (`kind`, `p0..p35`
  -- sized to the sampler's 36 descriptor ids, `SAMPLER_PARAM_*` in
  `mooloop-core/src/generator.rs`, per the plan's instruction to check the
  sampler's table even though only Aux In is wired), `in property <SourceRow>
  source`, and `callback source-param-changed(int, float)` on `MainWindow`.
  Removed the flat `in-out property <float> aux-in-level` and `callback
  aux-in-level-changed(float)`. Aux In's instantiation now binds `level:
  root.source.p2` (`aux_in::PARAM_LEVEL` is id 2) and forwards
  `level-changed(v) => { root.source-param-changed(2, v); }`.
- `crates/mooloop-ui/ui/aux-in-device.slint`: the Level knob dropped its
  explicit `minimum: 0; maximum: GainMath.db-to-linear(12.0);` and now works
  normalized 0..1, like every effect knob with no stated range. Its
  `default-value` moved from the natural default (`0.3552344`) to that
  default's normalized position (`0.0888086`). The `level`/`level-changed`
  names inside the component are unchanged -- only what feeds and reads them
  at the `main.slint` instantiation site moved.
- `crates/mooloop-ui/src/lib.rs`: `refresh_aux_in` now builds one `SourceRow`
  (`p2` from `aux_in::descriptor(PARAM_LEVEL).to_normalized(params.level)`)
  instead of `window.set_aux_in_level(..)`. The three Aux In closures became
  two pickers plus one shared `on_source_param_changed(id, normalized)`
  handler -- the DS-01 shape (`on_ds01_value_changed`) applied to the
  generator-wide callback finding 4 asks every face to eventually share,
  resolving `id` through `aux_in::descriptor` and writing through
  `GeneratorParams::AuxIn(..).set(id, descriptor.from_normalized(normalized))`.
  The old `on_aux_in_level_changed` handler is gone.
- `crates/mooloop-ui/tests/slint_face_agreement.rs`: `the_aux_in_face_agrees_
  with_its_table` now checks the normalized-knob shape (mirrors
  `every_effect_face_knob_agrees_with_its_table`'s `(None, None)` branch). A
  new `the_aux_in_face_reads_and_writes_level_by_id` is the read+write id
  check for Aux In that `every_face_reads_its_parameters_by_id` /
  `the_buffer_face_sends_descriptor_positions_for_edits` are for the effects
  -- the Reverb-incident shape, applied here.
- `crates/mooloop-ui/tests/source_snapshot.rs`: `render_aux_in_source_editor`
  builds a `SourceRow` instead of calling `set_aux_in_level`.
- `scripts/dupe-audit`'s `unchecked-face` list: untouched. Aux In's file was
  already `include_str!`'d in `slint_face_agreement.rs` before this pilot, so
  it was never on that list.

## Not touched

`scripts/dupe-audit` needed no edit (see above). No other file outside
`crates/mooloop-ui/` changed. Every other generator face (Sampler, Drum,
Mono, Poly, ML-M1, ML-P8, DS-01) still reads its own flat window properties;
`source` and `source-param-changed` are dead weight for them today, exactly
as the plan intends for a pilot.

## Measurements

- **Generated module line count, before/after**: to measure after build.
  `slint_build::compile("ui/main.slint")` produces it at build time
  (`docs/OPERATIONS.md`, `docs/plans/egui-view-layer/00-status.md`), so it
  cannot be counted without a `cargo build -p mooloop-ui`, which this session
  could not run.
- **`MainWindow` member count, before/after**: to measure after build, for
  the same reason -- finding 4's 567/944 counts were a script's count against
  compiled output, not a source grep. What is known without building: this
  pilot removed two `MainWindow` members (`aux-in-level`,
  `aux-in-level-changed`) and added two (`source`, `source-param-changed`),
  so the net member count is unchanged; the new `SourceRow` struct itself
  adds 37 fields' worth of generated accessors that were not there before,
  which is the cost this pilot pays to prove the shape out before the other
  seven faces pay it for real.

## Next, if the pilot is judged clean

The other seven generator faces, by descriptor count (ascending, so the
smallest risk goes first): Drum, Mono, Poly, ML-M1, then the wider ones. Two
notes for whoever picks this up:

- **DS-01 (92+ ids, some via a `PARAM_MATRIX_BASE = 100` row scheme) and the
  sampler (36 ids) are both wider than `SourceRow` as sized here.** DS-01
  already routes writes through one `(id, normalized)` callback
  (`ds01-value-changed`) but still reads its ninety-plus values from flat
  window properties, not `source.pK`; folding it into `SourceRow` means
  either widening the struct well past 36 or giving DS-01's matrix rows their
  own array property the way `EffectSlotRow`'s EQ bands get `eq-band-data`
  rather than `pK` slots. Decide that before wiring DS-01, not while doing
  it.
- **Sampler last, per the plan** -- it is the widest face with the
  established flat/`wire_*!` shape and the one every `refresh_editor` line
  count in finding 4 was measured against.

## Deviations from the plan text, and why

- **Aux In's Level moved from natural units (linear gain, 0..`MAX_LINEAR_
  GAIN`) to normalized 0..1.** The plan's step 1 says `SourceRow` should be
  filled "as `effect_slot_row` does," and `effect_slot_row` fills `p0..p17`
  normalized; DS-01's existing `on_ds01_value_changed` -- the one other
  generator-wide `(id, value)` callback in the tree -- also carries
  normalized values and converts with `descriptor.from_normalized`. Keeping
  Aux In's knob in natural units would have made `source-param-changed`
  disagree with its only precedent about what its own second argument means,
  which is a worse trap than the one it exists to close. This is why
  `the_aux_in_face_agrees_with_its_table` needed rewriting rather than
  staying untouched -- flagged here because it is the one existing test this
  pilot's markup change made stale, not just extended.
