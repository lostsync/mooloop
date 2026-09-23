# theming — status

Linear: project [Theming](https://linear.app/mooloop/project/theming-beb2299b6232).
What is left is MOO-153 (02), MOO-154 (the rest of 04), MOO-155 (05's
homages) and MOO-157 (the padding literals). MOO-156 (following the
desktop live) is done.

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

**A literal came back within a day, and it is worth knowing how.** Rebasing
this branch onto `musical-time/`'s merge turned up a fresh
`font-family: "monospace"` in `controls.slint` — `BbtText`, extracted from the
transport readout while this sweep was in flight, carried the literal with it
rather than the token. Nothing could have caught that: the sweep is a one-time
pass over what exists, and the two branches were green separately and green
together. `scripts/dupe-audit` does not look for this shape either, because a
family name is not a *duplicated* value, it is an untokenized one. The habit
that catches it is the one the rebase forced: after merging, grep the axis you
just closed.

### 03 — the theme file — **landed, widened**

Taken against the brief above rather than as written, which means one thing:
**a theme is a sixteen-colour ramp with two variants**, not three seeds. The
rest of `03-the-theme-file.md` stands and was followed -- TOML beside
`settings.toml`, every field optional, a malformed file skipped with a message,
a theme that names a font nobody has still loading.

The new Rust lives in `crates/mooloop-ui/src/theme/`:

| | |
| --- | --- |
| `color.rs` | `Rgb`, `Hsl`, mixing, shading, WCAG contrast. Moved out of `settings.rs`, which is no longer the only thing that needs them. |
| `ramp.rs` | The sixteen slots, the palette derivation, the swatch palette, the light/dark derivation, and `from_seeds`/`from_ansi`. |
| `builtins.rs` | The schemes, as data. |
| `wal.rs` | pywal and wallust cache reading. |
| `system.rs` | The desktop's own light/dark setting. |
| `file.rs` | The TOML format. |
| `catalog.rs` | Built-ins + user files + the wallpaper row, cached. |

**The seed form did not become a legacy path.** It synthesizes a ramp, so
there is one derivation downstream rather than two, and the synthesis is tuned
so that the palette it produces matches the one the three seeds produced
before -- **to within one byte per channel**, at every contrast setting, on a
light base as well as a dark one.
`a_seed_ramp_reproduces_the_palette_the_seeds_used_to_produce` holds a copy of
the old arithmetic and compares against it, because comparing the new code
against itself would pass whatever it did.

That tolerance is the one place this is not a pure widening, and it is worth
knowing why rather than waving at. The old derivation went from the seed to
the contrast pole in one interpolation; this one stops at a ramp slot on the
way, and a slot is eight bits per channel. One byte is what the extra rounding
costs. It was measured before the tolerance was picked -- five schemes, eight
contrast settings, twelve tokens, worst case one -- rather than chosen and
then justified.

**A first attempt at it was wrong in a way the tolerance would have hidden.**
Applying contrast as a *clamped* blend from the background toward the slot
reproduced the old palette below 1.0 and quietly broke it above: at contrast
1.4, text used to reach `#FFFFFF` and stopped at the colour the theme had
authored, so the widen-the-hierarchy control did nothing at its own maximum.
`Ramp::palette` uses `color::extend`, which lets the factor past 1 and clamps
per channel at the end; `contrast_at_its_maximum_still_reaches_the_pole` is
the test that says so. The numbers were run against a model of the arithmetic
before the build, which is how it was caught at all.

**Thirteen built-in themes**, most with both published variants: Mooloop,
Dracula/Alucard, Nord/Snow Storm, Gruvbox, Everforest, Solarized, Catppuccin
Mocha/Latte, Tokyo Night/Day, Rosé Pine/Dawn, Monokai, plus the four seed
schemes that were already there. A fourteenth row, **Wallpaper**, appears when
pywal or wallust has cached a palette.

**Two migrations**, both in `settings.rs` and both one-way:

- `scheme` is read as `theme` through a serde alias, and written as `theme`.
- `Daylight` was light-mode Mooloop before a theme had two variants, so it
  becomes exactly that -- unless the user has since saved a theme under the
  name, in which case theirs wins.
- `user-schemes` is written out as theme files on first load and the array is
  cleared. **Only if every write succeeded**: a read-only home must not cost
  somebody their saved schemes, so a failed migration leaves the array alone
  and the next launch tries again.

`SCHEMA_VERSION` deliberately did **not** bump. `UiSettings::load_from` rejects
any version it does not recognise and `load_or_default` then discards the whole
file, so bumping would have wiped every existing configuration -- every new
field is `#[serde(default)]` instead, which is what the format was already
shaped for.

### 04 — accessibility — **landed in part**

- **`type-scale` is on the Appearance page**, 75% to 200% over the eight type
  steps. This was the whole reason the plan was written down as accessibility
  rather than homage, and it is the half that could not exist before step 01.
- **`density`** likewise, over control heights and the padding ramp: the
  answer when the interface is too tight to hit rather than too small to read.
- **Contrast is checked in two places.** `builtins.rs` fails if any variant of
  any built-in has body text under 4.5:1 or an accent under 3:1 -- including
  the *derived* variants, because a theme that only authored a dark ramp still
  answers Light and the answer has to be readable or the mode switch is a
  trap. `legible_against` is what makes that hold: mooloop's own lime is 2.1:1
  on a near-white background, so a Mooloop flipped to light without it would
  ship an accent the program's own validator rejects.

  And the **Appearance page reports both ratios live**, amber below the bar,
  which is the half the plan actually asked for: a user could pick seeds that
  produced an unreadable interface and the page would show it to them without
  comment. The arithmetic had been sitting in `relative_luminance` since the
  palette was first derived and nothing ever called it.
- **Reduced motion is labelled** (MOO-154): the Motion speed row says
  "Instant is reduced motion: nothing animates". Instant is already the
  default, so a first run animates nothing and there is no platform setting
  to follow.
- **Not done:** screen-reader coverage is untouched. The plan already says the
  second is its own work and no amount of theming touches it.

### 02 — relief — **the drawing landed; the look waits on Adam** (MOO-153)

`Theme.relief` (0 flat, 1 bevel, 2 inset) and `Theme.relief-depth`, a
`Bevel` component in `controls.slint`, and `ToolButton` as the first adopter,
which brings `ToggleButton`, `SegmentedControl`, the pane tabs and the mute
buttons with it. A theme file's `[shape]` takes `relief` and `relief-depth`;
the Appearance page has a Relief selector and a Depth fader, and says when a
bevel is sitting on rounded corners rather than refusing it.

Three things the doing decided:

- **The edges are mixed toward white and black, not `brighter()`/`darker()`.**
  Those scale lightness, so on mooloop's near-black surfaces they gave edges
  within a few levels of the fill: a bevel that vanished on every dark theme
  shipped. Measured on the sketch before choosing: top edge 106 on a fill of
  42, bottom 21.
- **Relief belongs to the theme.** Every other shape scalar is left alone by a
  theme that doesn't state it, which keeps a type scale somebody set for their
  eyes. A bevel is the look, not the reader's, so a theme that states no
  relief goes back to flat. Without that, Nord drew Platinum's slabs.
- **A latched button reads as pushed in**, the way a Platinum toggle stays
  down, so `active` inverts the bevel as a press does.

**Stopped here on purpose, per the step's own order:** "Do `ToolButton` alone,
sketch it, and look at it before touching the other 27." The step's risk is
whether a derived 1px bevel looks right at mooloop's sizes, and that is Adam's
eye, not a pass. MOO-153 carries the question. The remaining adopters are the
knobs' caps, the device header, the rack row, the panel and pane edges and
the dock.

### 05 — the homages — **landed as built-ins** (MOO-155)

Platinum and Impulse are built-in themes now, not only worked examples. Both
are authored ramps rather than the step's seeds-plus-contrast: the step's
Impulse asked for a contrast of 2.2 to push grey boxes off a black field, and
the page stops at 1.4, where a ramp just states the boxes. Both set
`roundness = 0` and a bevel (Impulse deeper, at 1.4). Impulse names only a
monospaced *readout* face: a monospaced face over every label is a layout
change nobody has checked at every width. Neither can set motion, which a
theme doesn't carry. `the_homages_are_square_and_bevelled_and_nothing_else_is`
holds both, and every existing contrast test covers them and their derived
variants.

### What the build found that the reasoning did not

Three defects in this work were caught by modelling the arithmetic in Python
before compiling it, and two by the suite. Worth keeping because they are the
same shape -- **a rule that was right about the case in front of it and wrong
one step out**:

- **A clamped contrast blend** reproduced the old palette below 1.0 and broke
  it above: at 1.4 text stopped at the colour the theme authored instead of
  reaching white, so the widen-the-hierarchy control did nothing at its own
  maximum. `color::extend` lets the factor past 1.
- **Five of the twenty-six shipped variants** failed the contrast bar they are
  now held to. Three published accents are darkened a step with the reason
  recorded beside them, and the derivation holds an accent to 3:1 against the
  *surface* rather than just retargeting its lightness.
- **`r#"..."#` ends at the first `"#`**, and every `"#RRGGBB"` in a JSON or
  TOML fixture is one. This is a *lexer* error, so two `#[cfg(test)]` fixtures
  took the non-test build down with them.
- **The wallpaper scanner paired a container key with the first key inside
  it.** `"special": { "background": ... }` became `("special", "background")`
  and shifted every pair after it, which lost `color0` and the background --
  and then produced sixteen plausible colours anyway through the positional
  fallback. A scanner that is wrong this quietly needs a test that names the
  shape rather than one that checks the output looks reasonable.
- **ANSI's bright black is not always the comment colour.** Taking it whenever
  it sat between background and foreground put slot 03 *lighter* than slot 04
  on one real palette: faint text drawn brighter than muted text. It is taken
  only when it sits between the two slots it would go between.

## What is still ahead

- **Relief's remaining adopters**, after Adam has looked at the bevel (MOO-153).
- **`05-authoring-a-theme.md`'s guide landed as `docs/THEMES.md`**, and its two
  homages as built-ins.
- ~~**The desktop is asked, not watched.**~~ Done 2026-09-23 (MOO-156), and
  without a D-Bus client of our own: Slint's winit backend already subscribes
  to the portal's `SettingChanged` (and winit reports macOS's appearance), so
  `MainWindow.desktop-color-scheme` mirrors `Palette.color-scheme` and a
  `changed` handler hands the new side to `system::desktop_reported`. Auto
  re-sides the saved appearance, or the page's uncommitted edit while
  Preferences is open. The startup probes in `system.rs` stay for the first
  answer.
- **The remaining literals.** The type and stroke sweep is complete. Metrics
  are not: `density` reaches `Theme.control-height` and the padding ramp, and
  the great majority of paddings in the device faces are still literals. That
  is the honest position and it is the one step 01 asked for -- an untokenized
  metric is a theme that has less effect, and a wrongly tokenized one is a
  layout that breaks at a setting nobody tested.
