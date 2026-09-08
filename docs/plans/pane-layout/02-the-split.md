# 02 — The split, its divider, and the status-bar chip

**Landed 2026-09-08.** `00-status.md` records what the doing changed.

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

**The split chip, and the cluster it joins.**

Adam, 2026-09-08, on the sketch:

> in your sketches, down in the statusbar, we have the bottom pane button to
> the right of the right pane button. let's make sure those buttons are in
> their screen order, so left side, middle stuff, right side.

So the cluster has a rule now, and it is worth writing down because it is what
makes a row of near-identical glyphs readable at all: **a chip sits in the
cluster where its region sits on the screen, and its glyph draws that region
in the same place.** A cluster ordered any other way is three 16px squares
that have to be learned individually.

**Checking that rule against what is there found a defect.** The browser
sidebar is the *second* child of the work-area `HorizontalLayout` and
`main.slint:1990` says outright that it is "docked on the right" -- but its
status-bar chip draws `ToolIcons.panel-left`, whose rule is at `x 6.5`, the
left third. The cluster is already lying about screen position, and it has
been since the chip was written. Fix that here rather than adding a third chip
to a row that is wrong: it is two characters in a path and it is the whole
reason the ordering rule is worth having.

**The order is by each region's left edge**, which is the only reading of
"screen order" that is monotone and does not need a tiebreak: the dock starts
at `x 0`, the split starts at the divider, the browser starts at the sidebar's
edge. That also puts the dock chip to the *left* of the split chip, which is
what Adam pointed at.

The family, all on the shared `M 2.5 3 L 13.5 3 L 13.5 13 L 2.5 13 Z`
outline. Position alone did not separate them -- a rule at `x 8` and a rule at
`x 10` are the same 16px square at a glance, and the sketch said so. What
carries the distinction is **fill**: a docked panel appears and disappears, so
it is drawn as a solid region; a split is two editors and is drawn as two
empty halves. That is the same distinction VS Code's split-editor icon makes
against its panel icons, and for the same reason.

| Chip | Region | Rule | Fill |
| --- | --- | --- | --- |
| dock | the bottom dock, from `x 0` | `M 2.5 9.5 L 13.5 9.5` | `M 2.5 9.5 L 13.5 9.5 L 13.5 13 L 2.5 13 Z` |
| split | the top pane divides in two | `M 8 3 L 8 13` | none |
| browser | the sidebar, at the right edge | `M 11 3 L 11 13` | `M 11 3 L 13.5 3 L 13.5 13 L 11 13 Z` |

The fill is the same brush as the stroke at 0.55 opacity, drawn as a second
`Path` under the outline, because one `Path` has one fill and filling the
outline would fill the whole window shape.

`panel-left` keeps its definition in `ToolIcons` for a left-docked panel,
because `FOCUS.md` step 3 draws a left channel sidebar and it will want it.
It just stops being what the browser toggle uses. `panel-bottom` gains its
fill; it is the same icon, saying the same thing, in the family the other two
now belong to.

Same hand-rolled two-state chip as its neighbours, same 20px box, tinted while
the split is open.

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
