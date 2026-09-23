# Writing a theme

A theme is one TOML file in `<config>/mooloop/themes/`, which on Linux is
`~/.config/mooloop/themes/`. One theme per file, so a theme is a thing you can
send somebody.

Nothing in the file is required. Every section and every field falls back to
whatever the interface is already set to, so **a theme that only changes the
font is four lines** and a theme written against this schema will still load
against the next one. A file that will not parse is skipped with a message on
the console; it does not stop mooloop starting.

The Appearance page writes these files too: set the colours up the way you
want them, type a name, press **Save Theme**. Everything below is what that
produces and how to edit it by hand afterwards.

## The shape

```toml
schema-version = 1
name = "Platinum"
description = "An homage to Mac OS 8, not a reproduction of it."

[color]
contrast = 1.15

[color.dark]
accent = "#BD93F9"
slots = [
  "#282A36", "#313341", "#44475A", "#6272A4", "#A3ACC7", "#F8F8F2",
  "#FBFBF7", "#FFFFFF", "#FF5555", "#FFB86C", "#F1FA8C", "#50FA7B",
  "#8BE9FD", "#BD93F9", "#FF79C6", "#B48759",
]

[color.light]
base = "#FFFBEB"
accent = "#644AC9"
alert = "#846E15"

[shape]
roundness = 0.0
hairline = 1
stroke-emphasis = 2
relief = "bevel"
relief-depth = 1.0

[type]
family = "Charcoal, Geneva, sans-serif"
family-mono = "Monaco, monospace"
scale = 1.0
weight = 400

[metrics]
density = 1.0
```

## Colours

A variant is stated in one of two forms and you can mix them between the two
variants of one theme, as above.

**Sixteen slots** is the base16 palette, in base16's order. If you already have
a base16 scheme, this is a copy and paste:

| Slot | base16 calls it | mooloop draws |
| --- | --- | --- |
| 00 | default background | the window background, and the panel a step below it |
| 01 | lighter background | surfaces |
| 02 | selection background | raised surfaces, and the active surface just above |
| 03 | comments, invisibles | faint text, and the border just below |
| 04 | dark foreground | muted text |
| 05 | default foreground | text |
| 06, 07 | light foreground and background | unused directly; they are where a derived light variant starts |
| 08 | red | destructive actions, and a clipping meter |
| 09 | orange | the swatch palette |
| 0A | yellow | warnings, meter headroom |
| 0B–0E | green, cyan, blue, magenta | the swatch palette; 0D is the accent unless you name one |
| 0F | brown | the swatch palette |

`accent` is optional and is the colour the scheme is *known by* — Nord's frost
cyan, Dracula's purple, Monokai's green. It drives selection, focus and the
safe band of every meter. Without it, slot 0D is used.

**Three seeds** is `base`, `accent` and `alert`, the same three the Appearance
page's colour pickers write. mooloop grows a full ramp from them, so a seed
variant is a real variant and not a lesser one: it gets a swatch palette, a
light counterpart and everything else. Use it when you know the three colours
you want and do not care which green the channel picker offers.

`contrast` sits under `[color]` rather than under a variant, because it scales
the distance between the neutrals rather than choosing any of them. 1.0 is the
ramp as you wrote it; 0.6 tightens the hierarchy and 1.4 opens it.

### Light and dark

Write both variants if the scheme publishes both. mooloop derives the missing
one when you do not — the neutral ladder reverses and the hues have their
lightness retargeted, holding hue and saturation — and the Appearance page
marks that row **derived**, because a derived Snow Storm is not Snow Storm.

Which variant is worn is the user's choice, not the theme's: the Dark / Light /
Auto control sits above the theme list, and **Auto** follows the desktop's own
`org.freedesktop.appearance color-scheme` setting.

### Two things worth checking

**Text on surface should clear 4.5:1**, and **the accent against surface should
clear 3:1**. Every theme that ships is held to both by a test. Nothing checks a
theme you wrote, so check them yourself; an unreadable scheme is the one bug
here that a screenshot will not show you.

