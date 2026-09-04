# Plate: fix knobs overflowing the frame, give the scope real content

## Problem

From `docs/archive/EFFECTS_FEEDBACK.md`: "Plate: Knobs extend outside of frame.
Unclear what should be in scope - doesnt really show anything. Scope
should probably show the shape of the reverb. Maybe we could try to make
this look cool, maybe a spectral display that kinda lets you 'see' the
tone, eg low end might ring out longer and be shown at the bottom. NI
does something like this in RC24 and RC48."

Read directly from `plate-device.slint`: the device is declared `kind:
"FX / 1U"` (`plate-device.slint:11`), and per `docs/UI_DESIGN.md` a unit
is quantized in 220px increments. The knob row
(`plate-device.slint:55-99`) holds 4 `ParameterKnob`s at 69px width each
= 276px total, which does not fit a 220px (1U) face — this is the
concrete overflow, not a rendering glitch. The "scope" area
(`plate-device.slint:41-54`) is a plain `Rectangle` containing only the
static caption text `"comb / allpass"` — there is no plotted content at
all, confirming "doesn't really show anything" exactly.

## What to do

1. Fix the overflow: either widen the device to the next unit (e.g. 1.5U
   per `docs/UI_DESIGN.md`'s "half-unit widths are valid for compact
   effects") to fit the existing 4 knobs, or shrink knob width/diameter to
   fit inside 1U — pick based on what reads better rather than defaulting
   to whichever is less code.
2. Replace the static caption with a real display of the plate's decay/
   tone shape, derived from the same `size`/`decay`/`damping`/`width-value`
   parameters that drive `PlateEffect`'s DSP
   (`crates/mooloop-dsp/src/effects/plate.rs`) — per
   `docs/UI_DESIGN.md`'s "device plots derive from the parameters and
   transfer functions used by the audio path," not a decorative curve.
3. Consider the spectral/tone-over-time idea from the feedback (low end
   ringing longer, shown lower in the display, similar to NI RC24/RC48) as
   the target look — this is more visually ambitious than a simple
   waveform-envelope trace like Drum Synth's voice-shape preview, so scope
   it honestly: if a true spectral-over-time view is too heavy for this
   device's size/budget, a simpler decay-envelope trace (matching the
   Reverb device's new display from `12-reverb-scope-rework.md`, for
   visual consistency between the two reverb-family devices) is an
   acceptable fallback — decide and note which was chosen and why.

## Verification

Software-rendered snapshot confirming all 4 knobs render fully inside the
device frame with no clipping, and that the new display visibly changes
shape as Size/Decay/Damping/Width are adjusted.
