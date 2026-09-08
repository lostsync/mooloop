# Pane layout status

Written 2026-09-08. **Nothing has landed yet.** `README.md` holds the design
and the argument for it; this file records what each step changed once it is
done, and in particular anything the doing proved wrong about the plan.

## Steps

| Step | State |
| --- | --- |
| `01-the-pane-model.md` | **Landed 2026-09-08** on `feat/pane-split`. See below. |
| `02-the-split.md` | **Landed 2026-09-08.** See below. |
| `03-zoom-and-resize.md` | **Landed 2026-09-08.** See below. |
| `04-moving-a-view.md` | **Landed 2026-09-08**, less the tab context menu. See below. |

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

## Step 02 — what the doing changed

The top pane splits, the divider drags and resets and closes, and the status
bar's layout cluster reads in screen order with a glyph family that can be
told apart.

**`editor-page` and `mixer-visible` are gone.** Step 01 kept them and derived
the slot model from them; that could not survive this step, because
requirement 1 is `PLAYLIST` full-width in the *top* pane and a page index of
the lower dock cannot name a view that has left it. Rust now reveals a view
with `invoke_show_view(id)` and asks after one with `get_showing_notes()` and
friends, which stay true however the panes are arranged. `apply_pane` went
from six lines of "clear the mixer flag, then set a page index" to one.

**The view ids are public.** `mooloop_ui::view::{STEPS, MIXER, DEVICES, NOTES,
PLAYLIST}` — ten test files set `editor_page` directly and now reveal a view
the way the application does, off one definition rather than a literal each.
`view_id(Pane)` is the only place the session's `Pane` and the Slint ids meet.

**Closing the split leaves `main-active` alone.** The first cut moved the
split's view into the main slot, which is wrong: closing a split takes a pane
away, it does not change what the pane you kept was showing. The exception is
dragging the divider all the way *left* — there the main slot is the one being
squeezed out, so what survives is what the split was showing.

**The `View` menu is one row per view.** It was three rows for the lower
dock's pages; it is five now, checked on `showing-*`, with the `Ctrl+1..5`
chords shown and a `Split Top Pane` row under a separator. `menubar.rs` moved
with it: Playlist is row 4 rather than row 2, which is a coordinate the test
had hard-coded.

**Both narrow-column worries needed no code, which is worth recording so
they are not re-checked.** `MixerPane` already sets
`viewport-width: max(self.visible-width, strip-row.preferred-width)`, so a
strip row wider than its column scrolls rather than compressing -- which is
what `UI_DESIGN.md` asks for and what Adam confirmed he wanted. The channel
rack's own `ScrollView` already derives its viewport width from the pattern
length, so halving its column is the case it was written for. Verified by
reading them, not assumed.

**The dock chip and the browser chip swapped places, and two tests clicked
the old ones.** Ordering by each region's left edge puts the dock first and
the browser last, so in a 960px window the dock's chip went from centre 944 to
892 and the browser's from 922 to 944 -- each landing where the other had
been, which is the worst possible arrangement for a coordinate a test
hard-codes, since the click keeps working and toggles the wrong thing.
`dock_resize.rs` caught it; `sidebar.rs` would have opened the split instead
of the sidebar. Both now carry the chip-position formula in a comment rather
than a bare number.

**The layout cluster's glyphs needed fill, not position.** A rule at `x 8` and
a rule at `x 10` are the same square at 16px — the sketch said so before any
of it was built. A docked panel is drawn solid and a split is drawn as two
empty halves, which is the true distinction and the one VS Code draws too.
The browser chip had been drawing `panel-left` for a sidebar docked on the
right since the chip was written; that is fixed here rather than left beside
a new chip.

## Step 03 — what the doing changed

Any pane fills the window on a double-click of its active tab and comes back
on another or on `Esc`. The bottom divider is live for any view that does not
declare its own height, and each view remembers the height it was left at.

