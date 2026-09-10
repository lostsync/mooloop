# Measuring a reference plugin

How to get real numbers out of a plugin on Adam's studio machine, and what to
capture, so that one session produces everything rather than three sessions
each finding the last one missed something.

Written 2026-09-09, when the first voicing numbers had to be picked rather
than measured. Adam: *"we can do some sweep measurements on UAD and Waves
plugs later to try and get some real numbers"*, and *"i have a few plugins
that might be worth measuring for various mooloop stuff."*

> **Run on 2026-09-10.** This protocol has been executed — 25 plugins,
> preamps, EQs and compressors. The scripts are
> [`spikes/preamp-measure/`](../spikes/preamp-measure/README.md) and the
> numbers are in its
> [`RESULTS.md`](../spikes/preamp-measure/RESULTS.md). This document stays as
> the protocol and the reasoning behind it; what the run learned that changes
> the *method* is folded in below, marked as such.

Revised the same day, when Adam pointed out that the plugins can simply be
loaded and driven in Python. The first version routed everything through
rendered WAVs and was a worse plan for a reason worth keeping: the cost of
"host the plugin" had been priced against the wrong option.

## Load the plugins in Python and drive their parameters

Adam, 2026-09-09: *"that instance of you can write some python that'll load
the vsts and use their individual params to get the measurements you want w/o
a bunch of wav files and shit."*

