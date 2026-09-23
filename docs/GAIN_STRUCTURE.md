# Gain structure

Standing documentation for how levels work in mooloop. The authority is
`crates/mooloop-core/src/gain.rs` — if this document and the code disagree,
the code wins and the document should be fixed. The decisions behind it were
made in `docs/plans/archive/gain-structure/01-the-gain-contract.md`.

## Operating level

**-12 dBFS is unity operating level** (`gain::REFERENCE_PEAK_DBFS`). A
generator's default patch, played at default velocity with its channel at
unity, peaks at approximately -12 dBFS. The 12 dB of headroom above it is
what lets sources sum without pulling the master down first.

Each generator calibrates itself against the reference:

- `DrumSynth`: `drumsynth::OUTPUT_REFERENCE`, the absolute anchor shared by
  the per-character balance tables.
- `Ds01`: `ds01::VOICE_OUTPUT_REFERENCE`, calibrated so the default patch —
  one tone layer at its default level, the device Level at 0.8, full velocity,
  and a shape stage that is *exactly* transparent at its defaults — lands
  where a default `DrumSynth` kick lands. Its shaper is separate from the
  reference on purpose: v1's one `OUTPUT_REFERENCE` constant is a mix
  decision, a character control and a safety bound at once, and DS-01 splits
  those into `PARAM_LEVEL`, the Drive/Character/Bias/Bits stage, and
  `ds01::device_bound`.
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
operating level, eight identical channels peak near +6 dBFS at the master
bus — inside the +12 dB clamp, audible on its meter, and the user's problem,
not the engine's. (What leaves the master is held at 0 dBFS by the output
guard below.)

**No fader touches another track.** The whole summing path is linear: a
channel's gain and pan, `StereoBus::add_from`, and the bus walk in
`RenderState::process_block` are all plain multiply-and-add, so rendering
two channels together gives exactly what rendering them apart and adding
gives, at every fader position. `summing_stays_linear_however_the_faders_sit`
holds that sample by sample up to the +12 dB ceiling. If one track ever
appears to duck another, nothing in the summing path can be responsible;
look for a shared *nonlinear* stage instead — a driven filter, the drive
effect, a compressor or limiter on a bus every source drains through, or
**the channel strip's own compressor or drive on such a track**, which since
2026-09-11 is one switch rather than a device somebody placed. Those
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

**Nothing bounds a sample in the live path — unless console summing is
switched on, or a strip's drive is.** The engine's only writes into a bus
buffer are the effect container's input trim, its wet/dry blend and output
trim, the dry-path delay ring, the channel strip's four sections, and
`StereoBus`'s add and multiply. Every one of them is linear in the signal
except the strip's **drive**, which bounds a sample twice over: its harmonic
shaper clamps its *input* to `[-1, 1]` before evaluating a polynomial, so a
driven sample stops growing at full scale, and the `Iron` voicing adds a slew
limit on top. Both are inside a section that is out until somebody switches
it in, and both are the point of the control rather than a side effect of it.
The strip's EQ and compressor are level-dependent but bound nothing: a biquad
and a gain are multiplies.

**The exception, and it is deliberate.** A strip switched to console summing
(`mooloop_dsp::console`) leaves through `sin` and is decoded at its
destination through `asin`, so **every summing point a console strip reaches
hard-limits at `PI/2`, which is +3.92 dBFS.** That bound *is* the effect
rather than a cost of it — Adam's ruling, 2026-09-09: *"you can do a perfectly
clean mix on it if you want but you could also drive it and get something nice
in return."*

Three things keep it honest rather than hidden:

- **Off is the default, and off is this document's linear path unchanged.**
  `summing_stays_linear_however_the_faders_sit` still holds sample for sample,
  and console-on gets its own tests rather than a tolerance added to that one.
- **A lone console strip is the identity**, measured at 1.5e-8 against a 0.251
  peak — about -156 dBFS — so switching one on and hearing nothing is correct.
  The character is entirely in the interaction between strips.