**Slot 0A must not be your accent.** The meters draw safe in the accent,
headroom in slot 0A and a clip in slot 08. Two of those being one colour is a
two-colour meter.

## Type

`family` and `family-mono` are CSS-style lists: `"Iosevka Aile, Inter,
sans-serif"`. mooloop hands the whole string to the renderer, which walks it.

**A theme names a font; it cannot ship one.** Slint 1.17.1 has no runtime font
registration, so a family is resolved from what is installed on the machine and
a name nobody has falls back to the platform default without a word. If your
theme depends on a face, say so in `description`.

`scale` multiplies the whole type scale. The interface's working range is
7–11px, so 1.5 is 10–17px and is a different program to sit in front of for an
evening. `weight` is a CSS weight; anything above 500 at 9px is a smudge.

## Shape and metrics

`roundness` multiplies every corner radius — 0 gives square corners
throughout. `hairline` and `stroke-emphasis` are the two stroke widths in
pixels, and `hairline = 0` gives a borderless interface without any face
knowing that is what it asked for.

`relief` is how a surface is drawn: `flat` (a fill and a hairline, mooloop's
own look), `bevel` (a lit block: light top and left edges, dark bottom and
right, swapped while pressed or latched) or `inset` (the same block sunk).
The edges are derived from the surface's own fill, so a theme names a relief
and a `relief-depth` (0 to 2, default 1) and never a bevel colour. A bevel is
square, so a theme with one sets `roundness = 0`; the Appearance page warns
rather than refuses when the two disagree.

**Relief belongs to the theme; the other shape values do not.** Selecting a
theme that states no `relief` goes back to flat, where a theme that states no
`roundness` leaves the reader's roundness alone. The built-in homages,
Platinum and Impulse, carry one. So far `ToolButton`, and everything built on
it, draws the bevel; the rest of the controls are still flat
(`docs/plans/theming/00-status.md`).

`density` multiplies control heights and the padding-and-spacing ramp. It is
the control to reach for if the interface is too tight to hit rather than too
small to read; `scale` is the other one.

## Wallpaper colours

If pywal or wallust has run, a **Wallpaper** row appears in the theme list and
is whatever is in the cache. mooloop reads these, newest first:

```
~/.cache/wal/colors.json        pywal, and pywal16
~/.cache/wallust/*.json         wallust
~/.cache/wal/colors             sixteen lines of #RRGGBB
~/.cache/wallust/colors         the same, from a wallust template
```

It reads the cache; it does not run the generator. Re-run `wal` or `wallust`
and re-open Preferences to pick up a new wallpaper.

A terminal palette is not base16 — ANSI's second eight colours are brightness
variants rather than eight more hues, and it has no slot for a comment colour,
an orange or a brown. So the neutral ladder is interpolated between the
background and the foreground, ANSI's bright black is taken as the comment
colour when it genuinely sits between them, and orange and brown are mixed from
red and yellow. The accent is blue if blue is legible against the background,
and otherwise whichever colour is.

## The swatch palette

The colours offered for a channel, a track or a pattern follow the theme.
Eleven of them: the ramp's eight hues in hue order, plus a midpoint in each of
its three widest gaps around the wheel. That is adaptive — a scheme with four
blues and no green gets its three extra colours somewhere useful — and it keeps
the rule the hand-picked eleven were chosen under, which is that **a channel
colour is an identifier and two greens that agree at 20px are one colour with
extra steps**.

You do not have to think about it. It is worth knowing that **a song stores the
colour it was given, not a palette index**, so changing themes never repaints
anybody's channels — and that a channel coloured from Nord will keep looking
like Nord after you switch to Gruvbox, which is correct and occasionally
surprising.

## What a theme cannot do

- **Ship a font.** See above.
- **Redraw a control.** Slint has no cascade: you change what a component
  reads, and `Theme` is what it reads. `relief` is the one drawing a theme
  can choose, and only the controls that have adopted `Bevel` draw it.
- **Decorate the window.** Slint does not draw the title bar and neither does
  this.
- **Reskin a device.** The DS-01 and the EQ draw their own instrument
  graphics. They take the palette for their chrome and keep their own drawing.
