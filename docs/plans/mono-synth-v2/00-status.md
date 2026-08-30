In progress. 02-06 are in, restructured; 07 and 08 are not started.

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
  `Ml1Params` is a new struct in `crates/mooloop-core/src/ml1.rs` with
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
  `ML1_DESCRIPTORS` is built from it rather than from either v1 table.
- **The v1 mono synth is still present and still loadable.** Deleting it is
  blocked on the poly toggles, since old projects' MonoSynth channels need
  somewhere to land. Until then the device picker shows both, as "Mono" and
  "Mono 2". Naming settles when the v1 device goes.
- **`DeviceKind::Ml1` is a transitional name.** It takes the plain name
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
- **04:** `PreDrive` sits between the mix and the filter, with RMS-matched
  makeup gain rather than peak-matched — a saturated wave carries more energy
  for the same peak, so peak matching let Drive add 4.2 dB. Its gain range is
  4.0 against `apply_drive`'s 15, because `PreDrive` sees a raw mix near unity
  while `apply_drive` is anchored at the -12 dBFS operating level; at 15 the
  shaper squared everything and oscillator level stopped changing the tone.
  The `Ladder` is four one-poles inside a `tanh` feedback loop.
- **05, opened up by Adam:** as many filters as are useful, so the Model
  switch has three — `Ladder`, `Acid`, `Clean`. 01's rule survives: it forbids
  an LP/BP/HP *response-shape* menu, and all three of these are low-pass. Every
  constant and the reason it is what it is are in the commit message for
  `feat(ml1): three filter characters behind one Model switch`.
- **06:** Accent, id 29, on the PERF page. Velocity is the carrier, as the
  plan requires — no new event type. Accent rides the *same smoothed
  `velocity_amp` the VCA uses*, which is what gives it per-note capture, the
  priority fallback's winning-note velocity, and the legato slide without any
  new state at all. It scales `filter_env_amount` by up to 4/3 and adds up to
  0.35 to the smoothed drive.
- **The face:** `ml1-device.slint`, three pages, the third being PERF.
  `OscillatorDeviceStrip` was extracted to `device-oscillator.slint` — the
  part of 07 that could not wait, since a third face would have been a third
  copy.

## What is not

07 and 08, unchanged as written, except that they now apply to `Ml1` rather
than to `MonoSynth`. 07's remaining work is the duplication `Ml1` inherited
from the v1 synths: `note_to_freq`, `MIN_GLIDE_S`, `STOP_RELEASE_S` and
`PARAM_SMOOTH_S` are still copied across `monosynth.rs`, `polysynth.rs` and
`ml1.rs`. 08 is the six factory patches and the listening pass, which is
expected to re-voice several of the constants above — `ACCENT_ENV_SCALE` and
`ACCENT_DRIVE_PUSH` included, since both were chosen against measurements
rather than against music.

## Deviations from the plan as written

- **06 asks for a `Theme.warning` fill on the Accent knob**, "matching Drive
  and the other character controls". No knob in the project uses that fill —
  Drive included — so it would have made Accent the only warning-coloured
  control rather than one of a set. Left on the default fill. If the character
  grouping is wanted, it should land as a pass over every character control at
  once.

Read 01 first regardless of which step you pick up.

Source spec: `~/Downloads/mooloop_synth_v2_spec.md`, sections 4, 5, 8-13.
