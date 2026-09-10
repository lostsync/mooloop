# Reference measurements — what the real units actually do

Measured 2026-09-10 on Adam's Mac, from 25 licensed plugins driven offline.
Method and re-run instructions are in [README.md](README.md); the raw JSON is
in `out/`, and every table below is printed by `tables.py` from that JSON
rather than retyped.

**Levels.** Amplitudes are dBFS peak. mooloop's unity operating level is
-12 dBFS (`GAIN_STRUCTURE.md`), so the -12 dBFS row is the one a voicing is
authored from and it is the row quoted throughout. The plugins themselves
assume the studio convention of -18 dBFS = 0 VU = +4 dBu.

---

## 1. Preamps

### The three consoles, measured against each other

Waves NLS Channel is the instrument this section leans on, for the reason
`REFERENCE_MEASUREMENTS.md` gives about FrontDAW: three console models behind
one Drive control, with no EQ in the path, means the voicings are measured
*relative to each other* under identical conditions.

At -12 dBFS in, Drive at 6 of 12. `h2`..`h5` are dB below the fundamental at
80 Hz; `tilt` is how much further down the 2nd harmonic is at 5 kHz than at
80 Hz, which is the "warm on a kick, clean on a hat" number.

| unit | h2 | h3 | h4 | h5 | h2-h3 | h2 at 5 kHz | tilt |
| --- | --- | --- | --- | --- | --- | --- | --- |
| NLS Nevo (Neve 5116) | -69.8 | -59.9 | -97.0 | -73.5 | -9.8 | -93.6 | +23.8 |
| NLS Mike (SSL 4000 G+) | -52.3 | -36.6 | -80.3 | -50.4 | -15.8 | -54.6 | +2.3 |
| NLS Spike (EMI TG12345) | -47.0 | -58.1 | -79.5 | -73.1 | +11.1 | -49.3 | +2.3 |

Three things fall out of that table, and two of them revise
`06-preamp-modelling.md`.

**The frequency-dependent drive is real, and it is enormous — on the Neve.**
Nevo's 2nd harmonic is 23.8 dB louder at 80 Hz than at 5 kHz, and 37 dB
louder than at 10 kHz.

| frequency | 40 | 60 | 80 | 110 | 160 | 220 | 320 | 440 | 630 | 880 | 1250 | 5000 | 10000 Hz |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| h2, dB below fundamental | -58.4 | -65.5 | -69.8 | -73.8 | -77.7 | -80.6 | -83.9 | -86.3 | -88.5 | -90.0 | -90.8 | -93.6 | -95.6 |

Fitting a half-way corner to that puts it at **240-250 Hz**, near the 220 Hz
`IRON_PREAMP` already guesses, with a tilt about **10 dB deeper** than the
14 dB it is set to. The mechanism 06 reasoned its way to is the one the
measurement finds. That is the good news.

The shape is the caveat. It is not one first-order shelf: the fall is
**11.4 dB across the 40-80 Hz octave**, 7.9 dB across 80-160, 6.2 across
160-320 and 4.6 across 320-630, flattening to about 1.4 dB per octave above
1 kHz. A single shelf biquad — which is what `Preamp` uses — tops out at
6 dB per octave, so it can match the corner and the total or the steepness
at the bottom, but not both. Whether that matters is an ears question; that
it cannot be fitted exactly is not.

**The even-order assumption is wrong for the Neve, and right for the EMI.**
06 reasons from topology — single-ended gives even, push-pull gives odd — to
`Iron` being even-dominant, and `IRON` is authored with its 2nd 18 dB above
its 3rd. Measured, the Neve console channel is **odd**-dominant by 9.8 dB,
and so is the SSL by 15.8 dB. The only even-dominant unit of the three is
the EMI, by 11.1 dB. bx_console N agrees emphatically and independently: its
Neve VXS channel is odd to the point of having no measurable 2nd harmonic at
any setting of its THD control.

So the reasoning is sound and its application was not — and the measurements
say so twice, because the split is visible *within the same manufacturer*:

| Neve-derived unit | what it models | h2 | h3 | h2-h3 |
| --- | --- | --- | --- | --- |
| Scheps 73, Preamp Drive on | 1073 mic preamp | -19.4 | -30.0 | **+10.6** |
| NLS Nevo, Drive 6 | 5116 console channel | -69.8 | -59.9 | **-9.8** |
| bx_console N, THD -30 | VXS console channel | -119.2 | -52.4 | **-66.8** |

