# 05 — favourites

A star on each plugin row. Clicking it stars or unstars the plugin, and so
does the row's context menu. No default key: with step 01 a bare letter in
the browser types into the filter.

- Kept in the settings file as `plugins.favourites`, a list of
  `{ format, id }`, so it survives a rescan and is the same in every song.
- A **★** chip on the filter row shows only starred plugins.
- A starred plugin that is no longer installed is kept in the list and shown
  nowhere.

**Done when** starring writes the setting, a reload reads it back, and the
star chip narrows the list.
