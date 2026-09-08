# Pane layout status

Written 2026-09-08. **Nothing has landed yet.** `README.md` holds the design
and the argument for it; this file records what each step changed once it is
done, and in particular anything the doing proved wrong about the plan.

## Steps

| Step | State |
| --- | --- |
| `01-the-pane-model.md` | **Landed 2026-09-08** on `feat/pane-split`. See below. |
| `02-the-split.md` | Not started |
| `03-zoom-and-resize.md` | Not started |
| `04-moving-a-view.md` | Not started |

## Step 01 — what the doing changed

The mechanism is in and the arrangement is unchanged: `STEPS`/`MIXER` in the
top slot, `DEVICES`/`NOTES`/`PLAYLIST` in the dock, each view drawn into a
computed rectangle rather than nested in layouts. Five `ViewSlot`s, five
`PaneToolbar`s, three tab strips' worth of membership, and the four inline
blocks moved without a single binding being re-threaded — which was the whole
bet, and it held.

Four things the plan did not predict.

**`editor-page` and `mixer-visible` stayed the writable state.** The plan had
step 01 introducing `main-views` / `split-views` / `bottom-views` outright.
Deriving the slot model from the two properties Rust already owns instead
meant `lib.rs` did not have to change at all, which kept the entire step to
one file of markup and therefore to `slint-sketch`'s three-second loop rather
than a four-minute build per mistake. Step 02 inverts the ownership, when a
third slot exists to need it.

**Membership is five scalars, not an array.** Slint has no way to assign into
an array element, and a slot is something you assign a view *to*. The `[int]`
array survives only so a tab strip can iterate its members.

**A nested model index reads correctly and does not evaluate.**
`slot-active[view-slots[v]]` is the obvious spelling, type-checks, and
silently yields the wrong answer — the first render had the piano roll's
toolbar drawn over the device rack's. Functions do work, and are what the file
uses now, with a comment saying so where the next person will reach for the
array.

**Slint will not take an `if` as the body of a `for`,** and a zero-width child
still earns its layout gap. So a tab strip's membership test is the tab's
*width*, with no spacing on the strip. Nothing is lost: only one tab is ever
tinted and an untinted tab is already the strip's own colour, so the 1px rules
that spacing would have drawn would have drawn nothing.

One thing worth having in hand for step 02: **`preferred-width: 0px;
preferred-height: 0px` is what lets a slot read `parent.width`.** Without it
the container sizes to its children while its children size off it, and the
layout closes a loop through `layoutinfo-v`. That was the first thing the
prototype hit and it is the load-bearing line in the work container.

## Decisions taken before any code, so they are not re-litigated

- **A view is in one slot at a time.** The alternative — the mixer visible in
  two slots at once — was considered and rejected: it makes a tab strip stop
  being a statement about what is on screen, makes a tab drag ambiguous
  between move and copy, and gives `Ctrl+2` two answers. Nothing in Adam's
  requirements asks for it.
- **`SOURCE` is renamed `DEVICES`.** It is the device chain, which is the name
  `UI_DESIGN.md` and the rest of the interface already use.
- **The split chip goes in the status bar, not a toolbar.** `panel-left` and
  `panel-bottom` are already there and already mean exactly this for the other
  two regions.
- **Zoom is a double-click on the active tab**, not a button per slot.
- **The status bar's layout chips read in screen order**, and each glyph draws
  its region where that region actually is. Adam's call, 2026-09-08. Applying
  it turned up that the browser toggle draws `panel-left` for a sidebar that
  is docked on the right; step 02 fixes that rather than adding a third chip
  to a row that is already wrong.
- **The mixer scrolls rather than compresses in a narrow split column**, per
  `UI_DESIGN.md` > Responsive Behavior. Adam's call, 2026-09-08.
- **Every view has exactly one toolbar row, led by its slot's tab strip.**
  Adam's call, 2026-09-08, on finding the bottom pane carrying two stacked
  toolbars: *"i dont see why it cant just be 1 dynamic toolbar with sensible
  controls in each."* The slot model requires it anyway — a slot-level header
  cannot hold per-view controls once views are placeable. The channel preset
  browser goes to `DEVICES` alone; `SONG ARRANGEMENT` goes entirely. Folded
  into step 01 rather than made a step of its own, because keeping the old
  content in the merged row would be doing the merge twice.
  See `README.md` > The implementation shape. This is what keeps the change
  off the four-minute rebuild loop for as long as possible.
