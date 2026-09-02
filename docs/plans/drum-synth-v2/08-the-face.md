# The face

## The idea

A drum sound is one event unfolding over a few hundred milliseconds. Every
control in DS-01 is either a description of a moment in that event or a
description of a layer's contribution to it. So every layer gets **its own
scope, directly under the controls that make it**, and every scope is drawn
over the same span — with the rendered output under the envelope that shapes
it.

That is the device's own idea, and it is not transferable to another device
in this project. ML-P8's centre is a routing grid because ML-P8 is a network.
DS-01's centre is a timeline because DS-01 is a hit.

It also settles a complaint the taste brief makes directly — "an envelope
deserves an actual envelope display" — with something better than four
separate small envelope widgets: envelopes on one axis show their
*relationship*, which is the thing a drum patch is actually made of. A snare
is the noise envelope being shorter than the amp envelope. A clap is four
impulses inside one amp envelope. You should be able to see that.

Where the first two attempts got it wrong was by treating the display as one
box to be shared. Sharing a span and sharing a box are different things, and
only the first one is the idea. Give each layer its own scope and two things
follow: the panel stops needing a legend, and the scope can become the place
that envelope is *edited* rather than the place it is shown.

This is the third version. The other two are in `mockups/` with their
reasons.

## Layout

There are rendered concepts for this in `mockups/`, at the real face size and
against the real widgets. Read them before building: they settled the choice
below, and two of the three settled it by failing.

**Each layer's scope sits directly underneath the controls that make it.**
TONE, NOISE and BODY are parallel columns because they are parallel in the
signal path. AMP is the fourth column because the summed output is what its
envelope shapes, and the rendered hit is what its scope draws. One screen, no
pages, no tabs over control groups.

```text
┌ DS-01  TUNE +0  CHOKE 1/5ms  MONO  VEL 100%      every scope  0 – 240 ms ┐
├──────────────┬──────────────┬──────────────┬─────────────────────────────┤
│ TONE         │ NOISE        │ BODY         │ AMP · MOD ENV               │
│ lvl pit wav  │ lvl col rate │ lvl pit ratio│ curve sus rel gate          │
│ sprd fm ratio│ cut res gate │ dec damp exc │ m.curve m.sus m.rel m.gate  │
│ ┌──────────┐ │ ┌──────────┐ │ ┌──────────┐ │ ┌─────────────────────────┐ │
│ │╲___      │ │ │╲__       │ │ │╲‾‾╲___   │ │ │ ▁▂▅█▇▅▃▂▁▁ rendered hit │ │
│ │ tone+pitch│ │ │ noise env│ │ │ body ring│ │ │ amp env + mod env       │ │
│ └──────────┘ │ └──────────┘ │ └──────────┘ │ └─────────────────────────┘ │
├──────────────┴──────┬───────┴──────────────┴──┬──────────────────────────┤
│ BURST ▏ ▏  ▏  ▏     │ SHAPE drv char bias bits │ MOD  src → dst   amount  │
│ rpt spc sprd lvl pch │       hp level           │      × 8                 │
└──────────────────────┴──────────────────────────┴──────────────────────────┘
```

The rules divide sections along the signal path, as on ML-P8's face: three
sources side by side, their sum to the right of them, then the shaper and the
per-hit matrix in a band underneath because both come after the voice.

### The scopes are the envelope editor

This is the property the layout exists for, not a refinement of it. A display
sitting directly under its own controls can carry that envelope's handles, so
**envelope times are dragged on the curve rather than dialled on a knob**.
Attack, hold, decay, and the pitch envelope's depth are handles. Curve,
sustain, release, and gate stay as controls, because dragging a point cannot
express them.

That is what makes DS-01 fit in one face at all:

| | cells |
| --- | --- |
| Four columns, two rows of four | 32 |
| Header globals | 5 |
| BURST and SHAPE bands | 11 |
| Envelope times, on the scopes | 13 |
| **Total** | **61** |

The device has roughly 55 controls. Every layout that treated the display as a
readout rather than an editor was showing about two thirds of them and would
have needed a page or a scroll to finish — which is the failure this plan
refuses in `01`. Build the handles in the same step as the scopes; a scope
without them is not a smaller version of this design, it is a different one
that does not fit.

A handle and its knob are one parameter and one edit. Where both exist — the
body's decay is drawn as a handle and dialled as a knob — they move together
and produce one automation write.

