# EQ: fix point selection, resolve the band-toggle row, sync selected-band params, fix vertical space

## Problem

From `docs/archive/EFFECTS_FEEDBACK.md`: "Has a lot of UI quirks, mostly around
point selection. Clicking a point doesn't reliably select it, especially
if there are multiple on top of each other. There is a row of buttons
from Low -> LP under the scope. They toggle on and off, but i dont think
that does anything? There's a separate ON toggle that does seem to be the
actual on/off control for the selected band. When you do select a band,
the params in the UI do not update to show this. For example that on/off
button wouldn't change from ON to OFF if i clicked from an enabled band
to a disabled one. We don't currently have enough vertical space to stack
the scope, buttons under it, and then knobs too."

Read directly from `eq-device.slint`, four distinct issues, each with a
concrete cause already visible in the code:

1. **Selection reliability.** `EqResponseDisplay`'s per-band handles
   (`eq-device.slint:62-79`, `for index in 7 : TouchArea`) only select a
   band as a side effect of `point-dragged` inside the `moved` handler
   (`eq-device.slint:118`: `point-dragged(index, freq, gain) => {
   root.target-changed(index / 8); ... }`). A plain click with no
   perceptible pointer movement never fires `moved`, so it never selects
   anything — this alone explains "doesn't reliably select." Overlapping
   handles compound it: when multiple bands share the same freq/gain, the
   `TouchArea`s stack and only the topmost (highest `index`, drawn last)
   receives the click.
2. **The LOW→LP row's apparent no-op toggle.** The row at
   `eq-device.slint:120-128` uses `ToggleButton` (`checked`/`toggled`
   semantics, implying independent per-item on/off) for what is actually
   a single mutually-exclusive band *selector*: `checked: root.target-index
   == index; toggled(_) => { root.target-changed(index / 8); }`. It looks
   like 9 independent on/off switches but behaves like one radio group —
   a widget-type mismatch, not a wiring bug. This is a different control
   from the actual per-band enable, which is the separate `ON` button at
   line 127 (`checked: root.enabled >= 0.5`).
3. **Selected-band params not updating.** Need to trace where
   `frequency`/`gain`/`q`/`enabled` on `EqDeviceFace` get re-derived when
   `target` changes — confirm whether the Rust side
   (`mooloop-ui/src/lib.rs`) re-syncs these four properties from the
   newly-selected band's stored values on every `target` change, or only
   on an explicit `target-changed` callback that might not fire on every
   selection path (e.g. clicking the LOW→LP row itself does call
   `target-changed`, so check whether *that* path re-syncs correctly while
   the point-click path doesn't, given issue 1 above).
4. **Vertical space.** The face stacks the scope (126px,
   `eq-device.slint:117`), the LOW→LP + ON row (22px,
   `eq-device.slint:120-128`), and the Freq/Gain/Q knob row
   (`eq-device.slint:129-136`) inside a fixed 268px rack height
   (`docs/UI_DESIGN.md`'s device-face height contract) — confirm the
   actual overflow/clipping and whether the device needs a second stable
   page (per `docs/UI_DESIGN.md`'s "device with more controls than one
   face can hold uses stable internal pages") rather than fighting for
   space in one page.

## What to do

1. Fix selection: give each band handle (once migrated onto the shared
   `DraggablePoint` from `05-draggable-graph-points.md`) an explicit
   click/pointer-down selection independent of movement, and resolve
   overlapping-handle ambiguity (e.g. hit-test smallest-distance-to-press
   rather than pure z-order, or nudge overlapping handles visually).
2. Replace the LOW→LP row's `ToggleButton`s with a `SelectorBank`
   (matching `04-clean-up-device-headers.md`'s unification) so it reads
   as the single-selection control it actually is, instead of 9 fake
   independent toggles.
3. Ensure `frequency`/`gain`/`q`/`enabled` are re-derived from
   `band-data[target-index]` on every path that can change `target`
   (point click, row click, and any future keyboard/tab selection),
   not just the ones exercised today.
4. Resolve the vertical space conflict — either tighten the three rows to
   fit 268px cleanly, or split into two stable pages (e.g. "bands" page
   with scope + selector + knobs, "curve" page with the analyzer toggle
   and slope/character controls) per the device-face paging contract.

## Verification

Software-rendered snapshot of the EQ face showing the reworked selector
row and confirming knob values change immediately on band selection; a
live/manual check clicking overlapping points on two bands tuned to the
same frequency/gain to confirm both are independently selectable.
