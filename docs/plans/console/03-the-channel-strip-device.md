# 03 — The channel strip

> **Reshaped by Adam's mockup, 2026-09-09. Read
> [`THE-STRIP.md`](THE-STRIP.md) first.** It contradicts this file's opening
> sentence and it is right: the channel strip is not a device you insert, it
> is what every strip *has*. It also moves the voicing selector to the top of
> the strip beside the preamp, so it governs the preamp, the EQ and the
> compressor together rather than the last two alone -- which pulls step 06
> into this build instead of leaving it parked.
>
> What survives unchanged is everything below about *reuse* and about the
> order of work. The shared units, the extraction of `EqResponseDisplay`, and
> the rule that an idle strip costs nothing are the same either way.

A composite of the shared DSP units, present on every channel and every bus,
with no mixer dependency at all. EQ + comp in one face, under one strip-wide
voicing: **Moo, Grip, Punch, Iron**. `THE-STRIP.md` says what each is going
for, and why they are named for a sound rather than for a company.

## Reuse rather than rebuild

This device is mostly assembly, and every part it needs already exists or is
already queued to be extracted.

- **The shared `Biquad`.** `adopt-shared-biquad-in-eq/` is a queued plan to
  stop `eq.rs` declaring its own. Do that here rather than adding a third
  copy.
- **The shared detector and gain computer** the gate, compressor and limiter
  already share -- as *primitives*. The strip's compressor is **not**
  `EffectKind::Compressor` behind a different face: Adam's ruling is that it
  is *"a totally different comp than the one built in as a device"*, voiced by
  the strip mode, with `w/d mix` a straight parallel-compression balance
  rather than the device-host blend the effect container already provides.
- **`DynamicsCurveDisplay`**, which already draws a gain-computer curve.
- **`EqResponseDisplay`**, which is currently **private to `eq-device.slint`**
  and would have to be extracted. That extraction is the one piece of face
  work this step has to do before it can start.

## Voicing is a mode, not four devices

A voicing selects curve shapes, time constants **and a harmonic target** --
knee, ratio law, attack and release curves, EQ band frequencies and Q, and the
input stage's distortion profile -- over one signal path. Four separate
devices would be four faces, four descriptor tables and four sets of presets
for one idea. `COMPOSABLE_DEVICE_UNITS.md` is the contract for how the shared
units are addressed.

**The distortion is not a garnish on the curves.** Adam's instruction when he
named the voicings was that it is *"not just curves"* he is after, and
`THE-STRIP.md` records how a harmonic target becomes a number rather than a
vibe: author each voicing's shaper through its Chebyshev decomposition, so the
2nd, 3rd and 4th harmonic amplitudes at the operating level are *specified*
and then asserted against `SpectrumAnalyzer` in a test.

Adam's taste brief applies here more than anywhere else in this plan: a device
face is not a page of knob rows, it is a drawing of what makes the device
different. The channel strip's difference is that the EQ and the compressor
are looking at the same signal, so the face should say so.

## Order of work

Follow `AGENTS.md`'s device-work order, which exists because the three parts
of a device differ by four orders of magnitude in cost to see:

1. DSP against unit tests (1-4 s per iteration, laptop);
2. face iterated with `scripts/slint-sketch` (0.05 s);
3. cross into `main.slint` **once**, with every property and callback batched
   into that single pass (8.7 min).

## The face collapses rather than shrinking

The strip is tall, and the answer is a show/hide toggle per section -- preamp,
EQ, compressor, sends -- rather than a second compact variant. The output
section is the elastic one: the fader may be squeezed from the top, down to a
floor. One face to build and one to maintain, and what a user hides is a
choice rather than a mode.

The EQ inside it reads **left to right, top to bottom**: high shelf, high mid,
low mid, low shelf. Say so in the markup, because two columns of three knobs
invite the assumption that the columns mean something.

## Every track has one, and it is off

Adam, 2026-09-09: *"comp and eq yes, every track, but defaults to turned off.
control for this is via the 'eq in'/'comp in' buttons."*

So the EQ and the compressor are present on all 256 tracks and neither runs
until its `in` button is pressed. That is a stronger and simpler rule than
"free when flat": there is no threshold to test against and no state to reason
about, because an EQ that is out is *out*.

Those two buttons are already drawn on the mockup, at the head of their
sections. They are the same switch `analog sum` is -- a per-track boolean that
the engine reads and skips on -- so they cost a defaulted field each and
nothing at all while they are off.

The consequence for [`06`](06-preamp-modelling.md) is worth stating: the
voicing selector still governs the strip, but a track with both sections out
and `Moo` selected is bit-identical to no strip. Any voicing's *character*
therefore has to arrive through a control that was deliberately turned on,
which is the same shape as `analog sum` and the same reason it is defensible.

## Free while it is out

`is_at_rest` / `skip_block` are what make an idle device cost nothing, and a
composite device has to implement them over its parts rather than inheriting
them. A channel strip with a flat EQ and a compressor below threshold should
be as cheap as an empty slot.

## Verification

`slint_face_agreement.rs` holds a face to its descriptor table. Extend it for
this device rather than adding a parallel check -- that test exists precisely
so a new device cannot ship a face that disagrees with what the engine reads.
