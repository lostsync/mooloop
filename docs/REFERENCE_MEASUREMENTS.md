# Measuring a reference plugin

How to get real numbers out of a plugin on Adam's studio machine, and what to
capture, so that one session produces everything rather than three sessions
each finding the last one missed something.

Written 2026-09-09, when the first voicing numbers had to be picked rather
than measured. Adam: *"we can do some sweep measurements on UAD and Waves
plugs later to try and get some real numbers"*, and *"i have a few plugins
that might be worth measuring for various mooloop stuff."* Nothing here is
built yet; this is the protocol, so that the scripts are a short job and the
studio time is not wasted.

## The split: mooloop never hosts the plugin

```text
   here                studio machine              here
   ────                ──────────────              ────
   stimulus.wav  ──▶   render through   ──▶   response.wav  ──▶  fit
                       the plugin
```

**This is the whole design decision.** Hosting VST3/AU/AAX to automate a
measurement means a JUCE or CLAP host, a plugin scanner, parameter automation
and a licence dance with iLok and UAD — days of work, none of it mooloop. A
rendered WAV is the same information, and the DAW that already has the plugin
authorised does the hosting for free.

So the tooling is two small binaries, both here, and neither is exotic:
`hound` is already a workspace dependency in three crates and
`mooloop_dsp::analysis` already has the FFT.

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

Per plugin, one WAV:

| | |
| --- | --- |
| Levels | -36, -30, -24, -18, -12, -6, 0 dBFS peak |
| Frequencies | 40, 60, 80, 120, 200, 400, 800, 1k5, 3k, 5k, 8k, 12k Hz |
| Tone length | 1.0 s, with 0.25 s of silence between |
| Order | frequency-major, so a drifting plugin drifts *across* levels rather than within one |

Eighty-four tones is about 105 seconds. Add at the head:

- **2 s of digital silence** — the noise floor, measured before anything else,
  because `Grip`-class harmonics live at -90 dB and plenty of "analog"
  plugins have a floor above that. A profile fitted to hiss is worse than a
  picked one.
- **A short alignment chirp**, so the analyser can find frame zero without
  being told the plugin's latency.

## Four things that would waste the session

Stated as a checklist because each one silently produces plausible numbers
that are wrong.

1. **Auto-gain, auto-makeup and output normalisation off.** They corrupt the
   level dependence, which is the single thing this exercise exists to
   capture. The analyser measures the fundamental in *and* out and flags a
   plugin whose gain moves with level, so this gets caught rather than
   believed — but catching it at analysis means re-rendering.
2. **Record the input level and the plugin's own input/drive setting.** A
   harmonic profile is meaningless without the level it was measured at.
   That is exactly why the first-pass voicing numbers could not be authored:
   *"-28 dB of 2nd harmonic" says nothing without saying what arrives.*
3. **Nothing else in the chain**, and no dither on the render. Render to
   32-bit float.
4. **Render at the rate mooloop runs at.** Plugin oversampling changes with
   sample rate, and so does where its aliasing lands.

Also worth a note in the filename: whether the plugin was in a "clean" or
"drive/hot" mode, if it has one. Two renders of the same plugin at different
front-panel settings are more useful than one, and cost a minute.

## What to capture, by what it is for

Adam has *"a few plugins that might be worth measuring for various mooloop
stuff"*, and they do not all want the same stimulus.

### Preamps, saturators, tape — the harmonic surface

The battery above, unchanged. It produces harmonic amplitude as a function of
level and frequency, which is precisely what `PreampVoicing` is a table of.
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
then wanted. For a compressor the detector's behaviour matters more than the
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
response by the stimulus, envelope-follow the result.

### Reverbs

Not this. A reverb wants an impulse response, which is a different capture and
a different use, and mooloop's is an FDN rather than a convolver
(`REVERB.md`). Out of scope here.

## What Adam has to decide before anything is scripted

**Which plugins, and what each one is *for*.** "Measure everything" produces a
pile of numbers with no home. "This one is the `Iron` target, these two are
for a tape device later, this comp is the one I want the strip to feel like"
decides which stimulus each needs and stops the session generating renders
nobody fits.

## What lands afterwards

`PreampVoicing`'s four rows and `harmonics.rs`'s four profiles stop being
picked and become measured — and the doc comments that currently say
*provisional* get to say what they were fitted to instead. Nothing else in
either module changes, which was the point of keeping them as plain tables of
numbers with no behaviour attached.
