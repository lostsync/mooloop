# 07 — lane heights

Adam: *"we should be able to vertically resize the lanes independently and
all at once."* No row or lane anywhere in the app resizes vertically today.
The roll's lanes are fixed at 55 and 72 px (`main.slint:6935`, `:7062`).
Step 03 fixed the panel's lanes at 72 px.

## The gesture

- **One lane:** drag the lane's bottom edge (`ns-resize`). Use the dock
  grip's idiom: re-anchor on the grab, clamp, and commit at the end of the
  drag (`main.slint:7915-7967`).
- **Every lane at once:** the same drag with a modifier sets every lane in
  the panel to the height being dragged. Choose a modifier the panel isn't
  already using for marquee or free snap (step 06). A **Lane height** row in
  the panel header's menu does the same for people who don't know the
  modifier.
- **Limits:** a lane is at least one header row tall, so its name stays
  readable at 200% text. That's the tall-text lesson from MOO-278 and
  MOO-328. The maximum is the panel's height.

## Where heights live

- **With the song**, as a `height` field on the song lane, omitted at the
  default. It is how the user arranged their panel, so it survives a reopen.
  The precedent is a device's `collapsed` flag
  (`core/src/effect.rs:4088`), which is saved with the song so a folded rack
  is still folded when reopened.
  - Treat a height the way `collapsed` is treated for undo and for the
    unsaved-changes flag. Check what that is in the tree, don't assume it,
    and write it down in `00-status.md`.
- **Nothing to the engine.** Resizing is drawing, not sound.

## Done when

- **Resizing:**
  - Dragging a lane's edge resizes that lane alone.
  - The modifier-drag and the menu row resize every lane.
  - A test for each, in `tests/dock_resize.rs` style.
- **Persistence:** heights survive save and reopen. A song whose lanes are
  all the default height saves byte-identical.
- **Navigation:** resizing sends nothing to the engine, and treats undo the
  way folding a device does.
