# The strip, as Adam drew it

Adam's mockup, 2026-09-09, delivered mid-step-02:
[`img/strip-mockup.png`](img/strip-mockup.png) (and the `.svg` beside it).

It is one vertical strip containing, top to bottom:

```text
  pre in / drive          [ Moo | Grip | Punch | Iron ]   <- strip mode
  eq in
      high shelf    high mid
      low mid       low shelf          freq / gain / q each,
                                       the shelves switchable to bell
  comp in
      thresh   ratio
      in trim  attack   rel
      knee     w/d mix  makeup
  four send bars
  meter   fader                    mute
                                   solo
                                   polarity

                                   analog sum
```

The mockup drew the strip mode as `moo / ssl / api / neve`. **Those names do
not ship**, by Adam's instruction on the day he drew it: a voicing is named
for the sound it goes for, not for a company. See "The four voicings" below.

## What it settles

**The channel strip is not a device you insert. It is the strip.** This is
the one place the mockup contradicts the written plan, and the mockup is
right — it is decision 1 of the `README.md` (*the mixer is a view of strips*)
carried through to the face. `03-the-channel-strip-device.md` proposed a
composite `EffectKind` hosted on a channel or a bus; what Adam drew is a
console strip that every strip *has*.

Those are not as far apart as they look, and the reconciliation is worth
stating because it is what keeps step 03 buildable: **the shared DSP units do
not change, and neither does the "free while it is out" rule.** What changes
is where the thing lives and how it is reached — a fixed section of the strip
rather than a row you add to a chain. `is_at_rest` / `skip_block` are what
make an EQ that is flat and a compressor that is below threshold cost nothing
on all 256 strips.

**The voicing is strip-wide, and it sits with the preamp.** The mode selector
is at the *top*, next to `pre in / drive` — so one selector governs the
preamp, the EQ curves and the compressor's time constants together. The plan
had the voicing as a mode on the channel-strip device alone. Strip-wide is the
stronger reading and the more console-like one: none of the desks this is
after gets its character from its compressor alone.

This also puts step 06 (preamp modelling) back in the frame earlier than the
plan parked it — not as a separate device at the head of a chain, but as the
strip's own input stage, which is the same control the voicing selector is
already next to.

**Sends are four bars, inline.** Not a list, not a dialog: four horizontal
faders in the strip. That is a capacity decision as well as a layout one, and
it belongs in `CAPACITY_POLICY.md` terms — four is a *drawn* limit rather than
an engine one, so the model should still carry a `Vec` and the face should
show four.

**"Analog sum" is the name, and it goes at the foot.** Adopted in step 02 on
the day the mockup arrived: the mixer strip now carries the switch at the
bottom, set apart from mute rather than beside the destination picker where it
was first put. Set apart is right — it is the last thing that happens on the
way out, after the fader, and it is not an output control the way mute and pan
are.

The interface says **analog sum**; the code says `console`, because that is
the technique's name and what `mooloop_dsp::console` and every document about
it are written around. `ConsoleButton`'s doc comment records the split so
nobody reconciles it in the wrong direction.

## The four voicings

Adam, 2026-09-09: *"dont reference these by name, can use a 'character name'
that sorta describes what we've gone for in the sound."* So the selector reads
as four sounds, and the names are the specification rather than a label over
one:

| Name | The sound it goes for |
| --- | --- |
| **Moo** | The house voicing. Calibrated and uncoloured: no added harmonics, the EQ curves the project chose for itself, and the compressor's own timing. This is what the strip does when it is not pretending to be furniture. |
| **Grip** | Fast, tight and controlled. Glue rather than warmth: a quick detector, a firm knee, a slightly forward upper-mid, and low distortion that is mostly odd-order. |
| **Punch** | Forward transients and thick low-mids. Slower to clamp, louder-feeling at the same level, with a harmonic mix strong enough to hear on a snare. |
| **Iron** | Transformer warmth. Even-order dominant, a rounded top, and a low end that swells rather than tightens — the one whose distortion is the point rather than a side effect. |

Adam's own `moo` is kept, because it is his word for the house sound and it is
already the odd one out: the other three go somewhere, and `Moo` is where the
strip already is.

## The distortion is part of it, and it can be a number

Adam: *"note its not just curves im hoping to capture...idk if we can do it but
some THD that's on-target would be really cool."*

**It can be, and more precisely than "sounds warm" — with two honest limits.**

A memoryless waveshaper's harmonic output is *directly specifiable* through
its Chebyshev decomposition: because `T_n(cos θ) = cos(n θ)`, a shaper written
as `Σ aₙ Tₙ(x)` feeds a unit-amplitude sine straight back out as harmonic `n`
at amplitude `aₙ`. So a voicing's distortion can be authored as a **target
table** — 2nd at −46 dB, 3rd at −58 dB, and so on, at a stated input level —
and the polynomial solved for rather than dialled in by ear. That also makes
it *testable*: render a sine at `gain::REFERENCE_PEAK_DBFS` through each
voicing and assert the harmonic amplitudes against the table. `SpectrumAnalyzer`
already exists, so the measurement costs nothing to build.

