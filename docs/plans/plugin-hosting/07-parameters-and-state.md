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

## Requirement: a plugin parameter id never crosses into Slint as `i32`

A CLAP parameter id is any `u32`. Every Slint model that carries a parameter
(`ModulationRouteRow.param`, the lane rows, a face's knob rows) is `int`,
which is `i32`, so an id above `i32::MAX` wraps negative on the way to the
face and comes back naming a different parameter, or none. For a
`PluginParam` address, what crosses is the **dense index** into the slot's
`params` (`DeviceParams::index_of`). It is mapped back to the id on the Rust
side, where it came from, and never cast. A test drives an id of
`4_000_000_000` through a route row and a knob and back. (Orchestrator's
condition on MOO-78, 2026-09-23.)

## Automation and modulation

**Rewritten 2026-09-23, after step 03.** This section used to say nothing
changes in `control_events_for_slot`. That was true under the old option 1,
where a plugin's parameter was an `Effect` address, and it stopped being true
when Adam chose `ParamOwner::PluginParam` (MOO-74). The control pass finds
destinations by walking each kind's descriptor table, and a plugin has none.
So a lane or route on a plugin parameter is saved and kept, but it produces
no events until the pass walks what is driven rather than what is described
(MOO-195). This step needs that, or a plugin-only equivalent, plus
normalization against the slot's `PluginParamInfo`: the `DeviceParams` view
step 03 describes, built here against the session and UI callers that
actually draw and edit a plugin's parameters.

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
- **`params.rescan` while the song is open** is supported (Adam,
  2026-09-23, MOO-74). The plugin's request sets a bit in the instance's flag
  word, and the pump reads the new list on its next tick and replaces
  `PluginSlotState.params`. A lane or route whose id is gone goes through step
  03's missing-parameter rule, and it resolves again if a later rescan brings
  the id back. A rescan is not an undoable edit, but it does mark the song
  dirty, because `params` is saved.

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
