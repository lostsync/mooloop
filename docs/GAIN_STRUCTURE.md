# Gain structure

Standing documentation for how levels work in mooloop. The authority is
`crates/mooloop-core/src/gain.rs` — if this document and the code disagree,
the code wins and the document should be fixed. The decisions behind it were
made in `docs/plans/gain-structure/01-the-gain-contract.md`.

## Operating level

**-12 dBFS is unity operating level** (`gain::REFERENCE_PEAK_DBFS`). A
generator's default patch, played at default velocity with its channel at
unity, peaks at approximately -12 dBFS. The 12 dB of headroom above it is
what lets sources sum without pulling the master down first.

Each generator calibrates itself against the reference:

- `DrumSynth`: `drumsynth::OUTPUT_REFERENCE`, the absolute anchor shared by
  the per-character balance tables.
- `MonoSynth` / `PolySynth`: `VOICE_OUTPUT_REFERENCE` in each file; the
  default patch runs one oscillator at its 0 dB top, and one oscillator at
  that top *is* the reference.
- `Sampler`: the builtin `default_kick` is generated to match. Arbitrary
  user samples cannot be calibrated — matching them to the reference is
  what the channel trim is for.

Adam explicitly waived backwards compatibility for level changes: existing
projects got quieter, and no migration, compatibility flag, or version bump
was added for that.

## Summing

**Sum honestly, like gear.** No summing point normalizes by its input count
and nothing auto-attenuates as sources are added. N equal sources are up to
20·log10(N) dB louder than one, and should be: channels sum into a bus at
unity (`MixerBus::new`), buses sum into the master at unity. With the
operating level, eight identical channels peak near +6 dBFS — inside the
+12 dB clamp, audible, and the user's problem, not the engine's.

## Fader taper

`MixerFader` travel is **linear in dB**, piecewise between breakpoints held
in `gain::FADER_BREAKPOINTS`, interpolated in dB: unity at three-quarter
travel, +6 dB at full throw, -60 dB near the bottom, silence at 0. The
stored `value` is always linear gain; only the travel mapping is tapered.
`GainMath` in `ui/gain.slint` mirrors the table, and
`crates/mooloop-ui/tests/gain_slint_agreement.rs` fails if Rust and Slint
ever disagree.

Knobs do not use the fader taper — a knob's travel is already linear in dB
over its own range (`TrimKnob`, -60 to +12, unity default).

## Ranges and readouts

- Stored gains are linear: channel and bus volume clamp to
  `MAX_LINEAR_GAIN` (+12 dB). The dB is presentation; wire and project
  formats stay linear.
- Trim/gain knobs: -60 dB to +12 dB, unity default, `-inf` at the floor.
- Oscillator level: -inf to 0 dB — an oscillator never boosts past its
  device's reference.
- Every gain, trim, level, and fader reads through `gain::format_db`
  (Slint: `GainMath.format-db`): `-inf`, `±0.0 dB`, `+3.0 dB`, `-12.4 dB`.
  The tooltip carries the value; explanatory text belongs in the status
  bar.
- Blend controls (wet/dry, per-effect `mix`) are ratios, stay in percent,
  and are deliberately not gains.

## Metering

Peak metering in dBFS, floor -60 dBFS (`gain::MIN_DB`). Green below -10,
yellow -10 to -3, red above -3. Ballistics per IEC 60268-18 digital peak:
instantaneous attack, 20 dB fall in 1.7 s, 1 s peak hold. Implemented in
`mooloop-ui/src/meter.rs` (`MeterBallistics`).

## Where things live

| Concern | Implementation |
| --- | --- |
| Conversions, reference level, taper, formatting | `mooloop-core/src/gain.rs` |
| Slint mirror of the same | `mooloop-ui/ui/gain.slint` (`GainMath`) |
| Rust/Slint taper agreement | `mooloop-ui/tests/gain_slint_agreement.rs` |
| Measured level pinning | `mooloop-engine/src/gain_structure_tests.rs` |
| Meter ballistics | `mooloop-ui/src/meter.rs` |
