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

**Sends are bars, inline.** Not a list of names, not a dialog: horizontal
faders in the strip.

~~four horizontal faders … four is a *drawn* limit rather than an engine one,
so the model should still carry a `Vec` and the face should show four.~~
**Amended by Adam, 2026-09-09**, reading step 05: *"i drew 4 sends bc that's
how many fit in my drawing. if there are no sends, we wouldnt show any. we're
not limiting to 4… if we gain more than will fit, that area should scroll."*

So there is no drawn ceiling either. The area draws exactly the sends that
exist and scrolls past the room it has, rather than the strip growing or the
faders shrinking. `CAPACITY_POLICY.md` gets the plainer version of its own
rule: nothing reserves for a number of sends anywhere, in the model, in the
plan or in the face. See [`05-sends.md`](05-sends.md).

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

**Built and proven, 2026-09-09**, in `mooloop_dsp::harmonics`, and **re-based
on measurement 2026-09-10**: every voicing's stated profile is what a sine
*at `gain::REFERENCE_PEAK_DBFS`* measures coming out of it, to 0.00 dB, and
every profile is now read off a named unit in the 106-unit survey rather than
picked. `harmonics_hit_their_stated_target_at_the_operating_level` is that
claim as a test.

The two limits, stated up front so the target is not quietly missed:

1. **The correspondence holds at one amplitude, and that amplitude is the
   operating level.** The first version authored at full scale, which is
   exact where the Chebyshev identity is exact and wrong everywhere music is:
   `Iron` claimed a 2nd harmonic "near −40 dB at the operating level" and
   delivered −44, with its 4th and 5th at −84 and −108 dB. The shaper now
   *solves* for the coefficients that hit the profile at −12 dBFS.

   That is why the profile carries **two** harmonics and not four. `Tₙ`'s own
   output scales as `aⁿ` while its spill into the fundamental scales as `a`,
   so at `a = 0.251` a `T₅` big enough to make an audible 5th costs 15 dB of
   fundamental. Nothing is lost: the four-harmonic form delivered its 4th and
   5th 80 dB down at the operating level anyway.

   The character still reacts to level, which is what
   `reference/ADAM.md` asks for — a distortion whose spectrum is the same at
   −30 and −6 dBFS is the one that sounds like a plugin. What a fixed
   polynomial cannot do is react at the *rate* real units do: it forces a
   harmonic up `n-1` dB per dB, where the survey's 186 unit-settings median
   +0.79 dB/dB for the 2nd and +1.28 for the 3rd. So the voicings hold their
   balance at and below the operating level and flatten above it, where the
   references hold theirs about ten times further.

2. **Real transformer distortion is frequency-dependent and hysteretic, and a
   memoryless shaper is neither.** Core saturation rises steeply toward low
   frequencies, which is most of why `Iron` sounds like `Iron` on a kick and
   nearly clean on a hat. The cheap and well-understood approximation is to
   drive the shaper harder at low frequencies — a tilt into the shaper and its
   inverse after — and that is worth building for `Iron` specifically rather
   than for all four.

Since 2026-09-10 a match to a measured unit *is* claimed for the harmonic
half: `Grip` is Waves NLS "Mike" (SSL 4000 G+), `Punch` is the UA 610-A, and
`Iron` is NLS "Spike" (EMI TG12345), each at the drive setting where it makes
1% THD at the operating level. `tilt_db`, `tilt_hz` and `slew` are still
picked rather than fitted.

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

## Both sections default to out

Adam: *"comp and eq yes, every track, but defaults to turned off. control for
this is via the 'eq in'/'comp in' buttons."* The two toggles the mockup
already draws are the whole mechanism, and they answer the 256-track cost
question outright -- a section that is out does not run, so the price of the
strip existing everywhere is two booleans per track.

## Where the strip is drawn, settled 2026-09-10

