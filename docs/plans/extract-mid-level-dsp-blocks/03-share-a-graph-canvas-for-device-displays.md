# Give the device visualizers a shared canvas primitive

## Problem

The Slint side is well layered everywhere except one file.
`device-displays.slint` (365 lines) holds eight visualizers —
`SampleTrace`, `OscillatorTrace`, `BitcrusherTrace`, `FilterResponseDisplay`,
`DriveTransferDisplay`, `BitcrushTrace`, `DelayEchoDisplay`,
`DynamicsCurveDisplay` — and they sit directly on `Rectangle` with no shared
substrate beneath them. Every one independently establishes its own plot
area, background, grid, axis handling and curve rendering.

This is the same gap as the DSP one, one layer up: `Rectangle` is the
primitive, the visualizers are the devices, and the "graph canvas" rung in
between doesn't exist. Compare `controls.slint`, which does have that rung —
`TrimKnob inherits MiniKnob` (`controls.slint:817`), `MuteButton` and
`SoloButton` both `inherits ToolButton` (`:689`, `:898`).

## What to do

1. Read all eight visualizers and extract what's genuinely common: plot
   background and border, inset/padding conventions, optional grid lines,
   normalized-coordinate-to-pixel mapping, and the theme colors they each
   reach for. Several of them are curve-over-log-frequency
   (`FilterResponseDisplay`, and whatever the EQ device draws), several are
   transfer curves (`DriveTransferDisplay`, `DynamicsCurveDisplay`), and a
   couple are time-domain traces. Those three families may want three
   components over one shared base rather than one component with modes.
2. Build the base (`GraphCanvas`, or whatever fits the existing naming) and
   convert the visualizers onto it one at a time. Same bar as elsewhere:
   each converted file gets shorter.
3. While in here, check the log-frequency mapping question raised in
   `docs/plans/share-dsp-primitives/02-add-the-missing-primitives.md`:
   `filter-device.slint` computes `round(20 * pow(1000, root.cutoff))`
   inline for its readout, and the DSP side computes
   `20.0 * (max_hz / 20.0).powf(x)` in four places. If a display and its
   knob readout ever disagree, the UI lies about the value. At minimum, get
   the Slint side down to one shared expression (`meters.slint:12` shows the
   `global MeterScaleMath` pattern this repo already uses for exactly this
   kind of shared math).

## Also worth checking while here

`gallery.slint` (747), `mockup.slint` (482) and `device-concepts.slint`
(319) are 1548 lines of what look like design-exploration surfaces. Confirm
whether they build from the real `controls.slint` components or contain
their own copies of them. If they've drifted into being a parallel
implementation, that's a bigger duplication than anything in
`device-displays.slint` — but it's a separate plan, not scope for this one.

## Verification

- `cargo test -p mooloop-ui` — the snapshot tests
  (`mixer_snapshot.rs`, `rack_tools.rs`, `source_snapshot.rs`).
- Visual: open each converted device face and compare against a screenshot
  taken before the change. These are pixel-level components; a subtle inset
  or color change is a regression even though nothing fails.
- Note the build cost: a full Slint rebuild in this repo is ~4 minutes, so
  batch the visual checks rather than rebuilding per component.
