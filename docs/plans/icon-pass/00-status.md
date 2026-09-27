# icon-pass — status

Linear: project [Icon pass](https://linear.app/mooloop/project/icon-pass-2146c941d0d3).
Steps: 01 is MOO-279, 02 is MOO-273, 03 is MOO-280, 04 is MOO-281, 05 is
MOO-282. Milestones: **A device shows what it is** (01-02, 0.1.6) and **One
family, one meaning** (03-05, 0.1.7).

Planned 2026-09-26. Step 01's Interface leg landed on its branch 2026-09-27;
the Mixer, Instruments and Effects legs are still to run (MOO-279).

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
| 01 — the registry | In progress (MOO-279): Interface's leg done |
| 02 — device kinds | Todo, blocked by 01 (MOO-273) |
| 03 — one family | Backlog (MOO-280) |
| 04 — one meaning, one icon | Backlog (MOO-281) |
| 05 — words to icons | Backlog (MOO-282) |

## `scripts/dupe-audit icon-literal`

**First run, 2026-09-27, against `main` at `a7ea4425`, before anything
moved: 88.** By kind:

| Kind | Count | What |
| --- | --- | --- |
| path | 29 | Every line of the four sets (`ToolIcons` 14, `StripIcons` 4, `SamplerDeviceIcons` 4, the EQ face's 6) and the Preferences close X. |
| element path | 3 | The colour picker's "none" (twice: the swatch and the chip) and `ConsoleButton`'s sine. |
| glyph | 55 | The survey's glyph sites, one per line; it counted "about 45". The extra are the transport's `▶ ⏸ ■ ●`, the settings `⚙`, the menu's `✓`, the browser row kinds `◇ ◈`, the math module's operator labels, and the device rack's `⇱ ▱ ❏ ⧉`. |
| glyph (lead) | 1 | The `→` inside the math module's clamp readout; step 04 decides it. |

Validated by reading every hit, and by what it does not report: the knob
arcs (`controls.slint`), the envelope (`envelope.slint`), the response
curves (`device-displays.slint`), the master compressor's needle
(`master-comp.slint`) and the automation lane's computed `MoveTo`s are all
built from values, and none is reported. The two LFO panels' fixed waveform
pictures (`poly-device.slint`, `mono-device.slint`) are allowlisted in the
check as displays: they are the hardcoded curve `WIDGET_INVENTORY.md` §12
already records as a bug. `"±0.0"` is a signed number
and is not reported.

**After Interface's leg (`b4ce5cfb`): 71.** `ToolIcons`' 14 lines, the close
X and the colour picker's two "none" Paths moved into the registry. What is
left of the path kind is the other teams' sets (Mixer 4, Instruments 4,
Effects 6), which their relay legs move.

## The FemtoVG cost gate (01), measured 2026-09-27

**Decision: Paths go ahead.** At the icon count of housey's saved layout they
add about 0.3 ms a frame against glyph text, under the 0.5 ms gate.

Laptop on AC, `performance` profile, release binaries built on antibox, each
run in its own headless sway (a real GPU window, FemtoVG), with
`PIPEWIRE_REMOTE=none`.

**200 icons** (`crates/mooloop-ui/examples/icon_cost.rs`): the window repaints
every frame while the icons stay still. Five rounds of 4 s per mode,
interleaved, and about 1,070 frames each. The cost is the UI thread's,
`BeforeRendering` to `AfterRendering`.

| Mode | Frame mean | p99 | Over the empty window | Per icon |
| --- | --- | --- | --- | --- |
| none | 106 µs | 134 | - | - |
| filled `Icon` (a Path) | 1,295 µs | 1,476 | 1,189 µs | 5.9 µs |
| outline `Icon` (today's) | 954 µs | 1,552 | 848 µs | 4.2 µs |
| glyph `Text` (`×`) | 688 µs | 780 | 582 µs | 2.9 µs |
| SVG `Image` + `colorize` | 598 µs | 896 | 492 µs | 2.5 µs |

A three-round run earlier gave the same order and the same numbers to within
40 µs. So a filled Path costs about **3.0 µs more than a glyph**, and 3.4 µs
more than an SVG image.

**Housey's saved layout** shows about 105 icons, counted on a screenshot of
it: the transport and toolbar about 25, the channel rack's rows 27, the device
rack's rails and headers 22, the browser's disclosures 15, the sidebar's
menus, the layout chips and the rest about 16. At 3.0 µs each, drawing all of
them as filled Paths instead of glyphs adds **about 0.32 ms** a frame. The
gate is 0.5 ms, so it passes with room for about 60 more icons at today's
costs. A layout that reaches about 165 icons would cross it; step 04 should
count again when it converts the glyphs.

**Before and after `ToolButton` draws through `Icon`**
(`MOOLOOP_PROFILE_UI=scenario`, 10 s phases). Base is `main` at `a7ea4425`,
branch is `b4ce5cfb`, alternated base, branch, base, branch:

| Phase | base 1 | branch 1 | base 2 | branch 2 |
| --- | --- | --- | --- | --- |
| playing, saved layout: frame mean | 3,809 µs | 3,707 | 3,947 | 3,805 |
| playing, device rack | 3,826 | 3,728 | 3,789 | 3,681 |
| playing, mixer | 4,931 | 5,151 | 4,817 | 4,800 |

No difference beyond run-to-run noise, as expected: `Icon` *is* a `Path`, so
the move changed no element count.

If a later layout does fail the gate, the fallback stays the plan's: `Icon`
draws an SVG `Image` instead, behind the same names.
