# 05 — faces, bends and cable activity

A box's face opens in place on the canvas, a cable's bend can be dragged,
and the cables show what is moving through them, as much as the user wants.
These are the three things Adam asked of the prototype's look.

## Faces in place

Adam (2026-09-27): *"an lfo block might have, you know, shape, speed, any
kind of options for smoothing or warping you wanna have, sync toggle, and a
retrigger mode selector. so we'd want at least small knobs and readouts for
all of that. it doesnt have to be full sized like device face style."*

- Each box has a fold arrow beside its spelling; it, or a double-click on
  the box, toggles `open` (step 01's field). An open box draws its face
  under its spelling: the module surfaces the shelf draws today
  (`ModulatorShape`, `StepBank`, the per-kind parameter rows,
  `modulation-shelf.slint:192-609`), at canvas scale, built from `MiniKnob`
  and the readouts, not Bitwig's look.
- Boxes with one or two parameters (`* x`, `counter n`, `slew`) show one
  knob each. A box with none says so.
- Knobs on an open face are assignment destinations like any other knob, so
  a wire is not the only way to move a box's parameter. **This is the first
  use of `ParamOwner::Modulator`** (`core/src/modulation.rs:74`), which the
  engine does not resolve yet (`render.rs:7994`): resolve it here, keyed by
  `ModSourceId`, so an outlet can be assigned to another box's knob.
- The selected box's surface below the canvas goes; the face is the only
  editor. The surface's input picker went with step 03's inlet tags.

## Cable bends

Adam: *"i'd want a way to manually position the angles in the cables."*
As the prototype does: drag a cable's middle segment to move its bend (the
segment moves along the axis it crosses), double-click the cable to go
back to automatic routing. The bend is step 01's `Wire.bend`, saved with the
song. One undo step per drag.

## Cable activity

Adam: *"this blinking is a bit loud. we'd have to tune it, and maybe
optionally make it tunable."*

A preference, **Cable activity**: Off, Subtle (default) or Full. Control
wires tint toward the accent colour with their level; note wires thicken
briefly on each note. Subtle is the prototype's 30 %; Off draws plain wires.
**It starts on Off when the system asks for reduced motion**, as the
prototype does, where Slint can read that setting; where it cannot, Subtle.

Levels come from the meters the pane already reads
(`refresh_modulation_offsets`, `ui/src/lib.rs:5927`), and the pump updates
only the wires whose level moved, as it does for module meters now.

## Done when

- Every box kind's face opens in place and edits its parameters, with one
  undo step per drag.
- An outlet can be assigned to a box's knob, and moves it (an engine test
  and a UI test).
- Bends drag, save and reopen; a double-click clears one.
- Cable activity has its three settings in the preferences, persisted.
- `LISTENING.md` has an item to look at faces, bends and each activity
  setting while a song plays.
