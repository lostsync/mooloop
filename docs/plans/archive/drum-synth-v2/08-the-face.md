# The face

## The idea

A drum sound is one event unfolding over a few hundred milliseconds. Every
control in DS-01 is either a description of a moment in that event or a
description of a layer's contribution to it. So the face is organised by
layer, and every layer's contribution is drawn as a contour on a scope over a
span the whole face shares.

That is the device's own idea, and it is not transferable to another device
in this project. ML-P8's centre is a routing grid because ML-P8 is a network.
DS-01's centre is a timeline because DS-01 is a hit.

It also settles a complaint the taste brief makes directly — "an envelope
deserves an actual envelope display" — and it settles it with a *display*.
See the ruling below: this step first tried to settle it by making the
display the editor, and that was wrong for the same reason ML-P8's first face
was wrong.

## The ruling — Adam, 2026-09-04

**The face spends pages, and the scopes are not the editor.**

The version of this step that was built first fitted ninety-two controls on
one screen by merging the two: envelope times were handles dragged on a
curve rather than knobs, and what was left over shrank to a 21px dial with an
8px caption at five rack units. It is the same trade ML-P8's first face made
and Adam rejected — "might be ok on a 24 inch screen but not 14" — arrived at
from the other direction.

So:

- **Every parameter has a control of its own**, at the 34px `KnobStack` the
  rest of the program uses, with its value typed into. No parameter is
  reachable only by dragging a picture.
- **The scopes are displays.** They carry no handles. A scope says what the
  patch is doing; a knob is how it is changed.
- **The face is six pages**, and a page is a few large controls inside modules
  that share one chrome — never a grid of small ones.
  `docs/plans/poly-synth-v2/mockups/README.md` is the argument, and the trap
  it names ("pages of knob rows") is the same one this plan's own mockups
  rejected once.
- **Four rack units, not five.** The fifth existed to make one screen fit.

The three checked-in concepts in `mockups/` are still the argument for how the
device divides — layers, their scopes, and the bands after the voice — and two
of the three still fail for the reasons recorded there. What they settled is
the grouping. What they got wrong is that the grouping had to fit at once.

## Layout

Six pages. Every one of DS-01's ninety-two parameters is on exactly one of
them, and the header of the face states the scopes' shared span once.

| Page | What is on it | Controls |
| --- | --- | --- |
| **VOICE** | The hit as a whole: `VOICE` (Tune, Level, Vel Amt, Choke group, Choke time, Retrigger), `BURST` with its impulse axis, `SHAPE` | 16 |
| **TONE** | `TONE` (7), and `PITCH ENV` (4) with the pitch contour over a quiet amplitude one for scale | 11 |
| **NOISE** | `NOISE` (6: layer and filter), and `NOISE ENV` (7) with its contour | 13 |
| **BODY** | `BODY` (6), and `RING`, the resonator's decay drawn over the same span | 6 |
| **AMP** | `AMP ENV` (7) with the rendered hit inside its contour, and `MOD ENV` (7) with its own | 14 |
| **DS-01 MOD** | The eight matrix rows: source, destination, amount, curve | 32 |

The division follows the signal path, as on ML-P8's face: the three source
layers get a page each, the sum's envelope and the contour that has no layer
share the fourth, and what belongs to the hit rather than to a layer — the
burst before it and the shaper after it — sits with the globals on the first.

Two placements are worth stating because they are not obvious:

- **The mod envelope is on the AMP page, not the MOD page.** It is an
  envelope, it is drawn the same way as the other three, and the matrix page
  needs its full height for eight rows. Its module caption says what it is
  for.
- **The globals share a page with the burst and the shaper**, because none of
  the three is a layer and all three describe the hit as a whole. A permanent
  header strip was the alternative and it costs every page the same vertical
  space that pages were adopted to buy back.

### Span

Every scope is drawn over the same span, stated once in the page bar rather
than once per page, with matching gridlines at the same fractions. A shorter
contour is then a shorter contour rather than a different scale.

