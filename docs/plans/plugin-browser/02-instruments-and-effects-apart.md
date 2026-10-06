# 02 — instruments and effects apart

The PLUGINS tab lists two groups, **Instruments** and **Effects**, each sorted by
name, filtered or not. Plugins that failed to scan stay at the bottom, under
the effects.

- The groups are `BrowserRow` kind 2 rows (the PRESETS tab's group shape),
  collapsible with a click, Left, Right or Enter. Their open state is the
  window's, not the song's.
- A filter row under the text field, shown on the PLUGINS tab only, with two
  toggle chips, **Instruments** and **Effects**. Neither on means both.
- The row detail stops saying `Instrument` / `FX`, which the group now says.

**Done when** `plugin_rows` returns the two groups with headers for a mixed
catalogue, a chip leaves out the other group, and the keyboard walk skips
over a collapsed group.
