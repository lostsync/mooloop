# Status

Work 01 → 08 in order; 01 is the contract every other step refers to and
should be read first even when picking up a later step.

07 (reverb normalization) and 08 (metering) are independent of 05/06 and
can be done out of order if something else blocks.

## Step 02 — measured (done, `feat/gain-structure-02-measure`)

Characterization tests live in
`crates/mooloop-engine/src/gain_structure_tests.rs` (`gain_structure`
module, header points here). All values below are master peak, dBFS,
48 kHz offline render. When a step deliberately moves a number, re-run
`cargo test -p mooloop-engine gain_structure -- --nocapture` and update
this table in the same commit.

- **Source peak at unity** (default patch, default velocity, channel at
  0 dB, master bus):
  - Sampler (builtin kick): **-0.9**
  - DrumSynth (kick): **-0.2**
  - MonoSynth: **-5.1**
  - PolySynth: **-8.1**
- **Kick + snare** (Adam's case: default channel volumes, kick step 0,
  snare one beat later): **-2.1** (Adam measured -4.2 live; see note).
- **Channel summing** (N identical MonoSynth channels at unity):
  N=1: **-5.1**, N=2: **+0.9**, N=4: **+7.0**, N=8: **+13.0**. Honest
  summing already; crosses 0 dBFS between 2 and 3 channels.
- **Oscillator summing** (1 vs 3 oscillators at full): MonoSynth delta
  **+9.5 dB**, PolySynth delta **+9.5 dB** — exactly the honest ~9.5.
- **Reverb wet/dry** (bypass vs 0% vs 100% wet, same source):
  - Reverb: bypass -5.1, 100% wet **+4.6** → wet path **+9.7 dB above**
    the dry signal it blends against.
  - Plate: bypass -5.1, 100% wet **+6.6** → **+11.7 dB above**.
  - Confirms the peak-normalized IR (`reverb.rs` clamps IR peak to 0.42)
    as the cause; step 07 owns the fix.
- **Fader travel**: today's `MixerFader` travel IS linear gain
  (`mixer.slint` binds `value: strip.volume`), so 0.75 travel = **-2.5
  dB**. Asserted in `fader_travel_maps_linearly_to_gain_today`; step 04
  flips it to the taper table.

Note: the kick + snare render measures -2.1 against Adam's live -4.2.
Adam confirmed the -4.2 was just what the output meter held as a peak, so
the difference does not matter; the range assertion (-8..0) absorbs it.

## Step 03 — shared gain module (done, same branch)

- `crates/mooloop-core/src/gain.rs`: `MIN_DB`/`MAX_DB`,
  `db_to_linear`/`linear_to_db` (moved from `mooloop-ui/src/meter.rs`,
  semantics unchanged, tests moved with them), `MAX_LINEAR_GAIN`
  (`channel.rs` re-exports it; engine `MAX_TRIM_GAIN` deleted, clamps
  untouched), `FADER_BREAKPOINTS` + `fader_position_to_db`/
  `fader_db_to_position` (round-trip tested; travel between 0 and the
  -60 dB breakpoint holds `MIN_DB` — interpolating towards -inf dB is
  meaningless), `format_db`.
- `ui/gain.slint`: `GainMath` global mirroring all of the above. Slint
  1.17 has no `log10`; `log(x, 10)` is the form. -inf is spelled
  `-99999.0` and `format-db` renders it as `-inf`. `TrimKnob` and both
  `main.slint` `pow(10, v / 20)` sites now go through it.
- `crates/mooloop-ui/tests/gain_slint_agreement.rs`: parses the taper
  lists out of `gain.slint` and fails if they drift from
  `FADER_BREAKPOINTS`; also guards `TrimKnob` against growing a second
  formatter.
- Nothing audible changed (step-02 characterization tests pass
  unchanged). Verification: mooloop-core 57, mooloop-engine 76+34,
  mooloop-ui full suite, all green — run on the remote box via
  `scripts/antibox`.

## Step 04 — fader taper and dB readouts (done, same branch)

- `MixerFader` grew `in property <bool> db-taper` (default true): travel
  is linear in dB through `GainMath.fader-position-to-db`/
  `fader-db-to-position`; `value` stays the stored linear gain, so no
  project-format change. The relative drag still moves position. Travel
  ticks now sit on the taper's breakpoints (+6/0/-12/-24/-40 ends) when
  tapered, even spacing otherwise. `default-value` is 1.0.
- Mixer strips (`mixer.slint`) and the bus output stage
  (`bus-device.slint`) cap their throw at `GainMath.db-to-linear(6.0)`
  and label `GainMath.format-db(GainMath.linear-to-db(volume))` — the
  readout travels in the tooltip per the project convention; the
  snapshot shows caps at three-quarter travel for a unity bus.
- Oscillator Level knobs on `mono-device.slint` and `poly-device.slint`
  are now `TrimKnob`s in dB (`maximum: 0`, double-click default -60 =
  the stored default of silence), converting to linear at the boundary.
  `OscParams::level` stays linear in [0, 1]; what the defaults should be
  is step 06's.
- Gallery volume demos read dB now. `mockup.slint` stays percent: it is
  the design playground, its values are abstract mock data, not gains.
- Step-02's fader test flipped: travel 0.75 now reads 0 dB, full throw
  +6 dB. Kick+snare peak unchanged; nothing audible moved. Full
  mooloop-ui suite + mixer snapshot (via `scripts/antibox --pull`) pass.

## Step 05 — reference level and headroom (done, same branch)

