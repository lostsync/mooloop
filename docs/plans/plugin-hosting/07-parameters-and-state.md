# 07 — Parameters, automation, modulation and state round-trip (#28)

This step turns the headless effect from step 06 into a device that a song
can depend on.

## Parameter values

- **Current values.** The session needs a plugin's current values to draw
  knobs and to find a modulation route's base value. Keep a
  `Vec<f64>` per slot, indexed by step 03's dense index, and fill it from
  the plugin's `params.get_value` whenever the instance is created or its
  state is loaded.
- **Edits.** An edit from the UI becomes the same `SetEffectParam` command
  a native parameter sends (`bridge.rs:344`), and the engine turns it into
  `ParamValue`. **No command specific to plugins.** The adapter wraps each
  UI drag in CLAP gesture-begin and gesture-end events, so a plugin that
  records undo gets one undo entry per drag.
- **Changes the plugin makes itself**, which step 06 queues on the ring,
  update the stored value and the knob. They **never** generate a
  `SetEffectParam` in return, which prevents feedback loops.
- **Plugin GUI edits (step 11)** use the same ring. When the plugin sends a
  gesture-end, the session records one undo step for that parameter, the
  same way a knob drag does.
- **Parameters the plugin marks non-automatable** can't take lanes or
  routes. **Hidden** parameters don't appear in the face.

## Automation and modulation

Nothing changes in `control_events_for_slot` (`engine/src/render.rs:~1200`).
It already works in normalized space and produces sample-timed `ParamValue`
events, and step 03 made the normalization per instance. The only check
needed here is a test.

## State

- **Save:** before a song or preset is written, call
  `HostedInstance::save_state()` on the control thread for each plugin slot
  and store the result in `PluginSlotState.state`. A plugin that fails to
  save **keeps its last good state** and logs a warning. It never writes an
  empty state over a good one.
- **Load:** create the instance, call `load_state`, then activate, then
  install. If the plugin rejects the state, the device loads with the
  plugin's defaults, **the rejected state stays in the song unchanged**, and
  the device shows the problem.
- **Parameters that changed since the song was saved:** compare the
  plugin's current parameter list with the saved `params`, and log the ids
  that were added or removed. Handle removed ids with step 03's orphan rule.

## Presets

A plugin device preset is the existing effect preset envelope carrying a
`PluginSlotState`. Add a new `contains` entry, `effect-plugin`, so older
readers refuse it. Presets live under `presets/effects/plugin/<vendor>/<id>/`.
Plugins' own factory presets (CLAP's `preset-discovery`) are **not** in this
plan.

## Tests

- Automate the test plugin's `gain` and render: the output follows the lane.
  Modulate it with an LFO: same result.
- Save, close, reopen: output is identical. Remove the test plugin, reopen,
  save, put the plugin back, reopen: output is identical again.
- A plugin that changes its own parameter shows the change without sending
  a command back.
- A full outgoing ring is visible in the telemetry (`DeviceTelemetry`).

## Done when

- [ ] #28's checklist, except "a generic headless parameter UI", which is
      step 08.
