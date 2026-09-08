# Pane layout status

Written 2026-09-08. **Nothing has landed yet.** `README.md` holds the design
and the argument for it; this file records what each step changed once it is
done, and in particular anything the doing proved wrong about the plan.

## Steps

| Step | State |
| --- | --- |
| `01-the-pane-model.md` | Not started |
| `02-the-split.md` | Not started |
| `03-zoom-and-resize.md` | Not started |
| `04-moving-a-view.md` | Not started |

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