**"Resizable except on devices" is one fact, not a condition.** `DEVICES` is
the only view with an intrinsic height, because a face is a fixed 268px. So
the grip's test is `!PaneViews.fixed-height[bottom-active]` rather than a
named page, and the playlist became resizable without anything being written
for it. The `changed height` clamp -- the one that catches the *window*
shrinking under a tall dock -- moved to the same test.

**Per-view dock heights were already there, written as a special case.** The
old binding hardcoded 410px for the playlist, 442-plus-shelf for the source
page, and a draggable number for notes. Those are three per-view heights in a
conditional chain; they are now four writable properties and two functions,
and `STEPS` and `MIXER` have one each because step 04 lets them into the dock.

**`Esc` had to be given a way to lose.** It already routed through
`shortcut-key` to the action dispatcher, and nothing binds it by default, so
the zoom branch sits in front of that and only fires when something is zoomed
*and* no dialog is open. Anything later bound to `Esc` still receives it, and
the preferences dialog was already rejecting every key above this branch.

**A chord and a menu row have no pointer, so they need to know which pane was
meant.** `active-slot` is the last slot a view was revealed in, set by
`show-view`, which every tab, chord and menu row already goes through. It is
not a focus model and does not pretend to be one -- `interface-iteration/`
step 04 is where that lives -- but it is an honest answer where the
alternative was to guess a slot.

**`ACTIONS.md`'s action count was stale before any of this.** It claimed 46
where the table held 45. It is 47 now and correct, which it reached partly by
accident; the document already says to read `actions.rs` rather than trust the
number, and that remains the right instruction.

## Step 04 — what the doing changed

A tab drags between panes, dropping on the right edge of an unsplit top pane
opens the split, and the `View` menu names the same moves for anyone who has
not found the gesture. The mixer reaches the bottom pane, which is Adam's
requirement 4 and the one case of the general answer.

**`PaneDrag` is `RackDrag` again, deliberately.** The two halves of a drag sit
in different places and neither contains the other: the grab is in a tab strip
that knows its view and nothing about the work area's bounds, the landing is
worked out by the work area from its own. The rack learned that the hard way
-- it used to divide pointer travel by a *nominal* row pitch and land short of
a wide device -- and the pointer is kept in window coordinates
(`absolute-position + mouse-x`) for the reason `RackDrag.dx` spells out: a
touch area's own `mouse-x` measures against a ruler the previous frame already
slid.

**The landing is a derived property, not something written on every move.**
`work.drop-slot` is computed from the pointer and the slot bounds, and the
drop reads it. There is nothing to keep in sync, which is a class of bug the
rack's `target` has to be careful about.

**The main slot may not empty**, or there is no pane left to drop anything
back onto. The last view in it refuses at the *grab* rather than accepting the
drag and snapping back, because a refusal you can see before you commit is
worth more than one you discover at the end.

**A drop highlight is a tint, not an opening gap.** The rack animates rows
aside because a reorder must say *where in a sequence*; a pane drop only has
to say *which pane*.

**The menu bar has no submenus, so `Move To >` is three rows.** They name the
view — "Move Mixer to Bottom Pane" — because a row that does not say what it
acts on is a row you have to try. Fifteen flat rows (five views by three
slots) was the alternative and is worse than three.

**Not built: the right-click tab menu.** The plan lists it, and it is the
discoverable version of gestures that are otherwise only gestures. The three
`View` rows cover the same operations, and a tab context menu wants menu
machinery that does not exist yet. Left deliberately, recorded here rather
than in the step file, which now describes something that is done.

**Not built: persisting the arrangement.** The plan reserved this as a
decision rather than an oversight and the decision is *not yet*: an
arrangement is UI state, not project state, so `PROJECT_FORMAT.md` is the
wrong home and a song must not carry a window layout. Where it does belong is
the settings file that already holds the palette seeds and the shortcut
bindings. Doing it needs no new mechanism, only the call.

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