A console *channel* is a chain of push-pull line amps and its residue is odd,
overwhelmingly so in bx_console N's case. The even-order warmth people mean
by "Neve" is the single-ended **mic preamp** driven hard, which is a
different stage from the one a console strip sits in — and Scheps 73, which
models exactly that stage, is even-dominant by 10.6 dB.

06 reasoned from topology to harmonic order and got it right. What it then
attached the reasoning to was the wrong box. **So `Iron` has a choice to
make that the document has not yet noticed: preamp or channel.** They do not
measure alike, and only one of them is even-order.

**The SSL's "nice fat round low end" is a linear lift, not distortion.**
06 predicted it would be frequency-dependent drive: *"keep the
frequency-dependent drive, so the low end still rounds when pushed"*. Mike
has almost none — 2.3 dB of tilt against Nevo's 23.8. What it has instead is
a low-frequency *gain* that grows with the Drive control, measured at
-40 dBFS where nothing is distorting at all:

| unit | drive | 40 Hz | 60 Hz | 80 Hz | 110 Hz | 220 Hz | 1250 Hz | 5000 Hz | 10000 Hz |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| NLS Mike (SSL) | `0` | +0.52 | +0.18 | +0.06 | +0.00 | -0.01 | +0.00 | -0.09 | -0.26 |
| NLS Mike (SSL) | `6` | +4.21 | +2.06 | +1.15 | +0.55 | +0.13 | +0.00 | -0.71 | -1.42 |
| NLS Mike (SSL) | `12` | +8.96 | +5.71 | +3.86 | +2.32 | +0.66 | +0.00 | -0.73 | -1.36 |
| NLS Nevo (Neve) | `12` | +3.03 | +1.28 | +0.61 | +0.20 | -0.07 | +0.00 | +0.14 | +0.30 |
| NLS Spike (EMI) | `12` | -0.29 | -0.27 | -0.27 | -0.26 | -0.17 | +0.00 | -0.05 | -0.23 |

Nine dB at 40 Hz is not a subtlety, and `Preamp` cannot currently produce it:
its tilt pair is *exactly reciprocal* by construction, so the stage "colours
without equalising" — which is a property the module is tested for and proud
of, and which rules this mechanism out. Whether `Grip` should have a
drive-dependent low shelf is a decision for Adam, not a bug; it is recorded
here because the reference unit plainly does it and the model plainly cannot.

### Per-unit notes

- **Waves NLS Channel** — the most useful unit here. Drive 0-12 dB, usable to
  about 9; at 12 the output clips and the harmonic balance degenerates
  (h2 and h3 converge, every frequency reads alike), so those rows are marked
  clipped and excluded from the tables.
- **Waves Scheps 73** — its gain selector sets level only, and its input trim
  moves level without moving distortion. Preamp Drive is a switch, not a
  continuum: off, the stage measures -95 dB of 2nd; on, -22 dB, essentially
  independent of level. Useful as a "driven 1073" data point, useless as a
  drive axis.
- **bx_console N** — its THD control is calibrated in dB and lives *inside
  the EQ section*. With every section bypassed the channel measures 0.0001%
  however the control is set; with the EQ switched in at a flat curve the
  whole 0.95% comes back. The control is then honest and linear: every 10 dB
  of marking moves the 3rd harmonic 10.2 dB, sitting a constant 24 dB below
  the marking at -12 dBFS (-60 gives -84.1 dB, -30 gives -52.4 dB). There is
  no measurable 2nd harmonic at any setting.
- **SSL EV2** — its analogue stage is switchable, and switching it off is a
  clean null: -135 dB across the band. Strongly odd (h2 -84.5, h3 -41.8 at
  line 0), and its tilt runs **the wrong way**: -14.1 dB, meaning it distorts
  *more* at 5 kHz than at 80 Hz, with the corner up at 7.4 kHz. Nothing here
  supports a transformer reading of an SSL channel, which agrees with NLS
  Mike having no tilt either. First measured with line gain alone, which
  walked the plugin's own output into clipping at +10 and +20; the numbers
  here compensate with the output trim.
