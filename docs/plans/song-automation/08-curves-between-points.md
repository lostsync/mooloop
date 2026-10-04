# 08 — curves between points

Adam: *"there are no curves. not a dealbreaker but it'd be nice to have"*.
A lane today is straight lines only:
- `AutomationPoint` has no shape (`core/src/automation.rs:36`);
- `value_at` interpolates linearly (`:294`);
- the UI draws `LineTo`.

`docs/plans/archive/automation-curves/` is a different thing: the engine's
per-control-tick delivery path. That path is what makes a curved lane cheap
to play, because the engine already evaluates the lane every 32 frames, not
once a block.

## The shape

**The shape belongs to the segment that starts at a point**, so it is a field
on `AutomationPoint`:
- **Linear**, the default, and what every existing point is.
- **Hold**: the value stays until the next point, then jumps. Use it for
  stepped changes.
- **Curve**, with a tension of −1..1. Zero is linear; positive bends towards
  a slow start and a fast finish, and negative the reverse. Use one function,
  for example a power curve, whose exponent the tension maps to, and keep it
  monotonic so a ramp never overshoots its ends.

**The type is shared** with pattern lanes (step 01), so pattern lanes can
hold curves the moment this lands. The piano roll gets the editing in step 09.

## Changes

- **Core.** `AutomationPoint` gains `shape` and `tension`, and `value_at`
  evaluates them.
  - The engine evaluates at control ticks. Keep `value_at` free of anything
    that allocates or branches unboundedly; it runs in the callback.
  - Write it so the per-block cost of `block_cost.rs` doesn't move for an
    all-linear song. A linear segment takes the same path it does today.
- **Format.** Both fields are omitted at their defaults, so every existing
  song saves byte-identical (`docs/PROJECT_FORMAT.md`). Integrity clamps
  tension to −1..1 and repairs an unknown shape to Linear.
- **Drawing.** A curved segment is drawn as the curve it plays, not as an
  approximation of it: sample `value_at` across the segment at the lane's
  pixel resolution, in Rust. Step 03's line test extends to a curved segment,
  whose ends must still land on its points.
- **Editing.**
  - Dragging a segment's **middle** vertically sets its tension, and
    Alt-click resets it. The handle is drawn at the segment's midpoint only
    while it is hovered.
  - The context menu (step 06) sets the shape of the selected points'
    segments.
  - It is one undo entry per drag.

## Done when

- **Core tests:**
  - `value_at` for Hold and Curve at both tension extremes and at zero.
  - Zero tension equals linear to the bit.
  - No curve leaves its ends' range.
- **Engine:** a curved song lane plays its curve per control tick, against
  the core function.
- **Format:** an existing song saves byte-identical, and a curved lane
  round-trips.
- **Drawing:** a snapshot test checks a curved segment's drawn path against
  `value_at`.
- **`docs/CURRENT.md`** describes curves.
