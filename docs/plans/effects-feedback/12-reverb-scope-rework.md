# Reverb: replace the room-plan display, add low-cut, add a darken option, defer IR loading

## Status: mostly resolved, August 2026

The device this plan describes no longer exists. The generated-room
convolution reverb was replaced by an eight-line feedback delay network (see
`docs/REVERB.md`), which settled most of this file by removing its subject
rather than by reworking it. What that change did and did not cover:

- **Replace the room plan and the redundant text panel** — done. The face
  carries a single full-width `HallResponse` display and seven other
  controls; there is no plan view, no fixed source marker (the "green
  square"), and no panel restating what the knobs already say. The display is
  derived from the parameters, per `docs/UI_DESIGN.md`.
- **Decide whether the capture-position control survives** — decided: it does
  not. Adam took the conventional-hall face over keeping the room geometry.
  The device has no spatial control now; `Width` and the network's two
  orthogonal output taps carry the stereo image instead.
- **Low cut on the input** — done, as a `Low Cut` knob (20..500 Hz) filtering
  the input ahead of the diffusers. `docs/REVERB.md` records why it cannot
  live in the feedback loop.
- **IR loading dropped from this device** — done, and now stronger than
  deferred: there is no IR player in the tree at all. `docs/REVERB.md`'s
  "Measured IRs" section records that a loader should arrive as its own
  device rather than as a mode of this one.
- **Darken/dim option for the display** — **still open.** This was always an
  appearance-preference item rather than a reverb item, and it should reuse
  whatever toggle mechanism
  `03-antialias-splines-with-a-prefs-toggle.md` settles on rather than
  growing a second one-off. It applies to every device display, not just this
  one, so it does not belong in a reverb-specific plan; move it there or to a
  general appearance plan.

The one remaining item is the only reason this file is not archived.

## Original problem

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