- **Scheps 73** — even-dominant at both settings, by 16.8 dB with the drive
  out and 10.6 dB with it in, and no tilt at all (0.5 dB).

### FrontDAW: five voicings that differ only in harmonic order

`REFERENCE_MEASUREMENTS.md` says to start with FrontDAW, because one plugin
covering every preamp style measures them against each other. It does, and
what it says is worth having precisely because it *disagrees* with NLS.

At -12 dBFS in, Mojo at 100 of 100:

| style | h2 | h3 | h4 | h5 | h2-h3 | tilt |
| --- | --- | --- | --- | --- | --- | --- |
| British | -50.0 | -44.9 | -79.5 | -79.9 | -5.1 | +0.1 |
| American | -45.0 | -40.7 | -71.3 | -58.8 | -4.3 | +0.0 |
| German | -35.7 | -41.8 | -64.8 | -74.1 | +6.1 | -0.1 |
| Magnetic | -24.7 | -32.5 | -47.3 | -59.8 | +7.9 | +0.0 |
| Clip | -23.5 | -29.8 | -37.0 | -46.9 | +6.3 | +0.7 |

Mojo at 0 is a true null — every harmonic below -190 dB — and digital silence
in gives digital silence out, so none of these numbers are sitting on a noise
floor.

**Every style has a tilt of zero.** FrontDAW distinguishes British from
American from German entirely by harmonic order and amount, with no
frequency-dependent drive at all, where NLS Nevo has 23.8 dB of it. Two
well-regarded emulations of the same class of hardware disagree completely
about whether the effect 06 calls "the one that turns a shaper into a preamp"
is present.

That is not a reason to disbelieve either. It is a reason to treat `tilt_db`
as the deliberate choice 06 already says it is — the mechanism is real and
Nevo measures it, but it is not what every modeller thinks the sound is made
of. If `Iron` should be obviously different on a kick from on a hat, NLS Nevo
is the target to fit; if it should just be warm, FrontDAW's German style is.

### Where mooloop's authored numbers sit on this ladder

`harmonics.rs` states its profiles at full scale and notes that what they
should be "cannot be settled until the strip's input stage is". The
measurements settle the surrounding question: where the real units sit.

Every unit measured, at the settings each is designed to be run at, ranked by
how much 2nd harmonic it makes at mooloop's operating level:

| | h2 at 80 Hz, -12 dBFS |
| --- | --- |
| NLS Nevo, Drive 6 | -69.8 |
| NLS Mike, Drive 6 | -52.3 |
| FrontDAW British, Mojo 100 | -50.0 |
| NLS Spike, Drive 6 | -47.0 |
| FrontDAW American, Mojo 100 | -45.0 |
| FrontDAW German, Mojo 100 | -35.7 |
| FrontDAW Magnetic, Mojo 100 | -24.7 |
| FrontDAW Clip, Mojo 100 | -23.5 |
| Scheps 73, Preamp Drive in | -19.4 |
| **mooloop `IRON`, authored** | **-28.0** |
| **mooloop `PUNCH`, authored** | **-36.0** |
| **mooloop `GRIP`, authored** | **-58.0** |

`GRIP` and `PUNCH` land inside the range the real units cover. `IRON` at
-28 dB sits between FrontDAW's tape and clip styles — dirtier than any
*console or preamp* emulation measured, at any setting.

`IRON` at -28 dB is no longer an outlier once Scheps 73 is on the list: a
driven 1073 preamp makes *more* 2nd harmonic than `IRON` asks for.

The other half of `IRON`'s authoring is still further out than anything
measured. It is written with its 2nd harmonic 18 dB above its 3rd, and among
units making more than -50 dB of 2nd, **the widest even-over-odd margin
measured is +11.1 dB** (NLS Spike); Scheps 73 driven manages +10.6, FrontDAW
Magnetic +7.9, Clip +6.3. Real stages that are this dirty are dirty in both
orders at once, and an 18 dB margin is cleaner in the 3rd than any of them.

That is not a defect — the voicings are named for a sound, not for a unit,
and `UI_DESIGN.md` says so. But the drive control has a job to do that the
numbers now describe: **`pre in / drive` has to span roughly -70 dB to -20 dB
of 2nd harmonic** to cover what these units cover between them, which is the
range from a console channel at rest to a 1073 leant on.

---

## 2. Transients, and the field the harmonic sweep cannot speak to

