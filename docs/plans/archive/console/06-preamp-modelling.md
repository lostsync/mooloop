# 06 — Preamp modelling

> **Measured 2026-09-10 — [`spikes/preamp-measure/RESULTS.md`](../../../../spikes/preamp-measure/RESULTS.md).**
> The sweeps this file asks for at the end have been run against 25 licensed
> plugins. Most of what is reasoned below survived; three claims did not, and
> each is marked **Measured** in place rather than deleted, because the
> reasoning that produced them is still the reasoning worth having.
>
> **Partly settled by Adam's mockup, 2026-09-09 — [`THE-STRIP.md`](THE-STRIP.md).**
> It is not a device at the head of a chain: it is the strip's own input stage,
> `pre in / drive`, drawn at the top beside the **Moo / Grip / Punch / Iron**
> selector that governs the whole strip. So this step is about the *modelling*
> — what each voicing's input stage does — rather than about where the control
> goes.

Adam, the same day: *"for starters i just straight dont know how you model a
preamp."* So this file answers that first, then turns his brief for each
voicing into mechanisms.

## What a preamp model is actually made of

Five things, in rough order of how much sound you get per unit of complexity.
A model that stops after the first two is already recognisable; the rest are
refinements.

### 1. A static nonlinearity, and its harmonic balance

The gain stage's transfer curve. The single most useful fact about it is
**topology decides the harmonic order**:

- **Single-ended class A** — asymmetric transfer, so **even-order dominant**,
  and the 2nd harmonic is what people hear as *warm* or *thick*.
- **Push-pull or differential** — the symmetry cancels even orders and leaves
  **odd**, and the 3rd is what people hear as *hard*, *edgy*, or on cymbals,
  *harsh*.

That one distinction carries most of the difference between "I want to drive
this" and "I want to back off". `mooloop_dsp::harmonics` already makes this
number authorable and testable — a profile is written down and hit to 0.00 dB.

> **Measured, and the rule holds — but it was pointed at the wrong box.**
> The topology rule is right and the measurements separate on it cleanly.
> What they also show is that *"a Neve"* is two different stages, and only
> one of them is single-ended:
>
> | Neve-derived unit | models | h2 - h3 |
> | --- | --- | --- |
> | Scheps 73, drive in | 1073 **mic preamp** | **+10.6 dB**, even |
> | NLS Nevo | 5116 **console channel** | **-9.8 dB**, odd |
> | bx_console N | VXS **console channel** | **-66.8 dB**, odd |
>
> A console channel is a chain of push-pull line amps and measures odd, hard.
> The even-order warmth people mean by "Neve" is the mic preamp being leant
> on. So **`Iron` has a choice to make that this document has not noticed:
> preamp or channel.**
>
> **Answered 2026-09-10 by the wider pass, and it is a third option.** Every
> Neve-derived channel in the 106-unit survey measures odd-dominant, so the
> even-order character `Iron` is named for is not a Neve channel at any
> drive. It is the **EMI TG12345** — NLS "Spike", 2nd 11 dB over its 3rd and
> holding that from −24 dBFS to −6. `IRON` is now authored from it, and stays
> even-dominant without pretending a push-pull channel is.

Asymmetry is also how you *get* even orders from a symmetric curve: a DC bias
into the shaper pushes the signal onto an uneven part of it.
`shaper.rs::tape` already does exactly that trick and subtracts the bias's own
DC back out.

### 2. Frequency-dependent drive — the transformer, and the biggest missing piece

> **Built 2026-09-09** as `mooloop_dsp::preamp`, with provisional numbers, on
> Adam's ruling: *"we can do some sweep measurements on UAD and Waves plugs
> later to try and get some real numbers. if we need them before then lets
> just pick some."* `PreampVoicing` is deliberately a table of numbers with no
> behaviour attached, so a measured fit replaces four rows and nothing else.
> **The harmonic half of that fit landed 2026-09-10**; `tilt_db`, `tilt_hz`
> and `slew` are still picked.
> Two things the building found are recorded at the end of this section.

