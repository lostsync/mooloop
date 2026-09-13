# Core Audio driver status

Written 2026-09-13. Adam wants to develop mooloop on a Mac as well as on
Fedora, and asked for it to build and run there. It does, as of the same day:
steps 01, 02, 04 and 05 have landed and step 03 is open.

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
| 01 One executor, two adapters | landed 2026-09-13 |
| 02 Core Audio output | landed 2026-09-13 |
| 03 Core MIDI input | not started |
| 04 The preferences page names its driver | landed 2026-09-13 |
| 05 Keep the Mac build honest | landed 2026-09-13; the macOS CI job has not run yet |

## What the doing changed

- **The Linux build can be checked from the Mac.** The build box does not
  resolve from every network the Mac sits on, and the JACK adapter no longer
  compiles there, so `scripts/linux-check` cross-checks for x86_64 Linux: a
  stand-in `pkg-config` for jack-sys, `zig cc` for mp3lame-sys's autoconf.
  Check and clippy both run through it.
- **The first launch of a fresh dev build sat for over ten seconds** before
  the engine reported starting; the second started at once. Not reproduced
  since, and most likely macOS's first-run scan of a new binary rather than
  anything in the driver, but worth knowing before calling it a hang.
- **A dev build reported a few xruns a second at idle**, in a run that was also
  being stack-sampled. Judge Core Audio dropouts in a release build before
  treating that as a driver fault.
