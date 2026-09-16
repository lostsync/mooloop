# 06 — A headless CLAP effect in a chain (#27)

In this step a real `.clap` effect first makes sound in a mooloop channel.
There is no UI yet: a test or the MCP harness inserts it.

## The adapter

`mooloop_plugin_host::clap` provides `ClapInstance: HostedInstance` and
`ClapProcessor: AudioNode + Send`.

- **Activation** uses the engine's actual sample rate
  (`EngineHandle::sample_rate()`), a minimum block of 1, and the driver's
  maximum block as the upper bound. If the driver can't report one, use
  4096 and say so in the code.
- **Ports:** accept only a plugin whose main input and main output are both
  stereo. Anything else is refused with `HostError::Incompatible`, and the
  browser lists the plugin but greys it out (step 08).
- **Buffers:** `StereoBus` is two planar `f32` channels. Pass them in place
  if the plugin allows processing in place. Otherwise copy them into two
  scratch buffers allocated at activation. **Never allocate in `process`.**
- **Events in:** each `Event::ParamValue { id, value }` becomes a CLAP
  `param_value` event at the same offset, using an event buffer with a fixed
  capacity that is allocated at activation. Also send CLAP's transport event
  every block, built from `ProcessContext` (`dsp/src/node.rs:41`).
- **Events out:** the plugin's own parameter changes and gestures go into a
  bounded ring from the processor to the instance. Step 07 reads it. If the
  ring is full, the event is dropped and counted, and the count is visible.
- **Sleep and tail:** map `CLAP_PROCESS_SLEEP` and the tail extension onto
  `is_at_rest` and `tail_frames` (`node.rs:162,188`), so a sleeping plugin
  is skipped the way a native device is.
- **Failure:** a `CLAP_PROCESS_ERROR` result, a non-finite sample, or a
  panic caught at the boundary sets the instance's `failed` flag. The
  processor then passes audio through from that block on. The UI shows the
  flag in step 08, and saving still works.

## The session path

The effect insert path is `on_add_effect_clicked` → session. Add a variant
that takes a `PluginRef`: resolve it through the scan cache, create and
activate the instance on the control thread, put it in the rack, and send
`InstallEffect` with the real processor. If the plugin can't be found, the
session installs step 02's placeholder and records why.

## Tests

- The test plugin in a channel, rendered **offline and in realtime** from
  the same song, produces identical output.
- Lifecycle: insert → play → change the sample rate (restart the engine) →
  play → remove. Also insert → fail → bypass → save → load.
- A 64-channel song with the test gain plugin on every channel renders with
  no allocation in the callback.

## Manual

Two stereo effects from different vendors in step 01's list (for example
one LSP and one ChowDSP) play through a channel on JACK at 64 and 1024
frames per block.

## Done when

- [ ] #27's remaining checklist.
