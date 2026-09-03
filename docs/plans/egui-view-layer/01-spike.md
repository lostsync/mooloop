# 01 — Spike: one pane, and a decision

Read `00-status.md` first. The session layer extraction must be finished.

The point of this step is **a decision, not a foundation.** Write it to be
thrown away, and judge it on what it teaches rather than on what it leaves
behind.

## What to build

A separate binary — `crates/mooloop-egui-spike`, not wired into
`mooloop-app` — that opens a window with `eframe`, holds a real `Session`, and
draws **the channel rack and the transport row**, live, against a running
engine.

That pane specifically, because it is the smallest thing that exercises every
category the real migration has to handle:

- a grid with per-cell interaction and drag-past-the-press-cell behaviour
  (`docs/CURRENT.md` notes the whole run of steps shares one hit area because a
  per-cell one cannot follow a drag — find out what immediate mode does with
  that),
- rows driven by a `Vec` that changes length,
- knobs and a meter,
- transport state refreshing on a timer against `Session::tick`,
- a custom-drawn widget with no toolkit equivalent.

Do not build a theme system, a settings dialog, or a widget abstraction. Draw
the pane the crudest way that works.

## What to measure, and write down

The spike's output is a findings section appended to `00-status.md`. Four
numbers and two judgements:

**Numbers**

1. Incremental rebuild time for a one-line visual change in the spike binary.
   This is the number that decides the plan; compare it against
   `scripts/slint-sketch`'s 0.05s type-check and 0.2s screenshot.
2. Lines of Rust for the pane, against the Slint version's markup plus
   projection code.
3. Frame time with the engine running and meters updating, on Adam's machine.
   Immediate mode redraws everything every frame; the rack is 256 addressable
   channels.
4. Binary build time from clean, since it gates how painful the migration's
   middle is.

**Judgements**

5. Does the step grid's drag behaviour survive? It is the interaction the
   current UI had to work hardest for.
6. Does drawing a visualizer feel as much better as `docs/WIDGET_INVENTORY.md`
   suggests it should? Draw one polyline plot and compare it to the
   seventeen-hand-rolled-workarounds situation it replaces.

## What would make this a no

Any of:

- the rebuild loop is slow enough that UI work stops being fun,
- frame time with a full rack is not comfortably inside the refresh budget,
- the step grid's drag needs more fighting in immediate mode than it did in
  Slint.

A no here is a good outcome, not a wasted week. It converts a standing "should
we switch" question into a recorded answer, and `docs/ARCHITECTURE_REVIEW.md`
already establishes that nothing else in the project is waiting on it.

## What would make this a yes

The projection layer is gone and not missed, the visualizers are obviously
better, and the rebuild loop is survivable with a small binary. In that case
write steps 02–04 properly, with the spike's numbers in them, before continuing.

## Verification

None in the usual sense — this builds nothing the application depends on. Keep
the branch, keep the findings, and do not merge the spike into `main` unless
step 02 is going ahead.