- **No hidden headroom trim.** The alternative to the bound was scaling the
  encode's input so the knee landed somewhere flattering, which is burying a
  gain, and it would also have removed the control that makes the drive
  playable. Which control that is, is worth stating: a bus's fader is *after*
  its decode, so **turning a bus down is volume and turning its feeders down
  is drive.**

The ceiling is visible rather than merely documented: a bus's input meter
reads the signal after the decode, so it stops climbing exactly where the
summing law stops.

**Per-oscillator unity reference.** A synth oscillator's 0 dB knob position
*is* the device reference: one oscillator at full peaks at
`REFERENCE_PEAK_DBFS`, three at full sum honestly to about -2.4 dBFS. The
alternative — scaling each oscillator to a third so three at 0 dB land at
unity — was rejected: it makes one oscillator quiet and its level dependent
on a design decision about oscillators the user is not using. Enabling a
second oscillator never changes the first one's level. The oscillator mix
is followed by compensated saturation (`apply_drive`), anchored at the
operating level, so raising drive changes character, not level.

All of that is about the mix, and it still holds at every bus including the
master. What changed on 2026-09-22 (MOO-93) is what happens *after* the
master: **the output guard**, below, stands between the master bus and the
driver, and it does bound a sample -- at 0 dBFS, and nowhere else. A mix under
0 dBFS passes it bit for bit, so everything this section says is still what
reaches the ports. Until then, sums above 0 dBFS reached the output device
intact and a 24-bit export hard-clipped them without a word.

## The output guard

`mooloop_dsp::output_guard`, run by `RenderState::process_block_inner` on the
master after everything else in the block -- the bus walk *and* the browser
preview -- so live playback and export both pass through it. Two jobs:

- **Non-finite samples become silence, and are counted.** A NaN or an
  infinity is replaced by zero before it reaches a port or a file. The count
  is a latched fault (`BusMeters::output_faults`, never cleared by a read) the
  interface reads, and an export reports it
  (`RenderSummary::non_finite_samples`). The master is scrubbed once more
  before the takes run, so a resample of the master never records one.
- **A safety limiter at 0 dBFS** (`OUTPUT_CEILING`). Engineering, not a
  musical device -- the master bus compressor is a separate item (MOO-13).
  Zero latency: instant attack, a 20 ms hold, a 150 ms release, both sides
  linked, and a final clamp at the ceiling. **Transparent below 0 dBFS**: its
  gain is held as a reduction from unity that is exactly zero at rest, and a
  frame is only multiplied while it is not, so a mix that never goes over
  leaves bit for bit (`a_signal_under_the_ceiling_passes_bit_identical`).
  After an over it releases back to exactly unity, and is bit-transparent
  again. No lookahead, because lookahead delays everything on the master --
  monitoring latency and every recording's alignment -- for a stage that
  should normally be doing nothing; the cost is that an over's first frame is
  shaped rather than ducked ahead of time.

**The master's meter reads the mix, before the guard.** A mix over 0 dBFS
still lights the master's clip latch while nothing over 0 dBFS leaves, which
is how a user learns the limiter is working rather than the limiter hiding it
(`the_master_meter_reads_the_mix_and_the_ports_read_the_ceiling`). A
non-finite sample reads as an infinite peak (`StereoBus::peak`), never as
silence, so a broken device lights the latch too and is not put to sleep by
the effect host's idle check.

An export reports what the guard did (`RenderSummary::overs`,
`non_finite_samples`) and logs it when either is non-zero. `pcm24`, the
24-bit WAV encoder in `mooloop-engine/src/offline.rs`, still clamps at full
scale, but the limiter has already held the signal there, so its own count
(`RenderSummary::clipped_samples`) is zero unless the limiter stopped doing
its job.

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
- Every control that moves a channel or track volume stops at the mixer
  fader's +6 dB (`FADER_MAX_GAIN`): the fader, the rack-row and rail knobs,
  and the strip's Volume descriptor, which lanes, routes and mapped hardware
  go through. The descriptor uses the fader's own taper (`ParamCurve::Fader`),
  so unity is three-quarter travel everywhere and a controller and the mouse
  never disagree about where a gain sits (MOO-131). The +12 dB clamp remains
  so a song saved hotter still loads at the gain it was saved at. Songs
  written under the old Linear volume curve are converted on load
  (`Project::migrate_linear_strip_volume`, marked by `strip_volume_taper`).
