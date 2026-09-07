# Interface iteration status

## Step 01 — the browser browses presets

Landed on `feat/preset-browsing` (2026-09-07). The browser sidebar has two
tabs over one row model. PRESETS scans every well-known preset directory,
groups what it finds, and loads one onto the selected channel.

In plain terms: the ninety-nine presets a seeded machine ships were reachable
only from a rail button on a row that already existed. They are now something
you can look through, and an effect preset can be added to a chain that does
not have that effect in it yet.

Where it is, all in `crates/mooloop-ui/src/lib.rs` except the markup:

- `scan_preset_catalog`, `build_preset_rows`, `preset_detail` — the catalogue
  and the rows
- `PresetSlot`, `PresetGroup`, `BrowserTab` — the vocabulary
- `append_effect_preset`, `load_preset_document` — the two load paths
- `BrowserRow`, the tab header and the row rendering — `ui/main.slint`

### Three things the doing turned up

- **The plan said the target is "the selected rack row for an effect". There
  is no such thing.** Only the *modulation* rack has a selected slot
  (`modulation_selected_slot`); the effect rack has none, because every rail
  button belongs to a row and so already knows its own index. Rather than
  invent a selected-device concept for this step, an effect preset from the
  browser **appends** a device to the end of the chain. That turned out to be
  the better gesture anyway, and it is the one this plan should have asked
  for: the rail replaces what is in a row, and the browser adds a row, which
  is how you audition a reverb you do not already own. It is also why an
  effect preset is always loadable while a generator preset is not.

  A selected-device concept is still wanted — step 02's clipboard and step
  04's device actions both need one — and it is now unblocked rather than
  needed here.

- **Category is not worth a tree level, which is what the plan asked for.**
  Of the ninety-nine presets on a seeded machine, sixty-six are categorised
  `"Factory"` and seventeen `"DS-01"` — restatements of the directory they
  are already filed under. A category level would have been two rows of
  chrome around one useful row. The taxonomy is one level deep (the kind),
  and category and tags are a trailing detail shown only when they say
  something the group does not. `a_category_that_restates_its_group_is_not_shown`
  is that rule as a test.

- **A preset group's identity is its directory, so expansion came free.**
  `Session::browser_expanded` is a `HashSet<PathBuf>` built for sample
  folders; a group row hands back its own directory path, so
  `toggle_browser_folder` works on it unchanged and the session never learns
  what a preset is. The one place this leaked was the right-click handler,
  which removed a browser *location* on any depth-0 row and would have tried
  to remove a preset group as one — it checks the kind now, not just the
  depth.

### What it does not do

- **No keyboard navigation.** It is the other half of Adam's ask and it is
  step 04's, which owns the focus vocabulary the tree would need. The tab
  makes the case sharper rather than solving it.
- **No preview.** A sample previews on click; an effect preset cannot without
  being loaded, and loading is one undo away, so load *is* the audition.
- **No filtering to the selection, and no search.** The catalogue is ninety-
  nine rows across thirteen or so groups, which fits. Both become worth
  building when a user's own bank does not.
- **Nothing about the factory-bank update problem.** `LOOSE_ENDS.md` records
  that banks self-seed once behind a marker and can never update. A browser
  makes that more visible and does not change it.

### One thing the tests found, and one they could not have

- **Two of the three new interaction tests were passing vacuously**, and the
  third is why that was found. `tests/browser.rs` locates a row by scanning a
  column for the first y that repaints under hover, and that column runs
  through the panel header -- which now holds two tabs instead of a static
  label. An *active* tab draws `surface-active` whether or not it is hovered,
  so the sample tests, which run on the SAMPLES tab, never noticed. On the
  PRESETS tab the SAMPLES tab is inactive, hovering it does repaint, and the
  scan stopped in the header. The two tests asserting that something did *not*
  happen passed anyway; `clicking_a_preset_loads_it`, which asserts something
  did, failed. Preset tests now scan `PRESET_ROW_X`, clear of the tab strip.

- **A generator preset's loadability went stale on a channel switch**, and no
  test would have caught it because none of them switches channels. Whether
  the row is greyed depends on the selected channel's device kind, and nothing
  rebuilt the rows when that changed -- so selecting a DS-01 channel left the
  DS-01 presets drawn as unloadable. `refresh_preset_menus` already exists to
  catch exactly those switches and is called from all four sites, so the fix
  is one call at the end of it.

### What clippy found once it could reach this crate

Not part of the step, and the most valuable thing in the commit. `cargo
clippy` had been failing at `mooloop-core` since `df52933`, and a run that
dies there never lints anything downstream — so `mooloop-ui` had never been
linted at all. With `mooloop-core` cleared earlier the same day, two errors
surfaced here, neither of them from this work:

- A redundant `let sample_rate = sample_rate;` in `on_wrap_effect_clicked`.
  Harmless; `sample_rate` is a `u32` and the `move` closure copies it anyway.
- **A `#[test]` attribute that had come adrift from its function.** The
  container face test was inserted between
  `effect_rack_scrolls_horizontally_to_reach_a_long_chain`'s doc comment and
  its `fn`, so both `#[test]`s landed on the container test and the scroll
  test had none. It had not run since. Its doc comment records the regression
  it exists for — a rack viewport sized for an empty chain, leaving every
  device past it laid out but unreachable — and that guard was off.

Both are fixed here rather than recorded, because they are two deleted or
moved lines in a crate this branch is already rebuilding, and because leaving
clippy red on `mooloop-ui` would have gone on hiding the next one. The test
passes now that it runs again, so nothing had regressed behind it.

### Acceptance

`build_preset_rows` and `preset_detail` carry six unit tests in
`mooloop-ui`'s `preset_browser_tests`, the ones worth naming being
`an_effect_preset_is_loadable_whatever_the_channel_holds` and
`a_generator_preset_is_loadable_only_on_its_own_kind` — together they are the
append-versus-replace rule above, which is the only real decision in the step.
