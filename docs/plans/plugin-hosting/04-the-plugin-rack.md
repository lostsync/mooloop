# 04 — `PluginRack`, main-thread requests, latency known at runtime (#26)

This step removes blockers 3 and 5. It is still tested with the fake plugin,
now given a main-thread half.

## The rack

`PluginRack` goes in `mooloop-session`, because the session is what the UI
pump drives. It holds `BTreeMap<PluginSlotId, Box<dyn HostedInstance>>`, and
`HostedInstance` is the main-thread trait, defined in `mooloop-plugin-host`:

```rust
pub trait HostedInstance {
    fn plugin(&self) -> &PluginRef;
    fn params(&self) -> &[PluginParamInfo];
    fn latency_frames(&self) -> u32;
    fn save_state(&mut self) -> Result<PluginState, HostError>;
    fn load_state(&mut self, state: &PluginState) -> Result<(), HostError>;
    fn value_text(&self, id: u32, value: f64) -> Option<String>;
    fn take_requests(&self) -> Requests;          // drains the flag word
    fn on_main_thread(&mut self);
    fn restart(&mut self) -> Result<Box<dyn AudioNode + Send>, HostError>;
    fn gui(&mut self) -> Option<&mut dyn HostedGui>; // step 11
}
```

`FakeInstance` implements it.

## Lifetime

- **Insert:** create the instance, activate it, build its processor, then
  send `InstallEffect`.
- **Remove:** send the remove, mark the entry as "dying", and drop it when
  the matching `StructuralReclaim` comes back through `EngineHandle::poll`
  (`engine/src/lib.rs:622`). Add a slot id to the reclaim message if it
  doesn't carry one yet.
- **Replace:** `ReplaceEffect` already checks the expected kind and resource
  key (`lib.rs:200`). Use the plugin slot id as the resource key, so a
  replacement that arrives after a remove does nothing.
- **Project close and app exit:** remove everything, then wait for every
  reclaim with a bounded timeout. After the timeout, stop the audio driver
  first and then drop the instances. The order is what matters here, not
  speed.

## Requests

Once a pump tick, `Session` calls `PluginRack::service()`:

- *callback*: call `on_main_thread()`.
- *restart*: call `restart()`, send `ReplaceEffect` with the new node, and
  recompute compensation.
- *latency changed*: recompute compensation.
- *params rescan*: replace `PluginSlotState.params` (see step 03's rule).
- *state dirty*: mark the song as modified.

## Latency

Wherever compensation reads `EffectKind::latency_frames()` for a slot,
read it through a `slot_latency(target, device)` that asks the rack for
`Plugin` and asks the kind for everything else. Find the call sites with
`codegraph_callers`. Do not add a second compensation path.

## Tests

- The fake instance reports latency 0, then 512. Compensation follows, and a
  sample-accurate impulse through a parallel dry path stays aligned.
- Remove followed immediately by project close: the instance is dropped only
  after its reclaim, which a drop counter checks.
- A restart request that arrives after its slot was removed does nothing.

## Done when

- [ ] #26: "no external object is created, destroyed, rescanned, or
      serialized on the audio thread", checked with the counting
      `GlobalAlloc` the engine tests already use (`mooloop-engine/src/lib.rs`).