**This replaces the first version of this document**, which routed everything
through rendered WAVs on the grounds that hosting a plugin was days of work.
That was true of the option being priced — a JUCE or CLAP host in Rust — and
false of the one that matters: `pedalboard` (Spotify's, `pip install
pedalboard`) loads VST3 and AU, exposes each parameter by name, and processes
numpy arrays. `dawdreamer` is the heavier alternative if parameter
*automation over time* is ever needed; it is not, for any of this.

The win is not mainly the absence of WAV shuttling. It is that **the plugin's
own controls become a measured axis** rather than a fixed choice made once at
render time. A preamp's drive knob is exactly the parameter whose effect on
the harmonic surface we want, and by hand it would mean one render per
setting. In-process it is a loop.

It is also reproducible: the script *is* the record of what was measured, and
a bad fit can be re-run without booking the studio again.

```python
from pedalboard import load_plugin
fx = load_plugin("/path/to/Scheps 73.vst3")
print({p.name: (p.min_value, p.max_value) for p in fx.parameters.values()})
fx.drive = 6.0
out = fx(stimulus, sample_rate=48000, reset=True)   # reset between tones
```

### Three things to test in the first ten minutes

Before generating a battery, load each plugin once and check:

1. **Does it load at all, and does it authorise in this process?** iLok and
   the Waves/SSL licence managers generally do, since the plugin is being
   loaded normally. **UAD is the one to try first**: the native UADx builds
   should be fine, while the DSP-accelerated UAD-2 versions want the Apollo
   or Satellite hardware present and may refuse a bare host.
2. **Are the parameter names and ranges legible?** Some plugins expose
   normalised `0..1` with cryptic names. Whatever the mapping is, print it and
   commit it beside the results — a measurement whose drive setting cannot be
   reproduced is a number without a level, which is the mistake this whole
   document exists to avoid.
3. **Is it deterministic?** Process the same buffer twice and compare. Several
   of these deliberately are not, and that decides how much averaging the
   battery needs.

### `reset=True` between tones, and why it matters here

Every tone must start from the same state, or a level step measured after a
loud tone is measuring the plugin's recovery rather than its transfer curve.
That is free in-process and was awkward by hand.

## Stepped tones, not a swept sine

A Farina exponential sweep is the elegant method — one pass gives the impulse
response *and* separates each harmonic order into its own arrival — and it is
the wrong tool here. It assumes the device under test is **time-invariant**,
and the plugins worth measuring are exactly the ones that are not: modelled
noise, hum, transformer drift and analogue-style randomisation are selling
points on this kind of product, and each of them smears a deconvolution.

Stepped sine tones at exact analysis-bin frequencies are dumber and hold up:
each tone is measured on its own, an integer number of cycles, no window and
no leakage, and non-stationary noise averages down instead of corrupting the
result.

### The battery

Each tone is processed on its own, with `reset=True`, so the grid is three
nested loops rather than one long file:

| | |
| --- | --- |
| Levels | -36, -30, -24, -18, -12, -6, 0 dBFS peak |
| Frequencies | 40, 60, 80, 120, 200, 400, 800, 1k5, 3k, 5k, 8k, 12k Hz |
| Drive / input | whatever the plugin exposes, at 3-5 settings across its range |
| Tone length | 0.5 s, of which the first half is discarded so anything level-dependent has settled |

That third axis is the one the WAV workflow could not afford and the
in-process one gets for nothing — and it is the axis the model actually wants,
since `pre in / drive` is the control being modelled.

Eighty-four tones per drive setting, at half a second each, is under a minute
of processing per plugin. Measure **digital silence first** for the noise
floor: `Grip`-class harmonics live at -90 dB and plenty of "analog" plugins
have a floor above that, and a profile fitted to hiss is worse than a picked
one.

No alignment chirp is needed — `pedalboard` reports and compensates plugin
latency — but check it rather than assume it: process an impulse and confirm
it comes back where it went in. It does: every unit measured came back within
one sample.

**Measured 2026-09-10 — repeat each tone and reject, do not average.** Point 3
above asks whether a plugin is deterministic. The answer for Waves plugins is
"almost": a clean render is bit-repeatable, and roughly one render in eight
carries a dropout that loses fundamental energy and lifts the broadband floor
by tens of dB. Averaging that in moves a quiet cell's 2nd harmonic by 40 dB.
Because the clean value is exact and the glitch only ever subtracts signal,
the fix is selection rather than averaging: render four times, keep the one
with the most fundamental in it, and record how many were discarded.

## Three things that silently produce wrong numbers

Fewer than the WAV version needed, because most of that list was about not
being able to see what the plugin was set to. These remain.

1. **Auto-gain, auto-makeup and output normalisation must be off.** They
   corrupt the level dependence, which is the single thing this exercise
   exists to capture. The analyser measures the fundamental in *and* out and
   flags a plugin whose gain moves with level — cheap to catch in-process, and
   worth catching, because a normalising plugin's harmonic surface looks
   flatter in level than it is.
2. **Every parameter value gets written down with the result.** A harmonic
   profile is meaningless without the level and the drive setting it was
   measured at — that is exactly why the first-pass voicing numbers could not
   be authored. Dump the full parameter dictionary alongside each run, not
   just the ones being swept: the defaults matter too.
3. **Run at the rate mooloop runs at.** Plugin oversampling changes with
   sample rate, and so does where its aliasing lands.

## What to capture, by what it is for

Adam has *"a few plugins that might be worth measuring for various mooloop
stuff"*, and they do not all want the same stimulus.

### Preamps, saturators, tape — the harmonic surface

The battery above, unchanged. It produces harmonic amplitude as a function of
level, frequency **and drive**, which is precisely what `PreampVoicing` is a
table of — and the drive axis is what tells us where on its own range a plugin
was designed to be run, which is a judgement we would otherwise have to make
by ear.
The fit is then mechanical: `mooloop_dsp::harmonics` authors a profile through
its Chebyshev decomposition, so measured harmonic amplitudes *are* the
coefficients, and `preamp.rs`'s `tilt_db` falls out of how the surface leans
across frequency.

**Fit at one level and check the others**, rather than least-squares over the
whole surface. `harmonics.rs` records why: a multi-term profile's terms
interact, so a fit that averages over levels will land somewhere that matches
nothing. Two closed-form predictions about that scheme have already turned out
wrong in the same way.

### EQs — response per band, and where they distort

The same render with a few more tones. Worth doing rather than assuming a
textbook curve, for one reason: an inductor-based EQ's nonlinearity sits
*inside* the filter network, so its distortion is **band-dependent** — it
appears where the band is boosted and not elsewhere. That is not something a
frequency response plot shows, and it is a large part of why those EQs are
described as musical.

Capture: each band swept through its gain range, one tone per frequency point,
plus the harmonic battery at two band settings (flat, and one band boosted
hard).

### Compressors — timing, not transfer curve

**A different stimulus entirely**, and the one most likely to be skipped and
then wanted. Also the one that gains most from being in-process: attack and
release are parameters, so their *stated* value can be measured against their
actual behaviour across the whole knob range in one loop. For a compressor the detector's behaviour matters more than the
gain element's transfer curve, so the tones above measure almost nothing
useful.

Capture instead:
- **Level steps** — a tone that jumps 20 dB and holds, then drops back. The
  gain-reduction envelope's shape is the attack and release curve, including
  whatever programme dependence the plugin has.
- **Bursts** at several durations, which is what exposes a two-stage or
  auto release.
- **A static staircase** for the ratio and knee: hold each level long enough
  to settle, and read the transfer curve off the settled values.

The gain envelope is recoverable without a sidechain output: divide the
response by the stimulus, envelope-follow the result. `reset=True` between
runs is doing real work here — a release measured while the plugin is still
recovering from the previous burst is measuring the wrong thing.

### Reverbs

Not this. A reverb wants an impulse response, which is a different capture and
a different use, and mooloop's is an FDN rather than a convolver
(`REVERB.md`). Out of scope here.

## What Adam has

Listed from memory 2026-09-09 and checked against the machine 2026-09-10.
The memory was right about all of it. What the folder also turned out to hold
is a full UADx set — 610-B, API Vision, Century, LA-2A, 1176 in three
revisions, dbx 160, Fairchild 660 and 670, Distressor, SSL G bus, Studer
A800, Manley Massive Passive — plus Waves PuigTec, VEQ3/4, CLA-3A, API-2500
and API-550, and TDR's Kotelnikov and SlickEQ.

`spikes/preamp-measure/candidates.py` is the working list, with a path and a
role for each. All 25 units tried were licensed and passed real audio.

**Starting with FrontDAW was the right call and it did not settle the
question.** Its five styles do differ from each other under identical
conditions, exactly as hoped — but every one of them has a *flat* harmonic
response across frequency, where Waves NLS's Neve model has 23.8 dB of
frequency-dependent drive. One plugin per voicing is enough to rank the
voicings and is not enough to decide whether the transformer mechanism is
part of the sound. That took a second opinion.

What still has to be decided at the machine is what each measurement is *for*.
"Measure everything" produces a pile of numbers with no home; "this is the
`Iron` preamp target, this comp is what the strip should feel like" decides
which stimulus each needs.

### Two traps that cost an hour each

- **UAD's Audio Unit builds do not scan; the VST3 builds of the same plugins
  load fine.** Every `uaudio_*.component` fails with "unsupported plugin
  format or scan failure", which reads exactly like a licensing failure and
  is not one.
- **Find the drive control before sweeping it.** A channel strip has several
  gains and usually only one of them reaches the nonlinearity; sweeping the
  wrong one produces a column of identical rows, which looks like data. Three
  of six preamp units needed this checked: Scheps 73's gain selector only
  sets level, SSL EV2's line gain walks its own output into clipping, and
  bx_console N's THD control does nothing whatever unless its EQ section is
  switched in.

## What lands afterwards

`PreampVoicing`'s four rows and `harmonics.rs`'s four profiles stop being
picked and become measured — and the doc comments that currently say
*provisional* get to say what they were fitted to instead. Nothing else in
either module changes, which was the point of keeping them as plain tables of
numbers with no behaviour attached.

**Still true after the run, with one addition.** The measurements are taken
and written down; nothing in `mooloop_dsp` has been re-authored from them
yet, because two of the findings are choices for Adam rather than fits:
whether `Iron` models a mic preamp or a console channel, which decides its
harmonic order, and whether `Grip` gets a drive-dependent low shelf, which
the SSL reference plainly has and the current exactly-reciprocal tilt pair
cannot produce. `spikes/preamp-measure/RESULTS.md` states both.
