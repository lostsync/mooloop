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
// 0 STEPS, 1 MIXER, 2 PLAYLIST, 3 NOTES, 4 DEVICES
in-out property <[int]> main-views: [0, 1];
in-out property <[int]> split-views: [];
in-out property <[int]> bottom-views: [4, 3, 2];
in-out property <int> main-active;
in-out property <int> split-active;
in-out property <int> bottom-active;
```

These defaults are today's layout, which is the point.

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
| Dock header, `height: 30px` | 2391-2440 | splits: the switcher becomes the slot's tab strip, the channel name and preset controls go to `DEVICES` and `NOTES` |
| Playlist page | 2442-2892 | the `PLAYLIST` view |
| Device rack page | 2894-4215 | the `DEVICES` view |
| Piano roll page | 4217-4864 | the `NOTES` view |
| Browser sidebar | 4870-5193 | unchanged |

**The top pane's toolbar is not inside the top pane.** That is the one thing
the map turned up that the design did not predict: the work-surface row lives
in the 62px toolbar block, *above* the work area, which is why there can only
be one of it today. It has to move into the slot before a second slot can have
a toolbar of its own. The move is invisible -- the rows are adjacent already,
and keeping the `0.6` hairline above and putting the `1.0` one below the
header reproduces the boundary exactly.

**The channel identity row is shared by two views.** The dock header carries
the channel name and the channel preset browser for `editor-page != 2`, which
is `DEVICES` and `NOTES`. Once those two are independently placeable it cannot
be one row in one slot, so each gets its own copy. It is the same information
about the same channel and both views edit a channel; `UI_DESIGN.md`'s "the
lower editor retains one channel row" survives as "the view that edits a
channel says which channel", which is the rule it was standing in for.

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

The application is indistinguishable from `main` at 1080p and at 960x760 —
both panes, both switchers, the dock divider on the notes page and not on the
others, every dock height as it was, every View menu row and every `Ctrl+1..5`
chord doing what it did. Verified from a software-rendered screenshot and from
`scripts/mooloop-mcp` driving the switchers, not from the source.
