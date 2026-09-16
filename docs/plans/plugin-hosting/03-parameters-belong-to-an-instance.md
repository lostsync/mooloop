# 03 — Parameters belong to an instance (#26)

This step removes blocker 1: parameters are no longer looked up by kind
alone. Native devices keep their static tables and their frozen ids. What
changes is the question that callers ask.

## `DeviceParams`

Add a view in `mooloop-core`:

```rust
pub enum DeviceParams<'a> {
    Static(&'static [ParamDescriptor]),
    Plugin(&'a [PluginParamInfo]),
}
```

It offers `len`, `iter` (yielding a small `ParamView` with a borrowed name),
`by_id(u32)`, `index_of(u32)`, `to_normalized` and `from_normalized`. For a
plugin, the curve is `Linear`, or `Stepped(n)` when the plugin reports it as
stepped.

`Project` (or `ChannelSetup`) gets `device_params(target, device) ->
DeviceParams`. It returns `EffectKind::descriptors()` for native kinds and
`plugins[slot].params` for plugins. `DeviceKind` gets the matching
`source_params`.

## Callers to move

Move only callers that a plugin can actually reach. Leave the rest on
`descriptors()`.

- `descriptor_slots` (`session/values.rs:125`) and the arrays it sizes
  (`session.rs:1490,1519`). Give plugin devices a map keyed by id, or size
  the arrays by `len()` and index them through `index_of`. Pick one and use
  it for both.
- `session/effects.rs:~601`, where a knob index becomes a descriptor, and
  `ui/src/lib.rs:~2330-2360` (defaults, policy, route counts) and `:4241`.
- `ModDestinationDescriptor` (`core/mod_metadata.rs:247`): build a plugin
  parameter's policy from its `modulatable` flag. When a plugin says
  nothing, allow it, with bipolar depth.
- The integrity pass (`project/src/integrity.rs`): a route to a plugin
  parameter is valid if the id is in the slot's `params`. **It must not
  drop routes for a missing plugin**, because `params` is the list as the
  plugin last reported it.

## A plugin's parameter list changes

`PluginSlotState.params` is replaced whenever the live instance reports a
new list. Step 07 handles the timing. The rule is set here: a route or lane
whose id disappears is **kept and shown as orphaned, not deleted**, so a
plugin update that comes back later brings the route back.

## Tests

- The fake plugin from step 02, with ids `{7, 1000, 4_000_000_000}`, can be
  modulated, automated and saved. Nothing allocates an array of 4 billion
  entries.
- `param_id_freeze_tests.rs` and `slint_face_agreement` pass without
  edits.

## Done when

- [ ] #26: "stable instance and parameter identities survive save/load and
      reorder" and "dynamic metadata changes have a bounded control-plane
      path."
