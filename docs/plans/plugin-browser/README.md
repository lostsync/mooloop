# Plugin browser

The browser's PLUGINS tab becomes something you can find a plugin in. Planned
2026-10-06 for 0.1.8 (the UI/UX push). Linear: project **Plugin browser**,
one issue per step (`00-status.md` has the list).

## Why, in Adam's words

**2026-10-06**:

> the plugin browser sidebar needs work. we should have a filter bar for text
> filtering. that needs to work such that if the sidebar pane has focus and
> someone starts typing, it starts filtering. so basically im just saying the
> field should have keyboard focus when that sidebar tab gets focused. we
> should have filters for instruments and effects - they should be separated
> when unfiltered as well. we will eventually need filters for plugin type so
> we may as well add that (e.g. CLAP, VST3, etc). filter for the type of
> effect, if this is reported. eg, EQ? Dynamics? etc. we should also be able
> to star/favorite plugins.

## What is there now

- One flat list, sorted by name, instruments and effects mixed; the row's
  detail says `vendor · Instrument` or `vendor · FX`
  (`plugin_ui::plugin_rows`).
- A text field above the list, shared by all three tabs (MOO-9). It filters
  by name, vendor, id and the detail text. It has keyboard focus only when
  clicked: the browser deliberately has no `FocusScope` of its own, and the
  root scope `keys` hears every key and routes the arrows to the focused
  row (`browser_move_focus` in `lib.rs`).
- Only CLAP is hosted. VST3 is plugin-hosting step 12 and AU step 13, both
  not started, so every scanned plugin today is CLAP.
- The scan keeps each plugin's CLAP feature strings
  (`ScannedPlugin::features`), which is where a category comes from.

## What a plugin reports about its kind

| Format | Reports a kind? | Where |
| --- | --- | --- |
| CLAP | **Yes.** Standard feature strings: `equalizer`, `compressor`, `limiter`, `gate`, `expander`, `transient-shaper`, `deesser`, `reverb`, `delay`, `distortion`, `chorus`, `flanger`, `phaser`, `tremolo`, `filter`, `pitch-shifter`, `pitch-correction`, `analyzer`, `utility`, ... and for instruments `synthesizer`, `sampler`, `drum`, `drum-machine`. Optional: a plugin may declare none. | `clap/plugin-features.h`; already in the scan cache |
| VST3 | **Yes**, once VST3 is hosted: the class's subcategory string, e.g. `Fx\|EQ`, `Fx\|Dynamics`, `Fx\|Reverb`, `Instrument\|Synth`. | `PClassInfo2::subCategories` |
| AU | **No.** Only effect vs. instrument (`aufx`, `aumu`, ...) and a four-character subtype the vendor picks. No EQ or dynamics category exists. | `AudioComponentDescription` |

A plugin that declares no category is listed under **Other**; nothing is
guessed from its name.

## Defaults this plan picked, open to Adam

- **The field takes the keys on every tab, not only PLUGINS.** It is one
  field for all three, and a rule that holds on one tab only is a rule nobody
  can remember. Up, Down, Left, Right, Enter, Ctrl+Enter and Esc still drive
  the rows from inside the field; **Space types a space** while the field has
  the keys, so it does not start the transport there.
- **Sections are collapsible groups**, `INSTRUMENTS` then `EFFECTS`, the same
  row shape the PRESETS tab's groups use, so Left and Right open and close
  them. They stay while a filter is on, with only the matching rows.
- **One category menu** covers both roles: EQ, Dynamics, Reverb, Delay,
  Distortion, Modulation, Filter, Pitch, Analyzer, Utility for effects, and
  Synth, Sampler, Drums for instruments. A plugin that declares two (a
  filter-EQ) is in both.
- **Favourites are kept in the settings file**, not in a song: a star is
  about you, not the track. Starred plugins are not re-ordered; the star
  filter shows only them.
- **Format chips show only formats that are installed**, so today there is
  one chip, CLAP, until step 12 of plugin-hosting lands.

## The steps

1. `01-the-field-takes-the-keys.md`
2. `02-instruments-and-effects-apart.md`
3. `03-filter-by-format.md`
4. `04-filter-by-category.md`
5. `05-favourites.md`
