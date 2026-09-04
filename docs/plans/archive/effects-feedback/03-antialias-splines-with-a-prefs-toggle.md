# Antialias device curves/splines, behind an appearance preference

## Problem

From `docs/archive/EFFECTS_FEEDBACK.md`: "Can we antialias the splines/lines used
to draw filter shapes, etc? If yes, lets put that in and tie it to an
appearance option in prefs."

The device visualizers live in `device-displays.slint`
(`FilterResponseDisplay`, `DriveTransferDisplay`, `DynamicsCurveDisplay`,
`DelayEchoDisplay`, etc. — see also
`docs/plans/extract-mid-level-dsp-blocks/03-share-a-graph-canvas-for-device-displays.md`,
which covers giving these a shared canvas primitive; if that plan has
landed by the time this one starts, add antialiasing there once instead
of per-visualizer). Slint's built-in `Path`/`Polygon` elements render
without antialiasing by default on some backends; check what element type
these visualizers actually use for their curves before assuming a single
one-line fix applies everywhere.

## What to do

1. Identify exactly which Slint drawing primitive each affected visualizer
   uses for its curve (`Path`, a `Polygon`, or manually plotted `Rectangle`
   segments/dots). Antialiasing may be a per-element property, a renderer-
   level setting, or may require switching a plotted-dots approach to a
   real `Path` to have anything to antialias.
2. Add a new appearance preference (find the existing prefs/settings
   surface used for other appearance toggles — likely alongside the JACK
   buffer size picker in the Audio/Preferences dialog referenced in
   `docs/plans/archive/reduce-audio-jack-buffer-size/`) named something like
   "Smooth curves" or "Antialiased graphs."
3. Wire the preference through to each visualizer (or the shared canvas,
   if step 3 depends on the mid-level-dsp-blocks plan's canvas landing
   first) so it can be toggled without restart.
4. Confirm the toggle doesn't regress the "graph handles must lie on the
   rendered envelope or curve" rule in `docs/UI_DESIGN.md` — antialiasing
   the line must not shift where the plotted curve actually sits.

## Verification

Software-rendered snapshots of a visualizer with the pref on and off
(`SLINT_BACKEND=winit-software`), compared visually; confirm the setting
persists across restarts via whatever config-persistence path the other
prefs use.
