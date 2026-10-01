# 06 — editing points

Adam: *"no edit controls, no point selection or multi-selection, no context
menu."* Step 03 left the panel with the roll lane's gestures. Today's lane
has these limits:
- Click adds a point, drag moves the point under the pointer, and right-click
  deletes it at once.
- Selection is implicit: `automation_selected_point` is "the last one
  touched" (`session/src/session.rs:87`).
- There's no keyboard support.

This step makes a lane editable the way a note grid is.

## Gestures

| Gesture | Does |
| --- | --- |
| Click empty lane | Adds a point, snapped (Shift: free), and selects only it |
| Click a point | Selects only it |
| Shift-click a point | Adds it to the selection, or takes it out |
| Drag on empty lane with a modifier (match the piano roll's marquee) | Marquee: selects the points inside |
| Drag a selected point | Moves the whole selection. Ticks snap by the grabbed point's offset; values keep their offsets, clamped together so the shape survives |
| Delete / Backspace | Removes the selection |
| Ctrl+A with the lane focused | Selects every point in the lane |
| Right-click | Opens the context menu, and selects the point under the pointer if it wasn't selected |

**The context menu:**
- Delete;
- Select all;
- **Set value…**, which types a value into every selected point in the
  parameter's own units, through the typed-entry parser knobs use;
- Clear lane;
- Remove lane;
- and, once step 08 lands, the segment shape.

Rows are wired callback first, close after.

**Selection is per lane, in the session**, like the note selection. It is
not a document edit and records no undo. Moving or deleting a selection is
**one** undo entry, bracketed as a gesture (`Gesture.begin`/`end`), not one
entry per point.

**Shortcuts** go through the action registry (`docs/ACTIONS.md`), with the
lane panel as their scope. That way they don't steal Delete from the pattern
rows above them or from the piano roll. MOO-287's focused-pane outline
shows which pane has them.

## Engine

None. Every gesture is the step 01 verbs applied to a set: one
`UpsertSongPoint`/`RemoveSongPoint` per point. Add a batched command only if
a large selection measurably floods the ring, and measure it before adding
one.

## Done when

- **The gesture table:** each row has a test that drives the lane's
  callbacks and asserts the session's points and selection (the
  `tests/piano_drag.rs` style).
- **Undo:**
  - Moving 50 selected points is one undo entry, and undo restores all 50.
  - Deleting a selection is one entry.
- **Keyboard:** Delete in the panel doesn't delete notes in the pattern rows
  or the roll, and the reverse holds.
- **The context menu's rows each do what they say:**
  - Set value… takes `-6 dB` on a fader lane;
  - Clear lane and Remove lane are undoable.
- **`docs/CURRENT.md`** describes lane editing.