`PreampVoicing::slew` is the transient half — the thing that tells a fast
edge from a loud steady tone, and the reason `Iron` "rounds a transient
rather than passing it whole" while `Punch` is specified as having no limit
at all. The harmonic sweep says nothing about it, so it gets its own
stimulus: a click train for crest-factor retention, and a square against a
sine for how fast edges are treated relative to slow ones.

The **slew index** below is the square's output-to-input slope ratio divided
by the sine's, so the stage's gain cancels out. 1.000 means a fast edge is
treated exactly like a slow tone; below 1 means fast edges are held back.

| unit | setting | crest change at -12 dBFS | at -3 dBFS | slew index at -3 dBFS |
| --- | --- | --- | --- | --- |
| Scheps 73 (1073), drive out | `drive_off` | -0.04 dB | -0.04 dB | 0.979 |
| NLS Nevo (Neve console) | `drive_6` | +0.02 dB | -0.36 dB | 0.959 |
| bx_console N | `thd_-30` | -0.02 dB | -0.14 dB | 0.930 |
| NLS Spike (EMI) | `drive_6` | +0.00 dB | +0.01 dB | 0.924 |
| NLS Mike (SSL) | `drive_6` | -0.59 dB | -0.91 dB | 0.864 |
| FrontDAW British | `mojo_100` | -0.08 dB | -0.52 dB | 0.812 |
| FrontDAW American | `mojo_100` | -0.11 dB | -0.54 dB | 0.767 |
| FrontDAW German | `mojo_100` | -0.11 dB | -0.97 dB | 0.660 |
| SSL EV2, line +10 | `line_10` | +0.01 dB | -0.80 dB | 0.626 |
| Scheps 73 (1073), drive in | `drive_on` | -0.89 dB | -3.08 dB | 0.260 |

**Read this one carefully, because it does not show what it looks like it
shows.** A static saturating curve produces a slew index below 1 all by
itself: a square wave sits at its peak, where the curve is compressing hard,
while a sine's steepest moment is its zero crossing, where the curve is
nearly linear. So squashing the edge more than the tone is what *any*
nonlinearity does, and it is not evidence of a slew limiter.

And that is what the column is: it tracks how much distortion each unit is
making. Scheps 73 with its drive out makes -95 dB of 2nd harmonic and scores
0.979; with the drive in it makes -19 dB and scores 0.260. FrontDAW's three
styles rank 0.812 / 0.767 / 0.660 in exactly the order of their harmonic
content. **No unit measured departs from what its own static curve already
explains**, so nothing here provides evidence that any of these emulations
models a distinct slew limit.

What that leaves for `PreampVoicing::slew`:

- `Punch`'s `slew: None` is the setting the references support. Every one of
  them behaves as a memoryless curve on this test.
- `Iron`'s `slew: Some(0.08)` is an addition beyond anything measured. It may
  well be the right *sound* — a transformer genuinely does have a transient
  behaviour a static curve lacks, and item 5 of 06's list (hysteresis) is the
  rigorous version of it — but it is not a fit to any of these units, and
  this document cannot be cited as support for the number.

The crest column is the one to trust without caveats, because it asks a
direct question: what happened to a percussive peak. At mooloop's operating
level almost nothing does — the largest is NLS Mike at -0.59 dB. Driven to
-3 dBFS, Scheps 73 takes 3.08 dB off the peak of a click train, which is
audible and is what "smeared rather than grabbed" means as a number.

---

## 3. EQ — the distortion is band-dependent, and not by a factor a shaper can fake

06 states the claim to test:

> **Inductor-based EQs saturate in the filter path.** The core in a passive EQ
> is a transformer-shaped nonlinearity sitting *inside* the frequency-shaping
> network, so the distortion is band-dependent rather than broadband.

It holds, in all four units, and the size of the effect rules out the easy
implementation.

### The controlled case: SlickEQ

TDR SlickEQ has a saturation switch that leaves the curve alone — measured,
`low_boost_clean` and `low_boost_sat` differ by 0.3 dB of response. So the
same 8 dB low boost can be measured with the nonlinearity in and out:

| setting | h2 at 40 Hz | 220 Hz | 1250 Hz | 10 kHz |
| --- | --- | --- | --- | --- |
| flat, saturation off | none | none | none | none |
| flat, saturation on | none | none | none | none |
| +8 dB at 40 Hz, saturation off | none | none | none | none |
| +8 dB at 40 Hz, saturation on | **-81.0** | -96.9 | -124.7 | -147.1 |