- Trim/gain knobs: -60 dB to +12 dB, unity default, `-inf` at the floor.
  The sampler's Output is the exception that proves the range rather than
  the default: same `TrimKnob`, same travel, same double-click-to-0 dB, but
  a fresh one starts at -9 dB for the reason above.
- Two dB/linear policies, both in `gain.rs`. `db_to_linear`/`linear_to_db`
  are for parameters and floor at `MIN_DB`: -60 dB and below is silence.
  `db_to_linear_unfloored`/`linear_to_db_unfloored` are for measured levels
  inside the dynamics stages, and have no dB floor (silence reads about
  -180 dB), so a detector told -70 dB hears -70 dB. They agree above -60 dB.
- Oscillator level: -inf to 0 dB — an oscillator never boosts past its
  device's reference.
- Every gain, trim, level, and fader reads through `gain::format_db`
  (Slint: `GainMath.format-db`): `-inf`, `±0.0 dB`, `+3.0 dB`, `-12.4 dB`.
  The tooltip carries the value; explanatory text belongs in the status
  bar.
- **A dB that is not a gain reads through `GainMath.format-plain-db`**:
  `-24.0`, `0.0`, `6.0`, and the unit appended by the caller. A threshold, a
  knee width, a gate's range and a limiter ceiling are settings rather than
  levels, so the `+` and the `-inf` floor would misreport them — but they
  still owe the reader one decimal place at a fixed width. Neither form is
  ever spelled `round(x * 10) / 10` in markup, because Slint drops a trailing
  zero and the field then changes width as the knob moves;
  `no_db_readout_rounds_for_itself` in `gain_slint_agreement.rs` is what
  holds that.
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
the SegmentedMeter/PeakMeter defaults read from there.

**Every meter in the application draws one continuous bar**, not LED
segments: `SegmentedMeter`'s `segments` defaults to 0, which means a bar, and
only `mockup-catalog.slint` still asks for a count. A colour still belongs to
a position on the scale rather than to the level, which for a bar means the
three zones are drawn at full height and the lit part is a *window* onto
them. Stretching one gradient to the lit height instead would turn the whole
bar red at 0 dBFS.

Ballistics: instantaneous attack, 1 s peak hold, and a fall rate the user
picks in Preferences > Appearance > Metering (`MeterBallistics` and
`FALLOFF_DB_PER_SECOND` in `mooloop-ui/src/meter.rs`). The default and middle
option is IEC 60268-18 digital peak, 20 dB in 1.7 s; the rows are labelled
with the rate itself, generated from the table so the label cannot drift from
what the meter does. The choice is persisted by name in `settings.toml` and
reaches the meters through the `MeterPrefs` Slint global, which the pump reads
once a tick -- the same shape `Motion` uses for panel animation. The clip
latch is a separate full-scale detector (≥ 0 dBFS) held **until it is
clicked**, not on a timer, and is not tied to the colour thresholds.
`clear_clip`'s own comment gives the reason: "A clip light that puts itself
out is a light that is off by the time anyone looks at the meter." This
paragraph said "2 s latch" until 2026-09-13; no such timer has ever
existed.

## Where things live

| Concern | Implementation |
| --- | --- |
| Conversions, reference level, taper, formatting | `mooloop-core/src/gain.rs` |
| Slint mirror of the same | `mooloop-ui/ui/gain.slint` (`GainMath`) |
| Rust/Slint taper agreement | `mooloop-ui/tests/gain_slint_agreement.rs` |
| Measured level pinning | `mooloop-engine/src/gain_structure_tests.rs` |
| Meter ballistics | `mooloop-ui/src/meter.rs` |
| The output guard: non-finite scrub and 0 dBFS safety limiter | `mooloop-dsp/src/output_guard.rs` |
| The guard end to end: ports, fault count, master meter | `mooloop-engine/src/output_guard_tests.rs` |
