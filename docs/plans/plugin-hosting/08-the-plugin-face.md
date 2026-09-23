# 08 — The plugin browser, the menu row, and the face for plugins with no GUI (#28)

This is the only UI step before GUIs. Draft every surface in
`scripts/slint-sketch` and build once. Read `UI_DESIGN.md`,
`WIDGET_INVENTORY.md` and `ACTIONS.md` first. If step 11 is also in flight,
batch its `main.slint` changes into this step.

## A face for plugins with no GUI is a real face

Adam, 2026-09-16: plugins with no GUI need to be handled, and Airwindows is
the named case. For those plugins, **this face is the device.** Airwindows
plugins have between one and about a dozen parameters, with plain names,
and the plugin supplies each value's display text. A row of the knobs
mooloop already uses (the same widget, the same modulation ring, the same
lane affordance as a native device) is the right shape for them. It is
*not* the pages-of-knob-rows that Adam rejected for native devices. That
rejection was about devices that have an idea of their own to show. A
generic plugin has nothing to show except its parameters, and the plugin
has already chosen their names and order.

- **Model:** `[PluginParamRow { id, name, text, value, stepped, steps,
  modulated, automated }]`. `text` comes from `HostedInstance::value_text`
  and is refreshed when a value changes, never once per frame.
- **Layout:** a `for` over the rows, grouped under a small label by the
  CLAP `module` path when the plugin gives one. Stepped parameters with at
  most eight steps show as a segmented selector. Everything else is a knob.
- **Large plugins** (Surge XT reports hundreds of parameters): the face
  shows the **pinned** parameters (`PluginSlotState.pinned`). By default
  those are the first eight the plugin doesn't mark hidden. A "Parameters"
  list in the sidebar, with search, shows everything and pins or unpins.
  That follows the split the mixer strip's sends settled on (`e034213`):
  the face draws the compact part, and the sidebar owns the rest.
- **Rail:** the rail is the same as a native device's: bypass, wet/dry,
  trims, preset. It adds an **Open editor** button when the plugin has a GUI
  (step 11), and a **failed** or **missing** badge whose tooltip gives the
  reason.
- `EffectSlotRow`'s fixed `p0..p9` (`main.slint:211`) doesn't carry these
  parameters. The face gets its own model property, keyed by slot, the way
  the EQ is already a special case.

## Adding a plugin

- The `EffectTypeMenu` (`device-rack.slint:172`) gets one more row,
  **Plugin…**, which opens the plugin browser, positioned to insert where
  the menu was opened.
- The **browser dock** gets a third tab, **Plugins**, next to samples and
  presets (`main.slint:1457-1515`). Rows come from the scan cache: name,
  vendor, and an effect or instrument badge. A plugin that failed to scan or
  is incompatible is greyed out with its reason. Filter on the name. Drag a
  plugin onto a rack row, or double-click it to insert after the selection.
  Instruments are disabled until step 10 lands.
- **Preferences** gets a **Plugins** page: extra paths, scan timeout,
  rescan on startup, **Rescan all**, and the list of failures.
- Register actions (`ACTIONS.md`) for `plugins.rescan` and
  `browser.plugins`.

## A parameter that is missing (Adam, 2026-09-23, MOO-74)

A lane, route or binding whose plugin parameter doesn't exist is kept, never
dropped. That happens when the plugin is missing, when an update removed the
parameter, or after a `params.rescan`. Step 03 has the rule. This step draws
it, and Adam gave the treatment in his own words: *"represented in the UI as
missing somehow (greyed out/crosshatched, or like itallicized/thin weight
title in the lane selection menu)"*.

- **Lane selection menu:** the missing parameter's row stays in the list,
  with its title in italic or thin weight, and it names the parameter by the
  name it last had (`PluginSlotState.params`). If there is none, it uses its
  id.
- **The lane itself, and the route row on the shelf:** drawn greyed or
  crosshatched. They're still selectable, deletable and saved, but they play
  nothing.
- **Reunion:** when the parameter comes back, the same rows draw normally
  again, with no action from the user and no repair step.

A test loads a song whose plugin reports no such id, reads the lane menu row
and the lane as missing, then rescans with the id present and reads them as
normal.

## Tests

- A `slint_face_agreement`-style test: the face declares no ranges (the
  rows carry them).
- A `mooloop-ui` test: the Plugins tab lists the test plugin, a
  double-click inserts it, and the face shows its three parameters with
  `text`.
- `scripts/dupe-audit unchecked-face` is still clean, or it reports
  `plugin-device.slint` with a reason recorded.

## Manual

Put an Airwindows plugin and an LSP plugin in one channel. Play, turn every
knob, automate one, modulate one, save and reopen. **Show Adam before
calling this step done**: this is a face, and the face rule says draw what
makes the device different.

## Done when

- [ ] #28: "Provide a generic headless parameter UI."
- [ ] Adam has seen the face on a GUI-less plugin.