"none" is below -150 dB, which is this plugin measuring as exactly linear.
Saturation switched on with a *flat* curve does nothing at all. Switched on
with a band boosted, the distortion appears in the boosted band and falls
away 66 dB across the band. That is the claim, isolated.

### The size of it: Manley Massive Passive

Flat, the Massive Passive is a broad, quiet tube stage: h2 at -64.3 dB and
essentially no 3rd anywhere. Boost a band and a 3rd harmonic appears, centred
on the band:

| setting | h3 at 40 Hz | 110 Hz | 220 Hz | 630 Hz | 1250 Hz | 5 kHz |
| --- | --- | --- | --- | --- | --- | --- |
| flat | -105.4 | -102.4 | -102.5 | -102.5 | -105.0 | -100.4 |
| low shelf, +15.8 dB at 40 Hz | **-37.9** | -34.2 | -41.3 | -55.7 | -66.1 | -89.4 |
| low-mid bell set to 560 Hz, peak +11.9 dB at 696 Hz | -100.7 | -73.4 | -54.9 | **-43.9** | -46.9 | -62.3 |

**The distortion peak moves with the band.** Set the low shelf and it lives at
40-110 Hz; move to the 560 Hz bell and it lives at 630 Hz.

And the magnitude is the part that matters for implementation. A static
shaper placed *after* the EQ would also give band-dependent distortion,
simply because the boosted band arrives louder — but by a predictable amount.
For a cubic term the 3rd harmonic grows as amplitude squared, so 15.8 dB of
boost predicts 31.6 dB more 3rd harmonic. **Measured: 67.5 dB.** The real
unit produces 36 dB more than a post-EQ shaper can, which settles the
structural question rather than leaving it to taste.

The Pultec misses in the other direction: 16.4 dB of low boost predicts
16.4 dB more 2nd harmonic from a post-EQ shaper and delivers 10.8 dB. So one
reference unit is far more nonlinear than a shaper-after-EQ would be and the
other is noticeably less. Neither is fitted by moving a coefficient; both
need the nonlinearity inside the network, which is what the filter-sandwich
structure in `preamp.rs` already does and what an EQ built on it would
inherit.

### Neve 1081, and a switch that only does something when a band is up

VEQ4 flat measures -96.3 dB of 2nd, and its `analog` switch changes *nothing*
measurable at a flat curve — the flat and analogue-off rows are identical to
0.1 dB. Boost a band and the colour arrives, odd-order and confined:

| setting | h3 at 40 Hz | 60 Hz | 160 Hz | 440 Hz | 1800 Hz | 7 kHz |
| --- | --- | --- | --- | --- | --- | --- |
| flat | -100.1 | -100.4 | -100.2 | -99.9 | -100.3 | -100.2 |
| LF shelf +12 at 100 Hz | **-75.7** | -78.6 | -96.8 | -95.1 | -101.6 | -100.5 |
| LMF bell +12 at 560 Hz | -99.6 | -99.4 | -96.8 | **-91.7** | -95.8 | -100.2 |
| HF shelf +12 at 10 kHz | -100.1 | -100.1 | -101.1 | -101.9 | -93.2 | **-84.4** |

### Curves worth having anyway

Since the sweep was running, the curve of every band at several gains is in
`out/eq/`. Three findings that a textbook biquad would have got wrong:

- **The Massive Passive's gain knob does not set the gain.** At the 560 Hz
  bell with the boost control at its maximum throughout, the bandwidth
  control alone moves the peak over a **16.5 dB** range, and moves the width
  with it:

  | bandwidth | peak | Q |
  | --- | --- | --- |
  | 1.0 | +4.7 dB @ 696 Hz | 0.08 |
  | 2.0 | +11.9 dB @ 696 Hz | 0.69 |
  | 3.0 | +21.2 dB @ 696 Hz | 2.17 |

  A biquad takes gain and Q as independent numbers. This one does not: the
  pair of knobs jointly decide both, which is what a passive network does and
  is why the unit is described as hard to set and good when set.
