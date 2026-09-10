# The comparison data set — what is in it and how to read it

A second, wider pass over the same rig as [RESULTS.md](RESULTS.md). That one
answered questions about mooloop's own modules and stopped when they were
answered. This one is a survey: **106 units**, measured on shared axes, for a
reader deciding what to reach for.

Raw measurements are in `out/chart/`, one JSON per unit per family. The
tables meant for plotting are in `out/tidy/`, long-format CSV, one row per
measured point. `export.py` derives the second from the first and measures
nothing; re-run it any time.

Nothing here is a conclusion. `RESULTS.md` is where the first pass's readings
are written up; this file describes the data and the traps in it.

---

## Levels and references

Amplitudes are **dBFS peak** throughout.

Two reference levels matter and they are not the same one. The plugins assume
the studio convention, **-18 dBFS = 0 VU = +4 dBu**, which is what their
headroom controls calibrate and where the hardware they model is designed to
sit. **mooloop's unity operating level is -12 dBFS**, six dB hotter. Summary
rows are quoted at -12 dBFS; the level sweeps run -60 to +3 so either
convention can be read off.

Sample rate is 48 kHz. Analysis blocks are 65,536 samples, about 0.73 Hz per
bin, with 16,384 samples discarded first so filters and programme-dependent
stages have settled.

## Frequency grids

One grid per purpose, shared by every unit, so two units are always sampled
in the same places and a chart can subtract them.

| grid | points | used for |
| --- | --- | --- |
| `analysis.CURVE_FREQS` | 73, 1/24 decade, 20 Hz–20 kHz | every response and EQ curve |
| `analysis.THIRD_OCTAVE` | 31, ISO 266 preferred centres | noise spectra, harmonics against frequency, compressor sidechain |
| `CHECK_FREQS` | 9 | the tone-by-tone check on each swept response |
| `ctiming.TIME_GRID_MS` | 15, log, 0.1 ms–5 s | compressor attack and release trajectories |

## The two comparisons the settings are built around

**Equal input level.** Every unit is measured at its own settings across a
common level sweep. This answers "what do I get if I insert it and play".

**Equal distortion.** Every unit with a continuous drive control is *also*
measured at the setting where it makes **0.1%, 1% and 3% THD** at 1 kHz,
-12 dBFS, found by bisection. Those settings appear as
`matched_thd_0.1_pct`, `matched_thd_1_pct`, `matched_thd_3_pct` in the
`setting` column. This is the comparison none of the controls offer: "Drive
6" means nothing across two makers and 1% THD means the same thing
everywhere, so what is left on the chart is character rather than amount.

A unit that cannot reach a target is recorded as `reached: false` in
`thd_matched` and gets no matched setting, rather than having its nearest
miss dressed up as one.

## Families

| family | units | script | output |
| --- | --- | --- | --- |
| `colour` | 42 preamps, console channels and saturators | `measure_colour.py` | `out/chart/colour/` |
| `tape` | 9 tape machines, on the same axes as the preamps | `measure_colour.py` | `out/chart/colour/` |
| `eq` | 20 equalisers, at matched band targets | `measure_eqc.py` | `out/chart/eq/` |
| `comp` | 35 compressors, at four depths | `measure_compc.py` | `out/chart/comp/` |
| — | the 20 units with a noise model, in and out | `measure_noise.py` | `out/chart/noise/` |

The 106 units fall into 33 `group`s, and the groups are the point: four
emulations of an 1176, three of an LA-2A, two of a dbx 160, three SSL bus
compressors, two Fairchilds, two Massive Passives, two Pultecs, three API
550s. "Do these agree?" is a `GROUP BY` rather than a judgement.

Tape is deliberately not a separate suite. A Studer and a 1073 are things a
reader is choosing *between*, so they get the same response, THD, harmonic
and transient measurements and the tape machines get wow and flutter on top.

## The tidy tables

### Colour and tape

| file | one row per | the chart it is for |
| --- | --- | --- |
| `colour_response.csv` | unit × setting × level × frequency | frequency response from a swept sine — dense, with phase and group delay |
| `colour_response_by_tone.csv` | unit × setting × level × frequency | the same response measured tone by tone — coarser, and right for a unit the sweep cannot handle |
| `colour_thd_vs_level.csv` | unit × setting × frequency × level | **THD against level** — the headline; where a unit starts to break up |
| `colour_harmonics_vs_freq.csv` | unit × setting × frequency × level | harmonic tilt: warm on a kick, clean on a hat |
| `colour_imd.csv` | unit × setting × test × level | intermodulation, SMPTE and CCIF |
| `colour_noise.csv` | unit × setting × condition × band | noise floor by third octave |
| `colour_transients.csv` | unit × setting × level | crest retention and the slew index |
| `tape_wow_flutter.csv` | unit × setting × modulation rate | speed stability, and **at what rate** — two machines with the same 0.05% figure wobble differently |
| `colour_summary.csv` | unit × setting | one row of headline numbers per setting |

