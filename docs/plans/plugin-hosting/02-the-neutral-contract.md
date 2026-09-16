# 02 — The neutral contract (#26)

This step decides what a song says about a hosted device and saves it. No
plugin is loaded here. A **fake plugin** proves the whole path from end to
end.

## Types

These go in `mooloop-core`, in a new `plugin.rs`: `PluginFormat`,
`PluginRef`, `PluginParamInfo`, `PluginSlotId`, `PluginSlotState`, and
`PluginStateText`. `00-status.md` gives their shapes.

- `PluginSlotId` is minted from `Project.next_plugin_slot`, which is
  `serde(default)`. Ids are never reused. Follow the `DeviceId` pattern:
  `UNASSIGNED`, plus a mint that runs when the device is inserted
  (`structure.rs`'s `mint_device_id` / `assign_device_ids`).
- `PluginStateText` holds `PluginState` in memory. It is written as base64
  in a multi-line TOML string, wrapped at 76 columns, and read back with
  whitespace ignored. Add `base64` to the workspace dependencies. Each chunk
  is written as `{ tag, data }`.

## Variants

- `EffectKind::Plugin`. `descriptors()` returns `&[]`. `latency_frames()`
  returns 0, with a doc comment that points to step 04, because the real
  number comes from the instance. `label()` returns `"Plugin"`. Leave
  `Plugin` out of `EffectKind::ALL`: the menu is not how you reach it, and
  the preset catalogue scan (`scan_preset_catalog`) must not create a
  `presets/effects/plugin/` folder. Check every place that uses `ALL` and
  record which ones need `Plugin` and which don't.
- `EffectParams::Plugin(PluginSlotId)`, with `#[serde(rename = "plugin")]`.
  `get` and `set` by id return `None` and do nothing. Parameter values for a
  plugin live in the instance and in the saved state, not in
  `EffectParams`. Step 07 adds base-value storage if it turns out to be
  needed.
- `build_effect` (`dsp/src/effects/mod.rs:99`) can't build a plugin, and
  it shouldn't know plugins exist. Make it return a pass-through
  placeholder for `Plugin`. The session replaces that placeholder with the
  real processor in step 06. **That placeholder is also the missing-plugin
  node.**

## The format document

Add a "Hosted plugins" section to `PROJECT_FORMAT.md`. It covers the
`plugins` table, the state encoding, and why plugin state is allowed in the
TOML when samples are not. Plugin state is small in the common case, it
belongs to the device the way its parameters do, and Adam asked for it.
Also say what a missing plugin does.

## A test that CLAP stays out

Add a test (in `mooloop-core/tests/`, or as a `scripts/dupe-audit` check if
that fits better) that reads each workspace member's `Cargo.toml` and fails
if any crate other than `mooloop-plugin-host` or `mooloop-test-plugin` names
`clack`, `clap-sys` or `vst3`.

## The fake plugin

`FakePluginNode` in the test helpers: a gain node whose "state" is its gain
as bytes. Use it to go through #26's first checklist item using the same
functions the UI calls: insert, reorder, address one parameter with a
modulation route and a lane, save, load, bypass, remove.

## Tests

- A song with a plugin slot round-trips exactly, including a 1 MB state.
- A song with an unknown `PluginRef` loads. The slot is a placeholder, its
  state and its lanes survive a save, and the saved file is byte-identical.
- **An older reader refuses the song instead of loading half of it.**
  `deserialize_effect_params` (`effect.rs:3082`) is `untagged` and falls back
  to `FilterParams`. That fallback fails today only because `cutoff_hz` is
  required. Write the test so it fails if that ever changes, since a plugin
  silently becoming a Filter is the worst outcome here.
- The existing id-freeze and format tests stay green without edits.

## Done when

- [ ] #26's checklist, except "dynamic metadata changes" (step 03) and
      "no external object created on the audio thread" (step 04).
