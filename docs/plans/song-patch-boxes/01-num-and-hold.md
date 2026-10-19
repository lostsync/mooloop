# 01 — num and hold (MOO-533)

Two control boxes, and the canvas showing a box's value.

- **`num`** (`num`, `num int`, `num 0 1`, `num note`): one control inlet,
  one outlet. It shows the value passing through. With nothing wired in, it
  sends the value dragged or typed into it, which makes it a constant too.
  `int` rounds, two numbers clamp the range (`num 0 1` is unsigned), `note`
  shows the value as a pitch (`0..1` = note/127, as `held` sends it).
- **`hold`**: `in` and `trigger` → `out`. Keeps `in` as of its last trigger
  (a trigger inlet, as MODULATION.md defines one). The held value is not saved.
- **Values on the canvas**: the canvas already reads outlet activity for
  cable blinking; it now reads a box's value too, so `num` can show it.

## Done when

Both can be typed, saved, reloaded and heard; a `num` shows a live value;
tests cover parse/spell round trips and the tick.