- **The Pultec's boost-and-cut-together setting is a real curve**, not a
  cancellation: +16.6 dB at 39 Hz, +14.3 dB at 102 Hz, and **-5.7 dB at
  1 kHz**. The unit's reputation is a measurement.
- **The Neve 1081's bell is symmetric in boost and cut** to within 0.1 dB
  (+8.01 / -7.92 dB at 575 Hz, +5.65 / -5.57 at 1022 Hz), and it is
  proportional-Q: a "+12 dB" setting delivers +8.0 dB at Q 0.61 and a "+6 dB"
  setting delivers +4.0 dB at Q 0.28. Its high-Q switch raises the peak from
  +8.0 to +12.0 dB rather than only narrowing it (Q 0.61 to 1.39).
- **The SSL E and G curves differ in height as well as shape.** Same band,
  same +12 dB setting, same 632 Hz centre: E peaks at +7.3 dB with Q 0.51 and
  G at +9.3 dB with Q 0.40. And SSL Native's EQ is perfectly linear — no
  measurable harmonic anywhere, which is what makes it the clean reference the
  four analogue units are being compared against.

---

## 4. Compressors — timing first, exactly as 06 ranks it

> **A compressor's gain element has its own character** — FET, opto, VCA and
> vari-mu each distort differently — but **the timing is most of the sound**.
> Detector shape, attack and release curves and programme dependence do more
> than the gain element's transfer curve does. Get those right first.

Nine units, one per gain element plus a digital control. Each was driven to
the *same* gain reduction before anything was timed, because the units do not
agree on what the compression control even is — Peak Reduction, Input,
Threshold — and a timing taken at 2 dB of reduction is not comparable with
one taken at 12. Calibration: about 10 dB of reduction on a 2 kHz tone at
-12 dBFS.

### Is the rig telling the truth?

TDR Kotelnikov states its times exactly, so it is the control. Measured
against stated:

| stated attack | measured to 63% | stated release | measured to 63% |
| --- | --- | --- | --- |
| 0.5 ms | 0.88 ms | 20 ms | 15.2 ms |
| 1 ms | 1.69 ms | 50 ms | 40.7 ms |
| 3 ms | 4.48 ms | 100 ms | 83.2 ms |
| 6 ms | 8.56 ms | 300 ms | 253.7 ms |
| 20 ms | 24.6 ms | 1000 ms | 850.7 ms |
| 60 ms | 59.4 ms | | |

Long settings land within a few percent. Short ones read high, because the
gain trace is the analytic envelope of a 2 kHz carrier and cannot resolve
below about 0.1 ms — so **a measured attack under about 1 ms is a ceiling,
not a value**. That matters for exactly one claim below and is stated where
it does.

### Attack and release, measured against the markings

The two units with marked times both keep their promises about ordering and
neither is literal about the number.

**UAD SSL G bus** — attack markings are close to honest; release markings
run about 3x the measured time to 63%, consistently, which is what you would
expect if the marking describes fuller recovery.

| marked attack | measured | marked release | measured |
| --- | --- | --- | --- |
| 0.1 ms | 0.31 ms | 0.1 s | 77 ms |
| 0.3 ms | 0.44 ms | 0.3 s | 114 ms |
| 1 ms | 0.94 ms | 0.6 s | 222 ms |
| 3 ms | 3.46 ms | 1.2 s | 411 ms |
| 10 ms | 8.88 ms | Auto | 3382 ms |
| 30 ms | 21.4 ms | | |

**Waves API-2500** — the release markings behave the same way, but its
attack has a **floor at about 3.8 ms**: the 0.03 ms and 0.1 ms settings
measure identically, and 3.8 ms is far above the rig's resolution limit, so
this is the plugin and not the measurement. Its fastest marked attack is
two orders of magnitude away from what it does.

**UAD 1176LN** — knob positions rather than times, so this is the number
that did not exist before:

| knob | attack to 63% | knob | release to 63% |
| --- | --- | --- | --- |
| 1 (slowest) | 4.96 ms | 1 (slowest) | 1395 ms |
| 2 | 3.58 ms | 2 | 1318 ms |
| 3 | 2.92 ms | 3 | 1060 ms |
| 4 | 2.00 ms | 4 | 781 ms |
| 5 | 1.44 ms | 5 | 480 ms |
| 6 | 0.92 ms | 6 | 143 ms |
| 7 (fastest) | 0.38 ms | 7 (fastest) | 107 ms |

