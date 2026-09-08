# 01 — The pane model and the computed work area

**Goal: change the mechanism and nothing else.** At the end of this step the
interface looks and behaves exactly as it does today — same two panes, same
switchers, same dock heights, same divider — and is running on the slot model
that steps 02-04 need.

Doing it this way means the step that is invisible is also the step that is
easy to verify: any visible difference is a defect.

## What it builds

**The view and slot state, on `main.slint`'s root.**

```
// 0 STEPS, 1 MIXER, 2 DEVICES, 3 NOTES, 4 PLAYLIST -- the order Ctrl+1..5
// already uses in actions.rs, so a chord is Ctrl+(id + 1) and a tab strip
// renders its members in a stable order without carrying one.
in-out property <int> steps-slot: 0;      // 0 main, 1 split, 2 bottom
in-out property <int> mixer-slot: 0;
in-out property <int> devices-slot: 2;
in-out property <int> notes-slot: 2;
in-out property <int> playlist-slot: 2;
```

These defaults are today's layout, which is the point. **Scalars rather than
an array per slot**, because Slint cannot assign into an array element and a
slot is something a view is assigned to.

**Per-view dock heights**, replacing the conditional chain at the bottom
pane's `height` binding. `DEVICES` keeps its derived `442px + shelf delta`,
because that is an intrinsic height rather than a remembered one.

**The computed work area.** One container replacing the nested
`HorizontalLayout` / `VerticalLayout` between the toolbar and the status bar.
Slot rectangles are derived once at the top and read by everything below:

- `work` is the window less the chrome rows and the status bar.
- `bottom-extent` is the active bottom view's height, intrinsic or remembered.
- `top-height` is `work.height - 1px - bottom-extent` while the dock is open.
- `main` and `split` divide `work.width` at `split-fraction`; with the split
  closed, `main` is the whole width.

**The four inline view blocks, wrapped and positioned.** The channel rack, the
playlist, the device rack and the piano roll each move inside a positioned
`Rectangle` and are otherwise untouched — no property is re-threaded, no
callback is forwarded. `MixerPane` is already a component and gets the same
treatment.

**The slot tab strip**, one component used three times, in the position each
pane's `SegmentedControl` occupies today. Its membership is `views`; its
`active` is an index into that. With today's defaults it renders
`STEPS | MIXER` and `DEVICES | NOTES | PLAYLIST`, which is what is there now.

## What it deliberately does not build

No split, no zoom, no tab drag, no status-bar chip. `split-views` stays empty
and the code paths that would show it stay unreachable. This is what keeps the
step verifiable against "looks identical".

## The map, taken 2026-09-08 against `8fa7ceb`

Brace-matched, so it is exact rather than eyeballed. Re-derive it if the file
has moved under you; do not trust these numbers after any other commit lands.

| Block | Lines | Becomes |
| --- | --- | --- |
| Toolbar container, `height: 62px` | 1713-1983 | `height: 33px` -- transport row plus its hairline, nothing else |
| Transport row | 1721-1822 | unchanged, stays in the toolbar |
| Hairline, `opacity: 0.6` | 1824 | unchanged; it is what keeps the boundary looking the same |
| **Work-surface row**, `height: 29px` | 1826-1981 | the `STEPS` view's own toolbar |
| Standalone `1px` border | 1986 | moves inside each top slot's header as a bottom edge |
| Work-area `HorizontalLayout` | 1993-5194 | keeps its shape: work container, then sidebar |
| Main-column `VerticalLayout` | 1996-4868 | **replaced** by the computed work container |
| `MixerPane` | 1999-2018 | the `MIXER` view |
| Channel rack `ScrollView` | 2020-2313 | the `STEPS` view's body |
| Dock splitter | 2315-2356 | the horizontal divider, generalised in step 03 |
| Bottom dock `Rectangle` | 2364-4866 | dissolved; its geometry becomes the bottom slot's |
| Dock header, `height: 30px` | 2391-2440 | dissolved: the switcher becomes the slot's tab strip, the channel name goes to `DEVICES` and `NOTES`, the preset controls to `DEVICES` alone |
| Playlist toolbar, `height: 34px` | 2447-2525 | merged into the `PLAYLIST` row |
| Playlist page | 2442-2892 | the `PLAYLIST` view |
| Device chain toolbar, `height: 34px` | 2899-2954 | merged into the `DEVICES` row |
| Device rack page | 2894-4215 | the `DEVICES` view |
| Piano roll toolbar, `height: 34px` | 4222-4356 | merged into the `NOTES` row |
| Piano roll page | 4217-4864 | the `NOTES` view |
| Browser sidebar | 4870-5193 | unchanged |

**The top pane's toolbar is not inside the top pane.** That is the one thing
the map turned up that the design did not predict: the work-surface row lives
in the 62px toolbar block, *above* the work area, which is why there can only
be one of it today. It has to move into the slot before a second slot can have
a toolbar of its own. The move is invisible -- the rows are adjacent already,
and keeping the `0.6` hairline above and putting the `1.0` one below the
header reproduces the boundary exactly.

## One toolbar per view

The map found the bottom pane carrying **two** stacked toolbars — a 30px slot
header and a 34px per-page row — and the shared header holding controls that
only two of its three pages want. Adam, 2026-09-08:

> that toolbar in the bottom half needs love anyway. it has that preset
> dropdown that doesn't really make sense to be there and is mostly just empty
> space. there's a second toolbar that pops up below it e.g. in the piano roll
> mode. i dont see why it cant just be 1 dynamic toolbar with sensible
> controls in each.

**It can, and the slot model requires it.** A slot-level header cannot hold
per-view controls once views are independently placeable — and the shared row
already had to ask `editor-page != 2` which of its controls to draw, which is
`UI_DESIGN.md`'s own stated symptom for a control in the wrong place:

> A setting that belongs to a pane lives in that pane's header, once. A
> control that has to ask which pane is open in order to know which value it
> is editing is in the wrong place — that question is the symptom.

So the rule is one line and it covers all five views, top and bottom:
**every view has exactly one toolbar row, and its slot's tab strip leads it.**
That is the rule the document already states for a switcher, applied once
rather than twice.

### What each row carries

`···` is the stretch. One row, `30px`, `Theme.surface`, `24px` controls, 3px
padding, 6px spacing, a 1px bottom border. Nothing shrinks to fit: the piano
roll's controls are already 24px, which is `ToolbarMetrics.control-height`.

| View | Row |
| --- | --- |
| `STEPS` | `[tabs] │ PAT [n] [name] [+] │ [tools] │ STEPS [n] GROUP [n] ···` |
| `MIXER` | `[tabs] ···` |
| `DEVICES` | `[tabs] │ DEVICE CHAIN [source] ··· [channel] [CHANNEL PRESET] [save]` |
| `NOTES` | `[tabs] │ [tools] [SNAP] [interval] │ TICK NOTE VEL LEN [len] │ VEL AUTO ··· [channel]` |
| `PLAYLIST` | `[tabs] │ SNAP [interval] [Loop] [range] ···` |

Three content decisions, each answering something Adam named:

- **The channel preset browser goes to `DEVICES` only.** It is on `NOTES`
  today because the shared header drew it for `editor-page != 2`, and on the
  piano roll a whole-channel preset browser is noise — you are editing notes,
  not the channel's sound. `DEVICES` is the view whose subject *is* the
  channel's sound. `FOCUS.md` step 3's left channel sidebar is its eventual
  home; this is where it lives until that exists.
- **`SONG ARRANGEMENT` goes.** It is a label saying what the pane is, and the
  tab beside it already says that. It is the same permanent-chrome sentence
  the work-surface row's own comment records deleting once already.
- **The channel name stays on `DEVICES` and `NOTES`**, and appears on neither
  of the others, because neither of those edits a channel.

### What it buys

**34px back in the bottom pane**, since two rows of 30 and 34 become one of
30 — which is the negative-space complaint this plan started from, applied to
the other half of the window. The `DEVICES` view's fixed height drops with it.

And the empty space goes. The shared header was a switcher, a name, a stretch
and a preset browser, so most of its width *was* the stretch. Merged, that
width carries the view's actual controls.

### Watch: the merged `NOTES` row clips sooner

Measured with `scripts/slint-sketch` before building it. At 1280px the merged
row ends around x 1090, with room to spare. At the 704px a 55% split column
gives it, it clips just after the snap interval, losing the note-property
steppers and the lane toggles — roughly 200px sooner than today, because the
tab strip is now in front of it.

That is the documented behaviour for this row rather than a new defect —
`UI_DESIGN.md`: *"The row clips rather than widening the window; keys 1-6
reach the tools and the snap toggle when it is narrow."* **The tab strip is
deliberately the part that never clips**, because it is the way out of the
pane. Step 03's zoom is the answer for actually editing notes in a narrow
column.

If it turns out to bite, the fix is a standard toolbar overflow menu at the
row's right edge, not a smaller control — but do not build one until a narrow
column is something Adam is actually working in.

## Watch for

- **`editor-page` has readers well outside the dock.** The menu bar's View
  rows, `Select All Notes`, the shortcut handler, the `changed height` clamp,
  and the piano-roll-only branches around lines 1383-1416. Each becomes a
  question about a *view* — "is NOTES the active view of its slot" — not about
  an index. Find them all before moving anything; a missed one is a shortcut
  that silently stops working, which is the failure mode `FOCUS.md` step 3
  spent a branch on already.
- **Computed geometry drops Slint's minimum-size propagation.** The clamps in
  `changed height` and inside the grip are load-bearing and must survive.
- **Reading `parent.height` inside a slot whose height feeds the layout loops.**
  The sketch hit this: bind against `root.height` less the known chrome
  instead. The comment explaining why belongs in the file.

## Done when

The application is indistinguishable from `main` at 1080p and at 960x760
**except for the toolbar merge**, which is the one visible change and is
specified above control by control: both panes, both switchers, the dock
divider on the notes page and not on the others, every dock height as it was
less the 34px the merge returns, every View menu row and every `Ctrl+1..5`
chord doing what it did.

Verified from a software-rendered screenshot and from `scripts/mooloop-mcp`
driving the switchers, not from the source. `scripts/slint-sketch` takes
`ui/main.slint` itself — 2.7s to type-check, 3s to render with empty models
— so the whole restructure iterates there and crosses into a `mooloop-ui`
build once.
