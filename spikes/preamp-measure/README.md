# preamp-measure — reference measurements from real plugins

`docs/plans/console/06-preamp-modelling.md` ends on a promise:

> we can do some sweep measurements on UAD and Waves plugs later to try and
> get some real numbers. if we need them before then lets just pick some.

This is the sweep. It drives licensed reference plugins offline on Adam's
Mac and writes down what they actually do, so `PreampVoicing`'s four rows,
the EQ's curves, and the compressor timings can be fitted to a measurement
instead of authored.

Results are in [RESULTS.md](RESULTS.md). This file is how to re-run them.

A second pass on 2026-09-10 widened it from 25 units to 106 and rebuilt the
settings so that two makers' boxes land on one axis — for a reader choosing a
plugin rather than for a question about mooloop's modules. That set has its
own roster (`roster.py`), its own settings (`voicings.py`), its own scripts
(`measure_colour.py`, `measure_eqc.py`, `measure_compc.py`,
`measure_noise.py`) and its own documentation in [DATA.md](DATA.md), which is
where to look for the schema, the grids and the traps. Everything below still
applies to both.

## What it runs on

A Mac, because that is where the plugins are licensed. Nothing here is
built, installed system-wide, or left running: it is a virtualenv in
`~/mooloop-measure` and a directory of JSON.

```sh
ssh paco
python3 -m venv ~/mooloop-measure/venv
~/mooloop-measure/venv/bin/pip install pedalboard numpy scipy
```

`pedalboard` hosts VST3 and AU plugins with no GUI, no audio device and no
DAW, which is what makes every number here reproducible from a shell rather
than read off a meter.

Copy the scripts over and run them:

```sh
rsync -a --exclude out spikes/preamp-measure/ paco:mooloop-measure/
ssh paco 'cd mooloop-measure && ./venv/bin/python measure_preamp.py nls_nevo'
rsync -a paco:mooloop-measure/out/ spikes/preamp-measure/out/
```

Every script writes `out/<kind>/<slug>.json` and prints a progress line. The
`report_*.py`, `fit_preamp.py` and `tables.py` scripts read those files back
locally and print tables; they need no plugins and no Mac.

**`out/` is committed on purpose**, which is the one place this ignores the
usual rule about generated output. Regenerating it needs Adam's Mac and about
a dozen licences, so it is closer to a recording than to a build artifact —
and `RESULTS.md`'s tables are printed from it rather than typed, so deleting
it would leave the document unable to check itself.

## The scripts

| Script | What it does |
| --- | --- |
| `mlab.py` | Hosting, signal generation, spectrum analysis. Everything else imports it. |
| `candidates.py` | Every unit on the machine that was worth trying, with its path. |
| `probe.py` | Loads one unit, dumps its parameters, proves it passes audio. Run this first for anything new. |
| `ranges.py`, `show_params.py`, `probe_drive.py` | Finding out what a plugin's controls are and which one reaches the nonlinearity. |
| `units.py`, `measure_preamp.py` | Harmonics against frequency, level and drive. |
| `eq_units.py`, `measure_eq.py` | EQ curves, and whether the distortion is band-dependent. |
| `comp_units.py`, `ctiming.py`, `measure_comp.py` | Compressor timing, static curve, detector shape. |
| `measure_transient.py` | Slew and crest-factor retention, for `PreampVoicing::slew`. |
| `roster.py`, `voicings.py` | The 2026-09-10 survey: which units, and how each is set. |
| `survey.py`, `dump_params.py` | Licence check and parameter dump for the whole roster. |
| `analysis.py` | Response, intermodulation, noise, wow and flutter, THD matching. |
| `aw.py` | Selecting one Airwindows processor out of the consolidated build. |
| `measure_colour.py`, `measure_eqc.py`, `measure_compc.py`, `measure_noise.py` | The survey's four suites. |
| `rerun_failed.py` | Re-measures anything whose result file is an error stub. |
| `export.py` | Flattens `out/chart/` into the long-format CSV in `out/tidy/`. |

## Four things that will bite the next person

**Licensing is the first question, not an afterthought.** An unlicensed
plugin usually loads and then outputs silence or dry signal, which measures
as a beautifully clean stage. `probe.py` exists to answer this before
anything else, and every one of the 25 units here was checked with it.

**UAD's Audio Unit builds do not scan; their VST3 builds do.** Every
`uaudio_*.component` fails with "unsupported plugin format or scan failure"
and the identical `.vst3` loads fine. This cost an hour of thinking the
licences were bad.

**Waves plugins glitch, deterministically, on about one render in eight.**
A glitched render loses fundamental energy and lifts the broadband floor by
tens of dB, which lands 40 dB above a quiet cell's real 2nd harmonic. A
clean render is bit-repeatable. `mlab.stable_harmonics` renders each cell
four times and keeps the one with the most fundamental in it, and records
how many it threw away — see `rejected_runs` in the JSON, which ran at 11.7%
across every Waves unit and 0% on everything else.

**Find the drive control before you sweep it.** A channel strip has several
gains and usually only one reaches the nonlinearity. Sweeping the wrong one
produces a column of identical rows, which looks like data. `probe_drive.py`
asks the question directly, and it caught three of the six preamp units:
Scheps 73's gain selector only sets level, SSL EV2's line gain walked the
output into clipping, and bx_console N's THD control does nothing at all
unless its EQ section is switched in.

## Level convention

Amplitudes are dBFS peak. The analogue reference is the usual one,
**-18 dBFS = 0 VU = +4 dBu**, so a -18 dBFS sine is a unit's nominal
operating level and 0 dBFS is +18 dBu into it. The UAD units carry their own
`headroom` control for this and are left at their default 16 dB.
