# 02 — every device kind has an icon

MOO-273. The device header opens with a 4×16 bar and an 8×8 dot
(`device-rack.slint:699-710`). Both show the device's colour, and neither is
labelled. Adam, 2026-09-26: *"i dont really know what the symbol is supposed
to mean currently"*. This step replaces the pair with an icon for the device's
**kind**, drawn from the registry in style B. It also puts the same icon on
the folded strip, where the dot was (`device-rack.slint:912`).

## The kinds

The sources are held in the order of `SOURCE_KINDS_IN_PICKER_ORDER`, the
numbers `device_kind_to_int` sends:

- Sampler;
- Gitdum DS-SX;
- Mono Synth and Poly Synth: retired, but an old song still shows them, so
  they need icons;
- Munotone ML-M1;
- Polyneight ML-P8;
- Dominic DS-01;
- Aux In.

The effects are held in the order of `EffectKind::ALL`, by
`effect_kind_index`: EQ, Mod, Filter, Preamp, Drive, Bitcrush, Delay, Reverb,
Plate, Gate, Comp, Bus Comp, Limiter, Buffer, Chain and Layer.

**Plugin** gets one generic icon, a plug, whether the plugin is a source or
an effect. Adam hasn't overruled that recommendation.

That is about 26 drawings. The four in the MOO-273 sketch are a start:
`/tmp/moo273-sketch/icons.slint`, which has DS-01, ML-P8, EQ and Reverb in
style B.

## How to draw them

- **16×16 grid, filled.** Holes use `fill-rule: evenodd`. Edges fall on whole
  or half pixels, so a 14 px icon at 1× is crisp: MOO-273's sketch showed
  outlines going soft at 1×, and that is why Adam chose filled.
- **A silhouette per kind**, readable at 14 px without its name. The test is
  the mockup sheet below at 1×, not at 2×.
- **Related kinds look related, never identical.** Reverb and Plate, Comp and
  Bus Comp, ML-P8 and Poly Synth, Chain and Layer can each share a motif. The
  registry test refuses two equal strings anyway.
- **Draw the thing when the thing is a shape** (`UI_DESIGN.md:737`): a filter
  is its response, a drive is its transfer curve, a delay is its echoes. A
  device that has no shape (Preamp, Aux In, Buffer) gets a clear object
  instead.

## Wiring

- `DeviceHeader` gains `in property <string> icon`, drawn by `Icon` where the
  bar and dot were, and loses the bar and dot.
  - The source header binds `Icons.source-kinds[root.source-kind]`.
  - Each effect row binds `Icons.effect-kinds[<the row's kind number>]`, or
    `Icons.plugin` for a plugin slot. The row already carries its kind for the
    faces, so use that. Don't add a field to `EffectSlotRow`, which Effects
    owns.
- `CollapsedDevice` takes the same `icon`, in place of its 8×8 dot.
- **Tint:** the device's colour, which today means sources in `Theme.accent`
  and effects in `Theme.warning`, and `Theme.text-faint` while bypassed or
  disabled. The icon still does the old chip's job: it tells source from
  effect, and on from off.
- The mockup catalog gets a **kind sheet**: every kind's header at 1× and 2×,
  one bypassed.

## Adam sees the sheet before it lands

This is a visual change shipping in 0.1.6. When the sheet is drawn, post it on
MOO-273 with the `Question` label: which drawings to redraw, and why. Carry on
wiring while he looks, and land once he has answered.

## Done when

- No header shows the bar and dot, and no folded strip shows the dot.
- Every kind, the retired two included, has a distinct icon from the
  registry.
- `icon_registry.rs` holds both kind tables to the Rust tables, with no empty
  entry.
- Adam has seen the sheet.
- Rung 4 on antibox with `--locked --no-fail-fast`, and the snapshots that
  show a header are updated and looked at.

Owner: **Interface** (the header, the strip and the drawings). Effects reviews
the Chain and Layer drawings, since the container drawing is theirs.
Milestone: **A device shows what it is.**