**This is the one that turns "a shaper" into "a preamp".** A transformer's
core flux is proportional to `V/f`: for a constant voltage, halve the
frequency and you double the flux. So a transformer saturates **from the
bottom up** — which is the whole reason iron-sounding gear is obvious on a
kick and nearly clean on a hi-hat, and why Adam's SSL note (*"except maybe in
the low end where you get a nice fat round sound"*) describes a real mechanism
rather than a preference.

`harmonics.rs` says in its own header that it cannot do this, because it is
memoryless. The standard fix is a **filter sandwich**, and it is cheap:

```text
   tilt (boost lows)  →  static shaper  →  inverse tilt (cut lows)
```

Low frequencies arrive at the curve hot and saturate first; the inverse tilt
puts the frequency response back where it started, so what is left is
frequency-dependent *distortion* rather than frequency-dependent *level*. Two
biquads and the polynomial that already exists.

This is a **Wiener–Hammerstein** model — linear filter, static nonlinearity,
linear filter — which is the standard block-oriented structure for exactly
this class of device, and a great deal of analog gear is well approximated by
it.

> **Measured: the mechanism is real, large, and not universal.** NLS's Neve
> console model has **23.8 dB** more 2nd harmonic at 80 Hz than at 5 kHz,
> falling as a first-order shelf with a half-way corner at **240-250 Hz** —
> which is `IRON_PREAMP`'s 220 Hz guess almost exactly, with a tilt about
> 10 dB deeper than the 14 dB it is set to.
>
> But FrontDAW's five styles have a tilt of **zero**, every one of them, and
> the SSL units tilt slightly the *wrong way* (-14 dB on SSL EV2: more
> distortion at 5 kHz than at 80 Hz). Two respected emulations of the same
> class of hardware disagree about whether this effect is part of the sound
> at all. That is not a reason to drop it; it is a reason to keep treating
> `tilt_db` as a voicing decision rather than as physics that must be
> present.

#### Cut the top, do not boost the bottom

The diagram above says "tilt (boost lows)" because that is the obvious reading
of *saturates from the bottom up*. **It is wrong in practice, and it was built
before it was noticed.** A 14 dB boost on material at the operating level
walks straight past the shaper's `[-1, 1]` domain, the monotone clamp takes
over, and what comes out is a square wave: all odd harmonics, `Iron`'s 3rd
sitting 31 dB *above* its 2nd, and — the tell — **more drive producing less**
2nd harmonic.

Attenuating the top instead is the same relative tilt with the loudest part of
the signal left where the curve can still shape it. Same two biquads, same
exact-inverse property, none of the blow-out.

#### And do not derive anything about a profile

The corrected structure suggested a lovely property: since the post-filter
restores fundamental and harmonics by the same factor, `tilt_db` ought to *be*
the dB difference in distortion across the corner. It was claimed, tested, and
false — `Grip` states 10 dB and delivers 31.

The reason is the level law `harmonics.rs` records: a harmonic moves faster
than the signal that makes it — `n-1` dB per dB for the `n`th — so holding the
top of the band back by `tilt_db` drops its harmonics by rather more, and how
much more depends on which harmonic and where the profile sits.

*(The original reading of this was an interaction between `T₂` and `T₄` that
nearly cancelled. The 2026-09-10 rebase dropped the 4th and 5th from the
profile entirely — see `harmonics.rs` on why they cannot be authored at the
operating level — so the cancellation is gone and the level law is the whole
of it. The conclusion below is unchanged, and was reached twice.)*

That is twice that a clean closed-form claim about this scheme has been wrong
in the same way, which makes it a rule rather than a coincidence: **derive
nothing about a profile, measure it.** `tilt_db` is a control to be turned,
not a specification to be read off, and the test that survives asserts
monotonicity rather than a law.

### 3. Slew limiting — the transient half

A static curve cannot tell a fast transient from a loud steady tone. A slew
limiter can: clamp how far the output may move per sample and the curve
becomes a function of `dV/dt` rather than of level.

