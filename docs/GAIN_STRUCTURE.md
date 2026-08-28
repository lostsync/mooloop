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

**No fader touches another track.** The whole summing path is linear: a
channel's gain and pan, `StereoBus::add_from`, and the bus walk in
`RenderState::process_block` are all plain multiply-and-add, so rendering
two channels together gives exactly what rendering them apart and adding
gives, at every fader position. `summing_stays_linear_however_the_faders_sit`
holds that sample by sample up to the +12 dB ceiling. If one track ever
appears to duck another, nothing in the summing path can be responsible;
look for a shared *nonlinear* stage instead — a driven filter, the drive
effect, a compressor or limiter on a bus every source drains through. Those
are level-dependent by design, they have no time constant when the shaper is
static, and no bus assignment escapes one sitting on the master.
What matters is placement, not the effect: a channel's chain runs on that
channel's own buffer and is only then added into its destination bus, so the
same device on a channel touches nothing else.
`a_shared_saturation_stage_is_what_ducks_one_track_under_another` measures
both: a filter at drive 0.6 on the master pulls the drums down 4.7 dB as the
pad's fader travels from unity to +12, and the same filter on the pad's own
channel moves them 0.00 dB. The bus walk itself adds nothing either: the
same superposition holds with the pad routed down a two-hop insert chain,
which is what puts audio through `mix_into`.

**Nothing bounds a sample in the live path.** The engine's only writes into
a bus buffer are the effect container's input trim, its wet/dry blend and
output trim, the dry-path delay ring, and `StereoBus`'s add and multiply —
every one of them linear in the signal. The single place a sample is
clipped anywhere in the codebase is `pcm24`, in
`mooloop-engine/src/offline.rs`: that is the 24-bit WAV encoder, so exports
hard-clip at full scale and live playback does not. Sums above 0 dBFS reach
the output device intact, and pulling them down is the user's business.

**Per-oscillator unity reference.** A synth oscillator's 0 dB knob position
*is* the device reference: one oscillator at full peaks at
`REFERENCE_PEAK_DBFS`, three at full sum honestly to about -2.4 dBFS. The
alternative — scaling each oscillator to a third so three at 0 dB land at
unity — was rejected: it makes one oscillator quiet and its level dependent
on a design decision about oscillators the user is not using. Enabling a
second oscillator never changes the first one's level. The oscillator mix
is followed by compensated saturation (`apply_drive`), anchored at the
operating level, so raising drive changes character, not level.

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

## Wet/dry and return effects

**Wet paths are level-matched to dry.** The convolution reverb energy-
normalizes its IR (`IR_ENERGY_TARGET` in `reverb.rs`); the plate sits
behind a calibrated output reference (`OUTPUT_REFERENCE` in `plate.rs`).
At 100% wet, broadband and percussive material lands within a few dB of
the dry signal. Sustained low-heavy tones can read hotter: a diffuse
tail's spectrum tilts low, so tonal partials sample a hotter point of the
response than the broadband average.

**The host blend is equal-power** (`render.rs`): `dry·cos(θ) + wet·sin(θ)`,
θ = wet·π/2. Correct for the decorrelated paths people actually blend
(reverb, chorus, delay); correlated ones — a filter or EQ at 50% mix —
sum up to ~3 dB hot where a linear fade was right. That trade-off is
accepted rather than adding a per-effect switch. Default blends in
`EffectSlotState::of_kind` are picked against the level-matched wet path:
reverb and plate open at 0.25, modulation at 0.5.

## Metering

Peak metering in dBFS, floor -60 dBFS (`gain::MIN_DB`). Green below -10
(`gain::METER_WARNING_DB`), yellow -10 to -3, red above -3
(`gain::METER_HOT_DB`); both thresholds are mirrored into `GainMath` and
the SegmentedMeter/PeakMeter defaults read from there. Ballistics per
IEC 60268-18 digital peak: instantaneous attack, 20 dB fall in 1.7 s,
1 s peak hold (`MeterBallistics` in `mooloop-ui/src/meter.rs`). The clip
latch is a separate full-scale detector (≥ 0 dBFS, 2 s latch) and is not
tied to the colour thresholds.

## Where things live

| Concern | Implementation |
| --- | --- |
| Conversions, reference level, taper, formatting | `mooloop-core/src/gain.rs` |
| Slint mirror of the same | `mooloop-ui/ui/gain.slint` (`GainMath`) |
| Rust/Slint taper agreement | `mooloop-ui/tests/gain_slint_agreement.rs` |
| Measured level pinning | `mooloop-engine/src/gain_structure_tests.rs` |
| Meter ballistics | `mooloop-ui/src/meter.rs` |
