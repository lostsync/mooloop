# 04 The preferences page names its driver

The Audio page says JACK throughout: the driver control, "No connectable JACK
inputs found", a buffer note about every JACK client on the machine, a sample
rate "set by the JACK server". Settings persist under `[audio.jack]`.

- The copy that differs by driver comes from Rust, once, for the compiled
  driver; the markup stops spelling it.
- Settings gain an `[audio.core-audio]` section of the same shape, and the UI
  reads and writes whichever section the build's driver owns.
