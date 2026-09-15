# Pane layout

Adam, 2026-09-08:

> in the top half, the pattern editor, at 1080p and 16 steps, a bunch of the UI
> is negative space. i want to make it so that that top pane can be a split
> view. [...] this a pretty IDE-adjacent UI paradigm which is cool and we're
> going to lean into it, so potentially there are some established design
> patterns that can be used for placing controls for this. don't just throw a
> bunch of buttons in a new toolbar somewhere. be intentional.

His requirements, in his order:

1. Pattern, mixer, or playlist full-width in the top pane.
2. Any combination of two of them in the top pane.
3. The split divider drags.
4. Access to the mixer in the bottom pane is acceptable.
5. The bottom pane resizes generally — **except on the devices page**.
6. Either top pane, or the bottom pane, can go full-window: "so that e.g. the
   piano roll takes up the whole window except tools and chrome".

## The idea, in one sentence

**There is one set of views and three slots that host them, a view lives in
exactly one slot at a time, and the switcher each pane already has becomes the
list of what that slot holds.**

That is the editor-group model from every IDE, and it is worth naming why it
is the right one here rather than a coincidence. Requirements 1, 2, 4 and 6
are all the same requirement — *this thing, in that place, at that size* —
asked four times about four surfaces. A design that answers them one at a time
grows a control per answer: a mixer-in-the-bottom toggle, a split-the-top
toggle, a maximize button per pane. A design that answers them once grows a
*rule*, and the rule is what the tab strip already almost says.

## What is already here, and is not being replaced

This is deliberately not a new interface. Four things in the tree already
point at this model, and the work is mostly joining them up.

- **Both panes already lead their toolbar with a switcher.**
  `UI_DESIGN.md` > Toolbars: *"A pane's switcher leads the toolbar whose
  contents it decides"*, and *"Two switchers for two panes look the same. Both
  are `SegmentedControl`."* The top pane's is `STEPS | MIXER`; the bottom
  pane's is `SOURCE | NOTES | PLAYLIST`. **A tab strip is a segmented control
  whose membership is state instead of markup.** Nothing about its shape,
  position or behaviour changes.
- **The status bar already owns the layout toggles.** `panel-left` for the
  browser sidebar and `panel-bottom` for the dock, drawn as one family:
  *"the outline is the window, the inner rule marks the pane each button
  reveals."* The split is the third member of that family and belongs in that
  cluster. It is the one new persistent control this plan adds.
- **The bottom pane already has a divider with the right behaviour.**
  `dock-grip` in `main.slint` — a 1px line, a 6px grab zone, a moving-origin
  drag integrator, and re-anchoring when a bound swallows the move. The
  vertical divider is that component rotated, and the "resizable except on
  devices" requirement is its existing `enabled: root.editor-page == 1`
  generalised rather than a new condition.
- **`Ctrl+1`..`Ctrl+5` already name the five views globally.**
  `actions.rs` registers `view.pane-steps/mixer/source/notes/playlist`. Today
  a chord means "switch a specific pane to this page" and has to know which
  pane that is. Under the slot model it means *"show this view, wherever it
  lives"*, which is both simpler and what the ids already say.

## The model

**Five views.** `STEPS`, `MIXER`, `PLAYLIST`, `NOTES`, `DEVICES`. (`SOURCE` is
renamed `DEVICES`: it is the device chain, the name the rest of the interface
and `UI_DESIGN.md` > Device Rack Layout already use, and "source" reads as a
generator now that the pane holds a whole chain.)

**Three slots.** `main` and `split` side by side across the top, `bottom`
below. `split` exists only while it holds something.

**A view is in exactly one slot.** Its tab is in that slot's strip and
nowhere else. This is the rule the whole design rests on:

- A slot's tab strip is a *partition* of the view set, so no strip can lie
  about what is on screen.
- Dragging a tab **moves** rather than copies, which is the only thing a drag
  can unambiguously mean.
- `Ctrl+2` has exactly one answer to "which mixer".
- Two copies of the mixer was never a feature anyone asked for; requirement 4
  asks for the mixer to be *reachable* from the bottom, and moving it there
  is that.

