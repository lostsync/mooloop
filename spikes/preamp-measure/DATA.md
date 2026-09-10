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
| `colour` | preamps, console channels, saturation | `measure_colour.py` | `out/chart/colour/` |
| `tape` | tape machines, measured on the same axes as the preamps | `measure_colour.py` | `out/chart/colour/` |
| `eq` | equalisers, at matched band targets | `measure_eqc.py` | `out/chart/eq/` |
| `comp` | compressors, at four depths | `measure_compc.py` | `out/chart/comp/` |
| — | self-noise, model in and out | `measure_noise.py` | `out/chart/noise/` |

Tape is deliberately not a separate suite. A Studer and a 1073 are things a
reader is choosing *between*, so they get the same response, THD, harmonic
and transient measurements and the tape machines get wow and flutter on top.

## The tidy tables

### Colour and tape

| file | one row per | the chart it is for |
| --- | --- | --- |
| `colour_response.csv` | unit × setting × level × frequency | frequency response, and how it moves with level |
| `colour_thd_vs_level.csv` | unit × setting × frequency × level | **THD against level** — the headline; where a unit starts to break up |
| `colour_harmonics_vs_freq.csv` | unit × setting × frequency × level | harmonic tilt: warm on a kick, clean on a hat |
| `colour_imd.csv` | unit × setting × test × level | intermodulation, SMPTE and CCIF |
| `colour_noise.csv` | unit × setting × condition × band | noise floor by third octave |
| `colour_transients.csv` | unit × setting × level | crest retention and the slew index |
| `colour_summary.csv` | unit × setting | one row of headline numbers per setting |

`colour_summary.csv` is the table to start from. `headroom_1pct_dbfs` is the
input level at which the unit reaches 1% THD, interpolated between measured
levels — the single number that puts a console channel, a tape machine and a
clipper on one axis. `tilt_db` is how much further down the 2nd harmonic is
at 5 kHz than at 80 Hz.

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
or the `under_tone_-12` condition, which is the floor beneath a signal and is
where a unit that only hisses when passing audio shows up.

**A swept sine is one render, and one render is what the Waves glitch
ruins.** Every dense response has nine tone-by-tone points measured beside
it, each the best of three or four renders, and the disagreement is recorded
as `sweep_check_worst_db`. On a clean unit it is under 0.1 dB. Filter on it
before trusting a curve.

**Waves plugins drop a glitch into roughly one render in eight.** A glitched
render loses fundamental energy and lifts the broadband floor by tens of dB.
Every harmonic cell is rendered four times and the one with the most
fundamental in it is kept; `rejected_runs` records how many were thrown away.

**Below about two dB of gain reduction a compressor's timing is the
envelope's own ripple.** `timing_reliable` is false there. It is not a
suggestion: before the crossing detector was given a dwell requirement, an
1176 at 3 dB reported a 0.17 ms release while its own release curve plainly
took half a second.

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
