# ML-M1 plan status

In progress. 02-07 are in, restructured. 08's bank and its automated checks
are in. **Adam played the bank on 2026-08-31**, so 08's listening pass has
happened; his verdict was that the synth sounds very good. See "Findings from
the listening pass" for what it turned up and what is still open.

## The name, corrected 2026-08-31

The device was always meant to be the **ML-M1**. An agent misread the name as
"ML-1" when the device was created, it was not caught at the time, and it
shipped that way through the whole plan. Source, UI copy, and these documents
now say ML-M1.

**Three on-disk identifiers keep the old `ml1` spelling and must not be
"fixed".** Projects, channel presets, and preset directories were written
before the correction, and a serialized name is an on-disk identifier like a
parameter id:

- `ChannelSource::MlM1` is `#[serde(rename = "ml1")]` — this is the `type`
  field in every saved song and channel preset.
- `DeviceKind::MlM1` is `#[serde(rename = "ml1")]`, for `Channel::kind`.
- `kind_slug` returns `"ml1"`, the `presets/generators/ml1/` directory name,
  and `MARKER_FILE` stays `.ml1-factory-v1`.

`a_source_saved_under_the_old_ml1_name_still_loads` pins the reader against a
literal pre-rename manifest. The round-trip test alone cannot catch a break
here, because renaming both ends at once still passes it.

One consequence Adam should know: **factory patches already seeded to disk
keep their old `ML-1` category label.** They became ordinary user presets on
first run and the marker file stops re-seeding. Deleting the bank and its
`.ml1-factory-v1` marker re-seeds them under `ML-M1`; leaving them alone is
also fine.

## The restructure, decided 2026-08-30

Adam's call, and it changes the shape of the whole plan: **Mono v2 (ML-M1) and
ML-P8 are new instruments, not edits of the existing ones.** The original
poly synth is *kept* — it is a simple three-oscillator synth that sounds good,
and losing it is not worth it — and will get a mono/poly toggle and a legato
toggle so it covers the "sometimes that is all you need" case. The original
mono synth is deleted once its channels have somewhere to migrate to, which is
that toggled poly. Three synths end up in the project, not two.

Consequences for the steps below:

- **02 no longer "lands on both synths".** The v1 synths are untouched.
  `MlM1Params` is a new struct in `crates/mooloop-core/src/mlm1.rs` with
  `#[serde(default)]` from the start, so the "make `MonoSynthParams` safe to
  extend first" work in 02 is moot here and the pre-v2 migration in 02.5 does
  not apply — there is no pre-v2 form of this device on disk.
  ML-P8's plan folds its separate filter envelope and keytracking into
  `docs/plans/poly-synth-v2/03-the-multimode-filter.md`; ML-M1 step 02 is not a
  prerequisite.
- **The descriptor split happened differently.** `MONO_DESCRIPTORS` and
  `POLY_DESCRIPTORS` still have their inheritance (`POLY_DESCRIPTORS` copies
  `MONO_DESCRIPTORS`), because those are the v1 devices and are on their way
  out. What 02 asked for was done for the new table:
  `SYNTH_CORE_DESCRIPTORS` was split out of `SHARED_SYNTH_DESCRIPTORS`, and
  `ML1_DESCRIPTORS` is built from it rather than from either v1 table.
- **The v1 mono synth is still present and still loadable.** Deleting it is
  blocked on the poly toggles, since old projects' MonoSynth channels need
  somewhere to land — planned in `docs/plans/poly-v1-mono-mode/`. Until then
  the device picker shows both, as "Mono" and "ML-M1". Naming settles when the
  v1 device goes.
- **`DeviceKind::MlM1` is a transitional name.** It takes the plain name
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
- **08, the bank:** six patches in `crates/mooloop-core/src/mlm1_factory.rs`,
  defined as data rather than as files so the DSP tests and the preset seeder
  share one source of truth. `mooloop_project::seed_mlm1_bank` writes them into
  the user's channel-preset directory on first run, after which they are
  ordinary editable user presets. The whole-instrument checks the step asks
  for run over the shipped patches: peak bound at full velocity across each
  patch's register, prompt release on transport stop, no step when cutoff,
  resonance, drive or accent is jumped end-to-end mid-note, and Round Bass
  against Acid Line as two instruments.
- **The face:** `mlm1-device.slint`, three pages, the third being PERF.
  `OscillatorDeviceStrip` was extracted to `device-oscillator.slint` — the
  part of 07 that could not wait, since a third face would have been a third
  copy.

## What is not

**Voicing the remaining constants by ear.** The listening pass has now happened
and produced one concrete DSP fix (see below), but the step's framing was that
the Ladder's bass compensation, the Acid filter's resonance path, the Accent
depth constants and the pre-drive makeup gain are all "tune by ear". Only the
filter level matching has been touched. `ACCENT_ENV_SCALE`,
`ACCENT_DRIVE_PUSH`, `PRE_DRIVE_GAIN_RANGE` and the Ladder/Acid compensation
values still stand where measurement left them, as do the new makeup
constants.

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

## Findings from the listening pass, 2026-08-31

Adam played the bank and reported that the synth sounds very good, with one
DSP note: the filter models had "pretty different apparent loudness between
types". Measured across a cutoff/resonance grid with a 110 Hz saw at the
operating level, that was **10.8 dB at worst** (5 kHz, resonance 0.9), and it
turned out to be two separate defects.

### Fixed: resonance was moving level, in opposite directions per model