The mockup above is the strip at full size, and this file never said where a
user *meets* it. In the ordinary paned layout the mixer holds one slot of a
shared work area, at whatever height the other views leave it; the only answer
the plan had was that the sections collapse, which makes the face shorter
without making the pane usable. Adam's answer is that
**the strip has two faces, and the mixer has three states**:

```text
  front (paned)        back (paned)          zoomed console
  ─────────────        ────────────          ──────────────
  name                 name                  the drawing at the top of
  meter   fader        meter  vol  pan       this file: every section at
  mute solo pol        ┌─────────────────┐   once, one strip per track,
  -> dest              │ EQ COMP DRV SND │   scrolling horizontally
  analog sum           │                 │
                       │   one page      │   no turn-over here: the
           [ over > ]  │        [ back ] │   room is the whole point
                       └─────────────────┘
```

The front is what you look at while **mixing**. The back is what you look at
while **setting one track up**. The zoomed console is the desk, and it needs
no turn-over because it has the room for the whole strip at once.

**The turn-over is a small button in the strip's lower-right corner.** One
button per strip, so one track can show its EQ while everything around it
still shows faders -- which is the comparison a global flip would take away.

**It does not have to be a literal flip.** Adam, 2026-09-10: *"it doesnt have
to literally flip. im open to what animation is used... we're already
animating the panes so even tho slint clearly isnt quickshell, i feel like we
should be able to do a decent transition."* Slint 1.17 has no 3D transform, so
a card flip is an x-scale through zero with the faces swapped at the midpoint;
a cross-dissolve with a small slide is equally available and may read better
at 92px. Whichever it is, it spends `Motion.duration` and `Motion.curve` --
the tokens the device rack's slide-aside and the dock's extent already animate
on -- rather than introducing a second timing. It is worth real work rather
than a state swap: *"this part of the app is probably one of the most likely
to be the focus of any public attention so there's a reason to get it right
and make it slick when we do."*

**The meter, the level and the pan survive the turn**, along with the name.
Two reasons, and the second is the load-bearing one: an EQ is set by ear while
watching what it does to the level, and a face that shares nothing with the
one it replaced reads as a different panel arriving rather than as this strip,
turned round.

**The pages, in order: EQ, COMP, DRIVE (carrying the voicing selector),
SENDS.** Sends last because they are the one section that also lives on the
track's rack face, so they are the one a user has somewhere else to reach.
Each page carries its own `in` switch -- `eq in` / `comp in` are already the
mechanism, and a page whose section is out should say so rather than drawing
live-looking knobs that change nothing.

## 92px, and why it is what removes the complication

Adam, 2026-09-10: *"white tie imperial makes it work at 92px. why dont we just
adopt that size fully?"* Adopted, for both faces and the zoomed console.

The arithmetic is the argument. A strip is 62px today with 4px padding, so
54px of content, and `MiniKnob` is 22px: **two knobs to a row, never three.**
At 92px there are 84px of content, three knobs and their gutters are 74px, and
so **an EQ band is one row** -- freq, gain, q -- with four bands in four rows.
The whole reason the back face wanted to be wider than the front was that one
number.

It pays on the front as well. Meter (22px) plus fader (30px) leaves 24px for a
column beside the fader, which is exactly where the mockup stacks mute, solo
and polarity. At 62px that column is what does not fit, which is why today's
strip puts pan and mute on a row *below* the fader instead. So 92px is not
merely room for the back: it is what lets the paned strip be a small version
of the drawn one rather than a different arrangement of the same controls.

What it costs is strip count: a ~1200px mixer pane shows about thirteen strips
where it showed about nineteen. `MixerPane` already scrolls horizontally and
was built to, so this is comfort rather than capability.

Two ideas it retires, both recorded because they were the obvious answers:

- **Widening a strip when it turns over**, neighbours sliding aside. Slick,
  and unnecessary once both faces are 92px -- and a strip that changes size
  when you turn it is harder to read as the same object.
