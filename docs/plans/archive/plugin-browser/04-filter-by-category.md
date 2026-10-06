# 04 — filter by category

A **Type** menu on the filter row: "Any type", then each category at least
one listed plugin declares, in a fixed order.

- One table in `mooloop-plugin-host` maps CLAP feature strings to a
  `PluginCategory` (`README.md` lists the mapping). The UI never reads a
  feature string itself, so VST3's subcategories can feed the same enum when
  step 12 lands. AU reports no category.
- A plugin with no category feature is **Other**.
- The row's detail names its category when it has exactly one.

**Done when** the table is tested against the CLAP feature names, and a
catalogue with an `equalizer` and a `compressor` plugin offers EQ and
Dynamics and filters to each.
