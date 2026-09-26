# Icon pass

Adam, some time before 2026-09-14 (`ENHANCEMENTS.md`): *"i want to do a text
label -> icon pass at some point"*. It was deferred the same day: *"Wanted, but
it is polish over a shell that is still moving, so doing it first means doing
it twice."* It sat in `FOCUS.md`'s "deliberately not now" list after that.

Adam, 2026-09-26, answering MOO-273's sketch (an icon for each device kind,
in place of the header's bar and dot): *"B, filled shapes. There has been an
'icon pass' planned for a while. Maybe it is time to go ahead and do this? I
thought it might make sense to build an icon registry to help with consistency
and avoid dupes."* And in chat, the same day: *"i have been thinking we should
have our icons in some kind of library class or something. let's plan out that
icon pass."*

## Why the deferral no longer holds

**The shell has stopped moving.** The pane layout, the console, the device
rack's joins (MOO-218) and folds (MOO-219), and the plugin faces have all
landed. `FOCUS.md` has no shell work in flight.

**A registry removes the "twice".** Doing the pass twice was a risk because
an icon was written where it was drawn: move the button and you redraw it.
With one registry, a moved button carries its icon's *name*, and a restyle is
one file. The deferral priced a pass without a registry.

**The cost of waiting is growing**, as the survey shows: one meaning, three
glyphs.

## The survey (2026-09-26, `main` at `83f3f78e`)

**There is already a small icon system, in four copies.** `ToolButton` has an
`icon` property that draws SVG path commands on a 16×16 grid, as a 1.3 px
outline with no fill (`controls.slint:246`, `:304-322`). It came from commit
`8ef32a22` (2026-08-19), because *"the pencil and scissors glyphs resolve
through font fallback, so each one arrived at a different weight and
baseline"* (`toolbar.slint:11-13`). Four separate sets of path strings use it:

- `ToolIcons` (`toolbar.slint:14-60`): tool modes and the panel toggles;
- `StripIcons` (`strip.slint:223-229`): EQ bell and shelves, and a dot;
- `SamplerDeviceIcons` (`sampler-device.slint:114-119`): previous, next,
  search, reverse;
- private properties in `eq-device.slint:159-164`: a **second, different**
  drawing of the shelves and passes.

Plus the inline shapes: the layout chips (`main.slint:8298`), `ConsoleButton`'s
sine (`controls.slint:2084`), the colour picker's "none", and a close X in
`appearance-dialog.slint:1503`.

**About 45 call sites draw icons as text glyphs** through the platform font,
with no bundled font and no image assets. The worst of it is one meaning drawn
different ways:

| Meaning | Drawn as |
| --- | --- |
| Remove | `×` (9 sites, literal and `\u{00d7}`), `✕` (remove device), `−` (remove channel) |
| Previous / next | `<` `>` (plugin face), `‹` `›` (every stepper, mixer pages), Path chevrons (sampler) |
| Fold / disclose | `<` `>` (device fold, the same glyphs as previous/next), `▾` `▸` (browser), `⌃` `⌄` (modulation shelf) |
| Save preset | `⌑` (the rack's rail), `💾` (an emoji, in the channel sidebar) |
| On / bypass | `●` `○` text, a Path dot, the word "ON" |
| EQ shapes | `StripIcons` on the strip, `eq-device`'s own paths on the face; a bell there is its band number |
| Modulation polarity | `±` `↑` (shelf), "BI" / "UNI" (ML-P8) |

`●` alone means bypass, pinned, "has a lane" and record arm. `♪` means both
"sample" and "autoplay", and the autoplay one has no tooltip.

**Words that wanted to be icons.** `UI_DESIGN.md:95` asks for waveform and
filter selectors as an "icon or short-label selector bank", and `:737-747`
says *"draw the thing when the thing is a shape"*. Today they are letters, and
different letters on every face: "S T W P", "SIN TRI SAW PLS", "Sin Tri Saw Sq
S&H", "SIN … SQR RND", "LP BP HP". The sampler's loop modes were Path icons in
`8ef32a22` and are words again.

## The design

**One registry: `ui/icons.slint`.**

- `export global Icons` holds every icon as a path string on the 16×16 grid,
  named for what it *means* (`remove`, `previous`, `fold`, `save-preset`), not
  for what it looks like.
- It also holds the two kind tables, `source-kinds: [string]` and
  `effect-kinds: [string]`. These are indexed by the numbers the markup
  already uses for kinds (`device_kind_to_int`, `effect_kind_index`), the
  way `SourceKinds.labels` is.
- `export component Icon` is the only thing that draws one: a `Path` at a
  size, filled with a tint.
- Nothing outside `icons.slint` spells a path string or an icon glyph.
  A check enforces that (step 01), and its count is the pass's progress bar.

**Adam's rulings (2026-09-26)**, taken in one sitting:

- **Filled everywhere.** Style B, one family: device kinds and action buttons
  alike. The existing outline icons are redrawn filled (step 03).
- **Path strings in a global**, extending what `ToolButton.icon` already
  does. No files, tinted from the theme, crisp at any scale. Step 01 measures
  what a Path costs the UI thread under FemtoVG before the pass commits to it;
  SVG with `colorize` is the fallback, behind the same `Icon` component.
- **Our own drawings, on one grid**, in the hand of the MOO-273 sketch. No
  outside icon set and no licence notice.
- **Steps 01 and 02 are 0.1.6** (MOO-273 is already there). Steps 03 to 05 are
  0.1.7.

MOO-273's two remaining recommendations stand until Adam says otherwise. A
plugin gets **one generic icon**, and a device's icon takes the **device
colour**, faint while bypassed.

## Steps

| Step | What becomes true | Release |
| --- | --- | --- |
| [01](01-the-registry.md) | One `Icons` registry and one `Icon` component; the four existing sets moved into it unchanged; a check that counts every icon drawn outside it; the FemtoVG cost measured | 0.1.6 |
| [02](02-device-kinds.md) | Every device kind has a filled icon, in the header and on the folded strip (MOO-273) | 0.1.6 |
| [03](03-one-family.md) | The existing outline icons are redrawn filled, and the EQ has one set of shapes | 0.1.7 |
| [04](04-one-meaning-one-icon.md) | Every text glyph that is an icon comes from the registry, one icon per meaning; the check reads zero | 0.1.7 |
| [05](05-words-to-icons.md) | The letter selectors and worded buttons that are really shapes become icons, after Adam sees a sheet | 0.1.7 |

Linear: project **Icon pass**.

## What this does not do

- **Mute and solo stay letters on the mixer.** Rack rows draw them as bars;
  the split is deliberate and documented (`controls.slint:2030`, `:2800-2805`).
- **Quantities stay words and numbers.** `UI_DESIGN.md:737` draws a shape
  only when the thing is a shape.
- **Displays are not icons**: knob arcs, envelopes, response curves, meters,
  lamps. Nor are the modulation tiles, which are *"parameter-derived previews,
  not generic type icons"* (`UI_DESIGN.md:519`).
- **Not the desktop app icon** (`LOOSE_ENDS.md:1508`), and not the mockup or
  `device-concepts.slint` files.
- **No icon without a tooltip.** *"Familiar icon buttons receive tooltips"*
  (`UI_DESIGN.md:980`). The pass adds the two that are missing.
