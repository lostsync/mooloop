# 01 — Spike: `clack-host` runs a plugin, and there is a plugin to run

This step builds the new crate, then decides one question: **is
`clack-host` 0.2 enough, or do the adapters sit directly on `clap-sys`?**
Nothing outside the new crates changes.

## The crates

- `crates/mooloop-plugin-host`, a new workspace member. It depends on
  `clack-host` and `clack-extensions` (with the extensions it needs:
  `audio-ports`, `params`, `state`, `latency`, `tail`, `gui`, `log`,
  `thread-check`, `note-ports`), plus `mooloop-dsp` for `AudioNode`.
  **It must not depend on `mooloop-ui` or `mooloop-session`.**
- `crates/mooloop-test-plugin`, a new `cdylib` built with `clack-plugin`,
  marked `publish = false`. It is the test double every later step uses:
  - `mooloop.test.gain`: a stereo effect with a `gain` parameter in plain
    dB (-60..+12), a stepped `latency` parameter (0, 64, 512) that asks for
    a restart when it changes, and a `fail` parameter that makes `process`
    return an error.
  - `mooloop.test.sine`: an instrument with one note input and a stereo
    output, one sine voice per note id, and a release so the tail can be
    measured.
  - Both are built twice: once with a `gui` extension that opens a trivial
    window, and once without, so the path for plugins with no GUI is tested
    from the start.

Put the test plugin behind the workspace's normal build. An integration test
in `mooloop-plugin-host` finds it through `CARGO_TARGET_DIR` or
`env!("CARGO_CDYLIB_FILE_...")`, whichever cargo supports on the pinned
toolchain. Record which one it was.

## What the spike has to show, in a test

1. Load `mooloop.test.gain`, read its descriptor, create it, and activate it
   at 48000 Hz with blocks of 1 to 4096 frames.
2. Process a block of known audio and see the gain applied. Send a parameter
   event at offset 100 and see the change land at frame 100.
3. Save state, create a new instance, load that state, and get the same
   output.
4. Read parameter info (ids, names, ranges, the stepped flag) and turn a
   value into display text.
5. Change `latency`, then observe `request_restart`, deactivate, activate,
   and read the new latency.
6. Load `mooloop.test.sine`, send a note on and a note off, and see the
   voice start and then finish its tail.

If any of the six needs `unsafe` beyond loading the library, or `clack-host`
cannot express it, write down **exactly what was missing** in `00-status.md`
and take that part from `clap-sys` instead. Do not fork `clack`.

## Also record

- Which free plugins ship a Linux CLAP build today: Surge XT, LSP, Dexed,
  ChowDSP, free-audio `clap-plugins`, and an Airwindows build. Give the
  package name or download for each on Fedora. That list becomes the manual
  test set for steps 06–11.
- What the added dependencies cost in build time, measured on the box.

## Done when

- [ ] The six checks pass in `cargo test -p mooloop-plugin-host`, on Linux
      and in the macOS CI job.
- [ ] `00-status.md` records the decision between `clack-host` and
      `clap-sys`, and the manual plugin set.