Release spans 1.40 s to 107 ms against a published 1.1 s to 50 ms, which is
close enough to believe the rig. The attack column runs into the resolution
limit at positions 6 and 7 and should be read as "at most this".

And it has a **two-stage attack**: t63 at position 4 is 2.0 ms while t90 is
514 ms. The grab is immediate and the rest of the reduction takes half a
second, which is a large part of what an 1176 sounds like and is not
representable by one time constant.

**UAD Fairchild 670** — six positions, no separate attack and release, and
the measured releases reproduce the published ordering (0.3 / 0.8 / 2 / 5 /
10 / 25 s) at about a third of the marked value:

| position | attack to 63% | release to 63% |
| --- | --- | --- |
| 1 | 1.52 ms | 104 ms |
| 2 | 1.94 ms | 306 ms |
| 3 | 3.54 ms | 909 ms |
| 4 | 7.10 ms | 1793 ms |
| 5 | 3.52 ms | 1760 ms |
| 6 | 1.56 ms | 1426 ms |

Positions 5 and 6 are the programme-dependent ones, and they show it: the
attack gets *faster* again while the release stays long, which is the
two-stage behaviour those positions are for.

### Programme dependence splits exactly along the gain element

The same release, measured after the unit was held down for 30 ms and for
3 s. Gain reduction still present at 500 ms, and what fraction of the initial
reduction that is:

| unit | element | after 30 ms hold | after 3 s hold | ratio |
| --- | --- | --- | --- | --- |
| UAD LA-2A | opto | 5.5% | 19.5% | **3.6x** |
| Waves CLA-3A | opto | 7.6% | 18.3% | **2.4x** |
| UAD 1176LN | FET | 16.3% | 40.2% | **2.5x** |
| UAD dbx 160 | VCA | 0.0% | 0.0% | 1.0x |
| UAD SSL G bus | VCA | 1.4% | 1.5% | 1.0x |
| TDR Kotelnikov | digital | 0.2% | 0.2% | 1.0x |

**The opto and FET units are strongly programme-dependent and the VCA and
digital ones are not at all.** That is the cleanest split in this entire
document, it falls exactly where the gain element does, and it is the thing
06 says to get right first.

The shape matters as much as the ratio. An LA-2A after a long hold is down
5.5 dB at 100 ms, 2.0 dB at 500 ms and still 1.2 dB at a second; a dbx 160
is *completely* recovered by 100 ms. Those are not the same envelope with
different constants — one has a long tail and the other has none.

### Detector shape: which unit is actually RMS

A tone and a 20%-duty bursting tone of the same peak have very different RMS
(7 dB apart). A peak detector reduces both about the same; an RMS detector
treats the bursts like the quieter tone. Gain reduction, dB:

| unit | element | tone | bursts, same peak | tone at the bursts' RMS | reads as |
| --- | --- | --- | --- | --- | --- |
| UAD LA-2A | opto | 11.0 | 9.9 | 5.4 | peak |
| Waves CLA-3A | opto | 10.9 | 8.6 | 5.6 | peak |
| UAD 1176LN | FET | 10.7 | 10.2 | 5.1 | **peak** |
| UAD Distressor | FET | 11.2 | 9.9 | 5.6 | peak |
| UAD SSL G bus | VCA | 11.1 | 10.7 | 6.1 | peak |
| UAD Fairchild 670 | vari-mu | 10.4 | 9.8 | 5.1 | peak |
| TDR Kotelnikov | digital | 10.1 | 7.1 | 4.4 | mixed |
| Waves API-2500 | VCA | 8.3 | 4.4 | 3.4 | **RMS** |
| UAD dbx 160 | VCA | 9.7 | 4.8 | 4.5 | **RMS** |

The dbx 160 lands within 0.3 dB of the same-RMS tone, which is true RMS
detection and is the thing the dbx 160 is famous for. The API-2500 does the
same. Everything else is peak-sensing, the 1176 most emphatically.

### The gain element's own distortion, ranked last on purpose

At about 10 dB of gain reduction, harmonics in dB below the fundamental:

