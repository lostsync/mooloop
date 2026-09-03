# Agent Operations Runbook

Read this only when running Cargo, software-rendered UI checks, or the live
application. The root agent contract has the workflow and verification rules.

## Cargo limits

Never run Cargo build, test, or Clippy commands concurrently on this machine.
Memory is the constraint; `nice` does not solve that. Keep the workspace
development profile's capped debug information intact.

Run Cargo through `scripts/cargo-capped`, which puts the run in a
memory-bounded cgroup:

```sh
scripts/cargo-capped check -p mooloop-ui
scripts/cargo-capped clippy -p mooloop-ui --all-targets
scripts/cargo-capped test -p mooloop-ui -j 2
```

It costs nothing in speed -- a measured `mooloop-ui` check runs 41s either way
-- and it is the only thing that keeps a heavy run from freezing the desktop
instead of just failing. Prefix `MOOLOOP_CAP_STATS=1` to print peak memory.

Two distinct memory problems live here, and the fix for one does not help the
other:

- **Linking**, which is what makes `cargo test` expensive: seven `mooloop-ui`
  test binaries link at once. That is what `.cargo/config.toml`'s job cap,
  `mold`, and the dev debug-info cap address. Cap jobs and prefer one relevant
  test target, as above.
- **Checking**, which never links, so none of the above applies to it. A
  `mooloop-ui` check peaks at 3.4 GB after an edit and 5.2 GB cold, in a
  *single* rustc process handling the one huge module `build.rs` generates
  from `ui/main.slint`. Job count cannot subdivide one process, so lowering
  `jobs` does not help; only the cgroup bound does.

`.cargo/config.toml` limits default Cargo jobs to three. Do not raise the
limit. When several worktrees need a shared cache, use the machine-local
`CARGO_TARGET_DIR` described in the README.

Do not set `CARGO_INCREMENTAL=0` to save memory. It is the obvious guess and
it is measurably wrong here: on the same `.slint` edit it cost 4.58 GB and
2m01s, against 3.42 GB and 41s with incremental left on.

For scale on where that single module comes from: `slint_build` expands
`ui/main.slint` into roughly 39 MB and 395,000 lines of Rust, so `mooloop-ui`
compiles about 412,000 lines of which 96% are generated. Nothing here can be
tuned below that; `docs/plans/egui-view-layer/00-status.md` measures what the
figures look like without it.

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

## Driving the live application over MCP

`scripts/mooloop-mcp` runs the application with Slint's embedded MCP server
switched on, which publishes the running UI as MCP tools: `list_windows`,
`get_element_tree`, `find_elements_by_id`, `get_element_properties`,
`query_element_descendants`, `set_element_value`, `click_element`,
`drag_element`, `dispatch_key_event`, `invoke_accessibility_action`,
`take_screenshot`, and event recording.

The tools are `i-slint-backend-testing`'s `ElementHandle` API over HTTP --
the same introspection the UI tests in `crates/mooloop-ui/tests` drive
in-process, which is why `first_click.rs` explains that the search half of it
needs debug info and clicks fixed coordinates instead.

This is the only view of the interface with the real Rust models behind it.
`scripts/slint-sketch` draws widgets with nothing in them, and the snapshot
tests render one frame of one window; here the engine is running, a click
lands on the same code path Adam's click lands on, and the next screenshot
shows what it did.

```sh
scripts/mooloop-mcp              # build on the box, start headless, print the endpoint
scripts/mooloop-mcp --status
scripts/mooloop-mcp --stop
scripts/mooloop-mcp --window     # a real window on the `agent` output instead
```

It is headless by default for the reasons software rendering is preferred
above -- no compositor, works while the screen is locked -- and because a
windowed run on a machine with no display cannot screenshot at all. JACK is
not optional either way: the engine starts before the UI and takes the process
down with it if it fails, so a port that never answers usually means the log
the script names, not the server.

`.mcp.json` registers the endpoint for Claude Code, so the tools appear in a
session started while the application is up; the entry is dead the rest of the
time, which is the cost of having it checked in. Any client can call the
endpoint directly, and `curl` is the reliable way to drive it from a script:

```sh
curl -s -X POST http://127.0.0.1:9010/mcp -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call",
       "params":{"name":"list_windows","arguments":{}}}'
```

The two handle kinds are the thing to get right, since they have the same
`{index, generation}` shape and are not interchangeable. `list_windows`
returns a window handle; `get_window_properties` on it returns
`rootElementHandle`; `get_element_tree` takes that *element* handle and walks
down from it, a thousand elements at a time. The elements come back with the
`.slint` ids and accessible labels -- `MainWindow::menu-bar`,
`ToolButton::tap`, "Show the step grid or the mixer: Mixer" -- and absolute
positions that line up with the screenshot, so finding the control you mean is
a search over the tree rather than a guess at coordinates.

`click_element` is a real pointer event, and the pointer stays where it left
it: the next screenshot may show a hover tooltip the application is right to
be drawing. The engine connects to JACK on startup and usually reports one
xrun while doing so, which is the connection, not a fault in what you are
testing.

Two switches gate the server, and both are off in anything released. The
`mcp` feature on `mooloop-app` compiles it in and makes `crates/mooloop-ui`'s
`build.rs` emit element debug info, without which every tool that names an
element fails at runtime. `$SLINT_MCP_PORT` starts it; unset, the code is
inert. It binds `127.0.0.1`, validates that the request origin is local, and
has no authentication -- it is a development tool, and the packaging never
turns the feature on.

Expect the feature flip to cost a full rebuild of the generated Slint module
in either direction -- 13m05s on the box for a cold release build with it on
-- which is why the script builds there by default and keeps its binary at
`bin/mooloop-mcp` rather than in `target/`.

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
