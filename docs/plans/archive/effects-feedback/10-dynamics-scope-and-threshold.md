# Comp/Gate/Limiter: show the input signal on the scope, and make threshold draggable

## Problem

From `docs/archive/EFFECTS_FEEDBACK.md`: "Comp/Gate/Limiter: All 3 need to show
the input signal on their scopes such that it is possible to have a
visual idea of where to set things. we could also show the threshold
point on the curve in the scope. it should probably be draggable."

`DynamicsCurveDisplay` (`device-displays.slint:307`) currently plots a
static transfer curve (input-dB → output-dB, parameterized by
`threshold-db`/`ratio`/`knee-db`/`extra-db`, mode-switched for
gate/compressor/limiter) plus a unity reference line. There is no
live-input indicator at all — nothing shows where the actual incoming
signal currently sits on that curve, and the threshold is drawn as part
of the curve shape, not as a distinct, identifiable, draggable point.

## What to do

1. Get the current input level into the display. Check what metering
   data already flows from the engine to the UI for effect slots
   (`crates/mooloop-engine/src/meters.rs` — effect-chain stage meters
   exist per `MAX_EFFECTS_PER_CHANNEL + 1` stages) and whether that
   per-slot level is already exposed to Slint properties for other scopes,
   or needs a new binding for `DynamicsCurveDisplay` specifically.
2. Render the live input level as a moving marker/dot on the curve
   (`output-db(current-input-db)` gives its y) or as a separate trace
   showing recent input history (a small horizontal scroll of levels,
   similar in spirit to `SampleTrace`) — pick whichever reads better at
   this display's size; a single moving dot is simpler and may be
   sufficient for "a visual idea of where to set things."
3. Add an explicit, visually distinct threshold marker on the curve (it's
   currently implicit in the curve's bend, not drawn as its own point),
   and make it draggable using the shared `DraggablePoint` component from
   `05-draggable-graph-points.md` (itself extracted from
   `EqResponseDisplay`'s existing relative-move drag pattern,
   `eq-device.slint:62-79`) — vertical drag adjusts `threshold-db` (and
   for Limiter, the ceiling, per the component's existing "gate and
   compressor: threshold, limiter: ceiling" comment at
   `device-displays.slint:309`).

## Verification

Software-rendered snapshot with a synthetic input level showing the
marker positioned correctly relative to the curve; a live/manual check
dragging the threshold point and confirming the effect's actual
`threshold-db` parameter updates and the curve redraws accordingly.
