# 06 — Preamp modelling

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

Asymmetry is also how you *get* even orders from a symmetric curve: a DC bias
into the shaper pushes the signal onto an uneven part of it.
`shaper.rs::tape` already does exactly that trick and subtracts the bias's own
DC back out.

### 2. Frequency-dependent drive — the transformer, and the biggest missing piece

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
- **A compressor's gain element has its own character** — FET, opto, VCA and
  vari-mu each distort differently — but **the timing is most of the sound**.
  Detector shape, attack and release curves and programme dependence do more
  than the gain element's transfer curve does. Get those right first.

## Verification

The flat harmonic target is already tested. The addition this step needs is
the **frequency-dependent** one, which is what makes "warm on a kick, clean on
a hat" a number rather than an adjective:

> measure the 2nd harmonic at 80 Hz and at 5 kHz, at the same input level, and
> assert the ratio between them.

That test is what says the filter sandwich is doing its job, and it fails
loudly if someone later "simplifies" the tilt away. It extends
`harmonics.rs`'s existing measurement rather than needing new machinery.
