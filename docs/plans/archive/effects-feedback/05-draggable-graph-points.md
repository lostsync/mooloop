# Give graph points a shared drag interaction

## Problem

From `docs/archive/EFFECTS_FEEDBACK.md`: "in general points on splines should be
draggable if it makes sense. for instance, a filter curve - the freq
point should be draggable x,y for freq,gain and maybe scrollwheel would
adjust Q." The Comp/Gate/Limiter feedback separately asks for a draggable
threshold point, and the EQ feedback separately asks for reliable point
selection/drag. All three want the same underlying capability.

The visualizers in `device-displays.slint` (`FilterResponseDisplay:89`,
`DriveTransferDisplay:148`, `BitcrushTrace:209`, `DelayEchoDisplay:245`,
`DynamicsCurveDisplay:307`) have no `TouchArea` today — pure,
non-interactive display `Rectangle`s. But this capability already exists
elsewhere and should be extracted, not reinvented: `EqResponseDisplay`
(`eq-device.slint:7`, private to that file) drags its 7 band points today
via a `for index in 7 : TouchArea` with a `moved` handler
(`eq-device.slint:62-79`) that moves *relative to the press point*
(`self.mouse-x - self.pressed-x`) rather than mapping absolute mouse
position — the code comments this explicitly as avoiding a feedback loop
because the handle re-centers under the pointer as it drags. That
relative-move pattern is the one to promote, not a new design from
scratch. It only handles x/y (no scroll-for-a-third-parameter yet, which
Filter's freq/gain/Q case needs).

## What to do

1. Extract `EqResponseDisplay`'s per-point `TouchArea` pattern
   (`eq-device.slint:62-79`) into a shared component (e.g.
   `DraggablePoint`) in `controls.slint` or the shared graph-canvas
   location, generalizing it to: pointer-down/move for x/y (kept
   relative-to-press, per the existing anti-feedback-loop comment), plus
   a `scroll-event` for a third bound parameter that Filter needs (Q) and
   EQ doesn't currently use.
2. Migrate `EqResponseDisplay` itself onto the shared component so there
   is exactly one implementation, and fix its known selection bug (see
   `11-eq-selection-and-layout.md`) as part of that migration rather than
   separately.
3. If `docs/plans/extract-mid-level-dsp-blocks/03-share-a-graph-canvas-for-device-displays.md`
   has landed, put the shared drag component there so every visualizer
   gets it from one place. If it hasn't landed yet, add it standalone and
   note it as a candidate to fold into the shared canvas later — do not
   block this on that unlanded plan.
4. Second consumer: `FilterResponseDisplay`'s cutoff/gain point (freq via
   x-drag, gain via y-drag, Q via scroll-wheel while hovering the point) —
   the concrete case named in the feedback, and the one that actually
   needs the new scroll axis EQ's version doesn't have.
5. Match `docs/UI_DESIGN.md`'s "graph handles must lie on the rendered
   envelope or curve" rule: the draggable point's rendered position must
   stay derived from the same transfer function the curve itself plots,
   not a separately-tracked coordinate that can drift from the curve —
   `EqResponseDisplay` already satisfies this (`band-value`-derived x/y);
   preserve it in the extraction.

## Verification

Software-rendered snapshot before/after a simulated drag
(`SLINT_BACKEND=winit-software`) on the Filter device showing the point
and curve moving together; a live/manual check that dragging updates the
actual DSP parameter (not just the visual) and that scroll-while-hovering
changes Q without also moving freq/gain.
