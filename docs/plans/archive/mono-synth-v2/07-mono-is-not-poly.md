# Mono is not Poly

A cleanup step, run after 02-06 are in and before the listening pass. It has
no user-visible behaviour of its own; its job is to make the divergence real
in the code rather than accidental.

## Why now

By this point `MonoSynthParams` has Accent, three note-engine enums, and a
filter model; `PolySynthParams` has none of them and will grow Unison, Drift,
and Chorus instead. Several places in the codebase were written when the two
were interchangeable and still assume it. Left alone, the next feature on
either device is written against a structure that is quietly lying.

## Do this

### 1. Audit what is still shared and confirm it should be

Check each and record the verdict here:

- `OscParams`, `OscWave` — **shared, correct.** Both instruments genuinely
  have the same three-oscillator front end. The present device-local
  `LfoParams`/`LfoWave` state is migrated to the channel `ModRack`; it must
  not remain a shared synth feature.
- `Adsr`, `Osc`, `Svf`, `Smoothed`, `Lfo`, `pan_gains`, event types —
  **shared, correct.** `Lfo` remains a shared primitive for channel modulator
  sources, not synth-owned state. Primitives are the whole point.
- `osc_descriptors()` and genuinely shared source-descriptor entries —
  **shared, correct.** Modulator descriptors belong to `ModRack` instead.
- The ADSR/cutoff/resonance/env-amount descriptor entries — shared today.
  Confirm the ranges and defaults are still right for both, since Mono's are
  now voiced against nonlinear filters.
- `note_to_freq` — duplicated verbatim in `monosynth.rs:33` and
  `polysynth.rs:27`. It was harmless when the files were mirrors; now it is
  two copies that can drift. Move it to a shared module.
- `MIN_GLIDE_S`, `STOP_RELEASE_S`, `PARAM_SMOOTH_S` — same duplication, same
  answer.

### 2. Kill the descriptor-table inheritance for good

Step 02 split `MONO_DESCRIPTORS` and `POLY_DESCRIPTORS`. Verify nothing
reintroduced a copy of one into the other, that both are still const-built and
`static`, and that the ID space has no collision between Mono's 20-29 and
whatever the Poly plan has claimed. Add a test that walks both tables and
asserts IDs are unique within each.

### 3. Check the validators

`validate_mono_synth` (`crates/mooloop-project/src/lib.rs:1024`) and
`validate_poly_synth` (`:1118`) are near-duplicates today. Every field added
by 02-06 needs a range check in the Mono one. Do not merge them — they are
about to be genuinely different — but do make sure neither has gone stale.

### 4. UI divergence

`mono-device.slint` and `poly-device.slint` share `OscillatorDeviceStrip` by
copy, not by import — the same component is defined in both files. Extract it
  to `controls.slint` or a shared device module. The AMP/FILTER and
  performance pages are now genuinely different between the two faces and
  should stay separate files; both expose channel modulation through the
  common frame.

Confirm no docs copy, tooltip, or status-bar string claims Moog, Roland,
SH-101, or TB-303 emulation, per 01.

### 5. Documentation

Update `docs/CURRENT.md` for Mono's new surface, and `docs/AUDIO_ARCHITECTURE.md`
if it describes the shared voice architecture the two synths no longer have.

## Done when

- No duplicated free function or constant between `monosynth.rs` and
  `polysynth.rs`.
- `OscillatorDeviceStrip` is defined once.
- Descriptor ID uniqueness is asserted by a test for both devices.
- Both validators cover every field of their struct.
- `cargo check` and the full test suite pass; no behaviour change.

## Completion note

Completed after the plan's three-instrument restructure. The audit applies to
ML-M1 as the new mono instrument; the v1 Mono remains temporarily for project
compatibility. Detailed verdicts are recorded in `00-status.md`.
