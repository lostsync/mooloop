# The face

## The idea

A drum sound is one event unfolding over a few hundred milliseconds. Every
control in DS-01 is either a description of a moment in that event or a
description of a layer's contribution to it. So the face has one spine: **a
single time axis that every layer and every envelope shares**, with the
actual rendered hit drawn behind it.

That is the device's own idea, and it is not transferable to another device
in this project. ML-P8's centre is a routing grid because ML-P8 is a network.
DS-01's centre is a timeline because DS-01 is a hit.

It also settles a complaint the taste brief makes directly — "an envelope
deserves an actual envelope display" — with something better than four
separate small envelope widgets: four envelopes on one axis show their
*relationship*, which is the thing a drum patch is actually made of. A snare
is the noise envelope being shorter than the amp envelope. A clap is four
impulses inside one amp envelope. You should be able to see that.

## Layout

One screen. No pages, no tabs over control groups.

```text
┌─ DS-01 ────────────────────────────────────────────────────────────────┐
│ TONE            │ NOISE             │ BODY            │ GLOBAL         │
│ level  pitch    │ level  color      │ level  pitch    │ tune           │
│ wave   partials │ rate   morph      │ ratio  decay    │ choke grp/time │
│ spread fm/ratio │ cutoff res        │ damp   excite   │ retrig  vel    │
├────────────────────────────────────────────────────────────────────────┤
│                                                                        │
│   THE HIT                                                              │
│   ┌──────────────────────────────────────────────────────────────┐     │
│   │      ▁▂▅█▇▅▃▂▁▁ rendered waveform, production voice path      │     │
│   │     ╱‾‾╲___                     amp envelope (focused)        │     │
│   │    ╱     ╲______                                              │     │
│   │   ╱ ┈┈┈┈╌╌╌╌___  noise, pitch, mod envelopes (faint)          │     │
│   └──────────────────────────────────────────────────────────────┘     │
│    ▏      ▏   ▏     ▏      burst impulses            0 ── 240 ms        │
│                                                                        │
├────────────────────────────────────────────────────────────────────────┤
│ BURST                    │ SHAPE                    │ MOD              │
│ repeats spacing spread   │ drive character bias     │ 8 rows:          │
│ level-step pitch-step    │ bits  output-hp  level   │ src > dst  amt   │
└────────────────────────────────────────────────────────────────────────┘
```

Rules divide the sections along the signal path, as on ML-P8's face. The three
source columns are parallel because the three layers are parallel; BURST sits
under the display because it is a property of the time axis; SHAPE and MOD sit
after everything because they are.

Density is comparable to ML-P8, which already puts fifty-four controls on one
screen.

## The display

- The waveform behind the envelopes is rendered through the **production voice
  path**, which is v1's best property and must be kept. What is drawn is what
  is heard, not an idealised curve.
- **The time axis auto-scales to the longest latched envelope in the patch**,
  with the scale printed. A fixed window — v1's 300 ms — draws a 5 ms hat as a
  single spike and clips a 4 s ride entirely, which makes the display useless
  at both ends of the range this instrument is supposed to reach.
- All four envelopes are drawn. The one belonging to the focused section is
  drawn solid; the rest are faint. Hovering or touching a control in a section
  focuses that section's envelope.
- Burst impulses are ticks on the axis below the display, showing spacing and
  spread directly. At Repeats = 1 there is one tick, which is honest rather
  than empty.
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

- Build the layout in `scripts/slint-sketch` first. `cargo build -p
  mooloop-ui` is about four minutes for any edit; the sketch type-checks in
  about 0.05 s and screenshots in about 0.2 s. See `docs/AGENT_OPERATIONS.md`.
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