`gain::REFERENCE_PEAK_DBFS = -12.0` added. Calibrations: `DrumSynth`
`OUTPUT_REFERENCE = 0.26` (the anchor shared by the character tables,
ratios kept), `MonoSynth`/`PolySynth` `VOICE_OUTPUT_REFERENCE = 0.36/0.51`
with the default patch now running one oscillator wide open
(`OscParams::level 0.8 → 1.0` in both defaults), builtin `default_kick`
generated at `0.278`. `Channel::new` volume is genuinely 1.0 now. No bus
or master default moved; no limiter added. Re-measured (same setup as
step 02):

- **Source peak at unity**: Sampler **-12.0**, DrumSynth **-11.9**,
  MonoSynth **-12.0**, PolySynth **-12.0** — all within a dB of the
  reference. One oscillator at full *is* -12.0 on both synths.
- **Kick + snare** (downbeat, unity): **-7.4** (plan predicted "somewhere
  near -9"; two hits at -12 summing on the downbeat can reach -6, so
  -7.4 sits inside honest-summing behaviour).
- **Channel summing**: N=1 **-12.0**, N=2 **-6.0**, N=4 **0.0**,
  N=8 **+6.0** — textbook 20·log10(N) growth, crosses 0 dBFS at ~4
  channels (was between 2 and 3). Eight channels at +6 sit inside the
  +12 dB clamp: the headroom is real.
- **Oscillator summing**: one at full **-12.0** → three **-2.5** on both
  synths, delta **+9.5 dB** — the contract's "-2.4 at three full
  oscillators" landed on the nose.
- **Reverb wet/dry**: bypass now -12.0; 100% wet peaks -2.4 (Reverb) /
  -0.3 (Plate), still +9.7/+11.7 dB above the dry path. Step 07's target,
  unchanged in relative terms.
- dsp 201, core 57, engine 76+34, all green. `docs/GAIN_STRUCTURE.md`
  written and added to AGENTS.md's task-context table.

Adam: this is the step to **listen** to — defaults are now 12 dB quieter
and the test suite can only confirm the numbers, not the musicality.

## Step 06 — oscillator summing (done, same branch)

- **Per-osc unity reference decided and documented** in
  `docs/GAIN_STRUCTURE.md`: an oscillator's 0 dB knob position *is* the
  device reference (the contract's preferred option). Verified against
  the three-oscillator case in step 05: one at full **-12.0**, three at
  full **-2.5** on both synths. No normalization by enabled-oscillator
  count; enabling osc 2 never moves osc 1.
- **Drive compensated**: `apply_drive` was peak-normalizing
  (`1/tanh(G)`), so a -12 dBFS oscillator got up to +24 dB louder as
  drive rose. It now normalizes by the shaper's own response at the
  operating level (`tanh(R·G)` with R = 0.251): a reference-level signal
  keeps its peak exactly at any drive setting, harmonics grow instead,
  and a full-scale peak saturates to the reference rather than to
  clipping. The sampler's inline copy of the old formula was replaced by
  the shared function. Drive defaults are 0 (bypass) everywhere, so
  nothing at rest changed — verified by the step-05 measurements
  re-running unchanged.
- **Device output trim**: already exists — the source `DeviceFrame` rail
  opts in (`output-trim-enabled: !editing-bus`) and binds it to channel
  volume, so every generator face has the knob. No code needed; noted
  here so the plan's question is answered.
- Tests: the plan's drive-compensation test added
  (`drive_changes_character_not_level_at_the_reference` — peak fixed,
  harmonic share grows); stale absolute-amplitude thresholds updated
  (`kick_decays_after_the_hit`, `drive_saturates_without_boosting_level`
  — the latter replaces a test that asserted the old +16x boost).
  mooloop-dsp 202, mooloop-engine 76+34, all green.

## Step 07 — reverb normalization and wet/dry (done, same branch)

- **IR energy normalization** replaces peak clamping
  (`reverb.rs::IR_ENERGY_TARGET = 1.2`): L2 norm across both channels.
  Measured 100% wet vs dry: kick (broadband) **+0.2 dB** — matched; the
  synth tone reads **+10.8 dB** because the diffuse tail's spectrum tilts
  low and tonal partials sample hot points of the response. That tilt is
  the generator's, not the normalizer's — the old IR measured +9.6 on the
  same probe. Documented in GAIN_STRUCTURE.md; the tonal probe is bounded
  wide in the test with the reason written next to it.
- **Plate** got a calibrated `OUTPUT_REFERENCE = 0.45` (comb-sum gain
  depends on decay/size/input spectrum; no natural unity): kick **-1.9
  dB**, tone **+4.8 dB** — balanced within ±5.
- **Host blend is equal-power** (`render.rs`): cos/sin gains computed
  once per block. New test proves a 50% blend of a decorrelated pair
  carries the average energy exactly (a linear fade would dip 3 dB). Two
  tests updated to express the new law: aligned dry+wet at 50% now
  recombines to √2 for a correlated path (the accepted trade-off, noted
  in GAIN_STRUCTURE.md), and Buffer Follow transparency is inaudible-
  exact rather than bit-exact (cos(π/2) dry leak).
- **Default blends re-picked**: reverb/plate open at **0.25** (was 0.35,
  chosen when wet sat ~10 dB hot), modulation stays 0.5. Existing
  projects keep their saved values; no migration per the contract.
- **Adam: the acceptance criterion is your ear at 1%, 10%, and 50% wet**
  — the numbers only prove the cause is gone. dsp 202, engine 77, core
  57, all green.