This is most of what "fast" means about a discrete op-amp, and it is the
difference between a snare that is *grabbed* and one that is *smeared*. It is
also about four lines of code.

### 4. Supply sag

Under load the rails droop and the stage's gain falls for a few milliseconds,
recovering after. A slow, signal-dependent gain change — audibly a kind of
compression nobody switched on. Worth having on the voicings whose hardware is
known for it; not worth having on all four.

### 5. Hysteresis — the proper answer, and probably not this one

A real magnetic core has *memory*: its B–H curve depends on where it has
been, not only on where it is. The published model is **Jiles–Atherton**
(1986), and Jatin Chowdhury's DAFx-19 work on real-time hysteresis for tape is
the practical reference for making it run per-sample. It is an ODE solve per
sample and a genuine step up in cost and difficulty.

Worth knowing it exists and is the rigorous option. Not worth reaching for
before 1–3 have been heard.

## What is published, and the one number that usually is not

There is a lot on *how* to do this — block-oriented nonlinear modelling,
Jiles–Atherton, the standard virtual-analog literature. There is much less of
the specific number this approach wants.

**Total THD is published and per-harmonic breakdown usually is not.** A spec
sheet gives "THD < 0.05% at +20 dBu"; it does not say how that splits between
the 2nd and the 3rd, and the split is the part that decides whether it sounds
warm or hard. So the practical route is:

- **topology decides the ratio** (single-ended → even, push-pull → odd),
- **published THD sets the magnitude**,
- **ears settle the rest.**

If Adam ever has one of these units, or material recorded through one, a sine
sweep at several levels gives the harmonic-versus-frequency-versus-level
surface directly, and `harmonics.rs` can be fitted to it rather than authored.
Absent that, the above is the honest path, and it is enough for what he asked
for: *"i'm really not super concerned with for example making a perfect
1073pre in software, but it just needs to kinda hit that vibe if you drive
it."*

## The brief, and what each voicing is made of

Adam's own words for each, and the mechanism that produces it.

### Iron — *"a little warm, mid forward"*

- Even-order dominant: 2nd well above 3rd.
- **Frequency-dependent drive, strongly.** This is where it earns its name;
  the warmth should arrive on a kick and be nearly absent on a hat.
- A broad presence lift as a fixed voicing curve — the "mid forward" is
  ordinary EQ and is a large part of the identity.
- A gentle top-end bump and rolloff, which is what an output transformer's
  leakage inductance and winding capacitance do.

### Grip — *"cleanest per dB of gain … the breakup would probably make you want to back off rather than lean in, except maybe in the low end where you get a nice fat round sound"*

The most specific of the three, and it maps onto mechanism almost directly:

- **Low overall harmonic content**, odd-dominant. Odd-order breakup on
  cymbals is harsh, which *is* the "back off" instinct.
- **But keep the frequency-dependent drive**, so the low end still rounds when
  pushed. Same structure as Iron with much less of it and the opposite
  harmonic balance — which is why one structure serves all four.

  > **Measured: the real SSL does this, and not this way.** NLS Mike has
  > 2.3 dB of tilt, essentially none. What it has instead is a low-frequency
  > *gain* that grows with the drive control — **+4.2 dB at 40 Hz** at Drive
  > 6 and **+9.0 dB** at Drive 12, measured at -40 dBFS where nothing is
  > distorting at all. The fat round low end is an equalisation, not a
  > distortion.
  >
  > `Preamp` cannot currently produce that: its tilt pair is exactly
  > reciprocal by construction, so the stage colours without equalising, and
  > there is a test that says so. Giving `Grip` a drive-dependent low shelf
  > is a decision for Adam, not a bug to fix quietly.
- Flattest voicing curve of the coloured three.

### Punch — *"needs to sound like rock and roll"*

- **High slew rate — that is, deliberately little slew limiting.** Transients
  survive intact, which is most of what "fast" and "forward" mean here.
