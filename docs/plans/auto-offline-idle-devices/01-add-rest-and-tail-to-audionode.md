# Give `AudioNode` a rest/tail contract

## Problem

`crates/mooloop-dsp/src/node.rs` documents itself as "deliberately shaped
like the modern plugin APIs (VST3's `IAudioProcessor`, CLAP's `process`,
LV2's `run`)", and it faithfully mirrors them — buffer in place, sorted
sample-timed events, optional event output, `latency_frames` /
`dry_path_latency_frames`. It is missing exactly one thing those APIs all
have: a way for a node to tell the host it currently has nothing to do.

- VST3: `IAudioProcessor::getTailSamples` + `setProcessing`.
- CLAP: `CLAP_PROCESS_SLEEP` / `CLAP_PROCESS_CONTINUE_IF_NOT_QUIET`.
- `AudioNode`: nothing.

The only idle concept anywhere in the crate is `Adsr::is_idle`
(`crates/mooloop-dsp/src/env.rs:114`, and the `ExpDecay` one at `:175`) and
`Sampler`'s private `is_idle` (`crates/mooloop-dsp/src/sampler.rs:171`).
Both are used internally for voice stealing and neither is visible to the
host. So the engine has no basis on which to skip anything.

## What to do

Add two defaulted methods to the `AudioNode` trait. Defaults must preserve
today's behaviour exactly, so an unconverted node keeps processing always:

```rust
/// Frames of audible output this node can still produce after its input
/// goes silent. `u32::MAX` (the default) means "unbounded / unknown" and
/// tells the host it may never skip this node.
fn tail_frames(&self) -> u32 {
    u32::MAX
}

/// True when the node's internal state has settled such that silent input
/// would produce silent output. Cheap to call; queried per block. The
/// default is `false` so nodes that have not opted in are never skipped.
fn is_at_rest(&self) -> bool {
    false
}
```

Then implement them per node, easiest first:

1. **Stateless / memoryless effects** — `BitcrushEffect`, `DriveEffect`
   (modulo the oversampler FIR delay line), `EqEffect`, `FilterEffect`.
   `tail_frames` is small and fixed (0, or the filter's settling time /
   `OVERSAMPLER_LATENCY_FRAMES`). `is_at_rest` can start as a simple
   "have I seen non-silent input recently" check, or for the filters, a
   check that the state variables are below a threshold.
2. **Finite-tail effects** — `DelayEffect` (`effects/delay.rs`),
   `ModulationEffect` (`effects/modulation.rs`), `ReverbEffect`
   (`effects/reverb.rs`). Tail is derivable from parameters: delay time ×
   feedback decay to -60 dB, IR length for the reverb (bounded already by
   `MAX_IR_SECONDS = 2.0`, `reverb.rs:24`). Be conservative — over-report
   the tail rather than truncate it. A wrong `tail_frames` here is an
   audible chopped reverb, which is far worse than the CPU it saves.
3. **Dynamics** — `CompressorEffect` / `GateEffect` / `LimiterEffect`
   (`effects/dynamics.rs`). Tail is the envelope follower's release time
   (`dynamics.rs:62` `set_times`); at rest once the follower's envelope has
   decayed to unity gain.
4. **Instruments** — surface `Sampler::is_idle` (already exists, just
   private, `sampler.rs:171`); for `MonoSynth`/`PolySynth`/`DrumSynth`,
   all voices' `Adsr::is_idle` / `ExpDecay::is_idle` returning true. These
   already do the check internally for voice management
   (`monosynth.rs:218`, `polysynth.rs:296`, `drumsynth.rs:354`).

## Constraints

- `is_at_rest` runs on the audio thread once per node per block. It must be
  a field read or a couple of comparisons, never a buffer scan.
- Nodes that hold an internal delay/FIR line (`DriveEffect`'s
  `Oversampler2x`, `DryAlign`) are only at rest once that line has flushed —
  don't report rest the instant input goes quiet, or the flush gets dropped.
- Anything reporting `tail_frames() != u32::MAX` must be able to defend the
  number. If you're not sure, leave the default; a node that never sleeps
  is only as slow as today.

## Verification

- `cargo test -p mooloop-dsp --release` — nothing should change yet, since
  no host code reads these methods until step 02.
- Add a test per converted node asserting the pair is consistent: feed
  silence for `tail_frames()` blocks after a burst, then assert both
  `is_at_rest()` is true *and* that continuing to process produces output
  below a silence threshold. That assertion is what makes the contract
  trustworthy for step 02.
