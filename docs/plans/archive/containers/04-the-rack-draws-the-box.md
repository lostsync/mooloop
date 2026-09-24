# 04 — The rack draws the box

Steps 01–03 leave a container that works and cannot be made. This step is the
gesture and the frame.

Read `UI_DESIGN.md` and `WIDGET_INVENTORY.md` first, and iterate with
`scripts/slint-sketch` rather than `cargo build -p mooloop-ui` — this is face
markup at 0.05s a look, and the one place it crosses into `main.slint` is the
row model, which should be crossed **once**, with every new field batched.

## What the row model has to carry

`EffectSlotRow` (`main.slint:144`) is flat and every row draws its full face
inline at its own rack-unit width. Two fields make a flat model draw a tree:

- `depth: int` — how many containers enclose this row.
- `span: int` — for a container, how many rows it encloses; `0` for a leaf.

That is enough to draw a frame around a run and to indent, and it keeps the
model flat, which matters because the rack is a horizontal sheet and a nested
model would have to be flattened for layout anyway.

A container's own face is small: a name, a mix knob (the `MiniKnob` the rail
already uses for per-slot wet/dry, at `device-rack.slint:234`), a bypass, and
a collapse. One rack unit.

## The two questions the prototypes raised

Recorded in the brief so they would not be rediscovered, and this is the step
that has to answer them.

**A container wider than the viewport needs a collapsed representation.** A
collapsed container draws as one rack unit showing its child count and its
mix, and its children are absent from the model rather than clipped. Collapse
is view state, not project state, on the same argument as
`Session::browser_expanded`: it is not something a project should reopen
differently because of.

**Vertical stacking cannot mean two things.** In the sketch, a rack that wraps
to the next row means "the chain continues"; a layer's branches also stack.
Both cannot be plain vertical adjacency. Since step 06 defers layers, this
step gets to fix the meaning: **vertical is continuation.** A layer, if it is
ever built, has to look different from a wrapped chain, and the constraint is
now written down rather than discovered when someone builds one.

## The gestures

The rack already has a drag-to-reorder with a half-unit horizontal threshold
(`device-rack.slint:423`). Containers add:

- **Wrap the selection** — one or more adjacent rows become a container's run.
  A container is far more often made around devices that already exist than
  inserted empty.
- **Unwrap** — the container goes, its children stay in place. Step 02 makes
  deletion take the run with it, so this is the escape hatch that makes that
  safe.
- **Drop into and out of a run** — the drop target has to distinguish "after
  the container" from "at the end of the container's run", which are the same
  pixel and different edits. Give the container's frame an inside edge.

## Done when

A container can be made from existing devices, collapsed, expanded, dragged as
a unit, dropped into and out of another container, and unwrapped — all with
the mouse, and all reflected in the saved project. Verified in the real window
through `scripts/mooloop-mcp`, not only in a snapshot: the drop-target
question above is an interaction, and a screenshot cannot fail it.
