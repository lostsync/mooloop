# Convolution Reverb

Status: generated-room player implemented, August 2026.

The reverb is an IR player first and a room generator second. Its stable
boundary is `mooloop_dsp::StereoIr`: mono input, stereo output, finite samples
at the engine sample rate. `PreparedIr` turns that representation into fixed
512-frame FFT partitions before it reaches the audio thread.

## Realtime contract

- The player reports one 512-frame partition of latency, but declares no dry
  alignment latency: its return is intentionally late while the host dry path
  remains in time with neighbouring channels.
- Processing uses bounded overlap-save convolution and preallocated complex
  history. It does not allocate, lock, decode, or generate an IR in `process`.
- Changing a room coalesces control edits for 80 ms on a worker. The completed
  node reaches the audio thread through the existing ordered structural stream
  and swaps at a block boundary.
- Prepared-resource replacements carry a fingerprint. The engine refuses a
  result whose slot no longer owns the room state it was generated from. This
  generic structural mechanism is intended for future resource-backed devices,
  not only reverb.

## Generated rooms

The generator combines a small image-source early-reflection model with a
material-filtered deterministic diffuse tail. Shape changes reflection density;
width, depth, height, decay, and a plan-view capture point remain explicit.
The direct sound is intentionally absent from generated IRs because the host
owns dry/wet blending.

## Measured IRs

File loading is deliberately not a second DSP path. A later asset loader must
decode and resample a WAV/AIFF IR off the audio thread, make a `StereoIr`, then
send a prepared replacement through the same command. Project persistence will
reference the source asset rather than serializing PCM into TOML.
