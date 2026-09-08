# 02 — The split, its divider, and the status-bar chip

**Goal: requirements 1, 2 and 3.** Pattern, mixer or playlist full-width in
the top pane; any two of them side by side; a divider that drags.

## What it builds

**The split slot.** `split-views` becomes reachable. The slot is drawn only
while it holds a view, so a closed split costs no pixels and no divider.

**The vertical divider.** `dock-grip` rotated: a 1px line taking `Theme.focus`
while hovered or pressed, a 7px grab zone straddling it, `ew-resize`, and the
same moving-origin integrator with re-anchoring on a clamped move — the
divider travels with the value it is setting, so replaying from the press
position is wrong here for the same reason it was wrong there.

- Bounds: 15% to 85% of the work area's width.
- Double-click resets to even.
- Dragging past the floor collapses the split and folds its views back into
  `main`, which is the established "drag it to nothing to close it" behaviour
  and means the divider is also the close gesture.

**The `panel-right` chip.** Third in the status bar's layout cluster, between
`panel-left` and `panel-bottom` so the three read left-to-right as the regions
they toggle. Same hand-rolled two-state chip as its neighbours, same 20px box,
tinted while the split is open. Its glyph is the existing family with the rule
on the other side:

```
M 2.5 3 L 13.5 3 L 13.5 13 L 2.5 13 Z M 9.5 3 L 9.5 13
```

Opening the split with an empty `split-views` has to put *something* in it.
It takes `main`'s next view — the one after the active tab, wrapping — which
is the answer that needs no prompt and no menu. With only one view in `main`,
the chip is disabled and its hint says so.

## Watch for

- **The top pane is genuinely short while `DEVICES` is open.** At 800px window
  height the sketch gives it 270px; at 1080p, roughly 550. A split at that
  height is two narrow columns, and the mixer's strips have a minimum width.
  Decide what a strip does when its column is too narrow — scroll, per
  `UI_DESIGN.md` > Responsive Behavior, rather than compress — before the
  first screenshot, not after.
- **The channel rack's `ScrollView` already sets `viewport-width` from the
  pattern length.** Halving its width is exactly the case it was written for,
  so this should need nothing; confirm it rather than assume it.

## Done when

`STEPS` and `MIXER` sit side by side with a divider that drags smoothly and
resets on a double-click, `PLAYLIST` can be either half or the whole width,
the chip opens and closes the split, and the top pane at 1080p with sixteen
steps is no longer mostly empty — which is the complaint this plan started
from and the only test of it that matters.
