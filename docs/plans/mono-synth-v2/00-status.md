In progress. 02 and 03 are in, restructured; 04-08 are not started.

## The restructure, decided 2026-08-30

Adam's call, and it changes the shape of the whole plan: **Mono v2 and Poly v2
are new instruments, not edits of the existing ones.** The original poly synth
is *kept* — it is a simple three-oscillator synth that sounds good, and losing
it is not worth it — and will get a mono/poly toggle and a legato toggle so it
covers the "sometimes that is all you need" case. The original mono synth is
deleted once its channels have somewhere to migrate to, which is that toggled
poly. Three synths end up in the project, not two.

Consequences for the steps below:

- **02 no longer "lands on both synths".** The v1 synths are untouched.
  `MonoV2Params` is a new struct in `crates/mooloop-core/src/mono_v2.rs` with
  `#[serde(default)]` from the start, so the "make `MonoSynthParams` safe to
  extend first" work in 02 is moot here and the pre-v2 migration in 02.5 does
  not apply — there is no pre-v2 form of this device on disk.
  `docs/plans/poly-synth-v2/00-status.md` names 02 as its prerequisite; that
  is no longer true, and Poly v2 will need its own equivalent.
- **The descriptor split happened differently.** `MONO_DESCRIPTORS` and
  `POLY_DESCRIPTORS` still have their inheritance (`POLY_DESCRIPTORS` copies
  `MONO_DESCRIPTORS`), because those are the v1 devices and are on their way
  out. What 02 asked for was done for the new table:
  `SYNTH_CORE_DESCRIPTORS` was split out of `SHARED_SYNTH_DESCRIPTORS`, and
  `MONO_V2_DESCRIPTORS` is built from it rather than from either v1 table.
- **The v1 mono synth is still present and still loadable.** Deleting it is
  blocked on the poly toggles, since old projects' MonoSynth channels need
  somewhere to land. Until then the device picker shows both, as "Mono" and
  "Mono 2". Naming settles when the v1 device goes.
- **`DeviceKind::MonoV2` is a transitional name.** It takes the plain name
  when `DeviceKind::MonoSynth` is deleted.

## What is in

- **02, for the new device:** separate amplitude and filter ADSRs, filter
  keytracking referenced to middle C and read off the gliding frequency, ids
  20-24, its own descriptor table, validator, and persistence round-trip. The
  gain-structure calibration test covers the new kind.
- **03:** `crates/mooloop-dsp/src/heldnotes.rs` — a fixed-size held-note stack
  in its own module, because the poly synth needs the same thing for its mono
  mode. Note priority, env trigger, and glide mode at ids 25-27. Fallback on
  note-off is a pitch change and never a retrigger.
- **The face:** `mono-v2-device.slint`, three pages, the third being PERF.
  `OscillatorDeviceStrip` was extracted to `device-oscillator.slint` — the
  part of 07 that could not wait, since a third face would have been a third
  copy.

## What is not

04-08, unchanged as written, except that they now apply to `MonoV2` rather
than to `MonoSynth`. In particular **drive is still post-filter**: moving it
ahead of the filter without the makeup-gain scheme 04 designs would change
loudness rather than character, so the two land together.

Read 01 first regardless of which step you pick up.

Source spec: `~/Downloads/mooloop_synth_v2_spec.md`, sections 4, 5, 8-13.
