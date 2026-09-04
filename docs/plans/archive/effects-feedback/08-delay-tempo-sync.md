# Delay: add a tempo sync option

## Problem

From `docs/archive/EFFECTS_FEEDBACK.md`: "Delay: Tempo sync option."

`DelayEffect`'s params already carry a `bpm: 120.0` field
(`crates/mooloop-dsp/src/effects/delay.rs:254`); confirm first whether
that field is currently live-updated from the transport's actual BPM
(`MainWindow`'s `bpm` property, threaded through in
`mooloop-ui/src/lib.rs`) or is a vestigial/unused default, since that
determines whether this is mostly a UI change or also a plumbing fix.

## What to do

1. If `DelayEffect`'s `bpm` isn't wired to the live transport BPM, wire
   it — the effect needs the current tempo to convert a beat division
   into a delay time in ms.
2. Add the `TimeDivisionKnob` from `06-add-a-time-division-knob.md` to
   `delay-device.slint` in place of (or alongside, if both a free-running
   ms mode and a synced mode are wanted) the existing time knob.
3. Decide and document the behavior when tempo changes while synced: the
   delay time should follow the new tempo immediately (matching how a
   synced delay behaves in any host), not require a knob touch to
   re-apply.

## Verification

`cargo test -p mooloop-dsp` for the delay's time-from-division math; a
live/manual check that changing the project BPM while a delay is
tempo-synced audibly changes the echo spacing without touching the knob.
