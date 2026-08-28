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
