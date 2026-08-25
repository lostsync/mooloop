# Reverb: replace the room-plan display, add low-cut, add a darken option, defer IR loading

## Problem

From `docs/EFFECTS_FEEDBACK.md`: "I dont actually know what that green
square in the scope is for. I can't move it. Don't know what it
means/does. The scope is not as wide as the 4 buttons below it. ... it
might be more trouble than it's worth, but i do think we could find
something more visually appealing and meaningful to show the space and
simulated capture position. we have a lot of blank space on the right
side of the plugin. all it's really showing is info we can already see on
the left. maybe it should show the IR, kinda like the drum synth does...
need to be able to darken it. needs a low cut on the input. it's supposed
to also allow the actual loading of IRs but i dont see that anywhere. im
ok with dropping that and just making an IR loader later."

Read directly from `reverb-device.slint`:

- The "green square" is the fixed source marker in `RoomPlan`
  (`reverb-device.slint:31`): `Rectangle { x: parent.width * 0.32 - 4px;
  ...; background: Theme.accent; }`, explicitly commented "Fixed
  asymmetric source marker; only the capture point is edited." It is
  *supposed* to be immovable — the actual draggable element is the
  separate round dot at lines 32-46 (the capture/listener point). This is
  a legend/discoverability gap (nothing on screen explains the square is
  the source and the dot is the mic), not a broken control — but the
  feedback's deeper point (find something more meaningful than this
  layout) stands regardless.
- `RoomPlan` is 272px wide (`reverb-device.slint:100`); the Width/Depth/
  Height/Decay knob row below it is 4×86px = 344px
  (`reverb-device.slint:116-119`) — confirms "the scope is not as wide as
  the 4 buttons below it."
- The `VerticalLayout` beside `RoomPlan` (`reverb-device.slint:105-112`)
  is exactly the "blank space on the right... showing info we can already
  see on the left" complaint: it renders width/depth/height as text
  (already shown on the Width/Depth/Height knobs), the capture coordinates
  (already implicit in the draggable dot's position), and decay time
  (already on the Decay knob) — no new information, padded with two
  `vertical-stretch: 1` spacers.
- There is no low-cut control and no darken toggle anywhere in this file,
  and no IR-file-loading UI at all despite the feedback's belief it's
  "supposed to" exist — confirming it was never built, not hidden.

## What to do

1. Replace the `RoomPlan` + redundant text panel with a single display
   that earns the full width (272 + panel's width, roughly matching the
   344px knob row) and shows something not already on a knob — the
   feedback's own suggestion is closest to Drum Synth's voice-shape
   preview (`docs/UI_DESIGN.md`'s "Drum synth" section: "a deterministic
   preview rendered through the production drum voice and reduced to
   waveform min/max bins"): render the actual generated IR's envelope/
   decay shape (or a simple spectral-tilt-over-time view) computed from
   the same `width-m`/`depth-m`/`height-m`/`decay-s`/shape/material
   parameters that drive the real DSP, per `docs/UI_DESIGN.md`'s "device
   plots derive from the parameters and transfer functions used by the
   audio path."
2. Decide whether the room-plan capture-position control (the one genuine
   piece of interactive state here) is worth keeping in a smaller form
   alongside the new display, or whether it's superseded — this needs a
   product call, not just a layout fix, since dropping it removes the
   only spatial control the device has. Flag this as a decision point
   before implementing, not an assumption to make silently.
3. Add a low-cut (high-pass) control on the reverb's input — check
   whether `ReverbEffect` (`crates/mooloop-dsp/src/effects/reverb.rs`) has
   any pre-filtering hook already, or needs one added ahead of the
   convolution input.
4. Add a darken/dim option for the display, consistent with whatever
   toggle mechanism `03-antialias-splines-with-a-prefs-toggle.md` uses for
   its appearance preference (reuse that pattern rather than building a
   second one-off appearance toggle mechanism).
5. Explicitly drop IR-file loading from this device's scope per Adam's
   direction — note in the device or in `docs/ROADMAP.md`/`docs/FOCUS.md`
   (whichever already tracks deferred work) that a dedicated IR loader is
   the intended future path, so it isn't re-flagged as a bug later.

## Verification

Software-rendered snapshot of the reworked Reverb face at the same
268px×3U footprint, confirming the new display fills its allotted width
(no residual blank column) and that its shape visibly changes with decay/
shape/material parameter changes; `cargo test -p mooloop-dsp` if the
low-cut addition changes `ReverbEffect`'s signal path.
