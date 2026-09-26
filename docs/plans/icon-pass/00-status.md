# icon-pass — status

Linear: project [Icon pass](https://linear.app/mooloop/project/icon-pass-2146c941d0d3).
Steps: 01 is MOO-279, 02 is MOO-273, 03 is MOO-280, 04 is MOO-281, 05 is
MOO-282. Milestones: **A device shows what it is** (01-02, 0.1.6) and **One
family, one meaning** (03-05, 0.1.7).

Planned 2026-09-26. Nothing is built yet.

## Adam's rulings

**2026-09-26, on MOO-273's sketch:** *"B, filled shapes. There has been an
'icon pass' planned for a while. Maybe it is time to go ahead and do this? I
thought it might make sense to build an icon registry to help with
consistency and avoid dupes."*

**2026-09-26, in chat:** *"i have been thinking we should have our icons in
some kind of library class or something. let's plan out that icon pass."*
He then took the recommended answer to all four of the plan's questions:

| Question | Answer |
| --- | --- |
| Do action icons follow the device icons' filled style? | **Filled everywhere.** One family; the existing outlines are redrawn (03). |
| How does the registry store icons? | **Path strings in one Slint global**, extending `ToolButton.icon`. Step 01 measures the FemtoVG cost first; SVG with `colorize` is the fallback, behind the same `Icon`. |
| Who draws them? | **Our own, on one 16 px grid.** No outside set. |
| How much is in 0.1.6? | **The registry and the device icons** (01, 02). Steps 03-05 are 0.1.7. |

Still standing as recommendations, not yet overruled: a plugin gets **one
generic icon**, and a device's icon takes the **device colour**.

## Steps

| Step | State |
| --- | --- |
| 01 — the registry | Todo (MOO-279) |
| 02 — device kinds | Todo, blocked by 01 (MOO-273) |
| 03 — one family | Backlog (MOO-280) |
| 04 — one meaning, one icon | Backlog (MOO-281) |
| 05 — words to icons | Backlog (MOO-282) |

## To record when it happens

- `scripts/dupe-audit icon-literal`'s first count, against the unfixed tree (01).
- The FemtoVG cost measurement and the gate's decision (01).
