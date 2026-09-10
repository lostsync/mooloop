# 01 — finish the token surface

Type, stroke and metrics become tokens on `Theme`, the way color and radius
already are. No visual change: every token's default is the literal it
replaces, and the acceptance test is that the snapshots do not move.

This is the step that gets more expensive with every device added, which is
why it is first.

## What gets added

To `ui/theme.slint`, beside the existing color and radius blocks.

```slint
export global Theme {
    // ... colors, roundness, radius-* as today ...

    // Type. `family` is a family name resolved at load; empty means the
    // platform default. `mono` is the family for anything that must not
    // reflow as a value changes -- readouts, the tempo field, hex.
    in-out property <string> font-family: "";
    in-out property <string> font-family-mono: "monospace";
    // Multiplies the whole scale. The accessibility control (step 04).
    in-out property <float> type-scale: 1.0;
    // Weight for body text; a theme that wants a heavier UI moves this once.
    in-out property <int> font-weight: 400;

    out property <length> text-xs:  7px * root.type-scale;
    out property <length> text-sm:  8px * root.type-scale;
    out property <length> text-md:  9px * root.type-scale;
    out property <length> text-lg: 10px * root.type-scale;
    out property <length> text-xl: 11px * root.type-scale;
    // Above the working range: headings, the transport readout, dialogs.
    out property <length> text-2xl: 13px * root.type-scale;
    out property <length> text-3xl: 16px * root.type-scale;
    out property <length> text-4xl: 22px * root.type-scale;

    // Stroke. A flat theme sets hairline to 0 and gets a borderless UI
    // without a face knowing that is what it asked for.
    in-out property <length> hairline: 1px;
    in-out property <length> stroke-emphasis: 2px;

    // Metrics. `ToolbarMetrics` in toolbar.slint is the precedent and folds
    // into these; the toolbar keeps its own global as a thin alias so the
    // step does not have to touch toolbar.slint's every call site.
    in-out property <float> density: 1.0;
    out property <length> control-height: 24px * root.density;
    out property <length> control-min-width: 44px * root.density;
    out property <length> pad-xs: 2px * root.density;
    out property <length> pad-sm: 4px * root.density;
    out property <length> pad-md: 6px * root.density;
    out property <length> pad-lg: 8px * root.density;
}
```

Eight type steps rather than five because the sweep has to land the 22 sizes
outside the 7-11px working range somewhere, and rounding them into `text-xl`
would visibly change three dialogs and the transport.

## The sweep

**Type: 313 sites.** 291 of them are one of five values and map mechanically:

| Literal | Token | Count |
| --- | --- | --- |
| `7px` | `Theme.text-xs` | 49 |
| `8px` | `Theme.text-sm` | 67 |
| `9px` | `Theme.text-md` | 93 |
| `10px` | `Theme.text-lg` | 44 |
| `11px` | `Theme.text-xl` | 38 |
| `12px`, `13px` | `Theme.text-2xl` | 7 |
| `14px`, `16px` | `Theme.text-3xl` | 12 |
| `20px`, `22px` | `Theme.text-4xl` | 3 |

The last three rows are the ones that need a look rather than a `sed`: 12px
and 13px are two sizes doing one job, and 14px/16px and 20px/22px are the same
story. Collapsing each pair is a deliberate one-pixel change in a handful of
places, and it is worth making rather than carrying eleven type sizes into a
themeable surface. **Record which sites moved**, because those are the only
places a snapshot may legitimately differ.

`min(9px, root.piano-row-height - 1px)` in `piano-grid.slint` stays a
computation and takes `Theme.text-md` as its ceiling.

**Family: 16 sites.** Every one is `font-family: "monospace"` and becomes
`Theme.font-family-mono`. Then set `default-font-family: Theme.font-family`
and `default-font-size: Theme.text-md` on the `Window` in `main.slint`: Slint
resolves an element's `font-*` against the window's `default-font-*`, so this
is what makes a theme's body font reach the text that never names a family.

**Stroke: 81 sites** of `border-width: 1px` to `Theme.hairline`, plus the two
`2px` to `Theme.stroke-emphasis`. The conditional ones
(`x ? 1px : 0px`) become `x ? Theme.hairline : 0px` -- the condition is
behaviour and stays.

**Metrics: by hand, and only where it is honest.** A `24px` that is a control
height becomes `Theme.control-height`; a `24px` that is the width of a
particular graphic stays a number. This is the part of the step that cannot be
scripted and the part where a wrong call is invisible until a theme with
`density: 1.3` is loaded. When in doubt, leave the literal -- an untokenized
metric is a theme that has less effect, and a wrongly tokenized one is a
layout that breaks at a setting nobody tested.

**The remaining 12 stray hex colors**, in `main.slint`, `controls.slint`, and
the four dialogs, take the palette token nearest what they are doing.

## The three faces that keep their literals

Named so the sweep does not argue with itself:

- **`ds01-device.slint` (31 hex, 8 font sizes)** and **`eq-device.slint`
  (7 hex)** draw instrument graphics. Their *chrome* takes tokens; their
  drawing does not.
- **`device-concepts.slint` (11 hex)** and the mockup files are not shipped
  interface.
- **`appearance-dialog.slint` (37 hex)** is the page that shows colors. A
  swatch of `#84cc16` is content.

## Verification

`scripts/slint-sketch crates/mooloop-ui/ui/main.slint --shot` after each file,
which is 3s against four minutes for a build. Then the snapshot suite on the
box, once, at the end.

**The snapshots are the acceptance test and they are supposed to be
unchanged.** Any diff is either a site from the collapsed-size list above or a
mistake, and the list is short enough to check every one. Do not accept a
batch of regenerated snapshots on the argument that a token refactor changes
rendering -- it does not, because every default is the literal it replaced.

## Why no `Theme.font-family` on every Text

Because `default-font-family` on the Window already does it, and putting the
token on 300 `Text` elements would be 300 places for a theme to be
inconsistent. A face names a family only when it needs the *mono* one, and
that is a statement about the content -- a value that must not reflow -- rather
than about the theme.
