# theming — status

Unparked 2026-09-15 by Adam, with a brief that is wider than the plan this
directory was written to:

> work on theming. this is linux-first so it needs to rice nice. allow it to
> pick up wallust/pywal colors from their caches. include common colorschemes,
> e.g. dracula, nord, etc. track/pattern/channel color palette should follow
> colorscheme. font options, padding/spacing if it makes sense, etc. make them
> saveable. each theme should have light/dark variant and there should be an
> option for light/dark/os.

That brief **answers the open design question** `ENHANCEMENTS.md` had been
holding: whether a named scheme like Nord is three seeds or a full sixteen-
colour ramp. pywal, wallust and base16 all hand over a ramp, so the seed model
grows a second form rather than being asked to approximate one.

## What has landed

### 01 — the token surface — **landed**

`ui/theme.slint` carries type, stroke and metric tokens beside the colour and
radius blocks it already had, and the sweep put the literals on them.

| | Sites | Now |
| --- | --- | --- |
| `font-size: Npx` | 303 | `Theme.text-xs` … `text-4xl` |
| `font-family: "monospace"` | 15 | `Theme.font-family-mono` |
| `border-width: 1px` | 96 | `Theme.hairline` |
| `border-width: 2px` | 10 | `Theme.stroke-emphasis` |

`MainWindow` binds `default-font-family`, `default-font-size` and
`default-font-weight` to the tokens, which is what makes a theme's font reach
the several hundred `Text` elements that never name a family.
`ToolbarMetrics` stayed, as the plan intended, but its three values are now
aliases of `Theme.control-height`, `Theme.text-md` and `Theme.text-2xl`.

**The ten deliberate one-pixel moves**, which are the only places a render may
legitimately differ, are the collapse of eleven type sizes into eight steps:

- `appearance-dialog.slint` — seven page headings, 14px → 16px
- `about-dialog.slint:24` — the wordmark, 20px → 22px
- `modulation-shelf.slint:795` — 12px → 13px
- `toolbar.slint` — `ToolbarMetrics.value-size`, 12px → 13px, which is the
  transport readout and every toolbar value that reads it

**Two departures from `01-finish-the-token-surface.md`, both deliberate.**
The step exempted `ds01-device.slint` and `eq-device.slint` from the sweep;
only their *hex colours* are exempt here, because a `type-scale` that skips
two device faces is an accessibility control that does not work. And the
padding ramp is one scale used for padding and spacing both, rather than two:
at 2-8px the distance between a control and its neighbour and the distance
between a control and its own edge are the same decision.

`device-concepts.slint` and the mockup files are untouched — they are not
shipped interface.

## What is still ahead

`02-relief.md` is unchanged and still the only design problem here. `03`, `04`
and `05` are being taken against the wider brief above rather than as written:
a theme is a ramp with two variants, not three seeds.
