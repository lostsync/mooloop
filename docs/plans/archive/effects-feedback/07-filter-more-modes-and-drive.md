# Filter: add band-pass and other modes, a slope control, and a drive stage

## Problem

From `docs/archive/EFFECTS_FEEDBACK.md`: "Need BP. Other filter modes would be
cool, e.g. Moog, etc. Needs poles or db/oct, a way to set slope. Note is
shown in freq knob value but that should only be visible in the tooltip.
Filter should have a sat/drive control."

`FilterMode` (`crates/mooloop-core/src/effect.rs:527`) currently defines
only `LowPass`/`HighPass` — there is no band-pass, no ladder/Moog-style
mode, and nothing modeling slope/pole count; the DSP node
(`crates/mooloop-dsp/src/effects/filter.rs`) branches on exactly those two
modes. The note-name display is presumably computed alongside the freq
knob's `value-text` in `filter-device.slint` — once
`02-rework-knob-label-value-layout.md` lands, that value moves into the
bright `ValueReadout` box, so "tooltip only" means removing it from that
box's always-visible text and adding it to the knob's `Tooltip` markdown
instead.

## What to do

1. Extend `FilterMode` with `BandPass` (and evaluate a Moog-style
   ladder/4-pole mode as a second addition — keep it a real different
   topology, not a relabeled biquad, if it's added). Every match on
   `FilterMode` in `mooloop-core` and `mooloop-dsp` needs the new arm(s);
   grep both crates for `FilterMode::` before assuming `effect.rs` and
   `filter.rs` are the only sites.
2. Add a slope/pole-count parameter (e.g. 12/24 dB per octave, or an
   explicit pole count) — this likely means cascading the existing SVF
   stage rather than a new filter type. `crates/mooloop-dsp/src/filter.rs`
   already has the shared SVF/one-pole primitives from
   `docs/plans/archive/share-dsp-primitives/`; reuse them for the cascade rather
   than hand-rolling a second implementation.
3. Add a saturation/drive stage. Check whether `DriveEffect`'s waveshaper
   (`crates/mooloop-dsp/src/effects/drive.rs`) is reusable as a shared
   primitive (per the share-dsp-primitives adoption work) before writing
   a new one.
4. Move the note-name text out of the freq knob's persistent value
   display into its tooltip only, once the readout styling from
   `02-rework-knob-label-value-layout.md` exists to know exactly where
   that text currently lives.
5. Update `filter-device.slint` for the new mode selector (per
   `docs/UI_DESIGN.md`: small fixed mode count → `SelectorBank`, matching
   the style unified in `04-clean-up-device-headers.md`) and the new
   slope/drive controls, staying within the device's existing rack-unit
   footprint if at all possible.

## Verification

`cargo test -p mooloop-dsp` and `cargo test -p mooloop-core` for the new
`FilterMode` variant(s) and slope math; a software-rendered snapshot of
`filter-device.slint` showing the new mode selector and drive control;
confirm the frequency response display (`FilterResponseDisplay`,
`docs/UI_DESIGN.md`'s "Response displays" section already anticipates
band-pass: "the reusable display supports low-pass, band-pass, and
high-pass even while current instruments expose low-pass") actually plots
the new modes correctly.