### Span

The three source scopes and the output scope are drawn over the same span,
stated once in the header rather than four times, with matching gridlines at
the same fractions. A shorter contour is then a shorter contour rather than a
different scale, which is what lets the noise column be visibly briefer than
the tone column without a shared continuous axis.

The span auto-scales to the longest latched envelope in the patch. A fixed
window — v1's 300 ms — draws a 5 ms hat as a single spike and clips a 4 s ride
entirely, which makes the display useless at both ends of the range this
instrument is supposed to reach.

### The layouts that were rejected

`mockups/concept-lanes.slint` gave each layer a row, controls left and contour
right, on one continuous axis. It was the adopted layout until Adam proposed
the columns. Two faults: stacking parallel layers as rows implies an order
they do not have, and with the scope beside its controls rather than under
them the layout never became an editor, so it fitted by showing seven controls
per lane and leaving the rest out.

`mockups/concept-overlay.slint` is the version this document originally
described: three source columns above one display with every envelope drawn on
top of the others. Four contours and a waveform in one ninety-pixel box is
soup — the curves cross each other and the trace, the focused envelope's fill
hides the rest, and it needs a legend, which is the admission that the picture
failed. Its source columns also come out as twenty-six near-identical small
knobs in a grid, which is the "pages of knob rows" that was rejected on
ML-P8's first face.

Do not reintroduce either because it reads more conventionally on paper.

## The display

- The waveform in the AMP scope is rendered through the **production voice
  path**, which is v1's best property and must be kept. What is drawn is what
  is heard, not an idealised curve. It sits in the amp column rather than
  behind every column because it is the device's output, and the output is
  what the amp envelope shapes.
- Each scope draws its own layer's envelope, filled, in that column's colour.
  Two columns carry a second unfilled contour because two layers genuinely
  have one: the tone column's pitch envelope, and the amp column's mod
  envelope, which has no layer of its own and so is drawn where it is edited.
  No scope carries more than two.
- Burst impulses are ticks on a short axis in the BURST section itself,
  showing spacing and spread directly, beside the controls that set them. At
  Repeats = 1 there is one tick, which is honest rather than empty.
- The focused column's scope is drawn solid; the others stay legible but
  quiet. Touching any control in a column focuses it.

## What the face must not become

- No drum-type selector, no kit browser dressed as a mode, no preset dropdown
  that changes which controls are visible. The instrument has one architecture
  and the face shows all of it.
- No pages of knob rows. If the screen will not hold the device, the device is
  too big, not the screen too small.
- No decoration that is not doing a job — no glow, no gradient, no screws.
- Tooltips carry the value only; explanatory text goes to the status bar, per
  the project convention.

## Practical notes

- Build the layout in `scripts/slint-sketch` first, starting from
  `mockups/concept-lanes.slint` rather than from scratch. `cargo build -p
  mooloop-ui` is about four minutes for any edit; the sketch type-checks in
  about 0.05 s and screenshots in about 0.2 s. See `docs/AGENT_OPERATIONS.md`.
- Two drawing details the mockup already paid for. A filled contour must be
  drawn in an **opaque** colour with its steps overlapping by half a pixel:
  translucent fill double-blends at every seam and reads as stripes, and
  exactly-abutting steps leave sub-pixel gaps and read as hatching. And
  `MiniKnob` draws only the dial, so a dense knob row places its own captions.
- The preview render must not run on the UI thread per keystroke. Debounce it
  and keep v1's bin-reduction shape.
- Reuse `SampleTrace` for the waveform layer rather than inventing a second
  waveform widget.

## Acceptance

- Every DS-01 parameter is reachable on one screen at the standard window
  size, with no page, tab, or scroll. Count them against the table above and
  fail the step if the count does not close.
- Every envelope time is draggable on its own scope, and every one of those
  handles is a modulation and automation destination on the same terms as a
  knob. Dragging a handle and turning the equivalent knob, where both exist,
  produce the same parameter change and one automation write.
- The scopes' shared span follows the patch, verified with a 5 ms hat and a
  4 s ride, and all four scopes agree on it.
- A software-rendered snapshot test covers the face at the default patch and
  at one long-tail patch.
- The face renders in `scripts/slint-sketch` before it is built, and
  `mockups/concept-columns.slint` is updated if the built layout departs from
  it, so the checked-in concept never contradicts the shipped face.