- Both orders present and more of them than Grip.
- A slight upper-mid emphasis, and no transient softening anywhere.

### Moo

Unity, and the null case: with both strip sections out and `Moo` selected a
track is bit-identical to no strip at all.

## The EQ and the compressor are nonlinear too

Adam: *"thing is all of that stuff is nonlinear and i really didnt even talk
about the eqs or comps. but same basic idea."* Correct, and the same toolkit
applies:

- **Inductor-based EQs saturate in the filter path.** The core in a passive EQ
  is a transformer-shaped nonlinearity sitting *inside* the frequency-shaping
  network, so the distortion is band-dependent rather than broadband. The same
  filter-sandwich structure covers it.

  > **Measured, in four units, and the magnitude settles the structure.**
  > The Massive Passive's 3rd harmonic peak *moves with the band*: at the low
  > shelf it lives at 40-110 Hz, at the 560 Hz bell it lives at 630 Hz. And
  > 15.8 dB of boost raises it by **67.5 dB**, where a static shaper placed
  > after the EQ predicts 31.6 dB. The Pultec misses the same prediction in
  > the other direction, 10.8 dB against 16.4. Neither is reachable by moving
  > a coefficient — the nonlinearity has to be *inside* the network, which is
  > what this structure does. SlickEQ isolates it perfectly: same curve, and
  > its saturation switch does nothing at all until a band is boosted.
- **A compressor's gain element has its own character** — FET, opto, VCA and
  vari-mu each distort differently — but **the timing is most of the sound**.
  Detector shape, attack and release curves and programme dependence do more
  than the gain element's transfer curve does. Get those right first.

## Verification

Both halves are now tested in `mooloop_dsp::preamp`.

The **frequency-dependent** measurement is what makes "warm on a kick, clean
on a hat" a number rather than an adjective — the 2nd harmonic at 80 Hz
against the same at 5 kHz, at one input level. Measured, at a modest drive:

```text
   Grip    -62.6 dB at 80 Hz    -93.8 dB at 5 kHz
   Punch   -37.9 dB at 80 Hz    -47.2 dB at 5 kHz
   Iron    -29.4 dB at 80 Hz    -86.2 dB at 5 kHz
```

It fails loudly if someone later "simplifies" the tilt away, which is the kind
of change that looks harmless.

Beside it: the sandwich is flat to 0.01 dB with the curve removed, so the
stage colours without equalising; `Iron`'s 2nd sits 17.5 dB above its 3rd and
`Grip`'s 3rd 16.4 dB above its 2nd, which is the warm-versus-hard axis holding;
driving `Iron` moves its 2nd from -41.7 to -28.2 dB; and a slew limit catches
a step while leaving a 100 Hz tone of the same peak untouched, which is what
says it acts on `dV/dt` rather than on level.

**What is still unbuilt from the list above:** supply sag, and hysteresis.

## The measured targets

`spikes/preamp-measure/RESULTS.md` has the surfaces; the four numbers that
bear on the table above, all at mooloop's -12 dBFS operating level:

| | h2 | h3 | tilt | corner |
| --- | --- | --- | --- | --- |
| NLS Nevo (Neve console), Drive 6 | -69.8 | -59.9 | +23.8 | ~245 Hz |
| NLS Mike (SSL console), Drive 6 | -52.3 | -36.6 | +2.3 | — |
| NLS Spike (EMI console), Drive 6 | -47.0 | -58.1 | +2.3 | — |
| Scheps 73 (1073 preamp), drive in | -19.4 | -30.0 | +0.5 | — |

And the calibration point, which is about `pre in / drive` rather than about
the profiles: the reference units span roughly **-70 dB to -20 dB of 2nd
harmonic** between a console channel at rest and a 1073 leant on, so that is
the range the drive control has to reach. `IRON` as authored (-28 dB, with
its 2nd 18 dB above its 3rd) sits inside that span for amount, but no unit
measured has an even-over-odd margin anywhere near 18 dB — the widest, among
units making more than -50 dB of 2nd, is +11.1 dB.
