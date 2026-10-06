# plugin-browser — status

Linear: project [Plugin browser](https://linear.app/mooloop/project/plugin-browser-02164e1c1308).
Release label `0.1.8`.

| Step | Issue |
| --- | --- |
| 01 | MOO-515 |
| 02 | MOO-516 |
| 03 | MOO-517 |
| 04 | MOO-518 |
| 05 | MOO-519 |

Step 02 blocks 03 to 05: they share the filter row it adds.

Planned 2026-10-06.

## Built 2026-10-06, all five steps in one change

The five steps share `plugin_ui::plugin_rows`, `BrowserRow` and the filter
row in `main.slint`, so they landed together rather than one commit each.

- **01.** `SearchField` gained `forwards-navigation`, `navigation-key`,
  `take-keys()` and `has-keys`. `focus-pane` gives the browser's field the
  keys when the browser is focused and hands them back to `keys` when
  another pane is; the field passes Up, Down, Page Up, Page Down, Return,
  and Left/Right while empty, to `shortcut-key`, so a rebound chord still
  applies. Esc keeps its old meaning in the field: clear, then give the keys
  back.
- **02.** The groups are kind-2 rows whose paths are
  `plugin_ui::INSTRUMENTS_GROUP` and `EFFECTS_GROUP`; their open state is
  `PluginView::collapsed`, toggled through the same `browser-row-toggled`
  the PRESETS tab uses. Files that failed to scan stay last, at depth 0.
- **03.** Format chips come from `filter_chips`, one per format any entry
  has. Only CLAP today.
- **04.** `mooloop_plugin_host::category` holds `PluginCategory` and the
  CLAP feature table; `ScannedPlugin::categories()` reads it (VST3 and AU
  return none until they are hosted). The Type menu is a `PickerChip`.
- **05.** `PluginSettings::favourites` (`[plugins] favourites`, a list of
  `{ format, id }`), toggled from the row's star or its new right-click
  menu, saved as it is clicked; a failed save puts the star back and says
  so in the status bar.

While a chip or the Type menu narrows the list, files that failed to scan
are left out: they have no role, format or category to match.
