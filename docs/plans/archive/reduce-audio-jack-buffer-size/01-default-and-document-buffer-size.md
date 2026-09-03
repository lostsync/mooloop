# Default and document a sane JACK buffer size

## Problem

`AudioConfig::default()` leaves `buffer_size: None`
(`crates/mooloop-ui/src/settings.rs:146`), so mooloop never calls
`Client::set_buffer_size` and inherits whatever quantum PipeWire's JACK
shim happens to be running — on this workstation, 1024 frames @ 48 kHz,
i.e. ~21ms per period and 40-60ms round trip. That reads as sluggish
clicks and UI response with no single attributable cause, because nothing
in the engine is actually slow — the period is just huge.

The picker for this already exists in the UI
(`JACK_BUFFER_SIZES: [u32; 6] = [64, 128, 256, 512, 1024, 2048]` at
`crates/mooloop-ui/src/lib.rs:154`), so this is a defaults/config change,
not new plumbing.

## What to do

1. Change `AudioConfig::default()`'s `buffer_size` to `Some(256)` (safe
   middle ground before the other engine-side fixes in
   `docs/plans/archive/skip-empty-effect-slots/` and
   `docs/plans/archive/amortize-reverb-partition-cost/` land — 128 becomes
   reasonable once those are done).
2. Confirm `Engine::new` still degrades gracefully (falls back to the
   server's current buffer size with a printed warning) when
   `set_buffer_size` fails on hardware/servers that don't support the
   requested quantum — it already does this at
   `crates/mooloop-core/src/bridge.rs` (see the `Engine::new` block); just
   verify the new default path exercises it.
3. Update `docs/CURRENT.md` (or wherever audio defaults are documented) to
   state the new default buffer size and note that a user chasing input
   latency should check Preferences > Audio before assuming a DSP
   bottleneck.
4. Existing users with `~/.config/mooloop/settings.toml` already saved
   (no `buffer-size` key under `[audio.jack]`) keep their prior behavior
   until they open Preferences and pick a size, since the default only
   applies when no saved config overrides it — confirm this is actually
   true by reading how `AudioConfig` is merged with `UiSettings` at
   startup, and call it out explicitly in the PR description either way.

## Verification

- `cargo test -p mooloop-ui` (settings/audio snapshot tests already exist
  at `crates/mooloop-ui/tests/preferences_audio_snapshot.rs`).
- Manual: fresh `~/.config/mooloop` (or a temp `XDG_CONFIG_HOME`), launch,
  confirm JACK/PipeWire reports the new quantum (`pw-metadata -n
  settings` or `jack_bufsize`).
