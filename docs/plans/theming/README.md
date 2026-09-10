# Theming

Adam, 2026-09-09, on whether the appearance settings could go further than
colors:

> could we have one that resembled like...idk, macos classic? schism/impulse?
> not just colors but fonts and shapes, borders etc. is it just a matter of
> stylesheet? ... it'd be cool but that's about all it'd bring to the table.
> maybe accessibility...which i hadnt really thought about at all.

This directory is the answer to both halves: what mooloop would need so that a
theme is a file rather than a patch, and how somebody then writes one. It is
**queued, not started**, and it is written now rather than later for the
reason Adam gave -- the work is a token sweep across every face in the
program, and every device added before it is another face to sweep.

## It is not a stylesheet, and it does not need to be

Slint has no cascade. You cannot restyle a component from outside it; you
change the component or you change what it reads. What it has instead is
globals, and `ui/theme.slint`'s `Theme` **already is the stylesheet** -- one
object, set once from Rust, read by every face. The question is not how to
introduce a styling layer. It is which axes that object is missing.

## The survey

Run against `feat/sends` at `fced7d9`, over `crates/mooloop-ui/ui/*.slint`.

**Color is done.** 18 tokens on `Theme`, all derived in `settings::derive_palette`
from three seeds plus a contrast scalar. 116 literal hex colors remain and
none of them is a face styling itself: 37 are the Appearance page's own
swatches, 18 are `theme.slint`'s fallback defaults, 31 + 7 are the DS-01 and
EQ graphics, 11 are `device-concepts.slint`, and the last 12 are scattered
one-offs worth folding in during the sweep.

**Radius is done.** `Theme.radius-{xs,sm,md,lg}` off one `roundness` float:
180 uses against 29 literals, and 16 of those literals are `something / 2`
pill and circle geometry that tracks its own bounds and *should* stay local.
13 literal pixel radii is the whole debt.

**Motion is done.** `Motion.duration` and `.curve`, with Instant as a real
setting.

**Type is untouched.** 313 literal `font-size:` values and 6 that read a
token. 16 literal `font-family: "monospace"`. There is no type token at all.

**Stroke is untouched.** 81 literal `border-width: 1px`.

**Metrics are half-started.** `toolbar.slint:52` has a `ToolbarMetrics` global
with `control-height`, `label-size` and `value-size` -- exactly the right
idea, scoped to one bar. Everywhere else, `min-height: 24px` and every padding
is a literal.

The control library is the leverage: `controls.slint` exports 28 components
and they are what the faces are built from. The long tail is inline chrome --
94 anonymous `Rectangle`s in `main.slint`, 48 in `modulation-shelf.slint`, 45
in `device-displays.slint` -- which is drawn by hand and picks up a token only
when somebody puts one in it.

## The four decisions

**1. A theme is `Theme` plus three new globals, not a new mechanism.** Type,
stroke and metrics join color, radius and motion as things set once from Rust
and read by name. Nothing about how a face is styled changes; the vocabulary
gets wider.

**2. Relief is a component, not a property.** A Slint `Rectangle` has one
border color, so "light on the top-left, dark on the bottom-right" cannot be
expressed as a token -- and a bevel is what macOS Classic and Impulse Tracker
*are*. So the relief token selects between drawings, and the drawing lives in
one shared `Surface` component the controls inherit from. This is the only
part of the work that is design rather than sweeping, and it is why step 02
exists on its own.

**3. A theme cannot ship a font, and the format must not pretend otherwise.**
Slint 1.17.1 has no runtime font registration -- `slint::register_font_from_path`
does not exist in its public API, and the only runtime hook is the unstable
`fontique_010` module. A font is embedded at compile time by `import "./x.ttf"`
in a `.slint` file, or it is resolved by name from the system. So a theme file
names a family and mooloop resolves it: first against the families it has
compiled in, then against the system, then a documented fallback. A theme that
asks for a font nobody has still loads and still looks deliberate.

**4. Accessibility is the reason, and homage is the payoff.** See below. It is
stated in that order because it decides what gets built when the work has to
be cut short.

## What accessibility actually gets, and what it does not

Adam had not thought about it, so this is written honestly rather than
persuasively.

The interface's working type size is 7-11px -- 291 of 313 literal sizes are in
those five buckets. That is small, it is not adjustable, and it is the single
largest accessibility problem in the program. **Type tokens fix it and nothing
else does.** A `type-scale` multiplier on a finished token surface turns the
whole interface up together; without one, the fix is 313 edits every time.

The rest, in descending order of what it is worth:

- **Contrast is derivable and currently unchecked.** `derive_palette` already
  computes `relative_luminance`. Nothing asserts that `text` on `surface`
  clears a ratio, so a user can pick seeds that produce an unreadable
  interface and the Appearance page will show it to them without comment. A
  WCAG check on the derived ramp is cheap, and a high-contrast built-in theme
  is then just a preset that passes it.
- **Reduced motion already exists** and is not labelled as an accessibility
  setting. `Motion.speed` has an Instant option. It costs a word.
- **Hit targets are mostly honest** -- `ToolButton` is `min-width: 44px`,
  `min-height: 24px` -- but the knobs and the mini-knobs are not, and a metrics
  token is what would let them grow without a per-face edit.
- **Themes do not fix screen readers.** `accessible-role` and
  `accessible-label` are set on the controls that have them and absent on the
  inline chrome, and no amount of theming touches that. Named here so it is
  not quietly counted as a benefit of this plan. It is its own work.

## Scope: what "within reason" means

**In:** type, stroke and metric tokens; a relief primitive; a theme file
format; the built-in themes; the Appearance page reading them; a contrast
check; two homage themes as the proof.

**Out, deliberately:**

- **Pixel-identical anything.** Impulse Tracker's ANSI box-drawing and the
  Platinum title-bar stripes are per-widget artwork, not tokens. The target is
  an homage that reads as the thing across a room, which the tokens reach.
- **Window chrome.** Slint does not decorate the window and this plan does not
  make it.
- **Per-device skins.** The DS-01 and the EQ draw their own graphics. They
  take the palette and they keep their own drawing.
- **A theme marketplace, hot reload, or scripting.** A theme is a TOML file
  next to `settings.toml`, read at startup and on apply.

## The steps

| | | |
| --- | --- | --- |
| `01-finish-the-token-surface.md` | Type, stroke and metrics become tokens. The sweep. | The bulk of the hours, none of the risk |
| `02-relief.md` | `Theme.relief` and a shared `Surface`. Bevels. | The only design problem here |
| `03-the-theme-file.md` | What a theme is on disk, how it loads, what the Appearance page does with it | Depends on 01, not on 02 |
| `04-accessibility.md` | The contrast check, the type scale, the labelling | Depends on 01 and 03 |
| `05-authoring-a-theme.md` | The guide, with two worked homages | Last, because it is the acceptance test for all of it |

01 and 03 are worth having even if 02 is never built: they are what turn "the
Appearance page has three color pickers" into "the appearance of the program
is data". 02 is what makes an homage look like an homage rather than like
mooloop wearing grey.
