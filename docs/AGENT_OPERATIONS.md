# Agent Operations Runbook

Read this only when running Cargo, software-rendered UI checks, or the live
application. The root agent contract has the workflow and verification rules.

## Cargo limits

Never run Cargo build, test, or Clippy commands concurrently on this machine.
Memory, especially during linking, is the constraint; `nice` does not solve
that. Keep the workspace development profile's capped debug information intact.

For `mooloop-ui`, cap jobs and prefer one relevant test target:

```sh
cargo test -p mooloop-ui -j 2
```

`.cargo/config.toml` limits default Cargo jobs to three. Do not raise the
limit. When several worktrees need a shared cache, use the machine-local
`CARGO_TARGET_DIR` described in the README.

## Remote builds and tests

The laptop's Cargo limits exist because of its memory. `scripts/antibox` sends
the work to the build box instead, where those limits do not apply: it rsyncs
the current working tree (uncommitted edits included, gitignored paths
excluded), runs the command there, streams the output back, and exits with the
remote status. Prefer it for anything heavier than a single small crate, and
especially for `--workspace` runs and `mooloop-ui`.

```sh
scripts/antibox                             # cargo test --workspace
scripts/antibox cargo test -p mooloop-ui
scripts/antibox cargo clippy --workspace --all-targets
```

Each local checkout gets its own remote directory keyed by absolute path, but
all of them share one remote Cargo target directory, so a dependency is built
once for the box rather than once per worktree. Cargo locks that directory, so
two remote runs queue behind each other instead of colliding. Cargo's job cap
is lifted to the remote core count.

Pull artifacts back with `--pull`, which is how remote UI snapshots work:

```sh
scripts/antibox --pull /tmp/window.ppm \
  env SLINT_BACKEND=winit-software MOOLOOP_PLAYLIST_SNAPSHOT=/tmp/window.ppm \
  cargo test -p mooloop-ui --test playlist_snapshot
```

`--clean` discards the remote checkout but keeps the shared target cache;
`$MOOLOOP_REMOTE_TARGET` moves that cache elsewhere. `--host` and
`$MOOLOOP_REMOTE_HOST` point at a different ssh host. Anything needing JACK, a
real audio device, or the live compositor still belongs on this machine.

For a runnable build of the current tree, `--release-bin` compiles the
`mooloop` binary with `--release` on the box, strips it, and copies it to
`./bin/mooloop-test`:

```sh
scripts/antibox --release-bin
scripts/antibox --release-bin /tmp/mooloop-candidate   # somewhere else
```

The binary is stripped, so it has no backtrace symbols; use it for listening
and interaction checks, not for diagnosing a crash.

## Software-rendered UI checks

Prefer headless software rendering: it is deterministic, does not need a
window, and works while the screen is locked. Slint's default GPU backend does
not support `take_snapshot`.

Capture the control gallery:

```sh
SLINT_BACKEND=winit-software MOOLOOP_GALLERY_SNAPSHOT=/tmp/gallery.ppm \
  MOOLOOP_GALLERY_SIZE=1000x1800 cargo run -p mooloop-ui --example control-gallery
```

Capture the real `MainWindow` through its playlist snapshot test:

```sh
MOOLOOP_PLAYLIST_SNAPSHOT=/tmp/window.ppm \
  cargo test -p mooloop-ui --test playlist_snapshot
```

Convert an image for inspection:

```sh
magick /tmp/whatever.ppm /tmp/whatever.png
```

## Live application

Use the dedicated headless Hyprland output named `agent`, never Adam's active
workspace:

```sh
hyprctl dispatch exec '[workspace name:agent] <command>'
grim -o agent /tmp/whatever.png
hyprctl clients -j | jq '.[] | select(.workspace.name=="agent")'
hyprctl dispatch closewindow address:<addr>
```

Keep `name:agent`; a bare workspace name is misparsed. Do not add `silent`:
the headless output must switch to its workspace to composite the window. If a
mapped window yields only wallpaper, the lock screen is engaged; use software
rendering rather than debugging the compositor. Recreate a genuinely missing
output with `hyprctl output create headless agent`.

`ydotool` input goes to the focused window. Keep live interaction brief and do
not leave the agent window focused.

## Hook activation

The tracked pre-commit hook protects `main` from ordinary commits. Enable it
once per clone; the setting is shared by that repository's linked worktrees:

```sh
git config core.hooksPath .githooks
```

Verify it with:

```sh
git config --get core.hooksPath
```
