# Drop the redundant in-device name, align header rows, and unify the type-toggle style

## Problem

Three related complaints from `docs/archive/EFFECTS_FEEDBACK.md`:

1. "In every device, it has its own name up at the top. The device's name
   is already shown in its frame. Maybe we drop the in-device name and
   left-align the header row buttons above the scopes if they exist.
   Related issue: in devices with no buttons up there like bitcrush and
   the dynamics ones, this header area is taller than in devices like
   drive and filter that do have buttons above the scope."
2. "Looking at drum synth and reverb next to each other, i see two similar
   but different styles of header-placed type toggle buttons. which is
   our 'real' one? i probably prefer the smaller style from effects
   devices. it looks like in the smaller all caps selector buttons from
   effects devices, longer labels like 'CHAMBER' stress the left/right
   padding."

`EffectDeviceShell` (`device-rack.slint`) already owns the device's name
in its 28px identity header
(`docs/UI_DESIGN.md`: "Every device has a 28 px identity header with
enabled state, name, kind, and size"), so a second name inside the face
is a literal duplicate, not just visual clutter.

The two toggle styles are genuinely two different components:
`SegmentedControl` (`controls.slint:116`) lays its options out in a
`HorizontalLayout` with `horizontal-stretch: 1` per segment, so segment
width is divided evenly across the control's total width regardless of
label length — this is what stresses "CHAMBER"'s padding in
`reverb-device.slint:85` (`SegmentedControl { options: ["STUDIO",
"CHAMBER", "HALL"]; }`). `SelectorBank` (`controls.slint:147`) instead
gives every segment the same fixed `segment-width` (content-sized, not
row-stretched) and is what Drum Synth uses for kick/snare/hat character
(`drum-device.slint:66-85`). Adam's stated preference is the
`SelectorBank` style.

## What to do

1. Find every effect face that repeats its own name/title text at the top
   of its working-controls area (below the shell header) and remove it —
   `EffectDeviceShell`'s header is authoritative. Grep each
   `*-device.slint` face for a `Text` bound to a device-name property near
   the top of its layout.
2. For header-row buttons that currently sit above a scope (Drive,
   Filter), left-align them per the feedback instead of whatever alignment
   they use now.
3. Normalize the header-row height so devices without buttons (Bitcrush,
   the three Dynamics types) don't reserve a taller empty band than
   devices with buttons — this likely means the button row's height
   should collapse to zero (or a fixed minimal strip) when there are no
   buttons, rather than always reserving button-row height.
4. Replace every effect-device use of `SegmentedControl` for a type/mode
   toggle with `SelectorBank`, matching Drum Synth's smaller, fixed-width,
   all-caps style. `reverb-device.slint:83-89`'s room-type toggle
   (`STUDIO`/`CHAMBER`/`HALL`) is the concrete instance named in the
   feedback — audit other devices (Mod's mode selector, Bitcrush if it
   gains a style row per
   `09-bitcrush-algorithm-toggle.md`) for the same swap. Do not delete
   `SegmentedControl` itself without checking whether anything legitimately
   wants even-stretch segments (e.g. a control meant to fill a full row
   width) — if nothing does after this pass, that's a follow-up cleanup,
   not part of this step.
5. Pick a `segment-width` for `SelectorBank` wide enough that "CHAMBER"
   doesn't clip or crowd — content-measure the widest label per instance
   rather than reusing the default 44px blindly.

## Verification

Software-rendered snapshots of Drive, Filter, Bitcrush, one Dynamics
device, Drum Synth, and Reverb side by side
(`SLINT_BACKEND=winit-software`) to confirm: no duplicate name, consistent
header height across all of them, and one visual style for type toggles
everywhere. Check against `docs/UI_DESIGN.md`'s "repeated modules use the
same dimensions and control order" checklist item.
