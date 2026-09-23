# 09 — A channel source that is a boxed node (#29)

This step removes blocker 4, and no plugin is involved yet. It depends only
on step 03, so it can run alongside steps 05–08.

## The problem

`ChannelStrip` (`engine/src/render.rs:2154`) holds all eight generators as
concrete fields. `active_source: DeviceKind` chooses one of them, through
`source_node` / `source_node_mut` (`:2359,2372`), `process` (`:2466`),
`push_source_base` (`:2280`) and `choke_group` (`:2440`). A source change
(`SetChannelSource`, `:4271`) resets the generators and allocates nothing.
Nowhere can hold a node that is built outside the engine.

## The change

- `DeviceKind::Plugin` (`core/src/channel.rs:28`),
  `GeneratorParams::Plugin(PluginSlotId)`, and
  `ChannelSource::Plugin(PluginSlotId)` with `serde(rename = "plugin")`.
- `ChannelStrip.hosted: Option<Box<dyn AudioNode + Send>>`, with
  `StructuralCommand::InstallSource { channel, slot: PluginSlotId, node }`.
  The node it replaces goes back on `reclaim`. When `active_source ==
  Plugin`, the dispatch arms use `hosted`, and **silence** when `hosted` is
  `None`. That silent case is also the missing-instrument placeholder.
- `SetChannelGeneratorParam` (`:4295`) and generator modulation events
  (`:~5080-5130`) are already `ParamValue` events on the channel's
  `EventList`. For `Plugin` they go to `hosted` as they are. `source_base`
  doesn't apply to a plugin; skip `push_source_base`.
- Leave the eight native generators exactly as they are. **Do not** turn
  them into boxes as part of this step. That is a separate refactor with its
  own reasons, and none of those reasons are this plan's.

  *2026-09-22:* Adam has since ruled that the separate refactor should be
  done, and that a source change may land as a queued structural edit rather
  than instantly. It is MOO-56, and `00-status.md` ("Adam's answers,
  2026-09-22") has his words. This step still does not do it. If MOO-56
  lands first, the plugin source goes into that one slot, not a ninth field.
- Choke: `Event::Choke` goes to `hosted` like any other event. Step 10 turns
  it into CLAP note-offs.
- Generator parameters are addressed with `ParamOwner::Source` and the
  plugin's id (step 03's `source_params`).

## Tests

- A `FakeSourceNode` (a sine voice per note id) installed as a channel's
  source plays from a pattern, offline and in realtime, with note
  boundaries accurate to the sample.
- Switching the source from the fake source to Sampler and back: the fake
  source is reclaimed and a new one is installed, with no allocation in the
  callback.
- The fake source is saved and loaded as a channel source with its state.

## Done when

- [ ] A channel can own a source that the engine did not construct.