`colour_summary.csv` is the table to start from. `headroom_1pct_dbfs` is the
input level at which the unit reaches 1% THD, interpolated between measured
levels — the single number that puts a console channel, a tape machine and a
clipper on one axis. `tilt_db` is how much further down the 2nd harmonic is
at 5 kHz than at 80 Hz.

**Read `tilt_db` only where `tilt_valid` is true.** A symmetrical shaper
makes no measurable 2nd harmonic at either end of the band, and the
difference between two readings on the -200 dB analysis floor is floor
noise — Airwindows Desk4 computes to -88 dB of tilt out of two numbers that
are both nothing at all. Of the 28 units that reach 1% THD, 15 have a
2nd harmonic above -120 dB at both ends and a tilt worth plotting.

### EQ

| file | one row per | notes |
| --- | --- | --- |
| `eq_curves.csv` | unit × band × gain × frequency | **referenced to the unit's own flat curve**, not to 1 kHz |
| `eq_band_shape.csv` | unit × band × gain | peak height, peak frequency, Q, bandwidth in octaves |
| `eq_distortion.csv` | unit × band × frequency × level | harmonics with a band boosted, and flat |

Each unit is asked for the same seven bands — `low_shelf` and `low_bell` at
100 Hz, `lomid_bell` at 500, `mid_bell` at 1 k, `himid_bell` at 3.3 k,
`high_bell` and `high_shelf` at 10 k — at the **nearest frequency it can
actually reach**, which is in `actual_hz` beside `target_hz`. An API 550A's
mid band stops at 400 and 800 Hz; a 1073's mid starts at 360. Neither is
rounded into the target and both are labelled.

Gains follow the shared list where a unit's markings are in dB. Where they
are not — a Pultec's Boost runs 0 to 11, a Massive Passive's 0 to 20, a
Kilohearts tone control reports a string — the unit's own markings are used,
`marking` says which, and `eq_band_shape.csv`'s `peak_db` is the dB it
actually delivered.

### Compressors

| file | one row per | notes |
| --- | --- | --- |
| `comp_calibration.csv` | unit × depth | what control setting produced 3, 6, 10 and 15 dB of reduction |
| `comp_static_curve.csv` | unit × depth × level | threshold, knee and the ratio actually delivered |
| `comp_timing.csv` | unit × depth × marking | attack and release, t63 and t90 |
| `comp_envelope.csv` | unit × marking × phase × time | the whole trajectory, which is what tells an exponential from a two-stage opto recovery |
| `comp_programme_dependence.csv` | unit × depth × hold | the same release after a 30 ms note and a 3 s one |
| `comp_detector.csv` | unit × depth | peak or RMS, asked as a question about duty cycle |
| `comp_sidechain.csv` | unit × depth × frequency | how much reduction the same level makes at each frequency — whether the unit hears bass |
| `comp_harmonics.csv` | unit × depth × frequency | the gain element's own colour, ranked last on purpose |
| `comp_summary.csv` | unit × depth | headline numbers |

Every unit is driven to the same gain reduction before anything is timed,
because the units do not agree on what the compression control even is —
Peak Reduction, Input, Threshold — and a timing taken at 2 dB is not
comparable with one taken at 12. Four depths rather than the first pass's
one, because an opto at 3 dB and the same opto at 15 dB are different
compressors.

### Noise

`noise_summary.csv` and `noise_bands.csv` carry each unit's output with its
noise model switched **out and in**, on silence and underneath a -12 dBFS
tone. Twenty units have such a control; the rest are silent on silence, which
is true and is why this needed a pass of its own.

---

## Traps

**Measured on silence with the noise off, everything reads -200 dBFS.** That
is the analysis floor, not a measurement. Use `noise_summary.csv` for noise,
and read both of its conditions: several models emit nothing at all until
signal is passing. Waves NLS Channel is silent on silence at every setting
and puts a floor 17 dB higher underneath a tone once its Drive is up.

**The `under_tone_-12` condition notches the tone and its first twenty-four
harmonics out and measures what is left.** On a clean unit that is the noise
floor. On a hard-driven one it is the noise floor plus any *non-harmonic*
products the unit made — aliasing, mostly — which is a real thing to know
about and is not hiss. Cross-check against `colour_imd.csv`'s CCIF row, which
is the test aimed at aliasing directly.

