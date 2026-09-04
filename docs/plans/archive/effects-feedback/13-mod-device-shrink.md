# Mod: shrink the footprint and drop redundant labels

**Status: done, 2026-09-02** (`87d18f8`, `5013ad9`). The face is 2U,
`modulation-device.slint` is 134 lines, the redundant text panel is gone, and
the knobs come off the shared instrument-panel composition rather than being
seven full-size `ParameterKnob`s. `modulation_uses_two_rack_units` in
`mooloop-ui/src/lib.rs` pins the width. The line references below are to the
file as it was before that work.

## Problem

From `docs/archive/EFFECTS_FEEDBACK.md`: "Mod: visually similar to Reverb in many
ways and suffering many of the same layout issues. It is very large
compared to what it does. The scope does not need to be that big. doesnt
need to say 'stereo variable delay' or have the rate and depth shown
independently of the knobs. i think using something other than full size
knobs would be good here. we have a lot of flexibility in laying out
these devices. shared surfaces are cool but not when they fight
usability."

Read directly from `modulation-device.slint`:

- `ModulationDisplay` is 330×108px (`modulation-device.slint:80`), the
  same scale as Reverb's room-plan-plus-panel row.
- The adjacent `VerticalLayout` (`modulation-device.slint:81-87`) prints
  exactly the literal text the feedback names: `"STEREO VARIABLE DELAY"`
  (or the mode-specific variants at line 83) and
  `round(root.rate-hz * 100) / 100 + " Hz / " + round(root.depth * 100) +
  " DEPTH"` at line 85 — both fully redundant with the Rate and Depth
  knobs directly below (`modulation-device.slint:91-92`).
- The knob row uses 7 full-size `ParameterKnob`s at 72px width/40px
  diameter each (`modulation-device.slint:91-97`) — the same knob class
  used everywhere else, not a size chosen for this device's actual control
  density.

## What to do

1. Remove the redundant text panel beside `ModulationDisplay`
   (`modulation-device.slint:81-87`) entirely — nothing in it isn't
   already on a knob.
2. Shrink `ModulationDisplay` to a size proportionate to what it conveys
   (an LFO shape trace), freeing width/height for the controls instead of
   matching Reverb's dominant-editor scale (`docs/UI_DESIGN.md`'s "160 px
   for a graph plus controls" row height may be closer to right than the
   current 108-116px dominant-editor treatment).
3. Replace the 7 full-size knobs with a more compact control set — per
   `docs/UI_DESIGN.md`'s "related continuous values that must align →
   short faders" guidance, evaluate whether some of Rate/Depth/character/
   Feedback/Spread/Tone/Stages read better as a fader bank or a mix of
   `MiniKnob` (`controls.slint:728`) and full knobs, rather than assuming
   knobs are the only option.
4. Reduce the device's overall rack footprint (currently `kind: "FX /
   3U"`, `modulation-device.slint:2` implicit via `EffectDeviceShell`) if
   the shrink leaves genuine unused width — per `docs/UI_DESIGN.md`, "an
   effect uses only the units its working controls require."

## Verification

Software-rendered snapshot of the reworked Mod face next to its current
form, confirming no bordered empty space remains (`docs/UI_DESIGN.md`'s
rectangle test) and that every remaining value is shown exactly once.