- **A section that toggles open below the fader.** An area that opens makes
  the strip *taller*, and the strip's height is the fader's; the fader is the
  one thing on it that cannot shrink. A turn-over changes nothing's size.

**92 gets a name, not a literal.** `MixerMetrics.strip-width`, the way
`DeviceRackMetrics` already holds the rack's numbers. The front face, the back
face, the zoomed console and `tests/mixer_snapshot.rs` all want it, and a
number spelled in four places that drift apart is the fault `AGENTS.md` opens
on.

## The strip's processing is a pinned rack row

The other half of where the strip is drawn. Adam, 2026-09-10, asked for the
strip's processing to be reachable from the **device rack**, as a row in the
track's chain: *"own row, pinned, but we should be able to just move the pin
or morph what is drawn."*

That is a narrower departure from this file's opening ruling than it looks.
The strip is still not a device you *insert*: it cannot be added, deleted or
duplicated, and every track has one whether or not anything is switched in.
What it gains is a position in the chain, which is a fact the rack can draw --
and the pinned position is then a **policy stated in one place**, not an
assumption spread through the compiler. Moving the pin later, or morphing what
that row draws, should be an edit to that one statement.

The track's rack face already exists and already does half of this: `BusDeviceFace`
(`ui/bus-device.slint`) stands in the rack's head slot when a track is
selected and carries the name, fader, pan, mute, destination, console switch
and the sends list. It is the face the strip's sections join.

**The two presentations are functionally interchangeable, and neither draws
what it is not showing.** Adam: *"in general we arent drawing things we arent
seeing, hopefully. but the two views should be functionally interchangeable."*
Two rules, and they pull in opposite directions on purpose:

1. A page that is not on screen is not built. In Slint that is an `if` rather
   than an `opacity: 0`, and it is what keeps a strip's back face free on all
   256 tracks.
2. No parameter is reachable from only one of them. A control that exists on
   the rack face and not on the back face -- or on the zoomed console and not
   in the paned mixer -- makes the mixer's state the thing a user has to
   manage before they can do the work.

## Built, 2026-09-11

Step 03 landed the strip: the two paned faces at 92px with the turn-over
between them, the pinned rack row, all four sections, and the voicing.
`00-status.md` records what the doing changed about this file. The two things
worth carrying up here:

**The pin is settled -- at the head**, which is the last entry that used to
be in the list below. `mooloop_core::mixer::STRIP_PIN` is the statement, and
both the block loop and the rack read it, so Adam's *"we should be able to
just move the pin"* is an edit to one constant.

**Pan is on the back face, as drawn.** This file's front sketch has mute /
solo / polarity in the column beside the fader and `meter vol pan` on the
back, so the built front face has no pan for the first time. It is on the
back and on the track's rack face, which is what the reachability rule asks
for.

The zoomed console is the one drawing of the three that is not built. It
needs nothing new -- `StripSections` is the full-format arrangement and takes
the room it is given -- and the two faces that are built already satisfy the
rule that no parameter is reachable from only one of them.

## Still open

- **Whether the sequencer rack draws a group.** Routing several channels to
  one track is all grouping needs to *work* (step 04's default project is made
  of it), but a "Drums" header with its channels nested under it is a real
  convenience once a kit is eight channels. Presentation, separable, and its
  own decision.
- **How each voicing's harmonic profile travels with level**, per the first
  limit above. That is authoring, and it wants ears.
- **A global turn-over** -- one control in the mixer's toolbar row that turns
  every strip at once, the way a desk's FLIP does. Floated 2026-09-10 and not
  ruled on; the per-strip button is settled and does not depend on it.

## Order this implies

Unchanged through step 04, and then the back half compresses: the strip face
is one build that carries the EQ, the compressor, the voicing and the preamp
together, because they are one face and `AGENTS.md`'s device-work order says
to cross into `main.slint` once. Sends and solo stay separate, because each is
a signal-path change rather than a face.
