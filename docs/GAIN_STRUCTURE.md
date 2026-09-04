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
- `MonoSynth` / `PolySynth` / `MlM1` / `MlP8`: `VOICE_OUTPUT_REFERENCE` in
  each file; the default patch runs one oscillator at its 0 dB top, and one
  oscillator at that top *is* the reference. The two mono voices are anchored
  at 0.36 and the two polyphonic ones at 0.51, because a polyphonic device's
  reference is set per voice against a chord rather than against one note.
- `Sampler`: the builtin `default_kick` is generated to match. Arbitrary
  user samples cannot be calibrated, so the sampler spends headroom instead
  of measuring: a fresh sampler's own output trim starts at
  `sampler::default_output_gain()`, which is
  `GENERATOR_OUTPUT_REFERENCE_DBFS` as a gain, so a normalized full-scale
  file peaks at the same -12 dBFS a default DrumSynth hit does. Nothing
  measures, peak-matches, or rewrites the audio — this is predictable
  headroom, not normalization. Loading or replacing a sample never moves the
  trim.

**Two references, 3 dB apart, and the difference is the pan law.**
`REFERENCE_PEAK_DBFS` (-12) is measured at the *master*, after a channel's
equal-power pan. `pan_gains(0.0)` is 0.707 a side, so a centred channel
spends `CENTRE_PAN_DB` (-3.01) that every generator pays equally, and a
calibrated generator's own output therefore peaks at
`GENERATOR_OUTPUT_REFERENCE_DBFS` (-8.99). A stage that has to place an
uncalibrated source level with the calibrated ones — the sampler's default
trim, the browser's audition monitor — targets the *device output* figure,
not the master one. Trimming to -12 instead would leave every loaded sample
3 dB under the synths. Parity then holds at any pan position, because pan
attenuates every channel identically.

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

## The sampler's output trim

The sampler stores `output_gain` linearly in `SamplerParams`, alongside the
rest of the patch, and applies it inside the device — last, after the
envelope and the drive/filter/lo-fi shaping, ahead of the channel's inserts.
It is the patch's level, not the mix's: the channel fader stays the
unity-default mix control, and the two are independently saved by channel
presets.

Three consequences worth stating, because each one is a decision:

- **Old projects keep their level.** The field deserializes to unity when a
  manifest predates it (`legacy_output_gain`), so a mix balanced against a
  sampler at unity still plays at unity. Only a sampler created after the
  field existed starts at -12 dB. `gain_structure_tests.rs` holds both cases.
- **The trim is a described parameter** (`SAMPLER_PARAM_OUTPUT_GAIN`), so
  automation and modulation reach it like any other. It is lagged through
  `Smoothed`, and one lag serves every voice: `render_range` hands each
  voice a copy and catches the original up once per segment, so a chord
  hears one trim rather than one per voice.
- **The browser's audition monitor starts at the same operating level**, by
  a different number. A preview is usually a full-scale commercial file, and
  unity would put it 12 dB over the project it is auditioned against. It
  uses `REFERENCE_PEAK_DBFS` rather than the sampler's generator output
  reference, because it plays straight to the master with no channel strip
  and so never pays the pan law the sampler's extra 3 dB cancels. Both
  arrive at -12 dBFS.

## Ranges and readouts

- Stored gains are linear: channel and bus volume clamp to
  `MAX_LINEAR_GAIN` (+12 dB). The dB is presentation; wire and project
  formats stay linear.
- Trim/gain knobs: -60 dB to +12 dB, unity default, `-inf` at the floor.
  The sampler's Output is the exception that proves the range rather than
  the default: same `TrimKnob`, same travel, same double-click-to-0 dB, but
  a fresh one starts at -9 dB for the reason above.
- Oscillator level: -inf to 0 dB — an oscillator never boosts past its
  device's reference.
- Every gain, trim, level, and fader reads through `gain::format_db`
  (Slint: `GainMath.format-db`): `-inf`, `±0.0 dB`, `+3.0 dB`, `-12.4 dB`.
  The tooltip carries the value; explanatory text belongs in the status
  bar.
- Blend controls (wet/dry, per-effect `mix`) are ratios, stay in percent,
  and are deliberately not gains.

## Wet/dry and return effects

**Wet paths are level-matched to dry, and the match is anchored on
sustained material.** Both reverbs sit behind a measured output reference
(`OUTPUT_REFERENCE` in `reverb.rs` and in `plate.rs`): a feedback network has
no natural unity, since its steady-state level depends on decay, size, and
where the input's energy sits against its modes, so the constant is measured
rather than derived. At 100% wet, a held note measured mid-sustain sits
within ~1 dB of dry, enforced by `steady_state_wet_path_is_level_matched`.

(The reverb was previously a convolution player and calibrated its impulse
response across spectral probes instead. That mechanism went with it; the
principle below did not.)

One scalar cannot match tonal and broadband material at once. The diffuse
tail's spectrum tilts low, so a narrowband partial samples a hotter point
of the response than the broadband average; whichever case the calibration
centers, the other lands several dB away. **We center the tonal case**,
because that is what sets perceived reverb level on the sustained material
a mix knob is usually ridden against. Broadband transients consequently
read a few dB under dry at 100% wet — correct, and unsurprising: a reverb
spreads a transient's energy across its tail rather than keeping it at the
onset.

Measure this on *steady-state energy*, never on whole-render peak or RMS.
Both flatter a reverb. Peak flatters it because a diffuse wet output has a
far lower crest factor than the dry transient it is compared against; a
plate that reads -5.7 dB on peak can sit at +1.3 dB on energy. Whole-render
RMS flatters it because the buildup and tail sit inside the window and pull
the average down. A wet branch several dB hot through the sustain passes
both — which is exactly how the reverb shipped at +4.7 dB over dry, making
1% wet audible and putting the mix knob at reverb/dry parity by 30%.

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
