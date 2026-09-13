# Core Audio driver status

Written 2026-09-13. Adam wants to develop mooloop on a Mac as well as on
Fedora, and asked for it to build and run there. Step 01 is in progress.

## What prompted it

Mooloop is Linux-first and says so, and `ROADMAP.md` parks platform support
beyond Linux. What changed is not the product's platform but Adam's desk: a
second machine he would like to work on. That is a narrower goal than "a macOS
release", and the plan is sized to it -- build, run, hear it, change it -- with
packaging, signing and a release pipeline left out.

## What the code already had

The engine was closer than the README suggests. Every line that knew about
JACK lived in `lib.rs` and `graph.rs`; `RenderState` renders without it, and
offline export already proves that. `driver.rs` said a second driver would
extract its shape from the first mechanically, and the settings already
persisted a driver choice with one variant.

On a Mac the workspace failed to compile in exactly one place: the realtime
scheduling readout called `sched_getscheduler` under `cfg(unix)`, which macOS
does not provide.

## Decisions

- **JACK on Linux, Core Audio on macOS, chosen at compile time.** No runtime
  driver switch; each platform builds the driver it has.
- **cpal**, not raw `coreaudio-rs`: the same crate reaches ALSA and WASAPI if
  either is ever wanted, and on macOS it already reports overloads as xruns,
  follows the system default output, and sets the device sample rate.
- **The output-target shape stays a left/right pair.** On Core Audio each half
  addresses a device channel, `<device>#<channel>`, so the preferences page and
  the settings file keep one shape across both drivers.

## Steps

| Step | State |
| --- | --- |
| 01 One executor, two adapters | in progress |
| 02 Core Audio output | not started |
| 03 Core MIDI input | not started |
| 04 The preferences page names its driver | not started |
| 05 Keep the Mac build honest | not started |
