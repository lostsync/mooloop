# ML-1 plan status

In progress. 02-07 are in, restructured. 08's bank and its automated checks
are in; **08's listening pass is not done and cannot be done without Adam** —
see "What is not".

## The restructure, decided 2026-08-30

Adam's call, and it changes the shape of the whole plan: **Mono v2 (ML-1) and
ML-P8 are new instruments, not edits of the existing ones.** The original
poly synth is *kept* — it is a simple three-oscillator synth that sounds good,
and losing it is not worth it — and will get a mono/poly toggle and a legato
toggle so it covers the "sometimes that is all you need" case. The original
mono synth is deleted once its channels have somewhere to migrate to, which is
that toggled poly. Three synths end up in the project, not two.

Consequences for the steps below:

- **02 no longer "lands on both synths".** The v1 synths are untouched.
  `Ml1Params` is a new struct in `crates/mooloop-core/src/ml1.rs` with
  `#[serde(default)]` from the start, so the "make `MonoSynthParams` safe to
  extend first" work in 02 is moot here and the pre-v2 migration in 02.5 does
  not apply — there is no pre-v2 form of this device on disk.
  ML-P8's plan folds its separate filter envelope and keytracking into
  `docs/plans/poly-synth-v2/03-the-multimode-filter.md`; ML-1 step 02 is not a
  prerequisite.
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
- **08, the bank:** six patches in `crates/mooloop-core/src/ml1_factory.rs`,
  defined as data rather than as files so the DSP tests and the preset seeder
  share one source of truth. `mooloop_project::seed_ml1_bank` writes them into
  the user's channel-preset directory on first run, after which they are
  ordinary editable user presets. The whole-instrument checks the step asks
  for run over the shipped patches: peak bound at full velocity across each
  patch's register, prompt release on transport stop, no step when cutoff,
  resonance, drive or accent is jumped end-to-end mid-note, and Round Bass
  against Acid Line as two instruments.
- **The face:** `ml1-device.slint`, three pages, the third being PERF.
  `OscillatorDeviceStrip` was extracted to `device-oscillator.slint` — the
  part of 07 that could not wait, since a third face would have been a third
  copy.

## What is not

**08's listening pass.** The step's own framing is that the Ladder's bass
compensation, the Acid filter's resonance path, the Accent depth constants and
the pre-drive makeup gain are "tune by ear", and that tuning them against six
concrete patches is what turns them from plausible to right. That has not
happened, and no constant in 04, 05 or 06 was changed during this pass.

This is a limitation of who did the work, not an oversight. The bank was
voiced against measurements — the same brightness and peak proxies the earlier
steps used — because that is the only instrument available to an agent that
cannot hear the output. So `ACCENT_ENV_SCALE`, `ACCENT_DRIVE_PUSH`,
`PRE_DRIVE_GAIN_RANGE` and the Ladder/Acid compensation and feedback values
still stand where measurement left them, and every patch below is a first
draft in the same sense.

What the bank does give is the thing the plan wanted it for: six real patches
that exercise the constants in combination, so that when Adam does play them,
what needs moving should be obvious and the file to move it in is small.
Anything changed then still needs writing back into 04, 05 or 06 with the
reason, per the step.

## 07 audit result

- The three synths share only DSP primitives, their oscillator front end, and
  the four voice conventions now in
  `crates/mooloop-dsp/src/synth_voice.rs`. Their note engines, filters,
  envelopes, and output calibrations remain local.
- `ML1_DESCRIPTORS` is built independently from the shared core and oscillator
  descriptor blocks. The v1 `POLY_DESCRIPTORS` still copies the v1 Mono table
  by design until the later v1 migration; ML-1 parameter ids 20-29 do not enter
  either legacy table. A test walks every generator table and rejects duplicate
  ids within a device.
- The shared cutoff/resonance and oscillator descriptor defaults, plus Poly's
  spread default, were stale. They now match all three parameter structs, with
  a test pinning that contract. The descriptor ranges remain correct for both
  linear and nonlinear filters.
- The v1 Mono, ML-1, and Poly validators cover every numeric parameter. Their
  bool and enum fields are valid by construction after deserialization and do
  not need numeric range checks.
- `OscillatorDeviceStrip` already lives once in `device-oscillator.slint` and
  is imported by all three faces. The ML-1 copy and status text make no
  hardware-emulation claim. `docs/AUDIO_ARCHITECTURE.md` did not describe a
  shared synth voice architecture, so it needed no change.

## Deviations from the plan as written

- **08's "101 Pluck" ships as "Snap Pluck".** The plan's table names it after
  a specific piece of hardware, and no user-facing string in this project may
  claim that lineage. The patch is unchanged in intent: fast filter decay,
  heavy keytrack, focused mono response.
- **08's bank is channel-scoped, not generator-scoped.** A generator preset is
  a bare `ChannelSource` with nowhere to put a `ModRack`, and Sequence Bleep is
  an S&H LFO routed to cutoff — it is nothing without one. That is a
  consequence of the ML-1 having no device-local LFO by design, not a
  workaround. The cost is that the bank appears in the channel-preset menu
  alongside other device kinds rather than in the ML-1-filtered generator menu.
- **06 asks for a `Theme.warning` fill on the Accent knob**, "matching Drive
  and the other character controls". No knob in the project uses that fill —
  Drive included — so it would have made Accent the only warning-coloured
  control rather than one of a set. Left on the default fill. If the character
  grouping is wanted, it should land as a pass over every character control at
  once.

## Findings from 08

The step says a patch needing fifteen precise settings means the defaults or
the ranges are wrong, and that this is the finding rather than the patch's
problem. `ml1_factory::moves_from_default` counts, and a test holds every
patch under fifteen. Three things came out of getting there:

- **There was no factory-bank mechanism at all.** Presets only existed as
  user-saved bundles in the config directory. Seeding on first run was chosen
  over merging a read-only factory directory into every scan, because it is
  much the smaller change — nothing in the browser, the loader, or the on-disk
  format learns about a second class of preset — and it leaves the patches
  editable, which for a starting point is the point. A marker file makes the
  bank non-self-healing on purpose, so deleting a patch you do not want keeps
  it deleted.
- **Modulation routes did not survive being loaded onto a different channel.**
  A `ModRoute` names its destination channel absolutely, which is right for a
  project and wrong for a preset. A channel preset saved from channel 3 and
  loaded onto channel 0 kept modulating channel 3. Pre-existing, and not
  specific to the ML-1 — it applied to every channel preset already savable —
  but the bank could not ship without fixing it. `rescope_modulation` runs on
  the channel-preset load path; kits are unaffected, since their channels land
  on the indices they were saved from.
- **The amplitude envelope defaults suit a pad better than a bass.** Several
  patches spend a move on `sustain` alone. Worth considering when the default
  patch is next revisited, though it is not obviously wrong — the default
  patch is also the gain reference.

Read 01 first regardless of which step you pick up.

Source spec: `~/Downloads/mooloop_synth_v2_spec.md`, sections 4, 5, 8-13.