**A swept sine measures the response only while the unit is linear, and it
is one render, which is what the Waves glitch ruins.** Both failures are
caught the same way: every dense response has nine tone-by-tone points
measured beside it, each the best of three or four renders, and the
disagreement is in `sweep_check_worst_db`. **Filter on it before trusting a
curve.** Thirty-seven of the fifty-one units agree within 0.5 dB in the
linear region and most of the rest are Airwindows plugins that are nonlinear
at every level — Mackity is 46 dB out at -40 dBFS, which is a fact about
Mackity and not about the rig.

For those, and for anything measured at -12 or -3 dBFS where a saturating
stage's fundamental is not its linear part, use
`colour_response_by_tone.csv`. It is coarser — 9 or 26 points against 73 —
and it has no phase, but a tone's fundamental is its fundamental whatever
else the stage is doing.

**Waves plugins drop a glitch into roughly one render in eight.** A glitched
render loses fundamental energy and lifts the broadband floor by tens of dB.
Every harmonic cell is rendered four times and the one with the most
fundamental in it is kept; `rejected_runs` records how many were thrown away.

**Below about two dB of gain reduction a compressor's timing is the
envelope's own ripple.** `timing_reliable` is false there. It is not a
suggestion: before the crossing detector was given a dwell requirement, an
1176 at 3 dB reported a 0.17 ms release while its own release curve plainly
took half a second.

**A Q above about 4 is under-read.** `eq_band_shape.csv` derives Q from where
the curve crosses half the peak height, on a 1/24-decade grid — about a
seventh of an octave between points. Checked against TDR Nova, which states
its Q exactly, that is faithful to within 1% up to Q 2 (0.3 reads 0.306, 0.7
reads 0.712, 2.0 reads 1.987) and starts losing the peak above it: a stated
6.0 reads 5.1. Nothing analogue in this collection is that narrow, but a
digital EQ set that way is.

**Wow and flutter totals are band-limited to 0.1–1000 Hz**, the same range
the bands cover. Unfiltered they are dominated by the analytic phase at the
ends of the segment: a machine with no modelled speed variation reads
0.0000% in every band and a 1.8% peak, which is the edge and not the tape.
`unfiltered_peak_pct` keeps the raw figure for anyone who wants to see it.

**Harmonics above Nyquist alias.** `valid_orders` says how many are real at
each frequency; a 10 kHz fundamental has only the 2nd below 24 kHz.

**A UAD control that pedalboard types as `str` cannot be set by number**, and
its value list is truncated, so the top of the range is unreachable by name.
Those are set as raw 0..1 and the `applied` field records what stuck.

**Airwindows parameters are raw 0..1 and the consolidated build hands a newly
selected processor whatever values the last one had.** One arrived panned
hard right and measured as silence. Every Airwindows entry states every
parameter, and the output trim is then found by bisection so units are
compared at matched gain rather than matched knob positions.

## What is not in it

- **Supply sag.** It needs a sustained tone with a loud interruption,
  watching the gain of the tone rather than of the interruption. Nothing here
  does that.
- **Stereo behaviour.** Everything is measured mono into the left channel.
- **Softube's Solid EQ.** Its gain and frequency controls take effect but
  report their previous value, so a measurement could be taken and could not
  be labelled with the setting that produced it. SSL's own plugin carries the
  SSL EQ comparison, in both its E and G curves.
- **Anything VST2-only.** `pedalboard` hosts VST3 and AU. Airwindows is
  reached through its consolidated VST3 build; see `aw.py`.

## Re-running

```sh
rsync -a --exclude out spikes/preamp-measure/ paco:mooloop-measure/
ssh paco 'cd mooloop-measure && ./venv/bin/python survey.py'        # licences
ssh paco 'cd mooloop-measure && ./venv/bin/python measure_colour.py'
ssh paco 'cd mooloop-measure && ./venv/bin/python measure_eqc.py'
ssh paco 'cd mooloop-measure && ./venv/bin/python measure_compc.py'
ssh paco 'cd mooloop-measure && ./venv/bin/python measure_noise.py'
rsync -a paco:mooloop-measure/out/ spikes/preamp-measure/out/
python3 export.py
```

Send stdout to `/dev/null` and read stderr: UAD's plugins print a page of
logger configuration every time one is instantiated, from a stream that
escapes the fd redirect, and every progress line here goes to stderr for that
reason.

`survey.py` runs first and separately for a reason the first pass paid for:
an unlicensed plugin usually loads and then outputs silence or dry signal,
which measures as a beautifully clean stage. It also writes the parameter
dumps that `voicings.py` is authored from.
