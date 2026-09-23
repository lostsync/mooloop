# 02 Core Audio output

`coreaudio_driver.rs`, compiled on macOS in place of the JACK adapter.

- One cpal output stream at the engine's sample rate, rendering in blocks of
  at most `MAX_BLOCK_SIZE` and interleaving the master bus into the route's
  two channels.
- The executor lives behind a mutex the callback only `try_lock`s, so a new
  device or buffer size reopens the stream without rebuilding the engine.
- Targets are `<device>#<channel>`, with `default` meaning the system output.
  A named device that goes away hands over to the system default; with
  auto-reconnect on, the stream returns when the device does.
- `EngineHandle::poll` drives that upkeep, because it is the control-thread
  call every caller already makes.

Done when `cargo run -p mooloop-app --bin mooloop` plays on this Mac.
