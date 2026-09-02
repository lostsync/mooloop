# The face

## The idea

A drum sound is one event unfolding over a few hundred milliseconds. Every
control in DS-01 is either a description of a moment in that event or a
description of a layer's contribution to it. So the face has one spine: **a
single time axis that every layer shares**, with each layer's controls and its
contour on the same row, and the rendered hit in the row the amplitude
envelope shapes.

That is the device's own idea, and it is not transferable to another device
in this project. ML-P8's centre is a routing grid because ML-P8 is a network.
DS-01's centre is a timeline because DS-01 is a hit.

It also settles a complaint the taste brief makes directly — "an envelope
deserves an actual envelope display" — with something better than four
separate small envelope widgets: envelopes on one axis show their
*relationship*, which is the thing a drum patch is actually made of. A snare
is the noise envelope being shorter than the amp envelope. A clap is four
impulses inside one amp envelope. You should be able to see that.

Where it gets it wrong is by drawing them on top of each other. Sharing an
axis and sharing a box are different things, and only the first one is the
idea. The layout below is the second version, after the first was built.

## Layout

There are rendered concepts for this in `mockups/`, at the real face size and
against the real widgets. Read them before building: they already settled the
choice below, and one of them settled it by failing.

**Each layer owns a lane.** Its controls sit on the left, its contour on the
right, and every lane's contour is drawn on the same time axis. One screen, no
pages, no tabs over control groups.

```text
┌ DS-01  TUNE +0  CHOKE 1/5ms  MONO ─────┬─ 0 ──── 60 ──── 120 ──── 180 ── 240ms ┐
│                                        │      ▏ ▏  ▏ ▏  burst impulses          │
├────────────────────────────────────────┼───────────────────────────────────────┤
│ TONE  lvl pitch wave part sprd fm p.env│ ▟▄▁▁___                    ╲ pitch env │
├────────────────────────────────────────┼───────────────────────────────────────┤
│ NOISE lvl color rate morph cut res dec │ ▙▂▁_                                   │
├────────────────────────────────────────┼───────────────────────────────────────┤
│ BODY  lvl pitch ratio dec damp exc m.e │ ▜▆▅▄▃▃▂▂▁▁▁___                         │
├────────────────────────────────────────┼───────────────────────────────────────┤
│ AMP   atk hold dec curve sus rel gate  │ ▁▂▅█▇▅▃▂▁▁ rendered hit + amp envelope │
├────────────────────────────────────────┴───────────────────────────────────────┤
│ BURST rpt spc sprd lvl pch │ SHAPE drv char bias bits hp lvl │ MOD src→dst amt  │
└────────────────────────────────────────────────────────────────────────────────┘
```

This is a multitrack arrangement view of a single two-hundred-millisecond
event, which is the honest description of what a drum patch is. Three
properties fall out of it that the alternative did not have:

- **A value never has to be carried across the panel.** A layer's knobs and the
  shape they produce are on the same row, at the same height.
- **The relationship between envelopes is the picture.** A snare is the noise
  lane being shorter than the tone lane. A clap is four impulses inside one amp
  lane. Both are visible without reading a number.
- **No legend is needed.** Five signals, each in its own row, each already
  labelled by the row it is in.

The rules divide sections along the signal path, as on ML-P8's face. BURST,
SHAPE and MOD sit in a band under the lanes because they come after the voice:
burst is the hit's internal structure, shape is the nonlinearity, and MOD is
the per-hit matrix from step 07.

### The layout that was rejected

`mockups/concept-overlay.slint` is the earlier version of this document: three
parallel source columns above one display carrying every envelope overlaid.
Building it is what settled the question, and the faults are structural rather
than tuning:

- Four contours and a waveform in one ninety-pixel box is soup. The curves
  cross each other and the trace, the focused envelope's fill hides the rest,
  and it needs a legend — which is the admission that the picture failed.
- The three source columns come out as twenty-six near-identical small knobs in
  a grid, which is the "pages of knob rows" that was rejected on ML-P8's first
  face, reached from a different direction.

Do not reintroduce it because it reads more conventionally on paper.

## The display

- The waveform in the AMP lane is rendered through the **production voice
  path**, which is v1's best property and must be kept. What is drawn is what
  is heard, not an idealised curve. It sits in the amp lane rather than behind
  every lane because it is the device's output, and the output is what the amp
  envelope shapes.
- **The time axis auto-scales to the longest latched envelope in the patch**,
  with the scale printed. A fixed window — v1's 300 ms — draws a 5 ms hat as a
  single spike and clips a 4 s ride entirely, which makes the display useless
  at both ends of the range this instrument is supposed to reach.
- Each lane draws its own envelope, filled, in that lane's colour. The tone
  lane draws the pitch envelope as a second unfilled contour, because the tone
  layer genuinely has two — that is the only lane carrying more than one, and
  it is why the overlay layout looked reasonable before it was built.
- Burst impulses are ticks on the header's ruler, showing spacing and spread
  directly, because they are positions on the shared axis rather than a
  property of any one lane. At Repeats = 1 there is one tick, which is honest
  rather than empty.
- Envelope handles are draggable on the display, and dragging one is the same
  edit as turning its knob. Direct manipulation is the point; the knobs remain
  for precision and for automation targets.

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

- Every DS-01 parameter is reachable on one screen without scrolling at the
  standard window size.
- The display's time scale follows the patch, verified with a 5 ms hat and a
  4 s ride.
- Dragging an envelope handle and turning the corresponding knob produce the
  same parameter change, and both are automatable.
- A software-rendered snapshot test covers the face at the default patch and
  at one long-tail patch.
