# 03 — the theme file

What a theme is on disk, how it loads, and what the Appearance page does with
it. Depends on step 01. Does not depend on step 02: a theme file can carry a
`relief` field that nothing reads yet, and adding the reader later is a no-op.

## The format

TOML, beside `settings.toml`, in `<config>/mooloop/themes/<name>.toml`. One
theme per file, so a theme is a thing you can send somebody.

```toml
schema-version = 1
name = "Platinum"
author = "Adam"
description = "An homage to Mac OS 8, not a reproduction of it."

[color]
base = "#dddddd"
accent = "#3366cc"
alert = "#cc7700"
contrast = 1.15

[shape]
roundness = 0.0
relief = "bevel"        # flat | bevel | inset
relief-depth = 1.0
hairline = 1             # px
stroke-emphasis = 2

[type]
family = "Charcoal, Geneva, sans-serif"
family-mono = "Monaco, monospace"
scale = 1.0
weight = 400

[metrics]
density = 1.0

[motion]
speed = "Fast"           # the existing names
easing = "Ease out"
```

Every section and every field is optional and falls back to the built-in
default, for the reason `settings.rs` already uses `#[serde(default)]`
everywhere: a theme written against schema 1 must still load against schema 2,
and a theme that only wants to change three colors should be nine lines.

## Fonts: the constraint, written into the format

Slint 1.17.1 has **no runtime font registration**. `register_font_from_path`
is not in the public API; the only runtime hook is the unstable
`fontique_010` module. A font is either embedded at build time by
`import "./x.ttf"` in a `.slint` file, or resolved by name from the system.

So `family` is a **list**, CSS-style, and mooloop walks it: each name is tried
against the families compiled in, then against the system, and the first hit
wins. If none does, the platform default is used and **the Appearance page
says which name was resolved**, because a theme silently falling back is how a
user concludes the theme is broken.

This also means: **mooloop should compile in one pixel font**, so that the
tracker homage works on a machine that has nothing installed. One 8px bitmap
TTF, embedded, named in the built-in theme. That is a build-time decision
belonging to this step, not something a theme can do for itself.

## Loading

`ThemeFile` in `settings.rs`, deserialized by the same machinery as
`UiSettings`, and resolved into the existing structures:

- `[color]` fills `AppearanceSettings`'s `base` / `accent` / `alert` /
  `contrast`, which already exist and already feed `derive_palette`.
- everything else fills a new `ThemeShape` / `ThemeType` / `ThemeMetrics`
  that `apply_appearance` pushes into `Theme` beside the palette.

`apply_theme` (`lib.rs:151`) is where the palette already lands and where the
rest lands too. `apply_appearance` (`lib.rs:176`) stays the one funnel for
live preview and Apply both -- it is documented as such and that property is
what makes the Appearance page's dragging honest.

**Note the existing motion carve-out.** `apply_appearance` deliberately does
not apply motion, because it runs on every live color preview and would
clobber in-progress edits. Type, shape and metrics have the same problem the
moment the Appearance page can edit them, and the same answer: they go through
`sync_preferences_properties` on startup, cancel and Apply.

## What happens to `ThemeScheme`

`ThemeScheme` is `{ name, base, accent, alert }` and `BUILTIN_SCHEMES` is six
of them. A theme is a superset, so the honest move is:

**A scheme becomes a theme with only a `[color]` section.** The six built-ins
become six built-in themes; `AppearanceSettings::user_schemes` migrates to
user themes; `ThemeScheme::is_builtin` becomes `ThemeFile::is_builtin`. One
concept, not two, and the migration is mechanical because a scheme's four
fields are four of a theme's fields.

The alternative -- schemes and themes side by side, a scheme being "just the
colors" -- was considered and is worse: it puts two lists on the Appearance
page and makes a user decide which kind of thing they are saving.

`schema_version` on `UiSettings` bumps, and the loader maps a v-N
`user_schemes` array into theme files on first run. `docs/PROJECT_FORMAT.md`
does not cover settings, so this is `settings.rs`'s own versioning; state the
migration in `00-status.md` when it lands.

## The Appearance page

It already has a scheme list with swatches, save, and delete
(`appearance-dialog.slint`, `scheme_rows` in `lib.rs:207`). It grows:

- the list shows themes, and a built-in theme is still undeletable;
- the three color pickers stay exactly as they are -- **editing a theme's
  colors is the common case and must not get further away**;
- shape, type and metrics go behind a disclosure, because a user who wants a
  greener accent should not have to walk past a font stack;
- a theme edited away from its file is marked as such, the way `scheme` is
  already emptied when the colors are edited;
- `AppearanceSchemeRow` gains whatever the row needs to show that a theme
  carries more than colors. A swatch triple does not say "this one also
  changes the font".

## Acceptance

- A theme file with only `[color]` behaves exactly as a scheme does today.
- A theme naming a font nobody has loads, renders, and reports the fallback.
- Deleting the themes directory leaves the six built-ins and a working app.
- A malformed theme is skipped with a message and does not stop startup --
  `load_or_default` already sets this precedent for `settings.toml`.
- Round trip: save a theme, quit, relaunch, and the interface is identical.
