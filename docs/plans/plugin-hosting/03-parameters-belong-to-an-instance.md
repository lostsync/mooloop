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
- The integrity pass (`project/src/integrity.rs`): see "The integrity pass
  must not judge a plugin address against a static table" below. A route or
  lane on a plugin parameter is never dropped for its `param`, whether or not
  the id is in the slot's `params`.

## The address: `ParamOwner::PluginParam { device }` (Adam, 2026-09-23)

MOO-74 settled it (`00-status.md`, "Parameters"). This step adds the
variant:

- `ParamOwner::PluginParam { device: DeviceId }`, with the plugin's own `u32`
  id in `param`. The payload is only a `DeviceId`, so
  `size_of::<ParamAddr>()` stays 16. Keep the assertion in
  `core/src/modulation.rs` as it is; if it moves, the payload grew and that
  is a 2 MiB decision (MOO-74, C.3), not a detail. Serde spelling:
  `owner.plugin_param.device = N`, a permanent `serde(rename)`. Add a
  format test that pins it.
- Give each of the ten exhaustive `owner` matches a real arm. Don't use a
  catch-all. MOO-74 counted them on 2026-09-22 (recount against the tree):
  `project/src/integrity.rs` (2), `session/midi.rs`
  (4), `session/session.rs` (2), `engine/src/render.rs` (1) and
  `ui/src/lib.rs` (1, the shelf's owner token). `ParamAddr::device()` has a
  `_ => None` arm and compiles without the new variant, so give it the arm
  by hand, returning the device.
- **The UI's `i32` owner token** (`ui/src/lib.rs`, the shelf's
  `ModulationRouteRow.owner`) needs a token for the new owner. `param`
  crosses into Slint as `i32`, so a plugin id above `i32::MAX` wraps. Carry
  the dense index (`index_of`) across the boundary, not the raw id, and map
  it back on the Rust side.
- **A plugin instrument (step 10) has no `DeviceId` today.** A channel's
  source is `ParamOwner::Source`. Before step 10, decide how the source slot
  gets a `DeviceId` without widening the payload. Minting one from the
  channel's chain counter is the obvious answer. Don't answer it here by
  adding a second payload field.

## The integrity pass must not judge a plugin address against a static table

This is MOO-74's C.6. `ChainShape::problem_with`
(`project/src/integrity.rs`) validates every address against a descriptor
table, and its callers **delete** what fails: a lane is `retain`ed out ("drop
the lane"), and a route is set to `None` ("drop the route"). The `Source` arm
is guarded by `descriptors().is_empty()`, with a comment saying why. **The
`Effect` arm has no such guard.** `EffectKind::Plugin`'s `descriptors()` is
`&[]` (step 02), so any path that sends a plugin device's address through the
`Effect` arm would delete every lane and route on it at load.

Required here:

- The `PluginParam` arm never consults `EffectKind::descriptors()` or any
  other `&'static` table. It checks only that the device exists on the
  chain and is a plugin. A `param` that isn't in the slot's `params` is
  **not a problem**. It is a missing parameter (below), so it's kept.
- The `Effect` arm gets the same `descriptors().is_empty()` guard the `Source`
  arm has, so an `Effect { device }` address on a plugin device (which
  should not exist, but a hand-edited file could hold one) is left alone
  instead of deleted.
- A test: a song with a plugin device, a lane and a route on it, and no
  entry in `Project.plugins` for its slot (the plugin was never scanned).
  Load it and save it: the lane and route survive and the file is
  byte-identical.

## Missing parameters are kept, never dropped (Adam, 2026-09-23)

MOO-74, Q2 and Q5. This rule covers every way a plugin parameter can go
missing:

- the plugin isn't installed, or failed to load;
- a plugin update removed the parameter;
- the plugin sent `params.rescan` while the song was open, and the new list
  doesn't have the id.

A lane, route or MIDI binding that names a missing parameter is **kept,
saved back unchanged, and shown as missing**. Adam's suggestion for the UI:
*"greyed out/crosshatched, or like itallicized/thin weight title in the lane
selection menu"*. Step 08 draws it. A missing parameter's address produces
no events. When the parameter comes back (the plugin is installed again, or
a later rescan reports the id), the same address resolves again with no
repair step, because the address never changed.

`PluginSlotState.params` is replaced whenever the live instance reports a new
list. Step 07 handles the timing of a rescan. Replacing the list never
deletes, rewrites or remaps an address. Whether an address is missing is
decided when it is read, by looking up `param` in the current `params`. It is
never stored as a flag.

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