The span auto-scales to the longest latched envelope in the patch. A fixed
window — v1's 300 ms — draws a 5 ms hat as a single spike and clips a 4 s ride
entirely, which makes the display useless at both ends of the range this
instrument is supposed to reach.

### The layouts that were rejected

`mockups/concept-lanes.slint` gave each layer a row, controls left and contour
right, on one continuous axis. Two faults: stacking parallel layers as rows
implies an order they do not have, and it fitted by showing seven controls per
lane and leaving the rest out.

`mockups/concept-overlay.slint` is the version this document originally
described: three source columns above one display with every envelope drawn on
top of the others. Four contours and a waveform in one ninety-pixel box is
soup — the curves cross each other and the trace, the focused envelope's fill
hides the rest, and it needs a legend, which is the admission that the picture
failed.

`mockups/concept-columns.slint` is the one that was built. Its grouping is
what the pages inherit. Its fit is what the ruling above overturns.

Do not reintroduce any of the three because it reads more conventionally on
paper.

## The display

- The waveform on the AMP page is rendered through the **production voice
  path**, which is v1's best property and must be kept. What is drawn is what
  is heard, not an idealised curve. It sits inside the amplitude envelope's
  contour because the envelope is what shaped it.
- Each scope draws its own layer's contour, filled, in that page's colour. The
  pitch page carries a second, unfilled contour — the amplitude envelope,
  quiet — because a pitch sweep is read against how long the hit lasts. No
  scope carries more than two.
- Burst impulses are ticks on a short axis in the `BURST` module's title row,
  showing spacing and spread directly, beside the controls that set them. At
  Repeats = 1 there is one tick, which is honest rather than empty.
- **There is no column dimming.** It existed to say which of four columns you
  were reading on a face that showed all four at once. One layer per page says
  it without a mechanism.

## What the face must not become

- No drum-type selector, no kit browser dressed as a mode, no preset dropdown
  that changes which controls are visible. The instrument has one architecture
  and the pages show all of it.
- No pages of knob rows. A page is modules of large controls; if a page needs
  more than about sixteen, it is two pages.
- No decoration that is not doing a job — no glow, no gradient, no screws.
- Tooltips carry the value only; explanatory text goes to the status bar, per
  the project convention.

## Practical notes

- Build the layout in `scripts/slint-sketch` first. `cargo build -p
  mooloop-ui` is about four minutes for any edit; the sketch type-checks in
  about 0.05 s and screenshots in about 0.2 s. See `docs/AGENT_OPERATIONS.md`.
- The face is indexed by descriptor id — arrays in, `(id, normalized)` out —
  so a page is a list of parameter ids rather than ninety-two properties.
  **A control's value is therefore a binding onto a model row, and Slint drops
  a binding at the first assignment to the property it feeds**, which is what
  a knob does to itself while it is dragged. Every control on this face sets
  `ParameterKnob.controlled`, which makes it report its change without writing
  it: the owner writes the model, the binding survives, and the value lives in
  exactly one place. Without that a knob shows the last value it was dragged
  to for the rest of the session, and the next patch loaded over it leaves
  that one control behind.
- Two drawing details the mockup already paid for. A filled contour must be
  drawn in an **opaque** colour with its steps overlapping by half a pixel:
  translucent fill double-blends at every seam and reads as stripes, and
  exactly-abutting steps leave sub-pixel gaps and read as hatching.
- The preview render must not run on the UI thread per keystroke. Debounce it
  and keep v1's bin-reduction shape.

## Acceptance

- Every DS-01 parameter is reachable at the standard window size on a page
  with a control of its own, with no scroll. Count them against the table
  above and fail the step if the count does not close.
- Every parameter that a knob can reach is also a modulation and automation
  destination on the same terms, and typed entry lands under the same clamping
  as a drag.
- The scopes' shared span follows the patch, verified with a 5 ms hat and a
  4 s ride.
- A software-rendered snapshot test covers every page, and a test proves a
  control still follows the patch after it has been dragged.
