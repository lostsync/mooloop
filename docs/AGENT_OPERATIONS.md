# Agent Operations Runbook

Read this only when running Cargo, software-rendered UI checks, or the live
application. `AGENTS.md` has the workflow and verification rules;
`docs/OPERATIONS.md` has the ordinary build, test, release, and git
mechanics. This file is what is specific to running them from an agent on
Adam's machine.

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

Each local checkout gets its own remote directory and Cargo target directory,
keyed by absolute path, so worktrees do not fight over one cache and two of
them may build remotely at the same time. Cargo's job cap is lifted to the
remote core count.

Dependencies are shared across checkouts by sccache rather than by a shared
target directory. sccache caches individual `rustc` invocations under a hash
of their inputs, so a checkout whose sources differ gets a cache miss and a
recompile -- never another checkout's artifact. A shared `CARGO_TARGET_DIR`
was tried and reverted for exactly that reason: two checkouts of this
workspace share package names and versions, and the second linked against the
first's stale `mooloop-core`, failing on code that was correct on disk. Use
`--no-sccache` or `$MOOLOOP_NO_SCCACHE=1` to bypass the wrapper, and
`$MOOLOOP_SCCACHE_SIZE` to change the 40G cap.

Pull artifacts back with `--pull`, which is how remote UI snapshots work:

```sh
scripts/antibox --pull /tmp/window.ppm \
  env SLINT_BACKEND=winit-software MOOLOOP_PLAYLIST_SNAPSHOT=/tmp/window.ppm \
  cargo test -p mooloop-ui --test playlist_snapshot
```

`--clean` discards the remote checkout and its target cache but keeps the
sccache dependency cache; `$MOOLOOP_REMOTE_TARGET` moves the target directory
elsewhere. `--host` and
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

Sketch and check individual widgets with `scripts/slint-sketch`, which drives
`slint-viewer` over the real `crates/mooloop-ui/ui` sources and never compiles
the crate:

```sh
scripts/slint-sketch sketch.slint            # type-check, ~0.05s
scripts/slint-sketch --shot sketch.slint     # render a PNG, ~0.2s, prints its path
scripts/slint-sketch --shot - <<'SKETCH'     # or straight from stdin
import { Theme } from "theme.slint";
import { ParameterKnob } from "controls.slint";
export component Probe inherits Window {
    width: 200px; height: 140px;
    background: Theme.background;
    ParameterKnob { label: "CUTOFF"; value: 0.62; value-text: "62%"; }
}
SKETCH
```

`cargo build -p mooloop-ui` costs about four minutes whether the edit was a new
device face or a 2px nudge, because rustc recompiles the whole generated module
either way. That prices out the look-and-adjust loop visual work depends on, so
do the adjusting here and build once at the end.

Its limits are worth knowing before you trust a render. Anything driven by a
Rust model -- the piano grid, mixer strips, the device rack's contents -- draws
empty, because only the `.slint` side exists; a device face renders its chrome
and controls but not its curve. It is for spacing, colour, proportion and
typography, not for interaction or live data.

Screenshots are properly headless: the viewer installs its own software backend,
so no display, compositor or `agent` workspace is involved and it works while
the screen is locked. Sketches and their PNGs land in `$TMPDIR`, outside the
repo -- keep them there, they are working notes rather than artefacts.

### Capturing the real widgets

Sketching stops where a Rust model starts. For anything model-driven, the
`mooloop-ui` test suite already builds the real window and can be asked to
write its snapshot to disk. Every one of these follows the same shape — the
test always runs and asserts; setting an environment variable additionally
writes the PPM it rendered:

```sh
MOOLOOP_PLAYLIST_SNAPSHOT=/tmp/window.ppm \
  cargo test -p mooloop-ui --test playlist_snapshot
```

There are around fifty of these across twenty test files, one per state
somebody wanted to look at — every source face and its pages, the mixer, the
modulation shelf and each module kind, the preferences pages, the effect rack
scrolled and unscrolled, before/after shots either side of a drag. Find the
one you want rather than adding another:

```sh
rg -o 'MOOLOOP_[A-Z_]+_SNAPSHOT' crates/mooloop-ui/tests | sort -u
```

The variable name says which test file to run; `rg -l <VARIABLE>
crates/mooloop-ui/tests` gets you there. Add a new one only when no existing
state shows what you changed, and follow the surrounding convention: assert
something, and write the image as a side effect.

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

`AGENTS.md` requires `git config core.hooksPath .githooks` once per clone; the
setting is shared by that repository's linked worktrees. Verify it with
`git config --get core.hooksPath`.
