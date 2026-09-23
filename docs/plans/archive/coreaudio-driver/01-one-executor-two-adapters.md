# 01 One executor, two adapters

Split the JACK process callback into the part that is about JACK and the part
that is not.

- `executor.rs` takes everything the callback did that no host API decides:
  the ordered command stream, reclamation, MIDI decoding, rendering, metering,
  xrun events and the load meter. It renders into two slices it is handed.
- `jack_driver.rs` keeps ports, the notification handler, destination
  discovery and the startup fallback, and moves those out of `lib.rs`.
- `Engine::new` opens the driver in two halves -- learn the sample rate, build
  the render state for it, then start -- because both drivers need that order.
- The scheduling readout asks `sched_getscheduler` on Linux and the Mach
  time-constraint policy on macOS.

Linux behaviour does not change. Done when the engine checks on macOS and,
through a stand-in `pkg-config`, for the Linux target from the same machine.
