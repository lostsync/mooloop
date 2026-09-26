# 03 — one family

Adam, 2026-09-26: **filled everywhere.** Step 01 moved the existing icons in
as 1.3 px outlines, unchanged. This step redraws them filled, in the device
icons' hand, and deletes `Icon`'s transitional `outline` mode.

## What gets redrawn

- `ToolIcons`:
  - the five tool modes (select, paint, slice, stretch, loop);
  - the panel toggles: `panel-frame`, left, right, bottom and split. Each
    panel toggle has a `-fill` variant today; see below.
- `StripIcons`: bell, high shelf, low shelf, dot.
- `SamplerDeviceIcons`: previous, next, search, reverse.
- The EQ face's shelves, passes, analyzer and proportional-Q triangle.
- The layout chips, the dialog's close X, and the colour picker's "none".

**The panel toggles' fills already mean something.** A chip is told apart by
which region is filled, not by where it sits (`UI_DESIGN.md:870-875`,
`plans/archive/pane-layout/02-the-split.md`). Keep that meaning. A filled
family draws the frame solid and the named region knocked out, or the other
way round, as long as the region still reads as the difference.

## One set of EQ shapes

The strip and the EQ face draw the same meanings (bell, the shelves, the
passes) with two different sets of paths, and the face shows a bell as its
band **number**. Pick one drawing per meaning and delete the other. The strip
is Mixer's and the face is Effects'. Put a side-by-side sheet to both owners,
and to Adam if the two teams disagree.

## `ConsoleButton`

It draws a flat line when off and a sine when on (`controls.slint:2084`),
built from `MoveTo`/`CubicTo` elements. It is an icon that shows state. Either
it becomes two registry entries, `console-off` and `console-on`, filled, or it
stays a drawing, and the check's allowlist says why. Decide by looking at both
at 1×.

## Done when

- `Icon` has no `outline` mode, and every registry entry is filled.
- The EQ has one set of shapes, used by the strip and the face.
- The mockup catalog's icon sheet shows the whole registry at 1× and 2×, and
  Adam has seen it.
- Rung 4 on antibox, and every snapshot that changed has been looked at.

Owner: **Interface**, with Mixer and Effects for the EQ shapes. 0.1.7.
