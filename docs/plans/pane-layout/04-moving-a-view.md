# 04 — Moving a view between panes

**Landed 2026-09-08**, less the tab context menu and layout persistence, both
recorded in `00-status.md` as deliberate.

**Goal: requirement 4, and the rest of requirements 1 and 2** — the general
answer, of which "the mixer in the bottom pane" is one case.

## The gesture

Drag a tab. It is the editor-group gesture from every IDE, and the rack
already speaks this vocabulary for reordering devices: *"the face follows the
pointer, its origin stays as an outline, and the rows in between animate aside
to open the gap it will land in"* (`UI_DESIGN.md`). A tab drag is that,
smaller, and should look like it rather than like a second drag language.

Drop targets:

- **Another slot's tab strip** — the view moves there and becomes active.
- **A slot's body** — same thing. The whole pane is the target, not a 24px
  strip, because a 24px strip is a hard thing to hit and IDEs learned that.
- **The right edge of `main` while the split is closed** — opens the split
  with that view in it. This is how the split gets created without the chip,
  and it is the gesture people arrive already knowing.
- **Its own strip** — reorders.

A slot emptied by a move stops being drawn. `main` never empties: the last
view in it is not draggable out, and the drag shows that by refusing rather
than by snapping back.

## The other surfaces, because a gesture is not a surface

- **`View` menu.** The five rows become checkable "show this view" rows, plus
  a `Move View To >` submenu. The rows stop meaning "switch the bottom pane"
  and start meaning what their action ids have said all along.
- **Actions.** `view.pane-steps`..`view.pane-playlist` keep their `Ctrl+1..5`
  chords and gain their real meaning: reveal that view wherever it lives, and
  focus its slot. New ids for the split, the zoom and the moves, registered in
  `actions.rs` so Preferences > Shortcuts lists them like everything else.
- **Right-click a tab.** Move to the other panes, close the split, zoom. The
  menu is the discoverable version of every gesture on this page, which is
  the standard arrangement and the reason the gestures are allowed to be
  gestures.

## Watch for

- **The layout is not persisted, and that is a decision, not an oversight.**
  Once panes can be arranged, an arrangement is worth keeping across a
  restart — but it is UI state, not project state, and putting it in the
  project file would make a song carry a window layout. `PROJECT_FORMAT.md`
  is the wrong home. Settle where it goes before writing it anywhere; not at
  all is an acceptable answer for this step.
- **`Ctrl+1..5` needs a focus notion to be worth much.** Step 04 of
  `interface-iteration/` is the keyboard pass and it wants the same thing. If
  it has landed by the time this does, use it; if not, do not build half of it
  here.

## Done when

The mixer can be dragged into the bottom pane and back, a view dropped on the
right edge of a closed top pane opens the split, `View` names every one of
these operations, and no chord or menu row means something different depending
on which pane happens to be open.
