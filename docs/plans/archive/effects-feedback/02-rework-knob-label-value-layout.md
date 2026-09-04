# Move the knob label above the knob, the value below it as a bright readout

## Problem

From `docs/archive/EFFECTS_FEEDBACK.md`: "Move knob labels above the actual knob
and put the knob's value display where those labels currently are, right
beneath the knobs. Put the values in a bright color, inside a small dark
box appropriately sized for the value to emulate a small readout display.
monospace ui font could work well here, maybe something like Lilex."

`ParameterKnob` (`crates/mooloop-ui/ui/controls.slint:313`) currently
stacks knob face → label (`Theme.text-muted`, 9px) → `ValueReadout`
(`controls.slint:205`, a `Theme.surface`-background box holding 10px
`Theme.text`). The box already exists; this is a rearrange-and-restyle,
not new structure. `MiniKnob` (`controls.slint:728`) likely repeats the
same stack and needs the same treatment — check it for its own
label/value ordering before assuming it matches `ParameterKnob`.

## What to do

1. In `ParameterKnob`'s `VerticalLayout` (`controls.slint:370-401`),
   reorder to: label text → knob face → `ValueReadout`. Labels stay
   `Theme.text-muted`/9px per `docs/UI_DESIGN.md`'s "module titles are
   quieter than parameter labels" hierarchy — only the *value* becomes the
   bright readout, not the label.
2. Restyle `ValueReadout`: keep the dark box (`Theme.surface` or a
   slightly darker/recessed variant if that reads better against
   `Theme.surface`), set the text color to an accent/bright tone (check
   `Theme` in `controls.slint`/`theme.slint` for an existing "bright"
   token before inventing a new one — reuse `Theme.accent` if it's legible
   at small size, otherwise add one theme color, not a one-off literal).
   Size the box to the text's `preferred-width` plus fixed padding rather
   than a fixed `min-width`, so it reads as sized-for-the-value rather
   than a generic pill.
3. Switch the value font to a monospace face. Evaluate bundling Lilex
   (https://github.com/mishamyrt/Lilex, OFL-licensed) as an embedded font
   resource for `mooloop-ui` (Slint supports embedding fonts via
   `Cargo.toml`/`slint-build`); if bundling is awkward, fall back to
   whatever monospace family is already available on the target systems
   and note that as a deliberate fallback, not an oversight.
4. Apply the same reorder/restyle to `MiniKnob` if it duplicates the
   label/value stack, so every knob in the rack matches — this is the
   kind of shared-component change that must not be done twice with two
   different outcomes.
5. Re-check every device face that uses `ParameterKnob`/`MiniKnob` (all
   nine effects, the source devices, mixer strips) for label truncation
   or misalignment now that the label is above rather than below — labels
   were previously below the fixed-diameter knob and may need different
   width assumptions above it.

## Verification

Software-rendered snapshot of the control gallery
(`SLINT_BACKEND=winit-software MOOLOOP_GALLERY_SNAPSHOT=/tmp/gallery.ppm
cargo run -p mooloop-ui --example control-gallery`, per `AGENTS.md`) to
check every knob variant at once, then spot-check two or three device
faces (e.g. Filter, EQ) for label/value alignment per `docs/UI_DESIGN.md`'s
Agent Acceptance Checklist.
