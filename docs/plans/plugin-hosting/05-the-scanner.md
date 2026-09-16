# 05 — The scanner, out of process (#27)

Adam's answer 4: from the first version, **a plugin that crashes during
scanning must not crash mooloop.**

## The child

`crates/mooloop-app/src/main.rs` runs straight into `run()` today. Before
logging and the engine start, add a check: if `argv[1] == "--scan-plugin"`,
call `mooloop_plugin_host::scan::child_main(path)`. That function loads the
file, lists every plugin factory entry (one `.clap` file can hold hundreds,
as Airwindows builds do), and writes one JSON document to stdout with each
plugin's `PluginRef`, features (instrument, effect, and so on), audio and
note ports, and whether it declares a GUI. The child starts no audio, opens
no window and uses no settings. The child parses its arguments by hand, so
no dependency is added for that.

## The parent

`mooloop_plugin_host::scan::Scanner` runs on a background thread
(`std::thread::spawn`, following the existing one-off threads in
`ui/src/lib.rs`):

- Search paths: `~/.clap`, `/usr/lib/clap`, `/usr/lib64/clap` (Fedora),
  `/usr/local/lib/clap`, then `CLAP_PATH`, then
  `UiSettings.plugins.extra_paths`. On Flatpak, also check
  `/app/extensions/Plugins/clap`. On macOS, use the two `Library/Audio/Plug-Ins/CLAP`
  folders. A `.clap` on macOS is a bundle; on Linux it is a file.
- Each candidate is launched with `std::env::current_exe()`, with a
  10-second timeout (a setting). A child that is killed, crashes, or exits
  non-zero is recorded as `Failed { reason, mtime }`.
- The cache is `<config>/plugins.toml` (`settings.rs:1118` has `config_dir`).
  Entries are keyed by canonical path, with mtime and size. A file whose
  mtime and size haven't changed isn't scanned again, and neither is a
  failure. Preferences gets a "rescan all" button (step 08) that clears
  the failures.
- Progress and results go back through `slint::invoke_from_event_loop`.

## Finding a saved plugin

`resolve(&PluginRef) -> Option<PathBuf>` looks up the cache by format and id.
If the cache has more than one version, the newest one wins, and the
difference is logged.

## Settings

Add `UiSettings.plugins: PluginSettings { extra_paths, scan_timeout_s,
scan_on_startup }` with `serde(default)` (`settings.rs:985`). The
preferences page itself is part of step 08.

## Tests

- A directory containing the test plugin, a zero-byte `.clap`, and a
  `.clap` that is actually a shell script which sleeps. The result: one good
  entry, one failed entry, and one timed-out entry, all cached. A second
  scan launches no children.
- `child_main` on the test plugin produces JSON that parses back into the
  expected `PluginRef`s.

## Done when

- [ ] #27: "scan results persist/cache without rescanning on every
      startup" and "distinguish scan failure, incompatible plugin, load
      failure."
- [ ] A scan of Adam's real plugin folders completes, and the cache is read
      back and checked by eye.