The two nonlinear models *lose* level as resonance rises, because their stages
only ever integrate a bounded shaper output and so compress as the feedback
path drives them harder. The linear `Svf` *gains* level from its resonant peak.
So the three diverge as the Resonance knob comes up, rather than merely sitting
at different offsets.

Neither existing guard could see it. `resonant_filter_and_drive_stay_bounded`
asserts only a peak ceiling, and the model-switch test measures the step at the
instant of switching, not the level either side of it — a steady 10 dB offset
passes both.

`VoiceFilter` now applies a resonance makeup shaped on each model's feedback
gain `k`, which already folds in cutoff tracking, so one term covers both axes.
Worst-case spread falls to **2.4 dB**, most of which is the honest slope
difference between a three-pole and a four-pole filter.

The compensation lives in the ML-M1 rather than in the filters, deliberately: a
`Ladder` used elsewhere should not arrive pre-trimmed to match two filters it
has never heard of, and `Svf` is shared with the v1 synths and the filter
effect, where changing its level would be a regression. `Ladder` and `Acid` now
publish `feedback_at()` so the host can read `k` instead of re-deriving it.

**The constants are a measured first pass and want an ear.** They are
`LADDER_MAKEUP_DB`/`KNEE`, `ACID_MAKEUP_DB`/`KNEE`/`STATIC_DB`, and
`CLEAN_MAKEUP_DB`/`KNEE` in `crates/mooloop-dsp/src/mlm1.rs`. Depth scales the
whole correction; knee sets how quickly it arrives as resonance opens.

### Not fixed: Acid's Cutoff knob means a different frequency

`ACID_POLE_COMPENSATION` is documented as making "the Cutoff knob mean one
frequency across every model", and it does not. Measured -3 dB corners, as a
fraction of the knob's nominal value:

| Model | Corner |
| --- | --- |
| Ladder | 0.68x |
| Clean (`Svf`) | 0.65x |
| Acid | **0.41x** |

Acid is about three quarters of an octave darker at the same setting, and the
constant's own formula, `1 / sqrt(2^(1/3) - 1)`, evaluates to 1.96 rather than
the 0.8 in the file.

Correcting it to 1.307 does line all three up, and it removes most of Acid's
standing level offset. **It also breaks the filter.** Acid's feedback, bass
compensation and Tape shaper are all voiced against the low corner: with the
corner moved, the resonance taper goes non-monotonic at 100 Hz and its range
collapses from about 12 dB to under 3, failing
`acid_resonance_climbs_smoothly_across_the_cutoff_range` and
`the_acid_sweeps_harder_than_the_ladder_at_the_same_settings`. A sweep of
`ACID_MAX_FEEDBACK` from 12.0 down to 4.5 recovers neither.

So 0.8 is not a typo — it is load-bearing, and lining the corners up means
re-deriving the filter rather than retuning a constant. Left alone, with the
standing offset absorbed by `ACID_MAKEUP_STATIC_DB` instead. Whether Acid
*should* track the others is also a taste question Adam has not been asked: a
darker, lower corner is part of what the model is for.

### Also corrected

`Acid`'s doc comment claimed "**No bass compensation**" while
`ACID_BASS_COMPENSATION = 0.25` was applied a few lines below. The constant was
added later and only its own comment was updated.

## 07 audit result

- The three synths share only DSP primitives, their oscillator front end, and
  the four voice conventions now in
  `crates/mooloop-dsp/src/synth_voice.rs`. Their note engines, filters,
  envelopes, and output calibrations remain local.
- `ML1_DESCRIPTORS` is built independently from the shared core and oscillator
  descriptor blocks. The v1 `POLY_DESCRIPTORS` still copies the v1 Mono table
  by design until the later v1 migration; ML-M1 parameter ids 20-29 do not enter
  either legacy table. A test walks every generator table and rejects duplicate
  ids within a device.
- The shared cutoff/resonance and oscillator descriptor defaults, plus Poly's
  spread default, were stale. They now match all three parameter structs, with
  a test pinning that contract. The descriptor ranges remain correct for both
  linear and nonlinear filters.
- The v1 Mono, ML-M1, and Poly validators cover every numeric parameter. Their
  bool and enum fields are valid by construction after deserialization and do
  not need numeric range checks.
- `OscillatorDeviceStrip` already lives once in `device-oscillator.slint` and
  is imported by all three faces. The ML-M1 copy and status text make no
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
  consequence of the ML-M1 having no device-local LFO by design, not a
  workaround. The cost is that the bank appears in the channel-preset menu
  alongside other device kinds rather than in the ML-M1-filtered generator menu.
- **06 asks for a `Theme.warning` fill on the Accent knob**, "matching Drive
  and the other character controls". No knob in the project uses that fill —
  Drive included — so it would have made Accent the only warning-coloured
  control rather than one of a set. Left on the default fill. If the character
  grouping is wanted, it should land as a pass over every character control at
  once.

## Findings from 08

The step says a patch needing fifteen precise settings means the defaults or
the ranges are wrong, and that this is the finding rather than the patch's
problem. `mlm1_factory::moves_from_default` counts, and a test holds every
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
  specific to the ML-M1 — it applied to every channel preset already savable —
  but the bank could not ship without fixing it. `rescope_modulation` runs on
  the channel-preset load path; kits are unaffected, since their channels land
  on the indices they were saved from.
- **The amplitude envelope defaults suit a pad better than a bass.** Several
  patches spend a move on `sustain` alone. Worth considering when the default
  patch is next revisited, though it is not obviously wrong — the default
  patch is also the gain reference.

Read 01 first regardless of which step you pick up.

Source spec: `~/Downloads/mooloop_synth_v2_spec.md`, sections 4, 5, 8-13.