**One view has an intrinsic height.** `DEVICES` — a face is a fixed 268px and
`UI_DESIGN.md` says so twice. Every other view stretches to its slot. That
single fact is requirement 5: the bottom divider is live unless the bottom
slot's active view is `DEVICES`, which is what the existing grip's
`editor-page == 1` was approximating with the one page it had been built for.

**Each view remembers its own height.** `main.slint` already half-does this,
in a conditional chain that hardcodes 410px for the playlist, 442px-plus-shelf
for the source page, and a draggable `piano-dock-height` for notes. Those are
three per-view heights written as a special case; the model makes them the
ordinary case.

## The gestures, and why each one is the established pattern

| Want | Gesture | Precedent |
| --- | --- | --- |
| Show a view | Click its tab | The switcher, unchanged |
| Move a view to another pane | Drag its tab onto that pane | Editor-group drag, everywhere. The rack's drag-to-reorder is already this vocabulary: face follows the pointer, origin stays as an outline |
| Open / close the split | The `panel-right` chip in the status bar | Sits beside `panel-left` and `panel-bottom`, which do exactly this for the other two regions |
| Resize | Drag the divider | `dock-grip`, rotated |
| Reset a split to even | Double-click the divider | Double-click-to-default, which every knob in the program already does |
| Full-window a pane | Double-click its active tab | Double-click a title bar to maximise. `Esc` restores |

**Nothing here is a new toolbar and only one is a new persistent control.**
The split chip is the single addition, and it lands in an existing cluster of
two controls that already mean precisely this.

### Why double-click a tab, rather than a zoom button per pane

A zoom button is three buttons — one per slot — for a mode that is entered
rarely and left immediately. Double-clicking the thing that names a pane is
the maximise gesture from title bars, and it costs no chrome at all. The state
is not hidden: the zoomed tab takes the full accent rather than the muted
active fill, the other slots are gone from the screen, and the status bar says
how to get back. It is also reachable from `View` and from a chord, which is
where a keyboard-first user will look first anyway.

## The implementation shape, which is the surprising part

The obvious reading of "any view in any slot" is that every view becomes a
component instantiated once per slot — three copies of the playlist's
bindings, three of the mixer's. That is roughly 1,300 lines of forwarding in
`main.slint` and a `mooloop-ui` rebuild for every property that turns out to
be missing, at four to nine minutes each.

**It is not necessary, and the one-slot rule is why.** A view is only ever in
one place, so it only ever needs *one* instance. What changes is not which
subtree hosts it but *where that subtree is drawn*. So:

- The work area becomes one container with **computed rectangles** for the up
  to three visible slots, instead of nested `HorizontalLayout` /
  `VerticalLayout`.
- Each view is instantiated **exactly once**, with the bindings it has today,
  inside a positioned `Rectangle` whose `x`/`y`/`width`/`height` are its
  slot's rectangle and whose `visible` is "I am my slot's active view".
- The three tab strips are the only genuinely new markup, and they are one
  small component used three times.

The four inline blocks — the channel rack, the playlist, the device rack, the
piano roll — **do not have to move or be re-threaded at all.** They get
wrapped and positioned. Extracting them into their own files afterwards is
hygiene worth doing, and it is no longer a prerequisite for any of this.

The cost of computed geometry is that Slint stops propagating minimum sizes
for us. `main.slint` already clamps the dock by hand, in two places and for a
reason its comments record, so this is a cost the file is already paying.

## Sketch

The shell was prototyped against the real widgets with `scripts/slint-sketch`
before any of it was built, per `AGENTS.md`. It renders the split, both
dividers, the tab strips, the status-bar cluster and the zoomed state, and it
confirmed the one thing worth confirming early: a zoomed bottom pane really is
the window minus the chrome row and the status bar, and the geometry does not
loop when a slot's height is the work area's own.

## The steps

Numbered files, worked in order. Each ends in something usable.

- `01` — the pane model and the computed work area, and one toolbar per view.
  Same views in the same places; the only visible change is that the bottom
  pane's two stacked toolbars become one, which the slot model forces and
  which returns 34px.
- `02` — the split, its divider, and the status-bar chip.
- `03` — zoom, and the bottom divider generalised past the devices view.
- `04` — moving a view between panes, and the actions/menu that name all of it.