| unit | 60 Hz h2/h3 | 1 kHz h2/h3 | 5 kHz h2/h3 |
| --- | --- | --- | --- |
| UAD 1176LN | -38.7 / -44.0 | -62.8 / -65.7 | -72.6 / -69.0 |
| Waves CLA-3A | -87.4 / -41.4 | -115.7 / -64.9 | -127.3 / -75.7 |
| UAD SSL G bus | -66.1 / -34.8 | -89.1 / -61.4 | -110.0 / -88.7 |
| TDR Kotelnikov | -99.9 / -39.6 | -148.5 / -63.9 | -146.2 / -78.1 |

Note the last row. Kotelnikov is a clean digital compressor with no modelled
analogue anything, and at 60 Hz it makes 1.1% THD — *more* third harmonic
than the SSL. That is not a gain element at all: at 60 Hz the detector can
follow the waveform itself, and the distortion is the envelope's. It is the
strongest possible argument for 06's ranking, since the unit with no
character to model produces character-shaped distortion purely out of its
timing.

### Static curves

Gain reduction against input level, which gives threshold, knee and the ratio
actually delivered. All nine are in `out/comp/`; the shape worth naming is
the dbx 160's, whose "over easy" knee is visible as the shallowest onset of
the nine — 0.6 dB of reduction at -24 dBFS where the SSL G bus is already at
2.8 dB, converging to within 1.3 dB of it by 0 dBFS.

---

## 5. What this leaves

Three things are now settled by measurement and can just be built. Two are
decisions that belong to Adam, because the references disagree with each
other or with the model's current shape, and picking for him would be
picking a sound.

### Settled

1. **An EQ's nonlinearity goes inside the filter network, not after it.**
   The Massive Passive makes 36 dB more third harmonic than a post-EQ shaper
   can, and the Pultec makes 5.6 dB less. No coefficient reaches either. The
   filter-sandwich structure `preamp.rs` already uses is the one that does.
2. **Compressor programme dependence is the opto/FET signature and the VCA
   units do not have it.** Opto and FET units hold 2.4x to 3.6x more gain
   reduction at 500 ms after a long note than after a short one; the VCA and
   digital units are within 1%. If the strip's compressor is meant to feel
   like an 1176 or an LA-2A, that ratio is the thing to build, and it is
   independent of the gain element's transfer curve.
3. **Detector shape is a per-unit choice with real consequences**, and the
   two units that measure as RMS rather than peak — dbx 160 and API-2500 —
   give 5 to 6 dB less gain reduction than the peak-sensing units on the same
   bursting signal.

### Adam's calls

4. **Is `Iron` a mic preamp or a console channel?** They do not measure
   alike and only one of them is even-order. A 1073 *preamp* driven is even
   by 10.6 dB and makes -19 dB of 2nd harmonic; a Neve *console channel* is
   odd by 9.8 dB and makes -70 dB. `IRON` is currently authored as the
   preamp reading and named for the console one. The tilt — the "warm on a
   kick, clean on a hat" mechanism, and the most distinctive thing in these
   measurements — belongs to the console channel, not the preamp.
5. **Should `Grip` equalise?** The SSL reference's fat low end is a
   drive-dependent low shelf reaching +9 dB at 40 Hz, not frequency-dependent
   distortion. `Preamp`'s tilt pair is exactly reciprocal on purpose, so the
   stage colours without equalising and a test enforces it. Adding a
   drive-dependent shelf is a deliberate change to what the stage is allowed
   to do, not a tuning.

### Not supported by anything here

`PreampVoicing::slew`. No unit measured behaves differently on a fast edge
than its own static curve predicts. `Punch`'s `slew: None` matches the
references; `Iron`'s `slew: Some(0.08)` is an invention that may sound right
and cannot cite this document.

### If someone re-runs this

The scripts take a slug and write JSON; adding a unit is a dict entry in
`units.py`, `eq_units.py` or `comp_units.py`. What is *not* measured, and
would be worth having:

- **Supply sag**, item 4 on 06's list. It needs a different stimulus again —
  a sustained tone with a loud interruption, watching the gain of the tone
  rather than of the interruption — and nothing here does that.
- **Intermodulation.** Everything here is single-tone. A two-tone test says
  things about a nonlinearity that harmonics do not, and it is the same rig.
- **The remaining UAD units.** 610-B, Century, API Vision, Fairchild 660,
  Studer A800 and the Distressor's `Dist 2`/`Dist 3` audio modes are all
  loaded and licensed and were probed but not swept.
