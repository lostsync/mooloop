# 03 — Zoom, and the bottom divider generalised

**Landed 2026-09-08.** `00-status.md` records what the doing changed.

**Goal: requirements 5 and 6.**

## Zoom

`zoomed` is a slot, or none. A zoomed slot is the whole work area; the other
two are not drawn. The toolbar, the menu bar and the status bar stay, which is
Adam's *"the whole window except tools and chrome"* read literally.

- **Enter and leave by double-clicking the active tab.** The gesture from a
  title bar, on the control that names the pane.
- **`Esc` restores**, and does not consume the key when nothing is zoomed.
- The zoomed tab takes the full accent rather than the muted active fill, so
  the state is visible on the control that set it.
- The status bar says how to get back, in the hint channel it already has.
- A row under `View`, and an action id, so it is reachable without knowing the
  gesture. `view.zoom-pane` toggles the pane that owns the active view.

**Zoom does not move a view.** Leaving it puts everything back exactly where
it was, which is what makes it safe to use for a glance.

## The bottom divider

Today: `enabled: root.editor-page == 1` — live on the notes page only.

After: live unless the bottom slot's active view has an intrinsic height, and
`DEVICES` is the only view that does. The playlist becomes resizable, which it
was not, and the device rack stays fixed, which Adam asked for by name.

The height a drag sets is the *active view's* remembered height, so switching
tabs restores the height that view was left at rather than sharing one number
across three editors. The three hardcoded heights in today's conditional chain
become those three views' defaults.

## Watch for

- **The dock's open/close animation and the grip fight over `height`.** The
  existing code steps the animation aside with `Motion.gesture-active` while
  the grip owns the value. Zoom is a third writer of the same geometry and
  must join that arrangement rather than work around it.
- **`Esc` already has meanings.** Check the dialogs and the piano roll's
  marquee before claiming it; restoring from zoom is the least specific of
  them and must lose every conflict.

## Done when

Any pane fills the window on a double-click and comes back on another or on
`Esc`; the piano roll zoomed is the window less two rows; the bottom pane
resizes on notes and on playlist and refuses to on devices; and each view
comes back to the height it was left at.