**Built and proven, 2026-09-09**, in `mooloop_dsp::harmonics`: every voicing's
stated profile is what a full-scale sine measures coming out of it, to 0.00 dB.
`harmonics_hit_their_stated_target` is that claim as a test.

The two limits, stated up front so the target is not quietly missed:

1. **The exact correspondence holds at one amplitude, and below it the
   character both thins *and changes shape*.** Building it turned up something
   the first draft of this file got wrong: there is no simple `a^(n-1)` law
   once a profile has more than one term. Off full scale every `Tₙ` spills
   into harmonics `n-2`, `n-4`…, so `T₄` feeds the 2nd against `T₂` and the
   two partially cancel — `Iron`'s 2nd falls 9 dB per halving, not 6. The
   module header carries the worked algebra and a test pins the number.

   That is wanted, not a flaw: `reference/ADAM.md` asks for colour that
   *"reacts to level"*, and a distortion whose spectrum is the same at −30 and
   −6 dBFS is the one that sounds like a plugin.

   **But it means the profile numbers cannot be authored on their own.** At
   −18 dBFS the first-pass `Grip` profile has receded past −110 dB, which is
   nothing; "−28 dB of 2nd harmonic" says nothing without saying what level
   arrives at the curve. The stage that decides that is `pre in / drive`,
   normalizing into the shaper the way `shaper::drive_compensation` already
   does for the Drive device. **Author the two together, with ears.**
2. **Real transformer distortion is frequency-dependent and hysteretic, and a
   memoryless shaper is neither.** Core saturation rises steeply toward low
   frequencies, which is most of why `Iron` sounds like `Iron` on a kick and
   nearly clean on a hat. The cheap and well-understood approximation is to
   drive the shaper harder at low frequencies — a tilt into the shaper and its
   inverse after — and that is worth building for `Iron` specifically rather
   than for all four.

What is *not* being claimed is a match to a measured unit. The claim is a
stated harmonic target, hit at a stated level, and tested.

## Two things the plan did not have at all

**Solo.** `LOOSE_ENDS.md` has been carrying it: `SoloButton` exists in
`controls.slint` with a `soloed` property and `mooloop-core` has no solo state
whatever. `MIXER_PLAN.md` specifies the intended behaviour and it is not a
routing change — an AFL-style monitor tap. Now that it is drawn on the strip
it needs a step of its own; it is the largest of the unbuilt things here,
because a monitor tap is a second output path rather than a control.

**Polarity.** A per-strip phase invert, which is one multiply in `OutputStage`
and one defaulted boolean in the project format. It is the cheapest thing on
the whole mockup and it belongs with whichever step next opens that struct.

## The three questions the drawing left, answered the same day

**The EQ is four bands, read left to right and top to bottom.** Adam: *"this
has been annoying ppl since EQs were invented. there's no good way to lay it
out. i chose ltr ttb."* So: **high shelf, high mid, low mid, low shelf**, from
the top-left to the bottom-right. Each band is freq / gain / q; the two
shelves switch to bell. That reading order is the layout's whole rule and it
should be stated in the face's comment, because the next person to look at it
will assume the two columns mean something.

**The sections collapse.** The strip is tall, and rather than a second compact
variant it gets a show/hide toggle per section — preamp, EQ, compressor,
sends. Adam: *"if we need to squish, there's room to squish the fader's top
downward but it should have some kind of reasonable min height."* So the
output section is the elastic one and the fader has a floor; the sections
above are the ones that go away entirely. That is a better answer than a
compact variant, because there is then only one face to build and one to
maintain, and what a user hides is a choice rather than a mode.

**The compressor is its own, and `w/d mix` is parallel balance.** Adam: *"this
is a totally different comp than the one built in as a device. w/d is just a
wet/dry balance control for parallel per strip."* So the strip compressor is
not `EffectKind::Compressor` behind a different face — it is a second design,
voiced by the strip mode, and the mix knob is a straight dry/wet balance for
parallel compression rather than the generic device-host blend the effect
container already provides.

That narrows step 03's reuse claim rather than deleting it: the *primitives*
under `COMPOSABLE_DEVICE_UNITS.md` — the detector, the gain computer, the
shared `Biquad` — are still what it is built from. What it is not is a wrapper
around the existing device.

## Still open

- **How much a rack row shows.** Section toggles answer the mixer strip's
  height; they do not answer what a channel's *rack row* shows, which is the
  same question step 04 asks about groups.
- **How each voicing's harmonic profile travels with level**, per the first
  limit above. That is authoring, and it wants ears.

## Order this implies

Unchanged through step 04, and then the back half compresses: the strip face
is one build that carries the EQ, the compressor, the voicing and the preamp
together, because they are one face and `AGENTS.md`'s device-work order says
to cross into `main.slint` once. Sends and solo stay separate, because each is
a signal-path change rather than a face.
